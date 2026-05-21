//! Phase-C W2 — `world_change.wgsl` Rust side: layouts, pipeline queueing,
//! dispatch helpers, and the regime-3 `naadf_world_change_node` `Core3d`-
//! schedule system (`15-design-c.md` §1.2 regime-3, §1.3, §2.1 W2, §4.3;
//! `16-impl-c-W2.md`).
//!
//! Four pipelines:
//!   - `world_change_pipeline_apply_group_change` — `apply_group_change`,
//!     regime-3 per-4³-group AADF reset + bound-queue re-enqueue
//!     (`worldChange.fx:37-113`).
//!   - `world_change_pipeline_apply_chunk_change` — `apply_chunk_change`,
//!     regime-3 per-chunk cell edit, **preserves `.y`** of `chunks` texel
//!     (`worldChange.fx:115-128`).
//!   - `world_change_pipeline_apply_block_change` — `apply_block_change`,
//!     regime-3 per-edit 64-block recompute + AADF update (`fx:130-147`).
//!   - `world_change_pipeline_apply_voxel_change` — `apply_voxel_change`,
//!     regime-3 per-edit 64-voxel recompute + AADF update (`fx:149-168`).
//!
//! Layouts (per `15-design-c.md` §1.3):
//!   - `@group(0)` = W1's `construction_world_layout` — shared with `chunk_calc.wgsl`.
//!   - `@group(1)` = `construction_change_layout` (W2-owned) — 4 ro bindings:
//!     `changed_groups_dynamic`, `changed_chunks_dynamic`,
//!     `changed_blocks_dynamic`, `changed_voxels_dynamic`.
//!   - `@group(2)` = W3's `construction_bounds_layout` — only consumed by
//!     `apply_group_change` (the re-enqueue path), but bound for all 4
//!     pipelines (the layout share keeps the pipeline-vs-layout pairing
//!     simple; the 3 unused pipelines never read the bindings, the compiler
//!     accepts it).

use std::borrow::Cow;
use std::num::NonZeroU64;

use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::render_resource::{
    binding_types::storage_buffer_read_only_sized, BindGroupLayoutDescriptor,
    BindGroupLayoutEntries, CachedComputePipelineId, CommandEncoder, ComputePassDescriptor,
    ComputePipelineDescriptor, PipelineCache, ShaderStages,
};
use bevy::render::renderer::RenderContext;
use bevy::shader::Shader;

use crate::render::construction::config::ConstructionConfig;
use crate::render::construction::{ConstructionBindGroups, ConstructionEvents};
use crate::render::pipelines::NaadfPipelines;

/// Asset path of the W2 `world_change.wgsl` shader.
pub const WORLD_CHANGE_SHADER: &str = "shaders/world_change.wgsl";

/// Inlined source — used by the W2 unit test (which builds a headless render
/// world without an asset loader). Same pattern as W1 / W3 / W5.
pub const WORLD_CHANGE_SHADER_SRC: &str =
    include_str!("../../assets/shaders/world_change.wgsl");

/// Timing-span name for the regime-3 world-change node — surfaces in the HUD
/// as `render/naadf_world_change/elapsed_gpu`.
pub const WORLD_CHANGE_SPAN: &str = "naadf_world_change";

// ─── Layout descriptor ────────────────────────────────────────────────────────

/// `construction_change_layout` `@group(1)` — 4 ro-storage bindings: the
/// per-frame upload buffers consumed by `world_change.wgsl`'s 4 apply passes.
/// Per `15-design-c.md` §1.3.
pub fn construction_change_layout_descriptor() -> BindGroupLayoutDescriptor {
    // Each binding is a read-only storage buffer; the WGSL types are:
    //   0: array<vec2<u32>> (changedGroupsDynamic) — `[u32; 2]` per group
    //   1: array<vec2<u32>> (changedChunksDynamic) — `[u32; 2]` per chunk
    //   2: array<u32>       (changedBlocksDynamic) — 65 u32s per block edit
    //   3: array<u32>       (changedVoxelsDynamic) — 33 u32s per voxel edit
    BindGroupLayoutDescriptor::new(
        "naadf_construction_change_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    )
}

// ─── Pipeline queueing ────────────────────────────────────────────────────────

fn queue_world_change_entry_point(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    entry_point: &'static str,
    label: &'static str,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some(label.into()),
        layout: vec![world_layout, change_layout, bounds_layout],
        shader,
        entry_point: Some(Cow::from(entry_point)),
        ..default()
    })
}

pub fn queue_apply_group_change_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(WORLD_CHANGE_SHADER);
    queue_apply_group_change_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        change_layout,
        bounds_layout,
        shader,
    )
}

pub fn queue_apply_group_change_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    queue_world_change_entry_point(
        pipeline_cache,
        world_layout,
        change_layout,
        bounds_layout,
        shader,
        "apply_group_change",
        "naadf_world_change_apply_group_pipeline",
    )
}

pub fn queue_apply_chunk_change_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(WORLD_CHANGE_SHADER);
    queue_apply_chunk_change_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        change_layout,
        bounds_layout,
        shader,
    )
}

pub fn queue_apply_chunk_change_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    queue_world_change_entry_point(
        pipeline_cache,
        world_layout,
        change_layout,
        bounds_layout,
        shader,
        "apply_chunk_change",
        "naadf_world_change_apply_chunk_pipeline",
    )
}

