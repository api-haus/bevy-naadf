//! Track-B editor — brush footprint helpers
//! (`docs/orchestrate/feature-completeness/02b-design-editor.md`).
//!
//! Three brushes, faithful ports of the C# tools:
//! - `paint_brush` — Euclidean `< r²` AND only replaces non-empty voxels
//!   (`Paint.cs:69-79`).
//! - `cube_brush` — Chebyshev `< r` (`Cube.cs:76-90`); `is_erase` writes EMPTY.
//! - `sphere_brush` — Euclidean `< r²` (`Sphere.cs:76-89`); `is_erase` writes
//!   EMPTY.
//!
//! All three iterate the AABB the radius defines around `pos`, build a
//! `Vec<(IVec3, VoxelTypeId)>` of voxels to mutate, and call
//! `WorldData::set_voxels_batch` once — one O(chunk-count) call instead of
//! N O(per-voxel) round-trips through `set_voxel` (the sanctioned-divergence
//! perf path; `02b-design-editor.md` Decision 5).
//!
//! Decision 8 — `voxel/grid.rs::fill_*` extraction REJECTED; these brushes
//! implement their own enumeration.

use bevy::math::{IVec3, Vec3};

use crate::voxel::{VoxelTypeId, CELL_DIM};
use crate::world::data::WorldData;

/// Compute the brush's affected-voxel AABB (inclusive), clamped to the world
/// bounds.
fn brush_aabb(world_data: &WorldData, pos: Vec3, radius: f32) -> (IVec3, IVec3) {
    let chunk_size_voxels = (CELL_DIM * CELL_DIM) as i32; // 16
    let sx = (world_data.size_in_chunks.x as i32) * chunk_size_voxels;
    let sy = (world_data.size_in_chunks.y as i32) * chunk_size_voxels;
    let sz = (world_data.size_in_chunks.z as i32) * chunk_size_voxels;
    let lo = IVec3::new(
        ((pos.x - radius).floor() as i32).max(0),
        ((pos.y - radius).floor() as i32).max(0),
        ((pos.z - radius).floor() as i32).max(0),
    );
    let hi = IVec3::new(
        ((pos.x + radius).ceil() as i32).min(sx - 1),
        ((pos.y + radius).ceil() as i32).min(sy - 1),
        ((pos.z + radius).ceil() as i32).min(sz - 1),
    );
    (lo, hi)
}

/// Paint brush — replaces existing non-empty voxels within Euclidean radius
/// with `ty`. Faithful port of `EditingToolPaint.cs:69-79`.
///
/// Note: Paint takes **no** `is_erase` — C# `Paint` has no `isErase` field.
/// The semantic is "replace non-empty voxels with the selected type"; erasure
/// belongs on Cube or Sphere with `is_erase = true`.
pub fn paint_brush(world_data: &mut WorldData, pos: Vec3, radius: f32, ty: VoxelTypeId) {
    if radius <= 0.0 {
        return;
    }
    let r2 = radius * radius;
    let mut edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();
    let (lo, hi) = brush_aabb(world_data, pos, radius);
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let voxel = IVec3::new(x, y, z);
                // Voxel-centre vs pos — matches C# `Paint.cs:68-72` translated.
                let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos;
                if d.length_squared() < r2 {
                    // Paint: replace only existing non-empty voxels.
                    if world_data
                        .get_voxel_type(voxel)
                        .is_some_and(|t| t != VoxelTypeId::EMPTY)
                    {
                        edits.push((voxel, ty));
                    }
                }
            }
        }
    }
    if !edits.is_empty() {
        world_data.set_voxels_batch(&edits);
    }
}

/// Cube brush — Chebyshev distance `< r`. Solid (writes ALL voxels in the
/// Chebyshev cube). `is_erase = true` writes `VoxelTypeId::EMPTY`.
/// Faithful port of `EditingToolCube.cs:76-90`.
pub fn cube_brush(
    world_data: &mut WorldData,
    pos: Vec3,
    radius: f32,
    ty: VoxelTypeId,
    is_erase: bool,
) {
    if radius <= 0.0 {
        return;
    }
    let target = if is_erase { VoxelTypeId::EMPTY } else { ty };
    let mut edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();
    let (lo, hi) = brush_aabb(world_data, pos, radius);
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let voxel = IVec3::new(x, y, z);
                let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos;
                let cheb = d.x.abs().max(d.y.abs()).max(d.z.abs());
                if cheb < radius {
                    edits.push((voxel, target));
                }
            }
        }
    }
    if !edits.is_empty() {
        world_data.set_voxels_batch(&edits);
    }
}

