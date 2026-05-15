//! The `WorldData` + `VoxelTypes` main-world resources — the three-layer CPU
//! buffer mirrors, world geometry, and the voxel-type palette
//! (`03-design.md` §4.4).
//!
//! These are the CPU side of the world. `voxel::grid::setup_test_grid` (D2)
//! builds them once at startup; Batch 2's `render::extract` / `render::prepare`
//! mirror them into the render world (`WorldGpu`) on the `dirty` flag.

use bevy::prelude::*;

use crate::voxel::VoxelType;

/// An inclusive integer AABB in voxel coordinates — the world's geometry
/// bounding box (`03-design.md` §4.4 `bounding_box: IAabb3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IAabb3 {
    /// Inclusive minimum corner, in voxels.
    pub min: IVec3,
    /// Inclusive maximum corner, in voxels.
    pub max: IVec3,
}

/// The CPU mirror of the NAADF three-layer voxel world (`03-design.md` §4.4).
///
/// In Phase A this is built once by `voxel::grid::setup_test_grid` and never
/// edited; `dirty` triggers the one-time GPU upload (Batch 2).
#[derive(Resource, Debug)]
pub struct WorldData {
    /// Chunk buffer mirror — one encoded `ChunkCell` `u32` per chunk.
    pub chunks_cpu: Vec<u32>,
    /// Block buffer mirror — encoded `BlockCell` `u32`s, 64 per mixed chunk.
    pub blocks_cpu: Vec<u32>,
    /// Voxel buffer mirror — packed voxel `u32`s, 32 per mixed block.
    pub voxels_cpu: Vec<u32>,
    /// World size in chunks.
    pub size_in_chunks: UVec3,
    /// Geometry bounding box, in voxels.
    pub bounding_box: IAabb3,
    /// Set when the CPU mirror has changed and needs (re-)uploading to the GPU.
    pub dirty: bool,
}

impl Default for WorldData {
    /// An empty, not-yet-built world.
    fn default() -> Self {
        Self {
            chunks_cpu: Vec::new(),
            blocks_cpu: Vec::new(),
            voxels_cpu: Vec::new(),
            size_in_chunks: UVec3::ZERO,
            bounding_box: IAabb3::default(),
            dirty: false,
        }
    }
}

/// The voxel-type palette (`03-design.md` §4.4, ported from
/// `World/VoxelTypeHandler.cs`).
///
/// Element `0` is the reserved empty placeholder (C# convention) — voxel
/// 15-bit type ids index into `types`.
#[derive(Resource, Debug)]
pub struct VoxelTypes {
    /// The palette. `types[0]` is the empty placeholder.
    pub types: Vec<VoxelType>,
    /// Set when the palette has changed and needs (re-)uploading.
    pub dirty: bool,
}

impl Default for VoxelTypes {
    /// A palette holding just the reserved empty placeholder.
    fn default() -> Self {
        Self {
            types: vec![VoxelType::default()],
            dirty: false,
        }
    }
}
