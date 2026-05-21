//! BRP (Bevy Remote Protocol) control surface — the e2e-harness restructure.
//!
//! The external e2e runner (`naadf_e2e`) spawns the production binary
//! `bin/bevy-naadf` as the system-under-test and drives it over this BRP
//! server — see `docs/orchestrate/e2e-ipc-rpc-restructure/02-design.md`.
//!
//! ## Module split — what is feature-gated and what is not
//!
//! - [`schema`] is compiled **unconditionally** (no `bevy_remote` dependency,
//!   only `serde`). The `naadf_e2e` runner imports the param/return structs
//!   from here without building `bevy_naadf --features e2e-brp` (design §7.1
//!   D8 / A7).
//! - [`verbs`] (the BRP handlers) and [`install_brp_server`] are behind
//!   `#[cfg(feature = "e2e-brp")]` — they pull `bevy_remote` + the HTTP
//!   transport tail, which must stay out of the default production build.
//!
//! ## Server scaffold (Phases 1–2)
//!
//! Phase 0 (the transport spike) confirmed `bevy/bevy_remote` resolves against
//! the project's `bevy = "=0.19.0-rc.1"` pin (A1) and that
//! `WinitSettings::Continuous` keeps the `RemoteLast` mailbox draining on an
//! unfocused SUT (A2). Phase 1 landed the scaffold + the first three verbs
//! (`naadf/step`, `naadf/run_until_idle`, `naadf/get_state`). Phase 2 adds the
//! remaining eight verbs (`naadf/capture`, `naadf/await_capture`,
//! `naadf/apply_brush`, `naadf/set_camera`, `naadf/load_world`,
//! `naadf/region_gate`, `naadf/resize_window` — main-world — and
//! `naadf/pipeline_scan` — render-world) and wires the `PipelineScanResult`
//! cross-world channel + the render-world scan system into
//! [`install_brp_server`] so `naadf/get_state` and `naadf/pipeline_scan`
//! surface real pipeline health (design §6.3).
//!
//! The opt-in is the `AppConfig::brp_port` field (set by `AppConfig::e2e_sut`),
//! read at the end of `build_app_core`.

pub mod schema;

#[cfg(feature = "e2e-brp")]
pub mod verbs;

#[cfg(feature = "e2e-brp")]
mod install {
    use bevy::prelude::*;
    use bevy::remote::{http::RemoteHttpPlugin, RemotePlugin};
    use bevy::render::{Render, RenderApp, RenderSystems};
    use bevy::winit::{UpdateMode, WinitSettings};

    use crate::e2e::checks::{scan_pipeline_errors_render_system, PipelineScanResult};

    use super::verbs;

