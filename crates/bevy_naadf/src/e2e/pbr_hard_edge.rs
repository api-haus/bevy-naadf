//! `--pbr-hard-edge` mode — splotch-artifact regression gate.
//!
//! ## REBUILT 2026-05-19 (post-`163cbac`) — facing-voxel close zoom + median-filtered detector
//!
//! The post-`a2c3aff` sun-direct splotch fix landed (gate 79 → 2 hard
//! jumps on the prior sunlit top-down pose) and the user confirms a
//! MAJOR improvement, but reports residual low-contrast noisy artifacts
//! on voxel types #2/#3/#7 (red cobblestone / red stone / snow) that:
//!
//! 1. **Only manifest while in shadow** — direct-sun pixels are clean.
//! 2. **React to camera angle** — view-dependent, not surface-fixed.
//! 3. Have a **6% HSV-V delta** (37% baseline → 43% clipped, i.e. ~16/255
//!    luma units) — too low-contrast for the prior `T_HARD = 76` detector.
//! 4. Are **NOISY** — naive per-pixel thresholding triggers on per-pixel
//!    GI MC noise.
//!
//! User direction (verbatim, 2026-05-19):
//!
//! > try to rotate the camera 180 degrees from its strating position on
//! > defautl scene and align in very closely with the red cobblestone voxel
//! > material behind it voxel type #2 2028, 6, 2034
//! >
//! > you have to align the camera so that its FACING the voxel OR increase
//! > resolution — otherwise you wont catch that
//! >
//! > your best bet — zoom in precisely to (0.5,0.5) of the voxel
//! >
//! > its barely visible AND noisy (not so many harsh pixels)
//! >
//! > goes from 37%(normal, darker) to 43% (clipped, brighter)
//!
//! ### New pose: facing the west face of red cobblestone voxel `(2028, 6, 2034)`
//!
//! - **Target voxel:** `(2028, 6, 2034)` in world voxel-int coordinates =
//!   `demo_origin_v + (12, 6, 18)` in small-relative coordinates. This is
//!   inside `TY_BOX_A` (`crate::voxel::grid::build_default_volume:730` —
//!   `fill_box [12,3,14]..=[23,20,25]` with VoxelTypeId 2 / `ground_tiles
//!   _08` red-cobblestone material with tint `[204, 76, 56]`). The
//!   **west face** of this voxel is the plane `x=12.0` with face normal
//!   `(-1, 0, 0)` and face centre `(12.0, 6.5, 18.5)`.
//!
//! - **Shadow proof.** Sun direction is `(0.514, 0.783, 0.351)`
//!   (`render/atmosphere.rs:323-330` — elev 0.9 rad / azim 0.6 rad).
//!   The west face normal dotted with the sun:
//!   `dot((-1, 0, 0), (0.514, 0.783, 0.351)) = -0.514 < 0` — the sun is
//!   on the wrong side of this surface, so direct-sun light is zero
//!   (geometric self-shadow). All visible light on this face comes from
//!   the INDIRECT/GI pathway — the regime the user reports the residual
//!   in.
//!
//! - **180° rotation vs default-scene start** (per user direction).
//!   `install_default_embedded_in_fixed_world` (`voxel/grid.rs:193-196`)
//!   spawns the camera at small-rel `(11, 7, 17)` looking at `(0, 4, -3)`
//!   — look direction `(-11, -3, -20)` (toward -x, -z, downward). The
//!   new pose looks toward `+x` (face normal -X → camera looks +X to face
//!   it). The +x component of the new look-direction is the diametric
//!   opposite of the default's -x component → 180° yaw rotation.
//!
//! - **Camera position.** Small-relative `(10.8, 6.5, 18.5)` — 1.2 m west
//!   of the voxel face, vertically centred on the face mid-height
//!   (`y=6.5` — the voxel spans `y=6..7`), z-centred on the face
//!   (`z=18.5` — voxel spans `z=18..19`). Camera is in empty space (Box
//!   A is `x=12..23`, camera at `x=10.8` is 1.2 m west of the box's
//!   western boundary, no occluder between camera and target).
//!
//! - **Camera look direction.** Straight at the face centre
//!   `(12.0, 6.5, 18.5)`. Look vector `(+1.2, 0, 0)` → pure `+x`. The
//!   `up` reference is `+Y` (no tilt — face normal is horizontal +/-X
//!   plane, the framebuffer's vertical reads as world +Y).
//!
//! - **Framing math.** At `d = 1.2 m` with `FOV = 45°` and `768×768`
//!   framebuffer, the visible footprint at the face plane is
//!   `2 * 1.2 * tan(22.5°) ≈ 0.994 m × 0.994 m`. Each 1 m voxel face
//!   spans `768 / 0.994 ≈ 772 px` — the target voxel's west face fills
//!   essentially the entire framebuffer at single-voxel resolution.
//!   This is the "zoom precisely to (0.5, 0.5) of the voxel" framing the
//!   user requested.
//!
//! - **Resolution.** [`PBR_HARD_EDGE_WIDTH`] × [`PBR_HARD_EDGE_HEIGHT`] =
//!   **768×768** (kept from the prior pose). At ~1 voxel filling the
//!   frame, each cobblestone tile within the texture spans ~50-100 px —
//!   well into the user-reported splotch-feature-size range.
//!
//! ### Analysis rect
//!
//! [`PBR_HARD_EDGE_RECT`] is a `400×400 px` rect centred at frame centre
//! `(384, 384)` — i.e. `(184, 184)..(584, 584)`. At the new pose this is
//! a 0.52 m × 0.52 m patch in the centre of the voxel face — wholly
//! inside one voxel, covering ~40-60 cobblestone tiles, sampling the
//! shadow-only GI-pathway residual splotch densely.
//!
//! ### Detector retune: stone-interior masked hard-jump detector
//!
//! **Why the prior hard-jump detector cannot catch this artifact.** The
//! prior `T_HARD = 76` (~30% V) was calibrated against the SUN-DIRECT
//! splotch at sunlit luma ~200 (a ~100-unit dip). The shadow-only
//! residual is 6% V (~16/255 luma units) at baseline luma ~94 —
//! physically below the prior threshold by ~5x.
//!
//! **Why naive V-band thresholding doesn't work.** The cobblestone
//! texture has natural per-tile brightness variation (some stones are
//! intrinsically brighter than the median). A V > median+10 threshold
//! captures both real splotches AND legitimate brighter stones —
//! impossible to distinguish without a reference image.
//!
//! **Why the simple hard-jump count doesn't work at low thresholds.**
//! The cobblestone texture has 2-px-wide moss gaps with sharp 1-px
//! transitions (V ~95 → V ~40 → V ~40 → V ~95 across 4 pixels). At
//! `T_HARD = 10` this trips the hard-jump predicate ~1500 times across
//! the 400×400 rect — drowning the actual splotch signal.
//!
//! **Chosen algorithm: stone-interior-masked hard-jump count.** The key
//! insight: the splotch artifact lives INSIDE stone tiles (V above the
//! green-moss V floor). The moss gaps live OUTSIDE (V below the floor).
//! By masking the hard-jump detector to only count jumps where BOTH
//! pixels of the jump are above the moss-V floor, the moss-boundary
//! noise is excluded while the splotch boundaries remain detectable.
//!
//! Algorithm:
//!
//! 1. Crop rect → HSV-V `GrayImage` (`max(R, G, B)`).
//! 2. Apply `imageproc::filter::median_filter(img, 1, 1)` (3×3 box) to
//!    suppress per-pixel MC noise.
//! 3. Compute the per-rect median V (`V_med` ≈ 105 on red cobblestone
//!    in shadow).
//! 4. Run the first-diff/second-diff hard-jump predicate at lowered
//!    thresholds `T_HARD = 10`, `T_SMOOTH = 5`, but ONLY count flagged
//!    pixels where BOTH `L(x)` AND `L(x+1)` (or `L(x, y)` AND `L(x,
//!    y+1)` for vertical) are >= the stone-interior floor `V_med -
//!    V_STONE_FLOOR_BELOW_MEDIAN` (so the moss gaps with V ~40-60 are
//!    excluded since they fall below `V_med - 35 = 70`).
//!
//! Why this catches the splotch and ignores the moss:
//! - **Splotch boundary:** both sides of the boundary live in stone
//!   interior (V ~88 and V ~110, both > 70). The hard-jump predicate
//!   flags the boundary; the stone-interior floor admits both sides;
//!   the pixel counts toward the FAIL metric.
//! - **Moss gap boundary:** one side is stone (V ~95, > 70), one side
//!   is moss (V ~40-60, < 70). The hard-jump predicate flags the
//!   transition; the stone-interior floor REJECTS it (one side below
//!   floor); the pixel is NOT counted.
//! - **Per-pixel noise:** median pre-filter kills it.
//! - **Inter-tile brightness variation:** the boundary is the MOSS
//!   GAP, not a splotch boundary — handled by the moss exclusion above.
//!
//! The unit tests are extended with:
//! - A high-frequency Gaussian-noise rect (mean 94, std 5) → MUST pass
//!   (median kills noise).
//! - A 30-px-wide coherent low-contrast bump (V 94 → 110 inside) →
//!   MUST fail (splotch shape inside the stone-interior floor).
//! - A natural texture test: synthetic moss gaps (V drops to 50) MUST
//!   pass (moss boundaries excluded by the stone-interior floor).
//!
//! The original sharp-jump / smooth-gradient / self-shadowed-dip tests
//! are preserved.

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

