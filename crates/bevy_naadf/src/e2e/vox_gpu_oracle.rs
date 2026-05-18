//! `--vox-gpu-oracle` mode — per-pixel CPU-oracle vs GPU-built diff gate
//! (`docs/orchestrate/vox-gpu-rewrite/03-impl.md` Stage 4, 2026-05-18).
//!
//! ## Why this gate exists
//!
//! The prior `--vox-gpu-construction` gate uses luminance-based metrics
//! (per-pixel rect mean RGB Δ, near-black pixel count) at a top-down birdseye
//! pose. Round 3's diagnostic established that those metrics are
//! **insensitive** to the visible W5 inversion symptom at any pose the prior
//! gate authors picked: the bug manifests as scattered bright sky-bleed
//! speckles and green-coloured pixels through dark rooftops, which
//! luminance-based discriminators dilute against legitimate scene content.
//!
//! Worse, the prior gate was repeatedly "fixed" by moving the camera or the
//! threshold rather than fixing the W5 GPU producer chain. The user's
//! directive (2026-05-18) is a metric that **cannot be gamed**: per-pixel RGB
//! diff against a known-good CPU oracle. The CPU oracle is
//! `install_vox_sized_to_model` — the same legacy path
//! `--oasis-edit-visual` uses, which the user confirmed renders Oasis
//! correctly.
//!
//! ## Mechanism — two render phases + a per-pixel diff phase
//!
//! Running two distinct bevy apps in one process is non-trivial (winit, GPU
//! device, etc.) — this gate uses **three subprocess invocations** of the
//! `e2e_render` binary:
//!
//! 1. **CPU oracle phase** (`--vox-gpu-oracle-cpu`): boots the e2e harness
//!    with `GridPreset::Vox { path: oasis }` + `vox_gpu_oracle_cpu_phase =
//!    true`, the SOLE test-only escape hatch in `setup_test_grid` that
//!    routes to the legacy `install_vox_sized_to_model` CPU loader — the
//!    world is sized to the model's natural `93×34×84` chunks
//!    (`1488×544×1344` voxels). Camera is pinned to a fixed pose **inside
//!    the world at Y < 512** so the CPU and GPU phases sample the same
//!    voxel volume. A single screenshot is saved to
//!    `target/e2e-screenshots/oracle_cpu.png`.
//!
//! 2. **GPU phase** (`--vox-gpu-oracle-gpu`): boots the e2e harness with
//!    `GridPreset::Vox { path: oasis }` (no oracle-CPU-phase flag) — the
//!    production install path `install_vox_in_fixed_world`. The world is
//!    the fixed `256×32×256` chunks (`4096×512×4096` voxels); the W5 GPU
//!    producer chain tiles Oasis in XZ with `voxelPos % modelSize` and
//!    clamps Y > 0 tiles to empty.
//!    Camera is pinned to **the exact same world voxel coordinates** as the
//!    CPU phase. Because the chosen camera coords sit inside the **first XZ
//!    tile** (`x ∈ [0, 1488)`, `z ∈ [0, 1344)`) and **at Y < 512**, the
//!    voxel volume the camera sees is byte-identical in the two worlds — IFF
//!    the W5 GPU path is correct. A single screenshot is saved to
//!    `target/e2e-screenshots/oracle_gpu.png`.
//!
//! 3. **Compare phase** (`--vox-gpu-oracle`): the top-level mode. Spawns the
//!    CPU oracle phase as a subprocess, waits for it, spawns the GPU phase
//!    as a subprocess, waits for it, loads both PNGs from disk, computes
//!    per-pixel RGB diff, asserts:
//!      - mean per-pixel RGB Δ < [`ORACLE_MEAN_DIFF_FLOOR`] (8.0 on a 0..=255
//!        scale; small enough to catch the current inversion artefacts,
//!        large enough to absorb TAA/GI run-to-run noise);
//!      - count of pixels with RGB Δ > [`ORACLE_PIXEL_DIFF_THRESHOLD`]
//!        (16.0 per channel) is below 1 % of the frame (catches scattered
//!        speckles even when their per-channel magnitude doesn't move the
//!        mean very far).
//!    Also runs **sanity guards** on the CPU oracle frame so the gate
//!    cannot falsely pass on degenerate captures:
//!      - some pixels with `lum > 50` (camera frames actual Oasis geometry,
//!        not pure sky);
//!      - some pixels with `lum < 200` (not entirely sky/emissive saturated);
//!      - frame dimensions match between CPU and GPU PNGs.
//!
//! ## Camera pose rationale
//!
//! The shared camera pose is a key constraint. It must:
//!   - Sit at the SAME world voxel coordinates in both worlds (otherwise the
//!     rendered geometry is fundamentally different, not byte-comparable).
//!   - Frame a region that exists IDENTICALLY in both worlds.
//!
//! The legacy CPU path's world is `1488×544×1344` voxels (Oasis natural
//! size). The W5 GPU path's world is `4096×512×4096` voxels — but at any
//! world voxel position `(x, y, z)` with `x < 1488`, `y < 512`, `z < 1344`,
//! the W5 path's tiled-and-Y-clamped output should equal the CPU path's
//! direct-load output at `(x, y, z)` (the X/Z are within the first tile so
//! `voxelPos % modelSize` is identity; the Y is below the world ceiling so
//! the Y-clamp doesn't fire).
//!
//! The chosen pose:
//!   - Camera position: `(744, 400, 672)` — middle of XZ overlap, Y < 512.
//!   - Look-at: `(744, 100, 672)` — looks **down** at the Oasis interior.
//!   - Up: `Vec3::X` (matches `oasis_edit_visual::birdseye_pose` convention
//!     so the resulting camera Y-axis aligns toward `+Z`, the framebuffer's
//!     up direction).
//!
//! This is a **top-down view of the Oasis interior** — frames the same voxel
//! geometry the user's broken-state screenshots show (Oasis rooftops with
//! scattered sky-bleed holes). The CPU oracle should render this cleanly;
//! the broken W5 GPU should show the speckle artefacts.
//!
//! ## What this gate catches
//!
//! - **Scattered mixed-block dropout** (the `06`/`07`/`08` diagnostic class)
//!   — bright sky-bleed pixels where solid walls should render.
//! - **Wrong dedup hits** — pixels with the wrong material colour (the green
//!   specks in the broken-state screenshots).
//! - **Empty-scene regression** — both phases pass the sanity guards
//!   independently, but if the GPU produces nothing the diff is huge and
//!   the gate trips.
//!
//! ## What this gate does NOT catch
//!
//! - Voxel correctness at world coords OUTSIDE the first XZ tile or above
//!   Y=512 — the camera doesn't see those, so neither phase's framebuffer
//!   touches them. The W5 tiling/clamping semantics are exercised
//!   (`voxelPos % modelSize` collapses to identity inside the first tile;
//!   that's the whole point — we measure correctness on the part of the
//!   world where the two paths SHOULD produce the same bytes).
//! - Anything that's a property of the legacy CPU path itself (it's the
//!   oracle — we trust it).

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

