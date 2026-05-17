//! Phase-C W2 — CPU oracles for `world_change.wgsl`'s 4 apply passes +
//! port of `EditingHandler.processChunks` (`15-design-c.md` §1.6, §2.1 W2).
//!
//! Three GPU-shader oracles:
//!   - [`apply_chunk_edit_cpu`] — mirrors `world_change.wgsl::apply_chunk_change`:
//!     write `chunks[chunkPos].x = new_state`, preserve `.y`.
//!   - [`apply_block_edit_cpu`] — mirrors `apply_block_change`: write 64
//!     blocks at `pointer..pointer+64`, recompute the 4³ AADF via
//!     `compute_aadf_layer` (W6 — same algorithm the GPU `compute_bounds_4`
//!     runs).
//!   - [`apply_voxel_edit_cpu`] — mirrors `apply_voxel_change`: unpack 32
//!     packed-voxel-pair u32s into 64 voxel half-words, recompute the 4³
//!     AADF, repack and write 32 u32s.
//!
//! One editing-handler port:
//!   - [`process_edit_batch`] — `EditingHandler.processChunks` (`EditingHandler.cs:75-249`):
//!     per-edited-chunk re-hash + free old voxel slots + fill the
//!     `changed_blocks` / `changed_voxels` arrays in the NAADF formats.
//!
//! All oracles operate on **the chunks buffer's packed `[u32; 2]` chunk
//! texel** so the comparison surface against a GPU readback of the `Rg32Uint`
//! chunks texture is a direct byte-equality on `bytemuck::cast_slice`d data.
//!
//! ## Buffer-format reference (`worldChange.fx`)
//!
//! - `changedChunks[]` — `Uint2[]` of `(chunk_pos_packed, new_state)`. Layout:
//!   `chunk_pos.x | y<<11 | z<<21`; `new_state` is the new chunk `.x` value
//!   (low 30 bits = AADF for empty / pointer for mixed; high 2 bits = state).
//! - `changedBlocks[]` — `u32[]` flat array. Per edit: 65 u32s — `[pointer,
//!   64 × block_word]`. `pointer` is the offset into `blocks[]` where the 64
//!   new blocks go.
//! - `changedVoxels[]` — `u32[]` flat array. Per edit: 33 u32s — `[pointer,
//!   32 × packed_voxel_pair]`. `pointer` is the offset into `voxels[]` where
//!   the 32 packed-voxel-pair u32s go.
//! - `changedGroupsWithDist[]` — `Uint2[]` of `(group_pos_packed, distance)`.
//!   Layout: `group_pos.x | y<<11 | z<<21`; `distance` carries the flood-fill
//!   6-direction-packed 5-bit AADFs (low 30 bits) + the reset-completely flag
//!   in bit 30.
//!
//! See `ChangeHandler.cs:53-59` for the source-of-truth layout the C# uses
//! when writing these buffers.

use crate::aadf::bounds::compute_aadf_layer;
use crate::aadf::cell::{pack_voxels, unpack_voxel, BlockCell, VoxelCell};
use crate::voxel::{
    AADF_MAX_SMALL, CELL_CHILDREN, CELL_DIM, VOXEL_FULL_FLAG, VOXEL_PAYLOAD_MASK,
};

// ─── GPU-shader oracles ────────────────────────────────────────────────────────

/// CPU oracle for `world_change.wgsl::apply_chunk_change` (`worldChange.fx:115-128`).
///
/// Applies the CPU-staged chunk-cell edit `(chunk_pos_packed, new_state)` to
/// the packed `[u32; 2]` chunks texel buffer at the chunk's flat index. **`.y`
/// (entity pointer channel from W4) is preserved**; only `.x` is overwritten
/// with the new state.
///
/// `chunks_packed` is the GPU `Rg32Uint` chunks texture readback flattened as
/// `[u32; 2]` per chunk in `(cx, cy, cz)` scan order. The packed-position layout
/// matches `worldChange.fx:122` exactly.
pub fn apply_chunk_edit_cpu(
    chunks_packed: &mut [[u32; 2]],
    size_in_chunks: [u32; 3],
    chunk_pos_packed: u32,
    new_state: u32,
) {
    let cx = (chunk_pos_packed & 0x7FF) as usize;
    let cy = ((chunk_pos_packed >> 11) & 0x3FF) as usize;
    let cz = (chunk_pos_packed >> 21) as usize;
    let sx = size_in_chunks[0] as usize;
    let sy = size_in_chunks[1] as usize;
    let idx = cx + cy * sx + cz * sx * sy;
    // Preserve `.y` (entity pointer channel) — load-bearing W2 contract.
    let existing_y = chunks_packed[idx][1];
    chunks_packed[idx] = [new_state, existing_y];
}

