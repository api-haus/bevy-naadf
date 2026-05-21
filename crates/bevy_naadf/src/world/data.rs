//! The `WorldData` + `VoxelTypes` main-world resources — the three-layer CPU
//! buffer mirrors, world geometry, and the voxel-type palette
//! (`03-design.md` §4.4).
//!
//! These are the CPU side of the world. `voxel::grid::setup_test_grid`
//! builds them once at startup; `render::extract::stage_world_gpu_buildonce`
//! hands them off to the render world ONCE for the GPU resource build
//! (`02f-design-world-container-rearch.md`). Per-edit changes flow via the
//! W2 delta chain (`pending_edits.batches` → `naadf_world_change_node`); no
//! whole-world clone or re-upload after startup.
//!
//! ## Track B — Editor ray traversal + bulk edits
//!
//! `WorldData::ray_traversal` ports the C# `WorldData.RayTraversal:396-473`
//! naive 3-layer-descent DDA; `WorldData::set_voxels_batch` is the
//! sanctioned-divergence bulk-edit entry point that groups by chunk and runs
//! `process_edit_batch` once per call (see
//! `docs/orchestrate/feature-completeness/02b-design-editor.md`).
//!
//! ## DIAGNOSTIC-ONLY methods (`02f` rearch)
//!
//! [`WorldData::set_voxel`] and [`WorldData::set_voxels_batch_oracle`] run
//! the slow whole-world `recompute_chunk_layer_aadfs` and emit synthetic
//! AADF-changed chunk uploads. They are **DIAGNOSTIC-ONLY** — call sites:
//!
//! - The `--edit-mode` e2e validation gate (single `set_voxel` call,
//!   confirms the W2 delta chain emits well-formed records).
//! - The unit tests in this file and `crate::aadf::edit`.
//!
//! **Production code paths NEVER call these methods.** Brushes call
//! [`WorldData::set_voxels_batch`] (runtime fast path; no whole-world rehash)
//! or [`WorldData::set_chunks_uniform_batch`] (brush inside-chunk fast path;
//! one state write per chunk). The diagnostic methods are `#[doc(hidden)]`
//! and tagged `<!-- DIAGNOSTIC-ONLY -->` in their doc-comments.

use bevy::prelude::*;

use crate::voxel::{VoxelType, VoxelTypeId, CELL_DIM};

/// A single-voxel edit — the named replacement for the anonymous
/// `(IVec3, VoxelTypeId)` tuple that the brush + diagnostic APIs previously
/// accepted (UA-1 close-out, D7).
///
/// One of these per voxel a brush touches. D2's `editor/tools.rs` brushes
/// construct these values and pass them via [`WorldData::set_voxels_batch`].
/// The `From`/`Into` impls below preserve interop with the old tuple form
/// for any straggling caller.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VoxelEdit {
    /// World-space voxel position. Negative components are silently dropped
    /// by [`WorldData::set_voxels_batch`].
    pub pos: IVec3,
    /// Target voxel type. [`VoxelTypeId::EMPTY`] clears the voxel.
    pub ty: VoxelTypeId,
}

impl From<(IVec3, VoxelTypeId)> for VoxelEdit {
    fn from((pos, ty): (IVec3, VoxelTypeId)) -> Self {
        Self { pos, ty }
    }
}

impl From<VoxelEdit> for (IVec3, VoxelTypeId) {
    fn from(e: VoxelEdit) -> Self {
        (e.pos, e.ty)
    }
}

/// A whole-chunk uniform-state edit — the named replacement for the
/// anonymous `([u32; 3], Option<VoxelTypeId>)` tuple that
/// [`WorldData::set_chunks_uniform_batch`] previously accepted (UA-1
/// close-out, D7). Used by the brush inside-chunk fast path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChunkUniformEdit {
    /// Chunk position in chunk-space.
    pub pos: UVec3,
    /// `Some(ty)` for `UniformFull(ty)`; `None` (or `Some(EMPTY)`) for Empty.
    pub ty: Option<VoxelTypeId>,
}

impl From<([u32; 3], Option<VoxelTypeId>)> for ChunkUniformEdit {
    fn from((p, ty): ([u32; 3], Option<VoxelTypeId>)) -> Self {
        Self {
            pos: UVec3::new(p[0], p[1], p[2]),
            ty,
        }
    }
}

impl From<ChunkUniformEdit> for ([u32; 3], Option<VoxelTypeId>) {
    fn from(e: ChunkUniformEdit) -> Self {
        ([e.pos.x, e.pos.y, e.pos.z], e.ty)
    }
}

/// An inclusive integer AABB in voxel coordinates — the world's geometry
/// bounding box (`03-design.md` §4.4 `bounding_box: IAabb3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IAabb3 {
    /// Inclusive minimum corner, in voxels.
    pub min: IVec3,
    /// Inclusive maximum corner, in voxels.
    pub max: IVec3,
}

/// The CPU mirror of the NAADF three-layer voxel world (`03-design.md` §4.4).
///
/// Built once by `voxel::grid::setup_test_grid` (or `build_world_from_vox`)
/// and mutated by the W2 delta chain on edits. The GPU resources
/// (`WorldGpu` / chunks texture + blocks/voxels buffers) are built ONCE from
/// this CPU mirror at startup by `prepare_world_gpu`; per-edit changes flow
/// through `pending_edits.batches` → `naadf_world_change_node`'s GPU
/// dispatches, **never** through a whole-world re-upload (`02f` rearch
/// deletes the `dirty` flag and the per-frame extract clone).
///
/// Single source of truth — matches C# `WorldData.cs:20-218`'s "the CPU
/// arrays and the GPU resources live on the same object" semantic. The
/// chunks_cpu/blocks_cpu/voxels_cpu arrays here ARE the CPU half of that
/// container; the GPU half lives in the render-world `WorldGpu` resource;
/// the two are kept consistent by the W2 delta chain after the build-once
/// hand-off.
#[derive(Resource, Debug)]
pub struct WorldData {
    /// Chunk buffer mirror — one encoded `ChunkCell` `u32` per chunk.
    pub chunks_cpu: Vec<u32>,
    /// Block buffer mirror — encoded `BlockCell` `u32`s, 64 per mixed chunk.
    pub blocks_cpu: Vec<u32>,
    /// Voxel buffer mirror — packed voxel `u32`s, 32 per mixed block.
    pub voxels_cpu: Vec<u32>,
    /// World size in chunks.
    pub size_in_chunks: UVec3,
    /// Geometry bounding box, in voxels.
    pub bounding_box: IAabb3,
    /// Phase-C W2 — per-frame edit batches awaiting extract into the render
    /// world. Drained by `extract_world_changes`.
    pub pending_edits: PendingEdits,
    /// Phase-C followup #1 — the *dense* pre-construction voxel-type stream
    /// (`size_in_voxels.x * y * z` u16s, `x + y * sx + z * sx * sy` indexing).
    /// Kept so the runtime GPU producer can rebuild `segment_voxel_buffer`
    /// without re-running CPU construction. Empty when the world was not
    /// authored from a `DenseVolume` (e.g. legacy code paths); the GPU
    /// dispatch falls back to its existing producer chain in that case.
    pub dense_voxel_types: Vec<u16>,
    /// Content-addressable storage for mixed-block voxel payloads (port of
    /// C# `BlockHashingHandler`). Maps each unique 32-u32 voxel block to a
    /// single `voxels_cpu` slot via refcounting + a free list. Eliminates
    /// the redundant re-uploads the simplified port did on every edit (see
    /// `aadf::block_hash` for the AADF-overflow correctness story).
    /// Seeded by [`Self::seed_block_hashing`] after construction.
    pub block_hashing: crate::aadf::block_hash::BlockHashingHandler,
}

impl Default for WorldData {
    /// An empty, not-yet-built world.
    fn default() -> Self {
        Self {
            chunks_cpu: Vec::new(),
            blocks_cpu: Vec::new(),
            voxels_cpu: Vec::new(),
            size_in_chunks: UVec3::ZERO,
            bounding_box: IAabb3::default(),
            pending_edits: PendingEdits::default(),
            dense_voxel_types: Vec::new(),
            block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
        }
    }
}

