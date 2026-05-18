//! Camera setup and the runtime DLSS Ray Reconstruction toggle.
//!
//! `FreeCameraPlugin` (added in `main`) handles the actual fly-camera movement;
//! here we just spawn the camera entity and provide a key to switch DLSS-RR
//! on/off at runtime. DLSS plumbing stays available (Phase-B-relevant) but is
//! dormant in Phase A.
//!
//! [`position_split`] holds NAADF's int+frac camera-relative position type (D1).

pub mod position_split;

use bevy::{
    camera::Hdr,
    camera_controller::free_camera::FreeCamera,
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
};

pub use position_split::{sync_position_split, PositionSplit};

/// World-size-derived camera pose, written by [`crate::voxel::grid::setup_test_grid`]'s
/// `GridPreset::Vox` arm so [`setup_camera`] can frame the loaded world instead
/// of the hard-coded test-grid pose.
///
/// **Faithful port** of the C# `WorldRender.Initialize` default (`Common/Camera.cs`
/// — `camPos = (500, 200, 40)` voxels, identity rotation → forward `(0, 0, 1)` —
/// `World/Render/WorldRender.cs:48-49`). In C# the default world is a fixed
/// 1024×128×1024 voxels and the camera coords are hand-tuned for it. To
/// generalise to any loaded `.vox` world size, we rescale the C# magic
/// coordinates proportionally to `(world_voxels_x, world_voxels_y, world_voxels_z)`:
///
/// ```text
/// pos.x = world_voxels.x * (500.0 / 1024.0) // ≈ 0.4883
/// pos.y = world_voxels.y * (200.0 / 128.0)  // ≈ 1.5625 — above the world ceiling
/// pos.z = world_voxels.z * (40.0  / 1024.0) // ≈ 0.0391 — near the −Z edge
/// look_at = pos + Vec3::Z                    // identity rotation, +Z forward
/// ```
///
/// In MonoGame Y-up (matches Bevy Y-up; the Z↔Y swap in `vox_import` is data-
/// side, the camera lives in world space and uses the same axes either way),
/// this scales the C# camera coords to whatever world the user loaded. For the
/// default 1024×128×1024 world it reproduces `(500, 200, 40)` exactly. For
/// Oasis 1488×544×1344 it produces `(726, 850, 52)`. For the e2e fixture
/// 64×32×64 it produces `(31.2, 50, 2.5)`.
///
/// **Not used** by the e2e harness — `e2e::setup_e2e_camera` spawns its own
/// fixed-pose camera and ignores this resource entirely (see
/// `e2e/gates.rs::e2e_camera_transform`).
#[derive(Resource, Clone, Copy, Debug)]
pub struct InitialCameraPose(pub Transform);

impl InitialCameraPose {
    /// Compute the C#-faithful initial camera pose for a loaded world of
    /// `[w, h, d]` voxels. See the type-level doc for the formula derivation.
    pub fn from_world_voxels(world_voxels: [u32; 3]) -> Self {
        let w = world_voxels[0] as f32;
        let h = world_voxels[1] as f32;
        let d = world_voxels[2] as f32;
        let pos = Vec3::new(w * (500.0 / 1024.0), h * (200.0 / 128.0), d * (40.0 / 1024.0));
        // C# default rotation is identity (`Matrix.CreateFromYawPitchRoll(0,0,0)`,
        // `Camera.cs:95`), which makes `camDir = (0, 0, 1)` (`Camera.cs:121`).
        // `looking_at(pos + Vec3::Z, Vec3::Y)` reproduces this — Y-up, +Z forward.
        let transform = Transform::from_translation(pos).looking_at(pos + Vec3::Z, Vec3::Y);
        InitialCameraPose(transform)
    }
}

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