/// CPU oracle for `world_change.wgsl::apply_block_change` (`worldChange.fx:130-147`).
///
/// Applies the CPU-staged 64-block edit to `blocks[]`. The 64 raw block words
/// in `new_blocks_raw` (one per intra-chunk block in `x + y*4 + z*16` child-cell
/// order — same as `compute_aadf_layer`'s iteration order on a 4³ layer) get
/// per-empty-block AADFs computed across the local 4³ extent via
/// `compute_aadf_layer`; non-empty blocks pass through unchanged. The 64
/// AADF-augmented words go into `blocks[pointer..pointer+64]`.
///
/// `new_blocks_raw[i]` is the raw block word — encoded `BlockCell` `.encode()`
/// or a pre-computed `state | payload`. The state at bits 30-31 is preserved;
/// for empty blocks (state == 0), the AADF in bits 0-11 is *re-computed* by
/// `compute_aadf_layer`. For non-empty blocks the raw word passes through.
pub fn apply_block_edit_cpu(
    blocks: &mut Vec<u32>,
    pointer: u32,
    new_blocks_raw: &[u32; CELL_CHILDREN],
) {
    // Pre-extract the "is empty" mask for the 4³ block layer; the AADF
    // calculation only writes empty cells.
    let is_empty_at = |c: [i32; 3]| -> bool {
        let i = (c[0] + c[1] * CELL_DIM as i32 + c[2] * CELL_DIM as i32 * CELL_DIM as i32) as usize;
        // Block state at bits 30-31; empty == 0 (`BLOCK_STATE_UNIFORM_EMPTY`).
        new_blocks_raw[i] >> 30 == 0
    };
    let aadfs = compute_aadf_layer(
        [CELL_DIM, CELL_DIM, CELL_DIM],
        AADF_MAX_SMALL,
        is_empty_at,
    );

    let base = pointer as usize;
    let end = base + CELL_CHILDREN;
    if blocks.len() < end {
        blocks.resize(end, 0);
    }
    for i in 0..CELL_CHILDREN {
        let raw = new_blocks_raw[i];
        if raw >> 30 != 0 {
            // Non-empty: pass through.
            blocks[base + i] = raw;
        } else {
            // Empty: encode the AADF into the BlockCell word.
            let cell = BlockCell::Empty(aadfs[i]);
            blocks[base + i] = cell.encode();
        }
    }
}

/// CPU oracle for `world_change.wgsl::apply_voxel_change` (`worldChange.fx:149-168`).
///
/// Applies the CPU-staged 64-voxel edit to `voxels[]`. `new_voxels_raw` is 64
/// raw voxel half-words in `x + y*4 + z*16` child-cell order. The voxel layer's
/// 4³ AADF is computed via `compute_aadf_layer` (max distance 3); non-empty
/// voxels (bit 15 set) pass through. The 64 AADF-augmented half-words are
/// packed two-per-u32 (low half = even index, high half = odd) into
/// `voxels[pointer..pointer+32]`.
pub fn apply_voxel_edit_cpu(
    voxels: &mut Vec<u32>,
    pointer: u32,
    new_voxels_raw: &[u16; CELL_CHILDREN],
) {
    let is_empty_at = |c: [i32; 3]| -> bool {
        let i = (c[0] + c[1] * CELL_DIM as i32 + c[2] * CELL_DIM as i32 * CELL_DIM as i32) as usize;
        // Voxel state at bit 15; empty == 0.
        new_voxels_raw[i] & VOXEL_FULL_FLAG == 0
    };
    let aadfs = compute_aadf_layer(
        [CELL_DIM, CELL_DIM, CELL_DIM],
        AADF_MAX_SMALL,
        is_empty_at,
    );

    let base = pointer as usize;
    let packed_count = CELL_CHILDREN / 2; // 32
    let end = base + packed_count;
    if voxels.len() < end {
        voxels.resize(end, 0);
    }
    // Compute the 64 final voxel half-words (AADFs filled in for empty cells).
    let mut final_voxels = [0u16; CELL_CHILDREN];
    for i in 0..CELL_CHILDREN {
        let raw = new_voxels_raw[i];
        if raw & VOXEL_FULL_FLAG != 0 {
            // Full voxel — pass through, masked to the 15-bit payload + flag.
            final_voxels[i] = (raw & (VOXEL_FULL_FLAG | VOXEL_PAYLOAD_MASK))
                | VOXEL_FULL_FLAG; // ensure flag stays set (raw may have only the flag)
            // Actually raw already has the flag; mask cleanly:
            final_voxels[i] = raw & (VOXEL_FULL_FLAG | VOXEL_PAYLOAD_MASK);
        } else {
            // Empty — encode AADF.
            let cell = VoxelCell::Empty(aadfs[i]);
            final_voxels[i] = cell.encode();
        }
    }
    // Pack two per u32 (`pack_voxels` = low | high << 16).
    for pair in 0..packed_count {
        voxels[base + pair] = pack_voxels(final_voxels[pair * 2], final_voxels[pair * 2 + 1]);
    }
}

// ─── Edit-batch staging ─────────────────────────────────────────────────────────

/// A queued edit batch produced by [`process_edit_batch`] — the per-edit
/// payload the GPU `world_change.wgsl` shaders consume verbatim (modulo the
/// `pointer` rebasing left to the caller's buffer-cursor accounting).
///
/// Mirrors the `ChangeHandler.cs:23-25` field set: one flat `changed_chunks`,
/// one flat `changed_blocks` (in NAADF's 65-u32-per-edit layout), one flat
/// `changed_voxels` (33-u32-per-edit layout). Each is uploaded verbatim into
/// the GPU's `changed*_dynamic` buffers.
#[derive(Debug, Default, Clone)]
pub struct EditBatch {
    /// `changedChunks[]` — `Uint2[]` of `(chunk_pos_packed, new_state)`. One
    /// entry per edited chunk.
    pub changed_chunks: Vec<[u32; 2]>,
    /// `changedBlocks[]` — flat `u32[]` of `(pointer, 64 × block_word)` per
    /// edit. `changed_block_count = changed_blocks.len() / 65`.
    pub changed_blocks: Vec<u32>,
    /// `changedVoxels[]` — flat `u32[]` of `(pointer, 32 × packed_voxel_pair)`
    /// per edit. `changed_voxel_count = changed_voxels.len() / 33`.
    pub changed_voxels: Vec<u32>,
}

