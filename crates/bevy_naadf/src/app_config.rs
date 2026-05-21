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
    /// `Some(port)` ⇒ install the BRP (Bevy Remote Protocol) control server on
    /// `127.0.0.1:port` at the end of `build_app_core`. The external e2e runner
    /// drives the production binary over this channel — see
    /// `docs/orchestrate/e2e-ipc-rpc-restructure/02-design.md` §2.2. `None` for
    /// both the production (`windowed()`) and the legacy in-app e2e (`e2e()`)
    /// configs; only [`AppConfig::e2e_sut`] sets it. The BRP server code is
    /// behind the `e2e-brp` cargo feature, so this field is read only when that
    /// feature is enabled — with the feature off it is an inert `None`.
    pub brp_port: Option<u16>,
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
            brp_port: None,
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
            brp_port: None,
        }
    }

    /// The **e2e SUT (system-under-test) profile** — the production binary
    /// booted as the system-under-test for the external BRP-driven e2e runner
    /// (`docs/orchestrate/e2e-ipc-rpc-restructure/02-design.md` §2.4 / §5).
    ///
    /// It is the e2e *determinism profile* — HUD off, free camera off,
    /// *synchronous* pipeline compilation (frame-numbered assertions stay
    /// stable), a fixed 256×256 window — but with **`add_e2e_systems: false`**:
    /// the in-app bounded-frame driver / fixed-pose camera / readback systems
    /// stay off. The SUT is driven *externally* over BRP, not by an in-app
    /// driver mode. `brp_port: Some(port)` installs the BRP server on
    /// `127.0.0.1:port`.
    ///
    /// The `WinitSettings::Continuous` knob the BRP mailbox needs is installed
    /// by `e2e_brp::install_brp_server`, co-located with the server it serves
    /// (design §2.4) — it is *not* an `AppConfig` field.
    ///
    /// **Budget:** callers boot this profile through the bootstrap fan-out
    /// directly (`build_app_with_bootstrap_inputs`), **not** `build_app_with_budget`
    /// — the SUT forces the canonical memory budget rather than running the
    /// production `probe_and_select`, exactly as the legacy `e2e_render` path
    /// does (`lib.rs` `run_e2e_render` → `build_app`, skipping the probe). This
    /// keeps the canonical world / TAA rungs for deterministic SSIM across runs
    /// and machines (the design's hard-gate resolution).
    pub fn e2e_sut(port: u16) -> Self {
        Self {
            add_hud: false,
            add_free_camera: false,
            synchronous_pipeline_compilation: true,
            window: WindowConfig::e2e(),
            add_e2e_systems: false,
            brp_port: Some(port),
        }
    }
}
