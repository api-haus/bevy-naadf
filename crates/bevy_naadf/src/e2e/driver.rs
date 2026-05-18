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
use super::framebuffer::{Framebuffer, Rect};
use super::gates::{
    batch_gate, e2e_orbit_camera_transform, e2e_resize_test_camera_transform, expected_spans,
    region_luminance_report, GateState, CURRENT_BATCH,
};
use super::readback::{shoot_primary_window, E2eScreenshot};
use super::{
    E2E_DRAIN_FRAMES, E2E_MOTION_FRAMES, E2E_RESIZE_A_HEIGHT, E2E_RESIZE_A_PNG, E2E_RESIZE_A_WIDTH,
    E2E_RESIZE_B_HEIGHT, E2E_RESIZE_B_PNG, E2E_RESIZE_B_WIDTH, E2E_RESIZE_INITIAL_PNG,
    E2E_RESIZE_LAUNCH_SETTLE_FRAMES, E2E_RESIZE_MIN_LUMA_RATIO, E2E_RESIZE_WAIT_FRAMES,
    E2E_SCREENSHOT_DIR, E2E_SCREENSHOT_LATEST, E2E_SETTLE_FRAMES, E2E_WARMUP_FRAMES,
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
    // --- Resize-test phases
    // (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
    //  `## GI-bounce-on-resize fix (2026-05-16)`) ---------------------------
    //
    // Selected when `AppArgs.resize_test == true`. The Warmup branch routes
    // straight into LaunchSettle on tick 0 instead of the production
    // Warmup→Motion→Settle→Shoot→Drain→Assert flow. The camera is pinned at
    // the resize-test pose (see [`super::gates::e2e_resize_test_camera_transform`])
    // — a low-angle, shadow-heavy framing — for the entire sequence.
    //
    // Three-step resize sequence: boot at 800×600, resize to 1920×1080,
    // resize to 2000×1000. Each step waits ~5 s before the screenshot.
    /// 5-second post-launch settle — lets the TAA 32-deep ring and GI
    /// 128-frame `sample_counts` accumulator fill before the first capture.
    LaunchSettle,
    /// Spawn `Screenshot::primary_window()` for the **initial 800×600** capture.
    ShootInitial,
    /// Wait (bounded) for the initial capture to deliver, then stash its
    /// framebuffer into `ResizeTestState.initial`.
    DrainInitial,
    /// One-shot: ask Hyprland to resize our window to 1920×1080 via
    /// `hyprctl dispatch resizewindowpixel`. The resulting Wayland resize
    /// event propagates through `bevy_winit`, the GPU surface is
    /// reconfigured, and `prepare_taa` / `prepare_gi` re-allocate + zero-clear
    /// the rings.
    ResizeA,
    /// 5-second post-resize settle after the resize to 1920×1080.
    WaitA,
    /// Spawn `Screenshot::primary_window()` for the post-resize-A capture.
    ShootA,
    /// Wait (bounded) for the post-resize-A capture to deliver, then stash
    /// it into `ResizeTestState.after_resize_a`.
    DrainA,
    /// One-shot: ask Hyprland to resize our window to 2000×1000.
    ResizeB,
    /// 5-second post-resize settle after the resize to 2000×1000.
    WaitB,
    /// Spawn `Screenshot::primary_window()` for the post-resize-B capture.
    ShootB,
    /// Wait (bounded) for the post-resize-B capture to deliver, then stash
    /// it into `ResizeTestState.after_resize_b`.
    DrainB,
    /// Compare the three luma values, fail (and write `AppExit::error()`) if
    /// either post-resize capture's full-frame luma falls below the threshold
    /// ratio vs the initial.
    ResizeAssert,
    // --- Oasis-edit-visual phases
    // (`crate::e2e::oasis_edit_visual` — `02f-followup`,
    //  the visual-diff edit-pipeline gate) -----------------------------------
    //
    // Selected when `AppArgs.oasis_edit_visual_mode == true`. The Warmup
    // branch routes straight into OasisWarmup on tick 0. The camera is pinned
    // birdseye over the loaded Oasis VOX scene's world centre for the entire
    // sequence; `pin_oasis_camera` overrides whatever pose the standard
    // driver writes.
    /// Warmup at the birdseye pose — let TAA + GI converge before the first
    /// screenshot.
    OasisWarmup,
    /// Spawn `Screenshot::primary_window()` for the **pre-edit** capture
    /// (frame A).
    OasisShootBefore,
    /// Wait (bounded) for the pre-edit capture to deliver, then stash its
    /// framebuffer into `OasisEditVisualState.before`.
    OasisDrainBefore,
    /// One-shot tick: invoke
    /// [`crate::editor::tools::sphere_brush`] with `is_erase = true` at the
    /// world centre via a deferred `Commands`-spawned system. After this
    /// tick the brush has fired exactly once.
    OasisApplyEdit,
    /// Wait `OASIS_POST_EDIT_WAIT_FRAMES` ticks (~5 s) — the W2 GPU
    /// dispatch propagates + W3 regime-2 background AADF chain converges +
    /// TAA / GI re-stabilise around the new geometry.
    OasisWaitPostEdit,
    /// Spawn `Screenshot::primary_window()` for the **post-edit** capture
    /// (frame B).
    OasisShootAfter,
    /// Wait (bounded) for the post-edit capture to deliver, then stash into
    /// `OasisEditVisualState.after`.
    OasisDrainAfter,
    /// Run [`super::oasis_edit_visual::assert_visual_edit_landed`], save
    /// both PNGs, write `AppExit::Success` / `AppExit::error()`.
    OasisAssert,
    // --- Small-edit-visual phases (`03g` — single-voxel edit gate) -----------
    //
    // Selected when `AppArgs.small_edit_visual_mode == true`. The Warmup
    // branch routes into SmallEditWarmup on tick 0. The camera is pinned
    // birdseye over the default-grid world centre by
    // `small_edit_visual::pin_small_edit_camera`.
    /// Birdseye warmup before snapshot A — TAA + GI convergence.
    SmallEditWarmup,
    /// Spawn `Screenshot::primary_window()` for snapshot A.
    SmallEditShootBefore,
    /// Wait (bounded) for snapshot A; on arrival count non-empty voxels
    /// (the CPU pre-condition) and stash into `SmallEditVisualState.before`.
    SmallEditDrainBefore,
    /// One-shot: invoke `cube_brush(radius=1.0)` at the configured click
    /// voxel via the runtime path.
    SmallEditApply,
    /// Wait `SMALL_EDIT_POST_EDIT_WAIT_FRAMES` ticks for W2 + W3 + TAA + GI.
    SmallEditWaitPostEdit,
    /// Spawn `Screenshot::primary_window()` for snapshot B.
    SmallEditShootAfter,
    /// Wait (bounded) for snapshot B; stash into `SmallEditVisualState.after`.
    SmallEditDrainAfter,
    /// Run `assert_small_edit_landed`, save PNGs, write the verdict.
    SmallEditAssert,
    // --- Small-edit-repro phases (2026-05-17 — user-captured Oasis repro) ---
    /// Camera-pinned warmup at the user's pose — TAA + GI convergence.
    SmallEditReproWarmup,
    /// Spawn `Screenshot::primary_window()` for snapshot A.
    SmallEditReproShootBefore,
    /// Drain snapshot A.
    SmallEditReproDrainBefore,
    /// One-shot: invoke `cube_brush(radius=1, pos=…, ty=…)` via runtime path.
    SmallEditReproApply,
    /// Wait `SMALL_EDIT_REPRO_POST_EDIT_WAIT_FRAMES` ticks for W2 + W3 + TAA + GI.
    SmallEditReproWaitPostEdit,
    /// Spawn `Screenshot::primary_window()` for snapshot B.
    SmallEditReproShootAfter,
    /// Drain snapshot B.
    SmallEditReproDrainAfter,
    /// Run `assert_no_pitch_black_pixels`, save PNGs, write the verdict.
    SmallEditReproAssert,
    // --- vox-gpu-oracle phases (Stage 4 — per-pixel CPU oracle vs GPU
    // gate; Stage 14 — SSIM-based comparison restoring real dual-capture) ---
    //
    // Selected when `AppArgs.vox_gpu_oracle_cpu_phase == true` OR
    // `AppArgs.vox_gpu_oracle_gpu_phase == true`. Single-screenshot
    // fast-path: warmup → shoot → drain → save → exit. The driver picks
    // the destination filename (`oracle_cpu.png` for the CPU phase,
    // `oracle_gpu.png` for the GPU phase). No edit phase, no Δ assertion —
    // the compare happens out-of-process in
    // `vox_gpu_oracle::run_vox_gpu_oracle_compare`, which loads both PNGs
    // and runs the SSIM (Structural Similarity Index) compare. SSIM
    // tolerates the renderer's inherent stochastic GI/TAA shimmer + the
    // GPU atomic-cursor nondeterminism + the install-path world-shape
    // divergence (natural-bound CPU vs fixed-tiled GPU) while still
    // dropping far below the threshold on gross regressions (sky-bleed,
    // dropouts, voxel-type corruption, palette OOB).
    //
    // Stage 13's Shape-C tautology (save same captured framebuffer as
    // both `oracle_cpu.png` and `oracle_gpu.png`) is reverted — that
    // shape made the gate catch nothing.
    /// Warmup at the shared oracle pose — TAA + GI convergence.
    VoxGpuOracleWarmup,
    /// Spawn `Screenshot::primary_window()`.
    VoxGpuOracleShoot,
    /// Drain the async capture, decode to a [`Framebuffer`], save the PNG to
    /// disk (`oracle_cpu.png` for the CPU phase, `oracle_gpu.png` for the GPU
    /// phase), then write `AppExit::Success`.
    VoxGpuOracleDrain,
    /// PBR-raymarching visual gate — warmup at the side-on metallic-pillar
    /// pose; converge TAA/GI.
    PbrVisualWarmup,
    /// Spawn `Screenshot::primary_window()` for the PBR baseline frame.
    PbrVisualShoot,
    /// Drain the async PBR-visual capture, save the PNG to
    /// `pbr_visual_baseline.png`, run the three PBR assertions, exit.
    PbrVisualDrain,
    /// PBR rendering-debugger gate — initial warmup (TAA/GI convergence).
    PbrDebugModesWarmup,
    /// Set the next debug mode, then settle for a few frames.
    PbrDebugModesSettle,
    /// Spawn `Screenshot::primary_window()` for the current debug mode.
    PbrDebugModesShoot,
    /// Drain the per-mode async capture; stash the framebuffer; advance to
    /// the next mode or to `PbrDebugModesAssert` when all modes are done.
    PbrDebugModesDrain,
    /// Walk every captured framebuffer; assert non-degeneracy; exit.
    PbrDebugModesAssert,
    /// PBR splotch-artifact gate — warmup at the metallic-pillar pose;
    /// converge TAA/GI before capturing.
    PbrHardEdgeWarmup,
    /// Spawn `Screenshot::primary_window()` for the splotch-rect frame.
    PbrHardEdgeShoot,
    /// Drain the async capture, save the PNG, run the hard-edge assertion,
    /// exit.
    PbrHardEdgeDrain,
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

/// Stash for the three framebuffers captured by the resize-test
/// (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
/// `## GI-bounce-on-resize fix (2026-05-16)`).
///
/// The driver's `DrainInitial` / `DrainA` / `DrainB` phases each consume the
/// shared [`E2eScreenshot`] resource — decode the `Image` to a CPU-side
/// [`Framebuffer`], dump the resource back to `None`, and stash the decoded
/// frame here. The `ResizeAssert` phase then compares the two post-resize
/// captures' full-frame luma against the initial.
#[derive(Resource, Default)]
pub struct ResizeTestState {
    /// 800×600 capture, before any resize.
    pub initial: Option<Framebuffer>,
    /// Capture after the first resize (1920×1080).
    pub after_resize_a: Option<Framebuffer>,
    /// Capture after the second resize (2000×1000).
    pub after_resize_b: Option<Framebuffer>,
}

/// Pin the camera to the resize-test pose ([`e2e_resize_test_camera_transform`])
/// — same pose for every resize-test phase (no orbit motion), so any luma
/// difference between the three captures is attributable to the resize, not
/// the camera.
fn pin_resize_test_camera(
    camera: &mut Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let pose = e2e_resize_test_camera_transform();
    let (transform, position_split) = &mut **camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
}

/// Send a `hyprctl dispatch resizewindowpixel exact W H,class:e2e_render`
/// asking the compositor to resize our window to `(width, height)` physical
/// pixels. Also dumps `hyprctl clients -j` immediately before and after so
/// the log captures the floating/size state at each transition. `label` is
/// a tag for the log line (e.g. "A" or "B").
fn dispatch_hyprctl_resize(label: &str, width: u32, height: u32) {
    let before = std::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .ok();
    println!(
        "e2e_render: resize-test {label} hyprctl clients (pre-resize): {}",
        summarise_clients_for_e2e_render(before.as_ref())
    );

    let selector = hyprctl_window_selector();
    let dispatch_arg = format!("exact {width} {height},{selector}");
    // test-only: hyprctl-driven Wayland resize
    let output = std::process::Command::new("hyprctl")
        .args(["dispatch", "resizewindowpixel", &dispatch_arg])
        .output();
    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            println!(
                "e2e_render: resize-test {label} hyprctl resizewindowpixel \
                 '{dispatch_arg}' -> exit {:?} stdout={stdout:?} stderr={stderr:?}",
                o.status
            );
        }
        Err(e) => eprintln!(
            "e2e_render: resize-test {label} hyprctl resizewindowpixel \
             '{dispatch_arg}' FAILED to spawn: {e} — test will report \
             failure via luma comparison"
        ),
    }

    let after = std::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .ok();
    println!(
        "e2e_render: resize-test {label} hyprctl clients (post-resize): {}",
        summarise_clients_for_e2e_render(after.as_ref())
    );
}

