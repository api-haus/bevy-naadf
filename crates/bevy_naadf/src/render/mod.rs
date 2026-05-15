//! `NaadfRenderPlugin` — registers the Phase-A render pipelines, bind-group
//! layouts, render-world resources, and render-graph nodes (`03-design.md` §5).
//!
//! - [`extract`] — `ExtractSchedule`: `WorldData` / camera → render-world mirror.
//! - [`prepare`] — `Prepare`: upload buffers, build bind groups, camera uniforms.
//! - [`graph`] — render-graph node systems + edges.
//! - [`pipelines`] — compute / render pipeline descriptors + bind-group layouts.
//! - [`gpu_types`] — `#[repr(C)]` structs mirroring every WGSL struct / uniform.
//!
//! The Phase-A render graph is two passes (`03-design.md` §5.1): a first-hit
//! compute pass, then a fullscreen final-blit pass. Both run in the `Core3d`
//! schedule's `PostProcess` set (the first-hit pass does its own raytracing —
//! it does not depend on the main 3D pass output) and before `tonemapping`
//! (the HUD's UI pass then draws on top).

pub mod atmosphere;
pub mod color_compression;
// Phase C (`15-design-c.md` §1.1) — the construction sub-module. W0 lands the
// empty seam (`ConstructionGpu` / `ConstructionBindGroups` shells +
// `ConstructionPipelines` empty registry + the prepare + startup placeholders
// + `ConstructionPlugin` wiring); W1..W5 each extend it with their own
// pipelines, buffers and bind groups.
pub mod construction;
pub mod extract;
pub mod gi;
pub mod gpu_types;
pub mod graph;
pub mod graph_b;
pub mod pipelines;
pub mod prepare;
pub mod taa;

use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::Core3dSystems;
use bevy::prelude::*;
use bevy::render::{
    ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderSystems,
};

use atmosphere::prepare_atmosphere;
use extract::{
    extract_camera, extract_camera_history, extract_gi_config, extract_taa_config,
    extract_world, ExtractedCameraData, ExtractedCameraHistory, ExtractedGiConfig,
    ExtractedTaaConfig, ExtractedWorld,
};
use gi::prepare_gi;
// Phase B Batch 6 (`09-design-b.md` §11 Batch 6 steps 17-18): the `base/` TAA
// path is rewired — `naadf_taa_reproject_node` (now the `base/` variant) +
// `naadf_calc_new_taa_sample_node` are back in the chain at their §4.2
// positions.
use graph::{
    naadf_calc_new_taa_sample_node, naadf_final_blit_node, naadf_first_hit_node,
    naadf_taa_reproject_node,
};
use graph_b::{
    naadf_atmosphere_node, naadf_denoise_node, naadf_global_illum_node,
    naadf_ray_queue_node, naadf_sample_refine_buckets_node,
    naadf_sample_refine_clear_node, naadf_sample_refine_count_invalid_node,
    naadf_sample_refine_count_valid_node, naadf_sample_refine_valid_history_node,
    naadf_spatial_resampling_node,
};
// Phase-C W3 — the regime-2 background AADF queue node lives in the
// construction sub-module. Inserted before `naadf_atmosphere_node` in the
// `Core3d` chain per `15-design-c.md` §3.
use construction::bounds_calc::naadf_bounds_compute_node;
// Phase-C W2 — the regime-3 world-change node, inserted between
// `naadf_bounds_compute_node` (W3) and `naadf_entity_update_node` (W4) per
// `15-design-c.md` §3. Body is gated on
// `ConstructionEvents::has_pending_changes()` — a single bool check on
// no-edit frames.
use construction::world_change::naadf_world_change_node;
use pipelines::{prepare_blit_pipeline, NaadfPipelines};
use prepare::{prepare_frame_gpu, prepare_world_gpu};
use taa::{prepare_taa, TaaRingConfig};

// W4 — the regime-3 entity-update node (gated on
// `ConstructionConfig.entities_enabled`). The system body is a no-op when the
// gate is off; with entities off (the W4 default), the chain is functionally
// byte-identical to pre-W4. See `16-impl-c-W4.md` integration notes for the
// wave-3 dispatch-body wiring follow-up.
use construction::entity_update::naadf_entity_update_node;

/// Plugin: wires the Phase-A NAADF render path into the render sub-app.
pub struct NaadfRenderPlugin;

