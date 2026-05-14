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

// === TEMPORARY STEP-8 INSTRUMENTATION — reverted before return ==============
use bevy::render::render_resource::{
    Buffer, BufferDescriptor, BufferUsages, MapMode, PollType,
};
use bevy::render::renderer::RenderDevice;

#[derive(Resource)]
pub struct TaaDebugReadback {
    pub staging: Buffer,
}

/// Temporary `Core3d` node: copy 8 bytes (`taa_sample_accum[center_pixel]`)
/// into a mappable staging buffer, run after the reproject node.
pub fn taa_debug_copy_node(
    mut render_context: RenderContext,
    frame_gpu: Option<Res<FrameGpu>>,
    taa_gpu: Option<Res<TaaGpu>>,
    render_device: Res<RenderDevice>,
    existing: Option<Res<TaaDebugReadback>>,
    mut commands: Commands,
) {
    let (Some(frame_gpu), Some(taa_gpu)) = (frame_gpu, taa_gpu) else {
        return;
    };
    let staging = match &existing {
        Some(r) => r.staging.clone(),
        None => {
            let s = render_device.create_buffer(&BufferDescriptor {
                label: Some("taa_debug_staging"),
                size: 8,
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            commands.insert_resource(TaaDebugReadback { staging: s.clone() });
            s
        }
    };
    // center pixel of a 1280x720-ish viewport — pick pixel_count/2 + 7 to land
    // somewhere on the grid geometry.
    let center = (frame_gpu.pixel_count / 2 + 7) as u64;
    let encoder = render_context.command_encoder();
    encoder.copy_buffer_to_buffer(&taa_gpu.taa_sample_accum, center * 8, &staging, 0, 8);
}

/// Temporary `Render`-schedule system: map the staging buffer, decode the
/// accumulated weight + RGB, and log them. Proves the TAA accumulation evolves
/// frame-to-frame.
pub fn taa_debug_readback_system(
    readback: Option<Res<TaaDebugReadback>>,
    render_device: Res<RenderDevice>,
) {
    let Some(readback) = readback else {
        return;
    };
    let slice = readback.staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(MapMode::Read, move |res| {
        let _ = tx.send(res);
    });
    let _ = render_device.poll(PollType::wait_indefinitely());
    if rx.recv().is_ok() {
        let data = slice.get_mapped_range();
        let x = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let y = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        // TEMP raw-integer-counter decode (matches taa.wgsl debug block):
        // .x = valid | dist_pass<<8 | screen_pass<<16 | accepted<<24
        // .y = u32(color_sum.a) | u32(first_hit_dist)<<16
        let valid = x & 0xFF;
        let dist_pass = (x >> 8) & 0xFF;
        let screen_pass = (x >> 16) & 0xFF;
        let accepted = (x >> 24) & 0xFF;
        let accepted_full = y & 0xFFFF;
        let first_hit_dist = y >> 16;
        info!(
            "TAA_DEBUG[center]: valid={valid} dist_pass={dist_pass} screen_pass={screen_pass} accepted={accepted} accepted_full={accepted_full} fhd={first_hit_dist} raw=({x:#010x},{y:#010x})"
        );
        let _ = half_to_f32(0);
        drop(data);
        readback.staging.unmap();
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let mant = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            // subnormal
            let mut e = -1i32;
            let mut m = mant;
            while (m & 0x400) == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x3FF;
            (sign << 31) | (((e + 127 + 1 - 15) as u32) << 23) | (m << 13)
        }
    } else if exp == 0x1F {
        (sign << 31) | (0xFF << 23) | (mant << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}
// === END TEMPORARY STEP-8 INSTRUMENTATION ===================================

/// Timing-span name for the first-hit pass — surfaces in the HUD as
/// `render/naadf_first_hit/elapsed_gpu`.
pub const FIRST_HIT_SPAN: &str = "naadf_first_hit";
/// Timing-span name for the TAA reproject pass — surfaces in the HUD as
/// `render/naadf_taa_reproject/elapsed_gpu`.
pub const TAA_REPROJECT_SPAN: &str = "naadf_taa_reproject";
/// Timing-span name for the final-blit pass — surfaces in the HUD as
/// `render/naadf_final_blit/elapsed_gpu`.
pub const FINAL_BLIT_SPAN: &str = "naadf_final_blit";

/// `Core3d` system: the NAADF first-hit compute pass.
///
/// Faithful port of the C# `WorldRenderAlbedo` first-hit dispatch: one compute
/// pass running `naadf_first_hit.wgsl`'s `calc_first_hit` over
/// `ceil(pixel_count / 64)` workgroups, binding `@group(0)` (world) +
/// `@group(1)` (frame) + `@group(2)` (the TAA sample ring). Writes
/// `first_hit_data` + `taa_sample_accum`, and — when `FLAG_IS_TAA` is set — one
/// `taa_samples` ring slot (`06-design-a2.md` §6).
///
/// Skips silently until the world + frame + TAA GPU resources exist and the
/// compute pipeline has finished compiling. The `@group(2)` bind group is
/// always bound (the shader's `if` guards the ring write), matching how the
/// Phase-A flags are handled at runtime rather than via pipeline variants.
pub fn naadf_first_hit_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    world_gpu: Option<Res<WorldGpu>>,
    frame_gpu: Option<Res<FrameGpu>>,
    taa_gpu: Option<Res<TaaGpu>>,
) {
    let (Some(world_gpu), Some(frame_gpu), Some(taa_gpu)) =
        (world_gpu, frame_gpu, taa_gpu)
    else {
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
        pass.set_bind_group(2, &taa_gpu.taa_first_hit_bind_group, &[]);
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
        info!("TAA_DEBUG reproject_node: missing taa_gpu/frame_gpu");
        return;
    };
    let _ = taa_gpu; // the bind group (in `FrameGpu`) carries the TAA buffers.
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.taa_reproject_pipeline)
    else {
        let st = pipeline_cache
            .get_compute_pipeline_state(pipelines.taa_reproject_pipeline);
        match st {
            bevy::render::render_resource::CachedPipelineState::Err(e) => {
                info!("TAA_DEBUG reproject pipeline ERR: {e}");
            }
            bevy::render::render_resource::CachedPipelineState::Queued => {
                info!("TAA_DEBUG reproject pipeline: Queued");
            }
            bevy::render::render_resource::CachedPipelineState::Creating(_) => {
                info!("TAA_DEBUG reproject pipeline: Creating");
            }
            bevy::render::render_resource::CachedPipelineState::Ok(_) => {
                info!("TAA_DEBUG reproject pipeline: Ok-but-not-compute?!");
            }
        }
        return;
    };
    info!("TAA_DEBUG reproject_node: DISPATCHING");

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
