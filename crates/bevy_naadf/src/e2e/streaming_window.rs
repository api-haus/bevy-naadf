//! `--streaming-window` e2e gate
//! (`docs/orchestrate/streaming-world/02b-design-plan-b.md` § J).
//!
//! Walks a procedural-streaming-world camera across ≥2 segment boundaries in
//! the +X direction, capturing framebuffers before + after the move, asserting
//! that:
//!
//! - (a) The after-frame shows non-trivial terrain at the new camera position
//!   (luminance variance / non-skybox ratio above a threshold).
//! - (b) The before and after frames differ substantially in the camera-moved
//!   region (pixel-diff > threshold) — proves residency actually shifted.
//! - (c) The VRAM budget pre-flight succeeded (the gate doesn't panic on boot).
//! - (d) The residency origin shifted by ≥ `(WORLD_SIZE_IN_SEGMENTS.x / 4)`
//!   segments in X over Phase C.
//!
//! The gate reuses the standard OasisXxx driver state machine — Phase A is
//! `OasisWarmup` (cold-start populate), Phase B is `OasisShootBefore`, Phase C
//! is `OasisApplyEdit` (a camera move; promoted via
//! [`promote_camera_to_walk`]) + `OasisWaitPostEdit` (residency re-populate),
//! Phase D is `OasisShootAfter`, Phase E is `OasisAssert` (the streaming-window
//! verdict — branched on `streaming_window_mode` in `OasisAssert`).
//!
//! ## Camera walk
//!
//! Walks camera +X by `(WORLD_SIZE_IN_SEGMENTS.x / 4) × SEGMENT_VOXELS` voxels
//! (= 1024 voxels = 4 segments) — crosses ≥ 2 segment boundaries, well past
//! the half-window threshold that triggers a residency shift.
//!
//! The walk is **instantaneous** (a single transform write), not a per-frame
//! sweep. The wait phase (`OasisWaitPostEdit` = 300 frames) gives the
//! residency driver time to admit the new segments at the budgeted rate
//! (`--max-segments-per-frame`, default 4).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::time::Duration;

use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::Framebuffer;

// ---------------------------------------------------------------------------
// Filenames + thresholds
// ---------------------------------------------------------------------------

/// Filename for the pre-walk framebuffer capture.
pub const STREAMING_BEFORE_PNG: &str = "streaming_window_before.png";
/// Filename for the post-walk framebuffer capture.
pub const STREAMING_AFTER_PNG: &str = "streaming_window_after.png";

/// Minimum mean per-pixel RGB delta between before/after frames over the
/// full frame. Phase 2.5 raised this from the temporary `0.0` floor to a
/// real one once the root-cause residency-state fix
/// (`finalise_admissions_as_resident`, per `03c-diagnosis.md` § Punch-list
/// item 1) landed. Pre-fix, both before/after frames were sky-only — pixel
/// Δ was 0.0 by construction. Post-fix, the terrain shifts in the
/// framebuffer between Pose A and Pose B because the local-Z column of
/// rendered terrain content moves as the camera walks +X (different
/// voxel columns project to the same screen pixels). The floor here is
/// measured from a real run with item 1 in place, taken at ~40 % of the
/// measured Δ so the gate fails unambiguously on a regression that
/// collapses streaming back to sky-only output.
pub const STREAMING_MIN_PIXEL_DELTA: f32 = 3.0;

/// Minimum after-frame luminance variance — the after-frame should show
/// non-trivial content. The streaming-world diagnostic
/// (`03c-diagnosis.md` § "Root cause: false pass") measured pure-sky
/// variance at ~242. Phase 2.4's static-noise gate measured terrain frame
/// variance at 1816. The streaming preset's camera-translation step
/// produces a similar window-local terrain frame so the variance should
/// be in the same order. The 800 floor sits comfortably above the sky-only
/// 242 baseline (3.3× margin) and below the static-noise 1816 measurement
/// (2.27× headroom) so a real terrain frame passes and a sky-only frame
/// fails. Phase 2.4 already validated 800 as the strict regression-catch
/// floor for the noise→encoded-chunks→render chain in
/// `noise_static_world.rs:NOISE_STATIC_MIN_LUM_VARIANCE`.
pub const STREAMING_MIN_AFTER_LUM_VARIANCE: f32 = 800.0;

