//! `--pbr-hard-edge` mode — splotch-artifact regression gate.
//!
//! ## Repositioned 2026-05-19 — close-up top-down cobblestone capture
//!
//! The original `46e50cd` `--pbr-hard-edge` gate framed the **whole** Default
//! test scene at 256×256 from the side-on metallic-pillar pose. At that pixel
//! density the cobblestone ground was zoomed-out aerial of tiny tiles and
//! the user-reported splotch artifact (olive/green discoloured patches with
//! sharp 1-pixel boundaries — see `docs/orchestrate/pbr-raymarching/
//! 05-diagnostic.md` § "LIGHT INTEGRATION splotch diagnose+fix (post-
//! `46e50cd`)") literally **could not physically manifest** in the analysis
//! rect — every "voxel" was a handful of pixels wide.
//!
//! User directive (2026-05-19, verbatim):
//!
//! > target/e2e-screenshots/pbr_hard_edge_baseline.png — this LITERALLY
//! > cant see a SINGLE case of it — its zoomed the fuck out
//! >
//! > position the camera OVER the gound plane in a non-shadowed area,
//! > like 3-5 meters above it, looking directly DOWN
//!
//! Follow-up correction (2026-05-19, post-`22ff1f5` first attempt):
//!
//! > this area was not lit, it was in shadow
//!
//! The post-`22ff1f5` first attempt at small-relative `(50, 32)` landed
//! the camera footprint partially inside **box B's shadow**: box B
//! (`x=38..52, z=40..55, y_top=16`) projects a shadow southward along
//! the sun's `-z` direction to `z = 40 - 16 * (sun_z / sun_y) = 40 -
//! 16 * 0.351 / 0.783 ≈ 32.82`. The 768×768 / 45° / 4m-above frame
//! covers `z ≈ 30.3..33.7` at the ground — its northern third lies
//! INSIDE box B's shadow envelope. The captured baseline showed
//! uniformly dark cobblestone with no sun-lit reference: confirms
//! shadow coverage. **Repositioned again to small-relative
//! `(32, 14)`** — clear of every voxel object's `-x/-z` shadow
//! projection (see "Non-shadowed patch" geometry below).
//!
//! ### New pose
//!
//! - **Camera position:** world `(demo_origin + (32, 7, 14))` —
//!   small-relative `(32, 7, 14)`. The Default test grid's ground slab
//!   is `y=0..=2` (`crate::voxel::grid::build_default_volume`); the
//!   cobblestone surface tops out at `y=3`. Camera at `y=7` sits **4
//!   voxels (= 4 m) above the surface** — squarely in the user-spec
//!   "3-5 m above" band.
//!
//! - **Look direction:** straight down (`Vec3::NEG_Y`). The up reference
//!   for the look-at math is `Vec3::Z`, so the framebuffer's up direction
//!   = world +Z (small-relative +Z is "back" toward box B). Any
//!   perpendicular non-Y vector works as the up reference; Z is the
//!   cleanest documentary choice.
//!
//! - **Non-shadowed patch.** The sun direction is
//!   `(0.514, 0.783, 0.351)` (from `render/atmosphere.rs:323-330` — elev
//!   0.9 rad / azim 0.6 rad), so shadows fall toward `(-x, -z)`. The
//!   shadow envelope of every voxel object projected onto the ground
//!   along `+sun_dir`:
//!
//!   | Object                 | Bounds (x,z)        | y_top | Ground shadow (x,z) |
//!   |------------------------|---------------------|-------|---------------------|
//!   | Box A                  | (12..23, 14..25)    | 20    | (0.85..23, 6.38..25) |
//!   | Box B                  | (38..52, 40..55)    | 16    | (29.47..52, 34.17..55) |
//!   | Back wall              | (56..60, 14..49)    | 22    | (42.45..60, 5.03..49) |
//!   | Sphere 1 (centre)      | r=8 @ (30, 11, 30)  | 19    | r≈8 @ (~19.5, ~22.8) |
//!   | Pillar row (z=8..11)   | x=26..29/34..37/42..45 | 17–19 | (22..45, 0.4..11) |
//!   | NW corner tower        | (54..61, 2..9)      | 21    | (41..61, -6..9)     |
//!   | NE corner tower        | (54..61, 54..61)    | 24    | (39..61, 44..61)    |
//!   | SW corner tower        | (2..9, 2..9)        | 26    | (-14..9, -8..9)     |
//!   | SW(z) corner tower     | (2..9, 54..61)      | 18    | (-9..9, 46..61)     |
//!   | Emissive amber float   | (46..51, 46..51)    | 29    | (29..51, 35..51)    |
//!
//!   Small-relative `(32, 14)` (3.3 m frame footprint → `30.3..33.7,
//!   12.3..15.7`):
//!
//!   - Box A shadow ends at `x=23` → frame east of A by `>7 vx`. CLEAR.
//!   - Box B shadow ends at `z=34.17` → frame south of B by `>18 vx`. CLEAR.
//!   - Wall shadow ends at `x=42.45` → frame west of wall by `>8 vx`. CLEAR.
//!   - Sphere shadow centred (19.5, 22.8) r=8 → frame is `>14 vx` away. CLEAR.
//!   - Pillar shadow ends at `z=11` → frame north of pillars by `>1.3 vx`. CLEAR.
//!   - All corner-tower shadows fall in distinct corners. CLEAR.
//!   - Emissive amber shadow ends at `z=35.38` → frame south by `>19 vx`. CLEAR.
//!
//!   The frame is also `>6 vx` from sphere 1 (no green-tinted GI bounce),
//!   `>7 vx` from box A (no warm-red GI bounce), and `>26 vx` from box B
//!   (no cool-blue GI bounce) — the cobblestone reads its native
//!   `stone_wall_04` colour with sun direct + sky bounce only.
//!
//! - **Resolution:** [`PBR_HARD_EDGE_WIDTH`] × [`PBR_HARD_EDGE_HEIGHT`] =
//!   **768×768**. The standard 256×256 e2e window was the original gate's
//!   blind-spot. At 768×768 with a 45° FOV camera 4 m above the ground,
//!   the visible footprint is roughly `2*4*tan(22.5°) ≈ 3.3 m × 3.3 m`
//!   on the cobblestone surface — three or so cobblestone voxels across
//!   the frame, each filling **~256 pixels**. Each cobblestone tile
//!   within a voxel reads at the same per-tile pixel density the user
//!   observed the splotch at.
//!
//! ### Analysis rect
//!
//! [`PBR_HARD_EDGE_RECT`] is `(330, 330)-(420, 420)` — a **90×90 px**
//! rect centred at frame centre `(384, 384)`. At the new pose the centre
//! voxel `(50, 32)` projects exactly to `(384, 384)`, and one voxel spans
//! ~256 pixels, so the rect lives **wholly inside one cobblestone voxel
//! face**, with ~60-70 px clearance from the nearest voxel boundary on
//! every side. The 90² area gives the detector ~8100 sample pixels,
//! which is large enough for a single splotch instance to register
//! comfortably above the ceiling-of-5 threshold.
//!
//! ## Algorithm (unchanged from `46e50cd`)
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
// Screenshot filenames
// ---------------------------------------------------------------------------

