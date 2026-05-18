//! `streaming::residency` — sliding-window residency manager
//! (`docs/orchestrate/streaming-world/02-design.md` §§ A.1–A.5, carried over
//! into `02b-design-plan-b.md` § D).
//!
//! Owns:
//! - The dense `slot_to_world` table mapping window-local slot indices to
//!   world-segment coordinates.
//! - The reverse `world_to_slot` map for `is_resident` lookups.
//! - Per-frame `admissions` / `evictions` deltas the render-world dispatcher
//!   consumes via `ExtractResource`.
//! - The `residency_driver` system that detects camera-segment boundary
//!   crossings, computes the target resident set, and produces the admission /
//!   eviction lists.
//!
//! Q1 (residency-only `i32` widening) is enforced here: world-segment positions
//! are stored as `IVec3` (a `WorldSegmentPos` newtype); slot indices are u32.
//! The renderer never sees the world `IVec3` — only window-local
//! `(slot_x, slot_y, slot_z)` derived from the slot index.
//!
//! VRAM-budget pre-flight (per § A.4) lives in [`assert_vram_budget_sufficient`]:
//! called at startup install time; panics with a clear message on insufficient
//! budget.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{WORLD_SIZE_IN_SEGMENTS, WORLD_GEN_SEGMENT_SIZE_IN_GROUPS};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotIndex(pub u32);

/// Per-slot generation state. Phase 2 uses GPU-side noise generation, so
/// "encoded" is collapsed: a slot is either Empty, Generating (admitted but
/// awaiting GPU dispatch), or Resident (dispatched + part of the rendered set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    /// No content; either freshly evicted or never populated.
    Empty,
    /// Admitted to the resident set but the GPU dispatch hasn't fired yet
    /// (`dispatched_frame` records the frame on which dispatch was scheduled
    /// — currently unused but reserved for budget-tracking diagnostics).
    Generating { dispatched_frame: u64 },
    /// Dispatched + rendered.
    Resident,
}

/// Main-world residency manager `Resource` (`02-design.md` § A.2).
///
/// Window geometry: world container = `WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)` →
/// 512 slots. The window shifts in X/Z under camera motion; Y is full-height
/// (only 2 segments tall, so splitting that axis costs more than it saves).
#[derive(Resource, Clone)]
pub struct Residency {
    /// World-origin offset in segments. Shifted whenever the camera crosses a
    /// segment boundary; world segment `s` lives at window-local slot
    /// `slot_xyz = s.0 - origin` (when in range).
    pub origin: IVec3,
    /// Dense forward map — `slot_index → WorldSegmentPos`. `None` means the
    /// slot is Empty.
    pub slot_to_world: Vec<Option<WorldSegmentPos>>,
    /// Reverse index — `WorldSegmentPos → SlotIndex`. ≤ 512 entries.
    pub world_to_slot: HashMap<WorldSegmentPos, SlotIndex>,
    /// Per-slot state.
    pub slot_state: Vec<SlotState>,
    /// Per-frame admissions (world-segment pos, slot it was assigned to) —
    /// drained by the render-world dispatch each frame.
    pub admissions_this_frame: Vec<(WorldSegmentPos, SlotIndex)>,
    /// Per-frame evictions (the slot that just became Empty). The eviction
    /// path doesn't currently re-issue W2 records — Phase 2 evictions just
    /// mark the slot Empty and the per-frame bounds chain refresh accounts for
    /// the change. (Per the Phase-2 brief, full W2-record eviction is a
    /// future-extension hook; the noise driver overwrites the slot's chunks
    /// the moment a new admission lands on it, so no zero-write is strictly
    /// necessary.)
    pub evictions_this_frame: Vec<SlotIndex>,
    /// Max admissions per frame (CLI: `--max-segments-per-frame`, default 4).
    pub max_segments_per_frame: u32,
    /// Frame counter for budget bookkeeping (mostly diagnostic).
    pub frame_counter: u64,
    /// Last camera segment seen — used to detect "did the camera cross a
    /// boundary?" without unnecessary recomputation.
    pub last_camera_seg: Option<IVec3>,
}

