//! W3 — `bounds_calc.wgsl` Rust side: layouts, pipeline queueing, dispatch
//! helpers, and the regime-2 `naadf_bounds_compute_node` `Core3d`-schedule
//! system (`15-design-c.md` §1.2 regime-2, §1.3, §2.1 W3, §4.2;
//! `16-impl-c-W3.md`).
//!
//! Pipelines:
//!   - `bounds_calc_pipeline_add_initial` — `add_initial_groups_to_bound_queue`,
//!     regime-1 one-shot seed (`boundsCalc.fx:39-48`). The W1 startup driver
//!     extends to call it after `compute_block_bounds`.
//!   - `bounds_calc_pipeline_prepare`     — `prepare_group_bounds`,
//!     regime-2 single-thread picker + indirect-count writer (`fx:51-93`).
//!   - `bounds_calc_pipeline_compute`     — `compute_group_bounds`,
//!     regime-2 4³-workgroup per-chunk AADF expander (`fx:118-193`).
//!
//! Layouts (per `15-design-c.md` §1.3):
//!   - `construction_bounds_world_layout` `@group(0)` — 2 bindings:
//!     `chunks` rw 3D texture + `params` uniform. **Smaller than W1's
//!     `construction_world_layout`** (which carries 8 bindings for the
//!     Algorithm-1 buffers): `boundsCalc` only needs chunks + params, so the
//!     dedicated narrow layout (a) lets the W3 prepare system run without
//!     W1's hash buffers existing, (b) cuts the bind-group descriptor count
//!     from 8 to 2.
//!   - `construction_bounds_layout`       `@group(1)` — 4 bindings: the
//!     bound-queue family `bound_queue_info` / `bound_group_queues` /
//!     `bound_group_masks` / `bound_refined_info`. All rw storage.
//!   - `bound_dispatch_indirect_layout`   `@group(2)` — 1 binding:
//!     `bound_dispatch_indirect` rw storage. **Separated** because the same
//!     buffer is also consumed by `dispatch_workgroups_indirect` as
//!     `INDIRECT`-args, and wgpu's `STORAGE_READ_WRITE` × `INDIRECT`
//!     exclusivity rule forbids both usages in one layout. Mirrors the
//!     Phase-B Batch-4 `sample_refine_dispatch_layout` split
//!     (`render/pipelines.rs:531-540`).

use std::borrow::Cow;
use std::num::NonZeroU64;

use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::render_resource::{
    binding_types::{storage_buffer_sized, texture_storage_3d, uniform_buffer_sized},
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId,
    CommandEncoder, ComputePassDescriptor, ComputePipelineDescriptor, PipelineCache,
    ShaderStages, StorageTextureAccess, TextureFormat,
};
use bevy::render::renderer::RenderContext;
use bevy::shader::Shader;

use crate::render::construction::config::ConstructionConfig;
use crate::render::construction::{ConstructionBindGroups, ConstructionGpu};
use crate::render::gpu_types::GpuConstructionParams;

/// Asset path of the W3 `bounds_calc.wgsl` shader.
pub const BOUNDS_CALC_SHADER: &str = "shaders/bounds_calc.wgsl";

/// Inlined source — used by the W3 unit test (which builds a headless render
/// world without an asset loader). Same pattern as W1 / W5.
pub const BOUNDS_CALC_SHADER_SRC: &str =
    include_str!("../../assets/shaders/bounds_calc.wgsl");

/// Timing-span name for the regime-2 bound-queue node — surfaces in the HUD
/// as `render/naadf_bounds_compute/elapsed_gpu`.
pub const BOUNDS_COMPUTE_SPAN: &str = "naadf_bounds_compute";

// ─── Layout descriptors ───────────────────────────────────────────────────────