impl WorldData {
    /// Walk the freshly-constructed world and register every mixed block's
    /// voxel slot in [`Self::block_hashing`]. **Must be called after
    /// construction (or `.vox` load) and before the first edit** so that
    /// the on-edit `add_block` / `delete_block` calls see the correct
    /// initial refcounts.
    ///
    /// Iteration: chunks_cpu → blocks_cpu (for Mixed chunks) → register each
    /// Mixed block's `voxel_ptr`. The handler's `add_block` is content-keyed,
    /// so identical voxel content across chunks shares a single slot — but
    /// construction-time `voxels_cpu` already allocated unique slots per
    /// block, so the seed pass intentionally bumps `use_count` for each
    /// block that points to an existing slot rather than allocating fresh.
    ///
    /// O(chunks × 64) mixed-block hashes + O(unique-blocks) hashmap inserts.
    /// For Oasis (~265 k chunks, ~10 M u32 voxels): ~200 k mixed blocks ⇒
    /// ~10 ms CPU on release builds. One-shot at load.
    pub fn seed_block_hashing(&mut self) {
        let n_chunks = self.chunks_cpu.len();
        for ci in 0..n_chunks {
            let chunk_raw = self.chunks_cpu[ci];
            if (chunk_raw >> 30) != 2 {
                // Empty / UniformFull — no block storage.
                continue;
            }
            let block_base = (chunk_raw & 0x3FFF_FFFF) as usize;
            for b in 0..crate::voxel::CELL_CHILDREN {
                let block_idx = block_base + b;
                if block_idx >= self.blocks_cpu.len() {
                    break;
                }
                let block_raw = self.blocks_cpu[block_idx];
                if (block_raw >> 30) != 2 {
                    // Empty / UniformFull block — no voxel-slot ref.
                    continue;
                }
                let voxel_ptr = block_raw & 0x3FFF_FFFF;
                let vbase = voxel_ptr as usize;
                if vbase + crate::aadf::block_hash::BLOCK_VOXEL_PAIRS
                    > self.voxels_cpu.len()
                {
                    continue;
                }
                // Hash the existing voxel content and register the slot.
                //
                // **Use `seed_block`, NOT `add_block`** (Bug 1 of
                // `docs/orchestrate/vox-gpu-rewrite/17-diagnostic-residual-
                // speckle-and-brush-clears.md`): the prior `add_block` call
                // unconditionally appended a duplicate copy of seeded
                // content to `voxels_cpu` and registered the appended END
                // pointer in the hash table. GPU's `voxels[]` only had data
                // at the ORIGINAL pointers (in the 0..N range populated by
                // construction); the appended END pointers (in the N..M
                // range) were never written to GPU. Edit-time
                // `add_block` for unchanged blocks then returned an END
                // pointer; GPU's `apply_block_change` wrote END pointer
                // into `blocks[]`; renderer descended into zero data → an
                // empty-voxel decode → 16-voxel-wide dark void around the
                // brush (the user-visible "making edits clears areas"
                // symptom).
                //
                // `seed_block` registers `voxel_ptr` (the existing pointer
                // already in `blocks_cpu` and on GPU's `blocks[]`) directly,
                // with no append. Subsequent edit-time `add_block` returns
                // the original pointer; GPU `voxels[]` has data there;
                // unchanged blocks render correctly post-edit.
                let pairs = {
                    let slice = &self.voxels_cpu
                        [vbase..vbase + crate::aadf::block_hash::BLOCK_VOXEL_PAIRS];
                    let mut buf = [0u32; crate::aadf::block_hash::BLOCK_VOXEL_PAIRS];
                    buf.copy_from_slice(slice);
                    buf
                };
                let hash = self.block_hashing.compute_hash(&pairs);
                let (registered_ptr, is_new) = self.block_hashing.seed_block(
                    hash,
                    &pairs,
                    voxel_ptr,
                    &self.voxels_cpu,
                );
                // If the seed produced a different pointer (because content
                // matched an earlier seed and was deduped), patch the block
                // word to point at the canonical slot. Both pointers
                // reference identical voxel content; GPU `voxels[]` has
                // correct data at both addresses, so the patch is purely a
                // CPU-side dedup that converges the CPU mirror onto the
                // handler's choice of canonical slot.
                if !is_new && registered_ptr != voxel_ptr {
                    self.blocks_cpu[block_idx] = (block_raw & !0x3FFF_FFFF) | registered_ptr;
                }
            }
        }
    }

    /// **DIAGNOSTIC-ONLY** single-voxel edit (`02f` rearch). Runs the
    /// whole-world `recompute_chunk_layer_aadfs` + emits synthetic
    /// AADF-changed chunk uploads — O(N_chunks × 31 × 3) per call. **Do not
    /// call from production code paths.**
    ///
    /// Call sites:
    /// - The `--edit-mode` e2e validation gate (one call, confirms the W2
    ///   delta chain emits well-formed records). Cost is irrelevant for a
    ///   one-shot validation run.
    /// - Unit tests in this file and `crate::aadf::edit`.
    ///
    /// **Production brushes call [`Self::set_voxels_batch`] or
    /// [`Self::set_chunks_uniform_batch`] instead.** Those skip the
    /// whole-world AADF rehash (the W3 GPU regime-2 self-perpetuating queue
    /// refreshes stale AADFs over subsequent frames, matching C# semantics).
    ///
    /// ## How it works (for diagnostic understanding)
    ///
    /// Phase-C W2 — programmatic single-voxel edit entry point
    /// (`15-design-c.md` §2.1 W2, `16-impl-c-W2.md`).
    ///
    /// Sets the voxel at world position `pos` (voxel coords) to `ty`. Walks
    /// the three-layer hierarchy from the chunk down, decoding into a
    /// 2048-u32 edit window, mutating, re-encoding through
    /// [`crate::aadf::edit::process_edit_batch`], and pushing the resulting
    /// `EditBatch` to `pending_edits` for W2 GPU upload. Out-of-bounds
    /// positions are silently ignored.
    #[doc(hidden)]
    pub fn set_voxel(&mut self, pos: IVec3, ty: VoxelTypeId) {
        crate::world::oracle::set_voxel(self, pos, ty);
    }
}

/// Result of a successful `WorldData::ray_traversal` call — the CPU pick hit
/// returned to the editor / brush systems
/// (`02b-design-editor.md`, ports C# `WorldData.RayTraversal:396-473` outputs).
#[derive(Debug, Clone)]
pub struct RayHit {
    /// Hit position in world space (origin + dir * distance).
    pub world_pos: Vec3,
    /// Voxel position of the hit voxel.
    pub voxel_pos: IVec3,
    /// Outward-facing axis-aligned unit normal of the hit face.
    pub normal: Vec3,
    /// Resolved voxel type id (low 15 bits of the C# `curNode & 0x3FFFFFFF`).
    pub voxel_type: VoxelTypeId,
    /// Distance along the ray (in world units = voxels) from origin to hit.
    pub distance: f32,
}

impl WorldData {
    /// CPU ray traversal — faithful port of C# `WorldData.RayTraversal`
    /// (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:396-473`),
    /// naive 3-layer-descent DDA (no AADF-skipping; the C# CPU traversal does
    /// not consult AADFs either).
    ///
    /// Returns `None` on world miss; on hit, returns the hit voxel position,
    /// world-space hit point, outward face normal, distance, and resolved
    /// voxel type id.
    ///
    /// Cited line numbers in the inline comments trace to the C# source.
    pub fn ray_traversal(&self, ray_origin: Vec3, ray_dir: Vec3) -> Option<RayHit> {
        // size_in_voxels = size_in_chunks * 16. CELL_DIM = 4, so 16 voxels/chunk axis.
        let size_v = (self.size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32)).as_vec3();
        if size_v.x == 0.0 || size_v.y == 0.0 || size_v.z == 0.0 {
            return None;
        }
        // C# WorldData.cs:399 — bounding box [(0.1), size_in_voxels - (0.1)].
        let world_min = Vec3::splat(0.1);
        let world_max = size_v - Vec3::splat(0.1);

        // C# WorldData.cs:399-404 — if origin is outside AABB AND the ray hits
        // it, advance start_pos by the entry distance.
        let mut start_pos = ray_origin;
        let world_bb_dist = ray_aabb_entry_distance(ray_origin, ray_dir, world_min, world_max);
        if !aabb_contains_point(world_min, world_max, ray_origin) {
            let dist = world_bb_dist?;
            start_pos += ray_dir * dist;
        }
        let world_bb_dist_or_zero = world_bb_dist.unwrap_or(0.0);

        // C# WorldData.cs:406-410 — DDA setup. `1e-10` matches C#.
        let inv_ray_dir_abs = Vec3::new(
            (1.0 / (1e-10 + ray_dir.x)).abs(),
            (1.0 / (1e-10 + ray_dir.y)).abs(),
            (1.0 / (1e-10 + ray_dir.z)).abs(),
        );
        let is_negative = IVec3::new(
            (ray_dir.x < 0.0) as i32,
            (ray_dir.y < 0.0) as i32,
            (ray_dir.z < 0.0) as i32,
        );
        let sign_ray_dir = Vec3::new(
            if ray_dir.x < 0.0 { -1.0 } else { 1.0 },
            if ray_dir.y < 0.0 { -1.0 } else { 1.0 },
            if ray_dir.z < 0.0 { -1.0 } else { 1.0 },
        );