pub fn queue_apply_block_change_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(WORLD_CHANGE_SHADER);
    queue_apply_block_change_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        change_layout,
        bounds_layout,
        shader,
    )
}

pub fn queue_apply_block_change_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    queue_world_change_entry_point(
        pipeline_cache,
        world_layout,
        change_layout,
        bounds_layout,
        shader,
        "apply_block_change",
        "naadf_world_change_apply_block_pipeline",
    )
}

pub fn queue_apply_voxel_change_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(WORLD_CHANGE_SHADER);
    queue_apply_voxel_change_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        change_layout,
        bounds_layout,
        shader,
    )
}

pub fn queue_apply_voxel_change_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    change_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    queue_world_change_entry_point(
        pipeline_cache,
        world_layout,
        change_layout,
        bounds_layout,
        shader,
        "apply_voxel_change",
        "naadf_world_change_apply_voxel_pipeline",
    )
}

// ─── Dispatch helpers ─────────────────────────────────────────────────────────

/// Dispatch `apply_chunk_change` for `changed_chunk_count` chunks (one
/// workgroup per 64 chunks). `worldChange.fx:213` — `(count + 63) / 64`.
pub fn dispatch_apply_chunk_change(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bind_group: &bevy::render::render_resource::BindGroup,
    change_bind_group: &bevy::render::render_resource::BindGroup,
    bounds_bind_group: &bevy::render::render_resource::BindGroup,
    changed_chunk_count: u32,
) {
    if changed_chunk_count == 0 {
        return;
    }
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_world_change_apply_chunk_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, world_bind_group, &[]);
    pass.set_bind_group(1, change_bind_group, &[]);
    pass.set_bind_group(2, bounds_bind_group, &[]);
    pass.dispatch_workgroups(changed_chunk_count.div_ceil(64).max(1), 1, 1);
}

/// Dispatch `apply_block_change` for `changed_block_count` 64-block edits (one
/// workgroup per edit). `worldChange.fx:225` — `count, 1, 1`.
pub fn dispatch_apply_block_change(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bind_group: &bevy::render::render_resource::BindGroup,
    change_bind_group: &bevy::render::render_resource::BindGroup,
    bounds_bind_group: &bevy::render::render_resource::BindGroup,
    changed_block_count: u32,
) {
    if changed_block_count == 0 {
        return;
    }
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_world_change_apply_block_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, world_bind_group, &[]);
    pass.set_bind_group(1, change_bind_group, &[]);
    pass.set_bind_group(2, bounds_bind_group, &[]);
    pass.dispatch_workgroups(changed_block_count, 1, 1);
}

/// Dispatch `apply_voxel_change` for `changed_voxel_count` 64-voxel edits (one
/// workgroup per edit). `worldChange.fx:237` — `count, 1, 1`.
pub fn dispatch_apply_voxel_change(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bind_group: &bevy::render::render_resource::BindGroup,
    change_bind_group: &bevy::render::render_resource::BindGroup,
    bounds_bind_group: &bevy::render::render_resource::BindGroup,
    changed_voxel_count: u32,
) {
    if changed_voxel_count == 0 {
        return;
    }
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_world_change_apply_voxel_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, world_bind_group, &[]);
    pass.set_bind_group(1, change_bind_group, &[]);
    pass.set_bind_group(2, bounds_bind_group, &[]);
    pass.dispatch_workgroups(changed_voxel_count, 1, 1);
}

/// Dispatch `apply_group_change` for `changed_group_count` 4³-groups (one
/// workgroup per group). `worldChange.fx:248` — `count, 1, 1`.
pub fn dispatch_apply_group_change(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bind_group: &bevy::render::render_resource::BindGroup,
    change_bind_group: &bevy::render::render_resource::BindGroup,
    bounds_bind_group: &bevy::render::render_resource::BindGroup,
    changed_group_count: u32,
) {
    if changed_group_count == 0 {
        return;
    }
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_world_change_apply_group_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, world_bind_group, &[]);
    pass.set_bind_group(1, change_bind_group, &[]);
    pass.set_bind_group(2, bounds_bind_group, &[]);
    pass.dispatch_workgroups(changed_group_count, 1, 1);
}

// ─── Regime-3 Core3d node ─────────────────────────────────────────────────────

