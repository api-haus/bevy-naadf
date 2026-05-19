//! `streaming::sliding_window` — pure-compute primitive that translates an
//! "old origin → new origin" shift into the (evict, admit) world-segment
//! delta the residency layer needs.
//!
//! ## Phase 2.14.c — primitive extraction
//!
//! Pre-extraction (Phase 2.6 → 2.14.b), the "old vs new origin → (evict, admit)"
//! computation lived in two places:
//!
//! - [`super::windowed_slot_map::WindowedSlotMap::set_origin`] computed the
//!   **evict** half inline (walks `world_to_slot` and filters out segments
//!   that fall outside the new AABB).
//! - `super::residency::residency_driver` Pass 2 computed the **admit**
//!   half inline with three nested `for lz / for ly / for lx` loops + a
//!   `resident.contains()` filter.
//!
//! Per the Phase 2.14 primitive audit (`04-audit-primitives.md` §
//! "Proposed primitive extractions" item 2) the split made each half
//! independently un-testable: an admit-iteration regression would only
//! surface through the e2e gate. This module pulls both halves into one
//! pure function over `WorldSegmentPos` + `IVec3` + `HashSet`, with no
//! Bevy / GPU / `&mut state` dependency.
//!
//! ## Iteration order — load-bearing
//!
//! The `admit` list is produced by iterating the new window's local
//! positions in `for lz / for ly / for lx` order (Z-slowest, Y, X-fastest).
//! That order matches `residency_driver` Pass 2's pre-extraction loop
//! shape, which the slot-assignment policy + the `oasis-edit-visual` e2e
//! pixel-diff gate (Phase 2.14.g) implicitly depend on. Changing the
//! order would not break correctness but would shift slot assignments
//! across the cold-start admission sort, which downstream tests pin to
//! the existing order.
//!
//! ## Caller contract
//!
//! `currently_bound` is the set of `WorldSegmentPos` that are bound
//! BEFORE the shift. The function does not mutate it — it only reads
//! membership. Callers (`WindowedSlotMap`, `residency_driver`) own the
//! actual binding state; this module is a query helper.

use std::collections::HashSet;

use bevy::math::{IVec3, UVec3};

use super::residency::WorldSegmentPos;

/// Result of [`compute_window_delta`] — the segments to evict (no longer
/// in the window) and the segments to admit (newly in the window, not
/// already bound).
///
/// The two `Vec`s are disjoint by construction: a segment in the new
/// window cannot also be outside it, and `admit` skips segments that
/// are already in `currently_bound` (which is where `evict` is sourced
/// from).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowDelta {
    /// Segments that were in `currently_bound` whose position is NOT
    /// inside `[new_origin, new_origin + window_size)`. Iteration order
    /// is unspecified — the caller-side `WindowedSlotMap::set_origin`
    /// path doesn't depend on a particular eviction order (each eviction
    /// fires an independent callback).
    pub evict: Vec<WorldSegmentPos>,
    /// Segments inside `[new_origin, new_origin + window_size)` that are
    /// NOT in `currently_bound`. Iteration order is `for lz / for ly /
    /// for lx` (X-fastest), matching the pre-extraction `residency_driver`
    /// Pass 2 loop.
    pub admit: Vec<WorldSegmentPos>,
}

