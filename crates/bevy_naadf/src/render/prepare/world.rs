//! `prepare/world.rs` — build-once world GPU resources + W4 bind-group rebuild.
//!
//! Houses [`prepare_world_gpu`], the `RenderSystems::PrepareResources` system
//! that creates the world GPU resources (`chunks`/`blocks`/`voxels`/
//! `voxel_types`/`world_meta`/the W4 placeholder buffers) ONCE from the
//! [`WorldGpuStaging`] hand-off, then handles the
//! [`VoxelTypesRefresh`]-driven focused-refresh path (web-vox-color-divergence
//! 2026-05-18). Also exposes [`rebuild_world_bind_group_with_entities`] — the
//! D5 entities-on rebuild hook that swaps the placeholder buffers for the
//! production W4 buffers (per the D4 architect's §3.2 seam tightener).
//!
//! Split out of the original `render/prepare.rs` per the codebase-tightening
//! D4 architect's Step 3 — pure structural relocation, no behaviour change.

use bevy::math::Vec3;
use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, Buffer, BufferDescriptor, BufferUsages, PipelineCache,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use crate::render::extract::{VoxelTypesRefresh, WorldGpuStaging};
use crate::render::gpu_types::{GpuVoxelType, GpuWorldMeta};
use crate::render::pipelines::NaadfPipelines;
use crate::world::buffer::GrowableBuffer;

use super::{FrameGpu, WorldGpu, W2_BUFFER_HEADROOM_MUL};

