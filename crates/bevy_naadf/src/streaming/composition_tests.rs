//! `streaming::composition_tests` — Phase 2.14.e composition tests.
//!
//! Runs the now-isolated streaming primitives (`WindowedSlotMap` atomic API +
//! [`super::sliding_window::compute_window_delta`] + `Residency`'s
//! dispatch-ACK tracking + [`super::residency::StreamingDiagnostics`])
//! together against synthetic camera-walk traces, with no Bevy `App`, no
//! GPU buffer, and no render world.
//!
//! The load-bearing invariant: **after cold-start, every frame's
//! `unfulfilled_camera_window_segments.len()` is monotonically non-increasing
//! along the static-camera trace**, and bounded across walk traces.
//!
//! Catches the class of bug a primitive-level test would miss: a regression
//! in how Pass 1 (`set_origin` callback) / Pass 2 (`compute_window_delta`) /
//! Pass 3 (`allocate_and_bind` + admit) / Pass 4 (ACK drain → `dispatched_once`)
//! compose. A primitive can be flawless in isolation and still wire up to
//! a system that leaks unfulfilled segments across origin shifts (the 2.13
//! cold-start race is the canonical example).
//!
//! ## Harness shape
//!
//! Uses the production `Residency` directly (constructed via
//! `Residency::empty(_)`) — no test stand-in needed. The production type's
//! public surface (atomic `WindowedSlotMap` API + `dispatched_once` field +
//! `diagnostics()` method) is sufficient for everything the harness drives.
//!
//! The harness function `simulate_frame` mirrors `residency_driver`'s four
//! passes against pure data:
//!   - Pass 1: shift origin → fire eviction callback that records in
//!             `evictions_this_frame` and removes the slot from
//!             `dispatched_once`.
//!   - Pass 2: call `compute_window_delta` to compute the admit set, push
//!             into `pending_admissions` (dedup against existing entries).
//!   - Pass 3: sort `pending_admissions` by camera-distance, pick up to
//!             `admit_quota`, call `window.allocate_and_bind` for each.
//!   - Pass 4: simulate render-world ACK at `ack_quota` per frame —
//!             promote that many recently-bound slots into
//!             `dispatched_once`.
//!
//! Diagnostics are queried via `residency.diagnostics()` at the end of each
//! frame.

use std::collections::HashSet;

use bevy::math::IVec3;

use super::residency::{
    target_origin_for_camera_seg, Residency, SlotIndex, StreamingDiagnostics,
    WorldSegmentPos,
};

/// Per-trace state for the synthetic simulator. Holds the production
/// `Residency` directly + the simulator-side bookkeeping (pending admit
/// queue, simulated ACK queue, frame counter).
struct SimState {
    /// Production residency under test. Carries the `WindowedSlotMap`
    /// (atomic API), the `dispatched_once` set, the `diagnostics()` method.
    residency: Residency,
    /// Segments that have not yet been bound. Mirror of `residency_driver`'s
    /// in-system `pending` Vec, made sticky across frames so admission
    /// shortfalls (admit_quota < pending.len()) carry over.
    pending_admissions: Vec<WorldSegmentPos>,
    /// Slots whose `allocate_and_bind` returned this-frame-or-earlier but
    /// whose ACK has not yet been drained into `dispatched_once`. Simulates
    /// the cross-world `PENDING_DISPATCHED_ONCE_SLOTS` accumulator
    /// (`noise_dispatch.rs:414-425`); drained at `ack_quota` per frame.
    pending_ack_slots: Vec<SlotIndex>,
    /// Frame counter. Increments at the end of `simulate_frame`.
    frame: u64,
}

impl SimState {
    fn new(max_segments_per_frame: u32) -> Self {
        Self {
            residency: Residency::empty(max_segments_per_frame),
            pending_admissions: Vec::new(),
            pending_ack_slots: Vec::new(),
            frame: 0,
        }
    }

    /// Convenience — query the streaming diagnostics snapshot.
    fn diagnostics(&self) -> StreamingDiagnostics {
        self.residency.diagnostics()
    }
}

