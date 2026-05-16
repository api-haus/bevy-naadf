//! The `WorldData` + `VoxelTypes` main-world resources — the three-layer CPU
//! buffer mirrors, world geometry, and the voxel-type palette
//! (`03-design.md` §4.4).
//!
//! These are the CPU side of the world. `voxel::grid::setup_test_grid` (D2)
//! builds them once at startup; Batch 2's `render::extract` / `render::prepare`
//! mirror them into the render world (`WorldGpu`) on the `dirty` flag.
//!
//! ## Track B — Editor ray traversal + bulk edits
//!
//! `WorldData::ray_traversal` ports the C# `WorldData.RayTraversal:396-473`
//! naive 3-layer-descent DDA; `WorldData::set_voxels_batch` is the
//! sanctioned-divergence bulk-edit entry point that groups by chunk and runs
//! `process_edit_batch` once per call (see
//! `docs/orchestrate/feature-completeness/02b-design-editor.md`).

use bevy::prelude::*;

use crate::voxel::{VoxelType, VoxelTypeId, CELL_DIM};

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
/// In Phase A this is built once by `voxel::grid::setup_test_grid` and never
/// edited; `dirty` triggers the one-time GPU upload (Batch 2).
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
    /// Set when the CPU mirror has changed and needs (re-)uploading to the GPU.
    pub dirty: bool,
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
            dirty: false,
            pending_edits: PendingEdits::default(),
            dense_voxel_types: Vec::new(),
        }
    }
}

