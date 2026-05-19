//! `streaming::residency` — sliding-window residency manager
//! (`docs/orchestrate/streaming-world/02-design.md` §§ A.1–A.5, carried over
//! into `02b-design-plan-b.md` § D, REWORKED in Phase 2.6 per
//! `02c-design-windowed-slot-map.md`).
//!
//! Owns:
//! - The [`WindowedSlotMap`](super::WindowedSlotMap) primitive (slot pool +
//!   bidirectional world↔slot mapping + GPU-uploaded window indirection
//!   table). Per Phase 2.6 (`02c` § A) this replaces the previous
//!   `slot_to_world` / `world_to_slot` / `slot_state` triple.
//! - Per-frame `admissions` / `evictions` deltas the render-world dispatcher
//!   consumes via `ExtractResource`.
//! - The `residency_driver` system that detects camera-segment boundary
//!   crossings, computes the target resident set, and produces the admission /
//!   eviction lists.
//!
//! Q1 (residency-only `i32` widening) is enforced here: world-segment positions
//! are stored as `IVec3` (a `WorldSegmentPos` newtype); slot indices are u32.
//! The renderer never sees the world `IVec3` — only the indirection table
//! flat indices `pack(local_xyz)`.
//!
//! ## Slot lifecycle is now IMPLICIT (Phase 2.6 D4)
//!
//! - In `window.free_list` ⟺ Empty.
//! - In `window.iter_bound()` AND in `admissions_this_frame` ⟺ Generating.
//! - In `window.iter_bound()` AND NOT in `admissions_this_frame` ⟺ Resident.
//!
//! Phase 2.5's explicit `SlotState` enum + the `Last`-stage
//! `finalise_admissions_as_resident` system are GONE. The transition from
//! Generating→Resident happens implicitly when `residency_driver` clears
//! `admissions_this_frame` at the next `PreUpdate` entry.

use std::collections::HashSet;

use bevy::prelude::*;

use crate::{WORLD_GEN_SEGMENT_SIZE_IN_GROUPS, WORLD_SIZE_IN_SEGMENTS};

use super::windowed_slot_map::WindowedSlotMap;

/// `SEGMENT_CHUNKS = 16` per `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4`. Mirrors
/// the constant the W5 driver loop derives at `mod.rs:2423`.
pub const SEGMENT_CHUNKS: u32 = WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;

/// `SEGMENT_VOXELS = 256` — voxels per segment per axis.
pub const SEGMENT_VOXELS: i32 = (SEGMENT_CHUNKS as i32) * 16;

/// World-segment coordinate (in segments, NOT chunks NOR voxels).
/// Newtype because `IVec3` flies around the codebase in chunk/voxel space and
/// the unit confusion would be lethal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldSegmentPos(pub IVec3);

/// Window-local slot index, `[0, total_slots) = [0, 512)` for the fixed
/// `(16, 2, 16)` window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotIndex(pub u32);

/// Main-world residency manager `Resource` (Phase 2.6 — `02c` § G.3).
///
/// Phase 2.6 collapses the previous (slot_to_world, world_to_slot, slot_state)
/// triple into one [`WindowedSlotMap`]. The renderer-side GPU upload (the
/// indirection buffer at `@group(0) @binding(8)`) comes from
/// `window.indirection_buffer()`.
#[derive(Resource, Clone)]
pub struct Residency {
    /// Phase 2.6 — the closed-API slot pool + bidirectional mapping + GPU
    /// indirection table. Replaces the previous `slot_to_world` / `world_to_slot`
    /// / `slot_state` triple. See `windowed_slot_map.rs`.
    pub window: WindowedSlotMap,
    /// Per-frame admissions (world-segment pos, slot it was assigned to) —
    /// drained by the render-world dispatch each frame.
    pub admissions_this_frame: Vec<(WorldSegmentPos, SlotIndex)>,
    /// Per-frame evictions (the slot that just became Empty). The eviction
    /// path doesn't currently re-issue W2 records — Phase 2 evictions just
    /// mark the slot Empty and the per-frame bounds chain refresh accounts for
    /// the change.
    pub evictions_this_frame: Vec<SlotIndex>,
    /// Max admissions per frame (CLI: `--max-segments-per-frame`, default 4).
    pub max_segments_per_frame: u32,
    /// Frame counter for budget bookkeeping (mostly diagnostic).
    pub frame_counter: u64,
    /// Last camera segment seen — used to detect "did the camera cross a
    /// boundary?" without unnecessary recomputation.
    pub last_camera_seg: Option<IVec3>,
    /// Phase 2.6 replacement for the old `Vec<SlotState>` enum. Tracks
    /// slot indices that have been pushed to `admissions_this_frame` at
    /// least once since they were last bound. Cleared on eviction (the
    /// `set_origin` callback that returns evicted slots to the pool also
    /// strips them from this set). A slot in this set is implicitly
    /// "Resident" (already dispatched); a bound slot NOT in this set is
    /// "Generating" (queued for dispatch).
    pub dispatched_once: HashSet<SlotIndex>,
    /// streaming-world Phase 2.12
    /// (`docs/orchestrate/streaming-world/02e-design-phase-2-12.md` § B,
    /// MUST-1) — slots whose binding changed and whose `chunks_buffer`
    /// region must be cleared by the render-world `clear_streaming_bound_slots`
    /// system before any reader (renderer, W3 chain, per-admission producer)
    /// consumes them.
    ///
    /// **Sticky semantics (Phase 2.12 fix iteration)**: this queue does NOT
    /// auto-clear at the start of each frame. It is APPENDED to by
    /// `WindowedSlotMap::bind()` calls in `residency_driver` Pass 3, and
    /// only DRAINED when the render-world's clear system actually issues
    /// the GPU clears. This avoids a race where Frame 0's residency pushes
    /// 512 slots onto the queue but `WorldGpu` isn't yet allocated by
    /// `prepare_world_gpu` (an asynchronous build-once system that may
    /// take 1-3 frames to land); a per-frame auto-clear of the queue would
    /// silently drop those 512 entries.
    ///
    /// The render world drains it via the `take_clear_on_bind_slots()`
    /// helper exposed on `Residency` — the extract reads the queue + clears
    /// it atomically in the main world. Outcome: when origin shifts and N
    /// slots get rebound, the indirection points NEW window-local positions
    /// at slots whose `chunks_buffer` data is freshly zero (= UNIFORM_EMPTY
    /// = sky) instead of the previous segment's data (= ghost-of-old-
    /// terrain). Per-admission encoders then fill them with real data over
    /// the next ~8 frames at 4/frame.
    pub clear_on_bind_queue: Vec<SlotIndex>,
}

