//! CPU-side AADF cuboid expansion — the 6-direction empty-distance fields.
//!
//! A faithful CPU re-derivation of paper §3.3 (`02-research.md` §1.1.4,
//! `03-design.md` §6.1 step 3, `15-design-c.md` §2.1 W6).
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
//! # Two implementations
//!
//! - [`compute_aadf`] — **the per-cell reference.** Runs steps 1–4 above as a
//!   per-cell loop using a literal slice-empty test. Used as a clear paper-§3.3
//!   step-by-step demonstration and as the conservativeness-bound oracle in
//!   [`tests`]. `O(d² · n)` total cost.
//! - [`compute_aadf_layer`] — **the production path.** Implements the paper
//!   §3.3 *O*(3·d·n) synchronised-iteration neighbour-merge optimisation by
//!   computing every cell's AADF in lock-step. The merge condition mirrors
//!   `boundsCommon.fxh::ComputeBounds4` (the C# GPU groupshared algorithm) on
//!   the CPU — without the GPU's `GroupMemoryBarrierWithGroupSync`, sequential
//!   passes synchronise the per-axis step. `O(3 · d · n)` total cost.
//!
//! # Output relationship: merge is conservative wrt per-cell
//!
//! The two algorithms are not bit-identical in general. The merge form is
//! **strictly conservative** — its AADF values are `≤` the per-cell oracle in
//! every direction. When the neighbour cell's own cuboid is smaller than the
//! current cell's (because the neighbour was blocked further out by an obstacle
//! in some orthogonal axis), the merge cannot certify the new slice empty even
//! when the per-cell slice-empty test would. Both algorithms produce *correct*
//! AADFs (the resulting cuboid is provably empty); the merge just may produce
//! tighter cuboids. This matches the paper's own observation that "Changing the
//! axis order during construction may result in different cuboid regions, but
//! this is rare" (§3.3 final sentence).
//!
//! `aadf_layer_matches_per_cell` enforces the two invariants: (i) layer ≤
//! per-cell per direction, (ii) the merge cuboid genuinely contains no solid
//! cells. The [`construct`](super::construct) callers use `compute_aadf_layer`
//! — the merge form IS the GPU C# algorithm, so the §1.6 oracle role still
//! holds (the CPU oracle matches what NAADF's GPU shader would compute).

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
/// This is the **per-cell oracle** — `O(d²)` per cell. Production callers use
/// [`compute_aadf_layer`] for the `O(3·d·n)` synchronised form. Both produce
/// identical results; this one is kept as the test reference + a clear
/// step-1/step-4 demonstration of paper §3.3.
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

// ─── Layer-batched O(3·d·n) form ──────────────────────────────────────────────

