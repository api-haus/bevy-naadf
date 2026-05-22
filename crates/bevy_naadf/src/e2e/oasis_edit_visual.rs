//! `--oasis-edit-visual` mode — load-bearing visual-diff edit-pipeline gate
//! (`02f-followup`, dispatched 2026-05-15).
//!
//! ## Why this gate exists
//!
//! The `02f` rearch consolidated WorldData ownership (no `ExtractedWorld`
//! clone, no `dirty` flag), but **broke the user-visible edit path
//! end-to-end**: `cargo run --bin bevy-naadf -- --vox <path>` + painting
//! produces no visible change. The CPU-oracle `--edit-mode` gate +
//! the `--runtime-edit-mode` record-counter gate BOTH pass — they assert
//! the **producer** side (W2 records get generated), not the **consumer**
//! side (records reach the GPU + change the framebuffer). This gate is
//! the missing end-to-end coverage.
//!
//! ## Mechanism
//!
//! 1. Load `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` through
//!    the production `GridPreset::Vox` path — same code path the binary
//!    runs.
//! 2. Pin the camera birdseye over the world centre, looking down — so
//!    the erased sphere projects into a known central screen rect.
//! 3. Warm up N frames (TAA + GI convergence).
//! 4. **Capture framebuffer A** — `target/e2e-screenshots/oasis_edit_before.png`.
//! 5. Programmatically invoke [`WorldData::set_voxels_batch`] with an
//!    erase sphere (~r=30 voxels) at world centre. This is the same
//!    runtime call shape [`crate::editor::tools::sphere_brush`] makes
//!    when LMB-drag erase is held in the binary — the load-bearing
//!    constraint is that the gate exercises THE SAME runtime path as
//!    user input, not the diagnostic oracle.
//! 6. Wait ~5 s (~300 frames at 60 fps) — the W2 GPU dispatch propagates
//!    through `naadf_world_change_node`'s 4 compute passes, the W3
//!    regime-2 background AADF chain converges (5 rounds/frame × 5 s ×
//!    60 fps = 1500 rounds), TAA / GI re-converge.
//! 7. **Capture framebuffer B** — `target/e2e-screenshots/oasis_edit_after.png`.
//! 8. **Assert** per-pixel mean RGB delta over a tight bounding box
//!    around the erased sphere's screen-space projection exceeds
//!    [`OASIS_EDIT_DIFF_FLOOR`] (generous; a real geometry-erase
//!    produces a massive change as sky / atmosphere replaces opaque
//!    material).
//!
//! ## Threshold rationale
//!
//! [`OASIS_EDIT_DIFF_FLOOR`] is a **mean** per-pixel RGB delta
//! (0.0..=255.0) over the bounding-box pixels (R + G + B channels
//! averaged), via [`Framebuffer::mean_pixel_delta`]. A bounding box
//! covering the erased sphere typically swings from opaque diffuse /
//! emissive geometry (luminance ~100-240) to atmosphere-tinted sky
//! (luminance ~140-160) — a per-channel delta on the order of 30-100
//! depending on the original surface material. A regression that
//! produces NO edit at all yields mean delta ~0-3 (just TAA / GI noise);
//! `OASIS_EDIT_DIFF_FLOOR = 8.0` sits comfortably above the noise floor
//! and well below the expected swing.
//!
//! ## What this gate catches that `--runtime-edit-mode` misses
//!
//! - The `d43f1f1` "no edits propagate at all" regression (W2 batch
//!   never reaches GPU dispatch).
//! - The `81171f9` "W2 batch generates correct records but framebuffer
//!   unchanged" regression (records reach GPU but writes go OOB / are
//!   dropped / consumed by a stale bind group / etc.).
//! - Any future regression in the brush → W2 → GPU dispatch chain that
//!   leaves the producer side intact.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::e2e::framebuffer::{Framebuffer, Rect};

// ---------------------------------------------------------------------------
// Fixture path
// ---------------------------------------------------------------------------

/// Workspace-relative path to the Oasis VOX fixture (Git LFS-tracked).
/// Anchored to the workspace root — the binary's `cwd` when run via
/// `cargo run` is the workspace root.
pub const OASIS_VOX_FIXTURE_PATH: &str = "crates/bevy_naadf/assets/test/oasis_hard_cover.vox";

/// Resolve the fixture path. Prefer the cwd-anchored workspace-relative path
/// (the typical `cargo run --bin e2e_render` invocation); fall back to a
/// crate-relative variant if the cwd is the crate directory.
pub fn oasis_vox_fixture_path() -> PathBuf {
    let workspace_relative = PathBuf::from(OASIS_VOX_FIXTURE_PATH);
    if workspace_relative.exists() {
        return workspace_relative;
    }
    PathBuf::from("assets/test/oasis_hard_cover.vox")
}