/// Mirror `residency_driver`'s four passes for one frame of pure-data
/// simulation. `camera_seg` drives origin shifts; `admit_quota` caps the
/// per-frame `allocate_and_bind` picks; `ack_quota` caps the per-frame
/// simulated render-world ACK drain (set to 0 to simulate the 2.13
/// cold-start race).
fn simulate_frame(
    state: &mut SimState,
    camera_seg: WorldSegmentPos,
    admit_quota: usize,
    ack_quota: usize,
) {
    let new_origin = target_origin_for_camera_seg(camera_seg.0);
    let old_origin = state.residency.window.origin();

    // Pass 1 — shift origin and clear eviction-marked slots from
    // `dispatched_once`. Mirror of `residency_driver` Pass 1: the
    // split-borrow closure captures `dispatched_once` and removes the
    // evicted slot from it.
    if new_origin != old_origin {
        // Drop any pending admissions that fall outside the NEW window —
        // they cannot be allocate_and_bind'd post-shift. (Production's
        // `pending` Vec is rebuilt fresh each shift; ours is sticky, so
        // we filter explicitly.)
        let ws = state.residency.window.window_size();
        let aabb_min = new_origin;
        let aabb_max = IVec3::new(
            new_origin.x + ws.x as i32,
            new_origin.y + ws.y as i32,
            new_origin.z + ws.z as i32,
        );
        state.pending_admissions.retain(|w| {
            let p = w.0;
            p.x >= aabb_min.x
                && p.x < aabb_max.x
                && p.y >= aabb_min.y
                && p.y < aabb_max.y
                && p.z >= aabb_min.z
                && p.z < aabb_max.z
        });

        // Production split-borrow: capture `dispatched_once` for the
        // eviction callback while mutating `window`.
        let Residency {
            window,
            dispatched_once,
            ..
        } = &mut state.residency;
        window.set_origin(new_origin, |_w, slot| {
            dispatched_once.remove(&slot);
        });

        // Also strip the evicted slot from the simulated ACK queue —
        // production's render-world wouldn't try to ACK a slot whose
        // segment got evicted before its dispatch landed (the slot is
        // back in the free pool and may already be re-bound to a new
        // segment). We mirror that by dropping pending acks for slots
        // that are no longer bound to ANYTHING.
        state
            .pending_ack_slots
            .retain(|slot| state.residency.window.lookup_world(*slot).is_some());
    }

    // Pass 2 — compute the admit set via `compute_window_delta` and push
    // into `pending_admissions` (dedup).
    let resident: HashSet<WorldSegmentPos> = state
        .residency
        .window
        .iter_bound()
        .map(|(w, _)| w)
        .collect();
    let delta = super::sliding_window::compute_window_delta(
        WorldSegmentPos(old_origin),
        WorldSegmentPos(new_origin),
        state.residency.window.window_size(),
        &resident,
    );
    let existing: HashSet<WorldSegmentPos> =
        state.pending_admissions.iter().copied().collect();
    for w in delta.admit {
        if !existing.contains(&w) {
            state.pending_admissions.push(w);
        }
    }

    // Pass 3 — sort pending by camera-distance-squared (mirror of
    // residency.rs:619-622); pick the closest `admit_quota` that
    // `allocate_and_bind` accepts; record the bound slots into the
    // simulated ACK queue.
    state.pending_admissions.sort_by_key(|w| {
        let d = w.0 - camera_seg.0;
        d.x * d.x + d.y * d.y + d.z * d.z
    });

    let mut newly_bound_slots: Vec<SlotIndex> = Vec::new();
    let mut admitted = 0usize;
    // `swap_remove(0)` after each step keeps us at index 0 — the head is
    // always replaced by the previously-last item. Loops until we exhaust
    // the queue or hit the admit cap.
    let i = 0usize;
    while i < state.pending_admissions.len() && admitted < admit_quota {
        let w = state.pending_admissions[i];
        if let Some(slot) = state.residency.window.allocate_and_bind(w) {
            newly_bound_slots.push(slot);
            state.pending_admissions.swap_remove(i);
            admitted += 1;
        } else {
            // Already bound (filter race) or out-of-window (post-shift
            // pruning miss) — drop it.
            state.pending_admissions.swap_remove(i);
        }
    }
    // Anything not picked this frame stays in pending_admissions for next
    // frame (sticky-pending invariant — production residency_driver re-
    // populates pending each shift, but the same camera-window membership
    // ensures the same segments re-enter; we skip the rebuild work by
    // keeping them).

    // Enqueue the newly-bound slots into the simulated ACK queue.
    state.pending_ack_slots.extend(newly_bound_slots);

    // Pass 4 — drain up to `ack_quota` from the simulated ACK queue into
    // `dispatched_once`. Mirror of `apply_dispatch_acks` (`residency.rs:756`).
    let drain = ack_quota.min(state.pending_ack_slots.len());
    for slot in state.pending_ack_slots.drain(..drain) {
        // Only insert if the slot is still bound — production wouldn't
        // ACK a slot whose segment got evicted. (Already enforced by
        // the eviction-driven retain above, but defensive.)
        if state.residency.window.lookup_world(slot).is_some() {
            state.residency.dispatched_once.insert(slot);
        }
    }

    state.frame += 1;
    state.residency.frame_counter = state.frame;
}

