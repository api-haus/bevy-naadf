//! Phase-C construction sub-module — the **empty extension seam** that every
//! Phase-C workstream (W1..W5) extends.
//!
//! Owns the render-world resources, the `Render`-schedule prepare system, the
//! `Startup`-schedule one-shot driver, and the Bevy `Plugin` that wires them.
//! W0 ships the skeleton; W1..W5 each add their own pipelines / buffers /
//! shaders behind the same seam (`15-design-c.md` §1.1, §1.2, §1.4, §3).
//!
//! ## What W0 lands
//!
//! 1. [`ConstructionGpu`] — the render-world `Resource` holding every
//!    Phase-C buffer family. **Every field is `Option<Buffer>` / `Option<…>`
//!    initialised to `None`** so each workstream owns the allocation of its
//!    family (W1: hash-map + segment voxel; W2: change family; W3: bound-queue
//!    family; W4: entity family). W0 inserts the empty resource shell only.
//! 2. [`ConstructionBindGroups`] — the parallel render-world `Resource`
//!    holding the construction-mode bind groups. Every field is
//!    `Option<BindGroup>` / `None`; workstreams allocate when their pipelines
//!    land. W0 inserts the empty shell only.
//! 3. [`ConstructionPipelines`] — the empty sibling of `NaadfPipelines`
//!    (`render/pipelines.rs`). W0 leaves it field-less; W1..W5 add their
//!    pipeline ID + layout fields. **`NaadfPipelines` is intentionally not
//!    edited** — the construction pipelines live in their own resource so
//!    every Phase-C workstream touches the same sibling instead of fighting
//!    over the shared `NaadfPipelines` struct.
//! 4. [`prepare_construction`] — the empty `Render`-schedule system in the
//!    `PrepareResources` set. W0's body just `init_resource`-s the two
//!    construction resources if they are missing; W1..W5 fill it with their
//!    allocate/resize/build-bind-group logic. Mirrors the
//!    `prepare_world_gpu` / `prepare_taa` pattern.
//! 5. [`run_gpu_construction_startup`] — the empty `Startup`-schedule
//!    one-shot driver. W0's body is a single `info!` placeholder when the
//!    config flag is on; W1 fills it with regime-1 dispatches
//!    (generator → chunk_calc → bounds_init) + the bit-exact CPU/GPU oracle.
//! 6. [`ConstructionPlugin`] — the wiring `Plugin`. **W0 does NOT insert
//!    construction nodes into the `Core3d` chain**; the three placeholder
//!    nodes (`naadf_bounds_compute_node` / `naadf_world_change_node` /
//!    `naadf_entity_update_node`) are left as commented TODOs in
//!    `render/mod.rs` — each workstream merges its node in its own PR.
//!
//! ## What W0 explicitly does NOT do
//!
//! - **No bind-group layouts.** Layouts are added per-workstream when their
//!   shaders need them (§1.3 — every workstream owns layouts only for its
//!   own bindings).
//! - **No buffer allocations.** Each workstream allocates only its own
//!   family. The `Option<>`-wrapped fields keep the resource a valid `None`
//!   shell until a workstream lands.
//! - **No WGSL shaders.** Every shader is greenfield in its workstream
//!   (W1: `chunk_calc.wgsl` + `map_copy.wgsl`; W2: `world_change.wgsl`;
//!   W3: `bounds_calc.wgsl`; W4: `entity_update.wgsl`; W5:
//!   `generator_model.wgsl`).
//! - **No edits to `NaadfPipelines`.** All construction pipelines live on
//!   `ConstructionPipelines`. The seam contract bars touching the shared
//!   `NaadfPipelines` from Phase-C workstreams (`15-design-c.md` §1.3).
//!
//! See `15-design-c.md` §1, §2.1 W0 row, and §3 for the full seam contract.

pub mod config;
pub mod generator_model;

use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupLayoutDescriptor, Buffer, CachedComputePipelineId, PipelineCache,
};
use bevy::render::{GpuResourceAppExt, Render, RenderApp, RenderSystems};

pub use config::ConstructionConfig;

