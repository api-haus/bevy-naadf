//! `streaming::windowed_slot_map` — Phase 2.6 closed-API primitive that
//! consolidates the slot pool + bidirectional world↔slot mapping + GPU-uploaded
//! window indirection table into ONE data structure with enforced invariants.
//!
//! See `docs/orchestrate/streaming-world/02c-design-windowed-slot-map.md` for
//! the full rationale + § A type spec + § B invariants + § C `set_origin`
//! algorithm + § E shader-side helpers + § F slot-indexed `chunks_buffer`
//! layout.
//!
//! ## Why this exists
//!
//! Phase 2.5's residency-driver Pass 3 assigned world segments to free slot
//! indices in arbitrary order (`empty_slots.pop()`), so slot `N` did NOT
//! geometrically correspond to window-local position `local_of(N)`. The
//! renderer ASSUMES that geometric mapping (since the camera is pre-translated
//! to window-local frame in `pin_streaming_window_camera`), so reads at world
//! camera position `(8, 1, 8)` looked up `chunks_buffer[slot_index_of(8,1,8) =
//! 280]`, which held content for whatever world segment happened to land at
//! slot 280 — not the one the camera was actually inside.
//!
//! `WindowedSlotMap` fixes this by introducing an EXPLICIT renderer-side
//! indirection table (`array<u32, 512>`) that maps `pack(local_xyz) →
//! SlotIndex`. Slot allocation is now pool-driven; the renderer reads
//! `chunks_buffer[indirection[pack(local_xyz)] * CHUNKS_PER_SEGMENT +
//! chunk_offset_within_segment]`. This decouples slot position in the GPU
//! buffer from window-local geometry — `set_origin` shifts keep existing
//! slots in place (just rebuild the indirection table to point to new local
//! positions) with no GPU memcpy.

use std::collections::HashMap;

use bevy::math::{IVec3, UVec3};

use super::residency::{SlotIndex, WorldSegmentPos};

/// Sentinel meaning "no slot occupies this local position".
/// `u32::MAX` is safe — capacity is 512 (16×2×16), far below MAX.
pub const EMPTY_SLOT: u32 = u32::MAX;

/// A fixed-capacity association between resident world segments and GPU slot
/// indices, plus the window-local indirection table the renderer consumes.
///
/// Three concerns, ONE invariant: the indirection table is always consistent
/// with the bindings, enforced by making every mutation go through
/// [`Self::bind`] / [`Self::unbind`] / [`Self::set_origin`].
#[derive(Clone, Debug)]
pub struct WindowedSlotMap {
    // -------- Pool (LIFO over [0, capacity)) ------------------------------
    /// Free-list of slot indices, popped from the back. Seeded in
    /// reverse order at `new()` so `allocate()` returns slot 0 first
    /// (deterministic — every test relies on this).
    free_list: Vec<SlotIndex>,

    // -------- Mapping (bidirectional, host-side) --------------------------
    world_to_slot: HashMap<WorldSegmentPos, SlotIndex>,
    /// Dense reverse lookup. `slot_to_world[slot.0 as usize]` is `Some(w)`
    /// when slot is bound to world-segment `w`, `None` when free.
    slot_to_world: Vec<Option<WorldSegmentPos>>,

    // -------- Window (derived view, GPU-uploaded) -------------------------
    origin: IVec3,
    window_size: UVec3,
    /// Flat row-major-X-fastest table of size `window_size.x * y * z`. Entry
    /// at `pack(local_xyz)` is `slot.0` when a world segment is bound at
    /// that local position, [`EMPTY_SLOT`] otherwise.
    ///
    /// **This is the buffer the GPU uploads each frame.** Layout is fixed
    /// by [`Self::pack`] and is byte-identical between Rust and WGSL.
    indirection: Vec<u32>,
}