    /// Install the BRP server into `app`: the `naadf/*` custom verb set + the
    /// built-in BRP verbs + the HTTP transport on `127.0.0.1:port`, the
    /// frame-stepping resources / system the verbs depend on, and the
    /// cross-world `PipelineCache`-scan channel.
    ///
    /// Called from the end of `build_app_core` (design §2.2) when
    /// `AppConfig::brp_port` is `Some(port)` — after `DefaultPlugins` (so
    /// `RenderPlugin`'s render sub-app exists, required for the
    /// `with_method_render` registration of `naadf/pipeline_scan` and for
    /// inserting the render-world half of the pipeline-scan channel).
    ///
    /// ## `WinitSettings::Continuous` (design §2.4)
    ///
    /// The production `AppConfig::windowed()` uses the default `WinitSettings`,
    /// which drops to `reactive_low_power` when the window loses focus — an
    /// unfocused SUT ticking only on events would stall the `RemoteLast`
    /// mailbox drain. `Continuous`/`Continuous` guarantees the app ticks every
    /// frame regardless of focus. A2 of the Phase 0 spike confirmed it is
    /// sufficient. It mirrors what `e2e::add_e2e_systems` does for the legacy
    /// in-app harness (`e2e/mod.rs:242-245`).
    ///
    /// ## Pipeline-scan channel (design §6.3)
    ///
    /// `PipelineCache` is a render-world resource a main-world handler cannot
    /// reach. The legacy `add_e2e_systems` wired a `PipelineScanResult`
    /// `Arc<Mutex>` channel into *both* worlds; the `e2e_sut` profile has
    /// `add_e2e_systems: false`, so Phase 2 wires the same channel here. The
    /// render-world `scan_pipeline_errors_render_system` writes it every render
    /// frame; `naadf/get_state` reads the main-world side; `naadf/pipeline_scan`
    /// runs *in* the render world and reads it directly.
    pub fn install_brp_server(app: &mut App, port: u16) {
        // A2: keep the SUT ticking (and thus the BRP mailbox draining)
        // regardless of window focus.
        app.insert_resource(WinitSettings {
            focused_mode: UpdateMode::Continuous,
            unfocused_mode: UpdateMode::Continuous,
        });

        // The cross-world `PipelineCache`-scan channel — cloned into both the
        // main world (read by `naadf/get_state` + `naadf/pipeline_scan`) and
        // the `RenderApp` (written by `scan_pipeline_errors_render_system`).
        let pipeline_scan = PipelineScanResult::default();
        app.insert_resource(pipeline_scan.clone());

        // `RemotePlugin::default()` keeps the built-in verbs (`rpc.discover`,
        // `world.*`, `registry.*`, …) — they cost nothing extra and
        // `rpc.discover` is a useful smoke handle. The project's domain verbs
        // are all chained on as **main-world** custom methods
        // (`with_method_main` / `with_watching_method_main`, verified against
        // `bevy_remote 0.19.0-rc.1` `src/lib.rs:591-642`).
        //
        // `naadf/pipeline_scan` is main-world (NOT `with_method_render`) — see
        // its handler doc for the design-§3/D7 correction: the verb reads the
        // `PipelineScanResult` cross-world *channel*, whose main-world clone
        // carries the render-world scan's result; a render-world verb would
        // also force the runner onto `bevy_remote`'s fixed, un-overridable
        // render-subapp port (`15703`), colliding between concurrent gates.
        let plugin = RemotePlugin::default()
            // --- frame-stepping / status (Phase 1) ---------------------------
            .with_method_main("naadf/step", verbs::step)
            .with_watching_method_main("naadf/run_until_idle", verbs::run_until_idle)
            .with_method_main("naadf/get_state", verbs::get_state)
            // --- capture (Phase 2) -------------------------------------------
            .with_method_main("naadf/capture", verbs::capture)
            .with_watching_method_main("naadf/await_capture", verbs::await_capture)
            // --- world / camera / edit (Phase 2) -----------------------------
            .with_method_main("naadf/apply_brush", verbs::apply_brush)
            .with_method_main("naadf/set_camera", verbs::set_camera)
            .with_method_main("naadf/load_world", verbs::load_world)
            .with_method_main("naadf/region_gate", verbs::region_gate)
            .with_method_main("naadf/resize_window", verbs::resize_window)
            // --- demo-region voxel count (Phase 3a — small_edit_visual gate) -
            .with_method_main("naadf/count_demo_voxels", verbs::count_demo_voxels)
            // --- pipeline scan (Phase 2 — main-world, design correction) -----
            .with_method_main("naadf/pipeline_scan", verbs::pipeline_scan);
        app.add_plugins(plugin);

        // `RemoteHttpPlugin` — JSON-RPC 2.0 over loopback HTTP. `with_port`
        // sets the main-world server port (default 15702). Native-only (the
        // plugin is `cfg(not(wasm))`).
        app.add_plugins(RemoteHttpPlugin::default().with_port(port));

        // The frame-stepping gate the `naadf/step` / `naadf/run_until_idle`
        // verbs read + write, and the `advance_e2e_control` system that ticks
        // it every `Update` (design §4.1).
        app.init_resource::<verbs::E2eControl>();
        app.init_resource::<verbs::RunUntilIdleWatch>();
        app.init_resource::<verbs::AwaitCaptureWatch>();
        app.init_resource::<crate::e2e::readback::E2eScreenshot>();
        app.add_systems(Update, verbs::advance_e2e_control);

        // The render-world half of the pipeline scan: the scan system runs
        // every render frame in the `Render` schedule (after the prepare/queue
        // systems have queued pipelines) and writes the shared channel.
        // `RenderApp` is present because `RenderPlugin` (part of
        // `DefaultPlugins`) builds it before this install point.
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .insert_resource(pipeline_scan)
                .add_systems(
                    Render,
                    scan_pipeline_errors_render_system.after(RenderSystems::Render),
                );
        }

        info!(
            "[e2e-brp] BRP HTTP server installed on 127.0.0.1:{port} \
             (12 naadf/* verbs + built-in world.*/rpc.*; pipeline-scan channel wired)"
        );
    }
}

#[cfg(feature = "e2e-brp")]
pub use install::install_brp_server;