impl Residency {
    /// Build a freshly-empty residency table with `total_slots = wx*wy*wz`
    /// entries. The window is camera-centred at install time (the first
    /// `residency_driver` tick produces the initial admissions list).
    pub fn empty(max_segments_per_frame: u32) -> Self {
        let window_size = UVec3::new(
            WORLD_SIZE_IN_SEGMENTS.x,
            WORLD_SIZE_IN_SEGMENTS.y,
            WORLD_SIZE_IN_SEGMENTS.z,
        );
        Self {
            window: WindowedSlotMap::new(window_size),
            admissions_this_frame: Vec::new(),
            evictions_this_frame: Vec::new(),
            max_segments_per_frame,
            frame_counter: 0,
            last_camera_seg: None,
            dispatched_once: HashSet::new(),
            clear_on_bind_queue: Vec::new(),
        }
    }

    /// Window total slot count.
    pub fn total_slots() -> u32 {
        WORLD_SIZE_IN_SEGMENTS.x * WORLD_SIZE_IN_SEGMENTS.y * WORLD_SIZE_IN_SEGMENTS.z
    }

    /// Compute slot index from window-local segment coordinates. Kept as a
    /// static helper because the Phase 2.5+ tests pin its formula (and the
    /// `WindowedSlotMap::pack` test cross-checks against it).
    pub fn slot_index_of(local_xyz: [u32; 3]) -> u32 {
        let [lx, ly, lz] = local_xyz;
        lx + ly * WORLD_SIZE_IN_SEGMENTS.x
            + lz * WORLD_SIZE_IN_SEGMENTS.x * WORLD_SIZE_IN_SEGMENTS.y
    }

    /// Decompose slot index into window-local segment coordinates.
    /// Phase 2.6: this is now only meaningful for `chunk_offset` math in the
    /// streaming dispatch (`slot.0` → window-local `(lx, ly, lz)` → chunk
    /// offset into the slot-indexed `chunks_buffer`).
    pub fn local_of(slot: u32) -> [u32; 3] {
        let wx = WORLD_SIZE_IN_SEGMENTS.x;
        let wy = WORLD_SIZE_IN_SEGMENTS.y;
        let lx = slot % wx;
        let ly = (slot / wx) % wy;
        let lz = slot / (wx * wy);
        [lx, ly, lz]
    }

    /// World-origin offset in segments. Convenience getter that forwards to
    /// `self.window.origin()`.
    pub fn origin(&self) -> IVec3 {
        self.window.origin()
    }

    /// True when the world-segment falls inside the current window.
    pub fn is_in_window(&self, s: WorldSegmentPos) -> bool {
        self.window.is_in_window(s)
    }

    /// Resolve a world-segment to a slot index, if resident.
    pub fn slot_of(&self, s: WorldSegmentPos) -> Option<SlotIndex> {
        self.window.lookup_slot(s)
    }

    /// streaming-world Phase 2.11
    /// (`docs/orchestrate/streaming-world/03n-diagnosis-aadf-building.md` punch-list
    /// item 1) — `true` once the cold-start admission burst has fully drained
    /// (every slot in the window has been admitted at least once).
    ///
    /// Used by the render-world to gate the W3 regime-1 seed dispatch.
    /// Pre-Phase-2.11 the seed fired on the first admission frame —
    /// at which point only 4 of 512 segments had real data and the W3 chain
    /// baked stale long-skip AADFs through 508 yet-to-be-admitted zero-chunks.
    /// Gating the seed on this method delays it until ALL 512 slots are
    /// chunk_calc-current, so the W3 chain's first expansion pass reads
    /// only real data.
    ///
    /// Implementation: returns `dispatched_once.len() == total_slots()`. The
    /// `dispatched_once` set is populated by `process_pending_admissions`
    /// (one slot per admission) and cleared per-slot on eviction
    /// (`set_origin` → `dispatched_once.remove`). At steady-state after
    /// cold-start, every bound slot is in `dispatched_once`, so this returns
    /// `true`. During a steady-state boundary crossing, evicted slots are
    /// removed from `dispatched_once`, this drops to `false` until the new
    /// admissions all dispatch, then climbs back to `true`.
    pub fn is_cold_start_complete(&self) -> bool {
        (self.dispatched_once.len() as u32) == Self::total_slots()
    }
}

