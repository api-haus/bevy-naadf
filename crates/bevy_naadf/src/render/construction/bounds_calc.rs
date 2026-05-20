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
//!   - `construction_bounds_layout`       `@group(1)` — 5 bindings: the
//!     bound-queue family `bound_queue_starts` / `bound_group_queues` /
//!     `bound_group_masks` / `bound_refined_info` / `bound_queue_sizes`. All
//!     rw storage. 2026-05-19 wasm-chunk-aadf-determinism fix split the
//!     original packed `bound_queue_info: array<BoundQueueInfo {start, size}>`
//!     into two top-level flat arrays (`bound_queue_starts: array<u32>` +
//!     `bound_queue_sizes: array<atomic<u32>>`) so Tint emits the proven-
//!     working `array<atomic<u32>>` lowering for the cross-pass atomic
//!     `size` field. See `assets/shaders/bounds_calc.wgsl` header for the
//!     full motivation.
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
    binding_types::{storage_buffer_sized, uniform_buffer_sized},
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId,
    CommandEncoder, ComputePassDescriptor, ComputePipelineDescriptor, PipelineCache,
    ShaderStages,
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
                // chunks_rw — `array<vec2<u32>>` storage buffer (W4 widened
                // the chunk pair; web-WebGPU migration replaced the original
                // `texture_storage_3d<rg32uint, read_write>` because WebGPU
                // forbids `read_write` storage textures on non-r32 formats).
                // The W3 WGSL still reads `.x` and writes preserving `.y`.
                storage_buffer_sized(false, None),
                // params — uniform.
                uniform_buffer_sized(false, Some(params_size)),
            ),
        ),
    )
}

/// `construction_bounds_layout` `@group(1)` (5 bindings: the bound-queue
/// family). Per `15-design-c.md` §1.3. 2026-05-19 wasm-chunk-aadf-determinism
/// fix: the original C# `BoundQueueInfo { start, size }` packed struct (1
/// binding) was split into two top-level flat buffers (`bound_queue_starts`
/// at binding 0 + `bound_queue_sizes` at binding 4) so Tint emits the same
/// `array<atomic<u32>>` lowering shape `bound_group_masks` already uses
/// correctly on Dawn/WebGPU. This adopts the proven-working cross-pass
/// atomic-visibility pattern for the `size` field that holds the regime-2
/// re-enqueue count. Layout count widened 4 → 5; well under the wasm
/// `max_storage_buffers_per_shader_stage` cap (≥ 8 per device snapshots).
pub fn construction_bounds_layout_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "naadf_construction_bounds_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                // bound_queue_starts_rw — `array<u32>` (rw, non-atomic;
                // written only by `prepare_group_bounds` at
                // `@workgroup_size(1, 1, 1)`).
                storage_buffer_sized(false, None),
                // bound_group_queues_rw — `array<u32>` (rw).
                storage_buffer_sized(false, None),
                // bound_group_masks_rw — `array<atomic<u32>>` (rw, atomic on
                // the WGSL side).
                storage_buffer_sized(false, None),
                // bound_refined_info_rw — `array<u32>` (16 elements; rw).
                storage_buffer_sized(false, None),
                // bound_queue_sizes_rw — `array<atomic<u32>>` (rw, atomic on
                // the WGSL side; written by `prepare_group_bounds` via
                // `atomicStore` and `compute_group_bounds` via `atomicAdd`).
                // 2026-05-19 web fix — adopts the `bound_group_masks` shape
                // for Tint cross-pass atomic-visibility.
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