/// Compute the eviction + admission sets implied by translating the
/// sliding window from `old_origin` to `new_origin`.
///
/// - `evict` = segments in `currently_bound` whose world position is
///   OUTSIDE the new window AABB `[new_origin, new_origin + window_size)`.
/// - `admit` = segments INSIDE the new window AABB that are NOT in
///   `currently_bound`.
///
/// `old_origin` is not used in the computation directly — the new
/// window's AABB is determined by `new_origin + window_size`, and the
/// eviction set is determined by `currently_bound \ new_window`. The
/// parameter is part of the API for symmetry + future enrichment (e.g.
/// a fast-path no-op short-circuit when `old_origin == new_origin`).
///
/// The function is pure: no `&mut`, no I/O, no Bevy world access.
///
/// ## Invariants the test suite carries
///
/// - **Identity:** `old_origin == new_origin` with empty `currently_bound`
///   → empty `evict`, `admit` covers every segment in the window once.
/// - **Translation:** shift origin by `+1` on X → `evict.len() == y * z`
///   (all segments at `local.x == 0`), `admit.len() == y * z` (all
///   segments at `local.x == window_size.x - 1` in the NEW window).
/// - **Disjointness:** `evict ∩ admit == ∅`.
/// - **Closure:** every segment in `[new_origin, new_origin + window_size)`
///   is either in `currently_bound \ evict` or in `admit`.
pub fn compute_window_delta(
    _old_origin: WorldSegmentPos,
    new_origin: WorldSegmentPos,
    window_size: UVec3,
    currently_bound: &HashSet<WorldSegmentPos>,
) -> WindowDelta {
    let new_origin_v = new_origin.0;
    let aabb_min = new_origin_v;
    let aabb_max = IVec3::new(
        new_origin_v.x + window_size.x as i32,
        new_origin_v.y + window_size.y as i32,
        new_origin_v.z + window_size.z as i32,
    );

    let inside_new = |p: IVec3| -> bool {
        p.x >= aabb_min.x
            && p.x < aabb_max.x
            && p.y >= aabb_min.y
            && p.y < aabb_max.y
            && p.z >= aabb_min.z
            && p.z < aabb_max.z
    };

    // Evict: every currently-bound segment whose position falls outside
    // the new window AABB.
    let mut evict: Vec<WorldSegmentPos> = Vec::new();
    for w in currently_bound.iter() {
        if !inside_new(w.0) {
            evict.push(*w);
        }
    }

    // Admit: every position in the new window that is NOT already bound.
    // Iteration order is `for lz / for ly / for lx` (X-fastest) to match
    // the pre-extraction Pass 2 loop. Pre-allocate capacity to avoid
    // reallocs for the standard 512-slot window.
    let cap = (window_size.x * window_size.y * window_size.z) as usize;
    let mut admit: Vec<WorldSegmentPos> = Vec::with_capacity(cap);
    for lz in 0..window_size.z {
        for ly in 0..window_size.y {
            for lx in 0..window_size.x {
                let world_seg = WorldSegmentPos(IVec3::new(
                    new_origin_v.x + lx as i32,
                    new_origin_v.y + ly as i32,
                    new_origin_v.z + lz as i32,
                ));
                if !currently_bound.contains(&world_seg) {
                    admit.push(world_seg);
                }
            }
        }
    }

    WindowDelta { evict, admit }
}

