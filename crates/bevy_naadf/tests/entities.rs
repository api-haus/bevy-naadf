//! BRP-driven e2e gate — `entities`, migrated from the legacy in-app
//! `e2e_render --entities` driver mode (the `EntitiesBoot` arm in
//! `bin/e2e_render.rs` — no `run_*` fn)
//! (`e2e-ipc-rpc-restructure` Phase 3b).
//!
//! ## What this gate proves
//!
//! The Phase-C W4 entity-track gate (`e2e/gates.rs::assert_entity_pixel` doc):
//! the legacy `--entities` boot spawns ONE 4×4×4 emissive-voxel test fixture
//! entity at the test scene's world centre and enables the W4 entity track,
//! then runs the **standard gate flow** (warmup → camera-motion sweep →
//! settle → capture) and adds one extra assertion — `assert_entity_pixel`:
//! the screen region the fixture entity projects into at the fixed readback
//! pose is brightly lit (mean luminance ≥ `ENTITY_PIXEL_MIN_LUM`). That proves
//! both (a) the entity-update dispatch landed (the entity rendered) and (b)
//! the underlying renderer is producing usable framebuffer content at that
//! screen position.
//!
//! ## The entity spawn is boot-time config (Forbidden Move #4)
//!
//! The legacy `EntitiesBoot` arm sets `ConstructionConfig.entities_enabled =
//! true` + `SpawnTestEntity(true)` on the `BootstrapInputs` *before* the App
//! is built — `spawn_phase_c_test_entity` reads `SpawnTestEntity` at
//! `Startup`, and the W4 entity track is a render-graph wiring decision. Both
//! are consumed before `app.run()`, so they cannot be BRP verbs — they ride
//! the spawn contract: the `--e2e-entities` CLI flag on `bin/bevy-naadf`
//! (Phase 3b) sets both on the SUT's `BootstrapInputs`.
//!
//! ## Why the camera-motion sweep is reproduced (same as the `standard` gate)
//!
//! `--entities` runs the *standard* driver flow — `entity_pixel_rect` is
//! calibrated for the standard readback pose `e2e_camera_transform()`, which
//! is `e2e_orbit_camera_transform(1.0)`, the pose the camera reaches by
//! sweeping the open motion path. The standard gate's three checks
//! (`check_not_degenerate`, `check_luminance_alive`, `assert_batch_6`) all
//! depend on that post-motion frame (`E2E_SETTLE_FRAMES = 1` on purpose), so
//! this gate reproduces the legacy driver's three phases verbatim, exactly as
//! `tests/standard.rs` does, and then adds the entity-pixel check.
//!
//! ## Migration fidelity (Phase 3b brief — binding)
//!
//! The frame budget (`E2E_WARMUP_FRAMES` / `_MOTION_FRAMES` / `_SETTLE_FRAMES`),
//! the camera-pose fn (`e2e_orbit_camera_transform`), and every assertion
//! (`check_not_degenerate`, `check_luminance_alive`, `batch_gate`,
//! `assert_entity_pixel` — the `ENTITY_PIXEL_MIN_LUM` floor lives inside it)
//! are reused from the library **verbatim**. No threshold is recalibrated.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test entities
//! ```

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::e2e::gates::{
    assert_entity_pixel, batch_gate, e2e_orbit_camera_transform, region_luminance_report,
    GateState, CURRENT_BATCH,
};

// ---------------------------------------------------------------------------
// Frame budget — ported VERBATIM from `crates/bevy_naadf/src/e2e/mod.rs`
// (the standard driver flow — `--entities` runs the standard gate).
// ---------------------------------------------------------------------------

/// `E2E_WARMUP_FRAMES` — static warmup at the motion-start pose.
const E2E_WARMUP_FRAMES: u32 = 96;
/// `E2E_MOTION_FRAMES` — the open-path camera-motion phase length.
const E2E_MOTION_FRAMES: u32 = 48;
/// `E2E_SETTLE_FRAMES` — post-motion settle at the readback pose.
const E2E_SETTLE_FRAMES: u32 = 1;

/// Set the SUT camera to a library `Transform` via `naadf/set_camera`
/// (identical to `tests/standard.rs::pin_camera` — the orbit poses are all
/// built with `looking_at(.., Vec3::Y)`).
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
fn entities() {
    // 1. Spawn the SUT with `--e2e-entities` — spawns the Phase-C 4×4×4
    //    emissive-voxel fixture + enables the W4 entity track. No `--vox` ⇒
    //    the `GridPreset::Default` scene; the legacy 256×256 e2e window so the
    //    `entity_pixel_rect` calibration stays valid.
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .window(256, 256)
            .entities(true),
    );

    // 2. World presence check.
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "entities: SUT reports world_loaded=false — the default test grid \
         failed to install"
    );

    // 3. WARMUP — static at the motion-start pose (t == 0).
    pin_camera(&mut sut, e2e_orbit_camera_transform(0.0));
    scenario::advance(sut.client(), E2E_WARMUP_FRAMES).expect("warmup advance");

    // 4. MOTION — sweep the open camera path, one camera write per frame.
    for tick in 1..=E2E_MOTION_FRAMES {
        let t = tick as f32 / E2E_MOTION_FRAMES as f32;
        pin_camera(&mut sut, e2e_orbit_camera_transform(t));
        scenario::advance_one_frame(sut.client()).expect("motion-phase frame advance");
    }

    // 5. SETTLE — static at the readback pose (t == 1).
    pin_camera(&mut sut, e2e_orbit_camera_transform(1.0));
    scenario::advance(sut.client(), E2E_SETTLE_FRAMES).expect("settle advance");

    // 6. Capture the readback frame.
    let fb = scenario::capture(sut.client()).expect("capture");
    let _ = fb.save_png("target/e2e-screenshots/e2e_entities_latest.png");
    println!(
        "entities: readback {}x{} saved to \
         crates/bevy_naadf/target/e2e-screenshots/e2e_entities_latest.png",
        fb.width(),
        fb.height()
    );

    // 7. Standard-gate assertions (1)-(3) — same as `tests/standard.rs`.
    fb.check_not_degenerate()
        .unwrap_or_else(|msg| panic!("entities gate FAIL — degenerate-frame floor:\n  {msg}"));
    fb.check_luminance_alive(CURRENT_BATCH)
        .unwrap_or_else(|msg| panic!("entities gate FAIL — luminance liveness gate:\n  {msg}"));
    println!("entities: {}", region_luminance_report(&fb));
    let gate_state = GateState { fb: &fb, fb_next: None };
    batch_gate(CURRENT_BATCH, &gate_state)
        .unwrap_or_else(|msg| panic!("entities gate FAIL — region gate:\n  {msg}"));

    // 8. The entity-specific assertion — `assert_entity_pixel` reused VERBATIM
    //    (the `ENTITY_PIXEL_MIN_LUM` floor lives inside it). This is the check
    //    the legacy driver runs ONLY in `--entities` mode.
    assert_entity_pixel(&gate_state)
        .unwrap_or_else(|msg| panic!("entities gate FAIL — entity_pixel gate:\n  {msg}"));

    // 9. Pipeline-error scan + node-dispatch check (same render-health checks
    //    the standard gate runs).
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");
    scenario::nodes_dispatched(sut.client())
        .expect("naadf/nodes_dispatched reported missing nodes");

    println!(
        "entities: PASS — degenerate floor + luminance liveness + Batch-{CURRENT_BATCH} \
         region gate + entity-pixel gate + pipeline scan + node-dispatch check all green"
    );
}
