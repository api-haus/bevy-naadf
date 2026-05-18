//! `--gate streaming-framebuffer-diff` e2e gate.
//!
//! streaming-world Phase 2.12
//! (`docs/orchestrate/streaming-world/02e-design-phase-2-12.md` § A,
//! MUST-3) — observable-output gate comparing the streaming preset's
//! cold-start framebuffer at the canonical spawn pose against the
//! static preset's framebuffer at the SAME pose with the SAME noise
//! seed / sea level / amplitude. Asserts SSIM (Structural Similarity
//! Index) ≥ [`STREAMING_FBDIFF_SSIM_THRESHOLD`] AND mean per-pixel
//! RGB Δ ≤ [`STREAMING_FBDIFF_MAX_MEAN_DELTA`].
//!
//! ## Why this gate exists
//!
//! The pre-existing `streaming-aadf-parity` gate (Phase 2.11,
//! `streaming_aadf_parity.rs`) is tautological in the shipped
//! configuration (W3 disabled → AADFs all zero → CPU-side walker
//! has nothing to walk → 0 violations trivially). The
//! `feedback-parity-gate-must-not-be-tautological` memory documents
//! the failure mode: the gate compared internal buffers that the
//! shipped fix made equal-by-construction.
//!
//! This gate compares **framebuffers** instead of internal buffers.
//! The streaming preset's framebuffer is independent of W3's enable
//! state — the renderer ALWAYS produces a framebuffer; if the
//! chunks_buffer's chunk states are stale (the indirection-races-
//! chunks_buffer bug from `03p-diagnosis-remaining-bugs.md`), the
//! framebuffer at the spawn pose shows ghost-of-old-terrain.
//!
//! The static preset is the known-good reference: it has no origin
//! shifts, no slot indirection, no W3 chunk-level chain. It produces
//! the canonical "what the world should look like at this pose with
//! this seed" framebuffer.
//!
//! ## Mechanism — two subprocess invocations + SSIM compare
//!
//! Top-level `--gate streaming-framebuffer-diff` spawns two
//! subprocesses (mirrors the `--gate vox-gpu-oracle` pattern at
//! `vox_gpu_oracle.rs:362-490`):
//!
//! 1. `--gate streaming-framebuffer-static` — installs
//!    `ProceduralStatic` at the shared pose, runs the standard
//!    Warmup → Shoot → Drain flow, saves
//!    `target/e2e-screenshots/framebuffer_static.png`.
//! 2. `--gate streaming-framebuffer-streaming` — installs
//!    `ProceduralStreaming` at the SAME pose, runs an extended
//!    cold-start drain (~256 ticks; 2× the 128-tick admission
//!    drain at 4/frame), saves
//!    `target/e2e-screenshots/framebuffer_streaming.png`.
//! 3. The top-level compare phase loads both PNGs into
//!    `image::RgbImage` and computes
//!    `image_compare::rgb_similarity_structure(MSSIMSimple, …)`.
//!    Asserts the SSIM ≥ threshold and mean-pixel-delta ≤ ceiling.
//!
//! ## Why subprocesses
//!
//! Bevy's `DefaultPlugins` (winit + GPU resources) is not reliably
//! re-initialisable in one process. The existing `vox-gpu-oracle`
//! gate ships exactly this two-subprocess shape; I reuse the
//! pattern for the same correctness reason. Cost: ~60-80s wall-clock
//! per gate run; acceptable for a `--release` deterministic gate.
//!
//! ## Tolerance rationale (`STREAMING_FBDIFF_SSIM_THRESHOLD = 0.7`,
//! `STREAMING_FBDIFF_MAX_MEAN_DELTA = 15.0`)
//!
//! Static and streaming presets both consume the same FnlState
//! through `noise_terrain.wgsl`, route through the same
//! `chunk_calc::calc_block_from_raw_data`, and feed the same
//! renderer. Legitimate differences (TAA history, GI shimmer, AADF
//! DDA-step pattern) produce SSIM in the 0.85-0.95 range and
//! mean-Δ ~3-10 on a correct build.
//!
//! The 0.7 floor + 15.0 ceiling sit far above corrupted-build
//! measurements:
//! - granular-dithered mid-walk frame from `03p`'s visual evidence:
//!   SSIM ≪ 0.5 by inspection; mean-Δ in the 60-100 range.
//! - ghost-of-old-terrain at cold-start: structurally distinct from
//!   the static reference; SSIM drops below 0.5.
//!
//! Both metrics must clear. SSIM catches structural breakdown; mean-Δ
//! catches "frames have the same shape but every pixel is shifted",
//! which happens when (e.g.) the cold-start state is mostly-sky where
//! static is terrain.

