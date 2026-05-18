//! `--small-edit-visual` mode — single-voxel edit gate (`03g`,
//! dispatched 2026-05-15).
//!
//! ## Why this gate exists
//!
//! The user reported two regressions after commit `729b604`:
//!
//! - **Mode 1** ("cross-section / missing sides"): a placed 1×1×1 cube
//!   renders correctly from some camera angles but as a cross-section
//!   from others — classic AADF-not-invalidated symptom (ray from one
//!   direction sees the voxel, ray from another AADF-skips past it).
//! - **Mode 2** ("phantoms"): clicking once produces 3 voxels — the
//!   target voxel + 2 phantoms at sibling-half-word positions.
//!
//! The `--oasis-edit-visual` gate (`03f`) catches whole-sphere erase
//! visibility but NOT the small-scale single-voxel case: its tight
//! 30%×30% rect averages over hundreds of changed pixels; a 1-voxel
//! change projects to ~1 framebuffer pixel and gets diluted to
//! sub-noise.
//!
//! ## Mechanism
//!
//! 1. Boot the default test grid (`GridPreset::Default`, 64×32×64
//!    voxels = 4×2×4 chunks). Empty regions exist between the four
//!    corner towers (chunks ~(1, 1, 1) to (2, 1, 2) is clear air).
//! 2. Pin a birdseye camera over world centre.
//! 3. Warm up `OASIS_WARMUP_FRAMES` for TAA + GI convergence.
//! 4. **Snapshot A**: count non-empty voxels in the world via
//!    `count_non_empty_voxels`; capture framebuffer A.
//! 5. **Apply edit**: call `cube_brush(radius=1.0)` at a known empty
//!    voxel coordinate. radius=1 emits exactly one voxel (verified by
//!    the `cube_brush_radius_one_emits_exactly_one_voxel` unit test).
//! 6. **CPU assertion (deterministic, fast)**: count B - count A == 1.
//!    If this fails, Mode 2 is caught BEFORE the framebuffer round-trip.
//! 7. Wait for W2 GPU dispatch + W3 regime-2 background AADF + TAA / GI
//!    re-convergence.
//! 8. **Snapshot B**: capture framebuffer B.
//! 9. **Framebuffer assertion**: target voxel's projected screen rect
//!    shows a measurable change; adjacent rects show no significant
//!    change (catches Mode 1 / phantom-voxel cases).
//!
//! ## What this gate catches that prior gates miss
//!
//! - `--edit-mode` is an in-process oracle (no GPU + no render).
//! - `--runtime-edit-mode` checks record counts (no framebuffer).
//! - `--oasis-edit-visual` averages over a 30% rect (signal lost at
//!   single-voxel scale).
//!
//! `--small-edit-visual` is the **single-voxel** scale; it pairs a CPU
//! pre-condition (Mode 2 catch) with a tight framebuffer post-condition
//! (Mode 1 catch).

use std::path::Path;

use bevy::math::{IVec3, Vec3};
use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::{Framebuffer, Rect};
use crate::voxel::{VoxelTypeId, CELL_DIM};
use crate::world::data::WorldData;

// ---------------------------------------------------------------------------
// Screenshot filenames
// ---------------------------------------------------------------------------

/// PNG saved for the pre-edit capture.
pub const SMALL_EDIT_BEFORE_PNG: &str = "small_edit_before.png";
/// PNG saved for the post-edit capture.
pub const SMALL_EDIT_AFTER_PNG: &str = "small_edit_after.png";

// ---------------------------------------------------------------------------
// Frame budgets
// ---------------------------------------------------------------------------

/// Warmup frames before snapshot A. Same convention as `--oasis-edit-visual`.
pub const SMALL_EDIT_WARMUP_FRAMES: u32 = 120;
/// Frames waited between the edit and snapshot B. 300 frames ~ 5s @ 60 FPS.
/// Covers W2 dispatch + W3 regime-2 1500-round convergence + TAA + GI ring.
pub const SMALL_EDIT_POST_EDIT_WAIT_FRAMES: u32 = 300;
/// Max frames the driver waits for a screenshot to be delivered.
pub const SMALL_EDIT_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// Edit geometry
// ---------------------------------------------------------------------------

/// Brush radius — exactly 1.0 voxel. With pos at a voxel centre this
/// emits exactly one voxel edit (verified by
/// `cube_brush_radius_one_emits_exactly_one_voxel` in
/// `crate::editor::tools::tests`).
pub const SMALL_EDIT_RADIUS: f32 = 1.0;