/// Median-filtered analysis-rect PNG — shows the noise-suppressed input the
/// detector actually consumes. Diagnostic; lets the user see whether the
/// median pre-filter is preserving / killing the suspected splotch features.
pub const PBR_HARD_EDGE_MEDIAN_PNG: &str = "pbr_hard_edge_median.png";

// ---------------------------------------------------------------------------
// Window resolution — 768×768 (kept from prior gate). At the new facing-voxel
// pose ~1 voxel face fills the frame, so each cobblestone tile within the
// texture spans ~50-100 px — comfortably in the splotch-feature-size range.
// ---------------------------------------------------------------------------

/// Window width for this gate (logical pixels).
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
// Analysis rect + thresholds (REBUILT 2026-05-19 — shadow-only residual)
// ---------------------------------------------------------------------------

/// 400×400 px rect centred on the framebuffer at the facing-voxel close-up
/// pose. At 1.2 m distance, FOV 45°, 768×768 framebuffer the entire frame
/// is one voxel face; this central 400² patch is wholly inside that face
/// and covers ~40-60 cobblestone tiles — a dense sample of the shadow-only
/// GI-pathway residual splotch.
pub const PBR_HARD_EDGE_RECT: Rect = Rect { x0: 184, y0: 184, x1: 584, y1: 584 };