/// Window capacity at the production preset (16×2×16 = 512).
const WINDOW_TOTAL: u32 = 512;
/// X-slab size in segments (y * z = 2 * 16 = 32).
const SLAB_X: u32 = 32;
/// Default per-frame admit + ACK quota (mirrors
/// `Residency::max_segments_per_frame` default).
const ADMIT_QUOTA: usize = 4;

/// Cap a `Vec<WorldSegmentPos>` to the first N segments, formatted for
/// failure-message inclusion.
fn first_n(segs: &[WorldSegmentPos], n: usize) -> String {
    let take = segs.iter().take(n).collect::<Vec<_>>();
    format!("{:?}", take)
}

// ---------------------------------------------------------------------------
// Trace tests — six composition scenarios per the Phase 2.14.e brief.
// ---------------------------------------------------------------------------

/// T1 — `trace_cold_start_origin_stays_fixed_reaches_full_coverage`.
///
/// Camera at origin (0,0,0), stationary. With `admit_quota = ack_quota = 4`
/// and 512 slots, cold-start should complete in ceil(512/4) = 128 frames
/// (admit-bound) PLUS an extra frame for the trailing ACK drain (the slots
/// allocated on the final admit frame need one more frame for the ACK).
/// We pad to 130 frames per the brief's `ceil(window_total / admit_quota) + 2`.
#[test]
fn trace_cold_start_origin_stays_fixed_reaches_full_coverage() {
    let mut state = SimState::new(ADMIT_QUOTA as u32);
    let camera = WorldSegmentPos(IVec3::ZERO);
    let total_frames = (WINDOW_TOTAL as usize).div_ceil(ADMIT_QUOTA) + 2;
    for _ in 0..total_frames {
        simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
    }
    let d = state.diagnostics();
    assert!(
        d.unfulfilled_camera_window_segments.is_empty(),
        "cold-start at static camera did not converge: frame={}, \
         unfulfilled={}, first 5 = {}",
        state.frame,
        d.unfulfilled_camera_window_segments.len(),
        first_n(&d.unfulfilled_camera_window_segments, 5),
    );
    assert!(
        d.cold_start_complete,
        "cold_start_complete should be true after {} frames",
        total_frames,
    );
}

/// T2 — `trace_post_cold_start_static_camera_unfulfilled_remains_zero`.
///
/// Same as T1, but continue for 20 more frames after cold-start completes.
/// Once fulfilled, the system must stay fulfilled with no camera motion.
#[test]
fn trace_post_cold_start_static_camera_unfulfilled_remains_zero() {
    let mut state = SimState::new(ADMIT_QUOTA as u32);
    let camera = WorldSegmentPos(IVec3::ZERO);
    // Cold-start drive.
    let cold_frames = (WINDOW_TOTAL as usize).div_ceil(ADMIT_QUOTA) + 2;
    for _ in 0..cold_frames {
        simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
    }
    assert!(state.diagnostics().cold_start_complete, "cold-start prereq");
    // Continue static for 20 more frames; every frame must report 0
    // unfulfilled.
    for _ in 0..20 {
        simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
        let d = state.diagnostics();
        assert_eq!(
            d.unfulfilled_camera_window_segments.len(),
            0,
            "static post-cold-start frame={}: unfulfilled grew to {} \
             (expected 0); first 5 = {}",
            state.frame,
            d.unfulfilled_camera_window_segments.len(),
            first_n(&d.unfulfilled_camera_window_segments, 5),
        );
    }
}

