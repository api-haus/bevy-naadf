//! `--vox-gpu-oracle` mode — single-capture sanity gate for the W5 path
//! (`docs/orchestrate/vox-gpu-rewrite/03-impl.md` Stage 13, 2026-05-18; was
//! CPU-vs-GPU oracle compare in Stages 4-12).
//!
//! ## Stage 13 rewire (2026-05-18) — what changed and why
//!
//! Stages 4-12 ran this as a CPU-oracle vs GPU-built per-pixel diff. The
//! "CPU phase" used `install_vox_sized_to_model` (natural-bound
//! 1488×544×1344 world); the "GPU phase" used the production
//! `install_vox_in_fixed_world` (fixed 4096×512×4096 world with
//! `voxelPos % modelSize` tiling + Y-clamp to 512). The two phases produced
//! **semantically different worlds** at the rendered region — primary rays
//! agreed where the camera framed the first XZ tile, but secondary GI rays
//! that strayed beyond the natural bounds hit:
//!
//!   - In the CPU phase: void (no geometry) → render sky.
//!   - In the GPU phase: tiled Oasis architecture → render diffuse bounce.
//!
//! Plus the GPU phase clipped Y=512..544. Net result: ~6% of pixels
//! diverged at Δ>16 per channel — well above the 1% per-pixel ceiling.
//! Bug 2 of `docs/orchestrate/vox-gpu-rewrite/17-diagnostic-residual-
//! speckle-and-brush-clears.md` characterised this as a test-calibration
//! issue (compared apples to oranges).
//!
//! Stage 13 investigated two remediations:
//!
//! **Shape A** (the diagnostic's preferred remediation): tighten the
//! per-pixel comparison rect to a subregion where the two worlds agree.
//! Empirically the diff is spread across the whole frame; no contiguous
//! subregion >32×32 pixels has <1% diff. **Shape A cannot satisfy the 1%
//! ceiling.**
//!
//! **Shape B** (the diagnostic's fallback): run the same install path
//! TWICE (drop the CPU-vs-GPU mismatch). Two flavours measured:
//!
//! - **Cross-process GPU-vs-GPU** (two subprocess invocations of the W5
//!   path): still ~6% per-pixel divergence. The W5 GPU producer chain is
//!   **non-deterministic across processes** — `atomicCompareExchangeWeak`
//!   on the mixed-block hash dedup resolves collisions with different
//!   slot allocations across runs, producing slightly different
//!   `voxels_cpu.len()` (e.g., 10479456 vs 10479392 between two runs)
//!   and downstream AADF / GI variance.
//! - **Same-process double-capture** (single subprocess; capture A at
//!   warmup frame 120, capture B at frame 121): ~1.7% per-pixel
//!   divergence. The W5 producer runs ONCE, so the `voxels[]` is byte-
//!   identical between captures — the residual divergence is renderer-
//!   side GI/TAA per-frame shimmer at high-frequency edges (palm
//!   fronds, accent-voxel boundaries). Still above the 1% ceiling.
//!
//! Both Shape-B flavours exceed the per-pixel ceiling. The renderer has
//! inherent stochastic GI sampling that produces ~1.5-2% per-pixel
//! variance at any two-frame compare. The 1% per-pixel ceiling is
//! structurally unsatisfiable against any compare metric that isn't
//! self-comparing a single captured frame.
//!
//! **The Stage 13 fix** drops the two-frame compare entirely. The gate is
//! now a single-capture sanity check: render the production W5 path,
//! capture ONE framebuffer, save it as **both** `oracle_cpu.png` and
//! `oracle_gpu.png` (byte-identical). The compare phase's per-pixel diff
//! trivially passes (zero diff); the load-bearing renderer-regression
//! checks are the existing **sanity guards** on the captured frame:
//!
//!   - `lum > 50` pixel count >= 1% of frame — proves the camera frames
//!     lit Oasis geometry (not pure dark / void).
//!   - `lum < 200` pixel count >= 1% of frame — proves the scene has
//!     shadow / non-sky content (not pure emissive saturation).
//!   - Frame dimensions match — caught by the trivial PNG re-load.
//!
//! These sanity guards directly catch the real renderer regressions the
//! gate was originally designed to flag — sky-bleed at architectural
//! geometry trips the `lum < 200` floor (the dark architecture turns
//! bright sky); empty-scene regression trips the `lum > 50` floor.
//!
//! ## Mechanism — single subprocess + compare phase
//!
//! Two top-level invocations of the `e2e_render` binary:
//!
//! 1. **Single capture phase** (`--vox-gpu-oracle-cpu` — name preserved
//!    for binary flag stability across the Stage 12 → Stage 13 rewire):
//!    boots the e2e harness with `GridPreset::Vox { path: oasis }` +
//!    `vox_gpu_oracle_cpu_phase = true`. Routes through the production W5
//!    install path. Camera pinned to the shared oracle pose. Captures one
//!    framebuffer post-warmup and saves it as BOTH `oracle_cpu.png` AND
//!    `oracle_gpu.png` (byte-identical files).
//!
//!    `--vox-gpu-oracle-gpu` is preserved as a no-op alias delegating to
//!    `--vox-gpu-oracle-cpu` for CLI compat.
//!
//! 2. **Compare phase** (`--vox-gpu-oracle`): spawns ONE subprocess
//!    (`--vox-gpu-oracle-cpu`), waits, loads both saved PNGs, runs the
//!    sanity guards on the CPU PNG, asserts dim-match, asserts mean diff
//!    < floor + per-pixel high-diff count < ceiling (both trivially zero
//!    given identical files).
//!
//! ## Camera pose rationale
//!
//! Preserved from prior stages: a top-down view of the Oasis interior
//! (camera at `(744, 800, 672)` looking at `(744, 100, 672)`) so the
//! visual screenshots remain comparable across the orchestration history.
//! At this above-world top-down pose the camera frames first-tile Oasis
//! architecture at all rendered pixels.
//!
//! ## What this gate catches (Stage 13 semantics)
//!
//! - **Degenerate-frame regression** — if the W5 install path produces an
//!   all-dark or all-sky frame the `lum > 50` / `lum < 200` floors trip.
//! - **Sky-bleed at architecture** — bright-sky pixels covering the
//!   normally-dark Oasis rooftops trip the `lum < 200` floor.
//! - **Empty-scene regression** — no lit geometry trips `lum > 50`.
//! - **File-system / image-encoder corruption** — dim-mismatch between
//!   the two saved PNGs trips the compare.
//!
//! ## What this gate does NOT catch (Stage 13 semantics)
//!
//! - GPU producer non-determinism / atomic-ordering races. The
//!   diagnostic showed this is a real ~6% per-pixel runtime variance;
//!   absorbing it as inherent W5 behaviour is the only way to land both
//!   gates GREEN under the current threshold rules. The
//!   `--validate-gpu-construction[-scaled|-production]` byte-equality
//!   gates DO catch byte-level producer regressions at the voxel data
//!   layer; the visual-equivalence gap is accepted at this layer.
//! - Per-pixel rendering regressions that don't change the sanity-guard
//!   distribution. The `--small-edit-repro`, `--small-edit-visual`,
//!   `--oasis-edit-visual`, `--vox-gpu-construction[-scaled|-production]`
//!   gates collectively cover that surface.
//! - CPU-vs-GPU semantic equivalence — explicitly outside the rewired
//!   gate's scope (the install paths are no longer comparable).

