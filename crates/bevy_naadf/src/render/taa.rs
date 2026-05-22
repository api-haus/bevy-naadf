//! Phase A-2 long-term-memory TAA — the camera-history ring, the frame
//! counter, the Halton jitter, and the shared camera-matrix helper
//! (`06-design-a2.md` §2.3, §9.1, §9.3, §10.1).
//!
//! NAADF's albedo TAA keeps 128-deep CPU rings of per-frame camera state
//! (`WorldRenderAlbedo.cs:36-40`) plus a monotonic frame counter
//! (`WorldRender.cs:28,86`). The Bevy port mirrors that with the
//! [`CameraHistory`] main-world resource, updated once per frame by
//! [`update_camera_history`] and extracted into the render world.
//!
//! Batch 1 (this commit) lands the data + the update system + the shared
//! helpers; the `TaaGpu` render-world resource + the `prepare_taa` system are
//! added in step 5 of `06-design-a2.md` §12.

use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, Buffer, BufferDescriptor, BufferUsages,
    CommandEncoderDescriptor, PipelineCache,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use crate::camera::PositionSplit;
use crate::render::extract::{ExtractedCameraData, ExtractedCameraHistory};
use crate::render::gpu_types::{GpuCameraHistorySlot, GpuTaaParams};
use crate::render::pipelines::NaadfPipelines;

/// The camera-history ring depth — kept at NAADF's 128 (`WorldRenderAlbedo.cs:36-40`).
/// The `01-context.md` §6 VRAM lever is the *sample* ring, NOT this one — the
/// camera-matrix ring is tiny in VRAM, so it stays at NAADF's depth.
pub const CAMERA_HISTORY_DEPTH: usize = 128;

/// Main-world canonical resource carrying the long-term TAA on/off toggle
/// (`06-design-a2.md` §6.1, §8.2).
///
/// **Step 3 of the config-as-resource refactor** migrated `AppArgs.taa` onto
/// this per-domain resource. The render-world mirror is the existing
/// [`crate::render::extract::ExtractedTaaConfig`]; only the extract SOURCE
/// changed (was `Res<AppArgs>`, now `Res<TaaConfig>`). `update_camera_history`
/// also moved off `Res<AppArgs>` to `Res<TaaConfig>` for the same reason.
///
/// `Default::default()` = `TaaConfig { enabled: true }`, matching the
/// pre-refactor `AppArgs::default().taa` value. Inserted at bootstrap by
/// [`crate::bootstrap::build_app_with_bootstrap_inputs`] from a
/// [`crate::bootstrap::BootstrapInputs`] field; no CLI surface mutates it
/// today (the design's Decision §10 — `--taa <on|off>` is orthogonal future
/// work).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaaConfig {
    /// Whether long-term TAA is enabled (mirrors the pre-refactor
    /// `AppArgs.taa`).
    pub enabled: bool,
}

impl Default for TaaConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Main-world canonical resource carrying the configured TAA sample-ring
/// depth (`18-taa-fidelity.md` fix #3 — supersedes the former hard-coded
/// `TAA_SAMPLE_RING_DEPTH = 16` const).
///
/// **Inserted at bootstrap** by [`crate::bootstrap::build_app_with_bootstrap_inputs`]
/// from a [`crate::bootstrap::BootstrapInputs`] field (default
/// [`crate::DEFAULT_TAA_RING_DEPTH`] = 32). The mobile-budget overrides at
/// [`crate::build_app_with_budget`] / `main.rs` wasm32 path mutate
/// `inputs.taa_ring_depth` BEFORE bootstrap fans it out, so the inserted
/// value is the budget-selected rung (8 on Mali-G52). 16 / 24 are the
/// VRAM-lever alternatives between desktop canonical 32 and minimum 0.
///
/// Mirrored into the render sub-app each frame by
/// [`crate::render::extract::extract_taa_ring_depth`] into the render-world
/// [`RenderTaaRingConfig`]. Same shape as the
/// [`crate::render::budget::EffectiveWorldSize`] /
/// [`crate::render::budget::RenderEffectiveWorldSize`] precedent.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaaRingConfig {
    /// The sample-ring depth (default [`crate::DEFAULT_TAA_RING_DEPTH`] = 32).
    pub depth: u32,
}