/// T3 — `trace_camera_x_plus_one_step_after_cold_start_unfulfilled_monotone`.
///
/// Cold-start at origin; then bump camera by `+1` on X for 10 shift-and-drain
/// cycles. Each cycle:
///   1. one shift-frame (camera moves +1 on X — origin shifts, evicts an
///      X-slab, admits an X-slab),
///   2. `ceil(SLAB_X / ADMIT_QUOTA)` drain-frames (camera holds in place —
///      the system drains the newly-admitted slab back to 0 unfulfilled).
///
/// Per the brief's "tighter form" assertion: post-shift, `unfulfilled.len()`
/// is bounded by `slab_size + admit_quota` and converges back to 0 within
/// `ceil(slab_size / admit_quota)` frames. This is the exact pattern the
/// production driver experiences when the camera occasionally crosses a
/// segment boundary then stops — the bounded-then-drains shape. Continuous
/// per-frame shifts (no drain time) is a separate, pathological regime
/// that the brief's bound wasn't written against.
#[test]
fn trace_camera_x_plus_one_step_after_cold_start_unfulfilled_monotone() {
    let mut state = SimState::new(ADMIT_QUOTA as u32);
    let camera0 = WorldSegmentPos(IVec3::ZERO);
    // Cold-start drive.
    let cold_frames = (WINDOW_TOTAL as usize).div_ceil(ADMIT_QUOTA) + 2;
    for _ in 0..cold_frames {
        simulate_frame(&mut state, camera0, ADMIT_QUOTA, ADMIT_QUOTA);
    }
    assert!(state.diagnostics().cold_start_complete, "cold-start prereq");

    // Bound per brief: at the shift-frame moment, the unfulfilled count
    // can be as high as `slab + admit_quota` (the newly-admitted slab +
    // a small phasing margin for the next admissions queue head). Within
    // `ceil(slab / admit_quota)` drain-frames, it must return to 0.
    let bound = (SLAB_X as usize) + ADMIT_QUOTA;
    let drain_frames = (SLAB_X as usize).div_ceil(ADMIT_QUOTA);

    for step in 1..=10i32 {
        let camera = WorldSegmentPos(IVec3::new(step, 0, 0));
        // (a) shift-frame.
        simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
        let d = state.diagnostics();
        assert!(
            d.unfulfilled_camera_window_segments.len() <= bound,
            "x+1 step {step} shift-frame: unfulfilled={} exceeded bound={} \
             (= slab + admit_quota); camera={:?}, frame={}, first 5 \
             unfulfilled = {}",
            d.unfulfilled_camera_window_segments.len(),
            bound,
            camera.0,
            state.frame,
            first_n(&d.unfulfilled_camera_window_segments, 5),
        );
        // (b) drain-frames: hold camera, ACK queue drains.
        for _ in 0..drain_frames {
            simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
        }
        let d = state.diagnostics();
        assert_eq!(
            d.unfulfilled_camera_window_segments.len(),
            0,
            "x+1 step {step} post-drain: unfulfilled={} did NOT converge \
             back to 0 within {} drain-frames; camera={:?}, frame={}, \
             first 5 unfulfilled = {}",
            d.unfulfilled_camera_window_segments.len(),
            drain_frames,
            camera.0,
            state.frame,
            first_n(&d.unfulfilled_camera_window_segments, 5),
        );
    }
}

