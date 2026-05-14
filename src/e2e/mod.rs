//! The windowed end-to-end render-test harness (`e2e-render-test.md`).
//!
//! A single deterministic invocation — `cargo run --bin e2e_render` — that
//! boots the **real** `DefaultPlugins` + `WinitPlugin` windowed app (the same
//! wiring as `main.rs`, via the shared [`crate::build_app`]), runs the render
//! graph for a fixed frame budget, reads the on-screen window's framebuffer
//! back, runs region/statistic assertions plus a `PipelineCache` error scan,
//! and exits 0 on success / non-zero on failure.
//!
//! It exists to catch WGSL/shader/naga-oil/pipeline/bind-group errors that
//! `cargo build`/`cargo test` miss — those compile only at runtime. It
//! **replaces the open-ended live smoke-run** as the verification step for this
//! project's impl agents (`e2e-render-test.md` §10): the run is bounded and
//! self-terminating, so the agent runs it *once* and reads the exit code — no
//! rebuild→rerun loop.
//!
//! Module layout (`e2e-render-test.md` §9):
//! - [`driver`] — the bounded-frame state-machine system + the `AppExit` write.
//! - [`readback`] — `Screenshot::primary_window()` + the capture observer.
//! - [`checks`] — the batch-agnostic `PipelineCache` scan + node-dispatch check.
//! - [`framebuffer`] — the format-aware `Framebuffer` wrapper + region helpers.
//! - [`gates`] — the per-batch region gates, the camera pose, `CURRENT_BATCH`.

pub mod checks;
pub mod driver;
pub mod framebuffer;
pub mod gates;
pub mod readback;

use bevy::prelude::*;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::winit::{UpdateMode, WinitSettings};

use crate::camera::position_split::PositionSplit;
use crate::e2e::checks::{scan_pipeline_errors_render_system, PipelineScanResult};

// --- Frame-budget constants (`e2e-render-test.md` §3.3, §4.1, §5.2) --------

/// Fixed e2e window resolution — small + fixed so the readback is fast, the GI
/// dispatch is cheap, and every `pixel_count`-sized buffer is identical
/// run-to-run (`e2e-render-test.md` §4.2 / §9). 256² is large enough for stable
/// region gates.
pub const E2E_WIDTH: u32 = 256;
/// Fixed e2e window resolution height — see [`E2E_WIDTH`].
pub const E2E_HEIGHT: u32 = 256;

/// Render frames the driver counts in the `RUN` phase before requesting the
/// readback (`e2e-render-test.md` §3.3 / §4.1).
///
/// Comfortably above the resource-build latency (~3 frames: extract the world,
/// prepare GPU resources, first full graph execution) with margin for the
/// camera-history ring to spin up. With `synchronous_pipeline_compilation:
/// true` (`AppConfig::e2e`) every pipeline a render node queues is resolved to
/// `Ok`/`Err` the same frame it is queued — so by the time `RUN` ends every
/// render-graph pipeline has been **created**, which is exactly what the §3.1
/// `PipelineCache` scan needs (R3). If a future batch adds a pipeline still
/// `Queued` at the scan, the fix is bumping this const, not a redesign.
pub const E2E_RENDER_FRAMES: u32 = 8;

/// The fixed directory every run writes its readback screenshot PNG(s) into
/// (`e2e-render-test.md` Implementation log — 2026-05-14 screenshot-to-disk
/// addition). Relative to the worktree root (`cargo run` cwd); `target/` is
/// already gitignored and persists across runs. The directory is created on
/// demand. An orchestrator/agent can `Read` the PNGs here for visual analysis.
pub const E2E_SCREENSHOT_DIR: &str = "target/e2e-screenshots";

/// The stable filename of the final asserted readback frame inside
/// [`E2E_SCREENSHOT_DIR`] — overwritten every run, so the path is fixed and
/// documented. The harness reads back exactly one frame (the final asserted
/// frame), so this single file is the whole screenshot output.
pub const E2E_SCREENSHOT_LATEST: &str = "e2e_latest.png";

/// Max extra frames the driver waits in the `DRAIN` phase for the async
/// `Screenshot` capture to deliver (`e2e-render-test.md` §5.2, R2).
///
/// `Screenshot::primary_window()` capture is async — it "may not be available
/// immediately after the frame the component is spawned on". This bound is
/// generous precisely so a slow-but-working readback is not a false failure; if
/// the capture never arrives within it, *that* is a correct failure ("no
/// framebuffer produced — the render path never delivered a frame").
pub const E2E_DRAIN_FRAMES: u32 = 8;

// --- App wiring -----------------------------------------------------------