/// `construction_bounds_world_layout` `@group(0)` (2 bindings: `chunks` rw
/// texture + `params` uniform). Distinct from W1's 8-binding
/// `construction_world_layout` so W3 doesn't depend on the hash-map family
/// (`15-design-c.md` §1.3, `16-impl-c-W3.md` decision #2).
pub fn construction_bounds_world_layout_descriptor() -> BindGroupLayoutDescriptor {
    let params_size =
        NonZeroU64::new(std::mem::size_of::<GpuConstructionParams>() as u64).unwrap();
    BindGroupLayoutDescriptor::new(
        "naadf_construction_bounds_world_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                // chunks_rw — `texture_storage_3d<r32uint, read_write>`.
                // Forward-compat: `.x` selection in WGSL keeps the W4 widening
                // to `Rg32Uint` a no-op (`15-design-c.md` §1.7).
                texture_storage_3d(TextureFormat::R32Uint, StorageTextureAccess::ReadWrite),
                // params — uniform.
                uniform_buffer_sized(false, Some(params_size)),
            ),
        ),
    )
}

/// `construction_bounds_layout` `@group(1)` (4 bindings: the bound-queue
/// family). Per `15-design-c.md` §1.3.
pub fn construction_bounds_layout_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "naadf_construction_bounds_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                // bound_queue_info_rw — `array<BoundQueueInfo>` (rw storage,
                // with the `size` field declared `atomic<u32>` on the WGSL
                // side; the wgpu binding type is the same `storage_buffer_sized`).
                storage_buffer_sized(false, None),
                // bound_group_queues_rw — `array<u32>` (rw).
                storage_buffer_sized(false, None),
                // bound_group_masks_rw — `array<atomic<u32>>` (rw, atomic on
                // the WGSL side).
                storage_buffer_sized(false, None),
                // bound_refined_info_rw — `array<u32>` (3 elements; rw).
                storage_buffer_sized(false, None),
            ),
        ),
    )
}

/// `bound_dispatch_indirect_layout` `@group(2)` (1 binding: the indirect-
/// dispatch counter, write-side only). The same buffer is consumed by
/// `dispatch_workgroups_indirect` as `INDIRECT`-args — the wgpu rule that
/// `STORAGE_READ_WRITE` and `INDIRECT` usages cannot share a single layout
/// makes this its own layout, mirroring Phase B's `sample_refine_dispatch_layout`
/// (`render/pipelines.rs:531-540`, `15-design-c.md` §1.3).
pub fn bound_dispatch_indirect_layout_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "naadf_bound_dispatch_indirect_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (storage_buffer_sized(false, None),),
        ),
    )
}

// ─── Pipeline queueing ────────────────────────────────────────────────────────

/// Queue the `add_initial_groups_to_bound_queue` pipeline against the W3
/// layouts. Only `@group(0)` + `@group(1)` are bound (no indirect output).
pub fn queue_add_initial_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(BOUNDS_CALC_SHADER);
    queue_add_initial_pipeline_with_handle(pipeline_cache, world_layout, bounds_layout, shader)
}

pub fn queue_add_initial_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_bounds_calc_add_initial_pipeline".into()),
        layout: vec![world_layout, bounds_layout],
        shader,
        entry_point: Some(Cow::from("add_initial_groups_to_bound_queue")),
        ..default()
    })
}

/// Queue the `prepare_group_bounds` pipeline. Binds all 3 groups (writes to
/// `bound_refined_info` in `@group(1)` AND to `bound_dispatch_indirect` in
/// `@group(2)`).
pub fn queue_prepare_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    dispatch_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(BOUNDS_CALC_SHADER);
    queue_prepare_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        bounds_layout,
        dispatch_layout,
        shader,
    )
}

pub fn queue_prepare_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    dispatch_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_bounds_calc_prepare_pipeline".into()),
        layout: vec![world_layout, bounds_layout, dispatch_layout],
        shader,
        entry_point: Some(Cow::from("prepare_group_bounds")),
        ..default()
    })
}

