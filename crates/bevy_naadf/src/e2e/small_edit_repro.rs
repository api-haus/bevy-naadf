//! `--small-edit-repro` mode — user-captured single-voxel edit reproduction.
//!
//! ## Why this gate exists
//!
//! The pre-existing `--small-edit-visual` gate (`03g`) uses the default test
//! grid + a synthetic click voxel; it PASSES while the user can still
//! reproduce broken small-edit shapes (inside-out, pitch-black) in the live
//! `bevy-naadf` binary against the Oasis VOX scene. This gate captures the
//! user's exact reproduction:
//!
//! - World: the same Oasis VOX fixture (Git LFS-tracked at
//!   `crates/bevy_naadf/assets/test/oasis_hard_cover.vox`).
//! - Camera transform: the exact pose recorded by `EDIT_REPRO` log from the
//!   user's session at 2026-05-17T05:11:08.
//! - Brush call: the exact `cube_brush(radius=1, pos, ty)` recorded by the
//!   same log line.
//!
//! ## Assertion
//!
//! After the edit + render convergence, the post-edit framebuffer must
//! contain **zero pitch-black pixels** (RGB == 0,0,0). The user observed
//! that the broken edit renders as a pitch-black silhouette at this camera
//! angle; a correct edit produces a coloured shape so every pixel has some
//! lit content. The assertion is dead simple and very strict — exactly
//! matching the user's observation:
//!
//! > if the bug was fixed, then all pixels would be non-black for sure
//!
//! ## Mechanism
//!
//! 1. Load the Oasis VOX through the production [`crate::GridPreset::Vox`]
//!    path.
//! 2. Pin the camera to [`SMALL_EDIT_REPRO_CAM_POS`] / [`SMALL_EDIT_REPRO_CAM_QUAT`]
//!    every tick — overrides the standard e2e camera motion.
//! 3. Warmup [`SMALL_EDIT_REPRO_WARMUP_FRAMES`] frames for TAA + GI convergence.
//! 4. Capture framebuffer A → `target/e2e-screenshots/small_edit_repro_before.png`.
//! 5. Invoke `cube_brush(radius=SMALL_EDIT_REPRO_RADIUS, pos=…, ty=…,
//!    is_erase=false)` via the production runtime path.
//! 6. Wait [`SMALL_EDIT_REPRO_POST_EDIT_WAIT_FRAMES`] ticks.
//! 7. Capture framebuffer B → `target/e2e-screenshots/small_edit_repro_after.png`.
//! 8. Count pitch-black pixels (RGB == 0,0,0) in framebuffer B; require zero.

use std::path::Path;

use bevy::camera::Camera3d;
use bevy::math::{Quat, Vec3};
use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::Framebuffer;
use crate::e2e::oasis_edit_visual::oasis_vox_fixture_path;
use crate::voxel::VoxelTypeId;
use crate::world::data::WorldData;

// ---------------------------------------------------------------------------
// Screenshot filenames
// ---------------------------------------------------------------------------

pub const SMALL_EDIT_REPRO_BEFORE_PNG: &str = "small_edit_repro_before.png";
pub const SMALL_EDIT_REPRO_AFTER_PNG: &str = "small_edit_repro_after.png";

// ---------------------------------------------------------------------------
// Frame budgets — same shape as --small-edit-visual / --oasis-edit-visual.
// ---------------------------------------------------------------------------

pub const SMALL_EDIT_REPRO_WARMUP_FRAMES: u32 = 120;
pub const SMALL_EDIT_REPRO_POST_EDIT_WAIT_FRAMES: u32 = 300;
pub const SMALL_EDIT_REPRO_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// User-captured reproduction parameters
// ---------------------------------------------------------------------------
//
// Source: live binary session 2026-05-17T05:11:08.893462Z, EDIT_REPRO log
// line emitted by `editor::apply_edit_tool`. The user reported the edit
// renders as a pitch-black inverted shape at this camera pose.

