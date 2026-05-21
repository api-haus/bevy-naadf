//! W1 — `chunk_calc.wgsl` Rust side: layout + pipeline queueing + dispatch
//! helpers (`15-design-c.md` §2.1 W1, §4.1).
//!
//! Pipelines:
//!   - `chunk_calc_pipeline_calc_block` — `calc_block_from_raw_data` entry
//!     point, Algorithm 1 (paper §3.2, `chunkCalc.fx:117-181`).
//!   - `chunk_calc_pipeline_voxel_bounds` — `compute_voxel_bounds`,
//!     2-bit voxel AADFs (`chunkCalc.fx:193-217`).
//!   - `chunk_calc_pipeline_block_bounds` — `compute_block_bounds`,
//!     2-bit block AADFs (`chunkCalc.fx:219-241`).
//!
//! Layout: a single `construction_world_layout` `@group(0)` shared by all
//! three entry points (per `15-design-c.md` §1.3) with the addition of one
//! binding for the `hash_coefficients` table — a port deviation from the C#
//! design (where the coefficients are a 65-element uniform array; in WGSL,
//! storing them as a read-only storage buffer is the idiomatic mirror and
//! avoids the std140 16-B-stride waste).
//!
//! The 8-binding layout total:
//!   0: chunks_rw          storage_buffer<array<vec2<u32>>>          (W4-widened
//!                         pair; was `texture_storage_3d<rg32uint, read_write>`
//!                         pre-web-WebGPU migration)
//!   1: blocks_rw          storage_buffer<array<u32>>
//!   2: voxels_rw          storage_buffer<array<u32>>
//!   3: block_voxel_count  storage_buffer<array<atomic<u32>>>
//!   4: segment_voxel_buf  storage_buffer<array<u32>>   (read-only)
//!   5: hash_map_rw        storage_buffer<array<HashValueSlot>>
//!   6: params             uniform<ConstructionParams>
//!   7: hash_coefficients  storage_buffer<array<u32>>   (read-only) — W1 add

use std::borrow::Cow;
use std::num::NonZeroU64;

use bevy::prelude::*;
use bevy::render::render_resource::{
    binding_types::{
        storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized,
    },
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId,
    CommandEncoder, ComputePassDescriptor, ComputePipelineDescriptor, PipelineCache,
    ShaderStages,
};
use bevy::shader::Shader;

use crate::render::gpu_types::GpuConstructionParams;

/// Asset path of the W1 `chunk_calc.wgsl` shader.
pub const CHUNK_CALC_SHADER: &str = "shaders/chunk_calc.wgsl";

/// Inlined source — used by the W1 unit test (which builds a headless render
/// world without an asset loader). The same pattern W5's `generator_model.rs`
/// uses (`16-impl-c-W5.md` decision #7).
pub const CHUNK_CALC_SHADER_SRC: &str =
    include_str!("../../assets/shaders/chunk_calc.wgsl");

/// Build the `construction_world_layout` bind-group-layout descriptor
/// (`15-design-c.md` §1.3 + W1's `hash_coefficients` deviation).
///
/// Used by all three `chunk_calc.wgsl` entry points + W2's `world_change`
/// (which extends with `@group(1)` for the change-staging buffers).
pub fn construction_world_layout_descriptor() -> BindGroupLayoutDescriptor {
    let params_size =
        NonZeroU64::new(std::mem::size_of::<GpuConstructionParams>() as u64).unwrap();
    BindGroupLayoutDescriptor::new(
        "naadf_construction_world_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                // chunks_rw — `array<vec2<u32>>` storage buffer (W4 §1.7;
                // web-WebGPU migration replaced
                // `texture_storage_3d<rg32uint, read_write>` because WebGPU
                // forbids `read_write` storage textures on non-r32 formats).
                storage_buffer_sized(false, None),
                // blocks_rw / voxels_rw / block_voxel_count_rw — rw storage
                // arrays. Atomic access is on the WGSL side
                // (`array<atomic<u32>>` for the 2-element counter); the wgpu
                // binding type is the same `storage_buffer_sized(false, None)`.
                storage_buffer_sized(false, None),
                storage_buffer_sized(false, None),
                storage_buffer_sized(false, None),
                // segment_voxel_buffer — ro storage.
                storage_buffer_read_only_sized(false, None),
                // hash_map_rw — rw storage (the `HashValueSlot` array).
                storage_buffer_sized(false, None),
                // params — uniform.
                uniform_buffer_sized(false, Some(params_size)),
                // hash_coefficients — ro storage (W1 deviation, see file doc).
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    )
}

