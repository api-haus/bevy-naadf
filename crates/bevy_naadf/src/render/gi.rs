//! Phase B GI subsystem — the compressed-ReSTIR GI buffers, the per-frame GI
//! uniform, and the per-frame counter helpers (`09-design-b.md` §2.1, §3.7,
//! §3.8, §10.3).
//!
//! NAADF's `WorldRenderBase` GI pipeline works through a large set of
//! `StructuredBuffer`s (`WorldRenderBase.cs:104-171`): the adaptive ray queue,
//! the lit/unlit GI sample rings, the 8×8-bucket refine buffers, the 128-frame
//! sample-count accumulation ring, the indirect-dispatch arg buffers, and the
//! denoiser scratch. Phase B mirrors all of them on the [`GiGpu`] render-world
//! resource. Batch 3 lands them all (created/resized/seeded once) — `prepare_gi`
//! creates them and uploads the per-frame [`GpuGiParams`] uniform; the GI
//! passes that consume them arrive in Batches 3-5.
//!
//! This module owns:
//! - [`GiGpu`] — the render-world resource: every §3.7 GI buffer + the
//!   per-frame GI uniform + the resize-trigger geometry.
//! - [`prepare_gi`] — the `PrepareResources` system that creates/resizes/seeds
//!   the buffers and uploads `gi_params`.
//! - [`accum_index_of`] / [`rand_salts_of`] / [`bucket_grid_of`] — the
//!   per-frame counter / RNG-salt / bucket-grid helpers (`09-design-b.md`
//!   §10.3, all derived from the A-2 frame counter).
//!
//! The GI bind groups that *mix* `GiGpu` with `FrameGpu` / `TaaGpu` are NOT
//! built here — they go in [`GiBindGroups`], built by `prepare_frame_gpu` after
//! all three resources exist (`09-design-b.md` §10.3 — the same ordering rule
//! A-2 used for `taa_reproject_bind_group`).

use bevy::math::UVec2;
use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use crate::render::atmosphere::Atmosphere;
use crate::render::extract::{
    ExtractedCameraData, ExtractedCameraHistory, ExtractedGiConfig,
};
use crate::render::gpu_types::{
    GpuGiParams, GpuSampleValid, GI_FLAG_IS_ATMOSPHERE_INTERACTION, GI_FLAG_IS_DENOISE,
    GI_FLAG_IS_SAMPLE_LEVELING, GI_FLAG_IS_VARYING_RADIUS, GI_FLAG_SKIP_SAMPLES,
};

/// `globalIlumSampleCounts` element count — `128 + 3` (`WorldRenderBase.cs:165`):
/// the 128-frame `(validCount, invalidCount)` ring + 3 header slots (`[0]` write
/// cursors, `[1]` total counts, `[2]` coprime shuffle seeds). Fixed-size.
pub const SAMPLE_COUNTS_LEN: u32 = 128 + 3;

/// The lit-sample ring depth multiplier (C# `globalIllumValidSampleStorageCount`
/// — `WorldRenderBase.cs:161`). `valid_samples` is `pixel_count * this`.
pub const VALID_SAMPLE_STORAGE_COUNT: u32 = 2;
/// The unlit-sample ring depth multiplier (C# `globalIllumInvalidSampleStorageCount`).
/// `invalid_samples` is `pixel_count * this`.
pub const INVALID_SAMPLE_STORAGE_COUNT: u32 = 8;
/// Per-bucket refined-sample capacity (C# `globalIllumBucketStorageCount`).
/// `valid_samples_refined` is `bucket_count * this`.
pub const BUCKET_STORAGE_COUNT: u32 = 32;
/// Per-bucket compressed-sample capacity (C# `globalIllumRefinedBucketStorageCount`).
/// `valid_samples_compressed` is `bucket_count * this`.
pub const REFINED_BUCKET_STORAGE_COUNT: u32 = 8;

/// `GpuGiParams`-buffer usage. `STORAGE | COPY_DST` for the GI working buffers;
/// the indirect-dispatch buffers additionally get `INDIRECT`.
const GI_BUFFER_USAGES: BufferUsages = BufferUsages::STORAGE.union(BufferUsages::COPY_DST);
/// The three indirect-dispatch buffers (`ray_queue_indirect`, `valid_dispatch`,
/// `invalid_dispatch`) need `INDIRECT` on top of `STORAGE | COPY_DST` so they
/// can be both written by a compute pass and consumed by
/// `dispatch_workgroups_indirect`.
const GI_INDIRECT_USAGES: BufferUsages = BufferUsages::STORAGE
    .union(BufferUsages::COPY_DST)
    .union(BufferUsages::INDIRECT);