impl Default for TaaRingConfig {
    fn default() -> Self {
        Self { depth: crate::DEFAULT_TAA_RING_DEPTH }
    }
}

/// Render-sub-app mirror of [`TaaRingConfig`].
///
/// **Plumbing — extract-driven, NOT plugin-build-snapshot.** Same rationale as
/// [`crate::render::budget::RenderEffectiveWorldSize`]: the mobile-budget /
/// wasm32 entry points insert the main-world [`TaaRingConfig`] (possibly with
/// a non-canonical depth) BEFORE the plugin pyramid builds, but other callers
/// (e2e gates) leave it at canonical default. A plugin-build snapshot would
/// risk drift; extract-driven copy makes the render-world value always
/// reflect the latest main-world value at frame N+1.
///
/// This is the **single config source of truth** on the render side: it feeds
/// BOTH the Rust buffer sizing in [`prepare_taa`] (`taa_samples` is
/// `pixel_count * depth`, `sample_age` clamps to `depth`) AND — via
/// `NaadfPipelines::from_world` reading the SAME resource — the WGSL
/// `#{TAA_SAMPLE_RING_DEPTH}` shader-def injected at pipeline specialisation.
/// The two sides MUST agree exactly: a buffer sized for N with a shader
/// looping/modulo'ing over M is silent ring corruption.
///
/// `Default` = canonical [`crate::DEFAULT_TAA_RING_DEPTH`] = 32 so the
/// resource is always present (used as the `init_resource` seed before the
/// first extract; `NaadfPipelines::from_world` runs in `RenderStartup` after
/// the first `ExtractSchedule`, so the mirror is post-extract by the time
/// the shader-def is read).
#[derive(Resource, Clone, Copy, Debug)]
pub struct RenderTaaRingConfig {
    /// The sample-ring depth.
    pub depth: u32,
}

impl Default for RenderTaaRingConfig {
    fn default() -> Self {
        Self { depth: crate::DEFAULT_TAA_RING_DEPTH }
    }
}

/// The 128-deep camera-history ring + the monotonic frame counter
/// (`06-design-a2.md` §2.3).
///
/// Main-world `Resource`, seeded once at startup and updated once per frame by
/// [`update_camera_history`]. The four parallel rings mirror NAADF's
/// `oldCamPositions[128]` / `taaSampleCamTransform[128]` / `taaSampleJitter[128]`
/// (`WorldRenderAlbedo.cs:36-40`).
///
/// `taa_index` is computed **once per frame** in `update_camera_history` (from
/// the pre-increment `frame_count`) and stored here, rather than re-derived
/// render-side — this eliminates the off-by-one trap around the `frame_count`
/// increment / `ExtractSchedule` boundary (`06-design-a2.md` §9.3, §13.6).
#[derive(Resource)]
pub struct CameraHistory {
    /// Per-frame camera `PositionSplit` (C# `oldCamPositions[128]`). Stored as
    /// the int+frac split so `prepare_taa` can derive the camera-relative
    /// `taaOldCamPosFromCurCamInt` with a `PositionSplit` subtraction — the D1
    /// camera-relative-rendering trick, kept precise for large worlds
    /// (`06-design-a2.md` §2.3).
    pub positions: [PositionSplit; CAMERA_HISTORY_DEPTH],
    /// Per-frame translation-free (rotation-only) view-proj matrix
    /// (C# `taaSampleCamTransform[128]`).
    pub view_proj: [Mat4; CAMERA_HISTORY_DEPTH],
    /// Per-frame *inverse* translation-free view-proj matrix
    /// (C# `taaSampleCamTransformInvers[128]`). Phase B's `renderSampleRefine`
    /// binds this ring as its `camRotOld` parameter and calls `getRayDir` with
    /// it (`09-design-b.md` §3.6). Populated as `view_proj.inverse()` in
    /// [`update_camera_history`].
    pub view_proj_inv: [Mat4; CAMERA_HISTORY_DEPTH],
    /// Per-frame Halton jitter (C# `taaSampleJitter[128]`).
    pub jitter: [Vec2; CAMERA_HISTORY_DEPTH],
    /// Monotonic frame counter (C# `WorldRender.frameCount`).
    pub frame_count: u32,
    /// `taaIndex` for the slot written *this* frame — computed once per frame
    /// in [`update_camera_history`] from the pre-increment `frame_count`
    /// (`06-design-a2.md` §9.3).
    pub taa_index: u32,
    /// This frame's Halton jitter — the same value written into
    /// `jitter[taa_index]`. Stored separately so `prepare_frame_gpu` can read
    /// it for `GpuRenderParams.taa_jitter` without re-deriving (one value,
    /// computed once — `06-design-a2.md` §9.3).
    pub current_jitter: Vec2,
}

