//! Command-line options, parsed once and stored as a Bevy `Resource`
//! (`03-design.md` §4.1).
//!
//! **Step 7 of the config-as-resource refactor** migrated the last
//! surviving field (`vox_e2e_mode`) off this struct onto its own
//! [`crate::e2e::VoxE2eAssertion`] per-domain resource (Bucket A — an
//! ASSERT-time data tag, not a flow selector — Decision §3). `AppArgs`
//! is now a zero-field shell; Step 9 deletes it and this file entirely.
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
/// After Steps 2-8 of the config-as-resource refactor every field has
/// migrated to its own per-domain resource — this is now a zero-field
/// shell. Step 9 deletes it and this file entirely.
#[derive(Resource, Clone, Default)]
pub struct AppArgs;

// Step 2 of the config-as-resource refactor (`02-design.md` §4 Step 2): the
// `taa_ring_depth` pin tests moved to `render/taa.rs::tests` along with the
// migrated field. No remaining `AppArgs`-rooted tests live in this file;
// subsequent migration steps may add per-bucket tests on their target
// resources, not here.
