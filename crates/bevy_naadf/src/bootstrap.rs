//! Bootstrap-time configuration carrier.
//!
//! [`BootstrapInputs`] is the transient parser-output that the production
//! binaries (`main.rs`, `android_main.rs`, `bin/e2e_render.rs`) build from
//! CLI / argv / build-constants / device-probe values, and that the App
//! bootstrap fans into per-domain Bevy resources before the App runs. It is
//! the non-budget analogue of [`crate::render::budget::BudgetCaps`] —
//! transient at boot, dropped after the resource fan-out, never stored as a
//! `Resource` itself.
//!
//! Introduced at **Step 1** of the config-as-resource refactor
//! (`docs/orchestrate/config-as-resource-refactor/02-design.md`). At Step 1
//! it is a thin wrapper around [`AppArgs`]; subsequent steps progressively
//! move fields out of `AppArgs` into typed per-domain resource fields on this
//! struct (e.g. `taa_ring_depth`, `gi`, `construction_overrides`, …) until
//! `AppArgs` is fully drained and deleted.
//!
//! [`build_app_with_bootstrap_inputs`] fans `BootstrapInputs` into the
//! per-domain resources; [`run_e2e_render_with_bootstrap_inputs`] is the
//! e2e-gate entry point — it builds the App via the fan-out, picks the
//! window config from `inputs.gate_mode`, and drives the run to
//! completion. As of Step 6 every e2e gate routes through these; the
//! legacy `run_e2e_render_with_args` entry point was deleted.

use bevy::prelude::{App, AppExit};

use crate::e2e::gate::E2eGateMode;
use crate::render::construction::{ConstructionConfig, SpawnTestEntity};
use crate::render::taa::{TaaConfig, TaaRingConfig};
use crate::{AppArgs, AppConfig, GiSettings, GridPreset};

