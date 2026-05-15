//! W4 — `entity_update.wgsl` Rust side: layouts + pipeline queueing + dispatch
//! helpers (`15-design-c.md` §2.1 W4, §4.6).
//!
//! Pipelines:
//!   - `entity_update_pipeline_update_chunks` — `update_chunks` entry point.
//!     Per-chunk entity-pointer write (`entityUpdate.fx:15-24`).
//!   - `entity_update_pipeline_copy_entity_chunk_instances` —
//!     `copy_entity_chunk_instances` entry point. Bulk upload-buffer copy
//!     (`entityUpdate.fx:26-33`).
//!   - `entity_update_pipeline_copy_entity_history` — `copy_entity_history`
//!     entry point. Per-instance history-ring write
//!     (`entityUpdate.fx:35-42`).
//!
//! Layouts:
//!   - `entity_world_layout` `@group(0)` — `chunks_rw` (Rg32Uint) + the
//!     `EntityUpdateParams` uniform. Distinct from W1's
//!     `construction_world_layout` because the entity passes do **not** need
//!     `blocks` / `voxels` / `block_voxel_count` / `segment_voxel_buffer` /
//!     `hash_map` / `hash_coefficients` — the entity track only touches the
//!     `.y` channel of `chunks`.
//!   - `construction_entity_layout` `@group(1)` — the 5 entity buffers
//!     consumed by the three entry points (`15-design-c.md` §1.3).

use std::borrow::Cow;
use std::num::NonZeroU64;

use bevy::prelude::*;
use bevy::render::render_resource::{
    binding_types::{
        storage_buffer_read_only_sized, storage_buffer_sized, texture_storage_3d,
        uniform_buffer_sized,
    },
    BindGroup, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId,
    CommandEncoder, ComputePassDescriptor, ComputePipelineDescriptor, PipelineCache,
    ShaderStages, StorageTextureAccess, TextureFormat,
};
use bevy::shader::Shader;
use bytemuck::{Pod, Zeroable};

/// Asset path of the W4 `entity_update.wgsl` shader.
pub const ENTITY_UPDATE_SHADER: &str = "shaders/entity_update.wgsl";

/// Inlined source — used by headless unit tests.
pub const ENTITY_UPDATE_SHADER_SRC: &str =
    include_str!("../../assets/shaders/entity_update.wgsl");

/// `EntityUpdateParams` — the W4 uniform mirrored on the GPU
/// (`entity_update.wgsl::EntityUpdateParams`).
///
/// 32 B = 2 × 16-byte rows. Every field is a `u32`; no `vec3`-then-scalar
/// hazard.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, Default)]
pub struct GpuEntityUpdateParams {
    /// `entityUpdate.fx::entityInstanceCount` — count of live entity instances.
    pub entity_instance_count: u32,
    /// `entityUpdate.fx::entityChunkInstanceCount` — count of distinct
    /// (chunk × entity) instances this frame.
    pub entity_chunk_instance_count: u32,
    /// `entityUpdate.fx::taaIndex` — current TAA ring slot.
    pub taa_index: u32,
    /// `entityUpdate.fx::updateCount` — count of `chunkUpdatesDynamic` entries.
    pub update_count: u32,
    /// `WorldRender.cs:88` — per-frame entity-instance cap stride.
    pub max_entity_instances: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
}

const _: () = assert!(std::mem::size_of::<GpuEntityUpdateParams>() == 32);

/// Build the `entity_world_layout` `@group(0)` for the three W4 entry points.
///
/// 2 bindings:
///   0: chunks_rw — `texture_storage_3d<rg32uint, read_write>`
///   1: params — uniform<EntityUpdateParams>
pub fn entity_world_layout_descriptor() -> BindGroupLayoutDescriptor {
    let params_size =
        NonZeroU64::new(std::mem::size_of::<GpuEntityUpdateParams>() as u64).unwrap();
    BindGroupLayoutDescriptor::new(
        "naadf_entity_world_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_storage_3d(
                    TextureFormat::Rg32Uint,
                    StorageTextureAccess::ReadWrite,
                ),
                uniform_buffer_sized(false, Some(params_size)),
            ),
        ),
    )
}

