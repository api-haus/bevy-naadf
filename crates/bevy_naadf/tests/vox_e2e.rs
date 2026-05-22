//! BRP-driven e2e gate — `vox_e2e`, migrated from the legacy in-app
//! `e2e_render --vox-e2e` driver mode (`e2e::vox_e2e::run_vox_e2e`)
//! (`e2e-ipc-rpc-restructure` Phase 3a).
//!
//! ## What this gate proves
//!
//! The Track A `.vox` load-path regression gate (`e2e/vox_e2e.rs` module doc):
//! synthesise a multi-model `.vox` fixture in memory (two emissive models under
//! non-trivial `nTRN` translations), write it to disk, boot the SUT through the
//! production `--vox <path>` ingestion path, warm up, capture, and assert the
//! central screen rect — where the synthesised emissive geometry projects —
//! sits above the sky-luminance band *and* has meaningful per-channel colour.
//! A scene-graph composition regression that stacks both models at origin, or
//! a colour-upload regression, trips the gate.
//!
//! ## Migration fidelity (Phase 3a brief — binding)
//!
//! The fixture builder (`vox_e2e::write_vox_e2e_fixture_to_temp`) and the
//! assertion (`vox_e2e::assert_vox_geometry_visible`, with its `SKY_LUMINANCE_CEILING`
//! / `VOX_GEOMETRY_CHANNEL_MAX_FLOOR` thresholds) are reused from the library
//! **verbatim** — the test never re-implements them. Only the harness changes:
//! the legacy in-app driver is replaced by the BRP SUT.
//!
//! ## Camera pose
//!
//! The legacy gate uses the standard driver flow — the `gates::e2e_camera_transform()`
//! readback pose (the fixture is sized + positioned so its emissive cubes fill
//! the central frame at exactly that pose). The BRP test pins that pose.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test vox_e2e
//! ```

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::e2e::gates::e2e_orbit_camera_transform;
use bevy_naadf::e2e::vox_e2e::{assert_vox_geometry_visible, write_vox_e2e_fixture_to_temp};

// ---------------------------------------------------------------------------
// Frame budget — the legacy `--vox-e2e` gate runs the STANDARD driver flow
// (`bin/e2e_render.rs` `--vox-e2e` branch — `E2eGateMode::Standard`), so the
// same `E2E_WARMUP_FRAMES` + `E2E_MOTION_FRAMES` + `E2E_SETTLE_FRAMES` budget
// and the same open-path camera sweep the `standard` gate uses (`e2e/mod.rs`).
// Ported verbatim — see `tests/standard.rs` for the camera-sweep rationale.
// ---------------------------------------------------------------------------

/// `E2E_WARMUP_FRAMES` — TAA + GI temporal convergence at the motion-start pose.
const E2E_WARMUP_FRAMES: u32 = 96;
/// `E2E_MOTION_FRAMES` — the legacy driver's camera-motion phase length.
const E2E_MOTION_FRAMES: u32 = 48;
/// `E2E_SETTLE_FRAMES` — the legacy driver's post-motion settle.
const E2E_SETTLE_FRAMES: u32 = 1;

/// Set the SUT camera to a library `Transform` via `naadf/set_camera` — the
/// `e2e_orbit_camera_transform` poses are all `looking_at(.., Vec3::Y)`.
fn pin_camera(sut: &mut Sut, pose: bevy::prelude::Transform) {
    let fwd = pose.forward();
    scenario::set_camera(
        sut.client(),
        [pose.translation.x, pose.translation.y, pose.translation.z],
        [
            pose.translation.x + fwd.x,
            pose.translation.y + fwd.y,
            pose.translation.z + fwd.z,
        ],
        Some([0.0, 1.0, 0.0]),
    )
    .expect("naadf/set_camera");
}

#[test]
fn vox_e2e() {
    // 1. Synthesise the multi-model `.vox` fixture to disk (reuses the library
    //    `write_vox_e2e_fixture_to_temp` verbatim). The test process CWD is the
    //    crate root, so the fixture lands under `crates/bevy_naadf/target/` —
    //    the same place the SUT (CWD = crate root) resolves a relative path.
    let fixture_abs = write_vox_e2e_fixture_to_temp()
        .expect("vox_e2e: failed to write the synthesised .vox fixture");
    println!(
        "vox_e2e: synthesised .vox fixture written to {} (2 models, 2 nTRN translations)",
        fixture_abs.display()
    );
    // `write_vox_e2e_fixture_to_temp` returns a path relative to the process
    // CWD (`target/e2e-screenshots/vox_e2e_fixture.vox`). The SUT CWD is the
    // same crate root, so the same relative path resolves for it.
    let fixture_rel = "target/e2e-screenshots/vox_e2e_fixture.vox";

    // 2. Spawn the SUT through the production `--vox <path>` ingestion path,
    //    the legacy 256×256 e2e window (the gate rect fractions are calibrated
    //    against it).
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .vox(fixture_rel)
            .window(256, 256),
    );

    // 3. World presence check — confirms the `.vox` load actually populated a
    //    world (not the silent-empty-world regression the gate guards).
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "vox_e2e: SUT reports world_loaded=false — the synthesised .vox \
         fixture failed to load through the production --vox path"
    );

    // 4. WARMUP — static at the motion-start pose (t == 0).
    pin_camera(&mut sut, e2e_orbit_camera_transform(0.0));
    scenario::advance(sut.client(), E2E_WARMUP_FRAMES).expect("warmup advance");

    // 5. MOTION — sweep the open camera path, one camera write per frame
    //    (`t = tick / E2E_MOTION_FRAMES`, exactly the legacy driver's
    //    `E2ePhase::Motion` arm). SETTLE — 1 frame static at t == 1.
    for tick in 1..=E2E_MOTION_FRAMES {
        let t = tick as f32 / E2E_MOTION_FRAMES as f32;
        pin_camera(&mut sut, e2e_orbit_camera_transform(t));
        scenario::advance_one_frame(sut.client()).expect("motion-phase frame advance");
    }
    pin_camera(&mut sut, e2e_orbit_camera_transform(1.0));
    scenario::advance(sut.client(), E2E_SETTLE_FRAMES).expect("settle advance");

    // 6. Capture.
    let fb = scenario::capture(sut.client()).expect("capture");
    let _ = fb.save_png("target/e2e-screenshots/vox_e2e_latest.png");
    println!(
        "vox_e2e: readback {}x{} saved to crates/bevy_naadf/target/e2e-screenshots/vox_e2e_latest.png",
        fb.width(),
        fb.height()
    );

    // 7. The load-bearing gate — `assert_vox_geometry_visible` reused verbatim
    //    (the `SKY_LUMINANCE_CEILING` + `VOX_GEOMETRY_CHANNEL_MAX_FLOOR`
    //    thresholds live inside it; the brief mandates verbatim port).
    assert_vox_geometry_visible(&fb)
        .unwrap_or_else(|msg| panic!("vox_e2e gate FAIL — {msg}"));

    // 8. Pipeline-error scan.
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");

    println!("vox_e2e: PASS — synthesised .vox geometry visible above the sky band");
}