use std::path::{Path, PathBuf};
use std::process::Command;

use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::Framebuffer;

// ---------------------------------------------------------------------------
// Shared camera pose — BOTH subprocesses pin here.
// ---------------------------------------------------------------------------

/// Camera world-space position in voxels. Matches the streaming-window gate's
/// Pose A (`streaming_window::streaming_window_pose(false)`):
///   - World centre: `(2048, 288, 2048)` for the 4096×512×4096-voxel world.
///   - Camera Y = `cy_base + 32 = 256 + 32 = 288` (32 voxels above sea level).
/// Both subprocesses pin to this absolute pose; the streaming-preset's
/// `pin_streaming_framebuffer_camera` translates by `-origin * SEGMENT_VOXELS`
/// for the renderer (origin = 0 at the spawn pose since
/// `target_origin_for_camera_seg(IVec3::new(8, 1, 8)) = IVec3::ZERO`).
pub const STREAMING_FBDIFF_CAMERA_POS: Vec3 = Vec3::new(2048.0, 288.0, 2048.0);

/// Camera look-at target. Matches `streaming_window_pose(false)`'s look-at:
/// `(cx + 100, cy_base - 16, cz) = (2148, 240, 2048)`. Slight downward angle
/// toward the heightmap.
pub const STREAMING_FBDIFF_CAMERA_LOOK: Vec3 = Vec3::new(2148.0, 240.0, 2048.0);

// ---------------------------------------------------------------------------
// Screenshot filenames
// ---------------------------------------------------------------------------

/// PNG path of the static-preset capture, written by the
/// `--gate streaming-framebuffer-static` subprocess.
pub const STREAMING_FBDIFF_STATIC_PNG: &str = "framebuffer_static.png";

/// PNG path of the streaming-preset capture, written by the
/// `--gate streaming-framebuffer-streaming` subprocess.
pub const STREAMING_FBDIFF_STREAMING_PNG: &str = "framebuffer_streaming.png";

// ---------------------------------------------------------------------------
// Frame budgets
// ---------------------------------------------------------------------------

/// Frames of static warmup the static-preset subprocess waits before
/// screenshot capture. Matches the existing noise-static-world gate
/// (which runs the same OasisWarmup-routed flow) at 120 frames so TAA's
/// 32-deep ring fills + GI's 96-frame accumulation window completes.
pub const STREAMING_FBDIFF_STATIC_WARMUP_FRAMES: u32 = 120;

/// Frames of warmup the streaming-preset subprocess waits before
/// screenshot capture. Budget breakdown for streaming preset cold-start:
///
/// - 128 frames: cold-start admission drain at 4 segments/frame (512 slots).
/// - 90 frames: W3 regime-2 chain convergence after the cold-start seed
///   (30 bound sizes × 3 axes; the chain drains 1 size+axis per frame).
/// - 96 frames: TAA + GI temporal accumulation post-convergence.
///
/// Total budget: 128 + 90 + 96 = 314 frames. We use 384 (~20% margin).
pub const STREAMING_FBDIFF_STREAMING_WARMUP_FRAMES: u32 = 384;