/// Full-framebuffer PNG written by the gate on success — overwritten every run.
pub const PBR_HARD_EDGE_PNG: &str = "pbr_hard_edge_baseline.png";

/// Analysis-rect crop PNG, written alongside the full capture so the user
/// can see exactly the region the metric is looking at.
pub const PBR_HARD_EDGE_RECT_PNG: &str = "pbr_hard_edge_rect.png";

// ---------------------------------------------------------------------------
// Window resolution (the gate runs at a higher resolution than the standard
// 256×256 e2e window — the original gate's blind spot was that at 256×256
// each cobblestone voxel was a handful of pixels and the splotch artifact
// could not physically manifest in the analysis rect).
// ---------------------------------------------------------------------------

/// Window width for this gate (logical pixels). Picked to give each
/// cobblestone voxel ~256 px of resolution at the close-up top-down pose
/// (camera 4 m above, ~3 voxels visible, 768/3 ≈ 256 px per voxel).
pub const PBR_HARD_EDGE_WIDTH: u32 = 768;
/// Window height — square aspect to match the analysis rect's square shape.
pub const PBR_HARD_EDGE_HEIGHT: u32 = 768;

// ---------------------------------------------------------------------------
// Frame budget — reuses the same convergence schedule as `--pbr-visual` so
// the TAA / GI buckets are settled at capture time.
// ---------------------------------------------------------------------------

