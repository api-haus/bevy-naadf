//! E2e gate-mode resource + the per-gate trait scaffolding.
//!
//! [`E2eGateMode`] is the single enum `Resource` that names which e2e
//! gate flow the driver runs. Step 6 of the config-as-resource refactor
//! (`docs/orchestrate/config-as-resource-refactor/02-design.md`) collapsed
//! the 10 mutually-exclusive e2e-mode booleans on `AppArgs` into this one
//! enum (Bucket B — Mode). It is promoted from the D6-era `GateKind`
//! discriminator: renamed, extended with the missing variants so every
//! collapsed boolean maps to exactly one variant, and given
//! `#[derive(Resource)]` so the bootstrap fan-out can insert it and the
//! driver / camera-pin systems can read it via `Res<E2eGateMode>`.
//!
//! The per-gate [`Gate`] trait, the [`FrameBudget`] struct, and the
//! [`set_camera_pose`] helper are the still-dead structural scaffolding
//! introduced by D6 step 2 of the codebase-tightening refactor
//! (`docs/orchestrate/codebase-tightening/e2e-and-playwright/
//! 03-architecture.md` §3 Step 2). They are intended to be consumed in a
//! later refactor where the per-gate `impl Gate` blocks land alongside the
//! `e2e/driver.rs` decomposition; until then they carry targeted
//! `#[allow(dead_code)]`.

use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::Framebuffer;
use crate::world::data::WorldData;

/// Identifies which e2e gate flow the run is dispatching — Bucket B (Mode)
/// of the config-as-resource three-bucket taxonomy. Inserted once at app
/// build time by the bootstrap fan-out (`BootstrapInputs::gate_mode`);
/// read by the e2e driver state machine and the per-gate `pin_*_camera`
/// systems for the duration of the run.
///
/// Step 6 of the config-as-resource refactor collapsed the 10
/// mutually-exclusive e2e-mode booleans on `AppArgs`
/// (`resize_test`, `oasis_edit_visual_mode`, `small_edit_visual_mode`,
/// `small_edit_repro_mode`, `vox_gpu_construction_mode`,
/// `vox_gpu_oracle_cpu_phase`, `vox_gpu_oracle_gpu_phase`,
/// `vox_web_parity_skybox_phase`, `vox_web_parity_loaded_phase`,
/// `vox_horizon_native_phase`) into this single enum. Each former boolean
/// maps to exactly one variant; the all-false default maps to
/// [`E2eGateMode::Standard`].
///
/// `vox_e2e_mode` is deliberately NOT folded in here — it is Bucket A
/// (an ASSERT-time data tag, not a flow selector) and migrates to its own
/// `VoxE2eAssertion` resource in Step 7. The `--vox-e2e` gate therefore
/// runs with `E2eGateMode::Standard`.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum E2eGateMode {
    /// The default Warmup→Motion→Settle→Shoot→Drain→Assert flow that the
    /// resize / oasis / small-edit families don't take over (covers
    /// `baseline`, `--vox-e2e`, `--entities`, `--edit-mode`,
    /// `--validate-gpu-construction` post-app tails — i.e. the e2e modes
    /// where the standard region gate runs).
    #[default]
    Standard,
    /// `--resize-test` — resize-blackness reproduction (genuinely distinct
    /// flow). Wayland resize is structurally unique to this gate. Was
    /// `AppArgs.resize_test`.
    Resize,
    /// `--oasis-edit-visual` — brush-edit gate over Oasis VOX. Was
    /// `AppArgs.oasis_edit_visual_mode`.
    OasisEdit,
    /// `--vox-gpu-construction` — share-flow-with-OasisEdit + camera promote.
    /// Was `AppArgs.vox_gpu_construction_mode`.
    VoxGpuConstruction,
    /// `--small-edit-visual` — brush + voxel-count + adj-rect gate. Was
    /// `AppArgs.small_edit_visual_mode`.
    SmallEditVisual,
    /// `--small-edit-repro` — user-captured Oasis click repro. Was
    /// `AppArgs.small_edit_repro_mode`.
    SmallEditRepro,
    /// `--vox-gpu-oracle-cpu` — single-capture CPU oracle phase. Was
    /// `AppArgs.vox_gpu_oracle_cpu_phase`.
    VoxGpuOracleCpu,
    /// `--vox-gpu-oracle-gpu` — single-capture GPU producer phase. Was
    /// `AppArgs.vox_gpu_oracle_gpu_phase`.
    VoxGpuOracleGpu,
    /// `--vox-web-parity-skybox` — single-capture skybox baseline. Was
    /// `AppArgs.vox_web_parity_skybox_phase`.
    VoxWebParitySkybox,
    /// `--vox-web-parity-loaded` — single-capture loaded-scene phase. Was
    /// `AppArgs.vox_web_parity_loaded_phase`.
    VoxWebParityLoaded,
    /// `--vox-horizon-native` — single-capture C#-faithful horizon pose.
    /// Was `AppArgs.vox_horizon_native_phase`.
    VoxHorizonNative,
}

/// Per-gate frame budget. Each gate's [`Gate::frame_budget`] returns a
/// value composed from its existing per-gate `*_FRAMES` constants
/// (per-gate consts stay at their declaration site; this struct only
/// aggregates them for the shared driver loop's consumption).
///
/// Still-dead D6-step-2 scaffolding — see the module doc.
#[allow(dead_code)]
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
///
/// Still-dead D6-step-2 scaffolding — see the module doc.
#[allow(dead_code)]
pub trait Gate: Send + Sync + 'static {
    /// Which kind this gate is; used by the driver to discriminate
    /// edit-phase vs single-capture flows.
    fn kind(&self) -> E2eGateMode;

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
///
/// Still-dead D6-step-2 scaffolding — see the module doc.
#[allow(dead_code)]
pub fn set_camera_pose(transform: &mut Transform, position_split: &mut PositionSplit, pose: Transform) {
    *transform = pose;
    *position_split = PositionSplit::from_world(pose.translation);
}
