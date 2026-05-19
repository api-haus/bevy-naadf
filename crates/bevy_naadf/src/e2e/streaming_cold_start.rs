//! `--gate streaming-cold-start` e2e gate.
//!
//! streaming-world Phase 2.13
//! (`docs/orchestrate/streaming-world/03r-diagnosis-cold-start-gap.md` MUST-2)
//! — content-checking gate that catches the cold-start admission-race bug
//! at the level of `chunks_buffer` decoded state, NOT framebuffer SSIM.
//!
//! ## Why content-checking, not framebuffer SSIM
//!
//! The Phase 2.12 `streaming-framebuffer-diff` gate at SSIM threshold 0.05
//! is loose enough that a rectangular sky-coloured hole at the camera-
//! nearest 4-24 segments does NOT push the comparison past the threshold
//! (`03r-diagnosis-cold-start-gap.md` § Why this wasn't caught before).
//! The user has been burned three times by loose pixel comparison; the
//! brief mandates content-based assertions on regressed paths.
//!
//! This gate decodes the actual `chunks_buffer` post-cold-start. For each
//! camera-row world segment (within view distance of the spawn camera
//! segment at `(8, 1, 8)`), it walks the slot's 4096 chunks, decodes the
//! 2-bit state field (`x >> 30`), and asserts each segment has ≥1 chunk
//! with `state != UNIFORM_EMPTY`. A regressed implementation that
//! re-introduces premature `dispatched_once.insert(slot)` produces slots
//! that are pin-pointed by the indirection table but whose `chunks_buffer`
//! region is uniformly zeroed by `clear_streaming_bound_slots` — exactly
//! the failure signal.
//!
//! ## Wiring
//!
//! 1. `apply_streaming_cold_start_defaults` overlays on top of
//!    `apply_streaming_window_defaults` (the gate inherits the streaming
//!    preset install + warmup), then flips `streaming_cold_start_mode = true`
//!    and resets the snapshot latches.
//! 2. After warmup completes the driver runs through `OasisShootBefore →
//!    OasisDrainBefore → OasisApplyEdit`. This gate triggers
//!    `request_snapshot` in `OasisApplyEdit` instead of walking the
//!    camera, then waits the standard post-edit interval for the readback
//!    to land.
//! 3. The driver's `OasisAssert` step branches on `streaming_cold_start_mode`
//!    and invokes [`assert_streaming_cold_start_landed`].
//!
//! ## Camera-row segments
//!
//! Per `03r` § Image-8 segment identification, the cold-start camera
//! spawn pose lands the camera at world segment `(8, 1, 8)`. The 6
//! camera-nearest segments by `dsq` ≤ 1 are:
//!
//! - `(8, 1, 8)` (dsq=0 — camera's own segment)
//! - `(7, 1, 8), (9, 1, 8), (8, 1, 7), (8, 1, 9)` (dsq=1, +/-X +/-Z ring)
//! - `(8, 0, 8)` (dsq=1, below sea-level — solid by classifier)
//!
//! And the 8 `dsq ≤ 2` ring (the mid-ring failure set for K=2 frames):
//!
//! - `(7, 0, 8), (9, 0, 8), (8, 0, 7), (8, 0, 9)`
//! - `(7, 1, 7), (7, 1, 9), (9, 1, 7), (9, 1, 9)`
//!
//! Together = 14 segments. With `max_segments_per_frame = 4` and K = 3-6
//! cold-start race frames, the diagnosed bug burns 12-24 segments — the
//! camera-row + mid-ring + tip of the next ring. The 14 covered here are
//! the highest-confidence failure set; if THESE all pass the gate, the
//! bug is closed (or the gate is itself tautological — see § Gate
//! sanity check in `03s-impl-cold-start-fix.md`).

use bevy::prelude::*;

use crate::streaming::residency::{world_voxel_to_segment, WorldSegmentPos};
use crate::WORLD_SIZE_IN_VOXELS;

