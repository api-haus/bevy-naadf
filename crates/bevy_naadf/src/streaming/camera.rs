//! `streaming::camera` — production-side camera-position bookkeeping for the
//! streaming preset (`docs/orchestrate/streaming-world/03j-diagnosis-camera-nudge-loop.md`
//! § Punch-list, Phase 2.9 fix).
//!
//! ## Why this exists
//!
//! `FreeCameraPlugin::run_freecamera_controller` writes
//! `Transform.translation += velocity * dt * {right/up/forward}` in **absolute
//! world voxel coords** — the controller knows nothing about the residency
//! window. The residency driver however assumes `Transform.translation` is
//! **window-local** (per `02b-design-plan-b.md` § H / Q1: "the renderer never
//! sees world IVec3, only window-local"). Without a host-side translation the
//! two coordinate systems drift apart the moment `Residency::origin` advances:
//! `residency_driver::camera_segment_pos` adds `origin * SEGMENT_VOXELS` on top
//! of an already-absolute `pos_int`, the inferred `cam_seg` chases the origin,
//! the origin shifts again next frame, and the camera enters an endless
//! reposition loop visually.
//!
//! The fix mirrors the e2e gate's `pin_streaming_window_camera`: maintain a
//! [`CameraAbsolutePosition`] resource as the single source of truth for the
//! camera's absolute world position, then re-derive the window-local
//! `Transform.translation` from it each tick. Stateless re-derivation under
//! the current `Residency::origin` — no floating-point drift accumulates.
//!
//! ## Ownership / mutation lifecycle
//!
//! - [`install_streaming_camera_position`] seeds [`CameraAbsolutePosition`]
//!   from the spawn `Transform` (called by
//!   `install_procedural_streaming_world` at the moment the camera entity is
//!   committed). Origin is `IVec3::ZERO` at install time, so the spawn
//!   Transform's translation == absolute world voxel coord.
//! - [`track_and_pin_camera`] runs **after** `FreeCameraPlugin`'s controller
//!   and **before** `sync_position_split`. It reads the post-controller
//!   `Transform.translation`, treats the delta against the previous
//!   re-pinned window-local position as an absolute-world delta (since the
//!   controller is window-/world-blind, the delta IS the actual input
//!   movement), folds it into [`CameraAbsolutePosition`], then re-pins
//!   `Transform.translation` to the window-local frame at the current
//!   `Residency::origin`.
//! - `residency_driver` reads [`CameraAbsolutePosition`] when present (the
//!   streaming preset) to compute the camera segment — bypassing the
//!   `pos_int + origin * SEGMENT_VOXELS` formula's window-local assumption.
//!
//! ## Default / Vox / Static preset behaviour
//!
//! None of these install [`CameraAbsolutePosition`]:
//!
//! - `Default` / `Vox` — no streaming, no residency, no need.
//! - `ProceduralStatic` — runs the renderer in absolute-world coords directly
//!   (no `Residency` resource, no window-local translation step). The
//!   `pin_streaming_window_camera` translation helper returns identity when
//!   no residency exists; the same identity carries through here.
//!
//! [`track_and_pin_camera`] takes `Option<Res<Residency>>` +
//! `Option<ResMut<CameraAbsolutePosition>>` and early-returns when either is
//! absent. The `FreeCamera` controller continues to write `Transform`
//! directly, which is the correct behaviour for the non-streaming presets.

use bevy::prelude::*;

use crate::streaming::SEGMENT_VOXELS;

/// Per-frame absolute world-voxel position of the streaming camera.
///
/// Split int + frac to avoid f32 precision loss at world scales of
/// `(4096, 512, 4096)` voxels (per `PositionSplit`'s D1 motivation). The two
/// parts compose as `pos_int.as_vec3() + pos_frac` to recover the absolute
/// world voxel coord; the residency driver consumes `pos_int` directly to
/// pick the camera's world segment.
///
/// Resource (not Component) — bevy-naadf is single-camera today
/// (`camera::setup_camera` spawns exactly one `Camera3d`). Promoting to a
/// per-entity Component is a localised change later, if multi-camera ever
/// happens.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct CameraAbsolutePosition {
    /// Integer voxel component of the absolute world position.
    pub pos_int: IVec3,
    /// Fractional offset in `[0, 1)^3` (normalised — same convention as
    /// `PositionSplit`).
    pub pos_frac: Vec3,
}

impl CameraAbsolutePosition {
    /// Split an absolute world `Vec3` into int + frac.
    pub fn from_world(world: Vec3) -> Self {
        let floor = world.floor();
        Self {
            pos_int: floor.as_ivec3(),
            pos_frac: world - floor,
        }
    }

    /// Recombine into an absolute world `Vec3`.
    pub fn to_world(self) -> Vec3 {
        self.pos_int.as_vec3() + self.pos_frac
    }

    /// Window-local `Vec3` for the supplied residency origin.
    pub fn window_local(self, residency_origin: IVec3) -> Vec3 {
        let origin_voxels = (residency_origin * SEGMENT_VOXELS).as_vec3();
        self.to_world() - origin_voxels
    }
}

/// Bookkeeping for the previous frame's re-pinned window-local translation.
/// Stored as a `Local` on [`track_and_pin_camera`] — never read by other
/// systems. Public only because Bevy requires `Local<T>` parameter types to
/// be at least as visible as the system using them.
#[derive(Default, Clone, Copy, Debug)]
pub struct PrevWindowLocal(Option<Vec3>);