/// `RenderSystems::PrepareResources` system: create the world GPU resources
/// **once** from the [`WorldGpuStaging`] hand-off, build the world bind
/// group, then consume + drop the staging resource (`02f` rearch).
///
/// Build-once (`02f` Decision 3): no dirty flag, no re-upload path. The
/// `existing.is_some()` gate keeps this a true no-op on every frame after
/// the first. The per-edit upload path is the W2 delta chain
/// (`naadf_world_change_node`), NOT this system; this system's GPU buffer
/// allocation includes [`W2_BUFFER_HEADROOM_MUL`] headroom to accommodate
/// the W2 dispatch's atomic-cursor appends without per-frame realloc.
// Bevy systems legitimately exceed clippy's 7-argument ceiling once
// `web-vox-color-divergence` (2026-05-18) added `voxel_types_refresh` for the
// focused-refresh path.
#[allow(clippy::too_many_arguments)]
pub fn prepare_world_gpu(
    mut commands: Commands,
    staging: Option<Res<WorldGpuStaging>>,
    mut existing: Option<ResMut<WorldGpu>>,
    // web-vox-color-divergence (2026-05-18) — focused-refresh hand-off from
    // `stage_world_gpu_buildonce`. When `WorldGpu` is `Some` AND this is
    // `Some`, the system re-uploads the palette to GPU, rebuilds
    // `WorldGpu.bind_group`, removes `FrameGpu` so `prepare_frame_gpu` re-
    // creates its bind groups (including `calc_new_taa_sample_bind_group`
    // which binds `voxel_types`), and drops `VoxelTypesRefresh`.
    voxel_types_refresh: Option<Res<VoxelTypesRefresh>>,
    pipelines: Res<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    // Phase-C followup #1 — when `gpu_construction_enabled = true` the
    // chunks/blocks/voxels buffer **contents** are produced by the GPU
    // dispatch chain in `prepare_construction`; we still allocate the GPU
    // buffers here (the production renderer reads them through `WorldGpu`),
    // but we skip uploading the CPU-built data. The runtime GPU producer
    // writes Algorithm 1 outputs into those buffers on the next system
    // (after `prepare_world_gpu`).
    construction_config: Option<Res<crate::render::construction::ConstructionConfig>>,
) {
    // Build-once: WorldGpu already exists → this system is forever done EXCEPT
    // when `VoxelTypesRefresh` is pending (focused-refresh path, web-vox-color-
    // divergence 2026-05-18). The refresh branch re-uploads the palette,
    // rebuilds `WorldGpu.bind_group`, and removes `FrameGpu` to invalidate the
    // downstream `calc_new_taa_sample_bind_group` that binds `voxel_types`.
    if let Some(world_gpu) = existing.as_mut() {
        if let Some(refresh) = voxel_types_refresh.as_deref() {
            // ---- focused-refresh body (web-vox-color-divergence Step 5) ----
            //
            // 1. Re-pack the new palette to `Vec<GpuVoxelType>` (mirrors the
            //    build-once site at `:380-388`).
            let voxel_types_data: Vec<GpuVoxelType> = if refresh.types.is_empty() {
                vec![GpuVoxelType { data: [0; 4] }]
            } else {
                refresh.types.iter().map(GpuVoxelType::from_voxel_type).collect()
            };

            // 2. Emit a `debug!` log so a future `RUST_LOG=bevy_naadf=debug`
            //    run can confirm the refresh path fires exactly once per
            //    palette change. Distinguishable from the build-once upload by
            //    the `(refresh)` tag.
            {
                let preview: Vec<[u32; 4]> = voxel_types_data
                    .iter()
                    .take(5)
                    .map(|e| e.data)
                    .collect();
                debug!(
                    "[palette-upload] (refresh) prepare_world_gpu re-uploading \
                     voxel_types to GPU (palette_len={}, first_5_raw={:?})",
                    voxel_types_data.len(),
                    preview,
                );
            }

            // 3. Upload the new palette. `GrowableBuffer::upload_all` calls
            //    `reserve_discard` which reallocates if the new length exceeds
            //    the current capacity (the 13 → 257 default→Oasis transition
            //    on web), and writes from offset 0 — wholesale replacement,
            //    not append.
            world_gpu.voxel_types.upload_all(
                &voxel_types_data,
                &render_device,
                &render_queue,
            );

            // 4. Rebuild `WorldGpu.bind_group` with the (possibly new)
            //    `voxel_types.buffer()` handle. SAFETY (schedule ordering):
            //    this runs in `RenderSystems::PrepareResources`; the
            //    consumer `prepare_frame_gpu` runs later in
            //    `RenderSystems::PrepareBindGroups`, so the rebuilt
            //    `bind_group` is the one downstream readers see.
            //    The construction-side bind groups
            //    (`ConstructionBindGroups`) DO NOT bind `voxel_types` —
            //    verified by grep: every `world_gpu.` reference in
            //    `crates/bevy_naadf/src/render/construction/mod.rs` is
            //    `chunks_buffer`/`blocks`/`voxels`/`chunks_size_in_chunks`,
            //    never `voxel_types`. So construction-side bind groups are
            //    unaffected by this refresh.
            let new_bind_group = render_device.create_bind_group(
                "naadf_world_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipelines.world_layout),
                &BindGroupEntries::sequential((
                    world_gpu.chunks_buffer.as_entire_buffer_binding(),
                    world_gpu.blocks.buffer().as_entire_buffer_binding(),
                    world_gpu.voxels.buffer().as_entire_buffer_binding(),
                    world_gpu.voxel_types.buffer().as_entire_buffer_binding(),
                    world_gpu.world_meta.as_entire_buffer_binding(),
                    world_gpu
                        .entity_chunk_instances_placeholder
                        .as_entire_buffer_binding(),
                    world_gpu
                        .entity_voxel_data_placeholder
                        .as_entire_buffer_binding(),
                    world_gpu
                        .entity_instances_history_placeholder
                        .as_entire_buffer_binding(),
                )),
            );
            world_gpu.bind_group = new_bind_group;

            // 5. Remove `FrameGpu` so `prepare_frame_gpu` rebuilds all of its
            //    bind groups, including `calc_new_taa_sample_bind_group` which
            //    binds `world_gpu.voxel_types.buffer()` at slot 3
            //    (`prepare/frame.rs`). Without this, the cached TAA bind
            //    group would still reference the OLD buffer handle if
            //    `upload_all` reallocated. SAFETY:
            //    `commands.remove_resource::<FrameGpu>()` is queued and
            //    flushes at the `PrepareResources` → `PrepareBindGroups`
            //    set boundary, so `prepare_frame_gpu` sees the removal on
            //    its next run and falls through its `existing.is_none()`
            //    path that rebuilds everything.
            //    Cost: a one-shot per-pixel storage buffer rebuild
            //    (`first_hit_data` + `first_hit_absorption` + `final_color` +
            //    TAA accumulators); acceptable for ≤2 palette refreshes per
            //    app lifetime.
            commands.remove_resource::<FrameGpu>();

            // 6. Single-use: drop the refresh resource so the next frame's
            //    `prepare_world_gpu` runs in steady-state (the
            //    `existing.is_some() && voxel_types_refresh.is_none()`
            //    branch — early-return).
            commands.remove_resource::<VoxelTypesRefresh>();
        }
        return;
    }
    // Waiting for the extract-stage hand-off — staging populated by
    // `stage_world_gpu_buildonce` once both `WorldData` and `VoxelTypes`
    // are present in the main world.
    let Some(extracted) = staging else {
        return;
    };
    // vox-gpu-rewrite W5.1 — the fixed-world `.vox` install path leaves
    // `chunks_cpu`/`blocks_cpu`/`voxels_cpu` EMPTY because the W5 GPU producer
    // chain populates the GPU-side `chunks_buffer`/blocks/voxels directly via
    // per-segment `generator_model` + `chunk_calc` dispatches. The fixed world
    // size is carried by `extracted.size_in_chunks` (non-zero); building the
    // GPU resources from a non-zero size with empty CPU data is correct.
    //
    // The legacy "setup_test_grid has not run" check used `chunks.is_empty()`
    // as a proxy for "size_in_chunks is meaningful". With W5.1 that proxy is
    // wrong (size_in_chunks can be `WORLD_SIZE_IN_CHUNKS` with empty
    // chunks_cpu); use the actual condition instead.
    if extracted.size_in_chunks == UVec3::ZERO {
        // `setup_test_grid` has not run / extracted yet.
        return;
    }

    let size = extracted.size_in_chunks.max(UVec3::ONE);

    // Phase-C followup #1 — pick the producer. `true` → skip the CPU upload
    // for chunks/blocks/voxels; GPU dispatch in `prepare_construction` will
    // populate them. The CPU mirror (`WorldData::*_cpu`) is still maintained
    // for the editing-path consumer + the bit-exact oracle (E4).
    let gpu_producer_enabled = construction_config
        .as_deref()
        .is_some_and(|c| c.gpu_construction_enabled);
    // Phase-C followup #1 — buffer sizing is GPU-producer-aware (the GPU
    // dispatch's outputs shift by +64 u32s for blocks / +32 u32s for voxels
    // because of the cursor seeds; we size with this headroom), but the
    // **upload-skip** lever stays off in this revision. (Historical note:
    // pre-migration this comment flagged a texture-aliasing hazard between
    // the construction-side `texture_storage_3d<rg32uint, read_write>` and
    // the renderer-side `texture_3d<u32>` bindings. Web-WebGPU migration
    // unified both bindings on a single `array<vec2<u32>>` storage buffer;
    // wgpu inserts STORAGE→STORAGE barriers automatically, so the hazard is
    // moot. The upload-skip lever stays off only because the CPU upload is
    // still the simplest correctness baseline.)
    let gpu_producer_skip_upload = false;
    let _ = gpu_producer_enabled;

    // --- chunk layer: an `array<vec2<u32>>` storage buffer, CPU-built today
    //     + GPU-writable for Phase C ------------------------------------------
    // Phase A built the chunks resource as a CPU-built, upload-only `R32Uint`
    // 3D texture. Phase C (`15-design-c.md` §1.4) makes construction the
    // GPU-side producer: `chunkCalc.fx` / `worldChange.fx` (W1, W2) /
    // `boundsCalc.fx` (W3) / `entityUpdate.fx` (W4) write the chunks resource
    // via `read_write` storage. **W4 widened the chunks pair to
    // `(state, entity_y)`** (`15-design-c.md` §1.7):
    //   .x = block-state pointer + AADF (W1/W2/W3, unchanged semantics)
    //   .y = entity pointer + counter (W4, `entityUpdate.wgsl` writes;
    //         `rayTracing.fxh:107` reads)
    // Every existing renderer-side reader takes `.x` explicitly so the widened
    // pair is a no-op for the no-entities path. The CPU upload pairs each
    // `R32Uint`-era u32 from `WorldData.chunks_cpu` with a zero `.y` channel
    // (entity pointer 0 = "no entities in this chunk").
    //
    // Web-WebGPU migration: the chunks resource is a storage **buffer**, not
    // a 3D texture, because WebGPU only permits `read_write` storage textures
    // on `r32{uint,sint,float}`. The flat layout is x-fastest:
    // `idx = z*sx*sy + y*sx + x` (matches `common.wgsl::flatten_index` and
    // `entity_handler.rs::chunk_index_to_pos`).
    let chunk_count = (size.x * size.y * size.z) as usize;
    let chunk_data_paired: Vec<[u32; 2]> = if gpu_producer_skip_upload {
        vec![[0u32, 0u32]; chunk_count]
    } else {
        let mut chunk_data_single = extracted.chunks.clone();
        chunk_data_single.resize(chunk_count, 0);
        let mut paired: Vec<[u32; 2]> = Vec::with_capacity(chunk_count);
        for c in chunk_data_single.iter().copied() {
            paired.push([c, 0u32]);
        }
        paired
    };
    let chunks_buffer_size = (chunk_count as u64) * 8; // 8 B per [u32; 2]
    let chunks_buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_chunks"),
        size: chunks_buffer_size,
        // STORAGE for the rw/ro storage bindings; COPY_DST for the seed +
        // future `write_buffer`-driven seeding (test fixtures + W2 staging);
        // COPY_SRC for the GPU-vs-CPU oracle readback path in
        // `construction/mod.rs::tests_w1` (build-once today; cheap to keep).
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    render_queue.write_buffer(
        &chunks_buffer,
        0,
        bytemuck::cast_slice(&chunk_data_paired),
    );

    // --- block / voxel / voxel-type growable buffers ------------------------
    // wgpu storage buffers can't be zero-length — ensure at least one element.
    // **Phase-C followup #1** — when the GPU producer is enabled, we still
    // size the buffers from the CPU mirror's known lengths (the GPU
    // dispatch's outputs match the CPU algorithm's output size up to the
    // GPU's `+64` cursor seed offset; the CPU mirror is the correct upper
    // bound), but skip the content upload. The GPU dispatch in
    // `prepare_construction` writes into these buffers.
    //
    // The size upper bound also covers the GPU cursor seeds: the GPU
    // `block_voxel_count[1]` seeds at `64`, so allocations need
    // `cpu_size + 64` headroom. Mirror what `validate_gpu_construction` uses.
    let cpu_blocks_len = extracted.blocks.len().max(1);
    let cpu_voxels_len = extracted.voxels.len().max(1);
    // `02f` R3 — W2-edit headroom. The W2 dispatch appends block/voxel
    // records past the build-time CPU mirror size; without per-edit
    // re-alloc, the build-time allocation must absorb stroke growth.
    // 2× the build-time size is the safe-for-typical-strokes baseline; a
    // future iteration adds dynamic growth on cursor-overflow detection.
    let blocks_with_headroom = (cpu_blocks_len as u64) * W2_BUFFER_HEADROOM_MUL;
    let voxels_with_headroom = (cpu_voxels_len as u64) * W2_BUFFER_HEADROOM_MUL;
    // vox-gpu-rewrite W5.3-fix Stage 1 — when the GPU producer is the source
    // of truth (the W5 `.vox` install path inserts `chunks_cpu / blocks_cpu
    // / voxels_cpu = Vec::new()` by design — `voxel/grid.rs:409-425`), the
    // CPU mirror's length is ZERO and cannot be used to size the GPU output
    // buffers. The original `((1 + 64) * 2).max(64) = 130 u32s` allocation
    // for blocks (520 B) plus the matching 66 u32s for voxels (264 B)
    // silently drops every atomic-cursor write past the first ~2 mixed
    // chunks (WebGPU spec §Storage Buffer Access: OOB writes are no-ops).
    // The result is `chunks` populated with state pointers that index into
    // unwritten regions of `blocks` / `voxels`; the renderer dereferences
    // those pointers, reads zero bytes, and treats every chunk as empty.
    //
    // Fix: when the CPU mirror is empty AND the GPU producer is enabled,
    // derive an upper bound from `size_in_chunks` instead. Mirrors C#
    // `WorldData.cs:77-79`'s up-front per-segment-cubic allocation, scaled
    // to cover the full-world cumulative cursor output (C# additionally
    // grows per segment via `SetNewMinCount` at `:148-151`; the Rust port
    // doesn't implement per-segment grow, so the static allocation must
    // cover the worst case up front).
    //
    // Sizing for the 256×32×256 chunk fixed world (the W5 install path's
    // target via `lib.rs:WORLD_SIZE_IN_CHUNKS`):
    //   chunk_count          = 2,097,152
    //   blocks_alloc_len     = chunk_count * 64    = 134,217,728 u32s = 512 MiB
    //   voxels_alloc_len     = chunk_count * 128   = 268,435,456 u32s =   1 GiB
    //
    // Both fit comfortably within wgpu's default `max_buffer_size`
    // (typically 2 GiB) on desktop Vulkan / Metal / DX12 backends. The
    // voxel cap uses `chunks * 128` (= chunks * 64 mixed blocks * 32
    // voxel-pair u32s / 16 sparsity factor) — empirically generous for
    // stamp-block layouts like Oasis (per `05-diagnostic.md:312-326`).
    let chunk_count_u64 = (size.x as u64) * (size.y as u64) * (size.z as u64);
    let blocks_alloc_len = if gpu_producer_enabled {
        // 64 = GPU `block_voxel_count[1]` cursor seed (`chunkCalc.fx`).
        // Still apply the W2 headroom on top of the producer's cursor seed.
        let from_cpu_with_headroom =
            ((cpu_blocks_len + 64) as u64) * W2_BUFFER_HEADROOM_MUL;
        // Upper bound on mixed-block count when no CPU mirror is available
        // (W5 .vox install path) — assume worst case `chunk_count * 64`
        // mixed blocks. Take the larger of (CPU-derived headroom) and
        // (chunks-derived upper bound) so the legacy non-empty-CPU path
        // still gets its 2× headroom while the W5 empty-CPU path is sized
        // to absorb the GPU producer's full cumulative cursor output.
        let from_chunks = chunk_count_u64.saturating_mul(64);
        from_chunks.max(from_cpu_with_headroom).max(64) as usize
    } else {
        blocks_with_headroom.max(1) as usize
    };
    let voxels_alloc_len = if gpu_producer_enabled {
        // 32 = GPU `block_voxel_count[0]` cursor seed.
        let from_cpu_with_headroom =
            ((cpu_voxels_len + 32) as u64) * W2_BUFFER_HEADROOM_MUL;
        // Realistic cap for the W5 empty-CPU path — see comment above.
        let from_chunks = chunk_count_u64.saturating_mul(128);
        from_chunks.max(from_cpu_with_headroom).max(32) as usize
    } else {
        voxels_with_headroom.max(1) as usize
    };

    let voxel_types_data: Vec<GpuVoxelType> = if extracted.voxel_types.is_empty() {
        vec![GpuVoxelType { data: [0; 4] }]
    } else {
        extracted
            .voxel_types
            .iter()
            .map(GpuVoxelType::from_voxel_type)
            .collect()
    };

    info!(
        "vox-gpu-rewrite W5.3-fix Stage 1 — prepare_world_gpu allocating \
         buffers: chunks={} u32-pairs ({} MiB), blocks={} u32s ({} MiB), \
         voxels={} u32s ({} MiB) (gpu_producer_enabled={}, \
         cpu_blocks_len={}, cpu_voxels_len={}, chunk_count={}).",
        chunk_count,
        (chunk_count as u64 * 8) / (1024 * 1024),
        blocks_alloc_len,
        (blocks_alloc_len as u64 * 4) / (1024 * 1024),
        voxels_alloc_len,
        (voxels_alloc_len as u64 * 4) / (1024 * 1024),
        gpu_producer_enabled,
        cpu_blocks_len,
        cpu_voxels_len,
        chunk_count_u64,
    );

    // vox-gpu-rewrite Q4 instrumentation (2026-05-18) — verify whether the
    // allocated `blocks`/`voxels`/`chunks` storage buffers exceed the device's
    // `max_storage_buffer_binding_size`. Per
    // `docs/orchestrate/vox-gpu-rewrite/14-diagnostic-type-decode.md` Q4, an
    // overrun causes WebGPU to silently truncate the binding to the limit;
    // every reader dereference past the truncation returns zero/garbage,
    // which is the leading hypothesis for the renderer's "random thousands"
    // voxel-type symptom in `oracle_gpu.png`.
    //
    // We use `error!` rather than `assert!` so the binary continues running —
    // we want the log to fire AND the gate to complete so the
    // before/after-fix comparison is well-defined. TODO: once Q4's fix lands
    // and the gate is permanently green, convert this to a hard `assert!`
    // so a future regression that re-introduces overruns is caught
    // immediately.
    {
        let limits = render_device.limits();
        let max_binding_bytes = limits.max_storage_buffer_binding_size as u64;
        let blocks_bytes = (blocks_alloc_len as u64) * 4;
        let voxels_bytes = (voxels_alloc_len as u64) * 4;
        let chunks_bytes = chunks_buffer_size;
        bevy::log::info!(
            "vox-gpu-rewrite Q4 instrumentation — device.limits().max_storage_buffer_binding_size = {} B ({} MiB); \
             allocated chunks = {} B ({} MiB), blocks = {} B ({} MiB), voxels = {} B ({} MiB).",
            max_binding_bytes,
            max_binding_bytes / (1024 * 1024),
            chunks_bytes,
            chunks_bytes / (1024 * 1024),
            blocks_bytes,
            blocks_bytes / (1024 * 1024),
            voxels_bytes,
            voxels_bytes / (1024 * 1024),
        );
        if blocks_bytes > max_binding_bytes
            || voxels_bytes > max_binding_bytes
            || chunks_bytes > max_binding_bytes
        {
            bevy::log::error!(
                "vox-gpu-rewrite Q4 CONFIRMED: storage-buffer binding overrun — \
                 blocks {} B (> limit? {}), voxels {} B (> limit? {}), chunks {} B (> limit? {}), \
                 max_storage_buffer_binding_size = {} B. \
                 WebGPU will silently truncate the bound range; the renderer reads zeros past the cap.",
                blocks_bytes,
                blocks_bytes > max_binding_bytes,
                voxels_bytes,
                voxels_bytes > max_binding_bytes,
                chunks_bytes,
                chunks_bytes > max_binding_bytes,
                max_binding_bytes,
            );
        }
    }
    let mut blocks = GrowableBuffer::<u32>::new(&render_device, "naadf_blocks", blocks_alloc_len as u64);
    let mut voxels = GrowableBuffer::<u32>::new(&render_device, "naadf_voxels", voxels_alloc_len as u64);
    let mut voxel_types = GrowableBuffer::<GpuVoxelType>::new(
        &render_device,
        "naadf_voxel_types",
        voxel_types_data.len() as u64,
    );
    if gpu_producer_skip_upload {
        // Allocate-only: zero-init the blocks/voxels buffers. The GPU
        // dispatch in `prepare_construction` writes Algorithm 1 output here.
        // Allocate one zero element so wgpu accepts the buffer; the storage
        // buffer itself is sized `blocks_alloc_len` u32s.
        blocks.upload_all(&[0u32], &render_device, &render_queue);
        voxels.upload_all(&[0u32], &render_device, &render_queue);
    } else {
        let blocks_data: Vec<u32> = if extracted.blocks.is_empty() {
            vec![0]
        } else {
            extracted.blocks.clone()
        };
        let voxels_data: Vec<u32> = if extracted.voxels.is_empty() {
            vec![0]
        } else {
            extracted.voxels.clone()
        };
        blocks.upload_all(&blocks_data, &render_device, &render_queue);
        voxels.upload_all(&voxels_data, &render_device, &render_queue);
    }
    // web-vox-color-divergence (2026-05-18) — one-shot palette upload trace.
    // Logs the GPU upload event with palette length + first 5 packed entries.
    // Demoted to `debug!` post-fix (Decision D-LOGS-DEBUG-NOT-TRACE); reachable
    // via `RUST_LOG=bevy_naadf=debug` for future regression diagnosis. The
    // matching `[palette-install]` logs in `voxel/grid.rs` and the
    // `[palette-refresh]` log in `extract.rs` complete the trace.
    // **DO NOT REMOVE** — this is the smoke detector for the
    // `web-vox-color-divergence` regression class.
    {
        let preview: Vec<[u32; 4]> = voxel_types_data
            .iter()
            .take(5)
            .map(|e| e.data)
            .collect();
        debug!(
            "[palette-upload] prepare_world_gpu uploading voxel_types to GPU \
             (palette_len={}, first_5_raw={:?})",
            voxel_types_data.len(),
            preview,
        );
    }
    voxel_types.upload_all(&voxel_types_data, &render_device, &render_queue);

    // --- world_meta uniform -------------------------------------------------
    // The ray-AABB bounds NAADF's `rayAABB` / `shootRay` clip to. Faithful to
    // `WorldData.setEffect` (`WorldData.cs:477-478`): the world extent inset by
    // 0.1 voxel on every side — `boundingBoxMin = (0.1,0.1,0.1)`,
    // `boundingBoxMax = sizeInVoxels - (0.1,0.1,0.1)`. `extracted.bounding_box`
    // is the inclusive integer voxel AABB `{ min: 0, max: sizeInVoxels - 1 }`,
    // so `sizeInVoxels = bounding_box.max + 1`. The 0.1 inset keeps the ray
    // entry point off the integer voxel planes — without it, an out-of-volume
    // camera's entry point lands exactly on a voxel boundary and `floor()`
    // flips per-pixel with f32 noise (the concentric-lines artifact).
    let size_in_voxels = (extracted.bounding_box.max + IVec3::ONE).as_vec3();
    let world_meta_data = GpuWorldMeta {
        size_in_chunks: size,
        _pad0: 0,
        bounding_box_min: extracted.bounding_box.min.as_vec3() + Vec3::splat(0.1),
        _pad1: 0,
        bounding_box_max: size_in_voxels - Vec3::splat(0.1),
        _pad2: 0,
    };
    let world_meta = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_world_meta"),
        size: std::mem::size_of::<GpuWorldMeta>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    render_queue.write_buffer(&world_meta, 0, bytemuck::bytes_of(&world_meta_data));

    // --- W4 wave-3 — placeholder entity-track buffers (`15-design-c.md` §1.7)
    //
    // `NaadfPipelines::world_layout` carries 3 W4 entity bindings unconditionally
    // (slots 5/6/7: `entity_chunk_instances`, `entity_voxel_data`,
    // `entity_instances_history`). On the no-entities path these point at
    // single-element placeholder storage buffers so the bind group is
    // well-formed and the WGSL bindings exist — the `shoot_ray` entity
    // sub-traversal branch never fires (gated by the `ENTITIES_ENABLED`
    // shader-def). `prepare_construction` rebuilds this bind group with the
    // real W4 buffers (and the production `WorldGpu` chunks view) once
    // `ConstructionGpu` has them allocated AND `entities_enabled = true`.
    let placeholder_entity_chunk_instances = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_world_entity_chunk_instances_placeholder"),
        // 20 B = one GpuEntityChunkInstance (the layout's expected stride).
        size: 20,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let placeholder_entity_voxel_data = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_world_entity_voxel_data_placeholder"),
        size: 4, // one u32
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let placeholder_entity_instances_history = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_world_entity_instances_history_placeholder"),
        size: 16, // one vec4<u32>
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- @group(0) bind group ----------------------------------------------
    // Phase-C wave-3 — `world_layout` now has 8 bindings; slots 5/6/7 are the
    // W4 entity-track read-only buffers. Bound to placeholders here; the
    // construction-side `prepare_construction` may rebuild this bind group with
    // the real W4 buffers when `ConstructionConfig.entities_enabled = true`.
    let bind_group = render_device.create_bind_group(
        "naadf_world_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.world_layout),
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            blocks.buffer().as_entire_buffer_binding(),
            voxels.buffer().as_entire_buffer_binding(),
            voxel_types.buffer().as_entire_buffer_binding(),
            world_meta.as_entire_buffer_binding(),
            placeholder_entity_chunk_instances.as_entire_buffer_binding(),
            placeholder_entity_voxel_data.as_entire_buffer_binding(),
            placeholder_entity_instances_history.as_entire_buffer_binding(),
        )),
    );

    commands.insert_resource(WorldGpu {
        chunks_buffer,
        chunks_size_in_chunks: size,
        blocks,
        voxels,
        voxel_types,
        world_meta,
        bind_group,
        entity_chunk_instances_placeholder: placeholder_entity_chunk_instances,
        entity_voxel_data_placeholder: placeholder_entity_voxel_data,
        entity_instances_history_placeholder: placeholder_entity_instances_history,
    });
    // Build-once consumed. Drop the staging resource so its ~48 MiB on
    // Oasis is reclaimed and no future code path can accidentally re-read
    // a stale buffer image (`02f` rearch).
    commands.remove_resource::<WorldGpuStaging>();
}

