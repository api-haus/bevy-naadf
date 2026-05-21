//! `ExtractSchedule`: build-once world hand-off + per-frame camera mirror
//! (`03-design.md` §4.5, §5; rearch'd in
//! `docs/orchestrate/feature-completeness/02f-design-world-container-rearch.md`).
//!
//! ## World data: build-once hand-off (post-`02f`)
//!
//! C# NAADF owns `WorldData` as a single object — the CPU mirror arrays
//! (`dataChunk`/`dataBlock`/`dataVoxel`) and the GPU resources
//! (`dataChunkGpu`/`dataBlockGpu`/`dataVoxelGpu`) live on the same instance,
//! mutated by the editor and read by the renderer with no clone/extract layer
//! (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:20-218`). The
//! port's main↔render sub-app boundary forces a one-time CPU→GPU ferry at
//! startup; after that, per-edit changes flow through the W2 delta chain
//! (`pending_edits.batches` → `naadf_world_change_node`) and never through a
//! whole-world copy. The post-`02f` shape:
//!
//! - **No `dirty` flag**, no `ExtractedWorld` resource, no per-frame
//!   `extract_world` clone. (`02e` identified the per-frame 48 MiB clone as a
//!   ~20 ms/frame cost on Oasis; `03e` patched the flag but the clone path
//!   remained as a structural failure mode if any future code re-asserted
//!   `dirty`. `02f` deletes the path entirely.)
//! - [`stage_world_gpu_buildonce`] runs in `ExtractSchedule` once, gated on
//!   `Option<Res<WorldGpu>>::is_none()`. It clones the CPU mirror buffers
//!   into a transient [`WorldGpuStaging`] render-world resource — a single
//!   per-app hand-off, NOT a per-frame mirror.
//! - `render::prepare::prepare_world_gpu` consumes `WorldGpuStaging` once,
//!   builds the chunks 3D texture + blocks/voxels/voxel_types
//!   `GrowableBuffer`s + world_meta uniform + entity placeholders + the
//!   world bind group, inserts `WorldGpu`, then **drops** the staging
//!   resource. After that the build-once gate keeps both systems no-ops.
//! - W2 delta uploads consume `pending_edits.batches` via
//!   `extract_world_changes` → `ConstructionEvents` → `naadf_world_change_node`.
//!   The chunks texture / blocks buffer / voxels buffer are sized with edit
//!   headroom (`02f` R3) so the W2 dispatch's `block_voxel_count[]`-cursor
//!   appends land within the allocated capacity for the duration of typical
//!   editing strokes.
//!
//! ## Camera + flag mirrors (every-frame, fixed-size)
//!
//! - [`extract_camera`] / [`extract_camera_history`] / [`extract_taa_config`] /
//!   [`extract_gi_config`] — every frame, cheap fixed-size copies of camera /
//!   history / runtime-flag state. These are NOT world-data and have no
//!   bearing on the `02f` rearch — they stay as is.

use bevy::prelude::*;
use bevy::render::Extract;

use crate::camera::PositionSplit;
use crate::render::taa::{rotation_only_view_proj, CameraHistory, CAMERA_HISTORY_DEPTH};
use crate::voxel::VoxelType;
use crate::world::data::{IAabb3, VoxelTypes, WorldData};

/// Transient render-world hand-off resource carrying the CPU mirror buffers +
/// world metadata from the main-world [`WorldData`] / [`VoxelTypes`] for the
/// **one-time** GPU resource build (`02f` rearch). Inserted by
/// [`stage_world_gpu_buildonce`] on the first frame the main-world world data
/// exists; **consumed and dropped** by `prepare_world_gpu` once the GPU
/// `WorldGpu` is built.
///
/// Not used after frame ≤1 of the app. **No per-frame clone**: the build-once
/// gate (`Option<Res<WorldGpu>>::is_none()`) ensures the staging clone happens
/// once and never again.
///
/// If a future feature ever needs a whole-world re-upload (e.g. world reload
/// or live re-import), it re-creates this resource at that boundary — but no
/// such code path exists today. The W2 delta chain handles per-edit changes.
#[derive(Resource, Default)]
pub struct WorldGpuStaging {
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
    /// Phase-C followup #1 — dense pre-construction voxel-type stream
    /// (`size_in_voxels.x*y*z` u16s). Used by the runtime GPU producer
    /// dispatch to build `segment_voxel_buffer`. Empty when the source
    /// `WorldData` does not carry it (e.g. sparse `.vox` path —
    /// `02a-v2` Δ-GPUProducer).
    pub dense_voxel_types: Vec<u16>,
}

