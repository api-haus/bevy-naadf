//! BRP-driven e2e gate — `oasis_edit_visual`, migrated from the legacy in-app
//! `e2e_render --oasis-edit-visual` driver mode (`e2e-ipc-rpc-restructure`
//! Phase 2).
//!
//! ## What this gate proves
//!
//! The load-bearing end-to-end edit-pipeline check (`e2e/oasis_edit_visual.rs`
//! module doc): load Oasis, pin a birdseye camera over the world centre, warm
//! up, capture frame A, apply an erase-sphere at the world centre through the
//! **production runtime brush path**, wait for the W2→GPU dispatch to
//! propagate, capture frame B, and assert the per-pixel mean RGB delta over a
//! central rect exceeds the floor. A regression that leaves the producer side
//! intact but never reaches the framebuffer (the `d43f1f1` / `81171f9` classes)
//! drops the delta to noise and fails this gate.
//!
//! ## Migration fidelity (Phase 2 brief — binding)
//!
//! Every numeric constant below is ported **verbatim** from the legacy gate
//! (`e2e/oasis_edit_visual.rs`): the diff floor, the diff rect fractions, the
//! warmup / post-edit frame budgets, the erase radius, and the birdseye camera
//! pose math (`birdseye_pose` / `world_centre_voxel`). The migrated gate must
//! pass the **legacy threshold** — if it fails, that is a migration-fidelity
//! bug (the BRP path is not reproducing the gate), not a reason to recalibrate
//! the threshold.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy_naadf --features e2e-brp --test oasis_edit_visual
//! ```
//!
//! The `--features e2e-brp` makes `CARGO_BIN_EXE_bevy-naadf` an `e2e-brp`-built
//! binary — `Sut::spawn` drives that as the system-under-test.

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::e2e::framebuffer::{Framebuffer, Rect};

// ---------------------------------------------------------------------------
// Constants — ported VERBATIM from `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs`
// ---------------------------------------------------------------------------

/// `OASIS_WARMUP_FRAMES` — warmup before capture A (TAA + GI convergence).
const OASIS_WARMUP_FRAMES: u32 = 120;
/// `OASIS_POST_EDIT_WAIT_FRAMES` — wait between the brush and capture B.
const OASIS_POST_EDIT_WAIT_FRAMES: u32 = 300;
/// `OASIS_ERASE_RADIUS` — erase-sphere radius in voxels.
const OASIS_ERASE_RADIUS: f32 = 30.0;
/// `OASIS_DIFF_RECT_FRACS` — central 30%×30% diff rect.
const OASIS_DIFF_RECT_FRACS: (f32, f32, f32, f32) = (0.35, 0.35, 0.65, 0.65);
/// `OASIS_EDIT_DIFF_FLOOR` — minimum mean per-pixel RGB delta to PASS.
const OASIS_EDIT_DIFF_FLOOR: f32 = 8.0;

/// The bundled Oasis VOX fixture, relative to the SUT's CWD (the `bevy_naadf`
/// crate root — see `Sut::spawn`). The legacy gate's `OASIS_VOX_FIXTURE_PATH`
/// is the workspace-relative `crates/bevy_naadf/assets/test/oasis_hard_cover.vox`;
/// with the SUT CWD at the crate root the crate-relative form resolves.
const OASIS_VOX_FIXTURE: &str = "assets/test/oasis_hard_cover.vox";

// ---------------------------------------------------------------------------
// Camera-pose geometry — ported VERBATIM from `e2e/oasis_edit_visual.rs`
// `birdseye_pose` + `world_centre_voxel`.
// ---------------------------------------------------------------------------

/// World-centre voxel coord — the brush position. Y is mid-height. Verbatim
/// port of `oasis_edit_visual::world_centre_voxel`.
fn world_centre_voxel(world_size_voxels: [u32; 3]) -> [f32; 3] {
    [
        world_size_voxels[0] as f32 * 0.5,
        world_size_voxels[1] as f32 * 0.5,
        world_size_voxels[2] as f32 * 0.5,
    ]
}