/// Queue the `calc_block_from_raw_data` pipeline against the given layout.
pub fn queue_calc_block_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(CHUNK_CALC_SHADER);
    queue_calc_block_pipeline_with_handle(pipeline_cache, layout, shader)
}

/// Same as [`queue_calc_block_pipeline`] but takes an already-resolved shader
/// handle (the headless-test entry point).
pub fn queue_calc_block_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_chunk_calc_calc_block_pipeline".into()),
        layout: vec![layout],
        shader,
        entry_point: Some(Cow::from("calc_block_from_raw_data")),
        ..default()
    })
}

/// Queue the `compute_voxel_bounds` pipeline.
pub fn queue_voxel_bounds_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(CHUNK_CALC_SHADER);
    queue_voxel_bounds_pipeline_with_handle(pipeline_cache, layout, shader)
}

pub fn queue_voxel_bounds_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_chunk_calc_voxel_bounds_pipeline".into()),
        layout: vec![layout],
        shader,
        entry_point: Some(Cow::from("compute_voxel_bounds")),
        ..default()
    })
}

/// Queue the `compute_block_bounds` pipeline.
pub fn queue_block_bounds_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(CHUNK_CALC_SHADER);
    queue_block_bounds_pipeline_with_handle(pipeline_cache, layout, shader)
}

pub fn queue_block_bounds_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_chunk_calc_block_bounds_pipeline".into()),
        layout: vec![layout],
        shader,
        entry_point: Some(Cow::from("compute_block_bounds")),
        ..default()
    })
}

/// Dispatch `calc_block_from_raw_data` for one segment. Dispatch shape: one
/// workgroup per chunk in the segment (`15-design-c.md` §4.1).
pub fn dispatch_calc_block_from_raw_data(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    segment_size_in_chunks: u32,
) {
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_chunk_calc_calc_block_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(
        segment_size_in_chunks,
        segment_size_in_chunks,
        segment_size_in_chunks,
    );
}

/// Phase-C followup #1 — dispatch `calc_block_from_raw_data` over a
/// non-cubic world extent. Used by the runtime GPU producer in
/// `prepare_construction` for worlds whose chunk extent is not a perfect
/// cube (e.g. the bevy-naadf 4×2×4 test grid). The shader still uses
/// `params.segment_size_in_chunks` as its X/Y stride (kept at the cubic
/// max), so the segment_voxel_buffer's indexing remains
/// `chunk_index_in_segment = gx + gy*seg + gz*seg*seg` — the dispatch is
/// just bounded to the actual world shape so out-of-bounds `textureStore`
/// writes never happen.
pub fn dispatch_calc_block_from_raw_data_world_sized(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    world_size_in_chunks: [u32; 3],
) {
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_chunk_calc_calc_block_pass_world_sized"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(
        world_size_in_chunks[0],
        world_size_in_chunks[1],
        world_size_in_chunks[2],
    );
}

/// Wgpu's per-dispatch-axis workgroup-count cap (the WebGPU spec minimum;
/// the native wgpu default). When a 1D workgroup count exceeds this, the
/// dispatch is repacked into a 3D shape via [`split_3d_dispatch`] so the
/// shader can flatten the dispatch ID back into a 1D index.
pub const WGPU_MAX_WORKGROUPS_PER_DIM: u32 = 65535;