/// Packed chunk position layout: `pos.x | y<<11 | z<<21`.
pub fn pack_chunk_pos(p: [u32; 3]) -> u32 {
    p[0] | (p[1] << 11) | (p[2] << 21)
}

/// Unpack a chunk position from the packed `u32` layout.
pub fn unpack_chunk_pos(packed: u32) -> [u32; 3] {
    [
        packed & 0x7FF,
        (packed >> 11) & 0x3FF,
        packed >> 21,
    ]
}

/// Port of `EditingHandler.processChunks` (`EditingHandler.cs:75-249`).
///
/// **Runs on the runtime path** (`02f` rearch — this is NOT the diagnostic
/// "CPU rebuild" that the rearch's headline retires). C# runs the equivalent
/// per-edit-frame via `EditingHandler.processChunks`; same cost shape (per-
/// touched-chunk encode, not whole-world). The diagnostic-only artefact
/// `02f` retires is `recompute_chunk_layer_aadfs` (the WHOLE-WORLD
/// `O(N_chunks × 31 × 3)` AADF rehash), not this function. See
/// `02f-design-world-container-rearch.md` R5 for the interpretation note.
///
/// For each chunk in `edited_chunks`, hashes its 64 new blocks (the
/// `edit_data[64*32 = 2048]` window per chunk — 64 blocks × 32 packed-voxel-pair
/// u32s) and produces the `changed_chunks` / `changed_blocks` / `changed_voxels`
/// arrays in the NAADF on-wire formats. **Simplified port:** this Rust version
/// does NOT hash-dedup voxel-block groups (the C# `BlockHashingHandler.AddBlock`
/// path) — it appends fresh voxel slots for every mixed block. The simplification
/// is acceptable for W2 because (a) the dedup is a *cursor* optimisation on
/// `voxels[]` storage, not a correctness requirement, and (b) the GPU
/// `apply_voxel_change` writes voxels at the pointer the CPU supplies — so as
/// long as the CPU pointer + GPU pointer agree, the byte layout is correct.
///
/// `voxel_cursor` is the running `block_voxel_count[0]` value (in `u32` units,
/// i.e. each voxel-pointer claim consumes 32 u32s = 64 voxels). The caller
/// updates it on success.
/// `block_cursor` is the running `block_voxel_count[1]` value — each
/// mixed-block claim consumes 64 u32s.
///
/// `edit_data` is the per-chunk edit window: 2048 u32s per edited chunk,
/// packed in `chunkCalc.fx::calcBlockFromRawData`'s expected layout (block-major
/// within chunk, 2 voxels per u32, intra-block index = `vx + vy*4 + vz*16`).
/// `edited_chunks` is the flat list of `(chunk_pos, edit_data_offset)` pairs.
///
/// Returns the `EditBatch` ready for upload + the new (voxel_cursor, block_cursor)
/// pair the caller should write back into `block_voxel_count`.
pub fn process_edit_batch(
    edit_data: &[u32],
    edited_chunks: &[([u32; 3], u32)], // (chunk_pos, edit_data_offset)
    voxel_cursor: u32,
    block_cursor: u32,
) -> (EditBatch, u32, u32) {
    let mut batch = EditBatch::default();
    let mut v_cursor = voxel_cursor;
    let mut b_cursor = block_cursor;

    for &(chunk_pos, edit_offset) in edited_chunks {
        let edit_base = edit_offset as usize;
        // 64 blocks per chunk, each 32 u32s = 64 voxels.
        let mut new_blocks = [0u32; CELL_CHILDREN];
        let mut all_blocks_same = true;
        let mut reference_block: u32 = 0;

        for b in 0..CELL_CHILDREN {
            let block_base = edit_base + b * 32;
            // Determine if this block is uniform — all 64 voxel half-words
            // equal.
            let mut is_uniform_full = true;
            let first_voxel_pair = edit_data[block_base];
            let lo0 = first_voxel_pair & 0xFFFF;
            let hi0 = first_voxel_pair >> 16;
            if lo0 != hi0 {
                is_uniform_full = false;
            }
            if is_uniform_full {
                for i in 0..32 {
                    let pair = edit_data[block_base + i];
                    if (pair & 0xFFFF) != lo0 || (pair >> 16) != lo0 {
                        is_uniform_full = false;
                        break;
                    }
                }
            }
            let first_type = (lo0 & VOXEL_PAYLOAD_MASK as u32) as u16;
            if is_uniform_full {
                // BlockCell::Empty or BlockCell::UniformFull, encoded inline.
                let state = if first_type == 0 { 0u32 } else { 1u32 };
                new_blocks[b] = (first_type as u32) | (state << 30);
            } else {
                // Mixed — append the 32 packed-voxel-pair u32s to
                // `changed_voxels`. **Diagnostic-only path** (only
                // `WorldData::set_voxel` / unit tests use this `process_edit_batch`
                // function now). The production runtime path
                // [`WorldData::set_voxels_batch`] is the hashing-dedup'd
                // port of C# `EditingHandler.processChunks` and handles
                // the AADF-zero-input requirement itself.
                let voxel_ptr = v_cursor;
                v_cursor += 32;
                batch.changed_voxels.push(voxel_ptr);
                for i in 0..32 {
                    batch.changed_voxels.push(edit_data[block_base + i]);
                }
                new_blocks[b] = voxel_ptr | (2u32 << 30); // BLOCK_STATE_CHILD
            }
            if b == 0 {
                reference_block = new_blocks[0];
            }
            if new_blocks[b] != reference_block {
                all_blocks_same = false;
            }
        }

        // Per `EditingHandler.cs:146-160`:
        let new_chunk_value: u32;
        if all_blocks_same {
            new_chunk_value = reference_block;
        } else {
            // Mixed chunk — claim 64 block slots, write `changedBlocks` payload.
            let block_ptr = b_cursor;
            b_cursor += 64;
            batch.changed_blocks.push(block_ptr);
            for b in 0..CELL_CHILDREN {
                batch.changed_blocks.push(new_blocks[b]);
            }
            new_chunk_value = block_ptr | (2u32 << 30); // ChunkCell::Mixed
        }
        // Chunk update — `(pos_packed, new_state)`.
        batch.changed_chunks.push([
            pack_chunk_pos(chunk_pos),
            new_chunk_value,
        ]);
    }

    (batch, v_cursor, b_cursor)
}

