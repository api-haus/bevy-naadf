//! `VoxelIoPlugin` — bundles all main-app voxel I/O wiring.
//!
//! Centralises the systems that drove the inline `lib.rs:setup_test_grid` + async
//! pump + wasm `web_vox` registrations + native drag-and-drop listener so the
//! `build_app_with_args` spine adds them with one `app.add_plugins(VoxelIoPlugin)`
//! call. D3 architect F4 + D7 architect Step 7 designed this; the D7
//! cleanup-follow-up landed the actual extraction.
//!
//! Self-gates on `Res<AppConfig>.add_e2e_systems` for the native drag-and-drop
//! pair (`voxel/grid::log_native_dnd_registered` + `native_vox_drop_listener`):
//! winit emits the drop events under e2e too, but the harness should never see
//! foreign input. Wasm-side systems (`startup_fetch_default_vox`,
//! `apply_pending_vox`, `pin_web_horizon_camera`) are unconditionally registered
//! under the wasm32 cfg.

use bevy::prelude::*;

use super::{async_vox, grid};

#[cfg(target_arch = "wasm32")]
use super::web_vox;

#[cfg(not(target_arch = "wasm32"))]
use crate::AppConfig;

pub struct VoxelIoPlugin;

impl Plugin for VoxelIoPlugin {
    fn build(&self, app: &mut App) {
        // The test grid + camera spawn — shared. On web,
        // `startup_fetch_default_vox` runs `.before(setup_test_grid)` so it can
        // mutate `AppArgs.grid_preset` to `GridPreset::WebSkybox`
        // (`?skybox=1` URL-param handling) before `setup_test_grid` reads it.
        app.add_systems(Startup, grid::setup_test_grid);

        // Async `.vox` parse pump (`web-vox-async-loading Step 4`, 2026-05-18).
        // The polling system drains the `PendingVoxParse` hand-off resource
        // produced by the target-specific async parse spawn (native:
        // `AsyncComputeTaskPool::spawn`; web: `rayon::spawn`). Resource +
        // system registered on BOTH targets so the cfg-gated internals share
        // one main-thread driver.
        app.init_resource::<async_vox::PendingVoxParse>()
            .add_systems(Update, async_vox::poll_pending_vox_parse);

        // Web-only .vox streaming: kick off the default-model HTTP fetch on
        // `Startup`, and run the consumer system on `Update` so both the fetch
        // and any drag-dropped `.vox` files swap the active scene the moment
        // their bytes are ready. The default scene from `setup_test_grid` stays
        // visible until then.
        //
        // Order: `apply_pending_vox` runs `.after(poll_pending_vox_parse)` so
        // its overlay-hide branch sees `pending.inner.is_none()` the same frame
        // the polling system clears the slot post-install.
        #[cfg(target_arch = "wasm32")]
        {
            app.add_systems(
                Startup,
                web_vox::startup_fetch_default_vox.before(grid::setup_test_grid),
            )
            .add_systems(
                Update,
                web_vox::apply_pending_vox.after(async_vox::poll_pending_vox_parse),
            )
            // 2026-05-19 — `?pose=horizon` URL-param camera pin. Runs every
            // frame when the override resource is present; bypasses FreeCamera
            // input so the cross-target SSIM gate's WASM-side capture is
            // deterministic. `.run_if(resource_exists)` keeps the scheduler
            // from invoking the system body when the param is absent (the
            // common case).
            .add_systems(
                Update,
                web_vox::pin_web_horizon_camera
                    .after(async_vox::poll_pending_vox_parse)
                    .run_if(bevy::ecs::schedule::common_conditions::resource_exists::<
                        web_vox::WebHorizonPoseOverride,
                    >),
            );
        }

        // Native drag-and-drop: drop a `.vox` file onto the window to replace
        // the active scene. Gated off the e2e harness — winit emits the event
        // in both modes but the e2e harness should never see foreign input.
        // Reads `Res<AppConfig>` (inserted by `build_app_with_args` before
        // `add_plugins(VoxelIoPlugin)`).
        #[cfg(not(target_arch = "wasm32"))]
        {
            let add_e2e_systems = app
                .world()
                .get_resource::<AppConfig>()
                .map(|c| c.add_e2e_systems)
                .unwrap_or(false);
            if !add_e2e_systems {
                app.add_systems(Startup, grid::log_native_dnd_registered)
                    .add_systems(Update, grid::native_vox_drop_listener);
            }
        }
    }
}