/// `globalIlumAccumIndex = globalIllumMaxAccum - (frameCount % globalIllumMaxAccum) - 1`
/// (`WorldRenderBase.cs:181`). The 128-frame `sample_counts` ring slot the GI
/// passes write/read this frame — the single source of truth for the formula.
pub fn accum_index_of(frame_count: u32, max_accum: u32) -> u32 {
    max_accum - (frame_count % max_accum) - 1
}

/// The two per-frame RNG salts (`09-design-b.md` §10.3). NAADF refills a
/// `randValues[32]` table per frame and indexes it 7× (`WorldRenderBase` reads
/// `randValues[randCounter++]` for `globalIllum` / `sampleRefine`); the A-2
/// simplification uses the monotonic frame counter as the salt. Phase B extends
/// it: the load-bearing property is *two distinct per-frame-varying salts*, so
/// `(frame_count, frame_count ^ 0x9E3779B9)` — the second derived with the
/// golden-ratio constant so the two never collide and both vary every frame.
pub fn rand_salts_of(frame_count: u32) -> (u32, u32) {
    (frame_count, frame_count ^ 0x9E37_79B9)
}

/// The 8×8 bucket-grid geometry for a viewport (`WorldRenderBase.cs:157-159`):
/// `bucket_size = ceil(viewport / 8)`, `bucket_count = bucket_size.x * .y`.
pub fn bucket_grid_of(viewport: UVec2) -> (UVec2, u32) {
    let bucket_size = (viewport + UVec2::splat(7)) / 8;
    let bucket_count = bucket_size.x * bucket_size.y;
    (bucket_size, bucket_count)
}

/// The render-world GPU resource owning the Phase-B GI buffers + the GI uniform
/// (`09-design-b.md` §2.1, §3.7). Created/resized once per resolution by
/// [`prepare_gi`].
///
/// None of these grow at runtime — they are all fixed-per-resolution
/// (`09-design-b.md` §3.1): `pixel_count`- and `bucket_count`-sized buffers
/// resize on a viewport change; `sample_counts` + the three indirect buffers
/// are fixed-size. All use plain `create_buffer`, not `GrowableBuffer`.
#[derive(Resource)]
pub struct GiGpu {
    // --- ray-queue (rayQueueCalc — §7) -------------------------------------
    /// `rayQueueBuffer` — `array<u32>`, `pixel_count + 1`. The adaptive ray
    /// queue: each entry a packed `pixelPos.x | (pixelPos.y << 16)`.
    pub ray_queue: Buffer,
    /// `rayQueueIndirectBuffer` — 5×`u32`, `INDIRECT`. Element `[0]` is the
    /// queued-pixel counter (then the indirect `GroupCountX` for `globalIllum`);
    /// `[1]`/`[2]` are `GroupCountY/Z`, seeded `1`.
    pub ray_queue_indirect: Buffer,
    // --- GI sample lists (globalIllum / sampleRefine — §8) -----------------
    /// `globalIlumValidSamples` — the lit-sample ring, `array<GpuSampleValid>`,
    /// `pixel_count * VALID_SAMPLE_STORAGE_COUNT`.
    pub valid_samples: Buffer,
    /// `globalIlumInvalidSamples` — the unlit-sample ring, `array<vec4<u32>>`,
    /// `pixel_count * INVALID_SAMPLE_STORAGE_COUNT`.
    pub invalid_samples: Buffer,
    /// `globalIlumValidSamplesRefined` — `array<vec4<u32>>`,
    /// `bucket_count * BUCKET_STORAGE_COUNT`. Written by Batch 4's `sampleRefine`.
    pub valid_samples_refined: Buffer,
    /// `globalIlumValidSamplesCompressed` — `array<vec4<u32>>`,
    /// `bucket_count * REFINED_BUCKET_STORAGE_COUNT`. Written by Batch 4.
    pub valid_samples_compressed: Buffer,
    /// `globalIlumBucketInfo` — `array<vec2<u32>>`, `bucket_count`. The 8×8
    /// screen-space region data. Written by Batch 4.
    pub bucket_info: Buffer,
    /// `globalIlumSampleCounts` — `array<vec2<u32>>`, `SAMPLE_COUNTS_LEN`. The
    /// 128-frame accumulation ring. Fixed-size; zero-cleared **only on
    /// creation** (it carries the ring — `09-design-b.md` §3.7).
    pub sample_counts: Buffer,
    /// `globalIlumValidDispatch` — 5×`u32`, `INDIRECT`. Batch 4's
    /// `CountValidAndRefine` indirect args; seeded `[1,1,1,0,0]`.
    pub valid_dispatch: Buffer,
    /// `globalIlumInvalidDispatch` — 5×`u32`, `INDIRECT`. Batch 4's
    /// `CountInvalid` indirect args; seeded `[1,1,1,0,0]`.
    pub invalid_dispatch: Buffer,
    // --- denoiser scratch (spatialResampling writes / denoiseSplit reads) --
    /// `denoisePreprocessed` — `array<vec4<u32>>` (the C# `Uint3` stored padded
    /// to a 16-byte stride — `09-design-b.md` §3.3), `pixel_count`.
    pub denoise_preprocessed: Buffer,
    /// `denoisePreprocessedHorizontal` — `array<vec4<u32>>`, `pixel_count`.
    pub denoise_preprocessed_horizontal: Buffer,
    // --- per-frame GI uniform ----------------------------------------------
    /// The `GpuGiParams` uniform — rewritten every frame.
    pub gi_params: Buffer,
    // --- resize-trigger / bucket-grid geometry -----------------------------
    /// Pixel count the `pixel_count`-sized buffers are currently sized for.
    pub pixel_count: u32,
    /// Total 8×8-bucket count for the current viewport.
    pub bucket_count: u32,
    /// 8×8 bucket-grid cell size in pixels.
    pub bucket_size: UVec2,
}