/// 2026-05-19 probe-1B — `prepare_probe_history_layout` `@group(3)` (1 binding:
/// the per-call probe history buffer, write-side only — but the wgpu binding
/// type is `storage` rw because WGSL declares it as `read_write`). Only the
/// `prepare_group_bounds` pipeline includes this group; the other entry points
/// (`add_initial_groups_to_bound_queue` and `compute_group_bounds`) leave the
/// binding declared in WGSL but unreferenced from their entry-point bodies,
/// and their Rust pipeline-layouts omit this 4th group entirely.
///
/// Storage-buffer count check (per wasm `max_storage_buffers_per_shader_stage =
/// 16`): `prepare_group_bounds` previously bound 1 (chunks) + 4 (bound_queue
/// family) + 1 (bound_dispatch_indirect) = 6 storage buffers. With this 4th
/// group: 6 + 1 = **7 storage buffers**, well under the 16 cap. The
/// `params` uniform in group 0 binding 1 does not count against the storage
/// cap.
///
/// Bind-group count check (per wasm `max_bind_groups = 4`): the prepare
/// pipeline already used 3 bind groups (0, 1, 2). Adding group 3 puts the
/// pipeline at **4 bind groups = exactly at the wasm limit** — legal per
/// WebGPU spec (the limit is inclusive).
pub fn prepare_probe_history_layout_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "naadf_prepare_probe_history_bind_group_layout",
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

/// Queue the `prepare_group_bounds` pipeline. Binds all 4 groups (writes to
/// `bound_refined_info` in `@group(1)`, `bound_dispatch_indirect` in
/// `@group(2)`, and the probe-1B `prepare_probe_history` in `@group(3)`).
pub fn queue_prepare_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    dispatch_layout: BindGroupLayoutDescriptor,
    probe_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(BOUNDS_CALC_SHADER);
    queue_prepare_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        bounds_layout,
        dispatch_layout,
        probe_layout,
        shader,
    )
}

pub fn queue_prepare_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    dispatch_layout: BindGroupLayoutDescriptor,
    probe_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_bounds_calc_prepare_pipeline".into()),
        layout: vec![world_layout, bounds_layout, dispatch_layout, probe_layout],
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

/// 2026-05-20 probe-2 — `end_of_encoder_noop` pipeline (M1 confirmation probe
/// per `07-diagnosis-round2.md` §I item 1). Uses the same 2-layout list as
/// `compute_group_bounds` (`world_layout + bounds_layout`); only `@group(1)`'s
/// `bound_queue_sizes` is actually accessed by the entry-point body, but
/// keeping the world group in the layout matches the compute pipeline's shape
/// so the existing `bounds_world_bg` can be bound without ceremony.
///
/// Only dispatched from the wasm-only per-round encoder branch in
/// `naadf_bounds_compute_node` (the native path never references this
/// pipeline). The compile cost is cheap (1-line entry point) and the WGSL is
/// always declared — only the dispatch is cfg-gated.
pub fn queue_end_of_encoder_noop_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(BOUNDS_CALC_SHADER);
    queue_end_of_encoder_noop_pipeline_with_handle(
        pipeline_cache,
        world_layout,
        bounds_layout,
        shader,
    )
}