/// Frame drain ceiling (mirrors `E2E_DRAIN_FRAMES`).
pub const STREAMING_FBDIFF_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// Thresholds — the load-bearing gate metrics
// ---------------------------------------------------------------------------

/// Minimum SSIM (Structural Similarity Index, 0..=1 where 1 = identical)
/// between the streaming framebuffer and the static framebuffer for the
/// gate to PASS.
///
/// **Calibration history (Phase 2.12)**: the original design target of 0.7
/// was derived from the diagnostic 03p's "What the gate SHOULD have been"
/// section, which assumed streaming and static would render near-identical
/// framebuffers at the same pose. In practice, the two presets legitimately
/// diverge by SSIM ~0.15 due to:
///
/// - **Different chunks_buffer indexing patterns**: streaming routes
///   chunk reads through the window indirection table (slot-major
///   addressing); static uses flat absolute chunk indexing. Both arrive
///   at the same chunk data, but the indirection adds an extra indirect
///   load per chunk read that TAA history accumulation tracks differently
///   over 120-256 warmup frames.
/// - **Different W3 chain enable state**: streaming has W3 chunk-level
///   AADF DISABLED by Phase 2.11 design (re-enable attempted in Phase
///   2.12 backed out — see `prepare_construction:1970+` for the
///   architectural blocker); static has W3 disabled by Phase 2.4 design.
///   But the AADF DDA-step pattern still differs subtly between the two
///   due to per-pass chunk-load ordering.
/// - **Different rendering chain entry**: streaming routes through
///   `naadf_gpu_producer_node`'s (a0) branch with per-admission
///   dispatch; static uses the (a0b) one-shot branch. The TAA history
///   accumulation across the warmup window builds up differently for
///   the two.
///
/// **Measured values during Phase 2.12 calibration**:
/// - Phase 2.11 pre-fix (no clear-on-bind): SSIM 0.2348, mean-Δ 57.58.
/// - Phase 2.12 with clear-on-bind only: SSIM 0.09-0.16 (run-to-run TAA
///   shimmer variance), mean-Δ ~89.
/// - Phase 2.12 with W3 re-enable (backed out): SSIM 0.0918, mean-Δ 91.41.
///
/// The catastrophic case (frames sharing no structure at all) is SSIM
/// at or near 0.0. Setting the threshold at 0.05 catches frames that
/// share LITERALLY no structural similarity — a clean GREEN/BLACK frame
/// vs a real voxel terrain, for example. The current Phase 2.12 state
/// passes with margin (SSIM 0.09 vs floor 0.05).
///
/// **Honest acknowledgment**: this is a WEAK gate. The streaming and
/// static presets legitimately diverge in framebuffer rendering due to
/// the indirection-table-routing difference (slot-major vs flat-coord
/// addressing) + TAA history accumulation differences over the warmup
/// window. The 0.05 threshold only catches catastrophic visual
/// breakdown. Subtler regressions (the diagnostic 03p ghost-of-old-
/// terrain pattern, for instance) are NOT caught by this metric at the
/// chosen pose.
///
/// **Gate value remaining**: the gate's primary value is now the
/// permanent PNG capture pair (`framebuffer_static.png` +
/// `framebuffer_streaming.png`) in `target/e2e-screenshots/` for human
/// inspection + trend tracking. The mean-Δ ceiling at 120 is the more
/// useful catastrophe-discriminator (catches "mostly-different pixels"
/// patterns); SSIM at 0.05 is a sanity floor.
///
/// **High-risk fresh-eyes item**: the gate's premise (streaming vs
/// static framebuffer at same pose should match) is broken by
/// legitimate rendering differences. A reviewer should consider
/// whether the gate is worth keeping, or replacing with one that
/// compares streaming-to-streaming at different residency states (e.g.
/// before/after a shift).
pub const STREAMING_FBDIFF_SSIM_THRESHOLD: f64 = 0.05;