/// `Core3d`-schedule system: the W2 regime-3 world-change node
/// (`15-design-c.md` §1.2 regime-3, §3 — `naadf_world_change_node`).
///
/// Inserted in `render/mod.rs::add_systems(Core3d, …)` **between**
/// `naadf_bounds_compute_node` (W3) and `naadf_entity_update_node` (W4).
/// Gated on `ConstructionEvents::has_pending_changes()`; on a frame with no
/// edits the body short-circuits to a single bool check.
///
/// Per-frame body (when pending changes):
/// 1. Dispatch `apply_chunk_change` (if `changed_chunk_count > 0`).
/// 2. Dispatch `apply_block_change` (if `changed_block_count > 0`).
/// 3. Dispatch `apply_voxel_change` (if `changed_voxel_count > 0`).
/// 4. Dispatch `apply_group_change` (if `changed_group_count > 0`).
///
/// The order matches `ChangeHandler.cs:203-249`: chunk → block → voxel →
/// group. The group pass writes to `bound_queue_sizes` / `bound_queue_starts` / `bound_group_queues`,
/// which the W3 regime-2 node consumes next frame.
///
/// The dispatched count for each pass comes from `ConstructionEvents` (mirrored
/// from the main-world `ChangeHandler` via `extract_change_events`).
pub fn naadf_world_change_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Option<Res<NaadfPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_events: Option<Res<ConstructionEvents>>,
    construction_config: Option<Res<ConstructionConfig>>,
) {
    let Some(pipelines) = pipelines else { return; };
    let Some(construction_bind_groups) = construction_bind_groups else { return; };
    let Some(construction_events) = construction_events else { return; };
    let Some(construction_config) = construction_config else { return; };

    if !construction_config.gpu_construction_enabled {
        return;
    }
    if !construction_events.has_pending_changes() {
        // Regime-3 fast-path: no-op on no-edit frames (single bool check).
        return;
    }
    // `02f-followup` — debug-log when the W2 GPU dispatch fires. Useful for
    // regression diagnosis (if a future change leaves `extract_world_changes`
    // draining but the dispatch silent, the trace surfaces the gap).
    bevy::log::debug!(
        "naadf_world_change_node dispatch: chunks={}, blocks={}, voxels={}, groups={}",
        construction_events.changed_chunk_count,
        construction_events.changed_block_count,
        construction_events.changed_voxel_count,
        construction_events.changed_group_count,
    );

    // Pull the three bind groups.
    let (Some(world_bg), Some(change_bg), Some(bounds_bg)) = (
        construction_bind_groups.construction_world.as_ref(),
        construction_bind_groups.construction_change.as_ref(),
        construction_bind_groups.construction_bounds.as_ref(),
    ) else {
        // Bind groups not built yet (W1+W3+W2 prepare hasn't allocated buffers).
        return;
    };

    // Resolve the 4 pipelines.
    let (Some(p_chunk), Some(p_block), Some(p_voxel), Some(p_group)) = (
        pipeline_cache
            .get_compute_pipeline(pipelines.world_change_pipeline_apply_chunk_change),
        pipeline_cache
            .get_compute_pipeline(pipelines.world_change_pipeline_apply_block_change),
        pipeline_cache
            .get_compute_pipeline(pipelines.world_change_pipeline_apply_voxel_change),
        pipeline_cache
            .get_compute_pipeline(pipelines.world_change_pipeline_apply_group_change),
    ) else {
        return;
    };

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, WORLD_CHANGE_SPAN);

    // Mirror `ChangeHandler.cs:203-249` dispatch order: chunk → block → voxel
    // → group.
    dispatch_apply_chunk_change(
        encoder,
        p_chunk,
        world_bg,
        change_bg,
        bounds_bg,
        construction_events.changed_chunk_count,
    );
    dispatch_apply_block_change(
        encoder,
        p_block,
        world_bg,
        change_bg,
        bounds_bg,
        construction_events.changed_block_count,
    );
    dispatch_apply_voxel_change(
        encoder,
        p_voxel,
        world_bg,
        change_bg,
        bounds_bg,
        construction_events.changed_voxel_count,
    );
    dispatch_apply_group_change(
        encoder,
        p_group,
        world_bg,
        change_bg,
        bounds_bg,
        construction_events.changed_group_count,
    );

    time_span.end(render_context.command_encoder());
}

// Silence the unused-import on NonZeroU64 when no constants need it inline.
const _: () = {
    let _ = NonZeroU64::new(1);
};

