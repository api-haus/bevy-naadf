//! `Prepare` set: upload buffers, build bind groups, write camera uniforms
//! (`03-design.md` §4.5, §5).
//!
//! Two prepare systems:
//!
//! - [`prepare_world_gpu`] — on the first dirty frame, create the `chunks` 3D
//!   texture + the `blocks` / `voxels` / `voxel_types` `GrowableBuffer`s + the
//!   `world_meta` uniform, upload all of them, and build `bind_group_world`.
//!   Build-once (D2): later frames are a no-op.
//! - [`prepare_frame_gpu`] — every frame: `write_buffer` the `GpuCamera` +
//!   `GpuRenderParams` uniforms, (re)create the `first_hit_data` storage buffer
//!   on a viewport resize, and build `bind_group_frame`. The per-pixel
//!   accumulated-colour buffer (Phase A's `shaded_color` stand-in) moved into
//!   `TaaGpu` as the real `taa_sample_accum` — `prepare_frame_gpu` reads
//!   `TaaGpu` and binds it (`06-design-a2.md` §5.5, §9.4).
//!
//! The chunk layer is a CPU-built, upload-only 3D texture (`03-design.md`
//! §2.5, §6.1) — the render pass only ever *reads* it, sidestepping wgpu's
//! storage-texture read-write restriction.

use std::f32::consts::PI;

use bevy::math::Vec3;
use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
    Extent3d, PipelineCache, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use crate::render::extract::{ExtractedCameraData, ExtractedCameraHistory, ExtractedWorld};
use crate::render::gpu_types::{
    GpuCamera, GpuRenderParams, GpuVoxelType, GpuWorldMeta, FLAG_CHECK_SUN, FLAG_IS_TAA,
};
use crate::render::pipelines::NaadfPipelines;
use crate::render::taa::TaaGpu;
use crate::world::buffer::{GrowableBuffer, GROWABLE_BUFFER_USAGES};

/// The GPU side of the voxel world (`03-design.md` §4.4 — render-world
/// `WorldGpu` resource). Created once by [`prepare_world_gpu`].
#[derive(Resource)]
pub struct WorldGpu {
    /// The chunk layer — a CPU-built, upload-only `R32Uint` 3D texture.
    pub chunks: Texture,
    /// View of [`chunks`](Self::chunks) for binding.
    pub chunks_view: TextureView,
    /// The block layer — a growable `u32` storage buffer.
    pub blocks: GrowableBuffer<u32>,
    /// The voxel layer — a growable `u32` storage buffer (packed voxels).
    pub voxels: GrowableBuffer<u32>,
    /// The material buffer — a growable `vec4<u32>` storage buffer.
    pub voxel_types: GrowableBuffer<GpuVoxelType>,
    /// The `world_meta` uniform buffer.
    pub world_meta: Buffer,
    /// `@group(0)` bind group binding all of the above.
    pub bind_group: BindGroup,
}

/// The per-frame GPU resources (`03-design.md` §4.4 — render-world `FrameGpu`
/// resource). The uniforms are rewritten every frame; the storage buffers are
/// rebuilt only on a viewport resize.
#[derive(Resource)]
pub struct FrameGpu {
    /// `GpuCamera` uniform buffer.
    pub camera: Buffer,
    /// `GpuRenderParams` uniform buffer.
    pub render_params: Buffer,
    /// The Phase-A G-buffer — one `vec4<u32>` per pixel (`03-design.md` §5.3).
    pub first_hit_data: Buffer,
    /// Pixel count the storage buffers are currently sized for.
    pub pixel_count: u32,
    /// `@group(1)` bind group for the first-hit compute pass. Binds
    /// `taa_sample_accum` (owned by `TaaGpu`) at slot 3 — Phase A-2 moved the
    /// per-pixel accumulated-colour buffer out of `FrameGpu` (it used to be
    /// the `shaded_color` stand-in — `06-design-a2.md` §9.4).
    pub bind_group: BindGroup,
    /// The final-blit pass's own bind group (`first_hit_data`,
    /// `taa_sample_accum`, `render_params`).
    pub blit_bind_group: BindGroup,
    /// The TAA reproject pass's single bind group (`06-design-a2.md` §5.3,
    /// §5.5). Mixes `TaaGpu` resources (`taa_params`, `camera_history`,
    /// `taa_samples`, `taa_sample_accum`) with `FrameGpu.first_hit_data`, so it
    /// is built here in `prepare_frame_gpu` (after both `TaaGpu` and
    /// `first_hit_data` exist). Consumed by `naadf_taa_reproject_node`.
    pub taa_reproject_bind_group: BindGroup,
}