/// The render-world `Resource` holding every Phase-C buffer family
/// (`15-design-c.md` §1.4).
///
/// **W0 — empty shell.** Every buffer field is `Option<Buffer>` initialised
/// to `None`. Each workstream populates its own family:
///
/// - **W1** (Algorithm 1): `segment_voxel_buffer`, `block_voxel_count`,
///   `hash_map`, `hash_coefficients`.
/// - **W3** (background AADF queue): `bound_queue_info`, `bound_group_queues`,
///   `bound_group_masks`, `bound_refined_info`, `bound_dispatch_indirect`.
/// - **W2** (editing): `changed_groups_dynamic`, `changed_chunks_dynamic`,
///   `changed_blocks_dynamic`, `changed_voxels_dynamic`.
/// - **W4** (entities, only when `ConstructionConfig.entities_enabled`):
///   `entity_chunk_instances`, `entity_voxel_data`, `entity_instances_history`,
///   `chunk_updates_dynamic`, `entity_chunk_instances_dynamic`,
///   `entity_history_dynamic`.
///
/// W0 declares the field set with explicit `None` initialisers so each
/// workstream lands a `Some(GrowableBuffer<T>)` / `Some(Buffer)` swap rather
/// than a struct-shape change. Later workstreams that introduce
/// `GrowableBuffer<T>` typing wrap their fields in
/// `Option<crate::world::buffer::GrowableBuffer<T>>`; W0 keeps every field as
/// `Option<Buffer>` so the empty shell compiles without pulling the family
/// types in.
#[derive(Resource, Default)]
pub struct ConstructionGpu {
    // === W1 — Algorithm 1 inputs / outputs (`chunkCalc.fx` family) ===========
    /// `segmentVoxelBuffer` (`chunkCalc.fx:38`, `WorldData.cs:73`) —
    /// `segmentSizeInChunks^3 * 2048` u32s. W1 owns the allocation.
    pub segment_voxel_buffer: Option<Buffer>,
    /// `blockVoxelCount` (`chunkCalc.fx:37`) — 2 × u32 atomic cursors. W1.
    pub block_voxel_count: Option<Buffer>,
    /// `hashMap` (`chunkCalc.fx:39`, `mapCopy.fx:13`) — open-addressing slot
    /// array, doubled by `mapCopy.fx` (`BlockHashingHandler` /
    /// `ConstructionConfig.wanted_empty_ratio`). W1 owns allocation + growth.
    pub hash_map: Option<Buffer>,
    /// `hashCoefficients` (`BlockHashingHandler.cs:50-55`) — 65 × u32 fixed
    /// table of `31^(64-i)` values. Never grows. W1.
    pub hash_coefficients: Option<Buffer>,

    // === W3 — Bound-queue family (`boundsCalc.fx` family) ===================
    /// `boundQueueInfo` (`WorldBoundHandler.cs:44`) — 32*3 × BoundQueueInfo.
    /// Fixed-size. W3.
    pub bound_queue_info: Option<Buffer>,
    /// `boundGroupQueues` (`WorldBoundHandler.cs:46`) — `32*3*boundGroupCount`
    /// u32s. Fixed-size for the test grid. W3.
    pub bound_group_queues: Option<Buffer>,
    /// `boundGroupMasks` (`WorldBoundHandler.cs:47`) — `boundGroupCount` ×
    /// 3 × `atomic<u32>` per-axis masks. W3.
    pub bound_group_masks: Option<Buffer>,
    /// `boundRefinedInfo` (`WorldBoundHandler.cs:45`) — 3 × u32. W3.
    pub bound_refined_info: Option<Buffer>,
    /// `boundDispatchIndirect` (`WorldBoundHandler.cs:49`) — 5 × u32 INDIRECT
    /// args. The wgpu `STORAGE_READ_WRITE` × `INDIRECT` split lives on this
    /// buffer (`15-design-c.md` §1.3, mirrors the Phase-B Batch-4
    /// `sample_refine_dispatch_layout` fix). W3.
    pub bound_dispatch_indirect: Option<Buffer>,

