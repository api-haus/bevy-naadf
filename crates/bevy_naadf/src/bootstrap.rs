//! Bootstrap-time configuration carrier.
//!
//! [`BootstrapInputs`] is the transient parser-output that the production
//! binaries (`main.rs`, `android_main.rs`) build from CLI / argv /
//! build-constants / device-probe values, and that the App bootstrap fans
//! into per-domain Bevy resources before the App runs. It is
//! the non-budget analogue of [`crate::render::budget::BudgetCaps`] —
//! transient at boot, dropped after the resource fan-out, never stored as a
//! `Resource` itself.
//!
//! Introduced at **Step 1** of the config-as-resource refactor
//! (`docs/orchestrate/config-as-resource-refactor/02-design.md`). It began
//! as a thin wrapper around the old `AppArgs` god-resource; Steps 2-8
//! progressively moved each field out into a typed per-domain resource
//! field on this struct, and **Step 9** deleted the now-empty `AppArgs`
//! shell entirely. The migration is complete — every config value travels
//! as its own per-domain resource.
//!
//! [`build_app_with_bootstrap_inputs`] fans `BootstrapInputs` into the
//! per-domain resources. It is the boot funnel both the production binary
//! (`main.rs`, native + wasm) and the e2e SUT path route through.

use bevy::prelude::App;

use crate::render::construction::{ConstructionConfig, SpawnTestEntity};
use crate::render::taa::{TaaConfig, TaaRingConfig};
use crate::{AppConfig, GiSettings, GridPreset};

/// Transient bootstrap-time configuration carrier.
///
/// Every field is a typed per-domain resource (Decision §2) that the
/// [`build_app_with_bootstrap_inputs`] fan-out inserts. Steps 2-8 of the
/// config-as-resource refactor progressively drained the original
/// `AppArgs` god-resource into these per-domain fields; Step 7 migrated
/// the last field (`vox_e2e_mode` → `vox_e2e_assertion`), so the legacy
/// `args: AppArgs` field is gone.
///
/// `BootstrapInputs::default()` produces every per-domain resource at its
/// canonical default, which is byte-identical to what the old
/// `AppArgs::default()` god-resource produced — the e2e-determinism
/// requirement (`docs/orchestrate/config-as-resource-refactor/01-context.md`
/// — "E2e-path byte-identical defaults").
#[derive(Clone)]
pub struct BootstrapInputs {
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
    /// `SpawnTestEntity(false)`; the `--e2e-entities` SUT spawn flag flips
    /// it on. The fan-out inserts it as a main-world `Res<SpawnTestEntity>`
    /// that gates `spawn_phase_c_test_entity`.
    pub spawn_test_entity: SpawnTestEntity,
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
            taa_ring_depth: TaaRingConfig::default(),
            taa: TaaConfig::default(),
            gi: GiSettings::default(),
            construction_config: ConstructionConfig::for_target_arch(),
            grid_preset: GridPreset::default(),
            spawn_test_entity: SpawnTestEntity::default(),
        }
    }
}

/// Build the bevy-naadf `App` from an [`AppConfig`] and a [`BootstrapInputs`].
///
/// Calls [`crate::build_app_core`] for the shared plugin-pyramid wiring +
/// the defensive per-domain resource seeds, then `insert_resource`-overwrites
/// each seed with the caller's `inputs` value. After Step 9 of the
/// config-as-resource refactor the `AppArgs` god-resource is fully gone —
/// every config value travels as its own per-domain resource.
pub fn build_app_with_bootstrap_inputs(cfg: AppConfig, inputs: BootstrapInputs) -> App {
    let mut app = crate::build_app_core(cfg);
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
    app
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
        // off by default; the `--e2e-entities` SUT spawn flag flips it.
        assert!(!inputs.spawn_test_entity.0);
    }
}