/// The mixed GI bind groups — built by `prepare_frame_gpu`, not `prepare_gi`,
/// because they reference `GiGpu` + `FrameGpu` + `TaaGpu` buffers and must be
/// built after all three resources exist (`09-design-b.md` §10.3).
///
/// Batch 3 lands two of them, Batch 4 adds `sample_refine_bind_group`, Batch 5
/// adds `spatial_resampling_bind_group` + `denoise_bind_group`; the last
/// (`calc_new_taa_sample_bind_group`) arrives in Batch 6.
#[derive(Resource)]
pub struct GiBindGroups {
    /// `@group(0)` for the `ray_queue_calc` passes (`09-design-b.md` §4.5):
    /// `gi_params`, `first_hit_data` (read), `ray_queue` (rw),
    /// `ray_queue_indirect` (rw), `taa_sample_accum` (read).
    pub ray_queue_bind_group: BindGroup,
    /// `@group(1)` for the `naadf_global_illum` pass (`09-design-b.md` §8.1):
    /// `gi_params`, `first_hit_data` (read), `first_hit_absorption` (rw),
    /// `valid_samples` (rw), `invalid_samples` (rw), `sample_counts` (rw),
    /// `final_color` (rw), `ray_queue` (read), `camera_history` (read).
    pub global_illum_bind_group: BindGroup,
    /// `@group(0)` shared by all 5 `sample_refine` passes (`09-design-b.md`
    /// §8.2): `gi_params`, `first_hit_data` (read), `bucket_info` (rw),
    /// `valid_samples` (read), `valid_samples_refined` (rw),
    /// `valid_samples_compressed` (rw), `invalid_samples` (read),
    /// `sample_counts` (rw), `taa_dist_min_max` (read — `TaaGpu`),
    /// `ray_queue_indirect` (rw), `camera_history` (read — `TaaGpu`). Mixes
    /// `GiGpu` + `FrameGpu` + `TaaGpu`, so built here in `prepare_frame_gpu`.
    /// (`valid_dispatch` / `invalid_dispatch` are NOT here — see
    /// `sample_refine_dispatch_bind_group`.)
    pub sample_refine_bind_group: BindGroup,
    /// `@group(1)` for `sample_refine`'s `compute_valid_history` pass ONLY —
    /// `valid_dispatch` + `invalid_dispatch` (rw). The wgpu indirect-vs-storage
    /// split (`09-design-b.md` §8.2): these buffers are written by
    /// `compute_valid_history` and then consumed as `dispatch_workgroups_indirect`
    /// sources by the two count passes, so they cannot be bound rw in the shared
    /// `@group(0)` (the count passes bind that group).
    pub sample_refine_dispatch_bind_group: BindGroup,
    /// `@group(1)` for `naadf_spatial_resampling_node` (`09-design-b.md` §8.3):
    /// `gi_params`, `first_hit_data` / `first_hit_absorption` / `bucket_info` /
    /// `valid_samples_compressed` / `taa_sample_accum` (read), `final_color` /
    /// `denoise_preprocessed` (rw). Mixes `GiGpu` + `FrameGpu`
    /// (`first_hit_data` / `first_hit_absorption` / `final_color`) + `TaaGpu`
    /// (`taa_sample_accum`), so built here in `prepare_frame_gpu`. The pass also
    /// binds `@group(0)` world (it traverses).
    pub spatial_resampling_bind_group: BindGroup,
    /// `@group(0)` shared by the two `naadf_denoise_node` passes
    /// (`09-design-b.md` §9.1): `gi_params`, `first_hit_absorption` /
    /// `denoise_preprocessed` (read), `denoise_preprocessed_horizontal` /
    /// `final_color` (rw). Mixes `GiGpu` + `FrameGpu` (`first_hit_absorption` /
    /// `final_color`), so built here in `prepare_frame_gpu`.
    pub denoise_bind_group: BindGroup,
    /// Pixel count these bind groups' buffers were sized for — the rebuild
    /// trigger (mirrors `FrameGpu.pixel_count`).
    pub pixel_count: u32,
}

