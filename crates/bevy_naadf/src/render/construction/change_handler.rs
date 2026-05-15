//! Phase-C W2 — CPU flood-fill port of `ChangeHandler.UpdateWorld`
//! (`15-design-c.md` §1.2 regime-3, §1.6 oracle table; `16-impl-c-W2.md`).
//!
//! Ports `ChangeHandler.cs:69-255` line-for-line: the **two distinct loops**
//! that drive the on-edit AADF invalidation:
//!
//! 1. **BFS-expand** (`ChangeHandler.cs:73-110`) — from each freshly edited
//!    4³-chunk group, walk the 27-cell neighbourhood; for any neighbour that
//!    hasn't been touched yet (`distanceFloodFill[idx] == 0x3FFFFFFF`), assign
//!    `newDistance = curDistance + 4` and enqueue it if `curDistance < 28`.
//!    Output: every group within ~7 hops of any edit is in `changedGroups`,
//!    keyed by its flat index.
//! 2. **`addBounds` propagation** (`ChangeHandler.cs:124-174`) — 7 rounds × 3
//!    axes per round = 21 sweeps. Each sweep updates the per-axis 5-bit AADF
//!    of every changed group's `distanceFloodFill` entry by examining its
//!    neighbour on the relevant axis side via `checkMatchingBoundCell`. The
//!    end-effect packs 6 directional bounds × 5 bits into the low 30 bits of
//!    each `distanceFloodFill[group]` u32.
//!
//! Output format (consumed verbatim by `world_change.wgsl::apply_group_change`):
//!
//! - `changedGroupsWithDist[i]` — `[u32; 2]` of `(group_pos_packed, distance)`.
//!   `group_pos_packed = x | y<<11 | z<<21`. The `distance` u32's high 2 bits
//!   hold the "reset-completely" flag (`0xC0000000` for groups edited directly
//!   in this batch — `i < originalChangedGroupCount`); the low 30 bits hold
//!   the 6 × 5-bit directional bounds.
//!
//! ## Simplifications relative to NAADF
//!
//! - The C# allocates `distanceFloodFill[]` lazily and reuses it across
//!   frames; this port allocates `Vec<u32>` per-batch + does the same work
//!   each call. The performance delta is irrelevant at the test-grid scale
//!   (a 16³ chunk world has at most 4³ = 64 groups).

use crate::aadf::edit::{pack_chunk_pos, unpack_chunk_pos};

/// Distance sentinel — "this group hasn't been touched by the flood fill yet".
/// Matches `ChangeHandler.cs:48` `distanceFloodFill[i] = 0x3FFFFFFF`.
pub const DIST_UNTOUCHED: u32 = 0x3FFF_FFFF;

/// Distance marker — "this group was edited directly in this batch (not just
/// touched by the flood fill)". Matches `ChangeHandler.cs:271`
/// `distanceFloodFill[groupIndex] = 0x80000000`.
pub const DIST_RESET_COMPLETELY: u32 = 0x8000_0000;

/// Direction masks identical to `ChangeHandler.cs:118-123`.
const MASK_MX: u32 = 0x3D; // 0b111101
const MASK_PX: u32 = 0x3E; // 0b111110
const MASK_MY: u32 = 0x37; // 0b110111
const MASK_PY: u32 = 0x3B; // 0b111011
const MASK_MZ: u32 = 0x1F; // 0b011111
const MASK_PZ: u32 = 0x2F; // 0b101111

/// Compute group index from `(x, y, z)` group position, given the world's
/// size in groups along each axis.
#[inline]
fn group_index(pos: [u32; 3], size: [u32; 3]) -> usize {
    (pos[0] + pos[1] * size[0] + pos[2] * size[0] * size[1]) as usize
}

/// `ChangeHandler.checkMatchingBoundCell` (`ChangeHandler.cs:290-301`).
///
/// Per-direction mask: bit `i` set means `neighbour`'s 5-bit AADF in direction
/// `i` is `>=` `curVoxel`'s — i.e. the neighbour's "empty distance" along that
/// axis is at least as great as ours, so a propagation step from us toward
/// the neighbour is legal.
fn check_matching_bound_cell(neighbour: u32, cur_voxel: u32) -> u32 {
    let mut mask: u32 = 0;
    for i in 0..6u32 {
        let shift = i * 5;
        let n = (neighbour >> shift) & 0x1F;
        let c = (cur_voxel >> shift) & 0x1F;
        if n >= c {
            mask |= 1 << i;
        }
    }
    mask
}

