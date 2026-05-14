//! Bind-group layouts + cached pipeline ids for the Phase-A render passes
//! (`03-design.md` §2.6, §5).
//!
//! [`NaadfPipelines`] is the standard Bevy "pipeline resource": a `FromWorld`
//! resource created once in `RenderStartup`, holding the three stable
//! bind-group-layout descriptors, the cached compute-pipeline id, and a
//! per-target-format cache of the final-blit render pipeline.
//!
//! Two stable bind-group layouts, shared across the Phase-A passes
//! (`03-design.md` §2.6):
//!
//! - `@group(0)` — world data (read-only in render passes): `chunks`
//!   (`texture_3d<u32>`), `blocks` / `voxels` / `voxel_types` (read-only
//!   storage), `world_meta` (uniform).
//! - `@group(1)` — frame data: `camera` + `render_params` uniforms,
//!   `first_hit_data` + `taa_sample_accum` read-write storage.
//!
//! The first-hit compute pass binds `@group(0)` + `@group(1)`. The final-blit
//! fullscreen pass binds its own small layout (`first_hit_data` +
//! `taa_sample_accum` + `render_params`) — it does not need the world buffers
//! (`03-design.md` §5.4).
//!
//! ## Per-format blit pipeline
//!
//! The final-blit fragment pipeline's colour-target format must match the view
//! target's main-texture format, which Bevy chooses per-camera (`Rgba16Float`
//! for an HDR-precision `Core3d` view target, `Rgba8UnormSrgb` for a plain
//! SDR one). So the blit pipeline is *not* a single cached id — it is queued
//! lazily per `TextureFormat` by [`prepare_blit_pipeline`] reading the view's
//! `ExtractedView::target_format`, and cached in
//! [`NaadfPipelines::blit_pipelines`]. This is the lightweight form of the
//! `FullscreenMaterial` specialiser pattern.

use std::borrow::Cow;
use std::num::NonZeroU64;

use bevy::core_pipeline::FullscreenShader;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{
    binding_types::{
        storage_buffer_read_only_sized, storage_buffer_sized, texture_3d, uniform_buffer_sized,
    },
    BindGroup, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
    CachedComputePipelineId, CachedRenderPipelineId, ColorTargetState, ColorWrites,
    ComputePipelineDescriptor, FragmentState, PipelineCache, RenderPipelineDescriptor,
    ShaderStages, TextureFormat, TextureSampleType, VertexState,
};
use bevy::render::renderer::RenderDevice;
use bevy::render::view::ExtractedView;
use bevy::shader::Shader;

use crate::render::gpu_types::{
    GpuAtmosphereParams, GpuCamera, GpuGiParams, GpuRenderParams, GpuTaaParams,
    GpuWorldMeta,
};

/// Asset paths of the Phase-A entry-point WGSL shaders + the Phase-A-2 TAA
/// reproject shader.
pub const FIRST_HIT_SHADER: &str = "shaders/naadf_first_hit.wgsl";
pub const FINAL_BLIT_SHADER: &str = "shaders/naadf_final.wgsl";
/// Asset path of the Phase-A-2 TAA reproject compute shader (`06-design-a2.md`
/// §8.4) — port of `albedo/renderTaaSampleReverse.fx`.
pub const TAA_REPROJECT_SHADER: &str = "shaders/taa.wgsl";
/// Asset path of the Phase-B atmosphere precompute compute shader
/// (`09-design-b.md` §5.1) — port of `base/renderAtmosphere.fx`.
pub const ATMOSPHERE_SHADER: &str = "shaders/naadf_atmosphere.wgsl";
/// Asset path of the Phase-B `rayQueueCalc` compute shader (`09-design-b.md`
/// §5.1, §7) — port of `base/rayQueueCalc.fx`.
pub const RAY_QUEUE_SHADER: &str = "shaders/ray_queue_calc.wgsl";
/// Asset path of the Phase-B `renderGlobalIllum` compute shader
/// (`09-design-b.md` §5.1, §8.1) — port of `base/renderGlobalIllum.fx`.
pub const GLOBAL_ILLUM_SHADER: &str = "shaders/naadf_global_illum.wgsl";
/// Asset path of the Phase-B `renderSampleRefine` compute shader
/// (`09-design-b.md` §5.1, §5.7, §8.2) — the 5-pass sample-refine stage, port
/// of `base/renderSampleRefine.fx`. One file, five entry points.
pub const SAMPLE_REFINE_SHADER: &str = "shaders/sample_refine.wgsl";