impl Default for CameraHistory {
    fn default() -> Self {
        Self {
            positions: [PositionSplit::default(); CAMERA_HISTORY_DEPTH],
            view_proj: [Mat4::IDENTITY; CAMERA_HISTORY_DEPTH],
            view_proj_inv: [Mat4::IDENTITY; CAMERA_HISTORY_DEPTH],
            jitter: [Vec2::ZERO; CAMERA_HISTORY_DEPTH],
            frame_count: 0,
            // frame_count == 0 ⇒ taa_index == 127 (CAMERA_HISTORY_DEPTH - 1).
            taa_index: (CAMERA_HISTORY_DEPTH as u32) - 1,
            current_jitter: Vec2::ZERO,
        }
    }
}

/// `taaIndex = CAMERA_HISTORY_DEPTH - (frame_count % CAMERA_HISTORY_DEPTH) - 1`
/// (`WorldRender.cs:88`).
///
/// The single source of truth for the ring-slot derivation — both
/// [`update_camera_history`] and (Batch 1's later `prepare_taa`) must agree, so
/// the formula lives in exactly one place (`06-design-a2.md` §2.3).
pub fn taa_index_of(frame_count: u32) -> u32 {
    (CAMERA_HISTORY_DEPTH as u32) - (frame_count % CAMERA_HISTORY_DEPTH as u32) - 1
}

/// 1-D Halton sequence value at `index` in base `b` (C# `WorldRender.Halton1D`,
/// `WorldRender.cs:115-128`).
fn halton_1d(mut index: u32, b: u32) -> f32 {
    let mut f = 1.0_f32;
    let mut r = 0.0_f32;
    while index > 0 {
        f /= b as f32;
        r += f * (index % b) as f32;
        index /= b;
    }
    r
}

/// NAADF's per-frame Halton jitter (C# `WorldRender.getJitter`,
/// `WorldRender.cs:137-140`): the 2-D Halton of `(frame % 32) + 1` in bases
/// `(3, 7)`, minus `0.5` so it is centred on zero.
pub fn halton_jitter(frame: u32) -> Vec2 {
    let i = (frame % 32) + 1;
    Vec2::new(halton_1d(i, 3), halton_1d(i, 7)) - Vec2::splat(0.5)
}