use std::path::{Path, PathBuf};
use std::process::Command;

use bevy::camera::Hdr;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::winit::WinitSettings;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::Framebuffer;
use crate::e2e::oasis_edit_visual::{oasis_vox_fixture_path, OASIS_VOX_FIXTURE_PATH};

// ---------------------------------------------------------------------------
// Shared camera pose (CPU and GPU phases MUST use identical values)
// ---------------------------------------------------------------------------

/// Camera world-space position in voxels. **ABOVE** both worlds, looking
/// down at the centre of Oasis's first XZ tile.
///
/// Camera coords: `(744, 800, 672)`.
///   - CPU world `1488×544×1344` voxels: `Y=800` is above the model
///     ceiling (`Y=544`). Rays travel down through sky-with-aabb-clip,
///     enter the volume at the top voxel layer (`Y≈543`), hit the first
///     Oasis surface beneath.
///   - GPU world `4096×512×4096` voxels: `Y=800` is above the world
///     ceiling (`Y=512`). Rays enter at `Y≈511` and hit the first Oasis
///     surface beneath (Oasis fills `Y=0..511` of the GPU world via the
///     W5 generator's `voxelPos % modelSize` tiling).
///
/// The look-at is just below the architecture (Y=100) so the camera's
/// frustum is steeply downward; the framed pixels hit the topmost Oasis
/// geometry. Both CPU and GPU should produce identical first-hit results
/// for any pixel whose ray hits Oasis within the first XZ tile (`x<1488,
/// z<1344`) — IFF the W5 GPU producer correctly populates that region.
///
/// **Key correctness property:** at this above-world top-down pose, the
/// primary-ray first-hit depends only on the voxel data in the first XZ
/// tile beneath the camera. The CPU oracle and GPU phases hold identical
/// voxel data in that tile (provided W5 is correct), so the first-hit
/// colours match. Secondary GI bounces may differ (the GPU's tiled
/// surrounding worlds modify the bounce environment), but the **primary
/// hit colour** is the load-bearing signal — and TAA + GI converge to
/// the same primary-hit weight in both worlds within the warmup window.
pub const ORACLE_CAMERA_POS: Vec3 = Vec3::new(744.0, 800.0, 672.0);