/// Helper: build an `edit_data` window (2048 u32s) from a `WorldData`-style
/// CPU mirror, for a single edited chunk. Mirrors
/// `WorldData.FillChunkData` (the C# helper `EditingHandler.getChunkDataToEdit`
/// calls — pulls the chunk's 2048 voxel words back into a flat window so the
/// edit pass can mutate them).
///
/// `voxels_cpu` is the CPU voxel buffer (packed two voxels per u32). The
/// chunk's 64 blocks are at offset `chunk_index * 2048` in the buffer; each
/// block's 32 u32s are at consecutive `32-u32` strides within the chunk.
///
/// Returns a fresh `Vec<u32>` of length 2048.
///
/// **Test-helper only.** Production editing reconstructs the chunk's voxel
/// window by decoding `chunks_cpu[chunk_idx]` (mixed → walk blocks_cpu →
/// walk voxels_cpu) which `WorldData::set_voxel` does in-place.
#[allow(dead_code)]
pub fn build_chunk_edit_window_solid_type(ty: u16) -> Vec<u32> {
    let payload = ((ty & VOXEL_PAYLOAD_MASK) as u32) | (VOXEL_FULL_FLAG as u32);
    let packed = payload | (payload << 16);
    vec![packed; 2048]
}

/// Helper: build an `edit_data` window of length 2048 u32s that mirrors the
/// chunk currently encoded in `chunks_cpu[chunk_idx]` / `blocks_cpu` /
/// `voxels_cpu` — the inverse of the construction process for a single chunk.
///
/// Decodes a `ChunkCell` from its packed `u32`:
/// - `Empty` → all 2048 u32s are 0 (empty voxel = `VoxelCell::Empty(zero AADF)`).
/// - `UniformFull(ty)` → every voxel half-word = full payload of `ty`.
/// - `Mixed(block_ptr)` → walk the 64 blocks; for each block,
///     - `Empty` / uniform → flat fill the 32 u32s with the matching voxel
///       value (empty = 0, uniform full = packed `ty | flag`).
///     - `Mixed(voxel_ptr)` → copy the 32 u32s from `voxels_cpu[voxel_ptr..voxel_ptr+32]`.
pub fn build_chunk_edit_window_from_world(
    chunks_cpu: &[u32],
    blocks_cpu: &[u32],
    voxels_cpu: &[u32],
    chunk_idx: usize,
) -> Vec<u32> {
    let mut out = vec![0u32; 2048];
    let chunk_raw = chunks_cpu[chunk_idx];
    let chunk_state = chunk_raw >> 30;

    if chunk_state == 0 {
        // Empty chunk — all 2048 u32s are 0.
        return out;
    }
    if chunk_state == 1 {
        // Uniform full chunk — every voxel = the chunk's voxel-type payload.
        let ty = (chunk_raw & 0x7FFF) as u16;
        let payload = ((ty & VOXEL_PAYLOAD_MASK) as u32) | (VOXEL_FULL_FLAG as u32);
        let packed = payload | (payload << 16);
        for v in out.iter_mut() {
            *v = packed;
        }
        return out;
    }
    // Mixed chunk — walk 64 blocks.
    let block_base = (chunk_raw & 0x3FFF_FFFF) as usize;
    for b in 0..CELL_CHILDREN {
        let block_raw = blocks_cpu[block_base + b];
        let block_state = block_raw >> 30;
        let block_window_base = b * 32;

        if block_state == 0 {
            // Empty block — 32 u32s of 0. (out is already zeroed.)
            continue;
        }
        if block_state == 1 {
            // Uniform full block.
            let ty = (block_raw & 0x7FFF) as u16;
            let payload = ((ty & VOXEL_PAYLOAD_MASK) as u32) | (VOXEL_FULL_FLAG as u32);
            let packed = payload | (payload << 16);
            for i in 0..32 {
                out[block_window_base + i] = packed;
            }
            continue;
        }
        // Mixed block — copy 32 packed u32s from `voxels_cpu` at the voxel ptr.
        let voxel_base = (block_raw & 0x3FFF_FFFF) as usize;
        for i in 0..32 {
            out[block_window_base + i] = voxels_cpu[voxel_base + i];
        }
    }

    out
}

