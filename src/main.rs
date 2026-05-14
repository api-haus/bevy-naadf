//! bevy-naadf — Bevy 0.19 port of the NAADF voxel renderer.
//!
//! Port of NAADF (`/mnt/archive4/DEV/NAADF`, a C#/MonoGame engine — "Nested
//! Axis-Aligned Distance Fields", Ulschmid et al., CGF 2026) to Rust/Bevy.
//!
//! Phase A is the smallest runnable slice: a `PositionSplit` camera + a
//! hard-coded voxel test grid + the AADF three-layer cell structure + CPU-side
//! AADF construction + DDA-with-AADF traversal + an albedo first-hit WGSL
//! render. No Solari, no TAA, no world generator, no file I/O.

mod aadf;
mod camera;
mod hud;
mod render;
mod voxel;
mod world;

use bevy::{
    camera_controller::free_camera::FreeCameraPlugin,
    diagnostic::FrameTimeDiagnosticsPlugin,
    prelude::*,
    render::diagnostic::RenderDiagnosticsPlugin,
};

#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy::anti_alias::dlss::DlssProjectId;

/// Which hard-coded Phase-A test grid `voxel::grid::setup_test_grid` builds (D2).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum GridPreset {
    /// The default scene: ground slab + axis-aligned boxes + a sphere + one
    /// emissive box.
    #[default]
    Default,
}

/// Command-line options, parsed once and stored as a resource (`03-design.md` §4.1).
#[derive(Resource, Clone, Copy)]
pub struct AppArgs {
    /// Which hard-coded test grid to build (D2).
    pub grid_preset: GridPreset,
    /// Long-term TAA. Wired but always `false` in Phase A (D4) — Phase A-2
    /// turns it on.
    pub taa: bool,
}

fn main() {
    let args = AppArgs {
        grid_preset: GridPreset::default(),
        taa: false,
    };

    let mut app = App::new();

    // `DlssProjectId` must be inserted before `DefaultPlugins` so the render
    // sub-app sees it during DLSS initialisation. DLSS plumbing stays available
    // (Phase-B-relevant) but is dormant in Phase A. Generate your own UUID per
    // project — do not reuse this one in a shipping app.
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
    app.insert_resource(DlssProjectId(bevy::asset::uuid::uuid!(
        "8f6b1d2e-3c4a-4f5b-9a7c-1e2d3f4a5b6c"
    )));

    app.insert_resource(args).add_plugins((
        DefaultPlugins,
        FreeCameraPlugin,
        FrameTimeDiagnosticsPlugin::default(),
        RenderDiagnosticsPlugin,
    ));

    app.add_systems(
        Startup,
        (
            voxel::grid::setup_test_grid,
            camera::setup_camera,
            hud::setup_hud,
        ),
    )
    .add_systems(
        Update,
        (
            // `FreeCameraPlugin` drives the `Transform` in `RunFixedMainLoop`
            // (ordered before `Update`), so by the time `sync_position_split`
            // runs here the `Transform` is already current for this frame.
            camera::sync_position_split,
            camera::toggle_dlss,
            hud::update_hud,
        ),
    );

    app.run();
}
