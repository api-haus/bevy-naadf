//! The bounded-frame driver — a counting state machine, not a manual
//! `app.update()` loop (`e2e-render-test.md` §4.1).
//!
//! The run is driven by the real winit runner, so there is no manual update
//! loop: the driver is a single `Update` system that owns a frame counter +
//! a state machine and advances one step per tick. The winit runner ticks
//! `Update` every frame (`UpdateMode::Continuous` — set in
//! [`crate::e2e::add_e2e_systems`]), so the fixed frame budget advances
//! deterministically.
//!
//! ```text
//! WARMUP (E2E_WARMUP_FRAMES ticks): static at the fixed pose — let GI converge
//! MOTION (E2E_MOTION_FRAMES ticks): the camera orbits a deterministic path —
//!                                   exercises the TAA camera-motion reprojection
//! SETTLE (E2E_SETTLE_FRAMES ticks): static at the fixed pose again — the camera
//!                                   has just stopped moving; a correct TAA holds
//!                                   the GI bounce, a broken one has decayed it
//! SHOOT  (1 tick):                  spawn Screenshot::primary_window() + observer
//! DRAIN  (<= E2E_DRAIN_FRAMES):     wait for ScreenshotCaptured to populate E2eScreenshot
//! ASSERT (1 tick):                  build Framebuffer, run the gates, write AppExit
//! DONE:                             AppExit written — the winit runner exits the event loop
//! ```
//!
//! **The moving-camera phase (2026-05-15).** The original harness only ever
//! exercised a *static* camera — the coverage gap that let the TAA
//! camera-motion reprojection decay through the review gate (`10-impl-b.md` —
//! TAA shadow decay-to-black). `WARMUP → MOTION → SETTLE` closes it: the camera
//! orbits a fixed deterministic path ([`super::gates::e2e_orbit_camera_transform`])
//! during `MOTION`, then `e2e_orbit_camera_transform(1.0)` lands it back
//! *exactly* on the fixed [`super::gates::e2e_camera_transform`] pose for
//! `SETTLE` + the readback — so every camera-pose-coupled gate rectangle stays
//! valid, while the frames leading up to the readback were under continuous
//! motion. If the TAA reprojection decays shadowed/indirect regions under
//! motion, `assert_batch_6`'s `solid_block_rect` GI-bounce check fails at the
//! settled readback; a correct reprojection keeps it GI-lit.

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;

use std::path::Path;

use crate::camera::PositionSplit;

use super::checks::{assert_nodes_dispatched, pipeline_scan_result, PipelineScanResult};
use super::framebuffer::Framebuffer;
use super::gates::{
    batch_gate, e2e_orbit_camera_transform, expected_spans, GateState, CURRENT_BATCH,
};
use super::readback::{shoot_primary_window, E2eScreenshot};
use super::{
    E2E_DRAIN_FRAMES, E2E_MOTION_FRAMES, E2E_SCREENSHOT_DIR, E2E_SCREENSHOT_LATEST,
    E2E_SETTLE_FRAMES, E2E_WARMUP_FRAMES,
};

/// The driver's state-machine phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum E2ePhase {
    /// Static at the fixed pose — count render frames so GI converges & every
    /// pipeline compiles.
    #[default]
    Warmup,
    /// The camera orbits a deterministic path — exercises the TAA
    /// camera-motion reprojection.
    Motion,
    /// Static at the fixed pose again — the camera has just stopped; a correct
    /// TAA holds the GI bounce, a broken one has decayed it to black.
    Settle,
    /// Spawn the screenshot this tick.
    Shoot,
    /// Wait (bounded) for the async capture to deliver.
    Drain,
    /// Build the framebuffer, run the gates, write `AppExit`.
    Assert,
    /// `AppExit` written — the winit runner is exiting; the driver no-ops.
    Done,
}

/// The driver resource: the current phase + a per-phase tick counter.
#[derive(Resource, Default)]
pub struct E2eState {
    pub phase: E2ePhase,
    /// Ticks elapsed *within* the current phase.
    pub phase_ticks: u32,
}