/// Rebuild `WorldGpu.bind_group` against the production W4 entity buffers.
///
/// `prepare_world_gpu` builds the world bind group against single-element
/// placeholder buffers for the three W4 entity slots (entities-off path). Once
/// the construction-side W4 buffers are allocated (entities-on, post-extract),
/// `prepare_construction` calls this helper to swap them in. The placeholder
/// buffers stay alive on `WorldGpu` so a toggle-off rebuild can re-seat them
/// without reallocating.
///
/// Owns the structural shape of the world `@group(0)` layout (D4 territory per
/// `00-reuse-audit.md`). The construction-side caller (D5 territory) supplies
/// the W4 buffers; D4 owns the binding order. Pre-Resolution-D, this rebuild
/// was inlined in `prepare_construction` with a re-declared
/// `BindGroupLayoutDescriptor` mirror — folding the helper here makes the
/// cross-write a single named function call rather than a 35-LOC inline
/// duplicate of the layout (`render/pipelines.rs::NaadfPipelines::world_layout`).
pub(crate) fn rebuild_world_bind_group_with_entities(
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &NaadfPipelines,
    world_gpu: &WorldGpu,
    entity_chunk_instances: &Buffer,
    entity_voxel_data: &Buffer,
    entity_instances_history: &Buffer,
) -> BindGroup {
    let bgl = pipeline_cache.get_bind_group_layout(&pipelines.world_layout);
    render_device.create_bind_group(
        "naadf_world_bind_group_with_entities",
        &bgl,
        &BindGroupEntries::sequential((
            world_gpu.chunks_buffer.as_entire_buffer_binding(),
            world_gpu.blocks.buffer().as_entire_buffer_binding(),
            world_gpu.voxels.buffer().as_entire_buffer_binding(),
            world_gpu.voxel_types.buffer().as_entire_buffer_binding(),
            world_gpu.world_meta.as_entire_buffer_binding(),
            entity_chunk_instances.as_entire_buffer_binding(),
            entity_voxel_data.as_entire_buffer_binding(),
            entity_instances_history.as_entire_buffer_binding(),
        )),
    )
}