/// Transient bootstrap-time configuration carrier.
///
/// At Step 1 this is a thin wrapper around [`AppArgs`]. Each subsequent
/// migration step moves one or more fields off `AppArgs` into a typed
/// per-domain field on this struct (matching the per-domain resources the
/// fan-out inserts). When the migration completes, the `args` field goes
/// away.
///
/// `BootstrapInputs::default()` is byte-identical, by construction, to
/// `AppArgs::default()` — `Default` is derived and the only inner field is
/// `args: AppArgs`, whose `Default` impl is the canonical-default contract at
/// `crates/bevy_naadf/src/app_args.rs:185-207`. This guarantees that
/// `build_app_with_bootstrap_inputs(cfg, BootstrapInputs::default())` and
/// `build_app_with_args(cfg, AppArgs::default())` produce byte-identical
/// `App`s, which is the e2e-determinism requirement
/// (`docs/orchestrate/config-as-resource-refactor/01-context.md` —
/// "E2e-path byte-identical defaults").
#[derive(Clone)]
pub struct BootstrapInputs {
    /// Transient carrier for the [`AppArgs`] fields not yet migrated to
    /// per-domain resources. As fields move out of `AppArgs` in subsequent
    /// migration steps, they relocate to typed sibling fields on this struct.
    ///
    /// (No `Debug` derive — `AppArgs` itself is not `Debug` at
    /// `crates/bevy_naadf/src/app_args.rs:24`, only `Resource + Clone`.
    /// Adding `Debug` here would force a derive on `AppArgs` and its inner
    /// types, which is out of scope for Step 1.)
    pub args: AppArgs,
    /// TAA sample-ring depth (`18-taa-fidelity.md` fix #3). Migrated out of
    /// `AppArgs.taa_ring_depth` in Step 2 of the config-as-resource refactor.
    /// `TaaRingConfig::default()` = [`crate::DEFAULT_TAA_RING_DEPTH`] = 32;
    /// the mobile-budget / wasm32 entry points overwrite this with the
    /// budget-selected rung before the fan-out runs.
    pub taa_ring_depth: TaaRingConfig,
    /// Long-term TAA on/off (`06-design-a2.md` §6.1). Migrated out of
    /// `AppArgs.taa` in Step 3 of the config-as-resource refactor.
    /// `TaaConfig::default()` = `TaaConfig { enabled: true }`.
    pub taa: TaaConfig,
    /// Phase-B GI pipeline settings (`09-design-b.md` §3.8). Migrated out of
    /// `AppArgs.gi` in Step 3 of the config-as-resource refactor.
    /// `GiSettings::default()` = `GiSettings::DEFAULTS`. The settings panel
    /// mutates this resource at runtime via `ResMut<GiSettings>`.
    pub gi: GiSettings,
    /// Phase-C GPU-construction configuration (`15-design-c.md` §1.8).
    /// Migrated out of `AppArgs.construction_config` in Step 4 of the
    /// config-as-resource refactor. The default is
    /// [`ConstructionConfig::for_target_arch()`] — the same wasm32 clamp
    /// that previously lived inside the deleted `From<&AppArgs>` impl
    /// (Decision §5). E2e gates that need a non-default
    /// `gpu_construction_enabled` / `entities_enabled` override construct
    /// their own value off `for_target_arch()` and write it here.
    pub construction_config: ConstructionConfig,
    /// Which hard-coded test grid `setup_test_grid` installs at `Startup`.
    /// Migrated out of `AppArgs.grid_preset` in Step 5 of the
    /// config-as-resource refactor. `GridPreset::default()` =
    /// `GridPreset::Default` (the embedded primitive test scene).
    ///
    /// The native `--vox <path>` flag and the wasm32 `?skybox=1` URL param
    /// both resolve into this field BEFORE the App is built (the latter via
    /// [`crate::voxel::web_vox::resolve_skybox_only_param`], which Step 5
    /// relocated out of the old `Startup`-time `AppArgs.grid_preset`
    /// mutation in `web_vox::startup_fetch_default_vox`). The fan-out
    /// inserts it as a main-world `Res<GridPreset>` that `setup_test_grid`
    /// reads.
    pub grid_preset: GridPreset,
    /// Whether the Phase-C `--entities` test fixture (one 4×4×4
    /// emissive-voxel block at world centre) is spawned at `Startup`.
    /// Migrated out of `AppArgs.spawn_test_entity` in Step 8 of the
    /// config-as-resource refactor. `SpawnTestEntity::default()` =
    /// `SpawnTestEntity(false)`; the `e2e_render --entities` boot flips it
    /// on. The fan-out inserts it as a main-world `Res<SpawnTestEntity>`
    /// that gates `spawn_phase_c_test_entity` and that the e2e driver reads
    /// to pick the entity-aware ASSERT baseline.
    pub spawn_test_entity: SpawnTestEntity,
    /// Which e2e gate flow the run dispatches — Bucket B (Mode). Migrated
    /// out of the 10 mutually-exclusive e2e-mode booleans on `AppArgs`
    /// (`resize_test`, `oasis_edit_visual_mode`, …, `vox_horizon_native_phase`)
    /// in Step 6 of the config-as-resource refactor. `E2eGateMode::default()`
    /// = `E2eGateMode::Standard` (the standard Warmup→Motion→Settle→Shoot
    /// flow); each per-gate `run_*` builder sets the matching variant. The
    /// fan-out inserts it as a main-world `Res<E2eGateMode>` that the e2e
    /// driver state machine and the per-gate `pin_*_camera` systems read,
    /// and that [`crate::window_config::window_for_gate_mode`] reads to pick
    /// the e2e window resolution. `vox_e2e_mode` is NOT folded in here — it
    /// is Bucket A and stays on `AppArgs` until Step 7.
    pub gate_mode: E2eGateMode,
}

impl Default for BootstrapInputs {
    fn default() -> Self {
        // Step 4 of the config-as-resource refactor: the construction-config
        // default is `for_target_arch()`, NOT `ConstructionConfig::default()`,
        // because the wasm32 divergence must travel through the bootstrap
        // (Decision §5). The desktop arm of `for_target_arch()` IS
        // `ConstructionConfig::default()`, so on native targets the value is
        // byte-identical to pre-Step-4. All other fields use their own
        // `Default` impls.
        Self {
            args: AppArgs::default(),
            taa_ring_depth: TaaRingConfig::default(),
            taa: TaaConfig::default(),
            gi: GiSettings::default(),
            construction_config: ConstructionConfig::for_target_arch(),
            grid_preset: GridPreset::default(),
            spawn_test_entity: SpawnTestEntity::default(),
            gate_mode: E2eGateMode::default(),
        }
    }
}