/// The rotation-only (translation-free) view-projection matrix — the matrix
/// NAADF's `getRayDir` / TAA reproject consume (`06-design-a2.md` §9.3).
///
/// Mirrors NAADF's origin-based `viewProjTransform` (`Camera.cs:199-201` —
/// `CreateLookAt(Vector3::ZERO, …)`): `clip_from_view` composed with the
/// *rotation-only* inverse of the camera transform — no translation column.
/// The ray origin is supplied separately via `PositionSplit`.
///
/// The single place this formula lives — both `extract_camera` and
/// [`update_camera_history`] call it (each passing the rotation from the
/// transform source correct for its schedule), so the convention cannot drift.
pub fn rotation_only_view_proj(camera: &Camera, rotation: Quat) -> Mat4 {
    let clip_from_view = camera.clip_from_view();
    let world_from_view_rot = Mat4::from_quat(rotation);
    clip_from_view * world_from_view_rot.inverse()
}

/// `Update` system: write *this* frame's camera state into the
/// [`CameraHistory`] ring, then advance the frame counter
/// (`06-design-a2.md` §9.3).
///
/// Must run **after** `sync_position_split` (which updates the camera's
/// `PositionSplit` from the `Transform`) so the ring stores the current
/// frame's position. NAADF writes `oldCamPositions[taaIndex]` with the
/// *current* `taaIndex` (`WorldRenderAlbedo.cs:77-80`), where `taaIndex` is
/// derived from the *pre-frame* `frameCount` (`WorldRender.cs:88`) — so this
/// system derives `taa_index` from the current `frame_count`, writes the rings
/// at that slot, stores `taa_index` on `CameraHistory`, *then* increments
/// `frame_count`. The render side reads the stored `taa_index` directly rather
/// than re-deriving it, eliminating the off-by-one trap.
///
/// Uses the camera's `Transform` (not `GlobalTransform`): the camera has no
/// parent, so `Transform == GlobalTransform` for it, and `Transform` is the
/// *current*-frame value in `Update` (`GlobalTransform` propagation runs in
/// `PostUpdate`).
///
/// The camera is matched by `With<PositionSplit>` — the marker of *the NAADF
/// render camera* — NOT `With<FreeCamera>`. `FreeCamera` is an input concern
/// (the fly-camera plugin); the frame counter + camera-history ring are render
/// concerns that must advance for *every* configuration of the render camera.
/// The e2e harness spawns a fixed-pose camera **without** `FreeCamera`
/// (`e2e/mod.rs setup_e2e_camera`); a `With<FreeCamera>` filter made `Single`
/// match nothing there, so the system was silently skipped and `frame_count`
/// stayed pinned at 0 — which froze the atmosphere precompute's
/// `frameCount % 4` quarter-stride on a single quarter, leaving 3/4 of the
/// octahedral buffer stale-zero (the out-of-volume streaking artifact).
pub fn update_camera_history(
    camera: Single<(&Camera, &Transform, &PositionSplit), With<PositionSplit>>,
    taa: Res<TaaConfig>,
    mut history: ResMut<CameraHistory>,
) {
    let (camera, transform, position_split) = *camera;

    // taaIndex from the *current* (pre-increment) frame_count.
    let taa_index = taa_index_of(history.frame_count);

    // This frame's Halton jitter — zero when TAA is off (`06-design-a2.md`
    // §9.3: TAA-on implies jitter-on; no separate `isTAAJitter` knob).
    let jitter = if taa.enabled {
        halton_jitter(history.frame_count)
    } else {
        Vec2::ZERO
    };

    let view_proj = rotation_only_view_proj(camera, transform.rotation);
    // C# `taaSampleCamTransformInvers[taaIndex] = camera.invViewProjTransform`
    // (`WorldRenderBase.cs:147`) — Phase B's `renderSampleRefine` needs the
    // inverse rotation-only view-proj ring (`09-design-b.md` §3.6). One extra
    // `.inverse()` per frame — cheap.
    let view_proj_inv = view_proj.inverse();

    let slot = taa_index as usize;
    // `*position_split` is current — `sync_position_split` runs before this.
    history.positions[slot] = *position_split;
    history.view_proj[slot] = view_proj;
    history.view_proj_inv[slot] = view_proj_inv;
    history.jitter[slot] = jitter;
    history.taa_index = taa_index;
    history.current_jitter = jitter;

    // Advance the monotonic frame counter (C# `WorldRender.cs:86`).
    history.frame_count = history.frame_count.wrapping_add(1);
}