/// Helper: set a single voxel inside an existing edit window (2048 u32s).
///
/// `voxel_in_chunk` is the voxel coord inside the chunk `(0..16, 0..16, 0..16)`.
/// `ty` is the new voxel type id (0 = empty, non-zero = full).
pub fn set_voxel_in_window(window: &mut [u32], voxel_in_chunk: [u32; 3], ty: u16) {
    let block_in_chunk = [
        voxel_in_chunk[0] / CELL_DIM as u32,
        voxel_in_chunk[1] / CELL_DIM as u32,
        voxel_in_chunk[2] / CELL_DIM as u32,
    ];
    let voxel_in_block = [
        voxel_in_chunk[0] % CELL_DIM as u32,
        voxel_in_chunk[1] % CELL_DIM as u32,
        voxel_in_chunk[2] % CELL_DIM as u32,
    ];
    let block_index =
        (block_in_chunk[0] + block_in_chunk[1] * 4 + block_in_chunk[2] * 16) as usize;
    let voxel_index =
        (voxel_in_block[0] + voxel_in_block[1] * 4 + voxel_in_block[2] * 16) as usize;
    let u32_offset = block_index * 32 + voxel_index / 2;
    let is_high = voxel_index & 1 == 1;
    let new_word = if ty == 0 {
        0u16
    } else {
        VOXEL_FULL_FLAG | (ty & VOXEL_PAYLOAD_MASK)
    };
    let cur = window[u32_offset];
    let lo = if is_high { cur & 0xFFFF } else { new_word as u32 };
    let hi = if is_high { new_word as u32 } else { cur >> 16 };
    window[u32_offset] = lo | (hi << 16);
}

/// Recompute chunk-layer AADFs for the entire world, updating empty chunks'
/// low 30 bits in `chunks_cpu` to reflect the post-edit world state.
///
/// **Bug 4 fix (followup-editor-bugs-234):** the W2 GPU regime-3 chain only
/// updates AADFs for chunks within the BFS reach (~32 chunks of any direct
/// edit). For large `.vox`-loaded worlds where empty chunks far from the
/// loaded geometry have construction-time AADFs saturated at `AADF_MAX_CHUNK
/// = 31`, edits inside that distance hull leave the far-side AADFs correct,
/// but the AADF cap means **chunks within 30 chunks of any edit can have
/// stale large AADFs that overshoot the new geometry**. The renderer's DDA
/// reads those stale AADFs and skips OVER painted voxels — visible as
/// "painted shapes terminate at some level / depend on view angle".
///
/// This recompute walks **the whole chunks layer** via `compute_aadf_layer`
/// (same algorithm regime-2 implements GPU-side, paper §3.3 form) and
/// rewrites every empty chunk's AADF to the correct distance to the nearest
/// non-empty chunk. The CPU mirror becomes authoritative for chunk-layer
/// AADFs.
///
/// `chunks_cpu`: the CPU mirror buffer (one `u32` per chunk in `(cx, cy,
/// cz)` scan order). State bits 30-31 are preserved; AADF bits (low 30 of
/// empty chunks) are overwritten.
///
/// `size_in_chunks`: the world dimensions in chunks.
///
/// Returns the list of flat chunk indices whose encoded value changed —
/// the caller emits these into the `EditBatch.changed_chunks` upload list
/// so the GPU chunks texture stays in sync via `apply_chunk_change`.
///
/// **Complexity**: O(world chunks × 3 axes × AADF_MAX_CHUNK iterations) =
/// O(3 · 31 · N) per call. For Oasis_Hard_Cover.vox (93×34×84 = 265 k
/// chunks), ~25 M ops per edit ≈ ~5 ms CPU (release build). **DO NOT call
/// from production code paths** — see DIAGNOSTIC-ONLY note below.
///
/// ## DIAGNOSTIC-ONLY (`02f` rearch)
///
/// This whole-world AADF rehash is **DIAGNOSTIC-ONLY**. Call sites:
///
/// - `WorldData::set_voxel` (`set_voxel` is itself DIAGNOSTIC-ONLY — see
///   `world/data.rs`).
/// - `WorldData::set_voxels_batch_oracle` (same).
/// - Unit tests in this file.
///
/// **Production brushes call [`WorldData::set_voxels_batch`] or
/// [`WorldData::set_chunks_uniform_batch`]**, which skip this rehash. The
/// W3 GPU regime-2 self-perpetuating queue refreshes stale AADFs over
/// subsequent frames (matches C# `WorldBoundHandler.cs:91-121` semantics —
/// far-away AADFs converge over many frames, NOT synchronously per edit).
#[doc(hidden)]
pub fn recompute_chunk_layer_aadfs(
    chunks_cpu: &mut [u32],
    size_in_chunks: [u32; 3],
) -> Vec<usize> {
    let sx = size_in_chunks[0] as usize;
    let sy = size_in_chunks[1] as usize;
    let sz = size_in_chunks[2] as usize;
    if sx == 0 || sy == 0 || sz == 0 {
        return Vec::new();
    }
    if chunks_cpu.len() != sx * sy * sz {
        // Defensive: dimension mismatch — refuse to recompute (avoid OOB).
        return Vec::new();
    }
    // Snapshot the empty-mask before mutation so the closure reads a stable
    // pre-recompute classification (Mixed/UniformFull/Empty boundary).
    let snapshot: Vec<u32> = chunks_cpu.to_vec();
    let snapshot_ref = &snapshot;
    let is_empty = |c: [i32; 3]| -> bool {
        if c[0] < 0 || c[1] < 0 || c[2] < 0 {
            return false;
        }
        let x = c[0] as usize;
        let y = c[1] as usize;
        let z = c[2] as usize;
        if x >= sx || y >= sy || z >= sz {
            return false;
        }
        let idx = x + y * sx + z * sx * sy;
        // State bits 30-31 == 0 means ChunkCell::Empty (the only state that
        // carries an AADF in the chunk layer).
        (snapshot_ref[idx] >> 30) == 0
    };
    let aadfs = compute_aadf_layer(
        [sx, sy, sz],
        crate::voxel::AADF_MAX_CHUNK,
        is_empty,
    );

    let mut changed: Vec<usize> = Vec::new();
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let idx = x + y * sx + z * sx * sy;
                // Only empty chunks carry chunk-layer AADFs in their low
                // 30 bits — Mixed/UniformFull use those bits for ptr/type.
                if (snapshot_ref[idx] >> 30) != 0 {
                    continue;
                }
                let new_val =
                    crate::aadf::cell::ChunkCell::Empty(aadfs[idx]).encode();
                if chunks_cpu[idx] != new_val {
                    chunks_cpu[idx] = new_val;
                    changed.push(idx);
                }
            }
        }
    }
    changed
}

