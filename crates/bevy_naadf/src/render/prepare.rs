//! `Prepare` set: upload buffers, build bind groups, write camera uniforms
//! (`03-design.md` §4.5, §5).
//!
//! Two prepare systems:
//!
//! - [`prepare_world_gpu`] — on the first dirty frame, create the `chunks` 3D
//!   texture + the `blocks` / `voxels` / `voxel_types` `GrowableBuffer`s + the
//!   `world_meta` uniform, upload all of them, and build `bind_group_world`.
//!   Build-once (D2): later frames are a no-op.
//! - [`prepare_frame_gpu`] — every frame: `write_buffer` the `GpuCamera` +
//!   `GpuRenderParams` uniforms, (re)create the `first_hit_data` storage buffer
//!   on a viewport resize, and build `bind_group_frame`. The per-pixel
//!   accumulated-colour buffer (Phase A's `shaded_color` stand-in) moved into
//!   `TaaGpu` as the real `taa_sample_accum` — `prepare_frame_gpu` reads
//!   `TaaGpu` and binds it (`06-design-a2.md` §5.5, §9.4).
//!
//! The chunk layer is an `array<vec2<u32>>` storage buffer (`.x` = block-state
//! pointer + AADF, `.y` = entity pointer + counter; W4's chunk-pair widening,
//! `15-design-c.md` §1.3 / §1.7). Phase A landed it as `R32Uint`, CPU-built
//! and upload-only; Phase C widened it to `Rg32Uint` and gave it
//! `STORAGE_BINDING | TEXTURE_BINDING | COPY_DST` so the W1/W2/W3/W4
//! construction passes could write it via
//! `texture_storage_3d<rg32uint, read_write>`. The web-WebGPU migration
//! replaces the 3D texture with a flat storage buffer because the WebGPU spec
//! only permits `read_write` storage textures on `r32{uint,sint,float}`.
//!
//! Both `world_layout` (read-only, render passes) and the three construction
//! layouts (`construction_world_layout` /
//! `construction_bounds_world_layout` / `entity_world_layout`, read-write,
//! construction sub-graph) now bind the same underlying GPU storage buffer
//! through `storage_buffer_read_only_sized` / `storage_buffer_sized`. Chunk
//! position flattens to a linear index via `flatten_index(chunk_pos,
//! size_in_chunks.x, size_in_chunks.x * size_in_chunks.y)` (the existing
//! `common.wgsl:32` helper, x-fastest convention).

use std::f32::consts::PI;

use bevy::math::Vec3;
use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
    PipelineCache,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use crate::render::atmosphere::AtmosphereGpu;
use crate::render::extract::{
    ExtractedCameraData, ExtractedCameraHistory, ExtractedGiConfig, WorldGpuStaging,
};
use crate::render::gi::{GiBindGroups, GiGpu};
use crate::render::gpu_types::{
    GpuCamera, GpuRenderParams, GpuVoxelType, GpuWorldMeta, FLAG_CHECK_SUN,
    FLAG_IS_ATMOSPHERE_INTERACTION, FLAG_IS_TAA,
};
use crate::render::pipelines::NaadfPipelines;
use crate::render::taa::TaaGpu;
use crate::world::buffer::{GrowableBuffer, GROWABLE_BUFFER_USAGES};

