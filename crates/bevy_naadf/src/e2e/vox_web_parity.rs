//! `--vox-web-parity` mode — native gate that captures a skybox-baseline
//! frame and a vox-loaded frame, then SSIM-asserts they are **dissimilar**
//! (proves the `.vox` install actually rendered geometry rather than
//! falling back silently to a pure-sky frame).
//!
//! web-vox-async-loading 2026-05-18 follow-up Step 8 / Q5 — see
//! `03-architecture.md` § Q5 for the design.
//!
//! ## Why this gate exists
//!
//! The existing `--vox-gpu-oracle` gate asserts CPU-vs-GPU **similarity**
//! (SSIM ≥ 0.85) — both branches render the same scene; the gate catches
//! gross GPU-renderer regressions. It does **not** prove that the `.vox`
//! install path actually populated the world: a regression that silently
//! turned the install into a no-op would pass `--vox-gpu-oracle` (both CPU
//! and GPU renders would be empty sky → SSIM = 1.0). The new
//! `--vox-web-parity` gate flips the assertion: compare the loaded
//! framebuffer against a **skybox-only baseline** and assert SSIM is
//! **below** a threshold (dissimilar).
//!
//! On native this gate exercises the production W5 GPU producer chain end
//! to end. On web (via the Q6 Playwright spec) the same logic runs in
//! Chrome — but the SSIM compare itself shells out to this same binary's
//! `--ssim-compare` flag (Step 9), so the metric is bit-identical on both
//! sides per Decision 4.
//!
//! ## Three sub-phases
//!
//! 1. **`--vox-web-parity-skybox`** — boots with [`crate::GridPreset::Empty`]
//!    (no `.vox` install, no `ModelData`, empty `dense_voxel_types` → W5
//!    producer chain disabled → renderer reads empty `WorldGpu` buffers).
//!    Captures `vox_web_parity_skybox.png`.
//!
//! 2. **`--vox-web-parity-loaded`** — boots with [`crate::GridPreset::Vox`]
//!    using the Oasis fixture, runs the full W5 producer chain + Q3
//!    cross-frame readback, captures `vox_web_parity_loaded.png`. The
//!    `--vox-web-parity-loaded` phase also asserts the global
//!    [`super::tracing_error_counter::TRACING_ERROR_COUNT`] is zero
//!    post-warmup.
//!
//! 3. **`--vox-web-parity`** (top-level) — spawns the two sub-modes as
//!    subprocesses, loads both PNGs, runs the SSIM compare via the shared
//!    helper [`super::ssim::ssim_compare_framebuffers`], asserts the score
//!    is below [`VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX`].

use std::path::{Path, PathBuf};

use bevy::math::Vec3;

use crate::e2e::framebuffer::Framebuffer;

// ---------------------------------------------------------------------------
// Camera pose — shared between skybox + loaded phases
// ---------------------------------------------------------------------------

/// Camera position used by BOTH sub-phases. Same coordinates as
/// `--vox-gpu-oracle`'s top-down pose so the skybox + loaded frames are
/// captured from a viewpoint that frames significant Oasis geometry in
/// the loaded phase (high downward) — the SSIM-dissimilarity is then a
/// real signal rather than a spurious "the sky is slightly different".
pub const PARITY_CAMERA_POS: Vec3 = Vec3::new(744.0, 800.0, 672.0);

/// Camera look-at — identical to `--vox-gpu-oracle`'s pose.
pub const PARITY_CAMERA_LOOK: Vec3 = Vec3::new(744.0, 100.0, 672.0);

// ---------------------------------------------------------------------------
// Screenshot filenames
// ---------------------------------------------------------------------------

pub const PARITY_SKYBOX_PNG: &str = "vox_web_parity_skybox.png";
pub const PARITY_LOADED_PNG: &str = "vox_web_parity_loaded.png";

// ---------------------------------------------------------------------------
// Frame + wall-clock budgets
// ---------------------------------------------------------------------------

/// Frames of static warmup before screenshot capture. Reuses the
/// `--vox-gpu-oracle` constant so the TAA 32-deep ring + GI 96-frame
/// accumulator fully converge before capture.
pub const PARITY_WARMUP_FRAMES: u32 = 120;

/// Frame drain ceiling — same shape as the standard
/// `E2E_DRAIN_FRAMES`/`ORACLE_DRAIN_FRAMES`. Reused from
/// `vox_gpu_oracle::ORACLE_DRAIN_FRAMES`.
pub const PARITY_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// SSIM threshold — load-bearing gate metric (assertion direction inverted
// from `vox_gpu_oracle`: this gate asserts SSIM **<** threshold)
// ---------------------------------------------------------------------------

/// Maximum SSIM the gate tolerates between the skybox baseline and the
/// vox-loaded frame. Above this, the two frames are structurally too
/// similar — the `.vox` install path didn't render meaningful geometry
/// (silent failure mode: the empty fallback path fired instead of the
/// real install).
///
/// **Initial value 0.85** — tuned conservatively. Per the architect's
/// design (`03-architecture.md` § Q5 + Assumptions §5):
///
/// - When the .vox is present and rendered correctly, the loaded frame is
///   heavily voxel-filled at the chosen top-down pose; structurally very
///   different from the gradient sky baseline; expected SSIM lands in
///   0.2–0.6.
/// - When the .vox install silently no-ops, both frames are pure sky;
///   SSIM = 1.0 (clearly fail).
/// - 0.85 sits comfortably between these regimes.
///
/// **Empirically tuned post-impl** to a higher value if the measured
/// difference between skybox and loaded turns out to be smaller (e.g. the
/// camera frames mostly sky at this pose). See follow-up dispatch
/// `04-refactoring.md` "Step 8 — SSIM threshold tuning".
pub const VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX: f64 = 0.85;

/// Per-channel mean-max floor on the central rect of the
/// `vox_web_parity_loaded.png` capture. See
/// `vox_e2e.rs::VOX_GEOMETRY_CHANNEL_MAX_FLOOR` for the rationale; same
/// calibration applies. 30.0 leaves 2× headroom above natural noise and well
/// below the colorful Oasis reference's measured ~60+ R/G/B means.
///
/// Added by `web-vox-color-divergence` (2026-05-18) Decision 4 — the SSIM-only
/// compare at `:307` is structurally color-blind (a near-black render still
/// scores SSIM ≈ 0 vs the gradient skybox baseline because the geometry's
/// silhouettes differ regardless of color). The per-channel floor catches the
/// "geometry correct, colors collapsed" regression class directly.
pub const VOX_WEB_PARITY_CHANNEL_MAX_FLOOR: f32 = 30.0;

// ---------------------------------------------------------------------------
// PNG path helpers
// ---------------------------------------------------------------------------

pub fn parity_skybox_png_path() -> PathBuf {
    Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(PARITY_SKYBOX_PNG)
}

pub fn parity_loaded_png_path() -> PathBuf {
    Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(PARITY_LOADED_PNG)
}

/// Save a framebuffer to `target/e2e-screenshots/<filename>`. Best-effort —
/// mirrors `vox_gpu_oracle::save_oracle_screenshot`.
pub fn save_parity_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --vox-web-parity: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --vox-web-parity: {filename} save failed: {e}"
        ),
    }
}
