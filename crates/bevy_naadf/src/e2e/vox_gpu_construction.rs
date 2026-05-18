//! `--vox-gpu-construction` mode — PRODUCTION-PATH gate for the
//! vox-gpu-rewrite W5 GPU producer chain
//! (`docs/orchestrate/vox-gpu-rewrite/`).
//!
//! ## Goal
//!
//! End-to-end gate that exercises **the same code path** as the production
//! binary `cargo run --release --bin bevy-naadf -- --vox <path>` and asserts
//! that the world is actually populated — not just that the framebuffer is
//! non-pure-black.
//!
//! ### Why the W5.3 "sky-band luminance > 40" floor was rejected
//!
//! The original W5.5 gate passed when the W5 GPU producer chain dispatched
//! WITHOUT writing meaningful geometry — sky luminance 146.2 cleared the 40
//! floor even though the chunks buffer was populated with state pointers
//! that indexed into UNWRITTEN regions of an undersized blocks/voxels
//! buffer (see `docs/orchestrate/vox-gpu-rewrite/05-diagnostic.md`). The
//! gate gave a green signal for an empty scene; the user's live visual
//! check caught the regression that the gate missed.
//!
//! ## Mechanism — two-frame camera-sweep Δ
//!
//! Mirrors `--oasis-edit-visual`'s Δ-based assertion shape, but with a
//! **camera-translation Δ** instead of a brush-edit Δ. The W5 install
//! path leaves `WorldData.chunks_cpu / blocks_cpu / voxels_cpu = Vec::new()`
//! by design (the GPU producer is the source of truth); the CPU
//! `sphere_brush` writes through `chunks_cpu[ci]` indexing and silently
//! no-ops on the empty mirror, so a brush-edit Δ would always be zero. A
//! camera-sweep Δ achieves the same regression signal — moving the camera
//! through a populated world sweeps geometry through the framebuffer
//! (large Δ); moving the camera through an empty world shows sky on both
//! frames (Δ near zero).
//!
//! 1. Load `OASIS_VOX_FIXTURE_PATH` through the production
//!    `install_vox_in_fixed_world` W5 GPU producer chain (the *same* code
//!    path the binary runs when given `--vox <path>`).
//! 2. Pin camera A at the scaled C# spawn pose (`(2000, 800, 160)`
//!    voxels in the 4096×512×4096 world — Y=800 above the world's 512-
//!    voxel ceiling) tilted DOWN toward Oasis architecture (look-at
//!    `(2000, 200, 1160)`). This matches the production binary's
//!    `setup_camera::from_world_voxels` pose (`camera/mod.rs:54-64`).
//! 3. Warmup → capture frame A.
//! 4. `OasisApplyEdit` phase: instead of a brush, *promote the camera*
//!    via the `oasis.edit_applied` flag — the pin function reads the flag
//!    and switches to camera B at `(2800, 800, 160)` (800 voxels lateral
//!    sweep in +X) with matched downward tilt.
//! 5. Wait ~5 s for TAA + GI convergence at the new pose → capture frame B.
//! 6. Assert per-pixel mean RGB Δ over a central rect exceeds
//!    [`VOX_GPU_CONSTRUCTION_DIFF_FLOOR`] (catches empty-scene regression
//!    class) AND assert frame A's count of pixels with luminance below
//!    [`VOX_GPU_CONSTRUCTION_NEAR_BLACK_THRESHOLD`] stays under
//!    [`VOX_GPU_CONSTRUCTION_NEAR_BLACK_FRACTION_CEILING`] of the frame
//!    (catches inversion-class regression where scattered mixed blocks
//!    fail the CAS hash insert and render as voxel pointer `2` → empty
//!    voids exposing whatever lies past).
//!
//! ## Why this catches both regression classes
//!
//! In a CORRECTLY-rendered populated world, camera A sees Oasis
//! architecture top-down (lots of mid-luminance stone surfaces, very few
//! near-black "void" pixels) and camera B sees a DIFFERENT view of
//! architecture from 800 voxels +X — geometry sweeps laterally through
//! the frustum, producing a substantial per-pixel Δ.
//!
//! - **Empty-scene regression** (W5.3 pre-Stage-1; buffer underallocation):
//!   both camera A and camera B render the atmosphere-tinted sky
//!   (luminance ~146, near-constant); the per-pixel Δ collapses to
//!   near-zero (TAA noise floor only) — the Δ-vs-floor check fails.
//! - **Inversion-class regression** (W5.3-fix Stage 1; placeholder
//!   `hash_map` / `hash_coefficients` leaking through on the W5 install
//!   path): camera A sees Oasis architecture but with scattered "hole"
//!   pixels through what should be solid walls (the renderer descends
//!   into sentinel-2 blocks → reads zero voxels → renders as voids
//!   exposing whatever lies past). The Δ assertion still passes (some
//!   geometry IS visible), but the near-black-pixel count spikes —
//!   the near-black-count check fails.