    // === W2 — Change-staging family (`worldChange.fx` family) ===============
    /// `changedGroups` (`ChangeHandler.cs:56`) — `Uint2[]` per edited 4³
    /// group. W2 owns allocation + per-frame upload.
    pub changed_groups_dynamic: Option<Buffer>,
    /// `changedChunks` (`ChangeHandler.cs:57`). W2.
    pub changed_chunks_dynamic: Option<Buffer>,
    /// `changedBlocks` (`ChangeHandler.cs:58`). W2.
    pub changed_blocks_dynamic: Option<Buffer>,
    /// `changedVoxels` (`ChangeHandler.cs:59`). W2.
    pub changed_voxels_dynamic: Option<Buffer>,

    // === W4 — Entity family (`entityUpdate.fx` family) ======================
    /// `entityChunkInstances` (`EntityHandler.cs:148`). W4.
    pub entity_chunk_instances: Option<Buffer>,
    /// `entityVoxelData` (`EntityHandler.cs:147`). W4.
    pub entity_voxel_data: Option<Buffer>,
    /// `entityInstancesHistory` (`EntityHandler.cs:149`). W4.
    pub entity_instances_history: Option<Buffer>,
    /// `chunkUpdatesDynamic` (`entityUpdate.fx:3`). W4.
    pub chunk_updates_dynamic: Option<Buffer>,
    /// `entityChunkInstancesDynamic`. W4.
    pub entity_chunk_instances_dynamic: Option<Buffer>,
    /// `entityHistoryDynamic`. W4.
    pub entity_history_dynamic: Option<Buffer>,
}

/// The render-world `Resource` holding every Phase-C construction-side bind
/// group (`15-design-c.md` §1.4).
///
/// **W0 — empty shell.** Every field is `Option<BindGroup>` / `None`. Each
/// workstream builds its own bind groups in its own merge of
/// [`prepare_construction`].
#[derive(Resource, Default)]
pub struct ConstructionBindGroups {
    /// `construction_world` — the parallel-to-`world_layout` bind group for
    /// the construction passes (chunkCalc / mapCopy / boundsCalc /
    /// worldChange). W1 builds this when the world buffers exist.
    pub construction_world: Option<BindGroup>,
    /// `construction_bounds` — the `@group(1)` bound-queue bind group used by
    /// `boundsCalc`. W3.
    pub construction_bounds: Option<BindGroup>,
    /// `construction_change` — the `@group(1)` change-staging bind group used
    /// by `worldChange`. W2.
    pub construction_change: Option<BindGroup>,
    /// `construction_entity` — the `@group(1)` entity-track bind group used
    /// by `entityUpdate`. W4.
    pub construction_entity: Option<BindGroup>,
    /// `bound_dispatch` — the one-binding `STORAGE_READ_WRITE` layout for
    /// `bound_dispatch_indirect`'s write side, separated from the consuming
    /// indirect dispatch per the wgpu `STORAGE_READ_WRITE` × `INDIRECT` split
    /// (`15-design-c.md` §1.3). W3.
    pub bound_dispatch: Option<BindGroup>,
}

/// The sibling of `NaadfPipelines` (`render/pipelines.rs`) for Phase-C
/// construction-side pipelines + layouts (`15-design-c.md` §1.3).
///
/// W0 landed this as an empty struct with a `Default`-derived `FromWorld`.
/// **W5 adds the first real fields** — `generator_model_pipeline` +
/// `generator_model_layout` — and flips the resource to a real `FromWorld`
/// impl that queues the W5 pipeline against the W5 layout.
///
/// **Field set planned per `15-design-c.md` §1.3:**
/// - W5 (**landed**): `generator_model_pipeline`, `generator_model_layout`.
/// - W1: `chunk_calc_pipeline_*`, `map_copy_pipeline`, plus layouts
///   `construction_world_layout`.
/// - W3: `bounds_calc_pipeline_*`, plus layouts `construction_bounds_layout`,
///   `bound_dispatch_indirect_layout`.
/// - W2: `world_change_pipeline_*`, plus layout `construction_change_layout`.
/// - W4: `entity_update_pipeline_*`, plus layout `construction_entity_layout`.
///
/// The `FromWorld` impl is the SINGLE seam each later workstream extends:
/// add a field, add the corresponding layout build + pipeline queue in
/// `from_world`, register the resulting handle in the struct literal at the
/// bottom. The clone-cost of `BindGroupLayoutDescriptor` keeps the seam
/// trivial for parallel-merge — every workstream's field is an additive edit.
#[derive(Resource)]
pub struct ConstructionPipelines {
    /// W5 — `@group(0)` layout for `generator_model.wgsl`. Bind-group construction
    /// happens in the W5 unit test (or in W1's regime-1 driver once W1 lands).
    pub generator_model_layout: BindGroupLayoutDescriptor,
    /// W5 — cached compute pipeline ID for `generator_model.wgsl`'s
    /// `fill_chunk_data_with_model_data_16` entry point.
    pub generator_model_pipeline: CachedComputePipelineId,
}