/// Transient render-world hand-off carrying a refreshed palette
/// (`web-vox-color-divergence` fix, 2026-05-18). Emitted by
/// [`stage_world_gpu_buildonce`] when `Changed<VoxelTypes>` fires AFTER
/// `WorldGpu` is built (the async `.vox` install case + any future
/// runtime palette swap). Consumed and dropped by `prepare_world_gpu`'s
/// focused-refresh branch.
///
/// Single-shot: emitted once per `Changed<VoxelTypes>` event after the
/// initial build-once `WorldGpuStaging` hand-off has been consumed.
/// `prepare_world_gpu`'s refresh branch re-packs the palette to
/// `Vec<GpuVoxelType>`, calls `world_gpu.voxel_types.upload_all(...)`,
/// rebuilds `WorldGpu.bind_group` (the @group(0) world bind group), and
/// removes `FrameGpu` so `prepare_frame_gpu` re-creates
/// `calc_new_taa_sample_bind_group` (which also binds
/// `world_gpu.voxel_types.buffer()`).
#[derive(Resource, Default)]
pub struct VoxelTypesRefresh {
    /// The refreshed palette to upload to GPU. Carried by value so the
    /// extract-side clone happens exactly once per refresh event.
    pub types: Vec<VoxelType>,
}

/// Render-world metadata mirror of `WorldData`'s size + dense voxel-type stream
/// (`02f` rearch). Populated alongside [`WorldGpuStaging`] by
/// [`stage_world_gpu_buildonce`] but **outlives** the staging resource: the
/// W1 GPU producer (`naadf_gpu_producer_node`) + `prepare_construction` read
/// `size_in_chunks` + `dense_voxel_types` to build `segment_voxel_buffer` on
/// the first frame the producer can dispatch (which may be several frames
/// after the staging hand-off, depending on pipeline-cache compilation).
///
/// Carrying just the size + the dense stream (not the chunks/blocks/voxels
/// CPU buffers) keeps this resource ~256 KiB for the test grid and ~0 B for
/// the `.vox` sparse path (which sets `dense_voxel_types = Vec::new()`). The
/// `02e` per-frame 48 MiB clone is GONE.
///
/// **DELIBERATELY MINIMAL.** Adding fields requires re-reading the brief's
/// "no `ExtractedWorld` resource" deletion directive (`02f` Decision 4). If a
/// new system needs more world data, prefer `Extract<Res<WorldData>>` from
/// `ExtractSchedule` instead of growing this resource.
#[derive(Resource, Default)]
pub struct WorldDataMeta {
    /// World size in chunks.
    pub size_in_chunks: UVec3,
    /// `WorldData.blocks_cpu.len()` at build time — used by
    /// `naadf_gpu_producer_node` to size its read of the GPU buffer.
    pub blocks_cpu_len: u32,
    /// `WorldData.voxels_cpu.len()` at build time — used by
    /// `naadf_gpu_producer_node` as above.
    pub voxels_cpu_len: u32,
    /// Phase-C followup #1 — dense pre-construction voxel-type stream
    /// (`size_in_voxels.x*y*z` u16s). Empty on the sparse `.vox` path.
    pub dense_voxel_types: Vec<u16>,
}

