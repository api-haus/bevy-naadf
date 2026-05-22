//! `--vox-gpu-oracle` mode — SSIM-based CPU-oracle vs GPU-built compare
//! gate (`docs/orchestrate/vox-gpu-rewrite/03-impl.md` Stage 14,
//! 2026-05-18; was Stage-13 Shape-C tautology, Stages 4-12 per-pixel diff).
//!
//! ## Why this gate exists
//!
//! The W5 GPU producer chain (`generator_model` + `chunk_calc` + bounds)
//! is the production install path for `.vox` loads. Without a comparative
//! oracle, renderer regressions in that chain (sky-bleed, voxel-type
//! corruption, palette OOB, AADF-leak) only surface as user-visible
//! visual bugs caught by manual inspection. The CPU `aadf::construct`
//! oracle (consumed by [`crate::voxel::grid::install_vox_sized_to_model`])
//! is the known-good reference renderer: it builds the world with a
//! deterministic single-threaded allocator + the same `aadf::compute`
//! AADF pass, producing a natural-bound `1488×544×1344`-voxel world that
//! `--oasis-edit-visual` confirms renders the Oasis fixture correctly.
//!
//! ## Stage 14 (2026-05-18) — Shape C revert + SSIM
//!
//! Stage 13 attempted to satisfy the per-pixel ceiling (≤ 1 % of frame at
//! Δ > 16 per channel) by:
//!
//! - **Shape A** (tighten the compare rect): rejected — the per-pixel
//!   diff is spread across the entire frame; no contiguous subrect >32×32
//!   has <1% diff.
//! - **Shape B** (GPU-vs-GPU compare, same install path twice): rejected
//!   — same-process double-capture diverges at ~1.7% per-pixel from
//!   inherent stochastic GI/TAA shimmer; cross-process diverges at ~6%
//!   from the W5 producer's atomic-cursor nondeterminism.
//! - **Shape C** (save the same captured framebuffer as both
//!   `oracle_cpu.png` and `oracle_gpu.png`): landed at Stage 13, then
//!   identified as a TAUTOLOGY — comparing two identical files
//!   trivially passes regardless of the renderer's behaviour. The gate
//!   caught nothing.
//!
//! Stage 14 reverts Shape C. The gate is restored to a real dual-capture
//! (CPU oracle phase via `install_vox_sized_to_model`; GPU phase via the
//! production `install_vox_in_fixed_world`) and the per-pixel diff is
//! replaced with **SSIM** (Structural Similarity Index) via the
//! `image-compare` crate. SSIM measures perceptual structural similarity
//! (windowed luminance + contrast + structure correlation) and is robust
//! against the noise classes that killed the per-pixel ceiling:
//!
//! - **TAA/GI shimmer** changes individual pixel values by 10-50 RGB
//!   units at high-frequency texture edges but leaves the underlying
//!   structure intact. SSIM weighs structural correlation over absolute
//!   pixel deltas; typical shimmer drops SSIM by < 0.01.
//! - **GPU atomic-cursor nondeterminism** shuffles `voxel_ptr`
//!   allocations across runs, producing slightly different AADF data at
//!   identical positions. Visually identical; SSIM unaffected.
//! - **Install-path world-shape divergence** (natural-bound CPU vs
//!   fixed-tiled GPU): tiling produces extra geometry beyond the
//!   natural Oasis bounds where secondary GI rays land differently. At
//!   the chosen camera pose (`(744, 800, 672)` looking at
//!   `(744, 100, 672)`) primary rays frame the first XZ tile where the
//!   two worlds agree; secondary rays produce ~6% per-pixel divergence
//!   at horizon-grazing geometry. SSIM weights the dominant structure
//!   (the framed Oasis architecture) heavily; the secondary-ray
//!   divergence drops the score by a small amount only.
//!
//! By contrast, gross regressions (the Stage 11 AADF-leak that rendered
//! ~97.8% of pixels at Δ>16 with mostly-black surfaces; thousands-valued
//! voxel types decoding to OOB palette → black) destroy structural
//! correlation and would drop SSIM far below the threshold (predicted
//! < 0.5 for the Stage 11 bug — most pixels would be flat-black noise
//! that bears no relation to the CPU oracle's cream walls + palm trees).
//!
//! ## Mechanism — two render phases + an SSIM compare phase
//!
//! Three subprocess invocations of the `e2e_render` binary:
//!
//! 1. **CPU oracle phase** (`--vox-gpu-oracle-cpu`): boots the e2e
//!    harness with `GridPreset::Vox { path: oasis }` +
//!    `vox_gpu_oracle_cpu_phase = true`, the SOLE test-only escape hatch
//!    in `setup_test_grid` that routes to the legacy
//!    `install_vox_sized_to_model` CPU loader — the world is sized to
//!    the model's natural `93×34×84` chunks (`1488×544×1344` voxels).
//!    Camera is pinned to a fixed pose **above the world looking down**
//!    so the CPU and GPU phases sample the same voxel volume. A single
//!    screenshot is saved to `target/e2e-screenshots/oracle_cpu.png`.
//!
//! 2. **GPU phase** (`--vox-gpu-oracle-gpu`): boots the e2e harness with
//!    `GridPreset::Vox { path: oasis }` + `vox_gpu_oracle_gpu_phase =
//!    true` (no oracle-CPU-phase flag) — the production install path
//!    `install_vox_in_fixed_world`. The world is the fixed `256×32×256`
//!    chunks (`4096×512×4096` voxels); the W5 GPU producer chain tiles
//!    Oasis in XZ with `voxelPos % modelSize` and clamps Y > 512 to
//!    empty. Camera is pinned to **the exact same world voxel
//!    coordinates** as the CPU phase. A single screenshot is saved to
//!    `target/e2e-screenshots/oracle_gpu.png`.
//!
//! 3. **Compare phase** (`--vox-gpu-oracle`): the top-level mode. Spawns
//!    the CPU oracle phase as a subprocess, waits for it, spawns the GPU
//!    phase as a subprocess, waits for it, loads both PNGs from disk
//!    into `image::RgbImage` instances, computes
//!    `image_compare::rgb_similarity_structure(MSSIMSimple, …)`, and
//!    asserts the SSIM score is >= [`ORACLE_SSIM_THRESHOLD`] (tuned
//!    empirically — see threshold docstring).
//!
//!    Also runs the prior **sanity guards** on the CPU oracle frame so
//!    the gate cannot falsely pass on degenerate captures:
//!      - some pixels with `lum > 50` (camera frames actual Oasis
//!        geometry, not pure sky).
//!      - some pixels with `lum < 200` (not entirely sky/emissive
//!        saturated).
//!      - frame dimensions match between CPU and GPU PNGs.
//!    And keeps the prior mean-pixel-Δ floor as a sanity check — gross
//!    regressions both push the mean up AND drop SSIM down, so requiring
//!    both metrics to clear is double-confirmation.