        let mut mask = Vec3::ZERO;
        let mut cur_dist: f32 = 0.0;

        let sx = size_v.x as i32;
        let sy = size_v.y as i32;
        let sz = size_v.z as i32;

        // C# WorldData.cs:419 — 1000-step cap; verbatim.
        for _step in 0..1000 {
            let cur_pos = start_pos + ray_dir * cur_dist;
            // C# WorldData.cs:422 — face-snap to current cell.
            let cur_cell_v = (mask * sign_ray_dir * 0.5 + cur_pos).floor();
            let cur_cell = cur_cell_v.as_ivec3();

            // Bounds check — C# WorldData.cs:424.
            if cur_cell.x < 0
                || cur_cell.y < 0
                || cur_cell.z < 0
                || cur_cell.x >= sx
                || cur_cell.y >= sy
                || cur_cell.z >= sz
            {
                return None;
            }

            // C# WorldData.cs:428-430 — chunk lookup.
            let voxel_pos_in_chunk = IVec3::new(
                cur_cell.x.rem_euclid(16),
                cur_cell.y.rem_euclid(16),
                cur_cell.z.rem_euclid(16),
            );
            let chunk_pos = IVec3::new(cur_cell.x / 16, cur_cell.y / 16, cur_cell.z / 16);
            let chunk_idx = (chunk_pos.x
                + chunk_pos.y * self.size_in_chunks.x as i32
                + chunk_pos.z * self.size_in_chunks.x as i32 * self.size_in_chunks.y as i32)
                as usize;
            if chunk_idx >= self.chunks_cpu.len() {
                return None;
            }
            let mut cur_node: u32 = self.chunks_cpu[chunk_idx];

            // C# WorldData.cs:433 — bounds-in-direction at the chunk layer.
            let mut bounds_in_dir = IVec3::new(
                if ray_dir.x < 0.0 { voxel_pos_in_chunk.x } else { 15 - voxel_pos_in_chunk.x },
                if ray_dir.y < 0.0 { voxel_pos_in_chunk.y } else { 15 - voxel_pos_in_chunk.y },
                if ray_dir.z < 0.0 { voxel_pos_in_chunk.z } else { 15 - voxel_pos_in_chunk.z },
            );

            // C# WorldData.cs:435 — `(curNode >> 31) != 0` → mixed chunk. The
            // Rust port encodes Mixed as state value 2 in the top 2 bits
            // (bit 31 set, bit 30 clear); both checks are equivalent.
            let chunk_state = cur_node >> 30;
            if chunk_state == 2 {
                // C# WorldData.cs:437-442 — block descent.
                let block_pos_in_chunk = voxel_pos_in_chunk / 4;
                let block_base = (cur_node & 0x3FFF_FFFF) as usize;
                let block_idx = block_base
                    + (block_pos_in_chunk.x
                        + block_pos_in_chunk.y * 4
                        + block_pos_in_chunk.z * 16) as usize;
                if block_idx >= self.blocks_cpu.len() {
                    return None;
                }
                cur_node = self.blocks_cpu[block_idx];
                let voxel_pos_in_block = IVec3::new(
                    cur_cell.x.rem_euclid(4),
                    cur_cell.y.rem_euclid(4),
                    cur_cell.z.rem_euclid(4),
                );
                bounds_in_dir = IVec3::new(
                    if ray_dir.x < 0.0 { voxel_pos_in_block.x } else { 3 - voxel_pos_in_block.x },
                    if ray_dir.y < 0.0 { voxel_pos_in_block.y } else { 3 - voxel_pos_in_block.y },
                    if ray_dir.z < 0.0 { voxel_pos_in_block.z } else { 3 - voxel_pos_in_block.z },
                );

                // C# WorldData.cs:443 — block Mixed → descend to voxel.
                let block_state = cur_node >> 30;
                if block_state == 2 {
                    // C# WorldData.cs:445-447 — voxel descent (packed-pair u32).
                    let voxel_base_pair = (cur_node & 0x3FFF_FFFF) as usize;
                    let voxel_index = voxel_base_pair * 2
                        + (voxel_pos_in_block.x
                            + voxel_pos_in_block.y * 4
                            + voxel_pos_in_block.z * 16) as usize;
                    let pair_idx = voxel_index / 2;
                    if pair_idx >= self.voxels_cpu.len() {
                        return None;
                    }
                    let cur_voxel_pair = self.voxels_cpu[pair_idx];
                    let half = (cur_voxel_pair >> (16 * (voxel_index & 0x1))) & 0xFFFF;
                    // C# WorldData.cs:449-452 — bit 15 of the half-word = full flag.
                    if (half & 0x8000) != 0 {
                        // C# WorldData.cs:450 — promote: bit 30 = hit flag,
                        // low 15 bits = voxel type.
                        cur_node = (1 << 30) | (half & 0x7FFF);
                    } else {
                        // C# WorldData.cs:452 — empty voxel inside Mixed block.
                        bounds_in_dir = IVec3::ZERO;
                        // cur_node already has high bits clear (empty); hit
                        // test below fails; continue to step-distance.
                    }
                }
            }

            // C# WorldData.cs:456 — hit test (bit 30 set = full voxel or
            // uniform-full block/chunk).
            if (cur_node & 0x4000_0000) != 0 {
                let hit_type = (cur_node & 0x3FFF_FFFF) as u16;
                let result_length = cur_dist + world_bb_dist_or_zero;
                let world_pos = ray_origin + ray_dir * result_length;
                // C# WorldData.cs:461 — normal = mask × (rayDir<0 ? +1 : -1).
                let normal = Vec3::new(
                    mask.x * if ray_dir.x < 0.0 { 1.0 } else { -1.0 },
                    mask.y * if ray_dir.y < 0.0 { 1.0 } else { -1.0 },
                    mask.z * if ray_dir.z < 0.0 { 1.0 } else { -1.0 },
                );
                return Some(RayHit {
                    world_pos,
                    voxel_pos: cur_cell,
                    normal,
                    voxel_type: VoxelTypeId(hit_type),
                    distance: result_length,
                });
            }

            // C# WorldData.cs:465-469 — DDA step.
            let cur_pos_frac = Vec3::new(
                (is_negative.x as f32 - (cur_pos.x - cur_pos.x.trunc())).abs(),
                (is_negative.y as f32 - (cur_pos.y - cur_pos.y.trunc())).abs(),
                (is_negative.z as f32 - (cur_pos.z - cur_pos.z.trunc())).abs(),
            );
            let dist_for_intersect = ((Vec3::ONE + bounds_in_dir.as_vec3())
                - (Vec3::ONE - mask) * cur_pos_frac)
                * inv_ray_dir_abs;
            let min_dist = dist_for_intersect
                .x
                .min(dist_for_intersect.y)
                .min(dist_for_intersect.z);
            mask = Vec3::new(
                if min_dist >= dist_for_intersect.x { 1.0 } else { 0.0 },
                if min_dist >= dist_for_intersect.y { 1.0 } else { 0.0 },
                if min_dist >= dist_for_intersect.z { 1.0 } else { 0.0 },
            );
            cur_dist += min_dist.max(0.00001); // C# WorldData.cs:469 — min step 1e-5.
        }