impl Residency {
    /// Build a freshly-empty residency table with `total_slots = wx*wy*wz`
    /// entries. The window is camera-centred at install time (the first
    /// `residency_driver` tick produces the initial admissions list).
    pub fn empty(max_segments_per_frame: u32) -> Self {
        let total = (WORLD_SIZE_IN_SEGMENTS.x
            * WORLD_SIZE_IN_SEGMENTS.y
            * WORLD_SIZE_IN_SEGMENTS.z) as usize;
        Self {
            origin: IVec3::ZERO,
            slot_to_world: vec![None; total],
            world_to_slot: HashMap::with_capacity(total),
            slot_state: vec![SlotState::Empty; total],
            admissions_this_frame: Vec::new(),
            evictions_this_frame: Vec::new(),
            max_segments_per_frame,
            frame_counter: 0,
            last_camera_seg: None,
        }
    }

    /// Window total slot count.
    pub fn total_slots() -> u32 {
        WORLD_SIZE_IN_SEGMENTS.x * WORLD_SIZE_IN_SEGMENTS.y * WORLD_SIZE_IN_SEGMENTS.z
    }

    /// Compute slot index from window-local segment coordinates.
    pub fn slot_index_of(local_xyz: [u32; 3]) -> u32 {
        let [lx, ly, lz] = local_xyz;
        lx + ly * WORLD_SIZE_IN_SEGMENTS.x
            + lz * WORLD_SIZE_IN_SEGMENTS.x * WORLD_SIZE_IN_SEGMENTS.y
    }

    /// Decompose slot index into window-local segment coordinates.
    pub fn local_of(slot: u32) -> [u32; 3] {
        let wx = WORLD_SIZE_IN_SEGMENTS.x;
        let wy = WORLD_SIZE_IN_SEGMENTS.y;
        let lx = slot % wx;
        let ly = (slot / wx) % wy;
        let lz = slot / (wx * wy);
        [lx, ly, lz]
    }

    /// True when the world-segment falls inside the current window (X/Z + Y).
    pub fn is_in_window(&self, s: WorldSegmentPos) -> bool {
        let d = s.0 - self.origin;
        d.x >= 0
            && d.x < WORLD_SIZE_IN_SEGMENTS.x as i32
            && d.y >= 0
            && d.y < WORLD_SIZE_IN_SEGMENTS.y as i32
            && d.z >= 0
            && d.z < WORLD_SIZE_IN_SEGMENTS.z as i32
    }