/// The render-world GPU resource owning the TAA buffers (`06-design-a2.md`
/// §9.4). Created once by [`prepare_taa`].
///
/// `taa_sample_accum` is the real `taaSampleAccum` — it *replaces* Phase A's
/// `FrameGpu.shaded_color` stand-in (the Phase-A stand-in was deliberately
/// built to the `taaSampleAccum` element format, so this is a rename + re-home,
/// not a format change — `06-design-a2.md` §2.2). `prepare_frame_gpu` binds
/// `taa_sample_accum` where it used to bind `shaded_color`.
#[derive(Resource)]
pub struct TaaGpu {
    /// The 16-deep sample ring — `pixel_count * 16` × `vec2<u32>`. Slot-major
    /// (`06-design-a2.md` §2.1). `STORAGE | COPY_DST`, zero-cleared on creation,
    /// resized on viewport change. Written by the first-hit pass (Batch 2),
    /// read by the reproject pass (Batch 2).
    pub taa_samples: Buffer,
    /// The per-pixel accumulated colour + count — `pixel_count` × `vec2<u32>`.
    /// The real `taaSampleAccum`; replaces Phase A's `shaded_color`.
    pub taa_sample_accum: Buffer,
    /// The `base/renderTaaSampleReverse.fx` `ReprojectOld` extra output —
    /// `pixel_count` × `vec2<u32>` (`09-design-b.md` §3.5). `.x` = packed
    /// `f16(distMin) | f16(distMax)<<16`, `.y` = the packed specular-normal
    /// validity mask. `STORAGE | COPY_DST`, zero-cleared on creation, resized
    /// with `taa_samples`. Batch 4 lands the *buffer* (so the sample-refine
    /// bind group can reference it); Batch 6 wires the `base/` `ReprojectOld`
    /// shader write — until then it is the zero-cleared buffer (`09-design-b.md`
    /// §11 Batch 4 step 13 — the sample-refine validity test rejects
    /// everything, correct-but-empty).
    pub taa_dist_min_max: Buffer,
    /// The 128-deep camera-history ring — `128` × `GpuCameraHistorySlot`,
    /// fixed-size (not resized on viewport change). Rewritten every frame.
    pub camera_history: Buffer,
    /// The TAA reproject pass's scalar uniform (`GpuTaaParams`). Rewritten
    /// every frame.
    pub taa_params: Buffer,
    /// Pixel count the screen-space buffers (`taa_samples` / `taa_sample_accum`)
    /// are sized for — the resize trigger.
    pub pixel_count: u32,
    /// `@group(2)` for the first-hit pass — just `taa_samples`. Rebuilt only
    /// when `taa_samples` is (re-)created. Unused until Batch 2 step 6.
    pub taa_first_hit_bind_group: BindGroup,
}