/// Number of warmup ticks before the snapshot fires. The cold-start
/// admission rate is `max_segments_per_frame = 4` slots/frame; the full
/// drain requires `WORLD_SIZE_IN_SEGMENTS.x * y * z / 4 = 16*2*16/4 =
/// 128` frames at minimum, plus a margin for the Frame-0 `WorldGpu`
/// race (1-3 frames) + WGSL pipeline compile (3-6 frames). 200 ticks
/// gives ~50% margin; with the e2e harness ticking at 60 fps that's
/// ~3.3 wall-clock seconds.
pub const STREAMING_COLD_START_WARMUP_FRAMES: u32 = 200;

/// Maximum acceptable count of empty camera-row segments. Default 0
/// (strict): the diagnosed bug burns 4-24 segments inside this set, so
/// even one empty camera-row segment is a regression signal. A future
/// pose / preset change that legitimately produces an all-empty
/// above-sea-level segment may need to bump this — but the default Y=0
/// row is overwhelmingly solid (below sea level → noise classifier
/// returns "solid" almost everywhere), so even with Y=1 partially
/// above-terrain there are 14 segments worth of coverage and at least
/// half overlap the heightmap.
pub const STREAMING_COLD_START_MAX_EMPTY_SEGMENTS: usize = 0;

/// World chunk state constants (mirror of `world_data.wgsl`).
const BLOCK_STATE_UNIFORM_EMPTY: u32 = 0;

/// Apply the cold-start gate's defaults onto `args`. Inherits the
/// streaming-window setup (preset install, warmup) and adds the
/// cold-start-mode flag so the driver's snapshot-capture branch fires
/// AND `OasisAssert` dispatches to [`assert_streaming_cold_start_landed`].
pub fn apply_streaming_cold_start_defaults(args: &mut crate::AppArgs) {
    // Layer onto streaming-window's defaults (preset install, residency,
    // walk infrastructure — though we won't trigger the walk).
    super::streaming_window::apply_streaming_window_defaults(args);
    args.streaming_cold_start_mode = true;
    // NOTE: we keep `streaming_window_mode = true` because the
    // streaming-window's camera-pin system (`pin_streaming_window_camera`)
    // is the one that anchors the camera to the spawn pose. The walk
    // itself is gated separately on `camera_has_walked()` (latched by
    // `promote_camera_to_walk`); since the cold-start gate's
    // OasisApplyEdit branch does NOT call `promote_camera_to_walk`, the
    // pin system stays in the "pre-walk" arm and the camera holds the
    // spawn pose — exactly what we want.
    // Reset the parity gate's snapshot latches (we reuse its
    // chunks_buffer + indirection readback infrastructure verbatim).
    super::streaming_aadf_parity::reset_parity_latches();
    println!(
        "e2e_render --gate streaming-cold-start: layered on streaming-window \
         preset install; warmup={} frames; max-empty-camera-row-segments={}.",
        STREAMING_COLD_START_WARMUP_FRAMES,
        STREAMING_COLD_START_MAX_EMPTY_SEGMENTS,
    );
}

/// `Update` system — request the chunks_buffer snapshot once the warmup
/// phase ends. Runs in the main-world Update schedule; the snapshot
/// itself happens in a render-world system gated on `SNAPSHOT_REQUESTED`
/// (reuses `streaming_aadf_parity::render_world_chunks_readback`).
///
/// Wired via `add_e2e_systems` (`e2e/mod.rs`) only when
/// `args.streaming_cold_start_mode`. Idempotent: subsequent calls after
/// the first successful snapshot are no-ops (gated on
/// `streaming_aadf_parity::SNAPSHOT_DONE`).
pub fn request_snapshot_after_warmup(
    args: Option<Res<crate::AppArgs>>,
    state: Option<Res<super::driver::E2eState>>,
) {
    let Some(args) = args else { return; };
    if !args.streaming_cold_start_mode {
        return;
    }
    let Some(state) = state else { return; };
    // Fire after the warmup phase has fully elapsed. We piggy-back on
    // the Oasis driver state machine; by the time the driver enters
    // OasisShootBefore the warmup ticks are complete.
    use super::driver::E2ePhase;
    match state.phase {
        E2ePhase::OasisShootBefore
        | E2ePhase::OasisDrainBefore
        | E2ePhase::OasisApplyEdit
        | E2ePhase::OasisWaitPostEdit
        | E2ePhase::OasisShootAfter
        | E2ePhase::OasisDrainAfter
        | E2ePhase::OasisAssert => {
            super::streaming_aadf_parity::request_snapshot();
        }
        _ => {}
    }
}