/// Render-world mirror of the main-world [`crate::aadf::generator::ModelData`]
/// (vox-gpu-rewrite W5.1). Populated **build-once** by
/// [`stage_model_data_buildonce`] on the first frame the main-world
/// `ModelData` exists (after `install_vox_in_fixed_world` inserts it). Drives
/// the W5 GPU producer chain in `naadf_gpu_producer_node`: presence of this
/// resource is the gate that switches the node from the chunk_calc-only
/// branch to the per-segment generator + chunk_calc chain.
///
/// Mirrors the `WorldGpuStaging` discipline (`extract.rs:67-87`) but is
/// **long-lived** — `prepare_construction` reads it every frame the W5 bind
/// group is being built (the buffers stay; the bind group only rebuilds when
/// `Option<BindGroup>` is `None`).
#[derive(Resource, Default, Clone)]
pub struct ModelDataRender {
    /// `ModelData.data_chunk` — `size_in_chunks.x * y * z` u32 entries.
    pub data_chunk: Vec<u32>,
    /// `ModelData.data_block` — variable length.
    pub data_block: Vec<u32>,
    /// `ModelData.data_voxel` — variable length, two voxels per u32.
    pub data_voxel: Vec<u32>,
    /// Model size in chunks (`ModelData.size_in_chunks`).
    pub size_in_chunks: [u32; 3],
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

/// `ExtractSchedule` system: **build-once** hand-off of the main-world
/// [`WorldData`] + [`VoxelTypes`] into the render-world [`WorldGpuStaging`]
/// resource (`02f` rearch).
///
/// Gated on `Option<Res<WorldGpu>>::is_none()` — once `prepare_world_gpu`
/// builds the GPU resources from the staging on the first frame, this system
/// short-circuits on every subsequent frame. **There is no per-frame clone.**
///
/// Also (re)populates [`WorldDataMeta`] with the size + dense voxel-type
/// stream, which the Phase-C followup #1 GPU producer reads on the first
/// frame all its dependencies (pipelines compiled, bind groups built) are
/// ready (which may be several frames after the GPU resource build).
///
/// Per-edit changes do NOT pass through here — they flow via the W2 delta
/// chain (`pending_edits.batches` → `extract_world_changes` →
/// `ConstructionEvents` → `naadf_world_change_node`). The build-once
/// staging is purely the initial world hand-off.
///
/// Replaces the pre-`02f` `extract_world` system (deleted along with
/// `ExtractedWorld`). The post-`02e` per-frame full-world clone is GONE.
pub fn stage_world_gpu_buildonce(
    mut commands: Commands,
    world_gpu_already_built: Option<Res<crate::render::prepare::WorldGpu>>,
    staging_existing: Option<Res<WorldGpuStaging>>,
    voxel_types_refresh_existing: Option<Res<VoxelTypesRefresh>>,
    mut meta: ResMut<WorldDataMeta>,
    world_data: Extract<Option<Res<WorldData>>>,
    voxel_types: Extract<Option<Res<VoxelTypes>>>,
) {
    // Build-once gate: if WorldGpu exists OR staging is already populated
    // (waiting to be consumed by prepare_world_gpu), do nothing — EXCEPT
    // when `WorldGpu` exists AND the main-world `VoxelTypes` resource
    // changed since our last extract run (the async `.vox` install case +
    // any future runtime palette swap). In that case, emit a one-shot
    // `VoxelTypesRefresh` hand-off so `prepare_world_gpu` can re-upload
    // the palette to GPU without rebuilding the geometry buffers
    // (`web-vox-color-divergence` design D-FOCUSED-REFRESH). Guard against
    // double-emission with `voxel_types_refresh_existing.is_some()`.
    if world_gpu_already_built.is_some() || staging_existing.is_some() {
        // Build-once already done. Check for the focused-refresh trigger.
        if world_gpu_already_built.is_some() && voxel_types_refresh_existing.is_none() {
            if let Some(vt) = voxel_types.as_ref() {
                if vt.is_changed() {
                    // web-vox-color-divergence (2026-05-18) — emit the
                    // refresh hand-off. This is the load-bearing fix for
                    // the async `.vox` install path: `install_imported_vox`
                    // does `commands.insert_resource(VoxelTypes { … })`
                    // over an existing resource, which flips `Changed<R>`
                    // for the next-tick extract query.
                    debug!(
                        "[palette-refresh] stage_world_gpu_buildonce emitting \
                         VoxelTypesRefresh (palette_len={})",
                        vt.types.len(),
                    );
                    commands.insert_resource(VoxelTypesRefresh {
                        types: vt.types.clone(),
                    });
                }
            }
        }
        return;
    }
    let (Some(world_data), Some(voxel_types)) = (&*world_data, &*voxel_types) else {
        return;
    };
    // One-time clone — the unavoidable main→render CPU buffer ferry. After
    // this frame, prepare_world_gpu consumes + drops the staging, and the
    // build-once gate above keeps both systems no-ops forever.
    let staging = WorldGpuStaging {
        chunks: world_data.chunks_cpu.clone(),
        blocks: world_data.blocks_cpu.clone(),
        voxels: world_data.voxels_cpu.clone(),
        voxel_types: voxel_types.types.clone(),
        size_in_chunks: world_data.size_in_chunks,
        bounding_box: world_data.bounding_box,
        dense_voxel_types: world_data.dense_voxel_types.clone(),
    };
    // Meta resource carries the size + dense voxel-type stream for the
    // GPU producer node, which may not run on the same frame as
    // prepare_world_gpu (pipeline-cache compilation is async).
    meta.size_in_chunks = world_data.size_in_chunks;
    meta.blocks_cpu_len = world_data.blocks_cpu.len() as u32;
    meta.voxels_cpu_len = world_data.voxels_cpu.len() as u32;
    meta.dense_voxel_types.clone_from(&world_data.dense_voxel_types);
    commands.insert_resource(staging);
}

/// `ExtractSchedule` system: **build-once** hand-off of the main-world
/// [`crate::aadf::generator::ModelData`] into the render-world
/// [`ModelDataRender`] resource (vox-gpu-rewrite W5.1).
///
/// Gated on `Option<Res<ModelDataRender>>::is_none()` — once
/// [`prepare_construction`]-side bind-group construction has its source
/// payload, this system short-circuits on every subsequent frame. **There is
/// no per-frame clone.** Mirrors `stage_world_gpu_buildonce` 1:1
/// (`extract.rs:167-203`).
///
/// Per Q2 decision (`vox-gpu-rewrite/01-context.md`): a SEPARATE resource
/// rather than extending `WorldDataMeta` (which carries the "DELIBERATELY
/// MINIMAL" docstring at `extract.rs:102-105`).
pub fn stage_model_data_buildonce(
    mut commands: Commands,
    existing: Option<Res<ModelDataRender>>,
    model_data: Extract<Option<Res<crate::aadf::generator::ModelData>>>,
) {
    if existing.is_some() {
        return;
    }
    let Some(model_data) = &*model_data else {
        return;
    };
    commands.insert_resource(ModelDataRender {
        data_chunk: model_data.data_chunk.clone(),
        data_block: model_data.data_block.clone(),
        data_voxel: model_data.data_voxel.clone(),
        size_in_chunks: model_data.size_in_chunks,
    });
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

/// `ExtractSchedule` system: mirror the main-world [`crate::render::budget::InvalidSampleStorageCount`]
/// into the render-world [`crate::render::budget::RenderInvalidSampleStorageCount`].
///
/// Per the post-deploy fix (`docs/orchestrate/mobile-budget/05-consolidated-fix.md`
/// Implementation log), the Android entry inserts `InvalidSampleStorageCount`
/// AFTER `build_app_with_args` returns — so a plugin-build-time snapshot would
/// see the defensive canonical seed (8), not the budget-selected mobile value
/// (typically 4). Extract runs every frame from `ExtractSchedule`, so the first
/// real frame (= when `prepare_gi` runs) sees the post-override budget value.
pub fn extract_invalid_sample_storage_count(
    mut mirror: ResMut<crate::render::budget::RenderInvalidSampleStorageCount>,
    src: Extract<Option<Res<crate::render::budget::InvalidSampleStorageCount>>>,
) {
    if let Some(src) = &*src {
        mirror.0 = src.0;
    }
}

/// `ExtractSchedule` system: mirror the main-world
/// [`crate::render::budget::EffectiveWorldSize`] into the render-world
/// [`crate::render::budget::RenderEffectiveWorldSize`].
///
/// Same rationale as [`extract_invalid_sample_storage_count`]: the Android
/// entry's `app.insert_resource(EffectiveWorldSize::from_segments(...))` runs
/// AFTER `build_app_with_args` returns — so a `NaadfRenderPlugin::build`-time
/// snapshot would capture only the defensive canonical seed `(16, 2, 16)`,
/// not the budget-selected mobile rung (e.g. `(6, 2, 6)` on Mali-G52). Extract
/// runs every frame, so the first real frame sees the post-override value.
pub fn extract_effective_world_size(
    mut mirror: ResMut<crate::render::budget::RenderEffectiveWorldSize>,
    src: Extract<Option<Res<crate::render::budget::EffectiveWorldSize>>>,
) {
    if let Some(src) = &*src {
        mirror.0 = **src;
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
