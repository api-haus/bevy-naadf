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
use bevy::render::{Extract, MainWorld};

use crate::camera::PositionSplit;
use crate::render::taa::{rotation_only_view_proj, CameraHistory, CAMERA_HISTORY_DEPTH};
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
    /// Phase-C followup #1 — dense pre-construction voxel-type stream
    /// (`size_in_voxels.x*y*z` u16s). Used by the runtime GPU producer dispatch
    /// to build `segment_voxel_buffer` without re-running CPU construction.
    /// Empty if the source `WorldData` does not carry it.
    pub dense_voxel_types: Vec<u16>,
}

/// Render-world mirror of the camera's render-relevant state (`03-design.md`
/// §4.5, §5.2). Rebuilt every frame by [`extract_camera`].
#[derive(Resource, Default, Clone, Copy)]
pub struct ExtractedCameraData {
    /// The camera's int+frac camera-relative position (D1).
    pub position_split: PositionSplit,
    /// Rotation-only `view_from_clip` — the inverse view-projection `getRayDir`
    /// needs. Built from a *translation-free* view matrix (mirrors NAADF's
    /// origin-based `invViewProjTransform`, `Camera.cs:199`): the camera world
    /// translation is stripped before inverting, so `getRayDir` can treat the
    /// unprojected vector as a pure direction. The ray *origin* is supplied
    /// separately via [`PositionSplit`].
    pub inv_view_proj: Mat4,
    /// Rotation-only (translation-free) `clip_from_view` — the *non-inverted*
    /// matrix `inv_view_proj` is the inverse of (mirrors NAADF's
    /// `camera.viewProjTransform`, `Camera.cs:201`). The Phase-A-2 TAA reproject
    /// pass needs this (C# `camMatrix`) to project a reprojected virtual pos
    /// into the current screen; stored directly to avoid a redundant inverse
    /// (`06-design-a2.md` §9.2).
    pub view_proj: Mat4,
    /// Render-target size in pixels, taken from the camera viewport.
    pub viewport_size: UVec2,
    /// `true` once a real camera has been seen at least once.
    pub valid: bool,
}