/// The GPU side of the voxel world (`03-design.md` §4.4 — render-world
/// `WorldGpu` resource). Created once by [`prepare_world_gpu`].
#[derive(Resource)]
pub struct WorldGpu {
    /// The chunk layer — an `array<vec2<u32>>` storage buffer indexed by
    /// `flatten_index(chunk_pos, sx, sx*sy)` (x-fastest), where each pair
    /// carries `(state, entity_y)`. Web-WebGPU migration replaced the
    /// previous `Rg32Uint` 3D texture because WebGPU forbids `read_write`
    /// storage textures on non-r32 formats.
    pub chunks_buffer: Buffer,
    /// World size in chunks — cached so consumers can derive the buffer's
    /// 3D shape without reaching into a no-longer-existing texture. Matches
    /// the `size_in_chunks` field on `GpuWorldMeta` / `GpuConstructionParams`.
    pub chunks_size_in_chunks: UVec3,
    /// The block layer — a growable `u32` storage buffer.
    pub blocks: GrowableBuffer<u32>,
    /// The voxel layer — a growable `u32` storage buffer (packed voxels).
    pub voxels: GrowableBuffer<u32>,
    /// The material buffer — a growable `vec4<u32>` storage buffer.
    pub voxel_types: GrowableBuffer<GpuVoxelType>,
    /// The `world_meta` uniform buffer.
    pub world_meta: Buffer,
    /// `@group(0)` bind group binding all of the above + the W4 entity bindings
    /// (production or placeholder — see [`entity_chunk_instances_placeholder`]).
    pub bind_group: BindGroup,
    /// Phase-C wave-3 — 1-element placeholder buffer for the
    /// `entity_chunk_instances` slot (5) of `world_layout`. Used when
    /// `ConstructionConfig.entities_enabled = false` so the layout is
    /// satisfied without allocating the real entity buffers. When entities are
    /// enabled `prepare_construction` rebuilds the world bind group binding
    /// the real `ConstructionGpu::entity_chunk_instances` instead.
    pub entity_chunk_instances_placeholder: Buffer,
    /// Phase-C wave-3 — placeholder for `entity_voxel_data` (slot 6). See
    /// [`entity_chunk_instances_placeholder`].
    pub entity_voxel_data_placeholder: Buffer,
    /// Phase-C wave-3 — placeholder for `entity_instances_history` (slot 7).
    pub entity_instances_history_placeholder: Buffer,
}

/// The per-frame GPU resources (`03-design.md` §4.4 — render-world `FrameGpu`
/// resource). The uniforms are rewritten every frame; the storage buffers are
/// rebuilt only on a viewport resize.
#[derive(Resource)]
pub struct FrameGpu {
    /// `GpuCamera` uniform buffer.
    pub camera: Buffer,
    /// `GpuRenderParams` uniform buffer.
    pub render_params: Buffer,
    /// The G-buffer — one `vec4<u32>` per pixel (`03-design.md` §5.3,
    /// `09-design-b.md` §3.4).
    pub first_hit_data: Buffer,
    /// Per-pixel accumulated transmittance along the primary-ray path — one
    /// `vec2<u32>` per pixel (`base/renderFirstHit.fx:7`, `09-design-b.md`
    /// §3.4). Written by the `base/` first-hit; read by the GI passes (Batch 3+).
    pub first_hit_absorption: Buffer,
    /// The GI working-colour buffer — one `vec2<u32>` per pixel
    /// (`base/renderFirstHit.fx:8`, `09-design-b.md` §3.4). The `base/`
    /// first-hit writes the primary-ray light here; the GI passes thread their
    /// result through it (Batch 5); `CalcNewTaaSample` folds it into the TAA
    /// history (Batch 6). In Batch 2 it is also the *temporary* final-blit
    /// source (`09-design-b.md` §11 Batch 2 step 8 — reverted in Batch 6).
    pub final_color: Buffer,
    /// Pixel count the storage buffers are currently sized for.
    pub pixel_count: u32,
    /// `@group(1)` bind group for the first-hit compute pass. Binds
    /// `taa_sample_accum` (owned by `TaaGpu`) at slot 3, plus
    /// `first_hit_absorption` + `final_color` at slots 4/5 (the Phase-B Batch-2
    /// widening — `09-design-b.md` §6.3).
    pub bind_group: BindGroup,
    /// `@group(2)` for the Phase-B 4-plane first-hit — the read-only
    /// precomputed atmosphere (`atmosphere_params` + `atmosphere_comp`). Mixes
    /// `AtmosphereGpu` resources, so it is built here in `prepare_frame_gpu`
    /// (after `AtmosphereGpu` exists). `09-design-b.md` §6.3 / §10.3.
    pub first_hit_atmosphere_bind_group: BindGroup,
    /// The final-blit pass's own bind group. Phase B Batch 6 reverts the
    /// Batch-2 temporary seam: it binds `taa_sample_accum` at slot 1 again (the
    /// real `base/` blit source — correctly filled by `ReprojectOld` +
    /// `CalcNewTaaSample`), not `final_color` (`09-design-b.md` §11 Batch 6
    /// step 19).
    pub blit_bind_group: BindGroup,
    /// The TAA reproject pass's single bind group (`06-design-a2.md` §5.3,
    /// §5.5, `09-design-b.md` §5.8.1). Mixes `TaaGpu` resources (`taa_params`,
    /// `camera_history`, `taa_samples`, `taa_sample_accum`, `taa_dist_min_max`)
    /// with `FrameGpu.first_hit_data`, so it is built here in `prepare_frame_gpu`
    /// (after both `TaaGpu` and `first_hit_data` exist). Consumed by
    /// `naadf_taa_reproject_node`.
    pub taa_reproject_bind_group: BindGroup,
    /// The `calc_new_taa_sample` pass's `@group(1)` bind group (`09-design-b.md`
    /// §4.10 / §5.8.2). Mixes `TaaGpu` (`taa_params`, `taa_samples`,
    /// `taa_sample_accum`) + `FrameGpu` (`first_hit_data`, `final_color`) +
    /// `WorldGpu` (`voxel_types`), so it is built here in `prepare_frame_gpu`
    /// (after all three resources exist). Consumed by
    /// `naadf_calc_new_taa_sample_node`.
    pub calc_new_taa_sample_bind_group: BindGroup,
}

