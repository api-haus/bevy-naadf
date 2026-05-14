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
//! RUN   (E2E_RENDER_FRAMES ticks): just count — let the graph render & pipelines compile
//! SHOOT (1 tick):                  spawn Screenshot::primary_window() + observer
//! DRAIN (<= E2E_DRAIN_FRAMES):     wait for ScreenshotCaptured to populate E2eScreenshot
//! ASSERT (1 tick):                 build Framebuffer, run the gates, write AppExit
//! DONE:                            AppExit written — the winit runner exits the event loop
//! ```

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;

use super::checks::{assert_nodes_dispatched, pipeline_scan_result, PipelineScanResult};
use super::framebuffer::Framebuffer;
use super::gates::{batch_gate, expected_spans, GateState, CURRENT_BATCH};
use super::readback::{shoot_primary_window, E2eScreenshot};
use super::{E2E_DRAIN_FRAMES, E2E_RENDER_FRAMES};

/// The driver's state-machine phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum E2ePhase {
    /// Counting render frames — let the graph render & every pipeline compile.
    #[default]
    Run,
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
pub fn e2e_driver(
    mut state: ResMut<E2eState>,
    mut outcome: ResMut<E2eOutcome>,
    mut screenshot: ResMut<E2eScreenshot>,
    diagnostics: Res<DiagnosticsStore>,
    pipeline_scan: Res<PipelineScanResult>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    match state.phase {
        E2ePhase::Run => {
            state.phase_ticks += 1;
            // E2E_RENDER_FRAMES render frames is comfortably above the
            // resource-build latency (~3 frames: extract world, prepare GPU
            // resources, first full graph execution) with margin for the
            // camera-history ring to spin up — and with
            // `synchronous_pipeline_compilation` every pipeline a node queues
            // resolves the same frame it is queued, so by the time RUN ends
            // every render-graph pipeline has been created (R3).
            if state.phase_ticks >= E2E_RENDER_FRAMES {
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
            let result =
                run_assertions(screenshot.as_mut(), &diagnostics, &pipeline_scan);
            match &result {
                Ok(()) => {
                    println!(
                        "e2e_render: PASS (batch {CURRENT_BATCH}) — {E2E_RENDER_FRAMES} render \
                         frames, framebuffer read back & non-degenerate, per-batch region gate \
                         green, every pipeline created cleanly, every expected render-graph \
                         node dispatched."
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

/// Run **all three checks** at the `ASSERT` step and fold them into one
/// `Result` (`e2e-render-test.md` §6.5 — every check runs inside the app
/// because the winit runner consumes the `App`, so there is no post-`run()`
/// inspection point):
///
/// 1. **Degenerate-frame floor** (§7) — the readback must not be a stuck clear
///    colour / contrast-less frame. Runs first so a uniformly-black frame gives
///    a clear message rather than a confusing region-mean assertion.
/// 2. **Per-batch region gate** (§6.2) — `batch_gate` for `CURRENT_BATCH`;
///    older batches' gates are kept as called helpers so an earlier-gate
///    regression still trips.
/// 3. **Node-dispatch check** (§8) — every expected render-graph span has a
///    recorded `DiagnosticsStore` measurement (`DiagnosticsStore` is
///    main-world).
/// 4. **`PipelineCache` error scan** (§3.1) — the load-bearing check, read from
///    the shared cross-world channel the render-world scan system fills.
///
/// All four are collected so a single run reports *every* failure, not just the
/// first — that is the whole point of the harness (`e2e-render-test.md` §1).
fn run_assertions(
    screenshot: &mut E2eScreenshot,
    diagnostics: &DiagnosticsStore,
    pipeline_scan: &PipelineScanResult,
) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();

    // --- The framebuffer-dependent checks (1 + 2).
    match screenshot.0.as_ref() {
        Some(image) => match Framebuffer::from_image(image) {
            Ok(fb) => {
                if let Err(msg) = fb.check_not_degenerate() {
                    failures.push(format!("degenerate-frame floor:\n  {msg}"));
                }
                let state = GateState {
                    fb: &fb,
                    fb_next: None,
                };
                if let Err(msg) = batch_gate(CURRENT_BATCH, &state) {
                    failures.push(format!("region gate:\n  {msg}"));
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
