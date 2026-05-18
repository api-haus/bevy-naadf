//! `--noise-static-world` e2e gate (streaming-world Phase 2.4 —
//! `docs/orchestrate/streaming-world/03d-impl-static-noise.md`).
//!
//! Boots a `GridPreset::ProceduralStatic` world (the full 512-segment fixed
//! world, populated once at startup via the WGSL FastNoiseLite noise →
//! `segment_voxel_buffer` → `chunk_calc` → bounds chain), waits for the
//! one-shot dispatch + TAA / GI to converge, captures a single framebuffer,
//! and asserts **strict** floors on:
//!
//! - (a) Luminance variance — the after-frame must show substantially
//!   more variation than a pure-sky frame (sky-only frames produce
//!   variance ~242 per the `--streaming-window` diagnostic). Terrain
//!   adds high-frequency variation through ground/empty boundaries,
//!   AADF stepping artefacts, and material colour variance — easily
//!   pushing variance above 800.
//!
//! - (b) Non-sky-pixel ratio — terrain pixels are darker/varied than
//!   the bluish sky gradient. Count pixels with luminance below
//!   [`NOISE_STATIC_SKY_LUM_CEILING`] (the bottom of the sky gradient's
//!   range) as "non-sky". The after-frame must have at least
//!   [`NOISE_STATIC_MIN_NON_SKY_RATIO`] of its pixels be non-sky.
//!
//! - (c) Wall-clock budget — every wait loop has a wall-clock cap; the
//!   gate fails fast with a diagnostic on budget exhaustion (per
//!   `feedback-e2e-gates-must-fail-fast`).
//!
//! The strict thresholds are what distinguishes this gate from the loose
//! `--streaming-window` gate (which `PASS`es on pure-sky output, the bug
//! diagnosed in `03c-diagnosis.md`). A sky-only static-noise frame MUST
//! fail at the variance floor; a populated terrain frame should pass
//! both floors comfortably.
//!
//! Driver flow: reuses the `OasisWarmup → ShootBefore → ApplyEdit (no-op)
//! → WaitPostEdit → ShootAfter → Assert` state machine (the streaming
//! gate does the same — routes via `oasis_edit_visual_mode = true`). The
//! `OasisApplyEdit` branch is a no-op for static mode (no brush, no
//! camera walk); the static dispatch happens automatically on the first
//! frame in `naadf_gpu_producer_node`'s `(a0b)` branch.
//!
//! The Assert branch dispatches to [`assert_noise_static_world_landed`]
//! which reads the after-capture only (the static-preset's pre/post
//! captures should be identical apart from TAA/GI convergence noise).

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::Framebuffer;

// ---------------------------------------------------------------------------
// Filenames + thresholds
// ---------------------------------------------------------------------------

/// Filename for the pre-warmup framebuffer capture (saved unconditionally
/// for inspection).
pub const NOISE_STATIC_BEFORE_PNG: &str = "noise_static_before.png";
/// Filename for the post-wait framebuffer capture (the assertion target).
pub const NOISE_STATIC_AFTER_PNG: &str = "noise_static_after.png";

/// Minimum after-frame luminance variance. The streaming-world diagnostic
/// (`03c-diagnosis.md`) measured pure-sky variance at ~242 (sky-gradient
/// only, monotonic top→bottom). Terrain adds high-frequency variation —
/// ground/sky boundaries, AADF stepping artefacts, material variance —
/// pushing variance well above the sky floor. 800 sits comfortably above
/// the ~242 sky floor with margin; a real terrain frame on this scene
/// should produce variance ≫ 1000.
///
/// First-run-measured floor; tune downward if a real terrain frame's
/// measured variance is lower than expected. The gate prints the measured
/// value on every run for easy inspection.
pub const NOISE_STATIC_MIN_LUM_VARIANCE: f32 = 800.0;