/// Compute-shader workgroup size — `[numthreads(64,1,1)]` in the HLSL
/// `albedo/renderFirstHit.fx` `calcFirstHit` (`03-design.md` §5.1). The Phase-B
/// atmosphere / GI passes also use a 64-wide group.
pub const FIRST_HIT_WORKGROUP_SIZE: u32 = 64;

/// The Phase-A render pipelines + bind-group layouts (`03-design.md` §5).
#[derive(Resource)]
pub struct NaadfPipelines {
    /// `@group(0)` — world data: `chunks`, `blocks`, `voxels`, `voxel_types`,
    /// `world_meta`.
    pub world_layout: BindGroupLayoutDescriptor,
    /// `@group(1)` — frame data: `camera`, `render_params`, `first_hit_data`,
    /// `taa_sample_accum`.
    pub frame_layout: BindGroupLayoutDescriptor,
    /// The final-blit pass's own small layout: `first_hit_data`,
    /// `taa_sample_accum`, `render_params`.
    pub blit_layout: BindGroupLayoutDescriptor,
    /// The `taa_samples` 16-ring write layout — one read-write storage binding
    /// (`06-design-a2.md` §5.2). `TaaGpu` builds its `taa_first_hit_bind_group`
    /// field against this. Phase B Batch 2 moves the `taa_samples` ring write
    /// OFF the first-hit pass (the `base/` first-hit does not write it —
    /// `09-design-b.md` §6.3); the layout stays so `TaaGpu`'s bind-group field
    /// keeps compiling — Batch 6 re-homes it onto the `calc_new_taa_sample`
    /// pipeline.
    pub taa_layout: BindGroupLayoutDescriptor,
    /// `@group(3)` for the Phase-B 4-plane first-hit pass — the precomputed
    /// atmosphere (`09-design-b.md` §4.4 / §6.3): `atmosphere_params` (uniform),
    /// `atmosphere_comp` (read-only storage). Distinct from `atmosphere_layout`
    /// (the precompute pass's `@group(0)`, which has `atmosphere_comp` as
    /// *read-write* storage) — the first-hit only reads the buffer.
    pub atmosphere_read_layout: BindGroupLayoutDescriptor,
    /// The TAA reproject pass's single bind group layout (`06-design-a2.md`
    /// §5.3): `taa_params` (uniform), `camera_history` / `first_hit_data` /
    /// `taa_samples` (read-only storage), `taa_sample_accum` (read-write
    /// storage). The reproject pass does not traverse the voxel world, so it
    /// binds no `@group(0)` world data — this is its only group.
    pub taa_reproject_layout: BindGroupLayoutDescriptor,
    /// `@group(0)` for the Phase-B atmosphere precompute pass: `atmosphere_params`
    /// (uniform), `atmosphere_comp` (read-write storage) — `09-design-b.md`
    /// §4.3. The precompute pass writes one quarter of `atmosphere_comp` per
    /// frame; it binds no `@group(0)` world data.
    pub atmosphere_layout: BindGroupLayoutDescriptor,
    /// An entry-less bind-group layout — wgpu pipeline layouts are a `Vec`
    /// indexed by group number, so a shader that uses `@group(0)` + `@group(1)`
    /// + `@group(3)` (skipping `@group(2)`) needs a placeholder at index 2.
    /// `naadf_global_illum.wgsl` does exactly that (`09-design-b.md` §8.1 binds
    /// world `@group(0)`, GI `@group(1)`, atmosphere `@group(3)`).
    pub empty_layout: BindGroupLayoutDescriptor,
    /// `@group(0)` for the Phase-B `rayQueueCalc` passes (`09-design-b.md`
    /// §4.5): `gi_params` (uniform), `first_hit_data` (read-only storage),
    /// `ray_queue` (read-write storage), `ray_queue_indirect` (read-write
    /// storage), `taa_sample_accum` (read-only storage). Shared by both the
    /// `RayQueue` and `RayQueueStore` passes.
    pub ray_queue_layout: BindGroupLayoutDescriptor,
    /// `@group(1)` for the Phase-B `renderGlobalIllum` pass (`09-design-b.md`
    /// §8.1): `gi_params` (uniform), `first_hit_data` (read-only),
    /// `first_hit_absorption` / `valid_samples` / `invalid_samples` /
    /// `sample_counts` / `final_color` (read-write), `ray_queue` (read-only),
    /// `camera_history` (read-only). `globalIllum` also binds `@group(0)` world
    /// + `@group(3)` atmosphere.
    pub global_illum_layout: BindGroupLayoutDescriptor,
    /// `@group(0)` for the Phase-B `renderSampleRefine` passes (`09-design-b.md`
    /// §8.2). One shared layout bound by all 5 sample-refine entry points
    /// (`WorldRenderBase.cs` re-binds the same effect 5×). 11 bindings:
    /// `gi_params` (uniform), `first_hit_data` (read), `bucket_info` (rw),
    /// `valid_samples` (read), `valid_samples_refined` (rw),
    /// `valid_samples_compressed` (rw), `invalid_samples` (read),
    /// `sample_counts` (rw), `taa_dist_min_max` (read), `ray_queue_indirect`
    /// (rw), `camera_history` (read). The sample-refine passes do not traverse
    /// the voxel world. (`valid_dispatch` / `invalid_dispatch` are NOT here —
    /// see `sample_refine_dispatch_layout`.)
    pub sample_refine_layout: BindGroupLayoutDescriptor,
    /// `@group(1)` for `renderSampleRefine`'s `compute_valid_history` pass ONLY
    /// (`09-design-b.md` §8.2 — the wgpu-forced split). `valid_dispatch` +
    /// `invalid_dispatch` (both rw storage): `compute_valid_history` writes
    /// them, then `count_valid_data_and_refine` / `count_invalid_data` consume
    /// them as `dispatch_workgroups_indirect` sources. wgpu forbids a buffer
    /// being bound `STORAGE_READ_WRITE` AND used as `INDIRECT` in one dispatch,
    /// so they cannot sit in the shared `@group(0)` (the count passes bind that
    /// group). Only the `valid_history` pipeline's layout includes this group.
    pub sample_refine_dispatch_layout: BindGroupLayoutDescriptor,
    /// Cached id of the `naadf_first_hit` compute pipeline.
    pub first_hit_pipeline: CachedComputePipelineId,
    /// Cached id of the `taa.wgsl` `reproject_old_samples` compute pipeline
    /// (`06-design-a2.md` §8.4).
    pub taa_reproject_pipeline: CachedComputePipelineId,
    /// Cached id of the `naadf_atmosphere.wgsl` `precompute_atmosphere` compute
    /// pipeline (`09-design-b.md` §4.3 / §5.1).
    pub atmosphere_pipeline: CachedComputePipelineId,
    /// Cached id of the `ray_queue_calc.wgsl` `calc_ray_queue` compute pipeline
    /// (`09-design-b.md` §4.5 / §7) — the adaptive ray-queue builder.
    pub ray_queue_pipeline: CachedComputePipelineId,
    /// Cached id of the `ray_queue_calc.wgsl` `calc_ray_queue_store` compute
    /// pipeline (`[numthreads(1,1,1)]`) — converts the raw queued-pixel count
    /// into the indirect workgroup count for `globalIllum`.
    pub ray_queue_store_pipeline: CachedComputePipelineId,
    /// Cached id of the `naadf_global_illum.wgsl` `calc_global_ilum` compute
    /// pipeline (`09-design-b.md` §4.6 / §8.1) — the ≤3-bounce GI tracer,
    /// dispatched indirect off `ray_queue_indirect`.
    pub global_illum_pipeline: CachedComputePipelineId,
    /// Cached id of `sample_refine.wgsl`'s `clear_buckets_and_calc_mask`
    /// (`09-design-b.md` §4.7 / §8.2) — the per-frame ring/queue reset + the
    /// per-bucket normal-mask + min/max-distance scan.
    pub sample_refine_clear_pipeline: CachedComputePipelineId,
    /// Cached id of `sample_refine.wgsl`'s `compute_valid_history`
    /// (`[numthreads(1,1,1)]`) — the 128-frame ring walk + the indirect-arg
    /// write for the two count passes.
    pub sample_refine_valid_history_pipeline: CachedComputePipelineId,
    /// Cached id of `sample_refine.wgsl`'s `count_valid_data_and_refine` —
    /// dispatched INDIRECT off `valid_dispatch`; reprojects lit samples into the
    /// 8×8 bucket grid and writes `valid_samples_refined`.
    pub sample_refine_count_valid_pipeline: CachedComputePipelineId,
    /// Cached id of `sample_refine.wgsl`'s `count_invalid_data` — dispatched
    /// INDIRECT off `invalid_dispatch`; reprojects unlit samples, `atomicAdd`s
    /// the bucket's invalid count.
    pub sample_refine_count_invalid_pipeline: CachedComputePipelineId,
    /// Cached id of `sample_refine.wgsl`'s `refine_buckets` — the
    /// `COLOR_DIF_PROB` brightness-leveling, writes `valid_samples_compressed`.
    pub sample_refine_buckets_pipeline: CachedComputePipelineId,
    /// An entry-less bind group for `empty_layout` — `naadf_global_illum_node`
    /// must `set_bind_group(2, ...)` since the `globalIllum` pipeline layout
    /// has a placeholder at index 2 (`09-design-b.md` §8.1 — the shader skips
    /// `@group(2)`). Created once in `from_world`.
    pub empty_bind_group: BindGroup,
    /// Per-`TextureFormat` cache of the `naadf_final` fullscreen render
    /// pipeline (see the module doc — the colour-target format is per-view).
    pub blit_pipelines: HashMap<TextureFormat, CachedRenderPipelineId>,
    /// Fullscreen-triangle vertex state, captured at init for re-queuing the
    /// blit pipeline per format.
    blit_vertex: VertexState,
    /// Strong handle to the `naadf_final` fragment shader, kept so re-queuing
    /// the blit pipeline per format does not re-load it.
    blit_shader: Handle<Shader>,
}