/// Convert a world-voxel `IVec3` position to a `WorldSegmentPos` via
/// `pos_int.div_euclid(SEGMENT_VOXELS)`. Negative coords floor toward `-inf`
/// (which is what we want — a segment at world-origin (0,0,0) covers voxels
/// `[0, SEGMENT_VOXELS)`).
pub fn world_voxel_to_segment(world_voxel: IVec3) -> WorldSegmentPos {
    WorldSegmentPos(IVec3::new(
        world_voxel.x.div_euclid(SEGMENT_VOXELS),
        world_voxel.y.div_euclid(SEGMENT_VOXELS),
        world_voxel.z.div_euclid(SEGMENT_VOXELS),
    ))
}

/// `WorldSegmentPos → world-voxel origin (IVec3)`. The (0,0,0) voxel of the
/// segment.
pub fn segment_to_voxel_origin(s: WorldSegmentPos) -> IVec3 {
    s.0 * SEGMENT_VOXELS
}

/// Compute the residency origin that places the camera segment at the centre
/// of the X/Z window. Y is FIXED at 0 — the streaming preset uses the bottom
/// row of the 2-tall window for ground content (`world_y ∈ [0, 256)`) and the
/// top row for above-ground (`world_y ∈ [256, 512)`). The camera Y is
/// otherwise unconstrained — the window covers the full world-Y extent. Per
/// `02-design.md` § A.3 ("Y is full-height — both Y segments always
/// resident"). Translation: origin.y is ALWAYS 0; the camera's world-Y
/// position is the full world Y range (no sliding in Y).
pub fn target_origin_for_camera_seg(cam_seg: IVec3) -> IVec3 {
    let half_x = (WORLD_SIZE_IN_SEGMENTS.x as i32) / 2;
    let half_z = (WORLD_SIZE_IN_SEGMENTS.z as i32) / 2;
    let _ = cam_seg.y; // intentionally unused — Y origin pinned at 0.
    IVec3::new(cam_seg.x - half_x, 0, cam_seg.z - half_z)
}

/// VRAM budget pre-flight check per `02-design.md` § A.4. Panics with a clear
/// diagnostic when the configured budget is below the slab requirement.
pub fn assert_vram_budget_sufficient(vram_budget_mib: u32) {
    let required_mib = compute_slab_total_mib();
    if (vram_budget_mib as u64) < required_mib {
        panic!(
            "streaming-world VRAM budget pre-flight FAILED: configured \
             --vram-budget-mib {} MiB is below the required slab total {} MiB. \
             Slab covers: segment_voxel_buffer (~128 MiB) + WorldGpu.chunks_buffer \
             (~64 MiB) + WorldGpu.blocks + WorldGpu.voxels + hash_map + bound_* \
             buffers. Bump the budget to at least {} MiB, or reduce \
             WORLD_SIZE_IN_SEGMENTS (not supported — drift-guard test at \
             lib.rs:920-946 pins the constants).",
            vram_budget_mib, required_mib, required_mib,
        );
    }
}

/// Compute slab total in MiB. Conservative — covers `segment_voxel_buffer +
/// chunks_buffer + blocks/voxels worst-case + hash_map + bounds queues`.
pub fn compute_slab_total_mib() -> u64 {
    let segment_voxel_mib = (SEGMENT_CHUNKS as u64).pow(3) * 2048 * 4 / (1024 * 1024);
    let world_chunks = (crate::WORLD_SIZE_IN_CHUNKS.x as u64)
        * (crate::WORLD_SIZE_IN_CHUNKS.y as u64)
        * (crate::WORLD_SIZE_IN_CHUNKS.z as u64);
    let chunks_buffer_mib = world_chunks * 2 * 4 / (1024 * 1024);
    let blocks_mib = 256;
    let voxels_mib = 256;
    let hash_map_mib = 16;
    let bounds_mib = 24;
    let misc_mib = 4;

    segment_voxel_mib + chunks_buffer_mib + blocks_mib + voxels_mib + hash_map_mib
        + bounds_mib + misc_mib
}

/// Camera segment for the current frame, in `WorldSegmentPos` units.
///
/// The camera Transform / `PositionSplit::pos_int` is **window-local** (Phase
/// 2.5 — `pin_streaming_window_camera` pre-translates the world Transform by
/// `-origin * SEGMENT_VOXELS` each frame so the renderer reads correct
/// indirection slots). To recover the absolute world voxel coord — the
/// quantity that determines which world-segment the camera is in — we add the
/// current `origin * SEGMENT_VOXELS` back.
fn camera_segment_pos(camera_pos_int: IVec3, residency_origin: IVec3) -> WorldSegmentPos {
    let world_voxel = camera_pos_int + residency_origin * SEGMENT_VOXELS;
    world_voxel_to_segment(world_voxel)
}