        None
    }

    /// Look up the voxel type at a CPU-mirror world position by walking the
    /// 3-layer descent (chunk → block → voxel). Returns `None` if out of
    /// bounds, `Some(VoxelTypeId::EMPTY)` for empty voxels, `Some(ty)` for
    /// full voxels. Companion to `ray_traversal` — the brushes use this in the
    /// per-voxel non-empty check (`Paint.cs:75`).
    pub fn get_voxel_type(&self, pos: IVec3) -> Option<VoxelTypeId> {
        if pos.x < 0 || pos.y < 0 || pos.z < 0 {
            return None;
        }
        let size_v = self.size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
        if pos.x >= size_v.x as i32 || pos.y >= size_v.y as i32 || pos.z >= size_v.z as i32 {
            return None;
        }
        let chunk_pos = IVec3::new(pos.x / 16, pos.y / 16, pos.z / 16);
        let chunk_idx = (chunk_pos.x
            + chunk_pos.y * self.size_in_chunks.x as i32
            + chunk_pos.z * self.size_in_chunks.x as i32 * self.size_in_chunks.y as i32)
            as usize;
        if chunk_idx >= self.chunks_cpu.len() {
            return None;
        }
        let chunk_raw = self.chunks_cpu[chunk_idx];
        let chunk_state = chunk_raw >> 30;
        if chunk_state == 0 {
            return Some(VoxelTypeId::EMPTY);
        }
        if chunk_state == 1 {
            // Uniform Full chunk — chunk_raw low 15 bits = type.
            return Some(VoxelTypeId((chunk_raw & 0x7FFF) as u16));
        }
        // Mixed chunk — descend to block.
        let voxel_pos_in_chunk = IVec3::new(
            pos.x.rem_euclid(16),
            pos.y.rem_euclid(16),
            pos.z.rem_euclid(16),
        );
        let block_pos = voxel_pos_in_chunk / 4;
        let block_base = (chunk_raw & 0x3FFF_FFFF) as usize;
        let block_idx =
            block_base + (block_pos.x + block_pos.y * 4 + block_pos.z * 16) as usize;
        if block_idx >= self.blocks_cpu.len() {
            return None;
        }
        let block_raw = self.blocks_cpu[block_idx];
        let block_state = block_raw >> 30;
        if block_state == 0 {
            return Some(VoxelTypeId::EMPTY);
        }
        if block_state == 1 {
            return Some(VoxelTypeId((block_raw & 0x7FFF) as u16));
        }
        // Mixed block — descend to voxel pair.
        let voxel_pos_in_block = IVec3::new(
            pos.x.rem_euclid(4),
            pos.y.rem_euclid(4),
            pos.z.rem_euclid(4),
        );
        let voxel_base_pair = (block_raw & 0x3FFF_FFFF) as usize;
        let voxel_index = voxel_base_pair * 2
            + (voxel_pos_in_block.x + voxel_pos_in_block.y * 4 + voxel_pos_in_block.z * 16)
                as usize;
        let pair_idx = voxel_index / 2;
        if pair_idx >= self.voxels_cpu.len() {
            return None;
        }
        let cur_voxel_pair = self.voxels_cpu[pair_idx];
        let half = ((cur_voxel_pair >> (16 * (voxel_index & 0x1))) & 0xFFFF) as u16;
        if (half & 0x8000) != 0 {
            Some(VoxelTypeId(half & 0x7FFF))
        } else {
            Some(VoxelTypeId::EMPTY)
        }
    }

    /// Track-B bulk-edit **runtime fast path** — algorithmically aligned to
    /// C# `EditingHandler.processChunks` semantics
    /// (`docs/orchestrate/feature-completeness/02c-design-edit-pipeline-alignment.md`,
    /// Decision 1+2+3+6+7).
    ///
    /// Mirrors C# per-frame digest:
    /// - Per touched chunk: decode current state → mutate window → encode via
    ///   `process_edit_batch`.
    /// - Emit ONE `changed_chunks` entry per touched chunk (the new state).
    /// - **No whole-world AADF recompute.** The W3 GPU regime-2 self-perpetuating
    ///   queue refreshes stale AADFs over subsequent frames, same as C#'s
    ///   `WorldBoundHandler.Update` chain. The CPU mirror's chunk-layer AADF
    ///   bits stay stale on indirectly-affected chunks (matches C# `dataChunk`
    ///   at `WorldData.cs:381-394`); no CPU consumer reads those bits
    ///   (`ray_traversal` / `get_voxel_type` / `build_chunk_edit_window_from_world`
    ///   only read state + ptr/type — verified in `02c` §"CPU-mirror consistency
    ///   contract").
    /// - **C#'s `AddChangedChunk` gate** (`WorldData.cs:392-393`): enqueue the
    ///   chunk's group into `edited_groups` only when the empty/non-empty
    ///   content boundary flipped OR the new state is empty.
    ///
    /// Sanctioned divergences from C# (per `01-context.md` §2 + `02c` Decisions):
    /// - Simplified port appends fresh voxel/block slots (no free-list reuse;
    ///   long-session leak, accepted — `02c` Divergence #4 / Risk #6).
    /// - Per-chunk decode + mutate parallelised via `bevy_tasks::ComputeTaskPool`
    ///   (matches C# `Parallel.For` at `EditingHandler.cs:82`). Below an
    ///   8-chunk threshold falls back to serial per `02c` Risk #2.
    ///
    /// For the slow-but-bit-exact `chunks_cpu` invariant (full chunk-layer AADF
    /// recompute + synthetic chunk uploads — the pre-`02c` behaviour), see
    /// [`Self::set_voxels_batch_oracle`]. The `--edit-mode` e2e gate continues
    /// to call [`Self::set_voxel`] which preserves the oracle semantics.
    pub fn set_voxels_batch(&mut self, edits: &[VoxelEdit]) {
        if edits.is_empty() {
            return;
        }
        let chunk_size_voxels = (CELL_DIM * CELL_DIM) as u32; // 16
        let sx_v = self.size_in_chunks.x * chunk_size_voxels;
        let sy_v = self.size_in_chunks.y * chunk_size_voxels;
        let sz_v = self.size_in_chunks.z * chunk_size_voxels;
        if sx_v == 0 || sy_v == 0 || sz_v == 0 {
            return;
        }

        // Group by chunk_pos. Practical brush radii (≤16) touch ~125 chunks; a
        // sphere r=400 worst-case touches ~16k chunks (still HashMap-fine).
        // Insertion order within each chunk preserves caller's last-write-wins
        // semantics — important for tests that mutate the same voxel twice.
        let mut by_chunk: std::collections::HashMap<[u32; 3], Vec<([u32; 3], u16)>> =
            std::collections::HashMap::new();
        for &VoxelEdit { pos, ty } in edits {
            if pos.x < 0 || pos.y < 0 || pos.z < 0 {
                continue;
            }
            let p = [pos.x as u32, pos.y as u32, pos.z as u32];
            if p[0] >= sx_v || p[1] >= sy_v || p[2] >= sz_v {
                continue;
            }
            let chunk = [
                p[0] / chunk_size_voxels,
                p[1] / chunk_size_voxels,
                p[2] / chunk_size_voxels,
            ];
            let voxel_in_chunk = [
                p[0] % chunk_size_voxels,
                p[1] % chunk_size_voxels,
                p[2] % chunk_size_voxels,
            ];
            by_chunk
                .entry(chunk)
                .or_default()
                .push((voxel_in_chunk, ty.raw()));
        }
        if by_chunk.is_empty() {
            return;
        }

        // Build the per-chunk task list + snapshot pre-edit chunk states for
        // the SetChunk-AddChangedChunk gate below. Each chunk gets a disjoint
        // 2048-u32 slice in `edit_data`; offsets are i*2048.
        let chunk_count = by_chunk.len();
        let mut edit_data: Vec<u32> = vec![0; chunk_count * 2048];
        let mut edited_chunks: Vec<([u32; 3], u32)> = Vec::with_capacity(chunk_count);
        // Snapshot (chunk_idx, old_state) for the AddChangedChunk gate
        // (`WorldData.cs:392-393`).
        let mut old_states: Vec<(usize, u32)> = Vec::with_capacity(chunk_count);

        struct ChunkTask {
            chunk_idx: usize,
            edit_offset: u32,
            per_chunk_edits: Vec<([u32; 3], u16)>,
        }
        let mut tasks: Vec<ChunkTask> = Vec::with_capacity(chunk_count);
        for (i, (chunk_pos, per_chunk_edits)) in by_chunk.into_iter().enumerate() {
            let chunk_idx = (chunk_pos[0]
                + chunk_pos[1] * self.size_in_chunks.x
                + chunk_pos[2] * self.size_in_chunks.x * self.size_in_chunks.y)
                as usize;
            if chunk_idx >= self.chunks_cpu.len() {
                continue;
            }
            let edit_offset = (i * 2048) as u32;
            edited_chunks.push((chunk_pos, edit_offset));
            old_states.push((chunk_idx, self.chunks_cpu[chunk_idx]));
            tasks.push(ChunkTask { chunk_idx, edit_offset, per_chunk_edits });
        }

        if edited_chunks.is_empty() {
            return;
        }

        // Per-chunk decode + mutate. C# uses `Parallel.For` at
        // `EditingHandler.cs:82`; we use `bevy_tasks::ComputeTaskPool` as the
        // Bevy equivalent (`02c` Decision 7). Threshold: below 8 chunks the
        // task-spawn overhead dominates the serial cost; we fall back to
        // serial there + when the global pool isn't initialised (unit tests
        // running on `MinimalPlugins`).
        const PARALLEL_THRESHOLD: usize = 8;
        let chunks_cpu_ro: &[u32] = &self.chunks_cpu;
        let blocks_cpu_ro: &[u32] = &self.blocks_cpu;
        let voxels_cpu_ro: &[u32] = &self.voxels_cpu;
        let results: Vec<(u32, Vec<u32>)> = if tasks.len() >= PARALLEL_THRESHOLD
            && bevy::tasks::ComputeTaskPool::try_get().is_some()
        {
            let pool = bevy::tasks::ComputeTaskPool::get();
            pool.scope(|s| {
                for t in &tasks {
                    s.spawn(async move {
                        let mut window =
                            crate::aadf::edit::build_chunk_edit_window_from_world(
                                chunks_cpu_ro,
                                blocks_cpu_ro,
                                voxels_cpu_ro,
                                t.chunk_idx,
                            );
                        for (voxel_in_chunk, ty) in &t.per_chunk_edits {
                            crate::aadf::edit::set_voxel_in_window(
                                &mut window,
                                *voxel_in_chunk,
                                *ty,
                            );
                        }
                        (t.edit_offset, window)
                    });
                }
            })
        } else {
            tasks
                .iter()
                .map(|t| {
                    let mut window =
                        crate::aadf::edit::build_chunk_edit_window_from_world(
                            chunks_cpu_ro,
                            blocks_cpu_ro,
                            voxels_cpu_ro,
                            t.chunk_idx,
                        );
                    for (voxel_in_chunk, ty) in &t.per_chunk_edits {
                        crate::aadf::edit::set_voxel_in_window(
                            &mut window,
                            *voxel_in_chunk,
                            *ty,
                        );
                    }
                    (t.edit_offset, window)
                })
                .collect()
        };
        // Stitch the per-task windows back into the flat edit_data buffer.
        for (offset, window) in results {
            let off = offset as usize;
            edit_data[off..off + 2048].copy_from_slice(&window);
        }

        // Per-chunk encode with hash-dedup (port of C#
        // `EditingHandler.processChunks` — `EditingHandler.cs:75-180`):
        // for each edited chunk, walk its 64 new blocks; for mixed blocks,
        // dedup the 32-u32 voxel payload through `block_hashing.add_block`
        // (which reuses an existing slot when content matches, or allocates
        // a fresh slot from the free list / extends `voxels_cpu`). Then
        // decrement the OLD mixed blocks' refcounts via `delete_block`,
        // freeing slots that drop to zero.
        //
        // Order matches C# `processChunks:82-167`: AddBlock pass (Stage A)
        // runs BEFORE the DeleteBlock pass (Stage B). If an old block's
        // content survived the edit (no voxel positions inside it were
        // touched), Stage A bumps its `use_count` and Stage B decrements
        // it — net zero, no spurious free.
        let mut batch = crate::aadf::edit::EditBatch::default();
        for &(chunk_pos, edit_offset) in &edited_chunks {
            let cx = chunk_pos[0];
            let cy = chunk_pos[1];
            let cz = chunk_pos[2];
            let chunk_idx = (cx
                + cy * self.size_in_chunks.x
                + cz * self.size_in_chunks.x * self.size_in_chunks.y)
                as usize;
            let edit_base = edit_offset as usize;
            let old_chunk_state = self.chunks_cpu[chunk_idx];
            let old_chunk_was_mixed = (old_chunk_state >> 30) == 2;
            let old_block_ptr = if old_chunk_was_mixed {
                Some((old_chunk_state & 0x3FFF_FFFF) as usize)
            } else {
                None
            };

            // Stage A — build new_blocks via hash dedup on mixed blocks.
            let mut new_blocks = [0u32; crate::voxel::CELL_CHILDREN];
            let mut all_blocks_same = true;
            let mut reference_block: u32 = 0;
            for b in 0..crate::voxel::CELL_CHILDREN {
                let block_base = edit_base + b * 32;
                let first_pair = edit_data[block_base];
                let lo0 = first_pair & 0xFFFF;
                let hi0 = first_pair >> 16;
                let mut is_uniform = lo0 == hi0;
                if is_uniform {
                    for i in 0..32 {
                        let pair = edit_data[block_base + i];
                        if (pair & 0xFFFF) != lo0 || (pair >> 16) != lo0 {
                            is_uniform = false;
                            break;
                        }
                    }
                }
                if is_uniform {
                    let first_type =
                        (lo0 & crate::voxel::VOXEL_PAYLOAD_MASK as u32) as u16;
                    let state = if first_type == 0 { 0u32 } else { 1u32 };
                    new_blocks[b] = (first_type as u32) | (state << 30);
                } else {
                    // Mixed — pass the verbatim voxel data through hash dedup.
                    // The AADF bits of empty voxels are preserved here (no
                    // CPU-side zeroing); the GPU `apply_voxel_change` shader
                    // resets them locally before its additive AADF recompute,
                    // so its output is idempotent independent of input AADF
                    // state. This means the seed pass at construction time
                    // (which registered hashes computed over construction-
                    // time AADFs) and this edit pass produce matching hashes
                    // for unchanged blocks → `add_block` returns `is_new=false`
                    // and we skip the upload.
                    let mut payload = [0u32; 32];
                    for i in 0..32 {
                        payload[i] = edit_data[block_base + i];
                    }
                    let hash = self.block_hashing.compute_hash(&payload);
                    let (voxel_ptr, is_new) = self
                        .block_hashing
                        .add_block(hash, &payload, &mut self.voxels_cpu);
                    if is_new {
                        batch.changed_voxels.push(voxel_ptr);
                        for &v in &payload {
                            batch.changed_voxels.push(v);
                        }
                    }
                    new_blocks[b] = voxel_ptr | (2u32 << 30); // Mixed
                }
                if b == 0 {
                    reference_block = new_blocks[0];
                }
                if new_blocks[b] != reference_block {
                    all_blocks_same = false;
                }
            }

            // Stage B — free OLD voxel slots (C# processChunks:127-144).
            // Walk the OLD chunk's blocks, decrement refcount for each old
            // mixed block. If any drop to 0 they get queued for reuse.
            if let Some(old_bptr) = old_block_ptr {
                for b in 0..crate::voxel::CELL_CHILDREN {
                    let bi = old_bptr + b;
                    if bi >= self.blocks_cpu.len() {
                        break;
                    }
                    let old_block = self.blocks_cpu[bi];
                    if (old_block >> 30) != 2 {
                        continue;
                    }
                    let old_voxel_ptr = old_block & 0x3FFF_FFFF;
                    let vbase = old_voxel_ptr as usize;
                    if vbase + crate::aadf::block_hash::BLOCK_VOXEL_PAIRS
                        > self.voxels_cpu.len()
                    {
                        continue;
                    }
                    let mut buf = [0u32; crate::aadf::block_hash::BLOCK_VOXEL_PAIRS];
                    buf.copy_from_slice(
                        &self.voxels_cpu
                            [vbase..vbase + crate::aadf::block_hash::BLOCK_VOXEL_PAIRS],
                    );
                    let hash = self.block_hashing.compute_hash(&buf);
                    let _freed = self.block_hashing.delete_block(hash, old_voxel_ptr);
                }
            }

            // Stage C — write new blocks to blocks_cpu, allocate/reuse the
            // chunk's block pointer. C# `SetBlocks:332-342` reuses the
            // existing block slot when the chunk was already Mixed;
            // matching that here avoids a fresh 64-block allocation per
            // edit on an already-mixed chunk (and the corresponding
            // block-slot leak the simplified port had).
            let new_chunk_value: u32;
            if all_blocks_same {
                new_chunk_value = reference_block;
                // Note: when the chunk transitions Mixed → Empty/UniformFull,
                // the old 64-block slot is leaked here (no block-slot free
                // list yet — a future port adds `freeBlockSlots` matching
                // `WorldData.cs:39`). Voxel slots were already freed in
                // Stage B above.
            } else {
                let block_ptr = if let Some(old_bptr) = old_block_ptr {
                    // Reuse — write the new 64 blocks at the same offset.
                    old_bptr as u32
                } else {
                    // Allocate fresh — extend blocks_cpu by 64.
                    let p = self.blocks_cpu.len() as u32;
                    self.blocks_cpu.resize(self.blocks_cpu.len() + 64, 0);
                    p
                };
                // Always emit the 65-u32 changed_blocks record so the GPU
                // apply_block_change dispatch updates the block layer
                // (and recomputes BLOCK-layer AADFs on empty blocks).
                batch.changed_blocks.push(block_ptr);
                for b in 0..crate::voxel::CELL_CHILDREN {
                    batch.changed_blocks.push(new_blocks[b]);
                }
                let block_ptr_usize = block_ptr as usize;
                let target_len = block_ptr_usize + 64;
                if self.blocks_cpu.len() < target_len {
                    self.blocks_cpu.resize(target_len, 0);
                }
                // Mirror the GPU `apply_block_change` AADF computation on the
                // CPU mirror so `blocks_cpu` stays consistent with what the
                // renderer reads from `world_gpu.blocks`. Empty blocks get
                // their 2-bit AADFs recomputed; non-empty pass through.
                let mut raw = [0u32; 64];
                raw[..64].copy_from_slice(&new_blocks);
                crate::aadf::edit::apply_block_edit_cpu(
                    &mut self.blocks_cpu,
                    block_ptr,
                    &raw,
                );
                new_chunk_value = block_ptr | (2u32 << 30);
            }

            batch.changed_chunks.push([
                crate::aadf::edit::pack_chunk_pos(chunk_pos),
                new_chunk_value,
            ]);
            self.chunks_cpu[chunk_idx] = new_chunk_value;
        }
        // NOTE (`02e-perframe-cpu-investigation.md`, 2026-05-16): do NOT set
        // `self.dirty = true` here. The W2 delta chain (above — `pending_edits`
        // batch + `naadf_world_change_node` GPU dispatch) carries per-edit
        // changes to the GPU. The full-world re-extract `dirty` triggers is
        // redundant + caused the per-edit full-world re-upload bottleneck.

        // C#'s `WorldData.SetChunk` `AddChangedChunk` gate (`WorldData.cs:392-393`):
        // enqueue the chunk's group only when the empty/non-empty content
        // boundary flipped OR the new state is empty. Chunks that stay
        // Mixed-with-different-content don't enqueue — their AADFs don't
        // change because only Empty chunks carry AADFs at the chunk layer.
        //
        // Per `02c` Decision 3: NO `recompute_chunk_layer_aadfs` here. The
        // W3 GPU regime-2 self-perpetuating queue refreshes stale AADFs
        // incrementally over subsequent frames (`bounds_calc.wgsl`'s
        // re-enqueue at next-bound-size), seeded from these `edited_groups`
        // via `apply_group_change` (`world_change.wgsl:395-419`).
        let sx_c = self.size_in_chunks.x as usize;
        let sy_c = self.size_in_chunks.y as usize;
        for (ci, old_state) in &old_states {
            let new_state = self.chunks_cpu[*ci];
            let old_has_content = (old_state >> 30) != 0;
            let new_has_content = (new_state >> 30) != 0;
            if old_has_content != new_has_content || !new_has_content {
                let cz = ci / (sx_c * sy_c);
                let rem = ci % (sx_c * sy_c);
                let cy = rem / sx_c;
                let cx = rem % sx_c;
                self.pending_edits.edited_groups.push([
                    cx as u32 / CELL_DIM as u32,
                    cy as u32 / CELL_DIM as u32,
                    cz as u32 / CELL_DIM as u32,
                ]);
            }
        }
        self.pending_edits.batches.push(batch);
        // Note: `dense_voxel_types` is intentionally NOT updated here — same
        // behaviour as `set_voxel` (the GPU dispatch chain reads chunks/blocks/
        // voxels directly during edit strokes; `dense_voxel_types` is only
        // consulted on the initial-build path).
    }

    /// **Brush inside-chunk fast path** — bulk-fill a set of chunks each with
    /// a single uniform voxel type (or empty). Mirrors C#'s inside-chunk fast
    /// path at `EditingToolSphere.cs:91-100` / `EditingToolCube.cs:92-101`:
    /// `Array.Fill(editData, type | (type << 16), pointer, 2048)`.
    ///
    /// For each chunk in `chunks`, writes the new uniform state (UniformFull
    /// for `Some(ty)` with `ty != EMPTY`, Empty for `None`/`Some(EMPTY)`)
    /// directly into `chunks_cpu` and emits ONE `changed_chunks` entry.
    /// **Zero block/voxel uploads** — the new uniform chunk state has no
    /// pointer to fill.
    ///
    /// SetChunk's AddChangedChunk gate applies — enqueues into `edited_groups`
    /// only when the empty/non-empty boundary flipped or the new state is empty.
    ///
    /// Sanctioned divergence: leaks any prior block/voxel slots the overwritten
    /// chunk used (same simplified-port behaviour as `set_voxels_batch` — no
    /// free-list reuse, sanctioned per `02c` Divergence #4 / Risk #6).
    pub fn set_chunks_uniform_batch(
        &mut self,
        chunks: &[ChunkUniformEdit],
    ) {
        if chunks.is_empty() {
            return;
        }
        let sx = self.size_in_chunks.x;
        let sy = self.size_in_chunks.y;
        let sz = self.size_in_chunks.z;
        if sx == 0 || sy == 0 || sz == 0 {
            return;
        }
        let mut batch = crate::aadf::edit::EditBatch::default();
        for &ChunkUniformEdit { pos: chunk_pos, ty: ty_opt } in chunks {
            if chunk_pos.x >= sx || chunk_pos.y >= sy || chunk_pos.z >= sz {
                continue;
            }
            let ci = (chunk_pos.x
                + chunk_pos.y * sx
                + chunk_pos.z * sx * sy) as usize;
            if ci >= self.chunks_cpu.len() {
                continue;
            }
            // Encode the new chunk state.
            // - `Some(t)` with `t != EMPTY` → UniformFull(t) → state=1 | type.
            // - `None` OR `Some(EMPTY)` → Empty (AADF=0; W3 GPU queue will
            //   refresh it on subsequent frames).
            let new_state = match ty_opt {
                Some(t) if t != VoxelTypeId::EMPTY => {
                    (1u32 << 30) | (t.raw() as u32 & 0x7FFF)
                }
                _ => 0u32, // Empty with AADF=0
            };
            let old_state = self.chunks_cpu[ci];
            if old_state == new_state {
                continue; // No-op write.
            }
            self.chunks_cpu[ci] = new_state;
            batch.changed_chunks.push([
                crate::aadf::edit::pack_chunk_pos([chunk_pos.x, chunk_pos.y, chunk_pos.z]),
                new_state,
            ]);
            // SetChunk's AddChangedChunk gate.
            let old_has_content = (old_state >> 30) != 0;
            let new_has_content = (new_state >> 30) != 0;
            if old_has_content != new_has_content || !new_has_content {
                self.pending_edits.edited_groups.push([
                    chunk_pos.x / CELL_DIM as u32,
                    chunk_pos.y / CELL_DIM as u32,
                    chunk_pos.z / CELL_DIM as u32,
                ]);
            }
        }
        if !batch.changed_chunks.is_empty() {
            self.pending_edits.batches.push(batch);
            // NOTE (`02e-perframe-cpu-investigation.md`, 2026-05-16): do NOT
            // set `self.dirty = true` here. The pushed batch flows through the
            // W2 delta chain (`extract_world_changes` → `naadf_world_change_node`)
            // on the next frame; the full-world re-extract `dirty` triggers
            // is redundant and was the per-edit bottleneck on Oasis-class worlds.
        }
    }

    /// **DIAGNOSTIC-ONLY** bulk-edit oracle (`02f` rearch). Slow-but-bit-exact
    /// path — runs `recompute_chunk_layer_aadfs` over the whole world +
    /// emits synthetic `changed_chunks` entries for every AADF-changed
    /// chunk. O(N_chunks × 31 × 3) per call. **Do not call from production
    /// code paths.**
    ///
    /// Call sites:
    /// - CPU-fallback rendering (if `gpu_construction_enabled = false`,
    ///   currently not re-enabled).
    /// - Future regression tests pinning byte-exact `chunks_cpu` equality
    ///   with the C#-canonical "construct + edit + reconstruct" reference.
    /// - Unit tests in this file.
    ///
    /// **Production brushes call [`Self::set_voxels_batch`] instead.**
    ///
    /// Complexity: O(N_chunks × 31 × 3) per call. For Oasis-class worlds:
    /// ~75 ms per call. Never on the runtime hot path.
    #[doc(hidden)]
    pub fn set_voxels_batch_oracle(&mut self, edits: &[VoxelEdit]) {
        crate::world::oracle::set_voxels_batch_oracle(self, edits);
    }
}