/// `RenderSystems::PrepareResources` system: create/resize/seed the [`GiGpu`]
/// buffers and upload the per-frame [`GpuGiParams`] uniform (`09-design-b.md`
/// §10.3).
///
/// Runs alongside `prepare_world_gpu` / `prepare_taa` / `prepare_atmosphere` in
/// `PrepareResources` — before `prepare_frame_gpu` (`PrepareBindGroups`), which
/// builds the *mixed* GI bind groups (`GiBindGroups`) once `GiGpu` +
/// `FrameGpu` + `TaaGpu` all exist. Skips silently until the camera +
/// camera-history have been extracted.
pub fn prepare_gi(
    mut commands: Commands,
    extracted_camera: Res<ExtractedCameraData>,
    extracted_history: Res<ExtractedCameraHistory>,
    extracted_gi: Res<ExtractedGiConfig>,
    existing: Option<Res<GiGpu>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    if !extracted_camera.valid || !extracted_history.valid {
        return;
    }
    let viewport = extracted_camera.viewport_size.max(UVec2::ONE);
    let pixel_count = viewport.x * viewport.y;
    let (bucket_size, bucket_count) = bucket_grid_of(viewport);
    let gi = extracted_gi.settings;

    // --- create / resize the buffers ---------------------------------------
    // The `pixel_count`- and `bucket_count`-sized buffers rebuild on a viewport
    // change; `sample_counts` + the three indirect buffers are fixed-size,
    // created once and kept. `sample_counts` MUST NOT be re-zeroed on a resize
    // it survives — it carries the 128-frame ring; but on a viewport change the
    // screen-space sample buffers are discarded, so the ring's contents become
    // stale anyway — re-zero it then too (the next 128 frames rebuild it).
    let resources = match &existing {
        Some(gpu) if gpu.pixel_count == pixel_count => GiBuffers {
            ray_queue: gpu.ray_queue.clone(),
            ray_queue_indirect: gpu.ray_queue_indirect.clone(),
            valid_samples: gpu.valid_samples.clone(),
            invalid_samples: gpu.invalid_samples.clone(),
            valid_samples_refined: gpu.valid_samples_refined.clone(),
            valid_samples_compressed: gpu.valid_samples_compressed.clone(),
            bucket_info: gpu.bucket_info.clone(),
            sample_counts: gpu.sample_counts.clone(),
            valid_dispatch: gpu.valid_dispatch.clone(),
            invalid_dispatch: gpu.invalid_dispatch.clone(),
            denoise_preprocessed: gpu.denoise_preprocessed.clone(),
            denoise_preprocessed_horizontal: gpu.denoise_preprocessed_horizontal.clone(),
            gi_params: gpu.gi_params.clone(),
            fresh: false,
        },
        _ => create_gi_buffers(&render_device, &render_queue, pixel_count, bucket_count),
    };

    // --- per-frame reset of the ray-queue indirect counter -----------------
    // `ray_queue_indirect[0]` is the queued-pixel counter: `calcRayQueue`
    // `atomicAdd`s into it, `calcRayQueueStore` rewrites it as the workgroup
    // count. It MUST be zeroed each frame *before* `calcRayQueue` runs
    // (`09-design-b.md` §7.3). Batch 3 re-seeded `ray_queue_indirect` from the
    // CPU here every frame as a standalone-correctness seam; **Batch 4 moves
    // that reset in-shader** — `clear_buckets_and_calc_mask` (the first
    // sample-refine pass, scheduled BEFORE `naadf_ray_queue_node` in the
    // `09-design-b.md` §4.2 chain) does `ray_queue_indirect[0] = 0u` when
    // `global_id.x == 0` (`renderSampleRefine.fx:39`). The CPU re-seed is
    // therefore deleted — the in-shader reset is the faithful NAADF behaviour.
    // (`ray_queue_indirect` is still seeded `[0,1,1,0,0]` *on creation* in
    // `create_gi_buffers` so `[1]`/`[2]` = `GroupCountY/Z` = 1.)

    // --- upload the per-frame GI uniform -----------------------------------
    let frame_count = extracted_history.frame_count;
    let (rand_counter, rand_counter2) = rand_salts_of(frame_count);
    let accum_index = accum_index_of(frame_count, gi.global_illum_max_accum);
    let current_pos = extracted_camera.position_split;

    // The sun direction — the same hand-tuned fixed direction
    // `prepare_atmosphere` / `prepare_frame_gpu` use (`atmosphere.rs:321-328`).
    // Phase B shares ONE `sky_sun_dir` across the atmosphere + GI uniforms
    // (`09-design-b.md` §3.9); the value is derived identically here.
    let sun_elev = 0.9_f32;
    let sun_azim = 0.6_f32;
    let sky_sun_dir = Vec3::new(
        sun_elev.cos() * sun_azim.cos(),
        sun_elev.sin(),
        sun_elev.cos() * sun_azim.sin(),
    )
    .normalize();

    // `sunColor = Atmosphere.GetLightForPoint((0, 10, 0))` (`WorldRender.cs:96`)
    // — the CPU atmosphere model (`09-design-b.md` §9.2).
    let sun_color = Atmosphere::get_light_for_point(Vec3::new(0.0, 10.0, 0.0), sky_sun_dir);

    // Pack the GI flags from the extracted settings.
    let mut flags = 0u32;
    if gi.skip_samples {
        flags |= GI_FLAG_SKIP_SAMPLES;
    }
    if gi.is_denoise {
        flags |= GI_FLAG_IS_DENOISE;
    }
    if gi.is_sample_leveling {
        flags |= GI_FLAG_IS_SAMPLE_LEVELING;
    }
    if gi.is_varying_resampling_radius {
        flags |= GI_FLAG_IS_VARYING_RADIUS;
    }
    if gi.is_atmosphere_interaction {
        flags |= GI_FLAG_IS_ATMOSPHERE_INTERACTION;
    }

    let gi_params_data = GpuGiParams {
        inv_view_proj: extracted_camera.inv_view_proj,
        view_proj: extracted_camera.view_proj,
        cam_pos_int: current_pos.pos_int,
        _pad0: 0,
        cam_pos_frac: current_pos.pos_frac,
        _pad1: 0,
        sky_sun_dir,
        _pad2: 0,
        sun_color,
        _pad3: 0,
        screen_width: viewport.x,
        screen_height: viewport.y,
        frame_count,
        taa_index: extracted_history.taa_index,
        accum_index,
        rand_counter,
        rand_counter2,
        max_bounce_count: gi.bounce_count,
        bucket_size_x: bucket_size.x,
        bucket_size_y: bucket_size.y,
        bucket_count,
        sample_max_accum: gi.global_illum_max_accum,
        valid_sample_storage_count: VALID_SAMPLE_STORAGE_COUNT,
        invalid_sample_storage_count: INVALID_SAMPLE_STORAGE_COUNT,
        bucket_storage_count: BUCKET_STORAGE_COUNT,
        refined_bucket_storage_count: REFINED_BUCKET_STORAGE_COUNT,
        spatial_resample_size: gi.spatial_resample_size,
        radius_lit_factor: gi.radius_lit_factor,
        noise_suppression_factor: gi.noise_suppression_factor,
        denoise_thresh: gi.denoise_thresh,
        flags,
        _pad4: 0,
        // The GI sample-generation + spatial-resampling rays are fired through
        // this per-frame Halton sub-pixel offset (the C# `taaJitter` passed to
        // `getRayDir` in `renderGlobalIllum.fx:69` / `renderSpatialResampling
        // .fx:351`). Reuse the SAME jitter source of truth `prepare_frame_gpu`
        // already routes to `GpuRenderParams.taa_jitter` for the first-hit pass
        // — `extracted_history.current_jitter`, computed once per frame in
        // `update_camera_history` (zero when `AppArgs.taa` is off). Do NOT
        // compute a second jitter. Without this the GI rays sample the exact
        // same sub-pixel point every frame and the long-term TAA can never
        // resolve sub-pixel detail (`18-taa-fidelity.md` cause #1).
        taa_jitter: extracted_history.current_jitter,
        // Multi-tap sun shadow (`spatial_resampling.wgsl:529-560` — paper §5.2
        // mitigation; Dispatch A in `19-gi-reservoir-scope.md` §3.1). The
        // shader clamps `< 1u` to `1u`, so a zero here resolves to the C#
        // single-tap path.
        sun_shadow_taps: gi.sun_shadow_taps,
        _pad5: 0,
        _pad6: 0,
        _pad7: 0,
        // Quality-panel runtime knobs (`21-design-quality-panel.md` §4.2). 5
        // `MAX_RAY_STEPS_*` caps + the Algorithm-2 spatial iter count. Each
        // consumer WGSL site clamps `max(_, 1u)` defensively, so the
        // bytemuck::Zeroable / `GiSettings { ..: 0, .. }` cases are harmless.
        max_ray_steps_secondary: gi.max_ray_steps_secondary,
        max_ray_steps_sun: gi.max_ray_steps_sun,
        max_ray_steps_sun_secondary: gi.max_ray_steps_sun_secondary,
        max_ray_steps_visibility: gi.max_ray_steps_visibility,
        spatial_iter_count: gi.spatial_iter_count,
        _pad8: 0,
        _pad9: 0,
        _pad10: 0,
    };
    render_queue.write_buffer(&resources.gi_params, 0, bytemuck::bytes_of(&gi_params_data));

    commands.insert_resource(GiGpu {
        ray_queue: resources.ray_queue,
        ray_queue_indirect: resources.ray_queue_indirect,
        valid_samples: resources.valid_samples,
        invalid_samples: resources.invalid_samples,
        valid_samples_refined: resources.valid_samples_refined,
        valid_samples_compressed: resources.valid_samples_compressed,
        bucket_info: resources.bucket_info,
        sample_counts: resources.sample_counts,
        valid_dispatch: resources.valid_dispatch,
        invalid_dispatch: resources.invalid_dispatch,
        denoise_preprocessed: resources.denoise_preprocessed,
        denoise_preprocessed_horizontal: resources.denoise_preprocessed_horizontal,
        gi_params: resources.gi_params,
        pixel_count,
        bucket_count,
        bucket_size,
    });
}

