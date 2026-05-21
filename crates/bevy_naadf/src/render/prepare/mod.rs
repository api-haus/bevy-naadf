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
//!
//! Split into [`world`] + [`frame`] submodules per the codebase-tightening
//! D4 architect's Step 3. The struct defs ([`WorldGpu`], [`FrameGpu`]) live
//! here in `mod.rs` so external imports (`use crate::render::prepare::{WorldGpu,
//! FrameGpu};`) keep resolving verbatim — the split is invisible to D5
//! callers (per architect §6 side-note 6).

pub mod frame;
pub mod world;

pub use frame::prepare_frame_gpu;
pub use world::prepare_world_gpu;
pub(crate) use world::rebuild_world_bind_group_with_entities;

use bevy::prelude::*;
use bevy::render::render_resource::{BindGroup, Buffer};

use crate::render::gpu_types::GpuVoxelType;
use crate::world::buffer::GrowableBuffer;

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
pub(super) const W2_BUFFER_HEADROOM_MUL: u64 = 2;
