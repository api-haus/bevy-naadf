//! BRP (Bevy Remote Protocol) control surface — the e2e-harness restructure.
//!
//! This whole module is behind the `e2e-brp` cargo feature; it is absent from
//! the default-feature production build (`cargo build --workspace`). The
//! external e2e runner spawns the production binary `bin/bevy-naadf` as the
//! system-under-test and drives it over this BRP server — see
//! `docs/orchestrate/e2e-ipc-rpc-restructure/02-design.md`.
//!
//! ## Phase 1 — BRP server scaffold
//!
//! Phase 0 (the transport spike) confirmed `bevy/bevy_remote` resolves against
//! the project's `bevy = "=0.19.0-rc.1"` pin (A1) and that
//! `WinitSettings::Continuous` keeps the `RemoteLast` mailbox draining on an
//! unfocused SUT (A2). Phase 1 turns that minimal seed into the real server
//! scaffold:
//!
//! - the real [`install_brp_server`] — `RemotePlugin` with the custom verbs
//!   chained, `RemoteHttpPlugin`, `WinitSettings::Continuous`, [`verbs::E2eControl`]
//!   + [`verbs::RunUntilIdleWatch`] resources, and the [`verbs::advance_e2e_control`]
//!   frame-stepping system;
//! - the three Phase-1 verbs only — `naadf/step`, `naadf/run_until_idle`,
//!   `naadf/get_state` (design §3 / §4). The other 8 verbs are Phase 2.
//!
//! The opt-in is the `AppConfig::brp_port` field (set by `AppConfig::e2e_sut`),
//! read at the end of `build_app_core` — Phase 1 replaces Phase 0's temporary
//! `BEVY_NAADF_E2E_BRP_PORT` env-var bridge.

pub mod verbs;

use bevy::prelude::*;
use bevy::remote::{http::RemoteHttpPlugin, RemotePlugin};
use bevy::winit::{UpdateMode, WinitSettings};

/// Install the BRP server into `app`: the `naadf/*` custom verb set + the
/// built-in BRP verbs + the HTTP transport on `127.0.0.1:port`, plus the
/// frame-stepping resources / system the verbs depend on.
///
/// Called from the end of `build_app_core` (design §2.2) when
/// `AppConfig::brp_port` is `Some(port)` — after `DefaultPlugins` (so
/// `RenderPlugin`'s render sub-app exists, required for the future
/// `with_method_render` registration in Phase 2).
///
/// ## `WinitSettings::Continuous` (design §2.4)
///
/// The production `AppConfig::windowed()` uses the default `WinitSettings`,
/// which drops to `reactive_low_power` when the window loses focus — an
/// unfocused SUT ticking only on events would stall the `RemoteLast` mailbox
/// drain. `Continuous`/`Continuous` guarantees the app ticks every frame
/// regardless of focus. This knob is co-located here because the BRP server is
/// the only thing that needs it; A2 of the Phase 0 spike confirmed it is
/// sufficient. It mirrors what `e2e::add_e2e_systems` does for the legacy
/// in-app harness (`e2e/mod.rs:242-245`).
pub fn install_brp_server(app: &mut App, port: u16) {
    // A2: keep the SUT ticking (and thus the BRP mailbox draining) regardless
    // of window focus.
    app.insert_resource(WinitSettings {
        focused_mode: UpdateMode::Continuous,
        unfocused_mode: UpdateMode::Continuous,
    });

    // `RemotePlugin::default()` keeps the built-in verbs (`rpc.discover`,
    // `world.*`, `registry.*`, …) — they cost nothing extra and `rpc.discover`
    // is a useful smoke handle. The project's domain verbs are chained on as
    // custom methods; Phase 1 registers only the three frame-stepping / status
    // verbs (design §9 Phase 1), all main-world (`with_method_main` /
    // `with_watching_method_main`, verified `bevy_remote 0.19.0-rc.1`
    // `src/lib.rs:591,632`). The remaining 8 verbs land in Phase 2.
    let plugin = RemotePlugin::default()
        .with_method_main("naadf/step", verbs::step)
        .with_watching_method_main("naadf/run_until_idle", verbs::run_until_idle)
        .with_method_main("naadf/get_state", verbs::get_state);
    app.add_plugins(plugin);

    // `RemoteHttpPlugin` — JSON-RPC 2.0 over loopback HTTP. `with_port` sets
    // the main-world server port (default 15702). Native-only (the plugin is
    // `cfg(not(wasm))`).
    app.add_plugins(RemoteHttpPlugin::default().with_port(port));

    // The frame-stepping gate the `naadf/step` / `naadf/run_until_idle` verbs
    // read + write, and the `advance_e2e_control` system that ticks it every
    // `Update` (design §4.1).
    app.init_resource::<verbs::E2eControl>();
    app.init_resource::<verbs::RunUntilIdleWatch>();
    app.add_systems(Update, verbs::advance_e2e_control);

    info!("[e2e-brp] BRP HTTP server installed on 127.0.0.1:{port} (naadf/step, naadf/run_until_idle, naadf/get_state)");
}