use std::path::PathBuf;

use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::{Framebuffer, Rect};
use crate::e2e::oasis_edit_visual::{oasis_vox_fixture_path, OASIS_VOX_FIXTURE_PATH};

// ---------------------------------------------------------------------------
// Camera poses — C#-faithful literal voxel coordinates
// ---------------------------------------------------------------------------

/// Top-down birdseye camera A pose, mirroring
/// [`crate::e2e::oasis_edit_visual::birdseye_pose`] for the fixed
/// 4096×512×4096-voxel world. Computed values:
///   cx = 2048, cz = 2048, mid_y = 256, cam_y = 512 + 250 = 762.
///
/// vox-gpu-rewrite W5.3-fix Stage 3 (top-down gate move, 2026-05-18) —
/// moved from the prior C#-faithful inside-world spawn `(500, 200, 40)`
/// to the same top-down birdseye pose `--oasis-edit-visual` uses. From
/// this vantage holes in Oasis roofs are unmistakable AND there is no
/// legitimate dark interior to dilute the near-black metric (the camera
/// looks DOWN at the world ceiling Y=256). The metric (`lum<10`, `<1%`
/// floor) is unchanged from Stage 1.5; only the vantage is corrected
/// per the user's directive. See
/// `docs/orchestrate/vox-gpu-rewrite/08-diagnostic-inversion-round-3.md`
/// for round-3 findings and the subsequent dispatch's brief.
pub const VOX_GPU_CONSTRUCTION_CAMERA_POS_A: Vec3 = Vec3::new(2048.0, 762.0, 2048.0);

/// Camera A look-at target: world centre at mid-height. Combined with
/// `Vec3::X` up (in [`pin_vox_gpu_construction_camera`]) this produces
/// the same top-down framing as
/// [`crate::e2e::oasis_edit_visual::birdseye_pose`].
pub const VOX_GPU_CONSTRUCTION_CAMERA_LOOK_A: Vec3 = Vec3::new(2048.0, 256.0, 2048.0);

/// Camera B — same top-down birdseye, laterally translated +X by 256
/// voxels (one segment-width) so the sweep Δ assertion catches
/// architecture sweeping through the frustum from a parallel pose. Both
/// cameras share the Y=762 altitude and `Vec3::X` up reference; the
/// look-at follows the camera laterally so the framing stays top-down.
pub const VOX_GPU_CONSTRUCTION_CAMERA_POS_B: Vec3 = Vec3::new(2304.0, 762.0, 2048.0);

/// Camera B look-at target: matched lateral offset, same downward gaze.
pub const VOX_GPU_CONSTRUCTION_CAMERA_LOOK_B: Vec3 = Vec3::new(2304.0, 256.0, 2048.0);

// ---------------------------------------------------------------------------
// Diff threshold + bounding box fractions
// ---------------------------------------------------------------------------

/// Central rect fractions for the per-pixel Δ assertion (same shape as
/// `--oasis-edit-visual`'s 30 % × 30 % rect). The brush at `(500, 200,
/// 100)` projects to the central region of the framebuffer for the camera
/// pose at `(500, 200, 40)` looking `+Z`.
pub const VOX_GPU_CONSTRUCTION_DIFF_RECT_FRACS: (f32, f32, f32, f32) =
    (0.35, 0.35, 0.65, 0.65);

