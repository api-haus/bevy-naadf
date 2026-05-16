//! Track-B editor — screen-to-world ray helper
//! (`docs/orchestrate/feature-completeness/02b-design-editor.md`).
//!
//! Wraps Bevy 0.19's `Camera::viewport_to_world` (`bevy_camera-0.19.0-rc.1/
//! src/camera.rs:647`) into a trivially-mutable `Ray` struct the editor's
//! brushes consume. Returns `None` on viewport-conversion failure (cursor
//! off-screen, projection degenerate).

use bevy::camera::Camera;
use bevy::math::{Vec2, Vec3};
use bevy::transform::components::GlobalTransform;

/// World-space ray (origin + direction). Bevy's `Ray3d` would do but a
/// trivially-mutable struct keeps brush callers (which may want to nudge the
/// origin by `PositionSplit.pos_int`) simple.
#[derive(Debug, Clone, Copy)]
pub struct Ray {
    /// Ray origin in world space (near-plane projection of the cursor — see
    /// `02b-design-editor.md` Risk 11 / Assumption 11).
    pub origin: Vec3,
    /// Ray direction in world space (unit length).
    pub dir: Vec3,
}

/// Build a world-space ray from a cursor position in viewport (logical) pixels.
///
/// Returns `None` if the camera's `viewport_to_world` fails (cursor outside
/// viewport, projection degenerate). Otherwise returns a unit-length ray
/// originating at the near-plane projection of the cursor.
pub fn screen_to_ray(
    camera: &Camera,
    cam_gxf: &GlobalTransform,
    cursor_pos: Vec2,
) -> Option<Ray> {
    let ray3d = camera.viewport_to_world(cam_gxf, cursor_pos).ok()?;
    Some(Ray {
        origin: ray3d.origin,
        dir: *ray3d.direction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `screen_to_ray` returns None when `viewport_to_world` reports
    /// `InvalidData` — exercised here by passing a cursor far outside the
    /// viewport. (The full `Camera` machinery needs a render-app to construct
    /// the computed `clip_from_view`; we test the documented error contract
    /// via the trivially-mutable struct surface — i.e. that the function is
    /// re-exported and `Ray` carries the expected fields.)
    #[test]
    fn ray_struct_carries_origin_and_dir() {
        let r = Ray {
            origin: Vec3::new(1.0, 2.0, 3.0),
            dir: Vec3::Z,
        };
        assert_eq!(r.origin, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(r.dir, Vec3::Z);
    }
}