impl WindowedSlotMap {
    /// Build an empty map.
    ///
    /// `window_size` defines the indirection table extent and the capacity
    /// (`= window_size.x * y * z`). For the streaming preset this is
    /// `UVec3::new(16, 2, 16) = 512`.
    pub fn new(window_size: UVec3) -> Self {
        let capacity = (window_size.x * window_size.y * window_size.z) as usize;
        let mut free_list = Vec::with_capacity(capacity);
        // Push in REVERSE order so `pop()` returns SlotIndex(0) first.
        // Deterministic ordering matches existing residency_driver test fixtures.
        for i in (0..capacity).rev() {
            free_list.push(SlotIndex(i as u32));
        }
        Self {
            free_list,
            world_to_slot: HashMap::with_capacity(capacity),
            slot_to_world: vec![None; capacity],
            origin: IVec3::ZERO,
            window_size,
            indirection: vec![EMPTY_SLOT; capacity],
        }
    }

    // --- queries (`&self`) -----------------------------------------------

    pub fn capacity(&self) -> u32 {
        self.window_size.x * self.window_size.y * self.window_size.z
    }

    pub fn origin(&self) -> IVec3 {
        self.origin
    }

    pub fn window_size(&self) -> UVec3 {
        self.window_size
    }

    /// True iff `world_seg` falls inside `[origin, origin + window_size_signed)`.
    pub fn is_in_window(&self, world_seg: WorldSegmentPos) -> bool {
        let d = world_seg.0 - self.origin;
        d.x >= 0
            && d.x < self.window_size.x as i32
            && d.y >= 0
            && d.y < self.window_size.y as i32
            && d.z >= 0
            && d.z < self.window_size.z as i32
    }

    /// Window-local position (`world_seg.0 - origin`). Caller must ensure
    /// `is_in_window` — otherwise the result is meaningless / negative.
    pub fn local_of(&self, world_seg: WorldSegmentPos) -> IVec3 {
        world_seg.0 - self.origin
    }

    /// Slot index currently bound to `world_seg`, or `None` if not resident.
    pub fn lookup_slot(&self, world_seg: WorldSegmentPos) -> Option<SlotIndex> {
        self.world_to_slot.get(&world_seg).copied()
    }

    /// World-segment bound to `slot`, or `None` if the slot is free.
    pub fn lookup_world(&self, slot: SlotIndex) -> Option<WorldSegmentPos> {
        self.slot_to_world.get(slot.0 as usize).copied().flatten()
    }