/// Minimum mean per-pixel RGB Δ over the central rect for the gate to
/// PASS. Generous floor matched to `--oasis-edit-visual`'s 8.0; a real
/// brush stroke into populated geometry produces a Δ of 30+ (sphere
/// replaces sky-band ~150 with emissive ~250 over ~10-20 % of the rect).
/// A regression that leaves the world empty produces Δ near zero (no
/// geometry → brush adds voxels into an empty world but the renderer's
/// chunks pointers are still bogus → framebuffer unchanged).
pub const VOX_GPU_CONSTRUCTION_DIFF_FLOOR: f32 = 8.0;

/// Near-black luminance threshold for the inversion-class regression
/// assertion (`docs/orchestrate/vox-gpu-rewrite/06-diagnostic-inversion.md`).
///
/// Pixels with Rec.709 luminance **strictly below** this value are counted
/// as "hole pixels" — the visual signature of the inversion bug, where
/// scattered mixed blocks fail the CAS hash insert and render as voxel
/// pointer `2` → reads from the seed region of `voxels[]` (all zero) →
/// renders as empty voids exposing whatever is past the missing block.
///
/// Tuned by observation: the post-Stage-1.5 (correct) frame at camera A
/// (Y=800 above world, top-down) measures ZERO near-black pixels at the
/// `lum < 10` threshold — even shadowed wall surfaces and dark crenellation
/// underside luminate ABOVE 10 because they carry material-colour tint
/// (typically luminance 30+). The pre-Stage-1.5 (inverted) frame at the
/// scaled production camera pose had a visible "hole pixel" population
/// (the diagnostic's screenshot at `image-cache/...1.png` shows scattered
/// near-black holes through what should be solid stone walls). Anything
/// with `lum < 10` on the corrected camera-A frame is consistent with a
/// true "hole pixel" (the renderer descending into a sentinel-2 block
/// and reading zero voxels).
pub const VOX_GPU_CONSTRUCTION_NEAR_BLACK_THRESHOLD: f32 = 10.0;

/// Maximum allowed fraction of near-black pixels in the camera-A frame
/// for the gate to PASS, expressed as a fraction of the frame's pixel
/// count. The post-Stage-1.5 frame measures ZERO near-black pixels at
/// the (2000, 800, 160) production-faithful camera A pose; the pre-fix
/// frame at the same pose would have produced a substantial near-black
/// count (~3-15% based on the diagnostic's screenshot observation of
/// scattered hole pixels). Set conservatively at 1% (655 pixels on a
/// 256×256 frame) — well above the observed post-fix count of 0,
/// allowing for occasional TAA-noise-floor undershoot on a few isolated
/// pixels, while still tripping firmly on any meaningful re-emergence
/// of the inversion symptom.
pub const VOX_GPU_CONSTRUCTION_NEAR_BLACK_FRACTION_CEILING: f32 = 0.01;

// ---------------------------------------------------------------------------
// Entry point — invoked from `bin/e2e_render.rs`
// ---------------------------------------------------------------------------

