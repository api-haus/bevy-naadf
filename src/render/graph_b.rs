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
/// Timing-span name shared by all 5 `renderSampleRefine` passes (`09-design-b.md`
/// §4.7 — "one span is cleaner; designer's call, one span recommended"). The 5
/// passes are 5 separate `Core3d` node systems (they interleave with
/// `rayQueueCalc` / `globalIllum` in NAADF's dispatch order), but they share
/// one HUD line + one node-dispatch-check entry.
pub const SAMPLE_REFINE_SPAN: &str = "naadf_sample_refine";

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

/// `Core3d` system: `renderSampleRefine` pass 1 — `clear_buckets_and_calc_mask`
/// (`09-design-b.md` §4.7 / §8.2 / §7.3).
///
/// Faithful port of `WorldRenderBase.cs:272-273` (`sampleRefineEffect`,
/// `ClearBucketsAndCalcMask` pass): one compute dispatch over
/// `ceil(bucket_count / 64)` workgroups. Lane 0 does the per-frame reset of
/// `ray_queue_indirect[0]` + `sample_counts[3+accumIndex]` (the in-shader reset
/// that **replaces** Batch 3's CPU re-seed in `prepare_gi`); each bucket lane
/// then scans its 8×8 pixel region's `first_hit_data` into the bucket's
/// normal-mask + min/max distance.
///
/// **Runs BEFORE `naadf_ray_queue_node`** in the §4.2 chain — it owns the
/// per-frame `ray_queue_indirect[0]` reset that `calcRayQueue` then `atomicAdd`s
/// into (`09-design-b.md` §7.3, `renderSampleRefine.fx:39`).
pub fn naadf_sample_refine_clear_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(gi_gpu), Some(gi_bind_groups)) = (gi_gpu, gi_bind_groups) else {
        return;
    };
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.sample_refine_clear_pipeline)
    else {
        return;
    };

    // `ceil(bucket_count / 64)` workgroups (`WorldRenderBase.cs:273`).
    let workgroups = gi_gpu.bucket_count.div_ceil(FIRST_HIT_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, SAMPLE_REFINE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_sample_refine_clear_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}

/// `Core3d` system: `renderSampleRefine` pass 2 — `compute_valid_history`
/// (`09-design-b.md` §4.7 / §8.2).
///
/// Faithful port of `WorldRenderBase.cs:352-353` (`ValidHistory` pass): a
/// single `[numthreads(1,1,1)]` dispatch. Walks the 128-frame `sample_counts`
/// ring back from `accum_index`, writes the ring write cursors / totals /
/// `findCoprime` shuffle seeds, and writes the two indirect-dispatch arg
/// buffers (`valid_dispatch` / `invalid_dispatch`) that the next two passes
/// dispatch off.
pub fn naadf_sample_refine_valid_history_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(_gi_gpu), Some(gi_bind_groups)) = (gi_gpu, gi_bind_groups) else {
        return;
    };
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.sample_refine_valid_history_pipeline)
    else {
        return;
    };

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, SAMPLE_REFINE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_sample_refine_valid_history_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[]);
        // `@group(1)` — `valid_dispatch` / `invalid_dispatch`, written here only
        // (the wgpu indirect-vs-storage split — `09-design-b.md` §8.2).
        pass.set_bind_group(1, &gi_bind_groups.sample_refine_dispatch_bind_group, &[]);
        // `[numthreads(1,1,1)]` — a single workgroup (`WorldRenderBase.cs:353`).
        pass.dispatch_workgroups(1, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}

/// `Core3d` system: `renderSampleRefine` pass 3 — `count_valid_data_and_refine`
/// (`09-design-b.md` §4.7 / §8.2).
///
/// Faithful port of `WorldRenderBase.cs:355-356` (`CountValidAndRefine` pass):
/// dispatched **indirect** off `valid_dispatch` — `compute_valid_history` wrote
/// the workgroup count into `valid_dispatch[0]`. Reprojects each lit sample in
/// the temporal ring into the 8×8 bucket grid and writes `valid_samples_refined`.
pub fn naadf_sample_refine_count_valid_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(gi_gpu), Some(gi_bind_groups)) = (gi_gpu, gi_bind_groups) else {
        return;
    };
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.sample_refine_count_valid_pipeline)
    else {
        return;
    };

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, SAMPLE_REFINE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_sample_refine_count_valid_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[]);
        // Indirect dispatch off `valid_dispatch` (`WorldRenderBase.cs:356`).
        pass.dispatch_workgroups_indirect(&gi_gpu.valid_dispatch, 0);
    }
    time_span.end(render_context.command_encoder());
}

/// `Core3d` system: `renderSampleRefine` pass 4 — `count_invalid_data`
/// (`09-design-b.md` §4.7 / §8.2).
///
/// Faithful port of `WorldRenderBase.cs:358-359` (`CountInvalid` pass):
/// dispatched **indirect** off `invalid_dispatch`. The same reprojection as
/// `count_valid_data_and_refine` for unlit samples — it only `atomicAdd`s the
/// bucket's invalid count, no sample stored.
pub fn naadf_sample_refine_count_invalid_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(gi_gpu), Some(gi_bind_groups)) = (gi_gpu, gi_bind_groups) else {
        return;
    };
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.sample_refine_count_invalid_pipeline)
    else {
        return;
    };

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, SAMPLE_REFINE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_sample_refine_count_invalid_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[]);
        // Indirect dispatch off `invalid_dispatch` (`WorldRenderBase.cs:359`).
        pass.dispatch_workgroups_indirect(&gi_gpu.invalid_dispatch, 0);
    }
    time_span.end(render_context.command_encoder());
}

/// `Core3d` system: `renderSampleRefine` pass 5 — `refine_buckets`
/// (`09-design-b.md` §4.7 / §8.2).
///
/// Faithful port of `WorldRenderBase.cs:361-362` (`RefineBuckets` pass): one
/// compute dispatch over `ceil(bucket_count / 64)` workgroups — the
/// `COLOR_DIF_PROB` brightness-leveling, writing ≤8 survivors per bucket into
/// `valid_samples_compressed` and packing the bucket header into `bucket_info`.
///
/// Batch 4: `valid_samples_compressed` is written but not yet read by anything
/// — `spatialResampling` (Batch 5) is its first consumer, so the image is
/// unchanged this batch.
pub fn naadf_sample_refine_buckets_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<NaadfPipelines>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(gi_gpu), Some(gi_bind_groups)) = (gi_gpu, gi_bind_groups) else {
        return;
    };
    let Some(pipeline) =
        pipeline_cache.get_compute_pipeline(pipelines.sample_refine_buckets_pipeline)
    else {
        return;
    };

    // `ceil(bucket_count / 64)` workgroups (`WorldRenderBase.cs:362`).
    let workgroups = gi_gpu.bucket_count.div_ceil(FIRST_HIT_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, SAMPLE_REFINE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_sample_refine_buckets_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}
