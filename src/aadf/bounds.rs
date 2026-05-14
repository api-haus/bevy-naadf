//! CPU-side AADF cuboid expansion — the 6-direction empty-distance fields.
//!
//! A faithful CPU re-derivation of paper §3.3 (`02-research.md` §1.1.4,
//! `03-design.md` §6.1 step 3) — *not* a transliteration of `boundsCommon.fxh`.
//!
//! The AADF of an empty cell is a cuboid bounding box, empty of geometry,
//! extending around the cell by some number of cells in each of the 6
//! axis-aligned directions. Construction (paper §3.3):
//!
//! 1. Start with a cuboid equal to the cell itself (all 6 distances = 0).
//! 2. Iterate, **alternating between the three dimensions**, expanding
//!    concurrently in *both* the positive and negative direction of that
//!    dimension by one cell per iteration.
//! 3. The expansion in a dimension is bounded by either the max AADF field
//!    size (2-bit → 3, 5-bit → 31) **or** the containing upper-layer cell.
//! 4. If the cells that *would be added* in the new slice are all empty,
//!    increment that direction's distance.
//!
//! The paper's O(3·d·n) "merge with the neighbour's already-computed cuboid"
//! optimisation is *optional* for Phase A's tiny static grid (`03-design.md`
//! §6.1 step 3) — this is the straightforward per-cell expansion.

use crate::aadf::cell::{Aadf6, DIR_NEG_X, DIR_NEG_Y, DIR_NEG_Z, DIR_POS_X, DIR_POS_Y, DIR_POS_Z};