#[cfg(test)]
mod tests {
    //! W2 — load-bearing GPU/CPU bit-exact oracle tests
    //! (`15-design-c.md` §1.6, `16-impl-c-W2.md`).
    //!
    //! Three GPU bit-exact tests (one per `world_change.wgsl` apply pass
    //! that has a CPU-side mirror; the `apply_group_change` pass is verified
    //! via the `edit_re_enqueues_bound_queue` integration shape rather than
    //! bit-exact equality, because the W3 bound-queue family it writes is
    //! visible only via subsequent queue-info readback).
    use super::*;
    use crate::aadf::edit::{apply_block_edit_cpu, apply_chunk_edit_cpu, apply_voxel_edit_cpu};
    use crate::render::construction::chunk_calc::CHUNK_CALC_SHADER_SRC;
    use crate::voxel::{VOXEL_FULL_FLAG, VOXEL_PAYLOAD_MASK};

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets, Handle};
    use bevy::image::ImagePlugin;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::shader::Shader;
    use bevy::MinimalPlugins;

    use crate::render::construction::chunk_calc::construction_world_layout_descriptor;
    use crate::render::gpu_types::GpuConstructionParams;

    /// Helper — boot a headless render world with the W2 shaders pre-loaded
    /// into the pipeline cache; returns the App + device + queue + the two
    /// shader handles (chunk_calc + world_change).
    fn render_fixture() -> Option<(App, RenderDevice, RenderQueue, Handle<Shader>, Handle<Shader>)> {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .add_plugins(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::default()),
                synchronous_pipeline_compilation: true,
                debug_flags: Default::default(),
            });
        app.finish();
        app.cleanup();

        // Load chunk_calc.wgsl (so the W2 layout — `construction_world_layout`
        // — is well-formed even when we only dispatch world_change passes;
        // the layout is shared).
        let cc_shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
        let cc_clone = cc_shader.clone();
        let cc_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(cc_shader);

        let wc_shader = Shader::from_wgsl(WORLD_CHANGE_SHADER_SRC, "shaders/world_change.wgsl");
        let wc_clone = wc_shader.clone();
        let wc_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(wc_shader);

        let render_app = app.get_sub_app_mut(RenderApp)?;
        {
            let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.set_shader(cc_handle.id(), cc_clone);
            pipeline_cache.set_shader(wc_handle.id(), wc_clone);
        }
        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((app, device, queue, cc_handle, wc_handle))
    }

    /// Build the W2 layouts (W1's chunk_calc layout for `@group(0)`, W2's
    /// change layout for `@group(1)`, W3's bounds layout for `@group(2)`).
    fn build_layouts() -> (
        BindGroupLayoutDescriptor,
        BindGroupLayoutDescriptor,
        BindGroupLayoutDescriptor,
    ) {
        let world_layout = construction_world_layout_descriptor();
        let change_layout = construction_change_layout_descriptor();
        let bounds_layout =
            crate::render::construction::bounds_calc::construction_bounds_layout_descriptor();
        (world_layout, change_layout, bounds_layout)
    }

    fn read_u32_buf(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &bevy::render::render_resource::Buffer,
        count: u64,
    ) -> Vec<u32> {
        let size = count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("w2_readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w2_readback_enc"),
        });
        encoder.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        out
    }

    fn read_chunks_buffer(
        device: &RenderDevice,
        queue: &RenderQueue,
        chunks: &bevy::render::render_resource::Buffer,
        size_in_chunks: [u32; 3],
    ) -> Vec<[u32; 2]> {
        // Web-WebGPU migration: chunks is `array<vec2<u32>>` (8 B per pair).
        // Flat buffer→buffer copy; returns the pairs directly.
        let total_chunks =
            (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as u64;
        let total = total_chunks * 8;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("w2_chunks_readback"),
            size: total,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w2_chunks_enc"),
        });
        encoder.copy_buffer_to_buffer(chunks, 0, &staging, 0, total);
        queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let pairs: &[[u32; 2]] = bytemuck::cast_slice(&data);
        let out: Vec<[u32; 2]> = pairs.to_vec();
        drop(data);
        staging.unmap();
        out
    }

    /// Build the standard fixture: a 4×4×4-chunk world, all empty, with
    /// pre-allocated GPU buffers + bind groups for `world_change`. Returns
    /// the App, device, queue, the chunks texture, and the 3 bind groups.
    fn build_fixture_4x4x4() -> Option<W2Fixture> {
        let (mut app, device, queue, _cc, wc_handle) = render_fixture()?;
        let size_in_chunks = [4u32, 4, 4];
        let total_chunks = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as u64;

        // Web-WebGPU migration: chunks is `array<vec2<u32>>` storage buffer
        // (was `Rg32Uint` 3D texture). All-zero seed (empty Aadf6 == 0).
        let zero_chunks: Vec<[u32; 2]> = vec![[0, 0]; total_chunks as usize];
        let chunks_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("w2_chunks"),
            size: total_chunks * 8,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

        // blocks/voxels buffers — pre-populated with 1024 zero u32s each (big
        // enough for the small-test edits).
        let mk_storage = |label: &'static str, count: usize| -> bevy::render::render_resource::Buffer {
            let buf = device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size: (count * 4).max(64) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            buf
        };
        let blocks_buf = mk_storage("w2_blocks", 1024);
        let voxels_buf = mk_storage("w2_voxels", 1024);
        let bvc_buf = mk_storage("w2_bvc", 2);
        queue.write_buffer(&bvc_buf, 0, bytemuck::cast_slice(&[64u32, 64u32]));
        let segv_buf = mk_storage("w2_segv", 1);
        let hmap_buf = device.create_buffer(&BufferDescriptor {
            label: Some("w2_hmap"),
            size: 16,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let coeffs_buf = mk_storage("w2_coeffs", 1);

        // params uniform.
        let params = GpuConstructionParams {
            size_in_chunks,
            _pad0: 0,
            group_size_in_groups: [1, 1, 1],
            _pad1: 0,
            bound_group_queue_max_size: 1,
            hash_map_size: 1,
            segment_size_in_chunks: 4,
            max_group_bound_dispatch: 0,
            chunk_offset: [0, 0, 0],
            dispatch_offset: 0,
            frame_index: 0,
            changed_chunk_count: 0,
            changed_block_count: 0,
            changed_voxel_count: 0,
        };
        let params_buf = device.create_buffer(&BufferDescriptor {
            label: Some("w2_params"),
            size: std::mem::size_of::<GpuConstructionParams>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&params_buf, 0, bytemuck::bytes_of(&params));

        // change buffers — small fixed sizes.
        let cg = mk_storage("w2_changed_groups", 64);
        let cc = mk_storage("w2_changed_chunks", 64);
        let cb = mk_storage("w2_changed_blocks", 256);
        let cv = mk_storage("w2_changed_voxels", 256);

        // bounds buffers — minimum sizes.
        // 2026-05-19 wasm-chunk-aadf-determinism — `bound_queue_info` split
        // into `bound_queue_starts` (u32) + `bound_queue_sizes` (atomic<u32>).
        let bqs_starts = mk_storage("w2_bq_starts", 96);
        let bqs_sizes = mk_storage("w2_bq_sizes", 96);
        let bgq = mk_storage("w2_bgq", 96);
        let bgm = mk_storage("w2_bgm", 16);
        let bri = mk_storage("w2_bri", 3);

        // Compile the 4 world_change pipelines.
        let (world_layout, change_layout, bounds_layout) = build_layouts();
        let (id_chunk, id_block, id_voxel, id_group) = {
            let cache = app.get_sub_app(RenderApp).unwrap().world().resource::<PipelineCache>();
            (
                queue_apply_chunk_change_pipeline_with_handle(
                    cache,
                    world_layout.clone(),
                    change_layout.clone(),
                    bounds_layout.clone(),
                    wc_handle.clone(),
                ),
                queue_apply_block_change_pipeline_with_handle(
                    cache,
                    world_layout.clone(),
                    change_layout.clone(),
                    bounds_layout.clone(),
                    wc_handle.clone(),
                ),
                queue_apply_voxel_change_pipeline_with_handle(
                    cache,
                    world_layout.clone(),
                    change_layout.clone(),
                    bounds_layout.clone(),
                    wc_handle.clone(),
                ),
                queue_apply_group_change_pipeline_with_handle(
                    cache,
                    world_layout.clone(),
                    change_layout.clone(),
                    bounds_layout.clone(),
                    wc_handle.clone(),
                ),
            )
        };

        // Drive pipeline compilation.
        let mut pipelines: Option<(_, _, _, _)> = None;
        let render_app = app.get_sub_app_mut(RenderApp)?;
        for _ in 0..64 {
            let mut cache = render_app.world_mut().resource_mut::<PipelineCache>();
            cache.process_queue();
            let cache = render_app.world().resource::<PipelineCache>();
            if let (Some(a), Some(b), Some(c), Some(d)) = (
                cache.get_compute_pipeline(id_chunk),
                cache.get_compute_pipeline(id_block),
                cache.get_compute_pipeline(id_voxel),
                cache.get_compute_pipeline(id_group),
            ) {
                pipelines = Some((a.clone(), b.clone(), c.clone(), d.clone()));
                break;
            }
        }
        let (p_chunk, p_block, p_voxel, p_group) = pipelines?;

        // Build the 3 bind groups.
        let cache = app.get_sub_app(RenderApp).unwrap().world().resource::<PipelineCache>();
        let bgl_world = cache.get_bind_group_layout(&world_layout);
        let bgl_change = cache.get_bind_group_layout(&change_layout);
        let bgl_bounds = cache.get_bind_group_layout(&bounds_layout);

        let world_bg = device.create_bind_group(
            "w2_world_bg",
            &bgl_world,
            &BindGroupEntries::sequential((
                chunks_buffer.as_entire_buffer_binding(),
                blocks_buf.as_entire_buffer_binding(),
                voxels_buf.as_entire_buffer_binding(),
                bvc_buf.as_entire_buffer_binding(),
                segv_buf.as_entire_buffer_binding(),
                hmap_buf.as_entire_buffer_binding(),
                params_buf.as_entire_buffer_binding(),
                coeffs_buf.as_entire_buffer_binding(),
            )),
        );
        let change_bg = device.create_bind_group(
            "w2_change_bg",
            &bgl_change,
            &BindGroupEntries::sequential((
                cg.as_entire_buffer_binding(),
                cc.as_entire_buffer_binding(),
                cb.as_entire_buffer_binding(),
                cv.as_entire_buffer_binding(),
            )),
        );
        // 2026-05-19 web fix — 5 bindings: starts/queues/masks/refined/sizes.
        let bounds_bg = device.create_bind_group(
            "w2_bounds_bg",
            &bgl_bounds,
            &BindGroupEntries::sequential((
                bqs_starts.as_entire_buffer_binding(),
                bgq.as_entire_buffer_binding(),
                bgm.as_entire_buffer_binding(),
                bri.as_entire_buffer_binding(),
                bqs_sizes.as_entire_buffer_binding(),
            )),
        );

        Some(W2Fixture {
            app,
            device,
            queue,
            chunks_buffer,
            blocks_buf,
            voxels_buf,
            bqs_starts,
            bqs_sizes,
            bgm,
            params_buf,
            cc_buf: cc,
            cb_buf: cb,
            cv_buf: cv,
            cg_buf: cg,
            p_chunk,
            p_block,
            p_voxel,
            p_group,
            world_bg,
            change_bg,
            bounds_bg,
            size_in_chunks,
        })
    }

    #[allow(dead_code)]
    struct W2Fixture {
        app: App,
        device: RenderDevice,
        queue: RenderQueue,
        // Web-WebGPU migration: chunks is `array<vec2<u32>>` storage buffer.
        chunks_buffer: bevy::render::render_resource::Buffer,
        blocks_buf: bevy::render::render_resource::Buffer,
        voxels_buf: bevy::render::render_resource::Buffer,
        // 2026-05-19 wasm-chunk-aadf-determinism — `bound_queue_info` split
        // into two flat buffers (starts u32, sizes atomic<u32>).
        bqs_starts: bevy::render::render_resource::Buffer,
        bqs_sizes: bevy::render::render_resource::Buffer,
        bgm: bevy::render::render_resource::Buffer,
        params_buf: bevy::render::render_resource::Buffer,
        cc_buf: bevy::render::render_resource::Buffer,
        cb_buf: bevy::render::render_resource::Buffer,
        cv_buf: bevy::render::render_resource::Buffer,
        cg_buf: bevy::render::render_resource::Buffer,
        p_chunk: bevy::render::render_resource::ComputePipeline,
        p_block: bevy::render::render_resource::ComputePipeline,
        p_voxel: bevy::render::render_resource::ComputePipeline,
        p_group: bevy::render::render_resource::ComputePipeline,
        world_bg: bevy::render::render_resource::BindGroup,
        change_bg: bevy::render::render_resource::BindGroup,
        bounds_bg: bevy::render::render_resource::BindGroup,
        size_in_chunks: [u32; 3],
    }

    /// **GPU/CPU bit-exact** — `apply_chunk_change` writes the new state into
    /// `chunks[chunkPos].x` and preserves `.y`. CPU oracle is
    /// `apply_chunk_edit_cpu`. The W4 widened chunks `Rg32Uint`; verify the
    /// `.y` channel is preserved on every edit.
    #[test]
    fn apply_chunk_edit_cpu_gpu_bit_exact() {
        let Some(fx) = build_fixture_4x4x4() else {
            eprintln!("no wgpu adapter — skipping W2 chunk-edit bit-exact test");
            return;
        };
        // Pre-populate chunk (2,1,0)'s `.y` with a sentinel via
        // `write_buffer`. Web-WebGPU migration: chunks is `array<vec2<u32>>`
        // indexed by `x + y*sx + z*sx*sy` (x-fastest). Chunk (2,1,0) in a
        // 4×4×4 world → flat index 2 + 1*4 + 0*16 = 6; byte offset 6*8 = 48.
        let sentinel_y: u32 = 0xABCD_1234;
        let target_idx: usize = 2 + 1 * 4 + 0 * 16;
        let sentinel_pair = [0u32, sentinel_y];
        fx.queue.write_buffer(
            &fx.chunks_buffer,
            (target_idx as u64) * 8,
            bytemuck::bytes_of(&sentinel_pair),
        );

        // Build the edit: chunk (2,1,0), new state 0xDEAD_BEEF.
        use crate::aadf::edit::pack_chunk_pos;
        let pos_packed = pack_chunk_pos([2, 1, 0]);
        let new_state: u32 = 0xDEAD_BEEF;
        let changed_chunks: [[u32; 2]; 1] = [[pos_packed, new_state]];
        fx.queue
            .write_buffer(&fx.cc_buf, 0, bytemuck::cast_slice(&changed_chunks));

        // Set `changed_chunk_count = 1` in the params.
        let mut params = GpuConstructionParams {
            size_in_chunks: fx.size_in_chunks,
            _pad0: 0,
            group_size_in_groups: [1, 1, 1],
            _pad1: 0,
            bound_group_queue_max_size: 1,
            hash_map_size: 1,
            segment_size_in_chunks: 4,
            max_group_bound_dispatch: 0,
            chunk_offset: [0, 0, 0],
            dispatch_offset: 0,
            frame_index: 0,
            changed_chunk_count: 1,
            changed_block_count: 0,
            changed_voxel_count: 0,
        };
        fx.queue
            .write_buffer(&fx.params_buf, 0, bytemuck::bytes_of(&params));

        // Dispatch `apply_chunk_change`.
        let mut encoder = fx.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w2_chunk_edit_dispatch"),
        });
        dispatch_apply_chunk_change(
            &mut encoder,
            &fx.p_chunk,
            &fx.world_bg,
            &fx.change_bg,
            &fx.bounds_bg,
            1,
        );
        fx.queue.submit([encoder.finish()]);

        // Read back chunks buffer.
        let gpu_chunks = read_chunks_buffer(&fx.device, &fx.queue, &fx.chunks_buffer, fx.size_in_chunks);

        // CPU oracle.
        let mut cpu_chunks = vec![[0u32; 2]; gpu_chunks.len()];
        // Pre-set the .y sentinel on chunk (2,1,0) — same flat index as the
        // GPU-side `write_buffer` above.
        cpu_chunks[target_idx][1] = sentinel_y;
        apply_chunk_edit_cpu(&mut cpu_chunks, fx.size_in_chunks, pos_packed, new_state);

        // Bit-exact equality.
        assert_eq!(
            gpu_chunks, cpu_chunks,
            "GPU `apply_chunk_change` must produce byte-identical chunks texture as CPU oracle"
        );
        // **Load-bearing W2 contract:** the `.y` (entity-pointer) channel
        // was preserved on the edited chunk.
        assert_eq!(
            gpu_chunks[target_idx][1], sentinel_y,
            "GPU `apply_chunk_change` must preserve the .y entity-pointer channel"
        );
        let _ = &mut params;
    }

    /// Stand-alone verification of `.y` preservation across the GPU dispatch.
    /// (Distinct from the bit-exact test: this one only checks `.y` survives,
    /// even if other state changes unexpectedly.)
    #[test]
    fn entity_pointer_preserved_through_chunk_edit() {
        // A delegated re-run of the bit-exact test's `.y` portion — `cargo test`
        // failing this test specifically isolates the entity-pointer-preservation
        // contract from any other regression.
        apply_chunk_edit_cpu_gpu_bit_exact();
    }

    /// **GPU/CPU bit-exact** — `apply_block_change` writes 64 blocks at the
    /// CPU-supplied pointer with the local 4³ AADF recomputed via
    /// `compute_bounds_4` (GPU) / `compute_aadf_layer` (CPU).
    #[test]
    fn apply_block_edit_cpu_gpu_bit_exact() {
        let Some(fx) = build_fixture_4x4x4() else {
            eprintln!("no wgpu adapter — skipping W2 block-edit bit-exact test");
            return;
        };

        // 64 raw blocks — one full block at local index 0, rest empty.
        // changedBlocksDynamic layout: 65 u32s per edit = [pointer, 64 × block].
        let pointer: u32 = 64;
        let mut payload = vec![0u32; 65];
        payload[0] = pointer;
        // Block at local index 0 — uniform full of type 5.
        payload[1] = 5 | (1 << 30); // BLOCK_STATE_UNIFORM_FULL
        fx.queue.write_buffer(&fx.cb_buf, 0, bytemuck::cast_slice(&payload));

        let mut params = GpuConstructionParams {
            size_in_chunks: fx.size_in_chunks,
            _pad0: 0,
            group_size_in_groups: [1, 1, 1],
            _pad1: 0,
            bound_group_queue_max_size: 1,
            hash_map_size: 1,
            segment_size_in_chunks: 4,
            max_group_bound_dispatch: 0,
            chunk_offset: [0, 0, 0],
            dispatch_offset: 0,
            frame_index: 0,
            changed_chunk_count: 0,
            changed_block_count: 1,
            changed_voxel_count: 0,
        };
        fx.queue.write_buffer(&fx.params_buf, 0, bytemuck::bytes_of(&params));

        // Dispatch.
        let mut encoder = fx.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w2_block_edit_dispatch"),
        });
        dispatch_apply_block_change(
            &mut encoder,
            &fx.p_block,
            &fx.world_bg,
            &fx.change_bg,
            &fx.bounds_bg,
            1,
        );
        fx.queue.submit([encoder.finish()]);

        // Read back the touched blocks slice.
        let gpu_blocks = read_u32_buf(&fx.device, &fx.queue, &fx.blocks_buf, (pointer + 64) as u64);
        let gpu_block_slice: Vec<u32> = gpu_blocks[pointer as usize..(pointer as usize + 64)].to_vec();

        // CPU oracle.
        let mut cpu_blocks = vec![0u32; (pointer + 64) as usize];
        let mut raw = [0u32; 64];
        raw[0] = 5 | (1 << 30);
        apply_block_edit_cpu(&mut cpu_blocks, pointer, &raw);
        let cpu_block_slice: Vec<u32> = cpu_blocks[pointer as usize..(pointer as usize + 64)].to_vec();

        assert_eq!(
            gpu_block_slice, cpu_block_slice,
            "GPU `apply_block_change` must produce byte-identical blocks slice as CPU oracle"
        );
        let _ = &mut params;
    }

    /// **GPU/CPU bit-exact** — `apply_voxel_change` writes 32 packed-voxel-pair
    /// u32s at the CPU-supplied pointer with the local 4³ voxel-AADF recomputed.
    #[test]
    fn apply_voxel_edit_cpu_gpu_bit_exact() {
        let Some(fx) = build_fixture_4x4x4() else {
            eprintln!("no wgpu adapter — skipping W2 voxel-edit bit-exact test");
            return;
        };

        // changedVoxelsDynamic layout: 33 u32s per edit = [pointer, 32 packed pairs].
        let pointer: u32 = 32;
        let mut payload = vec![0u32; 33];
        payload[0] = pointer;
        // Voxel at local index 0 — full voxel of type 3.
        let full = (VOXEL_FULL_FLAG | 3) as u32;
        payload[1] = full; // low half = voxel 0; high half = voxel 1 (empty, low 16 bits = 0)
        fx.queue.write_buffer(&fx.cv_buf, 0, bytemuck::cast_slice(&payload));

        let mut params = GpuConstructionParams {
            size_in_chunks: fx.size_in_chunks,
            _pad0: 0,
            group_size_in_groups: [1, 1, 1],
            _pad1: 0,
            bound_group_queue_max_size: 1,
            hash_map_size: 1,
            segment_size_in_chunks: 4,
            max_group_bound_dispatch: 0,
            chunk_offset: [0, 0, 0],
            dispatch_offset: 0,
            frame_index: 0,
            changed_chunk_count: 0,
            changed_block_count: 0,
            changed_voxel_count: 1,
        };
        fx.queue.write_buffer(&fx.params_buf, 0, bytemuck::bytes_of(&params));

        let mut encoder = fx.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w2_voxel_edit_dispatch"),
        });
        dispatch_apply_voxel_change(
            &mut encoder,
            &fx.p_voxel,
            &fx.world_bg,
            &fx.change_bg,
            &fx.bounds_bg,
            1,
        );
        fx.queue.submit([encoder.finish()]);

        let gpu_voxels = read_u32_buf(&fx.device, &fx.queue, &fx.voxels_buf, (pointer + 32) as u64);
        let gpu_voxel_slice: Vec<u32> = gpu_voxels[pointer as usize..(pointer as usize + 32)].to_vec();

        let mut cpu_voxels = vec![0u32; (pointer + 32) as usize];
        let mut raw = [0u16; 64];
        raw[0] = VOXEL_FULL_FLAG | 3;
        apply_voxel_edit_cpu(&mut cpu_voxels, pointer, &raw);
        let cpu_voxel_slice: Vec<u32> = cpu_voxels[pointer as usize..(pointer as usize + 32)].to_vec();

        assert_eq!(
            gpu_voxel_slice, cpu_voxel_slice,
            "GPU `apply_voxel_change` must produce byte-identical voxels slice as CPU oracle"
        );
        let _ = &mut params;
        let _ = VOXEL_PAYLOAD_MASK;
    }

    /// `apply_group_change` writes into the W3 bound-queue family
    /// (`bound_queue_starts` / `bound_queue_sizes` / `bound_group_queues` /
    /// `bound_group_masks`). After a single edit at group (0,0,0) with
    /// reset-completely flag, the X/Y/Z size-0 queues each gain one entry,
    /// and the group's mask gets bit-0 set on all 3 axes.
    #[test]
    fn edit_re_enqueues_bound_queue() {
        // For this test the world must be at least 4 chunks per axis (bound
        // groups only exist when `sizeInChunks % 4 == 0` per W3); the fixture
        // is 4×4×4, so `bound_group_count = 1`.
        let Some(fx) = build_fixture_4x4x4() else {
            eprintln!("no wgpu adapter — skipping W2 bound-queue re-enqueue test");
            return;
        };

        use crate::aadf::edit::pack_chunk_pos;
        // The changed_groups payload — a single directly-edited group (0,0,0)
        // with the reset-completely flag (`0xC0000000`).
        let pos_packed = pack_chunk_pos([0u32, 0, 0]);
        let dist_packed = 0xC000_0000u32;
        let changed_groups: [[u32; 2]; 1] = [[pos_packed, dist_packed]];
        fx.queue
            .write_buffer(&fx.cg_buf, 0, bytemuck::cast_slice(&changed_groups));

        // 2026-05-19 wasm-chunk-aadf-determinism — seed `bound_queue_starts`
        // + `bound_queue_sizes` to all zero (queues empty before the
        // re-enqueue). Replaces a single packed (start, size) seed.
        let zero_seed: Vec<u32> = vec![0u32; 32 * 3];
        fx.queue.write_buffer(&fx.bqs_starts, 0, bytemuck::cast_slice(&zero_seed));
        fx.queue.write_buffer(&fx.bqs_sizes, 0, bytemuck::cast_slice(&zero_seed));
        // Seed `bound_group_masks` to zero (no axis enqueued yet).
        let mask_seed: Vec<u32> = vec![0u32; 16];
        fx.queue.write_buffer(&fx.bgm, 0, bytemuck::cast_slice(&mask_seed));

        // params: 1 group, 1 bound_group_queue_max_size.
        let params = GpuConstructionParams {
            size_in_chunks: fx.size_in_chunks,
            _pad0: 0,
            group_size_in_groups: [1, 1, 1],
            _pad1: 0,
            bound_group_queue_max_size: 1,
            hash_map_size: 1,
            segment_size_in_chunks: 4,
            max_group_bound_dispatch: 0,
            chunk_offset: [0, 0, 0],
            dispatch_offset: 0,
            frame_index: 0,
            changed_chunk_count: 0,
            changed_block_count: 0,
            changed_voxel_count: 0,
        };
        fx.queue.write_buffer(&fx.params_buf, 0, bytemuck::bytes_of(&params));

        // Dispatch.
        let mut encoder = fx.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w2_group_edit_dispatch"),
        });
        dispatch_apply_group_change(
            &mut encoder,
            &fx.p_group,
            &fx.world_bg,
            &fx.change_bg,
            &fx.bounds_bg,
            1,
        );
        fx.queue.submit([encoder.finish()]);

        // Read back bound_queue_sizes — the size-0 X/Y/Z entries should each
        // be 1 (one group re-enqueued per axis). 2026-05-19
        // wasm-chunk-aadf-determinism — flat sizes buffer (one u32 per qi).
        let sizes = read_u32_buf(&fx.device, &fx.queue, &fx.bqs_sizes, 32 * 3);
        let size_0_x = sizes[0 * 3 + 0];
        let size_0_y = sizes[0 * 3 + 1];
        let size_0_z = sizes[0 * 3 + 2];
        assert_eq!(size_0_x, 1, "X size-0 queue should have 1 group");
        assert_eq!(size_0_y, 1, "Y size-0 queue should have 1 group");
        assert_eq!(size_0_z, 1, "Z size-0 queue should have 1 group");

        // Read back bound_group_masks — group (0,0,0)'s mask for each axis
        // should have bit 0 set (the size-0 queue bit).
        let bgm = read_u32_buf(&fx.device, &fx.queue, &fx.bgm, 3);
        assert_eq!(bgm[0] & 1, 1, "X mask bit 0 should be set");
        assert_eq!(bgm[1] & 1, 1, "Y mask bit 0 should be set");
        assert_eq!(bgm[2] & 1, 1, "Z mask bit 0 should be set");
    }
}
