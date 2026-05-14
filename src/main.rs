//! bevy-naadf — Bevy 0.19 + Solari raytracing + DLSS Ray Reconstruction.
//!
//! Foundation for porting the NAADF voxel renderer (`/mnt/archive4/DEV/NAADF`,
//! a C#/MonoGame engine) to Rust/Bevy. This proof of concept stands up a generic
//! Solari scene denoised by DLSS Ray Reconstruction, so the toolchain — DLSS SDK
//! build, Vulkan ray tracing, the Bevy 0.19 API — is proven before the port.
//!
//! Run with `--pathtracer` to use Solari's reference pathtracer (ground truth)
//! instead of the realtime lighting system.

mod camera;
mod hud;
mod scene;

use bevy::{
    camera_controller::free_camera::FreeCameraPlugin,
    diagnostic::FrameTimeDiagnosticsPlugin,
    prelude::*,
    render::diagnostic::RenderDiagnosticsPlugin,
    solari::{pathtracer::PathtracingPlugin, prelude::SolariPlugins},
};

#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy::anti_alias::dlss::DlssProjectId;

/// Command-line options, parsed once and stored as a resource.
#[derive(Resource, Clone, Copy)]
pub struct AppArgs {
    /// Use Solari's reference pathtracer instead of realtime lighting.
    pub pathtracer: bool,
}

fn main() {
    let args = AppArgs {
        pathtracer: std::env::args().any(|a| a == "--pathtracer"),
    };

    let mut app = App::new();

    // `DlssProjectId` must be inserted before `DefaultPlugins` so the render
    // sub-app sees it during DLSS initialisation. Generate your own UUID per
    // project — do not reuse this one in a shipping app.
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
    app.insert_resource(DlssProjectId(bevy::asset::uuid::uuid!(
        "8f6b1d2e-3c4a-4f5b-9a7c-1e2d3f4a5b6c"
    )));

    app.insert_resource(args).add_plugins((
        DefaultPlugins,
        SolariPlugins,
        FreeCameraPlugin,
        FrameTimeDiagnosticsPlugin::default(),
        RenderDiagnosticsPlugin,
    ));

    // The pathtracer is opt-in: its render node is only registered when asked
    // for, and `camera::setup_camera` then spawns a `Pathtracer` camera.
    if args.pathtracer {
        app.add_plugins(PathtracingPlugin);
    }

    app.add_systems(
        Startup,
        (scene::setup_scene, camera::setup_camera, hud::setup_hud),
    )
    .add_systems(Update, (camera::toggle_dlss, hud::update_hud));

    app.run();
}
