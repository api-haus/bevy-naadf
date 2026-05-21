//! Android entry point — wired by `cargo-ndk` + Gradle as the GameActivity
//! JNI bridge target.
//!
//! Bevy's `#[bevy_main]` proc-macro expands on `target_os = "android"` into a
//! `#[no_mangle] pub unsafe extern "C" fn android_main(...)` symbol that the
//! GameActivity JNI shim (linked into `libbevy_naadf.so` via the
//! `android-game-activity` Bevy feature) looks up after the APK loads. The
//! function below is what gets called once per activity startup.
//!
//! ## 2026-05-21 — budget-aware production entry
//!
//! The earlier minimal-probe build (`DefaultPlugins` only) was a stopgap to
//! survive the first Galaxy Tab A8 launch — the C#-faithful 256×32×256
//! fixed-world install OOM-rebooted the kernel on touch because the three
//! big storage-buffer bindings (`voxels` = 1024 MiB, `blocks` = 512 MiB,
//! `taa_samples` ≈ 720 MiB at depth=32) blew past Mali-G52's 256 MiB
//! `max_storage_buffer_binding_size` and the device's 2.5 GiB unified RAM.
//!
//! This entry now runs the GPU budget preselection routine BEFORE building
//! the real app:
//!
//!   1. [`crate::render::budget::probe_and_select`] spins up a throwaway
//!      render `App` with `MinimalPlugins + AssetPlugin + ImagePlugin +
//!      RenderPlugin`, reads `RenderDevice::limits()`, and drops the probe
//!      app. The cap (Mali = 256 MiB) drives the selection of safe values
//!      from the [`crate::render::budget::WORLD_SIZE_LADDER`] +
//!      [`crate::render::budget::TAA_RING_DEPTH_LADDER`] descending ladders.
//!      On Mali-G52 this returns `taa_ring_depth = 8, world_size_in_segments
//!      = (6, 2, 6)` (= 96×32×96 chunks = 1536×512×1536 voxels).
//!   2. The chosen TAA depth is written into [`crate::AppArgs`]; the chosen
//!      `EffectiveWorldSize` is inserted into the App after
//!      [`crate::build_app_with_args`] returns so the defensive seed inside
//!      that helper is overridden in-place (Bevy `insert_resource` second-
//!      call semantic is overwrite).
//!   3. The full Naadf plugin pyramid runs against the reduced budgets;
//!      `prepare_world_gpu` allocates `voxels` at 144 MiB, `blocks` at
//!      72 MiB, and `taa_samples` at ~192 MiB — every binding under the
//!      256 MiB cap with the 75% headroom factor.
//!
//! Mobile divergence is APPROVED per the user's faithful-port rule (locked
//! Q2 in `docs/orchestrate/mobile-budget/01-context.md`): the C# canonical
//! `(16, 2, 16)` segments const stays intact at
//! `crates/bevy_naadf/src/world_size.rs:16` together with its compile-time
//! pin test; the mobile divergence lives entirely in the [`EffectiveWorldSize`]
//! resource.

use bevy::prelude::*;
use bevy::window::WindowMode;
use bevy::winit::WinitSettings;

use crate::{build_app_with_budget, AppConfig, GridPreset};

#[bevy_main]
fn main() {
    // GPU budget preselection (probe → select → apply) lives in the shared
    // `build_app_with_budget` helper alongside the desktop / web entry. Same
    // probe-app, same `BudgetCaps`, same overrides — only the JNI cdylib entry
    // is Android-specific. See `crates/bevy_naadf/src/lib.rs::build_app_with_budget`.
    //
    // On Galaxy Tab A8 / Mali-G52 this picks `taa_ring_depth=8,
    // world_size_in_segments=(6,2,6), invalid_sample_storage_count=4` and emits
    // the `[budget] …` line to logcat (visible via `adb logcat | grep budget`).
    // Step 5 of the config-as-resource refactor — `grid_preset` is a
    // per-domain resource. Android has no CLI surface (no `--vox` flag), so
    // always boots into the default test grid. Step 9 removed the last
    // `AppArgs` reference — `build_app_with_budget` takes only the grid preset.
    let mut app = build_app_with_budget(
        AppConfig::windowed(),
        GridPreset::default(),
    );

    // Android-specific window config — full-screen borderless. Preserved from
    // the pre-budget minimal-probe entry; the Gradle project's `MainActivity`
    // is built around this shape.
    {
        let mut window_q = app.world_mut().query::<&mut Window>();
        if let Ok(mut window) = window_q.single_mut(app.world_mut()) {
            window.resizable = false;
            window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Primary);
        }
    }
    app.insert_resource(WinitSettings::mobile());

    app.run();
}