impl FromWorld for NaadfPipelines {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>().clone();
        let asset_server = world.resource::<AssetServer>().clone();
        let fullscreen_shader = world.resource::<FullscreenShader>().clone();
        let pipeline_cache = world.resource::<PipelineCache>();

        // Minimum binding sizes for the uniform buffers. The `#[repr(C)]` GPU
        // structs are not `ShaderType`, so the sized helpers are used directly
        // with the Rust struct size (which the compile-time asserts in
        // `gpu_types` keep in step with the WGSL declarations).
        let camera_size = NonZeroU64::new(std::mem::size_of::<GpuCamera>() as u64).unwrap();
        let params_size =
            NonZeroU64::new(std::mem::size_of::<GpuRenderParams>() as u64).unwrap();
        let world_meta_size =
            NonZeroU64::new(std::mem::size_of::<GpuWorldMeta>() as u64).unwrap();
        let taa_params_size =
            NonZeroU64::new(std::mem::size_of::<GpuTaaParams>() as u64).unwrap();
        let atmosphere_params_size =
            NonZeroU64::new(std::mem::size_of::<GpuAtmosphereParams>() as u64).unwrap();

        // --- @group(0): world data ------------------------------------------
        // chunks: texture_3d<u32>; blocks / voxels / voxel_types: runtime-sized
        // read-only storage arrays; world_meta: uniform.
        let world_layout = BindGroupLayoutDescriptor::new(
            "naadf_world_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    texture_3d(TextureSampleType::Uint),
                    // blocks / voxels / voxel_types are `var<storage, read>` in
                    // `world_data.wgsl` — read-only in every render pass.
                    storage_buffer_read_only_sized(false, None), // blocks: array<u32>
                    storage_buffer_read_only_sized(false, None), // voxels: array<u32>
                    storage_buffer_read_only_sized(false, None), // voxel_types: array<vec4<u32>>
                    uniform_buffer_sized(false, Some(world_meta_size)),
                ),
            ),
        );

        // --- @group(1): frame data ------------------------------------------
        // camera + render_params uniforms; first_hit_data + taa_sample_accum +
        // first_hit_absorption + final_color read-write storage arrays.
        // Phase B Batch 2 widens this by 2 bindings: the `base/` first-hit
        // writes `firstHitData` + `firstHitAbsorption` + `finalColor`
        // (`base/renderFirstHit.fx:6-8`, `09-design-b.md` §3.4 / §6.3).
        // `taa_sample_accum` stays bound at slot 3 for layout stability — the
        // `base/` first-hit no longer writes it (`ReprojectOld` +
        // `CalcNewTaaSample` do — Batch 6), the shader touches it so naga keeps
        // the binding.
        let frame_layout = BindGroupLayoutDescriptor::new(
            "naadf_frame_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, Some(camera_size)),
                    uniform_buffer_sized(false, Some(params_size)),
                    storage_buffer_sized(false, None), // first_hit_data: array<vec4<u32>>, rw
                    storage_buffer_sized(false, None), // taa_sample_accum: array<vec2<u32>>, rw
                    storage_buffer_sized(false, None), // first_hit_absorption: array<vec2<u32>>, rw
                    storage_buffer_sized(false, None), // final_color: array<vec2<u32>>, rw
                ),
            ),
        );

        // --- final-blit layout (fullscreen fragment pass) -------------------
        // first_hit_data (read), taa_sample_accum (read), render_params (uniform).
        let blit_layout = BindGroupLayoutDescriptor::new(
            "naadf_blit_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    // The blit pass only reads these — `var<storage, read>` in
                    // `naadf_final.wgsl`.
                    storage_buffer_read_only_sized(false, None), // first_hit_data: array<vec4<u32>>
                    storage_buffer_read_only_sized(false, None), // taa_sample_accum: array<vec2<u32>>
                    uniform_buffer_sized(false, Some(params_size)),
                ),
            ),
        );

        // --- @group(2): the first-hit pass's TAA-sample-ring write ----------
        // One read-write storage binding — `taa_samples: array<vec2<u32>>`
        // (`06-design-a2.md` §5.2). The first-hit pipeline's layout below is
        // extended to bind this group (Batch 2 step 6); `naadf_first_hit.wgsl`
        // writes one ring slot when `FLAG_IS_TAA` is set.
        let taa_layout = BindGroupLayoutDescriptor::new(
            "naadf_taa_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_sized(false, None), // taa_samples: array<vec2<u32>>, rw
                ),
            ),
        );

        // --- the TAA reproject pass's bind group layout ---------------------
        // `taa_params` uniform; `camera_history` / `first_hit_data` /
        // `taa_samples` read-only storage; `taa_sample_accum` read-write
        // storage (`06-design-a2.md` §5.3). The reproject pass binds no
        // `@group(0)` world data — this is its single group.
        let taa_reproject_layout = BindGroupLayoutDescriptor::new(
            "naadf_taa_reproject_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, Some(taa_params_size)),
                    storage_buffer_read_only_sized(false, None), // camera_history
                    storage_buffer_read_only_sized(false, None), // first_hit_data
                    storage_buffer_read_only_sized(false, None), // taa_samples
                    storage_buffer_sized(false, None),           // taa_sample_accum, rw
                ),
            ),
        );

        // --- @group(0): the Phase-B atmosphere precompute pass --------------
        // `atmosphere_params` uniform; `atmosphere_comp` read-write storage
        // (`09-design-b.md` §4.3). The precompute pass binds no `@group(0)`
        // world data — this is its single group.
        let atmosphere_layout = BindGroupLayoutDescriptor::new(
            "naadf_atmosphere_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, Some(atmosphere_params_size)),
                    storage_buffer_sized(false, None), // atmosphere_comp: array<vec4<u32>>, rw
                ),
            ),
        );

        // --- @group(3): the 4-plane first-hit's read-only atmosphere --------
        // `atmosphere_params` uniform; `atmosphere_comp` *read-only* storage
        // (`09-design-b.md` §4.4 / §6.3). The first-hit only samples the
        // precomputed buffer — `applyAtmosphere` (miss) +
        // `addLightForDirection` (the atmosphere-interaction path).
        let atmosphere_read_layout = BindGroupLayoutDescriptor::new(
            "naadf_atmosphere_read_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, Some(atmosphere_params_size)),
                    storage_buffer_read_only_sized(false, None), // atmosphere_comp: array<vec4<u32>>
                ),
            ),
        );

        let gi_params_size =
            NonZeroU64::new(std::mem::size_of::<GpuGiParams>() as u64).unwrap();

        // --- the entry-less placeholder layout ------------------------------
        // wgpu pipeline layouts are a `Vec` indexed by group number;
        // `naadf_global_illum.wgsl` uses `@group(0)` + `@group(1)` + `@group(3)`
        // (skipping `@group(2)` — `09-design-b.md` §8.1), so index 2 needs a
        // placeholder with zero bindings — a `BindGroupLayoutDescriptor` over an
        // empty entry slice.
        let empty_layout =
            BindGroupLayoutDescriptor::new("naadf_empty_bind_group_layout", &[]);

        // --- @group(0): the Phase-B `rayQueueCalc` bind group ---------------
        // `gi_params` uniform; `first_hit_data` read-only storage; `ray_queue` +
        // `ray_queue_indirect` read-write storage; `taa_sample_accum` read-only
        // storage (`09-design-b.md` §4.5). Shared by `RayQueue` + `RayQueueStore`.
        let ray_queue_layout = BindGroupLayoutDescriptor::new(
            "naadf_ray_queue_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, Some(gi_params_size)),
                    storage_buffer_read_only_sized(false, None), // first_hit_data
                    storage_buffer_sized(false, None),           // ray_queue, rw
                    storage_buffer_sized(false, None),           // ray_queue_indirect, rw
                    storage_buffer_read_only_sized(false, None), // taa_sample_accum
                ),
            ),
        );

        // --- @group(1): the Phase-B `renderGlobalIllum` bind group ----------
        // `gi_params` uniform; `first_hit_data` / `ray_queue` / `camera_history`
        // read-only storage; `first_hit_absorption` / `valid_samples` /
        // `invalid_samples` / `sample_counts` / `final_color` read-write storage
        // (`09-design-b.md` §8.1). `globalIllum` also binds `@group(0)` world +
        // `@group(3)` atmosphere.
        let global_illum_layout = BindGroupLayoutDescriptor::new(
            "naadf_global_illum_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, Some(gi_params_size)),
                    storage_buffer_read_only_sized(false, None), // first_hit_data
                    storage_buffer_sized(false, None),           // first_hit_absorption, rw
                    storage_buffer_sized(false, None),           // valid_samples, rw
                    storage_buffer_sized(false, None),           // invalid_samples, rw
                    storage_buffer_sized(false, None),           // sample_counts, rw
                    storage_buffer_sized(false, None),           // final_color, rw
                    storage_buffer_read_only_sized(false, None), // ray_queue
                    storage_buffer_read_only_sized(false, None), // camera_history
                ),
            ),
        );

        // --- @group(0): the Phase-B `renderSampleRefine` bind group ---------
        // One shared layout for all 5 sample-refine passes (`09-design-b.md`
        // §8.2). 11 bindings — within wgpu's `maxBindingsPerBindGroup` default.
        // `bucket_info` / `sample_counts` / `ray_queue_indirect` are bound
        // `storage_buffer_sized` (rw): the WGSL declares `bucket_info` as
        // `array<BucketInfoSlot>` (with an `atomic<u32>` member),
        // `sample_counts` / `ray_queue_indirect` as plain arrays — all need
        // read-write access. `valid_dispatch` / `invalid_dispatch` are NOT in
        // this group — they go in `sample_refine_dispatch_layout` (the wgpu
        // indirect-vs-storage-exclusivity split).
        let sample_refine_layout = BindGroupLayoutDescriptor::new(
            "naadf_sample_refine_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, Some(gi_params_size)),
                    storage_buffer_read_only_sized(false, None), // first_hit_data
                    storage_buffer_sized(false, None),           // bucket_info, rw (atomic)
                    storage_buffer_read_only_sized(false, None), // valid_samples
                    storage_buffer_sized(false, None),           // valid_samples_refined, rw
                    storage_buffer_sized(false, None),           // valid_samples_compressed, rw
                    storage_buffer_read_only_sized(false, None), // invalid_samples
                    storage_buffer_sized(false, None),           // sample_counts, rw
                    storage_buffer_read_only_sized(false, None), // taa_dist_min_max
                    storage_buffer_sized(false, None),           // ray_queue_indirect, rw
                    storage_buffer_read_only_sized(false, None), // camera_history
                ),
            ),
        );

        // --- @group(1): `compute_valid_history`'s indirect-arg buffers ------
        // `valid_dispatch` + `invalid_dispatch` (both rw storage). Only the
        // `valid_history` pipeline binds this group; the count passes get the
        // buffers as `dispatch_workgroups_indirect` sources (no shader binding),
        // so there is no `STORAGE_READ_WRITE` ⨉ `INDIRECT` usage conflict.
        let sample_refine_dispatch_layout = BindGroupLayoutDescriptor::new(
            "naadf_sample_refine_dispatch_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_sized(false, None), // valid_dispatch, rw
                    storage_buffer_sized(false, None), // invalid_dispatch, rw
                ),
            ),
        );

        // --- compute pipeline (single, format-agnostic) ---------------------
        let first_hit_shader = asset_server.load(FIRST_HIT_SHADER);
        let first_hit_pipeline =
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("naadf_first_hit_pipeline".into()),
                // Phase B Batch 2: the layout is `[world, frame, atmosphere]`
                // (`09-design-b.md` §6.3). The `@group(2)` `taa_samples` ring
                // is GONE — the `base/` first-hit no longer writes it; it
                // re-homes onto the `calc_new_taa_sample` pipeline (Batch 6).
                // `@group(3)` is the read-only precomputed atmosphere
                // (`applyAtmosphere` on a miss + `addLightForDirection` along
                // the atmosphere-interaction path).
                layout: vec![
                    world_layout.clone(),
                    frame_layout.clone(),
                    atmosphere_read_layout.clone(),
                ],
                shader: first_hit_shader,
                entry_point: Some(Cow::from("calc_first_hit")),
                ..default()
            });

        // --- the TAA reproject compute pipeline (single, format-agnostic) ---
        let taa_reproject_shader = asset_server.load(TAA_REPROJECT_SHADER);
        let taa_reproject_pipeline =
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("naadf_taa_reproject_pipeline".into()),
                layout: vec![taa_reproject_layout.clone()],
                shader: taa_reproject_shader,
                entry_point: Some(Cow::from("reproject_old_samples")),
                ..default()
            });

        // --- the Phase-B atmosphere precompute pipeline ---------------------
        let atmosphere_shader = asset_server.load(ATMOSPHERE_SHADER);
        let atmosphere_pipeline =
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("naadf_atmosphere_pipeline".into()),
                layout: vec![atmosphere_layout.clone()],
                shader: atmosphere_shader,
                entry_point: Some(Cow::from("precompute_atmosphere")),
                ..default()
            });

        // --- the Phase-B `rayQueueCalc` pipelines (Batch 3) -----------------
        // Both passes share the `ray_queue_layout` `@group(0)` — `RayQueue` is
        // `[numthreads(64,1,1)]`, `RayQueueStore` is `[numthreads(1,1,1)]`
        // (`09-design-b.md` §4.5 / §7). One shader file, two entry points.
        let ray_queue_shader = asset_server.load(RAY_QUEUE_SHADER);
        let ray_queue_pipeline =
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("naadf_ray_queue_pipeline".into()),
                layout: vec![ray_queue_layout.clone()],
                shader: ray_queue_shader.clone(),
                entry_point: Some(Cow::from("calc_ray_queue")),
                ..default()
            });
        let ray_queue_store_pipeline =
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("naadf_ray_queue_store_pipeline".into()),
                layout: vec![ray_queue_layout.clone()],
                shader: ray_queue_shader,
                entry_point: Some(Cow::from("calc_ray_queue_store")),
                ..default()
            });

        // --- the Phase-B `renderGlobalIllum` pipeline (Batch 3) -------------
        // Layout `[world, global_illum, empty, atmosphere_read]` — the shader
        // uses `@group(0)` world + `@group(1)` GI + `@group(3)` atmosphere
        // (`09-design-b.md` §8.1); index 2 is the entry-less placeholder.
        // Dispatched INDIRECT off `ray_queue_indirect` (`WorldRenderBase.cs:323`).
        let global_illum_shader = asset_server.load(GLOBAL_ILLUM_SHADER);
        let global_illum_pipeline =
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("naadf_global_illum_pipeline".into()),
                layout: vec![
                    world_layout.clone(),
                    global_illum_layout.clone(),
                    empty_layout.clone(),
                    atmosphere_read_layout.clone(),
                ],
                shader: global_illum_shader,
                entry_point: Some(Cow::from("calc_global_ilum")),
                ..default()
            });

        // --- the Phase-B `renderSampleRefine` pipelines (Batch 4) -----------
        // One shader file, five entry points (`09-design-b.md` §8.2); all five
        // bind the single `sample_refine_layout` `@group(0)`. `clear` /
        // `count_valid` / `count_invalid` / `buckets` are `[numthreads(64,1,1)]`,
        // `valid_history` is `[numthreads(1,1,1)]`. `count_valid` / `count_invalid`
        // are dispatched INDIRECT (`WorldRenderBase.cs:356,359`).
        let sample_refine_shader = asset_server.load(SAMPLE_REFINE_SHADER);
        // The 4 passes that bind ONLY `@group(0)` (`clear` / `count_valid` /
        // `count_invalid` / `buckets`).
        let mk_sample_refine = |label: &'static str, entry: &'static str| {
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some(label.into()),
                layout: vec![sample_refine_layout.clone()],
                shader: sample_refine_shader.clone(),
                entry_point: Some(Cow::from(entry)),
                ..default()
            })
        };
        let sample_refine_clear_pipeline = mk_sample_refine(
            "naadf_sample_refine_clear_pipeline",
            "clear_buckets_and_calc_mask",
        );
        // `compute_valid_history` additionally binds `@group(1)` (the
        // indirect-arg buffers it writes) — its layout vec has both groups.
        let sample_refine_valid_history_pipeline =
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("naadf_sample_refine_valid_history_pipeline".into()),
                layout: vec![
                    sample_refine_layout.clone(),
                    sample_refine_dispatch_layout.clone(),
                ],
                shader: sample_refine_shader.clone(),
                entry_point: Some(Cow::from("compute_valid_history")),
                ..default()
            });
        let sample_refine_count_valid_pipeline = mk_sample_refine(
            "naadf_sample_refine_count_valid_pipeline",
            "count_valid_data_and_refine",
        );
        let sample_refine_count_invalid_pipeline = mk_sample_refine(
            "naadf_sample_refine_count_invalid_pipeline",
            "count_invalid_data",
        );
        let sample_refine_buckets_pipeline = mk_sample_refine(
            "naadf_sample_refine_buckets_pipeline",
            "refine_buckets",
        );

        // The blit pipeline is queued lazily per target format — see
        // `prepare_blit_pipeline`. Capture the vertex state + fragment shader
        // handle so re-queuing is cheap.
        let blit_vertex = fullscreen_shader.to_vertex_state();
        let blit_shader = asset_server.load(FINAL_BLIT_SHADER);

        // The entry-less bind group for `empty_layout` — `globalIllum`'s
        // pipeline layout has a placeholder at `@group(2)`, so the node must
        // bind *something* there. Created once here.
        let empty_bind_group = render_device.create_bind_group(
            "naadf_empty_bind_group",
            &pipeline_cache.get_bind_group_layout(&empty_layout),
            &[],
        );

        NaadfPipelines {
            world_layout,
            frame_layout,
            blit_layout,
            taa_layout,
            taa_reproject_layout,
            atmosphere_layout,
            atmosphere_read_layout,
            empty_layout,
            ray_queue_layout,
            global_illum_layout,
            sample_refine_layout,
            sample_refine_dispatch_layout,
            first_hit_pipeline,
            taa_reproject_pipeline,
            atmosphere_pipeline,
            ray_queue_pipeline,
            ray_queue_store_pipeline,
            global_illum_pipeline,
            sample_refine_clear_pipeline,
            sample_refine_valid_history_pipeline,
            sample_refine_count_valid_pipeline,
            sample_refine_count_invalid_pipeline,
            sample_refine_buckets_pipeline,
            empty_bind_group,
            blit_pipelines: HashMap::default(),
            blit_vertex,
            blit_shader,
        }
    }
}

