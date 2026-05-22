//! E2e harness support code — pure primitives + per-gate assertion helpers.
//!
//! ## Post-restructure shape (`docs/orchestrate/e2e-ipc-rpc-restructure/`)
//!
//! The booted-window e2e harness is no longer an in-app driver mode. The 13
//! booted-window gates are BRP-driven `#[test]` bodies in
//! `crates/bevy_naadf/tests/`, driving the production `bin/bevy-naadf`
//! binary as the system-under-test over the Bevy Remote Protocol (the
//! `crate::e2e_brp` module). Phase 5 of that restructure deleted the in-app
//! driver machinery — `e2e/driver.rs` (the `match`-over-`E2ePhase` state
//! machine), `e2e/gate.rs` (`E2eGateMode`), `add_e2e_systems`, the 6
//! `pin_*_camera` systems, and the per-gate `run_*` boot fns.
//!
//! What this module still owns, and what the BRP harness depends on:
//! - [`readback`] — `Screenshot::primary_window()` capture + the observer.
//!   Wrapped by the `naadf/capture` BRP verb.
//! - [`checks`] — the `PipelineCache` error scan + node-dispatch check.
//!   Wrapped by `naadf/pipeline_scan` + `naadf/nodes_dispatched`.
//! - [`framebuffer`] — the format-aware `Framebuffer` wrapper + region
//!   helpers. The pure assertion math the 13 test files import.
//! - [`gates`] — the per-batch region gates, the camera poses, `CURRENT_BATCH`.
//! - [`ssim`] — the pure CPU PNG-diff (`ssim_compare_command` backs the
//!   `bin/e2e_render` utility; `ssim_compare_framebuffers` backs the
//!   compare gates).
//! - [`tracing_error_counter`] — the process-global `tracing::error!` count.
//! - the per-gate `<gate>.rs` modules — now **just the pure assertion /
//!   geometry / constant helpers** the migrated `tests/<gate>.rs` files
//!   import; the `run_*` / `pin_*` / `*State` machinery was deleted.
//!
//! The constants below (`E2E_*`) are pinned gate parameters the migrated
//! BRP test files + [`crate::window_config`] read.

pub mod checks;
pub mod framebuffer;
pub mod gates;
pub mod oasis_edit_visual;
pub mod readback;
pub mod small_edit_repro;
pub mod small_edit_visual;
pub mod ssim;
pub mod tracing_error_counter;
pub mod vox_e2e;
pub mod vox_gpu_construction;
pub mod vox_gpu_oracle;
pub mod vox_horizon_parity;
pub mod vox_web_parity;

// --- Frame-budget / window constants (`e2e-render-test.md` §3.3, §4.1, §5.2) -

/// Fixed e2e window resolution — small + fixed so the readback is fast, the GI
/// dispatch is cheap, and every `pixel_count`-sized buffer is identical
/// run-to-run (`e2e-render-test.md` §4.2 / §9). 256² is large enough for stable
/// region gates. Read by [`crate::window_config::WindowConfig::e2e`].
pub const E2E_WIDTH: u32 = 256;
/// Fixed e2e window resolution height — see [`E2E_WIDTH`].
pub const E2E_HEIGHT: u32 = 256;

/// Warmup-phase frame count — static at the fixed pose before the camera
/// starts moving. Comfortably above the resource-build latency with margin
/// for the camera-history ring to spin up.
///
/// **Phase B GI accumulation requirement (2026-05-15).** NAADF's compressed-
/// ReSTIR GI is a *temporal* algorithm: `renderGlobalIllum` writes lit/unlit
/// samples into the 128-frame `sample_counts` accumulation ring, and
/// `renderSampleRefine`'s `refineBuckets` has a hard gate
/// (`renderSampleRefine.fx:411`) that zeros a bucket's compressed output until
/// it has accumulated ≥12 samples across the temporal window. 96 frames is
/// comfortably past the up-to-64-frame `computeValidHistory` ring-capacity
/// window, so the buckets fully populate and the GI bounce converges to a
/// stable, visible result. The migrated BRP `standard` / `vox_e2e` test
/// bodies issue this as a `naadf/step` budget.
pub const E2E_WARMUP_FRAMES: u32 = 96;

