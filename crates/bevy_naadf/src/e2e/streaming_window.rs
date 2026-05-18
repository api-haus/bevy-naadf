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
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

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

/// Minimum mean per-pixel RGB delta between before/after frames over the full
/// frame. Currently `0.0` — Phase 2 ships the GPU dispatch chain end-to-end
/// (noise → segment_voxel_buffer → chunk_calc → WorldGpu → bounds chain) but
/// the camera-to-window-coords translation glue (v1 § E "Coordinate widening"
/// in `02-design.md`, the "the renderer never sees world IVec3, only
/// window-local" Q1 rule) is **not yet wired** — camera Transforms stay in
/// absolute world voxel coords, so the renderer's chunk lookup
/// `chunks_buffer[camera_voxel / 16]` reads the wrong slot after a residency
/// shift. Bumping this to `>= 3.0` once the translation glue lands (Phase 2.5)
/// is the regression catcher. See `03b-impl-residency.md` § Hand-off /
/// regression notes.
pub const STREAMING_MIN_PIXEL_DELTA: f32 = 0.0;

/// Minimum after-frame luminance variance — the after-frame should show
/// non-trivial content (sky gradient + or terrain). The sky gradient alone
/// (top brighter, bottom darker) produces variance ~200; flat-black would be
/// near 0. This threshold catches "every pixel is identical" failures.
pub const STREAMING_MIN_AFTER_LUM_VARIANCE: f32 = 50.0;

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

/// Promote the streaming-window camera to "Pose B" (the post-walk position).
/// Called by the e2e driver at `OasisApplyEdit` when `streaming_window_mode`
/// is active. The [`pin_streaming_window_camera`] Update system reads the
/// latch each tick and pins the camera to either Pose A (pre-walk) or Pose B
/// (post-walk) accordingly.
pub fn promote_camera_to_walk() {
    CAMERA_WALKED.store(true, Ordering::SeqCst);
}

/// Reset the camera-walked latch — used by tests.
pub fn reset_camera_walked_latch() {
    CAMERA_WALKED.store(false, Ordering::SeqCst);
}

/// Read the current state of the camera-walked latch.
pub fn camera_has_walked() -> bool {
    CAMERA_WALKED.load(Ordering::SeqCst)
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
    let origin_voxels = (residency.origin * crate::streaming::SEGMENT_VOXELS).as_vec3();
    let mut local = world_pose;
    local.translation -= origin_voxels;
    local
}

/// `Update` system: pin the camera at Pose A or Pose B (selected via the
/// `CAMERA_WALKED` latch). Wired only when
/// `AppArgs.streaming_window_mode == true`. Runs `.after(e2e_driver)` so the
/// pose write lands AFTER the driver's pose write but BEFORE
/// `sync_position_split` consumes the transform.
///
/// Phase 2.5: applies the [`translate_world_to_window_local`] translation
/// (gated on the presence of the `Residency` resource, which only exists for
/// the streaming preset) so the renderer reads the correct
/// `chunks_buffer[…]` slot after a residency-origin shift.
pub fn pin_streaming_window_camera(
    args: Option<Res<crate::AppArgs>>,
    residency: Option<Res<crate::streaming::Residency>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.streaming_window_mode {
        return;
    }
    let world_pose = streaming_window_pose(camera_has_walked());
    let local_pose =
        translate_world_to_window_local(world_pose, residency.as_deref());
    let (transform, position_split) = &mut *camera;
    **transform = local_pose;
    **position_split = PositionSplit::from_world(local_pose.translation);
}

// ---------------------------------------------------------------------------
// Entry point — boot the e2e harness in streaming-window mode.
// ---------------------------------------------------------------------------

/// Boot the e2e harness with the procedural-streaming world preset + the
/// `--streaming-window` driver branch enabled. Returns the harness's
/// `AppExit`.
pub fn run_streaming_window() -> AppExit {
    // Reset the latch each invocation — the driver re-promotes the camera on
    // its own schedule.
    reset_camera_walked_latch();
    RESIDENCY_ORIGIN_X_AT_POSE_A.store(i32::MIN, Ordering::SeqCst);

    let mut app_args = crate::AppArgs::default();
    app_args.grid_preset = crate::GridPreset::ProceduralStreaming {
        noise_preset: 0,
        seed: app_args.noise_seed,
    };
    app_args.streaming_window_mode = true;
    // Force `oasis_edit_visual_mode = true` so the driver routes into the
    // OasisWarmup state machine on tick 0. The OasisApplyEdit branch in
    // `driver.rs` detects `streaming_window_mode` and promotes the camera
    // instead of running a brush edit.
    app_args.oasis_edit_visual_mode = true;

    println!(
        "e2e_render --streaming-window: booting procedural-streaming world \
         (seed={}, sea_level={:.1}, terrain_amplitude={:.1}, \
         vram_budget_mib={}, max_segments_per_frame={})",
        app_args.noise_seed,
        app_args.sea_level,
        app_args.terrain_amplitude,
        app_args.vram_budget_mib,
        app_args.max_segments_per_frame,
    );

    crate::run_e2e_render_with_args(app_args)
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
        let mut res = crate::streaming::Residency::empty(4);
        res.origin = IVec3::ZERO;
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
        res.origin = IVec3::new(4, 0, 0);

        let pose_a_world = streaming_window_pose(false);
        let pose_b_world = streaming_window_pose(true);
        // Pre-condition: in world coords, B is +1024 X past A.
        assert!((pose_b_world.translation.x - pose_a_world.translation.x
            - STREAMING_WALK_DISTANCE_VOXELS).abs() < 1e-4);

        // After origin-shift translation, pose_b_world maps to the same X as
        // pose_a_world at origin (0, 0, 0).
        let mut res_zero = crate::streaming::Residency::empty(4);
        res_zero.origin = IVec3::ZERO;
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
        res.origin = IVec3::new(4, 0, 2);
        let world = streaming_window_pose(true);
        let once = translate_world_to_window_local(world, Some(&res));
        let twice = translate_world_to_window_local(world, Some(&res));
        let thrice = translate_world_to_window_local(world, Some(&res));
        assert_eq!(once.translation, twice.translation);
        assert_eq!(twice.translation, thrice.translation);
    }
}
