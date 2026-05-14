//! The hard-coded Phase-A test-grid builder (D2).
//!
//! `setup_test_grid` authors a dense voxel volume from simple primitives — a
//! ground slab, a few axis-aligned boxes, a sphere, and one emissive box —
//! builds the `VoxelTypes` palette, runs CPU-side AADF construction
//! (`aadf::construct`), and fills the `WorldData` resource (`03-design.md`
//! §6.1 step 1).
//!
//! No `.vox` reader, no `WorldGenerator` port (D2) — this is the smallest
//! content path that gets voxels on screen.

use bevy::prelude::*;

use crate::aadf::construct::{construct, DenseVolume};
use crate::voxel::{MaterialBase, MaterialLayer, VoxelType, VoxelTypeId};
use crate::world::data::{IAabb3, VoxelTypes, WorldData};
use crate::{AppArgs, GridPreset};

// Palette indices into `VoxelTypes::types`. Index 0 is the reserved empty
// placeholder (C# convention) — see `VoxelTypes::default`.
const TY_GROUND: VoxelTypeId = VoxelTypeId(1);
const TY_BOX_A: VoxelTypeId = VoxelTypeId(2);
const TY_BOX_B: VoxelTypeId = VoxelTypeId(3);
const TY_SPHERE: VoxelTypeId = VoxelTypeId(4);
const TY_EMISSIVE: VoxelTypeId = VoxelTypeId(5);

/// World size for the Phase-A test grid: 4×2×4 chunks = 64×32×64 voxels
/// (`03-design.md` §6.1 step 1).
const GRID_SIZE_IN_CHUNKS: [u32; 3] = [4, 2, 4];

/// Startup system: build the hard-coded Phase-A voxel test grid (D2).
///
/// Replaces `main::setup_scene_placeholder`. Inserts the `WorldData` and
/// `VoxelTypes` resources.
pub fn setup_test_grid(mut commands: Commands, args: Res<AppArgs>) {
    let palette = build_palette();
    let volume = match args.grid_preset {
        GridPreset::Default => build_default_volume(),
    };

    let world = construct(&volume);
    let size = volume.size_in_voxels();

    info!(
        "NAADF test grid ({:?}): {} chunks, {} blocks, {} voxel-u32s ({}x{}x{} voxels)",
        args.grid_preset,
        world.chunks.len(),
        world.blocks.len(),
        world.voxels.len(),
        size[0],
        size[1],
        size[2],
    );

    commands.insert_resource(WorldData {
        chunks_cpu: world.chunks,
        blocks_cpu: world.blocks,
        voxels_cpu: world.voxels,
        size_in_chunks: UVec3::from_array(world.size_in_chunks),
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(size[0] as i32 - 1, size[1] as i32 - 1, size[2] as i32 - 1),
        },
        dirty: true,
    });

    commands.insert_resource(VoxelTypes {
        types: palette,
        dirty: true,
    });
}

/// Build the Phase-A voxel-type palette. Index 0 is the empty placeholder; the
/// rest match the `TY_*` constants.
fn build_palette() -> Vec<VoxelType> {
    vec![
        // 0 — reserved empty placeholder.
        VoxelType::default(),
        // 1 — ground: a flat grey diffuse slab.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.9,
            color_base: Vec3::new(0.55, 0.55, 0.58),
            color_layered: Vec3::ZERO,
        },
        // 2 — box A: warm diffuse.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.8,
            color_base: Vec3::new(0.80, 0.30, 0.22),
            color_layered: Vec3::ZERO,
        },
        // 3 — box B: cool diffuse.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.8,
            color_base: Vec3::new(0.25, 0.45, 0.80),
            color_layered: Vec3::ZERO,
        },
        // 4 — sphere: green diffuse.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.7,
            color_base: Vec3::new(0.30, 0.70, 0.32),
            color_layered: Vec3::ZERO,
        },
        // 5 — emissive box. `color_layered` doubles as emissive intensity
        // (`02-research.md` §4.6); the contribution itself is Phase B.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(1.0, 0.92, 0.78),
            color_layered: Vec3::new(8.0, 7.4, 6.2),
        },
    ]
}