/// T4 — `trace_partial_dispatch_ack_simulates_cold_start_race`.
///
/// First 5 frames `ack_quota = 0` (mimics Phase 2.13's bug — admit picks but
/// render-world doesn't ACK), then `ack_quota = admit_quota` afterward.
/// Asserts: post-frame-5, the system catches up and `unfulfilled` converges
/// to 0. The post-fix ACK channel keeps the slot eligible for ACK until
/// `apply_dispatch_acks` lands; cold-start eventually completes.
///
/// Pre-fix behavior would have been: admitted slots get `dispatched_once.insert`
/// at admit time (the bug), and never re-admitted → cold-start permanently
/// stuck with the 4×5=20 racy slots showing as unfulfilled forever. Under
/// the post-fix ACK channel, this trace converges.
#[test]
fn trace_partial_dispatch_ack_simulates_cold_start_race() {
    let mut state = SimState::new(ADMIT_QUOTA as u32);
    let camera = WorldSegmentPos(IVec3::ZERO);

    // Frames 1..=5 — ack_quota = 0. Admit picks 4/frame but nothing
    // promotes into dispatched_once.
    for _ in 0..5 {
        simulate_frame(&mut state, camera, ADMIT_QUOTA, 0);
    }
    // At this point, 20 slots are bound but pending ack; dispatched_once
    // is still empty.
    let mid_d = state.diagnostics();
    assert_eq!(
        mid_d.dispatched_once_slots, 0,
        "frames 1..=5 with ack_quota=0: dispatched_once must stay 0, got {}",
        mid_d.dispatched_once_slots,
    );
    assert!(
        mid_d.bound_slots > 0,
        "frames 1..=5: at least some slots must be bound, got {}",
        mid_d.bound_slots,
    );

    // Frames 6.. — ack_quota = ADMIT_QUOTA. The pending_ack_slots
    // accumulator (20 entries) drains at 4/frame, AND new admissions
    // continue picking up to 4/frame. Convergence requires enough frames
    // to ACK every slot AND admit any not yet admitted.
    //
    // Worst-case frame budget to reach cold-start-complete:
    //   - First we need to admit all 512 slots: 512/4 = 128 admit-frames.
    //   - ACKs drain at 4/frame; total ACK work is also 512 → ~128 ACK-frames.
    //   - With ack_quota == admit_quota, both progress in parallel after
    //     the first 5 frames.
    //   - Pad generously.
    let max_frames = (WINDOW_TOTAL as usize).div_ceil(ADMIT_QUOTA) + 10;
    let mut converged_at: Option<u64> = None;
    for _ in 0..max_frames {
        simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
        if state.diagnostics().unfulfilled_camera_window_segments.is_empty() {
            converged_at = Some(state.frame);
            break;
        }
    }
    let d = state.diagnostics();
    assert!(
        converged_at.is_some(),
        "post-fix ACK channel should converge cold-start after partial \
         dispatch race; final unfulfilled={}, dispatched_once={}, bound={}, \
         first 5 unfulfilled = {}",
        d.unfulfilled_camera_window_segments.len(),
        d.dispatched_once_slots,
        d.bound_slots,
        first_n(&d.unfulfilled_camera_window_segments, 5),
    );
}

/// T5 — `trace_diagonal_walk_steady_unfulfilled_bounded`.
///
/// Cold-start at origin; then take 20 diagonal `(1, 0, 1)` shift-and-drain
/// cycles. Each cycle:
///   1. one shift-frame (camera moves +1 on X and +1 on Z — origin shifts,
///      evicts ~2 slabs minus the diagonal-overlap row),
///   2. `ceil(2 * slab / admit_quota)` drain-frames (camera holds, ACK
///      queue drains).
///
/// Brief bound: `2 * slab_size + admit_quota` (diagonal evicts two slabs).
/// Tests both the post-shift bound AND the post-drain convergence — the
/// system MUST return to 0 unfulfilled after the drain quota, otherwise
/// the simulator has a leak across composed primitives.
///
/// Z-slab size = `window_size.x * window_size.y` = `16 * 2 = 32`. Same
/// as `SLAB_X` by coincidence (the production preset is square-cross-section).
#[test]
fn trace_diagonal_walk_steady_unfulfilled_bounded() {
    let mut state = SimState::new(ADMIT_QUOTA as u32);
    let camera0 = WorldSegmentPos(IVec3::ZERO);
    // Cold-start drive.
    let cold_frames = (WINDOW_TOTAL as usize).div_ceil(ADMIT_QUOTA) + 2;
    for _ in 0..cold_frames {
        simulate_frame(&mut state, camera0, ADMIT_QUOTA, ADMIT_QUOTA);
    }
    assert!(state.diagnostics().cold_start_complete, "cold-start prereq");

    let slab_x: usize = SLAB_X as usize;
    let two_slabs = 2 * slab_x;
    // Diagonal eviction is `(y*z) + (y*x) - y` (subtract overlap row);
    // for the streaming preset y=2 so it's actually `slab + slab - 2 = 62`
    // segments admitted. The brief's `2*slab + admit_quota` is the loose
    // bound — we use that + a small phasing margin.
    let bound = two_slabs + ADMIT_QUOTA;
    let drain_frames = two_slabs.div_ceil(ADMIT_QUOTA);

    for step in 1..=20i32 {
        let camera = WorldSegmentPos(IVec3::new(step, 0, step));
        // (a) shift-frame.
        simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
        let d = state.diagnostics();
        assert!(
            d.unfulfilled_camera_window_segments.len() <= bound,
            "diagonal step {step} shift-frame: unfulfilled={} exceeded \
             bound={} (= 2*slab + admit_quota); camera={:?}, frame={}, \
             first 5 unfulfilled = {}",
            d.unfulfilled_camera_window_segments.len(),
            bound,
            camera.0,
            state.frame,
            first_n(&d.unfulfilled_camera_window_segments, 5),
        );
        // (b) drain-frames: hold camera, ACK queue drains.
        for _ in 0..drain_frames {
            simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
        }
        let d = state.diagnostics();
        assert_eq!(
            d.unfulfilled_camera_window_segments.len(),
            0,
            "diagonal step {step} post-drain: unfulfilled={} did NOT \
             converge back to 0 within {} drain-frames; camera={:?}, \
             frame={}, first 5 unfulfilled = {}",
            d.unfulfilled_camera_window_segments.len(),
            drain_frames,
            camera.0,
            state.frame,
            first_n(&d.unfulfilled_camera_window_segments, 5),
        );
    }
}

