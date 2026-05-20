//! Phase-C — runtime GPU producer dispatch (render-graph node).
//!
//! Owns the `naadf_gpu_producer_node` system that runs the W1 chunk_calc
//! chain + the W5 per-segment generator+chunk_calc chain ONE TIME per app
//! lifecycle against the production `WorldGpu` buffers. Gated by
//! `ConstructionGpu::gpu_producer_has_run`.
//!
//! Three-way branch ladder (see body):
//!   (a) `ModelDataRender` present → per-segment W5 chain (the C#
//!       `WorldData.GenerateWorld` loop).
//!   (b) `world_data_meta.dense_voxel_types` non-empty → chunk-calc-only.
//!   (c) else → CPU upload fallback (early-return).

use bevy::prelude::*;
use bevy::render::render_resource::{CommandEncoderDescriptor, PipelineCache};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue};

use super::{
    bounds_calc, chunk_calc, config, generator_model, ConstructionBindGroups, ConstructionGpu,
    ConstructionPipelines,
};

/// Phase-C followup #1 — runtime GPU producer dispatch (render-graph node).
///
/// Runs the chunk_calc chain (`calc_block_from_raw_data` → `compute_voxel_bounds`
/// → `compute_block_bounds`) ONE TIME against the production `WorldGpu`
/// buffers, on the first frame all dependencies are compiled + allocated.
/// One-shot, gated by `ConstructionGpu::gpu_producer_has_run`.
///
/// Lives in the `Core3d` chain BEFORE `naadf_bounds_compute_node` so the W3
/// bounds-init seed can read the chunks `.x` state the chain produces. Uses
/// `RenderContext::command_encoder()` so wgpu/Vulkan auto-inserts the
/// STORAGE-write → SAMPLED-read texture barrier between this node's writes
/// and the renderer's reads (`prepare_construction`'s separate-encoder
/// dispatch pattern would NOT propagate the writes across submits — see
/// the comment block in `prepare_construction`'s GPU-producer section).
///
/// Skipped when:
/// - `gpu_construction_enabled = false` (E4 CPU fallback).
/// - The pipelines have not compiled yet (re-tried next frame).
/// - The bind group is not yet built (re-tried next frame).
/// - The producer has already run.
#[allow(clippy::too_many_arguments)]
pub fn naadf_gpu_producer_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    construction_pipelines: Option<Res<ConstructionPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_gpu: Option<ResMut<ConstructionGpu>>,
    construction_config: Option<Res<config::ConstructionConfig>>,
    // `02f` rearch: moved from `ExtractedWorld` (deleted) to `WorldDataMeta`
    // — the long-lived metadata-only mirror populated once at startup.
    world_data_meta: Option<Res<crate::render::extract::WorldDataMeta>>,
    // vox-gpu-rewrite W5.3 — needed by the W5 branch to rewrite the per-segment
    // `GpuGeneratorModelParams` and `GpuConstructionParams` uniforms 512 times
    // (one rewrite per segment). `RenderContext::command_encoder` does not
    // expose `write_buffer`; the staging-belt write APIs live on the queue.
    render_queue: Res<RenderQueue>,
    // vox-gpu-rewrite W5.3-fix Stage 1 — needed by the W5 branch to create a
    // fresh `CommandEncoder` per segment. `wgpu::Queue::write_buffer`
    // schedules writes BEFORE the next submit; a per-segment submit ensures
    // each segment's params are visible to its OWN dispatches (rather than
    // all 512 dispatches seeing the last segment's params, the pre-fix bug).
    render_device: Res<RenderDevice>,
    // vox-gpu-rewrite W5.3 — drives the three-way branch ladder. Present iff a
    // `.vox` file was loaded into the fixed world via
    // `install_vox_in_fixed_world` (W5.1).
    model_data: Option<Res<crate::render::extract::ModelDataRender>>,
) {
    let Some(config) = construction_config else { return; };
    if !config.gpu_construction_enabled {
        return;
    }
    let Some(mut gpu) = construction_gpu else { return; };
    if gpu.gpu_producer_has_run {
        return;
    }
    let Some(construction_pipelines) = construction_pipelines else { return; };
    let Some(construction_bind_groups) = construction_bind_groups else { return; };

    // Common-prerequisite pipelines (Algorithm 1 + bounds chain). Both branches
    // of the ladder below need these; resolve up-front.
    let Some(world_bg) = construction_bind_groups.construction_world.as_ref() else {
        return;
    };
    let (Some(p_calc), Some(p_voxel), Some(p_block)) = (
        pipeline_cache
            .get_compute_pipeline(construction_pipelines.chunk_calc_pipeline_calc_block),
        pipeline_cache
            .get_compute_pipeline(construction_pipelines.chunk_calc_pipeline_voxel_bounds),
        pipeline_cache
            .get_compute_pipeline(construction_pipelines.chunk_calc_pipeline_block_bounds),
    ) else {
        return;
    };

    // vox-gpu-rewrite W5.3 — three-way producer gate ladder
    // (`docs/orchestrate/vox-gpu-rewrite/02-design.md` § "Three-way producer
    // gate ordering"):
    //   (a) `ModelDataRender` present + W5 deps ready → per-segment generator
    //       + chunk_calc dispatch chain (the C# `WorldData.GenerateWorld` loop
    //       at `WorldData.cs:120-156`).
    //   (b) else `world_data_meta` has `dense_voxel_types` non-empty → the
    //       existing chunk-calc-only branch (CPU-built segment_voxel_buffer
    //       drives the chunk_calc chain; the default-scene path).
    //   (c) else → CPU upload fallback (early-return; the renderer reads the
    //       pre-built CPU mirror via `prepare_world_gpu`).
    if let Some(model_data) = model_data.as_deref() {
        // === (a) W5 branch — per-segment generator + chunk_calc =============
        //
        // Mirrors C# `NAADF/NAADF/World/Data/WorldData.cs:120-156`:
        //   for (z = 0; z < segs.Z; ++z)
        //       for (y = 0; y < segs.Y; ++y)
        //           for (x = 0; x < segs.X; ++x):
        //               worldGenerator.CopyToChunkData(segmentPosInChunks, ...);
        //               CalculateChunkBlocks(segmentPosInChunks);
        // Then ONCE after the loop:
        //   ComputeVoxelBounds; ComputeBlockBounds.
        let Some(p_gen) = pipeline_cache
            .get_compute_pipeline(construction_pipelines.generator_model_pipeline)
        else {
            return;
        };
        let Some(gen_bg) = construction_bind_groups
            .construction_generator_model
            .as_ref()
        else {
            return;
        };
        let Some(params_buf) = gpu.model_data_params_buffer.as_ref() else {
            return;
        };
        let Some(bounds_params_buf) = gpu.bounds_params_buffer.as_ref() else {
            return;
        };

        let world_size_in_voxels = [
            crate::WORLD_SIZE_IN_VOXELS.x,
            crate::WORLD_SIZE_IN_VOXELS.y,
            crate::WORLD_SIZE_IN_VOXELS.z,
        ];
        // Per-segment chunk extent. C# `WorldData.cs:73,143-145` uses
        // `worldGenSegmentSizeInChunks = WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4
        // = 16` per axis. The dispatch shape (for both generator_model and
        // chunk_calc.calc_block_from_raw_data) is `[16, 16, 16]` workgroups —
        // one per chunk in the segment.
        let segment_chunks: u32 = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
        let group_size_in_chunks =
            [segment_chunks, segment_chunks, segment_chunks];

        // vox-gpu-rewrite W5.3-fix Stage 1 — wgpu's `Queue::write_buffer`
        // writes are scheduled BEFORE the next `Queue::submit`; the writes
        // do NOT interleave with dispatches recorded in the same
        // command-encoder. Pre-fix, 512 write_buffer calls + 512
        // encoder.dispatch calls into ONE encoder + ONE submit meant ALL
        // dispatches saw the LAST write (segment 511's params) — so every
        // segment's chunk_calc.calc_block dispatch wrote to chunk position
        // [60, 4, 60] (the last segment's chunk_offset) instead of its own
        // offset. Result: 511 of the 512 segments' worth of generator output
        // was discarded; only the last segment's chunks landed at the
        // intended world position.
        //
        // Fix: per-segment fresh encoder + submit. `render_queue.write_buffer`
        // is now ordered with the per-segment submit; each submit sees only
        // its own segment's writes; each dispatch uses the correct params.
        //
        // Trade-off: 512 submits/frame instead of 1. The W5 producer runs
        // ONCE per app lifecycle (gated by `gpu_producer_has_run`), so this
        // is a one-time cost at startup, not a per-frame cost. C# behaves
        // identically (`WorldData.cs:120-156` submits per segment via the
        // DirectX immediate context — each `ApplyCompute()` + `DispatchCompute()`
        // is independently submitted with the latest parameter values).
        //
        // The bounds chain AFTER the loop continues to use the
        // `render_context` encoder, since it does NOT need per-segment
        // params rewrites (the bounds chain reads from blocks/voxels, not
        // params.chunk_offset).
        let mut segment_count: u32 = 0;
        for sz in 0..crate::WORLD_SIZE_IN_SEGMENTS.z {
            for sy in 0..crate::WORLD_SIZE_IN_SEGMENTS.y {
                for sx in 0..crate::WORLD_SIZE_IN_SEGMENTS.x {
                    let group_offset_in_chunks = [
                        sx * segment_chunks,
                        sy * segment_chunks,
                        sz * segment_chunks,
                    ];

                    // 1) Per-segment generator_model uniform — mirrors C#
                    //    `WorldGeneratorModel.cs:32-60` `CopyToChunkData`:
                    //      modelSizeInChunks   ← ModelData.size_in_chunks
                    //      sizeInVoxels        ← WorldData.actualSizeInVoxels
                    //      groupOffsetInChunks ← segmentPos * segmentChunks
                    //      groupSizeInChunksX/Y← per-segment chunk extent (16)
                    let gen_params = generator_model::GpuGeneratorModelParams {
                        size_in_voxels: world_size_in_voxels,
                        _pad0: 0,
                        model_size_in_chunks: model_data.size_in_chunks,
                        _pad1: 0,
                        group_offset_in_chunks,
                        group_size_in_chunks_x: segment_chunks,
                        group_size_in_chunks_y: segment_chunks,
                        _pad2: 0,
                        _pad3: 0,
                        _pad4: 0,
                    };
                    render_queue.write_buffer(
                        params_buf,
                        0,
                        bytemuck::bytes_of(&gen_params),
                    );

                    // 2) Per-segment construction params — mirrors C#
                    //    `WorldData.cs:492-503` `CalculateChunkBlocks`.
                    //
                    // vox-gpu-rewrite W5.3-fix Stage 1.5 (2026-05-18) —
                    // `bound_group_queue_max_size` is preserved at
                    // `bound_group_count.max(1)` (not the stale `1` from
                    // pre-Stage-1.5). chunk_calc.wgsl does NOT read this
                    // field, but the post-loop `add_initial_groups`
                    // dispatch DOES (`bounds_calc.wgsl:239 / :257-260`):
                    // its workgroup gate
                    // `if group_index >= params.bound_group_queue_max_size
                    //  { return; }` short-circuited 32767 of 32768
                    // workgroups when this field was `1`, leaving the
                    // chunk-level AADF acceleration structure unbuilt.
                    // Effect was perf-only (rays step chunk-by-chunk
                    // instead of skipping at chunk granularity); diagnostic
                    // at `06-diagnostic-inversion.md:477-507`.
                    let bound_group_count = bounds_calc::bound_group_count_of([
                        crate::WORLD_SIZE_IN_CHUNKS.x,
                        crate::WORLD_SIZE_IN_CHUNKS.y,
                        crate::WORLD_SIZE_IN_CHUNKS.z,
                    ]);
                    let construction_params = crate::render::gpu_types::GpuConstructionParams {
                        size_in_chunks: [
                            crate::WORLD_SIZE_IN_CHUNKS.x,
                            crate::WORLD_SIZE_IN_CHUNKS.y,
                            crate::WORLD_SIZE_IN_CHUNKS.z,
                        ],
                        _pad0: 0,
                        group_size_in_groups:
                            bounds_calc::group_size_in_groups_of([
                                crate::WORLD_SIZE_IN_CHUNKS.x,
                                crate::WORLD_SIZE_IN_CHUNKS.y,
                                crate::WORLD_SIZE_IN_CHUNKS.z,
                            ]),
                        _pad1: 0,
                        bound_group_queue_max_size: bound_group_count.max(1),
                        hash_map_size: config.initial_hash_map_size,
                        segment_size_in_chunks: segment_chunks,
                        max_group_bound_dispatch: config.max_group_bound_dispatch,
                        chunk_offset: group_offset_in_chunks,
                        dispatch_offset: 0,
                        frame_index: 0,
                        changed_chunk_count: 0,
                        changed_block_count: 0,
                        changed_voxel_count: 0,
                    };
                    render_queue.write_buffer(
                        bounds_params_buf,
                        0,
                        bytemuck::bytes_of(&construction_params),
                    );

                    // 3 + 4) Generator → segment_voxel_buffer +
                    //    chunk_calc.calc_block_from_raw_data, per-segment
                    //    encoder + submit (see comment block above).
                    let mut seg_encoder = render_device.create_command_encoder(
                        &CommandEncoderDescriptor {
                            label: Some("naadf_w5_segment_encoder"),
                        },
                    );
                    generator_model::dispatch_generator_model_with_encoder(
                        &mut seg_encoder,
                        p_gen,
                        gen_bg,
                        group_size_in_chunks,
                    );
                    chunk_calc::dispatch_calc_block_from_raw_data_world_sized(
                        &mut seg_encoder,
                        p_calc,
                        world_bg,
                        group_size_in_chunks,
                    );
                    render_queue.submit([seg_encoder.finish()]);

                    segment_count += 1;
                }
            }
        }

        // The bounds chain dispatches on the shared `render_context`
        // encoder — no per-segment params needed, so it can share the
        // single submit at the end of the frame.
        let encoder = render_context.command_encoder();

        // After the per-segment loop, run the bounds chain ONCE (mirrors C#
        // `WorldData.cs:158-210`'s post-loop `ComputeVoxelBounds` +
        // `ComputeBlockBounds` invocations).
        //
        // vox-gpu-rewrite W5.3-fix Stage 1 — for the W5 path
        // `world_data_meta.{blocks,voxels}_cpu_len` are 0 (the W5 install
        // path leaves the CPU mirror empty), so we cannot derive the actual
        // GPU output count without a mid-frame CPU readback (not possible
        // inside a render-graph node). C# DOES readback the cursor each
        // segment (`WorldData.cs:148-151`) and dispatches the bounds chain
        // with `(voxelCount/64, 1, 1)` / `(blockCount/64, 1, 1)`; the Rust
        // port must cover the full-world worst case in one shot.
        //
        // PRE-FIX: the dispatch helpers took a 1D `workgroups: u32` and the
        // call site clamped to wgpu's 65535/axis cap. That under-dispatched
        // by 32×–2046× and left the AADF bits empty on most of the world.
        //
        // POST-FIX: the dispatch helpers
        // (`chunk_calc::dispatch_compute_voxel_bounds` /
        // `dispatch_compute_block_bounds`) repack the 1D count into a 3D
        // shape (`split_3d_dispatch`); the WGSL entry points flatten
        // `(group_id, num_workgroups)` back into a 1D `block_index` /
        // `chunk_index`. Extra workgroups past the actual count read zero
        // blocks (the buffers are sized to worst-case in
        // `render/prepare.rs::prepare_world_gpu`) and are correct no-ops.
        //
        // Upper bound derivation: assume every chunk is mixed and every
        // block is mixed (the absolute worst case for the AADF bounds
        // chain). For the 256×32×256 chunk fixed world:
        //   world_chunks         = 2,097,152
        //   max_blocks_u64       = 134,217,728   (chunks * 64)
        //   max_voxels_u64       = 4,294,967,296 (max_blocks * 32)
        //   voxel_workgroups raw = 134,217,729   (voxels/32 + 1; one wg/mixed block)
        //   block_workgroups raw =   2,097,153   (blocks/64 + 1; one wg/chunk)
        //
        // `split_3d_dispatch` repacks these to 3D shapes within the 65535
        // per-axis cap; the WGSL flattens.
        let world_chunks = crate::WORLD_SIZE_IN_CHUNKS.x
            * crate::WORLD_SIZE_IN_CHUNKS.y
            * crate::WORLD_SIZE_IN_CHUNKS.z;
        let max_blocks_u64 = (world_chunks as u64) * 64;
        let max_voxels_u64 = max_blocks_u64 * 32;
        let voxel_workgroups =
            ((max_voxels_u64 / 32 + 1).max(1)).min(u32::MAX as u64) as u32;
        let block_workgroups =
            ((max_blocks_u64 / 64 + 1).max(1)).min(u32::MAX as u64) as u32;
        let voxel_dispatch = chunk_calc::split_3d_dispatch(voxel_workgroups);
        let block_dispatch = chunk_calc::split_3d_dispatch(block_workgroups);

        // 2026-05-19 — bounds chain runs as a single dispatch on both
        // native and web. (An earlier wasm-only split-dispatch experiment
        // tested whether Dawn was losing invocations from the large 134M
        // dispatch; SSIM moved by 0.02 at 16M batches and noise at 1M
        // batches → confirmed the dispatch completes fine. The
        // `params.dispatch_offset` field stays plumbed in case it's
        // useful for future batching needs; both shaders read it as 0.)
        let _ = (&render_device, &render_queue, bounds_params_buf);
        chunk_calc::dispatch_compute_voxel_bounds(
            encoder,
            p_voxel,
            world_bg,
            voxel_workgroups,
        );
        chunk_calc::dispatch_compute_block_bounds(
            encoder,
            p_block,
            world_bg,
            block_workgroups,
        );

        gpu.gpu_producer_has_run = true;
        info!(
            "vox-gpu-rewrite W5 — per-segment GPU producer chain DISPATCHED \
             ({} segments × (generator_model + calc_block); bounds chain ×1; \
             voxel_workgroups={voxel_workgroups} dispatched as 3D {:?} \
             (= {} total workgroups, covers {} requested), \
             block_workgroups={block_workgroups} dispatched as 3D {:?} \
             (= {} total workgroups, covers {} requested)).",
            segment_count,
            voxel_dispatch,
            voxel_dispatch[0] as u64 * voxel_dispatch[1] as u64 * voxel_dispatch[2] as u64,
            voxel_workgroups,
            block_dispatch,
            block_dispatch[0] as u64 * block_dispatch[1] as u64 * block_dispatch[2] as u64,
            block_workgroups,
        );
        return;
    }

    // === (b) chunk-calc-only branch (existing behaviour) ====================
    let Some(meta) = world_data_meta else { return; };
    if meta.dense_voxel_types.is_empty() {
        // === (c) CPU upload fallback ========================================
        // Source scene didn't author a `DenseVolume` AND no `ModelData` —
        // GPU producer is unsafe to run (the segment_voxel_buffer the
        // chunk_calc dispatch needs cannot be built from CPU data, AND
        // there's no model to generate from). Fall back to the CPU upload
        // path (the renderer reads the pre-built CPU mirror via
        // `prepare_world_gpu`).
        return;
    }
    let size_in_chunks = [
        meta.size_in_chunks.x,
        meta.size_in_chunks.y,
        meta.size_in_chunks.z,
    ];
    // Upper-bound the bound dispatches from the CPU mirror's sizes (each
    // mixed-block produces 32 u32s of voxel data; each mixed-chunk produces
    // 64 u32s of block data — the GPU output sizes match the CPU oracle).
    let cpu_blocks = meta.blocks_cpu_len;
    let cpu_voxels = meta.voxels_cpu_len;
    let voxel_workgroups = (cpu_voxels / 32 + 1).max(1);
    let block_workgroups = (cpu_blocks / 64 + 1).max(1);

    let encoder = render_context.command_encoder();
    // Step 2: calc_block_from_raw_data — Algorithm 1. Dispatch shape = real
    // world extent in chunks (one workgroup per chunk; the workgroup's 64
    // threads each handle one of the 64 blocks per chunk).
    chunk_calc::dispatch_calc_block_from_raw_data_world_sized(
        encoder,
        p_calc,
        world_bg,
        size_in_chunks,
    );
    // Step 3: compute_voxel_bounds — one workgroup per mixed block.
    chunk_calc::dispatch_compute_voxel_bounds(
        encoder,
        p_voxel,
        world_bg,
        voxel_workgroups,
    );
    // Step 4: compute_block_bounds — one workgroup per mixed chunk.
    chunk_calc::dispatch_compute_block_bounds(
        encoder,
        p_block,
        world_bg,
        block_workgroups,
    );

    gpu.gpu_producer_has_run = true;
    info!(
        "phase-c followup#1 — GPU producer chain DISPATCHED (size_in_chunks={:?}, \
         voxel_workgroups={}, block_workgroups={}). \
         Algorithm 1 is now the runtime producer for chunks/blocks/voxels.",
        size_in_chunks, voxel_workgroups, block_workgroups
    );
}
