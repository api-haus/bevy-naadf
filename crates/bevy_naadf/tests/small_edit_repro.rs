//! BRP-driven e2e gate — `small_edit_repro`, migrated from the legacy in-app
//! `e2e_render --small-edit-repro` driver mode
//! (`e2e::small_edit_repro::run_small_edit_repro`)
//! (`e2e-ipc-rpc-restructure` Phase 3a).
//!
//! ## What this gate proves
//!
//! The user-captured single-voxel edit reproduction (`e2e/small_edit_repro.rs`
//! module doc): load the Oasis VOX, pin the camera to the EXACT pose recorded
//! in the user's broken-edit session, warm up, capture frame A, apply the EXACT
//! `cube_brush(radius=1)` from the same log line, wait, capture frame B, and
//! assert frame B adds **zero anomalously dark pixels** over frame A — the
//! inside-out / inverted-shape rendering the user observed.
//!
//! ## Migration fidelity (Phase 3a brief — binding)
//!
//! All `SMALL_EDIT_REPRO_*` constants (camera pose, brush params, frame
//! budgets, the 1920×1080 window, the dark-pixel threshold) and the assertion
//! `assert_no_pitch_black_pixels` are reused from the library module
//! **verbatim**.
//!
//! ## Reconstructing the raw-quaternion camera pose via `naadf/set_camera`
//!
//! The legacy `pin_small_edit_repro_camera` writes `Transform { translation,
//! rotation: SMALL_EDIT_REPRO_CAM_QUAT, .. }` — a *raw quaternion*. The
//! `naadf/set_camera` verb rebuilds `Transform::from_translation(t).looking_at(
//! look_at, up)`. Feeding it `look_at = pos + quat·(−Z)` (the camera forward)
//! and `up = quat·(+Y)` reproduces the rotation **exactly**: `quat` is a unit
//! rotation, so its forward + up basis vectors are already orthonormal and
//! `looking_at`'s re-orthonormalisation is a no-op — the reconstructed
//! `Transform` is bit-for-bit the same orientation.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test small_edit_repro
//! ```

use bevy::math::Vec3;

use naadf_e2e::{scenario, schema, Sut, SutOpts};

use bevy_naadf::e2e::small_edit_repro::{
    assert_no_pitch_black_pixels, SMALL_EDIT_REPRO_BRUSH_POS, SMALL_EDIT_REPRO_CAM_POS,
    SMALL_EDIT_REPRO_CAM_QUAT, SMALL_EDIT_REPRO_HEIGHT, SMALL_EDIT_REPRO_IS_ERASE,
    SMALL_EDIT_REPRO_POST_EDIT_WAIT_FRAMES, SMALL_EDIT_REPRO_RADIUS, SMALL_EDIT_REPRO_TY,
    SMALL_EDIT_REPRO_WARMUP_FRAMES, SMALL_EDIT_REPRO_WIDTH,
};

/// The Oasis VOX fixture, crate-root-relative (the SUT CWD). The legacy
/// `oasis_vox_fixture_path()` resolves the workspace-relative path or the
/// crate-relative fallback; with the SUT CWD at the crate root the
/// crate-relative form is the one that resolves.
const OASIS_VOX_FIXTURE: &str = "assets/test/oasis_hard_cover.vox";

#[test]
fn small_edit_repro() {
    println!(
        "small_edit_repro: camera pos={:?} quat={:?}; cube_brush pos={:?} radius={} ty={}; \
         window {}x{}",
        SMALL_EDIT_REPRO_CAM_POS,
        SMALL_EDIT_REPRO_CAM_QUAT,
        SMALL_EDIT_REPRO_BRUSH_POS,
        SMALL_EDIT_REPRO_RADIUS,
        SMALL_EDIT_REPRO_TY,
        SMALL_EDIT_REPRO_WIDTH,
        SMALL_EDIT_REPRO_HEIGHT,
    );

    // 1. Spawn the SUT — Oasis VOX via `--vox`, the user's 1920×1080 window.
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .vox(OASIS_VOX_FIXTURE)
            .window(SMALL_EDIT_REPRO_WIDTH, SMALL_EDIT_REPRO_HEIGHT),
    );

    // 2. World presence check.
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "small_edit_repro: SUT reports world_loaded=false — the Oasis VOX \
         load failed"
    );

    // 3. Pin the user-captured camera pose. Reconstruct the raw quaternion via
    //    forward + up (see the module doc — exact for a unit rotation).
    let pos = SMALL_EDIT_REPRO_CAM_POS;
    let quat = SMALL_EDIT_REPRO_CAM_QUAT;
    let forward = quat * Vec3::NEG_Z;
    let up = quat * Vec3::Y;
    let look_at = pos + forward;
    scenario::set_camera(
        sut.client(),
        [pos.x, pos.y, pos.z],
        [look_at.x, look_at.y, look_at.z],
        Some([up.x, up.y, up.z]),
    )
    .expect("naadf/set_camera");

    // 4. Warm up — `SMALL_EDIT_REPRO_WARMUP_FRAMES`.
    scenario::advance(sut.client(), SMALL_EDIT_REPRO_WARMUP_FRAMES).expect("warmup advance");

    // 5. Capture frame A.
    let before = scenario::capture(sut.client()).expect("capture A");

    // 6. Apply the EXACT user-captured cube brush via the production path.
    //    `naadf/apply_brush` `kind:"cube"` calls the same `cube_brush` the
    //    legacy `apply_small_edit_repro_edit` does.
    let bpos = SMALL_EDIT_REPRO_BRUSH_POS;
    let brush: schema::ApplyBrushResult = sut
        .client()
        .call_typed(
            "naadf/apply_brush",
            serde_json::json!({
                "kind": "cube",
                "pos": [bpos.x, bpos.y, bpos.z],
                "radius": SMALL_EDIT_REPRO_RADIUS,
                "voxel_type": SMALL_EDIT_REPRO_TY as u32,
                "erase": SMALL_EDIT_REPRO_IS_ERASE,
            }),
        )
        .expect("naadf/apply_brush");
    println!(
        "small_edit_repro: cube_brush at {bpos:?} — voxels_delta {} blocks_delta {} batches {}",
        brush.voxels_delta, brush.blocks_delta, brush.batches
    );

    // 7. Wait for render convergence.
    scenario::advance(sut.client(), SMALL_EDIT_REPRO_POST_EDIT_WAIT_FRAMES)
        .expect("post-edit advance");

    // 8. Capture frame B.
    let after = scenario::capture(sut.client()).expect("capture B");

    // 9. Save both framebuffers (legacy filenames).
    let _ = before.save_png("target/e2e-screenshots/small_edit_repro_before.png");
    let _ = after.save_png("target/e2e-screenshots/small_edit_repro_after.png");

    // 10. The load-bearing gate — `assert_no_pitch_black_pixels` reused verbatim.
    let report = assert_no_pitch_black_pixels(&before, &after)
        .unwrap_or_else(|msg| panic!("small_edit_repro gate FAIL — {msg}"));

    // 11. Pipeline-error scan.
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");

    println!("small_edit_repro: {report}");
}