impl NaadfPipelines {
    /// Queue (if not already cached) the `naadf_final` fullscreen render
    /// pipeline for `format`, returning its cached id.
    fn blit_pipeline_for(
        &mut self,
        format: TextureFormat,
        pipeline_cache: &PipelineCache,
    ) -> CachedRenderPipelineId {
        if let Some(id) = self.blit_pipelines.get(&format) {
            return *id;
        }
        let id = pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("naadf_final_blit_pipeline".into()),
            layout: vec![self.blit_layout.clone()],
            vertex: self.blit_vertex.clone(),
            fragment: Some(FragmentState {
                shader: self.blit_shader.clone(),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        });
        self.blit_pipelines.insert(format, id);
        id
    }
}

/// `RenderSystems::Prepare` system: ensure the final-blit pipeline for the
/// current view's target format is queued.
///
/// Reads `ExtractedView::target_format` (Bevy chooses the view target's
/// main-texture format per-camera) and queues the matching `naadf_final`
/// pipeline variant if it is not yet cached.
pub fn prepare_blit_pipeline(
    mut pipelines: ResMut<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    views: Query<&ExtractedView, With<ExtractedCamera>>,
) {
    for view in &views {
        let _ = pipelines.blit_pipeline_for(view.target_format, &pipeline_cache);
    }
}
