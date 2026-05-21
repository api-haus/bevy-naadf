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

use crate::{AppArgs, AppConfig};

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
}

/// Build the bevy-naadf `App` from an [`AppConfig`] and a [`BootstrapInputs`].
///
/// Step-1 shape — forwards to [`crate::build_app_with_args`]. Subsequent
/// steps replace the forward with a direct fan-out that inserts per-domain
/// resources from the typed fields on `inputs`.
pub fn build_app_with_bootstrap_inputs(cfg: AppConfig, inputs: BootstrapInputs) -> App {
    crate::build_app_with_args(cfg, inputs.args)
}

/// Boot the bounded windowed e2e render test from a [`BootstrapInputs`] and
/// return its [`AppExit`].
///
/// Step-1 shape — forwards to [`crate::run_e2e_render_with_args`].
/// Subsequent steps replace the forward with the per-domain resource fan-out
/// shape described in `02-design.md` §3.3.
pub fn run_e2e_render_with_bootstrap_inputs(inputs: BootstrapInputs) -> AppExit {
    crate::run_e2e_render_with_args(inputs.args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GridPreset, DEFAULT_TAA_RING_DEPTH};

    /// `BootstrapInputs::default()` must wrap the canonical `AppArgs::default()`
    /// — this is the e2e-determinism contract from the design doc's
    /// "E2e-path byte-identical defaults" section. `AppArgs` is neither
    /// `Debug` nor `PartialEq`, so this test spot-checks the load-bearing
    /// fields field-by-field rather than via a single equality assertion.
    /// If any of these spot-checks ever fails, the migration has introduced
    /// a default divergence and the e2e-determinism requirement is violated.
    #[test]
    fn default_wraps_canonical_app_args_defaults() {
        let inputs = BootstrapInputs::default();
        // The user's named smell — must stay at the documented constant.
        assert_eq!(inputs.args.taa_ring_depth, DEFAULT_TAA_RING_DEPTH);
        // TAA on by default (Phase A-2; both production and e2e boot TAA on).
        assert!(inputs.args.taa);
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
