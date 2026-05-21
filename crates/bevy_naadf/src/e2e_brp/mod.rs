//! BRP (Bevy Remote Protocol) control surface — Phase 0 transport spike.
//!
//! This whole module is behind the `e2e-brp` cargo feature; it is absent from
//! the default-feature production build (`cargo build --workspace`). It exists
//! to confirm assumptions A1 + A2 of the e2e-harness restructure design
//! (`docs/orchestrate/e2e-ipc-rpc-restructure/02-design.md`):
//!
//! - **A1** — `bevy/bevy_remote` resolves cleanly against the project's
//!   `bevy = "=0.19.0-rc.1"` pin.
//! - **A2** — `RemotePlugin` + `RemoteHttpPlugin` with
//!   `WinitSettings { Continuous, Continuous }` keep `process_remote_requests`
//!   (the `RemoteLast` mailbox drain) servicing requests on a running SUT,
//!   including while the SUT window is unfocused / backgrounded.
//!
//! Phase 0 is a deliberately minimal spike: it installs the default BRP verb
//! set + the HTTP transport and nothing else. The custom `naadf/*` verb set,
//! `E2eControl`, `AppConfig::brp_port`, and the `naadf_e2e` runner crate are
//! Phase 1+ — they are NOT in this module yet.

use bevy::prelude::*;
use bevy::remote::{http::RemoteHttpPlugin, RemotePlugin};
use bevy::winit::{UpdateMode, WinitSettings};

/// Install the BRP server (default verb set + HTTP transport on `port`) into
/// `app`, and force `WinitSettings::Continuous` in both focus modes so the
/// BRP mailbox keeps draining while the SUT window is unfocused (A2).
///
/// Design install point (§2.2): the end of `build_app_core`, after
/// `DefaultPlugins` (so `RenderPlugin`'s render sub-app exists — required for
/// the render-world custom-method registration in Phase 1+). For the Phase 0
/// spike only `RemotePlugin::default()` (built-in verbs) is installed; the
/// custom `naadf/*` methods land in Phase 1.
pub fn install_brp_server(app: &mut App, port: u16) {
    // A2: the production `AppConfig::windowed()` uses the default
    // `WinitSettings`, which drops to `reactive_low_power` when the window
    // loses focus — an unfocused SUT ticking only on events would stall the
    // `RemoteLast` mailbox drain. `Continuous`/`Continuous` guarantees the app
    // ticks every frame regardless of focus. This mirrors what
    // `e2e::add_e2e_systems` already does (`e2e/mod.rs:242-245`).
    app.insert_resource(WinitSettings {
        focused_mode: UpdateMode::Continuous,
        unfocused_mode: UpdateMode::Continuous,
    });

    // `RemotePlugin::default()` keeps the built-in verbs (`rpc.discover`,
    // `world.*`, `registry.*`, …) — they cost nothing extra and `rpc.discover`
    // is a useful spike smoke handle.
    app.add_plugins(RemotePlugin::default());

    // `RemoteHttpPlugin` — JSON-RPC 2.0 over loopback HTTP. `with_port` sets
    // the main-world server port (default 15702); the render sub-app server,
    // if any, binds `port + 1`. Native-only (the plugin is `cfg(not(wasm))`).
    app.add_plugins(RemoteHttpPlugin::default().with_port(port));

    info!("[e2e-brp] BRP HTTP server installed on 127.0.0.1:{port}");
}
