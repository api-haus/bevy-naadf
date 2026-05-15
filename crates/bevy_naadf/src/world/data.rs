//! The `WorldData` + `VoxelTypes` main-world resources — the three-layer CPU
//! buffer mirrors, world geometry, and the voxel-type palette
//! (`03-design.md` §4.4).
//!
//! These are the CPU side of the world. `voxel::grid::setup_test_grid` (D2)
//! builds them once at startup; Batch 2's `render::extract` / `render::prepare`
//! mirror them into the render world (`WorldGpu`) on the `dirty` flag.

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
