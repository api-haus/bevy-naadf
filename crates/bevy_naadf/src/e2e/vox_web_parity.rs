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
use std::process::Command;

use bevy::prelude::*;
use bevy::winit::WinitSettings;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::{Framebuffer, Rect};
use crate::e2e::oasis_edit_visual::{oasis_vox_fixture_path, OASIS_VOX_FIXTURE_PATH};

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
// Driver state stash
// ---------------------------------------------------------------------------

/// Per-run state owned by the e2e driver. Same shape as
/// `vox_gpu_oracle::VoxGpuOracleState`. The driver writes the captured
/// framebuffer here, then issues the save+exit at `VoxWebParityDrain`.
#[derive(Resource, Default)]
pub struct VoxWebParityState {
    pub captured: Option<Framebuffer>,
    pub saved: bool,
}

// ---------------------------------------------------------------------------
// Sub-phase entry points
// ---------------------------------------------------------------------------

/// Boot the e2e harness configured for the skybox-baseline phase. Saves
/// `target/e2e-screenshots/vox_web_parity_skybox.png`.
pub fn run_vox_web_parity_skybox_phase() -> AppExit {
    println!(
        "e2e_render --vox-web-parity-skybox: booting with GridPreset::Empty \
         (skybox-only baseline). Camera pinned to {:?} look {:?}. Saving to {}.",
        PARITY_CAMERA_POS, PARITY_CAMERA_LOOK, PARITY_SKYBOX_PNG,
    );

    let mut app_args = crate::AppArgs::default();
    app_args.grid_preset = crate::GridPreset::Empty;
    app_args.vox_web_parity_skybox_phase = true;
    crate::run_e2e_render_with_args(app_args)
}

/// Boot the e2e harness configured for the loaded phase. Loads the Oasis
/// `.vox` via the production W5 GPU producer chain
/// (`install_vox_in_fixed_world`), runs the Q3 cross-frame readback, then
/// captures `vox_web_parity_loaded.png`.
///
/// Per Step 8 option (a): native `Startup` install is **synchronous** (the
/// existing gate compat decision the prior dispatch made). The driver
/// warms up for [`PARITY_WARMUP_FRAMES`] before capture, which is
/// comfortably long enough for the W5 GPU producer chain to dispatch +
/// the Q3 cross-frame readback state machine to complete + TAA/GI to
/// converge.
pub fn run_vox_web_parity_loaded_phase() -> AppExit {
    let path = oasis_vox_fixture_path();
    if !path.exists() {
        eprintln!(
            "e2e_render --vox-web-parity-loaded: FIXTURE MISSING at {} \
             (cwd = {:?}). The fixture is Git LFS-tracked at \
             {OASIS_VOX_FIXTURE_PATH}. Run `git lfs pull`, OR run from the \
             workspace root.",
            path.display(),
            std::env::current_dir().ok()
        );
        return AppExit::error();
    }
    println!(
        "e2e_render --vox-web-parity-loaded: loading Oasis VOX from {} via \
         the W5 GPU producer chain (install_vox_in_fixed_world). Camera \
         pinned to {:?} look {:?}. Saving to {}.",
        path.display(),
        PARITY_CAMERA_POS,
        PARITY_CAMERA_LOOK,
        PARITY_LOADED_PNG,
    );

    let mut app_args = crate::AppArgs::default();
    app_args.grid_preset = crate::GridPreset::Vox { path };
    app_args.construction_config.gpu_construction_enabled = true;
    app_args.vox_web_parity_loaded_phase = true;
    crate::run_e2e_render_with_args(app_args)
}

// ---------------------------------------------------------------------------
// Top-level compare entry point
// ---------------------------------------------------------------------------

