//! Per-gate trait absorbing the Warmup→Shoot→Drain→Save→Assert pattern.
//!
//! Each e2e gate provides its frame budgets, its camera pose, its edit
//! hook (if any), its assertion, and its verdict log. The shared driver
//! loop drives all of them through one [`crate::e2e::driver::E2ePhase`]
//! state machine.
//!
//! This module is the structural scaffolding introduced by D6 step 2 of
//! the codebase-tightening refactor (`docs/orchestrate/codebase-tightening/
//! e2e-and-playwright/03-architecture.md` §3 Step 2). The trait, the
//! [`GateKind`] enum, the [`FrameBudget`] struct, and the
//! [`set_camera_pose`] / [`pin_active_gate_camera`] helpers are intended
//! to be consumed in subsequent steps (3+) where the per-gate `impl Gate`
//! blocks land alongside the `e2e/driver.rs` decomposition. Until then
//! the symbols are unused — `cargo build` warnings about dead code are
//! expected and intentional.

#![allow(dead_code)]

use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::Framebuffer;
use crate::world::data::WorldData;

/// Identifies which gate the run is dispatching. Set once at app build
/// time from `AppArgs`; held inside the e2e driver state for the duration
/// of the run.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GateKind {
    /// The default Warmup→Motion→Settle→Shoot→Drain→Assert flow that the
    /// resize / oasis / small-edit families don't take over (covers
    /// `baseline`, `--vox-e2e`, `--entities`, `--edit-mode`,
    /// `--validate-gpu-construction` post-app tails — i.e. the e2e modes
    /// where the standard region gate runs).
    #[default]
    Standard,
    /// `--resize-test` — resize-blackness reproduction (genuinely distinct
    /// flow). Wayland resize is structurally unique to this gate.
    Resize,
    /// `--oasis-edit-visual` — brush-edit gate over Oasis VOX.
    OasisEdit,
    /// `--vox-gpu-construction` — share-flow-with-OasisEdit + camera promote.
    VoxGpuConstruction,
    /// `--small-edit-visual` — brush + voxel-count + adj-rect gate.
    SmallEditVisual,
    /// `--small-edit-repro` — user-captured Oasis click repro.
    SmallEditRepro,
    /// `--vox-gpu-oracle` — single-capture; CPU vs GPU phase.
    VoxGpuOracle,
    /// `--vox-web-parity` — single-capture; skybox/loaded/horizon phase.
    VoxWebParity,
}

/// Per-gate frame budget. Each gate's [`Gate::frame_budget`] returns a
/// value composed from its existing per-gate `*_FRAMES` constants
/// (per-gate consts stay at their declaration site; this struct only
/// aggregates them for the shared driver loop's consumption).
#[derive(Clone, Copy, Debug)]
pub struct FrameBudget {
    /// Frames spent before the first capture (settles GI + TAA).
    pub warmup: u32,
    /// Frames spent between the pre-edit capture and the post-edit capture.
    /// `None` for single-capture gates with no edit phase.
    pub post_edit_wait: Option<u32>,
    /// Frames the screenshot drain phase is allowed before failing.
    pub drain: u32,
}

/// The trait every gate implements. Owned by the gate's `e2e/<gate>.rs`
/// module; consumed by the shared driver loop in `driver.rs`.
///
/// `&self` methods — gate config is static at boot. Mutating capture
/// state lives in the driver-owned `GateCaptures` resource (introduced
/// alongside the driver decomposition).
pub trait Gate: Send + Sync + 'static {
    /// Which kind this gate is; used by the driver to discriminate
    /// edit-phase vs single-capture flows.
    fn kind(&self) -> GateKind;

    /// Per-gate frame budget.
    fn frame_budget(&self) -> FrameBudget;

    /// Compute the camera pose this gate pins. `world_data` is `None` if
    /// the gate's pose doesn't depend on world size (resize / horizon /
    /// web-parity); `Some` for the world-centre poses (oasis / small-edit
    /// / vox-gpu-construction).
    ///
    /// Returns `None` if the gate's pose isn't computable yet (e.g. the
    /// world hasn't loaded) — the driver leaves the camera at whatever
    /// the standard pin wrote.
    fn camera_pose(&self, world_data: Option<&WorldData>) -> Option<Transform>;

    /// Apply the gate's edit (brush, camera promote, …). Default impl is
    /// a no-op for single-capture gates. The driver calls this exactly
    /// once on the `Apply` phase.
    fn apply_edit(&self, _world_data: Option<&mut WorldData>) -> Result<(), String> {
        Ok(())
    }

    /// Run the gate's assertion against the captured before/after
    /// framebuffer(s). `after` is `None` for single-capture gates;
    /// `before` is `None` for single-capture gates without a pre-edit
    /// capture. Returns the gate's PASS message string on `Ok`.
    fn assert(
        &self,
        before: Option<&Framebuffer>,
        after: Option<&Framebuffer>,
    ) -> Result<String, String>;

    /// Format the gate's PASS verdict log (called only on `Ok`).
    /// Defaults to the assert payload — gates with extra config to
    /// surface override.
    fn verdict_log(&self, ok_msg: &str) -> String {
        ok_msg.to_string()
    }

    /// Filename pair this gate writes its captures to. For single-capture
    /// gates `(before, after)` is `(None, Some(_))`. The driver calls
    /// [`Framebuffer::save_in_screenshots_dir`](crate::e2e::framebuffer::
    /// Framebuffer::save_in_screenshots_dir) with the gate's
    /// [`log_tag`](Self::log_tag) for each non-`None` filename.
    fn capture_filenames(&self) -> (Option<&'static str>, &'static str);

    /// Per-gate log prefix (used by save + assert log lines).
    fn log_tag(&self) -> &'static str;
}

/// Write `pose` to the camera's `Transform` + recompute `PositionSplit`.
/// Replaces the 3-line `**transform = pose; **position_split =
/// PositionSplit::from_world(pose.translation);` write that the seven
/// `pin_*_camera` systems and the driver-inline writes currently
/// duplicate verbatim (DUP-6 in `00-reuse-audit.md §3.2`).
pub fn set_camera_pose(transform: &mut Transform, position_split: &mut PositionSplit, pose: Transform) {
    *transform = pose;
    *position_split = PositionSplit::from_world(pose.translation);
}