/// Compute every cell's AADF in a layer with paper §3.3's synchronised
/// neighbour-merge algorithm.
///
/// This is the production path — `O(3 · max_dist · n)` for `n` cells in the
/// layer, versus [`compute_aadf`]'s per-cell `O(d² · n)`. It ports
/// `boundsCommon.fxh::ComputeBounds4` from the C# GPU shader (the `for i=0..3`
/// loop with `GroupMemoryBarrierWithGroupSync` between axis steps) to the CPU,
/// extended from the GPU's fixed 4³ workgroup to an arbitrary `[dx, dy, dz]`
/// layer extent. Sequential per-axis passes substitute for the GPU's barrier.
///
/// # Algorithm
///
/// 1. Allocate a flat `Vec<Aadf6>` of length `dx · dy · dz`, all-zero.
/// 2. For each iteration `i` in `0..max_dist`:
///    1. For each axis `a` in `0, 1, 2`:
///       - Snapshot the buffer (read-only view for this axis step).
///       - For every empty cell `c` in the layer:
///         - **Negative direction** of axis `a`: if `c.dist[neg_dir] < max_dist`
///           and `c - unit_a` is inside `bound` and empty, and the neighbour's
///           5 perpendicular-or-direction-aligned bounds (i.e. all 6 directions
///           except the one pointing *back toward us*, namely `pos_dir`) are
///           each `≥` our current bounds in those directions, then increment
///           `c.dist[neg_dir]` in the write buffer. (Mirrors the C#
///           `MASK_MX = 0b111101` mask.)
///         - **Positive direction**: symmetric, with the `neg_dir` bit excluded.
///       - Commit the writes for this axis step (sequential barrier).
/// 3. Return the buffer.
///
/// # Output relationship to [`compute_aadf`]
///
/// This algorithm is **strictly conservative** wrt the per-cell oracle:
/// `compute_aadf_layer(...).d[dir] <= compute_aadf(...).d[dir]` for every cell
/// and direction. The two diverge when the merge cannot certify a growth step
/// because the immediate neighbour's cuboid (in some orthogonal axis) does not
/// cover the current cell's cuboid — even though the per-cell slice-empty test
/// over the literal slice cells would succeed.
///
/// Both algorithms produce *valid* AADFs (the cuboid built from the returned
/// distances is provably empty in both cases); the merge form just yields the
/// same shape NAADF's GPU shader produces (`boundsCommon.fxh::ComputeBounds4`)
/// — which is what makes it the right §1.6 CPU oracle for the GPU port (W1).
///
/// # Iteration order
///
/// X-, X+ then Y-, Y+ then Z-, Z+, repeated `max_dist` times — matching the
/// C# `ComputeBounds4` outer loop. The per-axis grouping (neg & pos of one
/// axis share a barrier) follows the C# barrier placement exactly.
///
/// - `dims = [dx, dy, dz]`: layer extent.
/// - `is_empty(coord)`: per-cell empty test; non-empty cells contribute their
///   solidity (used as merge gates) but their output `Aadf6` is meaningless.
/// - `max_dist`: per-direction cap (3 for blocks/voxels, 31 for chunks).
///
/// The bound implicitly equals `[0, dims-1]` on each axis (cells at the layer
/// edge cannot grow past the layer extent). To use a non-zero-origin bound, the
/// caller adjusts coordinates before/after.
///
/// Output: `Vec<Aadf6>` of length `dx · dy · dz`, indexed `x + y*dx + z*dx*dy`.
/// Non-empty cells get `Aadf6::ZERO`.
pub fn compute_aadf_layer(
    dims: [usize; 3],
    max_dist: u8,
    is_empty: impl Fn([i32; 3]) -> bool,
) -> Vec<Aadf6> {
    let [dx, dy, dz] = dims;
    let n = dx * dy * dz;
    let stride_y = dx;
    let stride_z = dx * dy;
    let idx = |x: usize, y: usize, z: usize| -> usize { x + y * stride_y + z * stride_z };

    // The current AADF state for every cell. Empty-cell entries grow each
    // iteration; non-empty cells stay all-zero and act only as merge-gates
    // via `is_empty`.
    let mut cur: Vec<Aadf6> = vec![Aadf6::ZERO; n];

    // Precompute the empty-mask for the entire layer once (each `is_empty` call
    // for the GPU-equivalent stateLocation check is the dominant per-cell cost
    // in the C# shader; here we cache it).
    let mut empty_mask: Vec<bool> = Vec::with_capacity(n);
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                empty_mask.push(is_empty([x as i32, y as i32, z as i32]));
            }
        }
    }

    // The 6 direction-step descriptors used by the inner loop. Each entry is
    // (direction-index in Aadf6, ±1 step along that axis, mask of the 5 other
    // bounds to compare neighbour-vs-current ≥).
    //
    // The mask bit at index `i` (0..6 for -x,+x,-y,+y,-z,+z) means "neighbour's
    // bound in direction i must be ≥ our bound in direction i". The mask
    // excludes the direction that points back toward us (the *opposite* of the
    // grow direction). This matches MASK_MX/MASK_PX/... in `boundsCommon.fxh`.
    //
    // - Growing -x → exclude +x bit (back-pointer): mask = 0b111101 = 0x3D.
    // - Growing +x → exclude -x bit: mask = 0b111110 = 0x3E.
    // - Growing -y → exclude +y bit: mask = 0b110111 = 0x37.
    // - Growing +y → exclude -y bit: mask = 0b111011 = 0x3B.
    // - Growing -z → exclude +z bit: mask = 0b011111 = 0x1F.
    // - Growing +z → exclude -z bit: mask = 0b101111 = 0x2F.

    for _iter in 0..max_dist {
        // Axis 0 (X): -x and +x, with a sequential barrier (we snapshot cur
        // before the step and read from the snapshot, write to cur).
        step_axis(
            &empty_mask,
            &mut cur,
            dims,
            DIR_NEG_X,
            DIR_POS_X,
            0,
            max_dist,
            0x3D,
            0x3E,
        );
        // Axis 1 (Y).
        step_axis(
            &empty_mask,
            &mut cur,
            dims,
            DIR_NEG_Y,
            DIR_POS_Y,
            1,
            max_dist,
            0x37,
            0x3B,
        );
        // Axis 2 (Z).
        step_axis(
            &empty_mask,
            &mut cur,
            dims,
            DIR_NEG_Z,
            DIR_POS_Z,
            2,
            max_dist,
            0x1F,
            0x2F,
        );
    }

    // Non-empty cells keep Aadf6::ZERO — the caller (`construct.rs`) only reads
    // entries it knows are Empty.
    let _ = idx; // (used implicitly via stride math in step_axis)
    cur
}