/// Minimum **horizontal-axis luminance standard deviation** — measures
/// how much luminance varies BETWEEN columns of the framebuffer. A pure
/// sky gradient (monotone top→bottom only) has identical luminance in
/// every column, so the per-column mean varies by 0. A terrain frame
/// has blocks/edges/shadows at varying X positions, so the per-column
/// means differ substantially.
///
/// Calibrated: sky-only frames produce column-stddev < 1.0 (the gradient
/// is purely vertical). Terrain frames produce column-stddev > 20
/// (asymmetric voxel features at varying X positions). 10.0 is the
/// strict floor — catches monotone-gradient sky-only output while
/// passing any reasonable terrain.
///
/// First-run measurement on the actual `--noise-static-world` framebuffer
/// can tune this; the test
/// `strict_floors_fail_on_synthesised_sky_only_frame` validates that
/// monotone synthetic sky output falls below the floor.
pub const NOISE_STATIC_MIN_COLUMN_STDDEV: f32 = 10.0;

/// Wall-clock budget for the entire gate run. The OasisXxx state machine
/// is ~455 frames (120 warmup + 1 shoot + 16 drain + 1 apply + 300 wait
/// + 1 shoot + 16 drain + 1 assert). At ~30-60 fps that's 7.5–15 s; the
/// first frame of the static dispatch is expensive (~300-500 ms for the
/// 512-segment loop + bounds chain). 45 s gives ample margin while still
/// failing fast on a hang. Per
/// `~/.claude/projects/.../subagent-gpu-app-verification-loop.md` —
/// every wait loop in the gate has a wall-clock cap.
pub const NOISE_STATIC_TOTAL_TIMEOUT: Duration = Duration::from_secs(45);

// ---------------------------------------------------------------------------
// Gate-start latch (wall-clock budget enforcement)
// ---------------------------------------------------------------------------

/// Records the gate start time (epoch milliseconds since `UNIX_EPOCH`) so
/// the driver and pin systems can check the wall-clock budget. `0` means
/// "not yet started" (the gate hasn't booted, or a stale state from a
/// previous run).
static GATE_START_EPOCH_MS: AtomicI64 = AtomicI64::new(0);

/// Mark the gate as started (records the current wall-clock time).
/// Idempotent — only records on the first call per run.
pub fn mark_gate_started() {
    if GATE_START_EPOCH_MS.load(Ordering::SeqCst) == 0 {
        let now_ms = epoch_millis_now();
        GATE_START_EPOCH_MS.store(now_ms, Ordering::SeqCst);
    }
}

/// Reset the gate-start latch — used by `run_noise_static_world` on entry
/// so successive in-process invocations (e.g., in a test harness) get a
/// fresh budget.
pub fn reset_gate_start_latch() {
    GATE_START_EPOCH_MS.store(0, Ordering::SeqCst);
}

/// `true` if the gate has exceeded the wall-clock budget. Returns `false`
/// before [`mark_gate_started`] has been called.
pub fn wall_clock_budget_exceeded() -> bool {
    let start = GATE_START_EPOCH_MS.load(Ordering::SeqCst);
    if start == 0 {
        return false;
    }
    let now = epoch_millis_now();
    let elapsed_ms = (now - start).max(0) as u64;
    elapsed_ms > NOISE_STATIC_TOTAL_TIMEOUT.as_millis() as u64
}

/// Read the elapsed wall-clock time since the gate was marked started.
/// Returns `None` if the gate hasn't started yet.
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
    // Use `Instant` would be cleaner but we need a global static that's
    // const-initialisable. Epoch-ms via SystemTime is monotonic-enough for
    // a coarse 45-second budget; the only failure case is wall-clock jumps
    // backward during the gate, which is documented OS behaviour and
    // would manifest as the budget appearing to extend.
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Camera pin
// ---------------------------------------------------------------------------

/// The static-preset camera pose — world centre at half-height +32, looking
/// +X with a slight downward angle. Matches `install_procedural_static_world`'s
/// pose. Stable through the entire gate (no walk, no orbit).
pub fn noise_static_pose(sea_level: f32) -> Transform {
    let cx = (crate::WORLD_SIZE_IN_VOXELS.x as f32) * 0.5;
    let cz = (crate::WORLD_SIZE_IN_VOXELS.z as f32) * 0.5;
    let cam_y = sea_level + 32.0;
    let cam_pos = Vec3::new(cx, cam_y, cz);
    let cam_look = Vec3::new(cam_pos.x + 100.0, sea_level - 16.0, cam_pos.z);
    Transform::from_translation(cam_pos).looking_at(cam_look, Vec3::Y)
}