/// Motion-phase frame count — the camera sweeps a deterministic open path
/// ([`gates::e2e_orbit_camera_transform`]) from the motion-start pose to the
/// fixed readback pose, exercising the TAA camera-motion reprojection.
pub const E2E_MOTION_FRAMES: u32 = 48;

/// Settle-phase frame count — static at the fixed readback pose, immediately
/// after the camera stops moving.
///
/// Kept at the **bare minimum (1 frame)** on purpose. The readback pose is one
/// the camera has never been static at; the decay must be caught *immediately*
/// after the motion — every extra static frame lets the static-camera running
/// average re-converge and washes the regression out.
pub const E2E_SETTLE_FRAMES: u32 = 1;

/// The fixed directory every gate writes its readback screenshot PNG(s) into.
/// Relative to the process CWD; `target/` is already gitignored and persists
/// across runs. The per-gate `save_*` helpers + path fns join into here.
pub const E2E_SCREENSHOT_DIR: &str = "target/e2e-screenshots";

/// The stable filename of the final asserted readback frame inside
/// [`E2E_SCREENSHOT_DIR`].
pub const E2E_SCREENSHOT_LATEST: &str = "e2e_latest.png";

/// Max extra frames a capture-await waits for the async `Screenshot` capture
/// to deliver (`e2e-render-test.md` §5.2, R2).
pub const E2E_DRAIN_FRAMES: u32 = 8;

// --- Resize-test constants
// (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
//  `## GI-bounce-on-resize fix (2026-05-16)`) -----------------------------
//
// Three-step resize: boot at 800×600, resize to 1920×1080, resize to 2000×1000.
// The migrated `resize_test` BRP test body reads these as window sizes +
// settle-frame budgets.

/// Boot width for the resize-test window (user spec: "start the game in
/// 800×600").
pub const E2E_RESIZE_BOOT_WIDTH: u32 = 800;
/// Boot height for the resize-test window — see [`E2E_RESIZE_BOOT_WIDTH`].
pub const E2E_RESIZE_BOOT_HEIGHT: u32 = 600;

/// First resize target (user spec: "resize it to 1920×1080").
pub const E2E_RESIZE_A_WIDTH: u32 = 1920;
/// First resize target height — see [`E2E_RESIZE_A_WIDTH`].
pub const E2E_RESIZE_A_HEIGHT: u32 = 1080;

/// Second resize target (user spec: "resize it to 2000×1000").
pub const E2E_RESIZE_B_WIDTH: u32 = 2000;
/// Second resize target height — see [`E2E_RESIZE_B_WIDTH`].
pub const E2E_RESIZE_B_HEIGHT: u32 = 1000;

/// Render frames between window launch and the first (initial-baseline)
/// screenshot. ≈ 5 s at 60 fps; gives the rings time to fill.
pub const E2E_RESIZE_LAUNCH_SETTLE_FRAMES: u32 = 300;

/// Render frames between each resize and the corresponding post-resize
/// screenshot. ≈ 5 s at 60 fps.
pub const E2E_RESIZE_WAIT_FRAMES: u32 = 300;

/// The minimum post-resize / initial luma ratio at which a single resize
/// step passes. Per the dispatch brief: a ≥ 30% full-frame luma drop fails.
pub const E2E_RESIZE_MIN_LUMA_RATIO: f32 = 0.7;

/// Filenames for the three screenshots saved by the resize-test (alongside
/// [`E2E_SCREENSHOT_LATEST`] inside [`E2E_SCREENSHOT_DIR`]).
pub const E2E_RESIZE_INITIAL_PNG: &str = "resize_initial.png";
pub const E2E_RESIZE_A_PNG: &str = "resize_a.png";
pub const E2E_RESIZE_B_PNG: &str = "resize_b.png";