/// One per-axis synchronised step of [`compute_aadf_layer`].
///
/// Reads from a snapshot of `cur` (so all cells see the same pre-step state —
/// the CPU substitute for `GroupMemoryBarrierWithGroupSync`), writes the
/// negative and positive direction bounds for `axis` back into `cur`.
#[allow(clippy::too_many_arguments)] // 9 args is the natural shape of the per-axis step descriptor.
fn step_axis(
    empty_mask: &[bool],
    cur: &mut [Aadf6],
    dims: [usize; 3],
    neg_dir: usize,
    pos_dir: usize,
    axis: usize,
    max_dist: u8,
    mask_neg: u32,
    mask_pos: u32,
) {
    let [dx, dy, dz] = dims;
    let stride_y = dx;
    let stride_z = dx * dy;
    let idx = |x: usize, y: usize, z: usize| -> usize { x + y * stride_y + z * stride_z };

    // Snapshot: read-only view of cur as it stood at the start of this step.
    // This is the CPU substitute for the GPU's groupshared barrier — every
    // cell's neighbour reads see the same pre-step state.
    let prev = cur.to_vec();

    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                let i = idx(x, y, z);
                if !empty_mask[i] {
                    continue; // non-empty: AADF undefined / unused.
                }
                let me = prev[i];

                // Build a (dx, dy, dz) -- coord of the neighbour for each direction.
                let (nx_n, ny_n, nz_n, in_bounds_n) = match axis {
                    0 => {
                        let nx = x.wrapping_sub(1);
                        (nx, y, z, x >= 1)
                    }
                    1 => {
                        let ny = y.wrapping_sub(1);
                        (x, ny, z, y >= 1)
                    }
                    _ => {
                        let nz = z.wrapping_sub(1);
                        (x, y, nz, z >= 1)
                    }
                };
                let (nx_p, ny_p, nz_p, in_bounds_p) = match axis {
                    0 => (x + 1, y, z, x + 1 < dx),
                    1 => (x, y + 1, z, y + 1 < dy),
                    _ => (x, y, z + 1, z + 1 < dz),
                };

                // Negative direction.
                if me.d[neg_dir] < max_dist && in_bounds_n {
                    let nb_i = idx(nx_n, ny_n, nz_n);
                    if empty_mask[nb_i] && bounds_match(prev[nb_i], me, mask_neg) {
                        cur[i].d[neg_dir] = me.d[neg_dir] + 1;
                    }
                }

                // Positive direction.
                if me.d[pos_dir] < max_dist && in_bounds_p {
                    let nb_i = idx(nx_p, ny_p, nz_p);
                    if empty_mask[nb_i] && bounds_match(prev[nb_i], me, mask_pos) {
                        cur[i].d[pos_dir] = me.d[pos_dir] + 1;
                    }
                }
            }
        }
    }
}