/// Queue the `compute_group_bounds` pipeline. Binds `@group(0)` + `@group(1)`
/// (the indirect-dispatch is consumed via `dispatch_workgroups_indirect`, NOT
/// bound to the shader — `15-design-c.md` §1.3 split).
pub fn queue_compute_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(BOUNDS_CALC_SHADER);
    queue_compute_pipeline_with_handle(pipeline_cache, world_layout, bounds_layout, shader)
}

pub fn queue_compute_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_bounds_calc_compute_pipeline".into()),
        layout: vec![world_layout, bounds_layout],
        shader,
        entry_point: Some(Cow::from("compute_group_bounds")),
        ..default()
    })
}

// ─── Dispatch helpers ─────────────────────────────────────────────────────────

/// Dispatch `add_initial_groups_to_bound_queue` for `bound_group_count`
/// groups (one workgroup per 64 groups). Called in regime-1 startup.
pub fn dispatch_add_initial_groups(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bind_group: &bevy::render::render_resource::BindGroup,
    bounds_bind_group: &bevy::render::render_resource::BindGroup,
    bound_group_count: u32,
) {
    if bound_group_count == 0 {
        return;
    }
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_bounds_calc_add_initial_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, world_bind_group, &[]);
    pass.set_bind_group(1, bounds_bind_group, &[]);
    pass.dispatch_workgroups(bound_group_count.div_ceil(64).max(1), 1, 1);
}

/// W3 regime-2 helper: run `n_rounds` of {prepare → indirect compute} inside
/// the given encoder. Mirrors NAADF's `WorldBoundHandler.Update` loop
/// (`WorldBoundHandler.cs:113-120`).
#[allow(clippy::too_many_arguments)]
pub fn dispatch_regime_2_rounds(
    encoder: &mut CommandEncoder,
    prepare_pipeline: &bevy::render::render_resource::ComputePipeline,
    compute_pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bind_group: &bevy::render::render_resource::BindGroup,
    bounds_bind_group: &bevy::render::render_resource::BindGroup,
    dispatch_bind_group: &bevy::render::render_resource::BindGroup,
    indirect_buffer: &bevy::render::render_resource::Buffer,
    n_rounds: u32,
) {
    for _ in 0..n_rounds {
        // Pass 1: `prepare_group_bounds` — single-thread.
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("naadf_bounds_calc_prepare_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(prepare_pipeline);
            pass.set_bind_group(0, world_bind_group, &[]);
            pass.set_bind_group(1, bounds_bind_group, &[]);
            pass.set_bind_group(2, dispatch_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        // Pass 2: `compute_group_bounds` — indirect off the dispatch buffer
        // `prepare_group_bounds` just wrote. wgpu's automatic
        // STORAGE→INDIRECT barrier serialises the access.
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("naadf_bounds_calc_compute_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(compute_pipeline);
            pass.set_bind_group(0, world_bind_group, &[]);
            pass.set_bind_group(1, bounds_bind_group, &[]);
            pass.dispatch_workgroups_indirect(indirect_buffer, 0);
        }
    }
}

// ─── Regime-2 Core3d node ─────────────────────────────────────────────────────

/// `Core3d`-schedule system: the W3 regime-2 background AADF queue node
/// (`15-design-c.md` §1.2 regime-2, §3 — `naadf_bounds_compute_node`).
///
/// Inserted in `render/mod.rs::add_systems(Core3d, …)` **before**
/// `naadf_atmosphere_node`. Runs `ConstructionConfig.n_bounds_rounds` rounds
/// of {`prepare_group_bounds` → indirect `compute_group_bounds`} per frame —
/// the regime-2 "one queue per frame" rate from paper §3.3.
///
/// Skips silently until the W3 GPU resources + bind groups exist (W1 prepare
/// has populated `WorldGpu` and the W3 prepare extension has allocated the
/// bound-queue buffers + built the bind groups). On a static world, after the
/// regime-1 startup seed exhausts (every chunk's AADF converged), subsequent
/// frames find every queue empty and `prepare_group_bounds` writes
/// `bound_refined_info[1] = 0` + `bound_dispatch_indirect[0] = 1`; the
/// indirect `compute_group_bounds` then runs but `count = 0` so every chunk
/// thread short-circuits — net work per round is a single 4³-thread group
/// that bails immediately. (NAADF accepts the same minimum-dispatch cost —
/// `boundsCalc.fx:92` `max(1, groupAmount)`.)
pub fn naadf_bounds_compute_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    construction_pipelines: Option<Res<super::ConstructionPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_gpu: Option<Res<ConstructionGpu>>,
    construction_config: Option<Res<ConstructionConfig>>,
) {
    let Some(construction_pipelines) = construction_pipelines else { return; };
    let Some(construction_bind_groups) = construction_bind_groups else { return; };
    let Some(construction_gpu) = construction_gpu else { return; };
    let Some(construction_config) = construction_config else { return; };

    if !construction_config.gpu_construction_enabled {
        return;
    }
    if construction_config.max_group_bound_dispatch == 0 {
        // NAADF early-return — `WorldBoundHandler.cs:94-95`.
        return;
    }

    // Pull the three bind groups + the indirect buffer.
    let Some(bounds_world_bg) = construction_bind_groups.construction_bounds_world.as_ref()
    else { return; };
    let Some(bounds_bg) = construction_bind_groups.construction_bounds.as_ref() else {
        return;
    };
    let Some(dispatch_bg) = construction_bind_groups.bound_dispatch.as_ref() else {
        return;
    };
    let Some(indirect_buffer) = construction_gpu.bound_dispatch_indirect.as_ref() else {
        return;
    };

    // Resolve the two pipelines.
    let (Some(prepare_pipeline), Some(compute_pipeline)) = (
        pipeline_cache.get_compute_pipeline(construction_pipelines.bounds_calc_pipeline_prepare),
        pipeline_cache.get_compute_pipeline(construction_pipelines.bounds_calc_pipeline_compute),
    ) else {
        return;
    };

    let n_rounds = construction_config.n_bounds_rounds.max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, BOUNDS_COMPUTE_SPAN);
    dispatch_regime_2_rounds(
        encoder,
        prepare_pipeline,
        compute_pipeline,
        bounds_world_bg,
        bounds_bg,
        dispatch_bg,
        indirect_buffer,
        n_rounds,
    );
    time_span.end(render_context.command_encoder());
}

