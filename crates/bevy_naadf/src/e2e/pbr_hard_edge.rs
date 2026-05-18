//! `--pbr-hard-edge` mode — splotch-artifact regression gate.
//!
//! Per `docs/orchestrate/pbr-raymarching/05-diagnostic.md` § "LIGHT
//! INTEGRATION splotch diagnose+fix (post-`46e50cd`)": capture a single
//! frame of the default test grid from the same metallic-pillar side-on pose
//! as `--pbr-visual`, save the screenshot, then walk a known cobblestone-
//! interior rect counting **hard 1-pixel luminance jumps**.
//!
//! A "hard 1-pixel jump" pixel satisfies BOTH:
//!   1. `|L(p+1) - L(p)| > T_HARD` — the immediate step is large
//!      (`T_HARD = 0.30 * 255` units of Rec.709 luminance, i.e. ≥30% HSV
//!      Value drop in one pixel).
//!   2. `|L(p+2) - L(p+1)| < T_SMOOTH` — the second step is small
//!      (`T_SMOOTH = 0.10 * 255`), so the discontinuity is a SINGLE-PIXEL
//!      jump rather than the start of a smooth gradient.
//!
//! This matches the user's specification (image-cache image #11):
//!
//! > compared to natural self-shadowed dip — it goes through at least a few
//! > pixels SMOOTHLY producing a gradient — which the test should IGNORE.
//! > only HARSH 1-1 direct PIXEL JUMPS
//!
//! Natural self-shadowed dips ramp smoothly across 3-4 pixels → the second
//! diff is comparable to the first → they DO NOT match the predicate.
//! Splotch boundaries (Image #13: cobblestone with olive/green discoloured
//! patches with sharp 1-pixel edges) have a single huge first step then near-
//! zero second step → they match.
//!
//! Both horizontal (x+1, x+2) and vertical (y+1, y+2) directions are checked
//! independently. The gate fails when the count of hard-jump pixels in the
//! analysis rect exceeds [`PBR_HARD_EDGE_MAX_HARD_JUMPS`].
//!
//! `imageproc::edges::canny` is also run on the same rect as a corroborating
//! diagnostic (logged but not asserted) — canny's edge density correlates
//! with splotch presence and provides a sanity cross-check on the primary
//! hard-jump count.

use std::path::Path;

use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::{Framebuffer, Rect};
use crate::voxel::CELL_DIM;
use crate::world::data::WorldData;

// Note: `PbrHardEdgeState` is NOT a Bevy `Resource` — it is embedded inside
// `super::pbr_visual::PbrVisualState` (which IS a `Resource`) to keep the
// `e2e_driver` system's `SystemParam` count under Bevy 0.19's tuple-arity
// ceiling. The driver accesses `pbr_visual.hard_edge` directly.

// ---------------------------------------------------------------------------
// Screenshot filename
// ---------------------------------------------------------------------------

/// PNG written by the gate on success — overwritten every run.
pub const PBR_HARD_EDGE_PNG: &str = "pbr_hard_edge_baseline.png";

// ---------------------------------------------------------------------------
// Frame budget — reuses the same convergence schedule as `--pbr-visual` so
// the TAA / GI buckets are settled at capture time.
// ---------------------------------------------------------------------------

pub const PBR_HARD_EDGE_WARMUP_FRAMES: u32 = 150;
pub const PBR_HARD_EDGE_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// Analysis rect + thresholds
// ---------------------------------------------------------------------------

/// 50×20 px rect on the cobblestone ground (the `stone_wall_04` material
/// region in the default test grid) — pinned by post-hoc inspection of the
/// baseline screenshot at `target/e2e-screenshots/pbr_hard_edge_baseline
/// .png` to land WHOLLY INSIDE a single voxel face's ground area, away
/// from voxel boundaries (which legitimately produce sharp luminance
/// jumps) and away from object silhouettes (the pillar / wall / emissive
/// blocks).
///
/// Empirical baseline at this rect on a clean post-warmup capture:
/// mean luminance ~170, std-dev ~9, max horizontal pixel-diff ~29, max
/// vertical pixel-diff ~24 — i.e. natural cobblestone shows smooth
/// variation across the analysis rect. The splotch artifact (user
/// images #13, #17) produces 50-60% HSV Value drops in one pixel, which
/// at luminance scale 255 corresponds to 127-152 unit jumps — five or
/// more such jumps in the small 50×20 rect would be the splotch
/// signature.
pub const PBR_HARD_EDGE_RECT: Rect = Rect { x0: 110, y0: 230, x1: 160, y1: 250 };