// ---------------------------------------------------------------------------
// Screenshot filenames (under `E2E_SCREENSHOT_DIR`)
// ---------------------------------------------------------------------------

/// PNG saved for the pre-edit capture (framebuffer A).
pub const OASIS_EDIT_BEFORE_PNG: &str = "oasis_edit_before.png";
/// PNG saved for the post-edit capture (framebuffer B).
pub const OASIS_EDIT_AFTER_PNG: &str = "oasis_edit_after.png";
/// PNG saved with a visualisation of the bounding-box region used for the
/// diff comparison (drawn on top of frame B as a magenta border).
pub const OASIS_EDIT_DIFF_REGION_PNG: &str = "oasis_edit_diff_region.png";

// ---------------------------------------------------------------------------
// Frame budgets
// ---------------------------------------------------------------------------

/// Frames spent in the warmup phase before screenshot A is captured.
/// Generous enough for TAA convergence (32-deep ring) + GI temporal
/// accumulation (96 frames covers the C# default; we use 120 for slack).
pub const OASIS_WARMUP_FRAMES: u32 = 120;

/// Frames spent waiting between the brush call and screenshot B.
/// 300 frames at 60 fps ≈ 5 s — covers:
///   - W2 regime-3 dispatch propagation (1 frame),
///   - W3 regime-2 background AADF refinement (5 rounds/frame × 300 = 1500
///     rounds; whole-world bound queue converges),
///   - TAA / GI ring re-stabilisation around the new geometry.
pub const OASIS_POST_EDIT_WAIT_FRAMES: u32 = 300;

/// Max frames the driver waits for the screenshot capture to deliver
/// (same shape as the standard `E2E_DRAIN_FRAMES`).
pub const OASIS_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// Brush geometry
// ---------------------------------------------------------------------------

/// Erase-sphere radius in voxels. Chosen large enough to cover ~10-15% of
/// the framebuffer's central rect from the birdseye pose (so the diff is
/// unambiguously caused by the geometry change, not noise).
pub const OASIS_ERASE_RADIUS: f32 = 30.0;

// ---------------------------------------------------------------------------
// Diff threshold + bounding box fractions
// ---------------------------------------------------------------------------

/// Fractional screen-space bounding box around the erased-sphere
/// projection at the birdseye camera pose. Chosen as a central 30%×30%
/// region (35..65%) — tight enough that most pixels in the rect are
/// part of the geometry that actually changed (the sphere projects to
/// roughly ~15% of the framebuffer at the birdseye altitude), but wide
/// enough to comfortably absorb minor floating-point drift in the
/// projection between frames A and B.
///
/// The earlier 40%×40% rect averaged the ~5% sphere-projection swing
/// over a ~16× larger area; the mean-delta math diluted the signal
/// below the threshold even though the edit was visually obvious. A
/// tighter rect lifts the signal-to-noise.
pub const OASIS_DIFF_RECT_FRACS: (f32, f32, f32, f32) = (0.35, 0.35, 0.65, 0.65);

/// Minimum mean per-pixel RGB delta over the diff bounding box for the
/// gate to PASS. The metric is [`Framebuffer::mean_pixel_delta`] over
/// the rect. See module-level doc for rationale.
pub const OASIS_EDIT_DIFF_FLOOR: f32 = 8.0;

// ---------------------------------------------------------------------------
// Camera pose helper
// ---------------------------------------------------------------------------

/// Compute a birdseye camera pose centred over the world. Camera sits at
/// `(cx, world_top + 250, cz)` looking down at the world's mid-y point.
/// Returns the resulting `Transform`.
pub fn birdseye_pose(world_size_voxels: [u32; 3]) -> Transform {
    let cx = world_size_voxels[0] as f32 * 0.5;
    let cz = world_size_voxels[2] as f32 * 0.5;
    let mid_y = world_size_voxels[1] as f32 * 0.5;
    let cam_y = world_size_voxels[1] as f32 + 250.0;
    // Look DOWN at the world centre; +X is the "up" reference vector for the
    // look_at math so the resulting camera Y-axis aligns toward +Z (the
    // framebuffer's up direction).
    Transform::from_xyz(cx, cam_y, cz).looking_at(Vec3::new(cx, mid_y, cz), Vec3::X)
}