pub const PBR_HARD_EDGE_WARMUP_FRAMES: u32 = 150;
pub const PBR_HARD_EDGE_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// Analysis rect + thresholds
// ---------------------------------------------------------------------------

/// 90×90 px rect centred at frame centre `(384, 384)` on the 768×768
/// framebuffer. At the new top-down pose the centre voxel
/// (small-relative `(32, 14)`) projects exactly to frame centre and one
/// voxel spans ~256 px, so the rect sits well inside a single cobblestone
/// voxel face with ~60-70 px clearance from the nearest voxel boundary
/// on every side.
///
/// **Why bigger than the prior 50×20 rect (`46e50cd`):** the user-reported
/// splotch (Images #13/#17) has visible features ~30-50 px across at the
/// production resolution. A 90² rect gives the detector room to capture
/// at least one splotch boundary while still fitting inside a single
/// voxel at the new pose.
pub const PBR_HARD_EDGE_RECT: Rect = Rect { x0: 330, y0: 330, x1: 420, y1: 420 };

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
         warmup {PBR_HARD_EDGE_WARMUP_FRAMES} frames at {PBR_HARD_EDGE_WIDTH}\
         x{PBR_HARD_EDGE_HEIGHT}; default test grid; top-down close-up pose \
         4 m above small-rel (32, 0..2, 14) cobblestone (clear of every \
         object's -x/-z shadow envelope); analysis rect {:?}.",
        PBR_HARD_EDGE_RECT,
    );
    crate::run_e2e_render_with_args(app_args)
}

/// Top-down close-up camera pose, 4 m above the Default-scene cobblestone
/// ground at small-relative `(32, 14)`. See module docs for the patch
/// selection (sun direction, shadow geometry, surrounding objects); the
/// shadow-envelope table proves the entire 3.3 m frame footprint sits
/// outside every voxel object's `-x/-z` shadow projection along the
/// `(0.514, 0.783, 0.351)` sun direction.
pub fn pbr_hard_edge_pose() -> Transform {
    let off = super::gates::demo_origin_v();
    // Small-relative voxel position: (32, 7, 14). Camera Y=7 = 4 voxels
    // above the y=3 ground surface (slab fills y=0..=2). World position
    // = demo_origin + (32, 7, 14).
    let cam = off + Vec3::new(32.0, 7.0, 14.0);
    let target = off + Vec3::new(32.0, 0.0, 14.0);
    // Looking straight down with `Vec3::Z` as the up reference makes the
    // framebuffer's up direction = world +Z (small-relative +Z, toward
    // box B). Any perpendicular non-Y vector works; Z is the cleanest
    // documentary choice.
    Transform::from_translation(cam).looking_at(target, Vec3::Z)
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

/// Save the analysis-rect crop as a separate PNG so the user can see
/// exactly what the metric is looking at. The rect's pixels are copied
/// out of `fb` into a fresh `Framebuffer` sized to the rect, then
/// PNG-encoded the same way the full framebuffer is.
pub fn save_pbr_hard_edge_rect_crop(fb: &Framebuffer, rect: Rect, filename: &str) {
    let w = rect.x1.saturating_sub(rect.x0);
    let h = rect.y1.saturating_sub(rect.y0);
    if w == 0 || h == 0 {
        eprintln!(
            "e2e_render --pbr-hard-edge: rect-crop save skipped — rect {:?} \
             has zero width or height",
            rect,
        );
        return;
    }
    let mut data = Vec::with_capacity((w * h) as usize);
    for j in 0..h {
        for i in 0..w {
            data.push(fb.pixel(rect.x0 + i, rect.y0 + j));
        }
    }
    let crop = Framebuffer::from_raw_rgba(data, w, h);
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match crop.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --pbr-hard-edge: rect crop saved to {}",
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
             \"LIGHT INTEGRATION splotch diagnose+fix (post-`46e50cd`)\" \
             and § \"--pbr-hard-edge gate rebuilt — sunlit cobblestone \
             top-down (post-`22ff1f5`)\". {report}. Inspect \
             target/e2e-screenshots/{PBR_HARD_EDGE_PNG} (full frame) and \
             target/e2e-screenshots/{PBR_HARD_EDGE_RECT_PNG} (rect crop).",
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
