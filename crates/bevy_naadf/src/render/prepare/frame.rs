//! `prepare/frame.rs` — per-frame uniform writes + bind-group rebuilds.
//!
//! Houses [`prepare_frame_gpu`], the `RenderSystems::PrepareBindGroups` system
//! that rewrites the `GpuCamera` + `GpuRenderParams` uniforms every frame,
//! (re)creates the `first_hit_data` / `first_hit_absorption` / `final_color`
//! storage buffers on a viewport resize, and rebuilds the 5 frame-level bind
//! groups (`bind_group`, `first_hit_atmosphere_bind_group`, `blit_bind_group`,
//! `taa_reproject_bind_group`, `calc_new_taa_sample_bind_group`) plus the
//! 6 mixed [`GiBindGroups`] (`09-design-b.md` §10.3).
//!
//! Split out of the original `render/prepare.rs` per the codebase-tightening
//! D4 architect's Step 3 — pure structural relocation, no behaviour change.

use std::f32::consts::PI;

use bevy::math::Vec3;
use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, PipelineCache,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use crate::render::atmosphere::AtmosphereGpu;
use crate::render::extract::{ExtractedCameraData, ExtractedCameraHistory, ExtractedGiConfig};
use crate::render::gi::{GiBindGroups, GiGpu};
use crate::render::gpu_types::{
    GpuCamera, GpuRenderParams, FLAG_CHECK_SUN, FLAG_IS_ATMOSPHERE_INTERACTION, FLAG_IS_TAA,
};
use crate::render::pipelines::NaadfPipelines;
use crate::render::taa::TaaGpu;
use crate::world::buffer::GROWABLE_BUFFER_USAGES;

use super::{FrameGpu, WorldGpu};