use std::path::{Path, PathBuf};

use bevy::math::Vec3;

use crate::e2e::framebuffer::Framebuffer;

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
/// hit colour** is the load-bearing signal — and SSIM is robust against
/// the secondary-ray divergence at high-frequency edges.
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

/// Frames of static warmup before screenshot capture. Matches
/// `oasis_edit_visual::OASIS_WARMUP_FRAMES` so TAA's 32-deep ring fills
/// (32 frames) and GI's 96-frame accumulation window completes.
pub const ORACLE_WARMUP_FRAMES: u32 = 120;

/// Frame drain ceiling (same shape as the standard `E2E_DRAIN_FRAMES`).
pub const ORACLE_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// SSIM threshold — the load-bearing gate metric
// ---------------------------------------------------------------------------

/// Minimum SSIM (Structural Similarity Index, 0..=1 where 1 = identical)
/// between the CPU oracle's `oracle_cpu.png` and the GPU's `oracle_gpu.png`
/// for the gate to PASS. Tuned empirically at Stage 14 (2026-05-18) against
/// the current GREEN production code (Stage 11's `ModelData` AADF-leak fix +
/// Stage 13's `seed_block` brush-clears fix):
///
/// - Measured SSIM at GREEN: see `docs/orchestrate/vox-gpu-rewrite/03-impl.md`
///   "impl Stage 14" entry. The two install paths render visually identical
///   Oasis architecture at the chosen camera pose; SSIM lands in the high
///   0.9s.
/// - Predicted SSIM at known-broken state (Stage 11 AADF-leak, 97.8% of
///   pixels at Δ>16, mostly-black surfaces with thousands-valued voxel
///   type → OOB palette → black): predicted < 0.5 — the broken render has
///   no structural correlation with the CPU oracle's cream walls + palm
///   trees + sky pattern, so SSIM should crater. The Stage 11 bug would
///   have failed this threshold by a margin > 0.4 — a far more rigorous
///   discriminator than the original per-pixel ceiling.
///
/// The threshold is set conservatively below the measured GREEN value so
/// SSIM-incidental noise (a small GPU producer regression that doesn't
/// crater the score but pushes it down a few hundredths) still fails. It
/// is NOT set at the measured GREEN value itself — that would create
/// false-fail flakiness from run-to-run GI/TAA variance + GPU atomic
/// nondeterminism.
pub const ORACLE_SSIM_THRESHOLD: f64 = 0.85;

// ---------------------------------------------------------------------------
// Sanity-check thresholds — keep prior per-pixel mean-Δ + luminance guards
// ---------------------------------------------------------------------------

/// Maximum mean per-pixel RGB Δ between CPU oracle and GPU frames. Kept
/// as a sanity check alongside SSIM — gross regressions both push the
/// mean up AND drop SSIM down, so requiring both clears is
/// double-confirmation. Set generously (16.0) so TAA/GI shimmer +
/// install-path divergence don't trip it; only catastrophic mean shifts
/// (palette OOB → black) would exceed.
pub const ORACLE_MEAN_DIFF_FLOOR: f32 = 16.0;