/// Wire the e2e-specific systems + resources into the app (called by
/// [`crate::build_app`] when `AppConfig::add_e2e_systems` is set).
///
/// This is the "e2e systems on" delta (`e2e-render-test.md` §2.2 point 2 & 4):
/// the `WinitSettings::game()`-style `Continuous`-in-both-modes update mode, the
/// bounded-frame driver + readback resources, and the fixed-pose camera spawn
/// (replacing the production `setup_camera`).
pub fn add_e2e_systems(app: &mut App) {
    // The cross-world `PipelineCache`-scan channel — inserted (cloned) into
    // *both* the main world and the `RenderApp` so the render-world scan system
    // writes and the main-world `ASSERT` step reads (`checks.rs` module docs).
    let pipeline_scan = PipelineScanResult::default();

    app
        // `UpdateMode::Continuous` in *both* focused and unfocused modes
        // (`e2e-render-test.md` §2.2 point 2, R8): the default `WinitSettings`
        // drops to `reactive_low_power` when the window loses focus, and a
        // `Reactive` mode only ticks on events — which would stall the bounded
        // frame loop if the e2e window never gains focus on a busy desktop.
        // `Continuous` guarantees the app ticks every frame regardless of
        // focus, so the fixed frame budget advances deterministically and the
        // run still terminates. This is `WinitSettings::game()`'s shape with
        // unfocused also `Continuous`.
        .insert_resource(WinitSettings {
            focused_mode: UpdateMode::Continuous,
            unfocused_mode: UpdateMode::Continuous,
        })
        .insert_resource(pipeline_scan.clone())
        .init_resource::<readback::E2eScreenshot>()
        .init_resource::<driver::E2eState>()
        .init_resource::<driver::E2eOutcome>()
        .add_systems(Startup, setup_e2e_camera)
        .add_systems(Update, driver::e2e_driver);

    // The render-world half of the pipeline scan: the scan system runs every
    // render frame in the `Render` schedule (after all the prepare/queue
    // systems have had their chance to queue pipelines) and writes the result
    // through the shared channel. `RenderApp` is present because `RenderPlugin`
    // (part of `DefaultPlugins`) builds it.
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .insert_resource(pipeline_scan)
            .add_systems(
                Render,
                scan_pipeline_errors_render_system.after(RenderSystems::Render),
            );
    }
}

/// Spawn the fixed-pose e2e camera — the determinism anchor
/// (`e2e-render-test.md` §4.2).
///
/// The same component set as the production `camera::setup_camera`, **minus
/// `FreeCamera`**: `FreeCameraPlugin` is omitted from the e2e config, so even
/// though the window is real and can receive focus/input, no system consumes
/// those events to move the camera — the `Transform` never changes.
/// `sync_position_split` (still added) is a pure function of the `Transform`,
/// so the `PositionSplit` is deterministic. The camera pose lives in
/// [`gates::e2e_camera_transform`] — the single named const all the
/// camera-pose-coupled gate rectangles are derived from (R5).
fn setup_e2e_camera(mut commands: Commands) {
    let start = gates::e2e_camera_transform();
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        start,
        // NAADF's int+frac camera-relative position (D1). Seeded from the
        // spawn `Transform`; `sync_position_split` keeps it in step each frame
        // (it never changes here — the pose is fixed).
        PositionSplit::from_world(start.translation),
        // The NAADF render path is compute + a fullscreen blit, not MSAA-
        // rasterised — keep MSAA off (matches `camera::setup_camera`).
        Msaa::Off,
    ));
}

// --- The entry point ------------------------------------------------------

/// Boot the bounded windowed e2e render test and return its `AppExit`
/// (`e2e-render-test.md` §2.4 / §9).
///
/// 1. `build_app(AppConfig::e2e())` — the real windowed app, four deliberate
///    e2e deltas.
/// 2. `app.run()` — the winit runner drives the loop. The bounded-frame driver
///    ([`driver::e2e_driver`]) counts the frame budget, shoots + drains the
///    screenshot, then at the `ASSERT` step runs **all three checks**: the
///    degenerate-frame floor + per-batch region gate, the node-dispatch check
///    (`DiagnosticsStore` is main-world), and the `PipelineCache` error scan
///    (read from the shared cross-world channel the render-world scan system
///    fills). It folds every failure into a single `AppExit::Success` /
///    `AppExit::error()`. The winit runner sees `should_exit()` and returns
///    that `AppExit`.
///
/// **Why not "post-`app.run()`":** `App::run()` `mem::replace`s the `App` with
/// an empty one and the winit runner consumes it — there is no `App` to inspect
/// afterwards. So every check runs *inside* the app (`e2e-render-test.md` §6.5's
/// alternative; see `checks.rs` module docs). The `AppExit` the runner returns
/// is therefore already the complete verdict.
///
/// The `PipelineCache` scan is the load-bearing check — it surfaces *all*
/// shader/pipeline/bind-group errors in a single run (it inspects every
/// pipeline after the frame budget), unlike the old live smoke-run that aborted
/// on the first bad shader.
pub fn run_e2e_render() -> AppExit {
    let mut app = crate::build_app(crate::AppConfig::e2e());

    // The winit runner drives the loop; the driver self-terminates after the
    // bounded frame budget, having run every check and written the verdict
    // `AppExit`. (A panic inside `app.update()` — a `DeviceLost`, a failed
    // `queue.submit` — propagates through the winit runner and aborts the
    // process non-zero with the wgpu message on stderr; that is also a correct
    // failure, `e2e-render-test.md` §3.2.)
    app.run()
}