/// Repack a 1D workgroup count into a 3D dispatch shape `(x, y, z)` where
/// `x * y * z >= count` and `max(x, y, z) <= WGPU_MAX_WORKGROUPS_PER_DIM`.
///
/// vox-gpu-rewrite W5.3-fix Stage 1 — the W5 GPU producer chain's
/// bounds-chain dispatch may need up to `chunks * 64 ≈ 2.1M` workgroups for
/// `compute_block_bounds` and `chunks * 64 / 2 ≈ 4.2M` for
/// `compute_voxel_bounds` on the 256×32×256 fixed world. Neither fits the
/// 65535/axis cap. The chunk_calc WGSL entry points were updated to compute
/// a flat workgroup id from `group_id.x + group_id.y * num_workgroups.x +
/// group_id.z * num_workgroups.x * num_workgroups.y`, so this helper
/// distributes the count across axes:
///   - if `count <= cap`: `(count, 1, 1)` (1D fast path; matches C#).
///   - else if `count <= cap * cap`: `(cap, ceil(count / cap), 1)`.
///   - else: `(cap, cap, ceil(count / (cap * cap)))`.
///
/// The total dispatched workgroup count `x * y * z` may exceed `count` by
/// up to `(cap - 1) + (cap*cap - 1)` workgroups; the extra workgroups read
/// past the valid block/voxel data and OOB writes are spec-defined no-ops
/// (WebGPU §Storage Buffer Access) — provided the buffers are sized at or
/// above the worst-case (`chunks * 64` blocks, `chunks * 128` voxels;
/// guaranteed by `render/prepare.rs::prepare_world_gpu`'s W5-aware
/// sizing). The shader processes zero blocks / voxels in those extra
/// groups, yielding zero AADF bits — a correct no-op.
pub fn split_3d_dispatch(count: u32) -> [u32; 3] {
    if count == 0 {
        return [0, 0, 0];
    }
    let cap = WGPU_MAX_WORKGROUPS_PER_DIM;
    if count <= cap {
        return [count, 1, 1];
    }
    let cap_u64 = cap as u64;
    let count_u64 = count as u64;
    if count_u64 <= cap_u64 * cap_u64 {
        // Two-axis distribution. y = ceil(count / cap).
        let y = ((count_u64 + cap_u64 - 1) / cap_u64) as u32;
        return [cap, y, 1];
    }
    // Three-axis distribution. z = ceil(count / (cap * cap)).
    let cap_sq = cap_u64 * cap_u64;
    let z = ((count_u64 + cap_sq - 1) / cap_sq) as u32;
    [cap, cap, z]
}

/// Dispatch `compute_voxel_bounds` over `block_count` blocks (one workgroup
/// per block, 64 threads/group = 64 voxels per block).
///
/// vox-gpu-rewrite W5.3-fix Stage 1 — when `block_count` exceeds wgpu's
/// per-axis 65535 limit, the dispatch is split across axes via
/// [`split_3d_dispatch`]; the WGSL entry point flattens
/// `(group_id, num_workgroups)` back into a 1D `block_index`.
pub fn dispatch_compute_voxel_bounds(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    block_count: u32,
) {
    if block_count == 0 {
        return;
    }
    let [x, y, z] = split_3d_dispatch(block_count);
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_chunk_calc_voxel_bounds_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(x, y, z);
}

/// Dispatch `compute_block_bounds` over `chunk_count` chunks (one workgroup
/// per chunk, 64 threads/group = 64 blocks per chunk).
///
/// vox-gpu-rewrite W5.3-fix Stage 1 — see [`dispatch_compute_voxel_bounds`].
pub fn dispatch_compute_block_bounds(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    chunk_count: u32,
) {
    if chunk_count == 0 {
        return;
    }
    let [x, y, z] = split_3d_dispatch(chunk_count);
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_chunk_calc_block_bounds_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(x, y, z);
}