impl FromWorld for ConstructionPipelines {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>().clone();
        let pipeline_cache = world.resource::<PipelineCache>();

        // === W5 — generator_model pipeline + layout ==========================
        let generator_model_layout =
            generator_model::generator_model_layout_descriptor();
        let generator_model_pipeline = generator_model::queue_generator_model_pipeline(
            &asset_server,
            pipeline_cache,
            generator_model_layout.clone(),
        );

        Self {
            generator_model_layout,
            generator_model_pipeline,
        }
    }
}

/// `RenderSystems::PrepareResources` system — the empty Phase-C prepare seam.
///
/// **W0 body — ensure-exists only.** W0's responsibility is to guarantee the
/// two construction resources are present in the render world; W1..W5 fill
/// the body with their allocate/resize/build-bind-group logic.
///
/// Inserts `ConstructionGpu::default()` and `ConstructionBindGroups::default()`
/// when missing; if both exist (the steady state from frame 2 onward), the
/// system returns immediately. The `Render` schedule re-runs every frame, so
/// keep the body cheap.
///
/// Runs in `PrepareResources` alongside `prepare_world_gpu` / `prepare_taa` /
/// `prepare_atmosphere` / `prepare_gi`. No ordering constraint vs. those in
/// W0 (the empty body cannot conflict); W1..W5 add `.before(...)` /
/// `.after(...)` as their bind groups gain real `WorldGpu` /
/// `ConstructionGpu` dependencies.
pub fn prepare_construction(
    mut commands: Commands,
    gpu: Option<Res<ConstructionGpu>>,
    bind_groups: Option<Res<ConstructionBindGroups>>,
) {
    // W0 — empty seam. Just guarantee the two resources exist.
    //
    // W1..W5 will fill this with their workstream's allocate/upload/build
    // logic. The pattern matches `prepare_world_gpu` (`render/prepare.rs:128`)
    // — `Option<Res<…>>` for the resource, `Commands::insert_resource` on
    // first creation, no-op when already present.
    if gpu.is_none() {
        commands.insert_resource(ConstructionGpu::default());
    }
    if bind_groups.is_none() {
        commands.insert_resource(ConstructionBindGroups::default());
    }
}

/// `Startup`-schedule one-shot driver — the empty Phase-C regime-1 seam
/// (`15-design-c.md` §1.2 regime 1, §3 startup-schedule).
///
/// **W0 body — gated no-op.** When
/// `ConstructionConfig.gpu_construction_enabled` is `false` (W0 default), the
/// system returns immediately and the CPU `aadf::construct::construct` path
/// stays the producer (E4). When `true`, W0 logs a placeholder line at
/// `info!` so a future invocation that flips the flag without W1's payload
/// landed still produces visible diagnostics rather than silent dispatch
/// nothingness. W1 replaces the body with the regime-1 dispatch chain
/// (generator → chunk_calc → bounds_init) + the bit-exact CPU/GPU oracle
/// assert.
///
/// Lives in the main `App` (not the render sub-app) because regime-1 owns
/// its own command-encoder submission against `RenderDevice` (the same
/// `RenderQueue::submit` pattern `prepare_world_gpu` uses today —
/// `render/prepare.rs:168-180`). Runs **once**.
pub fn run_gpu_construction_startup(args: Res<crate::AppArgs>) {
    // ConstructionConfig is owned by `AppArgs` on the main world; the render
    // sub-app gets it via the `From<&AppArgs>` lift at plugin-build time.
    // Here in the main-world Startup system we read directly off `AppArgs`.
    if !args.construction_config.gpu_construction_enabled {
        // W0 default: CPU path is the producer; nothing to do.
        return;
    }
    // W1 fills this body. Until W1 lands, the flag is a tracer — useful for
    // surfacing a misconfigured run (flag flipped on, but no W1 payload yet).
    info!(
        "phase-c W0 seam — gpu construction startup placeholder (no-op until W1 lands)"
    );
}