/// `ChangeHandler.addBounds` (`ChangeHandler.cs:280-288`).
///
/// If the neighbour at `(cur_idx + direction_offset)` is not flagged
/// "reset-completely" (`0x80000000`) AND its bound mask covers the input
/// direction `mask`, bump `cur_voxel` by `4 << bounds_location` (one
/// flood-fill step of 4 in the named axis side).
fn add_bounds(
    distance: &[u32],
    cur_idx: usize,
    mask: u32,
    direction_offset: i32,
    bounds_location: u32,
    cur_voxel: &mut u32,
) {
    let neighbour_idx = (cur_idx as i32 + direction_offset) as usize;
    let neighbour = distance[neighbour_idx];
    if (neighbour & DIST_RESET_COMPLETELY) == 0
        && (check_matching_bound_cell(neighbour, *cur_voxel) & mask) == mask
    {
        *cur_voxel += 4 << bounds_location;
    }
}

/// The output of [`compute_change_groups`] — the `changedGroupsWithDist[]`
/// array ready for upload into `changed_groups_dynamic`.
///
/// Each entry is `(group_pos_packed, distance)`. Two distinct shapes:
/// - `distance` high 2 bits set (`0xC0000000`) → this group was edited
///   directly in this batch; `apply_group_change` reads
///   `is_reset_completely = (change.y >> 30) != 0` from this flag.
/// - `distance` high 2 bits clear → this group was touched by the flood fill;
///   low 30 bits hold the 6 × 5-bit packed AADFs.
#[derive(Debug, Clone, Default)]
pub struct ChangedGroups {
    pub entries: Vec<[u32; 2]>,
}