    /// Resolve a world-segment to a slot index, if resident.
    pub fn slot_of(&self, s: WorldSegmentPos) -> Option<SlotIndex> {
        self.world_to_slot.get(&s).copied()
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
///
/// Per `02-design.md` § A.4's accounting table. Numbers documented in the
/// design; the function recomputes them so a future constant change is
/// reflected here without a separate edit.
pub fn compute_slab_total_mib() -> u64 {
    // segment_voxel_buffer: SEGMENT_CHUNKS^3 × 2048 u32 × 4 B (one segment-cubic
    // scratch reused across all dispatches).
    let segment_voxel_mib = (SEGMENT_CHUNKS as u64).pow(3) * 2048 * 4 / (1024 * 1024);
    // WorldGpu.chunks_buffer: WORLD_SIZE_IN_CHUNKS × 2 u32 × 4 B (Rg32Uint).
    let world_chunks = (crate::WORLD_SIZE_IN_CHUNKS.x as u64)
        * (crate::WORLD_SIZE_IN_CHUNKS.y as u64)
        * (crate::WORLD_SIZE_IN_CHUNKS.z as u64);
    let chunks_buffer_mib = world_chunks * 2 * 4 / (1024 * 1024);
    // WorldGpu.blocks / voxels worst-case — per render/prepare.rs sizing logic
    // these scale with chunks * 64 (blocks) and chunks * 128 (voxels at 2× /
    // 0.5 ratio). The numbers in `02-design.md` § A.4 are 256 MiB each.
    let blocks_mib = 256;
    let voxels_mib = 256;
    // hash_map: 1 << 20 × 16 B = 16 MiB. (`ConstructionConfig::initial_hash_map_size`.)
    let hash_map_mib = 16;
    // bound_* queues + masks + indirect — design figures it at ~24 MiB.
    let bounds_mib = 24;
    // model_data buffers — Phase 2 streaming does NOT use them (noise_terrain
    // bypasses model_data). 0 MiB.
    // Misc overhead — palette, params uniforms, etc. ~4 MiB headroom.
    let misc_mib = 4;

    segment_voxel_mib + chunks_buffer_mib + blocks_mib + voxels_mib + hash_map_mib
        + bounds_mib + misc_mib
}

/// Camera segment for the current frame, in `WorldSegmentPos` units. Reads
/// the camera's `PositionSplit::pos_int` (world-voxel coord) and divides by
/// `SEGMENT_VOXELS`.
fn camera_segment_pos(camera_pos_int: IVec3) -> WorldSegmentPos {
    world_voxel_to_segment(camera_pos_int)
}

/// `PreUpdate` system — detect camera-segment crossings, recompute the target
/// resident set, populate `admissions_this_frame` + `evictions_this_frame`,
/// honour the `--max-segments-per-frame` budget. Per `02-design.md` § A.3.
///
/// `residency` is `Option<ResMut>` because the resource is only present when
/// `GridPreset::ProceduralStreaming` was installed at startup. Non-streaming
/// presets see `None` and early-return.
pub fn residency_driver(
    residency: Option<ResMut<Residency>>,
    camera: Option<
        Single<&crate::camera::position_split::PositionSplit, With<bevy::prelude::Camera3d>>,
    >,
) {
    let Some(mut residency) = residency else {
        return;
    };
    residency.frame_counter = residency.frame_counter.wrapping_add(1);
    residency.admissions_this_frame.clear();
    residency.evictions_this_frame.clear();

    let Some(camera) = camera else {
        // No camera yet — the very first frame of the e2e harness can hit
        // this. Defer until camera exists.
        return;
    };
    let cam_seg_world = camera_segment_pos(camera.pos_int);

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
    residency.origin = new_origin;
    residency.last_camera_seg = Some(cam_seg_world.0);

    // Compute target resident set: every world-segment whose local coord is
    // in [0, w) for each axis.
    let mut target: Vec<WorldSegmentPos> = Vec::with_capacity(Residency::total_slots() as usize);
    for lz in 0..WORLD_SIZE_IN_SEGMENTS.z {
        for ly in 0..WORLD_SIZE_IN_SEGMENTS.y {
            for lx in 0..WORLD_SIZE_IN_SEGMENTS.x {
                let world_seg = WorldSegmentPos(IVec3::new(
                    new_origin.x + lx as i32,
                    new_origin.y + ly as i32,
                    new_origin.z + lz as i32,
                ));
                target.push(world_seg);
            }
        }
    }

    // Pass 1 — evict slots whose current contents are no longer in-window.
    let mut to_evict: Vec<SlotIndex> = Vec::new();
    for (slot_i, world_opt) in residency.slot_to_world.iter().enumerate() {
        if let Some(world_seg) = world_opt {
            // If the slot's content is NOT in the new target window, evict.
            if !residency.is_in_window(*world_seg) {
                to_evict.push(SlotIndex(slot_i as u32));
            }
        }
    }
    for slot in &to_evict {
        let s = *slot;
        if let Some(Some(prev)) = residency.slot_to_world.get(s.0 as usize).copied() {
            residency.world_to_slot.remove(&prev);
        }
        if let Some(slot_ref) = residency.slot_to_world.get_mut(s.0 as usize) {
            *slot_ref = None;
        }
        if let Some(state) = residency.slot_state.get_mut(s.0 as usize) {
            *state = SlotState::Empty;
        }
        residency.evictions_this_frame.push(s);
    }

    // Pass 2 — figure out which target segments are not yet resident; queue
    // them for admission (camera-distance first per D.11).
    let cam_seg = cam_seg_world.0;
    let mut pending: Vec<WorldSegmentPos> = target
        .iter()
        .filter(|w| !residency.world_to_slot.contains_key(*w))
        .copied()
        .collect();
    pending.sort_by_key(|w| {
        let d = w.0 - cam_seg;
        d.x * d.x + d.y * d.y + d.z * d.z
    });

    // Pass 3 — assign empty slots to pending admissions (camera-distance first).
    // Move-out → reassign empty_slots without holding the borrow.
    let mut empty_slots: Vec<u32> = residency
        .slot_to_world
        .iter()
        .enumerate()
        .filter_map(|(i, w)| if w.is_none() { Some(i as u32) } else { None })
        .collect();
    // Reverse so we pop the lowest-index empty slot first (deterministic).
    empty_slots.reverse();

    for w in pending {
        let Some(slot_u) = empty_slots.pop() else {
            // No empty slots left this frame — leave the rest for the next
            // tick (they'll re-enter `pending` because the target set still
            // includes them).
            break;
        };
        residency.slot_to_world[slot_u as usize] = Some(w);
        residency.world_to_slot.insert(w, SlotIndex(slot_u));
        residency.slot_state[slot_u as usize] =
            SlotState::Generating { dispatched_frame: residency.frame_counter };
        // Don't push to admissions_this_frame yet — the budget gate below caps
        // it. The state machine progresses from Generating → Resident only
        // when the render-world actually dispatches.
    }

    // Pass 4 — budgeted admissions list — pick up to `max_segments_per_frame`
    // Generating slots in camera-distance order.
    process_pending_admissions(&mut residency);

    bevy::log::info!(
        "streaming-world residency shift: cam_seg={:?}, new_origin={:?}, \
         evictions={}, pending Generating slots={}, admissions_this_frame={}",
        cam_seg_world.0,
        new_origin,
        residency.evictions_this_frame.len(),
        residency.slot_state.iter().filter(|s| matches!(s, SlotState::Generating { .. })).count(),
        residency.admissions_this_frame.len(),
    );
}

/// Pick up to `max_segments_per_frame` Generating slots in camera-distance
/// order and place them into `admissions_this_frame`. The render-world dispatch
/// later marks each as `Resident`.
fn process_pending_admissions(residency: &mut Residency) {
    let cap = residency.max_segments_per_frame as usize;
    if cap == 0 {
        return;
    }
    let cam_seg = residency.last_camera_seg.unwrap_or(IVec3::ZERO);
    let mut candidates: Vec<(SlotIndex, WorldSegmentPos, i32)> = residency
        .slot_state
        .iter()
        .enumerate()
        .filter_map(|(i, st)| match st {
            SlotState::Generating { .. } => {
                let world = residency.slot_to_world.get(i).copied().flatten()?;
                let d = world.0 - cam_seg;
                let dsq = d.x * d.x + d.y * d.y + d.z * d.z;
                Some((SlotIndex(i as u32), world, dsq))
            }
            _ => None,
        })
        .collect();
    candidates.sort_by_key(|c| c.2);

    for (slot, world, _dsq) in candidates.into_iter().take(cap) {
        residency.admissions_this_frame.push((world, slot));
    }
}

/// Mark a slot Resident once the render-world has actually dispatched its
/// noise + chunk_calc passes. Called by the noise-dispatch system (or could be
/// folded into a Bevy `ExtractSchedule` mirror back to the main world).
pub fn mark_admissions_resident(
    residency: &mut Residency,
    admissions: &[(WorldSegmentPos, SlotIndex)],
) {
    for (_w, slot) in admissions {
        if let Some(state) = residency.slot_state.get_mut(slot.0 as usize) {
            *state = SlotState::Resident;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // A voxel at (-1, 0, 0) lives in segment (-1, 0, 0).
        let s = world_voxel_to_segment(IVec3::new(-1, 0, 0));
        assert_eq!(s.0, IVec3::new(-1, 0, 0));
        // A voxel at (0, 0, 0) lives in segment (0, 0, 0).
        let s = world_voxel_to_segment(IVec3::new(0, 0, 0));
        assert_eq!(s.0, IVec3::new(0, 0, 0));
        // A voxel at (-SEGMENT_VOXELS, 0, 0) lives in segment (-1, 0, 0).
        let s = world_voxel_to_segment(IVec3::new(-SEGMENT_VOXELS, 0, 0));
        assert_eq!(s.0, IVec3::new(-1, 0, 0));
        // A voxel at (SEGMENT_VOXELS, 0, 0) lives in segment (1, 0, 0).
        let s = world_voxel_to_segment(IVec3::new(SEGMENT_VOXELS, 0, 0));
        assert_eq!(s.0, IVec3::new(1, 0, 0));
    }

    #[test]
    fn target_origin_centers_camera_xz() {
        let cam = IVec3::new(50, 0, 50);
        let origin = target_origin_for_camera_seg(cam);
        // Camera segment lives at local (8, 0, 8) for the 16×2×16 window.
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
        assert_eq!(r.slot_to_world.len(), 512);
        assert!(r.slot_to_world.iter().all(Option::is_none));
        assert!(r.slot_state.iter().all(|s| matches!(s, SlotState::Empty)));
        assert!(r.world_to_slot.is_empty());
    }

    #[test]
    fn vram_budget_sufficient_passes_at_default() {
        // The design's documented 1024 MiB default covers the slab.
        super::assert_vram_budget_sufficient(1024);
    }

    #[test]
    #[should_panic(expected = "VRAM budget pre-flight FAILED")]
    fn vram_budget_panics_below_floor() {
        super::assert_vram_budget_sufficient(0);
    }
}