/// `RenderSystems::PrepareBindGroups` system: write the per-frame camera +
/// render-params uniforms, (re)create the `first_hit_data` storage buffer on a
/// viewport resize, and build the frame bind groups.
///
/// Runs in `PrepareBindGroups` (after `PrepareResources`) so the world bind
/// group / pipelines *and* `TaaGpu` are already created. Skips silently until
/// the camera has been extracted and `TaaGpu` exists.
///
/// Phase A-2: the per-pixel accumulated-colour buffer (Phase A's `shaded_color`
/// stand-in) moved into `TaaGpu` as the real `taa_sample_accum`; this system
/// reads `TaaGpu` and binds `taa_gpu.taa_sample_accum` where it used to bind
/// the local `shaded_color` (`06-design-a2.md` §5.5, §9.4).
// Bevy systems legitimately exceed clippy's 7-argument ceiling.
#[allow(clippy::too_many_arguments)]
pub fn prepare_frame_gpu(
    mut commands: Commands,
    extracted_camera: Res<ExtractedCameraData>,
    extracted_history: Res<ExtractedCameraHistory>,
    extracted_taa: Res<crate::render::extract::ExtractedTaaConfig>,
    extracted_gi: Res<ExtractedGiConfig>,
    existing: Option<ResMut<FrameGpu>>,
    existing_gi_bind_groups: Option<Res<GiBindGroups>>,
    taa_gpu: Option<Res<TaaGpu>>,
    atmosphere_gpu: Option<Res<AtmosphereGpu>>,
    gi_gpu: Option<Res<GiGpu>>,
    world_gpu: Option<Res<WorldGpu>>,
    pipelines: Res<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    if !extracted_camera.valid {
        return;
    }
    // `TaaGpu` (created in `PrepareResources` by `prepare_taa`) owns
    // `taa_sample_accum`; `AtmosphereGpu` (created by `prepare_atmosphere`)
    // owns the precomputed atmosphere buffer + uniform; `GiGpu` (created by
    // `prepare_gi`) owns every Phase-B GI buffer. Wait for all three before
    // building the bind groups (`09-design-b.md` §10.3) — the mixed GI bind
    // groups (`GiBindGroups`) reference `GiGpu` + `FrameGpu` + `TaaGpu`, so
    // they are built here, after all three exist.
    let Some(taa_gpu) = taa_gpu else {
        return;
    };
    let Some(atmosphere_gpu) = atmosphere_gpu else {
        return;
    };
    let Some(gi_gpu) = gi_gpu else {
        return;
    };
    // `WorldGpu` (created in `PrepareResources` by `prepare_world_gpu` once the
    // test grid has been extracted) owns `voxel_types` — the
    // `calc_new_taa_sample` bind group needs it (`09-design-b.md` §4.10). Wait
    // for it like the other three render-world resources.
    let Some(world_gpu) = world_gpu else {
        return;
    };
    let viewport = extracted_camera.viewport_size.max(UVec2::ONE);
    let pixel_count = viewport.x * viewport.y;

    // A simple fixed sun for Phase A's flat-lit scene. `sky_sun_dir` points
    // *towards* the sun (the C# `skySunDir` convention).
    let sun_elev = 0.9_f32;
    let sun_azim = 0.6_f32;
    let sky_sun_dir = Vec3::new(
        sun_elev.cos() * sun_azim.cos(),
        sun_elev.sin(),
        sun_elev.cos() * sun_azim.sin(),
    )
    .normalize();
    let _ = PI; // sun angles are hand-tuned constants for Phase A.

    let camera_data = GpuCamera {
        inv_view_proj: extracted_camera.inv_view_proj,
        cam_pos_int: extracted_camera.position_split.pos_int,
        _pad0: 0,
        cam_pos_frac: extracted_camera.position_split.pos_frac,
        _pad1: 0,
    };
    let render_params = GpuRenderParams {
        screen_width: viewport.x,
        screen_height: viewport.y,
        // The real monotonic frame counter (the carried `05-review.md` §4 fix —
        // `06-design-a2.md` §9.1). `frame_count` / `taa_index` come from the
        // extracted `CameraHistory`, computed once per frame in
        // `update_camera_history` (`06-design-a2.md` §9.3 — `taa_index` is
        // *stored*, not re-derived render-side, to avoid the off-by-one trap).
        frame_count: extracted_history.frame_count,
        // `rand_counter` = the frame counter (the monotonic per-frame RNG salt
        // — `init_rand` uses it only as salt). Deliberate A-2 simplification:
        // NAADF refills a `randValues[32]` table per frame and indexes it
        // (`WorldRender.cs:82-86`); the load-bearing property is a
        // per-frame-varying salt, which the counter already is — the table is
        // not ported (`06-design-a2.md` §4.1, §13.3).
        rand_counter: extracted_history.frame_count,
        taa_index: extracted_history.taa_index,
        // Phase B Batch 6 flags:
        // - `FLAG_IS_ATMOSPHERE_INTERACTION` is always set — the C#
        //   `WorldRenderBase.isAtmosphereInteraction` defaults to `true`
        //   (`WorldRenderBase.cs:16,224`), so the `base/` first-hit ray-marches
        //   the atmosphere along each primary-ray segment.
        // - `FLAG_BLIT_FINAL_COLOR` is NO LONGER set — Batch 6 reverts the
        //   Batch-2 temporary blit seam. The final blit reads `taa_sample_accum`
        //   again (the real `base/` blit source — correctly filled by
        //   `ReprojectOld` + `CalcNewTaaSample`); `09-design-b.md` §11 Batch 6
        //   step 19.
        // - `FLAG_CHECK_SUN` is left set for layout stability but is no longer
        //   read — the `base/` first-hit gets all sky light from the full
        //   atmosphere model, not the Phase-A inline sun term.
        // - `FLAG_IS_TAA` is set when `AppArgs.taa` is on (extracted into
        //   `ExtractedTaaConfig`); it gates the TAA jitter path + (Batch 6) the
        //   `naadf_taa_reproject_node` / `naadf_calc_new_taa_sample_node`
        //   dispatch.
        flags: if extracted_taa.enabled {
            FLAG_CHECK_SUN | FLAG_IS_TAA | FLAG_IS_ATMOSPHERE_INTERACTION
        } else {
            FLAG_CHECK_SUN | FLAG_IS_ATMOSPHERE_INTERACTION
        },
        // Was `_pad0a` (formerly `exposure` — dead since `18-taa-fidelity.md`
        // fix #2). Now `max_ray_steps_primary` — the quality-panel runtime
        // knob for the primary G-buffer DDA cap
        // (`21-design-quality-panel.md` §4.1). Default 120, bit-equivalent to
        // the pre-dispatch `MAX_RAY_STEPS_PRIMARY` const. Layout-preserving
        // rename; struct size unchanged.
        max_ray_steps_primary: extracted_gi.settings.max_ray_steps_primary,
        // Padding — formerly `tone_mapping_fac`, dead since fix #2.
        _pad0b: 0,
        sky_sun_dir,
        _pad1: 0,
        sun_color: Vec3::new(1.0, 0.95, 0.85),
        _pad2: 0,
        // This frame's Halton jitter — the same value `update_camera_history`
        // wrote into `CameraHistory.jitter[taa_index]` (one value, computed
        // once — `06-design-a2.md` §9.3). Zero unless `AppArgs.taa` is on.
        taa_jitter: extracted_history.current_jitter,
        _pad3: Vec2::ZERO,
        bounding_box_min: Vec3::ZERO, // filled below from WorldGpu's meta? — see note
        _pad4: 0,
        bounding_box_max: Vec3::ZERO,
        _pad5: 0,
    };

    // The bounding box the first-hit `rayAABB` tests against comes from the
    // extracted world, not the camera — but `prepare_frame_gpu` only has the
    // camera. The world's bounding box is uploaded in `world_meta`
    // (`@group(0)`), and the first-hit shader reads `rayAABB` bounds from
    // `world_meta`, so `GpuRenderParams.bounding_box_*` is left zeroed here
    // and the shader uses `world_meta` instead. Kept in the struct so the
    // uniform layout is stable for Phase A-2 / B.

    // (re)create the per-pixel storage buffers if the pixel count changed.
    // `taa_sample_accum` (Phase A's `shaded_color`) lives in `TaaGpu` and is
    // (re)sized by `prepare_taa` on the same trigger — they read the same
    // `extracted_camera.viewport_size`, so they stay coherent (`06-design-a2.md`
    // §9.4). Phase B Batch 2 adds `first_hit_absorption` + `final_color`
    // (`09-design-b.md` §3.4) — both `vec2<u32>` per pixel, created/resized/
    // zero-cleared alongside `first_hit_data`.
    let (first_hit_data, first_hit_absorption, final_color, needs_new_storage) =
        match &existing {
            Some(frame) if frame.pixel_count == pixel_count => (
                frame.first_hit_data.clone(),
                frame.first_hit_absorption.clone(),
                frame.final_color.clone(),
                false,
            ),
            _ => {
                // first_hit_data: vec4<u32> per pixel (16 bytes).
                let first_hit_data = render_device.create_buffer(&BufferDescriptor {
                    label: Some("naadf_first_hit_data"),
                    size: (pixel_count as u64) * 16,
                    usage: GROWABLE_BUFFER_USAGES,
                    mapped_at_creation: false,
                });
                // first_hit_absorption / final_color: vec2<u32> per pixel (8 B).
                let first_hit_absorption =
                    render_device.create_buffer(&BufferDescriptor {
                        label: Some("naadf_first_hit_absorption"),
                        size: (pixel_count as u64) * 8,
                        usage: GROWABLE_BUFFER_USAGES,
                        mapped_at_creation: false,
                    });
                let final_color = render_device.create_buffer(&BufferDescriptor {
                    label: Some("naadf_final_color"),
                    size: (pixel_count as u64) * 8,
                    usage: GROWABLE_BUFFER_USAGES,
                    mapped_at_creation: false,
                });
                (first_hit_data, first_hit_absorption, final_color, true)
            }
        };

    // The uniform buffers persist across frames; create them once.
    let (camera_buf, render_params_buf) = match &existing {
        Some(frame) => (frame.camera.clone(), frame.render_params.clone()),
        None => {
            let camera_buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_camera"),
                size: std::mem::size_of::<GpuCamera>() as u64,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let render_params_buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_render_params"),
                size: std::mem::size_of::<GpuRenderParams>() as u64,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            (camera_buf, render_params_buf)
        }
    };
    render_queue.write_buffer(&camera_buf, 0, bytemuck::bytes_of(&camera_data));
    render_queue.write_buffer(&render_params_buf, 0, bytemuck::bytes_of(&render_params));

    // Zero the per-pixel storage buffers when freshly (re)created so a clean
    // frame is shown until the first-hit pass fills them, rather than garbage.
    // (`taa_sample_accum` is zero-cleared by `prepare_taa` on its own
    // (re)creation.)
    if needs_new_storage {
        let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("naadf_clear_gbuffer"),
        });
        encoder.clear_buffer(&first_hit_data, 0, None);
        encoder.clear_buffer(&first_hit_absorption, 0, None);
        encoder.clear_buffer(&final_color, 0, None);
        render_queue.submit([encoder.finish()]);
    }

    // Rebuild the bind groups when storage changed; otherwise reuse.
    //
    // Phase B Batch 2 (`09-design-b.md` §6.3) — unchanged this batch:
    // - The frame `@group(1)` binds `first_hit_absorption` (slot 4) +
    //   `final_color` (slot 5) — the `base/` first-hit's two new outputs.
    //   `taa_sample_accum` stays at slot 3 for layout stability (the `base/`
    //   first-hit no longer writes it — `ReprojectOld` + `CalcNewTaaSample`
    //   do).
    // - The first-hit's `@group(2)` is the read-only precomputed atmosphere.
    //
    // Phase B Batch 6 (`09-design-b.md` §11 Batch 6 steps 17-19):
    // - The blit `@group(0)` binds `taa_sample_accum` at slot 1 again — the
    //   Batch-2 temporary `final_color` seam is REVERTED. `taa_sample_accum`
    //   is now correctly filled by `ReprojectOld` (the reprojected history) +
    //   `CalcNewTaaSample` (history + this frame's denoised GI light).
    // - The TAA reproject bind group gains `taa_dist_min_max` (slot 5) — the
    //   `base/` `ReprojectOld` extra output.
    // - The new `calc_new_taa_sample` bind group mixes `TaaGpu` + `FrameGpu` +
    //   `WorldGpu` (`voxel_types`) — built here once all three exist.
    //
    // `TaaGpu`'s `taa_sample_accum` / `taa_samples` / `taa_dist_min_max` resize
    // on the same `pixel_count` trigger as `first_hit_data`, so
    // `needs_new_storage` covers all of them. `voxel_types` (in `WorldGpu`) is
    // build-once; re-referencing it on a viewport-resize rebuild is harmless.
    let (
        bind_group,
        first_hit_atmosphere_bind_group,
        blit_bind_group,
        taa_reproject_bind_group,
        calc_new_taa_sample_bind_group,
    ) = if needs_new_storage || existing.is_none() {
        let bind_group = render_device.create_bind_group(
            "naadf_frame_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.frame_layout),
            &BindGroupEntries::sequential((
                camera_buf.as_entire_buffer_binding(),
                render_params_buf.as_entire_buffer_binding(),
                first_hit_data.as_entire_buffer_binding(),
                taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
                first_hit_absorption.as_entire_buffer_binding(),
                final_color.as_entire_buffer_binding(),
            )),
        );
        let first_hit_atmosphere_bind_group = render_device.create_bind_group(
            "naadf_first_hit_atmosphere_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.atmosphere_read_layout),
            &BindGroupEntries::sequential((
                atmosphere_gpu.atmosphere_params.as_entire_buffer_binding(),
                atmosphere_gpu.atmosphere_comp.as_entire_buffer_binding(),
            )),
        );
        let blit_bind_group = render_device.create_bind_group(
            "naadf_blit_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.blit_layout),
            &BindGroupEntries::sequential((
                first_hit_data.as_entire_buffer_binding(),
                // Phase B Batch 6: the real `base/` blit source —
                // `taa_sample_accum` (the Batch-2 temporary `final_color` seam
                // is reverted — `09-design-b.md` §11 Batch 6 step 19).
                taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
                render_params_buf.as_entire_buffer_binding(),
            )),
        );
        let taa_reproject_bind_group = render_device.create_bind_group(
            "naadf_taa_reproject_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.taa_reproject_layout),
            &BindGroupEntries::sequential((
                taa_gpu.taa_params.as_entire_buffer_binding(),
                taa_gpu.camera_history.as_entire_buffer_binding(),
                first_hit_data.as_entire_buffer_binding(),
                taa_gpu.taa_samples.as_entire_buffer_binding(),
                taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
                // Phase B Batch 6: the `base/` `ReprojectOld` extra output.
                taa_gpu.taa_dist_min_max.as_entire_buffer_binding(),
            )),
        );
        // `calc_new_taa_sample` `@group(1)` — `09-design-b.md` §4.10. Mixes
        // `TaaGpu` (`taa_params` / `taa_samples` / `taa_sample_accum`) +
        // `FrameGpu` (`first_hit_data` / `final_color`) + `WorldGpu`
        // (`voxel_types`). The pass folds the denoised GI `final_color` into
        // the 16-deep `taa_samples` ring + `taa_sample_accum`.
        let calc_new_taa_sample_bind_group = render_device.create_bind_group(
            "naadf_calc_new_taa_sample_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.calc_new_taa_sample_layout),
            &BindGroupEntries::sequential((
                taa_gpu.taa_params.as_entire_buffer_binding(),
                first_hit_data.as_entire_buffer_binding(),
                final_color.as_entire_buffer_binding(),
                world_gpu.voxel_types.buffer().as_entire_buffer_binding(),
                taa_gpu.taa_samples.as_entire_buffer_binding(),
                taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
            )),
        );
        (
            bind_group,
            first_hit_atmosphere_bind_group,
            blit_bind_group,
            taa_reproject_bind_group,
            calc_new_taa_sample_bind_group,
        )
    } else if let Some(frame) = existing.as_ref() {
        // The `else` of `needs_new_storage || existing.is_none()` — `existing`
        // is necessarily `Some` here; reuse the cached bind groups.
        (
            frame.bind_group.clone(),
            frame.first_hit_atmosphere_bind_group.clone(),
            frame.blit_bind_group.clone(),
            frame.taa_reproject_bind_group.clone(),
            frame.calc_new_taa_sample_bind_group.clone(),
        )
    } else {
        unreachable!("`needs_new_storage || existing.is_none()` was false → `existing` is `Some`")
    };

    // --- the mixed GI bind groups (`09-design-b.md` §10.3) ------------------
    // `GiBindGroups` mixes `GiGpu` + `FrameGpu` + `TaaGpu` buffers, so it is
    // built here (after all three resources exist) rather than in `prepare_gi`.
    // Rebuilt on the same `pixel_count` resize trigger as the frame buffers —
    // every buffer it references (`first_hit_data` / `first_hit_absorption` /
    // `final_color` / `taa_sample_accum` / the GI buffers) is `pixel_count`-
    // sized and re-created together. `camera_history` is fixed-size, but a
    // rebuild that re-references it is harmless.
    //
    // Batch 3 builds two: `ray_queue_bind_group` (`@group(0)` of the
    // `rayQueueCalc` passes) and `global_illum_bind_group` (`@group(1)` of
    // `renderGlobalIllum`). Batch 4 adds `sample_refine_bind_group` (`@group(0)`
    // shared by all 5 sample-refine passes — it mixes `GiGpu` + `FrameGpu`
    // (`first_hit_data`) + `TaaGpu` (`taa_dist_min_max` + `camera_history`),
    // exactly the mixed pattern). Batch 5 adds `spatial_resampling_bind_group`
    // (`@group(1)` of `renderSpatialResampling`) + `denoise_bind_group`
    // (`@group(0)` shared by the two `renderDenoiseSplit` passes). Batch 6 adds
    // the last (`calc_new_taa_sample_bind_group`).
    let gi_bind_groups_stale = match &existing_gi_bind_groups {
        Some(bg) => bg.pixel_count != pixel_count,
        None => true,
    };
    if needs_new_storage || existing_gi_bind_groups.is_none() || gi_bind_groups_stale {
        let ray_queue_bind_group = render_device.create_bind_group(
            "naadf_ray_queue_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.ray_queue_layout),
            &BindGroupEntries::sequential((
                gi_gpu.gi_params.as_entire_buffer_binding(),
                first_hit_data.as_entire_buffer_binding(),
                gi_gpu.ray_queue.as_entire_buffer_binding(),
                gi_gpu.ray_queue_indirect.as_entire_buffer_binding(),
                taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
            )),
        );
        let global_illum_bind_group = render_device.create_bind_group(
            "naadf_global_illum_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.global_illum_layout),
            &BindGroupEntries::sequential((
                gi_gpu.gi_params.as_entire_buffer_binding(),
                first_hit_data.as_entire_buffer_binding(),
                first_hit_absorption.as_entire_buffer_binding(),
                gi_gpu.valid_samples.as_entire_buffer_binding(),
                gi_gpu.invalid_samples.as_entire_buffer_binding(),
                gi_gpu.sample_counts.as_entire_buffer_binding(),
                final_color.as_entire_buffer_binding(),
                gi_gpu.ray_queue.as_entire_buffer_binding(),
                taa_gpu.camera_history.as_entire_buffer_binding(),
            )),
        );
        // `sample_refine_bind_group` (`@group(0)` for all 5 sample-refine
        // passes — `09-design-b.md` §8.2). 11 bindings, matching
        // `pipelines.sample_refine_layout` order exactly. `taa_dist_min_max` is
        // the zero-cleared `TaaGpu` buffer until Batch 6 wires `ReprojectOld`'s
        // write — the sample-refine validity test rejects everything until then
        // (correct-but-empty, `09-design-b.md` §11 Batch 4 step 13).
        let sample_refine_bind_group = render_device.create_bind_group(
            "naadf_sample_refine_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.sample_refine_layout),
            &BindGroupEntries::sequential((
                gi_gpu.gi_params.as_entire_buffer_binding(),
                first_hit_data.as_entire_buffer_binding(),
                gi_gpu.bucket_info.as_entire_buffer_binding(),
                gi_gpu.valid_samples.as_entire_buffer_binding(),
                gi_gpu.valid_samples_refined.as_entire_buffer_binding(),
                gi_gpu.valid_samples_compressed.as_entire_buffer_binding(),
                gi_gpu.invalid_samples.as_entire_buffer_binding(),
                gi_gpu.sample_counts.as_entire_buffer_binding(),
                taa_gpu.taa_dist_min_max.as_entire_buffer_binding(),
                gi_gpu.ray_queue_indirect.as_entire_buffer_binding(),
                taa_gpu.camera_history.as_entire_buffer_binding(),
            )),
        );
        // `sample_refine_dispatch_bind_group` (`@group(1)`, `compute_valid_history`
        // only) — `valid_dispatch` + `invalid_dispatch`. The wgpu split: these
        // are written here and consumed as `dispatch_workgroups_indirect`
        // sources by the count passes, so they cannot be bound rw in the shared
        // `@group(0)`.
        let sample_refine_dispatch_bind_group = render_device.create_bind_group(
            "naadf_sample_refine_dispatch_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.sample_refine_dispatch_layout),
            &BindGroupEntries::sequential((
                gi_gpu.valid_dispatch.as_entire_buffer_binding(),
                gi_gpu.invalid_dispatch.as_entire_buffer_binding(),
            )),
        );
        // `spatial_resampling_bind_group` (`@group(1)` for `renderSpatialResampling`
        // — `09-design-b.md` §8.3). 8 bindings, matching
        // `pipelines.spatial_resampling_layout` order exactly. Mixes `GiGpu` +
        // `FrameGpu` (`first_hit_data` / `first_hit_absorption` / `final_color`)
        // + `TaaGpu` (`taa_sample_accum`). CROSS-BATCH (`09-design-b.md` §11
        // Batch 5): `bucket_info` / `valid_samples_compressed` are
        // correct-but-empty until Batch 6 wires `taa_dist_min_max` — the
        // 12-tap reservoir loop yields nothing pre-B6, but the sun sample is
        // independent, so direct-sun bounce light still lands in `final_color`.
        let spatial_resampling_bind_group = render_device.create_bind_group(
            "naadf_spatial_resampling_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.spatial_resampling_layout),
            &BindGroupEntries::sequential((
                gi_gpu.gi_params.as_entire_buffer_binding(),
                first_hit_data.as_entire_buffer_binding(),
                first_hit_absorption.as_entire_buffer_binding(),
                gi_gpu.bucket_info.as_entire_buffer_binding(),
                gi_gpu.valid_samples_compressed.as_entire_buffer_binding(),
                taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
                final_color.as_entire_buffer_binding(),
                gi_gpu.denoise_preprocessed.as_entire_buffer_binding(),
            )),
        );
        // `denoise_bind_group` (`@group(0)` shared by both `renderDenoiseSplit`
        // passes — `09-design-b.md` §9.1). 5 bindings, matching
        // `pipelines.denoise_layout` order exactly. Mixes `GiGpu` + `FrameGpu`
        // (`first_hit_absorption` / `final_color`).
        let denoise_bind_group = render_device.create_bind_group(
            "naadf_denoise_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.denoise_layout),
            &BindGroupEntries::sequential((
                gi_gpu.gi_params.as_entire_buffer_binding(),
                first_hit_absorption.as_entire_buffer_binding(),
                gi_gpu.denoise_preprocessed.as_entire_buffer_binding(),
                gi_gpu.denoise_preprocessed_horizontal.as_entire_buffer_binding(),
                final_color.as_entire_buffer_binding(),
            )),
        );
        commands.insert_resource(GiBindGroups {
            ray_queue_bind_group,
            global_illum_bind_group,
            sample_refine_bind_group,
            sample_refine_dispatch_bind_group,
            spatial_resampling_bind_group,
            denoise_bind_group,
            pixel_count,
        });
    }

    commands.insert_resource(FrameGpu {
        camera: camera_buf,
        render_params: render_params_buf,
        first_hit_data,
        first_hit_absorption,
        final_color,
        pixel_count,
        bind_group,
        first_hit_atmosphere_bind_group,
        blit_bind_group,
        taa_reproject_bind_group,
        calc_new_taa_sample_bind_group,
    });
}