/// Frames of static warmup before screenshot capture. Matches
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

/// Boot the e2e harness configured for the CPU oracle phase. Returns the
/// harness's `AppExit`. Saves `target/e2e-screenshots/oracle_cpu.png` on
/// success.
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
        "e2e_render --vox-gpu-oracle-cpu: loading Oasis VOX fixture from {} \
         via the legacy CPU path (install_vox_sized_to_model) — \
         world size = model's natural 1488×544×1344 voxels. Camera pinned to \
         shared oracle pose pos={:?} look={:?}. Saving to {}.",
        path.display(),
        ORACLE_CAMERA_POS,
        ORACLE_CAMERA_LOOK,
        ORACLE_CPU_PNG,
    );

    let mut app_args = crate::AppArgs::default();
    app_args.grid_preset = crate::GridPreset::Vox { path };
    // vox-gpu-rewrite Stage 2 (2026-05-18): `fixed_world_size` is gone;
    // `setup_test_grid`'s only test-only escape hatch is
    // `vox_gpu_oracle_cpu_phase`, which routes to the legacy
    // `install_vox_sized_to_model` CPU oracle. This is the SOLE remaining
    // call site of the sized-to-model path and exists specifically so the
    // oracle gate can compare CPU vs W5 GPU output.
    app_args.vox_gpu_oracle_cpu_phase = true;
    crate::run_e2e_render_with_args(app_args)
}