/// The detailed gate outcome, stashed for `run_e2e_render`'s post-run report
/// (the `AppExit` alone only carries success/failure; this carries the *why*).
#[derive(Resource, Default)]
pub struct E2eOutcome {
    /// `None` while still running; `Some(Ok(()))` / `Some(Err(msg))` once the
    /// `ASSERT` step has run.
    pub gate_result: Option<Result<(), String>>,
}

/// The `Update` driver system — advances the state machine one step per tick.
///
/// Also drives the deterministic camera motion: during [`E2ePhase::Motion`] it
/// sets the camera `Transform` to [`e2e_orbit_camera_transform`] for the
/// phase's progress `t`, and keeps `PositionSplit` in step (the production
/// `sync_position_split` also runs, but the driver writes both so the camera
/// state is consistent within this very tick — no one-frame lag).
// Bevy systems legitimately exceed clippy's 7-argument ceiling.
#[allow(clippy::too_many_arguments)]
// Bevy systems legitimately exceed clippy's 7-argument ceiling — Phase-C
// followup #5 adds one read-only `AppArgs` parameter for the entity-pixel
// gate.
#[allow(clippy::too_many_arguments)]
pub fn e2e_driver(
    mut state: ResMut<E2eState>,
    mut outcome: ResMut<E2eOutcome>,
    mut screenshot: ResMut<E2eScreenshot>,
    diagnostics: Res<DiagnosticsStore>,
    pipeline_scan: Res<PipelineScanResult>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    app_args: Option<Res<crate::AppArgs>>,
) {
    match state.phase {
        E2ePhase::Warmup => {
            state.phase_ticks += 1;
            // Pin the camera at the motion-path START pose (the t == 0
            // endpoint). It is already spawned here (`setup_e2e_camera`); keep
            // it explicit so WARMUP is unambiguously static at the start pose
            // and the GI converges there before the camera moves.
            let pose = e2e_orbit_camera_transform(0.0);
            let (transform, position_split) = &mut *camera;
            **transform = pose;
            **position_split = PositionSplit::from_world(pose.translation);
            // E2E_WARMUP_FRAMES render frames is comfortably above the
            // resource-build latency (~3 frames: extract world, prepare GPU
            // resources, first full graph execution) with margin for the
            // camera-history ring to spin up — and with
            // `synchronous_pipeline_compilation` every pipeline a node queues
            // resolves the same frame it is queued, so by the time WARMUP ends
            // every render-graph pipeline has been created (R3). It is also
            // long enough for the temporal GI to fully converge at the start
            // pose before the camera starts moving.
            if state.phase_ticks >= E2E_WARMUP_FRAMES {
                state.phase = E2ePhase::Motion;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::Motion => {
            state.phase_ticks += 1;
            // Drive the deterministic open-path camera move. `t` runs (0, 1]
            // over the motion phase — the camera is genuinely moving every
            // frame of it, sweeping from the start pose toward the readback
            // pose.
            let t = state.phase_ticks as f32 / E2E_MOTION_FRAMES as f32;
            let pose = e2e_orbit_camera_transform(t);
            let (transform, position_split) = &mut *camera;
            **transform = pose;
            **position_split = PositionSplit::from_world(pose.translation);
            if state.phase_ticks >= E2E_MOTION_FRAMES {
                // `e2e_orbit_camera_transform(1.0)` is exactly the fixed
                // readback pose (`e2e_camera_transform`) — the t == 1 endpoint
                // of the open path — so SETTLE + the readback happen at the
                // pose all the gate rectangles are derived from. Critically
                // the camera was NEVER static here before now: all the GI/TAA
                // history feeding the readback came through the camera-motion
                // reprojection.
                state.phase = E2ePhase::Settle;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::Settle => {
            state.phase_ticks += 1;
            // Pin the camera at the fixed readback pose (the open path already
            // landed it here on the last MOTION tick; keep it explicit so
            // SETTLE is unambiguously static). These frames are the
            // diagnostic: the camera has just *stopped* moving at a pose it
            // was never static at before — a faithful TAA reprojection has
            // carried the GI bounce here through the motion, a broken one has
            // reprojected it away and the shadowed regions are black.
            let pose = e2e_orbit_camera_transform(1.0);
            let (transform, position_split) = &mut *camera;
            **transform = pose;
            **position_split = PositionSplit::from_world(pose.translation);
            if state.phase_ticks >= E2E_SETTLE_FRAMES {
                state.phase = E2ePhase::Shoot;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::Shoot => {
            // Read back the *actual on-screen window surface* — the exact
            // composited output `naadf_final_blit_node` produced (§5.1).
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::Drain;
            state.phase_ticks = 0;
        }
        E2ePhase::Drain => {
            state.phase_ticks += 1;
            if screenshot.0.is_some() {
                // The capture arrived — proceed to assert.
                state.phase = E2ePhase::Assert;
                state.phase_ticks = 0;
            } else if state.phase_ticks >= E2E_DRAIN_FRAMES {
                // The drain bound is generous (E2E_DRAIN_FRAMES) precisely so a
                // slow-but-working readback is not a false failure (R2). If it
                // is still empty, the render path never delivered a frame —
                // that is a real, correct failure.
                let msg = format!(
                    "no framebuffer produced — the render path never delivered a frame \
                     within {E2E_DRAIN_FRAMES} drain frames (Screenshot::primary_window \
                     capture never fired)"
                );
                eprintln!("e2e_render: FAIL — {msg}");
                outcome.gate_result = Some(Err(msg));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::Assert => {
            // Phase-C followup #5 — surface `--entities` mode to the
            // assertions so the entity-pixel luminance gate fires only when
            // a fixture entity was spawned (the gate baseline is
            // entity-mode-specific).
            let entities_mode =
                app_args.as_deref().is_some_and(|a| a.spawn_test_entity);
            let result =
                run_assertions(screenshot.as_mut(), &diagnostics, &pipeline_scan, entities_mode);
            match &result {
                Ok(()) => {
                    println!(
                        "e2e_render: PASS (batch {CURRENT_BATCH}) — {E2E_WARMUP_FRAMES} warmup + \
                         {E2E_MOTION_FRAMES} camera-motion + {E2E_SETTLE_FRAMES} settle frames, \
                         framebuffer read back & non-degenerate, per-batch region gate green \
                         through camera motion, every pipeline created cleanly, every expected \
                         render-graph node dispatched."
                    );
                    exit.write(AppExit::Success);
                }
                Err(msg) => {
                    eprintln!("e2e_render: FAIL —\n{msg}");
                    exit.write(AppExit::error());
                }
            }
            outcome.gate_result = Some(result);
            state.phase = E2ePhase::Done;
        }
        E2ePhase::Done => {
            // `AppExit` is written; the winit runner sees `should_exit()` and
            // exits the event loop. Nothing more to do.
        }
    }
}

/// Run **every check** at the `ASSERT` step and fold them into one `Result`
/// (`e2e-render-test.md` §6.5 — every check runs inside the app because the
/// winit runner consumes the `App`, so there is no post-`run()` inspection
/// point):
///
/// 1. **Screenshot-to-disk** — the readback `Framebuffer` is written to
///    `target/e2e-screenshots/e2e_latest.png` *unconditionally*, every run,
///    before the gates, so an orchestrator/agent can `Read` it for visual
///    analysis regardless of pass/fail (`e2e-render-test.md` Implementation log
///    — 2026-05-14). The saved path is printed to stdout. A save *failure* is
///    itself a folded gate failure.
/// 2. **Degenerate-frame floor** (§7) — the readback must not be a stuck clear
///    colour / contrast-less frame. Runs first so a uniformly-black frame gives
///    a clear message rather than a confusing region-mean assertion.
/// 3. **Global luminance liveness gate** — a large fraction of the frame must
///    not be pitch black (`Framebuffer::check_luminance_alive` — 2026-05-14): a
///    global "the scene isn't mostly dead" check alongside the floor.
///    Batch-aware — the user's "at least 50%" target is a hard gate from the
///    GI-lit batch (B5) on; the pre-GI batches use a lower real-liveness floor
///    (the scene is correctly mostly dark before GI bounce).
/// 4. **Per-batch region gate** (§6.2) — `batch_gate` for `CURRENT_BATCH`;
///    older batches' gates are kept as called helpers so an earlier-gate
///    regression still trips.
/// 5. **Node-dispatch check** (§8) — every expected render-graph span has a
///    recorded `DiagnosticsStore` measurement (`DiagnosticsStore` is
///    main-world).
/// 6. **`PipelineCache` error scan** (§3.1) — the load-bearing check, read from
///    the shared cross-world channel the render-world scan system fills.
///
/// All are collected so a single run reports *every* failure, not just the
/// first — that is the whole point of the harness (`e2e-render-test.md` §1).
fn run_assertions(
    screenshot: &mut E2eScreenshot,
    diagnostics: &DiagnosticsStore,
    pipeline_scan: &PipelineScanResult,
    entities_mode: bool,
) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();

    // --- The framebuffer-dependent checks (1 + 2 + 3 + 4).
    match screenshot.0.as_ref() {
        Some(image) => match Framebuffer::from_image(image) {
            Ok(fb) => {
                // (1) Persist the readback to a fixed, documented path — every
                // run, before the gates, so the PNG is on disk for visual
                // analysis whether or not the gates pass.
                let path = Path::new(E2E_SCREENSHOT_DIR).join(E2E_SCREENSHOT_LATEST);
                match fb.save_png(&path) {
                    Ok(()) => println!(
                        "e2e_render: screenshot saved to {}",
                        path.display()
                    ),
                    Err(msg) => failures.push(format!("screenshot save:\n  {msg}")),
                }
                // (2) Degenerate-frame floor.
                if let Err(msg) = fb.check_not_degenerate() {
                    failures.push(format!("degenerate-frame floor:\n  {msg}"));
                }
                // (3) Global luminance liveness gate — a large fraction of the
                // frame must not be pitch black. Batch-aware threshold: 50%
                // from the GI-lit batch on, a lower real-liveness floor before.
                if let Err(msg) = fb.check_luminance_alive(CURRENT_BATCH) {
                    failures.push(format!("luminance liveness gate:\n  {msg}"));
                }
                // Diagnostic — print the key region luminances every run
                // (pass or fail) so a moving-camera decay is visible as a
                // trend even when the gate still passes by a margin.
                println!("e2e_render: {}", super::gates::region_luminance_report(&fb));
                // (4) Per-batch region gate.
                let state = GateState {
                    fb: &fb,
                    fb_next: None,
                };
                if let Err(msg) = batch_gate(CURRENT_BATCH, &state) {
                    failures.push(format!("region gate:\n  {msg}"));
                }
                // (4b) Phase-C followup #5 — entity-pixel gate. Fires only
                // in `--entities` mode (where the fixture entity is spawned).
                if entities_mode {
                    if let Err(msg) = super::gates::assert_entity_pixel(&state) {
                        failures.push(format!("entity_pixel gate:\n  {msg}"));
                    }
                }
            }
            Err(msg) => failures.push(format!("framebuffer decode:\n  {msg}")),
        },
        None => failures
            .push("framebuffer: ASSERT reached with no screenshot — driver bug".to_string()),
    }

    // --- The node-dispatch check (3).
    if let Err(msg) = assert_nodes_dispatched(diagnostics, expected_spans(CURRENT_BATCH)) {
        failures.push(format!("node-dispatch check:\n  {msg}"));
    }

    // --- The load-bearing PipelineCache error scan (4).
    if let Err(msg) = pipeline_scan_result(pipeline_scan) {
        failures.push(format!("PipelineCache error scan:\n  {msg}"));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} check(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        ))
    }
}
