//! Android entry point — wired by `cargo-ndk` + Gradle as the GameActivity
//! JNI bridge target.
//!
//! Bevy's `#[bevy_main]` proc-macro expands on `target_os = "android"` into a
//! `#[no_mangle] pub unsafe extern "C" fn android_main(...)` symbol that the
//! GameActivity JNI shim (linked into `libbevy_naadf.so` via the
//! `android-game-activity` Bevy feature) looks up after the APK loads. The
//! function below is what gets called once per activity startup.
//!
//! ## 2026-05-21 — minimal-probe build
//!
//! The first full-app launch on Galaxy Tab A8 (Mali-G52, 2.5 GiB shared RAM)
//! kernel-OOM'd hard enough to reboot the device — the C#-faithful
//! 256×32×256 fixed-world container's `voxels` (1 GiB) + `blocks` (512 MiB)
//! GPU buffers blow past the tablet's physical RAM ceiling. The world
//! install runs as part of `build_app_with_args`'s plugin pyramid
//! (`world::WorldPlugin` + `voxel::VoxelIoPlugin` + `render::NaadfRenderPlugin`),
//! so there's no way to bring the renderer up *without* triggering it.
//!
//! For now this entry point bypasses the whole pyramid and brings up only
//! `DefaultPlugins` so that:
//!
//!   1. We never reach the world-install allocations and the tablet survives.
//!   2. We can read `RenderDevice.limits()` from logcat — the data point we
//!      need to design Task #7 (scalable TAA + (V)RAM budget preselection).
//!
//! Once Task #7 lands, this file flips back to the real `build_app_with_args`
//! path with budget-aware sizing.

use bevy::prelude::*;
use bevy::render::renderer::{RenderAdapterInfo, RenderDevice};
use bevy::window::WindowMode;
use bevy::winit::WinitSettings;

#[bevy_main]
fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                resizable: false,
                mode: WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(WinitSettings::mobile())
        .add_systems(Startup, log_render_device_limits)
        .run();
}

/// Minimal-probe diagnostic: dump the wgpu adapter info + limits to logcat
/// as soon as the render device exists. The single line we actually need is
/// the `Limits` debug print — it pins `max_buffer_size`,
/// `max_storage_buffer_binding_size`, and the rest of the per-device caps
/// that Task #7's budget routine has to read. Mirrors the pattern Bevy's own
/// `examples/mobile/src/lib.rs` uses.
fn log_render_device_limits(
    device: Res<RenderDevice>,
    adapter_info: Res<RenderAdapterInfo>,
) {
    info!(
        "[naadf-probe] wgpu adapter: name={:?} vendor={:#x} device={:#x} \
         device_type={:?} driver={:?} driver_info={:?} backend={:?}",
        adapter_info.name,
        adapter_info.vendor,
        adapter_info.device,
        adapter_info.device_type,
        adapter_info.driver,
        adapter_info.driver_info,
        adapter_info.backend,
    );
    info!("[naadf-probe] wgpu device limits = {:#?}", device.limits());
    info!("[naadf-probe] wgpu device features = {:#?}", device.features());
}