/// An inclusive integer box `[min, max]` in some cell-coordinate space — the
/// bound the cuboid expansion may not cross (the containing upper-layer cell,
/// or the whole world for chunks).
#[derive(Clone, Copy, Debug)]
pub struct CellBox {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl CellBox {
    /// A box spanning `[0, dim)` on every axis (a cell-grid of side `dim`).
    pub fn cube(dim: i32) -> CellBox {
        CellBox {
            min: [0, 0, 0],
            max: [dim - 1, dim - 1, dim - 1],
        }
    }
}

/// Compute the AADF of one empty cell at `cell` (in the coordinate space of
/// `bound`), expanding the empty cuboid by the alternating-axis algorithm.
///
/// - `is_empty(coord)` reports whether the cell at `coord` is empty of
///   geometry. It is only ever queried for coordinates inside `bound`.
/// - `bound` is the inclusive box the cuboid may not cross (the containing
///   upper-layer cell, or the world for chunks).
/// - `max_dist` caps every direction (3 for block/voxel, 31 for chunk).
///
/// `cell` itself must be empty; the returned [`Aadf6`] has all 6 per-direction
/// distances filled.
pub fn compute_aadf(
    cell: [i32; 3],
    bound: CellBox,
    max_dist: u8,
    is_empty: impl Fn([i32; 3]) -> bool,
) -> Aadf6 {
    debug_assert!(is_empty(cell), "compute_aadf called on a non-empty cell");

    // Current cuboid extent: [lo, hi] inclusive, per axis. Starts as the cell.
    let mut lo = cell;
    let mut hi = cell;
    // Whether each of the 6 directions can still grow.
    // Order matches the DIR_* constants: -x,+x,-y,+y,-z,+z.
    let mut open = [true; 6];
    // Resulting per-direction distances.
    let mut dist = [0u8; 6];

    // Alternate axes until no direction can grow any further. Each iteration
    // attempts one +1 step on both directions of one axis (paper §3.3 step 2).
    let mut axis = 0usize;
    while open.iter().any(|&o| o) {
        let neg_dir = axis * 2; // -x / -y / -z
        let pos_dir = axis * 2 + 1; // +x / +y / +z

        // Negative direction of this axis.
        if open[neg_dir] {
            let new_lo = lo[axis] - 1;
            if dist[neg_dir] >= max_dist
                || new_lo < bound.min[axis]
                || !slice_empty(axis, new_lo, lo, hi, &is_empty)
            {
                open[neg_dir] = false;
            } else {
                lo[axis] = new_lo;
                dist[neg_dir] += 1;
            }
        }

        // Positive direction of this axis.
        if open[pos_dir] {
            let new_hi = hi[axis] + 1;
            if dist[pos_dir] >= max_dist
                || new_hi > bound.max[axis]
                || !slice_empty(axis, new_hi, lo, hi, &is_empty)
            {
                open[pos_dir] = false;
            } else {
                hi[axis] = new_hi;
                dist[pos_dir] += 1;
            }
        }

        axis = (axis + 1) % 3;
    }

    Aadf6 {
        d: [
            dist[DIR_NEG_X],
            dist[DIR_POS_X],
            dist[DIR_NEG_Y],
            dist[DIR_POS_Y],
            dist[DIR_NEG_Z],
            dist[DIR_POS_Z],
        ],
    }
}

/// Test whether the new slice added by stepping `axis` to `axis_coord` is
/// entirely empty.
///
/// The slice spans the *current* cuboid extent `[lo, hi]` on the other two
/// axes and sits at `axis_coord` on `axis` (paper §3.3 step 4).
fn slice_empty(
    axis: usize,
    axis_coord: i32,
    lo: [i32; 3],
    hi: [i32; 3],
    is_empty: &impl Fn([i32; 3]) -> bool,
) -> bool {
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    for cu in lo[u]..=hi[u] {
        for cv in lo[v]..=hi[v] {
            let mut coord = [0i32; 3];
            coord[axis] = axis_coord;
            coord[u] = cu;
            coord[v] = cv;
            if !is_empty(coord) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In an entirely empty 4³ cube, a corner cell can expand 3 in the +x/+y/+z
    /// directions and 0 in the -x/-y/-z directions (bounded by the cube).
    #[test]
    fn empty_cube_corner_cell() {
        let bound = CellBox::cube(4);
        let aadf = compute_aadf([0, 0, 0], bound, 3, |_| true);
        // -x,+x,-y,+y,-z,+z
        assert_eq!(aadf.d, [0, 3, 0, 3, 0, 3]);
    }

    /// A centre-ish cell in an empty 4³ cube expands toward both walls,
    /// bounded by the cube on the short side.
    #[test]
    fn empty_cube_inner_cell() {
        let bound = CellBox::cube(4);
        let aadf = compute_aadf([1, 1, 1], bound, 3, |_| true);
        // -x: to 0 (1 cell); +x: to 3 (2 cells); same for y, z.
        assert_eq!(aadf.d, [1, 2, 1, 2, 1, 2]);
    }

    /// `max_dist` caps the expansion even when the bound would allow more.
    #[test]
    fn max_dist_caps_expansion() {
        // A large empty world, but a 2-bit AADF caps every direction at 3.
        let bound = CellBox {
            min: [-100, -100, -100],
            max: [100, 100, 100],
        };
        let aadf = compute_aadf([0, 0, 0], bound, 3, |_| true);
        assert_eq!(aadf.d, [3, 3, 3, 3, 3, 3]);
    }

    /// A solid wall at `x == 2` blocks +x expansion: the cell at x=0 can only
    /// reach x=1 (distance 1).
    #[test]
    fn wall_blocks_expansion() {
        let bound = CellBox::cube(4);
        // Occupied iff x == 2.
        let aadf = compute_aadf([0, 1, 1], bound, 3, |c| c[0] != 2);
        // +x stops before x=2 → distance 1. The slice test for the very first
        // +y / +z step would include x in [0,0] only, all empty, so y/z still
        // grow until they hit a slice that includes x=2... but the cuboid's x
        // extent is still [0,0] when y/z first expand. After +x is capped at 1
        // the x-extent becomes [0,1]; subsequent y/z slices then include x=2.
        // -x: 0 (wall side is +x, -x bounded by cube edge).
        assert_eq!(aadf.d[DIR_NEG_X], 0);
        assert_eq!(aadf.d[DIR_POS_X], 1);
    }

    /// The cuboid is genuinely a *box*: once x has grown, a later y-slice that
    /// would sweep occupied geometry inside the widened x-extent is rejected.
    #[test]
    fn expansion_keeps_cuboid_empty() {
        let bound = CellBox::cube(4);
        // Occupied only at (3, 3, 0).
        let aadf = compute_aadf([0, 0, 0], bound, 3, |c| c != [3, 3, 0]);
        // The final cuboid [lo..hi] must not contain (3,3,0). Reconstruct it:
        let lo = [
            0 - aadf.d[DIR_NEG_X] as i32,
            0 - aadf.d[DIR_NEG_Y] as i32,
            0 - aadf.d[DIR_NEG_Z] as i32,
        ];
        let hi = [
            0 + aadf.d[DIR_POS_X] as i32,
            0 + aadf.d[DIR_POS_Y] as i32,
            0 + aadf.d[DIR_POS_Z] as i32,
        ];
        let contains = (lo[0]..=hi[0]).contains(&3)
            && (lo[1]..=hi[1]).contains(&3)
            && (lo[2]..=hi[2]).contains(&0);
        assert!(!contains, "cuboid {lo:?}..={hi:?} swept occupied cell (3,3,0)");
    }
}