/// Phase 2.9 (`03j-diagnosis-camera-nudge-loop.md`) — camera segment derived
/// directly from the production-side `CameraAbsolutePosition` resource. The
/// preferred path: bypasses the window-local→absolute round-trip that
/// `camera_segment_pos` does, so a hypothetical out-of-sync Transform (e.g.
/// during the same frame `track_and_pin_camera` has yet to re-pin) cannot
/// drive the driver into an endless reposition loop.
fn camera_segment_pos_from_abs(abs_pos_int: IVec3) -> WorldSegmentPos {
    world_voxel_to_segment(abs_pos_int)
}

/// `PreUpdate` system — detect camera-segment crossings, recompute the target
/// resident set, populate `admissions_this_frame` + `evictions_this_frame`,
/// honour the `--max-segments-per-frame` budget.
///
/// Phase 2.6: drives `WindowedSlotMap` via its closed API
/// (`allocate` / `bind` / `unbind` / `set_origin`). Slot-position is now
/// pool-driven, NOT geometric — the renderer-side indirection table
/// (uploaded at `@binding(8)`) translates window-local chunk coords through
/// slot indices to the correct positions in `chunks_buffer`.
///
/// `residency` is `Option<ResMut>` because the resource is only present when
/// `GridPreset::ProceduralStreaming` was installed at startup. Non-streaming
/// presets see `None` and early-return.
pub fn residency_driver(
    residency: Option<ResMut<Residency>>,
    camera: Option<
        Single<&crate::camera::position_split::PositionSplit, With<bevy::prelude::Camera3d>>,
    >,
    abs_pos: Option<Res<super::CameraAbsolutePosition>>,
) {
    let Some(mut residency) = residency else {
        return;
    };
    residency.frame_counter = residency.frame_counter.wrapping_add(1);
    residency.admissions_this_frame.clear();
    residency.evictions_this_frame.clear();
    // streaming-world Phase 2.12 (`02e-design-phase-2-12.md` § B,
    // MUST-1) — `clear_on_bind_queue` is STICKY across frames. Do NOT
    // auto-clear it here: the render-world `clear_streaming_bound_slots`
    // system drains it via the extract path when it actually issues GPU
    // clears (gated on `WorldGpu` being available; `WorldGpu` is
    // allocated by `prepare_world_gpu`, an asynchronous build-once system
    // that may take 1-3 frames). A per-frame auto-clear here would race
    // and silently drop the initial 512 cold-start binds.

    // Phase 2.9 fix — prefer the production-side absolute position tracker
    // when available (the streaming preset's main camera path). Falls back
    // to the window-local→absolute round-trip on `PositionSplit` only when
    // the resource is absent (e2e gate before Phase 2.9 refactor, or any
    // future entry point that bypasses the production camera install).
    let cam_seg_world = if let Some(abs) = abs_pos.as_deref() {
        camera_segment_pos_from_abs(abs.pos_int)
    } else {
        let Some(camera) = camera else {
            // No camera yet — the very first frame of the e2e harness can hit
            // this. Defer until camera exists.
            return;
        };
        camera_segment_pos(camera.pos_int, residency.origin())
    };

    // First-tick init: place the origin so the camera is centered.
    let do_shift = match residency.last_camera_seg {
        None => true,
        Some(prev) => prev != cam_seg_world.0,
    };

    if !do_shift {
        // No segment change — but we may still have pending admissions from
        // the cold-start phase. Process up to `max_segments_per_frame` of them.
        process_pending_admissions(&mut residency);
        return;
    }

    let new_origin = target_origin_for_camera_seg(cam_seg_world.0);
    residency.last_camera_seg = Some(cam_seg_world.0);

    // Pass 1 — shift the origin and evict any pairs that fall outside the
    // new window. Phase 2.14.b — the atomic `set_origin` API fires a
    // per-eviction callback while each slot is still tracked; the slot
    // is internally pushed back to the free pool after the callback
    // returns. The previous two-step `set_origin → free` pattern is
    // gone (it left an in-flight state that violated I2).
    //
    // Split-borrow `residency` so the closure captures
    // `evictions_this_frame` + `dispatched_once` independently from
    // `window` (Rust allows disjoint-field borrows when the fields are
    // named directly).
    let Residency {
        window,
        evictions_this_frame,
        dispatched_once,
        ..
    } = &mut *residency;
    window.set_origin(new_origin, |_w, slot| {
        // Record the eviction for the per-frame delta the renderer
        // consumes via `ExtractResource`.
        evictions_this_frame.push(slot);
        // Drop the dispatched-once marker so the slot can be re-dispatched
        // after a future allocate_and_bind.
        dispatched_once.remove(&slot);
    });

    // Pass 2 — figure out which target segments are not yet resident; queue
    // them for admission (camera-distance first per D.11).
    let cam_seg = cam_seg_world.0;
    // Collect bound segments into a HashSet for the `not contains` filter.
    let resident: std::collections::HashSet<WorldSegmentPos> = residency
        .window
        .iter_bound()
        .map(|(w, _)| w)
        .collect();
    // Build the target set: every world-segment whose local coord is in
    // `[0, w)` for each axis.
    let mut pending: Vec<WorldSegmentPos> =
        Vec::with_capacity(Residency::total_slots() as usize);
    for lz in 0..WORLD_SIZE_IN_SEGMENTS.z {
        for ly in 0..WORLD_SIZE_IN_SEGMENTS.y {
            for lx in 0..WORLD_SIZE_IN_SEGMENTS.x {
                let world_seg = WorldSegmentPos(IVec3::new(
                    new_origin.x + lx as i32,
                    new_origin.y + ly as i32,
                    new_origin.z + lz as i32,
                ));
                if !resident.contains(&world_seg) {
                    pending.push(world_seg);
                }
            }
        }
    }
    pending.sort_by_key(|w| {
        let d = w.0 - cam_seg;
        d.x * d.x + d.y * d.y + d.z * d.z
    });

    // Pass 3 — bind pending admissions to freshly-allocated slots.
    //
    // Phase 2.6 (`02c` § F): the indirection table makes slot assignment
    // pool-driven; `window.bind(w, slot)` writes `indirection[pack(local)] =
    // slot.0`, so the renderer reads via the indirection table regardless of
    // which slot index `allocate()` chose. Slot identity is preserved across
    // origin shifts — no GPU memcpy on eviction.
    //
    // Phase 2.14.b — atomic API. `allocate_and_bind(w)` collapses the
    // previous two-step `allocate() + bind(w, slot)` (which left a
    // transient in-flight state between the two calls) into one atomic
    // op. Returns `None` only when the pool is empty (the other two
    // None-conditions — out-of-window, already-bound — are filtered out
    // upstream by the `is_in_window` window bounds + the `resident` set
    // diff at Pass 2).
    for w in pending {
        let Some(slot) = residency.window.allocate_and_bind(w) else {
            // No empty slots left this frame — leave the rest for the next
            // tick (they'll re-enter `pending` because the target set still
            // includes them).
            break;
        };
        // streaming-world Phase 2.12 (`02e-design-phase-2-12.md` § B,
        // MUST-1) — record this bind in the clear-on-bind queue so the
        // render world zeroes the slot's `chunks_buffer` region BEFORE
        // any renderer/producer consumes it. This forecloses the
        // "indirection points at slot whose chunks_buffer still holds
        // the previously-evicted segment's data" race (the ghost-of-
        // old-terrain bug from `03p-diagnosis-remaining-bugs.md` § Bug 1).
        residency.clear_on_bind_queue.push(slot);
        // Don't push to admissions_this_frame yet — the budget gate below
        // caps it. Bound slots that aren't in admissions_this_frame are
        // implicitly Generating (per D4).
    }

    // Pass 4 — budgeted admissions list — pick up to `max_segments_per_frame`
    // bound-but-undispatched slots in camera-distance order.
    process_pending_admissions(&mut residency);

    bevy::log::info!(
        "streaming-world residency shift: cam_seg={:?}, new_origin={:?}, \
         evictions={}, bound_segments={}, admissions_this_frame={}",
        cam_seg_world.0,
        new_origin,
        residency.evictions_this_frame.len(),
        residency.window.iter_bound().count(),
        residency.admissions_this_frame.len(),
    );
}

