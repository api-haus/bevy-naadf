// Phase-C W1 — `boundsCommon.fxh` ported to WGSL
// (`15-design-c.md` §4.1, §4.4; W1 ships the canonical WGSL form).
//
// Faithful port of NAADF's `Content/shaders/world/data/boundsCommon.fxh` (67
// lines). Exposes the `groupshared` 64-cell `cached_cell` array + the
// `ComputeBounds4` 3-iteration synchronised AADF loop used by:
//
// - W1 `chunk_calc.wgsl::compute_voxel_bounds` (2-bit voxel AADFs).
// - W1 `chunk_calc.wgsl::compute_block_bounds`  (2-bit block AADFs).
// - W2 `world_change.wgsl::apply_block_change` (block AADFs after edit).
// - W2 `world_change.wgsl::apply_voxel_change` (voxel AADFs after edit).
//
// Used by importing modules via `#include` (naga-oil "naga_oil::imports").
// Bevy 0.19's WGSL composition treats top-level decls as imports when the file
// is `#import`-ed; W1 inlines the contents because the imports surface in
// Bevy's pipeline cache is unpredictable across naga versions.
//
// MonoGame → wgpu deviations:
// - HLSL `GroupMemoryBarrierWithGroupSync()` → WGSL `workgroupBarrier()`.
// - HLSL `groupshared uint cachedCell[64]` → WGSL `var<workgroup> cached_cell:
//   array<u32, 64>` (the importing shader declares the storage; the helpers
//   read/write it directly).
// - HLSL `inout uint curVoxel` parameter passing → WGSL doesn't allow inout
//   user-defined fn params over `ptr<workgroup>` cleanly across naga
//   versions, so the helpers return the updated `cur_cell` value; the caller
//   re-assigns the local.

// `MASK_MX`..`MASK_PZ` from `boundsCommon.fxh:6-11`. Each mask excludes the
// back-pointer bit of the grow direction (the bit pointing back toward us):
//   grow -x → exclude +x = bit 1 → mask 0b111101 = 0x3D
//   grow +x → exclude -x = bit 0 → mask 0b111110 = 0x3E
//   grow -y → exclude +y = bit 3 → mask 0b110111 = 0x37
//   grow +y → exclude -y = bit 2 → mask 0b111011 = 0x3B
//   grow -z → exclude +z = bit 5 → mask 0b011111 = 0x1F
//   grow +z → exclude -z = bit 4 → mask 0b101111 = 0x2F
const MASK_MX: u32 = 0x3Du;
const MASK_PX: u32 = 0x3Eu;
const MASK_MY: u32 = 0x37u;
const MASK_PY: u32 = 0x3Bu;
const MASK_MZ: u32 = 0x1Fu;
const MASK_PZ: u32 = 0x2Fu;

// `groupshared uint cachedCell[64]` (`boundsCommon.fxh:13`). 64 = 4³ —
// matches every `numthreads(64,1,1)` caller's workgroup size.
var<workgroup> cached_cell: array<u32, 64>;

// `checkMatchingBounds(neighbour, curVoxel, shiftOffset, shiftCount, shiftMask)`
// — `boundsCommon.fxh:15-26`. Returns a 6-bit mask where bit `i` is set iff
// `neighbour`'s bound in direction `i` is `>=` `cur_voxel`'s bound in that
// direction. The 6 directions are extracted from the AADF bit-field stored at
// `shift_offset + shift_count * i`, masked by `shift_mask` (3 for 2-bit small
// AADFs, 0x1F for 5-bit chunk AADFs).
fn check_matching_bounds(
    neighbour: u32,
    cur_voxel: u32,
    shift_offset: u32,
    shift_count: u32,
    shift_mask: u32,
) -> u32 {
    var mask: u32 = 0u;
    let n0 = (neighbour >> (shift_offset + shift_count * 0u)) & shift_mask;
    let c0 = (cur_voxel  >> (shift_offset + shift_count * 0u)) & shift_mask;
    if (n0 >= c0) { mask = mask | (1u << 0u); }   // -x
    let n1 = (neighbour >> (shift_offset + shift_count * 1u)) & shift_mask;
    let c1 = (cur_voxel  >> (shift_offset + shift_count * 1u)) & shift_mask;
    if (n1 >= c1) { mask = mask | (1u << 1u); }   // +x
    let n2 = (neighbour >> (shift_offset + shift_count * 2u)) & shift_mask;
    let c2 = (cur_voxel  >> (shift_offset + shift_count * 2u)) & shift_mask;
    if (n2 >= c2) { mask = mask | (1u << 2u); }   // -y
    let n3 = (neighbour >> (shift_offset + shift_count * 3u)) & shift_mask;
    let c3 = (cur_voxel  >> (shift_offset + shift_count * 3u)) & shift_mask;
    if (n3 >= c3) { mask = mask | (1u << 3u); }   // +y
    let n4 = (neighbour >> (shift_offset + shift_count * 4u)) & shift_mask;
    let c4 = (cur_voxel  >> (shift_offset + shift_count * 4u)) & shift_mask;
    if (n4 >= c4) { mask = mask | (1u << 4u); }   // -z
    let n5 = (neighbour >> (shift_offset + shift_count * 5u)) & shift_mask;
    let c5 = (cur_voxel  >> (shift_offset + shift_count * 5u)) & shift_mask;
    if (n5 >= c5) { mask = mask | (1u << 5u); }   // +z
    return mask;
}

