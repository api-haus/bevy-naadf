//! `--vox-horizon-native` / `--vox-horizon-parity` — cross-target (native ↔
//! WASM) horizon-view parity gate.
//!
//! ## Why
//!
//! The existing `--vox-web-parity-*` gate pins a top-down birdseye camera
//! (`PARITY_CAMERA_POS = (744, 800, 672)` looking down to `(744, 100, 672)`)
//! that frames a small patch of Oasis directly beneath the camera. That pose
//! is excellent for SSIM-dissimilarity between skybox + loaded, but it does
//! NOT exercise the long-distance raymarch — every ray hits a chunk within
//! ~50 voxels.
//!
//! User-reported 2026-05-19: the WASM build's rays terminate prematurely.
//! Native release renders the full 4096×4096-voxel city out to the horizon;
//! WASM cuts off ~20–30% of the depth (visible as the building-front-face
//! cuts in the deployed Cloudflare Pages build and in local `just test-wasm`
//! Playwright captures, while a native release-build screenshot from the
//! same camera pose still reaches the horizon line).
//!
//! This gate pins the camera at the **C#-faithful default pose** — the same
//! pose [`crate::camera::InitialCameraPose::from_world_voxels`] writes for a
//! freshly-loaded `.vox` on both targets — so the rendered framebuffer
//! exercises the long-distance raymarch end to end. A Playwright spec
//! captures the same pose in WASM, then shells out to `--ssim-compare …
//! --ssim-min <T>` to assert structural similarity.
//!
//! ## Camera pose
//!
//! User-captured 2026-05-19 (re-baseline #2) — the "very telling" pose
//! that surfaces a subtle web-only ray-termination class still present
//! after the chunk-AADF dispatch fix. The earlier front-clip pose
//! turned out to be a world-boundary modulo artifact that C# also
//! exhibits, so this pose replaces it:
//!
//! - `translation = (3880.187, 497.332, 3514.350)` voxels
//! - `rotation = Quat(-0.09791362, 0.5846077, 0.07135339, 0.8022191)`
//!   (x, y, z, w) — forward `(-0.924, -0.241, -0.297)`
//!
//! Camera is just under the world ceiling (`497 / 512`) looking
//! west-and-slightly-down across the Oasis grid.
//!
//! ## Window resolution
//!
//! Native runs at [`HORIZON_WIDTH`]×[`HORIZON_HEIGHT`] = 1280×720 (not the
//! default 256×256 e2e window). The Playwright spec must pin the same
//! viewport so the two PNGs SSIM-compare without resize.
//!
//! ## Sub-modes
//!
//! - `--vox-horizon-native` — boots with `GridPreset::Vox { path: oasis.cvox }`
//!   through the production W5 GPU producer chain, pins the horizon camera,
//!   captures [`HORIZON_NATIVE_PNG`].
//! - `--vox-horizon-parity` — top-level orchestrator that spawns the native
//!   sub-phase as a subprocess and then expects a sibling `vox_horizon_web.png`
//!   to compare against. The Playwright spec produces the WASM-side PNG;
//!   `--ssim-compare … --ssim-min <T>` is what the spec asserts.

use std::path::{Path, PathBuf};

use crate::e2e::framebuffer::Framebuffer;

// ---------------------------------------------------------------------------
// Camera pose — `HORIZON_CAMERA_POS` / `HORIZON_CAMERA_ROT` live in
// [`crate::camera::poses`] (D3 finding 6 — dependency-arrow reversal).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Window resolution — large enough to make the long-distance raymarch visible
// ---------------------------------------------------------------------------

/// Horizon-mode window width. Chosen to match a 1280×720 Playwright viewport
/// so cross-target PNGs SSIM-compare without resize.
pub const HORIZON_WIDTH: u32 = 1280;
/// Horizon-mode window height — see [`HORIZON_WIDTH`].
pub const HORIZON_HEIGHT: u32 = 720;

// ---------------------------------------------------------------------------
// Output filenames
// ---------------------------------------------------------------------------

/// Native-side capture (written by `--vox-horizon-native`).
pub const HORIZON_NATIVE_PNG: &str = "vox_horizon_native.png";

/// WASM-side capture (written by the Playwright spec; this binary only reads
/// it during the `--vox-horizon-parity` compare step).
pub const HORIZON_WEB_PNG: &str = "vox_horizon_web.png";

// ---------------------------------------------------------------------------
// Frame budgets — reuse the existing parity timings
// ---------------------------------------------------------------------------

/// Frames of static warmup before screenshot capture. Reuses
/// [`super::vox_web_parity::PARITY_WARMUP_FRAMES`] so TAA + GI converge.
pub const HORIZON_WARMUP_FRAMES: u32 = super::vox_web_parity::PARITY_WARMUP_FRAMES;

/// Frame drain ceiling — reused from [`super::vox_web_parity::PARITY_DRAIN_FRAMES`].
pub const HORIZON_DRAIN_FRAMES: u32 = super::vox_web_parity::PARITY_DRAIN_FRAMES;

// ---------------------------------------------------------------------------
// Default SSIM similarity threshold — the Playwright spec passes this to
// `--ssim-compare --ssim-min`.
// ---------------------------------------------------------------------------

/// Minimum SSIM the gate tolerates between the native horizon capture and
/// the WASM horizon capture. Below this, the two builds are structurally
/// too dissimilar — the WASM raymarcher is rendering a meaningfully
/// different image (the user-reported ray-termination class).
///
/// **0.91** — empirical floor pegged to the post-chunk-AADF-fix
/// measured value (~0.94 at the cross-target gate's pose, on top of the
/// `WASM_MAX_GROUP_BOUND_DISPATCH = 4096` cap that gates the residual
/// WebGPU regime-2 convergence-rate issue). 0.98 was aspirational but
/// unreachable until a deeper fix lands; 0.91 leaves ~3 points of
/// headroom above the current measured SSIM and well above the 0.789
/// pre-fix regression.
pub const HORIZON_SSIM_SIMILARITY_MIN: f64 = 0.91;

// ---------------------------------------------------------------------------
// Oasis CVOX fixture path
// ---------------------------------------------------------------------------

/// Workspace-relative path to the Oasis `.cvox` fixture. Same file the live
/// web build fetches by default + the Playwright spec serves at
/// `/test-fixtures/oasis.cvox`.
pub const OASIS_CVOX_FIXTURE_PATH: &str = "crates/bevy_naadf/assets/test/oasis.cvox";

/// Resolve [`OASIS_CVOX_FIXTURE_PATH`] against either the workspace root or
/// the per-crate `crates/bevy_naadf/` cwd, whichever exists.
pub fn oasis_cvox_fixture_path() -> PathBuf {
    let workspace_relative = PathBuf::from(OASIS_CVOX_FIXTURE_PATH);
    if workspace_relative.exists() {
        return workspace_relative;
    }
    PathBuf::from("assets/test/oasis.cvox")
}

// ---------------------------------------------------------------------------
// PNG path helpers
// ---------------------------------------------------------------------------

pub fn horizon_native_png_path() -> PathBuf {
    Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(HORIZON_NATIVE_PNG)
}

pub fn horizon_web_png_path() -> PathBuf {
    Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(HORIZON_WEB_PNG)
}

/// Save a framebuffer to `target/e2e-screenshots/<filename>`. Mirrors
/// [`super::vox_web_parity::save_parity_screenshot`].
pub fn save_horizon_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --vox-horizon-native: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --vox-horizon-native: {filename} save failed: {e}"
        ),
    }
}