/// Boot the e2e harness with the production W5 GPU producer path enabled
/// AND the `--oasis-edit-visual`-shape brush-edit driver flow.
///
/// Returns the harness's `AppExit`. The driver routes through the
/// `OasisWarmup → ... → OasisAssert` phases (selected when EITHER
/// `oasis_edit_visual_mode` OR `vox_gpu_construction_mode` is `true`); the
/// camera is overridden by [`pin_vox_gpu_construction_camera`] (which
/// runs `.after(pin_oasis_camera)` so it takes precedence over the
/// birdseye); the brush is overridden by `apply_erase_brush`'s mode-aware
/// branch to spawn at [`VOX_GPU_CONSTRUCTION_BRUSH_POS`].
pub fn run_vox_gpu_construction() -> AppExit {
    let path = oasis_vox_fixture_path();
    if !path.exists() {
        eprintln!(
            "e2e_render --vox-gpu-construction: FIXTURE MISSING at {} \
             (cwd = {:?}). The fixture is Git LFS-tracked at \
             {OASIS_VOX_FIXTURE_PATH}. Run `git lfs pull` to fetch the \
             binary content, OR run the binary from the workspace root.",
            path.display(),
            std::env::current_dir().ok()
        );
        return AppExit::error();
    }
    println!(
        "e2e_render --vox-gpu-construction: loading Oasis VOX fixture from \
         {} ({} bytes) into the W5 GPU producer chain (production-path \
         camera-sweep gate; camera A at {:?} look {:?} → camera B at {:?} \
         look {:?}; expecting per-pixel RGB Δ ≥ {:.2} over central rect AND \
         frame-A near-black (lum<{:.1}) count ≤ {:.1}% of frame pixels)",
        path.display(),
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        VOX_GPU_CONSTRUCTION_CAMERA_POS_A,
        VOX_GPU_CONSTRUCTION_CAMERA_LOOK_A,
        VOX_GPU_CONSTRUCTION_CAMERA_POS_B,
        VOX_GPU_CONSTRUCTION_CAMERA_LOOK_B,
        VOX_GPU_CONSTRUCTION_DIFF_FLOOR,
        VOX_GPU_CONSTRUCTION_NEAR_BLACK_THRESHOLD,
        100.0 * VOX_GPU_CONSTRUCTION_NEAR_BLACK_FRACTION_CEILING,
    );

    let mut app_args = crate::AppArgs::default();
    // Production W5 path: fixed-size world + GPU construction default-on
    // (= the `bevy-naadf::main` shape, per `lib.rs:393` + `:143`).
    app_args.grid_preset = crate::GridPreset::Vox {
        path: PathBuf::from(&app_path_for_args(&path)),
        tiles: 1,
    };
    app_args.fixed_world_size = true;
    app_args.construction_config.gpu_construction_enabled = true;
    // Route through the Oasis brush-edit driver flow. The driver's
    // `OasisWarmup` fast-path triggers when EITHER `oasis_edit_visual_mode`
    // OR `vox_gpu_construction_mode` is set; the brush + assertion mechanics
    // are identical, the camera + brush position are mode-specific.
    // NOTE: we deliberately do NOT also set `oasis_edit_visual_mode = true`
    // — `pin_oasis_camera` would write a birdseye pose every tick that
    // `pin_vox_gpu_construction_camera` would then override; cleaner to
    // skip the birdseye write entirely.
    app_args.vox_gpu_construction_mode = true;

    crate::run_e2e_render_with_args(app_args)
}

/// Re-export the resolved path for the `AppArgs::grid_preset`. Mirrors the
/// shape of `oasis_edit_visual.rs::run_oasis_edit_visual`.
fn app_path_for_args(p: &std::path::Path) -> PathBuf {
    p.to_path_buf()
}

// ---------------------------------------------------------------------------
// Camera pin — overrides `pin_oasis_camera`'s birdseye
// ---------------------------------------------------------------------------

/// `Update` system: pin the camera at one of the two C#-faithful poses
/// (A pre-promotion, B post-promotion). The "promotion" is the
/// `OasisEditVisualState.edit_applied` flag, which the driver flips on
/// `OasisApplyEdit` — this gate hijacks that flag as the "promote to
/// camera B" trigger (instead of "apply brush"); the `OasisApplyEdit`
/// branch in `driver.rs` is mode-aware and skips the brush call entirely
/// for vox-gpu-construction mode.
///
/// Wired only when `AppArgs.vox_gpu_construction_mode == true`; runs
/// `.after(pin_oasis_camera)` so it overrides the birdseye pose the
/// Oasis pin would write (the Oasis driver fast-path doubles as our
/// fast-path; we need the brush-edit phases but NOT the birdseye camera).
pub fn pin_vox_gpu_construction_camera(
    args: Option<Res<crate::AppArgs>>,
    oasis: Option<Res<crate::e2e::oasis_edit_visual::OasisEditVisualState>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.vox_gpu_construction_mode {
        return;
    }
    let promoted = oasis.as_deref().is_some_and(|o| o.edit_applied);
    let (pos, look_at) = if promoted {
        (
            VOX_GPU_CONSTRUCTION_CAMERA_POS_B,
            VOX_GPU_CONSTRUCTION_CAMERA_LOOK_B,
        )
    } else {
        (
            VOX_GPU_CONSTRUCTION_CAMERA_POS_A,
            VOX_GPU_CONSTRUCTION_CAMERA_LOOK_A,
        )
    };
    // Top-down birdseye: look DOWN at the target with `Vec3::X` as the up
    // reference vector — same convention as `oasis_edit_visual::birdseye_pose`
    // so the resulting camera Y-axis aligns toward `+Z` (the framebuffer's
    // up direction).
    let pose = Transform::from_translation(pos).looking_at(look_at, Vec3::X);
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
    let _ = promoted; // referenced by camera pos choice above
}