/// World-space centre voxel coord — the brush position. Y is mid-height.
pub fn world_centre_voxel(world_size_voxels: [u32; 3]) -> Vec3 {
    Vec3::new(
        world_size_voxels[0] as f32 * 0.5,
        world_size_voxels[1] as f32 * 0.5,
        world_size_voxels[2] as f32 * 0.5,
    )
}

// ---------------------------------------------------------------------------
// Assertion + PNG saves
// ---------------------------------------------------------------------------

/// Compute the diff rect and assert it exceeds the floor. Saves both
/// PNGs unconditionally so an agent can inspect them either way.
pub fn assert_visual_edit_landed(
    before: &Framebuffer,
    after: &Framebuffer,
) -> Result<String, String> {
    // Sanity — dimensions must agree (size change during the run is a hard
    // failure).
    if before.width() != after.width() || before.height() != after.height() {
        return Err(format!(
            "frame A {}×{} vs frame B {}×{} — dimensions changed mid-run; \
             this should never happen with `AppConfig::e2e`'s fixed window",
            before.width(),
            before.height(),
            after.width(),
            after.height()
        ));
    }

    let (fx0, fy0, fx1, fy1) = OASIS_DIFF_RECT_FRACS;
    let rect = Rect::from_fractional(after, fx0, fy0, fx1, fy1);

    // Region-mean RGB before / after — useful for the log.
    let mean_before = before.region_mean(rect);
    let mean_after = after.region_mean(rect);
    let lum_before = Framebuffer::luminance(mean_before);
    let lum_after = Framebuffer::luminance(mean_after);

    // Per-pixel mean delta over the rect (the actual gate metric).
    let rect_delta = region_mean_pixel_delta(before, after, rect);
    // Also full-frame for diagnostic context.
    let full_delta = before.mean_pixel_delta(after);

    let report = format!(
        "rect=({},{},{},{}) frac=({:.2},{:.2},{:.2},{:.2}); \
         rect mean rgba: before={:?}, after={:?}; \
         rect luminance: before={:.1}, after={:.1}, Δ={:.1}; \
         rect mean per-pixel RGB Δ={:.2} (floor={:.2}); \
         full-frame mean per-pixel RGB Δ={:.2}",
        rect.x0,
        rect.y0,
        rect.x1,
        rect.y1,
        fx0,
        fy0,
        fx1,
        fy1,
        mean_before,
        mean_after,
        lum_before,
        lum_after,
        (lum_after - lum_before).abs(),
        rect_delta,
        OASIS_EDIT_DIFF_FLOOR,
        full_delta,
    );
    println!("e2e_render --oasis-edit-visual: {report}");

    if rect_delta < OASIS_EDIT_DIFF_FLOOR {
        return Err(format!(
            "oasis-edit-visual gate FAIL — rect mean per-pixel RGB delta \
             {rect_delta:.2} is below the floor {:.2}. The erase sphere \
             did NOT visibly land in the framebuffer. \
             {report}. \
             This is the regression `02f-followup` exists to catch: the \
             producer-side `--runtime-edit-mode` gate passes (W2 batches \
             generate correct records), but the records do not reach the \
             framebuffer — likely the W2 GPU dispatch is gated wrong, the \
             bind group is stale, the chunks/blocks/voxels buffers were \
             allocated without W2 headroom and the dispatch's appends went \
             OOB, OR the `extract_world_changes` drain doesn't see the \
             pending_edits.batches. Inspect \
             target/e2e-screenshots/{OASIS_EDIT_BEFORE_PNG} + \
             target/e2e-screenshots/{OASIS_EDIT_AFTER_PNG}.",
            OASIS_EDIT_DIFF_FLOOR,
        ));
    }

    Ok(format!("oasis-edit-visual gate PASS — {report}"))
}

/// Compute the mean per-pixel RGB delta over a rect (channels averaged
/// 0..3). The [`Framebuffer::mean_pixel_delta`] helper does the same
/// math whole-frame; this is the rect-scoped equivalent.
fn region_mean_pixel_delta(a: &Framebuffer, b: &Framebuffer, rect: Rect) -> f32 {
    if a.width() != b.width() || a.height() != b.height() {
        return f32::MAX;
    }
    let mut acc = 0.0f64;
    let mut n = 0u64;
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let pa = a.pixel(x, y);
            let pb = b.pixel(x, y);
            for c in 0..3 {
                acc += (pa[c] as f64 - pb[c] as f64).abs();
            }
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        (acc / (n as f64 * 3.0)) as f32
    }
}

/// Save a framebuffer to `target/e2e-screenshots/<filename>`. Best-effort
/// — logs failure but does not propagate.
pub fn save_oasis_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --oasis-edit-visual: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --oasis-edit-visual: {filename} save failed: {e}"
        ),
    }
}
