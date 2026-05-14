//! `ExtractSchedule`: `WorldData` / camera → render-world mirror
//! (`03-design.md` §4.5, §5).
//!
//! The render world is a separate ECS world rebuilt every frame from the main
//! world. These systems run in `ExtractSchedule` and copy the data the Phase-A
//! render passes need across the world boundary:
//!
//! - [`extract_world`] — on `WorldData.dirty`, mirror the three CPU buffers +
//!   the voxel-type palette into [`ExtractedWorld`]. Build-once: after the
//!   first frame this is a cheap no-op (the resource already holds the data).
//! - [`extract_camera`] — every frame, copy the camera's `PositionSplit` +
//!   inverse view-projection into [`ExtractedCameraData`].
//!
//! `render::prepare` consumes these to build `WorldGpu` / `FrameGpu`.

use bevy::prelude::*;
use bevy::render::Extract;

use crate::camera::PositionSplit;
use crate::voxel::VoxelType;
use crate::world::data::{IAabb3, VoxelTypes, WorldData};

/// Render-world mirror of the CPU voxel world (`03-design.md` §4.5).
///
/// Populated by [`extract_world`] from the main-world [`WorldData`] +
/// [`VoxelTypes`] when they are `dirty`. `render::prepare::prepare_world_gpu`
/// turns this into the GPU `WorldGpu` resource once.
#[derive(Resource, Default)]
pub struct ExtractedWorld {
    /// Chunk buffer mirror — one encoded `ChunkCell` `u32` per chunk.
    pub chunks: Vec<u32>,
    /// Block buffer mirror — encoded `BlockCell` `u32`s, 64 per mixed chunk.
    pub blocks: Vec<u32>,
    /// Voxel buffer mirror — packed voxel `u32`s, 32 per mixed block.
    pub voxels: Vec<u32>,
    /// Voxel-type palette (element 0 = reserved empty placeholder).
    pub voxel_types: Vec<VoxelType>,
    /// World size in chunks.
    pub size_in_chunks: UVec3,
    /// Geometry bounding box, in voxels (inclusive).
    pub bounding_box: IAabb3,
    /// Set when the contents changed this frame and the GPU needs a re-upload.
    /// `prepare_world_gpu` clears it after uploading.
    pub dirty: bool,
}

/// Render-world mirror of the camera's render-relevant state (`03-design.md`
/// §4.5, §5.2). Rebuilt every frame by [`extract_camera`].
#[derive(Resource, Default, Clone, Copy)]
pub struct ExtractedCameraData {
    /// The camera's int+frac camera-relative position (D1).
    pub position_split: PositionSplit,
    /// `world_from_clip` — the inverse view-projection `getRayDir` needs.
    pub inv_view_proj: Mat4,
    /// Render-target size in pixels, taken from the camera viewport.
    pub viewport_size: UVec2,
    /// `true` once a real camera has been seen at least once.
    pub valid: bool,
}

/// `ExtractSchedule` system: mirror the main-world [`WorldData`] + [`VoxelTypes`]
/// into the render-world [`ExtractedWorld`] when they are dirty.
///
/// Build-once (D2): `setup_test_grid` sets `dirty = true`, this copies the
/// buffers once, and after `prepare_world_gpu` clears the flag this stays a
/// no-op. The main-world `dirty` flag is left untouched (the main world does
/// not re-read it); the render-world copy carries its own flag.
pub fn extract_world(
    mut extracted: ResMut<ExtractedWorld>,
    world_data: Extract<Option<Res<WorldData>>>,
    voxel_types: Extract<Option<Res<VoxelTypes>>>,
) {
    let (Some(world_data), Some(voxel_types)) = (&*world_data, &*voxel_types) else {
        return;
    };
    // Build-once: only re-copy when the main-world data is flagged dirty.
    if !world_data.dirty && !voxel_types.dirty {
        return;
    }
    extracted.chunks.clone_from(&world_data.chunks_cpu);
    extracted.blocks.clone_from(&world_data.blocks_cpu);
    extracted.voxels.clone_from(&world_data.voxels_cpu);
    extracted.voxel_types.clone_from(&voxel_types.types);
    extracted.size_in_chunks = world_data.size_in_chunks;
    extracted.bounding_box = world_data.bounding_box;
    extracted.dirty = true;
}

/// `ExtractSchedule` system: copy the camera's `PositionSplit` + inverse
/// view-projection + viewport size into [`ExtractedCameraData`].
///
/// The inverse view-projection is `world_from_clip` —
/// `(clip_from_view * world_from_view⁻¹)⁻¹` — which the WGSL `getRayDir`
/// transforms an NDC ray by (translation drops out after normalisation,
/// `03-design.md` §5.2).
pub fn extract_camera(
    mut extracted: ResMut<ExtractedCameraData>,
    cameras: Extract<Query<(&Camera, &GlobalTransform, &PositionSplit), With<Camera3d>>>,
) {
    let Some((camera, global_transform, position_split)) = cameras.iter().next() else {
        return;
    };
    let clip_from_view = camera.clip_from_view();
    let world_from_view = global_transform.affine();
    let clip_from_world = clip_from_view * Mat4::from(world_from_view).inverse();
    let inv_view_proj = clip_from_world.inverse();

    let viewport_size = camera
        .physical_viewport_size()
        .unwrap_or(UVec2::new(1, 1))
        .max(UVec2::ONE);

    extracted.position_split = *position_split;
    extracted.inv_view_proj = inv_view_proj;
    extracted.viewport_size = viewport_size;
    extracted.valid = true;
}