/// 3×3 median pre-filter radius (`imageproc::filter::median_filter(img, 1, 1)`
/// — 1 px in each direction → 3×3 box). Kills isolated-pixel MC noise while
/// preserving edges 2+ pixels wide. Tuned to balance noise suppression
/// against feature preservation: 5×5 (`radius=2`) over-smooths the splotch
/// boundary; 3×3 (`radius=1`) leaves coherent low-contrast splotches
/// detectable.
pub const PBR_HARD_EDGE_MEDIAN_RADIUS: u32 = 1;

/// Stone-interior V floor (V units below per-rect median V). Hard-jump
/// pixels are only counted if BOTH the L(x) and L(x+1) pixels of the
/// jump are >= `V_med - V_STONE_FLOOR_BELOW_MEDIAN` — this excludes
/// moss-gap boundaries (where one side is moss V ~40-60) while
/// admitting splotch boundaries (where both sides are stone-interior V
/// ~88-110).
///
/// Sizing: red cobblestone median V is ~105 on the new pose. Stone
/// interiors vary V ~85-130 (natural texture brightness ± ~25). Moss
/// gaps drop V to ~40-60 (median - 45 to median - 65). Floor at
/// median - 35 = ~70 admits all stone-interior pixels and rejects all
/// moss-gap pixels.
pub const PBR_HARD_EDGE_V_STONE_FLOOR_BELOW_MEDIAN: i32 = 35;

/// First-difference threshold for a "hard" 1-pixel jump in HSV-V
/// (`0..=255` scale) AFTER median pre-filter, within the stone-interior
/// mask.
///
/// **User-supplied calibration (post-`163cbac`):**
///
/// > goes from 37%(normal, darker) to 43% (clipped, brighter)
/// > delta = 6% = ~16/255
///
/// `T_HARD = 10` (~4% V) sits below the user-reported 6% V delta so the
/// splotch boundary's single-pixel step is caught with margin.
pub const PBR_HARD_EDGE_T_HARD: f32 = 10.0;

/// Second-difference ceiling. A "hard" jump's NEXT step must be small —
/// otherwise the jump is the start of a gradient. Splotch boundaries
/// produce one-pixel discontinuities (`D1 high, D2 ≈ 0`); natural
/// texture gradients produce sustained ramps (`D1 ≈ D2`).
pub const PBR_HARD_EDGE_T_SMOOTH: f32 = 5.0;

/// Maximum number of stone-interior hard-jump pixels permitted in the
/// analysis rect.
///
/// **Calibration (rect 400×400 = 160k px, post-median, post-moss-mask).**
/// A clean shadowed cobblestone rect produces ≤ 10-50 stone-interior
/// hard-jumps (residual noise spikes that survive the median). A
/// splotch with a coherent ~30 px boundary produces 100+ stone-interior
/// hard-jumps along its perimeter. Ceiling of **80** sits in the gap.
pub const PBR_HARD_EDGE_MAX_HARD_JUMPS: usize = 80;

/// Canny edge-detection low / high thresholds (per `imageproc::edges::canny`
/// convention — luminance-scale thresholds on the gradient magnitude).
/// Logged for diagnostic; failure does NOT depend on canny.
pub const PBR_HARD_EDGE_CANNY_LOW: f32 = 8.0;
pub const PBR_HARD_EDGE_CANNY_HIGH: f32 = 20.0;

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
        "e2e_render --pbr-hard-edge: shadow-only residual splotch gate; \
         warmup {PBR_HARD_EDGE_WARMUP_FRAMES} frames at {PBR_HARD_EDGE_WIDTH}\
         x{PBR_HARD_EDGE_HEIGHT}; default scene; FACING-VOXEL CLOSE ZOOM pose \
         (post-`163cbac`) — camera at small-rel (10.8, 6.5, 18.5) looking +X \
         at red-cobblestone voxel #2 west face centre (small-rel \
         (12, 6.5, 18.5) = world (2028, 6, 2034)); face normal -X, sun dot \
         < 0, in self-shadow; GI-pathway residual splotch only; analysis \
         rect {:?}; median radius {}, T_hard {:.1}, T_smooth {:.1}, ceil {}.",
        PBR_HARD_EDGE_RECT,
        PBR_HARD_EDGE_MEDIAN_RADIUS,
        PBR_HARD_EDGE_T_HARD,
        PBR_HARD_EDGE_T_SMOOTH,
        PBR_HARD_EDGE_MAX_HARD_JUMPS,
    );
    crate::run_e2e_render_with_args(app_args)
}