/// Pick up to `max_segments_per_frame` Generating slots in camera-distance
/// order and place them into `admissions_this_frame`.
///
/// Phase 2.6 — a slot is implicitly "Generating" iff it is bound AND has been
/// admitted (i.e. produced) at least once in a prior tick whose admission
/// already extracted/dispatched. For Phase 2.6 we treat ALL bound-but-not-yet-
/// dispatched slots as candidates. To avoid double-dispatch (the bug Phase
/// 2.5's slot-state enum was protecting against), we track a per-Residency
/// `dispatched_slots` HashSet — slots are added when the producer dispatches
/// them on a given tick. Phase 2.6's implementation captures this implicitly
/// via [`Self::admissions_this_frame`]: each tick clears it; each tick adds
/// the next budgeted batch; the producer-node consumes the list each tick.
///
/// To support Phase 2.5's "drain pending Generating over multiple ticks"
/// behaviour, we need a parallel "already-dispatched-once" marker. Phase
/// 2.6 implements this as a `dispatched_once_marker: HashSet<SlotIndex>`
/// — set on each tick's `admissions_this_frame.push()`, never cleared
/// during streaming. (Cleared only on eviction — see `set_origin`'s
/// callback chain.)
fn process_pending_admissions(residency: &mut Residency) {
    let cap = residency.max_segments_per_frame as usize;
    if cap == 0 {
        return;
    }
    let cam_seg = residency.last_camera_seg.unwrap_or(IVec3::ZERO);

    // Build candidate list: bound segments whose slot has NOT yet been
    // dispatched (i.e. not yet appeared in admissions_this_frame on a
    // prior tick).
    //
    // We track "dispatched-once" implicitly via the
    // `Residency::dispatched_once` set, plumbed through below.
    let mut candidates: Vec<(SlotIndex, WorldSegmentPos, i32)> = residency
        .window
        .iter_bound()
        .filter(|(_, slot)| !residency.dispatched_once.contains(slot))
        .map(|(w, slot)| {
            let d = w.0 - cam_seg;
            let dsq = d.x * d.x + d.y * d.y + d.z * d.z;
            (slot, w, dsq)
        })
        .collect();
    candidates.sort_by_key(|c| c.2);

    for (slot, world, _dsq) in candidates.into_iter().take(cap) {
        residency.admissions_this_frame.push((world, slot));
        // streaming-world Phase 2.13
        // (`docs/orchestrate/streaming-world/03r-diagnosis-cold-start-gap.md`
        // MUST-1) — the previous `residency.dispatched_once.insert(slot)`
        // call lived here. It was REMOVED because it fired BEFORE the
        // render-world producer node had a chance to run; the producer's
        // 11+ early-return guards (pipelines compiling, `WorldGpu` not
        // yet allocated, bind groups not yet built, …) silently skipped
        // the per-segment dispatch every frame they triggered, while the
        // main-world filter at line 502 excluded the burned slots from
        // re-pick forever (until eviction). Result: 4-24 camera-nearest
        // slots stayed UNIFORM_EMPTY = sky, producing the visible
        // cold-start gap at the camera spawn position (`03r` § Image-8
        // segment identification).
        //
        // Fix: the render-world producer pushes the slot id onto
        // `PENDING_DISPATCHED_ONCE_SLOTS` (`noise_dispatch.rs`) AFTER each
        // successful `render_queue.submit`. The main-world
        // `apply_dispatch_acks` system (PreUpdate, `.before(residency_driver)`)
        // drains that accumulator into `dispatched_once`. A slot now
        // only enters `dispatched_once` after the GPU work for it has
        // been submitted — surviving the Frame-0 race.
    }
}