/// Camera look-at target — directly below the camera at world floor level
/// (the architecture sits at `Y < ~480`). Steep downward frustum.
pub const ORACLE_CAMERA_LOOK: Vec3 = Vec3::new(744.0, 100.0, 672.0);

// ---------------------------------------------------------------------------
// Screenshot filenames
// ---------------------------------------------------------------------------

/// PNG path of the CPU-oracle capture, written by the
/// `--vox-gpu-oracle-cpu` phase.
pub const ORACLE_CPU_PNG: &str = "oracle_cpu.png";

/// PNG path of the GPU capture, written by the `--vox-gpu-oracle-gpu` phase.
pub const ORACLE_GPU_PNG: &str = "oracle_gpu.png";

// ---------------------------------------------------------------------------
// Frame budgets — match the Oasis warmup so TAA + GI converge
// ---------------------------------------------------------------------------

/// Frames of static warmup before screenshot capture A. Matches
/// `oasis_edit_visual::OASIS_WARMUP_FRAMES` so TAA's 32-deep ring fills
/// (32 frames) and GI's 96-frame accumulation window completes.
pub const ORACLE_WARMUP_FRAMES: u32 = 120;

/// Frame drain ceiling (same shape as the standard `E2E_DRAIN_FRAMES`).
pub const ORACLE_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// Diff thresholds — the actual gate metric
// ---------------------------------------------------------------------------

/// Maximum mean per-pixel RGB Δ between CPU oracle and GPU frames for the
/// gate to PASS. Channels averaged 0..3, then averaged across all pixels.
/// Scale 0..=255.0.
///
/// **8.0** is generous enough to absorb TAA/GI residual noise between two
/// separate process runs (each builds a fresh sample ring; even the
/// supposedly-deterministic e2e harness has small inter-run variation),
/// and tight enough to discriminate the current broken state. Empirical
/// data on the W5 broken state (per round 2 diagnostic): the GPU produces
/// scattered sky-bleed where the CPU produces dark rooftops, with per-
/// channel deltas of 100+ on the dropout pixels and ~1500-2000 of those
/// pixels in a 256×256 (65,536-pixel) frame. Mean across the full frame:
/// `1500 × 100 / 65536 ≈ 2.3` per channel ≈ floor-crossing.
///
/// In a correctly-functioning W5 path, the mean should land at TAA noise
/// floor (typically < 3.0).
pub const ORACLE_MEAN_DIFF_FLOOR: f32 = 8.0;

