//! Command-line options, parsed once and stored as a Bevy `Resource`
//! (`03-design.md` §4.1).
//!
//! After Step 6 of the config-as-resource refactor this carries ONLY the
//! single `vox_e2e_mode` boolean — the 10 mutually-exclusive e2e-mode
//! booleans collapsed into the [`crate::e2e::gate::E2eGateMode`] enum
//! `Resource` (Bucket B — Mode). `vox_e2e_mode` is Bucket A (an
//! ASSERT-time data tag, not a flow selector — Decision §3) and migrates
//! to its own `VoxE2eAssertion` resource in Step 7; Step 9 then deletes
//! this now-vestigial shell.
//!
//! **Step 2 of the config-as-resource refactor** migrated the user's named
//! smell `taa_ring_depth` out of this struct onto the
//! [`crate::render::taa::TaaRingConfig`] per-domain main-world resource. The
//! pin tests moved with it to `crates/bevy_naadf/src/render/taa.rs::tests`.
//!
//! **Step 3 of the config-as-resource refactor** migrated `taa: bool` and
//! `gi: GiSettings` onto the per-domain
//! [`crate::render::taa::TaaConfig`] / [`crate::GiSettings`] resources. The
//! settings panel, the diagnostics dump, and the render-world extract systems
//! now read those resources directly; nothing in this file references TAA or
//! GI any more.
//!
//! **Step 4 of the config-as-resource refactor** migrated
//! `construction_config: ConstructionConfig` onto the per-domain
//! [`crate::render::construction::ConstructionConfig`] resource. Bootstrap
//! inserts it from `BootstrapInputs.construction_config`; the render sub-app
//! mirror is extract-driven; the wasm32 divergence (previously inside the
//! deleted `From<&AppArgs>` impl) now lives on
//! `ConstructionConfig::for_target_arch()` (Decision §5).
//!
//! **Step 5 of the config-as-resource refactor** migrated
//! `grid_preset: GridPreset` onto a per-domain main-world resource. The
//! native `--vox <path>` flag and the wasm32 `?skybox=1` URL param now
//! resolve into `BootstrapInputs.grid_preset` BEFORE the App is built;
//! `setup_test_grid` reads `Res<GridPreset>` instead of `Res<AppArgs>`.

use bevy::prelude::*;

/// Command-line options, parsed once and stored as a resource
/// (`03-design.md` §4.1).
///
/// After Steps 2-6 + 8 of the config-as-resource refactor only the single
/// `vox_e2e_mode` boolean (Bucket A — migrate in Step 7) remains here.
/// Steps 7/9 drain the shell.
#[derive(Resource, Clone)]
pub struct AppArgs {
    /// When `true`, the e2e driver swaps the default `assert_batch_6`
    /// region gates for the `--vox-e2e` "non-skybox" assertion. The
    /// default-scene gate rectangles (`solid_block_rect`, `emissive_rect`,
    /// etc.) are tuned for the hard-coded test grid's content layout, so
    /// they don't apply when [`GridPreset::Vox`] loaded a different scene.
    ///
    /// Permanent regression coverage for the `.vox` ingestion path landed
    /// in Track A (`docs/orchestrate/feature-completeness/03a-impl-vox-loading.md`)
    /// — the brief explicitly required an automated assert that the
    /// framebuffer captures something other than skybox after loading a
    /// `.vox` file through the production `--vox` path. See
    /// [`crate::e2e::vox_e2e`].
    pub vox_e2e_mode: bool,
}

impl Default for AppArgs {
    fn default() -> Self {
        Self {
            vox_e2e_mode: false,
        }
    }
}

// Step 2 of the config-as-resource refactor (`02-design.md` §4 Step 2): the
// `taa_ring_depth` pin tests moved to `render/taa.rs::tests` along with the
// migrated field. No remaining `AppArgs`-rooted tests live in this file;
// subsequent migration steps may add per-bucket tests on their target
// resources, not here.