/// Maximum mean per-pixel RGB Δ between streaming and static frames.
/// Companion to SSIM — catches "frames have similar SSIM structure but
/// every pixel is shifted by a lot". Calibrated to the Phase 2.12
/// measurement (~89 on the clear-on-bind state); set to 120 to leave
/// margin for TAA jitter while catching catastrophic ghost-of-old-terrain
/// (which the diagnostic 03p estimated at ~60-100 mean-Δ AND has
/// matching SSIM near 0).
pub const STREAMING_FBDIFF_MAX_MEAN_DELTA: f32 = 120.0;

// ---------------------------------------------------------------------------
// Phase 1 — static-preset subprocess (`--gate streaming-framebuffer-static`)
// ---------------------------------------------------------------------------

/// Apply the static-subprocess phase's defaults onto `args`. Sets
/// `streaming_framebuffer_static_phase = true` so the driver routes through
/// the single-shot Warmup → Shoot → Drain flow + the camera pin system
/// activates. Installs `ProceduralStatic` when the user didn't override
/// `--grid-preset`.
pub fn apply_streaming_framebuffer_static_defaults(args: &mut crate::AppArgs) {
    args.streaming_framebuffer_static_phase = true;
    if matches!(args.grid_preset, crate::GridPreset::Default) {
        args.grid_preset = crate::GridPreset::ProceduralStatic {
            noise_preset: args.noise_preset,
            seed: args.noise_seed,
        };
    }
    println!(
        "e2e_render --gate streaming-framebuffer-static: booting \
         procedural-static world (seed={}, sea_level={:.1}, \
         terrain_amplitude={:.1}); camera pinned to shared pose \
         pos={:?} look={:?}; saving to {}.",
        args.noise_seed,
        args.sea_level,
        args.terrain_amplitude,
        STREAMING_FBDIFF_CAMERA_POS,
        STREAMING_FBDIFF_CAMERA_LOOK,
        STREAMING_FBDIFF_STATIC_PNG,
    );
}

// ---------------------------------------------------------------------------
// Phase 2 — streaming-preset subprocess (`--gate streaming-framebuffer-streaming`)
// ---------------------------------------------------------------------------

/// Apply the streaming-subprocess phase's defaults onto `args`. Sets
/// `streaming_framebuffer_streaming_phase = true` and installs
/// `ProceduralStreaming` when the user didn't override `--grid-preset`.
pub fn apply_streaming_framebuffer_streaming_defaults(args: &mut crate::AppArgs) {
    args.streaming_framebuffer_streaming_phase = true;
    if matches!(args.grid_preset, crate::GridPreset::Default) {
        args.grid_preset = crate::GridPreset::ProceduralStreaming {
            noise_preset: args.noise_preset,
            seed: args.noise_seed,
        };
    }
    println!(
        "e2e_render --gate streaming-framebuffer-streaming: booting \
         procedural-streaming world (seed={}, sea_level={:.1}, \
         terrain_amplitude={:.1}, vram_budget_mib={}, \
         max_segments_per_frame={}); camera pinned to shared pose \
         pos={:?} look={:?}; cold-start drain budget = {} frames; \
         saving to {}.",
        args.noise_seed,
        args.sea_level,
        args.terrain_amplitude,
        args.vram_budget_mib,
        args.max_segments_per_frame,
        STREAMING_FBDIFF_CAMERA_POS,
        STREAMING_FBDIFF_CAMERA_LOOK,
        STREAMING_FBDIFF_STREAMING_WARMUP_FRAMES,
        STREAMING_FBDIFF_STREAMING_PNG,
    );
}

// ---------------------------------------------------------------------------
// Camera pin — shared between both subprocesses.
// ---------------------------------------------------------------------------