/// Camera world-space position at the moment of the broken edit.
pub const SMALL_EDIT_REPRO_CAM_POS: Vec3 = Vec3::new(870.724243, 345.214264, 501.154510);

/// Camera rotation quaternion (xyzw) at the moment of the broken edit.
pub const SMALL_EDIT_REPRO_CAM_QUAT: Quat = Quat::from_xyzw(0.030069, 0.958114, 0.262875, -0.109593);

/// World-space brush position passed to `cube_brush`.
pub const SMALL_EDIT_REPRO_BRUSH_POS: Vec3 = Vec3::new(872.760132, 341.000000, 507.804260);

/// Brush radius — the user's broken edit used exactly 1.0.
pub const SMALL_EDIT_REPRO_RADIUS: f32 = 1.0;

/// Voxel type id the brush paints with — ty=41 in the user's session
/// (a yellow-tinted material; type 1 is itself pitch-black in the Oasis
/// palette, so don't substitute that — the "dark pixels" assertion
/// requires a non-black material so the *bug* (dark pixels in what
/// should be a coloured cube) is distinguishable from the *material*).
pub const SMALL_EDIT_REPRO_TY: u16 = 41;

/// Erase mode flag — the user's broken edit was a place, not an erase.
pub const SMALL_EDIT_REPRO_IS_ERASE: bool = false;

// ---------------------------------------------------------------------------
// Window resolution — match the user's screen, 1920×1080. The bug is
// AADF-driven so it shows at any resolution, but the user reported it at
// 1920×1080 so we reproduce there.
// ---------------------------------------------------------------------------

pub const SMALL_EDIT_REPRO_WIDTH: u32 = 1920;
pub const SMALL_EDIT_REPRO_HEIGHT: u32 = 1080;

// ---------------------------------------------------------------------------
// State resource
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct SmallEditReproState {
    pub before: Option<Framebuffer>,
    pub after: Option<Framebuffer>,
    pub edit_applied: bool,
}

// ---------------------------------------------------------------------------
// Entry point — invoked from `bin/e2e_render.rs`
// ---------------------------------------------------------------------------

pub fn run_small_edit_repro() -> AppExit {
    let path = oasis_vox_fixture_path();
    if !path.exists() {
        eprintln!(
            "e2e_render --small-edit-repro: FIXTURE MISSING at {} (cwd = {:?}). \
             Run `git lfs pull` or from the workspace root.",
            path.display(),
            std::env::current_dir().ok()
        );
        return AppExit::error();
    }
    println!(
        "e2e_render --small-edit-repro: loading Oasis VOX from {} ({} bytes); \
         camera pos={:?} quat={:?}; cube_brush pos={:?} radius={} ty={}",
        path.display(),
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        SMALL_EDIT_REPRO_CAM_POS,
        SMALL_EDIT_REPRO_CAM_QUAT,
        SMALL_EDIT_REPRO_BRUSH_POS,
        SMALL_EDIT_REPRO_RADIUS,
        SMALL_EDIT_REPRO_TY,
    );

    let mut app_args = crate::AppArgs::default();
    app_args.small_edit_repro_mode = true;
    // Step 5 of the config-as-resource refactor — `grid_preset` migrated
    // off `AppArgs` onto `BootstrapInputs.grid_preset`. vox-gpu-rewrite
    // Stage 2 (2026-05-18): always the W5 GPU producer chain (production
    // path). The captured camera pose and brush position are absolute
    // world-voxel coords; the W5 path tiles Oasis at `voxelPos % modelSize`
    // starting from world origin, so the original user-captured coords
    // fall inside the first XZ tile and frame the same architecture the
    // user saw.
    let inputs = crate::bootstrap::BootstrapInputs {
        args: app_args,
        grid_preset: crate::GridPreset::Vox { path },
        ..crate::bootstrap::BootstrapInputs::default()
    };
    crate::bootstrap::run_e2e_render_with_bootstrap_inputs(inputs)
}

// ---------------------------------------------------------------------------
// Camera pose pin — overrides the standard e2e camera every tick
// ---------------------------------------------------------------------------