/// `RenderSystems::PrepareResources` system: create + upload the world GPU
/// resources on the first dirty frame, then build the world bind group.
///
/// Build-once (D2): after the first upload `ExtractedWorld.dirty` is cleared,
/// so subsequent frames return early. The chunk texture is written via
/// `queue.write_texture`; the block / voxel / voxel-type buffers go through
/// [`GrowableBuffer::upload_all`].
pub fn prepare_world_gpu(
    mut commands: Commands,
    mut extracted: ResMut<ExtractedWorld>,
    existing: Option<Res<WorldGpu>>,
    pipelines: Res<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    // Build-once: skip unless this is the first build or the data changed.
    if existing.is_some() && !extracted.dirty {
        return;
    }
    if extracted.chunks.is_empty() {
        // `setup_test_grid` has not run / extracted yet.
        return;
    }

    let size = extracted.size_in_chunks.max(UVec3::ONE);

    // --- chunk layer: a CPU-built, upload-only R32Uint 3D texture -----------
    let chunks = render_device.create_texture(&TextureDescriptor {
        label: Some("naadf_chunks"),
        size: Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: size.z,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D3,
        format: TextureFormat::R32Uint,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Pad the chunk buffer to the full texture extent (the CPU mirror is
    // already sized `size.x * y * z`, but be defensive).
    let chunk_count = (size.x * size.y * size.z) as usize;
    let mut chunk_data = extracted.chunks.clone();
    chunk_data.resize(chunk_count, 0);
    render_queue.write_texture(
        TexelCopyTextureInfo {
            texture: &chunks,
            mip_level: 0,
            origin: Default::default(),
            aspect: Default::default(),
        },
        bytemuck::cast_slice(&chunk_data),
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.x * 4),
            rows_per_image: Some(size.y),
        },
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: size.z,
        },
    );
    let chunks_view = chunks.create_view(&TextureViewDescriptor::default());

    // --- block / voxel / voxel-type growable buffers ------------------------
    // wgpu storage buffers can't be zero-length — ensure at least one element.
    let blocks_data: Vec<u32> = if extracted.blocks.is_empty() {
        vec![0]
    } else {
        extracted.blocks.clone()
    };
    let voxels_data: Vec<u32> = if extracted.voxels.is_empty() {
        vec![0]
    } else {
        extracted.voxels.clone()
    };
    let voxel_types_data: Vec<GpuVoxelType> = if extracted.voxel_types.is_empty() {
        vec![GpuVoxelType { data: [0; 4] }]
    } else {
        extracted
            .voxel_types
            .iter()
            .map(GpuVoxelType::from_voxel_type)
            .collect()
    };

    let mut blocks = GrowableBuffer::<u32>::new(&render_device, "naadf_blocks", blocks_data.len() as u64);
    let mut voxels = GrowableBuffer::<u32>::new(&render_device, "naadf_voxels", voxels_data.len() as u64);
    let mut voxel_types = GrowableBuffer::<GpuVoxelType>::new(
        &render_device,
        "naadf_voxel_types",
        voxel_types_data.len() as u64,
    );
    blocks.upload_all(&blocks_data, &render_device, &render_queue);
    voxels.upload_all(&voxels_data, &render_device, &render_queue);
    voxel_types.upload_all(&voxel_types_data, &render_device, &render_queue);

    // --- world_meta uniform -------------------------------------------------
    // The ray-AABB bounds NAADF's `rayAABB` / `shootRay` clip to. Faithful to
    // `WorldData.setEffect` (`WorldData.cs:477-478`): the world extent inset by
    // 0.1 voxel on every side — `boundingBoxMin = (0.1,0.1,0.1)`,
    // `boundingBoxMax = sizeInVoxels - (0.1,0.1,0.1)`. `extracted.bounding_box`
    // is the inclusive integer voxel AABB `{ min: 0, max: sizeInVoxels - 1 }`,
    // so `sizeInVoxels = bounding_box.max + 1`. The 0.1 inset keeps the ray
    // entry point off the integer voxel planes — without it, an out-of-volume
    // camera's entry point lands exactly on a voxel boundary and `floor()`
    // flips per-pixel with f32 noise (the concentric-lines artifact).
    let size_in_voxels = (extracted.bounding_box.max + IVec3::ONE).as_vec3();
    let world_meta_data = GpuWorldMeta {
        size_in_chunks: size,
        _pad0: 0,
        bounding_box_min: extracted.bounding_box.min.as_vec3() + Vec3::splat(0.1),
        _pad1: 0,
        bounding_box_max: size_in_voxels - Vec3::splat(0.1),
        _pad2: 0,
    };
    let world_meta = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_world_meta"),
        size: std::mem::size_of::<GpuWorldMeta>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    render_queue.write_buffer(&world_meta, 0, bytemuck::bytes_of(&world_meta_data));

    // --- @group(0) bind group ----------------------------------------------
    let bind_group = render_device.create_bind_group(
        "naadf_world_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.world_layout),
        &BindGroupEntries::sequential((
            &chunks_view,
            blocks.buffer().as_entire_buffer_binding(),
            voxels.buffer().as_entire_buffer_binding(),
            voxel_types.buffer().as_entire_buffer_binding(),
            world_meta.as_entire_buffer_binding(),
        )),
    );

    commands.insert_resource(WorldGpu {
        chunks,
        chunks_view,
        blocks,
        voxels,
        voxel_types,
        world_meta,
        bind_group,
    });
    // Build-once: consumed — clear the flag so this stays a no-op.
    extracted.dirty = false;
}