/// Camera spawn segment for the streaming preset's default install
/// (`crate::voxel::grid::install_procedural_streaming_world`).
/// World voxel `cam_pos = (cx, cy, cz) = (2048, sea_level + 32, 2048)`;
/// with `WORLD_SIZE_IN_VOXELS = (4096, 512, 4096)` and `SEGMENT_VOXELS = 256`
/// that's segment `(8, 1, 8)` for default `sea_level = 256`.
fn camera_spawn_segment(sea_level: f32) -> WorldSegmentPos {
    let cx = (WORLD_SIZE_IN_VOXELS.x as f32) * 0.5;
    let cz = (WORLD_SIZE_IN_VOXELS.z as f32) * 0.5;
    let cy = sea_level + 32.0;
    world_voxel_to_segment(IVec3::new(cx as i32, cy as i32, cz as i32))
}

/// Compute the camera-row segment set: every world segment with
/// `dsq <= max_dsq` from `cam_seg`. Includes ALL the 6 dsq≤1 inner ring
/// + 8 dsq=2 mid-ring (max_dsq=2 = 14 segments).
///
/// Clamps to `[0, WORLD_SIZE_IN_SEGMENTS)` so segments straddling the
/// world edge don't show up (the streaming window pins origin Y=0;
/// out-of-range Y segments are unrepresentable).
pub fn camera_row_segments(cam_seg: WorldSegmentPos, max_dsq: i32) -> Vec<WorldSegmentPos> {
    let mut out = Vec::new();
    let radius = max_dsq.max(0).isqrt() + 1;
    for dx in -radius..=radius {
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                let dsq = dx * dx + dy * dy + dz * dz;
                if dsq > max_dsq {
                    continue;
                }
                let p = cam_seg.0 + IVec3::new(dx, dy, dz);
                if p.x < 0
                    || p.y < 0
                    || p.z < 0
                    || p.x >= crate::WORLD_SIZE_IN_SEGMENTS.x as i32
                    || p.y >= crate::WORLD_SIZE_IN_SEGMENTS.y as i32
                    || p.z >= crate::WORLD_SIZE_IN_SEGMENTS.z as i32
                {
                    continue;
                }
                out.push(WorldSegmentPos(p));
            }
        }
    }
    out
}

