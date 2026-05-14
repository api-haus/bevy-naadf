//! Render-graph node systems + edges — the Phase-A first-hit + final-blit
//! nodes + the Phase-A-2/B TAA nodes (`03-design.md` §5.1, `06-design-a2.md`
//! §8, `09-design-b.md` §4.2 / §5.8).
//!
//! This file holds the four nodes that are *shared structure* across phases:
//! [`naadf_first_hit_node`] (the Phase-B 4-plane-bounce first-hit compute
//! pass), [`naadf_taa_reproject_node`] (the `base/` `ReprojectOld` compute
//! pass), [`naadf_calc_new_taa_sample_node`] (the `base/` `CalcNewTaaSample`
//! compute pass), and [`naadf_final_blit_node`] (the `base/renderFinal`
//! fullscreen fragment pass). The other ~9 Phase-B GI nodes live in
//! `graph_b.rs`.
//!
//! Phase B Batch 6 (`09-design-b.md` §11 Batch 6): the `base/` TAA path is
//! rewired — `naadf_taa_reproject_node` (now the `base/` variant, writing
//! `taa_dist_min_max`) + `naadf_calc_new_taa_sample_node` (folding the denoised
//! GI `final_color` into the 16-deep `taa_samples` ring + `taa_sample_accum`)
//! are back in the `Core3d` chain at their `09-design-b.md` §4.2 positions, and
//! `naadf_final_blit_node` reads `taa_sample_accum` again (the Batch-2
//! temporary `final_color` seam is reverted). Both TAA nodes are gated on the
//! runtime TAA toggle (`ExtractedTaaConfig.enabled`).
//!
//! In Bevy 0.19's render API a "render-graph node" is just a system in the
//! `Core3d` schedule that records commands via [`RenderContext`] — there is
//! no node-trait boilerplate. Each node wraps its work in a `time_span` so the
//! HUD can show per-pass GPU timings (`render/naadf_first_hit/elapsed_gpu`,
//! `render/naadf_final_blit/elapsed_gpu`).

use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::render_resource::{
    ComputePassDescriptor, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor,
};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};

use crate::render::extract::ExtractedTaaConfig;
use crate::render::pipelines::{NaadfPipelines, FIRST_HIT_WORKGROUP_SIZE};
use crate::render::prepare::{FrameGpu, WorldGpu};
use crate::render::taa::TaaGpu;

/// Timing-span name for the first-hit pass — surfaces in the HUD as
/// `render/naadf_first_hit/elapsed_gpu`.
pub const FIRST_HIT_SPAN: &str = "naadf_first_hit";
/// Timing-span name for the TAA reproject pass — surfaces in the HUD as
/// `render/naadf_taa_reproject/elapsed_gpu`.
pub const TAA_REPROJECT_SPAN: &str = "naadf_taa_reproject";
/// Timing-span name for the `base/` `CalcNewTaaSample` pass (`09-design-b.md`
/// §4.10) — surfaces in the HUD as `render/naadf_calc_new_taa_sample/elapsed_gpu`.
pub const CALC_NEW_TAA_SAMPLE_SPAN: &str = "naadf_calc_new_taa_sample";
/// Timing-span name for the final-blit pass — surfaces in the HUD as
/// `render/naadf_final_blit/elapsed_gpu`.
pub const FINAL_BLIT_SPAN: &str = "naadf_final_blit";