/// First-difference threshold for a "hard" 1-pixel jump (Rec.709 luminance,
/// 0..=255 scale). User-supplied measurement:
///
/// > the dip in brightness over 2 adjacent pixels is 50-60% (DARKENED) vs
/// > 90-99% (ADJACENT BRIGHT PIXEL (HSV Value))
///
/// 50% of 255 ≈ 128 luminance units. We use a tighter floor of 30%
/// (≈76 units) so the test fires on the smaller end of the user-observed
/// dip — splotches with `50-60%` dips comfortably exceed it.
pub const PBR_HARD_EDGE_T_HARD: f32 = 0.30 * 255.0;

/// Second-difference ceiling. A "hard" jump's NEXT step must be small —
/// otherwise the jump is the start of a gradient (a natural self-shadowed
/// dip, per user spec). 10% of 255 ≈ 25 units. Natural cobblestone surface
/// shading produces ~5-15 unit second-diffs; splotch boundaries produce
/// ~0-5 unit second-diffs (the splotch interior is roughly uniform).
pub const PBR_HARD_EDGE_T_SMOOTH: f32 = 0.10 * 255.0;

/// Maximum number of hard-jump pixels permitted in the analysis rect.
///
/// Empirical sanity: a 64×64 rect on clean cobblestone produces 0-2 hard
/// jumps from texture-edge aliasing. A splotch boundary produces 40+ hard
/// jumps along its perimeter. Ceiling of **5** sits comfortably in the gap
/// and catches a single splotch instance with margin.
pub const PBR_HARD_EDGE_MAX_HARD_JUMPS: usize = 5;

/// Canny edge-detection low / high thresholds (per `imageproc::edges::canny`
/// convention — these are luminance-scale thresholds on the gradient
/// magnitude, NOT on the underlying luminance). Logged for diagnostic;
/// failure does NOT depend on canny.
pub const PBR_HARD_EDGE_CANNY_LOW: f32 = 30.0;
pub const PBR_HARD_EDGE_CANNY_HIGH: f32 = 90.0;

// ---------------------------------------------------------------------------
// State resource
// ---------------------------------------------------------------------------

/// Per-run capture stash for the `--pbr-hard-edge` gate. Embedded inside
/// [`super::pbr_visual::PbrVisualState`] (not its own `Resource`) to keep
/// the driver's `SystemParam` arity under Bevy 0.19's tuple ceiling.
#[derive(Default)]
pub struct PbrHardEdgeState {
    pub captured: Option<Framebuffer>,
    pub saved: bool,
}

// ---------------------------------------------------------------------------
// Entry point + camera pose
// ---------------------------------------------------------------------------

/// Boot the e2e harness with `--pbr-hard-edge` mode active.
pub fn run_pbr_hard_edge() -> AppExit {
    let mut app_args = crate::AppArgs::default();
    app_args.pbr_hard_edge_mode = true;
    println!(
        "e2e_render --pbr-hard-edge: splotch-artifact regression gate; \
         warmup {PBR_HARD_EDGE_WARMUP_FRAMES} frames; default test grid; \
         side-on metallic-pillar pose; cobblestone analysis rect {:?}.",
        PBR_HARD_EDGE_RECT,
    );
    crate::run_e2e_render_with_args(app_args)
}

/// Camera pose — reuse the same side-on view as `--pbr-visual` so the
/// cobblestone surface is in frame at the same screen-space rect.
pub fn pbr_hard_edge_pose() -> Transform {
    super::gates::e2e_camera_transform()
}

/// Override the camera pose every frame while the gate is running.
pub fn pin_pbr_hard_edge_camera(
    args: Option<Res<crate::AppArgs>>,
    world_data: Option<Res<WorldData>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else {
        return;
    };
    if !args.pbr_hard_edge_mode {
        return;
    }
    let Some(world_data) = world_data else {
        return;
    };
    let size_v = world_data.size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
    if size_v.x == 0 || size_v.y == 0 || size_v.z == 0 {
        return;
    }
    let pose = pbr_hard_edge_pose();
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
}

// ---------------------------------------------------------------------------
// Save + assertion helpers
// ---------------------------------------------------------------------------

pub fn save_pbr_hard_edge_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --pbr-hard-edge: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --pbr-hard-edge: {filename} save failed: {e}"
        ),
    }
}