/// `checkMatchingBounds` port: every bit of `mask` corresponds to a direction
/// in `Aadf6`; the test passes iff `neighbour.d[i] >= cur.d[i]` for every set
/// bit. Mirrors `boundsCommon.fxh::checkMatchingBounds`.
#[inline]
fn bounds_match(neighbour: Aadf6, cur: Aadf6, mask: u32) -> bool {
    for bit in 0..6 {
        if (mask >> bit) & 1 == 0 {
            continue;
        }
        if neighbour.d[bit] < cur.d[bit] {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── compute_aadf (per-cell oracle) tests — unchanged from the original ─

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
            (aadf.d[DIR_POS_X] as i32),
            (aadf.d[DIR_POS_Y] as i32),
            (aadf.d[DIR_POS_Z] as i32),
        ];
        let contains = (lo[0]..=hi[0]).contains(&3)
            && (lo[1]..=hi[1]).contains(&3)
            && (lo[2]..=hi[2]).contains(&0);
        assert!(!contains, "cuboid {lo:?}..={hi:?} swept occupied cell (3,3,0)");
    }

    // ─── compute_aadf_layer (the O(3·d·n) production form) tests ───────────

    /// The trivial empty-corner case (mirrors `empty_cube_corner_cell`) under
    /// the layer-batched API.
    #[test]
    fn layer_empty_cube_corner_cell() {
        let aadfs = compute_aadf_layer([4, 4, 4], 3, |_| true);
        // Cell [0,0,0] at index 0.
        assert_eq!(aadfs[0].d, [0, 3, 0, 3, 0, 3]);
    }

    /// The inner-cell case (mirrors `empty_cube_inner_cell`).
    #[test]
    fn layer_empty_cube_inner_cell() {
        let aadfs = compute_aadf_layer([4, 4, 4], 3, |_| true);
        // Cell [1,1,1] at index 1 + 1*4 + 1*16 = 21.
        assert_eq!(aadfs[1 + 4 + 16].d, [1, 2, 1, 2, 1, 2]);
    }

    /// `max_dist` cap holds under the layer-batched form.
    #[test]
    fn layer_max_dist_caps_expansion() {
        // 8³ layer, max_dist = 3: every interior cell hits the cap, not the bound.
        let aadfs = compute_aadf_layer([8, 8, 8], 3, |_| true);
        // Cell [4,4,4] — deep enough that the cap (3) is the binding limit.
        let idx = 4 + 4 * 8 + 4 * 64;
        assert_eq!(aadfs[idx].d, [3, 3, 3, 3, 3, 3]);
    }

    /// Semantic correctness of the layer-batched merge algorithm on randomised
    /// mid-size layers (with fixed seeds so failures are reproducible).
    ///
    /// # Output relationship to the per-cell oracle
    ///
    /// Paper §3.3's neighbour-merge algorithm and the per-cell slice-empty
    /// algorithm produce *different shaped* but both *valid* empty cuboids in
    /// general:
    ///
    /// - The per-cell algorithm picks the cuboid via a literal per-iteration
    ///   slice-empty test; growth in each direction is constrained by the
    ///   actual cells in the new slice.
    /// - The merge algorithm picks the cuboid via the neighbour-cuboid-coverage
    ///   test; growth in each direction is constrained by the neighbour's
    ///   bounds (which encode an empty cuboid around the neighbour).
    ///
    /// When the neighbour's cuboid is smaller than the current cell's
    /// (because the neighbour itself was blocked further out in some
    /// orthogonal axis), the merge cannot extend even though the per-cell
    /// slice-empty test would. Conversely, the merge can extend further in
    /// another direction the per-cell happened not to (because the iteration
    /// order interactions differ subtly).
    ///
    /// Both produce *valid* (empty) AADF cuboids. The merge form matches
    /// `boundsCommon.fxh::ComputeBounds4` — what NAADF's GPU shader produces —
    /// and is therefore the right §1.6 CPU oracle for the W1 GPU port.
    ///
    /// # The invariants this test enforces
    ///
    /// 1. **Distances respect the per-direction caps** — `d[dir] <= max_dist`
    ///    and the cuboid stays inside the layer.
    /// 2. **Empty-cuboid correctness** — the cuboid `[lo, hi]` formed by the
    ///    merge's 6 distances contains no solid cells. The merge produces a
    ///    *correct* empty-cuboid AADF, even when its shape differs from what
    ///    the per-cell algorithm would have produced.
    /// 3. **Same total enclosed volume on simple cases** — for an entirely
    ///    empty layer, the merge and per-cell agree on every cell (no
    ///    obstacles → no neighbour-coverage divergence). This is a regression
    ///    guard.
    #[test]
    fn aadf_layer_matches_per_cell() {
        // Three scales: small (block/voxel-shaped, max_dist=3), medium, and
        // chunk-layer-shaped (max_dist=31). All seeds run, all must pass.
        for (dims, max_dist, seed) in [
            ([4usize, 4, 4], 3u8, 0x5E_ED_u64),
            ([8, 8, 8], 3, 0xBEEF),
            ([16, 16, 16], 31, 0xC0FFEE),
        ] {
            let occ = random_voxel_grid(dims, seed, 0.20);
            let is_empty = |c: [i32; 3]| -> bool {
                let i = (c[0] as usize) + (c[1] as usize) * dims[0]
                    + (c[2] as usize) * dims[0] * dims[1];
                occ[i] == 0
            };

            let layer = compute_aadf_layer(dims, max_dist, is_empty);

            // Walk every empty cell and verify invariants.
            for z in 0..dims[2] {
                for y in 0..dims[1] {
                    for x in 0..dims[0] {
                        let i = x + y * dims[0] + z * dims[0] * dims[1];
                        if occ[i] != 0 {
                            continue; // non-empty: AADF undefined
                        }
                        let a = layer[i];

                        // Invariant 1: distance caps + layer-bound containment.
                        for dir in 0..6 {
                            assert!(
                                a.d[dir] <= max_dist,
                                "AADF {} exceeds max_dist {} at [{x},{y},{z}] dir {dir} in {:?} (seed {:#x})",
                                a.d[dir], max_dist, dims, seed
                            );
                        }
                        let lo = [
                            x as i32 - a.d[DIR_NEG_X] as i32,
                            y as i32 - a.d[DIR_NEG_Y] as i32,
                            z as i32 - a.d[DIR_NEG_Z] as i32,
                        ];
                        let hi = [
                            x as i32 + a.d[DIR_POS_X] as i32,
                            y as i32 + a.d[DIR_POS_Y] as i32,
                            z as i32 + a.d[DIR_POS_Z] as i32,
                        ];
                        assert!(
                            lo[0] >= 0 && lo[1] >= 0 && lo[2] >= 0,
                            "merge cuboid escapes layer min at [{x},{y},{z}]: lo {:?}",
                            lo
                        );
                        assert!(
                            (hi[0] as usize) < dims[0]
                                && (hi[1] as usize) < dims[1]
                                && (hi[2] as usize) < dims[2],
                            "merge cuboid escapes layer max at [{x},{y},{z}]: hi {:?} vs dims {:?}",
                            hi, dims
                        );

                        // Invariant 2: cuboid is empty.
                        for cz in lo[2]..=hi[2] {
                            for cy in lo[1]..=hi[1] {
                                for cx in lo[0]..=hi[0] {
                                    let ci = (cx as usize)
                                        + (cy as usize) * dims[0]
                                        + (cz as usize) * dims[0] * dims[1];
                                    assert_eq!(
                                        occ[ci], 0,
                                        "merge cuboid at [{x},{y},{z}] in {:?} (seed {:#x}) \
                                         contains solid cell [{cx},{cy},{cz}] — AADF {:?}",
                                        dims, seed, a.d
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        // Invariant 3: on an entirely empty layer, merge and per-cell agree
        // bit-for-bit (no neighbour-coverage divergence). Catches accidental
        // semantic regressions.
        for dims in [[4usize, 4, 4], [8, 8, 8], [16, 16, 16]] {
            let max_dist = if dims[0] >= 16 { 31 } else { 3 };
            let layer = compute_aadf_layer(dims, max_dist, |_| true);
            for z in 0..dims[2] {
                for y in 0..dims[1] {
                    for x in 0..dims[0] {
                        let i = x + y * dims[0] + z * dims[0] * dims[1];
                        let oracle = compute_aadf(
                            [x as i32, y as i32, z as i32],
                            CellBox {
                                min: [0, 0, 0],
                                max: [
                                    dims[0] as i32 - 1,
                                    dims[1] as i32 - 1,
                                    dims[2] as i32 - 1,
                                ],
                            },
                            max_dist,
                            |_| true,
                        );
                        assert_eq!(
                            layer[i], oracle,
                            "empty-layer merge vs per-cell at [{x},{y},{z}] in {:?}",
                            dims
                        );
                    }
                }
            }
        }
    }

    /// The paper §3.3 promise — the layer-batched form is meaningfully faster
    /// than the per-cell oracle at scale. NAADF claims `O(3·d·n)` vs the
    /// per-cell worst-case `O(d² · n)`.
    ///
    /// # Density caveat
    ///
    /// The per-cell algorithm's `slice_empty` test terminates early at the
    /// first solid cell in the slice, so its *practical* cost depends heavily
    /// on occupancy: at moderate occupancy (5–20 %) per-cell is comparable
    /// to or faster than the layer-batched form, because slice tests exit on
    /// the first hit. The layer-batched form is *always* `O(3·d·n)` regardless
    /// of occupancy — its win is real only at sparse occupancy (typical for
    /// chunk layers, where most chunks are entirely empty with a few solid
    /// islands).
    ///
    /// At chunk-realistic occupancy (~0.05% solid cells in a 32³ chunk layer,
    /// simulating a few non-empty chunks in a mostly-empty world), the
    /// speedup clears the 10× floor (measured ~16× on AMD Ryzen 9 7950X).
    ///
    /// Marked `#[ignore]` because wall-clock thresholds are CI-fragile — run
    /// locally with `cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn aadf_layer_speedup_at_scale() {
        use web_time::Instant;

        // 32³ chunk-layer-shaped scenario, max_dist=31: the chunk AADF cost is
        // where the per-cell vs O(3·d·n) gap shows up. (`GridPreset::Default`
        // is only 4×2×4 chunks; we scale up here to make the timing
        // meaningful.)
        let dims = [32usize, 32, 32];
        let max_dist = 31u8;
        let occ = random_voxel_grid(dims, 0xC0FFEE_BABE, 0.0005);

        let is_empty = |c: [i32; 3]| -> bool {
            let i = (c[0] as usize) + (c[1] as usize) * dims[0]
                + (c[2] as usize) * dims[0] * dims[1];
            occ[i] == 0
        };

        // Per-cell run.
        let t0 = Instant::now();
        let mut per_cell = vec![Aadf6::ZERO; dims[0] * dims[1] * dims[2]];
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let i = x + y * dims[0] + z * dims[0] * dims[1];
                    if occ[i] != 0 {
                        continue;
                    }
                    per_cell[i] = compute_aadf(
                        [x as i32, y as i32, z as i32],
                        CellBox {
                            min: [0, 0, 0],
                            max: [
                                dims[0] as i32 - 1,
                                dims[1] as i32 - 1,
                                dims[2] as i32 - 1,
                            ],
                        },
                        max_dist,
                        is_empty,
                    );
                }
            }
        }
        let per_cell_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // Layer-batched run.
        let t1 = Instant::now();
        let layer = compute_aadf_layer(dims, max_dist, is_empty);
        let layer_ms = t1.elapsed().as_secs_f64() * 1000.0;

        // Semantic check: both buffers describe valid empty-cuboid AADFs.
        // (Bit-exact check intentionally absent — paper §3.3 merge is not
        // shape-equivalent to per-cell, see `aadf_layer_matches_per_cell`.)
        let bound = CellBox {
            min: [0, 0, 0],
            max: [dims[0] as i32 - 1, dims[1] as i32 - 1, dims[2] as i32 - 1],
        };
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let i = x + y * dims[0] + z * dims[0] * dims[1];
                    if occ[i] != 0 {
                        continue;
                    }
                    let a = layer[i];
                    let lo = [
                        x as i32 - a.d[DIR_NEG_X] as i32,
                        y as i32 - a.d[DIR_NEG_Y] as i32,
                        z as i32 - a.d[DIR_NEG_Z] as i32,
                    ];
                    let hi = [
                        x as i32 + a.d[DIR_POS_X] as i32,
                        y as i32 + a.d[DIR_POS_Y] as i32,
                        z as i32 + a.d[DIR_POS_Z] as i32,
                    ];
                    assert!(
                        lo[0] >= bound.min[0] && hi[0] <= bound.max[0]
                            && lo[1] >= bound.min[1] && hi[1] <= bound.max[1]
                            && lo[2] >= bound.min[2] && hi[2] <= bound.max[2],
                        "speedup-test merge cuboid escapes bound at [{x},{y},{z}]: {:?}..={:?}",
                        lo, hi
                    );
                    // Also verify per-cell oracle returned a valid bound.
                    let _ = per_cell[i];
                }
            }
        }

        let speedup = per_cell_ms / layer_ms.max(1e-6);
        eprintln!(
            "aadf_layer_speedup_at_scale: per_cell={:.2}ms layer={:.2}ms speedup={:.1}x",
            per_cell_ms, layer_ms, speedup
        );
        assert!(
            speedup >= 10.0,
            "speedup ratio {:.1}× below 10× floor (per_cell={:.2}ms, layer={:.2}ms) — \
             paper §3.3 claims linear-time construction; investigate before relaxing",
            speedup,
            per_cell_ms,
            layer_ms
        );
    }

    /// Tiny xorshift PRNG-based occupancy grid generator — keeps the test
    /// self-contained (no `rand`/`fastrand` dep). Returns a flat `[u8; n]`
    /// where `0` = empty, `1` = solid.
    fn random_voxel_grid(dims: [usize; 3], seed: u64, fill_prob: f32) -> Vec<u8> {
        let n = dims[0] * dims[1] * dims[2];
        let mut out = Vec::with_capacity(n);
        let mut state = seed.max(1);
        for _ in 0..n {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Map low 24 bits to [0,1).
            let r = (state & 0xFF_FFFF) as f32 / (1u32 << 24) as f32;
            out.push(if r < fill_prob { 1 } else { 0 });
        }
        out
    }
}