/// Sphere brush — Euclidean `r²` distance check. Solid (writes ALL voxels
/// inside the sphere, unlike Paint). `is_erase = true` writes
/// `VoxelTypeId::EMPTY`. Faithful port of `EditingToolSphere.cs:76-89`.
pub fn sphere_brush(
    world_data: &mut WorldData,
    pos: Vec3,
    radius: f32,
    ty: VoxelTypeId,
    is_erase: bool,
) {
    if radius <= 0.0 {
        return;
    }
    let target = if is_erase { VoxelTypeId::EMPTY } else { ty };
    let r2 = radius * radius;
    let mut edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();
    let (lo, hi) = brush_aabb(world_data, pos, radius);
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let voxel = IVec3::new(x, y, z);
                let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos;
                if d.length_squared() < r2 {
                    edits.push((voxel, target));
                }
            }
        }
    }
    if !edits.is_empty() {
        world_data.set_voxels_batch(&edits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::data::{IAabb3, PendingEdits, WorldData};
    use bevy::math::UVec3;

    fn make_empty_world(size_in_chunks: UVec3) -> WorldData {
        let n_chunks = (size_in_chunks.x * size_in_chunks.y * size_in_chunks.z) as usize;
        let size_v = size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
        WorldData {
            chunks_cpu: vec![0u32; n_chunks],
            blocks_cpu: Vec::new(),
            voxels_cpu: Vec::new(),
            size_in_chunks,
            bounding_box: IAabb3 {
                min: IVec3::ZERO,
                max: IVec3::new(size_v.x as i32 - 1, size_v.y as i32 - 1, size_v.z as i32 - 1),
            },
            dirty: false,
            pending_edits: PendingEdits::default(),
            dense_voxel_types: Vec::new(),
        }
    }

    /// Test #7 — sphere brush produces a solid sphere; all voxels in r²
    /// distance carry the target type, voxels outside stay empty.
    #[test]
    fn sphere_brush_produces_solid_sphere() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        let pos = Vec3::new(32.0, 16.0, 32.0);
        let radius = 4.0;
        let ty = VoxelTypeId(7);
        sphere_brush(&mut wd, pos, radius, ty, false);

        // Sample inside the sphere — voxel at (32,16,32) centre + 0.5: d≈0.5,
        // length_squared ~0.75 < 16 → should be type 7.
        assert_eq!(wd.get_voxel_type(IVec3::new(32, 16, 32)), Some(ty));
        assert_eq!(wd.get_voxel_type(IVec3::new(33, 16, 32)), Some(ty));
        assert_eq!(wd.get_voxel_type(IVec3::new(30, 14, 30)), Some(ty));
        // Voxel well outside the sphere — empty.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(50, 16, 50)),
            Some(VoxelTypeId::EMPTY)
        );
    }

    /// Test #8 — cube brush produces a Chebyshev cube; corners of the cube
    /// (cheb = r-1) inside, voxels with cheb >= r outside.
    #[test]
    fn cube_brush_produces_solid_cube() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        let pos = Vec3::new(32.5, 16.5, 32.5);
        let radius = 3.0;
        let ty = VoxelTypeId(11);
        cube_brush(&mut wd, pos, radius, ty, false);
        // Voxel at corner of the cube — (30,14,30): cheb = max(|30.5-32.5|,
        // |14.5-16.5|, |30.5-32.5|) = 2 < 3 → inside.
        assert_eq!(wd.get_voxel_type(IVec3::new(30, 14, 30)), Some(ty));
        // Centre — definitely inside.
        assert_eq!(wd.get_voxel_type(IVec3::new(32, 16, 32)), Some(ty));
        // Just outside — (29,14,30): cheb = 3 NOT < 3 → outside.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(29, 14, 30)),
            Some(VoxelTypeId::EMPTY)
        );
    }

    /// Test #9 — paint brush only replaces existing non-empty voxels. Set up
    /// a "ground" of type 1 voxels then paint type 7 over a sphere; voxels
    /// inside the sphere AND non-empty become 7, voxels inside the sphere
    /// but empty stay empty.
    #[test]
    fn paint_brush_only_replaces_non_empty() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        // Seed a 5x1x5 ground patch of type 1 at y=2.
        let mut seed = Vec::new();
        for z in 4..=8 {
            for x in 4..=8 {
                seed.push((IVec3::new(x, 2, z), VoxelTypeId(1)));
            }
        }
        wd.set_voxels_batch(&seed);
        // Sanity — ground is type 1.
        assert_eq!(wd.get_voxel_type(IVec3::new(6, 2, 6)), Some(VoxelTypeId(1)));
        // Sanity — air above is empty.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(6, 4, 6)),
            Some(VoxelTypeId::EMPTY)
        );
        // Paint a radius-4 sphere centred 1 voxel above the ground.
        paint_brush(&mut wd, Vec3::new(6.5, 3.5, 6.5), 4.0, VoxelTypeId(7));
        // Ground voxel inside the sphere now type 7.
        assert_eq!(wd.get_voxel_type(IVec3::new(6, 2, 6)), Some(VoxelTypeId(7)));
        // Air voxel inside the sphere stays empty (Paint does NOT fill empty).
        assert_eq!(
            wd.get_voxel_type(IVec3::new(6, 4, 6)),
            Some(VoxelTypeId::EMPTY)
        );
    }

    /// Test #10 — erase mode on sphere brush clears existing full voxels.
    #[test]
    fn erase_with_sphere_clears_voxels() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        // Seed a full 5x5x5 cube of type 1 around (10, 10, 10).
        let mut seed = Vec::new();
        for z in 8..=12 {
            for y in 8..=12 {
                for x in 8..=12 {
                    seed.push((IVec3::new(x, y, z), VoxelTypeId(1)));
                }
            }
        }
        wd.set_voxels_batch(&seed);
        // Sanity — cube voxel is type 1.
        assert_eq!(wd.get_voxel_type(IVec3::new(10, 10, 10)), Some(VoxelTypeId(1)));
        // Erase with sphere centred on the cube.
        sphere_brush(&mut wd, Vec3::new(10.5, 10.5, 10.5), 3.0, VoxelTypeId(0), true);
        // Centre now empty.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(10, 10, 10)),
            Some(VoxelTypeId::EMPTY)
        );
    }
}