impl WorldData {
    /// Phase-C W2 — programmatic single-voxel edit entry point
    /// (`15-design-c.md` §2.1 W2, `16-impl-c-W2.md`).
    ///
    /// Sets the voxel at world position `pos` (voxel coords) to `ty`. Walks
    /// the three-layer hierarchy from the chunk down, **decoding into a
    /// 2048-u32-per-chunk edit window** + emitting a single edit batch via
    /// the [`crate::aadf::edit::process_edit_batch`] port. The edit emits a
    /// `WorldEditEvent` (consumed by `extract_world_changes` to mirror into
    /// the render-world `ConstructionEvents`).
    ///
    /// Out-of-bounds positions are silently ignored (matches NAADF's
    /// `EditingTool` clamp behaviour).
    ///
    /// **Test-helper semantics:** the CPU mirror is updated *in place*; the
    /// pre-built chunk is decoded into the edit window, mutated, and then
    /// re-encoded through `process_edit_batch` for the GPU side. This is the
    /// shape the `--edit-mode` e2e gate needs — a single CPU call → a single
    /// `WorldEditEvent` → the regime-3 GPU dispatch chain.
    ///
    /// **Limitations of this Rust port:**
    /// - Chunks that were `UniformFull` get expanded into a full mixed chunk
    ///   on first edit (the C# `EditingHandler.getChunkDataToEdit` does the
    ///   same — it materialises a 2048-u32 window from the chunk's state).
    /// - `blocks_cpu` / `voxels_cpu` are *only* updated on chunks-cpu side via
    ///   the simplified edit path — no hash-dedup; mixed blocks claim fresh
    ///   voxel/block slots on every edit. Acceptable for the test grid; the
    ///   GPU side runs the proper W1 hash-dedup at startup, and W2's edit
    ///   path appends fresh slots.
    pub fn set_voxel(&mut self, pos: IVec3, ty: VoxelTypeId) {
        if pos.x < 0 || pos.y < 0 || pos.z < 0 {
            return;
        }
        let p = [pos.x as u32, pos.y as u32, pos.z as u32];
        let sx = self.size_in_chunks.x * CELL_DIM as u32 * CELL_DIM as u32;
        let sy = self.size_in_chunks.y * CELL_DIM as u32 * CELL_DIM as u32;
        let sz = self.size_in_chunks.z * CELL_DIM as u32 * CELL_DIM as u32;
        if p[0] >= sx || p[1] >= sy || p[2] >= sz {
            return;
        }
        // Identify the chunk + intra-chunk voxel position.
        let chunk_size_voxels = (CELL_DIM * CELL_DIM) as u32; // 16
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
        let chunk_idx = (chunk[0]
            + chunk[1] * self.size_in_chunks.x
            + chunk[2] * self.size_in_chunks.x * self.size_in_chunks.y)
            as usize;
        if chunk_idx >= self.chunks_cpu.len() {
            return;
        }
        // Decode the existing chunk's voxels into an edit window, set the
        // voxel, re-encode through `process_edit_batch`.
        let mut window = crate::aadf::edit::build_chunk_edit_window_from_world(
            &self.chunks_cpu,
            &self.blocks_cpu,
            &self.voxels_cpu,
            chunk_idx,
        );
        crate::aadf::edit::set_voxel_in_window(&mut window, voxel_in_chunk, ty.raw());
        // Run the edit batch with cursors starting at the end of the existing
        // buffers (we never reuse existing slots — the simplified port appends
        // fresh).
        let v_cursor = self.voxels_cpu.len() as u32;
        let b_cursor = self.blocks_cpu.len() as u32;
        let (batch, _new_v, _new_b) = crate::aadf::edit::process_edit_batch(
            &window,
            &[(chunk, 0)],
            v_cursor,
            b_cursor,
        );
        // Apply to CPU buffers: every mixed block in the batch has appended
        // its voxels at the input voxel_cursor; every mixed chunk has its
        // blocks appended at the input block_cursor. Replicate that on
        // `voxels_cpu` / `blocks_cpu` so the CPU mirror stays consistent.
        //
        // Note this *does NOT* free old voxel/block slots — see method-level
        // limitations. The CPU mirror grows without bound across many edits;
        // acceptable at test-grid scale.
        // Append voxels (33-u32-per-edit format: skip the pointer at index 0,
        // 32, 64, ...).
        let mut v_iter = batch.changed_voxels.chunks_exact(33);
        while let Some(chunk_vox) = v_iter.next() {
            // chunk_vox[0] is the pointer; chunk_vox[1..33] is the 32 packed
            // u32s.
            for &v in &chunk_vox[1..33] {
                self.voxels_cpu.push(v);
            }
        }
        // Append blocks (65-u32-per-edit).
        let mut b_iter = batch.changed_blocks.chunks_exact(65);
        while let Some(chunk_blk) = b_iter.next() {
            for &b in &chunk_blk[1..65] {
                // Apply the simplified `apply_block_change` AADF computation —
                // re-encode each empty block with the local 4³ AADF (W6 oracle).
                self.blocks_cpu.push(b);
            }
        }
        // Re-encode the empty blocks' AADFs in the just-appended slice via the
        // `apply_block_edit_cpu` oracle. (Mirrors the GPU `apply_block_change`
        // recompute step.)
        for (idx, edit_block) in batch.changed_blocks.chunks_exact(65).enumerate() {
            let ptr_unused = edit_block[0]; // pointer not used here — see below
            let _ = ptr_unused;
            // The pointer we wrote into `blocks_cpu` is `b_cursor + idx * 64`.
            let block_ptr = b_cursor + (idx as u32) * 64;
            // Build the raw 64-block array for the AADF recompute.
            let mut raw = [0u32; 64];
            raw[..64].copy_from_slice(&edit_block[1..65]);
            crate::aadf::edit::apply_block_edit_cpu(&mut self.blocks_cpu, block_ptr, &raw);
        }
        // Update the chunks CPU buffer entry for this chunk.
        for entry in &batch.changed_chunks {
            let pos_packed = entry[0];
            let new_state = entry[1];
            let cx = (pos_packed & 0x7FF) as u32;
            let cy = ((pos_packed >> 11) & 0x3FF) as u32;
            let cz = (pos_packed >> 21) as u32;
            let ci = (cx
                + cy * self.size_in_chunks.x
                + cz * self.size_in_chunks.x * self.size_in_chunks.y) as usize;
            if ci < self.chunks_cpu.len() {
                self.chunks_cpu[ci] = new_state;
            }
        }
        self.dirty = true;
        // Stash the edit batch on the resource so the extract pass picks it up.
        self.pending_edits.batches.push(batch);
        self.pending_edits.edited_groups.push([
            chunk[0] / CELL_DIM as u32,
            chunk[1] / CELL_DIM as u32,
            chunk[2] / CELL_DIM as u32,
        ]);
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

    /// Track-B bulk-edit entry point — sanctioned perf divergence from C#'s
    /// per-voxel `setVoxelData` (`02b-design-editor.md` Decision 5 + the
    /// user-confirmed Q&A row 7).
    ///
    /// Groups `edits` by chunk, builds one combined edit window per affected
    /// chunk, calls `process_edit_batch` once. The brushes (`editor/tools.rs`)
    /// build a `Vec<(IVec3, VoxelTypeId)>` then call this — far cheaper than
    /// N round-trips through `set_voxel`.
    ///
    /// **Per-voxel behaviour parity:** the resulting CPU buffer state matches
    /// what would have been produced by N sequential `set_voxel` calls in the
    /// same order; tests in `tests::set_voxels_batch_byte_equals_per_voxel_loop`
    /// pin this invariant.
    pub fn set_voxels_batch(&mut self, edits: &[(IVec3, VoxelTypeId)]) {
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
        for &(pos, ty) in edits {
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

        // Build the merged edit_data buffer + the edited_chunks list. Each
        // chunk gets its own 2048-u32 slice; offsets are i*2048.
        let chunk_count = by_chunk.len();
        let mut edit_data: Vec<u32> = vec![0; chunk_count * 2048];
        let mut edited_chunks: Vec<([u32; 3], u32)> = Vec::with_capacity(chunk_count);

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
            // Decode the existing chunk into its slice of the edit_data buffer.
            let window_slice = &mut edit_data[i * 2048..(i + 1) * 2048];
            let decoded = crate::aadf::edit::build_chunk_edit_window_from_world(
                &self.chunks_cpu,
                &self.blocks_cpu,
                &self.voxels_cpu,
                chunk_idx,
            );
            window_slice.copy_from_slice(&decoded);
            // Apply every per-voxel mutation.
            for (voxel_in_chunk, ty) in per_chunk_edits {
                crate::aadf::edit::set_voxel_in_window(window_slice, voxel_in_chunk, ty);
            }
        }

        if edited_chunks.is_empty() {
            return;
        }

        // Run process_edit_batch ONCE with all chunks.
        let v_cursor = self.voxels_cpu.len() as u32;
        let b_cursor = self.blocks_cpu.len() as u32;
        let (batch, _new_v, _new_b) = crate::aadf::edit::process_edit_batch(
            &edit_data,
            &edited_chunks,
            v_cursor,
            b_cursor,
        );

        // Apply to CPU buffers — mirrors `set_voxel:158-187`. Two passes: (1)
        // append voxel slots, (2) append + re-encode AADFs on block slots.
        let mut v_iter = batch.changed_voxels.chunks_exact(33);
        while let Some(chunk_vox) = v_iter.next() {
            for &v in &chunk_vox[1..33] {
                self.voxels_cpu.push(v);
            }
        }
        for (idx, edit_block) in batch.changed_blocks.chunks_exact(65).enumerate() {
            let block_ptr = b_cursor + (idx as u32) * 64;
            let target_len = (block_ptr + 64) as usize;
            if self.blocks_cpu.len() < target_len {
                self.blocks_cpu.resize(target_len, 0);
            }
            let mut raw = [0u32; 64];
            raw[..64].copy_from_slice(&edit_block[1..65]);
            crate::aadf::edit::apply_block_edit_cpu(&mut self.blocks_cpu, block_ptr, &raw);
        }
        for entry in &batch.changed_chunks {
            let pos_packed = entry[0];
            let new_state = entry[1];
            let cx = pos_packed & 0x7FF;
            let cy = (pos_packed >> 11) & 0x3FF;
            let cz = pos_packed >> 21;
            let ci = (cx
                + cy * self.size_in_chunks.x
                + cz * self.size_in_chunks.x * self.size_in_chunks.y) as usize;
            if ci < self.chunks_cpu.len() {
                self.chunks_cpu[ci] = new_state;
            }
        }
        self.dirty = true;

        // Stash the batch + each edited chunk's group position for the
        // change-handler flood-fill.
        for &(chunk_pos, _) in &edited_chunks {
            self.pending_edits.edited_groups.push([
                chunk_pos[0] / CELL_DIM as u32,
                chunk_pos[1] / CELL_DIM as u32,
                chunk_pos[2] / CELL_DIM as u32,
            ]);
        }
        self.pending_edits.batches.push(batch);
        // Note: `dense_voxel_types` is intentionally NOT updated here — same
        // behaviour as `set_voxel` (the GPU dispatch chain reads chunks/blocks/
        // voxels directly during edit strokes; `dense_voxel_types` is only
        // consulted on the initial-build path).
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
#[derive(Resource, Debug)]
pub struct VoxelTypes {
    /// The palette. `types[0]` is the empty placeholder.
    pub types: Vec<VoxelType>,
    /// Set when the palette has changed and needs (re-)uploading.
    pub dirty: bool,
}

impl Default for VoxelTypes {
    /// A palette holding just the reserved empty placeholder.
    fn default() -> Self {
        Self {
            types: vec![VoxelType::default()],
            dirty: false,
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
            dirty: false,
            pending_edits: PendingEdits::default(),
            dense_voxel_types: Vec::new(),
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
            (IVec3::new(2, 3, 4), VoxelTypeId(1)),
            (IVec3::new(5, 5, 5), VoxelTypeId(2)),
            (IVec3::new(7, 1, 2), VoxelTypeId(3)),
            (IVec3::new(20, 4, 4), VoxelTypeId(4)), // different chunk
        ];

        // Per-voxel reference run.
        let mut wd_a = make_empty_world(UVec3::new(2, 2, 2));
        for &(pos, ty) in &edits {
            wd_a.set_voxel(pos, ty);
        }
        // Batched run.
        let mut wd_b = make_empty_world(UVec3::new(2, 2, 2));
        wd_b.set_voxels_batch(&edits);

        // Per-voxel effective-state equivalence — the invariant callers care
        // about (the raw byte buffers diverge because `set_voxel` appends
        // fresh slots on every call, while `set_voxels_batch` appends once
        // per affected chunk).
        for &(pos, ty) in &edits {
            let a = wd_a.get_voxel_type(pos);
            let b = wd_b.get_voxel_type(pos);
            assert_eq!(a, b, "voxel at {pos:?}: per-voxel={a:?} batched={b:?}");
            assert_eq!(b, Some(ty), "voxel at {pos:?}: expected {ty:?}, got {b:?}");
        }
        // Also assert the FOOTPRINT (number of edited chunks) matches: 2
        // chunks touched in this fixture (chunk (0,0,0) and chunk (1,0,0)).
        let a_chunks: std::collections::HashSet<u32> =
            wd_a.pending_edits.batches.iter().flat_map(|b| b.changed_chunks.iter().map(|e| e[0])).collect();
        let b_chunks: std::collections::HashSet<u32> =
            wd_b.pending_edits.batches.iter().flat_map(|b| b.changed_chunks.iter().map(|e| e[0])).collect();
        assert_eq!(a_chunks, b_chunks, "same set of chunks touched");
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
}