/// Build the bevy-naadf `App` from an [`AppConfig`] and a [`BootstrapInputs`].
///
/// Step-2 shape — forwards to [`crate::build_app_with_args`] for the
/// not-yet-migrated `AppArgs` fields, then inserts the per-domain resources
/// already migrated off `AppArgs`. As more fields migrate, this becomes a
/// pure resource fan-out (no `AppArgs` forwarding).
pub fn build_app_with_bootstrap_inputs(cfg: AppConfig, inputs: BootstrapInputs) -> App {
    let mut app = crate::build_app_with_args(cfg, inputs.args);
    // Migrated in Step 2 — main-world `TaaRingConfig` resource. Bevy's
    // second-call `insert_resource` semantic is overwrite-in-place, so
    // mobile-budget callers that wrote a non-canonical depth into
    // `inputs.taa_ring_depth` end up with their value on the resource.
    // Inserted AFTER `build_app_with_args` (which adds the
    // `NaadfRenderPlugin`); the render-world consumer
    // `extract_taa_ring_depth` runs in `ExtractSchedule` on every frame, so
    // the first real frame sees this value before `RenderStartup`'s
    // `NaadfPipelines::from_world` reads the render-world mirror for the
    // `#{TAA_SAMPLE_RING_DEPTH}` shader-def.
    app.insert_resource(inputs.taa_ring_depth);
    // Migrated in Step 3 — `TaaConfig` (the long-term TAA on/off toggle) +
    // `GiSettings` (the Phase-B GI pipeline knobs the settings panel mutates).
    // Each is read by an extract system (`extract_taa_config`,
    // `extract_gi_config`) that pulls into the render sub-app per frame; the
    // settings panel takes `ResMut<GiSettings>` to mutate the GI knobs at
    // runtime. Like `taa_ring_depth`, these are inserted post-`build_app_with_args`
    // so any caller that overrode `BootstrapInputs::default()` wins.
    app.insert_resource(inputs.taa);
    app.insert_resource(inputs.gi);
    // Migrated in Step 4 — main-world `ConstructionConfig`. Inserted
    // post-`build_app_with_args` (which has a defensive
    // `ConstructionConfig::for_target_arch()` seed for direct-`build_app`
    // callers). The render sub-app reads its mirror via
    // `extract_construction_config` (mirror of `extract_effective_world_size`).
    app.insert_resource(inputs.construction_config);
    // Migrated in Step 5 — main-world `GridPreset`. `setup_test_grid` reads
    // it (`Res<GridPreset>`) at `Startup` to choose which world content to
    // install. Main-world only — the choice never crosses into the render
    // world. `build_app_with_args` has a defensive `GridPreset::default()`
    // seed for direct-`build_app` callers; this overwrite-in-place insert
    // wins for callers routing through the bootstrap fan-out (e.g. native
    // `--vox`, wasm32 `?skybox=1`, e2e gates).
    app.insert_resource(inputs.grid_preset);
    // Migrated in Step 8 — main-world `SpawnTestEntity`. Gates the
    // `spawn_phase_c_test_entity` `Startup` system; the e2e driver also
    // reads it for the entity-aware ASSERT baseline. `build_app_with_args`
    // has a defensive `SpawnTestEntity::default()` seed; this insert wins
    // for callers routing through the fan-out (the `--entities` e2e boot).
    app.insert_resource(inputs.spawn_test_entity);
    // Migrated in Step 6 — main-world `E2eGateMode`. The 10 e2e-mode
    // booleans on `AppArgs` collapsed into this one enum (Bucket B). The
    // e2e driver state machine branches on it (`Res<E2eGateMode>`), the
    // per-gate `pin_*_camera` systems read `Option<Res<E2eGateMode>>`, and
    // `setup_test_grid` reads it for the test-only CPU-oracle install
    // branch. `build_app_with_args` has a defensive `E2eGateMode::default()`
    // seed for direct-`build_app` callers; this insert wins for callers
    // routing through the fan-out (every e2e gate).
    app.insert_resource(inputs.gate_mode);
    app
}

