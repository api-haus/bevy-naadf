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

use bevy::camera::Hdr;
use bevy::core_pipeline::tonemapping::Tonemapping;
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

/// Render frames the driver counts in the `WARMUP` phase — static at the fixed
/// pose, before the camera starts moving (`e2e-render-test.md` §3.3 / §4.1).
///
/// Comfortably above the resource-build latency (~3 frames: extract the world,
/// prepare GPU resources, first full graph execution) with margin for the
/// camera-history ring to spin up. With `synchronous_pipeline_compilation:
/// true` (`AppConfig::e2e`) every pipeline a render node queues is resolved to
/// `Ok`/`Err` the same frame it is queued — so by the time `WARMUP` ends every
/// render-graph pipeline has been **created**, which is exactly what the §3.1
/// `PipelineCache` scan needs (R3). If a future batch adds a pipeline still
/// `Queued` at the scan, the fix is bumping this const, not a redesign.
///
/// **Phase B GI accumulation requirement (2026-05-15).** NAADF's compressed-
/// ReSTIR GI is a *temporal* algorithm: `renderGlobalIllum` writes lit/unlit
/// samples into the 128-frame `sample_counts` accumulation ring, and
/// `renderSampleRefine`'s `refineBuckets` has a hard gate
/// (`renderSampleRefine.fx:411` — `if (newValidCount + newInvalidCount < 12)
/// curCompressedIndex = 0`) that zeros a bucket's compressed output until it
/// has accumulated ≥12 samples across the temporal window. With the original
/// 8-frame budget the ring barely filled — buckets never reached the 12-sample
/// threshold, `valid_samples_compressed` stayed empty, and
/// `renderSpatialResampling`'s reservoir loop found `bucket_valid_stored == 0`
/// for every bucket ⇒ NO indirect GI bounce composited into `final_color` (only
/// the negligible independent sun sample survived). The GI pipeline is a
/// faithful port — it simply needs the frame budget a temporal-accumulation
/// renderer requires. 96 frames is comfortably past the up-to-64-frame
/// `computeValidHistory` ring-capacity window, so the buckets fully populate
/// and the GI bounce converges to a stable, visible result.
pub const E2E_WARMUP_FRAMES: u32 = 96;

/// Render frames the driver counts in the `MOTION` phase — the camera sweeps a
/// deterministic open path ([`gates::e2e_orbit_camera_transform`]) from the
/// motion-start pose to the fixed readback pose, exercising the TAA
/// camera-motion reprojection (`10-impl-b.md` — TAA shadow decay-to-black
/// coverage).
///
/// 48 frames over the full ~95°-yaw + large radius/height sweep is a brisk
/// ~2°/frame camera move with a substantial per-frame translation step — every
/// frame is a real, demanding reprojection step (the regime the TAA
/// reprojection path must hold the GI bounce through), while still being slow
/// enough that the recent-frame history overlaps the current frame (so the
/// reprojection genuinely *runs* rather than every sample disoccluding off
/// screen). The path is open: frame 48 lands exactly on the fixed readback
/// pose, which the camera has never been static at.
pub const E2E_MOTION_FRAMES: u32 = 48;

/// Render frames the driver counts in the `SETTLE` phase — static at the fixed
/// readback pose, immediately after the camera stops moving.
///
/// Kept at the **bare minimum (1 frame)** on purpose. The readback pose is one
/// the camera has never been static at; a faithful TAA reprojection has carried
/// the GI bounce here through the motion and the shadowed/indirect regions are
/// still GI-lit, a broken reprojection has decayed them to black during the
/// move and they are still black. The decay must be caught *immediately* after
/// the motion — every extra static frame lets the static-camera running average
/// re-converge from same-pose `taa_samples` and washes the regression out (the
/// same masking trap a long settle / a closed orbit / an eased path has). 1
/// frame is just enough to guarantee the camera is cleanly at the `t == 1`
/// readback pose for one rendered frame before `SHOOT` captures it.
pub const E2E_SETTLE_FRAMES: u32 = 1;

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

// --- Resize-test constants (`docs/orchestrate/taa-resize-blackness/`) ------
//
// The resize-test wait spans are specified by the user in WALL-CLOCK seconds:
// **5 seconds settle between each major step** (post-launch, post-togglefloating,
// post-resize). The bounded e2e driver has no wall clock; it only counts
// `Update` ticks. With vsync the present mode defaults to FIFO, so on a 60 Hz
// display the harness ticks at ~60 fps and the conversion below gives the
// user's spec exactly: 5 seconds × 60 fps = 300 ticks. On other refresh rates
// the constants approximate the spec rather than match it.

/// Render frames the driver counts before triggering the resize-prep hyprctl
/// dispatches — the user's first 5-second settle leg. At 60 fps ≈ 5.0 s. Long
/// enough that the TAA 32-deep ring and the GI 128-frame `sample_counts`
/// accumulator are meaningfully filled before they get zero-cleared.
/// **60 fps assumption** — see module comment above.
pub const E2E_RESIZE_PRE_FRAMES: u32 = 300;