/// World-space click position — small-world-relative voxel `(32, 29, 32)`.
/// The default-scene demo's local centre at y=29 sits above every fixture
/// (ground slab tops at y=2, tallest tower y=26, BOX_A y=20, BOX_B y=16,
/// tallest emissive y=28 — warm cube ymax=28). The sphere at (30,11,30)
/// r=8 has distance √(4+324+4)≈18.2 from (32,29,32) → outside. Demo
/// ceiling y=31. The position is empty pre-edit and surrounded by empty
/// cells in every direction.
///
/// **vox-gpu-rewrite Stage 2 (2026-05-18):** the demo is now embedded at
/// the centre of the fixed `(4096, 512, 4096)`-voxel world; callers
/// translate this small-world-local coord through
/// [`crate::e2e::gates::demo_origin_v`] to get the world-space click voxel.
///
/// See `voxel/grid.rs::build_default_volume`.
pub const SMALL_EDIT_CLICK_VOXEL: IVec3 = IVec3::new(32, 29, 32);

/// World-space click voxel — [`SMALL_EDIT_CLICK_VOXEL`] translated by the
/// demo origin offset.
pub fn small_edit_click_voxel_world() -> IVec3 {
    use crate::e2e::gates::demo_origin_v;
    let off = demo_origin_v();
    SMALL_EDIT_CLICK_VOXEL + IVec3::new(off.x as i32, off.y as i32, off.z as i32)
}

/// The non-empty type id painted at the click voxel. Matches
/// `TY_EMISSIVE_MAGENTA = VoxelTypeId(12)` — a bright magenta emissive
/// that contrasts strongly against the mostly-white default scene
/// (white-ish sphere, sand wall, neutral towers, warm/cool emissives).
/// Magenta gives the single voxel a colour signature distinct from
/// every nearby fixture, lifting the click-rect delta clearly above the
/// noise floor.
pub const SMALL_EDIT_PAINT_TYPE: VoxelTypeId = VoxelTypeId(12);

// ---------------------------------------------------------------------------
// Framebuffer diff thresholds
// ---------------------------------------------------------------------------

/// Minimum **max** per-pixel RGB-channel delta inside the
/// click-projection rect. A single 1×1×1 voxel projects to a 5-7 pixel
/// patch at the birdseye altitude; the rect averaging dilutes the
/// signal too much for a mean-delta floor at this scale. The max gives
/// a cleaner click/no-click signal — the new voxel is the brightest
/// pixel-change in the rect by far when it lands, vs the TAA / GI
/// re-convergence noise floor outside.
///
/// `15` sits above the deepest TAA convergence noise (~10) and below
/// the swing a clearly-landed voxel produces (~20-200 depending on
/// surface and emission). Calibrated empirically; the magenta emissive
/// at world centre shows ~20 per-pixel swing from below TAA, which is
/// the conservative end. If the renderer's GI integration improves
/// pre-edit warmup, the signal/noise ratio improves and this can be
/// raised.
pub const SMALL_EDIT_CLICK_RECT_FLOOR: f32 = 15.0;

/// Maximum per-pixel mean RGB delta inside an adjacent rect (one rect
/// width away in -X / +X / -Z / +Z). A correct edit only changes pixels
/// at the click location; adjacent rects should drift only by TAA / GI
/// re-convergence noise.
///
/// **Mode 1 catch (local)**: if AADFs got stale and the GPU DDA skips
/// through the new voxel + lands on different geometry from a different
/// view angle, the adjacent rect's pixels will swing too.
///
/// Set generously above pure noise (the `--oasis-edit-visual` noise
/// floor pre-fix was ~4.5 over a much larger rect; for a 5×5 rect with
/// less averaging the noise can spike higher). Tuned empirically to
/// catch a clear regression while tolerating GI bounce settling.
pub const SMALL_EDIT_ADJ_RECT_CEILING: f32 = 50.0;

/// Per-pixel RGB-sum threshold for "catastrophic" change (used only
/// for diagnostic logging, not asserted — see below).
pub const SMALL_EDIT_CATASTROPHIC_DELTA_THRESHOLD: u32 = 200;