/// FACING-VOXEL CLOSE-ZOOM POSE (2026-05-19, post-`163cbac`) — head-on view
/// of the red cobblestone voxel `(2028, 6, 2034)` west face from 1.2 m
/// distance.
///
/// **Target voxel.** `(2028, 6, 2034)` in world voxel-int coords =
/// `demo_origin_v + (12, 6, 18)` in small-relative coords. Inside Box A
/// (`build_default_volume:730` — VoxelTypeId 2 = red cobblestone). The
/// **west face** is the plane `x = 12.0` with face normal `(-1, 0, 0)`
/// and face centre `(12.0, 6.5, 18.5)`.
///
/// **Shadow proof.** Sun direction `(0.514, 0.783, 0.351)`. Face normal
/// `(-1, 0, 0)`. `dot(normal, sun) = -0.514 < 0` — direct-sun light is
/// zero on this face (geometric self-shadow). All visible light is GI
/// indirect — the regime the user reports the residual splotch in.
///
/// **180° vs default.** Default cam looks toward `(-x, -y, -z)`; new pose
/// looks toward `+x` (face normal `-X` → camera looks `+X` to face it).
/// The +x look-component is the diametric flip of the default's -x
/// component → 180° yaw rotation.
///
/// **Camera position / orientation.** Small-rel `(10.8, 6.5, 18.5)`,
/// looking at face centre `(12.0, 6.5, 18.5)` → pure `+X` look vector at
/// 1.2 m distance. Camera is in empty space (Box A starts at `x=12`,
/// camera at `x=10.8` is 1.2 m west of the box boundary, no occluder
/// between camera and target). `Vec3::Y` up reference (face normal is
/// horizontal, framebuffer vertical = world +Y).
///
/// **Framing.** 45° FOV / 768² framebuffer at 1.2 m distance →
/// `2 * 1.2 * tan(22.5°) ≈ 0.994 m` visible at the face plane. The 1 m
/// voxel face essentially fills the frame (`~772 px / 768 px`). This is
/// the "zoom precisely to (0.5, 0.5) of the voxel" framing the user
/// requested.
pub fn pbr_hard_edge_pose() -> Transform {
    let off = super::gates::demo_origin_v();
    // Small-relative camera position: (10.8, 6.5, 18.5) — 1.2 m west of
    // the voxel west face (at small-rel x = 12.0), vertically centred on
    // the face mid-height (y = 6.5), z-centred on the face (z = 18.5).
    let cam = off + Vec3::new(10.8, 6.5, 18.5);
    // Target: west face centre at small-rel (12.0, 6.5, 18.5) = world
    // (2028, 6.5, 2034.5). Look vector = (+1.2, 0, 0) → pure +X.
    let target = off + Vec3::new(12.0, 6.5, 18.5);
    Transform::from_translation(cam).looking_at(target, Vec3::Y)
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
/// exactly what the metric is looking at.
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

/// Save the median-filtered luma rect as an RGBA-greyscale PNG. Diagnostic:
/// lets the user see whether the median pre-filter preserves the suspected
/// splotch features and kills the MC noise the detector cannot reason about.
fn save_pbr_hard_edge_median_crop(median: &image::GrayImage, filename: &str) {
    let w = median.width();
    let h = median.height();
    let mut data = Vec::with_capacity((w * h) as usize);
    for j in 0..h {
        for i in 0..w {
            let l = median.get_pixel(i, j).0[0];
            data.push([l, l, l, 255]);
        }
    }
    let crop = Framebuffer::from_raw_rgba(data, w, h);
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match crop.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --pbr-hard-edge: median crop saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --pbr-hard-edge: {filename} save failed: {e}"
        ),
    }
}