/// Render frames the driver counts after the `hyprctl togglefloating`
/// dispatch, before the actual resize. The user's "at least 5 seconds to
/// settle in-between" requirement: lets the compositor finish unmapping the
/// tiled window and remapping it as a floating window before we ask for a
/// pixel-precise resize. 300 frames ≈ 5.0 s at 60 fps.
pub const E2E_RESIZE_FLOAT_SETTLE_FRAMES: u32 = 300;

/// Render frames the driver counts after triggering the resize, before the
/// post-resize screenshot. The user's third 5-second settle leg. 300 frames
/// ≈ 5.0 s at 60 fps. (Earlier 30-frame value was an attempt to catch the
/// drain near the bottom of the user-observed recovery window; for this
/// dispatch we settle conservatively first and let the assert decide.)
pub const E2E_RESIZE_POST_FRAMES: u32 = 300;

/// Post-resize physical resolution — picks an aspect-changing target so the
/// resize exercises the GI/TAA pixel-count buffer-recreation path AND a
/// non-trivial aspect ratio change (256×256 → 384×288 is 1:1 → 4:3, +50%
/// width / +12.5% height; matches `02-design.md` §A.3).
pub const E2E_RESIZE_WIDTH: u32 = 384;
/// Post-resize physical height — see [`E2E_RESIZE_WIDTH`].
pub const E2E_RESIZE_HEIGHT: u32 = 288;

/// The minimum post/pre luma ratio at which the resize-test passes. Healthy
/// shadow-band luma is ~242 on both screenshots (ratio ≈ 1.0); the broken
/// regime collapses post-resize to ~4 (ratio ≈ 0.017). 0.5 is the threshold:
/// well above the broken regime, well below a steady-state healthy run, so
/// the gate has massive headroom in both directions.
pub const E2E_RESIZE_MIN_LUMA_RATIO: f32 = 0.5;

/// Filenames for the two screenshots saved by the resize-test (alongside
/// [`E2E_SCREENSHOT_LATEST`] inside [`E2E_SCREENSHOT_DIR`]).
pub const E2E_RESIZE_PRE_PNG: &str = "resize_pre.png";
pub const E2E_RESIZE_POST_PNG: &str = "resize_post.png";

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
        .init_resource::<driver::ResizeTestState>()
        .add_systems(Startup, setup_e2e_camera)
        // The driver owns the deterministic camera motion — it writes the
        // camera `Transform` + `PositionSplit` during the `MOTION` / `SETTLE`
        // phases. It must run *before* `sync_position_split` (and thus before
        // `update_camera_history`, which is `.after(sync_position_split)`) so
        // this frame's first-hit / TAA-reproject / camera-history all see the
        // new pose the same frame the driver sets it — no one-frame lag
        // between the camera rotation and its `PositionSplit` origin.
        .add_systems(Update, driver::e2e_driver.before(crate::camera::sync_position_split));

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

/// Spawn the e2e camera at the motion-path *start* pose
/// (`e2e-render-test.md` §4.2).
///
/// The same component set as the production `camera::setup_camera`, **minus
/// `FreeCamera`**: `FreeCameraPlugin` is omitted from the e2e config, so no
/// *input* moves the camera. The camera is **not** static, though — the
/// bounded-frame [`driver::e2e_driver`] drives a deterministic camera move
/// during its `MOTION` phase (writing the `Transform` + `PositionSplit` itself,
/// as a pure function of the phase progress), so the run exercises the TAA
/// camera-motion reprojection.
///
/// The camera is spawned at [`gates::e2e_motion_start_transform`] (the `t == 0`
/// endpoint). The `WARMUP` phase renders here; the `MOTION` phase sweeps on an
/// **open** path to [`gates::e2e_camera_transform`] (the `t == 1` readback
/// pose); `SETTLE` + the readback happen there. The readback pose is therefore
/// one the camera has *never been static at* — every GI/TAA history sample in
/// the readback frame had to come through the camera-motion reprojection (see
/// [`gates::e2e_orbit_camera_transform`]'s open-path rationale). All the
/// camera-pose-coupled gate rectangles are derived from
/// [`gates::e2e_camera_transform`] — the readback pose, so they stay valid.
/// `sync_position_split` (still added) is a pure function of the `Transform`,
/// so the `PositionSplit` is deterministic whichever phase the driver is in.
fn setup_e2e_camera(mut commands: Commands) {
    let start = gates::e2e_motion_start_transform();
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        // `Hdr` + the `Tonemapping` component below — matches
        // `camera::setup_camera`. The NAADF blit writes raw linear HDR into the
        // `Rgba16Float` view target; Bevy's `tonemapping` node does the
        // tonemap. The e2e screenshot reads the post-tonemapping window
        // surface, so the e2e gates see the Bevy-tonemapped image
        // (`18-taa-fidelity.md` fix #2 — the e2e gates were recalibrated for
        // it).
        Hdr,
        // Bevy's built-in tonemapper — `TonyMcMapface` (Bevy's default).
        Tonemapping::default(),
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
    let app = crate::build_app(crate::AppConfig::e2e());
    run_with_app(app)
}

/// Phase-C wave-3 — run an already-built [`App`] through the windowed e2e
/// runner. Lets callers customise `AppArgs` before booting (the
/// `--entities` mode in `e2e_render` does this to flip
/// `entities_enabled = true` + `spawn_test_entity = true`).
pub fn run_with_app(mut app: bevy::prelude::App) -> AppExit {
    app.run()
}