/// Phase-C followup #1 — CPU-side encoder that builds the full-world
/// `segment_voxel_buffer` from a dense `u16` voxel-type stream
/// (`world_size_in_voxels.x*y*z` entries, indexed
/// `x + y*world_sx_v + z*world_sx_v*world_sy_v`).
///
/// `world_size_in_chunks` is the REAL world extent the dense buffer covers.
/// `segment_size_in_chunks` is the size of the segment to build (≥ world; for
/// non-cubic worlds, segment is padded to `max(world_dim)` so the shader's
/// cubic `(seg, seg, seg)` workgroup dispatch reads stay in bounds). Padded
/// chunks (outside the world) return 0 (all-empty) for every voxel.
///
/// The encoding produces 2048 u32s per chunk (64 blocks × 32 u32s/block; 2
/// voxels per u32 packed as `lo | (hi << 16)`); each voxel encodes as `(1u <<
/// 15) | type` for non-empty, `0` for empty.
///
/// **Canonical home for the production-runtime encoder.** Test-only encoders
/// with similar shapes (different input types — `ModelData`, `&[u16]`, brush
/// region) live with their callers in `render::construction::validation`:
/// `build_segment_voxel_buffer`, `build_segment_voxel_buffer_for_region`,
/// `build_segment_voxel_buffer_for_world`. Consolidation into a single
/// `encode_chunk(...)` helper is a follow-up refactor.
pub fn build_segment_voxel_buffer_from_dense(
    dense_voxel_types: &[u16],
    world_size_in_chunks: [u32; 3],
    segment_size_in_chunks: [u32; 3],
) -> Vec<u32> {
    let world_sx_v = world_size_in_chunks[0] * 16;
    let world_sy_v = world_size_in_chunks[1] * 16;
    let world_sz_v = world_size_in_chunks[2] * 16;
    let seg_chunks =
        (segment_size_in_chunks[0] * segment_size_in_chunks[1] * segment_size_in_chunks[2]) as usize;
    let total_u32s = seg_chunks * 2048;
    let mut out = vec![0u32; total_u32s];
    let voxel_at = |v: [u32; 3]| -> u16 {
        // Out-of-real-world voxel positions read as empty (padding chunks).
        if v[0] >= world_sx_v || v[1] >= world_sy_v || v[2] >= world_sz_v {
            return 0;
        }
        let idx = (v[0] + v[1] * world_sx_v + v[2] * world_sx_v * world_sy_v) as usize;
        if idx >= dense_voxel_types.len() {
            return 0;
        }
        let ty = dense_voxel_types[idx];
        if ty == 0 {
            0
        } else {
            crate::voxel::VOXEL_FULL_FLAG | (ty & crate::voxel::VOXEL_PAYLOAD_MASK)
        }
    };
    for cz in 0..segment_size_in_chunks[2] as usize {
        for cy in 0..segment_size_in_chunks[1] as usize {
            for cx in 0..segment_size_in_chunks[0] as usize {
                let chunk_index = cx
                    + cy * segment_size_in_chunks[0] as usize
                    + cz * segment_size_in_chunks[0] as usize
                        * segment_size_in_chunks[1] as usize;
                let chunk_base = chunk_index * 2048;
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let vi_lo = vi * 2;
                                let vi_hi = vi * 2 + 1;
                                let lvx = vi_lo % 4;
                                let lvy = (vi_lo / 4) % 4;
                                let lvz = vi_lo / 16;
                                let hvx = vi_hi % 4;
                                let hvy = (vi_hi / 4) % 4;
                                let hvz = vi_hi / 16;
                                let lo = voxel_at([
                                    (cx * 16 + bx * 4 + lvx) as u32,
                                    (cy * 16 + by * 4 + lvy) as u32,
                                    (cz * 16 + bz * 4 + lvz) as u32,
                                ]);
                                let hi = voxel_at([
                                    (cx * 16 + bx * 4 + hvx) as u32,
                                    (cy * 16 + by * 4 + hvy) as u32,
                                    (cz * 16 + bz * 4 + hvz) as u32,
                                ]);
                                out[block_base + vi] = (lo as u32) | ((hi as u32) << 16);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}