/// W2-edit growth headroom multiplier for the `blocks` / `voxels`
/// `GrowableBuffer`s allocated at build-once time (`02f` R3 mitigation).
///
/// The W2 GPU dispatch (`naadf_world_change_node`'s `apply_block_change.wgsl`
/// + `apply_voxel_change.wgsl`) appends new block/voxel records at indices
/// driven by atomic `block_voxel_count[]` cursors. Without per-edit re-alloc
/// (deleted in `02f`), the build-time allocation must absorb the edit-time
/// append capacity for the duration of typical strokes. 2× headroom on top
/// of the build-time CPU mirror size covers ~10 s of continuous r=16 brush
/// editing on Oasis (~125 mixed-blocks/frame × 600 frames × 64 u32s/block =
/// 4.8 MB growth, well under the 6.3 MiB headroom).
///
/// Worst-case: a sphere r=400 or a multi-Oasis-scale stroke could exceed
/// this. Larger headroom is straightforward (the cost is one-time
/// allocation at startup, not per-frame); a future iteration could wire
/// dynamic growth via a GPU readback of `block_voxel_count[]` cursors with
/// realloc on overflow.
const W2_BUFFER_HEADROOM_MUL: u64 = 2;

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
pub fn prepare_world_gpu(
    mut commands: Commands,
    staging: Option<Res<WorldGpuStaging>>,
    existing: Option<Res<WorldGpu>>,
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
    // Build-once: WorldGpu already exists → this system is forever done.
    if existing.is_some() {
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
    // web-vox-color-divergence diagnose-first (2026-05-18) — one-shot palette
    // upload trace. Logs the GPU upload event with palette length + first
    // 5 packed entries so we can compare native vs web ordering against the
    // [palette-install] log at `voxel/grid.rs`. This is diagnostic
    // instrumentation; the architect/implementer will demote it to `debug!`
    // once the divergence is fixed (per
    // `docs/orchestrate/web-vox-color-divergence/01-context.md` forbidden
    // move 11). DO NOT REMOVE without that demotion.
    {
        let preview: Vec<[u32; 4]> = voxel_types_data
            .iter()
            .take(5)
            .map(|e| e.data)
            .collect();
        info!(
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