/// Build the `construction_entity_layout` `@group(1)` per `15-design-c.md`
/// §1.3.
///
/// 5 bindings (3 read-only upload buffers + 2 read-write GPU buffers):
///   0: chunk_updates_dynamic           — ro `array<vec2<u32>>`
///   1: entity_chunk_instances_dynamic  — ro `array<EntityChunkInstance>`
///   2: entity_history_dynamic          — ro `array<vec4<u32>>`
///   3: entity_chunk_instances_rw       — rw `array<EntityChunkInstance>`
///   4: entity_instances_history_rw     — rw `array<vec4<u32>>`
pub fn construction_entity_layout_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "naadf_construction_entity_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_sized(false, None),
                storage_buffer_sized(false, None),
            ),
        ),
    )
}

/// Queue the `update_chunks` pipeline.
pub fn queue_update_chunks_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    entity_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(ENTITY_UPDATE_SHADER);
    queue_update_chunks_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        entity_layout,
        shader,
    )
}

pub fn queue_update_chunks_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    entity_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_entity_update_update_chunks_pipeline".into()),
        layout: vec![world_layout, entity_layout],
        shader,
        entry_point: Some(Cow::from("update_chunks")),
        ..default()
    })
}

/// Queue the `copy_entity_chunk_instances` pipeline.
pub fn queue_copy_entity_chunk_instances_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    entity_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(ENTITY_UPDATE_SHADER);
    queue_copy_entity_chunk_instances_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        entity_layout,
        shader,
    )
}

pub fn queue_copy_entity_chunk_instances_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    entity_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_entity_update_copy_entity_chunk_instances_pipeline".into()),
        layout: vec![world_layout, entity_layout],
        shader,
        entry_point: Some(Cow::from("copy_entity_chunk_instances")),
        ..default()
    })
}

/// Queue the `copy_entity_history` pipeline.
pub fn queue_copy_entity_history_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    entity_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(ENTITY_UPDATE_SHADER);
    queue_copy_entity_history_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        entity_layout,
        shader,
    )
}

pub fn queue_copy_entity_history_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    entity_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_entity_update_copy_entity_history_pipeline".into()),
        layout: vec![world_layout, entity_layout],
        shader,
        entry_point: Some(Cow::from("copy_entity_history")),
        ..default()
    })
}

/// Dispatch `update_chunks` over `update_count / 64` workgroups
/// (`entityUpdate.fx:399` — `(updateCount + 63) / 64`).
pub fn dispatch_update_chunks(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bg: &BindGroup,
    entity_bg: &BindGroup,
    update_count: u32,
) {
    if update_count == 0 {
        return;
    }
    let workgroups = (update_count + 63) / 64;
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_entity_update_chunks_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, world_bg, &[]);
    pass.set_bind_group(1, entity_bg, &[]);
    pass.dispatch_workgroups(workgroups, 1, 1);
}

/// Dispatch `copy_entity_chunk_instances` over `(count + 63) / 64` workgroups.
pub fn dispatch_copy_entity_chunk_instances(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bg: &BindGroup,
    entity_bg: &BindGroup,
    entity_chunk_instance_count: u32,
) {
    if entity_chunk_instance_count == 0 {
        return;
    }
    let workgroups = (entity_chunk_instance_count + 63) / 64;
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_entity_update_copy_chunk_instances_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, world_bg, &[]);
    pass.set_bind_group(1, entity_bg, &[]);
    pass.dispatch_workgroups(workgroups, 1, 1);
}

/// Dispatch `copy_entity_history` over `(count + 63) / 64` workgroups.
pub fn dispatch_copy_entity_history(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bg: &BindGroup,
    entity_bg: &BindGroup,
    entity_instance_count: u32,
) {
    if entity_instance_count == 0 {
        return;
    }
    let workgroups = (entity_instance_count + 63) / 64;
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_entity_update_copy_history_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, world_bg, &[]);
    pass.set_bind_group(1, entity_bg, &[]);
    pass.dispatch_workgroups(workgroups, 1, 1);
}