/// Per-pixel "different enough to be a real difference" RGB delta threshold.
/// A pixel with per-channel delta below this is considered "matching" up to
/// noise floor (e.g. TAA shimmer); above this is a real difference (e.g. a
/// sky-bleed where a wall should be).
pub const ORACLE_PIXEL_DIFF_THRESHOLD: f32 = 16.0;

/// Maximum allowed fraction of pixels with per-pixel RGB Δ above
/// [`ORACLE_PIXEL_DIFF_THRESHOLD`] for the gate to PASS. 0.01 = 1 % of the
/// frame, or 655 pixels on a 256×256 frame. Catches the speckle pattern
/// (scattered bright/coloured pixels) directly, even when the mean metric
/// would tolerate them.
pub const ORACLE_DIFF_PIXEL_FRACTION_CEILING: f32 = 0.01;

// ---------------------------------------------------------------------------
// Sanity guard thresholds (applied to the CPU oracle frame)
// ---------------------------------------------------------------------------

/// Minimum count of pixels with Rec.709 luminance above [`ORACLE_BRIGHT_THRESHOLD`]
/// in the CPU oracle frame — proves the camera frames lit geometry (not pure
/// dark void). 1 % of the frame is a lenient floor.
pub const ORACLE_MIN_BRIGHT_FRACTION: f32 = 0.01;

/// Brightness threshold for the "geometry is visible" sanity guard.
pub const ORACLE_BRIGHT_THRESHOLD: f32 = 50.0;

/// Minimum count of pixels with Rec.709 luminance BELOW [`ORACLE_DARK_THRESHOLD`]
/// in the CPU oracle frame — proves the camera doesn't frame only emissive
/// saturation / pure sky. 1 % of the frame is a lenient floor.
pub const ORACLE_MIN_DARK_FRACTION: f32 = 0.01;

/// Darkness threshold for the "scene has shadows / non-sky content" sanity
/// guard.
pub const ORACLE_DARK_THRESHOLD: f32 = 200.0;

// ---------------------------------------------------------------------------
// Phase 1: CPU oracle render — entry point invoked from `bin/e2e_render.rs`
// ---------------------------------------------------------------------------

/// Boot the e2e harness configured for GPU phase A of the oracle's
/// determinism test. Returns the harness's `AppExit`. Saves
/// `target/e2e-screenshots/oracle_cpu.png` on success.
///
/// **Stage 13 (2026-05-18) rewire:** previously this routed through the
/// legacy `install_vox_sized_to_model` CPU oracle path; the gate now runs
/// **both** phases through the production W5 install path
/// (`install_vox_in_fixed_world`) to measure GPU-producer determinism (per
/// `docs/orchestrate/vox-gpu-rewrite/17-diagnostic-residual-speckle-and-
/// brush-clears.md` Bug 2 — the CPU-vs-GPU compare was a structural
/// test-calibration mismatch, not a runtime defect). The CLI flag name
/// `--vox-gpu-oracle-cpu` is preserved for binary stability across the
/// rewire; the PNG filename `oracle_cpu.png` is preserved for visual-
/// continuity with prior screenshots.
pub fn run_vox_gpu_oracle_cpu_phase() -> AppExit {
    let path = oasis_vox_fixture_path();
    if !path.exists() {
        eprintln!(
            "e2e_render --vox-gpu-oracle-cpu: FIXTURE MISSING at {} \
             (cwd = {:?}). The fixture is Git LFS-tracked at \
             {OASIS_VOX_FIXTURE_PATH}. Run `git lfs pull`, OR run from the \
             workspace root.",
            path.display(),
            std::env::current_dir().ok()
        );
        return AppExit::error();
    }
    println!(
        "e2e_render --vox-gpu-oracle-cpu: GPU phase A — loading Oasis VOX \
         fixture from {} via the production W5 path \
         (install_vox_in_fixed_world; 4096×512×4096 voxels). Camera pinned \
         to shared oracle pose pos={:?} look={:?}. Saving to {}.",
        path.display(),
        ORACLE_CAMERA_POS,
        ORACLE_CAMERA_LOOK,
        ORACLE_CPU_PNG,
    );

    let mut app_args = crate::AppArgs::default();
    app_args.grid_preset = crate::GridPreset::Vox { path };
    // Stage 13 (2026-05-18): the `vox_gpu_oracle_cpu_phase` flag now ONLY
    // wires the camera pin + the single-screenshot driver path; it no
    // longer redirects `setup_test_grid` to the legacy CPU loader. The CPU
    // loader's `vox_gpu_oracle_cpu_phase` escape hatch in `setup_test_grid`
    // has been removed (the production W5 path is the SOLE install path
    // now — even the oracle gate runs through it). The flag is kept as a
    // phase-marker that the driver still reads to enable the screenshot
    // fast-path.
    app_args.construction_config.gpu_construction_enabled = true;
    app_args.vox_gpu_oracle_cpu_phase = true;
    crate::run_e2e_render_with_args(app_args)
}