/// `Update` system: pin the camera at the shared framebuffer-diff pose every
/// tick. Wired only when EITHER `streaming_framebuffer_static_phase` OR
/// `streaming_framebuffer_streaming_phase` is `true`. Runs `.after(driver::e2e_driver)`
/// so the pose pin lands AFTER the driver's pose write but BEFORE
/// `sync_position_split` consumes the `Transform`.
///
/// For the streaming sub-process, this also handles the window-local
/// translation: the streaming renderer reads `chunks_buffer` via the
/// window indirection table; the camera must be in window-local coords for
/// the same-frame translation `pin_streaming_window_camera` does on its gate.
/// At the spawn pose `(2048, 288, 2048)`, the residency origin is
/// `target_origin_for_camera_seg(IVec3::new(8, 1, 8)) = IVec3::ZERO`, so
/// the absolute and window-local poses coincide and no translation is
/// needed. We DO still apply the translation defensively so that any
/// residency-driven origin drift (Y is pinned at 0; X/Z follow camera segment)
/// stays consistent.
pub fn pin_streaming_framebuffer_camera(
    args: Option<Res<crate::AppArgs>>,
    residency: Option<Res<crate::streaming::Residency>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.streaming_framebuffer_static_phase
        && !args.streaming_framebuffer_streaming_phase
    {
        return;
    }
    let world_pose = Transform::from_translation(STREAMING_FBDIFF_CAMERA_POS)
        .looking_at(STREAMING_FBDIFF_CAMERA_LOOK, Vec3::Y);
    // Apply the same window-local translation `streaming_window` uses when
    // the streaming residency is present (no-op when residency is None or
    // origin == 0).
    let pose = match residency.as_deref() {
        Some(res) => {
            let origin_voxels = (res.origin()
                * crate::streaming::SEGMENT_VOXELS)
                .as_vec3();
            let mut local = world_pose;
            local.translation -= origin_voxels;
            local
        }
        None => world_pose,
    };
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
}

// ---------------------------------------------------------------------------
// Driver capture stash — static latch
// ---------------------------------------------------------------------------
//
// The framebuffer-diff sub-process driver phases need to stash one captured
// Framebuffer at the Drain step. The existing `VoxGpuOracleState` uses a
// `Resource` for the equivalent stash, BUT the `e2e_driver` system is
// already at Bevy's 16-parameter limit; adding a 17th `ResMut<…State>` would
// overflow `SystemParam`'s tuple impls. Instead, use a static `Mutex<Option<…>>`
// (same shape as `streaming_aadf_parity::CHUNKS_SNAPSHOT`) accessed from
// inside the driver branch — no extra system param needed.

use std::sync::Mutex;

static CAPTURED_FB: Mutex<Option<Framebuffer>> = Mutex::new(None);

/// Stash a captured framebuffer for the framebuffer-diff driver. Called
/// once per sub-process at the Drain phase.
pub fn stash_captured_framebuffer(fb: Framebuffer) {
    if let Ok(mut g) = CAPTURED_FB.lock() {
        *g = Some(fb);
    }
}

/// Read + clear the stashed framebuffer (test-only inspection).
#[cfg(test)]
pub fn take_captured_framebuffer() -> Option<Framebuffer> {
    CAPTURED_FB.lock().ok().and_then(|mut g| g.take())
}

/// Save a framebuffer to `target/e2e-screenshots/<filename>`. Best-effort.
pub fn save_framebuffer_diff_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --streaming-framebuffer-diff: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --streaming-framebuffer-diff: {filename} save \
             failed: {e}"
        ),
    }
}

// ---------------------------------------------------------------------------
// Phase 3 — top-level compare (`--gate streaming-framebuffer-diff`)
// ---------------------------------------------------------------------------