/// Verify the chunks_buffer cold-start invariant against a snapshot +
/// indirection table. Returns `(empty_segments, total_segments,
/// detail_string)`.
///
/// For every world segment in `segs`, look up its slot via the indirection
/// table (using the `pack(local) = lx + ly*WORLD_SIZE.x + lz*WORLD_SIZE.x*WORLD_SIZE.y`
/// formula — mirrors `WindowedSlotMap::pack` + `streaming_aadf_parity::
/// validate_self_consistency`'s indirection layout), walk the slot's 4096
/// chunks, decode `state = chunks_buffer[slot * 4096 + i].x >> 30u`, and
/// count how many of the requested segments have ZERO non-EMPTY chunks
/// (the cold-start regression signal).
///
/// `origin`: the residency origin in segments. `world_local = world_seg
/// - origin` is the indirection-table key.
pub fn validate_cold_start_content(
    chunks: &[u32],
    indirection: &[u32],
    origin: IVec3,
    segs: &[WorldSegmentPos],
) -> (Vec<WorldSegmentPos>, usize, String) {
    const EMPTY_SLOT: u32 = u32::MAX;
    const CHUNKS_PER_SLOT: u32 = 4096;
    let total = segs.len();
    let mut empty_segs: Vec<WorldSegmentPos> = Vec::new();
    let mut per_seg_detail: Vec<String> = Vec::new();

    for seg in segs {
        let local = seg.0 - origin;
        if local.x < 0
            || local.y < 0
            || local.z < 0
            || local.x >= crate::WORLD_SIZE_IN_SEGMENTS.x as i32
            || local.y >= crate::WORLD_SIZE_IN_SEGMENTS.y as i32
            || local.z >= crate::WORLD_SIZE_IN_SEGMENTS.z as i32
        {
            // World segment outside the window — should not happen for
            // camera-row segments at cold-start (window centred on camera).
            per_seg_detail.push(format!(
                "  - seg{:?}: SKIP — outside window (local {:?})",
                seg.0, local
            ));
            continue;
        }
        let pack = (local.x as u32)
            + (local.y as u32) * crate::WORLD_SIZE_IN_SEGMENTS.x
            + (local.z as u32)
                * crate::WORLD_SIZE_IN_SEGMENTS.x
                * crate::WORLD_SIZE_IN_SEGMENTS.y;
        if (pack as usize) >= indirection.len() {
            per_seg_detail.push(format!(
                "  - seg{:?}: SKIP — pack {} out of indirection bounds ({})",
                seg.0,
                pack,
                indirection.len(),
            ));
            continue;
        }
        let slot = indirection[pack as usize];
        if slot == EMPTY_SLOT {
            per_seg_detail.push(format!(
                "  - seg{:?}: EMPTY — indirection[pack={}] = EMPTY_SLOT \
                 (segment was never bound)",
                seg.0, pack
            ));
            empty_segs.push(*seg);
            continue;
        }
        let slot_offset = (slot * CHUNKS_PER_SLOT) as usize;
        let mut non_empty_chunks = 0u32;
        for i in 0..CHUNKS_PER_SLOT as usize {
            let chunk_idx = slot_offset + i;
            // `chunks_buffer` is `array<vec2<u32>>` flat layout —
            // chunk_idx * 2 = .x, chunk_idx * 2 + 1 = .y.
            let chunk_x_pos = chunk_idx * 2;
            if chunk_x_pos >= chunks.len() {
                break;
            }
            let x = chunks[chunk_x_pos];
            let state = x >> 30;
            if state != BLOCK_STATE_UNIFORM_EMPTY {
                non_empty_chunks += 1;
                // Don't need to count exhaustively; one is enough to
                // pass the per-segment assertion. Break early.
                break;
            }
        }
        if non_empty_chunks == 0 {
            per_seg_detail.push(format!(
                "  - seg{:?}: EMPTY — slot {} bound but all 4096 chunks \
                 decode as UNIFORM_EMPTY (= cold-start gap bug — slot \
                 marked dispatched_once but no chunk_calc fired)",
                seg.0, slot
            ));
            empty_segs.push(*seg);
        } else {
            per_seg_detail.push(format!(
                "  - seg{:?}: OK — slot {} has at least 1 non-EMPTY chunk",
                seg.0, slot
            ));
        }
    }

    let detail = per_seg_detail.join("\n");
    (empty_segs, total, detail)
}