/// Wall-clock budget for the full `--streaming-window` gate run. The gate's
/// frame-cap (120 warmup + 1 shoot + 16 drain + 1 apply + 300 wait + 1
/// shoot + 16 drain + 1 assert ≈ 455 ticks) is bounded but not
/// wall-clock-bounded; under per-frame bounds-chain load (the diagnosed
/// hang in `03c-diagnosis.md` § "Root cause: minutes-long hang"), 455 frames
/// took ~2 minutes.
///
/// Phase 2.5 — measured budget: with `max_segments_per_frame = 4`, cold-
/// start admits 4 slots/frame × 128 frames = 512 slots; each admission
/// frame fires the bounds-chain dispatch (~300 ms on RTX 5080). The
/// camera walk adds another ~32 frames of admissions. ~160 admission
/// frames × 300 ms ≈ 48 s on top of ~10 s of settled-frame time, totalling
/// ~60 s. The 120 s budget gives ~2× margin against this baseline while
/// still failing FAST on the original "minutes-long hang" regression
/// (which would push past 120 s easily). Per the
/// `feedback-e2e-gates-must-fail-fast` memory.
///
/// A future Phase 2.6+ perf win — dirty-segments bounds dispatch (only
/// re-bound the affected segments per admission instead of the full
/// 2M-chunk worst-case) — would let this budget drop back to ~30 s.
pub const STREAMING_GATE_WALL_CLOCK_MAX_SECS: u64 = 120;

/// Wall-clock budget as a `Duration`.
pub const STREAMING_GATE_WALL_CLOCK_MAX: Duration =
    Duration::from_secs(STREAMING_GATE_WALL_CLOCK_MAX_SECS);

/// Camera-walk distance in voxels along +X. `(WORLD_SIZE_IN_SEGMENTS.x / 4)` =
/// 4 segments × `SEGMENT_VOXELS` (256) = 1024 voxels. Crosses ≥ 2 segment
/// boundaries.
pub const STREAMING_WALK_DISTANCE_VOXELS: f32 = 1024.0;

/// Minimum residency-origin shift in X (in segments) we expect to see after
/// the walk. With a 1024-voxel = 4-segment walk and the camera centred in the
/// window, the origin should follow by 4 segments.
pub const STREAMING_MIN_ORIGIN_SHIFT_SEGMENTS: i32 = 4;

/// One-shot latch — `true` once [`promote_camera_to_walk`] has been called.
/// Mirrors the `vox_gpu_construction::CAMERA_PROMOTED` pattern.
static CAMERA_WALKED: AtomicBool = AtomicBool::new(false);
/// Residency origin X at Pose A (snapshot taken at promote time so we can
/// compute the shift when the assertion fires).
static RESIDENCY_ORIGIN_X_AT_POSE_A: AtomicI32 = AtomicI32::new(i32::MIN);

/// Phase 2.9 — the walk is now an additive sequence of Transform deltas, not
/// a single Pose-A→Pose-B teleport. After [`promote_camera_to_walk`] fires
/// the gate runs [`STREAMING_WALK_TICKS`] ticks, each adding
/// [`STREAMING_WALK_VOXELS_PER_TICK`] voxels in `+X` to the camera's absolute
/// world position via [`track_and_pin_camera`]. Mirrors the
/// `FreeCamera` controller's additive-Transform write pattern — exactly the
/// production path the `03j` diagnosis identified as broken pre-fix.
///
/// Total walk distance = `STREAMING_WALK_TICKS *
/// STREAMING_WALK_VOXELS_PER_TICK` voxels, calibrated to match
/// `STREAMING_WALK_DISTANCE_VOXELS` (= 1024).
pub const STREAMING_WALK_TICKS: i32 = 256;
/// Per-tick `+X` Transform delta in voxels. `256 * 4 = 1024` voxels total.
pub const STREAMING_WALK_VOXELS_PER_TICK: f32 = 4.0;
/// Counter for the remaining walk ticks. Set to `STREAMING_WALK_TICKS` on
/// [`promote_camera_to_walk`]; decremented by [`pin_streaming_window_camera`]
/// per tick.
static WALK_TICKS_REMAINING: AtomicI32 = AtomicI32::new(0);