/// Maximum allowed fraction of pixels OUTSIDE the click rect with a
/// catastrophic (>`SMALL_EDIT_CATASTROPHIC_DELTA_THRESHOLD`) per-pixel
/// RGB-sum change. The remaining frame should be mostly stable — even
/// allowing GI re-convergence noise. The default scene's emissive +
/// diffuse mix produces some GI bounce shift after any edit; we
/// tolerate up to ~10% of the frame changing modestly.
///
/// **Mode 1 catch (scene-scale)**: A single-voxel edit causing
/// global rendering corruption produces 30-60%+ catastrophic pixels in
/// pre-fix reproduction runs. The 15% ceiling sits comfortably above
/// the GI-only re-convergence floor (~5-8% in empirical reproduction)
/// and well below the bug signal.
pub const SMALL_EDIT_CATASTROPHIC_FRACTION_CEILING: f32 = 0.15;

// ---------------------------------------------------------------------------
// State resource
// ---------------------------------------------------------------------------

/// Stash for the two captured framebuffers + CPU snapshot counts +
/// edit-applied flag.
#[derive(Resource, Default)]
pub struct SmallEditVisualState {
    /// Pre-edit framebuffer.
    pub before: Option<Framebuffer>,
    /// Post-edit framebuffer.
    pub after: Option<Framebuffer>,
    /// Pre-edit count of non-empty voxels (resolved via the chunks/blocks/
    /// voxels three-layer descent).
    pub voxel_count_before: Option<u64>,
    /// Post-edit count of non-empty voxels.
    pub voxel_count_after: Option<u64>,
    /// Cached size_in_voxels at the time the brush fired.
    pub world_size_voxels: Option<[u32; 3]>,
    /// Edit-fired flag — driver fires the brush exactly once.
    pub edit_applied: bool,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Boot the e2e harness with `--small-edit-visual` mode active.
pub fn run_small_edit_visual() -> AppExit {
    let mut app_args = crate::AppArgs::default();
    app_args.small_edit_visual_mode = true;
    println!(
        "e2e_render --small-edit-visual: target click voxel {:?}, brush radius {SMALL_EDIT_RADIUS}, \
         paint type {:?}",
        SMALL_EDIT_CLICK_VOXEL, SMALL_EDIT_PAINT_TYPE
    );
    crate::run_e2e_render_with_args(app_args)
}

// ---------------------------------------------------------------------------
// Camera pose pin
// ---------------------------------------------------------------------------

/// Birdseye over the test grid centered on the click voxel.
///
/// **vox-gpu-rewrite Stage 2 (2026-05-18):** the camera is pinned to the
/// **demo's top + 50** rather than the full fixed-world top + 50 — the
/// demo is 64×32×64 voxels embedded in the centre of the
/// `(4096, 512, 4096)`-voxel container; framing the projected 1-voxel
/// edit against the full-world top would put the camera ~530 voxels above
/// the click and dilute the projection to sub-pixel. The demo-relative
/// altitude keeps the projection at the same screen size as the legacy
/// small-world layout.
///
/// Camera at `(click.x, demo_top+50, click.z)` looking straight down at
/// the click voxel centre. With +X as the up-reference vector,
/// screen-space +Y maps to world +X and screen-space +X maps to world +Z
/// (the standard look_at with `up=+X` pinning). The click voxel projects
/// to the framebuffer centre.
pub fn birdseye_pose() -> Transform {
    let click_world = small_edit_click_voxel_world();
    let cx = click_world.x as f32 + 0.5;
    let cz = click_world.z as f32 + 0.5;
    let cy = click_world.y as f32 + 0.5;
    // Demo Y extent is `crate::voxel::grid::DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS[1] * 16`
    // = 32 voxels; the demo embed sits at Y=0..32 inside the fixed world.
    let demo_top_y = crate::voxel::grid::DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS[1] as f32 * 16.0;
    let cam_y = demo_top_y + 50.0;
    Transform::from_xyz(cx, cam_y, cz).looking_at(Vec3::new(cx, cy, cz), Vec3::X)
}

/// Re-uses the `oasis_edit_visual::pin_oasis_camera` pattern: every tick,
/// override the camera pose. Wired only when
/// `AppArgs.small_edit_visual_mode == true`.
pub fn pin_small_edit_camera(
    args: Option<Res<crate::AppArgs>>,
    world_data: Option<Res<WorldData>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.small_edit_visual_mode {
        return;
    }
    let Some(world_data) = world_data else { return; };
    let size_v = world_data.size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
    if size_v.x == 0 || size_v.y == 0 || size_v.z == 0 {
        return;
    }
    let pose = birdseye_pose();
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
}

// ---------------------------------------------------------------------------
// CPU snapshot helper
// ---------------------------------------------------------------------------

/// Count non-empty voxels in the demo embed region of `WorldData` by
/// walking the chunks/blocks/voxels CPU mirror. Avoids touching the GPU
/// side. O(demo voxels) — fixed cost regardless of world container size.
///
/// **vox-gpu-rewrite Stage 2 (2026-05-18):** scoped to the demo embed
/// (~131k iterations) rather than the full fixed `(4096, 512, 4096)`-voxel
/// world (~8.5G iterations — multi-second per call). The +1 edit lands
/// inside the demo embed so the scoped count is the load-bearing signal.
///
/// **Mode 2 detection mechanism**: A correct single-voxel edit increments
/// this count by exactly 1; a phantom-emitting encoder would increment it
/// by 3 (the user's report) or some other off-by-N value.
pub fn count_non_empty_voxels(world_data: &WorldData) -> u64 {
    use crate::e2e::gates::demo_origin_v;
    use crate::voxel::grid::DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS;
    let off = demo_origin_v();
    let off_x = off.x as i32;
    let off_z = off.z as i32;
    let demo_sx = (DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS[0] * 16) as i32;
    let demo_sy = (DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS[1] * 16) as i32;
    let demo_sz = (DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS[2] * 16) as i32;
    let mut count = 0u64;
    for z in 0..demo_sz {
        for y in 0..demo_sy {
            for x in 0..demo_sx {
                let p = IVec3::new(off_x + x, y, off_z + z);
                if let Some(t) = world_data.get_voxel_type(p) {
                    if t != VoxelTypeId::EMPTY {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Edit application
// ---------------------------------------------------------------------------

/// Apply the small cube brush at the configured click voxel via the
/// production runtime path (`crate::editor::tools::cube_brush`). Captures
/// pre/post non-empty voxel counts into the state resource.
pub fn apply_small_cube_edit(
    world_data: &mut WorldData,
    state: &mut SmallEditVisualState,
) {
    let size_v = world_data.size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
    state.world_size_voxels = Some([size_v.x, size_v.y, size_v.z]);

    let count_before = count_non_empty_voxels(world_data);
    state.voxel_count_before = Some(count_before);

    // vox-gpu-rewrite Stage 2: click voxel is the demo-relative coord
    // translated through `demo_origin_v` to its world-space location.
    let click = small_edit_click_voxel_world();
    let pre_type = world_data.get_voxel_type(click);
    println!(
        "e2e_render --small-edit-visual: pre-edit voxel at {:?} has type {:?}, \
         non-empty voxel count = {}",
        click, pre_type, count_before
    );

    // Voxel-centre world coords — `cube_brush` tests `(voxel + 0.5) - pos`.
    let pos = click.as_vec3() + Vec3::splat(0.5);
    let radius = SMALL_EDIT_RADIUS;
    let ty = SMALL_EDIT_PAINT_TYPE;
    let is_erase = false;

    crate::editor::tools::cube_brush(world_data, pos, radius, ty, is_erase);

    let count_after = count_non_empty_voxels(world_data);
    state.voxel_count_after = Some(count_after);

    let mut chunk_records = 0usize;
    let mut block_records = 0usize;
    let mut voxel_records = 0usize;
    let batches = world_data.pending_edits.batches.len();
    let groups = world_data.pending_edits.edited_groups.len();
    for batch in &world_data.pending_edits.batches {
        chunk_records += batch.changed_chunks.len();
        block_records += batch.changed_blocks.len() / 65;
        voxel_records += batch.changed_voxels.len() / 33;
    }
    let post_type = world_data.get_voxel_type(click);
    println!(
        "e2e_render --small-edit-visual: cube_brush returned — voxels {}→{} (Δ={}), \
         click voxel {:?} now {:?}; pending_edits batches {batches}, edited_groups {groups}, \
         changed_chunks {chunk_records}, changed_blocks {block_records}, changed_voxels {voxel_records}",
        count_before,
        count_after,
        (count_after as i64 - count_before as i64),
        click,
        post_type,
    );

    state.edit_applied = true;
}

// ---------------------------------------------------------------------------
// Save helpers
// ---------------------------------------------------------------------------

/// Save a framebuffer under `target/e2e-screenshots/<filename>`.
pub fn save_small_edit_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --small-edit-visual: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --small-edit-visual: {filename} save failed: {e}"
        ),
    }
}

// ---------------------------------------------------------------------------
// Assertion
// ---------------------------------------------------------------------------

/// Compute the bounding-rect index on the framebuffer corresponding to
/// the world-space click voxel. Uses the same birdseye-projection math
/// as [`birdseye_pose`].
///
/// Returns `((click_rect, neg_x_rect, pos_x_rect, neg_z_rect, pos_z_rect))`.
/// Each rect is centred at the projected pixel + a small margin; the
/// adjacent rects are offset by a full rect width along their axis.
pub fn click_voxel_rects(
    fb_width: u32,
    fb_height: u32,
    world_size_voxels: [u32; 3],
    click: IVec3,
) -> (Rect, Rect, Rect, Rect, Rect) {
    // Birdseye projection (looking down -Y from above world centre with
    // +X as up reference). World coords -> screen coords:
    //   screen-x = (world.z - cz) / (world.z_extent / 2) * (fb_w / 2) + fb_w / 2
    //   screen-y = (world.x - cx) / (world.x_extent / 2) * (fb_h / 2) + fb_h / 2
    // BUT the actual projection depends on the camera FOV and the
    // birdseye altitude. Rather than back-compute the projection,
    // approximate: the world's full XZ extent maps to a roughly central
    // 50% of the framebuffer (camera is 100 voxels above a 32-voxel-tall
    // world looking down). The voxel `(32, 16, 32)` projects to
    // approximately the framebuffer centre.
    //
    // Tolerant of camera framing: use a 9×9 rect centred near the FB
    // centre as the click rect. With FOV ~60° at 100 voxels altitude
    // viewing a 64-voxel-wide world, the projected world maps to roughly
    // 40-60% of the FB width — the centre is the safe choice.
    let cx_screen = fb_width as i32 / 2;
    let cy_screen = fb_height as i32 / 2;
    let half = 8i32; // 17×17 rect — generous enough to absorb projection
                    // slop from the `looking_at(target, up=+X)` matrix
                    // without needing exact pixel-level back-solving.
    let mk_rect = |cx: i32, cy: i32| {
        let x0 = (cx - half).max(0) as u32;
        let y0 = (cy - half).max(0) as u32;
        let x1 = ((cx + half + 1) as u32).min(fb_width);
        let y1 = ((cy + half + 1) as u32).min(fb_height);
        Rect { x0, y0, x1, y1 }
    };
    let click_rect = mk_rect(cx_screen, cy_screen);
    // Adjacent rects offset by 32 pixels along each axis (2x the half-
    // width plus a small gap so they don't overlap the click rect). 32
    // pixels at 256² fb maps to roughly ~5-7 world voxels at the
    // birdseye altitude — far beyond any single-voxel edit's
    // projection.
    let off = 32i32;
    let neg_x = mk_rect(cx_screen - off, cy_screen);
    let pos_x = mk_rect(cx_screen + off, cy_screen);
    let neg_z = mk_rect(cx_screen, cy_screen - off);
    let pos_z = mk_rect(cx_screen, cy_screen + off);
    let _ = world_size_voxels;
    let _ = click;
    (click_rect, neg_x, pos_x, neg_z, pos_z)
}

/// Per-pixel mean RGB delta over a rect (averaged across R+G+B channels).
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

/// Max per-pixel R+G+B-summed-channel delta inside the rect — used for
/// the click-rect signal where a single voxel's projection covers a
/// small patch and rect-averaging dilutes it past the noise floor.
fn region_max_pixel_delta(a: &Framebuffer, b: &Framebuffer, rect: Rect) -> f32 {
    if a.width() != b.width() || a.height() != b.height() {
        return f32::MAX;
    }
    let mut max_d = 0u32;
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let pa = a.pixel(x, y);
            let pb = b.pixel(x, y);
            let d: u32 = (0..3)
                .map(|c| (pa[c] as i32 - pb[c] as i32).unsigned_abs())
                .sum();
            if d > max_d {
                max_d = d;
            }
        }
    }
    max_d as f32
}

/// Verdict: PASS / FAIL with diagnostic message.
pub fn assert_small_edit_landed(
    before: &Framebuffer,
    after: &Framebuffer,
    voxel_count_before: u64,
    voxel_count_after: u64,
    world_size_voxels: [u32; 3],
) -> Result<String, String> {
    // ── CPU assertion (Mode 2 catch) ──
    //
    // A correct radius=1 cube_brush emits exactly one voxel edit; the
    // non-empty voxel count must rise by exactly 1. If it rose by 2 or 3,
    // the encoder is phantom-emitting (e.g., the packed-2-per-u32 logic
    // writes both halves when only one was intended).
    if voxel_count_after != voxel_count_before + 1 {
        return Err(format!(
            "small-edit-visual gate FAIL — Mode 2 detected (phantom voxels). \
             Single-voxel cube_brush (radius=1.0) at {:?} (demo-relative \
             {:?}) should produce +1 non-empty voxel; CPU snapshot shows count \
             {voxel_count_before} → {voxel_count_after} (Δ={}). \
             Most likely sites: \
             - `aadf::cell::pack_voxels` / `unpack_voxel` (the 2-voxels-per-u32 \
             packing logic), \
             - `aadf::edit::set_voxel_in_window` (the half-word selection in \
             `cur` / `lo` / `hi` mixing), \
             - `aadf::edit::process_edit_batch` (the block-uniformity check \
             might be mis-classifying empty-with-flag voxels). \
             See `03g-impl-small-edit-fix.md` `## Root cause(s)` for diagnosis.",
            small_edit_click_voxel_world(),
            SMALL_EDIT_CLICK_VOXEL,
            (voxel_count_after as i64 - voxel_count_before as i64),
        ));
    }

    // ── Framebuffer assertions (Mode 1 catch) ──
    if before.width() != after.width() || before.height() != after.height() {
        return Err(format!(
            "frame A {}×{} vs frame B {}×{} — dimensions changed mid-run",
            before.width(),
            before.height(),
            after.width(),
            after.height(),
        ));
    }
    let (click_rect, neg_x, pos_x, neg_z, pos_z) = click_voxel_rects(
        before.width(),
        before.height(),
        world_size_voxels,
        small_edit_click_voxel_world(),
    );

    // Max delta in the click rect — catches "the voxel landed" with
    // single-pixel resolution (cleaner than mean for small projections).
    let click_max = region_max_pixel_delta(before, after, click_rect);
    let click_mean = region_mean_pixel_delta(before, after, click_rect);
    // Mean deltas in adjacent rects — should stay near noise floor.
    let neg_x_delta = region_mean_pixel_delta(before, after, neg_x);
    let pos_x_delta = region_mean_pixel_delta(before, after, pos_x);
    let neg_z_delta = region_mean_pixel_delta(before, after, neg_z);
    let pos_z_delta = region_mean_pixel_delta(before, after, pos_z);

    // Whole-frame catastrophic-pixel count (Mode 1 at scene scale):
    // count pixels OUTSIDE the click rect with per-pixel RGB-sum delta
    // exceeding `SMALL_EDIT_CATASTROPHIC_DELTA_THRESHOLD`. GI re-
    // convergence after the edit produces some bounce-light shift
    // across the frame; that's accepted up to
    // `SMALL_EDIT_CATASTROPHIC_FRACTION_CEILING`. A broken AADF chain
    // produces 30-60% + catastrophic pixels.
    let mut catastrophic = 0u64;
    let mut outside_total = 0u64;
    let mut max_outside = 0u32;
    let mut max_outside_pos = (0u32, 0u32);
    for y in 0..before.height() {
        for x in 0..before.width() {
            // Skip the click rect (the legitimate change region).
            if x >= click_rect.x0 && x < click_rect.x1
                && y >= click_rect.y0 && y < click_rect.y1
            {
                continue;
            }
            outside_total += 1;
            let pa = before.pixel(x, y);
            let pb = after.pixel(x, y);
            let d: u32 = (0..3)
                .map(|c| (pa[c] as i32 - pb[c] as i32).unsigned_abs())
                .sum();
            if d > SMALL_EDIT_CATASTROPHIC_DELTA_THRESHOLD {
                catastrophic += 1;
            }
            if d > max_outside {
                max_outside = d;
                max_outside_pos = (x, y);
            }
        }
    }
    let catastrophic_frac = if outside_total > 0 {
        catastrophic as f32 / outside_total as f32
    } else {
        0.0
    };

    let report = format!(
        "click rect=({},{},{},{}) max-Δ={click_max:.0} (floor={SMALL_EDIT_CLICK_RECT_FLOOR}) \
         mean-Δ={click_mean:.2}; \
         adj rects -x Δ={neg_x_delta:.2} +x Δ={pos_x_delta:.2} -z Δ={neg_z_delta:.2} +z Δ={pos_z_delta:.2} \
         (ceiling={SMALL_EDIT_ADJ_RECT_CEILING}); \
         catastrophic outside-click pixels={catastrophic}/{outside_total} ({:.1}%, \
         ceiling={:.1}%), max-outside-Δ={max_outside} at {max_outside_pos:?}; \
         CPU non-empty Δ={} (expected +1)",
        click_rect.x0, click_rect.y0, click_rect.x1, click_rect.y1,
        catastrophic_frac * 100.0,
        SMALL_EDIT_CATASTROPHIC_FRACTION_CEILING * 100.0,
        (voxel_count_after as i64 - voxel_count_before as i64),
    );

    println!("e2e_render --small-edit-visual: {report}");

    if click_max < SMALL_EDIT_CLICK_RECT_FLOOR {
        return Err(format!(
            "small-edit-visual gate FAIL — click rect max per-pixel RGB delta \
             {click_max:.0} is below the floor {SMALL_EDIT_CLICK_RECT_FLOOR}. \
             The edited voxel did NOT visibly land in the framebuffer at its \
             projected screen rect. {report}. Inspect \
             target/e2e-screenshots/{SMALL_EDIT_BEFORE_PNG} + \
             target/e2e-screenshots/{SMALL_EDIT_AFTER_PNG}."
        ));
    }

    // Mode 1 catch — adjacent rect signal:
    for (name, delta) in [
        ("-x", neg_x_delta),
        ("+x", pos_x_delta),
        ("-z", neg_z_delta),
        ("+z", pos_z_delta),
    ] {
        if delta > SMALL_EDIT_ADJ_RECT_CEILING {
            return Err(format!(
                "small-edit-visual gate FAIL — Mode 1 detected (cross-section / \
                 AADF-skip from this camera angle). Adjacent rect {name} mean \
                 per-pixel RGB delta {delta:.2} exceeds ceiling \
                 {SMALL_EDIT_ADJ_RECT_CEILING}. The edited voxel's neighbourhood \
                 changed — suggesting stale AADFs are causing the GPU DDA to \
                 render phantom geometry. {report}. Inspect \
                 target/e2e-screenshots/{SMALL_EDIT_BEFORE_PNG} + \
                 target/e2e-screenshots/{SMALL_EDIT_AFTER_PNG}."
            ));
        }
    }

    // Mode 1 catch — whole-frame catastrophic-pixel fraction:
    if catastrophic_frac > SMALL_EDIT_CATASTROPHIC_FRACTION_CEILING {
        return Err(format!(
            "small-edit-visual gate FAIL — Mode 1 detected (whole-frame rendering \
             corruption). {:.1}% of pixels OUTSIDE the click rect changed \
             catastrophically (per-pixel RGB-sum delta > {}), ceiling is {:.1}%. \
             A correct single-voxel edit only changes pixels at the click \
             projection + the natural GI bounce-light re-convergence; a \
             catastrophic fraction this high means the edit corrupted AADFs \
             across large regions of the world, triggered phantom geometry, or \
             broke the renderer's per-frame state. Most likely sites: \
             - W2 `apply_block_change` / `apply_voxel_change` writing into \
             out-of-bounds buffer slots (silent wgpu drop or write-into-wrong-\
             slot); check `W2_CHANGED_*_INIT` caps. \
             - The bind-group / extract layer's `pending_edits.batches` drain \
             leaking into other dispatches. \
             - Chunk-layer AADFs broken by the edit (other chunks' AADFs point \
             through the edited voxel; the W3 regime-2 self-perpetuating queue \
             didn't converge them in `SMALL_EDIT_POST_EDIT_WAIT_FRAMES` frames). \
             {report}. Inspect target/e2e-screenshots/{SMALL_EDIT_BEFORE_PNG} + \
             target/e2e-screenshots/{SMALL_EDIT_AFTER_PNG}.",
            catastrophic_frac * 100.0,
            SMALL_EDIT_CATASTROPHIC_DELTA_THRESHOLD,
            SMALL_EDIT_CATASTROPHIC_FRACTION_CEILING * 100.0,
        ));
    }

    Ok(format!("small-edit-visual gate PASS — {report}"))
}