impl Plugin for NaadfRenderPlugin {
    fn build(&self, app: &mut App) {
        // The configured TAA sample-ring depth (`18-taa-fidelity.md` fix #3):
        // read once from the main-world `AppArgs` (inserted before this plugin
        // in `build_app`) and mirrored into the render sub-app as
        // `TaaRingConfig`. `NaadfPipelines::from_world` (built in
        // `RenderStartup`, after this `build`) reads it for the WGSL
        // `#{TAA_SAMPLE_RING_DEPTH}` shader-def; `prepare_taa` reads it for the
        // buffer sizing — one source of truth, both sides agree.
        let taa_ring_depth = app
            .world()
            .get_resource::<crate::AppArgs>()
            .map(|args| args.taa_ring_depth)
            .unwrap_or(crate::DEFAULT_TAA_RING_DEPTH);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .insert_resource(TaaRingConfig {
                depth: taa_ring_depth,
            })
            .init_resource::<ExtractedWorld>()
            .init_resource::<ExtractedCameraData>()
            .init_resource::<ExtractedCameraHistory>()
            .init_resource::<ExtractedTaaConfig>()
            .init_resource::<ExtractedGiConfig>()
            // Pipelines + bind-group layouts — `FromWorld`, built once in
            // `RenderStartup` (after the render device exists).
            .init_gpu_resource::<NaadfPipelines>()
            // Extract: main world -> render world mirror.
            .add_systems(
                ExtractSchedule,
                (
                    extract_world,
                    extract_camera,
                    extract_camera_history,
                    extract_taa_config,
                    extract_gi_config,
                ),
            )
            // Prepare: create + upload GPU resources, build bind groups,
            // queue the per-target-format blit pipeline variant. `prepare_taa`
            // creates `TaaGpu` here in `PrepareResources` so it exists before
            // `prepare_frame_gpu` (`PrepareBindGroups`) binds `taa_sample_accum`
            // (`06-design-a2.md` §5.5, §9.4).
            // `prepare_atmosphere` (Phase B) creates `AtmosphereGpu` in
            // `PrepareResources` alongside `prepare_world_gpu` / `prepare_taa`
            // — its bind group is self-contained (no `FrameGpu` / `TaaGpu`
            // dependency), so it does not need the `PrepareBindGroups` split.
            // `prepare_gi` (Phase B Batch 3) creates `GiGpu` in
            // `PrepareResources` alongside `prepare_world_gpu` / `prepare_taa` /
            // `prepare_atmosphere` — its buffers are self-contained; the *mixed*
            // GI bind groups (`GiBindGroups`, which reference `GiGpu` +
            // `FrameGpu` + `TaaGpu`) are built later in `prepare_frame_gpu`
            // (`PrepareBindGroups`), once all three resources exist
            // (`09-design-b.md` §10.3).
            .add_systems(
                Render,
                (
                    prepare_world_gpu,
                    prepare_taa,
                    prepare_atmosphere,
                    prepare_gi,
                    prepare_blit_pipeline,
                )
                    .in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Render,
                prepare_frame_gpu.in_set(RenderSystems::PrepareBindGroups),
            )
            // Render graph — Phase B Batch 2 (`09-design-b.md` §11 Batch 2
            // step 8): atmosphere precompute -> 4-plane first-hit -> final-blit
            // fullscreen, all in PostProcess (the first-hit pass raytraces
            // independently of the main 3D pass) and before tonemapping so the
            // HUD draws over. `.chain()` gives the render-graph edges and
            // wgpu's automatic buffer barriers serialise the shared-buffer
            // accesses (`atmosphere_comp`, `first_hit_data`, `final_color`).
            //
            // `naadf_atmosphere_node` runs first — NAADF's dispatch order runs
            // the atmosphere precompute before the first-hit pass
            // (`WorldRenderBase.cs:205-228`, `09-design-b.md` §4.2). Batch 2
            // wires its output into the first-hit pass (`@group(2)`).
            //
            // Phase B Batch 6 (`09-design-b.md` §11 Batch 6 / §4.2): the
            // `base/` TAA path is rewired into the chain.
            //   * `naadf_taa_reproject_node` (the `base/` `ReprojectOld`
            //     variant — writes `taa_dist_min_max`, the per-pixel distance
            //     min/max + specular-normal validity mask) runs right AFTER
            //     `naadf_first_hit_node`, BEFORE `naadf_sample_refine_clear_node`.
            //     Its `taa_dist_min_max` write un-blocks Batch 4's
            //     `renderSampleRefine` reprojection validity test ⇒
            //     `valid_samples_compressed` + `bucket_info` populate ⇒ Batch
            //     5's `renderSpatialResampling` reservoir loop yields output ⇒
            //     the GI bounce composites into `final_color`. THIS is the
            //     wiring that makes the bounce visible (`10-impl-b.md`
            //     B5-vs-B6 finding).
            //   * `naadf_calc_new_taa_sample_node` (the `base/` `CalcNewTaaSample`
            //     pass) runs right AFTER `naadf_denoise_node`, BEFORE
            //     `naadf_final_blit_node` — it folds the denoised GI
            //     `final_color` into the 16-deep `taa_samples` ring +
            //     `taa_sample_accum` (the SOLE `taa_samples` writer in the
            //     `base/` pipeline).
            //   * `naadf_final_blit_node` reads `taa_sample_accum` again — the
            //     Batch-2 temporary `final_color` blit seam is reverted
            //     (`prepare_frame_gpu` clears `FLAG_BLIT_FINAL_COLOR` + binds
            //     `taa_sample_accum` at the blit slot).
            //   Both TAA nodes are gated on the runtime TAA toggle
            //   (`ExtractedTaaConfig.enabled`).
            // Phase B Batch 3 (`09-design-b.md` §11 Batch 3 steps 10-11):
            // the chain gains `naadf_ray_queue_node` + `naadf_global_illum_node`
            // between the first-hit and the final blit — `rayQueueCalc` builds
            // the adaptive ~0.25-spp ray queue, `globalIllum` traces the
            // ≤3-bounce GI rays indirect off it. Both write GI buffers the
            // blit does NOT read, so the Batch-2 image is unchanged through
            // Batch 3 (the done-bar is "the passes dispatch clean", not "the
            // image changes" — the GI result is not composited until the
            // denoiser in Batch 5).
            //
            // Phase B Batch 4 (`09-design-b.md` §11 Batch 4 / §4.2): the 5
            // `renderSampleRefine` passes land as 5 separate `Core3d` node
            // systems — they interleave with `rayQueueCalc` / `globalIllum` in
            // NAADF's dispatch order, so they cannot be one node:
            //   * `naadf_sample_refine_clear_node` runs BEFORE
            //     `naadf_ray_queue_node` — it owns the in-shader per-frame
            //     `ray_queue_indirect[0]` reset (`renderSampleRefine.fx:39`,
            //     §7.3), which **replaces** Batch 3's CPU re-seed in
            //     `prepare_gi` (now deleted).
            //   * the other four (`valid_history` → `count_valid` →
            //     `count_invalid` → `buckets`) run AFTER `naadf_global_illum_node`
            //     — they consume the GI sample rings `globalIllum` filled and
            //     produce `valid_samples_compressed` + `bucket_info`. Nothing
            //     reads those until Batch 5's `spatialResampling`, so the image
            //     is unchanged through Batch 4 (the done-bar is "the 5 passes
            //     dispatch clean", not "the image changes").
            //   * `count_valid` / `count_invalid` dispatch INDIRECT off
            //     `valid_dispatch` / `invalid_dispatch`, written by
            //     `valid_history` (`WorldRenderBase.cs:356,359`).
            //   * CROSS-BATCH: `taa_dist_min_max` is the zero-cleared `TaaGpu`
            //     buffer until Batch 6 wires `ReprojectOld`'s write — the
            //     sample-refine reprojection validity test rejects everything
            //     until then (correct-but-empty — `09-design-b.md` §11 Batch 4
            //     step 13).
            //
            // Phase B Batch 5 (`09-design-b.md` §11 Batch 5 / §4.2): the GI
            // *consumers* land — `naadf_spatial_resampling_node` (Algorithm 2:
            // the 12-iteration neighbour-reservoir loop + the single visibility
            // ray + the sun sample) and `naadf_denoise_node` (the two separable
            // sparse-bilateral passes, gated on `ExtractedGiConfig.is_denoise`),
            // inserted AFTER `naadf_sample_refine_buckets_node`, BEFORE
            // `naadf_final_blit_node`. Both write `final_color` — and Batch-2's
            // temporary `final_color` blit is still in place, so the GI bounce
            // light becomes VISIBLE at end-of-Batch-5. The full indirect
            // reservoir-resampled bounce needs Batch 6's `taa_dist_min_max`
            // (the refine buffers are correct-but-empty pre-B6), but the
            // spatial pass's sun sample is independent — direct-sun bounce
            // light on diffuse surfaces lands at end-of-B5.
            // Phase-C construction nodes — W3 + W4 landed (`15-design-c.md`
            // §3, §1.7). The remaining slot is reserved for W2:
            //   * `naadf_bounds_compute_node` — W3 (regime-2, background
            //     AADF queue, `n_bounds_rounds` × {prepare + indirect compute}
            //     per frame). LANDED HERE in W3.
            //   * `naadf_world_change_node`   — W2 (regime-3, gated on
            //     `ConstructionEvents::has_pending_changes()`). TODO W2.
            //   * `naadf_entity_update_node`  — W4 (regime-3, gated on
            //     `ConstructionConfig.entities_enabled`). LANDED HERE in W4.
            // Insertion order: construction nodes live **before**
            // `naadf_atmosphere_node` (the first existing node) so
            // atmosphere / first-hit / GI see the up-to-date chunk state.
            //
            // W4's `naadf_entity_update_node` body is a gated no-op until the
            // wave-3 integration agent wires the dispatch (see
            // `16-impl-c-W4.md` "Integration notes for the merge agent");
            // until then, inserting it in the chain is functionally
            // byte-identical to the pre-W4 chain because the gate stays off.
            .add_systems(
                Core3d,
                (
                    naadf_bounds_compute_node,
                    naadf_world_change_node,
                    naadf_entity_update_node,
                    naadf_atmosphere_node,
                    naadf_first_hit_node,
                    naadf_taa_reproject_node,
                    naadf_sample_refine_clear_node,
                    naadf_ray_queue_node,
                    naadf_global_illum_node,
                    naadf_sample_refine_valid_history_node,
                    naadf_sample_refine_count_valid_node,
                    naadf_sample_refine_count_invalid_node,
                    naadf_sample_refine_buckets_node,
                    naadf_spatial_resampling_node,
                    naadf_denoise_node,
                    naadf_calc_new_taa_sample_node,
                    naadf_final_blit_node,
                )
                    .chain()
                    .in_set(Core3dSystems::PostProcess)
                    .before(tonemapping),
            );
    }
}