pub fn queue_end_of_encoder_noop_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    world_layout: BindGroupLayoutDescriptor,
    bounds_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_bounds_calc_end_of_encoder_noop_pipeline".into()),
        layout: vec![world_layout, bounds_layout],
        shader,
        entry_point: Some(Cow::from("end_of_encoder_noop")),
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
///
/// **`compute_workgroups_override`** (2026-05-19 web-vox ray-termination fix):
/// when `Some(n)`, the regime-2 compute pass is dispatched **directly** with
/// `n` workgroups (1D) instead of via `dispatch_workgroups_indirect`. This is
/// the WebGPU workaround for a wgpu/Dawn ordering bug where the STORAGE→
/// INDIRECT barrier between `prepare_group_bounds` (which writes
/// `bound_dispatch_indirect[0]`) and the indirect `compute_group_bounds`
/// dispatch is not honoured — Dawn reads stale indirect args and dispatches
/// only the seeded `[1, 1, 1]`, leaving the chunk-AADF acceleration
/// permanently unbuilt and causing rays to step chunk-by-chunk (120 ×
/// 16 voxels = ~30 % of the production-world depth before exhaustion).
///
/// On native (`None`): unchanged — indirect dispatch reads `count` written
/// by `prepare_group_bounds` and dispatches exactly that many workgroups.
///
/// On web (`Some(n)`): direct dispatch of `n` workgroups; the shader's
/// existing `is_group_active = group_id.x < count` early-bail at
/// `bounds_calc.wgsl:331` short-circuits the wasted workgroups. `n` must
/// equal the `max_group_bound_dispatch` value uploaded to the params
/// uniform so prepare's `min(max_group_bound_dispatch, found_size)` cannot
/// claim more groups than the direct dispatch can drain (claimed-but-not-
/// drained groups would be silently lost from the queue).
#[allow(clippy::too_many_arguments)]
pub fn dispatch_regime_2_rounds(
    encoder: &mut CommandEncoder,
    prepare_pipeline: &bevy::render::render_resource::ComputePipeline,
    compute_pipeline: &bevy::render::render_resource::ComputePipeline,
    world_bind_group: &bevy::render::render_resource::BindGroup,
    bounds_bind_group: &bevy::render::render_resource::BindGroup,
    dispatch_bind_group: &bevy::render::render_resource::BindGroup,
    probe_bind_group: &bevy::render::render_resource::BindGroup,
    indirect_buffer: &bevy::render::render_resource::Buffer,
    n_rounds: u32,
    compute_workgroups_override: Option<u32>,
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
            // 2026-05-19 probe-1B — group 3 = `prepare_probe_history`.
            pass.set_bind_group(3, probe_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        // Pass 2: `compute_group_bounds` — indirect off the dispatch buffer
        // `prepare_group_bounds` just wrote on native (wgpu's automatic
        // STORAGE→INDIRECT barrier serialises the access), OR direct with
        // a fixed workgroup count on web (bypasses the buggy Dawn barrier).
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("naadf_bounds_calc_compute_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(compute_pipeline);
            pass.set_bind_group(0, world_bind_group, &[]);
            pass.set_bind_group(1, bounds_bind_group, &[]);
            match compute_workgroups_override {
                Some(n) => {
                    pass.dispatch_workgroups(n.max(1), 1, 1);
                }
                None => {
                    pass.dispatch_workgroups_indirect(indirect_buffer, 0);
                }
            }
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
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    render_device: Res<bevy::render::renderer::RenderDevice>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    render_queue: Res<bevy::render::renderer::RenderQueue>,
    // 2026-05-20 dispatch-2 iter-2-4 — chunks-self-copy intervention needs
    // access to the chunks buffer.
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
    // 2026-05-20 dispatch-2 iter-2-4 — lazy scratch buffer for chunks-self-copy.
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    mut chunks_scratch: bevy::ecs::system::Local<Option<bevy::render::render_resource::Buffer>>,
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
    // vox-gpu-rewrite Bug W3-T1 fix (2026-05-18) — regime-2 (prepare +
    // compute) MUST NOT run before regime-1 (`add_initial_groups_to_bound_queue`)
    // has populated `bound_group_queues`. The CPU pre-seed of
    // `bound_queue_info[0..2].size = 32768` plus zero-initialized
    // `bound_group_queues` is an internally inconsistent state — `compute_group_bounds`
    // would drain 32768 queue slots all decoding to group (0,0,0), corrupt the
    // queue with re-enqueues at (0,0,0), and permanently strand the real seed
    // data when it finally lands one frame later. C# avoids this by calling
    // `WorldBoundHandler.Initialize` synchronously before any `Update()`
    // (`WorldBoundHandler.cs:53-89`); the Rust port splits the two across
    // schedules so we must gate the consumer on the producer's flag.
    // Diagnostic: `docs/orchestrate/vox-gpu-rewrite/13-diagnostic-w3-bounds-calc.md`.
    if !construction_gpu.bounds_initialized {
        return;
    }

    // Pull the four bind groups + the indirect buffer. The probe bind group
    // is required by the prepare pipeline's 4-layout list.
    let Some(bounds_world_bg) = construction_bind_groups.construction_bounds_world.as_ref()
    else { return; };
    let Some(bounds_bg) = construction_bind_groups.construction_bounds.as_ref() else {
        return;
    };
    let Some(dispatch_bg) = construction_bind_groups.bound_dispatch.as_ref() else {
        return;
    };
    let Some(probe_bg) = construction_bind_groups.prepare_probe_history.as_ref() else {
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

    // 2026-05-20 probe-2 — resolve the end-of-encoder no-op pipeline (wasm
    // only). If the wasm-build hasn't yet resolved it, skip the node entirely
    // (rather than dispatching prepare+compute without the probe — that would
    // muddle the probe's signal).
    #[cfg(target_arch = "wasm32")]
    let Some(end_of_encoder_noop_pipeline) =
        pipeline_cache.get_compute_pipeline(
            construction_pipelines.bounds_calc_pipeline_end_of_encoder_noop,
        )
    else {
        return;
    };

    let n_rounds = construction_config.n_bounds_rounds.max(1);

    // 2026-05-20 dispatch-2 iter-2-2 H1 — use Bevy's main encoder for wasm
    // W3, same as native. The per-round-encoder+submit pattern (used in
    // dispatch-1 iter-0 through iter-5) does NOT solve the bug (iter-1
    // proved one-encoder-per-frame is also affected). But going one step
    // further — making wasm use Bevy's render_context.command_encoder()
    // identically to native — puts W3's compute pass + the renderer's
    // first-hit compute pass in the SAME command buffer. Dawn's intra-
    // encoder PassResourceUsageTracker should then insert a
    // SHADER_WRITE→SHADER_READ barrier on chunks[] between W3's last
    // compute pass and first-hit's read of chunks[] via world_data.wgsl's
    // read-only binding. Native gets this for free; wasm with the per-
    // round-submit pattern was losing it.
    //
    // We keep the direct-dispatch + 4096-cap on wasm (the original Dawn
    // STORAGE→INDIRECT barrier bug is still suspected; iter-2-3 if needed
    // will probe whether that bug still exists).

    // 2026-05-19 horizon-parity AADF diagnostic — one-shot log of the
    // construction-config values reaching the regime-2 node (verifies the
    // wasm clamp on `max_group_bound_dispatch` actually flows here from
    // `From<&AppArgs> for ConstructionConfig`).
    {
        static LOGGED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            bevy::log::info!(
                "[aadf-probe] regime-2 config: n_bounds_rounds={} \
                 max_group_bound_dispatch={} (the wasm clamp ceiling is 4096)",
                n_rounds,
                construction_config.max_group_bound_dispatch,
            );
        }
    }

    // 2026-05-19 web-vox ray-termination fix — on WebGPU, the STORAGE→INDIRECT
    // barrier between `prepare_group_bounds` (writes `bound_dispatch_indirect`)
    // and the indirect `compute_group_bounds` dispatch is not honoured by
    // wgpu's Dawn backend: Dawn reads the seeded `[1,1,1]` indirect args
    // instead of the post-prepare write, so `compute_group_bounds` dispatches
    // exactly 1 workgroup per round, the chunk-AADF acceleration never
    // builds, and rays step chunk-by-chunk (120 × 16 = ~1920 voxels ≈ 30 %
    // of the 4096-voxel world). Workaround: dispatch DIRECTLY with the
    // `max_group_bound_dispatch` workgroup count (clamped to a wasm-friendly
    // value via [`From<&AppArgs> for ConstructionConfig`] so the bail-out
    // cost on the shader's existing `is_group_active = group_id.x < count`
    // early-exit stays sub-ms in steady state). On native this is `None` →
    // unchanged indirect dispatch (the native path's barrier works).
    #[cfg(target_arch = "wasm32")]
    let compute_workgroups_override = Some(construction_config.max_group_bound_dispatch);
    #[cfg(not(target_arch = "wasm32"))]
    let compute_workgroups_override: Option<u32> = None;

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();

    // 2026-05-20 dispatch-2 iter-2-4 — on wasm, between rounds insert a
    // copy_buffer_to_buffer(chunks → scratch). This forces Dawn to emit a
    // SHADER_WRITE→TRANSFER_READ pipeline barrier on the FULL chunks buffer.
    // Retained in dispatch-3 iter-3-1 because iter-2-4 produced the highest
    // single SSIM (0.810) of any intervention; removing risks regressing
    // baseline.
    //
    // Allocate scratch lazily: same-size + COPY_DST + COPY_SRC. Single
    // allocation for the W3 node's lifetime (Local<Option<Buffer>>).
    #[cfg(target_arch = "wasm32")]
    let chunks_self_copy_dst: Option<bevy::render::render_resource::Buffer> = {
        if let Some(world_gpu) = world_gpu.as_ref() {
            let chunks_size = world_gpu.chunks_buffer.size();
            if chunks_scratch.is_none() {
                let scratch = render_device.create_buffer(
                    &bevy::render::render_resource::BufferDescriptor {
                        label: Some("naadf_chunks_self_copy_scratch_iter_2_4"),
                        size: chunks_size,
                        usage: bevy::render::render_resource::BufferUsages::COPY_DST
                            | bevy::render::render_resource::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    },
                );
                *chunks_scratch = Some(scratch);
                bevy::log::info!(
                    "[aadf-probe] iter-2-4: allocated chunks_self_copy_scratch size={} B",
                    chunks_size,
                );
            }
            chunks_scratch.as_ref().cloned()
        } else {
            None
        }
    };

    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, BOUNDS_COMPUTE_SPAN);

    // Per-round dispatch with chunks-self-copy between rounds (wasm) — the
    // dispatch-2 iter-2-4 state. This iteration's iter-3-1 cheap-fence probe
    // (dedicated W3 encoder + on_submitted_work_done + map_async fence) was
    // REFUTED — same 0.69-0.79 SSIM cluster. Reverted to iter-2-4 baseline.
    // The chunks-self-copy retained because iter-2-4 produced 0.810 (highest
    // single SSIM of any intervention) and removing it risks regressing.
    #[cfg(target_arch = "wasm32")]
    {
        let chunks_buf_opt = world_gpu.as_ref().map(|w| w.chunks_buffer.clone());
        for round_idx in 0..n_rounds {
            // prepare + compute pass for this round.
            dispatch_regime_2_rounds(
                encoder,
                prepare_pipeline,
                compute_pipeline,
                bounds_world_bg,
                bounds_bg,
                dispatch_bg,
                probe_bg,
                indirect_buffer,
                1, // one round at a time
                compute_workgroups_override,
            );
            // After each compute pass (except the last), copy chunks→scratch→
            // chunks. The src-buffer transition COMPUTE_SHADER_WRITE →
            // TRANSFER_READ forces a full-buffer flush of compute's writes.
            if round_idx + 1 < n_rounds {
                if let (Some(chunks), Some(scratch)) =
                    (chunks_buf_opt.as_ref(), chunks_self_copy_dst.as_ref())
                {
                    let copy_size = chunks.size().min(scratch.size());
                    encoder.copy_buffer_to_buffer(chunks, 0, scratch, 0, copy_size);
                    encoder.copy_buffer_to_buffer(scratch, 0, chunks, 0, copy_size);
                }
            }
        }
        // Suppress the unused `end_of_encoder_noop_pipeline` warning.
        let _ = end_of_encoder_noop_pipeline;
    }

    #[cfg(not(target_arch = "wasm32"))]
    dispatch_regime_2_rounds(
        encoder,
        prepare_pipeline,
        compute_pipeline,
        bounds_world_bg,
        bounds_bg,
        dispatch_bg,
        probe_bg,
        indirect_buffer,
        n_rounds,
        compute_workgroups_override,
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
