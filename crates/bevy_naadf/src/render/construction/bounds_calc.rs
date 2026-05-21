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
use crate::render::pipelines::NaadfPipelines;

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
                // chunks_mirror — read-only mirror of `chunks` (binding 0). All
                // `compute_group_bounds` reads of chunk-AADF state — own-chunk at
                // `bounds_calc.wgsl:523` AND neighbour-chunk at `:273` — come from this
                // mirror; the only writer of chunk-AADF state is the non-atomic rw view
                // of `chunks` at `:564`. The mirror is refreshed via
                // `copy_buffer_to_buffer(chunks → chunks_mirror)` once before round 0
                // (seeds from W5's chunk-classification bits — without the seed copy,
                // `chunk_state = cur_chunk >> 30` reads 0 and every chunk
                // false-positive-expands) and between every subsequent round (propagates
                // the prior round's writes via a TRANSFER-stage barrier — the strongest
                // cross-pass dependency wgpu offers).
                //
                // Load-bearing on BOTH targets. Both native and wasm run the same code
                // path; only `n_bounds_rounds` differs (5 native, 1 wasm — the wasm
                // clamp lives in `config.rs::From<&AppArgs>`). On native the mirror is
                // also load-bearing for cross-workgroup neighbour visibility within a
                // single round (the indirect dispatch's workgroups read each other's
                // pre-round chunk state via the mirror; intra-round writes to `chunks`
                // are not visible until the next mirror refresh).
                bevy::render::render_resource::binding_types::storage_buffer_read_only_sized(false, None),
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

/// Queue the `prepare_group_bounds` pipeline. Binds groups 0–2 (writes to
/// `bound_refined_info` in `@group(1)` and `bound_dispatch_indirect` in
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

/// Issue `copy_buffer_to_buffer(chunks → chunks_mirror)` with a
/// `min(src.size(), dst.size())` clamp. Returns silently if either
/// buffer is unavailable. Called once before round 0 (seed) and once
/// between every subsequent round (propagate prior round's writes).
fn refresh_chunks_mirror(
    encoder: &mut CommandEncoder,
    chunks: Option<&bevy::render::render_resource::Buffer>,
    chunks_mirror: Option<&bevy::render::render_resource::Buffer>,
) {
    if let (Some(src), Some(dst)) = (chunks, chunks_mirror) {
        let copy_size = src.size().min(dst.size());
        encoder.copy_buffer_to_buffer(src, 0, dst, 0, copy_size);
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
///
/// **wasm regime-2 mechanism (load-bearing facts):**
///
/// - `n_bounds_rounds = 5` on native, **clamped to `1` on wasm** at
///   `config.rs::From<&AppArgs>`.
/// - `chunks_mirror` is a RO mirror of `chunks`, refreshed via
///   `copy_buffer_to_buffer(chunks, chunks_mirror, full_size)` **once before
///   round 0** (seeds from W5's chunk-classification bits; omitting this would
///   zero the chunk-state read, causing false-positive AADF expansion on every
///   chunk) **and between every subsequent round**.
/// - `compute_group_bounds` reads ALL chunk-AADF state (own + neighbour) from
///   `chunks_mirror` and writes back to `chunks` (non-atomic rw view). **Both
///   targets run identical code; only `n_bounds_rounds` differs.**
/// - The direct-dispatch override (`compute_workgroups_override = Some(...)`)
///   on wasm is **orthogonal** to the chunks-RMW story — it works around
///   Dawn's broken STORAGE→INDIRECT barrier between `prepare_group_bounds`'
///   indirect-args write and `compute_group_bounds`' indirect-dispatch read.
///   Native uses `None` (indirect dispatch).
///
/// iter-history archaeology lives in
/// `docs/orchestrate/wasm-chunk-aadf-nondeterminism/` (docs 12-14); source
/// describes current behaviour only.
pub fn naadf_bounds_compute_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Option<Res<NaadfPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_gpu: Option<Res<ConstructionGpu>>,
    construction_config: Option<Res<ConstructionConfig>>,
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
) {
    let Some(pipelines) = pipelines else { return; };
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
        pipeline_cache.get_compute_pipeline(pipelines.bounds_calc_pipeline_prepare),
        pipeline_cache.get_compute_pipeline(pipelines.bounds_calc_pipeline_compute),
    ) else {
        return;
    };

    let n_rounds = construction_config.n_bounds_rounds.max(1);

    // [aadf-probe] one-shot config log — verifies the wasm clamp on
    // `max_group_bound_dispatch` actually flows here from
    // `From<&AppArgs> for ConstructionConfig`.
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

    // Direct-dispatch override on wasm: bypass the Dawn STORAGE→INDIRECT
    // barrier bug (see function docblock).
    #[cfg(target_arch = "wasm32")]
    let compute_workgroups_override = Some(construction_config.max_group_bound_dispatch);
    #[cfg(not(target_arch = "wasm32"))]
    let compute_workgroups_override: Option<u32> = None;

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();

    let chunks_buf_opt = world_gpu
        .as_ref()
        .map(|w| w.chunks_buffer.clone());
    let chunks_mirror_buf_opt = construction_gpu.chunks_mirror_buffer.clone();

    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, BOUNDS_COMPUTE_SPAN);

    // Seed: mirror must reflect W5's chunk-classification bits before round 0
    // (otherwise `chunk_state = cur_chunk >> 30` reads 0 and false-positive
    // expands every chunk). See function docblock.
    refresh_chunks_mirror(
        encoder,
        chunks_buf_opt.as_ref(),
        chunks_mirror_buf_opt.as_ref(),
    );

    for round_idx in 0..n_rounds {
        dispatch_regime_2_rounds(
            encoder,
            prepare_pipeline,
            compute_pipeline,
            bounds_world_bg,
            bounds_bg,
            dispatch_bg,
            indirect_buffer,
            1,
            compute_workgroups_override,
        );
        // Propagate this round's writes to the mirror so the next round's
        // reads see them via the TRANSFER-stage barrier.
        if round_idx + 1 < n_rounds {
            refresh_chunks_mirror(
                encoder,
                chunks_buf_opt.as_ref(),
                chunks_mirror_buf_opt.as_ref(),
            );
        }
    }

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