/// Birdseye camera pose over the world centre. Camera sits at
/// `(cx, world_top + 250, cz)` looking down at `(cx, mid_y, cz)`; `+X` is the
/// look-at "up" reference. Verbatim port of `oasis_edit_visual::birdseye_pose`,
/// expressed as the `(translation, look_at, up)` triple `naadf/set_camera`
/// takes.
fn birdseye_pose(world_size_voxels: [u32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let cx = world_size_voxels[0] as f32 * 0.5;
    let cz = world_size_voxels[2] as f32 * 0.5;
    let mid_y = world_size_voxels[1] as f32 * 0.5;
    let cam_y = world_size_voxels[1] as f32 + 250.0;
    (
        [cx, cam_y, cz],
        [cx, mid_y, cz],
        [1.0, 0.0, 0.0], // +X up — matches `Transform::looking_at(.., Vec3::X)`.
    )
}

// ---------------------------------------------------------------------------
// Assertion — ported VERBATIM from `e2e/oasis_edit_visual.rs`
// `region_mean_pixel_delta` (a private fn there).
// ---------------------------------------------------------------------------

/// Mean per-pixel RGB delta over a rect (channels averaged 0..3),
/// `0.0..=255.0`. Verbatim port of the private
/// `oasis_edit_visual::region_mean_pixel_delta`.
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

#[test]
fn oasis_edit_visual() {
    // 1. Spawn the production binary as the SUT, Oasis fixture preloaded via
    //    the `--vox` spawn contract (design §3.1, §5). `CARGO_BIN_EXE_bevy-naadf`
    //    is set by Cargo for this same-package integration test; with
    //    `--features e2e-brp` it points at the `e2e-brp`-built binary.
    //    `CARGO_MANIFEST_DIR` is the `bevy_naadf` crate root — the SUT CWD so
    //    `AssetPlugin`'s `src/assets` shaders resolve (Phase 0 forward-note).
    let mut sut = Sut::spawn(
        SutOpts::new(
            env!("CARGO_BIN_EXE_bevy-naadf"),
            env!("CARGO_MANIFEST_DIR"),
        )
        .vox(OASIS_VOX_FIXTURE)
        .window(256, 256),
    );

    // 2. Read the world size — needed for the camera pose + brush position.
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "oasis-edit-visual: SUT reports world_loaded=false — the Oasis VOX \
         load failed (check the SUT stderr for the fixture path)"
    );
    let world_size = state
        .world_size_voxels
        .expect("world_loaded but world_size_voxels is None");

    // 3. Birdseye camera over the world centre (replaces `pin_oasis_camera`).
    let (cam_translation, cam_look_at, cam_up) = birdseye_pose(world_size);
    scenario::set_camera(sut.client(), cam_translation, cam_look_at, Some(cam_up))
        .expect("naadf/set_camera");

    // 4. Warm up — the OASIS_WARMUP_FRAMES budget (TAA + GI convergence).
    scenario::advance(sut.client(), OASIS_WARMUP_FRAMES).expect("warmup advance");

    // 5. Capture frame A.
    let before = scenario::capture(sut.client()).expect("capture A");

    // 6. Erase-sphere at the world centre — the load-bearing runtime edit
    //    path. `naadf/apply_brush` calls `editor::tools::sphere_brush` with
    //    `erase: true`, exactly as the legacy `apply_erase_brush` does.
    let centre = world_centre_voxel(world_size);
    let brush = scenario::erase_sphere(sut.client(), centre, OASIS_ERASE_RADIUS)
        .expect("naadf/apply_brush");
    println!(
        "oasis-edit-visual: erase sphere centre {centre:?} r={OASIS_ERASE_RADIUS} \
         — voxels_delta {} blocks_delta {} batches {}",
        brush.voxels_delta, brush.blocks_delta, brush.batches
    );

    // 7. Wait for the W2→GPU dispatch to propagate — OASIS_POST_EDIT_WAIT_FRAMES.
    scenario::advance(sut.client(), OASIS_POST_EDIT_WAIT_FRAMES).expect("post-edit advance");

    // 8. Capture frame B.
    let after = scenario::capture(sut.client()).expect("capture B");

    // 9. Save both framebuffers to disk (design §7.3) — the SUT CWD is the
    //    crate root, so `target/e2e-screenshots/` lands at
    //    `crates/bevy_naadf/target/...`; the test process CWD is the same.
    let _ = before.save_png("target/e2e-screenshots/oasis_edit_before.png");
    let _ = after.save_png("target/e2e-screenshots/oasis_edit_after.png");
    println!(
        "oasis-edit-visual: saved before/after to \
         crates/bevy_naadf/target/e2e-screenshots/oasis_edit_{{before,after}}.png"
    );

    // 10. Assert — reuse the library's pure assertion math. Dimensions must
    //     agree (a mid-run size change is a hard failure).
    assert_eq!(
        (before.width(), before.height()),
        (after.width(), after.height()),
        "oasis-edit-visual: frame A/B dimensions diverged mid-run"
    );
    let (fx0, fy0, fx1, fy1) = OASIS_DIFF_RECT_FRACS;
    let rect = Rect::from_fractional(&after, fx0, fy0, fx1, fy1);
    let delta = region_mean_pixel_delta(&before, &after, rect);
    let full_delta = before.mean_pixel_delta(&after);
    let mean_before = before.region_mean(rect);
    let mean_after = after.region_mean(rect);
    println!(
        "oasis-edit-visual: rect=({},{},{},{}) rect mean rgba before={mean_before:?} \
         after={mean_after:?}; rect mean per-pixel RGB Δ={delta:.2} \
         (floor {OASIS_EDIT_DIFF_FLOOR:.2}); full-frame Δ={full_delta:.2}",
        rect.x0, rect.y0, rect.x1, rect.y1,
    );
    assert!(
        delta >= OASIS_EDIT_DIFF_FLOOR,
        "oasis-edit-visual gate FAIL — rect mean per-pixel RGB delta {delta:.2} \
         is below the floor {OASIS_EDIT_DIFF_FLOOR:.2}. The erase sphere did NOT \
         visibly land in the framebuffer (the `02f-followup` regression class). \
         Inspect target/e2e-screenshots/oasis_edit_{{before,after}}.png."
    );

    // 11. Pipeline-error scan — the render-world verb.
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");

    println!("oasis-edit-visual: PASS — rect mean per-pixel RGB Δ {delta:.2} >= floor {OASIS_EDIT_DIFF_FLOOR:.2}");
}