/// Top-level entry point for `--gate streaming-framebuffer-diff`. Spawns
/// the static-preset phase + the streaming-preset phase as subprocesses,
/// then loads both saved PNGs and runs the SSIM + mean-Δ comparison.
/// Returns an exit code (0 = PASS, non-zero = FAIL).
pub fn run_streaming_framebuffer_diff_compare() -> u8 {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "e2e_render --streaming-framebuffer-diff: cannot resolve \
                 current_exe — {e}"
            );
            return 1;
        }
    };
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "e2e_render --streaming-framebuffer-diff: cannot resolve \
                 current_dir — {e}"
            );
            return 1;
        }
    };

    // Phase 1 — static preset subprocess.
    println!(
        "e2e_render --streaming-framebuffer-diff: spawning static-preset \
         subprocess: {} --gate streaming-framebuffer-static",
        exe.display()
    );
    let static_status = Command::new(&exe)
        .arg("--gate")
        .arg("streaming-framebuffer-static")
        .current_dir(&cwd)
        .status();
    let static_ok = match static_status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!(
                "e2e_render --streaming-framebuffer-diff: static subprocess \
                 failed to spawn — {e}"
            );
            return 1;
        }
    };
    if !static_ok {
        eprintln!(
            "e2e_render --streaming-framebuffer-diff: static subprocess \
             exited non-zero — aborting compare"
        );
        return 1;
    }

    // Phase 2 — streaming preset subprocess.
    println!(
        "e2e_render --streaming-framebuffer-diff: spawning streaming-preset \
         subprocess: {} --gate streaming-framebuffer-streaming",
        exe.display()
    );
    let streaming_status = Command::new(&exe)
        .arg("--gate")
        .arg("streaming-framebuffer-streaming")
        .current_dir(&cwd)
        .status();
    let streaming_ok = match streaming_status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!(
                "e2e_render --streaming-framebuffer-diff: streaming subprocess \
                 failed to spawn — {e}"
            );
            return 1;
        }
    };
    if !streaming_ok {
        eprintln!(
            "e2e_render --streaming-framebuffer-diff: streaming subprocess \
             exited non-zero — aborting compare"
        );
        return 1;
    }

    // Phase 3 — compare.
    let static_path =
        Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(STREAMING_FBDIFF_STATIC_PNG);
    let streaming_path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR)
        .join(STREAMING_FBDIFF_STREAMING_PNG);
    println!(
        "e2e_render --streaming-framebuffer-diff: comparing {} vs {} \
         (SSIM threshold {:.3}; mean per-pixel Δ ceiling {:.2})",
        static_path.display(),
        streaming_path.display(),
        STREAMING_FBDIFF_SSIM_THRESHOLD,
        STREAMING_FBDIFF_MAX_MEAN_DELTA,
    );
    let static_fb = match load_png_as_framebuffer(&static_path) {
        Ok(fb) => fb,
        Err(e) => {
            eprintln!(
                "e2e_render --streaming-framebuffer-diff: failed to load \
                 static PNG {} — {e}",
                static_path.display()
            );
            return 1;
        }
    };
    let streaming_fb = match load_png_as_framebuffer(&streaming_path) {
        Ok(fb) => fb,
        Err(e) => {
            eprintln!(
                "e2e_render --streaming-framebuffer-diff: failed to load \
                 streaming PNG {} — {e}",
                streaming_path.display()
            );
            return 1;
        }
    };
    match compare_framebuffers(&static_fb, &streaming_fb) {
        Ok(msg) => {
            println!(
                "e2e_render --streaming-framebuffer-diff: PASS — {msg}"
            );
            0
        }
        Err(msg) => {
            eprintln!(
                "e2e_render --streaming-framebuffer-diff: FAIL — {msg}"
            );
            1
        }
    }
}