/// `Core3d` system: the NAADF 4-plane-bounce first-hit compute pass
/// (`09-design-b.md` §6 — the Phase-B `base/renderFirstHit.fx` port).
///
/// Faithful port of the C# `WorldRenderBase` first-hit dispatch: one compute
/// pass running `naadf_first_hit.wgsl`'s `calc_first_hit` over
/// `ceil(pixel_count / 64)` workgroups, binding `@group(0)` (world) +
/// `@group(1)` (frame — now including `first_hit_absorption` + `final_color`) +
/// `@group(2)` (the read-only precomputed atmosphere). Writes `first_hit_data`
/// + `first_hit_absorption` + `final_color`.
///
/// Phase B Batch 2 restructure (`09-design-b.md` §6.3): the `@group(2)`
/// `taa_samples` ring binding is GONE — the `base/` first-hit does not write
/// the ring (that moves to `CalcNewTaaSample` in Batch 6). `@group(2)` is now
/// the read-only atmosphere (`applyAtmosphere` on a miss + `addLightForDirection`
/// along the atmosphere-interaction path). The `base/` first-hit also no
/// longer writes `taa_sample_accum`.
///
/// Skips silently until the world + frame GPU resources exist and the compute
/// pipeline has finished compiling.
pub fn naadf_first_hit_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    world_gpu: Option<Res<WorldGpu>>,
    frame_gpu: Option<Res<FrameGpu>>,
) {
    let (Some(world_gpu), Some(frame_gpu)) = (world_gpu, frame_gpu) else {
        return;
    };
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.first_hit_pipeline)
    else {
        return;
    };

    let workgroups =
        frame_gpu.pixel_count.div_ceil(FIRST_HIT_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, FIRST_HIT_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_first_hit_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &world_gpu.bind_group, &[]);
        pass.set_bind_group(1, &frame_gpu.bind_group, &[]);
        // `@group(2)` — the read-only precomputed atmosphere (Phase B Batch 2,
        // replaces the A-2 `taa_samples` ring group — `09-design-b.md` §6.3).
        pass.set_bind_group(2, &frame_gpu.first_hit_atmosphere_bind_group, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}

/// `Core3d` system: the NAADF `base/` TAA `ReprojectOld` compute pass
/// (`06-design-a2.md` §8.1, `09-design-b.md` §5.8.1).
///
/// Faithful port of the C# `WorldRenderBase` `ReprojectOld` dispatch: one
/// compute pass running `taa.wgsl`'s `reproject_old_samples` over
/// `ceil(pixel_count / 64)` workgroups, binding the single
/// `taa_reproject_bind_group`. Reads `first_hit_data` + `taa_samples` +
/// `camera_history` + `taa_params`; writes `taa_dist_min_max` (the `base/`
/// extra output — the per-pixel distance min/max + specular-normal validity
/// mask `renderSampleRefine` consumes) and OVERWRITES `taa_sample_accum` with
/// the reprojected-history sum.
///
/// Gated on `ExtractedTaaConfig.enabled` (mirrors `AppArgs.taa` —
/// `06-design-a2.md` §8.2). For Phase B, TAA is on by default (the A-2
/// done-bar) — the gate is kept for the runtime `D`-key toggle; with TAA off,
/// `taa_sample_accum` is never written and the final blit shows first-hit
/// albedo only (documented in `09-design-b.md` §4.10).
///
/// Skips silently until the TAA + frame GPU resources exist and the reproject
/// pipeline has finished compiling.
///
/// Phase B Batch 6: re-added to the `Core3d` chain at its `09-design-b.md`
/// §4.2 position (Batch 2 had it temporarily out — the `base/` first-hit no
/// longer writes `taa_sample_accum`/`taa_samples`).
pub fn naadf_taa_reproject_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    taa_config: Res<ExtractedTaaConfig>,
    taa_gpu: Option<Res<TaaGpu>>,
    frame_gpu: Option<Res<FrameGpu>>,
) {
    // Gate the dispatch on the runtime TAA toggle.
    if !taa_config.enabled {
        return;
    }
    let (Some(taa_gpu), Some(frame_gpu)) = (taa_gpu, frame_gpu) else {
        return;
    };
    let _ = taa_gpu; // the bind group (in `FrameGpu`) carries the TAA buffers.
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.taa_reproject_pipeline)
    else {
        return;
    };

    let workgroups =
        frame_gpu.pixel_count.div_ceil(FIRST_HIT_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, TAA_REPROJECT_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_taa_reproject_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &frame_gpu.taa_reproject_bind_group, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}