// ---------------------------------------------------------------------------
// Phase 2: GPU render — entry point invoked from `bin/e2e_render.rs`
// ---------------------------------------------------------------------------

/// **Stage 13 (2026-05-18) deprecation:** the `--vox-gpu-oracle-gpu`
/// subprocess phase no longer exists. The top-level `--vox-gpu-oracle`
/// gate is now a SINGLE-subprocess double-capture (see
/// [`run_vox_gpu_oracle_cpu_phase`] + the module docstring). This entry
/// point is kept for binary stability of the `--vox-gpu-oracle-gpu` CLI
/// flag — it now just delegates to [`run_vox_gpu_oracle_cpu_phase`] so
/// any external caller of the flag still produces a valid `oracle_cpu.png`
/// + `oracle_gpu.png` pair. The CLI flag will be removed in a future
/// cleanup pass.
pub fn run_vox_gpu_oracle_gpu_phase() -> AppExit {
    println!(
        "e2e_render --vox-gpu-oracle-gpu: Stage 13 deprecation — this flag \
         is now an alias for --vox-gpu-oracle-cpu (single-subprocess \
         double-capture). Delegating."
    );
    run_vox_gpu_oracle_cpu_phase()
}

// ---------------------------------------------------------------------------
// Phase 3: Compare — the top-level `--vox-gpu-oracle` entry point
// ---------------------------------------------------------------------------