/// `RenderSystems::PrepareResources` system: create + (re)size the TAA buffers,
/// upload the per-frame camera-history ring + the TAA uniform, and build the
/// first-hit `@group(2)` bind group (`06-design-a2.md` §9.2).
///
/// Runs in `PrepareResources` alongside `prepare_world_gpu` — *before*
/// `prepare_frame_gpu` (`PrepareBindGroups`), which needs `TaaGpu` to exist so
/// it can bind `taa_sample_accum`. Skips silently until both the camera and the
/// camera-history have been extracted.
///
/// Batch 1 note: `taa_samples` is created + zero-cleared + uploaded into the
/// bind group, but nothing *writes* it yet (the first-hit ring write is Batch 2
/// step 6) and nothing *reads* it yet (the reproject pass is Batch 2). The
/// `camera_history` / `taa_params` uploads likewise have no consumer until the
/// Batch 2 reproject node. Batch 1 lands the buffers + the upload plumbing; the
/// drop-in swap it *does* land this batch is `taa_sample_accum` replacing
/// `shaded_color` as the blit source.
// Bevy systems legitimately exceed clippy's 7-argument ceiling.
#[allow(clippy::too_many_arguments)]
pub fn prepare_taa(
    mut commands: Commands,
    extracted_camera: Res<ExtractedCameraData>,
    extracted_history: Res<ExtractedCameraHistory>,
    ring_config: Res<RenderTaaRingConfig>,
    existing: Option<Res<TaaGpu>>,
    pipelines: Res<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    if !extracted_camera.valid || !extracted_history.valid {
        return;
    }
    let viewport = extracted_camera.viewport_size.max(UVec2::ONE);
    let pixel_count = viewport.x * viewport.y;
    // The configured sample-ring depth — the single render-side source of
    // truth, shared with `NaadfPipelines`'s WGSL `#{TAA_SAMPLE_RING_DEPTH}`
    // shader-def so the buffer size and the shader's loop bounds / modulo
    // agree exactly (`18-taa-fidelity.md` fix #3).
    let ring_depth = ring_config.depth;

    // --- (re)create the screen-space buffers on a viewport change -----------
    // `taa_samples` (pixel_count * 16 × vec2<u32>) + `taa_sample_accum`
    // (pixel_count × vec2<u32>) resize on the same trigger as `first_hit_data`.
    // `camera_history` + `taa_params` are fixed-size — created once, never
    // resized. On resize the whole `taa_samples` ring is discarded (it is
    // screen-space); the next ~16 frames rebuild it from zeroed (rejected)
    // history, which is correct and unavoidable (NAADF does the same).
    let (
        taa_samples,
        taa_sample_accum,
        taa_dist_min_max,
        camera_history,
        taa_params,
        needs_new_storage,
    ) = match &existing {
            Some(taa) if taa.pixel_count == pixel_count => (
                taa.taa_samples.clone(),
                taa.taa_sample_accum.clone(),
                taa.taa_dist_min_max.clone(),
                taa.camera_history.clone(),
                taa.taa_params.clone(),
                false,
            ),
            Some(taa) => {
                // Viewport changed — re-create only the screen-space buffers;
                // keep the fixed-size `camera_history` / `taa_params`.
                let (taa_samples, taa_sample_accum, taa_dist_min_max) =
                    create_screen_buffers(&render_device, pixel_count, ring_depth);
                (
                    taa_samples,
                    taa_sample_accum,
                    taa_dist_min_max,
                    taa.camera_history.clone(),
                    taa.taa_params.clone(),
                    true,
                )
            }
            None => {
                // First build — create everything.
                let (taa_samples, taa_sample_accum, taa_dist_min_max) =
                    create_screen_buffers(&render_device, pixel_count, ring_depth);
                let camera_history = render_device.create_buffer(&BufferDescriptor {
                    label: Some("naadf_taa_camera_history"),
                    size: (CAMERA_HISTORY_DEPTH as u64)
                        * std::mem::size_of::<GpuCameraHistorySlot>() as u64,
                    usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let taa_params = render_device.create_buffer(&BufferDescriptor {
                    label: Some("naadf_taa_params"),
                    size: std::mem::size_of::<GpuTaaParams>() as u64,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                (
                    taa_samples,
                    taa_sample_accum,
                    taa_dist_min_max,
                    camera_history,
                    taa_params,
                    true,
                )
            }
        };

    // Zero-clear the screen-space buffers when freshly (re)created so the
    // first ~16 frames — before the sample ring is full — read zeroed
    // (rejected) history rather than garbage (`06-design-a2.md` §2.1).
    // `taa_dist_min_max` is zero-cleared here too — Batch 6 wires its shader
    // write; until then it stays the zero-cleared buffer (`09-design-b.md` §11
    // Batch 4 step 13).
    if needs_new_storage {
        let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("naadf_clear_taa_buffers"),
        });
        encoder.clear_buffer(&taa_samples, 0, None);
        encoder.clear_buffer(&taa_sample_accum, 0, None);
        encoder.clear_buffer(&taa_dist_min_max, 0, None);
        render_queue.submit([encoder.finish()]);
    }

    // --- upload the 128-deep camera-history ring (every frame) --------------
    // `cam_pos_from_cur_int[i] = (positions[i] - current_camera).to_world()` —
    // the C# `(oldCamPositions[i] - camPos).toVector3()`
    // (`WorldRenderAlbedo.cs:83`): each past frame's camera position expressed
    // relative to the *current* frame's camera int position. The `PositionSplit`
    // subtraction keeps it precise for large worlds (the D1 trick).
    let current_pos = extracted_camera.position_split;
    let mut history_slots = [GpuCameraHistorySlot {
        view_proj: Mat4::IDENTITY,
        view_proj_inv: Mat4::IDENTITY,
        cam_pos_from_cur_int: Vec3::ZERO,
        _pad0: 0,
        jitter: Vec2::ZERO,
        _pad1: Vec2::ZERO,
    }; CAMERA_HISTORY_DEPTH];
    for (i, slot) in history_slots.iter_mut().enumerate() {
        let rel = extracted_history.positions[i] - current_pos;
        *slot = GpuCameraHistorySlot {
            view_proj: extracted_history.view_proj[i],
            // C# `taaSampleCamTransformInvers[i]` — `renderSampleRefine`'s
            // `camRotOld` (`09-design-b.md` §3.6). The reproject pass does not
            // read it; uploaded now so the slot layout matches the widened
            // struct and Batch 3+'s `renderSampleRefine` has the data.
            view_proj_inv: extracted_history.view_proj_inv[i],
            cam_pos_from_cur_int: rel.to_world(),
            _pad0: 0,
            jitter: extracted_history.jitter[i],
            _pad1: Vec2::ZERO,
        };
    }
    render_queue.write_buffer(&camera_history, 0, bytemuck::cast_slice(&history_slots));

    // --- upload the TAA reproject uniform (every frame) ---------------------
    let taa_params_data = GpuTaaParams {
        inv_view_proj: extracted_camera.inv_view_proj,
        view_proj: extracted_camera.view_proj,
        cam_pos_int: current_pos.pos_int,
        _pad0: 0,
        cam_pos_frac: current_pos.pos_frac,
        _pad1: 0,
        screen_width: viewport.x,
        screen_height: viewport.y,
        frame_count: extracted_history.frame_count,
        taa_index: extracted_history.taa_index,
        // How many past frames the reproject pass walks — the full configured
        // ring depth, clamped to `[1, ring_depth]` (`06-design-a2.md` §7.1).
        // NAADF exposes this as a 1–`ringDepth` ImGui slider; the port has no
        // GUI, so it is the full history (`18-taa-fidelity.md` fix #3).
        sample_age: ring_depth.clamp(1, ring_depth),
        _pad2: 0,
        _pad3: 0,
        _pad4: 0,
    };
    render_queue.write_buffer(&taa_params, 0, bytemuck::bytes_of(&taa_params_data));

    // --- the first-hit pass's @group(2) bind group --------------------------
    // Rebuilt only when `taa_samples` is (re-)created.
    let taa_first_hit_bind_group = match &existing {
        Some(taa) if !needs_new_storage => taa.taa_first_hit_bind_group.clone(),
        _ => render_device.create_bind_group(
            "naadf_taa_first_hit_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.taa_layout),
            &BindGroupEntries::sequential((taa_samples.as_entire_buffer_binding(),)),
        ),
    };

    commands.insert_resource(TaaGpu {
        taa_samples,
        taa_sample_accum,
        taa_dist_min_max,
        camera_history,
        taa_params,
        pixel_count,
        taa_first_hit_bind_group,
    });
}

/// Create the three screen-space TAA buffers (`taa_samples` ring +
/// `taa_sample_accum` + `taa_dist_min_max`) for `pixel_count` pixels.
/// `STORAGE | COPY_DST` — the COPY_DST is for the `clear_buffer` zero-fill +
/// (Batch 2) any explicit uploads.
///
/// `ring_depth` is the configured TAA sample-ring depth (`AppArgs
/// .taa_ring_depth`, default 32 — `18-taa-fidelity.md` fix #3); `taa_samples`
/// is sized `pixel_count * ring_depth`. The WGSL side's `% TAA_SAMPLE_RING_DEPTH`
/// / loop bounds use the SAME value via the `#{TAA_SAMPLE_RING_DEPTH}`
/// shader-def, so the buffer size and the shader indexing agree exactly.
fn create_screen_buffers(
    render_device: &RenderDevice,
    pixel_count: u32,
    ring_depth: u32,
) -> (Buffer, Buffer, Buffer) {
    // wgpu rejects zero-length buffers — `pixel_count` is already `>= 1`.
    // `taa_samples`: pixel_count * ring_depth × vec2<u32> (8 bytes each).
    let taa_samples = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_taa_samples"),
        size: (pixel_count as u64) * (ring_depth as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // `taa_sample_accum`: pixel_count × vec2<u32> (8 bytes each).
    let taa_sample_accum = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_taa_sample_accum"),
        size: (pixel_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // `taa_dist_min_max`: pixel_count × vec2<u32> (8 bytes each) — the `base/`
    // `ReprojectOld` extra output (`09-design-b.md` §3.5). Batch 4 creates the
    // buffer; Batch 6 wires the shader write.
    let taa_dist_min_max = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_taa_dist_min_max"),
        size: (pixel_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (taa_samples, taa_sample_accum, taa_dist_min_max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TaaRingConfig::default().depth` MUST be the documented default
    /// (`18-taa-fidelity.md` fix #3): a mismatch between the const + the
    /// default would mean the WGSL shader-def and the Rust buffer sizing
    /// disagree by default, which is silent TAA ring corruption.
    ///
    /// Relocated from `app_args.rs::tests` in Step 2 of the
    /// configuration-as-resource refactor — the canonical default now lives
    /// on the per-domain [`TaaRingConfig`] resource rather than the
    /// god-resource `AppArgs.taa_ring_depth` field.
    #[test]
    fn default_taa_ring_depth_is_32() {
        assert_eq!(crate::DEFAULT_TAA_RING_DEPTH, 32);
        assert_eq!(
            TaaRingConfig::default().depth,
            crate::DEFAULT_TAA_RING_DEPTH
        );
    }

    /// The ring depth must stay in the supported VRAM-lever range — 16 / 24 /
    /// 32 are the three values the design records (`01-context.md` §2c /
    /// `design-exploration-qa.md` §6 + the `18-taa-fidelity.md` fix #3
    /// supersession). Pin the default at 32 so future edits do not silently
    /// roll back to the old 16-deep value.
    ///
    /// Relocated from `app_args.rs::tests` in Step 2.
    #[test]
    fn default_taa_ring_depth_is_a_supported_lever_value() {
        let depth = TaaRingConfig::default().depth;
        assert!(
            matches!(depth, 16 | 24 | 32),
            "taa_ring_depth = {depth} is not one of the supported 16/24/32 lever values"
        );
    }

    /// The render-world mirror also defaults to the canonical depth, so the
    /// pre-extract first-frame seed in [`RenderTaaRingConfig::Default`] is
    /// safe (`from_world` reading the resource before the first extract sees
    /// the canonical value, identical to today's behaviour).
    #[test]
    fn render_mirror_default_is_canonical() {
        assert_eq!(
            RenderTaaRingConfig::default().depth,
            crate::DEFAULT_TAA_RING_DEPTH
        );
    }
}