// ---------------------------------------------------------------------------
// Phase 2: GPU render — entry point invoked from `bin/e2e_render.rs`
// ---------------------------------------------------------------------------

/// Boot the e2e harness configured for the GPU producer phase. Returns the
/// harness's `AppExit`. Saves `target/e2e-screenshots/oracle_gpu.png` on
/// success.
pub fn run_vox_gpu_oracle_gpu_phase() -> AppExit {
    let path = oasis_vox_fixture_path();
    if !path.exists() {
        eprintln!(
            "e2e_render --vox-gpu-oracle-gpu: FIXTURE MISSING at {} \
             (cwd = {:?}). The fixture is Git LFS-tracked at \
             {OASIS_VOX_FIXTURE_PATH}. Run `git lfs pull`, OR run from the \
             workspace root.",
            path.display(),
            std::env::current_dir().ok()
        );
        return AppExit::error();
    }
    println!(
        "e2e_render --vox-gpu-oracle-gpu: loading Oasis VOX fixture from {} \
         via the W5 GPU producer chain (install_vox_in_fixed_world) — \
         fixed world 4096×512×4096 voxels, GPU construction enabled. Camera \
         pinned to shared oracle pose pos={:?} look={:?}. Saving to {}.",
        path.display(),
        ORACLE_CAMERA_POS,
        ORACLE_CAMERA_LOOK,
        ORACLE_GPU_PNG,
    );

    let mut app_args = crate::AppArgs::default();
    app_args.grid_preset = crate::GridPreset::Vox { path };
    // vox-gpu-rewrite Stage 2 (2026-05-18): the production install path
    // (no oracle-CPU-phase flag) — `install_vox_in_fixed_world` + W5 GPU
    // producer chain. GPU construction default-on; explicit assignment
    // for belt-and-braces.
    app_args.construction_config.gpu_construction_enabled = true;
    app_args.vox_gpu_oracle_gpu_phase = true;
    crate::run_e2e_render_with_args(app_args)
}

// ---------------------------------------------------------------------------
// Phase 3: Compare — the top-level `--vox-gpu-oracle` entry point
// ---------------------------------------------------------------------------

/// Top-level entry point for `--vox-gpu-oracle`. Spawns the CPU oracle phase
/// + the GPU phase as subprocesses, then loads both saved PNGs and runs the
/// per-pixel diff assertion. Returns an exit code (0 = PASS, non-zero =
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

    // Phase 1 — CPU oracle.
    println!(
        "e2e_render --vox-gpu-oracle: spawning CPU oracle phase \
         (subprocess: {} --vox-gpu-oracle-cpu)",
        exe.display()
    );
    let cpu_status = Command::new(&exe)
        .arg("--vox-gpu-oracle-cpu")
        .current_dir(&cwd)
        .status();
    let cpu_ok = match cpu_status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!(
                "e2e_render --vox-gpu-oracle: CPU oracle subprocess failed \
                 to spawn — {e}"
            );
            return 1;
        }
    };
    if !cpu_ok {
        eprintln!(
            "e2e_render --vox-gpu-oracle: CPU oracle subprocess exited \
             non-zero — aborting compare"
        );
        return 1;
    }

    // Phase 2 — GPU.
    println!(
        "e2e_render --vox-gpu-oracle: spawning GPU phase \
         (subprocess: {} --vox-gpu-oracle-gpu)",
        exe.display()
    );
    let gpu_status = Command::new(&exe)
        .arg("--vox-gpu-oracle-gpu")
        .current_dir(&cwd)
        .status();
    let gpu_ok = match gpu_status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!(
                "e2e_render --vox-gpu-oracle: GPU subprocess failed to \
                 spawn — {e}"
            );
            return 1;
        }
    };
    if !gpu_ok {
        eprintln!(
            "e2e_render --vox-gpu-oracle: GPU subprocess exited non-zero — \
             aborting compare"
        );
        return 1;
    }

    // Phase 3 — compare.
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