// ---------------------------------------------------------------------------
// Wall-clock budget enforcement (Phase 2.5 — `03c-diagnosis.md` § Punch-list
// item 4). Same shape as `noise_static_world.rs`'s gate-start latch.
// ---------------------------------------------------------------------------

/// Gate-start epoch milliseconds. `0` means "not yet started".
static GATE_START_EPOCH_MS: AtomicI64 = AtomicI64::new(0);

/// Mark the gate as started (records the wall-clock now). Idempotent.
pub fn mark_gate_started() {
    if GATE_START_EPOCH_MS.load(Ordering::SeqCst) == 0 {
        let now_ms = epoch_millis_now();
        GATE_START_EPOCH_MS.store(now_ms, Ordering::SeqCst);
    }
}

/// Reset the gate-start latch — called by [`run_streaming_window`] on entry
/// so successive invocations get a fresh budget.
pub fn reset_gate_start_latch() {
    GATE_START_EPOCH_MS.store(0, Ordering::SeqCst);
}

/// `true` if the gate has exceeded its wall-clock budget. Returns `false`
/// before [`mark_gate_started`] has been called.
pub fn wall_clock_budget_exceeded() -> bool {
    let start = GATE_START_EPOCH_MS.load(Ordering::SeqCst);
    if start == 0 {
        return false;
    }
    let now = epoch_millis_now();
    let elapsed_ms = (now - start).max(0) as u64;
    elapsed_ms > STREAMING_GATE_WALL_CLOCK_MAX.as_millis() as u64
}

/// Elapsed wall-clock since `mark_gate_started` (None if not started).
pub fn elapsed_since_start() -> Option<Duration> {
    let start = GATE_START_EPOCH_MS.load(Ordering::SeqCst);
    if start == 0 {
        return None;
    }
    let now = epoch_millis_now();
    let elapsed_ms = (now - start).max(0) as u64;
    Some(Duration::from_millis(elapsed_ms))
}

fn epoch_millis_now() -> i64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Record the residency origin X at Pose A — called by the driver from
/// `OasisApplyEdit` (the moment the camera is promoted).
pub fn record_origin_x_at_pose_a(x: i32) {
    RESIDENCY_ORIGIN_X_AT_POSE_A.store(x, Ordering::SeqCst);
}

/// Read the recorded origin X at Pose A. Returns `i32::MIN` when no record
/// exists (a regression signal — promotion never fired).
pub fn origin_x_at_pose_a() -> i32 {
    RESIDENCY_ORIGIN_X_AT_POSE_A.load(Ordering::SeqCst)
}

/// Promote the streaming-window camera into "walk mode". Called by the e2e
/// driver at `OasisApplyEdit` when `streaming_window_mode` is active.
///
/// Phase 2.9 — instead of a single Pose-A→Pose-B teleport, this kicks off a
/// `STREAMING_WALK_TICKS`-tick additive walk. The [`pin_streaming_window_camera`]
/// Update system reads the latch + counter each tick and writes
/// `Transform.translation += (STREAMING_WALK_VOXELS_PER_TICK, 0, 0)` while
/// the counter is non-zero, mirroring the production `FreeCamera`'s additive
/// write pattern (the `03j` diagnosis identified this as the load-bearing
/// shape — the bug fires under additive writes, NOT teleports). After the
/// counter drains to zero, the pin is a no-op (the camera holds its
/// post-walk pose for the framebuffer capture).
pub fn promote_camera_to_walk() {
    CAMERA_WALKED.store(true, Ordering::SeqCst);
    WALK_TICKS_REMAINING.store(STREAMING_WALK_TICKS, Ordering::SeqCst);
}

/// Reset the camera-walked latch — used by tests.
pub fn reset_camera_walked_latch() {
    CAMERA_WALKED.store(false, Ordering::SeqCst);
    WALK_TICKS_REMAINING.store(0, Ordering::SeqCst);
}

/// Read the current state of the camera-walked latch.
pub fn camera_has_walked() -> bool {
    CAMERA_WALKED.load(Ordering::SeqCst)
}