/// `ExtractSchedule` system: mirror the main-world [`WorldData`] + [`VoxelTypes`]
/// into the render-world [`ExtractedWorld`] when they are dirty, then clear the
/// main-world `dirty` flags so subsequent frames stay a true no-op.
///
/// Build-once (D2): `setup_test_grid` sets `dirty = true`, this copies the
/// buffers once + clears the main-world flag in the same system, and the
/// render-world `extracted.dirty` is consumed by `prepare_world_gpu` (which
/// clears it after the GPU upload).
///
/// **Fix (`02e-perframe-cpu-investigation.md`, 2026-05-16):** the main-world
/// flag was previously left set indefinitely, causing this system to fire
/// every frame and re-clone the entire ~48 MiB CPU mirror on Oasis-class
/// worlds (~2.8 ms/frame), in turn re-triggering the ~16.7 ms/frame full-world
/// GPU re-upload in `prepare_world_gpu`. Mutating the main world via
/// `ResMut<MainWorld>` restores the originally-intended build-once shape.
/// Per-edit re-uploads still flow via the W2 delta-upload chain
/// (`pending_edits.batches` → `naadf_world_change_node`); `world_data.dirty = true`
/// writes in edit paths were also removed since the delta chain handles
/// per-edit changes without needing a full-world re-extract.
pub fn extract_world(
    mut extracted: ResMut<ExtractedWorld>,
    mut main_world: ResMut<MainWorld>,
) {
    // SAFETY note: `ResMut<MainWorld>` is the sanctioned bevy_render pattern
    // for mutating the main world from an extract system (see e.g.
    // `bevy_render::erased_render_asset::extract_render_asset`). The
    // alternative `Extract<Option<Res<_>>>` is read-only by design
    // (`ReadOnlySystemParam` bound on `Extract`), so we cannot clear the flag
    // through that path.
    let world = main_world.as_mut();
    let world_data = world.get_resource::<WorldData>();
    let voxel_types = world.get_resource::<VoxelTypes>();
    let (Some(world_data), Some(voxel_types)) = (world_data, voxel_types) else {
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
    extracted.dense_voxel_types.clone_from(&world_data.dense_voxel_types);
    extracted.dirty = true;
    // Clear the main-world flags AFTER the copy completes so subsequent
    // frames stay a true no-op.
    if let Some(mut wd) = world.get_resource_mut::<WorldData>() {
        wd.dirty = false;
    }
    if let Some(mut vt) = world.get_resource_mut::<VoxelTypes>() {
        vt.dirty = false;
    }
}

/// `ExtractSchedule` system: copy the camera's `PositionSplit` + inverse
/// view-projection + viewport size into [`ExtractedCameraData`].
///
/// The inverse view-projection is a rotation-only `view_from_clip` —
/// `(clip_from_view * world_from_view_rot⁻¹)⁻¹` — built from a *translation-free*
/// view matrix so the WGSL `getRayDir` can treat the unprojected NDC point as a
/// pure direction (`03-design.md` §5.2). This mirrors NAADF's origin-based
/// `invViewProjTransform` (`Camera.cs:199` — `CreateLookAt(Vector3::ZERO, …)`):
/// the camera world translation is *not* baked in; the ray origin is supplied
/// separately via `PositionSplit`.
// The `Extract<Query<…>>` filter tuple trips clippy's type-complexity lint —
// unavoidable noise for a Bevy extract system.
#[allow(clippy::type_complexity)]
pub fn extract_camera(
    mut extracted: ResMut<ExtractedCameraData>,
    cameras: Extract<Query<(&Camera, &GlobalTransform, &PositionSplit), With<Camera3d>>>,
) {
    let Some((camera, global_transform, position_split)) = cameras.iter().next() else {
        return;
    };
    // NAADF builds invCamMatrix from a view matrix at the ORIGIN
    // (Camera.cs:199 — CreateLookAt(Vector3::ZERO, camDir, Up)): rotation only,
    // no camera translation. getRayDir then treats the unprojected vector as a
    // pure direction. `rotation_only_view_proj` is the shared helper that
    // builds that translation-free view-proj — the single place the formula
    // lives, also called by `update_camera_history` (`06-design-a2.md` §9.3).
    let clip_from_view_rot = rotation_only_view_proj(camera, global_transform.rotation());
    let inv_view_proj = clip_from_view_rot.inverse();

    // Viewport size — **retain the last-known-good value on a degenerate read**
    // (`18-taa-fidelity.md` fix #4 / black-on-resize root cause). During a
    // window resize `Camera::physical_viewport_size()` transiently returns
    // `None` (Bevy's `camera_system` recomputes the viewport rect *after* the
    // window's new size is known, and `ExtractSchedule` can run on a frame
    // before that) — or a degenerate `(0, *)` / `(*, 0)`. The OLD code did
    // `.unwrap_or(UVec2::new(1,1))`, collapsing every screen-space buffer to
    // 1×1 while the final blit covered the full new-size view target → OOB
    // storage reads → a fully-black frame. Instead: keep whatever
    // `extracted.viewport_size` already holds (the previous frame's valid
    // size) until a real new size arrives — never shrink to a bogus size.
    // On the very first frame `extracted.viewport_size` is the `Default`
    // `UVec2::ZERO`; the `.max(UVec2::ONE)` floor in the consumers
    // (`prepare_taa` / `prepare_gi` / `prepare_frame_gpu`) covers that single
    // pre-first-valid-frame case — but that frame is also pre-`valid`, so the
    // prepare systems skip it anyway.
    if let Some(size) = camera.physical_viewport_size() {
        if size.x > 0 && size.y > 0 {
            extracted.viewport_size = size;
        }
        // else: degenerate read — keep the last-known-good `viewport_size`.
    }
    // else: `None` (mid-resize) — keep the last-known-good `viewport_size`.

    extracted.position_split = *position_split;
    extracted.inv_view_proj = inv_view_proj;
    extracted.view_proj = clip_from_view_rot;
    extracted.valid = true;
}

/// Render-world mirror of the 128-deep camera-history ring + the frame counter
/// (`06-design-a2.md` §9.1, §9.3). Rebuilt every frame by
/// [`extract_camera_history`] from the main-world [`CameraHistory`].
///
/// `render::taa::prepare_taa` consumes this to build the `GpuCameraHistorySlot`
/// array + `GpuTaaParams`, and `prepare_frame_gpu` reads `frame_count` /
/// `taa_index` / `current_jitter` for `GpuRenderParams`.
#[derive(Resource)]
pub struct ExtractedCameraHistory {
    /// Per-frame camera `PositionSplit` (C# `oldCamPositions[128]`).
    pub positions: [PositionSplit; CAMERA_HISTORY_DEPTH],
    /// Per-frame translation-free view-proj matrix (C# `taaSampleCamTransform[128]`).
    pub view_proj: [Mat4; CAMERA_HISTORY_DEPTH],
    /// Per-frame *inverse* translation-free view-proj matrix
    /// (C# `taaSampleCamTransformInvers[128]`) — Phase B's `renderSampleRefine`
    /// `camRotOld` ring (`09-design-b.md` §3.6 / §10.2).
    pub view_proj_inv: [Mat4; CAMERA_HISTORY_DEPTH],
    /// Per-frame Halton jitter (C# `taaSampleJitter[128]`).
    pub jitter: [Vec2; CAMERA_HISTORY_DEPTH],
    /// Monotonic frame counter (C# `frameCount`).
    pub frame_count: u32,
    /// `taaIndex` for the slot written this frame — computed once per frame in
    /// `update_camera_history` (`06-design-a2.md` §9.3).
    pub taa_index: u32,
    /// This frame's Halton jitter (= `jitter[taa_index]`).
    pub current_jitter: Vec2,
    /// `true` once the history has been extracted at least once.
    pub valid: bool,
}

impl Default for ExtractedCameraHistory {
    fn default() -> Self {
        Self {
            positions: [PositionSplit::default(); CAMERA_HISTORY_DEPTH],
            view_proj: [Mat4::IDENTITY; CAMERA_HISTORY_DEPTH],
            view_proj_inv: [Mat4::IDENTITY; CAMERA_HISTORY_DEPTH],
            jitter: [Vec2::ZERO; CAMERA_HISTORY_DEPTH],
            frame_count: 0,
            taa_index: (CAMERA_HISTORY_DEPTH as u32) - 1,
            current_jitter: Vec2::ZERO,
            valid: false,
        }
    }
}

/// `ExtractSchedule` system: mirror the main-world [`CameraHistory`] into the
/// render-world [`ExtractedCameraHistory`] (`06-design-a2.md` §9.1).
///
/// Runs every frame — the camera-history ring changes every frame. The rings
/// are fixed-size 128-element arrays, so this is a cheap fixed-cost copy.
pub fn extract_camera_history(
    mut extracted: ResMut<ExtractedCameraHistory>,
    history: Extract<Option<Res<CameraHistory>>>,
) {
    let Some(history) = &*history else {
        return;
    };
    extracted.positions = history.positions;
    extracted.view_proj = history.view_proj;
    extracted.view_proj_inv = history.view_proj_inv;
    extracted.jitter = history.jitter;
    extracted.frame_count = history.frame_count;
    extracted.taa_index = history.taa_index;
    extracted.current_jitter = history.current_jitter;
    extracted.valid = true;
}

/// Render-world mirror of the `AppArgs.taa` runtime toggle
/// (`06-design-a2.md` §6.1, §8.2). `AppArgs` is a main-world resource; the
/// render-world prepare / graph systems need the flag to (a) set `FLAG_IS_TAA`
/// in `GpuRenderParams` so the first-hit pass writes the `taa_samples` ring,
/// and (b) gate the TAA reproject node's dispatch — when TAA is off the node
/// early-returns, leaving `taa_sample_accum` bit-identical to Phase A.
#[derive(Resource, Default, Clone, Copy)]
pub struct ExtractedTaaConfig {
    /// Whether long-term TAA is enabled (mirrors `AppArgs.taa`).
    pub enabled: bool,
}

/// `ExtractSchedule` system: mirror `AppArgs.taa` into the render-world
/// [`ExtractedTaaConfig`] (`06-design-a2.md` §8.2).
pub fn extract_taa_config(
    mut extracted: ResMut<ExtractedTaaConfig>,
    args: Extract<Option<Res<crate::AppArgs>>>,
) {
    if let Some(args) = &*args {
        extracted.enabled = args.taa;
    }
}

/// Render-world mirror of `AppArgs.gi` — the Phase-B GI pipeline settings
/// (`09-design-b.md` §3.8 / §10.2). `AppArgs` is a main-world resource; the
/// render-world `prepare_gi` system needs these to build `GpuGiParams`, and
/// `naadf_denoise_node` (Batch 5) gates on `is_denoise`. Like A-2's
/// `ExtractedTaaConfig` — a flat `Copy` mirror, re-copied each frame.
#[derive(Resource, Clone, Copy)]
#[derive(Default)]
pub struct ExtractedGiConfig {
    /// The mirrored GI settings.
    pub settings: crate::GiSettings,
}


/// `ExtractSchedule` system: mirror `AppArgs.gi` into the render-world
/// [`ExtractedGiConfig`] (`09-design-b.md` §10.2).
pub fn extract_gi_config(
    mut extracted: ResMut<ExtractedGiConfig>,
    args: Extract<Option<Res<crate::AppArgs>>>,
) {
    if let Some(args) = &*args {
        extracted.settings = args.gi;
    }
}