// ─── Sizing helpers + per-frame uniform writer ────────────────────────────────

/// Number of bound groups for a world of `size_in_chunks`. Returns 0 when any
/// axis is not divisible by 4 OR when the total chunk count is < 64. NAADF
/// requires `sizeInChunks % 4 == 0` per axis (`WorldBoundHandler.cs:41`); the
/// `GridPreset::Default` test scene (4×2×4) yields **0 groups** because of
/// the Y dim — the bound queue infra still allocates fixed-size buffers but
/// no work runs.
pub fn bound_group_count_of(size_in_chunks: [u32; 3]) -> u32 {
    if !size_in_chunks[0].is_multiple_of(4)
        || !size_in_chunks[1].is_multiple_of(4)
        || !size_in_chunks[2].is_multiple_of(4)
    {
        return 0;
    }
    (size_in_chunks[0] / 4) * (size_in_chunks[1] / 4) * (size_in_chunks[2] / 4)
}

/// Number of bound groups along each axis. Always reflects the axis sizes
/// even when the count is 0 (so the WGSL `group_size_in_groups` is consistent
/// even on small worlds where regime-2 is dormant).
pub fn group_size_in_groups_of(size_in_chunks: [u32; 3]) -> [u32; 3] {
    [
        size_in_chunks[0] / 4,
        size_in_chunks[1] / 4,
        size_in_chunks[2] / 4,
    ]
}

#[cfg(test)]
mod tests;