/// `Update` system — runs **after** `FreeCameraPlugin`'s controller (which
/// lives in `RunFixedMainLoop::BeforeFixedMainLoop`, scheduled before `Update`
/// by the main schedule) and **before** `sync_position_split` so the
/// `PositionSplit` derived from `Transform.translation` is window-local at the
/// current `Residency::origin`.
///
/// Algorithm:
/// 1. Read the post-controller `Transform.translation`. If
///    `Residency::origin` shifted in this frame's `PreUpdate`, the controller
///    nonetheless saw the previous frame's re-pinned Transform — so the delta
///    `(transform.translation - prev_window_local)` is the actual input
///    movement (a window-local delta, but window-local deltas equal absolute-
///    world deltas because window-local is just absolute minus a constant).
/// 2. Fold the delta into [`CameraAbsolutePosition`].
/// 3. Re-pin `Transform.translation` to the window-local frame for the
///    **current** `Residency::origin`.
/// 4. Stash the new window-local translation as `prev_window_local` for the
///    next tick.
///
/// First-tick init: when `prev_window_local` is `None`, we treat the current
/// `Transform.translation` as the seed — the install path already wrote it as
/// an absolute world coord (origin == `IVec3::ZERO` at install), so
/// [`CameraAbsolutePosition`] is seeded from it. No delta-fold on the seed
/// tick.
pub fn track_and_pin_camera(
    residency: Option<Res<crate::streaming::Residency>>,
    abs_pos: Option<ResMut<CameraAbsolutePosition>>,
    mut prev_window_local: Local<PrevWindowLocal>,
    mut q: Query<&mut Transform, With<bevy::prelude::Camera3d>>,
) {
    let (Some(residency), Some(mut abs_pos)) = (residency, abs_pos) else {
        // Not the streaming preset — `FreeCamera`'s additive write to
        // `Transform.translation` IS the correct behaviour. Don't touch.
        return;
    };
    let Ok(mut transform) = q.single_mut() else {
        return;
    };

    let origin = residency.origin();
    let current_local = transform.translation;

    if let Some(prev) = prev_window_local.0 {
        // Delta the FreeCamera (or e2e harness pin) applied this tick — in
        // window-local coords, but window-local deltas equal absolute-world
        // deltas (the origin-times-segment-voxels term cancels in a
        // subtraction).
        let delta = current_local - prev;
        if delta != Vec3::ZERO {
            let new_world = abs_pos.to_world() + delta;
            *abs_pos = CameraAbsolutePosition::from_world(new_world);
        }
    }
    // On the first tick (`prev_window_local == None`) we DO NOT seed
    // `CameraAbsolutePosition` from the Transform — `CameraAbsolutePosition`
    // is already seeded by `install_streaming_camera_position` from the
    // streaming preset's spawn pose, which is authoritative. The e2e
    // harness's `setup_e2e_camera` spawns the Camera3d entity at the
    // `e2e_motion_start_transform` pose (not the streaming centre), so
    // seeding from that Transform would silently clobber the install-time
    // absolute pose. Instead we drop straight into the re-pin step below —
    // the Transform gets overwritten with the correct window-local pose
    // derived from the install-time `CameraAbsolutePosition`.

    // Re-pin Transform.translation to the window-local frame for the current
    // origin (which may have shifted in this frame's PreUpdate).
    let new_local = abs_pos.window_local(origin);
    transform.translation = new_local;
    prev_window_local.0 = Some(new_local);
}

/// Install the absolute-position resource at the same point the camera spawn
/// pose is decided. Called by `install_procedural_streaming_world` after
/// `InitialCameraPose` is written.
///
/// `spawn_world_pos` is the absolute-world camera pose translation (since
/// residency origin is `IVec3::ZERO` at install time, this equals the
/// window-local pose too — but we name the parameter "world" to keep the
/// invariant explicit).
pub fn install_streaming_camera_position(
    commands: &mut Commands,
    spawn_world_pos: Vec3,
) {
    commands.insert_resource(CameraAbsolutePosition::from_world(spawn_world_pos));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_world_splits_int_and_frac() {
        let p = CameraAbsolutePosition::from_world(Vec3::new(2048.25, 288.5, 2048.75));
        assert_eq!(p.pos_int, IVec3::new(2048, 288, 2048));
        assert!((p.pos_frac - Vec3::new(0.25, 0.5, 0.75)).length() < 1e-6);
    }

    #[test]
    fn from_world_then_to_world_round_trips() {
        let original = Vec3::new(-12.4, 3.9, 100.1);
        let p = CameraAbsolutePosition::from_world(original);
        assert!((p.to_world() - original).length() < 1e-4);
    }

    #[test]
    fn window_local_subtracts_origin_voxels() {
        let p = CameraAbsolutePosition::from_world(Vec3::new(2048.0, 288.0, 2048.0));
        let origin = IVec3::new(4, 0, 0); // 4 segments in X
        let local = p.window_local(origin);
        // 4 segments × 256 voxels = 1024 voxels.
        assert!((local - Vec3::new(1024.0, 288.0, 2048.0)).length() < 1e-4);
    }

    #[test]
    fn window_local_at_zero_origin_is_world() {
        let p = CameraAbsolutePosition::from_world(Vec3::new(2048.0, 288.0, 2048.0));
        let local = p.window_local(IVec3::ZERO);
        assert!((local - p.to_world()).length() < 1e-6);
    }
}