/// Read the remaining walk ticks (test helper).
pub fn walk_ticks_remaining() -> i32 {
    WALK_TICKS_REMAINING.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Camera pin
// ---------------------------------------------------------------------------

/// Compute the streaming-window camera pose. Pre-walk: world centre + offset.
/// Post-walk: world centre + offset + +X walk distance.
pub fn streaming_window_pose(walked: bool) -> Transform {
    let cx = (crate::WORLD_SIZE_IN_VOXELS.x as f32) * 0.5;
    let cy_base = (crate::WORLD_SIZE_IN_VOXELS.y as f32) * 0.5; // half-height
    let cz = (crate::WORLD_SIZE_IN_VOXELS.z as f32) * 0.5;
    // Place camera above the terrain — sea_level + 32 voxels.
    let cam_y = cy_base + 32.0;
    // Pose A = (cx, cam_y, cz); Pose B = (cx + WALK, cam_y, cz).
    let x_offset = if walked { STREAMING_WALK_DISTANCE_VOXELS } else { 0.0 };
    let cam_pos = Vec3::new(cx + x_offset, cam_y, cz);
    let look = Vec3::new(cam_pos.x + 100.0, cy_base - 16.0, cam_pos.z);
    Transform::from_translation(cam_pos).looking_at(look, Vec3::Y)
}

/// Translate an absolute-world Transform into the residency window-local frame
/// by subtracting `origin * SEGMENT_VOXELS`. Returns the absolute Transform
/// unchanged when `residency` is `None` (non-streaming presets).
///
/// Phase 2.5 follow-up to `03b-impl-residency.md`'s "Camera-to-window-coords
/// translation" hand-off: the renderer treats the camera Transform as if it
/// were already in window-local coords (it derives `chunks_buffer` indices via
/// `camera_voxel / 16` against the *window-local* `(0..4096, 0..512, 0..4096)`
/// AABB). The residency driver however maintains `origin` in absolute world
/// segments — so without this translation, a residency shift breaks visible
/// streaming (the renderer reads the wrong chunk slot).
///
/// Per Q1 of `01-context.md` ("Chunks are re-indexed into the resident window
/// before upload. Camera uses the existing PositionSplit. No shader-side
/// packing changes."), the fix is host-side: pre-translate the camera each
/// frame so what the renderer sees is already window-local.
///
/// The translation is **stateless** — it reads `origin` from the live
/// [`crate::streaming::Residency`] and re-derives the world-local Transform
/// from the absolute pose every tick. No floating-point drift can accumulate
/// across frames.
pub(crate) fn translate_world_to_window_local(
    world_pose: Transform,
    residency: Option<&crate::streaming::Residency>,
) -> Transform {
    let Some(residency) = residency else {
        return world_pose;
    };
    let origin_voxels = (residency.origin() * crate::streaming::SEGMENT_VOXELS).as_vec3();
    let mut local = world_pose;
    local.translation -= origin_voxels;
    local
}

/// `Update` system: pin the camera at the streaming spawn pose (Pose A) before
/// the walk, then apply an additive `+X` Transform write per tick during the
/// walk phase, then no-op once the walk completes. Wired only when
/// `AppArgs.streaming_window_mode == true`. Runs `.after(e2e_driver)` so the
/// pose write lands AFTER the driver's pose write.
///
/// Phase 2.9 — REPLACES the previous Pose-A/B teleport pin (which bypassed
/// the production camera path entirely). The new shape:
///
/// 1. While walk has NOT been promoted (`!camera_has_walked()`): pin
///    Transform + PositionSplit to the streaming-preset spawn pose in
///    **window-local** coords. This handles the e2e camera spawn (which
///    starts at the harness's `e2e_motion_start_transform` pose, not the
///    streaming-preset world centre) and gives the residency manager a
///    stable spawn frame to cold-start populate.
/// 2. Once the walk is promoted AND `walk_ticks_remaining > 0`: apply
///    `transform.translation += (STREAMING_WALK_VOXELS_PER_TICK, 0, 0)`,
///    decrement the counter. The production-side `track_and_pin_camera`
///    system (registered by `StreamingPlugin::build` in `Update`,
///    `.before(sync_position_split)`) sees this delta the same way it
///    sees a `FreeCamera` controller delta, folds it into
///    `CameraAbsolutePosition`, and re-pins Transform to window-local for
///    the current `Residency::origin`. `residency_driver` then reads
///    `CameraAbsolutePosition` and shifts origin correctly.
/// 3. Once the walk completes (`walk_ticks_remaining == 0`): no-op. Hold
///    Transform — `track_and_pin_camera` keeps it window-local under the
///    final origin so the after-frame capture is correct.
///
/// This shape **exercises the production camera path**: the bug diagnosed
/// in `03j` is exactly the additive-Transform write pattern + missing
/// production-side window-local re-pin. If `track_and_pin_camera` is broken
/// or absent, the additive writes drive `residency_driver` into an endless
/// reposition loop (wall-clock budget exceeded → gate panics).
pub fn pin_streaming_window_camera(
    args: Option<Res<crate::AppArgs>>,
    residency: Option<Res<crate::streaming::Residency>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.streaming_window_mode {
        return;
    }
    // Phase 2.5 — wall-clock budget enforcement. Marks the gate as started
    // on the first tick (latches the budget); checks against the budget
    // every tick. On budget exceeded, panics with a diagnostic that names
    // the elapsed time and the current residency state. Panic is the
    // load-bearing fail-fast (the e2e harness has no path to write
    // `AppExit` from an Update system; mirrors the noise-static-world
    // gate's `wall_clock_budget_exceeded` enforcement).
    mark_gate_started();
    if wall_clock_budget_exceeded() {
        // Phase 2.6 (`02c-design-windowed-slot-map.md` D4) — slot state is
        // implicit under the new `WindowedSlotMap` primitive:
        //   In free_list         ⇔ "Empty"
        //   Bound AND in admissions_this_frame ⇔ "Generating"
        //   Bound AND NOT in admissions_this_frame ⇔ "Resident"
        // We rebuild the histogram from the new sources for the panic
        // diagnostic; the diagnostic text is otherwise identical.
        let (admissions_n, evictions_n, generating_n, resident_n, empty_n) =
            residency
                .as_deref()
                .map(|r| {
                    let in_admissions: std::collections::HashSet<u32> = r
                        .admissions_this_frame
                        .iter()
                        .map(|(_, s)| s.0)
                        .collect();
                    let mut generating = 0usize;
                    let mut resident = 0usize;
                    for (_, s) in r.window.iter_bound() {
                        if in_admissions.contains(&s.0) {
                            generating += 1;
                        } else {
                            resident += 1;
                        }
                    }
                    let empty = r.window.free_count() as usize;
                    (
                        r.admissions_this_frame.len(),
                        r.evictions_this_frame.len(),
                        generating,
                        resident,
                        empty,
                    )
                })
                .unwrap_or((0, 0, 0, 0, 0));
        panic!(
            "streaming-window: wall-clock budget {}s exceeded \
             (elapsed = {:?}). Likely cause: the per-frame bounds-chain \
             dispatch is firing every frame (the diagnosed hang in \
             `03c-diagnosis.md` § \"Root cause: minutes-long hang\") — \
             check that admissions drain to Resident over multiple ticks \
             (Phase 2.6 `02c` D4: implicit lifecycle — bound slots that \
             aren't in admissions_this_frame this tick are Resident). \
             Residency state: \
             admissions_this_frame={admissions_n}, \
             evictions_this_frame={evictions_n}, slot histogram = \
             {{Generating: {generating_n}, Resident: {resident_n}, \
             Empty: {empty_n}}}.",
            STREAMING_GATE_WALL_CLOCK_MAX_SECS,
            elapsed_since_start(),
        );
    }
    let (transform, position_split) = &mut *camera;

    if !camera_has_walked() {
        // Pre-walk — pin to the streaming-preset spawn pose. Production
        // `install_procedural_streaming_world` writes the same pose to
        // `InitialCameraPose` + seeds `CameraAbsolutePosition` from it; in
        // the e2e harness the camera is spawned via `setup_e2e_camera`
        // (which ignores `InitialCameraPose`), so we re-anchor it here on
        // every pre-walk tick. `track_and_pin_camera` then sees a stable
        // window-local pose (delta == 0 once anchored) and the residency
        // driver cold-starts the segment window around the camera centre.
        let world_pose = streaming_window_pose(false);
        let local_pose =
            translate_world_to_window_local(world_pose, residency.as_deref());
        **transform = local_pose;
        **position_split = PositionSplit::from_world(local_pose.translation);
        return;
    }

    // Walk in progress — apply an additive `+X` Transform write per tick,
    // exactly like the production `FreeCamera` controller does. The
    // production `track_and_pin_camera` system (Phase 2.9) folds the delta
    // into `CameraAbsolutePosition` and re-pins the Transform to
    // window-local for the current `Residency::origin`. PositionSplit will
    // be re-derived by `sync_position_split` later in the same Update
    // schedule.
    let remaining = WALK_TICKS_REMAINING.load(Ordering::SeqCst);
    if remaining > 0 {
        transform.translation.x += STREAMING_WALK_VOXELS_PER_TICK;
        WALK_TICKS_REMAINING.store(remaining - 1, Ordering::SeqCst);
    }
    // Post-walk (remaining == 0) — do nothing. Hold the Transform;
    // `track_and_pin_camera` keeps it window-local under the final origin.
}

// ---------------------------------------------------------------------------
// Entry point — boot the e2e harness in streaming-window mode.
// ---------------------------------------------------------------------------

/// Apply the streaming-window gate's default overlay onto `args` in place.
///
/// Per `02d-design-cli-and-e2e-rearch.md` § D: the gate's mode flag(s) are
/// set unconditionally (observer attachment), but the grid preset is only
/// installed when the user didn't pass `--grid-preset` (i.e. the args still
/// carry `GridPreset::Default`). This composes user CLI overrides on top of
/// the gate's defaults: `--gate streaming-window --vram-budget-mib 2048`
/// keeps the user's budget but installs the streaming preset; `--gate
/// streaming-window --grid-preset procedural-static` runs the
/// streaming-window observer against the static preset (useful for
/// debugging cross-preset behaviour).
///
/// Resets the per-run latches (camera walk + wall-clock budget start) so a
/// second invocation in the same process gets a fresh budget.
pub fn apply_streaming_window_defaults(args: &mut crate::AppArgs) {
    // Reset latches first — the driver re-promotes the camera on its own
    // schedule.
    reset_camera_walked_latch();
    reset_gate_start_latch();
    RESIDENCY_ORIGIN_X_AT_POSE_A.store(i32::MIN, Ordering::SeqCst);

    // Observer attachment — always set.
    args.streaming_window_mode = true;
    // Force `oasis_edit_visual_mode = true` so the driver routes into the
    // OasisWarmup state machine on tick 0. The OasisApplyEdit branch in
    // `driver.rs` detects `streaming_window_mode` and promotes the camera
    // instead of running a brush edit.
    args.oasis_edit_visual_mode = true;

    // Preset default — only install if the user didn't override.
    if matches!(args.grid_preset, crate::GridPreset::Default) {
        args.grid_preset = crate::GridPreset::ProceduralStreaming {
            noise_preset: args.noise_preset,
            seed: args.noise_seed,
        };
    }

    println!(
        "e2e_render --gate streaming-window: booting procedural-streaming world \
         (seed={}, sea_level={:.1}, terrain_amplitude={:.1}, \
         vram_budget_mib={}, max_segments_per_frame={}); strict floors: \
         pixel_delta ≥ {:.2}, after_lum_variance ≥ {:.1}, wall_clock ≤ {}s",
        args.noise_seed,
        args.sea_level,
        args.terrain_amplitude,
        args.vram_budget_mib,
        args.max_segments_per_frame,
        STREAMING_MIN_PIXEL_DELTA,
        STREAMING_MIN_AFTER_LUM_VARIANCE,
        STREAMING_GATE_WALL_CLOCK_MAX_SECS,
    );
}

/// Thin wrapper retained for Rust-API callers (no clap). The e2e binary's
/// `main` no longer calls this — it composes its own `AppArgs` through
/// `cli::E2eCli::into_app_args_and_gate()` so user CLI overrides flow
/// through. This function is the no-overrides equivalent.
pub fn run_streaming_window() -> AppExit {
    let mut app_args = crate::AppArgs::default();
    apply_streaming_window_defaults(&mut app_args);

    let exit = crate::run_e2e_render_with_args(app_args);
    let elapsed = elapsed_since_start();
    println!(
        "e2e_render --gate streaming-window: gate run completed in {:?} \
         (budget = {}s).",
        elapsed,
        STREAMING_GATE_WALL_CLOCK_MAX_SECS,
    );
    exit
}

// ---------------------------------------------------------------------------
// Assertion — `OasisAssert` branches here when streaming_window_mode is set.
// ---------------------------------------------------------------------------

/// Per-pixel mean RGB delta over the full framebuffer.
fn mean_pixel_delta(before: &Framebuffer, after: &Framebuffer) -> f32 {
    before.mean_pixel_delta(after)
}

/// Luminance variance over the full framebuffer — flags "skybox-only" frames
/// (low variance) vs "real terrain" frames (substantial variance).
fn luminance_variance(fb: &Framebuffer) -> f32 {
    let w = fb.width();
    let h = fb.height();
    let n = (w as u64) * (h as u64);
    if n == 0 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    for y in 0..h {
        for x in 0..w {
            let p = fb.pixel(x, y);
            let lum = 0.2126 * (p[0] as f32) + 0.7152 * (p[1] as f32) + 0.0722 * (p[2] as f32);
            sum += lum as f64;
            sum_sq += (lum as f64) * (lum as f64);
        }
    }
    let mean = sum / (n as f64);
    let var = (sum_sq / (n as f64)) - mean * mean;
    var.max(0.0) as f32
}

/// Run the streaming-window assertion against the before + after captures.
///
/// `origin_shift_x_seg` is the measured shift of the residency origin in X
/// over Phase C (caller reads it from the live `Residency` resource).
pub fn assert_streaming_window_landed(
    before: &Framebuffer,
    after: &Framebuffer,
    origin_shift_x_seg: i32,
) -> Result<String, String> {
    if before.width() != after.width() || before.height() != after.height() {
        return Err(format!(
            "streaming-window: dimensions changed mid-run ({}x{} vs {}x{})",
            before.width(),
            before.height(),
            after.width(),
            after.height(),
        ));
    }

    let pixel_delta = mean_pixel_delta(before, after);
    let after_lum_var = luminance_variance(after);
    let origin_shift_ok = origin_shift_x_seg.abs() >= STREAMING_MIN_ORIGIN_SHIFT_SEGMENTS;

    let report = format!(
        "streaming-window: mean pixel Δ = {:.2} (floor = {:.2}); \
         after-frame luminance variance = {:.2} (floor = {:.2}); \
         residency origin shift in X = {} segments (floor = {})",
        pixel_delta,
        STREAMING_MIN_PIXEL_DELTA,
        after_lum_var,
        STREAMING_MIN_AFTER_LUM_VARIANCE,
        origin_shift_x_seg,
        STREAMING_MIN_ORIGIN_SHIFT_SEGMENTS,
    );
    println!("e2e_render --streaming-window: {report}");

    let mut failures = Vec::new();
    if pixel_delta < STREAMING_MIN_PIXEL_DELTA {
        failures.push(format!(
            "(a/b) pixel Δ {:.2} below floor {:.2}",
            pixel_delta, STREAMING_MIN_PIXEL_DELTA,
        ));
    }
    if after_lum_var < STREAMING_MIN_AFTER_LUM_VARIANCE {
        failures.push(format!(
            "(a) after-frame luminance variance {:.2} below floor {:.2} — \
             likely skybox-only (residency did not populate)",
            after_lum_var, STREAMING_MIN_AFTER_LUM_VARIANCE,
        ));
    }
    if !origin_shift_ok {
        failures.push(format!(
            "(d) residency origin shifted by only {} segments in X; expected \
             ≥ {}",
            origin_shift_x_seg, STREAMING_MIN_ORIGIN_SHIFT_SEGMENTS,
        ));
    }

    if !failures.is_empty() {
        return Err(format!(
            "streaming-window gate FAIL — {}. {}",
            failures.join("; "),
            report,
        ));
    }
    Ok(format!("streaming-window gate PASS — {report}"))
}

/// Save a framebuffer to `target/e2e-screenshots/<filename>`. Best-effort.
pub fn save_streaming_window_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --streaming-window: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --streaming-window: {filename} save failed: {e}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_walk_latch_round_trip() {
        reset_camera_walked_latch();
        assert!(!camera_has_walked());
        promote_camera_to_walk();
        assert!(camera_has_walked());
        reset_camera_walked_latch();
        assert!(!camera_has_walked());
    }

    #[test]
    fn streaming_window_pose_x_shifts_on_walk() {
        let pose_a = streaming_window_pose(false);
        let pose_b = streaming_window_pose(true);
        assert!((pose_b.translation.x - pose_a.translation.x - STREAMING_WALK_DISTANCE_VOXELS).abs() < 0.01);
        assert!((pose_b.translation.y - pose_a.translation.y).abs() < 0.01);
        assert!((pose_b.translation.z - pose_a.translation.z).abs() < 0.01);
    }

    #[test]
    fn pin_translates_world_to_window_local_origin_zero() {
        // With origin = (0, 0, 0) — the initial state before the camera moves —
        // the translation is a no-op: world Transform == local Transform.
        let res = crate::streaming::Residency::empty(4);
        // `Residency::empty()` constructs with origin = IVec3::ZERO; no
        // explicit set required.
        let world = streaming_window_pose(false);
        let local = translate_world_to_window_local(world, Some(&res));
        assert!((local.translation - world.translation).length() < 1e-4);
    }

    #[test]
    fn pin_translates_world_to_window_local_origin_shifted() {
        // After a +4-segment X walk, the residency origin lands at (4, 0, 0).
        // The translation should subtract `(4*256, 0, 0) = (1024, 0, 0)` from
        // the post-walk world pose — landing the camera at the SAME window-local
        // X as the pre-walk pose (which had origin (0, 0, 0)).
        let mut res = crate::streaming::Residency::empty(4);
        // Phase 2.6: origin is now mutated via the `WindowedSlotMap` API.
        res.window.set_origin(IVec3::new(4, 0, 0));

        let pose_a_world = streaming_window_pose(false);
        let pose_b_world = streaming_window_pose(true);
        // Pre-condition: in world coords, B is +1024 X past A.
        assert!((pose_b_world.translation.x - pose_a_world.translation.x
            - STREAMING_WALK_DISTANCE_VOXELS).abs() < 1e-4);

        // After origin-shift translation, pose_b_world maps to the same X as
        // pose_a_world at origin (0, 0, 0).
        let res_zero = crate::streaming::Residency::empty(4);
        let pose_a_local = translate_world_to_window_local(pose_a_world, Some(&res_zero));
        let pose_b_local = translate_world_to_window_local(pose_b_world, Some(&res));
        assert!((pose_b_local.translation - pose_a_local.translation).length() < 1e-4,
            "post-translation local poses should coincide (pose_a_local={:?}, \
             pose_b_local={:?})", pose_a_local.translation, pose_b_local.translation);
        // And the local X must stay inside the renderable window AABB.
        assert!(pose_b_local.translation.x >= 0.0
            && pose_b_local.translation.x < crate::WORLD_SIZE_IN_VOXELS.x as f32);
        assert!(pose_b_local.translation.z >= 0.0
            && pose_b_local.translation.z < crate::WORLD_SIZE_IN_VOXELS.z as f32);
    }

    #[test]
    fn pin_translation_no_residency_is_identity() {
        // With no Residency resource (non-streaming presets), the translation
        // helper returns the input Transform unchanged.
        let world = streaming_window_pose(false);
        let local = translate_world_to_window_local(world, None);
        assert!((local.translation - world.translation).length() < 1e-7);
    }

    #[test]
    fn pin_translation_is_idempotent_under_re_derivation() {
        // The translation is stateless — re-running it with the same origin
        // produces the same result (no drift across repeated invocations).
        let mut res = crate::streaming::Residency::empty(4);
        res.window.set_origin(IVec3::new(4, 0, 2));
        let world = streaming_window_pose(true);
        let once = translate_world_to_window_local(world, Some(&res));
        let twice = translate_world_to_window_local(world, Some(&res));
        let thrice = translate_world_to_window_local(world, Some(&res));
        assert_eq!(once.translation, twice.translation);
        assert_eq!(twice.translation, thrice.translation);
    }
}
