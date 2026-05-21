//! DIAGNOSTIC-ONLY edit oracles — extracted from `WorldData`'s public API
//! per the `/delegate` codebase-tightening D1 architect (Finding 1).
//!
//! Both functions run the whole-world AADF rehash
//! (`crate::aadf::edit::recompute_chunk_layer_aadfs`) + emit synthetic chunk
//! uploads. O(N_chunks × 31 × 3) per call. **Production code paths NEVER
//! call this module** — see [`crate::world::data::WorldData::set_voxels_batch`]
//! / [`crate::world::data::WorldData::set_chunks_uniform_batch`] for the
//! runtime fast paths.
//!
//! Call sites:
//! - `--edit-mode` / `--runtime-edit-mode` e2e gates (`render/construction/
//!   validation.rs`).
//! - Unit tests in `world/data.rs::tests` + `aadf/edit.rs::tests`.
//! - D5's e2e gate fixtures (`render/construction/mod.rs`).
//!
//! ## Why free functions on `&mut WorldData`
//!
//! Per `03-architecture.md` D1.4 — the free-function shape matches the
//! existing sibling helpers in `crate::aadf::edit` (e.g. `process_edit_batch`,
//! `merge_recomputed_aadfs_into_batch`) that mutate `WorldData` without being
//! methods on it. It keeps `WorldData`'s impl block focused on the 5
//! production methods (`Default::default`, `seed_block_hashing`,
//! `ray_traversal`, `get_voxel_type`, `set_voxels_batch`,
//! `set_chunks_uniform_batch`) and surfaces the diagnostic seam structurally
//! as a sibling module.

use bevy::math::IVec3;

use crate::voxel::{VoxelTypeId, CELL_DIM};
use crate::world::data::{VoxelEdit, WorldData};

/// DIAGNOSTIC-ONLY single-voxel edit (`02f` rearch). Runs the whole-world
/// `recompute_chunk_layer_aadfs` + emits synthetic AADF-changed chunk uploads
/// — O(N_chunks × 31 × 3) per call.
///
/// **Production brushes call [`WorldData::set_voxels_batch`] /
/// [`WorldData::set_chunks_uniform_batch`] instead.**
///
/// Phase-C W2 — programmatic single-voxel edit entry point
/// (`15-design-c.md` §2.1 W2, `16-impl-c-W2.md`).
pub(crate) fn set_voxel(world: &mut WorldData, pos: IVec3, ty: VoxelTypeId) {
    if pos.x < 0 || pos.y < 0 || pos.z < 0 {
        return;
    }
    let p = [pos.x as u32, pos.y as u32, pos.z as u32];
    let sx = world.size_in_chunks.x * CELL_DIM as u32 * CELL_DIM as u32;
    let sy = world.size_in_chunks.y * CELL_DIM as u32 * CELL_DIM as u32;
    let sz = world.size_in_chunks.z * CELL_DIM as u32 * CELL_DIM as u32;
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
        + chunk[1] * world.size_in_chunks.x
        + chunk[2] * world.size_in_chunks.x * world.size_in_chunks.y)
        as usize;
    if chunk_idx >= world.chunks_cpu.len() {
        return;
    }
    // Decode the existing chunk's voxels into an edit window, set the
    // voxel, re-encode through `process_edit_batch`.
    let mut window = crate::aadf::edit::build_chunk_edit_window_from_world(
        &world.chunks_cpu,
        &world.blocks_cpu,
        &world.voxels_cpu,
        chunk_idx,
    );
    crate::aadf::edit::set_voxel_in_window(&mut window, voxel_in_chunk, ty.raw());
    // Run the edit batch with cursors starting at the end of the existing
    // buffers (we never reuse existing slots — the simplified port appends
    // fresh).
    let v_cursor = world.voxels_cpu.len() as u32;
    let b_cursor = world.blocks_cpu.len() as u32;
    let (mut batch, _new_v, _new_b) = crate::aadf::edit::process_edit_batch(
        &window,
        &[(chunk, 0)],
        v_cursor,
        b_cursor,
    );
    // Apply to CPU buffers using the typed iter helpers (Step-4 helpers).
    for (_ptr, voxels) in batch.iter_voxel_edits() {
        world.voxels_cpu.extend_from_slice(voxels);
    }
    for (idx, (_ptr, blocks)) in batch.iter_block_edits().enumerate() {
        // The pointer we wrote into `blocks_cpu` is `b_cursor + idx * 64`.
        let block_ptr = b_cursor + (idx as u32) * 64;
        crate::aadf::edit::apply_block_edit_cpu(&mut world.blocks_cpu, block_ptr, blocks);
    }
    // Update the chunks CPU buffer entry for this chunk.
    for entry in &batch.changed_chunks {
        let [cx, cy, cz] = crate::aadf::edit::unpack_chunk_pos(entry[0]);
        let new_state = entry[1];
        let ci = (cx
            + cy * world.size_in_chunks.x
            + cz * world.size_in_chunks.x * world.size_in_chunks.y) as usize;
        if ci < world.chunks_cpu.len() {
            world.chunks_cpu[ci] = new_state;
        }
    }
    // Bug 4 fix — DRY-collapsed via `merge_recomputed_aadfs_into_batch`.
    let size_arr = [
        world.size_in_chunks.x,
        world.size_in_chunks.y,
        world.size_in_chunks.z,
    ];
    crate::aadf::edit::merge_recomputed_aadfs_into_batch(
        &mut world.chunks_cpu,
        size_arr,
        &mut batch,
    );

    // Stash the edit batch on the resource so the extract pass picks it up.
    world.pending_edits.batches.push(batch);
    world.pending_edits.edited_groups.push([
        chunk[0] / CELL_DIM as u32,
        chunk[1] / CELL_DIM as u32,
        chunk[2] / CELL_DIM as u32,
    ]);
}