/// Rec.709 luminance of an RGBA pixel (`0.0..=255.0`). Retained for
/// backwards compatibility with the legacy hard-jump helper / unit tests.
fn luma(p: [u8; 4]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// HSV-V channel of an RGBA pixel (`0..=255`). This is the user-reported
/// brightness metric (37% V baseline → 43% V splotch).
fn hsv_v(p: [u8; 4]) -> u8 {
    p[0].max(p[1]).max(p[2])
}

/// Build a `GrayImage` of the rect's Rec.709 luminance channel from `fb`.
/// Used by the legacy hard-jump diagnostic + the synthetic unit tests
/// (which feed grey-on-grey patterns where luma == V).
fn rect_to_luma_image(fb: &Framebuffer, rect: Rect) -> image::GrayImage {
    let w = (rect.x1 - rect.x0) as u32;
    let h = (rect.y1 - rect.y0) as u32;
    let mut buf = image::GrayImage::new(w, h);
    for j in 0..h {
        for i in 0..w {
            let p = fb.pixel(rect.x0 + i, rect.y0 + j);
            let l = (luma(p) + 0.5).clamp(0.0, 255.0) as u8;
            buf.put_pixel(i, j, image::Luma([l]));
        }
    }
    buf
}

/// Build a `GrayImage` of the rect's HSV-V channel (`max(R, G, B)`) from
/// `fb`. This is the channel the user-supplied splotch measurement
/// (37% → 43% V) was made in.
fn rect_to_v_image(fb: &Framebuffer, rect: Rect) -> image::GrayImage {
    let w = (rect.x1 - rect.x0) as u32;
    let h = (rect.y1 - rect.y0) as u32;
    let mut buf = image::GrayImage::new(w, h);
    for j in 0..h {
        for i in 0..w {
            let p = fb.pixel(rect.x0 + i, rect.y0 + j);
            buf.put_pixel(i, j, image::Luma([hsv_v(p)]));
        }
    }
    buf
}

/// Apply `imageproc::filter::median_filter` with the given radius to a
/// luma image.
fn median_filter(img: &image::GrayImage, radius: u32) -> image::GrayImage {
    imageproc::filter::median_filter(img, radius, radius)
}

/// Detect "hard 1-pixel jump" pixels on a pre-filtered luma image. For
/// every pixel `(x, y)` that has BOTH `(x+1, y)`, `(x+2, y)` AND
/// `(x, y+1)`, `(x, y+2)` neighbours (2 pixels of clearance from the
/// right + bottom edges), compute:
///
/// * Horizontal first-diff `D1_h = |L(x+1, y) - L(x, y)|`.
/// * Horizontal second-diff `D2_h = |L(x+2, y) - L(x+1, y)|`.
/// * Vertical first-diff `D1_v = |L(x, y+1) - L(x, y)|`.
/// * Vertical second-diff `D2_v = |L(x, y+2) - L(x, y+1)|`.
///
/// A pixel is flagged if EITHER `(D1_h > t_hard AND D2_h < t_smooth)` OR
/// `(D1_v > t_hard AND D2_v < t_smooth)`. Returns the count of flagged
/// pixels.
pub fn count_hard_one_pixel_jumps_luma(
    luma_img: &image::GrayImage,
    t_hard: f32,
    t_smooth: f32,
) -> usize {
    let w = luma_img.width() as usize;
    let h = luma_img.height() as usize;
    if w < 3 || h < 3 {
        return 0;
    }
    let lumas: Vec<f32> = luma_img.pixels().map(|p| p.0[0] as f32).collect();
    let mut count = 0usize;
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

/// Legacy adapter — `count_hard_one_pixel_jumps` on a Framebuffer rect
/// (no median pre-filter). Retained for backwards compat with tests.
pub fn count_hard_one_pixel_jumps(
    fb: &Framebuffer,
    rect: Rect,
    t_hard: f32,
    t_smooth: f32,
) -> usize {
    let luma_img = rect_to_luma_image(fb, rect);
    count_hard_one_pixel_jumps_luma(&luma_img, t_hard, t_smooth)
}

/// Run `imageproc::edges::canny` on a luma image. Returns the count of
/// edge pixels canny flagged.
pub fn count_canny_edges_luma(
    luma_img: &image::GrayImage,
    low: f32,
    high: f32,
) -> usize {
    if luma_img.width() < 3 || luma_img.height() < 3 {
        return 0;
    }
    let edges = imageproc::edges::canny(luma_img, low, high);
    let mut count = 0usize;
    for pixel in edges.pixels() {
        if pixel.0[0] > 0 {
            count += 1;
        }
    }
    count
}

/// Median V over a GrayImage's pixels. O(n log n) — fine for the gate's
/// 160k-pixel rect (single one-shot capture).
fn median_v(img: &image::GrayImage) -> u8 {
    let mut vs: Vec<u8> = img.pixels().map(|p| p.0[0]).collect();
    if vs.is_empty() {
        return 0;
    }
    vs.sort_unstable();
    vs[vs.len() / 2]
}

/// Stone-interior masked hard-jump counter on an HSV-V image. Counts
/// flagged jumps ONLY when BOTH pixels of the jump are at or above
/// `stone_floor` — this excludes moss-gap boundaries (where one side
/// is moss V ~40-60) while admitting splotch boundaries (where both
/// sides are stone-interior V ~88-110).
fn count_hard_jumps_stone_interior(
    v_img: &image::GrayImage,
    t_hard: f32,
    t_smooth: f32,
    stone_floor: u8,
) -> usize {
    let w = v_img.width() as usize;
    let h = v_img.height() as usize;
    if w < 3 || h < 3 {
        return 0;
    }
    let vs: Vec<u8> = v_img.pixels().map(|p| p.0[0]).collect();
    let floor = stone_floor;
    let mut count = 0usize;
    for j in 0..h - 2 {
        for i in 0..w - 2 {
            let v0 = vs[j * w + i];
            let v1h = vs[j * w + i + 1];
            let v2h = vs[j * w + i + 2];
            let v1v = vs[(j + 1) * w + i];
            let v2v = vs[(j + 2) * w + i];

            let mut hit = false;
            // Horizontal jump — both pixels (v0, v1h) must be stone-interior.
            if v0 >= floor && v1h >= floor {
                let d1h = (v1h as f32 - v0 as f32).abs();
                let d2h = (v2h as f32 - v1h as f32).abs();
                if d1h > t_hard && d2h < t_smooth {
                    hit = true;
                }
            }
            // Vertical jump — both pixels (v0, v1v) must be stone-interior.
            if !hit && v0 >= floor && v1v >= floor {
                let d1v = (v1v as f32 - v0 as f32).abs();
                let d2v = (v2v as f32 - v1v as f32).abs();
                if d1v > t_hard && d2v < t_smooth {
                    hit = true;
                }
            }
            if hit {
                count += 1;
            }
        }
    }
    count
}

pub fn assert_pbr_hard_edge(fb: &Framebuffer) -> Result<String, String> {
    // 1. Crop the analysis rect into an HSV-V image (the user-supplied
    //    measurement is in V: 37% baseline → 43% clipped).
    let v_raw = rect_to_v_image(fb, PBR_HARD_EDGE_RECT);
    // 2. Median pre-filter (3×3 box, kills isolated-pixel MC noise).
    let v_med_img = median_filter(&v_raw, PBR_HARD_EDGE_MEDIAN_RADIUS);
    // 3. Save the median crop for diagnostic.
    save_pbr_hard_edge_median_crop(&v_med_img, PBR_HARD_EDGE_MEDIAN_PNG);
    // 4. Per-rect baseline V (median over all pixels).
    let v_med = median_v(&v_med_img);
    // 5. Stone-interior floor: V_med - V_STONE_FLOOR_BELOW_MEDIAN. Moss
    //    gap V (~40-60) falls below this floor; stone interiors (V ~85
    //    upward) stay above it.
    let stone_floor: u8 = (v_med as i32 - PBR_HARD_EDGE_V_STONE_FLOOR_BELOW_MEDIAN)
        .clamp(0, 255) as u8;
    // 6. Run the stone-interior-masked hard-jump detector.
    let hard_jumps = count_hard_jumps_stone_interior(
        &v_med_img,
        PBR_HARD_EDGE_T_HARD,
        PBR_HARD_EDGE_T_SMOOTH,
        stone_floor,
    );
    // 7. Diagnostic: unmasked hard-jumps (for comparison) + canny on V.
    let hard_jumps_unmasked = count_hard_one_pixel_jumps_luma(
        &v_med_img,
        PBR_HARD_EDGE_T_HARD,
        PBR_HARD_EDGE_T_SMOOTH,
    );
    let canny_edges = count_canny_edges_luma(
        &v_med_img,
        PBR_HARD_EDGE_CANNY_LOW,
        PBR_HARD_EDGE_CANNY_HIGH,
    );

    // Diagnostic stats.
    let mean_v: f32 = {
        let n = v_med_img.pixels().len().max(1);
        let sum: f64 = v_med_img.pixels().map(|p| p.0[0] as f64).sum();
        (sum / n as f64) as f32
    };

    let report = format!(
        "pbr-hard-edge: rect {:?}; median radius {}; mean V {:.1}, median V {}, \
         stone floor {stone_floor} (V_med - {}); stone-interior hard-jumps = \
         {hard_jumps} (ceil {PBR_HARD_EDGE_MAX_HARD_JUMPS}, T_h {:.1}, T_s {:.1}); \
         diagnostic unmasked hard-jumps {hard_jumps_unmasked}, canny {canny_edges} \
         ({:.1}/{:.1})",
        PBR_HARD_EDGE_RECT,
        PBR_HARD_EDGE_MEDIAN_RADIUS,
        mean_v,
        v_med,
        PBR_HARD_EDGE_V_STONE_FLOOR_BELOW_MEDIAN,
        PBR_HARD_EDGE_T_HARD,
        PBR_HARD_EDGE_T_SMOOTH,
        PBR_HARD_EDGE_CANNY_LOW,
        PBR_HARD_EDGE_CANNY_HIGH,
    );
    println!("e2e_render --pbr-hard-edge: {report}");

    if hard_jumps > PBR_HARD_EDGE_MAX_HARD_JUMPS {
        return Err(format!(
            "pbr-hard-edge gate FAIL — {hard_jumps} stone-interior hard \
             1-pixel V-jumps detected in shadow-only red-cobblestone \
             analysis rect (post-median 3x3, T_hard {:.1}, T_smooth {:.1}, \
             stone floor V {stone_floor}), exceeding ceiling \
             {PBR_HARD_EDGE_MAX_HARD_JUMPS}. The GI/indirect light \
             pathway is producing low-contrast (~6%V) splotch artifacts \
             with sharp pixel-level boundaries inside individual stones \
             — per user-report images #20-#23 (red cobblestone at \
             small-rel (12, 6, 18) in self-shadow, residual splotch \
             after `a2c3aff` sun-direct fix). See \
             `docs/orchestrate/pbr-raymarching/05-diagnostic.md` § \
             \"Shadow-only residual splotch fix\". {report}. Inspect \
             target/e2e-screenshots/{PBR_HARD_EDGE_PNG} (full frame), \
             target/e2e-screenshots/{PBR_HARD_EDGE_RECT_PNG} (raw rect \
             crop), and target/e2e-screenshots/{PBR_HARD_EDGE_MEDIAN_PNG} \
             (post-median V).",
            PBR_HARD_EDGE_T_HARD,
            PBR_HARD_EDGE_T_SMOOTH,
        ));
    }

    Ok(format!(
        "pbr-hard-edge gate PASS — {hard_jumps} stone-interior hard-jumps \
         (ceil {PBR_HARD_EDGE_MAX_HARD_JUMPS}). {report}.",
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
        for _y in 0..h {
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
    /// every pixel's first-diff is `(240-80)/16 = 10` units. With the
    /// post-`163cbac` T_HARD = 10 the boundary is at the threshold itself
    /// — but the second-diff is the SAME 10 units, which is > T_SMOOTH = 5,
    /// so the smoothness predicate excludes the pixel.
    #[test]
    fn ignores_synthetic_smooth_gradient() {
        let w = 16u32;
        let h = 16u32;
        let mut data = Vec::with_capacity((w * h) as usize);
        for _y in 0..h {
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
    ///
    /// **Pattern shape note (post-`163cbac`):** the prior dip pattern
    /// `200, 180, 150, 120, 150, 180, 200, 200, ...` had a 20-unit step
    /// from the last ramp pixel (`180`) into the flat settle (`200`) →
    /// at the lowered `T_HARD = 10` / `T_SMOOTH = 5` thresholds, that
    /// 20-step landed exactly in the hard-jump predicate (D1=20 > 10,
    /// D2=0 < 5). Replace with a fully-ramping triangle dip (no flat
    /// terminus): the pattern returns to baseline via a smooth ramp on
    /// both sides, so every position has D2 ≈ D1 → smoothness predicate
    /// excludes every pixel.
    #[test]
    fn ignores_self_shadowed_dip() {
        let w = 16u32;
        let h = 16u32;
        let mut data = Vec::with_capacity((w * h) as usize);
        // Triangular dip down-then-up (D1 = 20 every step, D2 changes
        // sign at the trough but |D2| = |D1| = 20 → > T_SMOOTH = 5 →
        // never matches the smoothness predicate). Mirrored so the
        // bottom-edge skip doesn't reach the flat terminus.
        let pattern: [u8; 16] =
            [220, 200, 180, 160, 140, 120, 100, 80, 100, 120, 140, 160, 180, 200, 220, 240];
        for y in 0..h {
            for _x in 0..w {
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
        // First-diffs are 20 units; second-diffs are also 20 units
        // (sustained ramp or symmetric V at the trough) → not < T_SMOOTH
        // (5 units) → not flagged.
        assert_eq!(n, 0, "smooth shadow dip should NOT be flagged; got {n}");
    }

    /// A SYNTHETIC noisy flat rect at the user-reported baseline luma
    /// (mean 94, ~6%V std ≈ 5 units) MUST NOT be flagged AFTER median
    /// pre-filtering. Models the per-pixel GI MC noise in the shadowed
    /// red-cobblestone rect.
    #[test]
    fn median_kills_synthetic_noise() {
        let w = 32u32;
        let h = 32u32;
        // Deterministic pseudo-Gaussian using a simple hash → uniform → 12-tap
        // CLT approximation. Sample mean 94, std ~5.
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0f32;
                let mut s = (x.wrapping_mul(1664525u32))
                    .wrapping_add(y.wrapping_mul(1013904223u32))
                    .wrapping_add(7919u32)
                    .wrapping_mul(2654435761u32);
                for _ in 0..12 {
                    s = s.wrapping_mul(1664525u32).wrapping_add(1013904223u32);
                    acc += ((s >> 24) as f32) / 256.0;
                }
                // 12-tap CLT: mean=6, var=1 → z ~ N(0,1).
                let z = acc - 6.0;
                let v = (94.0 + z * 5.0).clamp(0.0, 255.0) as u8;
                data.push([v, v, v, 255]);
            }
        }
        let fb = Framebuffer::from_raw_rgba(data, w, h);
        let rect = Rect { x0: 0, y0: 0, x1: w, y1: h };
        let luma_raw = rect_to_luma_image(&fb, rect);
        let luma_med = median_filter(&luma_raw, PBR_HARD_EDGE_MEDIAN_RADIUS);
        let n = count_hard_one_pixel_jumps_luma(
            &luma_med,
            PBR_HARD_EDGE_T_HARD,
            PBR_HARD_EDGE_T_SMOOTH,
        );
        assert!(
            n <= 5,
            "post-median noisy flat rect (mean 94, std 5) should produce \
             <= 5 hard jumps; got {n} - median filter not suppressing noise"
        );
    }

    /// A SYNTHETIC low-contrast coherent bump (V=94 baseline, +16 inside
    /// a 30-px patch → V=110 inside — exactly the user-reported 6%V
    /// splotch shape) MUST be flagged by the stone-interior-masked
    /// detector AFTER median pre-filtering. Both sides of the splotch
    /// boundary are stone-interior V (94 and 110, both >> floor), so
    /// the mask admits the boundary; the 30-px boundary produces 100+
    /// flagged pixels.
    #[test]
    fn stone_interior_detects_low_contrast_bump() {
        let w = 64u32;
        let h = 64u32;
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let inside = x >= 17 && x < 47 && y >= 17 && y < 47;
                let v = if inside { 110u8 } else { 94u8 };
                data.push([v, v, v, 255]);
            }
        }
        let fb = Framebuffer::from_raw_rgba(data, w, h);
        let rect = Rect { x0: 0, y0: 0, x1: w, y1: h };
        let v_raw = rect_to_v_image(&fb, rect);
        let v_med_img = median_filter(&v_raw, PBR_HARD_EDGE_MEDIAN_RADIUS);
        let vm = median_v(&v_med_img);
        let stone_floor = (vm as i32 - PBR_HARD_EDGE_V_STONE_FLOOR_BELOW_MEDIAN)
            .clamp(0, 255) as u8;
        let n = count_hard_jumps_stone_interior(
            &v_med_img,
            PBR_HARD_EDGE_T_HARD,
            PBR_HARD_EDGE_T_SMOOTH,
            stone_floor,
        );
        // 30×30 splotch perimeter = ~4 × 30 = 120 1-px steps. The
        // double-direction scan double-counts at corners, so expect 100+
        // distinct flagged pixels.
        assert!(
            n >= 100,
            "post-median coherent 30x30 bump at +16 V (user splotch \
             shape, both sides stone-interior) MUST produce >= 100 \
             stone-interior hard-jumps; got {n} (median V {vm}, floor \
             {stone_floor}, ceil {PBR_HARD_EDGE_MAX_HARD_JUMPS})"
        );
        assert!(
            n > PBR_HARD_EDGE_MAX_HARD_JUMPS,
            "coherent splotch MUST exceed ceiling \
             {PBR_HARD_EDGE_MAX_HARD_JUMPS}; got {n}"
        );
    }

    /// A SYNTHETIC moss-gap-style rect (red baseline V=94 with green
    /// moss V-DROPS to 50 in 2-px-wide vertical strips) MUST PASS the
    /// stone-interior-masked detector — the moss has V *below* the
    /// stone floor (V_med - 35 ≈ 59), so the moss-gap boundaries fail
    /// the mask and the hard-jump predicate is never evaluated on them.
    #[test]
    fn stone_interior_ignores_moss_gaps() {
        let w = 64u32;
        let h = 64u32;
        let mut data = Vec::with_capacity((w * h) as usize);
        for _y in 0..h {
            for x in 0..w {
                let in_moss = (x >= 15 && x <= 16) || (x >= 45 && x <= 46);
                let v = if in_moss { 50u8 } else { 94u8 };
                data.push([v, v, v, 255]);
            }
        }
        let fb = Framebuffer::from_raw_rgba(data, w, h);
        let rect = Rect { x0: 0, y0: 0, x1: w, y1: h };
        let v_raw = rect_to_v_image(&fb, rect);
        let v_med_img = median_filter(&v_raw, PBR_HARD_EDGE_MEDIAN_RADIUS);
        let vm = median_v(&v_med_img);
        let stone_floor = (vm as i32 - PBR_HARD_EDGE_V_STONE_FLOOR_BELOW_MEDIAN)
            .clamp(0, 255) as u8;
        let n = count_hard_jumps_stone_interior(
            &v_med_img,
            PBR_HARD_EDGE_T_HARD,
            PBR_HARD_EDGE_T_SMOOTH,
            stone_floor,
        );
        // Moss V=50, stone floor V=94-35=59 → 50 < 59 → moss pixels
        // fail the mask. Stone-interior pixels are uniform V=94 → no
        // hard jumps. Expect ~0 stone-interior hard-jumps.
        assert!(
            n <= 5,
            "moss-gap rect (moss V below stone floor) MUST produce <= 5 \
             stone-interior hard-jumps; got {n} (median V {vm}, floor \
             {stone_floor})"
        );
    }

    /// A SYNTHETIC noisy flat rect at the user-reported baseline V
    /// (mean 94, std ~5) MUST PASS the stone-interior-masked detector
    /// — median kills isolated noise; residual stone-interior jumps
    /// stay below the ceiling.
    #[test]
    fn stone_interior_ignores_synthetic_noise() {
        let w = 64u32;
        let h = 64u32;
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0f32;
                let mut s = (x.wrapping_mul(1664525u32))
                    .wrapping_add(y.wrapping_mul(1013904223u32))
                    .wrapping_add(7919u32)
                    .wrapping_mul(2654435761u32);
                for _ in 0..12 {
                    s = s.wrapping_mul(1664525u32).wrapping_add(1013904223u32);
                    acc += ((s >> 24) as f32) / 256.0;
                }
                let z = acc - 6.0;
                let v = (94.0 + z * 5.0).clamp(0.0, 255.0) as u8;
                data.push([v, v, v, 255]);
            }
        }
        let fb = Framebuffer::from_raw_rgba(data, w, h);
        let rect = Rect { x0: 0, y0: 0, x1: w, y1: h };
        let v_raw = rect_to_v_image(&fb, rect);
        let v_med_img = median_filter(&v_raw, PBR_HARD_EDGE_MEDIAN_RADIUS);
        let vm = median_v(&v_med_img);
        let stone_floor = (vm as i32 - PBR_HARD_EDGE_V_STONE_FLOOR_BELOW_MEDIAN)
            .clamp(0, 255) as u8;
        let n = count_hard_jumps_stone_interior(
            &v_med_img,
            PBR_HARD_EDGE_T_HARD,
            PBR_HARD_EDGE_T_SMOOTH,
            stone_floor,
        );
        assert!(
            n <= PBR_HARD_EDGE_MAX_HARD_JUMPS,
            "Gaussian-noise rect (mean 94, std 5) MUST stay below ceiling \
             {PBR_HARD_EDGE_MAX_HARD_JUMPS} stone-interior hard-jumps; \
             got {n} (median V {vm}, floor {stone_floor})"
        );
    }
}