/// The Phase-C `Plugin` — wires the empty seam into the `App` and the
/// `RenderApp` (`15-design-c.md` §3).
///
/// W0's wiring is intentionally minimal:
/// - main app: registers [`run_gpu_construction_startup`] in `Startup`.
/// - render sub-app: registers [`ConstructionPipelines`] as a GPU resource
///   (built `FromWorld` in `RenderStartup`, same pattern `NaadfPipelines`
///   uses — `render/mod.rs:94`), and registers [`prepare_construction`] in
///   `RenderSystems::PrepareResources`.
///
/// W0 does **not** insert any construction nodes into the `Core3d` chain —
/// the `render/mod.rs` chain has commented TODO markers showing where W2 /
/// W3 / W4 each land their node. The empty seam stays out of the chain so
/// W0's render-graph topology is byte-identical to pre-W0.
///
/// The construction-config resource is mirrored from main-world `AppArgs`
/// into the render sub-app the same way `TaaRingConfig` is mirrored
/// (`render/mod.rs:73-86`).
pub struct ConstructionPlugin;

impl Plugin for ConstructionPlugin {
    fn build(&self, app: &mut App) {
        // Read the main-world `AppArgs.construction_config` once at
        // plugin-build time and mirror it into the render sub-app, same
        // pattern as `TaaRingConfig` (`render/mod.rs:73-86`).
        let construction_config = app
            .world()
            .get_resource::<crate::AppArgs>()
            .map(ConstructionConfig::from)
            .unwrap_or_default();

        // Main-world `Startup` driver (regime-1, `15-design-c.md` §1.2). W0
        // body is the gated no-op above; W1 fills it.
        app.add_systems(Startup, run_gpu_construction_startup);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            // Mirror the main-world construction config into the render sub-app.
            .insert_resource(construction_config)
            // Empty pipeline registry — W1..W5 add pipeline fields + a
            // proper `FromWorld` impl as they land.
            .init_gpu_resource::<ConstructionPipelines>()
            // Empty prepare seam — `init_resource`-only body.
            .add_systems(
                Render,
                prepare_construction.in_set(RenderSystems::PrepareResources),
            );
    }
}