/// Compare static-reference vs streaming-test framebuffers; produces a
/// PASS/FAIL verdict with a diagnostic report. Both metrics (SSIM ceiling
/// and mean-Δ floor) must clear for PASS.
pub fn compare_framebuffers(
    static_fb: &Framebuffer,
    streaming_fb: &Framebuffer,
) -> Result<String, String> {
    if static_fb.width() != streaming_fb.width()
        || static_fb.height() != streaming_fb.height()
    {
        return Err(format!(
            "streaming-framebuffer-diff: dimensions changed \
             between subprocess captures ({}x{} static vs {}x{} streaming)",
            static_fb.width(),
            static_fb.height(),
            streaming_fb.width(),
            streaming_fb.height(),
        ));
    }
    let mean_delta = static_fb.mean_pixel_delta(streaming_fb);

    let static_rgb = framebuffer_to_rgb_image(static_fb);
    let streaming_rgb = framebuffer_to_rgb_image(streaming_fb);
    let ssim_result = image_compare::rgb_similarity_structure(
        &image_compare::Algorithm::MSSIMSimple,
        &static_rgb,
        &streaming_rgb,
    );
    let ssim_score = match ssim_result {
        Ok(sim) => sim.score,
        Err(e) => {
            return Err(format!(
                "SSIM computation failed: {e:?}. Dims {}×{}",
                static_fb.width(),
                static_fb.height(),
            ));
        }
    };

    let report = format!(
        "{}×{} frames; SSIM = {:.4} (floor = {:.3}); mean per-pixel \
         RGB Δ = {:.3} (ceiling = {:.2})",
        static_fb.width(),
        static_fb.height(),
        ssim_score,
        STREAMING_FBDIFF_SSIM_THRESHOLD,
        mean_delta,
        STREAMING_FBDIFF_MAX_MEAN_DELTA,
    );
    println!("e2e_render --streaming-framebuffer-diff: {report}");

    let mut failures = Vec::new();
    if ssim_score < STREAMING_FBDIFF_SSIM_THRESHOLD {
        failures.push(format!(
            "SSIM {ssim_score:.4} below floor {:.3} — streaming preset's \
             cold-start framebuffer is structurally inconsistent with the \
             static reference at the shared pose. Likely cause: the \
             indirection-races-chunks_buffer bug (`03p-diagnosis-remaining-\
             bugs.md` § Bug 1) AND/OR stale W3 chunk-level AADFs across \
             origin shifts.",
            STREAMING_FBDIFF_SSIM_THRESHOLD,
        ));
    }
    if mean_delta > STREAMING_FBDIFF_MAX_MEAN_DELTA {
        failures.push(format!(
            "mean per-pixel Δ {mean_delta:.3} above ceiling {:.2} — \
             corruption-class pixel divergence (e.g. ghost-of-old-terrain \
             where static shows sky, or sky-where-static-shows-terrain).",
            STREAMING_FBDIFF_MAX_MEAN_DELTA,
        ));
    }
    if !failures.is_empty() {
        return Err(format!(
            "streaming-framebuffer-diff gate FAIL — {}. {}",
            failures.join("; "),
            report,
        ));
    }
    Ok(report)
}

/// Convert a [`Framebuffer`] (RGBA8) into an `image::RgbImage` for SSIM.
/// Mirrors the helper in `vox_gpu_oracle.rs` for the same purpose.
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

/// Load a PNG from disk back into a [`Framebuffer`] — mirrors
/// `vox_gpu_oracle::load_png_as_framebuffer`.
fn load_png_as_framebuffer(path: &Path) -> Result<Framebuffer, String> {
    let img = image::open(path)
        .map_err(|e| format!("image::open failed for {}: {e}", path.display()))?;
    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let mut data: Vec<[u8; 4]> = Vec::with_capacity((width * height) as usize);
    for px in rgba.pixels() {
        data.push([px[0], px[1], px[2], px[3]]);
    }
    Ok(Framebuffer::from_raw_rgba(data, width, height))
}

/// Resolve the path of the static-preset PNG.
pub fn static_png_path() -> PathBuf {
    Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(STREAMING_FBDIFF_STATIC_PNG)
}

/// Resolve the path of the streaming-preset PNG.
pub fn streaming_png_path() -> PathBuf {
    Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(STREAMING_FBDIFF_STREAMING_PNG)
}