    /// Iterator over every bound (world, slot) pair. Order unspecified.
    pub fn iter_bound(&self) -> impl Iterator<Item = (WorldSegmentPos, SlotIndex)> + '_ {
        self.world_to_slot.iter().map(|(w, s)| (*w, *s))
    }

    /// Slice the GPU uploads. `&[u32]` of length `capacity()`.
    pub fn indirection_buffer(&self) -> &[u32] {
        &self.indirection
    }

    /// Number of free slots remaining (≤ `capacity()`).
    pub fn free_count(&self) -> u32 {
        self.free_list.len() as u32
    }

    // --- pool primitives (`&mut self`) -----------------------------------

    /// Pop a free slot from the pool, returning `None` when the pool is
    /// empty. Does NOT bind — caller follows up with [`Self::bind`].
    pub fn allocate(&mut self) -> Option<SlotIndex> {
        self.free_list.pop()
    }

    /// Return `slot` to the pool. Panics in debug if `slot` is still bound
    /// (i.e. `slot_to_world[slot.0] != None`). Encodes the invariant "free
    /// slots have no mapping".
    pub fn free(&mut self, slot: SlotIndex) {
        debug_assert!(
            (slot.0 as usize) < self.slot_to_world.len(),
            "free({slot:?}) — slot index out of capacity {}",
            self.slot_to_world.len(),
        );
        debug_assert!(
            self.slot_to_world[slot.0 as usize].is_none(),
            "free({slot:?}) — slot is still bound to {:?}; \
             call unbind() first",
            self.slot_to_world[slot.0 as usize],
        );
        self.free_list.push(slot);
        #[cfg(debug_assertions)]
        self.audit_invariants();
    }

    // --- mapping mutators (`&mut self`) ----------------------------------

    /// Associate `world_seg → slot` and `slot → world_seg`. Updates the
    /// indirection table at `pack(local_of(world_seg))` to `slot.0`. Panics
    /// in debug if:
    /// - `world_seg` is outside the current window (`!is_in_window`).
    /// - `world_seg` is already bound to a different slot.
    /// - `slot` is already bound to a different world segment.
    pub fn bind(&mut self, world_seg: WorldSegmentPos, slot: SlotIndex) {
        debug_assert!(
            self.is_in_window(world_seg),
            "bind({world_seg:?}, {slot:?}) — world_seg outside window \
             (origin={:?}, size={:?})",
            self.origin,
            self.window_size,
        );
        debug_assert!(
            (slot.0 as usize) < self.slot_to_world.len(),
            "bind({world_seg:?}, {slot:?}) — slot index out of capacity {}",
            self.slot_to_world.len(),
        );
        // Forbid double-binding the same world segment to a different slot.
        if let Some(existing) = self.world_to_slot.get(&world_seg) {
            debug_assert_eq!(
                *existing, slot,
                "bind({world_seg:?}, {slot:?}) — already bound to {existing:?}",
            );
        }
        // Forbid double-binding the same slot to a different world segment.
        if let Some(existing) = self.slot_to_world[slot.0 as usize] {
            debug_assert_eq!(
                existing, world_seg,
                "bind({world_seg:?}, {slot:?}) — slot already bound to \
                 {existing:?}",
            );
        }

        self.world_to_slot.insert(world_seg, slot);
        self.slot_to_world[slot.0 as usize] = Some(world_seg);
        let local = self.local_of(world_seg);
        let idx = self.pack(local);
        self.indirection[idx as usize] = slot.0;

        #[cfg(debug_assertions)]
        self.audit_invariants();
    }

    /// Clear the binding for `world_seg`. Returns the freed slot for the
    /// caller to either re-`bind` (for an immediate admission) or `free`
    /// (return to pool). The indirection table is updated to
    /// [`EMPTY_SLOT`] at the corresponding local position. Returns `None`
    /// if `world_seg` was not bound.
    pub fn unbind(&mut self, world_seg: WorldSegmentPos) -> Option<SlotIndex> {
        let slot = self.world_to_slot.remove(&world_seg)?;
        // Clear the reverse lookup.
        self.slot_to_world[slot.0 as usize] = None;
        // Clear the indirection table entry (only if the segment is in the
        // current window — otherwise the local coord is meaningless).
        if self.is_in_window(world_seg) {
            let local = self.local_of(world_seg);
            let idx = self.pack(local);
            // Only clear if this slot owns the entry; defensive — another
            // slot may have re-bound to this local position (impossible
            // under the invariants, but the check costs one branch).
            if self.indirection[idx as usize] == slot.0 {
                self.indirection[idx as usize] = EMPTY_SLOT;
            }
        }
        #[cfg(debug_assertions)]
        self.audit_invariants();
        Some(slot)
    }

    /// Shift the window. Auto-unbinds every segment whose new local
    /// position would be out of window; rebuilds the indirection table
    /// from scratch for all remaining bound segments. Returns the
    /// `(world_seg, slot)` pairs that were unbound — the caller decides
    /// whether to `free()` them (return to pool) or `bind()` them to new
    /// admissions in the same call.
    ///
    /// `new_origin == origin()` is a fast-path no-op (returns empty Vec
    /// without touching the indirection buffer).
    pub fn set_origin(&mut self, new_origin: IVec3) -> Vec<(WorldSegmentPos, SlotIndex)> {
        // Edge case 1 — no shift, no work.
        if new_origin == self.origin {
            #[cfg(debug_assertions)]
            self.audit_invariants();
            return Vec::new();
        }

        // (1) Compute new window AABB. Window-size axes are u32, cast through
        // i32 for the half-open right edge math.
        let ws = self.window_size;
        let aabb_min = new_origin;
        let aabb_max = IVec3::new(
            new_origin.x + ws.x as i32,
            new_origin.y + ws.y as i32,
            new_origin.z + ws.z as i32,
        );

        // (2) Walk world_to_slot; collect every (world, slot) pair that falls
        // OUTSIDE the new AABB. Cannot mutate `world_to_slot` while iterating;
        // collect into a Vec first.
        let mut evicted: Vec<(WorldSegmentPos, SlotIndex)> = Vec::new();
        for (w, slot) in self.world_to_slot.iter() {
            let p = w.0;
            let inside = p.x >= aabb_min.x
                && p.x < aabb_max.x
                && p.y >= aabb_min.y
                && p.y < aabb_max.y
                && p.z >= aabb_min.z
                && p.z < aabb_max.z;
            if !inside {
                evicted.push((*w, *slot));
            }
        }

        // (3) Unbind each evicted pair. DO NOT push slots into free_list —
        // return them so the caller decides free vs immediate-re-bind.
        for (w, slot) in &evicted {
            self.world_to_slot.remove(w);
            self.slot_to_world[slot.0 as usize] = None;
            // indirection cleared in (5), so no per-slot write here.
        }

        // (4) Adopt the new origin.
        self.origin = new_origin;

        // (5) Rebuild `indirection` from scratch. Clear to EMPTY_SLOT, then
        // populate from every REMAINING bound pair.
        for entry in &mut self.indirection {
            *entry = EMPTY_SLOT;
        }
        for (w, slot) in self.world_to_slot.iter() {
            // After (3) every remaining `w` is inside the new window.
            let local = IVec3::new(
                w.0.x - new_origin.x,
                w.0.y - new_origin.y,
                w.0.z - new_origin.z,
            );
            let idx = self.pack(local);
            self.indirection[idx as usize] = slot.0;
        }

        #[cfg(debug_assertions)]
        self.audit_invariants();

        evicted
    }

    // --- packing helpers --------------------------------------------------

    /// Pack a `local_xyz: IVec3` into a flat `u32` indirection-table index.
    /// Row-major, X-fastest, matching the rest of the codebase's chunk
    /// indexing (`chunk_calc.wgsl:424-426`, `world_change.wgsl:320-322`,
    /// `bounds_calc.wgsl:365-367`, `ray_tracing.wgsl:290-294`).
    ///
    /// Caller must ensure `0 <= local_xyz.{x,y,z} < window_size.{x,y,z}`.
    /// Debug-asserts the bounds.
    pub fn pack(&self, local_xyz: IVec3) -> u32 {
        debug_assert!(
            local_xyz.x >= 0
                && (local_xyz.x as u32) < self.window_size.x
                && local_xyz.y >= 0
                && (local_xyz.y as u32) < self.window_size.y
                && local_xyz.z >= 0
                && (local_xyz.z as u32) < self.window_size.z,
            "pack({local_xyz:?}) outside window_size={:?}",
            self.window_size,
        );
        let lx = local_xyz.x as u32;
        let ly = local_xyz.y as u32;
        let lz = local_xyz.z as u32;
        lx + ly * self.window_size.x + lz * self.window_size.x * self.window_size.y
    }

    /// Verify every invariant (§ B). Called by every mutator at exit in
    /// `cfg(debug_assertions)`. Used directly by unit tests.
    #[cfg(debug_assertions)]
    fn audit_invariants(&self) {
        let cap = self.capacity() as usize;

        // I1 — buffer / pool size match.
        debug_assert_eq!(
            self.slot_to_world.len(),
            cap,
            "I1: slot_to_world length must equal capacity"
        );
        debug_assert_eq!(
            self.indirection.len(),
            cap,
            "I1: indirection length must equal capacity"
        );

        // I2 — pool + bound = capacity.
        debug_assert_eq!(
            self.free_list.len() + self.world_to_slot.len(),
            cap,
            "I2: free + bound must equal capacity (free={}, bound={}, cap={})",
            self.free_list.len(),
            self.world_to_slot.len(),
            cap,
        );

        // I3 — free_list ↔ slot_to_world[None] consistency.
        let none_count = self.slot_to_world.iter().filter(|s| s.is_none()).count();
        debug_assert_eq!(
            none_count,
            self.free_list.len(),
            "I3: slot_to_world None-count must equal free_list length",
        );

        // I4 — every entry in free_list maps to None in slot_to_world.
        for fs in &self.free_list {
            debug_assert!(
                self.slot_to_world[fs.0 as usize].is_none(),
                "I4: free slot {fs:?} has non-None slot_to_world",
            );
        }

        // I5 — bidirectional mapping consistency (forward → reverse).
        for (w, slot) in self.world_to_slot.iter() {
            debug_assert_eq!(
                self.slot_to_world[slot.0 as usize],
                Some(*w),
                "I5: world_to_slot {w:?} -> {slot:?} but slot_to_world disagrees",
            );
        }
        // I5b — bidirectional mapping consistency (reverse → forward).
        for (i, w_opt) in self.slot_to_world.iter().enumerate() {
            if let Some(w) = w_opt {
                debug_assert_eq!(
                    self.world_to_slot.get(w).copied(),
                    Some(SlotIndex(i as u32)),
                    "I5b: slot_to_world[{i}]={w:?} but world_to_slot disagrees",
                );
            }
        }

        // I6 — every bound world_seg is inside the current window.
        for w in self.world_to_slot.keys() {
            debug_assert!(
                self.is_in_window(*w),
                "I6: bound segment {w:?} out of window (origin={:?}, size={:?})",
                self.origin,
                self.window_size,
            );
        }

        // I7 — indirection[pack(local_of(w))] == slot for every bound pair.
        for (w, slot) in self.world_to_slot.iter() {
            let local = self.local_of(*w);
            let idx = self.pack(local);
            debug_assert_eq!(
                self.indirection[idx as usize], slot.0,
                "I7: indirection mismatch at {w:?} local={local:?} pack={idx}: \
                 expected slot {slot:?}, found {}",
                self.indirection[idx as usize],
            );
        }

        // I8 — every indirection entry that's NOT EMPTY_SLOT corresponds to a
        // bound pair. (Mirror of I7 in the other direction.)
        for (idx, slot_u) in self.indirection.iter().copied().enumerate() {
            if slot_u != EMPTY_SLOT {
                debug_assert!(
                    (slot_u as usize) < cap,
                    "I8: indirection[{idx}] = {slot_u} out of slot range",
                );
                let w_opt = self.slot_to_world[slot_u as usize];
                debug_assert!(
                    w_opt.is_some(),
                    "I8: indirection[{idx}] points to free slot {slot_u}",
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests — per `02c-design-windowed-slot-map.md` § H (T1..T20).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WORLD_SIZE_IN_SEGMENTS;

    fn standard_window() -> UVec3 {
        UVec3::new(
            WORLD_SIZE_IN_SEGMENTS.x,
            WORLD_SIZE_IN_SEGMENTS.y,
            WORLD_SIZE_IN_SEGMENTS.z,
        )
    }

    /// T1 — `new_empty_state`.
    #[test]
    fn new_empty_state() {
        let m = WindowedSlotMap::new(standard_window());
        assert_eq!(m.capacity(), 512);
        assert_eq!(m.free_count(), 512);
        assert_eq!(m.origin(), IVec3::ZERO);
        assert!(m.indirection_buffer().iter().all(|&e| e == EMPTY_SLOT));
    }

    /// T2 — `allocate_returns_slots_in_order_starting_from_zero`.
    #[test]
    fn allocate_returns_slots_in_order_starting_from_zero() {
        let mut m = WindowedSlotMap::new(standard_window());
        for expected in 0..5u32 {
            let s = m.allocate().expect("slot");
            assert_eq!(s, SlotIndex(expected));
        }
    }

    /// T3 — `allocate_returns_none_when_pool_empty`.
    #[test]
    fn allocate_returns_none_when_pool_empty() {
        let mut m = WindowedSlotMap::new(standard_window());
        for _ in 0..512 {
            assert!(m.allocate().is_some());
        }
        assert!(m.allocate().is_none());
    }

    /// T4 — `allocate_free_round_trips`.
    #[test]
    fn allocate_free_round_trips() {
        let mut m = WindowedSlotMap::new(standard_window());
        let mut held = Vec::new();
        for _ in 0..100 {
            held.push(m.allocate().expect("slot"));
        }
        assert_eq!(m.free_count(), 412);
        for s in held {
            m.free(s);
        }
        assert_eq!(m.free_count(), 512);
    }

    /// T5 — `bind_updates_indirection`.
    #[test]
    fn bind_updates_indirection() {
        let mut m = WindowedSlotMap::new(standard_window());
        let slot = m.allocate().expect("slot 0");
        let w = WorldSegmentPos(IVec3::new(3, 1, 5));
        m.bind(w, slot);
        let idx = m.pack(IVec3::new(3, 1, 5));
        assert_eq!(m.indirection_buffer()[idx as usize], slot.0);
    }

    /// T6 — `bind_round_trip_via_lookup`.
    #[test]
    fn bind_round_trip_via_lookup() {
        let mut m = WindowedSlotMap::new(standard_window());
        let slot = m.allocate().expect("slot 0");
        let w = WorldSegmentPos(IVec3::new(2, 0, 7));
        m.bind(w, slot);
        assert_eq!(m.lookup_slot(w), Some(slot));
        assert_eq!(m.lookup_world(slot), Some(w));
    }

    /// T7 — `unbind_clears_indirection`.
    #[test]
    fn unbind_clears_indirection() {
        let mut m = WindowedSlotMap::new(standard_window());
        let slot = m.allocate().expect("slot 0");
        let w = WorldSegmentPos(IVec3::new(4, 1, 9));
        m.bind(w, slot);
        let local = m.local_of(w);
        let idx = m.pack(local);
        assert_eq!(m.indirection_buffer()[idx as usize], slot.0);
        let returned = m.unbind(w).expect("slot returned");
        assert_eq!(returned, slot);
        assert_eq!(m.indirection_buffer()[idx as usize], EMPTY_SLOT);
    }

    /// T8 — `unbind_returns_slot_for_caller_disposition`.
    #[test]
    fn unbind_returns_slot_for_caller_disposition() {
        let mut m = WindowedSlotMap::new(standard_window());
        let slot = m.allocate().expect("slot");
        let w = WorldSegmentPos(IVec3::new(1, 0, 1));
        m.bind(w, slot);
        let initial_free = m.free_count();
        let returned = m.unbind(w).expect("slot returned");
        assert_eq!(returned, slot);
        // unbind does NOT push to free_list.
        assert_eq!(m.free_count(), initial_free);
        m.free(returned);
        assert_eq!(m.free_count(), initial_free + 1);
    }

    /// T9 — `set_origin_no_shift_returns_empty_vec`.
    #[test]
    fn set_origin_no_shift_returns_empty_vec() {
        let mut m = WindowedSlotMap::new(standard_window());
        let s = m.allocate().expect("s");
        m.bind(WorldSegmentPos(IVec3::new(2, 1, 2)), s);
        let before = m.indirection_buffer().to_vec();
        let evicted = m.set_origin(m.origin());
        assert!(evicted.is_empty());
        assert_eq!(before, m.indirection_buffer().to_vec());
    }

    /// T10 — `set_origin_full_evict_returns_all_pairs`.
    #[test]
    fn set_origin_full_evict_returns_all_pairs() {
        let mut m = WindowedSlotMap::new(standard_window());
        let mut pairs = Vec::new();
        for x in 0..5 {
            let s = m.allocate().expect("slot");
            let w = WorldSegmentPos(IVec3::new(x, 0, 0));
            m.bind(w, s);
            pairs.push((w, s));
        }
        let evicted = m.set_origin(IVec3::new(WORLD_SIZE_IN_SEGMENTS.x as i32, 0, 0));
        assert_eq!(evicted.len(), 5);
        assert!(m.indirection_buffer().iter().all(|&e| e == EMPTY_SLOT));
        assert!(m.iter_bound().next().is_none());
    }

    /// T11 — `set_origin_partial_shift_preserves_in_window`.
    #[test]
    fn set_origin_partial_shift_preserves_in_window() {
        let mut m = WindowedSlotMap::new(standard_window());
        // Bind a 16-wide row in X at (0..16, 0, 0).
        let mut pairs = Vec::new();
        for x in 0..16i32 {
            let s = m.allocate().expect("slot");
            let w = WorldSegmentPos(IVec3::new(x, 0, 0));
            m.bind(w, s);
            pairs.push((w, s));
        }
        let evicted = m.set_origin(IVec3::new(1, 0, 0));
        // Only the (0, 0, 0) segment was outside the new window.
        assert_eq!(evicted.len(), 1);
        assert_eq!((evicted[0].0).0, IVec3::new(0, 0, 0));
        // The remaining 15 bindings are at local.x in [0, 15).
        for (w, _) in m.iter_bound() {
            let local = m.local_of(w);
            assert!(local.x >= 0 && local.x < 15);
        }
    }

    /// T12 — `set_origin_rebuilds_indirection_correctly`.
    #[test]
    fn set_origin_rebuilds_indirection_correctly() {
        let mut m = WindowedSlotMap::new(standard_window());
        for x in 0..16i32 {
            let s = m.allocate().expect("slot");
            let w = WorldSegmentPos(IVec3::new(x, 0, 0));
            m.bind(w, s);
        }
        m.set_origin(IVec3::new(1, 0, 0));
        // For each remaining bound pair, indirection[pack(local_of(w))] == slot.0
        for (w, slot) in m.iter_bound() {
            let local = m.local_of(w);
            let idx = m.pack(local);
            assert_eq!(m.indirection_buffer()[idx as usize], slot.0);
        }
    }

    /// T13 — `bind_panics_on_out_of_window` (debug).
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "outside window")]
    fn bind_panics_on_out_of_window() {
        let mut m = WindowedSlotMap::new(standard_window());
        let s = m.allocate().expect("slot");
        m.bind(WorldSegmentPos(IVec3::new(100, 0, 0)), s);
    }

    /// T14 — `bind_panics_on_double_bind_world` (debug).
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "already bound to")]
    fn bind_panics_on_double_bind_world() {
        let mut m = WindowedSlotMap::new(standard_window());
        let s1 = m.allocate().expect("s1");
        let s2 = m.allocate().expect("s2");
        let w = WorldSegmentPos(IVec3::new(1, 0, 1));
        m.bind(w, s1);
        m.bind(w, s2);
    }

    /// T15 — `bind_panics_on_double_bind_slot` (debug).
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "slot already bound to")]
    fn bind_panics_on_double_bind_slot() {
        let mut m = WindowedSlotMap::new(standard_window());
        let s = m.allocate().expect("slot");
        let w1 = WorldSegmentPos(IVec3::new(1, 0, 1));
        let w2 = WorldSegmentPos(IVec3::new(2, 0, 2));
        m.bind(w1, s);
        m.bind(w2, s);
    }

    /// T16 — `free_panics_on_bound_slot` (debug).
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "still bound to")]
    fn free_panics_on_bound_slot() {
        let mut m = WindowedSlotMap::new(standard_window());
        let s = m.allocate().expect("slot");
        let w = WorldSegmentPos(IVec3::new(1, 0, 1));
        m.bind(w, s);
        m.free(s);
    }

    /// T17 — `indirection_buffer_length_equals_capacity`.
    #[test]
    fn indirection_buffer_length_equals_capacity() {
        let m = WindowedSlotMap::new(standard_window());
        assert_eq!(m.indirection_buffer().len() as u32, m.capacity());
        assert_eq!(m.indirection_buffer().len(), 512);
    }

    /// T18 — `audit_invariants_after_random_mutations` (debug).
    ///
    /// The audit_invariants method runs at the exit of every mutator under
    /// `cfg(debug_assertions)`; this test exercises a pseudo-random sequence
    /// of 200 ops and relies on the in-mutator audit to catch violations.
    #[test]
    fn audit_invariants_after_random_mutations() {
        // Deterministic LCG so failures are reproducible.
        let mut rng_state: u32 = 0xC0FF_EE42;
        let mut next = |rng: &mut u32| -> u32 {
            *rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            *rng
        };

        let mut m = WindowedSlotMap::new(standard_window());
        // Track bound segments so we can pick a real one to unbind.
        let mut bound: Vec<WorldSegmentPos> = Vec::new();

        for _ in 0..200 {
            let r = next(&mut rng_state) % 5;
            match r {
                0 => {
                    // allocate
                    let _ = m.allocate();
                }
                1 => {
                    // bind (only if a free slot is available and we can
                    // synthesise an in-window world segment that isn't bound).
                    if let Some(s) = m.allocate() {
                        // Pick a random local position in window.
                        let lx = (next(&mut rng_state)
                            % m.window_size().x) as i32;
                        let ly = (next(&mut rng_state)
                            % m.window_size().y) as i32;
                        let lz = (next(&mut rng_state)
                            % m.window_size().z) as i32;
                        let w = WorldSegmentPos(
                            m.origin() + IVec3::new(lx, ly, lz),
                        );
                        if m.lookup_slot(w).is_none() {
                            m.bind(w, s);
                            bound.push(w);
                        } else {
                            // Return the slot — couldn't bind.
                            m.free(s);
                        }
                    }
                }
                2 => {
                    // unbind one bound segment.
                    if !bound.is_empty() {
                        let i = (next(&mut rng_state) as usize) % bound.len();
                        let w = bound.swap_remove(i);
                        if let Some(s) = m.unbind(w) {
                            m.free(s);
                        }
                    }
                }
                3 => {
                    // set_origin shift by small random delta.
                    let dx = ((next(&mut rng_state) % 5) as i32) - 2;
                    let dz = ((next(&mut rng_state) % 5) as i32) - 2;
                    let new_origin = m.origin() + IVec3::new(dx, 0, dz);
                    let evicted = m.set_origin(new_origin);
                    for (w, s) in evicted {
                        m.free(s);
                        bound.retain(|b| *b != w);
                    }
                }
                _ => {
                    // free a held slot — we lazily walk bound + unbind
                    // (covered above). Use this slot for an explicit
                    // `allocate + free` round-trip.
                    if let Some(s) = m.allocate() {
                        m.free(s);
                    }
                }
            }
        }
        // Final audit fires through any mutator; explicitly call it via a
        // trivial set_origin no-op.
        m.set_origin(m.origin());
    }

    /// T19 — `pack_round_trip_x_fastest`.
    ///
    /// For every `(lx, ly, lz)` in `[0,16) × [0,2) × [0,16)`, the pack
    /// formula agrees with the existing `Residency::slot_index_of`
    /// row-major-X-fastest formula at `residency.rs:128-132`.
    #[test]
    fn pack_round_trip_x_fastest() {
        let m = WindowedSlotMap::new(standard_window());
        for lx in 0..WORLD_SIZE_IN_SEGMENTS.x {
            for ly in 0..WORLD_SIZE_IN_SEGMENTS.y {
                for lz in 0..WORLD_SIZE_IN_SEGMENTS.z {
                    let packed = m.pack(IVec3::new(lx as i32, ly as i32, lz as i32));
                    let res_idx = super::super::residency::Residency::slot_index_of([
                        lx, ly, lz,
                    ]);
                    assert_eq!(packed, res_idx);
                }
            }
        }
    }

    /// T20 — `set_origin_idempotent_under_re_derivation`.
    #[test]
    fn set_origin_idempotent_under_re_derivation() {
        let mut m = WindowedSlotMap::new(standard_window());
        for x in 0..5i32 {
            let s = m.allocate().expect("slot");
            m.bind(WorldSegmentPos(IVec3::new(x, 0, 0)), s);
        }
        let new_origin = IVec3::new(1, 0, 0);
        m.set_origin(new_origin);
        let after_first = m.indirection_buffer().to_vec();
        let evicted = m.set_origin(new_origin);
        assert!(evicted.is_empty());
        assert_eq!(after_first, m.indirection_buffer().to_vec());
    }
}