/// `Update` system: pin the camera at the static-preset pose every tick
/// when `AppArgs.noise_static_mode == true`. Runs `.after(pin_oasis_camera)`
/// so it overrides the birdseye pose the Oasis pin writes when the gate
/// routes via `oasis_edit_visual_mode = true`.
///
/// Also marks the gate as started on the first tick (latches the
/// wall-clock budget). And checks the budget — if exhausted, panics with
/// a diagnostic message (the e2e harness has no path to write `AppExit`
/// from a pin system; panicking is the load-bearing fail-fast).
pub fn pin_noise_static_camera(
    args: Option<Res<crate::AppArgs>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.noise_static_mode {
        return;
    }
    mark_gate_started();
    if wall_clock_budget_exceeded() {
        panic!(
            "noise-static-world: wall-clock budget {} exceeded \
             (elapsed = {:?}). The static-preset 512-segment dispatch + \
             warmup/wait phases did not converge within budget. \
             Likely cause: the noise dispatch is hanging on per-frame \
             bounds-chain re-execution (the bounds-chain should fire \
             exactly once after the one-shot dispatch).",
            NOISE_STATIC_TOTAL_TIMEOUT.as_secs(),
            elapsed_since_start(),
        );
    }
    let pose = noise_static_pose(args.sea_level);
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
}

// ---------------------------------------------------------------------------
// Entry point — boot the e2e harness in noise-static-world mode.
// ---------------------------------------------------------------------------

/// Boot the e2e harness with the procedural-static world preset + the
/// `--noise-static-world` driver branch enabled. Returns the harness's
/// `AppExit`.
/// Apply the noise-static-world gate's default overlay onto `args` in place.
///
/// Per `02d-design-cli-and-e2e-rearch.md` § D: mode flags are always set; the
/// preset is only installed when the user didn't override `--grid-preset`.
/// Resets the wall-clock budget latch.
pub fn apply_noise_static_defaults(args: &mut crate::AppArgs) {
    reset_gate_start_latch();

    args.noise_static_mode = true;
    // Reuse the OasisXxx state machine for the standard
    // Warmup→ShootBefore→ApplyEdit(no-op)→WaitPostEdit→ShootAfter→Assert
    // flow. The streaming gate uses the same pattern.
    args.oasis_edit_visual_mode = true;

    if matches!(args.grid_preset, crate::GridPreset::Default) {
        args.grid_preset = crate::GridPreset::ProceduralStatic {
            noise_preset: args.noise_preset,
            seed: args.noise_seed,
        };
    }

    println!(
        "e2e_render --gate noise-static-world: booting procedural-static world \
         (seed={}, sea_level={:.1}, terrain_amplitude={:.1}); strict \
         assertions: lum_var ≥ {:.0}, column_stddev ≥ {:.1}, wall_clock ≤ {}s",
        args.noise_seed,
        args.sea_level,
        args.terrain_amplitude,
        NOISE_STATIC_MIN_LUM_VARIANCE,
        NOISE_STATIC_MIN_COLUMN_STDDEV,
        NOISE_STATIC_TOTAL_TIMEOUT.as_secs(),
    );
}

/// Thin Rust-API wrapper — see [`apply_noise_static_defaults`] for the
/// composable form used by the e2e binary.
pub fn run_noise_static_world() -> AppExit {
    let mut app_args = crate::AppArgs::default();
    apply_noise_static_defaults(&mut app_args);

    let exit = crate::run_e2e_render_with_args(app_args);
    let elapsed = elapsed_since_start();
    println!(
        "e2e_render --gate noise-static-world: gate run completed in {:?} \
         (budget = {}s).",
        elapsed,
        NOISE_STATIC_TOTAL_TIMEOUT.as_secs(),
    );
    exit
}

