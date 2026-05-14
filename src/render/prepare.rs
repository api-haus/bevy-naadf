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
//!   `GpuRenderParams` uniforms, (re)create the `first_hit_data` + `shaded_color`
//!   storage buffers on a viewport resize, and build `bind_group_frame`.
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

use crate::render::extract::{ExtractedCameraData, ExtractedWorld};
use crate::render::gpu_types::{GpuCamera, GpuRenderParams, GpuVoxelType, GpuWorldMeta, FLAG_CHECK_SUN};
use crate::render::pipelines::NaadfPipelines;
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
    /// The blit-source stand-in — one `vec2<u32>` per pixel, the
    /// `taaSampleAccum` element format (`03-design.md` §5.3). Phase A keeps
    /// this in place of the real TAA accumulation buffer (D4).
    pub shaded_color: Buffer,
    /// Pixel count the storage buffers are currently sized for.
    pub pixel_count: u32,
    /// `@group(1)` bind group for the first-hit compute pass.
    pub bind_group: BindGroup,
    /// The final-blit pass's own bind group (`first_hit_data`, `shaded_color`,
    /// `render_params`).
    pub blit_bind_group: BindGroup,
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
/// render-params uniforms, (re)create the G-buffer storage buffers on a
/// viewport resize, and build the frame bind groups.
///
/// Runs in `PrepareBindGroups` (after `PrepareResources`) so the world bind
/// group / pipelines are already created. Skips silently until both the
/// camera has been extracted and `WorldGpu` exists.
pub fn prepare_frame_gpu(
    mut commands: Commands,
    extracted_camera: Res<ExtractedCameraData>,
    existing: Option<ResMut<FrameGpu>>,
    pipelines: Res<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    time: Res<Time>,
) {
    if !extracted_camera.valid {
        return;
    }
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
        frame_count: time.elapsed().as_millis() as u32,
        rand_counter: (time.elapsed_secs_f64() * 1000.0) as u32,
        taa_index: 0,
        // Phase A: no TAA (D4), no ray-step debug view; trace the sun shadow.
        flags: FLAG_CHECK_SUN,
        exposure: 1.5,
        _pad0: 0,
        sky_sun_dir,
        _pad1: 0,
        sun_color: Vec3::new(1.0, 0.95, 0.85),
        _pad2: 0,
        taa_jitter: Vec2::ZERO,
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

    // (re)create the storage buffers if the pixel count changed.
    let (first_hit_data, shaded_color, needs_new_storage) = match &existing {
        Some(frame) if frame.pixel_count == pixel_count => (
            frame.first_hit_data.clone(),
            frame.shaded_color.clone(),
            false,
        ),
        _ => {
            // first_hit_data: vec4<u32> per pixel (16 bytes).
            let first_hit_data = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_first_hit_data"),
                size: (pixel_count as u64) * 16,
                usage: GROWABLE_BUFFER_USAGES,
                mapped_at_creation: false,
            });
            // shaded_color: vec2<u32> per pixel (8 bytes).
            let shaded_color = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_shaded_color"),
                size: (pixel_count as u64) * 8,
                usage: GROWABLE_BUFFER_USAGES,
                mapped_at_creation: false,
            });
            (first_hit_data, shaded_color, true)
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

    // Zero the storage buffers when freshly (re)created so a black frame is
    // shown until the first-hit pass fills them, rather than garbage.
    if needs_new_storage {
        let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("naadf_clear_gbuffer"),
        });
        encoder.clear_buffer(&first_hit_data, 0, None);
        encoder.clear_buffer(&shaded_color, 0, None);
        render_queue.submit([encoder.finish()]);
    }

    // Rebuild the bind groups when storage changed; otherwise reuse.
    let (bind_group, blit_bind_group) = if needs_new_storage || existing.is_none() {
        let bind_group = render_device.create_bind_group(
            "naadf_frame_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.frame_layout),
            &BindGroupEntries::sequential((
                camera_buf.as_entire_buffer_binding(),
                render_params_buf.as_entire_buffer_binding(),
                first_hit_data.as_entire_buffer_binding(),
                shaded_color.as_entire_buffer_binding(),
            )),
        );
        let blit_bind_group = render_device.create_bind_group(
            "naadf_blit_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.blit_layout),
            &BindGroupEntries::sequential((
                first_hit_data.as_entire_buffer_binding(),
                shaded_color.as_entire_buffer_binding(),
                render_params_buf.as_entire_buffer_binding(),
            )),
        );
        (bind_group, blit_bind_group)
    } else {
        let frame = existing.as_ref().unwrap();
        (frame.bind_group.clone(), frame.blit_bind_group.clone())
    };

    commands.insert_resource(FrameGpu {
        camera: camera_buf,
        render_params: render_params_buf,
        first_hit_data,
        shaded_color,
        pixel_count,
        bind_group,
        blit_bind_group,
    });
}
