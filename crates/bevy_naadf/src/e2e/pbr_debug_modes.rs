//! `--pbr-debug-modes` mode — exercise every PBR rendering debugger view.
//!
//! Boots the same scene + camera pose as `--pbr-visual`, then iterates
//! every non-zero [`crate::debug_view::DebugViewMode`]; for each mode the
//! driver:
//!
//! 1. Sets `DebugViewState.mode = mode`.
//! 2. Waits a handful of frames so the per-frame uniform upload + GPU
//!    dispatch refresh the framebuffer with the new debug output.
//! 3. Captures `Screenshot::primary_window`, saves it as
//!    `target/e2e-screenshots/pbr_debug_mode_NN_<name>.png`.
//! 4. Asserts the framebuffer is NON-DEGENERATE: mean luminance over a
//!    central region exceeds a tiny floor, AND the std-dev exceeds a
//!    tiny floor — i.e. the debug branch produced visible, non-uniform
//!    output. A few modes (e.g. POM self-shadow on the default scene)
//!    can be near-uniform high; the std-dev gate is intentionally loose
//!    (1.0) so legitimate near-uniform outputs still pass while a
//!    fully-broken all-black or all-mid-grey output fails.
//!
//! The gate proves the debugger WORKS for every mode without asserting
//! specific pixel values (over-constrained for visualisations that change
//! with scene content).

use std::path::Path;

use bevy::prelude::*;

use crate::debug_view::DebugViewMode;
use crate::e2e::framebuffer::{Framebuffer, Rect};

/// Warmup frames at the camera pose before the first mode-capture; lets
/// TAA/GI converge enough that the first mode (which starts from
/// production state) has a meaningful baseline.
pub const PBR_DEBUG_MODES_WARMUP_FRAMES: u32 = 100;

/// Frames between setting a new mode and capturing the screenshot. The
/// first-hit pass renders the debug colour into `taa_sample_accum` every
/// frame at weight=1.0, so the very next frame already shows the debug
/// view; we wait a few more frames so the screenshot capture has the
/// settled value (and to absorb the async capture's one-frame lag).
pub const PBR_DEBUG_MODE_SETTLE_FRAMES: u32 = 4;

/// Max extra frames the driver waits for each per-mode async screenshot
/// capture to deliver.
pub const PBR_DEBUG_MODE_DRAIN_FRAMES: u32 = 16;

/// Central rect (in `target/e2e-screenshots/pbr_visual_baseline.png`
/// coordinates — 256×256 e2e window) the per-mode non-degeneracy assertion
/// reads. Sized to overlap the PBR materials in the default test grid.
pub const PBR_DEBUG_ASSERT_RECT: Rect = Rect { x0: 32, y0: 32, x1: 224, y1: 224 };

/// Minimum mean per-channel value (0..=255) inside [`PBR_DEBUG_ASSERT_RECT`]
/// for a passing capture. A fully-black framebuffer (the "debug mode
/// disabled / debug output discarded" failure mode) has mean ≈ 0.
pub const PBR_DEBUG_MEAN_FLOOR: f32 = 1.0;

/// Minimum 16-tap luminance std-dev inside [`PBR_DEBUG_ASSERT_RECT`] for
/// a passing capture. Catches the "debug mode hardcoded to a constant"
/// failure mode. Most debug modes produce per-pixel variation from
/// material/normal-map texture sampling; a few (e.g. AO on a uniform
/// surface) may be near-uniform — 1.0 is loose enough to admit those.
pub const PBR_DEBUG_STDDEV_FLOOR: f32 = 1.0;

