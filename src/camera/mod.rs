//! Camera setup and the runtime DLSS Ray Reconstruction toggle.
//!
//! `FreeCameraPlugin` (added in `main`) handles the actual fly-camera movement;
//! here we just spawn the camera entity and provide a key to switch DLSS-RR
//! on/off at runtime. DLSS plumbing stays available (Phase-B-relevant) but is
//! dormant in Phase A.
//!
//! [`position_split`] holds NAADF's int+frac camera-relative position type (D1).

pub mod position_split;

use bevy::{camera_controller::free_camera::FreeCamera, prelude::*};

pub use position_split::{sync_position_split, PositionSplit};

#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy::{
    anti_alias::dlss::{Dlss, DlssRayReconstructionFeature, DlssRayReconstructionSupported},
    render::camera::{MipBias, TemporalJitter},
};

/// The components a `Dlss<DlssRayReconstructionFeature>` brings in as required
/// components. Removing this whole tuple toggles DLSS-RR off cleanly — in
/// particular it strips `TemporalJitter`, which would otherwise leave the
/// camera visibly jittering with no upscaler to resolve it. `DepthPrepass` /
/// `MotionVectorPrepass` are intentionally *not* included.
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

pub fn setup_camera(mut commands: Commands) {
    let start = Transform::from_xyz(11.0, 7.0, 17.0).looking_at(Vec3::new(0.0, 4.0, -3.0), Vec3::Y);
    commands.spawn((
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
        start,
        // NAADF's int+frac camera-relative position (D1). Seeded from the
        // spawn `Transform`; `sync_position_split` keeps it in step each frame.
        PositionSplit::from_world(start.translation),
        // The NAADF render path is compute + a fullscreen blit, not MSAA-
        // rasterised — keep MSAA off.
        Msaa::Off,
    ));
}

/// `D` toggles DLSS Ray Reconstruction on/off. Dormant in Phase A — the NAADF
/// render path is not wired to DLSS yet, this is kept for Phase B.
#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
pub fn toggle_dlss(
    keys: Res<ButtonInput<KeyCode>>,
    dlss_rr_supported: Option<Res<DlssRayReconstructionSupported>>,
    camera: Single<(Entity, Has<Dlss<DlssRayReconstructionFeature>>), With<Camera3d>>,
    mut commands: Commands,
) {
    if dlss_rr_supported.is_none() || !keys.just_pressed(KeyCode::KeyD) {
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