/// streaming-world Phase 2.13
/// (`docs/orchestrate/streaming-world/03r-diagnosis-cold-start-gap.md` MUST-1)
/// — main-world `PreUpdate` system that drains the cross-world
/// `PENDING_DISPATCHED_ONCE_SLOTS` accumulator and marks each ack'd slot
/// Resident in `Residency::dispatched_once`. Runs `.before(residency_driver)`
/// so the next residency tick's filter at
/// [`process_pending_admissions`] sees fresh acks before it picks the
/// frame's admissions.
///
/// Cheap: drains one `Vec<SlotIndex>` per frame (typically 0-4 entries at
/// steady-state; up to ~24 across cold-start). Early-returns when the
/// `Residency` resource is missing (non-streaming presets).
pub fn apply_dispatch_acks(residency: Option<ResMut<Residency>>) {
    let acks: Vec<SlotIndex> = match super::noise_dispatch::PENDING_DISPATCHED_ONCE_SLOTS
        .lock()
    {
        Ok(mut acc) => std::mem::take(&mut *acc),
        Err(_) => return,
    };
    if acks.is_empty() {
        return;
    }
    let Some(mut residency) = residency else {
        return;
    };
    for slot in acks {
        residency.dispatched_once.insert(slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn slot_index_round_trip() {
        for lx in 0..WORLD_SIZE_IN_SEGMENTS.x {
            for ly in 0..WORLD_SIZE_IN_SEGMENTS.y {
                for lz in 0..WORLD_SIZE_IN_SEGMENTS.z {
                    let idx = Residency::slot_index_of([lx, ly, lz]);
                    assert_eq!(Residency::local_of(idx), [lx, ly, lz]);
                }
            }
        }
    }

    #[test]
    fn window_geometry_total_slots() {
        assert_eq!(Residency::total_slots(), 16 * 2 * 16);
    }

    #[test]
    fn world_voxel_to_segment_negative_handles_floor() {
        let s = world_voxel_to_segment(IVec3::new(-1, 0, 0));
        assert_eq!(s.0, IVec3::new(-1, 0, 0));
        let s = world_voxel_to_segment(IVec3::new(0, 0, 0));
        assert_eq!(s.0, IVec3::new(0, 0, 0));
        let s = world_voxel_to_segment(IVec3::new(-SEGMENT_VOXELS, 0, 0));
        assert_eq!(s.0, IVec3::new(-1, 0, 0));
        let s = world_voxel_to_segment(IVec3::new(SEGMENT_VOXELS, 0, 0));
        assert_eq!(s.0, IVec3::new(1, 0, 0));
    }

    #[test]
    fn target_origin_centers_camera_xz() {
        let cam = IVec3::new(50, 0, 50);
        let origin = target_origin_for_camera_seg(cam);
        assert_eq!(cam.x - origin.x, 8);
        assert_eq!(cam.z - origin.z, 8);
        assert_eq!(origin.y, 0, "Y origin must be fixed at 0");
    }

    #[test]
    fn target_origin_y_always_zero() {
        for cam_y in [-5, -1, 0, 1, 2, 5, 100] {
            let cam = IVec3::new(0, cam_y, 0);
            let origin = target_origin_for_camera_seg(cam);
            assert_eq!(origin.y, 0, "Y origin must be 0 regardless of camera Y={cam_y}");
        }
    }

    #[test]
    fn empty_residency_has_all_empty_slots() {
        let r = Residency::empty(4);
        assert_eq!(r.window.capacity(), 512);
        assert_eq!(r.window.free_count(), 512);
        assert!(r.window.iter_bound().next().is_none());
    }

    #[test]
    fn vram_budget_sufficient_passes_at_default() {
        super::assert_vram_budget_sufficient(1024);
    }

    #[test]
    #[should_panic(expected = "VRAM budget pre-flight FAILED")]
    fn vram_budget_panics_below_floor() {
        super::assert_vram_budget_sufficient(0);
    }

    /// Phase 2.9 (`03j-diagnosis-camera-nudge-loop.md`) regression catcher
    /// — under the absolute-position tracker the segment computation must
    /// NOT depend on the residency origin. Past bug: `camera_segment_pos`
    /// added `origin * SEGMENT_VOXELS` to the (already-absolute) Transform
    /// position, double-counting the origin and driving an endless
    /// reposition loop the moment the camera crossed a segment boundary.
    ///
    /// This test simulates the post-shift bug pattern: a camera holding
    /// world position `(2304, 288, 2048)` (one segment past the centre)
    /// with `origin = (1, 0, 0)` (the shift the driver applied last
    /// frame). The correct camera segment is `(9, 1, 8)` (= `2304/256`).
    /// `camera_segment_pos_from_abs` returns this directly; the legacy
    /// `camera_segment_pos` formula would have returned `(10, 1, 8)`
    /// (= `(2304-256)/256 + origin = 8 + 1` only IF `pos_int` is window-
    /// local — but under the bug `pos_int` is absolute, so the formula
    /// over-counts to `(10, 1, 8)` and the cam_seg drifts).
    #[test]
    fn camera_segment_pos_from_abs_is_origin_independent() {
        // Camera at absolute world position 1 segment +X of centre.
        let abs_pos_int = IVec3::new(2304, 288, 2048);
        let cam_seg_abs = camera_segment_pos_from_abs(abs_pos_int);
        assert_eq!(cam_seg_abs.0, IVec3::new(9, 1, 8));

        // Repeat at a shifted origin — the result must NOT change.
        let _origin_shifted = IVec3::new(1, 0, 0);
        // `camera_segment_pos_from_abs` takes only the abs pos — verify it
        // doesn't expose any origin parameter (compile-time test, ish).
        let cam_seg_abs_again = camera_segment_pos_from_abs(abs_pos_int);
        assert_eq!(cam_seg_abs.0, cam_seg_abs_again.0);
    }

    /// Phase 2.9 — under the pre-fix `camera_segment_pos(pos_int, origin)`
    /// formula, an absolute Transform + a shifted origin double-counted
    /// the origin (driving the reposition loop). Pin the bug-and-fix
    /// distinction in the unit suite so a future refactor that
    /// resurrects the formula has a clear regression signal.
    #[test]
    fn legacy_camera_segment_pos_double_counts_under_absolute_pos_int() {
        // Camera at absolute world (2304, 288, 2048), origin shifted to (1,0,0).
        // With pos_int treated as window-local (the C# / e2e contract):
        //   pos_int_local = abs - origin*SEG = (2304 - 256, 288, 2048) = (2048, 288, 2048)
        //   world_voxel = pos_int_local + origin*SEG = (2304, 288, 2048) ✓ correct
        let pos_int_window_local = IVec3::new(2048, 288, 2048);
        let origin = IVec3::new(1, 0, 0);
        let seg = camera_segment_pos(pos_int_window_local, origin);
        assert_eq!(seg.0, IVec3::new(9, 1, 8));

        // The bug: when `pos_int` is ABSOLUTE (no window-local pre-translation),
        // the formula adds `origin*SEG` ON TOP, producing an over-shifted
        // segment that drives the endless reposition loop.
        let pos_int_absolute = IVec3::new(2304, 288, 2048); // not window-local!
        let buggy_seg = camera_segment_pos(pos_int_absolute, origin);
        assert_eq!(
            buggy_seg.0,
            IVec3::new(10, 1, 8),
            "the legacy formula over-counts origin when pos_int is already absolute — \
             Phase 2.9 fix routes through camera_segment_pos_from_abs to bypass this"
        );
    }

    /// streaming-world Phase 2.11
    /// (`03n-diagnosis-aadf-building.md` punch-list item 1) — the
    /// `is_cold_start_complete` predicate gates the W3 regime-1 seed on the
    /// streaming preset. Empty residency reports false; after every slot has
    /// been admitted at least once it reports true; after an eviction that
    /// drops a slot from `dispatched_once`, it reverts to false until the
    /// new admission re-populates the set.
    #[test]
    fn is_cold_start_complete_tracks_full_admission() {
        let mut residency = Residency::empty(4);
        // Empty: 0 of 512 slots admitted.
        assert!(!residency.is_cold_start_complete());

        // Plant + dispatch every slot's worth of admissions.
        for sx in 0..WORLD_SIZE_IN_SEGMENTS.x as i32 {
            for sy in 0..WORLD_SIZE_IN_SEGMENTS.y as i32 {
                for sz in 0..WORLD_SIZE_IN_SEGMENTS.z as i32 {
                    let slot = residency
                        .window
                        .allocate_and_bind(WorldSegmentPos(IVec3::new(sx, sy, sz)))
                        .expect("slot");
                    residency.dispatched_once.insert(slot);
                }
            }
        }
        // Full: 512 slots, all admitted.
        assert!(residency.is_cold_start_complete());

        // Simulate eviction — `set_origin` removes evicted slots from
        // `dispatched_once`. After dropping ANY slot from the set, the
        // cold-start-complete predicate flips back to false.
        let any_slot = *residency.dispatched_once.iter().next().unwrap();
        residency.dispatched_once.remove(&any_slot);
        assert!(!residency.is_cold_start_complete());
    }

    /// Phase 2.6 — migrated regression catcher (Phase 2.5's
    /// `slot_admissions_eventually_drain_to_resident`).
    ///
    /// streaming-world Phase 2.13 update: `process_pending_admissions` no
    /// longer auto-inserts into `dispatched_once`. The drain is now
    /// produced by the render-world ACK accumulator; the unit-test
    /// simulates that by inserting into `dispatched_once` after each
    /// pick (the same effect `apply_dispatch_acks` would have on the
    /// next frame). The original invariant (bound∧!dispatched strictly
    /// decreases tick-over-tick under the ack pipeline) survives.
    #[test]
    fn slot_admissions_eventually_drain_to_resident() {
        let mut residency = Residency::empty(4);
        // Plant 12 bound segments — more than 1 frame's budget so we
        // observe the drain over multiple cycles.
        for slot_i in 0..12u32 {
            let _s = residency
                .window
                .allocate_and_bind(WorldSegmentPos(IVec3::new(slot_i as i32, 0, 0)))
                .expect("slot");
        }
        residency.last_camera_seg = Some(IVec3::ZERO);

        let count_generating = |r: &Residency| -> usize {
            r.window
                .iter_bound()
                .filter(|(_, s)| !r.dispatched_once.contains(s))
                .count()
        };

        let initial = count_generating(&residency);
        assert_eq!(initial, 12);

        let mut prev_count = initial;
        for tick in 0..3 {
            residency.admissions_this_frame.clear();
            residency.evictions_this_frame.clear();
            process_pending_admissions(&mut residency);
            let admitted = residency.admissions_this_frame.len();
            assert_eq!(
                admitted,
                4.min(prev_count),
                "tick {tick}: process_pending_admissions picked {admitted}, \
                 expected min(4, generating={prev_count})",
            );
            // Phase 2.13 — simulate the render-world ack arriving on the
            // SAME tick (the production path runs `apply_dispatch_acks`
            // at `PreUpdate` of the NEXT tick, but for this unit test
            // we collapse the round-trip — the invariant is that an
            // ack'd slot stops being picked, not that the round-trip
            // takes a particular number of ticks).
            for (_, slot) in &residency.admissions_this_frame {
                residency.dispatched_once.insert(*slot);
            }
            let now_count = count_generating(&residency);
            assert!(
                now_count < prev_count,
                "tick {tick}: bound∧!dispatched count did NOT decrease \
                 (was {prev_count}, now {now_count})",
            );
            prev_count = now_count;
        }

        assert_eq!(count_generating(&residency), 0);
        // 12 bound segments, every one dispatched once.
        let mut dispatched: HashSet<u32> = HashSet::new();
        for (_, s) in residency.window.iter_bound() {
            assert!(residency.dispatched_once.contains(&s));
            dispatched.insert(s.0);
        }
        assert_eq!(dispatched.len(), 12);
    }

    /// streaming-world Phase 2.13
    /// (`docs/orchestrate/streaming-world/03r-diagnosis-cold-start-gap.md`
    /// MUST-1) — `process_pending_admissions` no longer marks slots
    /// Resident at pick time. The slot list it picks ends up in
    /// `admissions_this_frame` only; `dispatched_once` stays empty
    /// until the render-world ACK lands.
    ///
    /// Before Phase 2.13 the line `residency.dispatched_once.insert(slot)`
    /// fired alongside the push to `admissions_this_frame`; this regression
    /// catcher asserts that line is GONE.
    #[test]
    fn process_pending_admissions_does_not_mark_dispatched_once() {
        let mut residency = Residency::empty(4);
        for slot_i in 0..4u32 {
            let _s = residency
                .window
                .allocate_and_bind(WorldSegmentPos(IVec3::new(slot_i as i32, 0, 0)))
                .expect("slot");
        }
        residency.last_camera_seg = Some(IVec3::ZERO);
        assert_eq!(residency.dispatched_once.len(), 0);

        process_pending_admissions(&mut residency);
        assert_eq!(
            residency.admissions_this_frame.len(),
            4,
            "expected 4 admissions picked"
        );
        assert_eq!(
            residency.dispatched_once.len(),
            0,
            "Phase 2.13: dispatched_once must NOT be populated until the \
             render-world ACK fires via PENDING_DISPATCHED_ONCE_SLOTS"
        );
    }
}
