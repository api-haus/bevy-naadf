//! The hard-coded Phase-A test-grid builder (D2).
//!
//! `setup_test_grid` authors a dense voxel volume from simple primitives — a
//! ground slab, several axis-aligned boxes, pillars, two spheres, and **five
//! emissive blocks** distributed through the volume — builds the `VoxelTypes`
//! palette, runs CPU-side AADF construction (`aadf::construct`), and fills the
//! `WorldData` resource (`03-design.md` §6.1 step 1).
//!
//! No `.vox` reader, no `WorldGenerator` port (D2) — this is the smallest
//! content path that gets voxels on screen.
//!
//! **Shared scene (e2e + production).** `setup_test_grid` is a `Startup` system
//! added by [`crate::build_app`] for **both** the production `bevy-naadf`
//! binary and the `e2e_render` harness — only the camera differs (the e2e
//! harness swaps in its own fixed-pose camera). The expanded scene therefore
//! enriches both the live `cargo run` app and the e2e render-test frame.
//!
//! **Scene-expansion (2026-05-14, e2e test-scene expansion task).** The scene
//! was expanded from "ground slab + 2 boxes + 1 sphere + 1 emissive box" to a
//! larger arrangement with **five emissive blocks** spread through the volume,
//! more solid geometry (corner towers, a pillar row, a wall, an arch, two
//! spheres), so the framed scene carries substantial guaranteed-non-black
//! content pre-GI (emissive blocks render white pre-GI) and is a richer GI
//! bounce-light test scene once Batch 5 lands. Still fully deterministic — fixed
//! positions, fixed emissive values, no RNG (the e2e harness depends on this).

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
/// The warm-white emissive type — the original single emissive block, kept.
const TY_EMISSIVE: VoxelTypeId = VoxelTypeId(5);
// Scene-expansion palette additions: more solid geometry colours + four extra
// emissive colours, so the expanded scene has varied geometry for GI bounce and
// several distinct emissive blocks (all render white-ish pre-GI; the colour
// matters for GI bounce tint once Batch 5 lands).
const TY_TOWER: VoxelTypeId = VoxelTypeId(6);
const TY_WALL: VoxelTypeId = VoxelTypeId(7);
const TY_PILLAR: VoxelTypeId = VoxelTypeId(8);
/// Cool-white emissive (slightly blue).
const TY_EMISSIVE_COOL: VoxelTypeId = VoxelTypeId(9);
/// Warm amber emissive.
const TY_EMISSIVE_AMBER: VoxelTypeId = VoxelTypeId(10);
/// Green emissive.
const TY_EMISSIVE_GREEN: VoxelTypeId = VoxelTypeId(11);
/// Magenta/pink emissive.
const TY_EMISSIVE_MAGENTA: VoxelTypeId = VoxelTypeId(12);

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

    // Phase-C followup #1 — preserve the dense voxel-type stream so the
    // runtime GPU construction dispatch can rebuild `segment_voxel_buffer`
    // without going through a CPU `construct()` re-run. Each `VoxelTypeId`
    // is a `u16`; total ~ size_in_voxels.x*y*z * 2 bytes.
    let dense_voxel_types: Vec<u16> = volume.voxels.iter().map(|t| t.0).collect();

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
        pending_edits: Default::default(),
        dense_voxel_types,
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
        // 5 — warm-white emissive box. `color_layered` doubles as emissive
        // intensity (`02-research.md` §4.6); the contribution itself is Phase B.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(1.0, 0.92, 0.78),
            color_layered: Vec3::new(8.0, 7.4, 6.2),
        },
        // 6 — tower: a neutral light-grey diffuse (corner towers).
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.85,
            color_base: Vec3::new(0.62, 0.60, 0.58),
            color_layered: Vec3::ZERO,
        },
        // 7 — wall: a warm sand diffuse (the back wall + arch).
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.85,
            color_base: Vec3::new(0.72, 0.62, 0.42),
            color_layered: Vec3::ZERO,
        },
        // 8 — pillar: a violet diffuse (the pillar row).
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.8,
            color_base: Vec3::new(0.45, 0.32, 0.62),
            color_layered: Vec3::ZERO,
        },
        // 9 — cool-white emissive.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(0.82, 0.88, 1.0),
            color_layered: Vec3::new(6.4, 6.9, 8.0),
        },
        // 10 — warm amber emissive.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(1.0, 0.66, 0.28),
            color_layered: Vec3::new(8.0, 5.3, 2.2),
        },
        // 11 — green emissive.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(0.40, 1.0, 0.46),
            color_layered: Vec3::new(3.2, 8.0, 3.7),
        },
        // 12 — magenta emissive.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(1.0, 0.42, 0.86),
            color_layered: Vec3::new(8.0, 3.4, 6.9),
        },
    ]
}

