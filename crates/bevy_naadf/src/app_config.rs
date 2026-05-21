//! The four deliberate, minimal ways the e2e app differs from the production
//! app (`e2e-render-test.md` §2.2 / §9). Everything else — `DefaultPlugins`,
//! `WinitPlugin`, the real window, the asset path, `WorldPlugin`,
//! `NaadfRenderPlugin`, the diagnostics plugins — is *identical*, so the e2e
//! run exercises the real boot path, not a near-copy of it.

use bevy::prelude::*;

use crate::WindowConfig;

/// The four deliberate, minimal ways the e2e app differs from the production
/// app (`e2e-render-test.md` §2.2 / §9). Inserted as a `Resource` at the top
/// of `build_app_with_args` so plugins can `.run_if(|cfg: Res<AppConfig>|
/// …)` on its fields (e.g. `DiagnosticsPlugin` self-skips under e2e,
/// `CameraPlugin` self-skips its `setup_camera` under e2e).
#[derive(Resource, Clone, Copy, Debug)]
pub struct AppConfig {
    /// Add the diagnostics HUD overlay (`setup_hud` / `update_hud`).
    pub add_hud: bool,
    /// Add `FreeCameraPlugin` + the runtime DLSS toggle (the fly camera).
    pub add_free_camera: bool,
    /// `RenderPlugin { synchronous_pipeline_compilation, .. }` — the e2e config
    /// flips this on so `PipelineCache` resolves every queued pipeline to
    /// `Ok`/`Err` within the same `app.update()`, making the bounded-frame run
    /// deterministic (`e2e-render-test.md` §2.2 point 1).
    pub synchronous_pipeline_compilation: bool,
    /// Window sizing/title.
    pub window: WindowConfig,
    /// Add the e2e bounded-frame driver + readback + assertion systems + the
    /// `WinitSettings::game()`-style `Continuous` update mode + the fixed-pose
    /// camera (`e2e-render-test.md` §4 / §6 / §2.2 point 2).
    pub add_e2e_systems: bool,
}

impl AppConfig {
    /// The production config: HUD on, free camera on, async pipeline
    /// compilation (no startup hitch), platform-default window, no e2e systems.
    pub fn windowed() -> Self {
        Self {
            add_hud: true,
            add_free_camera: true,
            synchronous_pipeline_compilation: false,
            window: WindowConfig::windowed(),
            add_e2e_systems: false,
        }
    }

    /// The e2e config: HUD off, free camera off, *synchronous* pipeline
    /// compilation, a 256×256 non-resizable window, e2e systems on
    /// (`e2e-render-test.md` §2.2 / §9).
    pub fn e2e() -> Self {
        Self {
            add_hud: false,
            add_free_camera: false,
            synchronous_pipeline_compilation: true,
            window: WindowConfig::e2e(),
            add_e2e_systems: true,
        }
    }
}