#[cfg(test)]
mod tests {
    //! W5 — the load-bearing bit-exact `GPU vs CPU` oracle test
    //! (`15-design-c.md` §1.6, §2.1 W5 row, §4.5).
    //!
    //! Builds a small fixed `ModelData`, runs BOTH the GPU pipeline and the
    //! `crate::aadf::generator::generate_segment_cpu` oracle against the same
    //! inputs, maps the GPU `segment_voxel_buffer` back to the CPU, and
    //! asserts byte-for-byte equality with the oracle's output.
    //!
    //! The test uses the same headless `App + RenderPlugin` fixture pattern
    //! `world::buffer::tests` uses (`world/buffer.rs:227-264`): build a
    //! minimal `App` with `RenderPlugin`, `finish()` + `cleanup()` to make
    //! `RenderDevice`/`RenderQueue` available, then drive the W5 dispatch
    //! by hand. No render schedule runs — that would require a full plugin
    //! set + a window.
    //!
    //! Skips with a warning when no wgpu adapter is available (CI box without
    //! a GPU). The CPU-oracle determinism + Y-clamp + OOB tests in
    //! `crate::aadf::generator::tests` exercise the oracle independently of
    //! the GPU path.
    use super::generator_model::{
        create_params_uniform, create_storage_buffer_u32, dispatch_generator_model,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        GpuGeneratorModelParams, CHUNK_DATA_U32S, GENERATOR_MODEL_SHADER,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::aadf::generator::{generate_segment_cpu, ModelData};

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets, Handle};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    /// Build a headless render world (same plumbing as
    /// `world::buffer::tests::render_device_queue`). Returns the `App`, the
    /// device, the queue, and an inline-built `Handle<Shader>` for the W5
    /// generator-model WGSL.
    ///
    /// The test does NOT drive the standard `ExtractSchedule` (which would
    /// require all the `bevy_render::render_asset` Message types to be
    /// initialised — `MinimalPlugins` + `AssetPlugin` only initialise some
    /// of them). Instead, it populates the cache's shader registry directly
    /// via the `pub fn PipelineCache::set_shader` entry point, which is
    /// exactly what `extract_shaders` does internally.
    fn render_fixture() -> Option<(App, RenderDevice, RenderQueue, Handle<Shader>)> {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .add_plugins(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::default()),
                synchronous_pipeline_compilation: true,
                debug_flags: Default::default(),
            });
        app.finish();
        app.cleanup();

        // Build a stable handle out of an inserted shader in the main world's
        // `Assets<Shader>` — the handle's `AssetId` is what the pipeline
        // cache keys on.
        let shader = Shader::from_wgsl(
            GENERATOR_MODEL_SHADER_SRC,
            "shaders/generator_model.wgsl",
        );
        let shader_clone = shader.clone();
        let shader_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(shader);

        // Inject the shader directly into the pipeline cache (mirrors what
        // `extract_shaders` does at the end of `ExtractSchedule`).
        let render_app = app.get_sub_app_mut(RenderApp)?;
        {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.set_shader(shader_handle.id(), shader_clone);
        }

        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((app, device, queue, shader_handle))
    }

    /// Read back the first `count` `u32`s of `src`.
    fn readback_u32(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &bevy::render::render_resource::Buffer,
        count: u64,
    ) -> Vec<u32> {
        let size = count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("naadf_generator_readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("naadf_generator_readback_encoder"),
        });
        encoder.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        out
    }

    /// W5's load-bearing test: GPU `generator_model.wgsl` output is
    /// **byte-for-byte equal** to `generate_segment_cpu` (the §1.6 oracle).
    ///
    /// Test setup: a 2×1×2 chunk model, uniform-full of type 0x42, queried
    /// over a 2×1×2 chunk segment. The model's voxel-level data path is
    /// exercised by the construct-and-run paths below; the Y-clamp does not
    /// fire (segment matches model height); the OOB short-circuit does not
    /// fire (segment matches world `sizeInVoxels`).
    ///
    /// Buffer size: 2×1×2 chunks × 2048 u32s = 8192 u32s = 32 KiB.
    #[test]
    fn generator_model_gpu_vs_cpu_bit_exact() {
        let Some((mut app, device, queue, shader_handle)) = render_fixture() else {
            eprintln!("no wgpu device — skipping W5 GPU/CPU bit-exact test");
            return;
        };

        // Fixed test inputs.
        let model = ModelData::uniform_full([2, 1, 2], 0x42);
        let group_offset_in_chunks = [0u32, 0, 0];
        let group_size_in_chunks = [2u32, 1, 2];
        let size_in_voxels = [32u32, 16, 32];

        // === CPU oracle =====================================================
        let cpu_out = generate_segment_cpu(
            &model,
            group_offset_in_chunks,
            group_size_in_chunks,
            size_in_voxels,
        );
        let total_chunks =
            (group_size_in_chunks[0] * group_size_in_chunks[1] * group_size_in_chunks[2])
                as u64;
        let chunk_data_u32s = total_chunks * CHUNK_DATA_U32S as u64;
        assert_eq!(cpu_out.len() as u64, chunk_data_u32s);

        // === GPU path =======================================================
        // Queue the pipeline + build the layout. We deliberately do NOT use
        // `app.init_gpu_resource::<ConstructionPipelines>()` here because that
        // pulls in the full `FromWorld` impl + the AssetServer load; for the
        // W5 isolated test, we go straight to the helpers + the inline shader.
        let layout = generator_model_layout_descriptor();
        let pipeline_id = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let pipeline_cache = render_app.world().resource::<PipelineCache>();
            queue_generator_model_pipeline_with_handle(
                pipeline_cache,
                layout.clone(),
                shader_handle.clone(),
            )
        };

        // Allocate the GPU buffers + uniform.
        let chunk_data_init = vec![0xDEAD_BEEFu32; chunk_data_u32s as usize];
        let gpu_chunk_data = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_segment_voxel_buffer_test",
            &chunk_data_init,
        );
        let gpu_model_chunk = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_chunk_test",
            &model.data_chunk,
        );
        let gpu_model_block = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_block_test",
            &model.data_block,
        );
        let gpu_model_voxel = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_voxel_test",
            &model.data_voxel,
        );
        let params = GpuGeneratorModelParams {
            size_in_voxels,
            _pad0: 0,
            model_size_in_chunks: model.size_in_chunks,
            _pad1: 0,
            group_offset_in_chunks,
            group_size_in_chunks_x: group_size_in_chunks[0],
            group_size_in_chunks_y: group_size_in_chunks[1],
            _pad2: 0,
            _pad3: 0,
            _pad4: 0,
        };
        let gpu_params = create_params_uniform(&device, &queue, &params);

        // Drive the pipeline to compile, then dispatch. The pipeline cache's
        // background compile is gated by `synchronous_pipeline_compilation`
        // (set true above), so `App::update()` once should be enough to fully
        // resolve the pipeline.
        // Drive pipeline compilation manually — `PipelineCache::process_queue`
        // is `pub` and `synchronous_pipeline_compilation = true` on the
        // RenderPlugin above forces compile-on-call. One pass is usually
        // enough; cap at 64 defensively.
        let pipeline = {
            let render_app = app.get_sub_app_mut(RenderApp).unwrap();
            let mut got = None;
            for _ in 0..64 {
                let mut pipeline_cache =
                    render_app.world_mut().resource_mut::<PipelineCache>();
                pipeline_cache.process_queue();
                if let Some(p) = pipeline_cache.get_compute_pipeline(pipeline_id) {
                    got = Some(p.clone());
                    break;
                }
            }
            got.expect("W5 generator_model pipeline did not compile in 64 ticks")
        };

        // Build the bind group against the resolved layout.
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let pipeline_cache = render_app.world().resource::<PipelineCache>();
        let bind_group_layout = pipeline_cache.get_bind_group_layout(&layout);
        let bind_group = device.create_bind_group(
            "naadf_generator_model_bind_group_test",
            &bind_group_layout,
            &BindGroupEntries::sequential((
                gpu_chunk_data.as_entire_buffer_binding(),
                gpu_model_chunk.as_entire_buffer_binding(),
                gpu_model_block.as_entire_buffer_binding(),
                gpu_model_voxel.as_entire_buffer_binding(),
                gpu_params.as_entire_buffer_binding(),
            )),
        );

        dispatch_generator_model(
            &device,
            &queue,
            &pipeline,
            &bind_group,
            group_size_in_chunks,
        );

        // Map the GPU buffer back + compare byte-for-byte to the oracle.
        let gpu_out = readback_u32(&device, &queue, &gpu_chunk_data, chunk_data_u32s);
        assert_eq!(gpu_out.len(), cpu_out.len());

        // Find the first divergence (if any) for a useful failure message.
        if gpu_out != cpu_out {
            for (i, (&g, &c)) in gpu_out.iter().zip(cpu_out.iter()).enumerate() {
                if g != c {
                    panic!(
                        "W5 GPU vs CPU bit-exact divergence @ u32[{i}]: gpu={g:#010x} cpu={c:#010x}"
                    );
                }
            }
        }
        assert_eq!(gpu_out, cpu_out, "W5 GPU output must equal CPU oracle byte-for-byte");

        // Sanity: the test exercised a real workload, not an all-zeros buffer.
        // For the uniform-full model + matching segment, every u32 should pack
        // two type-0x42 voxels with the full flag set.
        let expected_voxel = 0x42u32 | (1u32 << 15);
        let expected_packed = expected_voxel | (expected_voxel << 16);
        assert_eq!(
            gpu_out[0], expected_packed,
            "uniform-full model should pack 0x{:04x}_{:04x} into every voxel pair",
            expected_voxel, expected_voxel
        );
        // And the buffer is not the placeholder 0xDEADBEEF init pattern.
        assert!(gpu_out.iter().all(|&u| u != 0xDEAD_BEEF));
        // Reference `GENERATOR_MODEL_SHADER` so the constant stays load-bearing
        // (a future rename of the asset path must trip the test compile).
        let _ = GENERATOR_MODEL_SHADER;
    }
}