/// Per-mode capture state — sub-resource embedded in
/// [`crate::e2e::pbr_visual::PbrVisualState`] so the `e2e_driver` system's
/// `SystemParam` count stays under Bevy 0.19's `IntoSystemSet` arity
/// ceiling. The driver populates `captures` in order; the `Assert` phase
/// walks the vector and validates each.
#[derive(Default)]
pub struct PbrDebugModesState {
    /// Index of the mode currently being tested (`0..NUM_DEBUG_MODES`,
    /// i.e. iterates `DebugViewMode` discriminants 1..=N).
    pub mode_cursor: u32,
    /// Per-mode capture results — `(mode_id, mode_label, Framebuffer)`.
    /// Appended after each successful capture.
    pub captures: Vec<(u32, &'static str, Framebuffer)>,
}

/// Boot the e2e harness with `--pbr-debug-modes` mode active.
pub fn run_pbr_debug_modes() -> AppExit {
    let mut app_args = crate::AppArgs::default();
    app_args.pbr_debug_modes_mode = true;
    println!(
        "e2e_render --pbr-debug-modes: PBR rendering-debugger gate; {} \
         non-zero modes; warmup {} frames + settle {} frames per mode; \
         default test grid side-on metallic-pillar view.",
        DebugViewMode::NUM_DEBUG_MODES,
        PBR_DEBUG_MODES_WARMUP_FRAMES,
        PBR_DEBUG_MODE_SETTLE_FRAMES,
    );
    crate::run_e2e_render_with_args(app_args)
}

/// Camera pose — reuse the `--pbr-visual` side-on metallic-pillar view so
/// every debug mode is exercised against PBR voxels.
pub fn pbr_debug_modes_pose() -> Transform {
    super::gates::e2e_camera_transform()
}

/// `Update` system — pin the camera every frame during the debug-modes
/// gate. Mirrors `pin_pbr_visual_camera`.
pub fn pin_pbr_debug_modes_camera(
    args: Option<Res<crate::AppArgs>>,
    world_data: Option<Res<crate::world::data::WorldData>>,
    mut camera: Single<
        (&mut Transform, &mut crate::camera::position_split::PositionSplit),
        With<Camera3d>,
    >,
) {
    let Some(args) = args else { return };
    if !args.pbr_debug_modes_mode {
        return;
    }
    let Some(world_data) = world_data else { return };
    let size_v = world_data.size_in_chunks
        * (crate::voxel::CELL_DIM as u32 * crate::voxel::CELL_DIM as u32);
    if size_v.x == 0 || size_v.y == 0 || size_v.z == 0 {
        return;
    }
    let pose = pbr_debug_modes_pose();
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = crate::camera::position_split::PositionSplit::from_world(pose.translation);
}

/// Save a per-mode capture PNG. Names sort lexically by mode index so a
/// human can `ls target/e2e-screenshots/pbr_debug_mode_*.png` and step
/// through them.
pub fn save_pbr_debug_mode_png(fb: &Framebuffer, mode_id: u32, label: &str) {
    let safe = label
        .replace(' ', "_")
        .replace('(', "")
        .replace(')', "");
    let filename = format!("pbr_debug_mode_{:02}_{}.png", mode_id, safe);
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(&filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --pbr-debug-modes: mode {mode_id} ({label}) -> {}",
            path.display(),
        ),
        Err(e) => eprintln!(
            "e2e_render --pbr-debug-modes: mode {mode_id} ({label}) save FAILED: {e}",
        ),
    }
}

/// Compute mean per-channel byte value (0..=255) over a rect.
fn region_mean_channel(fb: &Framebuffer, rect: Rect) -> f32 {
    let mut acc = 0.0f64;
    let mut n = 0u64;
    let x0 = rect.x0.min(fb.width().saturating_sub(1));
    let y0 = rect.y0.min(fb.height().saturating_sub(1));
    let x1 = rect.x1.min(fb.width());
    let y1 = rect.y1.min(fb.height());
    for y in y0..y1 {
        for x in x0..x1 {
            let p = fb.pixel(x, y);
            acc += (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { (acc / n as f64) as f32 }
}

/// 16-tap luminance std-dev (Rec.709) over a rect — same shape as
/// `pbr_visual::region_luminance_std_dev_16` but inlined here so the
/// modules stay independent.
fn region_luma_std_16(fb: &Framebuffer, rect: Rect) -> f32 {
    let w = (rect.x1 - rect.x0) as i32;
    let h = (rect.y1 - rect.y0) as i32;
    if w <= 0 || h <= 0 {
        return 0.0;
    }
    let mut samples = [0.0f32; 16];
    for i in 0..16i32 {
        let gx = i % 4;
        let gy = i / 4;
        let sx = rect.x0 + ((gx * w) / 4) as u32;
        let sy = rect.y0 + ((gy * h) / 4) as u32;
        if sx < rect.x1 && sy < rect.y1 {
            let p = fb.pixel(sx, sy);
            samples[i as usize] =
                0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        }
    }
    let mean: f32 = samples.iter().sum::<f32>() / 16.0;
    let var: f32 =
        samples.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / 16.0;
    var.sqrt()
}

/// Per-mode non-degeneracy assertion: mean and std-dev must both exceed
/// the (tiny) thresholds.
pub fn assert_pbr_debug_mode_non_degenerate(
    mode_id: u32,
    label: &str,
    fb: &Framebuffer,
) -> Result<String, String> {
    let mean = region_mean_channel(fb, PBR_DEBUG_ASSERT_RECT);
    let std = region_luma_std_16(fb, PBR_DEBUG_ASSERT_RECT);
    let report = format!(
        "mode {mode_id:>2} ({label}): mean={mean:.2} (floor {PBR_DEBUG_MEAN_FLOOR}), \
         std={std:.2} (floor {PBR_DEBUG_STDDEV_FLOOR})",
    );
    if mean < PBR_DEBUG_MEAN_FLOOR {
        return Err(format!(
            "pbr-debug-modes gate FAIL — {report}. Capture is fully black; \
             the debug branch did not write any visible output."
        ));
    }
    if std < PBR_DEBUG_STDDEV_FLOOR {
        return Err(format!(
            "pbr-debug-modes gate FAIL — {report}. Capture is uniform — \
             the debug branch likely hardcoded a constant.",
        ));
    }
    Ok(report)
}