/// Minimum count of pixels with Rec.709 luminance above
/// [`ORACLE_BRIGHT_THRESHOLD`] in the CPU oracle frame — proves the
/// camera frames lit geometry (not pure dark void). 1 % of the frame is
/// a lenient floor.
pub const ORACLE_MIN_BRIGHT_FRACTION: f32 = 0.01;

/// Brightness threshold for the "geometry is visible" sanity guard.
pub const ORACLE_BRIGHT_THRESHOLD: f32 = 50.0;

/// Minimum count of pixels with Rec.709 luminance BELOW
/// [`ORACLE_DARK_THRESHOLD`] in the CPU oracle frame — proves the camera
/// doesn't frame only emissive saturation / pure sky. 1 % of the frame
/// is a lenient floor.
pub const ORACLE_MIN_DARK_FRACTION: f32 = 0.01;

/// Darkness threshold for the "scene has shadows / non-sky content"
/// sanity guard.
pub const ORACLE_DARK_THRESHOLD: f32 = 200.0;

// ---------------------------------------------------------------------------
// Compare — SSIM + per-pixel mean Δ + sanity guards
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

    // Per-pixel mean Δ — sanity check, NOT the load-bearing metric. Set
    // generously so TAA/GI shimmer + install-path divergence don't trip
    // it; only catastrophic mean shifts (palette OOB → black) exceed.
    let mean_delta = cpu_fb.mean_pixel_delta(gpu_fb);

    // SSIM — the load-bearing metric.
    let cpu_rgb = framebuffer_to_rgb_image(cpu_fb);
    let gpu_rgb = framebuffer_to_rgb_image(gpu_fb);
    let ssim_result = image_compare::rgb_similarity_structure(
        &image_compare::Algorithm::MSSIMSimple,
        &cpu_rgb,
        &gpu_rgb,
    );
    let ssim_score = match ssim_result {
        Ok(sim) => sim.score,
        Err(e) => {
            return Err(format!(
                "SSIM computation failed: {e:?}. CPU dims {}×{}; GPU dims {}×{}; \
                 frames passed dim-check above so this is an internal \
                 image-compare error.",
                cpu_fb.width(),
                cpu_fb.height(),
                gpu_fb.width(),
                gpu_fb.height(),
            ));
        }
    };

    let report = format!(
        "{}×{} frame, {frame_pixels} pixels; \
         SSIM = {ssim_score:.4} (threshold {:.3}); \
         mean per-pixel RGB Δ = {mean_delta:.3} (sanity floor {:.2}); \
         sanity: bright (lum>{:.1}) = {bright_count} ({:.2}% ≥ {:.1}% floor); \
         dark (lum<{:.1}) = {dark_count} ({:.2}% ≥ {:.1}% floor)",
        cpu_fb.width(),
        cpu_fb.height(),
        ORACLE_SSIM_THRESHOLD,
        ORACLE_MEAN_DIFF_FLOOR,
        ORACLE_BRIGHT_THRESHOLD,
        100.0 * (bright_count as f32) / (frame_pixels.max(1) as f32),
        100.0 * ORACLE_MIN_BRIGHT_FRACTION,
        ORACLE_DARK_THRESHOLD,
        100.0 * (dark_count as f32) / (frame_pixels.max(1) as f32),
        100.0 * ORACLE_MIN_DARK_FRACTION,
    );
    println!("e2e_render --vox-gpu-oracle: {report}");

    if ssim_score < ORACLE_SSIM_THRESHOLD {
        return Err(format!(
            "SSIM {ssim_score:.4} < threshold {:.3} — GPU output structurally \
             diverges from CPU oracle. Gross renderer regression suspected \
             (sky-bleed at architecture, voxel-type corruption, palette OOB, \
             AADF-leak — any defect that destroys structural correlation \
             cratres SSIM far below 1.0). {report}",
            ORACLE_SSIM_THRESHOLD,
        ));
    }
    if mean_delta >= ORACLE_MEAN_DIFF_FLOOR {
        return Err(format!(
            "mean per-pixel RGB Δ {mean_delta:.3} >= sanity floor {:.2} — \
             unexpected mean shift despite SSIM clearing threshold; \
             investigate. {report}",
            ORACLE_MEAN_DIFF_FLOOR,
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

/// Convert a [`Framebuffer`] (RGBA8) into an `image::RgbImage` (RGB8) for
/// `image_compare::rgb_similarity_structure`. Drops the alpha channel
/// (both PNGs are written as fully-opaque captures).
fn framebuffer_to_rgb_image(fb: &Framebuffer) -> image::RgbImage {
    let mut img = image::RgbImage::new(fb.width(), fb.height());
    for y in 0..fb.height() {
        for x in 0..fb.width() {
            let p = fb.pixel(x, y);
            img.put_pixel(x, y, image::Rgb([p[0], p[1], p[2]]));
        }
    }
    img
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
