//! Render-graph node systems + edges — the Phase-A node set + the Phase-A-2
//! TAA node (`03-design.md` §5.1, `06-design-a2.md` §8).
//!
//! The graph is three compute/fragment passes, chained: [`naadf_first_hit_node`]
//! (a compute pass that raytraces the AADF world and writes `first_hit_data` +
//! `taa_sample_accum`, and — when `FLAG_IS_TAA` is set — one `taa_samples` ring
//! slot), then [`naadf_taa_reproject_node`] (a compute pass that reprojects up
//! to 16 frames of history into `taa_sample_accum` — gated on the runtime TAA
//! toggle), then [`naadf_final_blit_node`] (a fullscreen fragment pass that
//! tonemaps `taa_sample_accum` onto the view target). All run in the `Core3d`
//! `PostProcess` set, chained, before tonemapping (see `render::mod`).
//!
//! With the TAA toggle off, `naadf_taa_reproject_node` early-returns and the
//! graph is the original Phase-A two-pass `first-hit → final-blit` path,
//! bit-identical.
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

/// `Core3d` system: the NAADF TAA reproject + accumulation compute pass
/// (`06-design-a2.md` §8.1).
///
/// Faithful port of the C# `WorldRenderAlbedo` `ReprojectOld` dispatch: one
/// compute pass running `taa.wgsl`'s `reproject_old_samples` over
/// `ceil(pixel_count / 64)` workgroups, binding the single
/// `taa_reproject_bind_group`. Reads `first_hit_data` + `taa_samples` +
/// `camera_history` + `taa_params`, reads-modifies-writes `taa_sample_accum`.
///
/// Gated on `ExtractedTaaConfig.enabled` (mirrors `AppArgs.taa` —
/// `06-design-a2.md` §8.2): when TAA is off the node early-returns and
/// `taa_sample_accum` is left untouched (the first-hit pass wrote it, the final
/// blit reads it — exactly Phase A's `shaded_color` path, bit-identical).
///
/// Otherwise skips silently until the TAA + frame GPU resources exist and the
/// reproject pipeline has finished compiling (the Phase-A
/// `let Some(...) else { return };` pattern).
///
/// Phase B Batch 2: this node is temporarily OUT of the render-graph chain
/// (`09-design-b.md` §11 Batch 2 step 8) — the `base/` first-hit no longer
/// writes `taa_sample_accum` / `taa_samples`, so reprojection has nothing to
/// reproject until Batch 6 ports `CalcNewTaaSample`. Kept defined so Batch 6
/// only needs to re-add it to the chain.
#[allow(dead_code)]
pub fn naadf_taa_reproject_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    taa_config: Res<ExtractedTaaConfig>,
    taa_gpu: Option<Res<TaaGpu>>,
    frame_gpu: Option<Res<FrameGpu>>,
) {
    // Gate the dispatch on the runtime TAA toggle — with TAA off, leaving
    // `taa_sample_accum` untouched makes the result bit-identical to Phase A.
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

/// `Core3d` system: the NAADF final-blit fullscreen pass.
///
/// Faithful port of the C# `albedo/renderFinal.fx` — a fullscreen-triangle
/// fragment pass running `naadf_final.wgsl`'s `fragment` over the view target,
/// reading `shaded_color`, tonemapping, and writing the swapchain. The C#
/// `Cube`+PS trick becomes a standard Bevy fullscreen triangle
/// (`03-design.md` §5.4).
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