// `unpack_voxel` re-export so test code can pull it through this module.
pub use crate::aadf::cell::unpack_voxel as cell_unpack_voxel;
// Suppress unused-import warning under cfg(test) — re-export hookup.
#[allow(unused_imports)]
use unpack_voxel as _unpack_voxel;

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aadf::cell::Aadf6;

    /// `apply_chunk_edit_cpu` preserves the `.y` channel of the chunks texel.
    /// This is the **load-bearing W2 contract** — W4 widened chunks to
    /// `Rg32Uint` with the entity-pointer in `.y`; a single-channel write
    /// would silently zero out entity pointers at every edited chunk.
    #[test]
    fn apply_chunk_edit_preserves_y_channel() {
        let mut chunks = vec![[0u32; 2]; 2 * 2 * 2];
        // Pre-populate `.y` with a sentinel.
        chunks[0] = [0xDEAD_BEEF, 0xCAFEBABE];
        chunks[3] = [0x11111111, 0x22222222];
        // Apply an edit at chunk (0,0,0) — packed pos 0.
        apply_chunk_edit_cpu(&mut chunks, [2, 2, 2], 0, 0x99999999);
        assert_eq!(
            chunks[0], [0x99999999, 0xCAFEBABE],
            "edited chunk's .x updated, .y preserved"
        );
        // Other chunks untouched.
        assert_eq!(chunks[3], [0x11111111, 0x22222222]);
        assert_eq!(chunks[1], [0, 0]);
    }

    /// `apply_chunk_edit_cpu` writes to the right chunk index (decodes the
    /// packed position correctly).
    #[test]
    fn apply_chunk_edit_packed_pos_decodes() {
        let mut chunks = vec![[0u32; 2]; 4 * 4 * 4];
        let pos = [2u32, 1, 3];
        let packed = pack_chunk_pos(pos);
        apply_chunk_edit_cpu(&mut chunks, [4, 4, 4], packed, 0xAAAAAAAA);
        let idx = (pos[0] + pos[1] * 4 + pos[2] * 16) as usize;
        assert_eq!(chunks[idx], [0xAAAAAAAA, 0]);
        // Untouched neighbours.
        assert_eq!(chunks[idx - 1], [0, 0]);
        assert_eq!(chunks[idx + 1], [0, 0]);
    }

    /// `apply_block_edit_cpu` writes 64 blocks AND recomputes AADFs on the
    /// empty ones (the GPU's `compute_bounds_4` is mirrored CPU-side via
    /// W6's `compute_aadf_layer`).
    #[test]
    fn apply_block_edit_writes_64_blocks_and_aadfs() {
        let mut blocks: Vec<u32> = vec![0; 128];
        let pointer = 64u32;
        // Build new_blocks_raw — one full block at (0,0,0), all others empty.
        let mut raw = [0u32; CELL_CHILDREN];
        // Block at local index 0 — uniform full of type 7.
        raw[0] = 7 | (1 << 30); // BLOCK_STATE_UNIFORM_FULL | type 7
        apply_block_edit_cpu(&mut blocks, pointer, &raw);
        // Block 0 passes through.
        assert_eq!(blocks[64], 7 | (1 << 30));
        // Block 1 (local pos (1,0,0)) is empty and is the immediate neighbour
        // of the full block on +X — AADF[-x] should be 0 (touching).
        let block1 = BlockCell::decode(blocks[65]);
        match block1 {
            BlockCell::Empty(aadf) => {
                // `-x` direction: distance to the full block at offset -1.
                // Since AADF is "empty distance" (cell-count along the axis),
                // touching the wall is distance 0.
                assert_eq!(aadf.d[0], 0, "block1 AADF[-x] should be 0 (touching)");
                // `+x` direction: 2 cells to the boundary (block layer is
                // 4-wide; block1 at x=1, far boundary at x=4 → distance 2).
                assert_eq!(aadf.d[1], 2);
            }
            _ => panic!("block 1 should be Empty, got {block1:?}"),
        }
    }

    /// `apply_voxel_edit_cpu` writes 32 packed-voxel-pair u32s into `voxels[]`
    /// and computes the 4³ voxel AADFs.
    #[test]
    fn apply_voxel_edit_writes_32_packed_and_aadfs() {
        let mut voxels: Vec<u32> = vec![0; 64];
        let pointer = 32u32;
        // 64 voxel half-words — one full at (0,0,0), all others empty.
        let mut raw = [0u16; CELL_CHILDREN];
        raw[0] = VOXEL_FULL_FLAG | 3; // full voxel, type 3
        apply_voxel_edit_cpu(&mut voxels, pointer, &raw);
        // First u32 — voxel 0 (full) in low half, voxel 1 (empty + AADF) in
        // high half.
        let pair = voxels[32];
        let v0 = (pair & 0xFFFF) as u16;
        let v1 = (pair >> 16) as u16;
        assert_eq!(v0, VOXEL_FULL_FLAG | 3, "voxel 0 = full type 3");
        // v1 is the voxel at local position (1,0,0) — neighbour of full on -X,
        // so AADF[-x] = 0.
        assert!(v1 & VOXEL_FULL_FLAG == 0, "voxel 1 is empty");
        let v1_cell = VoxelCell::decode(v1);
        match v1_cell {
            VoxelCell::Empty(aadf) => {
                assert_eq!(aadf.d[0], 0, "voxel 1 AADF[-x] = 0 (touching full)");
            }
            _ => panic!("voxel 1 should be Empty"),
        }
    }

    /// `process_edit_batch` produces correctly formatted `changed_chunks` /
    /// `changed_blocks` / `changed_voxels` arrays.
    #[test]
    fn process_edit_batch_basic() {
        // Two edited chunks: chunk (0,0,0) is all-empty, chunk (1,0,0) has a
        // single full voxel at (0,0,0).
        let mut edit_data = vec![0u32; 4096]; // 2 × 2048 u32s
        // Chunk 0 — leave all zeros (all-empty).
        // Chunk 1 — edit data starts at offset 2048.
        // Set voxel (0,0,0) of block (0,0,0) of chunk 1 to type 5.
        let ty = 5u16;
        let full = (VOXEL_FULL_FLAG | ty) as u32;
        edit_data[2048] = full; // low half — voxel 0 of block 0
        // Other voxels of block 0 stay 0 (empty).

        let edited = vec![
            ([0u32, 0, 0], 0u32),
            ([1, 0, 0], 2048),
        ];
        let (batch, v_cursor, b_cursor) = process_edit_batch(&edit_data, &edited, 64, 64);
        // 2 chunks edited.
        assert_eq!(batch.changed_chunks.len(), 2);
        // Chunk 0 is empty → chunk value is BLOCK_STATE_UNIFORM_EMPTY = 0.
        assert_eq!(batch.changed_chunks[0][0], 0); // pos 0
        assert_eq!(batch.changed_chunks[0][1], 0); // state = empty
        // Chunk 1 is mixed (has one mixed block, rest empty). It must claim a
        // block slot.
        let chunk1_state = batch.changed_chunks[1][1];
        assert_eq!(chunk1_state >> 30, 2, "chunk 1 should be Mixed");
        // One mixed block (block 0 of chunk 1, the rest are uniform empty).
        // `changed_blocks` should hold 1 edit = 65 u32s.
        assert_eq!(batch.changed_blocks.len(), 65);
        // `changed_voxels` should hold 1 edit = 33 u32s (the one mixed block's
        // voxel data).
        assert_eq!(batch.changed_voxels.len(), 33);
        // The voxel pointer points into a fresh slot starting at the input
        // voxel_cursor (64) → first 32 voxels go at offset 64.
        assert_eq!(batch.changed_voxels[0], 64);
        // The first packed voxel-pair: voxel 0 (full type 5) in low half.
        assert_eq!(batch.changed_voxels[1] & 0xFFFF, full);
        // The block pointer points into a fresh slot at b_cursor input (64).
        assert_eq!(batch.changed_blocks[0], 64);
        // Cursors advanced.
        assert_eq!(v_cursor, 64 + 32);
        assert_eq!(b_cursor, 64 + 64);

        // Avoid unused-warning on `unpack_voxel`.
        let _ = unpack_voxel;
        // Avoid unused on `Aadf6`.
        let _ = Aadf6::ZERO;
    }

    /// `build_chunk_edit_window_from_world` round-trips an all-empty chunk to
    /// 2048 zero u32s.
    #[test]
    fn build_window_empty_chunk_is_zeros() {
        let chunks_cpu = vec![0u32]; // ChunkCell::Empty(Aadf6::ZERO).encode() == 0
        let window = build_chunk_edit_window_from_world(&chunks_cpu, &[], &[], 0);
        assert_eq!(window.len(), 2048);
        assert!(window.iter().all(|&v| v == 0));
    }

    /// `set_voxel_in_window` round-trip: write a voxel, then verify it shows
    /// up at the correct flat offset.
    #[test]
    fn set_voxel_in_window_round_trip() {
        let mut window = vec![0u32; 2048];
        // Set voxel (1, 2, 3) inside the chunk to type 0xABC.
        set_voxel_in_window(&mut window, [1, 2, 3], 0xABC);
        // Decode: voxel (1,2,3) is in block (0,0,0), intra-block (1,2,3) =
        // 1 + 2*4 + 3*16 = 57. Voxel half-word at flat u32-offset 57/2 = 28,
        // high half (57 & 1 == 1).
        let pair = window[28];
        let high = (pair >> 16) as u16;
        assert_eq!(high & VOXEL_PAYLOAD_MASK, 0xABC);
        assert_eq!(high & VOXEL_FULL_FLAG, VOXEL_FULL_FLAG);
        // Low half untouched.
        assert_eq!(pair & 0xFFFF, 0);
    }

    /// Bug 4 fix — `recompute_chunk_layer_aadfs` shrinks stale AADFs to
    /// reflect a newly-inserted non-empty chunk.
    ///
    /// Set up an 8×1×1 chunks_cpu where every chunk is empty with an
    /// uninitialised AADF (saturated at 31 — the "construction-time"
    /// shape for a freshly loaded `.vox` where the empty cells around
    /// the geometry have AADF=31). Mark chunk 4 as Mixed (the simulated
    /// post-edit state) and recompute. Verify chunks 0..=3 now carry +X
    /// AADFs reflecting the distance to chunk 4 (3, 2, 1, 0), and chunks
    /// 5..=7 carry the same for -X.
    #[test]
    fn recompute_chunk_layer_aadfs_shrinks_stale_post_edit() {
        // 8 chunks in a row, all initially Empty(AADF=31 in every direction)
        // — encode `ChunkCell::Empty` with saturated AADFs.
        let saturated_aadf = crate::aadf::cell::Aadf6 { d: [31; 6] };
        let init_word = crate::aadf::cell::ChunkCell::Empty(saturated_aadf).encode();
        let mut chunks: Vec<u32> = vec![init_word; 8];
        // Mark chunk 4 as Mixed (state bits 30-31 = 0b10 = 2 → high bit
        // 31 set), payload doesn't matter for empty-classification.
        chunks[4] = (2u32 << 30) | 0x123;
        // Recompute.
        let changed =
            super::recompute_chunk_layer_aadfs(&mut chunks, [8, 1, 1]);
        // Every empty chunk's encoding changed (the +X / -X AADFs shrunk).
        // Chunk 4 (Mixed) untouched in `changed`.
        assert!(!changed.contains(&4), "Mixed chunk should not appear in changed list");
        // Chunk 0: +X distance to chunk 4 = 3 cells (chunks 1,2,3 empty
        // between 0 and the non-empty 4). -X AADF = 31 (world edge,
        // saturated). Decode and inspect.
        let chunk0 = crate::aadf::cell::ChunkCell::decode(chunks[0]);
        match chunk0 {
            crate::aadf::cell::ChunkCell::Empty(a) => {
                assert_eq!(a.d[1], 3, "chunk 0 +X AADF should be 3 (3 empty chunks before chunk 4)");
            }
            _ => panic!("chunk 0 should be Empty after recompute"),
        }
        // Chunk 3 — immediately -X of chunk 4. +X AADF = 0 (touching).
        let chunk3 = crate::aadf::cell::ChunkCell::decode(chunks[3]);
        match chunk3 {
            crate::aadf::cell::ChunkCell::Empty(a) => {
                assert_eq!(a.d[1], 0, "chunk 3 +X AADF should be 0 (touching chunk 4)");
            }
            _ => panic!("chunk 3 should be Empty after recompute"),
        }
        // Chunk 5 — immediately +X of chunk 4. -X AADF = 0 (touching).
        let chunk5 = crate::aadf::cell::ChunkCell::decode(chunks[5]);
        match chunk5 {
            crate::aadf::cell::ChunkCell::Empty(a) => {
                assert_eq!(a.d[0], 0, "chunk 5 -X AADF should be 0 (touching chunk 4)");
            }
            _ => panic!("chunk 5 should be Empty after recompute"),
        }
    }

    /// `recompute_chunk_layer_aadfs` is a no-op when the chunks layer is
    /// fully empty (every empty cell already has the max-distance AADF
    /// matching the world extent).
    #[test]
    fn recompute_chunk_layer_aadfs_idempotent_on_converged_world() {
        // 4×1×1 world, every chunk empty. The correct AADFs from
        // `compute_aadf_layer` produce ChunkCell::Empty encodings; running
        // recompute twice should produce no further changes.
        let mut chunks: Vec<u32> = vec![0u32; 4];
        let _first = super::recompute_chunk_layer_aadfs(&mut chunks, [4, 1, 1]);
        let second = super::recompute_chunk_layer_aadfs(&mut chunks, [4, 1, 1]);
        assert!(
            second.is_empty(),
            "second recompute call must produce no changes on a converged world"
        );
    }
}