// ---------------------------------------------------------------------------
// Unit tests — Phase 2.14.c. Pure-compute primitive; no Bevy / GPU setup.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fully-bound `HashSet` covering every segment in
    /// `[origin, origin + window_size)`.
    fn fully_bound(origin: IVec3, window_size: UVec3) -> HashSet<WorldSegmentPos> {
        let mut out = HashSet::with_capacity(
            (window_size.x * window_size.y * window_size.z) as usize,
        );
        for lz in 0..window_size.z {
            for ly in 0..window_size.y {
                for lx in 0..window_size.x {
                    out.insert(WorldSegmentPos(IVec3::new(
                        origin.x + lx as i32,
                        origin.y + ly as i32,
                        origin.z + lz as i32,
                    )));
                }
            }
        }
        out
    }

    /// T1 — `identity_no_shift_admits_all_unbound_in_window`. With
    /// `old_origin == new_origin` and empty `currently_bound`, every
    /// segment in the new window is admitted; no evictions.
    #[test]
    fn identity_no_shift_admits_all_unbound_in_window() {
        let origin = WorldSegmentPos(IVec3::new(0, 0, 0));
        let ws = UVec3::new(4, 2, 4);
        let bound: HashSet<WorldSegmentPos> = HashSet::new();
        let delta = compute_window_delta(origin, origin, ws, &bound);

        assert!(delta.evict.is_empty(), "no eviction when nothing bound");
        let expected_admit_count = (ws.x * ws.y * ws.z) as usize;
        assert_eq!(
            delta.admit.len(),
            expected_admit_count,
            "admit must cover every cell in the new window"
        );
        // Each segment in the new window appears exactly once.
        let admit_set: HashSet<WorldSegmentPos> = delta.admit.iter().copied().collect();
        assert_eq!(
            admit_set.len(),
            expected_admit_count,
            "admit must contain no duplicates"
        );
        // Verify coverage by spot-checking corners + interior.
        for lz in 0..ws.z {
            for ly in 0..ws.y {
                for lx in 0..ws.x {
                    let w = WorldSegmentPos(IVec3::new(lx as i32, ly as i32, lz as i32));
                    assert!(
                        admit_set.contains(&w),
                        "missing {:?} from admit set",
                        w
                    );
                }
            }
        }
    }

    /// T2 — `translation_x_plus_one_evicts_leftmost_admits_rightmost`.
    /// Shift by `+1` on X with a fully-bound window. The leftmost-X
    /// slab evicts, the rightmost-X slab admits, both of size `y * z`.
    #[test]
    fn translation_x_plus_one_evicts_leftmost_admits_rightmost() {
        let old_origin = WorldSegmentPos(IVec3::new(0, 0, 0));
        let new_origin = WorldSegmentPos(IVec3::new(1, 0, 0));
        let ws = UVec3::new(4, 2, 4);
        let bound = fully_bound(old_origin.0, ws);

        let delta = compute_window_delta(old_origin, new_origin, ws, &bound);

        let slab = (ws.y * ws.z) as usize;
        assert_eq!(
            delta.evict.len(),
            slab,
            "evict slab size = y*z = {}",
            slab,
        );
        assert_eq!(
            delta.admit.len(),
            slab,
            "admit slab size = y*z = {}",
            slab,
        );

        // Every eviction had `world.x == 0` (the OLD window's local_x==0
        // slab — equivalent to `world.x == old_origin.x` for this fixture).
        for w in &delta.evict {
            assert_eq!(
                w.0.x, old_origin.0.x,
                "evicted segment must be at the OLD window's leftmost X slab"
            );
        }
        // Every admission has `world.x == new_origin.x + window_size.x - 1`
        // (the NEW window's rightmost-X slab).
        let expected_admit_x = new_origin.0.x + ws.x as i32 - 1;
        for w in &delta.admit {
            assert_eq!(
                w.0.x, expected_admit_x,
                "admitted segment must be at the NEW window's rightmost X slab"
            );
        }
    }

    /// T3 — `evict_admit_disjoint`. Under arbitrary input, the two
    /// sets share no element.
    #[test]
    fn evict_admit_disjoint() {
        let old_origin = WorldSegmentPos(IVec3::new(0, 0, 0));
        let new_origin = WorldSegmentPos(IVec3::new(2, 0, 1));
        let ws = UVec3::new(4, 2, 4);
        let bound = fully_bound(old_origin.0, ws);

        let delta = compute_window_delta(old_origin, new_origin, ws, &bound);

        let evict_set: HashSet<WorldSegmentPos> = delta.evict.iter().copied().collect();
        let admit_set: HashSet<WorldSegmentPos> = delta.admit.iter().copied().collect();
        let intersection: Vec<WorldSegmentPos> =
            evict_set.intersection(&admit_set).copied().collect();
        assert!(
            intersection.is_empty(),
            "evict and admit must be disjoint, intersection = {:?}",
            intersection
        );
    }

    /// T4 — `closure_invariant`. Every segment in the NEW window is
    /// either still bound (`currently_bound \ evict`) or in `admit`.
    #[test]
    fn closure_invariant() {
        let old_origin = WorldSegmentPos(IVec3::new(0, 0, 0));
        let new_origin = WorldSegmentPos(IVec3::new(1, 0, 1));
        let ws = UVec3::new(4, 2, 4);
        let bound = fully_bound(old_origin.0, ws);

        let delta = compute_window_delta(old_origin, new_origin, ws, &bound);

        let evict_set: HashSet<WorldSegmentPos> = delta.evict.iter().copied().collect();
        let admit_set: HashSet<WorldSegmentPos> = delta.admit.iter().copied().collect();
        let post_shift_bound: HashSet<WorldSegmentPos> =
            bound.difference(&evict_set).copied().collect();

        // Every segment in the new window must be accounted for.
        for lz in 0..ws.z {
            for ly in 0..ws.y {
                for lx in 0..ws.x {
                    let seg = WorldSegmentPos(IVec3::new(
                        new_origin.0.x + lx as i32,
                        new_origin.0.y + ly as i32,
                        new_origin.0.z + lz as i32,
                    ));
                    let still_bound = post_shift_bound.contains(&seg);
                    let admitted = admit_set.contains(&seg);
                    assert!(
                        still_bound ^ admitted,
                        "segment {:?} must be in exactly one of \
                         (currently_bound \\ evict) or admit; \
                         still_bound={}, admitted={}",
                        seg,
                        still_bound,
                        admitted,
                    );
                }
            }
        }
    }

    /// T5 — `full_shift_no_overlap_evicts_all_admits_all`. Shift past
    /// the entire window (no overlap). Every bound segment evicts;
    /// every new window cell admits.
    #[test]
    fn full_shift_no_overlap_evicts_all_admits_all() {
        let old_origin = WorldSegmentPos(IVec3::new(0, 0, 0));
        let ws = UVec3::new(4, 2, 4);
        // Shift by exactly window_size.x on X — no overlap with old window.
        let new_origin = WorldSegmentPos(IVec3::new(ws.x as i32, 0, 0));
        let bound = fully_bound(old_origin.0, ws);

        let delta = compute_window_delta(old_origin, new_origin, ws, &bound);

        let total_cells = (ws.x * ws.y * ws.z) as usize;
        assert_eq!(
            delta.evict.len(),
            total_cells,
            "every bound segment must evict on full-window shift"
        );
        assert_eq!(
            delta.admit.len(),
            total_cells,
            "every new window cell must admit (none were already bound in the new region)"
        );
        // The evict set is exactly the previously-bound set.
        let evict_set: HashSet<WorldSegmentPos> = delta.evict.iter().copied().collect();
        assert_eq!(evict_set, bound, "evict set == previously bound set");
    }

    /// T6 — `partial_diagonal_shift`. Shift by `(1, 0, 1)`. The X+1 and
    /// Z+1 effects compose: evict the union of leftmost-X and
    /// frontmost-Z slabs; admit the union of rightmost-X and
    /// backmost-Z slabs. For a 4×2×4 window, evict-count and admit-count
    /// match `x_slab + z_slab - corner_overlap = (y*z) + (y*x) - y`.
    /// Here that's `(2*4) + (2*4) - 2 = 8 + 8 - 2 = 14`.
    #[test]
    fn partial_diagonal_shift() {
        let old_origin = WorldSegmentPos(IVec3::new(0, 0, 0));
        let new_origin = WorldSegmentPos(IVec3::new(1, 0, 1));
        let ws = UVec3::new(4, 2, 4);
        let bound = fully_bound(old_origin.0, ws);

        let delta = compute_window_delta(old_origin, new_origin, ws, &bound);

        // Hand-computed expected counts for 4×2×4 + (1,0,1) shift:
        //   evict = segments at OLD local.x==0 OR OLD local.z==0
        //         = (y * z) + (y * x) - (y * 1) (subtract the overlap row)
        //         = (2*4)   + (2*4)   - 2       = 14
        //   admit = segments at NEW local.x==3 OR NEW local.z==3
        //         = same shape: 14
        let expected = 14;
        assert_eq!(
            delta.evict.len(),
            expected,
            "expected evict.len()={} for (1,0,1) shift over {:?} window",
            expected,
            ws,
        );
        assert_eq!(
            delta.admit.len(),
            expected,
            "expected admit.len()={} for (1,0,1) shift over {:?} window",
            expected,
            ws,
        );

        // Verify every eviction lies in the leftmost-X OR frontmost-Z slab
        // of the OLD window.
        for w in &delta.evict {
            let on_old_left_x = w.0.x == old_origin.0.x;
            let on_old_front_z = w.0.z == old_origin.0.z;
            assert!(
                on_old_left_x || on_old_front_z,
                "evicted {:?} must be on old left-X (x==0) or front-Z (z==0) slab",
                w
            );
        }
        // Verify every admission lies in the rightmost-X OR backmost-Z slab
        // of the NEW window.
        let new_right_x = new_origin.0.x + ws.x as i32 - 1;
        let new_back_z = new_origin.0.z + ws.z as i32 - 1;
        for w in &delta.admit {
            let on_new_right_x = w.0.x == new_right_x;
            let on_new_back_z = w.0.z == new_back_z;
            assert!(
                on_new_right_x || on_new_back_z,
                "admitted {:?} must be on new right-X (x={}) or back-Z (z={}) slab",
                w,
                new_right_x,
                new_back_z,
            );
        }
    }
}