/// The freshly-built (or cloned) GI buffer set — the intermediate `prepare_gi`
/// passes around so the `existing`-clone path and the create path agree.
struct GiBuffers {
    ray_queue: Buffer,
    ray_queue_indirect: Buffer,
    valid_samples: Buffer,
    invalid_samples: Buffer,
    valid_samples_refined: Buffer,
    valid_samples_compressed: Buffer,
    bucket_info: Buffer,
    sample_counts: Buffer,
    valid_dispatch: Buffer,
    invalid_dispatch: Buffer,
    denoise_preprocessed: Buffer,
    denoise_preprocessed_horizontal: Buffer,
    gi_params: Buffer,
    /// `true` when freshly created this call (so callers can rebuild the mixed
    /// bind groups). Currently informational — `GiBindGroups` keys off its own
    /// `pixel_count` — but kept so a future caller need not re-derive it.
    #[allow(dead_code)]
    fresh: bool,
}

/// Create + seed + zero-clear the full §3.7 GI buffer set for `pixel_count`
/// pixels / `bucket_count` 8×8 buckets.
///
/// - all `STORAGE | COPY_DST` (the indirect buffers also `INDIRECT`);
/// - the `pixel_count` / `bucket_count` element sizes from `09-design-b.md` §3.1;
/// - everything zero-cleared on creation so a half-built pipeline reads zeroed
///   (correct-but-empty) data, never garbage;
/// - the three indirect buffers seeded: `ray_queue_indirect = [0,1,1,0,0]`,
///   `valid_dispatch = invalid_dispatch = [1,1,1,0,0]` (`WorldRenderBase.cs:136,
///   168,170`).
fn create_gi_buffers(
    render_device: &RenderDevice,
    render_queue: &RenderQueue,
    pixel_count: u32,
    bucket_count: u32,
) -> GiBuffers {
    // wgpu rejects zero-length buffers — `pixel_count` is `>= 1`, but
    // `bucket_count` for a 1×1 viewport is also `>= 1`; clamp defensively.
    let pixel_count = pixel_count.max(1) as u64;
    let bucket_count = bucket_count.max(1) as u64;

    let mk = |label: &str, size: u64, usages: BufferUsages| {
        render_device.create_buffer(&BufferDescriptor {
            label: Some(label),
            // wgpu rejects a zero-size buffer.
            size: size.max(16),
            usage: usages,
            mapped_at_creation: false,
        })
    };

    // `ray_queue`: (pixel_count + 1) × u32 (4 bytes).
    let ray_queue = mk("naadf_gi_ray_queue", (pixel_count + 1) * 4, GI_BUFFER_USAGES);
    // `ray_queue_indirect` / `valid_dispatch` / `invalid_dispatch`: 5 × u32.
    let ray_queue_indirect =
        mk("naadf_gi_ray_queue_indirect", 5 * 4, GI_INDIRECT_USAGES);
    // `valid_samples`: pixel_count * 2 × GpuSampleValid (32 bytes).
    let valid_samples = mk(
        "naadf_gi_valid_samples",
        pixel_count * VALID_SAMPLE_STORAGE_COUNT as u64
            * std::mem::size_of::<GpuSampleValid>() as u64,
        GI_BUFFER_USAGES,
    );
    // `invalid_samples`: pixel_count * 8 × vec4<u32> (16 bytes).
    let invalid_samples = mk(
        "naadf_gi_invalid_samples",
        pixel_count * INVALID_SAMPLE_STORAGE_COUNT as u64 * 16,
        GI_BUFFER_USAGES,
    );
    // `valid_samples_refined`: bucket_count * 32 × vec4<u32> (16 bytes).
    let valid_samples_refined = mk(
        "naadf_gi_valid_samples_refined",
        bucket_count * BUCKET_STORAGE_COUNT as u64 * 16,
        GI_BUFFER_USAGES,
    );
    // `valid_samples_compressed`: bucket_count * 8 × vec4<u32> (16 bytes).
    let valid_samples_compressed = mk(
        "naadf_gi_valid_samples_compressed",
        bucket_count * REFINED_BUCKET_STORAGE_COUNT as u64 * 16,
        GI_BUFFER_USAGES,
    );
    // `bucket_info`: bucket_count × vec2<u32> (8 bytes).
    let bucket_info = mk("naadf_gi_bucket_info", bucket_count * 8, GI_BUFFER_USAGES);
    // `sample_counts`: (128 + 3) × vec2<u32> (8 bytes) — fixed-size.
    let sample_counts = mk(
        "naadf_gi_sample_counts",
        SAMPLE_COUNTS_LEN as u64 * 8,
        GI_BUFFER_USAGES,
    );
    let valid_dispatch = mk("naadf_gi_valid_dispatch", 5 * 4, GI_INDIRECT_USAGES);
    let invalid_dispatch =
        mk("naadf_gi_invalid_dispatch", 5 * 4, GI_INDIRECT_USAGES);
    // `denoise_preprocessed` / `_horizontal`: pixel_count × vec4<u32> (16 bytes —
    // the C# `Uint3` stored padded to WGSL's 16-byte stride — §3.3).
    let denoise_preprocessed = mk(
        "naadf_gi_denoise_preprocessed",
        pixel_count * 16,
        GI_BUFFER_USAGES,
    );
    let denoise_preprocessed_horizontal = mk(
        "naadf_gi_denoise_preprocessed_horizontal",
        pixel_count * 16,
        GI_BUFFER_USAGES,
    );
    // The per-frame GI uniform.
    let gi_params = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_gi_params"),
        size: std::mem::size_of::<GpuGiParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- zero-clear everything, then seed the indirect buffers --------------
    let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("naadf_gi_clear_and_seed"),
    });
    encoder.clear_buffer(&ray_queue, 0, None);
    encoder.clear_buffer(&ray_queue_indirect, 0, None);
    encoder.clear_buffer(&valid_samples, 0, None);
    encoder.clear_buffer(&invalid_samples, 0, None);
    encoder.clear_buffer(&valid_samples_refined, 0, None);
    encoder.clear_buffer(&valid_samples_compressed, 0, None);
    encoder.clear_buffer(&bucket_info, 0, None);
    encoder.clear_buffer(&sample_counts, 0, None);
    encoder.clear_buffer(&valid_dispatch, 0, None);
    encoder.clear_buffer(&invalid_dispatch, 0, None);
    encoder.clear_buffer(&denoise_preprocessed, 0, None);
    encoder.clear_buffer(&denoise_preprocessed_horizontal, 0, None);
    render_queue.submit([encoder.finish()]);

    // Seed the indirect-dispatch arg buffers (after the zero-clear submit, so
    // the writes land last). `ray_queue_indirect = {GroupCountX=0, Y=1, Z=1}`
    // (`WorldRenderBase.cs:136`); `valid/invalid_dispatch = {1,1,1}`
    // (`WorldRenderBase.cs:168,170`). The 4th/5th `u32`s stay zero.
    render_queue.write_buffer(
        &ray_queue_indirect,
        0,
        bytemuck::cast_slice(&[0u32, 1u32, 1u32, 0u32, 0u32]),
    );
    render_queue.write_buffer(
        &valid_dispatch,
        0,
        bytemuck::cast_slice(&[1u32, 1u32, 1u32, 0u32, 0u32]),
    );
    render_queue.write_buffer(
        &invalid_dispatch,
        0,
        bytemuck::cast_slice(&[1u32, 1u32, 1u32, 0u32, 0u32]),
    );

    GiBuffers {
        ray_queue,
        ray_queue_indirect,
        valid_samples,
        invalid_samples,
        valid_samples_refined,
        valid_samples_compressed,
        bucket_info,
        sample_counts,
        valid_dispatch,
        invalid_dispatch,
        denoise_preprocessed,
        denoise_preprocessed_horizontal,
        gi_params,
        fresh: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accum_index_walks_the_ring() {
        // `accum_index = max_accum - (frame % max_accum) - 1` — a descending
        // ring index that wraps at `max_accum` (`WorldRenderBase.cs:181`).
        assert_eq!(accum_index_of(0, 128), 127);
        assert_eq!(accum_index_of(1, 128), 126);
        assert_eq!(accum_index_of(127, 128), 0);
        assert_eq!(accum_index_of(128, 128), 127);
        assert_eq!(accum_index_of(129, 128), 126);
    }

    #[test]
    fn rand_salts_are_two_distinct_per_frame_values() {
        // The load-bearing property: two distinct salts, both varying per frame
        // (`09-design-b.md` §10.3).
        for frame in [0u32, 1, 2, 100, 12345] {
            let (a, b) = rand_salts_of(frame);
            assert_ne!(a, b, "the two salts must never collide (frame {frame})");
            assert_eq!(a, frame, "the first salt is the raw frame counter");
        }
        // Both vary frame-to-frame.
        assert_ne!(rand_salts_of(0), rand_salts_of(1));
    }

    #[test]
    fn bucket_grid_ceils_to_eights() {
        // `bucket_size = ceil(viewport / 8)` (`WorldRenderBase.cs:157-159`).
        assert_eq!(bucket_grid_of(UVec2::new(8, 8)), (UVec2::new(1, 1), 1));
        assert_eq!(bucket_grid_of(UVec2::new(9, 8)), (UVec2::new(2, 1), 2));
        assert_eq!(bucket_grid_of(UVec2::new(1920, 1080)), (UVec2::new(240, 135), 240 * 135));
        // A non-multiple-of-8 viewport rounds each axis up.
        assert_eq!(bucket_grid_of(UVec2::new(1921, 1081)), (UVec2::new(241, 136), 241 * 136));
    }
}