/// `Core3d` system: the NAADF `base/` TAA `CalcNewTaaSample` compute pass
/// (`09-design-b.md` §4.10 / §5.8.2).
///
/// Faithful port of the C# `WorldRenderBase` `CalcNewTaaSample` dispatch: one
/// compute pass running `taa.wgsl`'s `calc_new_taa_sample` over
/// `ceil(pixel_count / 64)` workgroups, binding the `calc_new_taa_sample`
/// `@group(1)` bind group (the pipeline layout is `[empty, …]` — see
/// `pipelines.rs`). Reconstructs the first-hit virtual path, reads the denoised
/// GI result from `final_color`, compresses it into one slot of the 16-deep
/// `taa_samples` ring, and folds the light into `taa_sample_accum` with
/// `sample_weight + 1`. This is the SOLE `taa_samples` writer in the `base/`
/// pipeline (the `base/` first-hit no longer writes it — `09-design-b.md`
/// §6.3).
///
/// Runs after the denoiser, before the final blit (NAADF's dispatch order —
/// `WorldRenderBase.cs:421-422`, `09-design-b.md` §4.2). Gated on
/// `ExtractedTaaConfig.enabled` — same as `naadf_taa_reproject_node`; with TAA
/// off the GI result is not folded into `taa_sample_accum` (the documented
/// `AppArgs.taa`-off behaviour — `09-design-b.md` §4.10).
///
/// Skips silently until the TAA + frame GPU resources exist and the
/// `calc_new_taa_sample` pipeline has finished compiling.
pub fn naadf_calc_new_taa_sample_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    taa_config: Res<ExtractedTaaConfig>,
    taa_gpu: Option<Res<TaaGpu>>,
    frame_gpu: Option<Res<FrameGpu>>,
) {
    if !taa_config.enabled {
        return;
    }
    let (Some(taa_gpu), Some(frame_gpu)) = (taa_gpu, frame_gpu) else {
        return;
    };
    let _ = taa_gpu; // the bind group (in `FrameGpu`) carries the TAA buffers.
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.calc_new_taa_sample_pipeline)
    else {
        return;
    };

    let workgroups =
        frame_gpu.pixel_count.div_ceil(FIRST_HIT_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, CALC_NEW_TAA_SAMPLE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_calc_new_taa_sample_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        // The `calc_new_taa_sample` pipeline layout is `[empty, …]` — its
        // bindings live on `@group(1)` so they do not collide with
        // `reproject_old_samples`'s `@group(0)` in the shared `taa.wgsl`
        // module. `@group(0)` is the entry-less placeholder.
        pass.set_bind_group(0, &pipelines.empty_bind_group, &[]);
        pass.set_bind_group(1, &frame_gpu.calc_new_taa_sample_bind_group, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}

/// `Core3d` system: the NAADF final-blit fullscreen pass.
///
/// Faithful port of the C# `base/renderFinal.fx` — a fullscreen-triangle
/// fragment pass running `naadf_final.wgsl`'s `fragment` over the view target,
/// reading `taa_sample_accum`, tonemapping (with the `tone_mapping_fac`
/// uniform term — the `base/` variant), and writing the swapchain. The C#
/// `Cube`+PS trick becomes a standard Bevy fullscreen triangle
/// (`03-design.md` §5.4). Phase B Batch 6 reverted the Batch-2 temporary
/// `final_color` blit source.
///
/// Writes the view target's main texture directly (a single non-blended
/// `Operations::default()` clear-then-write); the HUD's UI pass then draws on
/// top, and tonemapping/upscaling run after this in the `Core3d` schedule.
pub fn naadf_final_blit_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    frame_gpu: Option<Res<FrameGpu>>,
    view: ViewQuery<(&ViewTarget, &ExtractedView)>,
) {
    let Some(frame_gpu) = frame_gpu else {
        return;
    };
    let (view_target, extracted_view) = view.into_inner();
    // The blit pipeline is specialised per the view target's main-texture
    // format (`prepare_blit_pipeline` queues it); pick the matching variant.
    let Some(&pipeline_id) =
        pipelines.blit_pipelines.get(&extracted_view.target_format)
    else {
        return;
    };
    let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id) else {
        return;
    };

    // Write straight into the view target's current main texture. The blit is
    // an opaque fullscreen overwrite, so we do not need the ping-pong
    // `post_process_write` — a plain render pass into `main_texture_view` is
    // enough (and keeps the source/destination logic simple for Phase A).
    let main_view = view_target.main_texture_view().clone();

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, FINAL_BLIT_SPAN);
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("naadf_final_blit_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &main_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &frame_gpu.blit_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    time_span.end(render_context.command_encoder());
}