/// Slab-method AABB entry distance for `origin + t * dir` against `[bmin, bmax]`.
/// Returns `None` if the ray misses or if the entry is behind the origin.
fn ray_aabb_entry_distance(origin: Vec3, dir: Vec3, bmin: Vec3, bmax: Vec3) -> Option<f32> {
    // Component-wise t-values; division by zero produces inf which sorts correctly.
    let t1 = (bmin - origin) / dir;
    let t2 = (bmax - origin) / dir;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    if tmax < tmin.max(0.0) {
        None
    } else {
        Some(tmin.max(0.0))
    }
}

fn aabb_contains_point(bmin: Vec3, bmax: Vec3, p: Vec3) -> bool {
    p.x >= bmin.x && p.x <= bmax.x
        && p.y >= bmin.y && p.y <= bmax.y
        && p.z >= bmin.z && p.z <= bmax.z
}

/// Phase-C W2 — staging area on `WorldData` for the per-frame edit batches
/// (`15-design-c.md` §2.1 W2). Each frame, `extract_world_changes` drains
/// this into the render-world `ConstructionEvents` + the per-buffer upload
/// queues consumed by `world_change.wgsl`.
#[derive(Debug, Default, Clone)]
pub struct PendingEdits {
    /// Per-edit batches (one per `set_voxel` call, or batched). Each batch
    /// holds `changed_chunks` / `changed_blocks` / `changed_voxels` arrays in
    /// NAADF on-wire format.
    pub batches: Vec<crate::aadf::edit::EditBatch>,
    /// Group positions of every directly-edited group (used as the input to
    /// `change_handler::compute_change_groups`).
    pub edited_groups: Vec<[u32; 3]>,
}

