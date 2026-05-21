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
//! The Step-1 wrappers ([`build_app_with_bootstrap_inputs`],
//! [`run_e2e_render_with_bootstrap_inputs`]) forward to the existing
//! [`crate::build_app_with_args`] / [`crate::run_e2e_render_with_args`]
//! entry points. **No behaviour change at Step 1** — these wrappers are
//! additive scaffolding; production callers continue to use the existing
//! entry points until later steps migrate them.

use bevy::prelude::{App, AppExit};

use crate::render::taa::{TaaConfig, TaaRingConfig};
use crate::{AppArgs, AppConfig, GiSettings};

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
#[derive(Clone, Default)]
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
    app
}

/// Boot the bounded windowed e2e render test from a [`BootstrapInputs`] and
/// return its [`AppExit`].
///
/// Step-2 shape — mirrors [`build_app_with_bootstrap_inputs`]: forward to
/// [`crate::run_e2e_render_with_args`] for the not-yet-migrated `AppArgs`
/// fields, then perform a post-build resource insert for fields already
/// migrated. As more fields migrate, this becomes a pure resource fan-out.
///
/// `run_e2e_render_with_args` calls `e2e::run_with_app` which drives the
/// `AppExit` to completion. To preserve byte-identical determinism after
/// the resource insert, this wrapper takes the inverted path: build the App
/// via [`build_app_with_bootstrap_inputs`] (the migrated-resource fan-out)
/// + the e2e window config + the e2e runner.
pub fn run_e2e_render_with_bootstrap_inputs(inputs: BootstrapInputs) -> AppExit {
    let mut cfg = AppConfig::e2e();
    cfg.window = crate::window_config::window_for_e2e_args(&inputs.args);
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
        // The default world content is the hard-coded test grid.
        assert_eq!(inputs.args.grid_preset, GridPreset::Default);
        // The fixture spawner is off by default; `--entities` flips it.
        assert!(!inputs.args.spawn_test_entity);
        // No e2e gate is active by default.
        assert!(!inputs.args.resize_test);
        assert!(!inputs.args.vox_e2e_mode);
        assert!(!inputs.args.oasis_edit_visual_mode);
        assert!(!inputs.args.small_edit_visual_mode);
        assert!(!inputs.args.small_edit_repro_mode);
        assert!(!inputs.args.vox_gpu_construction_mode);
        assert!(!inputs.args.vox_gpu_oracle_cpu_phase);
        assert!(!inputs.args.vox_gpu_oracle_gpu_phase);
        assert!(!inputs.args.vox_web_parity_skybox_phase);
        assert!(!inputs.args.vox_web_parity_loaded_phase);
        assert!(!inputs.args.vox_horizon_native_phase);
    }
}
