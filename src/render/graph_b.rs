//! Phase-B render-graph node systems (`09-design-b.md` §2.1, §4).
//!
//! Phase B expands the Phase-A-2 three-node graph (`naadf_first_hit →
//! naadf_taa_reproject → naadf_final_blit`) into NAADF's full deferred GI
//! pipeline. The new nodes land here (rather than in `graph.rs`) to keep the
//! A-2 graph readable — `09-design-b.md` §2.1.
//!
//! As in Phase A/A-2, a "render-graph node" in Bevy 0.19 is just a `Core3d`-
//! schedule system that records commands via [`RenderContext`]; each node
//! wraps its work in a `time_span` for the HUD and skips silently until its
//! resources + pipeline exist.
//!
//! **Batch 1** lands only [`naadf_atmosphere_node`] — the atmosphere precompute,
//! the first node in NAADF's dispatch order (`WorldRenderBase.cs:205-206`). The
//! remaining ~10 Phase-B nodes arrive in Batches 2-6.

use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::render_resource::{ComputePassDescriptor, PipelineCache};
use bevy::render::renderer::RenderContext;

use crate::render::atmosphere::{AtmosphereGpu, ATMOSPHERE_TEX_SIZE, ATMOSPHERE_WORKGROUP_SIZE};
use crate::render::pipelines::NaadfPipelines;

/// Timing-span name for the atmosphere precompute pass — surfaces in the HUD as
/// `render/naadf_atmosphere/elapsed_gpu`.
pub const ATMOSPHERE_SPAN: &str = "naadf_atmosphere";

/// `Core3d` system: the NAADF atmosphere precompute compute pass
/// (`09-design-b.md` §4.3 / §9.2).
///
/// Faithful port of the C# `WorldRenderBase` `renderSky` dispatch
/// (`WorldRenderBase.cs:205-206`): one compute pass running
/// `naadf_atmosphere.wgsl`'s `precompute_atmosphere`, binding the single
/// `atmosphere_bind_group`. Each frame it writes one quarter of the octahedral
/// `atmosphere_comp` buffer (`renderAtmosphere.fx:12`), so the dispatch covers
/// `ceil((ATMOSPHERE_TEX_SIZE² / 4) / 64)` workgroups
/// (`WorldRenderBase.cs:206` — `(1024·1024/4 + 63)/64`).
///
/// Runs *first* in the Phase-B `Core3d` chain (NAADF's dispatch order —
/// `09-design-b.md` §4.2). Skips silently until `AtmosphereGpu` exists and the
/// precompute pipeline has finished compiling.
pub fn naadf_atmosphere_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    atmosphere_gpu: Option<Res<AtmosphereGpu>>,
) {
    let Some(atmosphere_gpu) = atmosphere_gpu else {
        return;
    };
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.atmosphere_pipeline)
    else {
        return;
    };

    // One quarter of the octahedral buffer per frame, 64 texels per workgroup.
    let texels_per_frame = (ATMOSPHERE_TEX_SIZE * ATMOSPHERE_TEX_SIZE) / 4;
    let workgroups = texels_per_frame.div_ceil(ATMOSPHERE_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, ATMOSPHERE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_atmosphere_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &atmosphere_gpu.bind_group, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}