/// `RenderSystems::PrepareBindGroups` system: write the per-frame camera +
/// render-params uniforms, (re)create the `first_hit_data` storage buffer on a
/// viewport resize, and build the frame bind groups.
///
/// Runs in `PrepareBindGroups` (after `PrepareResources`) so the world bind
/// group / pipelines *and* `TaaGpu` are already created. Skips silently until
/// the camera has been extracted and `TaaGpu` exists.
///
/// Phase A-2: the per-pixel accumulated-colour buffer (Phase A's `shaded_color`
/// stand-in) moved into `TaaGpu` as the real `taa_sample_accum`; this system
/// reads `TaaGpu` and binds `taa_gpu.taa_sample_accum` where it used to bind
/// the local `shaded_color` (`06-design-a2.md` §5.5, §9.4).
pub fn prepare_frame_gpu(
    mut commands: Commands,
    extracted_camera: Res<ExtractedCameraData>,
    extracted_history: Res<ExtractedCameraHistory>,
    extracted_taa: Res<crate::render::extract::ExtractedTaaConfig>,
    existing: Option<ResMut<FrameGpu>>,
    taa_gpu: Option<Res<TaaGpu>>,
    pipelines: Res<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    if !extracted_camera.valid {
        return;
    }
    // `TaaGpu` (created in `PrepareResources` by `prepare_taa`) owns
    // `taa_sample_accum` — wait for it before building the bind groups.
    let Some(taa_gpu) = taa_gpu else {
        return;
    };
    let viewport = extracted_camera.viewport_size.max(UVec2::ONE);
    let pixel_count = viewport.x * viewport.y;

    // A simple fixed sun for Phase A's flat-lit scene. `sky_sun_dir` points
    // *towards* the sun (the C# `skySunDir` convention).
    let sun_elev = 0.9_f32;
    let sun_azim = 0.6_f32;
    let sky_sun_dir = Vec3::new(
        sun_elev.cos() * sun_azim.cos(),
        sun_elev.sin(),
        sun_elev.cos() * sun_azim.sin(),
    )
    .normalize();
    let _ = PI; // sun angles are hand-tuned constants for Phase A.

    let camera_data = GpuCamera {
        inv_view_proj: extracted_camera.inv_view_proj,
        cam_pos_int: extracted_camera.position_split.pos_int,
        _pad0: 0,
        cam_pos_frac: extracted_camera.position_split.pos_frac,
        _pad1: 0,
    };
    let render_params = GpuRenderParams {
        screen_width: viewport.x,
        screen_height: viewport.y,
        // The real monotonic frame counter (the carried `05-review.md` §4 fix —
        // `06-design-a2.md` §9.1). `frame_count` / `taa_index` come from the
        // extracted `CameraHistory`, computed once per frame in
        // `update_camera_history` (`06-design-a2.md` §9.3 — `taa_index` is
        // *stored*, not re-derived render-side, to avoid the off-by-one trap).
        frame_count: extracted_history.frame_count,
        // `rand_counter` = the frame counter (the monotonic per-frame RNG salt
        // — `init_rand` uses it only as salt). Deliberate A-2 simplification:
        // NAADF refills a `randValues[32]` table per frame and indexes it
        // (`WorldRender.cs:82-86`); the load-bearing property is a
        // per-frame-varying salt, which the counter already is — the table is
        // not ported (`06-design-a2.md` §4.1, §13.3).
        rand_counter: extracted_history.frame_count,
        taa_index: extracted_history.taa_index,
        // `FLAG_IS_TAA` gates the first-hit pass's `taa_samples` ring write
        // (`06-design-a2.md` §6.1) — set when `AppArgs.taa` is on (extracted
        // into `ExtractedTaaConfig`). When TAA is off the flag is clear, the
        // ring write is skipped, `update_camera_history` keeps `current_jitter`
        // zero (so `taa_jitter` below is zero), and the reproject node
        // early-returns — the result is bit-identical to Phase A.
        flags: if extracted_taa.enabled {
            FLAG_CHECK_SUN | FLAG_IS_TAA
        } else {
            FLAG_CHECK_SUN
        },
        exposure: 1.5,
        // C# `Settings.data.general.toneMappingFac` — a constant in the port
        // (`09-design-b.md` §5.9). Consumed by Batch 6's `base/` final blit;
        // set now so the layout slot carries the right value from Batch 1 on.
        tone_mapping_fac: 1.0,
        sky_sun_dir,
        _pad1: 0,
        sun_color: Vec3::new(1.0, 0.95, 0.85),
        _pad2: 0,
        // This frame's Halton jitter — the same value `update_camera_history`
        // wrote into `CameraHistory.jitter[taa_index]` (one value, computed
        // once — `06-design-a2.md` §9.3). Zero unless `AppArgs.taa` is on.
        taa_jitter: extracted_history.current_jitter,
        _pad3: Vec2::ZERO,
        bounding_box_min: Vec3::ZERO, // filled below from WorldGpu's meta? — see note
        _pad4: 0,
        bounding_box_max: Vec3::ZERO,
        _pad5: 0,
    };

    // The bounding box the first-hit `rayAABB` tests against comes from the
    // extracted world, not the camera — but `prepare_frame_gpu` only has the
    // camera. The world's bounding box is uploaded in `world_meta`
    // (`@group(0)`), and the first-hit shader reads `rayAABB` bounds from
    // `world_meta`, so `GpuRenderParams.bounding_box_*` is left zeroed here
    // and the shader uses `world_meta` instead. Kept in the struct so the
    // uniform layout is stable for Phase A-2 / B.

    // (re)create the `first_hit_data` storage buffer if the pixel count
    // changed. `taa_sample_accum` (Phase A's `shaded_color`) now lives in
    // `TaaGpu` and is (re)sized by `prepare_taa` on the same trigger — they
    // read the same `extracted_camera.viewport_size`, so they stay coherent
    // (`06-design-a2.md` §9.4).
    let (first_hit_data, needs_new_storage) = match &existing {
        Some(frame) if frame.pixel_count == pixel_count => {
            (frame.first_hit_data.clone(), false)
        }
        _ => {
            // first_hit_data: vec4<u32> per pixel (16 bytes).
            let first_hit_data = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_first_hit_data"),
                size: (pixel_count as u64) * 16,
                usage: GROWABLE_BUFFER_USAGES,
                mapped_at_creation: false,
            });
            (first_hit_data, true)
        }
    };

    // The uniform buffers persist across frames; create them once.
    let (camera_buf, render_params_buf) = match &existing {
        Some(frame) => (frame.camera.clone(), frame.render_params.clone()),
        None => {
            let camera_buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_camera"),
                size: std::mem::size_of::<GpuCamera>() as u64,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let render_params_buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_render_params"),
                size: std::mem::size_of::<GpuRenderParams>() as u64,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            (camera_buf, render_params_buf)
        }
    };
    render_queue.write_buffer(&camera_buf, 0, bytemuck::bytes_of(&camera_data));
    render_queue.write_buffer(&render_params_buf, 0, bytemuck::bytes_of(&render_params));

    // Zero `first_hit_data` when freshly (re)created so a clean frame is shown
    // until the first-hit pass fills it, rather than garbage. (`taa_sample_accum`
    // is zero-cleared by `prepare_taa` on its own (re)creation.)
    if needs_new_storage {
        let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("naadf_clear_gbuffer"),
        });
        encoder.clear_buffer(&first_hit_data, 0, None);
        render_queue.submit([encoder.finish()]);
    }

    // Rebuild the bind groups when storage changed; otherwise reuse. Slot 3 of
    // the frame group / slot 1 of the blit group bind `taa_gpu.taa_sample_accum`
    // (the real `taaSampleAccum`, owned by `TaaGpu`) — Phase A bound the local
    // `shaded_color` stand-in here (`06-design-a2.md` §5.1, §5.4). `TaaGpu`'s
    // `taa_sample_accum` / `taa_samples` resize on the same `pixel_count`
    // trigger as `first_hit_data`, so `needs_new_storage` covers all of them.
    // The TAA reproject bind group mixes `TaaGpu` (`taa_params` /
    // `camera_history` / `taa_samples` / `taa_sample_accum`) with `FrameGpu`'s
    // `first_hit_data`, so it is built here too (`06-design-a2.md` §5.5).
    let (bind_group, blit_bind_group, taa_reproject_bind_group) =
        if needs_new_storage || existing.is_none() {
            let bind_group = render_device.create_bind_group(
                "naadf_frame_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipelines.frame_layout),
                &BindGroupEntries::sequential((
                    camera_buf.as_entire_buffer_binding(),
                    render_params_buf.as_entire_buffer_binding(),
                    first_hit_data.as_entire_buffer_binding(),
                    taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
                )),
            );
            let blit_bind_group = render_device.create_bind_group(
                "naadf_blit_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipelines.blit_layout),
                &BindGroupEntries::sequential((
                    first_hit_data.as_entire_buffer_binding(),
                    taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
                    render_params_buf.as_entire_buffer_binding(),
                )),
            );
            let taa_reproject_bind_group = render_device.create_bind_group(
                "naadf_taa_reproject_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipelines.taa_reproject_layout),
                &BindGroupEntries::sequential((
                    taa_gpu.taa_params.as_entire_buffer_binding(),
                    taa_gpu.camera_history.as_entire_buffer_binding(),
                    first_hit_data.as_entire_buffer_binding(),
                    taa_gpu.taa_samples.as_entire_buffer_binding(),
                    taa_gpu.taa_sample_accum.as_entire_buffer_binding(),
                )),
            );
            (bind_group, blit_bind_group, taa_reproject_bind_group)
        } else {
            let frame = existing.as_ref().unwrap();
            (
                frame.bind_group.clone(),
                frame.blit_bind_group.clone(),
                frame.taa_reproject_bind_group.clone(),
            )
        };

    commands.insert_resource(FrameGpu {
        camera: camera_buf,
        render_params: render_params_buf,
        first_hit_data,
        pixel_count,
        bind_group,
        blit_bind_group,
        taa_reproject_bind_group,
    });
}