/// The voxel-type palette (`03-design.md` §4.4, ported from
/// `World/VoxelTypeHandler.cs`).
///
/// Element `0` is the reserved empty placeholder (C# convention) — voxel
/// 15-bit type ids index into `types`.
///
/// **Built once at startup, never mutated at runtime** (`02f` rearch deletes
/// the `dirty` flag — no code path mutates this resource after the initial
/// `setup_test_grid` / `build_world_from_vox` insertion).
#[derive(Resource, Debug)]
pub struct VoxelTypes {
    /// The palette. `types[0]` is the empty placeholder.
    pub types: Vec<VoxelType>,
}

impl Default for VoxelTypes {
    /// A palette holding just the reserved empty placeholder.
    fn default() -> Self {
        Self {
            types: vec![VoxelType::default()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voxel::CELL_DIM;

    /// Build an empty 2×2×2-chunk WorldData with all-empty chunks_cpu (which is
    /// the default `ChunkCell::Empty(zero AADF)` encoded as 0).
    fn make_empty_world(size_in_chunks: UVec3) -> WorldData {
        let n_chunks = (size_in_chunks.x * size_in_chunks.y * size_in_chunks.z) as usize;
        let size_v = size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
        WorldData {
            chunks_cpu: vec![0u32; n_chunks],
            blocks_cpu: Vec::new(),
            voxels_cpu: Vec::new(),
            size_in_chunks,
            bounding_box: IAabb3 {
                min: IVec3::ZERO,
                max: IVec3::new(size_v.x as i32 - 1, size_v.y as i32 - 1, size_v.z as i32 - 1),
            },
            pending_edits: PendingEdits::default(),
            dense_voxel_types: Vec::new(),
            block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
        }
    }

    /// Test #1 — ray on all-empty world returns None.
    #[test]
    fn ray_traversal_misses_empty_world() {
        let wd = make_empty_world(UVec3::new(2, 2, 2));
        let hit = wd.ray_traversal(Vec3::new(1.0, 16.0, 1.0), Vec3::X);
        assert!(hit.is_none(), "expected empty world miss; got {hit:?}");
    }

    /// Test #5 — `set_voxels_batch` produces the same effective per-voxel
    /// state as N sequential `set_voxel` calls. Raw `chunks_cpu`/`blocks_cpu`
    /// /`voxels_cpu` BYTES differ because the simplified port appends FRESH
    /// slots on every edit (different slot pointers between paths); the
    /// invariant we pin is "resolved voxel type matches per voxel". Picks
    /// 3 voxels in one chunk + 1 voxel in a different chunk so the multi-
    /// chunk batching path is exercised.
    #[test]
    fn set_voxels_batch_byte_equals_per_voxel_loop() {
        let edits = [
            VoxelEdit { pos: IVec3::new(2, 3, 4), ty: VoxelTypeId(1) },
            VoxelEdit { pos: IVec3::new(5, 5, 5), ty: VoxelTypeId(2) },
            VoxelEdit { pos: IVec3::new(7, 1, 2), ty: VoxelTypeId(3) },
            VoxelEdit { pos: IVec3::new(20, 4, 4), ty: VoxelTypeId(4) }, // different chunk
        ];

        // Per-voxel reference run.
        let mut wd_a = make_empty_world(UVec3::new(2, 2, 2));
        for &VoxelEdit { pos, ty } in &edits {
            wd_a.set_voxel(pos, ty);
        }
        // Batched run.
        let mut wd_b = make_empty_world(UVec3::new(2, 2, 2));
        wd_b.set_voxels_batch(&edits);

        // Per-voxel effective-state equivalence — the invariant callers care
        // about (the raw byte buffers diverge because `set_voxel` appends
        // fresh slots on every call, while `set_voxels_batch` appends once
        // per affected chunk).
        for &VoxelEdit { pos, ty } in &edits {
            let a = wd_a.get_voxel_type(pos);
            let b = wd_b.get_voxel_type(pos);
            assert_eq!(a, b, "voxel at {pos:?}: per-voxel={a:?} batched={b:?}");
            assert_eq!(b, Some(ty), "voxel at {pos:?}: expected {ty:?}, got {b:?}");
        }
        // Post-`02c`: `set_voxel` keeps the oracle behaviour (recomputes the
        // whole-world chunk-layer AADFs, emits synthetic entries for every
        // AADF-changed chunk — typically a superset of the directly-edited
        // chunks), while `set_voxels_batch` is now the runtime fast path
        // (only the directly-edited chunks). The invariant we pin now is:
        // every directly-edited chunk is touched by both paths (the runtime
        // path's chunks are a subset of the oracle path's).
        let a_chunks: std::collections::HashSet<u32> =
            wd_a.pending_edits.batches.iter().flat_map(|b| b.changed_chunks.iter().map(|e| e[0])).collect();
        let b_chunks: std::collections::HashSet<u32> =
            wd_b.pending_edits.batches.iter().flat_map(|b| b.changed_chunks.iter().map(|e| e[0])).collect();
        assert!(
            b_chunks.is_subset(&a_chunks),
            "runtime-path chunks {b_chunks:?} must be a subset of oracle-path chunks {a_chunks:?}"
        );
        // The runtime path must touch the directly-edited chunks. The fixture
        // edits voxels in chunks (0,0,0) and (1,0,0).
        let want_chunk_0 = crate::aadf::edit::pack_chunk_pos([0, 0, 0]);
        let want_chunk_1 = crate::aadf::edit::pack_chunk_pos([1, 0, 0]);
        assert!(b_chunks.contains(&want_chunk_0), "runtime path missed chunk (0,0,0)");
        assert!(b_chunks.contains(&want_chunk_1), "runtime path missed chunk (1,0,0)");
    }

    /// Test #6 — empty input is a no-op.
    #[test]
    fn set_voxels_batch_empty_is_noop() {
        let mut wd = make_empty_world(UVec3::new(2, 2, 2));
        let chunks_before = wd.chunks_cpu.clone();
        let blocks_before = wd.blocks_cpu.clone();
        let voxels_before = wd.voxels_cpu.clone();
        let pending_before = wd.pending_edits.batches.len();
        wd.set_voxels_batch(&[]);
        assert_eq!(wd.chunks_cpu, chunks_before);
        assert_eq!(wd.blocks_cpu, blocks_before);
        assert_eq!(wd.voxels_cpu, voxels_before);
        assert_eq!(wd.pending_edits.batches.len(), pending_before);
    }

    /// Test #2 — ray hits a known voxel placed via set_voxel; verifies the
    /// 3-layer descent end-to-end on a Mixed → Mixed → Full path.
    #[test]
    fn ray_traversal_hits_known_voxel() {
        let mut wd = make_empty_world(UVec3::new(2, 2, 2));
        // Place a full voxel at (5, 5, 5) of type 7.
        wd.set_voxel(IVec3::new(5, 5, 5), VoxelTypeId(7));
        // Shoot a ray from outside the world (along -X looking +X) toward (5,5,5).
        let origin = Vec3::new(-5.0, 5.5, 5.5);
        let dir = Vec3::X;
        let hit = wd.ray_traversal(origin, dir).expect("expected hit");
        assert_eq!(hit.voxel_pos, IVec3::new(5, 5, 5));
        assert_eq!(hit.voxel_type, VoxelTypeId(7));
    }

    /// Test #3 — normal of a +X ray entering a full voxel from the -X side
    /// must be `(-1, 0, 0)` (the hit face points back at the ray).
    #[test]
    fn ray_traversal_normal_is_face_normal() {
        let mut wd = make_empty_world(UVec3::new(2, 2, 2));
        wd.set_voxel(IVec3::new(10, 5, 5), VoxelTypeId(7));
        let origin = Vec3::new(0.5, 5.5, 5.5);
        let dir = Vec3::X;
        let hit = wd.ray_traversal(origin, dir).expect("hit expected");
        // Face normal for a +X ray hitting a voxel from -X: normal is -X.
        // C# WorldData.cs:461 — normal = mask × (rayDir<0 ? +1 : -1). For a
        // pure +X ray, mask after stepping = (1,0,0), rayDir.x > 0 so factor
        // = -1, giving normal (-1, 0, 0).
        assert!(
            (hit.normal - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-3,
            "expected (-1,0,0) face normal; got {:?}", hit.normal,
        );
    }

    /// `02c` Test #11 — `set_voxels_batch_oracle` preserves the pre-`02c`
    /// behaviour (full chunk-layer AADF recompute + synthetic chunk uploads).
    /// Assert the oracle's `chunks_cpu` has AADF bits populated on the
    /// indirectly-affected chunks AND `changed_chunks` carries the synthetic
    /// entries — distinguishing it from the runtime path which emits only
    /// the directly-edited chunk.
    #[test]
    fn set_voxels_batch_oracle_emits_synthetic_aadf_entries() {
        // 4×4×4 world. Place one voxel; the recompute will refresh AADFs on
        // surrounding empty chunks.
        let mut wd = make_empty_world(UVec3::new(4, 4, 4));
        wd.set_voxels_batch_oracle(&[VoxelEdit { pos: IVec3::new(32, 32, 32), ty: VoxelTypeId(5) }]);
        let batch = wd
            .pending_edits
            .batches
            .first()
            .expect("expected one batch");
        // The oracle path produces >1 changed_chunks entries (the directly-
        // edited chunk plus synthetic entries for AADF-changed empty chunks).
        // The runtime path produces exactly 1; this is the distinguishing
        // invariant.
        assert!(
            batch.changed_chunks.len() > 1,
            "oracle path must emit synthetic AADF entries: got {} changed_chunks",
            batch.changed_chunks.len()
        );
    }

    /// Test #4 — round-trip: `(origin + dir * distance)` ≈ `world_pos`.
    #[test]
    fn ray_traversal_distance_within_eps_of_world_pos() {
        let mut wd = make_empty_world(UVec3::new(2, 2, 2));
        wd.set_voxel(IVec3::new(10, 5, 5), VoxelTypeId(7));
        let origin = Vec3::new(0.5, 5.5, 5.5);
        let dir = Vec3::X;
        let hit = wd.ray_traversal(origin, dir).expect("hit");
        let reconstructed = origin + dir * hit.distance;
        assert!(
            (reconstructed - hit.world_pos).length() < 1e-3,
            "ray reconstruction mismatch: rec={reconstructed:?} world_pos={:?}",
            hit.world_pos,
        );
    }

    /// `03g` — Mode 2 reproducer variant — single voxel via cube_brush
    /// emit-shape into populated chunk. Place the voxel at intra-block
    /// position that is the high half-word of its u32 pair, to flush out
    /// any `set_voxel_in_window` packing bug.
    #[test]
    fn small_edit_high_half_voxel_no_phantoms() {
        let mut wd = make_empty_world(UVec3::new(2, 2, 2));
        // Voxel index 1 inside the block (1,0,0) is high-half. To target
        // an "odd voxel-index" position pick voxel (1, 0, 0) of block (0,0,0).
        // Pre-populate surrounding voxels so the chunk is Mixed.
        wd.set_voxels_batch(&[
            VoxelEdit { pos: IVec3::new(0, 0, 0), ty: VoxelTypeId(1) },
            VoxelEdit { pos: IVec3::new(2, 0, 0), ty: VoxelTypeId(1) },
        ]);
        // Click on (1, 0, 0) — high half of u32 pair (0, 0, 0).
        let mut around: Vec<(IVec3, Option<VoxelTypeId>)> = Vec::new();
        for x in 0..=3 {
            for y in 0..=3 {
                for z in 0..=3 {
                    let p = IVec3::new(x, y, z);
                    around.push((p, wd.get_voxel_type(p)));
                }
            }
        }
        wd.set_voxels_batch(&[VoxelEdit { pos: IVec3::new(1, 0, 0), ty: VoxelTypeId(2) }]);
        assert_eq!(
            wd.get_voxel_type(IVec3::new(1, 0, 0)),
            Some(VoxelTypeId(2)),
            "clicked voxel must be the target type"
        );
        for (p, pre) in &around {
            if *p == IVec3::new(1, 0, 0) {
                continue;
            }
            let post = wd.get_voxel_type(*p);
            assert_eq!(
                pre, &post,
                "voxel at {p:?} changed unexpectedly: pre={pre:?} post={post:?}"
            );
        }
    }

    /// `03g` — Mode 2 reproducer — placing into a UniformFull chunk.
    /// The chunk's pre-state is `UniformFull(ty=1)`. Clicking a single
    /// voxel of a different type should keep all OTHER voxels at type 1,
    /// not flip any neighbours.
    #[test]
    fn small_edit_into_uniform_full_chunk_no_phantoms() {
        let mut wd = make_empty_world(UVec3::new(2, 2, 2));
        // Pre-populate chunk (0,0,0) as UniformFull(1) via set_chunks_uniform_batch.
        wd.set_chunks_uniform_batch(&[ChunkUniformEdit {
            pos: UVec3::new(0, 0, 0),
            ty: Some(VoxelTypeId(1)),
        }]);
        // Now click voxel (5, 5, 5) to type 2.
        let mut around: Vec<(IVec3, Option<VoxelTypeId>)> = Vec::new();
        for x in 3..=7 {
            for y in 4..=6 {
                for z in 4..=6 {
                    let p = IVec3::new(x, y, z);
                    around.push((p, wd.get_voxel_type(p)));
                }
            }
        }
        wd.set_voxels_batch(&[VoxelEdit { pos: IVec3::new(5, 5, 5), ty: VoxelTypeId(2) }]);
        assert_eq!(
            wd.get_voxel_type(IVec3::new(5, 5, 5)),
            Some(VoxelTypeId(2)),
            "clicked voxel must be the target type"
        );
        for (p, pre) in &around {
            if *p == IVec3::new(5, 5, 5) {
                continue;
            }
            let post = wd.get_voxel_type(*p);
            assert_eq!(
                pre, &post,
                "voxel at {p:?} changed unexpectedly: pre={pre:?} post={post:?}"
            );
        }
    }

    /// `03g` — Mode 2 reproducer (Phase 3 diagnosis).
    ///
    /// Single voxel placed via `set_voxels_batch` into a chunk that already
    /// contains pre-existing voxels (the user's "OXO row in populated world"
    /// scenario). Asserts that exactly ONE voxel position changed from EMPTY
    /// to the target type — no phantom voxels at adjacent positions.
    ///
    /// If `set_voxels_batch` writes both halves of a packed `u32` when only
    /// one was intended (`02-research.md` divergence #4 hazard), the sibling
    /// voxel at the same `u32` storage slot would also become non-empty.
    ///
    /// User-reported failure mode: `OXO` row in the middle of a populated
    /// world; after click, `OXO → ONO` (expected) **plus** an `NN` row one
    /// position below in the chunk — phantoms at sibling-half-word
    /// positions.
    #[test]
    fn small_edit_one_voxel_into_populated_chunk_emits_exactly_one() {
        let mut wd = make_empty_world(UVec3::new(2, 2, 2));
        // Seed a populated context — a 3-voxel row "OXO" around (5, 5, 5)
        // with the centre EMPTY. Voxels at (4,5,5) and (6,5,5) are full.
        wd.set_voxels_batch(&[
            VoxelEdit { pos: IVec3::new(4, 5, 5), ty: VoxelTypeId(1) },
            VoxelEdit { pos: IVec3::new(6, 5, 5), ty: VoxelTypeId(1) },
        ]);
        // Sanity — middle is empty before the edit.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(5, 5, 5)),
            Some(VoxelTypeId::EMPTY),
            "pre-edit centre voxel must be empty"
        );
        // Snapshot the surrounding 5×3×3 voxel region — every voxel that was
        // empty must STAY empty after the click except the clicked one.
        let mut around: Vec<(IVec3, Option<VoxelTypeId>)> = Vec::new();
        for x in 3..=7 {
            for y in 4..=6 {
                for z in 4..=6 {
                    let p = IVec3::new(x, y, z);
                    around.push((p, wd.get_voxel_type(p)));
                }
            }
        }
        // Click in the middle — single voxel set, simulating cube_brush
        // radius=1 with one emitted edit.
        wd.set_voxels_batch(&[VoxelEdit { pos: IVec3::new(5, 5, 5), ty: VoxelTypeId(2) }]);
        // The clicked voxel is the target type.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(5, 5, 5)),
            Some(VoxelTypeId(2)),
            "clicked voxel must be the target type"
        );
        // EVERY other voxel in the surrounding region must be unchanged.
        for (p, pre) in &around {
            if *p == IVec3::new(5, 5, 5) {
                continue;
            }
            let post = wd.get_voxel_type(*p);
            assert_eq!(
                pre, &post,
                "voxel at {p:?} changed unexpectedly: pre={pre:?} post={post:?} \
                 — phantom voxel (Mode 2)"
            );
        }
    }
}