// ---------------------------------------------------------------------------
// Camera-promotion stub — replaces the brush call in OasisApplyEdit
// ---------------------------------------------------------------------------

/// Driver-stub for the `OasisApplyEdit` phase in vox-gpu-construction mode.
/// Does NOT touch `WorldData` (the W5 install path leaves the CPU mirror
/// empty; `sphere_brush` would silently no-op on the empty mirror). The
/// load-bearing side effect is `oasis.edit_applied = true` — which the
/// driver sets after returning from this function — which
/// `pin_vox_gpu_construction_camera` reads to promote the camera from
/// pose A to pose B.
pub fn promote_camera_to_pose_b() {
    println!(
        "e2e_render --vox-gpu-construction: promoting camera A→B \
         (pose A {:?} → pose B {:?}) — no brush; W5 install path's empty \
         CPU mirror would silently no-op a sphere_brush call",
        VOX_GPU_CONSTRUCTION_CAMERA_POS_A, VOX_GPU_CONSTRUCTION_CAMERA_POS_B,
    );
}

// ---------------------------------------------------------------------------
// Assertion — per-pixel mean RGB Δ over the central rect
// ---------------------------------------------------------------------------

/// Compute the central-rect mean per-pixel RGB Δ between `before` and
/// `after`; assert it exceeds [`VOX_GPU_CONSTRUCTION_DIFF_FLOOR`].
///
/// Returns `Ok(report)` on PASS; `Err(report)` on FAIL.
pub fn assert_vox_gpu_construction_landed(
    before: &Framebuffer,
    after: &Framebuffer,
) -> Result<String, String> {
    if before.width() != after.width() || before.height() != after.height() {
        return Err(format!(
            "frame A {}×{} vs frame B {}×{} — dimensions changed mid-run",
            before.width(),
            before.height(),
            after.width(),
            after.height()
        ));
    }

    let (fx0, fy0, fx1, fy1) = VOX_GPU_CONSTRUCTION_DIFF_RECT_FRACS;
    let rect = Rect::from_fractional(after, fx0, fy0, fx1, fy1);

    let mean_before = before.region_mean(rect);
    let mean_after = after.region_mean(rect);
    let lum_before = Framebuffer::luminance(mean_before);
    let lum_after = Framebuffer::luminance(mean_after);

    let rect_delta = region_mean_pixel_delta(before, after, rect);
    let full_delta = before.mean_pixel_delta(after);

    // vox-gpu-rewrite W5.3-fix Stage 1.5 — near-black-pixel assertion on
    // the camera-A frame (production-faithful spawn pose). Counts pixels
    // with luminance strictly below VOX_GPU_CONSTRUCTION_NEAR_BLACK_THRESHOLD
    // and asserts that count stays below
    // VOX_GPU_CONSTRUCTION_NEAR_BLACK_FRACTION_CEILING * frame_pixels.
    // See docstrings on those constants + `06-diagnostic-inversion.md`.
    let frame_pixels = (before.width() as usize) * (before.height() as usize);
    let near_black_ceiling =
        ((frame_pixels as f32) * VOX_GPU_CONSTRUCTION_NEAR_BLACK_FRACTION_CEILING) as usize;
    let near_black_count = before
        .count_pixels_with_luminance_below(None, VOX_GPU_CONSTRUCTION_NEAR_BLACK_THRESHOLD);

    let report = format!(
        "rect=({},{},{},{}) frac=({:.2},{:.2},{:.2},{:.2}); \
         rect mean rgba: before={:?}, after={:?}; \
         rect luminance: before={:.1}, after={:.1}, Δ={:.1}; \
         rect mean per-pixel RGB Δ={:.2} (floor={:.2}); \
         full-frame mean per-pixel RGB Δ={:.2}; \
         frame-A near-black (lum<{:.1}) count={} of {} pixels \
         ({:.2}% of frame; ceiling={} pixels = {:.1}% of frame)",
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
        VOX_GPU_CONSTRUCTION_DIFF_FLOOR,
        full_delta,
        VOX_GPU_CONSTRUCTION_NEAR_BLACK_THRESHOLD,
        near_black_count,
        frame_pixels,
        100.0 * (near_black_count as f32) / (frame_pixels.max(1) as f32),
        near_black_ceiling,
        100.0 * VOX_GPU_CONSTRUCTION_NEAR_BLACK_FRACTION_CEILING,
    );
    println!("e2e_render --vox-gpu-construction: {report}");

    if rect_delta < VOX_GPU_CONSTRUCTION_DIFF_FLOOR {
        return Err(format!(
            "vox-gpu-construction gate FAIL — rect mean per-pixel RGB Δ \
             {rect_delta:.2} is below the floor {:.2}. The camera-A→B \
             translation (pose A {:?} → pose B {:?}) did NOT produce a \
             measurable per-pixel framebuffer change. \
             {report}. \
             This is the W5.3 empty-scene regression class: the W5 GPU \
             producer chain dispatched but the chunks buffer points at \
             unwritten blocks/voxels regions (likely buffer underallocation \
             — see `docs/orchestrate/vox-gpu-rewrite/05-diagnostic.md`), so \
             the renderer reads zero bytes for every chunk and treats the \
             world as empty. Both camera poses render the atmosphere-tinted \
             sky (~146 luminance), so the per-pixel Δ collapses. Inspect \
             target/e2e-screenshots/vox_gpu_construction_before.png + \
             vox_gpu_construction_after.png.",
            VOX_GPU_CONSTRUCTION_DIFF_FLOOR,
            VOX_GPU_CONSTRUCTION_CAMERA_POS_A,
            VOX_GPU_CONSTRUCTION_CAMERA_POS_B,
        ));
    }

    if near_black_count > near_black_ceiling {
        return Err(format!(
            "vox-gpu-construction gate FAIL — frame A has {near_black_count} pixels \
             with luminance < {:.1} ({:.2}% of frame), exceeding the ceiling of \
             {near_black_ceiling} pixels ({:.1}% of frame). \
             {report}. \
             This is the W5.3-fix Stage 1 inversion regression class \
             (`docs/orchestrate/vox-gpu-rewrite/06-diagnostic-inversion.md`): \
             scattered mixed blocks failed the CAS hash insert (placeholder \
             `hash_map` + `hash_coefficients` buffers leak through when the \
             pre-allocation gate doesn't fire on the W5 install path) and \
             render as voxel pointer `2` → reads from the zero seed region of \
             `voxels[]` → renders as empty voids exposing whatever lies past \
             the missing block. Inspect \
             target/e2e-screenshots/vox_gpu_construction_before.png — \
             scattered dark holes through what should be solid stone walls.",
            VOX_GPU_CONSTRUCTION_NEAR_BLACK_THRESHOLD,
            100.0 * (near_black_count as f32) / (frame_pixels.max(1) as f32),
            100.0 * VOX_GPU_CONSTRUCTION_NEAR_BLACK_FRACTION_CEILING,
        ));
    }

    Ok(format!("vox-gpu-construction gate PASS — {report}"))
}

/// Per-rect mean per-pixel RGB delta (channels averaged 0..3). Same shape
/// as `oasis_edit_visual::region_mean_pixel_delta`.
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

// ---------------------------------------------------------------------------
// Screenshot saves (best-effort)
// ---------------------------------------------------------------------------

/// PNG saved for the pre-edit capture.
pub const VOX_GPU_CONSTRUCTION_BEFORE_PNG: &str = "vox_gpu_construction_before.png";
/// PNG saved for the post-edit capture.
pub const VOX_GPU_CONSTRUCTION_AFTER_PNG: &str = "vox_gpu_construction_after.png";

/// Save a framebuffer to `target/e2e-screenshots/<filename>`. Best-effort.
pub fn save_vox_gpu_construction_screenshot(fb: &Framebuffer, filename: &str) {
    let path = std::path::Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --vox-gpu-construction: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --vox-gpu-construction: {filename} save failed: {e}"
        ),
    }
}