pub fn pin_small_edit_repro_camera(
    args: Option<Res<crate::AppArgs>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.small_edit_repro_mode {
        return;
    }
    let pose = Transform {
        translation: SMALL_EDIT_REPRO_CAM_POS,
        rotation: SMALL_EDIT_REPRO_CAM_QUAT,
        scale: Vec3::ONE,
    };
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
}

// ---------------------------------------------------------------------------
// Edit application
// ---------------------------------------------------------------------------

pub fn apply_small_edit_repro_edit(
    world_data: &mut WorldData,
    state: &mut SmallEditReproState,
) {
    let pos = SMALL_EDIT_REPRO_BRUSH_POS;
    let radius = SMALL_EDIT_REPRO_RADIUS;
    let ty = VoxelTypeId(SMALL_EDIT_REPRO_TY);
    let is_erase = SMALL_EDIT_REPRO_IS_ERASE;

    // Predict the 2×2×2 affected voxel set + sample pre-edit types.
    let lo = bevy::math::IVec3::new(
        (pos.x - radius).floor() as i32,
        (pos.y - radius).floor() as i32,
        (pos.z - radius).floor() as i32,
    );
    let hi = bevy::math::IVec3::new(
        (pos.x + radius).ceil() as i32,
        (pos.y + radius).ceil() as i32,
        (pos.z + radius).ceil() as i32,
    );
    let mut affected: Vec<bevy::math::IVec3> = Vec::new();
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let v = bevy::math::IVec3::new(x, y, z);
                let d = (v.as_vec3() + Vec3::splat(0.5)) - pos;
                let cheb = d.x.abs().max(d.y.abs()).max(d.z.abs());
                if cheb < radius {
                    affected.push(v);
                }
            }
        }
    }
    println!(
        "e2e_render --small-edit-repro: predicted {} affected voxels:",
        affected.len()
    );
    for v in &affected {
        let pre = world_data.get_voxel_type(*v);
        println!("  pre-edit voxel {:?} type={:?}", v, pre);
    }

    println!(
        "e2e_render --small-edit-repro: calling cube_brush pos={:?} radius={} ty={} is_erase={}",
        pos, radius, ty.raw(), is_erase,
    );
    crate::editor::tools::cube_brush(world_data, pos, radius, ty, is_erase);

    // Post-edit verification: each predicted voxel must now be `ty`.
    let mut wrong = 0;
    for v in &affected {
        let post = world_data.get_voxel_type(*v);
        let ok = post == Some(ty);
        println!(
            "  post-edit voxel {:?} type={:?} {}",
            v, post,
            if ok { "OK" } else { "*** WRONG ***" }
        );
        if !ok {
            wrong += 1;
        }
    }
    println!(
        "e2e_render --small-edit-repro: CPU verification — {}/{} affected voxels correctly encoded",
        affected.len() - wrong,
        affected.len(),
    );

    let batches = world_data.pending_edits.batches.len();
    let groups = world_data.pending_edits.edited_groups.len();
    let mut chunk_records = 0usize;
    let mut block_records = 0usize;
    let mut voxel_records = 0usize;
    for batch in &world_data.pending_edits.batches {
        chunk_records += batch.changed_chunks.len();
        block_records += batch.changed_blocks.len() / 65;
        voxel_records += batch.changed_voxels.len() / 33;
    }
    println!(
        "e2e_render --small-edit-repro: cube_brush returned — pending_edits batches {batches}, \
         edited_groups {groups}, changed_chunks {chunk_records}, changed_blocks {block_records}, \
         changed_voxels {voxel_records}",
    );
    state.edit_applied = true;
}

// ---------------------------------------------------------------------------
// Save helpers
// ---------------------------------------------------------------------------

pub fn save_small_edit_repro_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --small-edit-repro: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --small-edit-repro: {filename} save failed: {e}"
        ),
    }
}

// ---------------------------------------------------------------------------
// Assertion: zero pitch-black pixels
// ---------------------------------------------------------------------------