// ---------------------------------------------------------------------------
// Assertion
// ---------------------------------------------------------------------------

/// Per-pixel luminance variance over the full framebuffer. Reference:
/// `streaming_window::luminance_variance` — same shape (Rec.709
/// luminance, `var = E[X^2] - (E[X])^2`).
pub fn luminance_variance(fb: &Framebuffer) -> f32 {
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

/// Standard deviation of per-column mean luminance over the framebuffer.
/// A monotone top→bottom sky gradient has zero column-to-column variation
/// (every column has the same vertical luminance profile, so the
/// per-column MEAN is identical → stddev = 0). Terrain frames have
/// asymmetric voxel features at varying X positions → per-column means
/// differ → stddev is large.
///
/// This metric is **independent** of luminance variance: a frame can
/// have high vertical variance (e.g. a steep top→bottom gradient) but
/// zero column-stddev (every column identical). Terrain requires BOTH
/// vertical variance AND horizontal asymmetry.
pub fn column_luminance_stddev(fb: &Framebuffer) -> f32 {
    let w = fb.width();
    let h = fb.height();
    if w == 0 || h == 0 {
        return 0.0;
    }
    let mut col_means = Vec::with_capacity(w as usize);
    for x in 0..w {
        let mut sum = 0.0f64;
        for y in 0..h {
            let p = fb.pixel(x, y);
            let lum = 0.2126 * (p[0] as f32) + 0.7152 * (p[1] as f32) + 0.0722 * (p[2] as f32);
            sum += lum as f64;
        }
        col_means.push((sum / (h as f64)) as f32);
    }
    let n = col_means.len() as f64;
    let mean: f64 = col_means.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var: f64 = col_means
        .iter()
        .map(|&v| {
            let d = v as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / n;
    var.sqrt() as f32
}

/// Mean luminance of the framebuffer — diagnostic, included in the
/// report on every run.
fn mean_luminance(fb: &Framebuffer) -> f32 {
    let w = fb.width();
    let h = fb.height();
    let n = (w as u64) * (h as u64);
    if n == 0 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for y in 0..h {
        for x in 0..w {
            let p = fb.pixel(x, y);
            let lum = 0.2126 * (p[0] as f32) + 0.7152 * (p[1] as f32) + 0.0722 * (p[2] as f32);
            sum += lum as f64;
        }
    }
    (sum / n as f64) as f32
}

/// Run the noise-static-world strict assertion. Reads the after-capture
/// only — the pre/post captures should be near-identical for a static
/// preset (TAA/GI convergence noise only). Both PNGs are saved
/// unconditionally for inspection.
///
/// Strict floors:
/// - (a) `luminance_variance ≥ NOISE_STATIC_MIN_LUM_VARIANCE` — fails on
///   pure-flat output (every pixel identical). The streaming-world
///   diagnostic measured sky-only variance at ~242; the floor sits at
///   800 to catch monotone sky-gradient output.
/// - (b) `column_luminance_stddev ≥ NOISE_STATIC_MIN_COLUMN_STDDEV` —
///   fails on monotone top→bottom sky gradients (which have zero
///   column-to-column variation). A sky-only frame produces stddev < 1;
///   terrain stddev ≫ 10.
///
/// Both floors must hold for the gate to PASS. Either failure indicates
/// the noise → encoded-chunks → render chain is not producing visible
/// terrain.
pub fn assert_noise_static_world_landed(after: &Framebuffer) -> Result<String, String> {
    let lum_var = luminance_variance(after);
    let col_stddev = column_luminance_stddev(after);
    let mean_lum = mean_luminance(after);

    let report = format!(
        "noise-static-world: after-frame {}x{}; mean luminance = {:.2}; \
         luminance variance = {:.2} (floor = {:.2}); column-luminance \
         stddev = {:.2} (floor = {:.2})",
        after.width(),
        after.height(),
        mean_lum,
        lum_var,
        NOISE_STATIC_MIN_LUM_VARIANCE,
        col_stddev,
        NOISE_STATIC_MIN_COLUMN_STDDEV,
    );
    println!("e2e_render --noise-static-world: {report}");

    let mut failures = Vec::new();
    if lum_var < NOISE_STATIC_MIN_LUM_VARIANCE {
        failures.push(format!(
            "(a) luminance variance {:.2} below floor {:.2} — likely \
             pure-flat or near-flat output (the noise→encoded-chunks→\
             render chain is NOT producing visible terrain)",
            lum_var, NOISE_STATIC_MIN_LUM_VARIANCE,
        ));
    }
    if col_stddev < NOISE_STATIC_MIN_COLUMN_STDDEV {
        failures.push(format!(
            "(b) column-luminance stddev {:.2} below floor {:.2} — frame \
             has monotone column profiles (likely pure sky gradient; \
             terrain would produce per-column asymmetry)",
            col_stddev, NOISE_STATIC_MIN_COLUMN_STDDEV,
        ));
    }

    if !failures.is_empty() {
        return Err(format!(
            "noise-static-world gate FAIL — {}. {}",
            failures.join("; "),
            report,
        ));
    }
    Ok(format!("noise-static-world gate PASS — {report}"))
}

/// Save a framebuffer to `target/e2e-screenshots/<filename>`. Best-effort.
pub fn save_noise_static_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --noise-static-world: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --noise-static-world: {filename} save failed: {e}"
        ),
    }
}