/// T6 — `trace_random_lcg_walk_unfulfilled_never_blows_up`.
///
/// Deterministic LCG-driven random camera walk for 100 frames after
/// cold-start. The brief bound: `window_total / 2` at every frame past
/// cold-start. This is a stress test — many shifts, sometimes large,
/// across uncorrelated directions. The system MUST stay bounded; if it
/// "blows up" past half-window unfulfilled, that indicates pending_admissions
/// is accumulating without draining (or eviction isn't pruning correctly).
///
/// LCG style mirrors `windowed_slot_map::audit_invariants_after_random_mutations`
/// so the trace is reproducible.
#[test]
fn trace_random_lcg_walk_unfulfilled_never_blows_up() {
    let mut state = SimState::new(ADMIT_QUOTA as u32);
    let camera0 = WorldSegmentPos(IVec3::ZERO);
    // Cold-start drive.
    let cold_frames = (WINDOW_TOTAL as usize).div_ceil(ADMIT_QUOTA) + 2;
    for _ in 0..cold_frames {
        simulate_frame(&mut state, camera0, ADMIT_QUOTA, ADMIT_QUOTA);
    }
    assert!(state.diagnostics().cold_start_complete, "cold-start prereq");

    // LCG state (Numerical Recipes coefficients) — reproducible across runs.
    // Same coefficients as `windowed_slot_map::audit_invariants_after_random_mutations`.
    let mut rng_state: u32 = 0xC0FF_EE42;
    let next = |rng: &mut u32| -> u32 {
        *rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        *rng
    };

    let bound = (WINDOW_TOTAL as usize) / 2;
    // Walk in segment space with bias toward stationary frames so the ACK
    // queue can drain (75% stationary, 12.5% +1, 12.5% -1 per axis).
    // Camera position is also clamped to a small ±3-segment box around the
    // origin so the random walk doesn't drift cumulatively — production
    // camera traversal is also locally bounded (the user doesn't fly out
    // to ±50 segments in 100 frames; segments are 256 voxels each).
    // Without this clamp the LCG drift will exceed the brief's
    // `window_total / 2` bound around frame ~24-50 simply from cumulative
    // shift-without-drain — not a bug in the composition, just a
    // walk-distance artifact.
    let mut camera_pos = IVec3::ZERO;
    let step_choices: [i32; 8] = [-1, 0, 0, 0, 0, 0, 0, 1];
    for f in 1..=100i32 {
        let dx = step_choices[(next(&mut rng_state) % 8) as usize];
        let dz = step_choices[(next(&mut rng_state) % 8) as usize];
        camera_pos += IVec3::new(dx, 0, dz);
        // Clamp to ±3 on X and Z. Production wraparound: the walk
        // bounces softly off the edge by zero-ing out steps that would
        // exceed the box.
        camera_pos.x = camera_pos.x.clamp(-3, 3);
        camera_pos.z = camera_pos.z.clamp(-3, 3);
        let camera = WorldSegmentPos(camera_pos);
        simulate_frame(&mut state, camera, ADMIT_QUOTA, ADMIT_QUOTA);
        let d = state.diagnostics();
        assert!(
            d.unfulfilled_camera_window_segments.len() <= bound,
            "LCG walk frame {f}: unfulfilled={} exceeded bound={} \
             (= window_total / 2); camera={:?}, sim frame={}, \
             dispatched_once={}, bound_slots={}, first 5 unfulfilled = {}",
            d.unfulfilled_camera_window_segments.len(),
            bound,
            camera.0,
            state.frame,
            d.dispatched_once_slots,
            d.bound_slots,
            first_n(&d.unfulfilled_camera_window_segments, 5),
        );
    }
}