// `addBoundsVoxelsOrBlocks(local_index, mask, direction_offset, bounds_location,
//                          state_location, state_mask, cur_cell)`
// — `boundsCommon.fxh:28-36`. Reads the neighbour cell from `cached_cell` at
// `local_index + direction_offset`; if the neighbour is empty (its state bits
// are zero) and its 5 perpendicular-or-direction-aligned bounds dominate the
// current cell's bounds (per `check_matching_bounds(_, _, 0, 2, 0x3) == mask`),
// increment the current cell's bound at `bounds_location` by 1.
//
// Returns the updated `cur_cell` (WGSL does not allow `inout` user params over
// workgroup storage portably; the C# `inout` is materialised as a return).
fn add_bounds_voxels_or_blocks(
    local_index: u32,
    mask: u32,
    direction_offset: i32,
    bounds_location: u32,
    state_location: u32,
    state_mask: u32,
    cur_cell: u32,
) -> u32 {
    let neighbour_idx = u32(i32(local_index) + direction_offset);
    let neighbour = cached_cell[neighbour_idx];
    var out = cur_cell;
    if (((neighbour >> state_location) & state_mask) == 0u) {
        if ((check_matching_bounds(neighbour, cur_cell, 0u, 2u, 0x3u) & mask) == mask) {
            out = cur_cell + (1u << bounds_location);
        }
    }
    return out;
}

// `ComputeBounds4(local_index, pos_in_volume, state_location, state_mask,
//                 cur_cell)` — `boundsCommon.fxh:38-64`. The 3-iteration
// alternating-axis synchronised loop.
//
// Each outer iteration steps once in each axis (X then Y then Z). Per axis:
//   - try -direction grow (if pos > 0)
//   - try +direction grow (if pos + 1 < 4)
//   - write `cur_cell` back to `cached_cell` + `workgroupBarrier()`
//
// The `cur_cell` value is threaded through: each helper call may bump one
// bit-field; the writes to `cached_cell` after each axis step propagate the
// updates to neighbours' reads in the next axis step (within the same outer
// iteration). After 3 outer iterations the small-AADF cap of 3 is reached.
//
// Returns the final `cur_cell` value the caller writes back. The caller is
// responsible for setting `cached_cell[local_index] = cur_cell` BEFORE calling
// (and for the surrounding `workgroupBarrier()` per the C# `:40` initial
// barrier).
fn compute_bounds_4(
    local_index: u32,
    pos_in_volume: vec3<i32>,
    state_location: u32,
    state_mask: u32,
    cur_cell_in: u32,
) -> u32 {
    var cur_cell = cur_cell_in;
    workgroupBarrier(); // boundsCommon.fxh:40 — initial sync.

    for (var i: i32 = 0; i < 3; i = i + 1) {
        // Axis X.
        if (pos_in_volume.x > 0) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_MX, -1, 0u,
                state_location, state_mask, cur_cell,
            );
        }
        if (pos_in_volume.x + 1 < 4) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_PX, 1, 2u,
                state_location, state_mask, cur_cell,
            );
        }
        cached_cell[local_index] = cur_cell;
        workgroupBarrier();

        // Axis Y.
        if (pos_in_volume.y > 0) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_MY, -4, 4u,
                state_location, state_mask, cur_cell,
            );
        }
        if (pos_in_volume.y + 1 < 4) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_PY, 4, 6u,
                state_location, state_mask, cur_cell,
            );
        }
        cached_cell[local_index] = cur_cell;
        workgroupBarrier();

        // Axis Z.
        if (pos_in_volume.z > 0) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_MZ, -16, 8u,
                state_location, state_mask, cur_cell,
            );
        }
        if (pos_in_volume.z + 1 < 4) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_PZ, 16, 10u,
                state_location, state_mask, cur_cell,
            );
        }
        cached_cell[local_index] = cur_cell;
        workgroupBarrier();
    }

    return cur_cell;
}
