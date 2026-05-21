//! BRP-driven e2e gate — `standard`, migrated from the legacy in-app
//! `e2e_render` default gate (no flag → `e2e::run_e2e_render`)
//! (`e2e-ipc-rpc-restructure` Phase 3a).
//!
//! ## What this gate proves
//!
//! The default windowed render-test (`e2e/mod.rs` module doc): boot the real
//! `DefaultPlugins` windowed app on the `GridPreset::Default` test scene, warm
//! up at the motion-path start pose, **sweep the camera along the open motion
//! path** to the fixed readback pose, settle one frame, read the on-screen
//! framebuffer back, and run the batch-aware visual checks — the
//! degenerate-frame floor, the global luminance-liveness gate, and the
//! Batch-6 per-batch region gate (`assert_batch_6`).
//!
//! ## Why the camera-motion sweep is reproduced (NOT a static-pinned pose)
//!
//! The legacy standard gate's readback pose is one the camera reaches **only by
//! moving** — `E2E_SETTLE_FRAMES` is deliberately `1` so the static-camera GI
//! running-average never re-converges (`e2e/mod.rs` const doc). `assert_batch_6`'s
//! TAA-camera-motion stability check (`MIN_GI_BOUNCE_AFTER_MOTION`) and the
//! `check_not_degenerate` `has_dark` requirement both depend on that
//! post-motion frame. A first migration draft that pinned the readback pose
//! statically and warmed up the full 145-frame budget FAILED the
//! degenerate-frame floor (`has_dark=false`) — a fully GI-converged static
//! frame has no dark geometry. So this test reproduces the legacy driver's
//! three phases verbatim:
//!   - `Warmup` — 96 frames static at `e2e_orbit_camera_transform(0.0)`;
//!   - `Motion` — 48 frames, one camera write per frame at
//!     `e2e_orbit_camera_transform(tick/48)`;
//!   - `Settle` — 1 frame static at `e2e_orbit_camera_transform(1.0)`.
//!
//! The per-frame camera writes use `scenario::advance_one_frame` (a Phase-3a
//! helper). The BRP SUT free-runs, so a single `naadf/step` maps to ~1-2 native
//! rendered frames rather than exactly one — the motion is therefore *very
//! close* to but not byte-identical to the legacy per-`Update`-tick sweep. See
//! the `03-impl.md` Phase 3a side-notes.
//!
//! ## Migration fidelity (Phase 3a brief — binding)
//!
//! Every frame-budget constant + camera-pose fn + assertion is reused from the
//! library **verbatim**: `E2E_WARMUP_FRAMES` / `E2E_MOTION_FRAMES` /
//! `E2E_SETTLE_FRAMES`, `e2e_orbit_camera_transform`, `check_not_degenerate`,
//! `check_luminance_alive`, `batch_gate(CURRENT_BATCH, ..)`.
//!
//! ## 5/5-check parity (Phase 3b)
//!
//! The legacy standard-gate `run_assertions` runs five checks. Phase 3a
//! migrated four (the pure `Framebuffer` / threshold checks) and noted the
//! fifth — `assert_nodes_dispatched` (the render-graph node-dispatch check
//! reading the main-world `DiagnosticsStore`) — had **no BRP verb**. Phase 3b
//! added `naadf/nodes_dispatched` (wrapping the already-`pub`
//! `assert_nodes_dispatched` + `expected_spans(CURRENT_BATCH)`); this gate now
//! calls it as step 10, restoring full 5/5-check parity with the legacy gate.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test standard
//! ```

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::e2e::gates::{
    batch_gate, e2e_orbit_camera_transform, region_luminance_report, GateState, CURRENT_BATCH,
};

// ---------------------------------------------------------------------------
// Frame budget — ported VERBATIM from `crates/bevy_naadf/src/e2e/mod.rs`.
// ---------------------------------------------------------------------------

/// `E2E_WARMUP_FRAMES` — static warmup at the motion-start pose (TAA + GI
/// temporal convergence before the camera moves).
const E2E_WARMUP_FRAMES: u32 = 96;
/// `E2E_MOTION_FRAMES` — the open-path camera-motion phase length.
const E2E_MOTION_FRAMES: u32 = 48;
/// `E2E_SETTLE_FRAMES` — post-motion settle at the readback pose (bare minimum
/// on purpose — see the `e2e/mod.rs` const doc).
const E2E_SETTLE_FRAMES: u32 = 1;