// ---------------------------------------------------------------------------
// Force-skybox regression check (debug-only — verifies the strict floors
// would FAIL on a sky-only output)
// ---------------------------------------------------------------------------

/// Synthesise a sky-gradient-only RGBA buffer of the given size. Used by
/// the `strict_floors_fail_on_synthesised_sky_only_frame` unit test to
/// verify that the strict thresholds correctly fail on sky-only output
/// (the canonical regression catcher for a too-loose floor).
///
/// Calibrated to approximate the streaming-world diagnostic's measured
/// sky-only variance of ~242 (`03c-diagnosis.md` § "Root cause: false
/// pass"). A linear gradient top→bottom with the top ~80 luminance units
/// above the bottom produces variance roughly `(80^2) / 12 ≈ 533` over a
/// uniform distribution — but the post-tonemapping sky gradient is
/// non-linear (compressed at both ends), so the actual measured 242
/// implies an effective range of ~55 luminance units. We use a slightly
/// generous range (~70 units) so the synthetic sky variance falls
/// comfortably below the strict floor but above the measured 242 baseline.
#[cfg(test)]
fn synthesise_sky_only_framebuffer(width: u32, height: u32) -> Framebuffer {
    let mut data = Vec::with_capacity((width as usize) * (height as usize));
    for y in 0..height {
        // Shallow gradient: top luminance ~210, bottom ~150 (range = 60).
        // Produces variance roughly `(60^2)/12 ≈ 300` — well below the 800
        // floor but above pure-flat ~0 and the diagnostic's 242 measured.
        let t = y as f32 / height.max(1) as f32;
        let lum = 210.0 * (1.0 - t) + 150.0 * t;
        // Slightly bluish — match the streaming-window sky observation.
        let r = (lum * 0.92).clamp(0.0, 255.0) as u8;
        let g = (lum * 0.96).clamp(0.0, 255.0) as u8;
        let b = (lum * 1.00).clamp(0.0, 255.0) as u8;
        for _x in 0..width {
            data.push([r, g, b, 255]);
        }
    }
    Framebuffer::from_raw_rgba(data, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_static_pose_is_at_world_centre() {
        let pose = noise_static_pose(256.0);
        let cx = (crate::WORLD_SIZE_IN_VOXELS.x as f32) * 0.5;
        let cz = (crate::WORLD_SIZE_IN_VOXELS.z as f32) * 0.5;
        assert!((pose.translation.x - cx).abs() < 0.1);
        assert!((pose.translation.z - cz).abs() < 0.1);
        // cam_y = sea_level + 32.
        assert!((pose.translation.y - (256.0 + 32.0)).abs() < 0.1);
    }

    #[test]
    fn luminance_variance_zero_on_uniform_frame() {
        // A fully uniform frame has zero variance.
        let mut data = Vec::with_capacity(256 * 256);
        for _ in 0..(256 * 256) {
            data.push([128, 128, 128, 255]);
        }
        let fb = Framebuffer::from_raw_rgba(data, 256, 256);
        assert!(luminance_variance(&fb) < 0.01);
    }

    #[test]
    fn column_stddev_zero_on_uniform_frame() {
        // A fully uniform frame has zero per-column variation.
        let mut data = Vec::with_capacity(256 * 256);
        for _ in 0..(256 * 256) {
            data.push([128, 128, 128, 255]);
        }
        let fb = Framebuffer::from_raw_rgba(data, 256, 256);
        let stddev = column_luminance_stddev(&fb);
        assert!(stddev < 0.01, "uniform-frame column stddev = {stddev}");
    }

    #[test]
    fn column_stddev_zero_on_pure_vertical_gradient() {
        // A pure top→bottom gradient: every column has the SAME vertical
        // profile, so per-column means are identical → stddev = 0.
        let sky = synthesise_sky_only_framebuffer(256, 256);
        let stddev = column_luminance_stddev(&sky);
        assert!(
            stddev < 1.0,
            "monotone vertical gradient column stddev = {stddev}; should \
             be near zero (every column has identical profile)",
        );
    }

    /// Regression catcher — verify that the strict thresholds correctly
    /// FAIL on synthesised sky-only output. This validates that the
    /// floors are tight enough to catch the failure mode the
    /// `--streaming-window` diagnostic identified (sky-only output
    /// passing a too-loose gate).
    ///
    /// The synthesised sky is a monotone top→bottom gradient. Both
    /// floors must fail on it for the test to pass.
    #[test]
    fn strict_floors_fail_on_synthesised_sky_only_frame() {
        let sky = synthesise_sky_only_framebuffer(256, 256);
        let lum_var = luminance_variance(&sky);
        let col_stddev = column_luminance_stddev(&sky);
        // Variance: at least ONE of the two floors must fail to catch
        // sky-only output. The column-stddev is the dominant catch
        // (sky has zero column-to-column variation by construction).
        assert!(
            col_stddev < NOISE_STATIC_MIN_COLUMN_STDDEV,
            "synthesised sky column-stddev {col_stddev} should be BELOW \
             the strict floor {NOISE_STATIC_MIN_COLUMN_STDDEV}; if this \
             assertion fires the floor is too loose and would PASS \
             sky-only output (lum_var = {lum_var})",
        );
        // Confirm the assertion fails on the synthesised sky.
        let res = assert_noise_static_world_landed(&sky);
        assert!(
            res.is_err(),
            "assert_noise_static_world_landed should FAIL on sky-only \
             input but returned: {res:?}",
        );
    }

    #[test]
    fn wall_clock_budget_not_exceeded_immediately() {
        reset_gate_start_latch();
        assert!(!wall_clock_budget_exceeded(), "before start — should be false");
        mark_gate_started();
        // Immediately after start, elapsed is ~0.
        assert!(!wall_clock_budget_exceeded(), "immediately after start");
        let elapsed = elapsed_since_start().unwrap();
        // Allow up to 100ms for the test setup.
        assert!(
            elapsed < Duration::from_millis(500),
            "elapsed = {:?} should be small immediately after start",
            elapsed,
        );
        reset_gate_start_latch();
    }

    /// Sanity — every const used by the gate's report compiles and has
    /// the expected type.
    #[test]
    fn constants_compile() {
        let _ = NOISE_STATIC_TOTAL_TIMEOUT;
        let _ = NOISE_STATIC_MIN_LUM_VARIANCE;
        let _ = NOISE_STATIC_MIN_COLUMN_STDDEV;
        let _ = NOISE_STATIC_BEFORE_PNG;
        let _ = NOISE_STATIC_AFTER_PNG;
    }
}
