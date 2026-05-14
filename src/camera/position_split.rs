//! `PositionSplit` — NAADF's int+frac camera-relative position type (D1).
//!
//! Faithful port of `Common/Camera.cs`'s `PositionSplit` (`03-design.md` §4.2):
//! an integer voxel position (`IVec3`, the C# `Point3 integer`) plus a
//! fractional offset (`Vec3`, the C# `Vector3 frac`). The C# `updateInternals`
//! folds `floor(frac)` into `integer` so the fractional part is kept in
//! `[0,1)³`; [`PositionSplit::normalise`] is that fold.
//!
//! NAADF threads `camPosInt` + `camPosFrac` separately through every render
//! shader — no `f32` world position is ever reconstructed. Phase A still uses
//! Bevy's free-fly camera for *input* (the reuse audit's "reuse"), but the
//! render-side position is this int+frac split (D1).

use bevy::prelude::*;

use bevy::camera_controller::free_camera::FreeCamera;

/// Camera-relative position as an integer voxel coordinate + a fractional
/// offset kept in `[0,1)³` (D1, `03-design.md` §4.2).
///
/// Attached as a component on the camera entity alongside `Camera3d` +
/// `FreeCamera` + `Transform`.
#[derive(Component, Clone, Copy, Default, Debug, PartialEq)]
pub struct PositionSplit {
    /// Integer voxel position (C# `Point3 integer`).
    pub pos_int: IVec3,
    /// Fractional offset within the voxel, normalised to `[0,1)³`
    /// (C# `Vector3 frac`).
    pub pos_frac: Vec3,
}

impl PositionSplit {
    /// Split a world-space position into int + frac via component-wise `floor`.
    /// The fractional part is `p - floor(p)`, hence already in `[0,1)³`.
    pub fn from_world(p: Vec3) -> Self {
        let floor = p.floor();
        Self {
            pos_int: floor.as_ivec3(),
            pos_frac: p - floor,
        }
    }

    /// Recombine into a single world-space position. Lossy for large
    /// `pos_int` — intended for debug / CPU-side use only; the render path
    /// keeps the two parts separate (D1).
    pub fn to_world(self) -> Vec3 {
        self.pos_int.as_vec3() + self.pos_frac
    }

    /// Fold `floor(pos_frac)` into `pos_int` so `pos_frac` returns to `[0,1)³`
    /// (the C# `updateInternals` step). Idempotent on an already-normalised
    /// value.
    pub fn normalise(&mut self) {
        let floor = self.pos_frac.floor();
        self.pos_int += floor.as_ivec3();
        self.pos_frac -= floor;
    }

    /// `self` after [`normalise`](Self::normalise) — convenience for the
    /// operator impls.
    fn normalised(mut self) -> Self {
        self.normalise();
        self
    }
}

impl core::ops::Add for PositionSplit {
    type Output = PositionSplit;

    /// Component-wise add of both parts, then normalise — mirrors the C#
    /// `operator +`.
    fn add(self, rhs: PositionSplit) -> PositionSplit {
        PositionSplit {
            pos_int: self.pos_int + rhs.pos_int,
            pos_frac: self.pos_frac + rhs.pos_frac,
        }
        .normalised()
    }
}

impl core::ops::Sub for PositionSplit {
    type Output = PositionSplit;

    /// Component-wise subtract of both parts, then normalise — mirrors the C#
    /// `operator -`.
    fn sub(self, rhs: PositionSplit) -> PositionSplit {
        PositionSplit {
            pos_int: self.pos_int - rhs.pos_int,
            pos_frac: self.pos_frac - rhs.pos_frac,
        }
        .normalised()
    }
}

/// `Update` system: derive the camera's [`PositionSplit`] from its
/// `FreeCamera`-driven `Transform` each frame (`03-design.md` §4.2).
///
/// Runs after `FreeCameraPlugin`'s movement system has updated the `Transform`.
/// Phase A uses Bevy's free-fly camera for *input*; this system converts the
/// resulting `Transform.translation` into the int+frac split the render path
/// consumes (D1).
pub fn sync_position_split(
    mut camera: Single<(&Transform, &mut PositionSplit), With<FreeCamera>>,
) {
    let (transform, split) = &mut *camera;
    **split = PositionSplit::from_world(transform.translation);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_world_splits_positive() {
        let s = PositionSplit::from_world(Vec3::new(3.25, 0.0, 17.75));
        assert_eq!(s.pos_int, IVec3::new(3, 0, 17));
        assert!((s.pos_frac - Vec3::new(0.25, 0.0, 0.75)).length() < 1e-6);
    }

    #[test]
    fn from_world_splits_negative() {
        // floor(-2.25) == -3, frac == 0.75 — keeps pos_frac in [0,1).
        let s = PositionSplit::from_world(Vec3::new(-2.25, -0.5, -1.0));
        assert_eq!(s.pos_int, IVec3::new(-3, -1, -1));
        assert!((s.pos_frac - Vec3::new(0.75, 0.5, 0.0)).length() < 1e-6);
    }

    #[test]
    fn round_trips_through_world() {
        let p = Vec3::new(-12.4, 3.9, 100.1);
        let s = PositionSplit::from_world(p);
        assert!((s.to_world() - p).length() < 1e-4);
    }

    #[test]
    fn normalise_folds_overflowed_frac() {
        let mut s = PositionSplit {
            pos_int: IVec3::new(1, 1, 1),
            pos_frac: Vec3::new(2.5, -0.25, 0.5),
        };
        s.normalise();
        assert_eq!(s.pos_int, IVec3::new(3, 0, 1));
        assert!((s.pos_frac - Vec3::new(0.5, 0.75, 0.5)).length() < 1e-6);
        // Idempotent.
        let again = s;
        let mut twice = s;
        twice.normalise();
        assert_eq!(twice, again);
    }

    #[test]
    fn add_and_sub_normalise() {
        let a = PositionSplit::from_world(Vec3::new(1.75, 0.0, 0.0));
        let b = PositionSplit::from_world(Vec3::new(0.75, 0.0, 0.0));
        let sum = a + b;
        // 1.75 + 0.75 == 2.5
        assert_eq!(sum.pos_int.x, 2);
        assert!((sum.pos_frac.x - 0.5).abs() < 1e-6);

        let diff = a - b;
        // 1.75 - 0.75 == 1.0
        assert_eq!(diff.pos_int.x, 1);
        assert!(diff.pos_frac.x.abs() < 1e-6);
    }
}