/// DIAGNOSTIC-ONLY bulk-edit oracle (`02f` rearch). Slow-but-bit-exact path
/// — runs `recompute_chunk_layer_aadfs` over the whole world + emits
/// synthetic `changed_chunks` entries for every AADF-changed chunk. O(N_chunks
/// × 31 × 3) per call.
///
/// **Production brushes call [`WorldData::set_voxels_batch`] instead.**
pub(crate) fn set_voxels_batch_oracle(world: &mut WorldData, edits: &[VoxelEdit]) {
    if edits.is_empty() {
        return;
    }
    let chunk_size_voxels = (CELL_DIM * CELL_DIM) as u32; // 16
    let sx_v = world.size_in_chunks.x * chunk_size_voxels;
    let sy_v = world.size_in_chunks.y * chunk_size_voxels;
    let sz_v = world.size_in_chunks.z * chunk_size_voxels;
    if sx_v == 0 || sy_v == 0 || sz_v == 0 {
        return;
    }

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

    let chunk_count = by_chunk.len();
    let mut edit_data: Vec<u32> = vec![0; chunk_count * 2048];
    let mut edited_chunks: Vec<([u32; 3], u32)> = Vec::with_capacity(chunk_count);

    for (i, (chunk_pos, per_chunk_edits)) in by_chunk.into_iter().enumerate() {
        let chunk_idx = (chunk_pos[0]
            + chunk_pos[1] * world.size_in_chunks.x
            + chunk_pos[2] * world.size_in_chunks.x * world.size_in_chunks.y)
            as usize;
        if chunk_idx >= world.chunks_cpu.len() {
            continue;
        }
        let edit_offset = (i * 2048) as u32;
        edited_chunks.push((chunk_pos, edit_offset));
        let window_slice = &mut edit_data[i * 2048..(i + 1) * 2048];
        let decoded = crate::aadf::edit::build_chunk_edit_window_from_world(
            &world.chunks_cpu,
            &world.blocks_cpu,
            &world.voxels_cpu,
            chunk_idx,
        );
        window_slice.copy_from_slice(&decoded);
        for (voxel_in_chunk, ty) in per_chunk_edits {
            crate::aadf::edit::set_voxel_in_window(window_slice, voxel_in_chunk, ty);
        }
    }
    if edited_chunks.is_empty() {
        return;
    }

    let v_cursor = world.voxels_cpu.len() as u32;
    let b_cursor = world.blocks_cpu.len() as u32;
    let (mut batch, _new_v, _new_b) = crate::aadf::edit::process_edit_batch(
        &edit_data,
        &edited_chunks,
        v_cursor,
        b_cursor,
    );

    // Step-4 helpers replace the hand-rolled `chunks_exact(33|65)` loops.
    for (_ptr, voxels) in batch.iter_voxel_edits() {
        world.voxels_cpu.extend_from_slice(voxels);
    }
    for (idx, (_ptr, blocks)) in batch.iter_block_edits().enumerate() {
        let block_ptr = b_cursor + (idx as u32) * 64;
        let target_len = (block_ptr + 64) as usize;
        if world.blocks_cpu.len() < target_len {
            world.blocks_cpu.resize(target_len, 0);
        }
        crate::aadf::edit::apply_block_edit_cpu(&mut world.blocks_cpu, block_ptr, blocks);
    }
    for entry in &batch.changed_chunks {
        let [cx, cy, cz] = crate::aadf::edit::unpack_chunk_pos(entry[0]);
        let new_state = entry[1];
        let ci = (cx
            + cy * world.size_in_chunks.x
            + cz * world.size_in_chunks.x * world.size_in_chunks.y) as usize;
        if ci < world.chunks_cpu.len() {
            world.chunks_cpu[ci] = new_state;
        }
    }

    // Whole-world AADF recompute + synthetic chunk uploads — collapsed via
    // `merge_recomputed_aadfs_into_batch` (F3 DRY helper).
    let size_arr = [
        world.size_in_chunks.x,
        world.size_in_chunks.y,
        world.size_in_chunks.z,
    ];
    crate::aadf::edit::merge_recomputed_aadfs_into_batch(
        &mut world.chunks_cpu,
        size_arr,
        &mut batch,
    );

    for &(chunk_pos, _) in &edited_chunks {
        world.pending_edits.edited_groups.push([
            chunk_pos[0] / CELL_DIM as u32,
            chunk_pos[1] / CELL_DIM as u32,
            chunk_pos[2] / CELL_DIM as u32,
        ]);
    }
    world.pending_edits.batches.push(batch);
}