pub fn setup_camera(
    mut commands: Commands,
    initial_pose: Option<Res<InitialCameraPose>>,
) {
    // If a `.vox` load (Δ-Vox arm of `setup_test_grid`) authored an
    // [`InitialCameraPose`], use it; otherwise fall back to the hard-coded
    // test-grid pose. The test-grid pose is preserved verbatim so the
    // `GridPreset::Default` path is byte-identical to pre-change behaviour.
    let start = match initial_pose.as_deref() {
        Some(InitialCameraPose(t)) => {
            info!(
                "camera::setup_camera: framing loaded world — pos=({:.2}, {:.2}, {:.2}), look_at=({:.2}, {:.2}, {:.2})",
                t.translation.x,
                t.translation.y,
                t.translation.z,
                t.translation.x + t.forward().x,
                t.translation.y + t.forward().y,
                t.translation.z + t.forward().z,
            );
            *t
        }
        None => Transform::from_xyz(11.0, 7.0, 17.0).looking_at(Vec3::new(0.0, 4.0, -3.0), Vec3::Y),
    };
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        // `Hdr` makes the `Core3d` view target an `Rgba16Float` HDR texture
        // (Bevy 0.19 split this off `Camera` into its own marker component —
        // `bevy_camera::Hdr`). The NAADF final-blit pass writes raw linear HDR
        // into it, and Bevy's built-in `tonemapping` render-graph node — which
        // runs after the NAADF passes (`render/mod.rs` chains them
        // `.before(tonemapping)`) — does the tonemap + sRGB encode.
        // TAA-fidelity fix #2: the port no longer tonemaps in
        // `naadf_final.wgsl` (`18-taa-fidelity.md` fix #2).
        Hdr,
        // Bevy's built-in tonemapper — `TonyMcMapface` (Bevy's default, the
        // idiomatic 0.19 choice). Replaces NAADF's custom Reinhard-ish tonemap.
        Tonemapping::default(),
        default_free_camera(),
        start,
        // NAADF's int+frac camera-relative position (D1). Seeded from the
        // spawn `Transform`; `sync_position_split` keeps it in step each frame.
        PositionSplit::from_world(start.translation),
        // The NAADF render path is compute + a fullscreen blit, not MSAA-
        // rasterised — keep MSAA off.
        Msaa::Off,
    ));
}

/// The canonical `FreeCamera` config for the production camera. Used at
/// spawn time and again by `crate::app_mode::restore_camera_input` when the
/// Escape overlay closes (which re-inserts the component).
///
/// Note: NOT using `bevy_state::DisableOnEnter`/`EnableOnExit` for this —
/// `Disabled` on a `Camera3d` entity makes Bevy's render-extraction queries
/// skip it, which blanks the entire screen (the UI also renders through the
/// camera). Toggling just the `FreeCamera` component keeps the camera
/// rendering while making `FreeCameraPlugin`'s input queries skip it.
pub fn default_free_camera() -> FreeCamera {
    FreeCamera {
        walk_speed: 4.0,
        run_speed: 14.0,
        ..default()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The C# anchor: `(500, 200, 40)` voxels in a 1024×128×1024 default world,
    /// identity rotation. Port formula reproduces this exactly.
    #[test]
    fn from_world_voxels_matches_csharp_default() {
        let pose = InitialCameraPose::from_world_voxels([1024, 128, 1024]);
        let t = pose.0;
        assert!(
            (t.translation - Vec3::new(500.0, 200.0, 40.0)).length() < 1e-3,
            "expected C# default (500, 200, 40), got {:?}",
            t.translation,
        );
        // Identity rotation → forward `(0, 0, 1)`.
        let fwd = t.forward();
        assert!(
            (Vec3::new(fwd.x, fwd.y, fwd.z) - Vec3::Z).length() < 1e-3,
            "expected forward +Z, got {:?}",
            fwd,
        );
    }

    /// Test-grid-sized world: 4×2×4 chunks = 64×32×64 voxels. The formula's
    /// proportional scaling produces a `(31.2, 50.0, 2.5)` pose looking +Z.
    #[test]
    fn from_world_voxels_scales_test_grid() {
        let pose = InitialCameraPose::from_world_voxels([64, 32, 64]);
        let t = pose.0;
        // x: 64 * 500/1024 = 31.25
        // y: 32 * 200/128  = 50.0
        // z: 64 * 40/1024  = 2.5
        assert!((t.translation - Vec3::new(31.25, 50.0, 2.5)).length() < 1e-3);
    }

    /// Oasis-sized world: 1488×544×1344 voxels. The formula's proportional
    /// scaling produces a `(726.6, 850.0, 52.5)` pose looking +Z.
    #[test]
    fn from_world_voxels_scales_oasis() {
        let pose = InitialCameraPose::from_world_voxels([1488, 544, 1344]);
        let t = pose.0;
        // x: 1488 * 500/1024 ≈ 726.5625
        // y:  544 * 200/128  = 850.0
        // z: 1344 *  40/1024 = 52.5
        assert!((t.translation - Vec3::new(726.5625, 850.0, 52.5)).length() < 1e-2);
    }
}