/// Build the default test volume (`03-design.md` §6.1 step 1).
///
/// **Expanded scene (2026-05-14).** A larger, richer arrangement than the
/// original "ground slab + 2 boxes + 1 sphere + 1 emissive box": a ground slab,
/// four corner towers, a back wall with an arch cut through it, a row of
/// pillars, two warm/cool diffuse boxes, two spheres, and **five emissive
/// blocks** distributed through the volume at varied positions and heights.
///
/// Rationale: the emissive blocks render white-ish pre-GI, so spreading five of
/// them through the framed volume guarantees substantial non-black content even
/// before GI bounce lighting lands (Batch 5), and the extra diffuse geometry
/// (towers, wall, pillars, spheres) gives varied surfaces for GI bounce light to
/// fall on once Batch 5 is in. Fully deterministic — fixed positions, fixed
/// emissive values, no RNG (the e2e harness depends on a bit-identical scene).
///
/// All coordinates are in voxels within the 64×32×64 volume.
fn build_default_volume() -> DenseVolume {
    let mut v = DenseVolume::empty(GRID_SIZE_IN_CHUNKS);
    let size = v.size_in_voxels();
    let (sx, _sy, sz) = (size[0], size[1], size[2]);

    // --- Ground + perimeter -------------------------------------------------

    // Ground slab — the bottom 3 voxel layers, full width/depth.
    fill_box(&mut v, [0, 0, 0], [sx - 1, 2, sz - 1], TY_GROUND);

    // Four corner towers — neutral grey, varied heights, framing the volume.
    fill_box(&mut v, [2, 3, 2], [9, 26, 9], TY_TOWER);
    fill_box(&mut v, [54, 3, 2], [61, 21, 9], TY_TOWER);
    fill_box(&mut v, [2, 3, 54], [9, 18, 61], TY_TOWER);
    fill_box(&mut v, [54, 3, 54], [61, 24, 61], TY_TOWER);

    // Back wall along the far +x edge with an arch cut through it — sand
    // diffuse, a big surface for GI bounce.
    fill_box(&mut v, [56, 3, 14], [60, 22, 49], TY_WALL);
    // Arch opening — carve a doorway back to empty.
    fill_box(&mut v, [55, 3, 26], [61, 14, 37], VoxelTypeId::EMPTY);

    // --- Mid-scene diffuse geometry ----------------------------------------

    // Box A — a tall warm box, sitting on the ground.
    fill_box(&mut v, [12, 3, 14], [23, 20, 25], TY_BOX_A);

    // Box B — a wider cool box on the far side.
    fill_box(&mut v, [38, 3, 40], [52, 16, 55], TY_BOX_B);

    // A row of three violet pillars marching across the mid-volume.
    fill_box(&mut v, [26, 3, 8], [29, 17, 11], TY_PILLAR);
    fill_box(&mut v, [34, 3, 8], [37, 19, 11], TY_PILLAR);
    fill_box(&mut v, [42, 3, 8], [45, 15, 11], TY_PILLAR);

    // Two green diffuse spheres, resting on the ground.
    fill_sphere(&mut v, [30, 11, 30], 8, TY_SPHERE);
    fill_sphere(&mut v, [44, 9, 24], 6, TY_SPHERE);

    // --- Five emissive blocks, distributed through the volume --------------
    //
    // These render white-ish pre-GI — the guaranteed-non-black content — and
    // are the GI bounce-light sources once Batch 5 lands. Spread across the
    // volume at varied positions and heights so several are in frame from any
    // sensible 3/4 vantage.

    // 1 — warm-white, a small bright cube floating near the volume centre
    // (the original single emissive block, kept in roughly its old place).
    fill_box(&mut v, [28, 23, 30], [34, 28, 36], TY_EMISSIVE);

    // 2 — cool-white, low and toward the near corner.
    fill_box(&mut v, [10, 6, 44], [15, 11, 49], TY_EMISSIVE_COOL);

    // 3 — warm amber, high up near the far corner.
    fill_box(&mut v, [46, 24, 46], [51, 29, 51], TY_EMISSIVE_AMBER);

    // 4 — green, mid-height on the +x / -z side.
    fill_box(&mut v, [44, 14, 14], [49, 19, 19], TY_EMISSIVE_GREEN);

    // 5 — magenta, low and toward the near +z edge, in front of box B.
    fill_box(&mut v, [20, 5, 50], [25, 10, 55], TY_EMISSIVE_MAGENTA);

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
        // Air above the scene in an empty region (well above all geometry).
        assert_eq!(v.voxel_at([31, 31, 20]), VoxelTypeId::EMPTY);
    }

    #[test]
    fn default_volume_has_five_emissive_blocks() {
        let v = build_default_volume();
        // One interior voxel from each of the five emissive blocks.
        assert_eq!(v.voxel_at([31, 25, 33]), TY_EMISSIVE, "warm-white block");
        assert_eq!(v.voxel_at([12, 8, 46]), TY_EMISSIVE_COOL, "cool-white block");
        assert_eq!(v.voxel_at([48, 26, 48]), TY_EMISSIVE_AMBER, "amber block");
        assert_eq!(v.voxel_at([46, 16, 16]), TY_EMISSIVE_GREEN, "green block");
        assert_eq!(v.voxel_at([22, 7, 52]), TY_EMISSIVE_MAGENTA, "magenta block");
        // Every one of the five emissive palette entries is Emissive.
        let p = build_palette();
        for ty in [
            TY_EMISSIVE,
            TY_EMISSIVE_COOL,
            TY_EMISSIVE_AMBER,
            TY_EMISSIVE_GREEN,
            TY_EMISSIVE_MAGENTA,
        ] {
            assert_eq!(
                p[ty.0 as usize].material_base,
                MaterialBase::Emissive,
                "palette entry {} must be Emissive",
                ty.0,
            );
        }
    }

    #[test]
    fn default_volume_arch_is_carved() {
        let v = build_default_volume();
        // The back wall is solid sand diffuse...
        assert_eq!(v.voxel_at([58, 18, 18]), TY_WALL, "wall above the arch");
        // ...with the arch doorway carved back to empty.
        assert_eq!(v.voxel_at([58, 8, 31]), VoxelTypeId::EMPTY, "arch opening");
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