/// Set the SUT camera to a library `Transform` via `naadf/set_camera`.
/// `set_camera` rebuilds `Transform::from_translation(t).looking_at(look_at, up)`;
/// feeding `look_at = translation + forward` with `up = +Y` reproduces the
/// `e2e_orbit_camera_transform` poses exactly (they are all built with
/// `looking_at(.., Vec3::Y)`).
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
fn standard() {
    // 1. Spawn the production binary as the SUT — no `--vox` ⇒ the
    //    `GridPreset::Default` embedded test scene; the legacy 256×256 e2e
    //    window so the `gates.rs` region-rect calibrations stay valid.
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR")).window(256, 256),
    );

    // 2. World presence check.
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "standard: SUT reports world_loaded=false — the default test grid \
         failed to install"
    );

    // 3. WARMUP phase — static at the motion-start pose (t == 0).
    pin_camera(&mut sut, e2e_orbit_camera_transform(0.0));
    scenario::advance(sut.client(), E2E_WARMUP_FRAMES).expect("warmup advance");

    // 4. MOTION phase — sweep the open camera path, one camera write per frame.
    //    `t` runs (0, 1] over `E2E_MOTION_FRAMES`, exactly as the legacy
    //    `E2ePhase::Motion` arm (`t = phase_ticks / E2E_MOTION_FRAMES`).
    for tick in 1..=E2E_MOTION_FRAMES {
        let t = tick as f32 / E2E_MOTION_FRAMES as f32;
        pin_camera(&mut sut, e2e_orbit_camera_transform(t));
        scenario::advance_one_frame(sut.client()).expect("motion-phase frame advance");
    }

    // 5. SETTLE phase — static at the readback pose (t == 1).
    pin_camera(&mut sut, e2e_orbit_camera_transform(1.0));
    scenario::advance(sut.client(), E2E_SETTLE_FRAMES).expect("settle advance");

    // 6. Capture the readback frame.
    let fb = scenario::capture(sut.client()).expect("capture");
    let _ = fb.save_png("target/e2e-screenshots/e2e_latest.png");
    println!(
        "standard: readback {}x{} saved to crates/bevy_naadf/target/e2e-screenshots/e2e_latest.png",
        fb.width(),
        fb.height()
    );

    // 7. Assertion (1) — degenerate-frame floor.
    fb.check_not_degenerate()
        .unwrap_or_else(|msg| panic!("standard gate FAIL — degenerate-frame floor:\n  {msg}"));

    // 8. Assertion (2) — global luminance-liveness gate (batch-aware).
    fb.check_luminance_alive(CURRENT_BATCH)
        .unwrap_or_else(|msg| panic!("standard gate FAIL — luminance liveness gate:\n  {msg}"));

    // Diagnostic — the same region-luminance line the legacy driver prints.
    println!("standard: {}", region_luminance_report(&fb));

    // 9. Assertion (3) — the Batch-6 per-batch region gate.
    let gate_state = GateState { fb: &fb, fb_next: None };
    batch_gate(CURRENT_BATCH, &gate_state)
        .unwrap_or_else(|msg| panic!("standard gate FAIL — region gate:\n  {msg}"));

    // 10. Pipeline-error scan (`naadf/pipeline_scan` — the legacy
    //     `PipelineCache` error scan).
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");

    // 11. Node-dispatch check (`naadf/nodes_dispatched` — the legacy
    //     `run_assertions` 5th check; restores 5/5-check parity, Phase 3b).
    //     Asserts every expected render-graph span for `CURRENT_BATCH`
    //     recorded a `DiagnosticsStore` measurement (the node ran).
    scenario::nodes_dispatched(sut.client())
        .expect("naadf/nodes_dispatched reported missing nodes");

    println!(
        "standard: PASS — degenerate floor + luminance liveness + Batch-{CURRENT_BATCH} \
         region gate + pipeline scan + node-dispatch check all green (5/5-check parity)"
    );
}