/// Pick the Hyprland window-selector string for our primary window.
///
/// Strategy: use `class:e2e_render` — Hyprland's `class:` selector matches
/// the Wayland `app_id` / X11 `WM_CLASS`, which `bevy_winit` defaults to the
/// binary name when no `Window.name` is set in [`crate::WindowConfig`]. The
/// e2e binary is named `e2e_render` (see `crates/bevy_naadf/src/bin/e2e_render.rs`),
/// so the default `app_id` is `e2e_render`. This per the dispatch brief's
/// directive (resize-blackness e2e: see
/// `docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
/// `## GI-bounce-on-resize fix (2026-05-16)`).
///
/// The selector is returned without its leading `,` separator. The caller
/// prepends `,` when building the full hyprctl dispatch argument
/// (`<resize-args>,<selector>` or `,<selector>` for togglefloating).
///
/// Only called from the resize-test phases (`ResizeA` / `ResizeB`), which
/// are gated behind `AppArgs.resize_test` — the default e2e harness never
/// shells out to hyprctl.
fn hyprctl_window_selector() -> String {
    "class:e2e_render".to_string()
}

/// Pull the JSON object describing the `e2e_render`-class window out of a
/// `hyprctl clients -j` output blob and return it as a `(floating, size, at)`
/// triple-style string for logging.
///
/// `hyprctl clients -j` prints a JSON array; each element is one window. We
/// don't pull a JSON dep in for a diagnostic — a forward scan from `"class":
/// "e2e_render"` back to the enclosing `{` and forward to the matching `}` is
/// enough to extract just our window's record. If the parse fails we fall back
/// to a length-capped raw substring so the diagnostic still carries signal.
fn summarise_clients_for_e2e_render(out: Option<&std::process::Output>) -> String {
    let Some(o) = out else {
        return "<hyprctl clients spawn failed>".to_string();
    };
    let stdout = String::from_utf8_lossy(&o.stdout);
    let needle = "\"class\": \"e2e_render\"";
    let Some(class_pos) = stdout.find(needle) else {
        return format!(
            "<no e2e_render entry in hyprctl clients output> exit={:?} stdout_len={}",
            o.status,
            stdout.len()
        );
    };
    // Walk backwards from `class_pos` to find the enclosing window record's
    // opening `{` — NOT the nearest `{` (the window record has nested objects
    // like `workspace: { id, name }`, so the nearest preceding `{` lands inside
    // that nested object). Count brace depth from `class_pos` backwards: every
    // `}` increases depth (we're entering a nested closed object) and every
    // `{` decreases it. The first `{` that drops depth below 0 is the outer
    // record's opener.
    let bytes = stdout.as_bytes();
    let mut depth = 0i32;
    let mut start = 0usize;
    for i in (0..class_pos).rev() {
        match bytes[i] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    start = i;
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    // Walk forwards from `start` to find the matching `}` by counting depth.
    let mut depth = 0i32;
    let mut end = stdout.len();
    for (i, ch) in stdout[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    let record = &stdout[start..end.min(stdout.len())];
    // Cap output at ~2000 chars so the log line stays readable.
    if record.len() > 2000 {
        format!("{}…<truncated>", &record[..2000])
    } else {
        record.to_string()
    }
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
    mut resize_test: ResMut<ResizeTestState>,
    mut oasis: ResMut<super::oasis_edit_visual::OasisEditVisualState>,
    mut small_edit: ResMut<super::small_edit_visual::SmallEditVisualState>,
    mut small_edit_repro: ResMut<super::small_edit_repro::SmallEditReproState>,
    mut vox_gpu_oracle: ResMut<super::vox_gpu_oracle::VoxGpuOracleState>,
    mut pbr_visual: ResMut<super::pbr_visual::PbrVisualState>,
    world_data: Option<ResMut<crate::world::data::WorldData>>,
    diagnostics: Res<DiagnosticsStore>,
    pipeline_scan: Res<PipelineScanResult>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    app_args: Option<Res<crate::AppArgs>>,
) {
    // Resize-test fast-path: at the very first Warmup tick, if --resize-test
    // is set, branch into the resize-test state machine entirely (skips the
    // standard Warmup→Motion→Settle→Shoot→Drain→Assert flow). All assertions
    // are inside the resize-test phases — the standard run_assertions /
    // batch_gate path does NOT run for resize-test runs.
    let resize_test_mode = app_args.as_deref().is_some_and(|a| a.resize_test);
    if resize_test_mode && state.phase == E2ePhase::Warmup && state.phase_ticks == 0 {
        // Hyprland-only gate. The resize-test triggers the real Wayland
        // resize chain via `hyprctl dispatch resizewindowpixel`, which only
        // exists on Hyprland. Bail loudly rather than wasting 5 s ticking
        // through the test on the wrong compositor and reporting a
        // misleading "test passed" result.
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
            let err = "resize-test requires Hyprland — HYPRLAND_INSTANCE_SIGNATURE \
                       env var is not set. Aborting (the test mechanism is hyprctl-driven; \
                       see docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md \
                       `## GI-bounce-on-resize fix (2026-05-16)`).".to_string();
            eprintln!("e2e_render: FAIL — {err}");
            outcome.gate_result = Some(Err(err));
            exit.write(AppExit::error());
            state.phase = E2ePhase::Done;
            return;
        }
        state.phase = E2ePhase::LaunchSettle;
        state.phase_ticks = 0;
    }

    // `02f-followup` — oasis-edit-visual fast-path. Routes the driver into
    // the alternate state machine on tick 0 when `AppArgs.oasis_edit_visual_mode`
    // is set. The camera pose is overwritten every tick by
    // `super::oasis_edit_visual::pin_oasis_camera` (Update system,
    // `.after(e2e_driver)`), so whatever pose this driver writes is harmless.
    //
    // `vox-gpu-rewrite W5.3-fix Stage 1` — the vox-gpu-construction gate
    // reuses the Oasis warm/shoot/edit/wait/shoot/assert flow but pins the
    // camera to C# `(500, 200, 40)` via `pin_vox_gpu_construction_camera`
    // (runs `.after(pin_oasis_camera)`) and substitutes the brush call at
    // `OasisApplyEdit`. The flag is OR'd into the Oasis route-in trigger.
    let oasis_mode = app_args
        .as_deref()
        .is_some_and(|a| a.oasis_edit_visual_mode);
    let vox_gpu_construction_mode = app_args
        .as_deref()
        .is_some_and(|a| a.vox_gpu_construction_mode);
    if (oasis_mode || vox_gpu_construction_mode)
        && state.phase == E2ePhase::Warmup
        && state.phase_ticks == 0
    {
        state.phase = E2ePhase::OasisWarmup;
        state.phase_ticks = 0;
    }

    // `03g` — small-edit-visual fast-path. Routes into SmallEditWarmup on
    // tick 0 when the flag is set. Camera pose owned by
    // `super::small_edit_visual::pin_small_edit_camera`.
    let small_edit_mode = app_args
        .as_deref()
        .is_some_and(|a| a.small_edit_visual_mode);
    if small_edit_mode && state.phase == E2ePhase::Warmup && state.phase_ticks == 0 {
        state.phase = E2ePhase::SmallEditWarmup;
        state.phase_ticks = 0;
    }

    // 2026-05-17 — small-edit-repro fast-path (user-captured Oasis click).
    // Routes into SmallEditReproWarmup on tick 0. Camera pose owned by
    // `super::small_edit_repro::pin_small_edit_repro_camera`.
    let small_edit_repro_mode = app_args
        .as_deref()
        .is_some_and(|a| a.small_edit_repro_mode);
    if small_edit_repro_mode && state.phase == E2ePhase::Warmup && state.phase_ticks == 0 {
        state.phase = E2ePhase::SmallEditReproWarmup;
        state.phase_ticks = 0;
    }

    // vox-gpu-rewrite W5.3-fix Stage 4 — vox-gpu-oracle fast-path. Routes
    // into VoxGpuOracleWarmup on tick 0 when either oracle phase flag is set.
    // Camera pose is owned by
    // `super::vox_gpu_oracle::pin_vox_gpu_oracle_camera`.
    let vox_gpu_oracle_mode = app_args.as_deref().is_some_and(|a| {
        a.vox_gpu_oracle_cpu_phase || a.vox_gpu_oracle_gpu_phase
    });
    if vox_gpu_oracle_mode && state.phase == E2ePhase::Warmup && state.phase_ticks == 0 {
        state.phase = E2ePhase::VoxGpuOracleWarmup;
        state.phase_ticks = 0;
    }

    // PBR-raymarching `--pbr-visual` fast-path. Routes into PbrVisualWarmup
    // on tick 0 when the flag is set. Camera pose owned by
    // `super::pbr_visual::pin_pbr_visual_camera`.
    let pbr_visual_mode = app_args
        .as_deref()
        .is_some_and(|a| a.pbr_visual_mode);
    if pbr_visual_mode && state.phase == E2ePhase::Warmup && state.phase_ticks == 0 {
        state.phase = E2ePhase::PbrVisualWarmup;
        state.phase_ticks = 0;
    }

    // PBR rendering-debugger `--pbr-debug-modes` fast-path. Routes into
    // PbrDebugModesWarmup on tick 0 when the flag is set. Camera pose owned
    // by `super::pbr_debug_modes::pin_pbr_debug_modes_camera`. See
    // `docs/orchestrate/pbr-raymarching/05-diagnostic.md` § "PBR rendering
    // debugger".
    let pbr_debug_modes_mode = app_args
        .as_deref()
        .is_some_and(|a| a.pbr_debug_modes_mode);
    if pbr_debug_modes_mode && state.phase == E2ePhase::Warmup && state.phase_ticks == 0 {
        state.phase = E2ePhase::PbrDebugModesWarmup;
        state.phase_ticks = 0;
    }

    // PBR splotch-artifact `--pbr-hard-edge` fast-path. Routes into
    // PbrHardEdgeWarmup on tick 0 when the flag is set. Camera pose owned
    // by `super::pbr_hard_edge::pin_pbr_hard_edge_camera`. See
    // `docs/orchestrate/pbr-raymarching/05-diagnostic.md` § "LIGHT
    // INTEGRATION splotch diagnose+fix (post-`46e50cd`)".
    let pbr_hard_edge_mode = app_args
        .as_deref()
        .is_some_and(|a| a.pbr_hard_edge_mode);
    if pbr_hard_edge_mode && state.phase == E2ePhase::Warmup && state.phase_ticks == 0 {
        state.phase = E2ePhase::PbrHardEdgeWarmup;
        state.phase_ticks = 0;
    }

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
            // vox-e2e mode — swap the default-scene `assert_batch_6` region
            // gate for the `assert_vox_geometry_visible` non-skybox gate.
            // The default-scene gate rects (`solid_block_rect`,
            // `emissive_rect`) sample voxels in `voxel/grid.rs`'s hardcoded
            // test grid and don't apply when a `.vox` file is loaded
            // (`crate::e2e::vox_e2e`).
            let vox_e2e_mode =
                app_args.as_deref().is_some_and(|a| a.vox_e2e_mode);
            let result = run_assertions(
                screenshot.as_mut(),
                &diagnostics,
                &pipeline_scan,
                entities_mode,
                vox_e2e_mode,
            );
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
        // ---- Resize-test phases ------------------------------------------
        E2ePhase::LaunchSettle => {
            // Pin the camera at the resize-test pose — a low-angle,
            // shadow-heavy framing where the bug (post-resize TAA/GI ring
            // drain → shadow regions go black) is observable in full-frame
            // luma. Pose held identical through every resize-test phase so
            // any luma drop between the three captures is attributable to
            // the resize itself, not camera motion.
            pin_resize_test_camera(&mut camera);
            state.phase_ticks += 1;
            if state.phase_ticks >= E2E_RESIZE_LAUNCH_SETTLE_FRAMES {
                // Drop any prior capture before requesting the initial one.
                screenshot.0 = None;
                state.phase = E2ePhase::ShootInitial;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::ShootInitial => {
            pin_resize_test_camera(&mut camera);
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::DrainInitial;
            state.phase_ticks = 0;
        }
        E2ePhase::DrainInitial => {
            pin_resize_test_camera(&mut camera);
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render: resize-test initial capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        resize_test.initial = Some(fb);
                        state.phase = E2ePhase::ResizeA;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "resize-test: initial framebuffer decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks >= E2E_DRAIN_FRAMES {
                let err = format!(
                    "resize-test: initial screenshot never delivered within \
                     {E2E_DRAIN_FRAMES} drain frames"
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::ResizeA => {
            pin_resize_test_camera(&mut camera);
            dispatch_hyprctl_resize("A", E2E_RESIZE_A_WIDTH, E2E_RESIZE_A_HEIGHT);
            state.phase = E2ePhase::WaitA;
            state.phase_ticks = 0;
        }
        E2ePhase::WaitA => {
            pin_resize_test_camera(&mut camera);
            state.phase_ticks += 1;
            if state.phase_ticks >= E2E_RESIZE_WAIT_FRAMES {
                screenshot.0 = None;
                state.phase = E2ePhase::ShootA;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::ShootA => {
            pin_resize_test_camera(&mut camera);
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::DrainA;
            state.phase_ticks = 0;
        }
        E2ePhase::DrainA => {
            pin_resize_test_camera(&mut camera);
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render: resize-test after_resize_a capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        resize_test.after_resize_a = Some(fb);
                        state.phase = E2ePhase::ResizeB;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "resize-test: after_resize_a framebuffer decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks >= E2E_DRAIN_FRAMES {
                let err = format!(
                    "resize-test: after_resize_a screenshot never delivered within \
                     {E2E_DRAIN_FRAMES} drain frames"
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::ResizeB => {
            pin_resize_test_camera(&mut camera);
            dispatch_hyprctl_resize("B", E2E_RESIZE_B_WIDTH, E2E_RESIZE_B_HEIGHT);
            state.phase = E2ePhase::WaitB;
            state.phase_ticks = 0;
        }
        E2ePhase::WaitB => {
            pin_resize_test_camera(&mut camera);
            state.phase_ticks += 1;
            if state.phase_ticks >= E2E_RESIZE_WAIT_FRAMES {
                screenshot.0 = None;
                state.phase = E2ePhase::ShootB;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::ShootB => {
            pin_resize_test_camera(&mut camera);
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::DrainB;
            state.phase_ticks = 0;
        }
        E2ePhase::DrainB => {
            pin_resize_test_camera(&mut camera);
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render: resize-test after_resize_b capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        resize_test.after_resize_b = Some(fb);
                        state.phase = E2ePhase::ResizeAssert;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "resize-test: after_resize_b framebuffer decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks >= E2E_DRAIN_FRAMES {
                let err = format!(
                    "resize-test: after_resize_b screenshot never delivered within \
                     {E2E_DRAIN_FRAMES} drain frames"
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::ResizeAssert => {
            let result = run_resize_test_assertions(resize_test.as_mut());
            match &result {
                Ok(()) => {
                    println!(
                        "e2e_render: resize-test PASS — both post-resize / initial luma ratios \
                         above threshold {E2E_RESIZE_MIN_LUMA_RATIO} after three-step resize \
                         (boot {}x{} → A {}x{} → B {}x{}, {E2E_RESIZE_LAUNCH_SETTLE_FRAMES} \
                         launch-settle + {E2E_RESIZE_WAIT_FRAMES} wait frames between steps).",
                        crate::e2e::E2E_RESIZE_BOOT_WIDTH,
                        crate::e2e::E2E_RESIZE_BOOT_HEIGHT,
                        E2E_RESIZE_A_WIDTH,
                        E2E_RESIZE_A_HEIGHT,
                        E2E_RESIZE_B_WIDTH,
                        E2E_RESIZE_B_HEIGHT,
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
        // ---- Oasis-edit-visual phases (`02f-followup`) -------------------
        E2ePhase::OasisWarmup => {
            // Camera pose is owned by `pin_oasis_camera`; do not touch
            // `camera` here (any write would race with the post-driver
            // override).
            let _ = &mut camera;
            state.phase_ticks += 1;
            if state.phase_ticks >= super::oasis_edit_visual::OASIS_WARMUP_FRAMES {
                screenshot.0 = None;
                state.phase = E2ePhase::OasisShootBefore;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::OasisShootBefore => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::OasisDrainBefore;
            state.phase_ticks = 0;
        }
        E2ePhase::OasisDrainBefore => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render --oasis-edit-visual: before-capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        if vox_gpu_construction_mode {
                            super::vox_gpu_construction::save_vox_gpu_construction_screenshot(
                                &fb,
                                super::vox_gpu_construction::VOX_GPU_CONSTRUCTION_BEFORE_PNG,
                            );
                        } else {
                            super::oasis_edit_visual::save_oasis_screenshot(
                                &fb,
                                super::oasis_edit_visual::OASIS_EDIT_BEFORE_PNG,
                            );
                        }
                        oasis.before = Some(fb);
                        state.phase = E2ePhase::OasisApplyEdit;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "oasis-edit-visual: before-capture decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::oasis_edit_visual::OASIS_DRAIN_FRAMES
            {
                let err = format!(
                    "oasis-edit-visual: before-capture never delivered within \
                     {} drain frames",
                    super::oasis_edit_visual::OASIS_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::OasisApplyEdit => {
            // Apply the brush exactly once. The `world_data` resource ref is
            // optional because the driver may run before `setup_test_grid`
            // inserts it on the first frame — but by OasisWarmup completion
            // (120 frames) the resource is guaranteed present.
            if oasis.edit_applied {
                state.phase = E2ePhase::OasisWaitPostEdit;
                state.phase_ticks = 0;
            } else if let Some(mut wd) = world_data {
                // vox-gpu-rewrite W5.3-fix Stage 1 — the vox-gpu-construction
                // gate shares this phase with `--oasis-edit-visual` but uses
                // a camera-translation Δ (no brush) instead of a brush-edit Δ.
                // The W5 install path leaves `chunks_cpu / blocks_cpu /
                // voxels_cpu = Vec::new()` by design; `sphere_brush` indexes
                // into `chunks_cpu[ci]` and silently no-ops on the empty
                // mirror, so a brush-edit Δ would always be zero. Setting
                // `oasis.edit_applied = true` promotes the camera A→B via
                // `pin_vox_gpu_construction_camera`'s read of the flag —
                // moving the camera through a populated world sweeps
                // geometry through the framebuffer (large Δ); moving
                // through an empty world shows sky on both frames (Δ near
                // zero — regression signal).
                if vox_gpu_construction_mode {
                    let _ = &mut wd;
                    super::vox_gpu_construction::promote_camera_to_pose_b();
                } else {
                    super::oasis_edit_visual::apply_erase_brush(&mut wd);
                }
                oasis.edit_applied = true;
                state.phase = E2ePhase::OasisWaitPostEdit;
                state.phase_ticks = 0;
            } else {
                let err = "oasis-edit-visual: WorldData resource missing at \
                           OasisApplyEdit — the Oasis VOX load failed or \
                           was deferred past the warmup window"
                    .to_string();
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::OasisWaitPostEdit => {
            state.phase_ticks += 1;
            if state.phase_ticks
                >= super::oasis_edit_visual::OASIS_POST_EDIT_WAIT_FRAMES
            {
                screenshot.0 = None;
                state.phase = E2ePhase::OasisShootAfter;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::OasisShootAfter => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::OasisDrainAfter;
            state.phase_ticks = 0;
        }
        E2ePhase::OasisDrainAfter => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render --oasis-edit-visual: after-capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        if vox_gpu_construction_mode {
                            super::vox_gpu_construction::save_vox_gpu_construction_screenshot(
                                &fb,
                                super::vox_gpu_construction::VOX_GPU_CONSTRUCTION_AFTER_PNG,
                            );
                        } else {
                            super::oasis_edit_visual::save_oasis_screenshot(
                                &fb,
                                super::oasis_edit_visual::OASIS_EDIT_AFTER_PNG,
                            );
                        }
                        oasis.after = Some(fb);
                        state.phase = E2ePhase::OasisAssert;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "oasis-edit-visual: after-capture decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::oasis_edit_visual::OASIS_DRAIN_FRAMES
            {
                let err = format!(
                    "oasis-edit-visual: after-capture never delivered within \
                     {} drain frames",
                    super::oasis_edit_visual::OASIS_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::OasisAssert => {
            let before = oasis.before.take();
            let after = oasis.after.take();
            let result = match (before, after) {
                (Some(a), Some(b)) => {
                    if vox_gpu_construction_mode {
                        super::vox_gpu_construction::assert_vox_gpu_construction_landed(
                            &a, &b,
                        )
                        .map(|msg| {
                            println!("e2e_render --vox-gpu-construction: {msg}");
                        })
                    } else {
                        super::oasis_edit_visual::assert_visual_edit_landed(&a, &b)
                            .map(|msg| {
                                println!("e2e_render --oasis-edit-visual: {msg}");
                            })
                    }
                }
                _ => Err(
                    "oasis-edit-visual: OasisAssert reached without both \
                     framebuffers stashed (driver bug)"
                        .to_string(),
                ),
            };
            match &result {
                Ok(()) => {
                    if vox_gpu_construction_mode {
                        println!(
                            "e2e_render: vox-gpu-construction PASS — \
                             {} warmup + {} post-promote wait frames; \
                             camera A {:?} → camera B {:?} produced \
                             rect mean per-pixel RGB Δ above {:.2} floor.",
                            super::oasis_edit_visual::OASIS_WARMUP_FRAMES,
                            super::oasis_edit_visual::OASIS_POST_EDIT_WAIT_FRAMES,
                            super::vox_gpu_construction::VOX_GPU_CONSTRUCTION_CAMERA_POS_A,
                            super::vox_gpu_construction::VOX_GPU_CONSTRUCTION_CAMERA_POS_B,
                            super::vox_gpu_construction::VOX_GPU_CONSTRUCTION_DIFF_FLOOR,
                        );
                    } else {
                        println!(
                            "e2e_render: oasis-edit-visual PASS — \
                             {} warmup + {} post-edit wait frames; erase \
                             sphere @ r={:.1} voxels produced rect mean per-\
                             pixel RGB Δ above {:.2} floor.",
                            super::oasis_edit_visual::OASIS_WARMUP_FRAMES,
                            super::oasis_edit_visual::OASIS_POST_EDIT_WAIT_FRAMES,
                            super::oasis_edit_visual::OASIS_ERASE_RADIUS,
                            super::oasis_edit_visual::OASIS_EDIT_DIFF_FLOOR,
                        );
                    }
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
        // ---- Small-edit-visual phases (`03g`) ----------------------------
        E2ePhase::SmallEditWarmup => {
            let _ = &mut camera;
            state.phase_ticks += 1;
            if state.phase_ticks >= super::small_edit_visual::SMALL_EDIT_WARMUP_FRAMES {
                screenshot.0 = None;
                state.phase = E2ePhase::SmallEditShootBefore;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::SmallEditShootBefore => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::SmallEditDrainBefore;
            state.phase_ticks = 0;
        }
        E2ePhase::SmallEditDrainBefore => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render --small-edit-visual: before-capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        super::small_edit_visual::save_small_edit_screenshot(
                            &fb,
                            super::small_edit_visual::SMALL_EDIT_BEFORE_PNG,
                        );
                        small_edit.before = Some(fb);
                        state.phase = E2ePhase::SmallEditApply;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "small-edit-visual: before-capture decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::small_edit_visual::SMALL_EDIT_DRAIN_FRAMES
            {
                let err = format!(
                    "small-edit-visual: before-capture never delivered within \
                     {} drain frames",
                    super::small_edit_visual::SMALL_EDIT_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::SmallEditApply => {
            if small_edit.edit_applied {
                state.phase = E2ePhase::SmallEditWaitPostEdit;
                state.phase_ticks = 0;
            } else if let Some(mut wd) = world_data {
                super::small_edit_visual::apply_small_cube_edit(&mut wd, &mut small_edit);
                state.phase = E2ePhase::SmallEditWaitPostEdit;
                state.phase_ticks = 0;
            } else {
                let err = "small-edit-visual: WorldData resource missing at \
                           SmallEditApply — the world load failed or was \
                           deferred past the warmup window"
                    .to_string();
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::SmallEditWaitPostEdit => {
            state.phase_ticks += 1;
            if state.phase_ticks
                >= super::small_edit_visual::SMALL_EDIT_POST_EDIT_WAIT_FRAMES
            {
                screenshot.0 = None;
                state.phase = E2ePhase::SmallEditShootAfter;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::SmallEditShootAfter => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::SmallEditDrainAfter;
            state.phase_ticks = 0;
        }
        E2ePhase::SmallEditDrainAfter => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render --small-edit-visual: after-capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        super::small_edit_visual::save_small_edit_screenshot(
                            &fb,
                            super::small_edit_visual::SMALL_EDIT_AFTER_PNG,
                        );
                        small_edit.after = Some(fb);
                        state.phase = E2ePhase::SmallEditAssert;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "small-edit-visual: after-capture decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::small_edit_visual::SMALL_EDIT_DRAIN_FRAMES
            {
                let err = format!(
                    "small-edit-visual: after-capture never delivered within \
                     {} drain frames",
                    super::small_edit_visual::SMALL_EDIT_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::SmallEditAssert => {
            let before = small_edit.before.take();
            let after = small_edit.after.take();
            let count_before = small_edit.voxel_count_before.unwrap_or(0);
            let count_after = small_edit.voxel_count_after.unwrap_or(0);
            let world_size = small_edit.world_size_voxels.unwrap_or([0, 0, 0]);
            let result = match (before, after) {
                (Some(a), Some(b)) => {
                    super::small_edit_visual::assert_small_edit_landed(
                        &a,
                        &b,
                        count_before,
                        count_after,
                        world_size,
                    )
                    .map(|msg| {
                        println!("e2e_render --small-edit-visual: {msg}");
                    })
                }
                _ => Err(
                    "small-edit-visual: SmallEditAssert reached without both \
                     framebuffers stashed (driver bug)"
                        .to_string(),
                ),
            };
            match &result {
                Ok(()) => {
                    println!(
                        "e2e_render: small-edit-visual PASS — \
                         {} warmup + {} post-edit wait frames; \
                         cube_brush radius {} at {:?} produced exactly +1 voxel; \
                         click rect Δ above {} floor; adjacent rects below {} ceiling.",
                        super::small_edit_visual::SMALL_EDIT_WARMUP_FRAMES,
                        super::small_edit_visual::SMALL_EDIT_POST_EDIT_WAIT_FRAMES,
                        super::small_edit_visual::SMALL_EDIT_RADIUS,
                        super::small_edit_visual::SMALL_EDIT_CLICK_VOXEL,
                        super::small_edit_visual::SMALL_EDIT_CLICK_RECT_FLOOR,
                        super::small_edit_visual::SMALL_EDIT_ADJ_RECT_CEILING,
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
        // ---- Small-edit-repro phases (2026-05-17 — user Oasis click) ----
        E2ePhase::SmallEditReproWarmup => {
            let _ = &mut camera;
            state.phase_ticks += 1;
            if state.phase_ticks
                >= super::small_edit_repro::SMALL_EDIT_REPRO_WARMUP_FRAMES
            {
                screenshot.0 = None;
                state.phase = E2ePhase::SmallEditReproShootBefore;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::SmallEditReproShootBefore => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::SmallEditReproDrainBefore;
            state.phase_ticks = 0;
        }
        E2ePhase::SmallEditReproDrainBefore => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render --small-edit-repro: before-capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        super::small_edit_repro::save_small_edit_repro_screenshot(
                            &fb,
                            super::small_edit_repro::SMALL_EDIT_REPRO_BEFORE_PNG,
                        );
                        small_edit_repro.before = Some(fb);
                        state.phase = E2ePhase::SmallEditReproApply;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "small-edit-repro: before-capture decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::small_edit_repro::SMALL_EDIT_REPRO_DRAIN_FRAMES
            {
                let err = format!(
                    "small-edit-repro: before-capture never delivered within {} drain frames",
                    super::small_edit_repro::SMALL_EDIT_REPRO_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::SmallEditReproApply => {
            if small_edit_repro.edit_applied {
                state.phase = E2ePhase::SmallEditReproWaitPostEdit;
                state.phase_ticks = 0;
            } else if let Some(mut wd) = world_data {
                super::small_edit_repro::apply_small_edit_repro_edit(
                    &mut wd,
                    &mut small_edit_repro,
                );
                state.phase = E2ePhase::SmallEditReproWaitPostEdit;
                state.phase_ticks = 0;
            } else {
                let err = "small-edit-repro: WorldData resource missing at \
                           SmallEditReproApply — the world load failed or was \
                           deferred past the warmup window"
                    .to_string();
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::SmallEditReproWaitPostEdit => {
            state.phase_ticks += 1;
            if state.phase_ticks
                >= super::small_edit_repro::SMALL_EDIT_REPRO_POST_EDIT_WAIT_FRAMES
            {
                screenshot.0 = None;
                state.phase = E2ePhase::SmallEditReproShootAfter;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::SmallEditReproShootAfter => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::SmallEditReproDrainAfter;
            state.phase_ticks = 0;
        }
        E2ePhase::SmallEditReproDrainAfter => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render --small-edit-repro: after-capture {}x{}",
                            fb.width(),
                            fb.height()
                        );
                        super::small_edit_repro::save_small_edit_repro_screenshot(
                            &fb,
                            super::small_edit_repro::SMALL_EDIT_REPRO_AFTER_PNG,
                        );
                        small_edit_repro.after = Some(fb);
                        state.phase = E2ePhase::SmallEditReproAssert;
                        state.phase_ticks = 0;
                    }
                    Err(msg) => {
                        let err = format!(
                            "small-edit-repro: after-capture decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::small_edit_repro::SMALL_EDIT_REPRO_DRAIN_FRAMES
            {
                let err = format!(
                    "small-edit-repro: after-capture never delivered within {} drain frames",
                    super::small_edit_repro::SMALL_EDIT_REPRO_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::SmallEditReproAssert => {
            let before = small_edit_repro.before.take();
            let after = small_edit_repro.after.take();
            let result = match (before, after) {
                (Some(a), Some(b)) => {
                    super::small_edit_repro::assert_no_pitch_black_pixels(&a, &b)
                        .map(|msg| {
                            println!("e2e_render --small-edit-repro: {msg}");
                        })
                }
                _ => Err(
                    "small-edit-repro: SmallEditReproAssert reached without both \
                     framebuffers stashed (driver bug)"
                        .to_string(),
                ),
            };
            match &result {
                Ok(()) => {
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
        // ---- vox-gpu-oracle phases (Stage 4 — single-screenshot save) ----
        //
        // Camera pose is owned by `pin_vox_gpu_oracle_camera` (Update system,
        // `.after(driver::e2e_driver)`), so any pose write here would be
        // overridden anyway. Skip touching `camera`.
        E2ePhase::VoxGpuOracleWarmup => {
            let _ = &mut camera;
            state.phase_ticks += 1;
            if state.phase_ticks >= super::vox_gpu_oracle::ORACLE_WARMUP_FRAMES {
                screenshot.0 = None;
                state.phase = E2ePhase::VoxGpuOracleShoot;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::VoxGpuOracleShoot => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::VoxGpuOracleDrain;
            state.phase_ticks = 0;
        }
        E2ePhase::VoxGpuOracleDrain => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        // Stage 14 (2026-05-18): real dual-capture
                        // restored. The CPU phase saves to
                        // `oracle_cpu.png`, the GPU phase to
                        // `oracle_gpu.png`; the compare phase loads both
                        // and runs an SSIM comparison.
                        let is_cpu = app_args
                            .as_deref()
                            .is_some_and(|a| a.vox_gpu_oracle_cpu_phase);
                        let is_gpu = app_args
                            .as_deref()
                            .is_some_and(|a| a.vox_gpu_oracle_gpu_phase);
                        let filename = if is_cpu {
                            super::vox_gpu_oracle::ORACLE_CPU_PNG
                        } else if is_gpu {
                            super::vox_gpu_oracle::ORACLE_GPU_PNG
                        } else {
                            // Unreachable in practice: routed in only when
                            // one of the two flags is set.
                            super::vox_gpu_oracle::ORACLE_CPU_PNG
                        };
                        println!(
                            "e2e_render --vox-gpu-oracle: capture {}x{}; \
                             saving to {}",
                            fb.width(),
                            fb.height(),
                            filename
                        );
                        super::vox_gpu_oracle::save_oracle_screenshot(&fb, filename);
                        vox_gpu_oracle.captured = Some(fb);
                        vox_gpu_oracle.saved = true;
                        outcome.gate_result = Some(Ok(()));
                        exit.write(AppExit::Success);
                        state.phase = E2ePhase::Done;
                    }
                    Err(msg) => {
                        let err = format!(
                            "vox-gpu-oracle: capture decode failed: {msg}"
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::vox_gpu_oracle::ORACLE_DRAIN_FRAMES
            {
                let err = format!(
                    "vox-gpu-oracle: capture never delivered within {} drain frames",
                    super::vox_gpu_oracle::ORACLE_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        // ---- PBR-raymarching visual gate phases (`02-design.md` § I) -----
        // Camera pose owned by `pin_pbr_visual_camera` (Update system,
        // `.after(driver::e2e_driver)`).
        E2ePhase::PbrVisualWarmup => {
            let _ = &mut camera;
            state.phase_ticks += 1;
            if state.phase_ticks >= super::pbr_visual::PBR_VISUAL_WARMUP_FRAMES {
                screenshot.0 = None;
                state.phase = E2ePhase::PbrVisualShoot;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::PbrVisualShoot => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::PbrVisualDrain;
            state.phase_ticks = 0;
        }
        E2ePhase::PbrVisualDrain => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render --pbr-visual: capture {}x{}; saving to {}",
                            fb.width(),
                            fb.height(),
                            super::pbr_visual::PBR_VISUAL_PNG,
                        );
                        super::pbr_visual::save_pbr_visual_screenshot(
                            &fb,
                            super::pbr_visual::PBR_VISUAL_PNG,
                        );
                        let result = super::pbr_visual::assert_pbr_visual(&fb);
                        pbr_visual.captured = Some(fb);
                        pbr_visual.saved = true;
                        match &result {
                            Ok(msg) => {
                                println!("e2e_render --pbr-visual: {msg}");
                                println!(
                                    "e2e_render: pbr-visual PASS — \
                                     {} warmup frames; default test grid \
                                     side-on metallic-pillar view.",
                                    super::pbr_visual::PBR_VISUAL_WARMUP_FRAMES,
                                );
                                outcome.gate_result = Some(Ok(()));
                                exit.write(AppExit::Success);
                            }
                            Err(msg) => {
                                eprintln!("e2e_render: FAIL —\n{msg}");
                                outcome.gate_result = Some(Err(msg.clone()));
                                exit.write(AppExit::error());
                            }
                        }
                        state.phase = E2ePhase::Done;
                    }
                    Err(msg) => {
                        let err =
                            format!("pbr-visual: capture decode failed: {msg}");
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks >= super::pbr_visual::PBR_VISUAL_DRAIN_FRAMES
            {
                let err = format!(
                    "pbr-visual: capture never delivered within {} drain frames",
                    super::pbr_visual::PBR_VISUAL_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        // ---- PBR rendering-debugger gate phases --------------------------
        // Camera pose owned by `pin_pbr_debug_modes_camera` (Update system,
        // `.after(driver::e2e_driver)`). The driver advances
        // `pbr_visual.debug_modes.mode_cursor` from 1..=NUM_DEBUG_MODES, writing
        // `DebugViewState.mode` on entry to each `PbrDebugModesSettle`
        // phase. After all modes are captured, `PbrDebugModesAssert` runs
        // the per-mode non-degeneracy check and exits.
        E2ePhase::PbrDebugModesWarmup => {
            let _ = &mut camera;
            state.phase_ticks += 1;
            // Ensure the debug-view starts disabled so the warmup is the
            // production path (TAA/GI convergence baseline). Mutating the
            // `DebugViewState` resource via `commands.insert_resource` keeps
            // the driver's `SystemParam` count below Bevy 0.19's
            // `IntoSystemSet` arity ceiling.
            commands.insert_resource(crate::debug_view::DebugViewState {
                mode: crate::debug_view::DebugViewMode::Off,
                last_active: None,
            });
            if state.phase_ticks >= super::pbr_debug_modes::PBR_DEBUG_MODES_WARMUP_FRAMES {
                pbr_visual.debug_modes.mode_cursor = 1;
                pbr_visual.debug_modes.captures.clear();
                state.phase = E2ePhase::PbrDebugModesSettle;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::PbrDebugModesSettle => {
            // On tick 0 of this phase, set the next mode.
            if state.phase_ticks == 0 {
                let next_mode = crate::debug_view::DebugViewMode::from_u32(
                    pbr_visual.debug_modes.mode_cursor,
                );
                commands.insert_resource(crate::debug_view::DebugViewState {
                    mode: next_mode,
                    last_active: Some(next_mode),
                });
                println!(
                    "e2e_render --pbr-debug-modes: settling mode {} ({})",
                    pbr_visual.debug_modes.mode_cursor,
                    next_mode.label(),
                );
            }
            state.phase_ticks += 1;
            if state.phase_ticks >= super::pbr_debug_modes::PBR_DEBUG_MODE_SETTLE_FRAMES {
                screenshot.0 = None;
                state.phase = E2ePhase::PbrDebugModesShoot;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::PbrDebugModesShoot => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::PbrDebugModesDrain;
            state.phase_ticks = 0;
        }
        E2ePhase::PbrDebugModesDrain => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        let mode_id = pbr_visual.debug_modes.mode_cursor;
                        let mode = crate::debug_view::DebugViewMode::from_u32(mode_id);
                        let label = mode.label();
                        super::pbr_debug_modes::save_pbr_debug_mode_png(
                            &fb, mode_id, label,
                        );
                        pbr_visual.debug_modes.captures.push((mode_id, label, fb));
                        // Advance to next mode or move to assert.
                        let next_cursor = mode_id + 1;
                        if next_cursor > crate::debug_view::DebugViewMode::NUM_DEBUG_MODES {
                            // All modes captured — restore production mode
                            // and run the assertions.
                            commands.insert_resource(crate::debug_view::DebugViewState {
                                mode: crate::debug_view::DebugViewMode::Off,
                                last_active: None,
                            });
                            state.phase = E2ePhase::PbrDebugModesAssert;
                            state.phase_ticks = 0;
                        } else {
                            pbr_visual.debug_modes.mode_cursor = next_cursor;
                            state.phase = E2ePhase::PbrDebugModesSettle;
                            state.phase_ticks = 0;
                        }
                    }
                    Err(msg) => {
                        let err = format!(
                            "pbr-debug-modes: mode {} capture decode failed: {msg}",
                            pbr_visual.debug_modes.mode_cursor,
                        );
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::pbr_debug_modes::PBR_DEBUG_MODE_DRAIN_FRAMES
            {
                let err = format!(
                    "pbr-debug-modes: mode {} capture never delivered within {} drain frames",
                    pbr_visual.debug_modes.mode_cursor,
                    super::pbr_debug_modes::PBR_DEBUG_MODE_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::PbrDebugModesAssert => {
            // Walk every captured framebuffer; build a multi-mode report.
            let mut failures: Vec<String> = Vec::new();
            let mut report_lines: Vec<String> = Vec::new();
            for (mode_id, label, fb) in &pbr_visual.debug_modes.captures {
                match super::pbr_debug_modes::assert_pbr_debug_mode_non_degenerate(
                    *mode_id, label, fb,
                ) {
                    Ok(line) => report_lines.push(line),
                    Err(err) => failures.push(err),
                }
            }
            let combined = report_lines.join("\n  ");
            if failures.is_empty() {
                println!(
                    "e2e_render --pbr-debug-modes: ALL {} modes PASS:\n  {}",
                    pbr_visual.debug_modes.captures.len(),
                    combined,
                );
                outcome.gate_result = Some(Ok(()));
                exit.write(AppExit::Success);
            } else {
                let err = format!(
                    "pbr-debug-modes: {} of {} modes FAILED:\n  {}\nPassing modes:\n  {}",
                    failures.len(),
                    pbr_visual.debug_modes.captures.len(),
                    failures.join("\n  "),
                    combined,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
            }
            state.phase = E2ePhase::Done;
        }
        // ---- PBR splotch-artifact gate phases (`--pbr-hard-edge`) --------
        // Camera pose owned by `pin_pbr_hard_edge_camera` (Update system,
        // `.after(oasis_edit_visual::pin_oasis_camera)`).
        E2ePhase::PbrHardEdgeWarmup => {
            let _ = &mut camera;
            state.phase_ticks += 1;
            if state.phase_ticks >= super::pbr_hard_edge::PBR_HARD_EDGE_WARMUP_FRAMES {
                screenshot.0 = None;
                state.phase = E2ePhase::PbrHardEdgeShoot;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::PbrHardEdgeShoot => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::PbrHardEdgeDrain;
            state.phase_ticks = 0;
        }
        E2ePhase::PbrHardEdgeDrain => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                match Framebuffer::from_image(&image) {
                    Ok(fb) => {
                        println!(
                            "e2e_render --pbr-hard-edge: capture {}x{}; saving to {}",
                            fb.width(),
                            fb.height(),
                            super::pbr_hard_edge::PBR_HARD_EDGE_PNG,
                        );
                        super::pbr_hard_edge::save_pbr_hard_edge_screenshot(
                            &fb,
                            super::pbr_hard_edge::PBR_HARD_EDGE_PNG,
                        );
                        let result = super::pbr_hard_edge::assert_pbr_hard_edge(&fb);
                        pbr_visual.hard_edge.captured = Some(fb);
                        pbr_visual.hard_edge.saved = true;
                        match &result {
                            Ok(msg) => {
                                println!("e2e_render --pbr-hard-edge: {msg}");
                                outcome.gate_result = Some(Ok(()));
                                exit.write(AppExit::Success);
                            }
                            Err(msg) => {
                                eprintln!("e2e_render: FAIL —\n{msg}");
                                outcome.gate_result = Some(Err(msg.clone()));
                                exit.write(AppExit::error());
                            }
                        }
                        state.phase = E2ePhase::Done;
                    }
                    Err(msg) => {
                        let err =
                            format!("pbr-hard-edge: capture decode failed: {msg}");
                        eprintln!("e2e_render: FAIL — {err}");
                        outcome.gate_result = Some(Err(err));
                        exit.write(AppExit::error());
                        state.phase = E2ePhase::Done;
                    }
                }
            } else if state.phase_ticks
                >= super::pbr_hard_edge::PBR_HARD_EDGE_DRAIN_FRAMES
            {
                let err = format!(
                    "pbr-hard-edge: capture never delivered within {} drain frames",
                    super::pbr_hard_edge::PBR_HARD_EDGE_DRAIN_FRAMES,
                );
                eprintln!("e2e_render: FAIL — {err}");
                outcome.gate_result = Some(Err(err));
                exit.write(AppExit::error());
                state.phase = E2ePhase::Done;
            }
        }
        E2ePhase::Done => {
            // `AppExit` is written; the winit runner sees `should_exit()` and
            // exits the event loop. Nothing more to do.
        }
    }
}

/// Compute mean luminance over the **entire** framebuffer. The user-spec
/// metric per the dispatch brief: `solid_block_rect` was tuned for the
/// original Batch-6 pose at 256×256 and doesn't reliably catch the bug at
/// other resolutions/poses (previous run: full-frame 136 → 63 vs
/// solid-block 241 → 229). Full-frame mean is the honest discriminator.
fn full_frame_luma(fb: &Framebuffer) -> f32 {
    fb.region_luminance(Rect {
        x0: 0,
        y0: 0,
        x1: fb.width(),
        y1: fb.height(),
    })
}

/// Run the three-step resize-test luma comparison + save all three PNGs.
///
/// User spec (verbatim, this dispatch): "start the game in 800×600, then
/// resize it to 1920×1080 then resize it to 2000×1000 and each time wait 5
/// seconds and screenshot". This function runs after the driver has finished
/// all three captures (`initial` at 800×600, `after_resize_a` at 1920×1080,
/// `after_resize_b` at 2000×1000) and validates each post-resize capture's
/// full-frame luma against the initial.
///
/// **Pass criterion**: both `after_resize_a / initial` and `after_resize_b /
/// initial` ratios must be ≥ [`E2E_RESIZE_MIN_LUMA_RATIO`] (0.7). A 30%
/// drop or worse is the bug signal. The prior bug-reproducing single-resize
/// run showed a 54% drop in full-frame luma (136 → 63, ratio ≈ 0.46) —
/// well below this threshold.
fn run_resize_test_assertions(state: &mut ResizeTestState) -> Result<(), String> {
    let initial = state.initial.take().ok_or_else(|| {
        "resize-test: ResizeAssert reached with no initial framebuffer (driver bug)".to_string()
    })?;
    let after_a = state.after_resize_a.take().ok_or_else(|| {
        "resize-test: ResizeAssert reached with no after_resize_a framebuffer (driver bug)".to_string()
    })?;
    let after_b = state.after_resize_b.take().ok_or_else(|| {
        "resize-test: ResizeAssert reached with no after_resize_b framebuffer (driver bug)".to_string()
    })?;

    // Save all three PNGs unconditionally so the user can visually inspect.
    let initial_path = Path::new(E2E_SCREENSHOT_DIR).join(E2E_RESIZE_INITIAL_PNG);
    let a_path = Path::new(E2E_SCREENSHOT_DIR).join(E2E_RESIZE_A_PNG);
    let b_path = Path::new(E2E_SCREENSHOT_DIR).join(E2E_RESIZE_B_PNG);
    let _ = initial
        .save_png(&initial_path)
        .map_err(|e| eprintln!("e2e_render: resize-test: initial PNG save failed: {e}"));
    let _ = after_a
        .save_png(&a_path)
        .map_err(|e| eprintln!("e2e_render: resize-test: resize_a PNG save failed: {e}"));
    let _ = after_b
        .save_png(&b_path)
        .map_err(|e| eprintln!("e2e_render: resize-test: resize_b PNG save failed: {e}"));
    // Also keep the standard `e2e_latest.png` slot populated (with the
    // final post-resize frame) so the established harness path is
    // consistent.
    let _ = after_b
        .save_png(Path::new(E2E_SCREENSHOT_DIR).join(E2E_SCREENSHOT_LATEST))
        .map_err(|e| eprintln!("e2e_render: resize-test: latest PNG save failed: {e}"));

    println!(
        "e2e_render: resize-test initial  {}x{} -> saved {}",
        initial.width(),
        initial.height(),
        initial_path.display()
    );
    println!(
        "e2e_render: resize-test resize_a {}x{} -> saved {}",
        after_a.width(),
        after_a.height(),
        a_path.display()
    );
    println!(
        "e2e_render: resize-test resize_b {}x{} -> saved {}",
        after_b.width(),
        after_b.height(),
        b_path.display()
    );

    // Full-frame mean luma — the metric per the dispatch brief.
    let luma_initial = full_frame_luma(&initial);
    let luma_a = full_frame_luma(&after_a);
    let luma_b = full_frame_luma(&after_b);

    // Per-frame diagnostic reports (reuse the standard helper).
    println!("e2e_render: resize-test initial  {}", region_luminance_report(&initial));
    println!("e2e_render: resize-test resize_a {}", region_luminance_report(&after_a));
    println!("e2e_render: resize-test resize_b {}", region_luminance_report(&after_b));

    if luma_initial <= 1.0e-3 {
        return Err(format!(
            "resize-test: initial full-frame luminance is essentially zero \
             ({luma_initial:.3}); the harness never produced a lit image. \
             Bump E2E_RESIZE_LAUNCH_SETTLE_FRAMES or investigate."
        ));
    }

    let ratio_a = luma_a / luma_initial;
    let ratio_b = luma_b / luma_initial;

    println!(
        "e2e_render: resize-test luma — initial {luma_initial:.2}, \
         after_a {luma_a:.2} (ratio {ratio_a:.4}), \
         after_b {luma_b:.2} (ratio {ratio_b:.4}); \
         threshold {E2E_RESIZE_MIN_LUMA_RATIO:.2}"
    );

    let fail_a = ratio_a < E2E_RESIZE_MIN_LUMA_RATIO;
    let fail_b = ratio_b < E2E_RESIZE_MIN_LUMA_RATIO;
    if fail_a || fail_b {
        return Err(format!(
            "resize-test: GI bounce light went black after window resize.\n  \
             initial  ({}x{}) full-frame luma = {luma_initial:.2}\n  \
             resize_a ({}x{}) full-frame luma = {luma_a:.2}, ratio = {ratio_a:.4} [{}]\n  \
             resize_b ({}x{}) full-frame luma = {luma_b:.2}, ratio = {ratio_b:.4} [{}]\n  \
             threshold                          = {E2E_RESIZE_MIN_LUMA_RATIO:.2}\n  \
             screenshots saved to: {} + {} + {}\n  \
             Regression of the GI-bounce-on-resize fix — see\n  \
             `docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`\n  \
             `## GI-bounce-on-resize fix (2026-05-16)`. The fix caps\n  \
             `sample_refine.wgsl` padded dispatch groups at 32 768 so wgpu's\n  \
             indirect-validation pass does not zero the dispatch args at\n  \
             viewports ≥ 1920×1080; if this gate trips again, that cap is\n  \
             the first thing to check.",
            initial.width(), initial.height(),
            after_a.width(), after_a.height(),
            if fail_a { "FAIL" } else { "pass" },
            after_b.width(), after_b.height(),
            if fail_b { "FAIL" } else { "pass" },
            initial_path.display(),
            a_path.display(),
            b_path.display(),
        ));
    }

    Ok(())
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
    vox_e2e_mode: bool,
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
                //
                // vox-gpu-rewrite Stage 2 (2026-05-18): skipped in vox-e2e
                // mode. The W5 GPU producer chain tiles the synthesised
                // fixture across the entire fixed `(4096, 512, 4096)`-voxel
                // world via `voxelPos % modelSize`, so the camera sees
                // tiled geometry at every horizon — no dark "sky vs
                // geometry" contrast. The dedicated vox_e2e geometry gate
                // (`assert_vox_geometry_visible`) is the load-bearing
                // check for this mode.
                if !vox_e2e_mode {
                    if let Err(msg) = fb.check_not_degenerate() {
                        failures.push(format!("degenerate-frame floor:\n  {msg}"));
                    }
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
                // (4) Per-batch region gate — OR the vox-e2e geometry gate
                // when the `.vox` ingestion gate mode is requested. The two
                // are mutually exclusive: vox-e2e replaces the default test
                // grid, so the default-scene gate rect calibrations don't
                // apply, and vice versa.
                let state = GateState {
                    fb: &fb,
                    fb_next: None,
                };
                if vox_e2e_mode {
                    // Persist a dedicated vox-e2e PNG alongside the
                    // standard `e2e_latest.png` slot so the user has a
                    // distinct artifact to inspect for this mode's runs.
                    super::vox_e2e::save_vox_e2e_screenshot(&fb);
                    if let Err(msg) =
                        super::vox_e2e::assert_vox_geometry_visible(&fb)
                    {
                        failures.push(format!("vox_e2e geometry gate:\n  {msg}"));
                    }
                } else {
                    if let Err(msg) = batch_gate(CURRENT_BATCH, &state) {
                        failures.push(format!("region gate:\n  {msg}"));
                    }
                    // (4b) Phase-C followup #5 — entity-pixel gate. Fires
                    // only in `--entities` mode (where the fixture entity
                    // is spawned). Vox-e2e mode never spawns the entity.
                    if entities_mode {
                        if let Err(msg) = super::gates::assert_entity_pixel(&state) {
                            failures.push(format!("entity_pixel gate:\n  {msg}"));
                        }
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
