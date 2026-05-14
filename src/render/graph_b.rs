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
use crate::render::gi::{GiBindGroups, GiGpu};
use crate::render::pipelines::{NaadfPipelines, FIRST_HIT_WORKGROUP_SIZE};
use crate::render::prepare::{FrameGpu, WorldGpu};

/// Timing-span name for the atmosphere precompute pass — surfaces in the HUD as
/// `render/naadf_atmosphere/elapsed_gpu`.
pub const ATMOSPHERE_SPAN: &str = "naadf_atmosphere";
/// Timing-span name for the ray-queue pass — surfaces in the HUD as
/// `render/naadf_ray_queue/elapsed_gpu`.
pub const RAY_QUEUE_SPAN: &str = "naadf_ray_queue";
/// Timing-span name for the global-illumination pass — surfaces in the HUD as
/// `render/naadf_global_illum/elapsed_gpu`.
pub const GLOBAL_ILLUM_SPAN: &str = "naadf_global_illum";

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

/// `Core3d` system: the NAADF `rayQueueCalc` adaptive ray-queue builder
/// (`09-design-b.md` §4.5 / §7).
///
/// Faithful port of the C# `WorldRenderBase` `rayQueueEffect` dispatch
/// (`WorldRenderBase.cs:285-288`): TWO dispatches in one node —
///   1. `RayQueue` (`calc_ray_queue`, `[numthreads(64,1,1)]`) over
///      `ceil(pixel_count / 64)` workgroups — per-pixel `should_ray` adaptive
///      test, the inline group-shared prefix-counter, the queue write;
///   2. `RayQueueStore` (`calc_ray_queue_store`, `[numthreads(1,1,1)]`) over a
///      single workgroup — converts the raw queued-pixel count in
///      `ray_queue_indirect[0]` into the workgroup count `(v + 63) / 64` for
///      the indirect `naadf_global_illum` dispatch.
///
/// Both passes bind the single `ray_queue_bind_group` (`@group(0)`). The two
/// dispatches share one node because `RayQueueStore` reads what `RayQueue`
/// wrote — wgpu's automatic buffer barriers between the two `dispatch`
/// calls serialise them.
///
/// Skips silently until `GiGpu` + `GiBindGroups` exist and both pipelines have
/// finished compiling. Batch 3: this node produces `ray_queue` +
/// `ray_queue_indirect` — nothing reads them until `naadf_global_illum_node`.
pub fn naadf_ray_queue_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(gi_gpu), Some(gi_bind_groups)) = (gi_gpu, gi_bind_groups) else {
        return;
    };
    let (Some(ray_queue_pipeline), Some(ray_queue_store_pipeline)) = (
        pipeline_cache.get_compute_pipeline(pipelines.ray_queue_pipeline),
        pipeline_cache.get_compute_pipeline(pipelines.ray_queue_store_pipeline),
    ) else {
        return;
    };

    // `RayQueue` covers one thread per pixel; `RayQueueStore` is a single
    // `[numthreads(1,1,1)]` invocation (`WorldRenderBase.cs:286-288`).
    let workgroups = gi_gpu.pixel_count.div_ceil(FIRST_HIT_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, RAY_QUEUE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_ray_queue_pass"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &gi_bind_groups.ray_queue_bind_group, &[]);
        // Pass 1: `RayQueue`.
        pass.set_pipeline(ray_queue_pipeline);
        pass.dispatch_workgroups(workgroups, 1, 1);
        // Pass 2: `RayQueueStore` — same bind group, single workgroup.
        pass.set_pipeline(ray_queue_store_pipeline);
        pass.dispatch_workgroups(1, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}

/// `Core3d` system: the NAADF `renderGlobalIllum` secondary-ray tracer
/// (`09-design-b.md` §4.6 / §8.1).
///
/// Faithful port of the C# `WorldRenderBase` `globalIllumEffect` dispatch
/// (`WorldRenderBase.cs:322-323`): one compute pass running
/// `naadf_global_illum.wgsl`'s `calc_global_ilum`, dispatched **indirect** off
/// `ray_queue_indirect` — `rayQueueCalc`'s `RayQueueStore` pass wrote the
/// workgroup count into `ray_queue_indirect[0]`, so `globalIllum` launches one
/// thread per *queued* pixel and its cost scales with the ~0.25-spp adaptive
/// rate, not the screen.
///
/// Binds `@group(0)` world, `@group(1)` `global_illum_bind_group`, `@group(2)`
/// the entry-less placeholder (the `globalIllum` shader skips `@group(2)` —
/// `09-design-b.md` §8.1), `@group(3)` the read-only precomputed atmosphere
/// (`first_hit_atmosphere_bind_group`, shared with the first-hit pass — same
/// `atmosphere_read_layout`).
///
/// Skips silently until the world + frame + GI GPU resources exist and the
/// pipeline has finished compiling. Batch 3: this writes the GI sample lists
/// (`valid_samples` / `invalid_samples` / `sample_counts`) — nothing reads
/// them until Batch 4's `sampleRefine`, so the image is unchanged.
pub fn naadf_global_illum_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    world_gpu: Option<Res<WorldGpu>>,
    frame_gpu: Option<Res<FrameGpu>>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(world_gpu), Some(frame_gpu), Some(gi_gpu), Some(gi_bind_groups)) =
        (world_gpu, frame_gpu, gi_gpu, gi_bind_groups)
    else {
        return;
    };
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.global_illum_pipeline)
    else {
        return;
    };

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, GLOBAL_ILLUM_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_global_illum_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &world_gpu.bind_group, &[]);
        pass.set_bind_group(1, &gi_bind_groups.global_illum_bind_group, &[]);
        // `@group(2)` — the entry-less placeholder (the shader skips it).
        pass.set_bind_group(2, &pipelines.empty_bind_group, &[]);
        // `@group(3)` — the read-only precomputed atmosphere, shared with the
        // first-hit pass (`first_hit_atmosphere_bind_group` is built against
        // the same `atmosphere_read_layout`).
        pass.set_bind_group(3, &frame_gpu.first_hit_atmosphere_bind_group, &[]);
        // Indirect dispatch off `ray_queue_indirect` — the workgroup count
        // `RayQueueStore` wrote into element `[0]` (`WorldRenderBase.cs:323`).
        pass.dispatch_workgroups_indirect(&gi_gpu.ray_queue_indirect, 0);
    }
    time_span.end(render_context.command_encoder());
}