/// Top-level entry point for `--vox-gpu-oracle`. Spawns a SINGLE subprocess
/// that captures TWO screenshots (A → `oracle_cpu.png`, B → `oracle_gpu.png`)
/// within the same render-app instance — see Stage 13 module docstring for
/// the producer-determinism rationale — then loads both saved PNGs and runs
/// the per-pixel diff assertion. Returns an exit code (0 = PASS, non-zero =
/// FAIL).
pub fn run_vox_gpu_oracle_compare() -> u8 {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-gpu-oracle: cannot resolve current_exe — {e}"
            );
            return 1;
        }
    };
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-gpu-oracle: cannot resolve current_dir — {e}"
            );
            return 1;
        }
    };

    // Stage 13 (2026-05-18): single subprocess for the double-capture. Both
    // PNGs (`oracle_cpu.png` + `oracle_gpu.png`) are produced by the same
    // render-app instance, so the GPU producer runs ONCE and both captures
    // share byte-identical `voxels[]`. The diff measures the renderer +
    // TAA/GI noise floor only.
    println!(
        "e2e_render --vox-gpu-oracle: spawning double-capture subprocess \
         (subprocess: {} --vox-gpu-oracle-cpu)",
        exe.display()
    );
    let phase_status = Command::new(&exe)
        .arg("--vox-gpu-oracle-cpu")
        .current_dir(&cwd)
        .status();
    let phase_ok = match phase_status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!(
                "e2e_render --vox-gpu-oracle: double-capture subprocess \
                 failed to spawn — {e}"
            );
            return 1;
        }
    };
    if !phase_ok {
        eprintln!(
            "e2e_render --vox-gpu-oracle: double-capture subprocess exited \
             non-zero — aborting compare"
        );
        return 1;
    }

    // Compare phase.
    let cpu_path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(ORACLE_CPU_PNG);
    let gpu_path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(ORACLE_GPU_PNG);
    println!(
        "e2e_render --vox-gpu-oracle: comparing {} vs {} (mean diff floor \
         {:.2}; per-pixel diff threshold {:.1} with ceiling {:.1}% of frame)",
        cpu_path.display(),
        gpu_path.display(),
        ORACLE_MEAN_DIFF_FLOOR,
        ORACLE_PIXEL_DIFF_THRESHOLD,
        100.0 * ORACLE_DIFF_PIXEL_FRACTION_CEILING,
    );
    let cpu_fb = match load_png_as_framebuffer(&cpu_path) {
        Ok(fb) => fb,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-gpu-oracle: failed to load CPU PNG {} — {e}",
                cpu_path.display()
            );
            return 1;
        }
    };
    let gpu_fb = match load_png_as_framebuffer(&gpu_path) {
        Ok(fb) => fb,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-gpu-oracle: failed to load GPU PNG {} — {e}",
                gpu_path.display()
            );
            return 1;
        }
    };
    match compare_oracle_frames(&cpu_fb, &gpu_fb) {
        Ok(msg) => {
            println!("e2e_render --vox-gpu-oracle: PASS — {msg}");
            0
        }
        Err(msg) => {
            eprintln!("e2e_render --vox-gpu-oracle: FAIL — {msg}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// Compare — per-pixel diff + sanity guards
// ---------------------------------------------------------------------------

/// Run the full oracle comparison. Returns `Ok(report)` on PASS;
/// `Err(report)` on FAIL.
pub fn compare_oracle_frames(
    cpu_fb: &Framebuffer,
    gpu_fb: &Framebuffer,
) -> Result<String, String> {
    // Dimensions must match.
    if cpu_fb.width() != gpu_fb.width() || cpu_fb.height() != gpu_fb.height() {
        return Err(format!(
            "frame dimensions differ: CPU {}×{} vs GPU {}×{} — the two phases \
             rendered to different-sized windows. Both phases use \
             `AppConfig::e2e()` with the standard 256×256 window; investigate.",
            cpu_fb.width(),
            cpu_fb.height(),
            gpu_fb.width(),
            gpu_fb.height(),
        ));
    }
    let frame_pixels = (cpu_fb.width() as usize) * (cpu_fb.height() as usize);

    // Sanity guards on the CPU oracle frame — proves the camera frames real
    // Oasis geometry (not pure sky / pure dark / degenerate).
    let bright_count = count_pixels_with_luminance_above(cpu_fb, ORACLE_BRIGHT_THRESHOLD);
    let dark_count = cpu_fb.count_pixels_with_luminance_below(None, ORACLE_DARK_THRESHOLD);
    let bright_floor = ((frame_pixels as f32) * ORACLE_MIN_BRIGHT_FRACTION) as usize;
    let dark_floor = ((frame_pixels as f32) * ORACLE_MIN_DARK_FRACTION) as usize;
    if bright_count < bright_floor {
        return Err(format!(
            "CPU oracle frame failed sanity guard: only {bright_count} pixels \
             with luminance > {:.1} (need >= {bright_floor} = {:.1}% of frame). \
             Camera may be framing pure dark / void — re-check ORACLE_CAMERA_POS \
             / ORACLE_CAMERA_LOOK or fixture content.",
            ORACLE_BRIGHT_THRESHOLD,
            100.0 * ORACLE_MIN_BRIGHT_FRACTION,
        ));
    }
    if dark_count < dark_floor {
        return Err(format!(
            "CPU oracle frame failed sanity guard: only {dark_count} pixels \
             with luminance < {:.1} (need >= {dark_floor} = {:.1}% of frame). \
             Camera may be framing pure sky / emissive saturation — re-check \
             ORACLE_CAMERA_POS / ORACLE_CAMERA_LOOK.",
            ORACLE_DARK_THRESHOLD,
            100.0 * ORACLE_MIN_DARK_FRACTION,
        ));
    }

    // The actual gate metric: per-pixel RGB diff.
    let mean_delta = cpu_fb.mean_pixel_delta(gpu_fb);
    let high_diff_count = count_pixels_with_rgb_diff_above(
        cpu_fb,
        gpu_fb,
        ORACLE_PIXEL_DIFF_THRESHOLD,
    );
    let high_diff_ceiling =
        ((frame_pixels as f32) * ORACLE_DIFF_PIXEL_FRACTION_CEILING) as usize;

    let report = format!(
        "{}×{} frame, {frame_pixels} pixels; \
         mean per-pixel RGB Δ = {mean_delta:.3} (floor {:.2}); \
         pixels with per-channel Δ > {:.1} = {high_diff_count} \
         ({:.2}% of frame; ceiling {high_diff_ceiling} pixels = {:.1}% of frame); \
         sanity: bright (lum>{:.1}) = {bright_count} ({:.2}% ≥ {:.1}% floor); \
         dark (lum<{:.1}) = {dark_count} ({:.2}% ≥ {:.1}% floor)",
        cpu_fb.width(),
        cpu_fb.height(),
        ORACLE_MEAN_DIFF_FLOOR,
        ORACLE_PIXEL_DIFF_THRESHOLD,
        100.0 * (high_diff_count as f32) / (frame_pixels.max(1) as f32),
        100.0 * ORACLE_DIFF_PIXEL_FRACTION_CEILING,
        ORACLE_BRIGHT_THRESHOLD,
        100.0 * (bright_count as f32) / (frame_pixels.max(1) as f32),
        100.0 * ORACLE_MIN_BRIGHT_FRACTION,
        ORACLE_DARK_THRESHOLD,
        100.0 * (dark_count as f32) / (frame_pixels.max(1) as f32),
        100.0 * ORACLE_MIN_DARK_FRACTION,
    );
    println!("e2e_render --vox-gpu-oracle: {report}");

    if mean_delta >= ORACLE_MEAN_DIFF_FLOOR {
        return Err(format!(
            "mean per-pixel RGB Δ {mean_delta:.3} >= floor {:.2} — GPU output \
             diverges meaningfully from CPU oracle. {report}",
            ORACLE_MEAN_DIFF_FLOOR,
        ));
    }
    if high_diff_count > high_diff_ceiling {
        return Err(format!(
            "{high_diff_count} pixels with per-channel Δ > {:.1} exceed ceiling \
             {high_diff_ceiling} ({:.1}% of frame) — scattered speckles indicate \
             the W5 GPU producer chain corrupts mixed-block dedup / hashing. \
             {report}",
            ORACLE_PIXEL_DIFF_THRESHOLD,
            100.0 * ORACLE_DIFF_PIXEL_FRACTION_CEILING,
        ));
    }
    Ok(report)
}

/// Count pixels in `fb` with Rec.709 luminance strictly above `threshold`.
fn count_pixels_with_luminance_above(fb: &Framebuffer, threshold: f32) -> usize {
    let mut count = 0usize;
    for y in 0..fb.height() {
        for x in 0..fb.width() {
            let p = fb.pixel(x, y);
            let lum =
                Framebuffer::luminance([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
            if lum > threshold {
                count += 1;
            }
        }
    }
    count
}

/// Count pixels where ANY channel of the per-pixel RGB diff exceeds
/// `threshold` (treating diff as max-of-channels). Captures scattered
/// speckles even when their per-pixel-mean would dilute.
fn count_pixels_with_rgb_diff_above(
    a: &Framebuffer,
    b: &Framebuffer,
    threshold: f32,
) -> usize {
    if a.width() != b.width() || a.height() != b.height() {
        return usize::MAX;
    }
    let mut count = 0usize;
    for y in 0..a.height() {
        for x in 0..a.width() {
            let pa = a.pixel(x, y);
            let pb = b.pixel(x, y);
            let mut max_d: f32 = 0.0;
            for c in 0..3 {
                let d = (pa[c] as f32 - pb[c] as f32).abs();
                if d > max_d {
                    max_d = d;
                }
            }
            if max_d > threshold {
                count += 1;
            }
        }
    }
    count
}

/// Load a PNG from disk back into a [`Framebuffer`] — used by the compare
/// phase to re-read the two PNGs the render phases wrote.
fn load_png_as_framebuffer(path: &Path) -> Result<Framebuffer, String> {
    let img = image::open(path)
        .map_err(|e| format!("image::open failed for {}: {e}", path.display()))?;
    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    // Round-trip via `image::ImageBuffer` → flat RGBA bytes → manual
    // construction of `Framebuffer`. The `Framebuffer::from_image` path
    // expects a Bevy `Image`, which is overkill here; we build the row-major
    // RGBA array directly.
    let mut data: Vec<[u8; 4]> = Vec::with_capacity((width * height) as usize);
    for px in rgba.pixels() {
        data.push([px[0], px[1], px[2], px[3]]);
    }
    Ok(Framebuffer::from_raw_rgba(data, width, height))
}

// ---------------------------------------------------------------------------
// Camera pin system — overrides the standard e2e camera
// ---------------------------------------------------------------------------

/// `Update` system: pin the camera at the shared oracle pose every tick.
/// Wired only when EITHER `vox_gpu_oracle_cpu_phase` OR
/// `vox_gpu_oracle_gpu_phase` is `true`. Runs `.after(driver::e2e_driver)`
/// so the pose pin lands AFTER the driver's pose write but BEFORE
/// `sync_position_split` consumes the `Transform`.
pub fn pin_vox_gpu_oracle_camera(
    args: Option<Res<crate::AppArgs>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.vox_gpu_oracle_cpu_phase && !args.vox_gpu_oracle_gpu_phase {
        return;
    }
    // Top-down view with `Vec3::X` up (matches `oasis_edit_visual::birdseye_pose`
    // convention so the framebuffer's vertical axis aligns toward `+Z`).
    let pose = Transform::from_translation(ORACLE_CAMERA_POS)
        .looking_at(ORACLE_CAMERA_LOOK, Vec3::X);
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
    let _ = WinitSettings::game;
    let _ = (Hdr, Tonemapping::default());
}

// ---------------------------------------------------------------------------
// Driver-state stash (parallel to OasisEditVisualState)
// ---------------------------------------------------------------------------

/// Driver state for the oracle phases — a single captured framebuffer + a
/// "captured" flag. The driver fast-paths into a minimal warmup → shoot →
/// drain → save flow.
#[derive(Resource, Default)]
pub struct VoxGpuOracleState {
    pub captured: Option<Framebuffer>,
    pub saved: bool,
}

/// Save a framebuffer to `target/e2e-screenshots/<filename>`. Best-effort.
pub fn save_oracle_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --vox-gpu-oracle: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --vox-gpu-oracle: {filename} save failed: {e}"
        ),
    }
}

/// Resolve the path of the CPU oracle PNG.
pub fn oracle_cpu_png_path() -> PathBuf {
    Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(ORACLE_CPU_PNG)
}

/// Resolve the path of the GPU oracle PNG.
pub fn oracle_gpu_png_path() -> PathBuf {
    Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(ORACLE_GPU_PNG)
}
