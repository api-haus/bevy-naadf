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
    asset::AssetPlugin,
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

/// The Phase-B GI pipeline settings (`09-design-b.md` §3.8). The C#
/// `WorldRenderBase` ImGui sliders (`SettingDataRenderBase`) become these
/// `AppArgs` constants — there is no GI settings GUI in the port (§1). The
/// values are the C# slider *defaults*.
#[derive(Clone, Copy, Debug)]
pub struct GiSettings {
    /// Max secondary-ray bounce count (C# `bounceCount`).
    pub bounce_count: u32,
    /// GI accumulation-ring depth (C# `globalIllumMaxAccum`).
    pub global_illum_max_accum: u32,
    /// Spatial-resampling neighbour-search size (C# `spatialResampleSize`).
    pub spatial_resample_size: f32,
    /// Spatial-resampling visibility ray-step count (C# `spatialVisibilityCount`).
    pub spatial_visibility_count: u32,
    /// Denoiser threshold (C# `denoiseThresh`).
    pub denoise_thresh: f32,
    /// Lit-radius factor (C# `radiusLitFactor`).
    pub radius_lit_factor: f32,
    /// Noise-suppression factor (C# `noiseSuppressionFactor`).
    pub noise_suppression_factor: f32,
    /// The 1↔0.25-spp toggle (C# `skipSamples`) — drives `rayQueueCalc`.
    pub skip_samples: bool,
    /// Run the sparse bilateral denoiser (C# `isDenoise`).
    pub is_denoise: bool,
    /// Brightness-level the bucket samples (C# `isSampleLeveling`).
    pub is_sample_leveling: bool,
    /// Vary the spatial-resampling radius per pixel (C# `isVaryingResmaplingRadius`).
    pub is_varying_resampling_radius: bool,
    /// Apply the in-volume atmosphere interaction (C# `isAtmosphereInteraction`).
    pub is_atmosphere_interaction: bool,
}

impl Default for GiSettings {
    fn default() -> Self {
        // The `SettingDataRenderBase` defaults (`WorldRenderBase.cs:14-25`).
        Self {
            bounce_count: 3,
            global_illum_max_accum: 128,
            spatial_resample_size: 500.0,
            spatial_visibility_count: 80,
            denoise_thresh: 400.0,
            radius_lit_factor: 3.0,
            noise_suppression_factor: 0.4,
            skip_samples: true,
            is_denoise: true,
            is_sample_leveling: true,
            is_varying_resampling_radius: true,
            is_atmosphere_interaction: true,
        }
    }
}

/// Command-line options, parsed once and stored as a resource (`03-design.md` §4.1).
#[derive(Resource, Clone, Copy)]
pub struct AppArgs {
    /// Which hard-coded test grid to build (D2).
    pub grid_preset: GridPreset,
    /// Long-term TAA. Wired but always `false` in Phase A (D4) — Phase A-2
    /// turns it on.
    pub taa: bool,
    /// The Phase-B GI pipeline settings (`09-design-b.md` §3.8).
    pub gi: GiSettings,
}

fn main() {
    let args = AppArgs {
        grid_preset: GridPreset::default(),
        taa: true,
        gi: GiSettings::default(),
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

    app.insert_resource(args)
        // The 128-deep camera-history ring + the monotonic frame counter
        // (`06-design-a2.md` §2.3). Main-world resource, `Default`-seeded,
        // updated each frame by `update_camera_history`.
        .init_resource::<render::taa::CameraHistory>()
        .add_plugins((
            // The NAADF WGSL render shaders live in `src/assets/shaders/`
            // (`03-design.md` §1 module layout) — point the asset server there.
            DefaultPlugins.set(AssetPlugin {
                file_path: "src/assets".to_string(),
                ..default()
            }),
            FreeCameraPlugin,
            FrameTimeDiagnosticsPlugin::default(),
            RenderDiagnosticsPlugin,
            world::WorldPlugin,
            render::NaadfRenderPlugin,
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
            // The camera-history ring update must run *after*
            // `sync_position_split` so the ring stores this frame's current
            // camera state (`06-design-a2.md` §9.3).
            render::taa::update_camera_history.after(camera::sync_position_split),
            camera::toggle_dlss,
            hud::update_hud,
        ),
    );

    app.run();
}