/// CPU port of `ChangeHandler.UpdateWorld`'s **two distinct loops** —
/// `ChangeHandler.cs:69-184`. Given a list of directly-edited group positions
/// (one per chunk that was touched), produces the full `changedGroupsWithDist`
/// upload array.
///
/// `size_in_groups` — the world size in 4³-chunk groups along each axis
/// (`sizeInChunks / 4`).
/// `directly_edited_groups` — the freshly-edited group positions; corresponds
/// to `ChangeHandler.AddChangedChunk` calls (one per chunk, deduplicated to the
/// group). These get the `0xC0000000` reset-completely flag.
pub fn compute_change_groups(
    size_in_groups: [u32; 3],
    directly_edited_groups: &[[u32; 3]],
) -> ChangedGroups {
    let total = (size_in_groups[0] * size_in_groups[1] * size_in_groups[2]) as usize;
    let mut distance = vec![DIST_UNTOUCHED; total];

    // `ChangeHandler.cs:269-275` — initial AddChangedChunk: assign
    // `0x80000000` to the directly edited groups and enqueue them.
    let mut queue: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    let mut changed_groups: Vec<u32> = Vec::new(); // flat indices

    for &group_pos in directly_edited_groups {
        // Bounds check — silently skip out-of-bounds edits.
        if group_pos[0] >= size_in_groups[0]
            || group_pos[1] >= size_in_groups[1]
            || group_pos[2] >= size_in_groups[2]
        {
            continue;
        }
        let idx = group_index(group_pos, size_in_groups);
        if distance[idx] == DIST_UNTOUCHED {
            distance[idx] = DIST_RESET_COMPLETELY;
            changed_groups.push(idx as u32);
            queue.push_back(pack_chunk_pos(group_pos));
        }
    }

    let original_changed_count = changed_groups.len();

    // === Loop 1: BFS-expand over 27-cell neighbourhood (`ChangeHandler.cs:73-110`) ===

    while let Some(cur_group_packed) = queue.pop_front() {
        let cur = unpack_chunk_pos(cur_group_packed);
        let cur_idx = group_index(cur, size_in_groups);
        let cur_distance = distance[cur_idx] & 0x7FFF_FFFF;

        for dz in -1i32..=1 {
            let nz = cur[2] as i32 + dz;
            if nz < 0 || nz >= size_in_groups[2] as i32 {
                continue;
            }
            for dy in -1i32..=1 {
                let ny = cur[1] as i32 + dy;
                if ny < 0 || ny >= size_in_groups[1] as i32 {
                    continue;
                }
                for dx in -1i32..=1 {
                    let nx = cur[0] as i32 + dx;
                    if nx < 0
                        || nx >= size_in_groups[0] as i32
                        || (dx == 0 && dy == 0 && dz == 0)
                    {
                        continue;
                    }
                    let next_pos = [nx as u32, ny as u32, nz as u32];
                    let next_idx = group_index(next_pos, size_in_groups);
                    if distance[next_idx] == DIST_UNTOUCHED {
                        let new_dist = cur_distance + 4;
                        distance[next_idx] = new_dist;
                        if cur_distance < 28 {
                            queue.push_back(pack_chunk_pos(next_pos));
                        }
                        changed_groups.push(next_idx as u32);
                    }
                }
            }
        }
    }

    // `ChangeHandler.cs:112-116` — after the BFS, reset every flood-fill
    // group's distance to 0 (so the 7-round `addBounds` propagation builds
    // up the per-axis 5-bit AADFs from scratch). Note: `changed_groups[0..originalChangedGroupCount]`
    // keep `0x80000000` (the reset-completely marker — they're handled
    // specially in the output below).
    for i in original_changed_count..changed_groups.len() {
        let g = changed_groups[i] as usize;
        distance[g] = 0;
    }

    // === Loop 2: 7-round `addBounds` propagation (`ChangeHandler.cs:124-174`) ===
    //
    // Each iteration runs 3 sub-passes (one per axis). For every group in
    // `changed_groups[originalChangedGroupCount..]` (the BFS-touched groups,
    // not the directly-edited ones), examine its ±axis neighbours and possibly
    // bump the per-axis bound.

    let stride_y = size_in_groups[0] as i32;
    let stride_z = (size_in_groups[0] * size_in_groups[1]) as i32;

    for _iter in 0..7 {
        // X axis.
        for v in original_changed_count..changed_groups.len() {
            let group_idx = changed_groups[v] as usize;
            let x = (group_idx as u32) % size_in_groups[0];
            let mut cur_group = distance[group_idx];
            if x > 0 {
                add_bounds(&distance, group_idx, MASK_MX, -1, 0, &mut cur_group);
            } else {
                cur_group += 4u32 << 0;
            }
            if x + 1 < size_in_groups[0] {
                add_bounds(&distance, group_idx, MASK_PX, 1, 5, &mut cur_group);
            } else {
                cur_group += 4u32 << 5;
            }
            distance[group_idx] = cur_group;
        }
        // Y axis.
        for v in original_changed_count..changed_groups.len() {
            let group_idx = changed_groups[v] as usize;
            let y = ((group_idx as u32) / size_in_groups[0]) % size_in_groups[1];
            let mut cur_group = distance[group_idx];
            if y > 0 {
                add_bounds(&distance, group_idx, MASK_MY, -stride_y, 10, &mut cur_group);
            } else {
                cur_group += 4u32 << 10;
            }
            if y + 1 < size_in_groups[1] {
                add_bounds(&distance, group_idx, MASK_PY, stride_y, 15, &mut cur_group);
            } else {
                cur_group += 4u32 << 15;
            }
            distance[group_idx] = cur_group;
        }
        // Z axis.
        for v in original_changed_count..changed_groups.len() {
            let group_idx = changed_groups[v] as usize;
            let z = (group_idx as u32) / (size_in_groups[0] * size_in_groups[1]);
            let mut cur_group = distance[group_idx];
            if z > 0 {
                add_bounds(&distance, group_idx, MASK_MZ, -stride_z, 20, &mut cur_group);
            } else {
                cur_group += 4u32 << 20;
            }
            if z + 1 < size_in_groups[2] {
                add_bounds(&distance, group_idx, MASK_PZ, stride_z, 25, &mut cur_group);
            } else {
                cur_group += 4u32 << 25;
            }
            distance[group_idx] = cur_group;
        }
    }

    // === Pack the output array (`ChangeHandler.cs:175-183`) ===

    let mut entries: Vec<[u32; 2]> = Vec::with_capacity(changed_groups.len());
    for (i, &g) in changed_groups.iter().enumerate() {
        let g = g as usize;
        let group_pos = [
            (g as u32) % size_in_groups[0],
            ((g as u32) / size_in_groups[0]) % size_in_groups[1],
            (g as u32) / (size_in_groups[0] * size_in_groups[1]),
        ];
        // Directly-edited groups get `0xC0000000` (reset-completely flag);
        // flood-fill groups get their packed-AADF distance value.
        let dist_value = if i < original_changed_count {
            0xC000_0000
        } else {
            distance[g]
        };
        entries.push([pack_chunk_pos(group_pos), dist_value]);
    }

    ChangedGroups { entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single directly-edited group with no neighbours touches one group
    /// only (the reset-completely entry); on a 1×1×1 world the BFS finds no
    /// neighbours.
    #[test]
    fn flood_fill_single_isolated_edit() {
        let groups = compute_change_groups([1, 1, 1], &[[0, 0, 0]]);
        assert_eq!(groups.entries.len(), 1);
        assert_eq!(groups.entries[0][0], 0); // packed pos = 0
        assert_eq!(groups.entries[0][1], 0xC000_0000); // reset-completely
    }

    /// A directly-edited group's 26 neighbours all show up in the BFS pass.
    /// On a 3×3×3-group world with an edit at the centre (1,1,1), the BFS
    /// finds all 26 neighbours.
    #[test]
    fn flood_fill_centre_edit_finds_26_neighbours() {
        let groups = compute_change_groups([3, 3, 3], &[[1, 1, 1]]);
        // 1 directly-edited + 26 flood-fill touched = 27.
        assert_eq!(groups.entries.len(), 27);
        // The first entry is the directly-edited group.
        let centre = pack_chunk_pos([1, 1, 1]);
        assert_eq!(groups.entries[0][0], centre);
        assert_eq!(groups.entries[0][1], 0xC000_0000);
        // The other 26 entries have non-reset-completely distance values.
        for e in &groups.entries[1..] {
            assert_eq!(e[1] & 0xC000_0000, 0, "flood-fill entry should NOT have reset-completely flag");
        }
    }

    /// **The load-bearing W2 distance-propagation test.** On a 9×1×1-group
    /// world with an edit at group (0,0,0), traces the BFS reach + the cap
    /// behaviour exactly per `ChangeHandler.cs:81-110`.
    ///
    /// Per `ChangeHandler.cs:98-106`, when a neighbour is touched for the
    /// first time:
    ///   - `distanceFloodFill[neighbour] = curDistance + 4` (always)
    ///   - `changedGroups[++count] = neighbour` (always)
    ///   - enqueue the neighbour `if curDistance < 28` (gated)
    ///
    /// So (7,0,0)'s distance is 28 and it IS enqueued (because (6,0,0)'s
    /// distance 24 < 28). When (7,0,0) is dequeued the enqueue-check is
    /// `28 < 28` = false, so (8,0,0) is NOT enqueued — BUT (8,0,0)'s distance
    /// IS set to 32 and it IS added to `changedGroups`. The BFS reaches
    /// (1..=8), so the total is 1 directly-edited + 8 BFS-touched = 9.
    #[test]
    fn flood_fill_distance_propagation_linear() {
        let groups = compute_change_groups([9, 1, 1], &[[0, 0, 0]]);
        let by_pos: std::collections::HashMap<[u32; 3], u32> = groups
            .entries
            .iter()
            .map(|&[pos, dist]| (unpack_chunk_pos(pos), dist))
            .collect();
        // (0,0,0) directly-edited; (1..=8) BFS-touched = 9 total.
        assert_eq!(by_pos.len(), 9, "BFS reach: 1 directly-edited + 8 touched");
        // (0,0,0): reset-completely flag.
        assert_eq!(by_pos[&[0, 0, 0]], 0xC000_0000);
        // (1..=8): touched (non-reset-completely).
        for x in 1..=8 {
            assert!(by_pos.contains_key(&[x, 0, 0]), "BFS should reach ({x},0,0)");
            let dist = by_pos[&[x, 0, 0]];
            assert_eq!(
                dist & 0xC000_0000,
                0,
                "({x},0,0) should NOT have reset-completely flag"
            );
        }
        // For a world of 9×1×1, only x indices 0..=8 exist; 8 is the last
        // valid index. The BFS does not go further (no (9,0,0) to reach).
    }

    /// Multiple directly-edited groups: each gets the reset-completely flag,
    /// their BFS expansions union (no double-counting).
    #[test]
    fn flood_fill_multiple_edits_no_double_count() {
        // 4×1×1 world, edits at (0,0,0) and (3,0,0). The BFS from each only
        // reaches direct neighbours (within distance 4); they don't overlap.
        let groups = compute_change_groups([4, 1, 1], &[[0, 0, 0], [3, 0, 0]]);
        let by_pos: std::collections::HashMap<[u32; 3], u32> = groups
            .entries
            .iter()
            .map(|&[pos, dist]| (unpack_chunk_pos(pos), dist))
            .collect();
        // (0,0,0) + (3,0,0) directly-edited; (1,0,0) + (2,0,0) BFS-touched
        // (each at distance 4 from one of the edits).
        assert_eq!(by_pos.len(), 4);
        assert_eq!(by_pos[&[0, 0, 0]], 0xC000_0000);
        assert_eq!(by_pos[&[3, 0, 0]], 0xC000_0000);
        // (1,0,0) and (2,0,0) — BFS-touched (non-reset-completely).
        assert!(by_pos.contains_key(&[1, 0, 0]));
        assert!(by_pos.contains_key(&[2, 0, 0]));
        assert_eq!(by_pos[&[1, 0, 0]] & 0xC000_0000, 0);
        assert_eq!(by_pos[&[2, 0, 0]] & 0xC000_0000, 0);
    }
}