/// Boot the bounded windowed e2e render test from a [`BootstrapInputs`] and
/// return its [`AppExit`].
///
/// The e2e-gate entry point: builds the App via the
/// [`build_app_with_bootstrap_inputs`] resource fan-out, picks the e2e
/// window config from `inputs.gate_mode`
/// ([`crate::window_config::window_for_gate_mode`]), and drives the run to
/// completion via [`crate::e2e::run_with_app`]. Every per-gate `run_*`
/// builder under `crate::e2e` constructs a `BootstrapInputs` and calls
/// this.
pub fn run_e2e_render_with_bootstrap_inputs(inputs: BootstrapInputs) -> AppExit {
    let mut cfg = AppConfig::e2e();
    cfg.window = crate::window_config::window_for_gate_mode(inputs.gate_mode);
    let app = build_app_with_bootstrap_inputs(cfg, inputs);
    crate::e2e::run_with_app(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GiSettings, GridPreset, DEFAULT_TAA_RING_DEPTH};

    /// `BootstrapInputs::default()` must produce the same canonical defaults
    /// as today's `AppArgs::default()` (for fields still on `AppArgs`) plus
    /// the migrated typed fields at their canonical-default values — this
    /// is the e2e-determinism contract from the design doc's "E2e-path
    /// byte-identical defaults" section.
    ///
    /// Step 2 of the config-as-resource refactor migrated `taa_ring_depth`
    /// off `AppArgs` onto the `taa_ring_depth: TaaRingConfig` field on this
    /// struct; Step 3 migrated `taa` and `gi` onto `taa: TaaConfig` and
    /// `gi: GiSettings`. The pins moved to the typed fields.
    #[test]
    fn default_wraps_canonical_app_args_defaults() {
        let inputs = BootstrapInputs::default();
        // The user's named smell — must stay at the documented constant.
        // Migrated to the typed `taa_ring_depth: TaaRingConfig` field in
        // Step 2.
        assert_eq!(inputs.taa_ring_depth.depth, DEFAULT_TAA_RING_DEPTH);
        // TAA on by default (Phase A-2; both production and e2e boot TAA on).
        // Step 3 — migrated from `AppArgs.taa` onto `BootstrapInputs.taa`.
        assert!(inputs.taa.enabled);
        // GI defaults match `GiSettings::DEFAULTS`; the round-trip is pinned
        // by `settings::tests::defaults_match_gi_settings_default`. Step 3
        // moved this field off `AppArgs.gi`.
        assert_eq!(inputs.gi, GiSettings::default());
        // Step 4 — `construction_config` migrated off `AppArgs` onto the
        // typed `BootstrapInputs.construction_config` field. The default is
        // `for_target_arch()` so the wasm32 clamp travels through the
        // bootstrap (Decision §5).
        assert_eq!(
            inputs.construction_config,
            crate::render::construction::ConstructionConfig::for_target_arch(),
        );
        // Step 5 — `grid_preset` migrated off `AppArgs` onto the typed
        // `BootstrapInputs.grid_preset` field. The default world content is
        // the hard-coded embedded test grid.
        assert_eq!(inputs.grid_preset, GridPreset::Default);
        // Step 8 — `spawn_test_entity` migrated off `AppArgs` onto the typed
        // `BootstrapInputs.spawn_test_entity` field. The fixture spawner is
        // off by default; the `--entities` e2e boot flips it.
        assert!(!inputs.spawn_test_entity.0);
        // Step 6 — the 10 e2e-mode booleans collapsed into the
        // `E2eGateMode` enum; the default is the standard gate flow.
        assert_eq!(inputs.gate_mode, crate::e2e::gate::E2eGateMode::Standard);
        // `vox_e2e_mode` (Bucket A) stays on `AppArgs` until Step 7; off
        // by default.
        assert!(!inputs.args.vox_e2e_mode);
    }
}