/// Rec.709 luminance of an RGBA pixel (`0.0..=255.0`).
fn luma(p: [u8; 4]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// Detect "hard 1-pixel jump" pixels per the gate spec. For every pixel
/// `(x, y)` inside `rect` that has BOTH `(x+1, y)`, `(x+2, y)` AND
/// `(x, y+1)`, `(x, y+2)` neighbours (i.e. 2 pixels of clearance from the
/// rect's right + bottom edges), compute:
///
/// * Horizontal first-diff `D1_h = |L(x+1, y) - L(x, y)|`.
/// * Horizontal second-diff `D2_h = |L(x+2, y) - L(x+1, y)|`.
/// * Vertical first-diff `D1_v = |L(x, y+1) - L(x, y)|`.
/// * Vertical second-diff `D2_v = |L(x, y+2) - L(x, y+1)|`.
///
/// A pixel is flagged if EITHER `(D1_h > t_hard AND D2_h < t_smooth)` OR
/// `(D1_v > t_hard AND D2_v < t_smooth)`. Returns the count of flagged
/// pixels.
pub fn count_hard_one_pixel_jumps(
    fb: &Framebuffer,
    rect: Rect,
    t_hard: f32,
    t_smooth: f32,
) -> usize {
    let mut count = 0usize;
    // Pre-compute luma per pixel in the rect for speed + clarity.
    let w = (rect.x1 - rect.x0) as usize;
    let h = (rect.y1 - rect.y0) as usize;
    if w < 3 || h < 3 {
        return 0;
    }
    let mut lumas = vec![0.0f32; w * h];
    for j in 0..h {
        for i in 0..w {
            lumas[j * w + i] = luma(fb.pixel(rect.x0 + i as u32, rect.y0 + j as u32));
        }
    }
    for j in 0..h - 2 {
        for i in 0..w - 2 {
            let l0 = lumas[j * w + i];
            let l1h = lumas[j * w + i + 1];
            let l2h = lumas[j * w + i + 2];
            let d1h = (l1h - l0).abs();
            let d2h = (l2h - l1h).abs();
            let l1v = lumas[(j + 1) * w + i];
            let l2v = lumas[(j + 2) * w + i];
            let d1v = (l1v - l0).abs();
            let d2v = (l2v - l1v).abs();
            if (d1h > t_hard && d2h < t_smooth) || (d1v > t_hard && d2v < t_smooth) {
                count += 1;
            }
        }
    }
    count
}

/// Run `imageproc::edges::canny` on the rect's luminance channel as a
/// corroborating diagnostic. Returns the count of edge pixels canny flagged.
/// Logged but NOT asserted — the primary assertion is the hard-jump count.
pub fn count_canny_edges(
    fb: &Framebuffer,
    rect: Rect,
    low: f32,
    high: f32,
) -> usize {
    let w = (rect.x1 - rect.x0) as u32;
    let h = (rect.y1 - rect.y0) as u32;
    if w < 3 || h < 3 {
        return 0;
    }
    // Build a luma-only GrayImage for canny. `imageproc::edges::canny`
    // requires a `GrayImage` (`ImageBuffer<Luma<u8>, _>`).
    let mut buf = image::GrayImage::new(w, h);
    for j in 0..h {
        for i in 0..w {
            let p = fb.pixel(rect.x0 + i, rect.y0 + j);
            let l = (luma(p) + 0.5).clamp(0.0, 255.0) as u8;
            buf.put_pixel(i, j, image::Luma([l]));
        }
    }
    let edges = imageproc::edges::canny(&buf, low, high);
    let mut count = 0usize;
    for pixel in edges.pixels() {
        if pixel.0[0] > 0 {
            count += 1;
        }
    }
    count
}

pub fn assert_pbr_hard_edge(fb: &Framebuffer) -> Result<String, String> {
    let hard_jumps = count_hard_one_pixel_jumps(
        fb,
        PBR_HARD_EDGE_RECT,
        PBR_HARD_EDGE_T_HARD,
        PBR_HARD_EDGE_T_SMOOTH,
    );
    let canny_edges = count_canny_edges(
        fb,
        PBR_HARD_EDGE_RECT,
        PBR_HARD_EDGE_CANNY_LOW,
        PBR_HARD_EDGE_CANNY_HIGH,
    );

    let report = format!(
        "pbr-hard-edge: rect {:?}; hard 1-pixel jumps = {hard_jumps} \
         (ceil {PBR_HARD_EDGE_MAX_HARD_JUMPS}, T_hard {:.1}, T_smooth {:.1}); \
         canny edges = {canny_edges} (diagnostic; thresholds {:.1}/{:.1})",
        PBR_HARD_EDGE_RECT,
        PBR_HARD_EDGE_T_HARD,
        PBR_HARD_EDGE_T_SMOOTH,
        PBR_HARD_EDGE_CANNY_LOW,
        PBR_HARD_EDGE_CANNY_HIGH,
    );
    println!("e2e_render --pbr-hard-edge: {report}");

    if hard_jumps > PBR_HARD_EDGE_MAX_HARD_JUMPS {
        return Err(format!(
            "pbr-hard-edge gate FAIL — {hard_jumps} hard 1-pixel luminance \
             jumps detected in cobblestone analysis rect, exceeding ceiling \
             {PBR_HARD_EDGE_MAX_HARD_JUMPS}. The LIGHT INTEGRATION pipeline \
             is producing splotch artifacts with sharp pixel-level \
             discontinuities (per user-report image #13 — olive/green \
             discoloured patches on cobblestone with HARD 1-pixel \
             boundaries that survive denoise OFF / sample_leveling OFF). \
             See `docs/orchestrate/pbr-raymarching/05-diagnostic.md` § \
             \"LIGHT INTEGRATION splotch diagnose+fix (post-`46e50cd`)\". \
             {report}. Inspect target/e2e-screenshots/{PBR_HARD_EDGE_PNG}.",
        ));
    }

    Ok(format!(
        "pbr-hard-edge gate PASS — {hard_jumps} hard jumps (ceil \
         {PBR_HARD_EDGE_MAX_HARD_JUMPS}). {report}.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic framebuffer with a SHARP 1-pixel-wide jump in the middle
    /// MUST be flagged. Build a 16×16 RGBA buffer: left half bright, right
    /// half dark, with the boundary at x=8 — exactly the splotch artifact
    /// signature.
    #[test]
    fn detects_synthetic_hard_jump() {
        let w = 16u32;
        let h = 16u32;
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = if x < 8 { 240u8 } else { 80u8 };
                data.push([v, v, v, 255]);
            }
        }
        let fb = Framebuffer::from_raw_rgba(data, w, h);
        let rect = Rect { x0: 0, y0: 0, x1: w, y1: h };
        let n = count_hard_one_pixel_jumps(
            &fb,
            rect,
            PBR_HARD_EDGE_T_HARD,
            PBR_HARD_EDGE_T_SMOOTH,
        );
        assert!(
            n > 0,
            "synthetic 50% step at x=8 should be flagged as hard jumps; got {n}"
        );
    }

    /// A synthetic framebuffer with a SMOOTH gradient MUST NOT be flagged.
    /// Build a 16×16 with luminance ramping 80..=240 linearly across x —
    /// every pixel's first-diff is `(240-80)/16 = 10` units, well below
    /// `T_HARD` (76 units). And even if T_HARD were lower, the second-diff
    /// is the SAME 10 units → not <T_SMOOTH (25 units) reliably below it,
    /// but in any case here D1 is well below T_HARD.
    #[test]
    fn ignores_synthetic_smooth_gradient() {
        let w = 16u32;
        let h = 16u32;
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let v = (80.0 + t * 160.0) as u8;
                data.push([v, v, v, 255]);
            }
        }
        let fb = Framebuffer::from_raw_rgba(data, w, h);
        let rect = Rect { x0: 0, y0: 0, x1: w, y1: h };
        let n = count_hard_one_pixel_jumps(
            &fb,
            rect,
            PBR_HARD_EDGE_T_HARD,
            PBR_HARD_EDGE_T_SMOOTH,
        );
        assert_eq!(
            n, 0,
            "smooth 10-unit/pixel ramp should NOT be flagged as hard jumps; got {n}"
        );
    }

    /// A natural self-shadowed dip ramps over 3-4 pixels. Build a synthetic
    /// "V-shaped" dip where the second-diff is comparable to the first-diff
    /// → MUST NOT be flagged.
    #[test]
    fn ignores_self_shadowed_dip() {
        let w = 16u32;
        let h = 16u32;
        let baseline = 200u8;
        let mut data = Vec::with_capacity((w * h) as usize);
        // y-direction: smooth dip pattern (120, 150, 180, 200, ...)
        let pattern: [u8; 16] =
            [200, 180, 150, 120, 150, 180, 200, 200, 200, 200, 200, 200, 200, 200, 200, 200];
        for y in 0..h {
            for _x in 0..w {
                let _ = baseline;
                let v = pattern[y as usize];
                data.push([v, v, v, 255]);
            }
        }
        let fb = Framebuffer::from_raw_rgba(data, w, h);
        let rect = Rect { x0: 0, y0: 0, x1: w, y1: h };
        let n = count_hard_one_pixel_jumps(
            &fb,
            rect,
            PBR_HARD_EDGE_T_HARD,
            PBR_HARD_EDGE_T_SMOOTH,
        );
        // First-diffs are ~30 units (below T_HARD=76); no flags.
        assert_eq!(n, 0, "smooth shadow dip should NOT be flagged; got {n}");
    }
}