/// Top-level entry point for `--vox-web-parity`. Spawns the skybox + loaded
/// phases as subprocesses, loads both saved PNGs, runs the SSIM compare,
/// and returns an exit code (0 = PASS, non-zero = FAIL).
pub fn run_vox_web_parity_compare() -> u8 {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-web-parity: cannot resolve current_exe — {e}"
            );
            return 1;
        }
    };
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-web-parity: cannot resolve current_dir — {e}"
            );
            return 1;
        }
    };

    // Phase 1 — skybox baseline.
    println!(
        "e2e_render --vox-web-parity: spawning skybox-baseline phase \
         (subprocess: {} --vox-web-parity-skybox)",
        exe.display()
    );
    let skybox_status = Command::new(&exe)
        .arg("--vox-web-parity-skybox")
        .current_dir(&cwd)
        .status();
    let skybox_ok = match skybox_status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!(
                "e2e_render --vox-web-parity: skybox phase subprocess failed to \
                 spawn — {e}"
            );
            return 1;
        }
    };
    if !skybox_ok {
        eprintln!(
            "e2e_render --vox-web-parity: skybox phase subprocess exited \
             non-zero — aborting compare"
        );
        return 1;
    }

    // Phase 2 — loaded.
    println!(
        "e2e_render --vox-web-parity: spawning loaded phase \
         (subprocess: {} --vox-web-parity-loaded)",
        exe.display()
    );
    let loaded_status = Command::new(&exe)
        .arg("--vox-web-parity-loaded")
        .current_dir(&cwd)
        .status();
    let loaded_ok = match loaded_status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!(
                "e2e_render --vox-web-parity: loaded phase subprocess failed \
                 to spawn — {e}"
            );
            return 1;
        }
    };
    if !loaded_ok {
        eprintln!(
            "e2e_render --vox-web-parity: loaded phase subprocess exited \
             non-zero — aborting compare"
        );
        return 1;
    }

    // Phase 3 — SSIM compare.
    let skybox_path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(PARITY_SKYBOX_PNG);
    let loaded_path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(PARITY_LOADED_PNG);
    println!(
        "e2e_render --vox-web-parity: comparing {} vs {} (asserting SSIM < \
         {:.3} for dissimilarity)",
        skybox_path.display(),
        loaded_path.display(),
        VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX,
    );

    let skybox_fb = match crate::e2e::ssim::load_png_as_framebuffer(&skybox_path) {
        Ok(fb) => fb,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-web-parity: failed to load skybox PNG {} — {e}",
                skybox_path.display()
            );
            return 1;
        }
    };
    let loaded_fb = match crate::e2e::ssim::load_png_as_framebuffer(&loaded_path) {
        Ok(fb) => fb,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-web-parity: failed to load loaded PNG {} — {e}",
                loaded_path.display()
            );
            return 1;
        }
    };

    // web-vox-color-divergence (2026-05-18) Decision 4 — the SSIM-only compare
    // below is color-blind by construction: a structurally-correct but
    // all-near-black render still scores SSIM ≈ 0 vs the skybox baseline
    // because the silhouettes differ regardless of color. Add a per-channel
    // spread assertion on the loaded frame itself BEFORE the SSIM compare so
    // the most diagnostic error fires first.
    let central = Rect::from_fractional(&loaded_fb, 0.30, 0.30, 0.70, 0.70);
    let loaded_channel_max = loaded_fb.region_channel_max(central);
    println!(
        "e2e_render --vox-web-parity: loaded frame central rect channel max = \
         {loaded_channel_max:.1} (threshold > {VOX_WEB_PARITY_CHANNEL_MAX_FLOOR:.0} \
         — meaningful per-voxel color)",
    );
    if loaded_channel_max <= VOX_WEB_PARITY_CHANNEL_MAX_FLOOR {
        eprintln!(
            "e2e_render --vox-web-parity: FAIL — loaded frame channel max \
             {loaded_channel_max:.1} <= floor {VOX_WEB_PARITY_CHANNEL_MAX_FLOOR:.0}. \
             The .vox install path rendered structurally correct geometry but \
             colorless / near-black voxels (web-vox-color-divergence class). \
             Inspect target/e2e-screenshots/{PARITY_LOADED_PNG}.",
        );
        return 1;
    }

    let ssim_score = match crate::e2e::ssim::ssim_compare_framebuffers(&skybox_fb, &loaded_fb) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("e2e_render --vox-web-parity: SSIM compare failed: {e}");
            return 1;
        }
    };

    println!(
        "e2e_render --vox-web-parity: SSIM = {:.4} (threshold < {:.3} for \
         dissimilarity); skybox {}×{}, loaded {}×{}",
        ssim_score,
        VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX,
        skybox_fb.width(),
        skybox_fb.height(),
        loaded_fb.width(),
        loaded_fb.height(),
    );

    if ssim_score >= VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX {
        eprintln!(
            "e2e_render --vox-web-parity: FAIL — SSIM {:.4} >= dissimilarity \
             max {:.3}. The loaded frame is structurally too similar to the \
             skybox baseline; the .vox install path likely failed to populate \
             the renderer.",
            ssim_score,
            VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX,
        );
        return 1;
    }
    println!("e2e_render --vox-web-parity: PASS — SSIM dissimilar enough");
    0
}

// ---------------------------------------------------------------------------
// Camera pin system — overrides the standard e2e camera (mirrors
// `vox_gpu_oracle::pin_vox_gpu_oracle_camera`)
// ---------------------------------------------------------------------------

/// `Update` system: pin the camera at the shared parity pose every tick.
/// Runs when either parity sub-phase flag is set, `.after(driver::e2e_driver)`
/// so the pose pin lands AFTER the driver's pose write but BEFORE
/// `sync_position_split` consumes the `Transform`.
pub fn pin_vox_web_parity_camera(
    args: Option<Res<crate::AppArgs>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.vox_web_parity_skybox_phase && !args.vox_web_parity_loaded_phase {
        return;
    }
    let pose = Transform::from_translation(PARITY_CAMERA_POS)
        .looking_at(PARITY_CAMERA_LOOK, Vec3::X);
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
    let _ = WinitSettings::game;
}

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