/// `Core3d` regime-3 node — W4 entity-update dispatch (Phase-C wave-3).
///
/// Gated on:
///   1. `ConstructionConfig.entities_enabled = true`.
///   2. `ConstructionEvents.has_entity_updates() = true` (regime-3 fast-path —
///      no-op on no-entity-update frames in microseconds).
///
/// Per-frame body: dispatches the 3 `entity_update.wgsl` entry points in
/// order (`entityUpdate.fx:15-42`):
///   1. `update_chunks` — writes the chunks-texture `.y` channel with the
///      per-chunk entity pointer + counter pairs (mirrors HLSL
///      `entityUpdate.fx:15-24`).
///   2. `copy_entity_chunk_instances` — bulk copies the per-frame upload
///      buffer into the GPU production buffer the renderer reads (mirrors
///      `entityUpdate.fx:26-33`).
///   3. `copy_entity_history` — writes the current TAA-ring slot of the
///      entity-instance history buffer (mirrors `entityUpdate.fx:35-42`).
///
/// The world bind group built here is a small inline `(chunks_rw, params)`
/// `@group(0)` bind group; the entity-track buffers live on `@group(1)` =
/// `ConstructionBindGroups::construction_entity` (built by `prepare_construction`).
pub fn naadf_entity_update_node(
    mut render_context: bevy::render::renderer::RenderContext,
    pipeline_cache: Res<bevy::render::render_resource::PipelineCache>,
    construction_pipelines: Option<Res<crate::render::construction::ConstructionPipelines>>,
    construction_bind_groups: Option<Res<crate::render::construction::ConstructionBindGroups>>,
    construction_gpu: Option<Res<crate::render::construction::ConstructionGpu>>,
    construction_events: Option<Res<crate::render::construction::ConstructionEvents>>,
    construction_config: Option<Res<crate::render::construction::ConstructionConfig>>,
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
    render_device: Res<bevy::render::renderer::RenderDevice>,
) {
    let Some(config) = construction_config else { return; };
    if !config.entities_enabled {
        return;
    }
    let Some(events) = construction_events else { return; };
    if !events.has_entity_updates() {
        // Regime-3 fast-path: no-op on no-entity-update frames.
        return;
    }
    let Some(construction_pipelines) = construction_pipelines else { return; };
    let Some(construction_bind_groups) = construction_bind_groups else { return; };
    let Some(construction_gpu) = construction_gpu else { return; };
    let Some(world_gpu) = world_gpu else { return; };

    let Some(entity_bg) = construction_bind_groups.construction_entity.as_ref() else {
        return;
    };
    let Some(params_buf) = construction_gpu.entity_update_params_buffer.as_ref() else {
        return;
    };

    // Resolve the 3 pipelines.
    let (Some(p_update), Some(p_copy_ci), Some(p_copy_hist)) = (
        pipeline_cache.get_compute_pipeline(construction_pipelines.entity_update_pipeline_update_chunks),
        pipeline_cache.get_compute_pipeline(construction_pipelines.entity_update_pipeline_copy_entity_chunk_instances),
        pipeline_cache.get_compute_pipeline(construction_pipelines.entity_update_pipeline_copy_entity_history),
    ) else {
        return;
    };

    // Build the entity_world bind group inline (cheap; the bind group is
    // not stashed because it depends on the per-frame params buffer).
    let entity_world_bgl = pipeline_cache
        .get_bind_group_layout(&construction_pipelines.entity_world_layout);
    let entity_world_bg = render_device.create_bind_group(
        "naadf_entity_update_world_bind_group",
        &entity_world_bgl,
        &bevy::render::render_resource::BindGroupEntries::sequential((
            &world_gpu.chunks_view,
            params_buf.as_entire_buffer_binding(),
        )),
    );

    let encoder = render_context.command_encoder();

    let update_count = events.entity_uploads.chunk_updates.len() as u32;
    let entity_chunk_instance_count =
        events.entity_uploads.entity_chunk_instances.len() as u32;
    let entity_instance_count = events.entity_uploads.entity_history.len() as u32;

    // Trace-level diagnostic — uncomment when verifying wave-3 plumbing:
    //   info!("entity dispatch: {} updates, {} ci, {} hist", update_count,
    //         entity_chunk_instance_count, entity_instance_count);

    dispatch_update_chunks(encoder, p_update, &entity_world_bg, entity_bg, update_count);
    dispatch_copy_entity_chunk_instances(
        encoder,
        p_copy_ci,
        &entity_world_bg,
        entity_bg,
        entity_chunk_instance_count,
    );
    dispatch_copy_entity_history(
        encoder,
        p_copy_hist,
        &entity_world_bg,
        entity_bg,
        entity_instance_count,
    );
}
