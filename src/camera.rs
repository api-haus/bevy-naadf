//! Camera setup and the runtime DLSS Ray Reconstruction toggle.
//!
//! `FreeCameraPlugin` (added in `main`) handles the actual fly-camera movement;
//! here we just spawn the camera entity with the components Solari and DLSS-RR
//! need, and provide a key to switch DLSS-RR on/off at runtime.

use bevy::{
    camera::CameraMainTextureUsages,
    camera_controller::free_camera::FreeCamera,
    prelude::*,
    render::render_resource::TextureUsages,
    solari::{pathtracer::Pathtracer, prelude::SolariLighting},
};

use crate::AppArgs;

#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy::{
    anti_alias::dlss::{Dlss, DlssRayReconstructionFeature, DlssRayReconstructionSupported},
    render::camera::{MipBias, TemporalJitter},
};

/// The components a `Dlss<DlssRayReconstructionFeature>` brings in as required
/// components. Removing this whole tuple toggles DLSS-RR off cleanly — in
/// particular it strips `TemporalJitter`, which would otherwise leave the
/// camera visibly jittering with no upscaler to resolve it. `DepthPrepass` /
/// `MotionVectorPrepass` are intentionally *not* included: they are cheap and
/// also useful to Solari, so we leave them in place across toggles.
#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
type DlssRrComponents = (Dlss<DlssRayReconstructionFeature>, TemporalJitter, MipBias);

#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
fn dlss_rr() -> Dlss<DlssRayReconstructionFeature> {
    Dlss::<DlssRayReconstructionFeature> {
        perf_quality_mode: Default::default(),
        reset: Default::default(),
        _phantom_data: Default::default(),
    }
}

pub fn setup_camera(
    mut commands: Commands,
    args: Res<AppArgs>,
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))] dlss_rr_supported: Option<
        Res<DlssRayReconstructionSupported>,
    >,
) {
    let mut camera = commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        FreeCamera {
            walk_speed: 4.0,
            run_speed: 14.0,
            ..default()
        },
        Transform::from_xyz(11.0, 7.0, 17.0).looking_at(Vec3::new(0.0, 4.0, -3.0), Vec3::Y),
        // `Msaa::Off` and `CameraMainTextureUsages` with `STORAGE_BINDING` are
        // both required for Solari.
        CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING),
        Msaa::Off,
    ));

    if args.pathtracer {
        camera.insert(Pathtracer::default());
    } else {
        camera.insert(SolariLighting::default());
    }

    // DLSS Ray Reconstruction denoises (and upscales) Solari's noisy output and
    // is _highly_ recommended with Solari. It is only meaningful for realtime
    // lighting — the reference pathtracer converges on its own.
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
    if !args.pathtracer && dlss_rr_supported.is_some() {
        camera.insert(dlss_rr());
    }
}

/// `D` toggles DLSS Ray Reconstruction on/off so the raw (noisy) Solari output
/// can be compared directly against the denoised result.
#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
pub fn toggle_dlss(
    keys: Res<ButtonInput<KeyCode>>,
    args: Res<AppArgs>,
    dlss_rr_supported: Option<Res<DlssRayReconstructionSupported>>,
    camera: Single<(Entity, Has<Dlss<DlssRayReconstructionFeature>>), With<Camera3d>>,
    mut commands: Commands,
) {
    if args.pathtracer || dlss_rr_supported.is_none() || !keys.just_pressed(KeyCode::KeyD) {
        return;
    }

    let (entity, has_dlss) = *camera;
    if has_dlss {
        commands.entity(entity).remove::<DlssRrComponents>();
    } else {
        commands.entity(entity).insert(dlss_rr());
    }
}

/// No-op when the crate is built without DLSS support.
#[cfg(any(not(feature = "dlss"), feature = "force_disable_dlss"))]
pub fn toggle_dlss() {}