/// Build the default test volume: a ground slab, two axis-aligned boxes, a
/// sphere, and one emissive box (`03-design.md` §6.1 step 1).
fn build_default_volume() -> DenseVolume {
    let mut v = DenseVolume::empty(GRID_SIZE_IN_CHUNKS);
    let size = v.size_in_voxels();
    let (sx, _sy, sz) = (size[0], size[1], size[2]);

    // Ground slab — the bottom 3 voxel layers, full width/depth.
    fill_box(&mut v, [0, 0, 0], [sx - 1, 2, sz - 1], TY_GROUND);

    // Box A — a tall warm box near one corner, sitting on the ground.
    fill_box(&mut v, [8, 3, 8], [19, 18, 19], TY_BOX_A);

    // Box B — a wider cool box on the far side.
    fill_box(&mut v, [40, 3, 36], [55, 14, 51], TY_BOX_B);

    // Sphere — green, resting on the ground roughly centre-ish.
    fill_sphere(&mut v, [34, 11, 18], 8, TY_SPHERE);

    // Emissive box — a small bright cube floating above the scene.
    fill_box(&mut v, [28, 24, 28], [33, 29, 33], TY_EMISSIVE);

    v
}

/// Fill the inclusive voxel box `[min, max]` with `ty`, clamped to the volume.
fn fill_box(v: &mut DenseVolume, min: [u32; 3], max: [u32; 3], ty: VoxelTypeId) {
    let size = v.size_in_voxels();
    let lo = [min[0].min(size[0] - 1), min[1].min(size[1] - 1), min[2].min(size[2] - 1)];
    let hi = [max[0].min(size[0] - 1), max[1].min(size[1] - 1), max[2].min(size[2] - 1)];
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                v.set([x, y, z], ty);
            }
        }
    }
}

/// Fill a solid sphere of integer `radius` centred at `center` with `ty`,
/// clamped to the volume.
fn fill_sphere(v: &mut DenseVolume, center: [u32; 3], radius: u32, ty: VoxelTypeId) {
    let size = v.size_in_voxels();
    let r2 = (radius * radius) as i64;
    let c = [center[0] as i64, center[1] as i64, center[2] as i64];
    let lo = [
        center[0].saturating_sub(radius),
        center[1].saturating_sub(radius),
        center[2].saturating_sub(radius),
    ];
    let hi = [
        (center[0] + radius).min(size[0] - 1),
        (center[1] + radius).min(size[1] - 1),
        (center[2] + radius).min(size[2] - 1),
    ];
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                let d = [x as i64 - c[0], y as i64 - c[1], z as i64 - c[2]];
                if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= r2 {
                    v.set([x, y, z], ty);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aadf::cell::ChunkCell;

    #[test]
    fn default_volume_has_expected_dimensions() {
        let v = build_default_volume();
        assert_eq!(v.size_in_chunks, GRID_SIZE_IN_CHUNKS);
        assert_eq!(v.size_in_voxels(), [64, 32, 64]);
    }

    #[test]
    fn default_volume_has_ground_and_air() {
        let v = build_default_volume();
        // Ground present at the bottom.
        assert_eq!(v.voxel_at([0, 0, 0]), TY_GROUND);
        assert_eq!(v.voxel_at([63, 2, 63]), TY_GROUND);
        // Air above the ground in an empty region.
        assert_eq!(v.voxel_at([2, 25, 2]), VoxelTypeId::EMPTY);
    }

    #[test]
    fn default_volume_constructs() {
        let v = build_default_volume();
        let w = construct(&v);
        // 4*2*4 = 32 chunks.
        assert_eq!(w.chunks.len(), 32);
        // The grid has geometry → some mixed chunks → a non-empty block buffer.
        assert!(!w.blocks.is_empty(), "expected mixed chunks → blocks");
        assert!(!w.voxels.is_empty(), "expected mixed blocks → voxels");
        // Every chunk word decodes to a valid cell (no panic).
        for &raw in &w.chunks {
            let _ = ChunkCell::decode(raw);
        }
    }

    #[test]
    fn palette_reserves_element_zero() {
        let p = build_palette();
        assert_eq!(p[0], VoxelType::default(), "element 0 must be the placeholder");
        assert_eq!(p[TY_EMISSIVE.0 as usize].material_base, MaterialBase::Emissive);
    }
}