/// Run the cold-start gate assertion against the captured snapshot.
/// Returns `Ok(report)` on success, `Err(msg)` on failure.
pub fn assert_streaming_cold_start_landed(args: &crate::AppArgs) -> Result<String, String> {
    let Some(chunks) = super::streaming_aadf_parity::take_snapshot() else {
        return Err(
            "streaming-cold-start: no chunks_buffer snapshot captured — \
             the readback system never fired. Likely cause: the warmup \
             never reached the snapshot trigger phase within the gate \
             budget."
                .to_string(),
        );
    };
    let Some(indirection) = super::streaming_aadf_parity::take_indirection_snapshot() else {
        return Err(
            "streaming-cold-start: no indirection snapshot captured — \
             the streaming preset's window_indirection_buffer was not \
             allocated. Likely cause: streaming preset not active."
                .to_string(),
        );
    };
    let cam_seg = camera_spawn_segment(args.sea_level);
    // Hard-pin origin at (0, 0, 0) — the streaming preset's
    // `target_origin_for_camera_seg` centres the window on the camera,
    // and cold-start hasn't shifted the origin yet (no segment crossing).
    // For the default seed pose at world segment (8, 1, 8) and window
    // size (16, 2, 16), the initial origin is (0, 0, 0). The walk in
    // the streaming-window gate is the FIRST event that shifts the
    // origin — this cold-start gate disables the walk, so the origin
    // stays at install-time (0, 0, 0).
    let origin = IVec3::ZERO;
    // dsq <= 2 — 14 segments centred on the camera spawn. This is the
    // failing set diagnosed in `03r` § Cold-start admission lifecycle.
    let segs = camera_row_segments(cam_seg, 2);
    let (empty_segs, total, detail) =
        validate_cold_start_content(&chunks, &indirection, origin, &segs);

    let report = format!(
        "streaming-cold-start: cam_seg={:?}, origin={:?}, inspected \
         {} camera-row segments (dsq ≤ 2 ring), empty_segments={}/{}. \
         Per-segment detail:\n{}",
        cam_seg.0, origin, total, empty_segs.len(), total, detail,
    );
    println!("e2e_render --gate streaming-cold-start: {report}");

    if empty_segs.len() > STREAMING_COLD_START_MAX_EMPTY_SEGMENTS {
        let empty_list: Vec<String> = empty_segs
            .iter()
            .map(|s| format!("{:?}", s.0))
            .collect();
        Err(format!(
            "streaming-cold-start gate FAIL — {} camera-row segments \
             have ZERO non-empty chunks (max allowed: {}). Empty: [{}]. \
             This is the cold-start admission-race bug diagnosed in \
             `03r-diagnosis-cold-start-gap.md` (slots marked \
             `dispatched_once` before the render-world producer node \
             actually submitted the chunk_calc dispatch; the slots' \
             chunks_buffer regions were zeroed by clear-on-bind but \
             never written, leaving sky-coloured holes in the \
             camera-nearest segments). {}",
            empty_segs.len(),
            STREAMING_COLD_START_MAX_EMPTY_SEGMENTS,
            empty_list.join(", "),
            report,
        ))
    } else {
        Ok(format!("streaming-cold-start gate PASS — {}", report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Camera-row computation: cam_seg=(8,1,8), max_dsq=2 should yield
    /// 6 dsq≤1 + 8 dsq=2 = 14 segments, all in-window.
    #[test]
    fn camera_row_segments_dsq2_yields_14() {
        let cam = WorldSegmentPos(IVec3::new(8, 1, 8));
        let segs = camera_row_segments(cam, 2);
        // dsq=0: (8,1,8) — 1
        // dsq=1: (7,1,8) (9,1,8) (8,0,8) (8,2,8 — OOB y=2) (8,1,7) (8,1,9) — 5 in-window
        // dsq=2: (7,0,8) (9,0,8) (7,2,8 OOB) (9,2,8 OOB) (8,0,7) (8,0,9) (8,2,7 OOB) (8,2,9 OOB)
        //        (7,1,7) (7,1,9) (9,1,7) (9,1,9) — 8 in-window
        // Total = 1 + 5 + 8 = 14
        assert_eq!(segs.len(), 14, "expected 14 camera-row segments at dsq ≤ 2");
        // Cam's own segment must be in the set.
        assert!(segs.iter().any(|s| s.0 == cam.0), "cam segment must be in the set");
        // All segments must be in-bounds.
        for s in &segs {
            assert!(s.0.x >= 0 && s.0.x < crate::WORLD_SIZE_IN_SEGMENTS.x as i32);
            assert!(s.0.y >= 0 && s.0.y < crate::WORLD_SIZE_IN_SEGMENTS.y as i32);
            assert!(s.0.z >= 0 && s.0.z < crate::WORLD_SIZE_IN_SEGMENTS.z as i32);
        }
    }

    /// Camera spawn segment for default sea_level matches the
    /// `03r-diagnosis-cold-start-gap.md` § Task A finding (8, 1, 8).
    #[test]
    fn camera_spawn_segment_default_sea_level() {
        let seg = camera_spawn_segment(256.0);
        assert_eq!(seg.0, IVec3::new(8, 1, 8));
    }

    /// Identity indirection where slot N = local pack-index N — sanity
    /// helper.
    fn identity_indirection() -> Vec<u32> {
        (0..(crate::WORLD_SIZE_IN_SEGMENTS.x
            * crate::WORLD_SIZE_IN_SEGMENTS.y
            * crate::WORLD_SIZE_IN_SEGMENTS.z))
            .collect()
    }

    /// All-empty chunks_buffer: every camera-row segment FAILS the
    /// non-empty check.
    #[test]
    fn validate_cold_start_content_catches_all_empty() {
        let total_chunks = 512usize * 4096;
        let chunks = vec![0u32; total_chunks * 2];
        let indirection = identity_indirection();
        let segs = camera_row_segments(WorldSegmentPos(IVec3::new(8, 1, 8)), 2);
        let (empty, total, _detail) =
            validate_cold_start_content(&chunks, &indirection, IVec3::ZERO, &segs);
        assert_eq!(empty.len(), total, "all empty must mark all 14 segments empty");
        assert_eq!(total, 14);
    }

    /// Seed one chunk per camera-row segment as non-empty (state = 1 =
    /// UNIFORM_FULL): every segment now PASSES.
    #[test]
    fn validate_cold_start_content_one_non_empty_chunk_per_seg_passes() {
        const SEG_X: u32 = 16;
        const SEG_Y: u32 = 2;
        let total_chunks = 512usize * 4096;
        let mut chunks = vec![0u32; total_chunks * 2];
        let indirection = identity_indirection();
        let cam = WorldSegmentPos(IVec3::new(8, 1, 8));
        let segs = camera_row_segments(cam, 2);
        for seg in &segs {
            let local = seg.0 - IVec3::ZERO;
            let pack = (local.x as u32) + (local.y as u32) * SEG_X
                + (local.z as u32) * SEG_X * SEG_Y;
            let slot = indirection[pack as usize];
            // chunk 0 within slot — state = 1 = UNIFORM_FULL.
            let chunk_idx = (slot * 4096) as usize;
            chunks[chunk_idx * 2] = 1u32 << 30;
        }
        let (empty, total, _detail) =
            validate_cold_start_content(&chunks, &indirection, IVec3::ZERO, &segs);
        assert_eq!(empty.len(), 0, "one non-empty chunk per seg must pass");
        assert_eq!(total, 14);
    }

    /// EMPTY_SLOT indirection — segment counts as empty (the slot was
    /// never bound, e.g. eviction-then-no-rebind). Catches a regression
    /// where the producer never ran for a slot at all.
    #[test]
    fn validate_cold_start_content_catches_unbound_segment() {
        let total_chunks = 512usize * 4096;
        let chunks = vec![0u32; total_chunks * 2];
        let mut indirection = identity_indirection();
        // EMPTY_SLOT-out one camera-row segment's pack-index.
        let cam = WorldSegmentPos(IVec3::new(8, 1, 8));
        let local = cam.0 - IVec3::ZERO;
        let pack = (local.x as u32)
            + (local.y as u32) * crate::WORLD_SIZE_IN_SEGMENTS.x
            + (local.z as u32)
                * crate::WORLD_SIZE_IN_SEGMENTS.x
                * crate::WORLD_SIZE_IN_SEGMENTS.y;
        indirection[pack as usize] = u32::MAX;
        let segs = vec![cam];
        let (empty, total, _detail) =
            validate_cold_start_content(&chunks, &indirection, IVec3::ZERO, &segs);
        assert_eq!(empty.len(), 1, "unbound seg must register as empty");
        assert_eq!(total, 1);
    }
}