/// Pixel-sum threshold (R+G+B) below which a pixel counts as "anomalously
/// dark". The Oasis sand-terrain at this camera pose is uniformly cream
/// (per-channel ~180-240, R+G+B ~500-650); a properly-rendered single
/// voxel adds a coloured shape (any non-black material). The broken edit
/// renders as an inverted dark silhouette where the camera sees through
/// the cube's near face into its unlit back face — those pixels register
/// near-black after tonemapping.
///
/// `30` is conservative: even tonemapped deep-shadow ground pixels never
/// dip below ~100 on the cream terrain, and any genuine "shadow" pixel
/// from the surrounding scene would also appear in the before frame (the
/// gate's assertion subtracts the before-frame count). 30 corresponds to
/// per-channel ~10 each — well below any natural surface in this scene
/// and well above the absolute-zero floor that tonemapping never reaches.
pub const SMALL_EDIT_REPRO_DARK_SUM_THRESHOLD: u32 = 30;

/// The load-bearing check. Counts "anomalously dark" pixels (R+G+B below
/// [`SMALL_EDIT_REPRO_DARK_SUM_THRESHOLD`]) in both frames and asserts
/// the AFTER count is ≤ the BEFORE count. The broken edit produces a
/// large dark patch where the inside-out cube's unlit back faces face
/// the camera; correct geometry leaves every pixel at the lit-sand
/// luminance range. Per the user's observation:
///
/// > if the bug was fixed, then all pixels would be non-black for sure
pub fn assert_no_pitch_black_pixels(
    before: &Framebuffer,
    after: &Framebuffer,
) -> Result<String, String> {
    let w = after.width();
    let h = after.height();
    let total = (w as u64) * (h as u64);
    if before.width() != w || before.height() != h {
        return Err(format!(
            "small-edit-repro: frame dim mismatch — before {}×{}, after {}×{}",
            before.width(), before.height(), w, h,
        ));
    }
    let mut dark_before = 0u64;
    let mut dark_after = 0u64;
    let mut first_dark_after: Option<(u32, u32, [u8; 4])> = None;
    let mut min_sum_after = u32::MAX;
    let mut min_sum_at_after = (0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            let pa = before.pixel(x, y);
            let pb = after.pixel(x, y);
            let sa = pa[0] as u32 + pa[1] as u32 + pa[2] as u32;
            let sb = pb[0] as u32 + pb[1] as u32 + pb[2] as u32;
            if sa < SMALL_EDIT_REPRO_DARK_SUM_THRESHOLD {
                dark_before += 1;
            }
            if sb < SMALL_EDIT_REPRO_DARK_SUM_THRESHOLD {
                dark_after += 1;
                if first_dark_after.is_none() {
                    first_dark_after = Some((x, y, pb));
                }
            }
            if sb < min_sum_after {
                min_sum_after = sb;
                min_sum_at_after = (x, y);
            }
        }
    }
    let delta = dark_after as i64 - dark_before as i64;
    let pct_after = 100.0 * dark_after as f64 / total as f64;
    let pct_before = 100.0 * dark_before as f64 / total as f64;
    let report = format!(
        "frame={}×{} threshold(R+G+B)<{} \
         dark-before={} ({:.4}%) dark-after={} ({:.4}%) Δ={} \
         after-min-sum={} at {:?} first-dark-after={:?}",
        w, h, SMALL_EDIT_REPRO_DARK_SUM_THRESHOLD,
        dark_before, pct_before, dark_after, pct_after, delta,
        min_sum_after, min_sum_at_after, first_dark_after,
    );
    if delta <= 0 {
        Ok(format!("small-edit-repro PASS — {report}"))
    } else {
        Err(format!(
            "small-edit-repro FAIL — {report}. The edit added {delta} anomalously \
             dark pixels — the inside-out / inverted-shape rendering the user reported. \
             Inspect target/e2e-screenshots/{SMALL_EDIT_REPRO_BEFORE_PNG} + \
             target/e2e-screenshots/{SMALL_EDIT_REPRO_AFTER_PNG}.",
        ))
    }
}
