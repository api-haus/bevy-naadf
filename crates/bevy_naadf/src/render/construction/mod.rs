//! Phase-C construction sub-module — the **empty extension seam** that every
//! Phase-C workstream (W1..W5) extends.
//!
//! Owns the render-world resources, the `Render`-schedule prepare system, the
//! `Startup`-schedule one-shot driver, and the Bevy `Plugin` that wires them.
//! W0 ships the skeleton; W1..W5 each add their own pipelines / buffers /
//! shaders behind the same seam (`15-design-c.md` §1.1, §1.2, §1.4, §3).
//!
//! ## What W0 lands
//!
//! 1. [`ConstructionGpu`] — the render-world `Resource` holding every
//!    Phase-C buffer family. **Every field is `Option<Buffer>` / `Option<…>`
//!    initialised to `None`** so each workstream owns the allocation of its
//!    family (W1: hash-map + segment voxel; W2: change family; W3: bound-queue
//!    family; W4: entity family). W0 inserts the empty resource shell only.
//! 2. [`ConstructionBindGroups`] — the parallel render-world `Resource`
//!    holding the construction-mode bind groups. Every field is
//!    `Option<BindGroup>` / `None`; workstreams allocate when their pipelines
//!    land. W0 inserts the empty shell only.
//! 3. [`ConstructionPipelines`] — the empty sibling of `NaadfPipelines`
//!    (`render/pipelines.rs`). W0 leaves it field-less; W1..W5 add their
//!    pipeline ID + layout fields. **`NaadfPipelines` is intentionally not
//!    edited** — the construction pipelines live in their own resource so
//!    every Phase-C workstream touches the same sibling instead of fighting
//!    over the shared `NaadfPipelines` struct.
//! 4. [`prepare_construction`] — the empty `Render`-schedule system in the
//!    `PrepareResources` set. W0's body just `init_resource`-s the two
//!    construction resources if they are missing; W1..W5 fill it with their
//!    allocate/resize/build-bind-group logic. Mirrors the
//!    `prepare_world_gpu` / `prepare_taa` pattern.
//! 5. [`run_gpu_construction_startup`] — the empty `Startup`-schedule
//!    one-shot driver. W0's body is a single `info!` placeholder when the
//!    config flag is on; W1 fills it with regime-1 dispatches
//!    (generator → chunk_calc → bounds_init) + the bit-exact CPU/GPU oracle.
//! 6. [`ConstructionPlugin`] — the wiring `Plugin`. **W0 does NOT insert
//!    construction nodes into the `Core3d` chain**; the three placeholder
//!    nodes (`naadf_bounds_compute_node` / `naadf_world_change_node` /
//!    `naadf_entity_update_node`) are left as commented TODOs in
//!    `render/mod.rs` — each workstream merges its node in its own PR.
//!
//! ## What W0 explicitly does NOT do
//!
//! - **No bind-group layouts.** Layouts are added per-workstream when their
//!   shaders need them (§1.3 — every workstream owns layouts only for its
//!   own bindings).
//! - **No buffer allocations.** Each workstream allocates only its own
//!   family. The `Option<>`-wrapped fields keep the resource a valid `None`
//!   shell until a workstream lands.
//! - **No WGSL shaders.** Every shader is greenfield in its workstream
//!   (W1: `chunk_calc.wgsl` + `map_copy.wgsl`; W2: `world_change.wgsl`;
//!   W3: `bounds_calc.wgsl`; W4: `entity_update.wgsl`; W5:
//!   `generator_model.wgsl`).
//! - **No edits to `NaadfPipelines`.** All construction pipelines live on
//!   `ConstructionPipelines`. The seam contract bars touching the shared
//!   `NaadfPipelines` from Phase-C workstreams (`15-design-c.md` §1.3).
//!
//! See `15-design-c.md` §1, §2.1 W0 row, and §3 for the full seam contract.

pub mod bounds_calc;
pub mod change_handler;
pub mod chunk_calc;
pub mod config;
pub mod entity_handler;
pub mod entity_update;
pub mod generator_model;
pub mod hashing;
pub mod map_copy;
pub mod shader_drift_guard;
pub mod world_change;

use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, Buffer, BufferDescriptor, BufferUsages,
    CachedComputePipelineId, CommandEncoderDescriptor, PipelineCache,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{GpuResourceAppExt, Render, RenderApp, RenderSystems};

pub use config::ConstructionConfig;

/// The render-world `Resource` holding every Phase-C buffer family
/// (`15-design-c.md` §1.4).
///
/// **W0 — empty shell.** Every buffer field is `Option<Buffer>` initialised
/// to `None`. Each workstream populates its own family:
///
/// - **W1** (Algorithm 1): `segment_voxel_buffer`, `block_voxel_count`,
///   `hash_map`, `hash_coefficients`.
/// - **W3** (background AADF queue): `bound_queue_info`, `bound_group_queues`,
///   `bound_group_masks`, `bound_refined_info`, `bound_dispatch_indirect`.
/// - **W2** (editing): `changed_groups_dynamic`, `changed_chunks_dynamic`,
///   `changed_blocks_dynamic`, `changed_voxels_dynamic`.
/// - **W4** (entities, only when `ConstructionConfig.entities_enabled`):
///   `entity_chunk_instances`, `entity_voxel_data`, `entity_instances_history`,
///   `chunk_updates_dynamic`, `entity_chunk_instances_dynamic`,
///   `entity_history_dynamic`.
///
/// W0 declares the field set with explicit `None` initialisers so each
/// workstream lands a `Some(GrowableBuffer<T>)` / `Some(Buffer)` swap rather
/// than a struct-shape change. Later workstreams that introduce
/// `GrowableBuffer<T>` typing wrap their fields in
/// `Option<crate::world::buffer::GrowableBuffer<T>>`; W0 keeps every field as
/// `Option<Buffer>` so the empty shell compiles without pulling the family
/// types in.
#[derive(Resource, Default)]
pub struct ConstructionGpu {
    // === W1 — Algorithm 1 inputs / outputs (`chunkCalc.fx` family) ===========
    /// `segmentVoxelBuffer` (`chunkCalc.fx:38`, `WorldData.cs:73`) —
    /// `segmentSizeInChunks^3 * 2048` u32s. W1 owns the allocation.
    pub segment_voxel_buffer: Option<Buffer>,
    /// `blockVoxelCount` (`chunkCalc.fx:37`) — 2 × u32 atomic cursors. W1.
    pub block_voxel_count: Option<Buffer>,
    /// `hashMap` (`chunkCalc.fx:39`, `mapCopy.fx:13`) — open-addressing slot
    /// array, doubled by `mapCopy.fx` (`BlockHashingHandler` /
    /// `ConstructionConfig.wanted_empty_ratio`). W1 owns allocation + growth.
    pub hash_map: Option<Buffer>,
    /// `hashCoefficients` (`BlockHashingHandler.cs:50-55`) — 65 × u32 fixed
    /// table of `31^(64-i)` values. Never grows. W1.
    pub hash_coefficients: Option<Buffer>,

    // === W3 — Bound-queue family (`boundsCalc.fx` family) ===================
    /// `boundQueueInfo` (`WorldBoundHandler.cs:44`) — 32*3 × BoundQueueInfo.
    /// Fixed-size. W3.
    pub bound_queue_info: Option<Buffer>,
    /// `boundGroupQueues` (`WorldBoundHandler.cs:46`) — `32*3*boundGroupCount`
    /// u32s. Fixed-size for the test grid. W3.
    pub bound_group_queues: Option<Buffer>,
    /// `boundGroupMasks` (`WorldBoundHandler.cs:47`) — `boundGroupCount` ×
    /// 3 × `atomic<u32>` per-axis masks. W3.
    pub bound_group_masks: Option<Buffer>,
    /// `boundRefinedInfo` (`WorldBoundHandler.cs:45`) — 3 × u32. W3.
    pub bound_refined_info: Option<Buffer>,
    /// `boundDispatchIndirect` (`WorldBoundHandler.cs:49`) — 5 × u32 INDIRECT
    /// args. The wgpu `STORAGE_READ_WRITE` × `INDIRECT` split lives on this
    /// buffer (`15-design-c.md` §1.3, mirrors the Phase-B Batch-4
    /// `sample_refine_dispatch_layout` fix). W3.
    pub bound_dispatch_indirect: Option<Buffer>,
    /// W3 — `GpuConstructionParams` uniform written once at startup with the
    /// fixed-for-the-world `size_in_chunks` / `group_size_in_groups` /
    /// `bound_group_queue_max_size` / `max_group_bound_dispatch`. Bound at
    /// slot 1 of the `construction_bounds_world` group. (`15-design-c.md` §1.8.)
    pub bounds_params_buffer: Option<Buffer>,
    /// W3 — `true` once the `add_initial_groups_to_bound_queue` regime-1
    /// seed dispatch has run (one-shot at prepare-time, no startup driver
    /// extension required for the static test grid). Mirrors
    /// `WorldBoundHandler.Initialize`'s one-time call (`WorldBoundHandler.cs:53`).
    pub bounds_initialized: bool,

    // === W2 — Change-staging family (`worldChange.fx` family) ===============
    /// `changedGroups` (`ChangeHandler.cs:56`) — `Uint2[]` per edited 4³
    /// group. W2 owns allocation + per-frame upload.
    pub changed_groups_dynamic: Option<Buffer>,
    /// `changedChunks` (`ChangeHandler.cs:57`). W2.
    pub changed_chunks_dynamic: Option<Buffer>,
    /// `changedBlocks` (`ChangeHandler.cs:58`). W2.
    pub changed_blocks_dynamic: Option<Buffer>,
    /// `changedVoxels` (`ChangeHandler.cs:59`). W2.
    pub changed_voxels_dynamic: Option<Buffer>,

    // === W4 — Entity family (`entityUpdate.fx` family) ======================
    /// `entityChunkInstances` (`EntityHandler.cs:148`). W4.
    pub entity_chunk_instances: Option<Buffer>,
    /// `entityVoxelData` (`EntityHandler.cs:147`). W4.
    pub entity_voxel_data: Option<Buffer>,
    /// `entityInstancesHistory` (`EntityHandler.cs:149`). W4.
    pub entity_instances_history: Option<Buffer>,
    /// `chunkUpdatesDynamic` (`entityUpdate.fx:3`). W4.
    pub chunk_updates_dynamic: Option<Buffer>,
    /// `entityChunkInstancesDynamic`. W4.
    pub entity_chunk_instances_dynamic: Option<Buffer>,
    /// `entityHistoryDynamic`. W4.
    pub entity_history_dynamic: Option<Buffer>,
    /// Phase-C wave-3 — `EntityUpdateParams` uniform buffer
    /// (`entity_update.wgsl::params`). Written every frame the entity dispatch
    /// fires with the current `entity_instance_count` / `taa_index` /
    /// `update_count` / `entity_chunk_instance_count` / `max_entity_instances`.
    pub entity_update_params_buffer: Option<Buffer>,
    /// Phase-C wave-3 — `true` once the world bind group has been rebuilt to
    /// reference the *production* W4 entity buffers (not the placeholders
    /// allocated by `prepare_world_gpu`). Used so the rebuild happens once,
    /// after all W4 buffers exist + `entities_enabled = true`.
    pub world_bind_group_has_entities: bool,
    /// Phase-C followup #1 — `true` once the runtime GPU producer chain has
    /// dispatched (chunk_calc + bounds_init) against the production
    /// `WorldGpu` buffers. One-shot per startup; flipped in
    /// `prepare_construction` on the first frame `gpu_construction_enabled`
    /// is true AND every dependency (compiled pipelines, allocated bind
    /// groups) is ready.
    pub gpu_producer_has_run: bool,
    /// vox-gpu-rewrite W5.3-fix Stage 5 (D1 fix) — `true` once the post-W5
    /// GPU→CPU readback has copied `WorldGpu::{chunks,blocks,voxels}` back
    /// into the main-world `WorldData::{chunks_cpu,blocks_cpu,voxels_cpu}`.
    /// One-shot per startup; flipped in
    /// `populate_cpu_mirror_from_gpu_producer` (extract-schedule system) on
    /// the first frame after `gpu_producer_has_run` is true.
    ///
    /// Mirrors C# `WorldData.cs:158-198` where `dataChunkGpu.GetData(dataChunk)`
    /// + `CopyFromStructuredBufferLarge` populate the CPU mirror buffers from
    /// the GPU producer output. Without this readback the W5 install path's
    /// `WorldData.chunks_cpu` is empty (constructed empty in
    /// `install_vox_in_fixed_world`), so the CPU-side `ray_traversal` (used
    /// by the editor's mouse-pick) immediately returns `None` for every
    /// position — every brush misses. (Diagnostic:
    /// `docs/orchestrate/vox-gpu-rewrite/10-diagnostic-encoding-comparison.md`
    /// "Bug D1".)
    pub cpu_mirror_populated: bool,

    // === W5 — generator_model storage uploads + per-segment uniform ===========
    // vox-gpu-rewrite W5.2 — these four buffers feed the W5
    // `construction_generator_model` bind group (built in
    // `prepare_construction`). They are allocated once on the first frame
    // `ModelDataRender` is present (the W5.1 extract has fired) and reused
    // for every subsequent producer dispatch.
    /// W5 — `modelDataChunk` storage buffer (read-only by the generator;
    /// `data_chunk` from `aadf::generator::ModelData`). Allocated + uploaded
    /// once by `prepare_construction` on the first frame `ModelDataRender` is
    /// present.
    pub model_data_chunk_buffer: Option<Buffer>,
    /// W5 — `modelDataBlock` storage buffer (read-only by the generator).
    pub model_data_block_buffer: Option<Buffer>,
    /// W5 — `modelDataVoxel` storage buffer (read-only by the generator).
    pub model_data_voxel_buffer: Option<Buffer>,
    /// W5 — `GpuGeneratorModelParams` uniform (64 B, `generator_model.rs:74-93`).
    /// **One buffer, rewritten in place 512 times per producer run** — once
    /// per segment in the W5.3 segment loop via `RenderQueue::write_buffer`.
    /// (See `02-design.md` Decision: "one params buffer vs 512 buffers".)
    pub model_data_params_buffer: Option<Buffer>,

    // === web-vox-async-loading Q4 (2026-05-18) — label stash for the Q4
    //     confirmation assertion ============================================
    //
    // wgpu 27's `Buffer::label()` accessor isn't re-exported through Bevy
    // 0.19's `render_resource::Buffer` wrapper, so we stash the label
    // strings here at allocation time. Used by the debug-only assertion in
    // `populate_cpu_mirror_from_gpu_producer` that catches a regression to
    // the gate at `mod.rs:1184-1186` routing a `.vox` run through the W2
    // placeholder block instead of the `naadf_*_gpu_producer` block.
    //
    // Per Decision 1 of `01-context.md` the three flagless W2 placeholders
    // are dead code on the `.vox` production path; this label-stash +
    // assertion defends the property explicitly. Release builds skip the
    // assertion via `#[cfg(debug_assertions)]`.
    /// Label of the buffer assigned to `block_voxel_count` (set at
    /// allocation time). Used by the debug-only Q4 assertion only.
    pub block_voxel_count_label: Option<&'static str>,
    /// Label of the buffer assigned to `segment_voxel_buffer`.
    pub segment_voxel_buffer_label: Option<&'static str>,
    /// Label of the buffer assigned to `hash_map`.
    pub hash_map_label: Option<&'static str>,
    /// Label of the buffer assigned to `hash_coefficients`.
    pub hash_coefficients_label: Option<&'static str>,

    // === web-vox-async-loading Q3 (2026-05-18 follow-up) — cross-frame
    //     CPU-mirror readback state machine ====================================
    //
    // Replaces the sync `Device::poll(wait_indefinitely)` + `get_mapped_range`
    // pattern that panicked on WebGPU (`Device::poll(wait_indefinitely)` is a
    // no-op on WebGPU; `get_mapped_range` runs before `mapAsync` resolves and
    // wgpu panics with "Failed to execute 'getMappedRange' on 'GPUBuffer'").
    //
    // The state machine ticks once per `populate_cpu_mirror_from_gpu_producer`
    // call (every frame in `ExtractSchedule`). Stages cycle:
    //
    //   `NotStarted` (initial)
    //     → issue `copy_buffer_to_buffer` for the cursor buffer, call
    //       `map_async` with a `Arc<AtomicBool>` callback, `device.poll(Poll)`.
    //   `CursorPending`
    //     → each frame `device.poll(Poll)` and check the atomic; once set,
    //       read the cursor (2 u32s), size + alloc the chunks/blocks/voxels
    //       staging buffers, record + submit one encoder containing all 3
    //       `copy_buffer_to_buffer`s, call `map_async` × 3 (each with its own
    //       atomic), advance to `FullSetPending`.
    //   `FullSetPending`
    //     → each frame `device.poll(Poll)` and check all 3 atomics; once all
    //       fire, read all 3 mapped ranges, commit to `WorldData`, unmap +
    //       drop staging buffers, set `cpu_mirror_populated = true`, advance
    //       to `Done`.
    //   `Done` → no-op.
    //
    // Each non-terminal stage increments `stall_frames` per frame; if it
    // reaches `READBACK_STALL_BUDGET_FRAMES` (600 frames ≈ 10s @ 60fps) the
    // state machine emits a diagnostic via `error!` and force-advances to
    // `Done` (marking the mirror populated so it stops retrying). Per
    // `feedback-e2e-gates-must-fail-fast.md`.
    //
    // **Target-agnostic — no `#[cfg(target_arch = "wasm32")]` branch.** The
    // architect's Q3 design (`03-architecture.md` § Q3) and Decision 2
    // (web `.vox` MUST build via the GPU pathway identical to native) are
    // load-bearing.
    pub cpu_mirror_readback: CpuMirrorReadback,
}

/// Frame budget for the cross-frame CPU mirror readback state machine. If
/// the state machine does not progress past its current stage within this
/// many `populate_cpu_mirror_from_gpu_producer` ticks the system emits a
/// diagnostic and force-advances to `Done`. 600 frames ≈ 10s @ 60fps —
/// per `feedback-e2e-gates-must-fail-fast.md`.
pub const READBACK_STALL_BUDGET_FRAMES: u32 = 600;

/// State machine stage for the cross-frame CPU mirror readback (Q3).
///
/// The state machine runs once per `.vox` install (gated on
/// `gpu_producer_has_run && model_data.is_some() && !cpu_mirror_populated`).
/// Once it reaches `Done` it stays there for the lifetime of the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ReadbackStage {
    /// Initial state — gate not yet satisfied (or just satisfied this frame
    /// and the cursor copy hasn't been issued yet).
    #[default]
    NotStarted,
    /// Cursor copy + `map_async` issued; waiting for the callback's
    /// `AtomicBool` to flip.
    CursorPending,
    /// Full-set (chunks + blocks + voxels) copies + `map_async`s issued;
    /// waiting for all three callbacks' atomics to flip.
    FullSetPending,
    /// Readback complete. `cpu_mirror_populated` is true; the state machine
    /// stays here for the rest of the run.
    Done,
}

/// Cross-frame readback state — owned by `ConstructionGpu`. Aggregates the
/// stage, staging buffers, completion atomics, sizes, and stall counter.
///
/// **Target-agnostic** — works identically on native (where `Device::poll`
/// is a real non-blocking poll) and WebGPU (where `poll` is a no-op but the
/// JS `mapAsync` promise resolves on subsequent event-loop ticks). See
/// `03-architecture.md` § Q3 for the design rationale.
#[derive(Default)]
pub struct CpuMirrorReadback {
    /// Current state.
    pub stage: ReadbackStage,
    /// Cursor staging buffer (2 u32s) — populated in `NotStarted → CursorPending`.
    pub cursor_staging: Option<Buffer>,
    /// `AtomicBool` set by the `map_async` callback on the cursor buffer.
    pub cursor_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Chunks staging buffer — sized once the cursor is read.
    pub chunks_staging: Option<Buffer>,
    /// `AtomicBool` for the chunks `map_async` callback.
    pub chunks_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Blocks staging buffer.
    pub blocks_staging: Option<Buffer>,
    /// `AtomicBool` for the blocks `map_async` callback.
    pub blocks_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Voxels staging buffer.
    pub voxels_staging: Option<Buffer>,
    /// `AtomicBool` for the voxels `map_async` callback.
    pub voxels_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Cursor[0] = voxels-buffer fill in u32-pairs (×2 to get u32 count).
    pub voxels_u32_count: u64,
    /// Cursor[1] = blocks-buffer fill in u32s.
    pub blocks_u32_count: u64,
    /// Chunks staging size, computed from `world_gpu.chunks_size_in_chunks`.
    pub chunks_pair_count_u32: u64,
    /// Frames spent in the current non-terminal stage. Reset when the stage
    /// advances. If it exceeds `READBACK_STALL_BUDGET_FRAMES` the state
    /// machine bails with a diagnostic.
    pub stall_frames: u32,
}

/// The render-world `Resource` holding every Phase-C construction-side bind
/// group (`15-design-c.md` §1.4).
///
/// **W0 — empty shell.** Every field is `Option<BindGroup>` / `None`. Each
/// workstream builds its own bind groups in its own merge of
/// [`prepare_construction`].
#[derive(Resource, Default)]
pub struct ConstructionBindGroups {
    /// `construction_world` — the parallel-to-`world_layout` bind group for
    /// the construction passes (chunkCalc / mapCopy / worldChange). 8-binding
    /// layout (`@group(0)` for `chunk_calc.wgsl` + `world_change.wgsl`). W1
    /// builds this when the world buffers exist.
    pub construction_world: Option<BindGroup>,
    /// `construction_bounds_world` — the W3 narrow `@group(0)` for
    /// `bounds_calc.wgsl` (chunks rw texture + params uniform only, 2
    /// bindings). Separate from `construction_world` so the W3 prepare path
    /// doesn't need W1's hash-map buffers to exist; built by `prepare_construction`
    /// once `WorldGpu` has its chunks texture (`15-design-c.md` §1.3,
    /// `16-impl-c-W3.md` decision #2).
    pub construction_bounds_world: Option<BindGroup>,
    /// `construction_bounds` — the `@group(1)` bound-queue bind group used by
    /// `boundsCalc`. W3.
    pub construction_bounds: Option<BindGroup>,
    /// `construction_change` — the `@group(1)` change-staging bind group used
    /// by `worldChange`. W2.
    pub construction_change: Option<BindGroup>,
    /// `construction_entity` — the `@group(1)` entity-track bind group used
    /// by `entityUpdate`. W4.
    pub construction_entity: Option<BindGroup>,
    /// `bound_dispatch` — the one-binding `STORAGE_READ_WRITE` layout for
    /// `bound_dispatch_indirect`'s write side, separated from the consuming
    /// indirect dispatch per the wgpu `STORAGE_READ_WRITE` × `INDIRECT` split
    /// (`15-design-c.md` §1.3). W3.
    pub bound_dispatch: Option<BindGroup>,
    /// W5 — `@group(0)` bind group for `generator_model.wgsl`'s
    /// `fill_chunk_data_with_model_data` entry point. 5 bindings:
    ///   binding 0 = `segment_voxel_buffer` (chunk_data_rw, the W1 buffer the
    ///               chunk_calc chain reads from after we write into it);
    ///   binding 1 = `model_data_chunk_buffer`;
    ///   binding 2 = `model_data_block_buffer`;
    ///   binding 3 = `model_data_voxel_buffer`;
    ///   binding 4 = `model_data_params_buffer` (rewritten per segment in
    ///               `naadf_gpu_producer_node`).
    /// Built once in `prepare_construction`; **same bind group reused for all
    /// 512 segments** (binding identities are stable; only the uniform
    /// contents rotate). vox-gpu-rewrite W5.2.
    pub construction_generator_model: Option<BindGroup>,
}

/// The sibling of `NaadfPipelines` (`render/pipelines.rs`) for Phase-C
/// construction-side pipelines + layouts (`15-design-c.md` §1.3).
///
/// W0 landed this as an empty struct with a `Default`-derived `FromWorld`.
/// **W5 adds the first real fields** — `generator_model_pipeline` +
/// `generator_model_layout` — and flips the resource to a real `FromWorld`
/// impl that queues the W5 pipeline against the W5 layout.
///
/// **Field set planned per `15-design-c.md` §1.3:**
/// - W5 (**landed**): `generator_model_pipeline`, `generator_model_layout`.
/// - W1: `chunk_calc_pipeline_*`, `map_copy_pipeline`, plus layouts
///   `construction_world_layout`.
/// - W3: `bounds_calc_pipeline_*`, plus layouts `construction_bounds_layout`,
///   `bound_dispatch_indirect_layout`.
/// - W2: `world_change_pipeline_*`, plus layout `construction_change_layout`.
/// - W4: `entity_update_pipeline_*`, plus layout `construction_entity_layout`.
///
/// The `FromWorld` impl is the SINGLE seam each later workstream extends:
/// add a field, add the corresponding layout build + pipeline queue in
/// `from_world`, register the resulting handle in the struct literal at the
/// bottom. The clone-cost of `BindGroupLayoutDescriptor` keeps the seam
/// trivial for parallel-merge — every workstream's field is an additive edit.
#[derive(Resource)]
pub struct ConstructionPipelines {
    /// W5 — `@group(0)` layout for `generator_model.wgsl`. Bind-group construction
    /// happens in the W5 unit test (or in W1's regime-1 driver once W1 lands).
    pub generator_model_layout: BindGroupLayoutDescriptor,
    /// W5 — cached compute pipeline ID for `generator_model.wgsl`'s
    /// `fill_chunk_data_with_model_data` entry point.
    pub generator_model_pipeline: CachedComputePipelineId,

    // === W1 (Algorithm 1) =====================================================
    /// W1 — `construction_world_layout` `@group(0)` shared by all three
    /// `chunk_calc.wgsl` entry points (`15-design-c.md` §1.3).
    /// 8 bindings — see `chunk_calc::construction_world_layout_descriptor`.
    pub construction_world_layout: BindGroupLayoutDescriptor,
    /// W1 — `chunk_calc.wgsl::calc_block_from_raw_data` (Algorithm 1, paper §3.2).
    pub chunk_calc_pipeline_calc_block: CachedComputePipelineId,
    /// W1 — `chunk_calc.wgsl::compute_voxel_bounds` (2-bit voxel AADFs).
    pub chunk_calc_pipeline_voxel_bounds: CachedComputePipelineId,
    /// W1 — `chunk_calc.wgsl::compute_block_bounds` (2-bit block AADFs).
    pub chunk_calc_pipeline_block_bounds: CachedComputePipelineId,
    /// W1 — `map_copy_layout` `@group(0)` for the hash-map regrow shader
    /// (`15-design-c.md` §4.4).
    pub map_copy_layout: BindGroupLayoutDescriptor,
    /// W1 — `map_copy.wgsl::copy_map` (regrow re-hash).
    pub map_copy_pipeline_copy: CachedComputePipelineId,
    /// W1 — `map_copy.wgsl::test_hash` (CPU-debug sanity probe; not in
    /// production startup).
    pub map_copy_pipeline_test: CachedComputePipelineId,

    // === W3 (Background AADF queue — `bounds_calc.wgsl`) =====================
    /// W3 — `construction_bounds_world_layout` `@group(0)` for
    /// `bounds_calc.wgsl` (chunks + params, 2 bindings — narrower than W1's
    /// 8-binding layout). See `bounds_calc::construction_bounds_world_layout_descriptor`.
    pub construction_bounds_world_layout: BindGroupLayoutDescriptor,
    /// W3 — `construction_bounds_layout` `@group(1)` for the bound-queue
    /// family (`bound_queue_info` / `bound_group_queues` / `bound_group_masks`
    /// / `bound_refined_info`, 4 bindings).
    pub construction_bounds_layout: BindGroupLayoutDescriptor,
    /// W3 — `bound_dispatch_indirect_layout` `@group(2)` for the
    /// indirect-dispatch counter write-side (1 binding). The same buffer is
    /// consumed by `dispatch_workgroups_indirect` as `INDIRECT`; the layout
    /// split mirrors Phase-B Batch-4's `sample_refine_dispatch_layout`
    /// (`15-design-c.md` §1.3 wgpu STORAGE_READ_WRITE × INDIRECT split).
    pub bound_dispatch_indirect_layout: BindGroupLayoutDescriptor,
    /// W3 — `bounds_calc.wgsl::add_initial_groups_to_bound_queue`
    /// (regime-1 one-shot seed; the W1 startup driver should call it after
    /// `compute_block_bounds`).
    pub bounds_calc_pipeline_add_initial: CachedComputePipelineId,
    /// W3 — `bounds_calc.wgsl::prepare_group_bounds` (regime-2 single-thread
    /// queue picker; writes `bound_refined_info` + `bound_dispatch_indirect`).
    pub bounds_calc_pipeline_prepare: CachedComputePipelineId,
    /// W3 — `bounds_calc.wgsl::compute_group_bounds` (regime-2 4³-workgroup
    /// per-chunk AADF expander; dispatched indirect off
    /// `bound_dispatch_indirect`).
    pub bounds_calc_pipeline_compute: CachedComputePipelineId,

    // === W4 (Entity track) ====================================================
    /// W4 — `entity_world_layout` `@group(0)` (chunks_rw `Rg32Uint` + params).
    pub entity_world_layout: BindGroupLayoutDescriptor,
    /// W4 — `construction_entity_layout` `@group(1)` (5 entity-track bindings).
    pub construction_entity_layout: BindGroupLayoutDescriptor,
    /// W4 — `entity_update.wgsl::update_chunks` pipeline.
    pub entity_update_pipeline_update_chunks: CachedComputePipelineId,
    /// W4 — `entity_update.wgsl::copy_entity_chunk_instances` pipeline.
    pub entity_update_pipeline_copy_entity_chunk_instances: CachedComputePipelineId,
    /// W4 — `entity_update.wgsl::copy_entity_history` pipeline.
    pub entity_update_pipeline_copy_entity_history: CachedComputePipelineId,

    // === W2 (Editing — `world_change.wgsl`) ===================================
    /// W2 — `construction_change_layout` `@group(1)` (4 read-only change-staging
    /// bindings).
    pub construction_change_layout: BindGroupLayoutDescriptor,
    /// W2 — `world_change.wgsl::apply_group_change`.
    pub world_change_pipeline_apply_group_change: CachedComputePipelineId,
    /// W2 — `world_change.wgsl::apply_chunk_change`.
    pub world_change_pipeline_apply_chunk_change: CachedComputePipelineId,
    /// W2 — `world_change.wgsl::apply_block_change`.
    pub world_change_pipeline_apply_block_change: CachedComputePipelineId,
    /// W2 — `world_change.wgsl::apply_voxel_change`.
    pub world_change_pipeline_apply_voxel_change: CachedComputePipelineId,
}

impl FromWorld for ConstructionPipelines {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>().clone();
        let pipeline_cache = world.resource::<PipelineCache>();

        // === W5 — generator_model pipeline + layout ==========================
        let generator_model_layout =
            generator_model::generator_model_layout_descriptor();
        let generator_model_pipeline = generator_model::queue_generator_model_pipeline(
            &asset_server,
            pipeline_cache,
            generator_model_layout.clone(),
        );

        // === W1 — chunk_calc pipelines + layout ==============================
        let construction_world_layout =
            chunk_calc::construction_world_layout_descriptor();
        let chunk_calc_pipeline_calc_block = chunk_calc::queue_calc_block_pipeline(
            &asset_server,
            pipeline_cache,
            construction_world_layout.clone(),
        );
        let chunk_calc_pipeline_voxel_bounds = chunk_calc::queue_voxel_bounds_pipeline(
            &asset_server,
            pipeline_cache,
            construction_world_layout.clone(),
        );
        let chunk_calc_pipeline_block_bounds = chunk_calc::queue_block_bounds_pipeline(
            &asset_server,
            pipeline_cache,
            construction_world_layout.clone(),
        );

        // === W1 — map_copy pipelines + layout ================================
        let map_copy_layout = map_copy::map_copy_layout_descriptor();
        let map_copy_pipeline_copy = map_copy::queue_copy_map_pipeline(
            &asset_server,
            pipeline_cache,
            map_copy_layout.clone(),
        );
        let map_copy_pipeline_test = map_copy::queue_test_hash_pipeline(
            &asset_server,
            pipeline_cache,
            map_copy_layout.clone(),
        );

        // === W3 — bounds_calc pipelines + 3 layouts ===========================
        let construction_bounds_world_layout =
            bounds_calc::construction_bounds_world_layout_descriptor();
        let construction_bounds_layout =
            bounds_calc::construction_bounds_layout_descriptor();
        let bound_dispatch_indirect_layout =
            bounds_calc::bound_dispatch_indirect_layout_descriptor();
        let bounds_calc_pipeline_add_initial = bounds_calc::queue_add_initial_pipeline(
            &asset_server,
            pipeline_cache,
            construction_bounds_world_layout.clone(),
            construction_bounds_layout.clone(),
        );
        let bounds_calc_pipeline_prepare = bounds_calc::queue_prepare_pipeline(
            &asset_server,
            pipeline_cache,
            construction_bounds_world_layout.clone(),
            construction_bounds_layout.clone(),
            bound_dispatch_indirect_layout.clone(),
        );
        let bounds_calc_pipeline_compute = bounds_calc::queue_compute_pipeline(
            &asset_server,
            pipeline_cache,
            construction_bounds_world_layout.clone(),
            construction_bounds_layout.clone(),
        );

        // === W4 — entity_update pipelines + layouts ==========================
        let entity_world_layout = entity_update::entity_world_layout_descriptor();
        let construction_entity_layout =
            entity_update::construction_entity_layout_descriptor();
        let entity_update_pipeline_update_chunks =
            entity_update::queue_update_chunks_pipeline(
                &asset_server,
                pipeline_cache,
                entity_world_layout.clone(),
                construction_entity_layout.clone(),
            );
        let entity_update_pipeline_copy_entity_chunk_instances =
            entity_update::queue_copy_entity_chunk_instances_pipeline(
                &asset_server,
                pipeline_cache,
                entity_world_layout.clone(),
                construction_entity_layout.clone(),
            );
        let entity_update_pipeline_copy_entity_history =
            entity_update::queue_copy_entity_history_pipeline(
                &asset_server,
                pipeline_cache,
                entity_world_layout.clone(),
                construction_entity_layout.clone(),
            );

        // === W2 — world_change pipelines + layout ============================
        let construction_change_layout =
            world_change::construction_change_layout_descriptor();
        let world_change_pipeline_apply_group_change =
            world_change::queue_apply_group_change_pipeline(
                &asset_server,
                pipeline_cache,
                construction_world_layout.clone(),
                construction_change_layout.clone(),
                construction_bounds_layout.clone(),
            );
        let world_change_pipeline_apply_chunk_change =
            world_change::queue_apply_chunk_change_pipeline(
                &asset_server,
                pipeline_cache,
                construction_world_layout.clone(),
                construction_change_layout.clone(),
                construction_bounds_layout.clone(),
            );
        let world_change_pipeline_apply_block_change =
            world_change::queue_apply_block_change_pipeline(
                &asset_server,
                pipeline_cache,
                construction_world_layout.clone(),
                construction_change_layout.clone(),
                construction_bounds_layout.clone(),
            );
        let world_change_pipeline_apply_voxel_change =
            world_change::queue_apply_voxel_change_pipeline(
                &asset_server,
                pipeline_cache,
                construction_world_layout.clone(),
                construction_change_layout.clone(),
                construction_bounds_layout.clone(),
            );

        Self {
            generator_model_layout,
            generator_model_pipeline,
            construction_world_layout,
            chunk_calc_pipeline_calc_block,
            chunk_calc_pipeline_voxel_bounds,
            chunk_calc_pipeline_block_bounds,
            map_copy_layout,
            map_copy_pipeline_copy,
            map_copy_pipeline_test,
            construction_bounds_world_layout,
            construction_bounds_layout,
            bound_dispatch_indirect_layout,
            bounds_calc_pipeline_add_initial,
            bounds_calc_pipeline_prepare,
            bounds_calc_pipeline_compute,
            entity_world_layout,
            construction_entity_layout,
            entity_update_pipeline_update_chunks,
            entity_update_pipeline_copy_entity_chunk_instances,
            entity_update_pipeline_copy_entity_history,
            construction_change_layout,
            world_change_pipeline_apply_group_change,
            world_change_pipeline_apply_chunk_change,
            world_change_pipeline_apply_block_change,
            world_change_pipeline_apply_voxel_change,
        }
    }
}

/// Phase-C W2 — render-world resource mirroring the per-frame edit state from
/// the main world's [`crate::world::data::WorldData::pending_edits`]
/// (`15-design-c.md` §1.2 regime-3, §2.1 W2; `16-impl-c-W2.md`).
///
/// Populated by [`extract_world_changes`] in `ExtractSchedule`; consumed by
/// [`world_change::naadf_world_change_node`] in the regime-3 dispatch path.
/// Cleared at the start of every extract (drain semantics — every frame is a
/// fresh batch).
///
/// `has_pending_changes()` returns `true` if any of the 4 counts is non-zero;
/// the regime-3 node uses it as the cheap fast-path gate.
#[derive(Resource, Debug, Default)]
pub struct ConstructionEvents {
    /// Number of edited chunks this frame (drives `apply_chunk_change` dispatch).
    pub changed_chunk_count: u32,
    /// Number of edited blocks this frame (drives `apply_block_change`).
    pub changed_block_count: u32,
    /// Number of edited voxels this frame (drives `apply_voxel_change`).
    pub changed_voxel_count: u32,
    /// Number of flood-fill groups this frame (drives `apply_group_change`).
    pub changed_group_count: u32,
    /// CPU-staged `changed_chunks_dynamic` payload, drained into the upload
    /// buffer in `prepare_construction`.
    pub changed_chunks: Vec<[u32; 2]>,
    /// CPU-staged `changed_blocks_dynamic` payload (65 u32s per edit).
    pub changed_blocks: Vec<u32>,
    /// CPU-staged `changed_voxels_dynamic` payload (33 u32s per edit).
    pub changed_voxels: Vec<u32>,
    /// CPU-staged `changed_groups_dynamic` payload (`[group_pos_packed,
    /// distance]` per group).
    pub changed_groups: Vec<[u32; 2]>,
    /// Phase-C wave-3 — W4 per-frame entity-update uploads (mirrors
    /// `EntityHandler::update`'s output).
    ///
    /// Populated by [`extract_world_changes`] when the main world has an
    /// `EntityHandler` resource + a non-empty entity list. The render-side
    /// `prepare_construction` uploads these into the dynamic GPU buffers; the
    /// `naadf_entity_update_node` dispatches the 3 `entity_update.wgsl` entry
    /// points to fold them into the production `entity_chunk_instances` /
    /// `entity_instances_history` buffers + the chunks texture's `.y` channel.
    pub entity_uploads: entity_handler::EntityUpdateUploads,
    /// W4 — current TAA ring index for the entity history slot
    /// (`entityUpdate.fx:39` `taaIndex * 16384` stride). Mirrored from the
    /// renderer's TAA state at extract time.
    pub entity_taa_index: u32,
    /// Phase-C wave-3 — per-entity AADF voxel-volume data (`EntityData` from
    /// `aadf::entity::EntityData::from_types`), one entity's 64-u32 volume
    /// per entry. Uploaded to the GPU `entity_voxel_data` buffer when
    /// `entity_voxel_data_dirty` is set. Empty on no-entities frames.
    pub entity_voxel_data: Vec<u32>,
    /// Phase-C wave-3 — `true` when `entity_voxel_data` should be re-uploaded.
    /// Set on first-frame init / when the entity type set changes.
    pub entity_voxel_data_dirty: bool,
}

impl ConstructionEvents {
    /// Regime-3 fast-path predicate — `true` iff there is at least one edit to
    /// dispatch. Used by `naadf_world_change_node` to early-return on no-edit
    /// frames within microseconds.
    pub fn has_pending_changes(&self) -> bool {
        self.changed_chunk_count > 0
            || self.changed_block_count > 0
            || self.changed_voxel_count > 0
            || self.changed_group_count > 0
    }

    /// Phase-C wave-3 — W4 fast-path predicate: `true` iff the entity track
    /// has uploads to dispatch this frame. Used by `naadf_entity_update_node`
    /// as the cheap fast-path gate (matches `has_pending_changes` discipline).
    pub fn has_entity_updates(&self) -> bool {
        !self.entity_uploads.chunk_updates.is_empty()
            || !self.entity_uploads.entity_chunk_instances.is_empty()
            || !self.entity_uploads.entity_history.is_empty()
    }
}

/// **Deprecated (`02f-followup`).** The post-`02f` rearch landed this as a
/// main-world `Last` system, but the standard Bevy schedule order runs
/// `Last` BEFORE the render sub-app's `ExtractSchedule` (whether or not
/// pipelined rendering is on). That cleared `WorldData::pending_edits` on
/// the same frame the brush appended to it, BEFORE [`extract_world_changes`]
/// could read it — so the W2 GPU dispatch never saw the edit. The
/// `--runtime-edit-mode` gate didn't surface this because it inspects
/// `WorldData` in-process without driving the schedule.
///
/// **The drain now lives inside [`extract_world_changes`] itself**, via
/// `ResMut<MainWorld>` access — the Bevy-sanctioned pattern for a render
/// system to mutate a main-world resource. This co-locates produce + consume,
/// eliminating the schedule race. See `02f-followup` doc.
///
/// This function is kept (now a no-op stub) so the system registration in
/// `ConstructionPlugin::build` need not be ripped out in this dispatch; the
/// orchestrator's follow-up may delete the registration entirely.
pub fn clear_world_data_pending_edits(_world_data: Option<ResMut<crate::world::data::WorldData>>) {
    // No-op — drain moved into `extract_world_changes` (see doc above).
}

/// Phase-C wave-3 — main-world resource holding the live entity list + the
/// `EntityHandler` state (`15-design-c.md` §3.6, `16-impl-c-W4.md` integration
/// notes).
///
/// **Optional**: the resource is absent on the no-entities path (the normal
/// e2e / baseline render). When present, the per-frame extract calls
/// `EntityHandler::update(&self.instances)` and forwards the resulting
/// `EntityUpdateUploads` to the render-world `ConstructionEvents`. The
/// renderer-side dispatch chain (W4 `entity_update.wgsl` 3 entry points + the
/// `shoot_ray` entity sub-traversal) fires automatically once the bind groups
/// are built.
///
/// The `--entities` e2e mode inserts one entity here so the rendered
/// framebuffer carries the entity hit on top of the world geometry.
#[derive(Resource, Default)]
pub struct MainWorldEntities {
    /// Live entity instances for this frame.
    pub instances: Vec<crate::render::gpu_types::EntityInstance>,
    /// Per-entity-id voxel-volume builds (`EntityData` from
    /// `aadf::entity::EntityData::from_types`). Index = entity id; each
    /// entry's 64 u32s is concatenated into the `entity_voxel_data` GPU
    /// buffer in upload order. The render path consumes this through
    /// `EntityInstance::voxel_start` (the C# pre-computes the offset; for
    /// the test fixture all entities are the same and `voxel_start = 0`).
    pub voxel_data: Vec<u32>,
    /// Generation counter — bumped whenever `voxel_data` changes. The
    /// render-world extract sees the change via the `Last`-set value vs the
    /// stored render-side mirror and triggers re-upload.
    pub voxel_data_generation: u32,
}

/// Phase-C wave-3 — render-world resource holding the W4 `EntityHandler`
/// state (across-frame: per-chunk entity-count u32 table + last-frame
/// overlapped chunks list). Lives in the render world so the `ExtractSchedule`
/// system can `&mut` it (Bevy's `Extract<>` only supports read-only main-world
/// access; render-world state goes through `Res` / `ResMut`).
///
/// Updated each frame by [`extract_world_changes`]: reads main-world
/// `MainWorldEntities::instances` and calls `handler.update(instances)` to
/// produce the per-frame uploads. The previous-tracked voxel-data generation
/// is also stored here so the extract can decide whether to copy the
/// (potentially large) voxel-data buffer.
#[derive(Resource)]
pub struct RenderWorldEntityState {
    pub handler: Option<entity_handler::EntityHandler>,
    pub last_uploaded_voxel_data_generation: u32,
}

impl Default for RenderWorldEntityState {
    fn default() -> Self {
        Self {
            handler: None,
            // Distinct from MainWorldEntities default (0) so the first frame
            // with any non-empty voxel_data triggers an upload.
            last_uploaded_voxel_data_generation: u32::MAX,
        }
    }
}

/// `ExtractSchedule` system: mirror the main-world [`crate::world::data::WorldData::pending_edits`]
/// into the render-world [`ConstructionEvents`] resource.
///
/// Drains the main-world `pending_edits.batches` + `pending_edits.edited_groups`
/// each frame: aggregates the per-batch `changed_*` arrays into the render-world
/// resource and runs the CPU flood-fill via
/// [`change_handler::compute_change_groups`] to produce `changed_groups`.
///
/// Phase-C wave-3 — also reads the optional [`MainWorldEntities`] and folds
/// the per-frame `EntityHandler::update` result into
/// [`ConstructionEvents::entity_uploads`].
pub fn extract_world_changes(
    mut commands: Commands,
    main_world: ResMut<bevy::render::MainWorld>,
    entity_state: Option<ResMut<RenderWorldEntityState>>,
) {
    // `02f-followup` — pull `WorldData` mutably from the main world via the
    // `ResMut<MainWorld>` pattern. The previous `Extract<Res<WorldData>>`
    // read-only path coexisted with a separate `clear_world_data_pending_edits`
    // system in main-world `Last`, but `Last` runs BEFORE the render sub-app's
    // ExtractSchedule in the standard Bevy schedule order (both with and
    // without pipelined rendering — see bevy_render-0.19's
    // `pipelined_rendering.rs` lines 75-92 schedule diagram). So the clear
    // raced ahead of the extract, the queue was empty by the time this system
    // ran, and the W2 GPU dispatch never fired on user-driven edits. Mutating
    // main-world from `ExtractSchedule` via `MainWorld` is the
    // Bevy-sanctioned pattern (`bevy::render::MainWorld` doc); it folds the
    // drain into the consume site, eliminating the race.
    let main_world: &mut bevy::ecs::world::World = &mut **main_world.into_inner();

    // Read `MainWorldEntities` (optional, read-only) before we take a mut
    // borrow on `WorldData`. Clone the small struct so we can drop the
    // borrow.
    let main_world_entities: Option<(
        Vec<crate::render::gpu_types::EntityInstance>,
        Vec<u32>,
        u32,
    )> = main_world
        .get_resource::<MainWorldEntities>()
        .map(|me| (me.instances.clone(), me.voxel_data.clone(), me.voxel_data_generation));

    // Now take the mutable WorldData borrow + drain.
    let Some(mut world_data) = main_world.get_resource_mut::<crate::world::data::WorldData>() else {
        commands.insert_resource(ConstructionEvents::default());
        return;
    };

    let mut events = ConstructionEvents::default();
    // Drain every batch's per-buffer payload into the render-world resource.
    // The main-world `WorldData::pending_edits` accumulates per-set_voxel
    // batches; we move them out here so the next main-world tick starts
    // with an empty queue — no separate `Last`-schedule clear needed.
    let drained_batches: Vec<crate::aadf::edit::EditBatch> =
        std::mem::take(&mut world_data.pending_edits.batches);
    let drained_groups: Vec<[u32; 3]> =
        std::mem::take(&mut world_data.pending_edits.edited_groups);
    for batch in &drained_batches {
        events.changed_chunks.extend_from_slice(&batch.changed_chunks);
        events.changed_blocks.extend_from_slice(&batch.changed_blocks);
        events.changed_voxels.extend_from_slice(&batch.changed_voxels);
    }
    events.changed_chunk_count = events.changed_chunks.len() as u32;
    // Block/voxel counts = number of 65-u32 / 33-u32 records.
    events.changed_block_count = (events.changed_blocks.len() / 65) as u32;
    events.changed_voxel_count = (events.changed_voxels.len() / 33) as u32;

    // `02f-followup` — debug-log when the extract sees a non-trivial edit
    // batch. Useful for regression diagnosis (if a future change re-breaks
    // the drain, this trace surfaces it in `RUST_LOG=debug` runs). Cheap
    // when empty — the `if` guard means no log allocation on no-edit frames
    // (the steady state).
    if !drained_batches.is_empty() {
        bevy::log::debug!(
            "extract_world_changes drained: {} batches, {} changed_chunks, \
             {} changed_blocks, {} changed_voxels, {} edited_groups",
            drained_batches.len(),
            events.changed_chunk_count,
            events.changed_block_count,
            events.changed_voxel_count,
            drained_groups.len(),
        );
    }

    // CPU flood-fill — produce `changed_groups_dynamic`.
    let size_in_chunks = world_data.size_in_chunks;
    if !drained_groups.is_empty()
        && size_in_chunks.x > 0
        && size_in_chunks.y > 0
        && size_in_chunks.z > 0
    {
        let size_in_groups = [
            size_in_chunks.x / 4,
            size_in_chunks.y / 4,
            size_in_chunks.z / 4,
        ];
        // Skip the flood fill if any axis would be 0 groups (test grid sizes
        // smaller than 4 chunks have no bound groups at all — W3 layout is
        // dormant there).
        if size_in_groups[0] > 0 && size_in_groups[1] > 0 && size_in_groups[2] > 0 {
            // Dedup directly-edited groups (multiple voxel edits in the same
            // group count once).
            let mut uniq: Vec<[u32; 3]> = Vec::new();
            for &g in &drained_groups {
                if !uniq.contains(&g) {
                    uniq.push(g);
                }
            }
            let groups = change_handler::compute_change_groups(size_in_groups, &uniq);
            events.changed_group_count = groups.entries.len() as u32;
            events.changed_groups = groups.entries;
        }
    }

    // Drop the WorldData borrow so the entity handler logic can run without
    // borrow conflicts (it reads world_data size_in_chunks which we cached
    // above).
    drop(world_data);

    // === Phase-C wave-3 — entity uploads ====================================
    // When the main-world `MainWorldEntities` resource exists and carries at
    // least one instance, run `EntityHandler::update` and fold the result into
    // `ConstructionEvents.entity_uploads`. The render-side dispatch + the
    // chunks-texture `.y` write fire next frame.
    if let (Some((instances, voxel_data, voxel_data_generation)), Some(mut state)) =
        (main_world_entities, entity_state)
    {
        // Mirror voxel-data into `ConstructionEvents` whenever the generation
        // counter changes.
        if voxel_data_generation != state.last_uploaded_voxel_data_generation {
            events.entity_voxel_data = voxel_data;
            events.entity_voxel_data_dirty = true;
            state.last_uploaded_voxel_data_generation = voxel_data_generation;
        }

        if !instances.is_empty() {
            if state.handler.is_none() {
                state.handler = Some(entity_handler::EntityHandler::new([
                    size_in_chunks.x,
                    size_in_chunks.y,
                    size_in_chunks.z,
                ]));
            }
            if let Some(handler) = state.handler.as_mut() {
                events.entity_uploads = handler.update(&instances);
            }
        }
    }

    commands.insert_resource(events);
}

/// vox-gpu-rewrite W5.3-fix Stage 5 (D1 fix) — GPU→CPU readback that
/// populates the main-world `WorldData::{chunks_cpu, blocks_cpu, voxels_cpu}`
/// from the W5 GPU producer's output (`WorldGpu::chunks_buffer`,
/// `WorldGpu::blocks`, `WorldGpu::voxels`) the first frame after
/// `gpu_producer_has_run` flips true.
///
/// **Why.** `install_vox_in_fixed_world` (`voxel/grid.rs:317-429`)
/// constructs a `WorldData` with empty CPU mirror buffers — the W5 GPU
/// producer chain populates the GPU buffers, but the CPU mirror stayed
/// empty. The CPU-side `WorldData::ray_traversal` (used by the editor's
/// mouse-pick) immediately returns `None` when `chunk_idx >=
/// self.chunks_cpu.len()` (i.e., always, since `len() == 0`), so every
/// edit-mode raycast misses. This system mirrors C# `WorldData.cs:158-198`
/// (`dataChunkGpu.GetData(dataChunk)` +
/// `CopyFromStructuredBufferLarge(dataBlockGpu/dataVoxelGpu)` after the
/// segment loop) — without it the editor brush has no CPU mirror to
/// raycast against.
///
/// **Shape B** per `docs/orchestrate/vox-gpu-rewrite/10-diagnostic-encoding-comparison.md:387-413`:
/// after the GPU readback, the system also calls
/// `WorldData::seed_block_hashing()` so the CPU-side edit-time hash table
/// is in sync with the just-readback voxel buffer (matches C#'s
/// post-`GetData()` editor state).
///
/// **One-shot.** Gated on `gpu_producer_has_run = true` AND
/// `cpu_mirror_populated = false`. The readback uses `device.poll()` to
/// drive the staging-buffer map, which is synchronous and stalls the
/// extract-schedule thread for the duration of the readback. For Oasis
/// at the 256×32×256-chunk fixed-world size this is ~16 MiB chunks (×2
/// for the pair-channel) + N MiB blocks + M MiB voxels — N+M+~32 MiB
/// total — ~10-20 ms one-shot at startup. Per-frame cost after that:
/// one boolean check.
///
/// **Read sizing.** Chunks are sized to the full fixed-world extent
/// (every chunk is read, including empty ones — the renderer reads the
/// full extent too). Blocks/voxels are sized from the
/// `block_voxel_count` cursor pair (mirrors C# where
/// `dataBlock.Length` / `dataVoxel.Length` track the GPU producer's
/// cursor). The cursors include the initial-prefix bump (cursor[0]=64,
/// cursor[1]=64 at producer entry), so the readback sizes are
/// `voxels_cpu.len() = block_voxel_count[0] / 2` and
/// `blocks_cpu.len() = block_voxel_count[1]` directly.
pub fn populate_cpu_mirror_from_gpu_producer(
    main_world: ResMut<bevy::render::MainWorld>,
    mut gpu: Option<ResMut<ConstructionGpu>>,
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
    // Only run on the W5 install path — the path where the CPU mirror was
    // installed EMPTY in `install_vox_in_fixed_world`. For the legacy default
    // / sized-to-model paths the CPU mirror is built from CPU `construct()`
    // and overwriting it with the GPU output would defeat the legacy paths'
    // bit-exact CPU oracle (and would propagate any GPU producer bug into
    // the CPU mirror, breaking the editor where it currently works).
    model_data: Option<Res<crate::render::extract::ModelDataRender>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    use bevy::render::render_resource::{MapMode, PollType};
    use std::sync::atomic::Ordering;

    let Some(gpu) = gpu.as_mut() else { return; };
    if !gpu.gpu_producer_has_run || gpu.cpu_mirror_populated {
        return;
    }
    if model_data.is_none() {
        // Legacy paths: CPU mirror is already populated by CPU `construct()`
        // (see `install_default_small_world` / `install_default_embedded_in_fixed_world`
        // / `install_vox_sized_to_model`); the readback is a no-op +
        // unnecessary risk. Mark populated so we don't keep checking.
        gpu.cpu_mirror_populated = true;
        gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
        return;
    }

    // web-vox-async-loading Q4 (2026-05-18) — confirmation assertion per
    // Decision 1 of `docs/orchestrate/web-vox-async-loading/01-context.md`:
    // the three flagless W2 placeholders (`hash_map_w2_placeholder`,
    // `segment_voxel_buffer_w2_placeholder`, `hash_coefficients_w2_placeholder`)
    // MUST be dead code on the `.vox` production path. The gate at
    // `mod.rs:1184-1186` widens `want_gpu_producer` to include
    // `model_data.is_some()`, so the production `naadf_*_gpu_producer`
    // allocations fire BEFORE the W2 placeholder block — every
    // `is_none()` guard in the placeholder block then returns `false`.
    //
    // Release builds skip the check entirely.
    #[cfg(debug_assertions)]
    {
        if let Some(label) = gpu.block_voxel_count_label {
            assert!(
                !label.contains("w2_placeholder"),
                "vox-gpu-rewrite Q4 regression: block_voxel_count is the W2 \
                 placeholder on a .vox run — gate logic regression at \
                 mod.rs:1184-1186 routed past the gpu_producer allocation. \
                 Label was: {label}"
            );
        }
        if let Some(label) = gpu.hash_map_label {
            assert!(
                !label.contains("w2_placeholder"),
                "vox-gpu-rewrite Q4 regression: hash_map is the W2 placeholder \
                 on a .vox run. Label was: {label}"
            );
        }
        if let Some(label) = gpu.segment_voxel_buffer_label {
            assert!(
                !label.contains("w2_placeholder"),
                "vox-gpu-rewrite Q4 regression: segment_voxel_buffer is the W2 \
                 placeholder on a .vox run. Label was: {label}"
            );
        }
        if let Some(label) = gpu.hash_coefficients_label {
            assert!(
                !label.contains("w2_placeholder"),
                "vox-gpu-rewrite Q4 regression: hash_coefficients is the W2 \
                 placeholder on a .vox run. Label was: {label}"
            );
        }
    }

    let Some(world_gpu) = world_gpu else { return; };

    // web-vox-async-loading Q3 (follow-up dispatch 2026-05-18) — cross-frame
    // CPU-mirror readback state machine. Replaces the sync
    // `Device::poll(wait_indefinitely)` + `get_mapped_range` panic site at
    // `mod.rs:944-957` (interim wasm32 escape hatch deleted per Q7).
    //
    // Each frame in `ExtractSchedule`, tick the state machine ONCE.
    // Target-agnostic — no `#[cfg(target_arch = "wasm32")]` branch on this
    // path (Decision 2).
    let device = render_device.as_ref();
    let queue = render_queue.as_ref();

    // Helper — issue copy_buffer_to_buffer + map_async with a callback that
    // sets `done` on completion. The staging buffer is returned to the caller
    // (it stays alive on `ConstructionGpu` until we read it in a later frame).
    fn issue_copy_and_map(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &Buffer,
        u32_count: u64,
        label: &'static str,
        done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Buffer {
        let size = u32_count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("vox_gpu_rewrite_cpu_mirror_readback_enc"),
        });
        enc.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        let done_for_cb = done.clone();
        slice.map_async(MapMode::Read, move |r| {
            // Set the flag regardless of map success — the consumer
            // checks the flag, then attempts `get_mapped_range`. A failed
            // map will panic at `get_mapped_range`; we log instead.
            if r.is_err() {
                bevy::log::error!(
                    "vox-gpu-rewrite Q3 readback: map_async callback received \
                     Err — staging buffer map failed"
                );
            }
            done_for_cb.store(true, std::sync::atomic::Ordering::Release);
        });
        staging
    }

    // Drain the device's callback queue without blocking — drives `mapAsync`
    // resolutions on native, no-op on WebGPU (the JS event loop drives that
    // backend's callbacks). Called once per stage tick.
    let poll_result = device.poll(PollType::Poll);
    if poll_result.is_err() {
        bevy::log::error!(
            "vox-gpu-rewrite Q3 readback: device.poll(Poll) returned Err — \
             {:?}",
            poll_result.err()
        );
    }

    match gpu.cpu_mirror_readback.stage {
        ReadbackStage::Done => {
            // Reached terminal state from a previous frame (e.g. legacy path
            // short-circuit). Nothing to do.
        }
        ReadbackStage::NotStarted => {
            let Some(block_voxel_count_buf) = gpu.block_voxel_count.as_ref() else {
                return;
            };
            // Reset atomic, issue cursor copy + map_async.
            gpu.cpu_mirror_readback
                .cursor_done
                .store(false, Ordering::Relaxed);
            let staging = issue_copy_and_map(
                device,
                queue,
                block_voxel_count_buf,
                2,
                "vox_gpu_rewrite_cpu_mirror_readback_cursor",
                gpu.cpu_mirror_readback.cursor_done.clone(),
            );
            gpu.cpu_mirror_readback.cursor_staging = Some(staging);
            gpu.cpu_mirror_readback.stage = ReadbackStage::CursorPending;
            gpu.cpu_mirror_readback.stall_frames = 0;
            bevy::log::info!(
                "vox-gpu-rewrite Q3 readback: stage NotStarted → CursorPending \
                 (cursor copy issued + map_async dispatched)"
            );
        }
        ReadbackStage::CursorPending => {
            if !gpu.cpu_mirror_readback.cursor_done.load(Ordering::Acquire) {
                // Still waiting for the cursor map_async callback.
                gpu.cpu_mirror_readback.stall_frames += 1;
                if gpu.cpu_mirror_readback.stall_frames >= READBACK_STALL_BUDGET_FRAMES {
                    bevy::log::error!(
                        "vox-gpu-rewrite Q3 readback: STALLED at stage CursorPending \
                         after {} frames (~10s @ 60fps) — `mapAsync` callback for the \
                         cursor staging buffer never fired. Possible causes: device \
                         lost, render-graph submission stuck, wgpu callback queue \
                         starved. Forcing advance to Done to unblock subsequent \
                         frames (CPU mirror stays empty; editor pick-ray will return \
                         None for every position until a subsequent .vox install \
                         re-triggers the producer chain).",
                        READBACK_STALL_BUDGET_FRAMES
                    );
                    gpu.cpu_mirror_populated = true;
                    gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                    gpu.cpu_mirror_readback.cursor_staging = None;
                }
                return;
            }
            // Cursor mapped — read it, size the full set, issue copies.
            let cursor_staging = gpu
                .cpu_mirror_readback
                .cursor_staging
                .as_ref()
                .expect("cursor_staging missing in CursorPending stage")
                .clone();
            let cursor: Vec<u32> = {
                let slice = cursor_staging.slice(..);
                let data = slice.get_mapped_range();
                let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
                drop(data);
                cursor_staging.unmap();
                out
            };
            if cursor.len() < 2 {
                bevy::log::warn!(
                    "vox-gpu-rewrite Q3 readback: block_voxel_count read returned \
                     {} u32s; cannot determine GPU-buffer fill levels — aborting \
                     CPU mirror population (marking populated to avoid retry)",
                    cursor.len(),
                );
                gpu.cpu_mirror_populated = true;
                gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                gpu.cpu_mirror_readback.cursor_staging = None;
                return;
            }
            let voxels_u32_count = (cursor[0] / 2) as u64;
            let blocks_u32_count = cursor[1] as u64;

            let chunks_extent = world_gpu.chunks_size_in_chunks;
            let chunk_count =
                (chunks_extent.x * chunks_extent.y * chunks_extent.z) as u64;
            let chunks_pair_count_u32 = chunk_count * 2;

            gpu.cpu_mirror_readback.voxels_u32_count = voxels_u32_count;
            gpu.cpu_mirror_readback.blocks_u32_count = blocks_u32_count;
            gpu.cpu_mirror_readback.chunks_pair_count_u32 = chunks_pair_count_u32;

            // Reset all three completion atomics.
            gpu.cpu_mirror_readback
                .chunks_done
                .store(false, Ordering::Relaxed);
            gpu.cpu_mirror_readback
                .blocks_done
                .store(false, Ordering::Relaxed);
            gpu.cpu_mirror_readback
                .voxels_done
                .store(false, Ordering::Relaxed);

            // Issue chunks copy + map_async. Always non-zero (the world has
            // at least one chunk).
            let chunks_staging = issue_copy_and_map(
                device,
                queue,
                &world_gpu.chunks_buffer,
                chunks_pair_count_u32,
                "vox_gpu_rewrite_cpu_mirror_readback_chunks",
                gpu.cpu_mirror_readback.chunks_done.clone(),
            );
            gpu.cpu_mirror_readback.chunks_staging = Some(chunks_staging);

            // Blocks + voxels copies — skip if u32_count == 0 (an empty world
            // with no allocated blocks/voxels — the cursor would be at the
            // initial-prefix bump of 64 minimum, so this is mostly defensive).
            if blocks_u32_count > 0 {
                let blocks_staging = issue_copy_and_map(
                    device,
                    queue,
                    world_gpu.blocks.buffer(),
                    blocks_u32_count,
                    "vox_gpu_rewrite_cpu_mirror_readback_blocks",
                    gpu.cpu_mirror_readback.blocks_done.clone(),
                );
                gpu.cpu_mirror_readback.blocks_staging = Some(blocks_staging);
            } else {
                gpu.cpu_mirror_readback
                    .blocks_done
                    .store(true, Ordering::Release);
            }
            if voxels_u32_count > 0 {
                let voxels_staging = issue_copy_and_map(
                    device,
                    queue,
                    world_gpu.voxels.buffer(),
                    voxels_u32_count,
                    "vox_gpu_rewrite_cpu_mirror_readback_voxels",
                    gpu.cpu_mirror_readback.voxels_done.clone(),
                );
                gpu.cpu_mirror_readback.voxels_staging = Some(voxels_staging);
            } else {
                gpu.cpu_mirror_readback
                    .voxels_done
                    .store(true, Ordering::Release);
            }

            gpu.cpu_mirror_readback.cursor_staging = None;
            gpu.cpu_mirror_readback.stage = ReadbackStage::FullSetPending;
            gpu.cpu_mirror_readback.stall_frames = 0;
            bevy::log::info!(
                "vox-gpu-rewrite Q3 readback: stage CursorPending → FullSetPending \
                 (cursor read: {} voxels-u32s, {} blocks-u32s, {} chunks-pairs-u32s; \
                 chunks_extent={}×{}×{})",
                voxels_u32_count,
                blocks_u32_count,
                chunks_pair_count_u32,
                chunks_extent.x,
                chunks_extent.y,
                chunks_extent.z,
            );
        }
        ReadbackStage::FullSetPending => {
            let chunks_ready =
                gpu.cpu_mirror_readback.chunks_done.load(Ordering::Acquire);
            let blocks_ready =
                gpu.cpu_mirror_readback.blocks_done.load(Ordering::Acquire);
            let voxels_ready =
                gpu.cpu_mirror_readback.voxels_done.load(Ordering::Acquire);
            if !(chunks_ready && blocks_ready && voxels_ready) {
                gpu.cpu_mirror_readback.stall_frames += 1;
                if gpu.cpu_mirror_readback.stall_frames >= READBACK_STALL_BUDGET_FRAMES {
                    bevy::log::error!(
                        "vox-gpu-rewrite Q3 readback: STALLED at stage FullSetPending \
                         after {} frames (~10s @ 60fps). Pending: chunks={}, blocks={}, \
                         voxels={}. Possible causes: device lost, render-graph \
                         submission stuck. Forcing advance to Done (CPU mirror stays \
                         empty).",
                        READBACK_STALL_BUDGET_FRAMES,
                        !chunks_ready,
                        !blocks_ready,
                        !voxels_ready,
                    );
                    gpu.cpu_mirror_populated = true;
                    gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                    gpu.cpu_mirror_readback.chunks_staging = None;
                    gpu.cpu_mirror_readback.blocks_staging = None;
                    gpu.cpu_mirror_readback.voxels_staging = None;
                }
                return;
            }

            // All three mapped — read the contents.
            let chunks_pairs: Vec<u32> = {
                let staging = gpu
                    .cpu_mirror_readback
                    .chunks_staging
                    .as_ref()
                    .expect("chunks_staging missing in FullSetPending stage")
                    .clone();
                let slice = staging.slice(..);
                let data = slice.get_mapped_range();
                let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
                drop(data);
                staging.unmap();
                out
            };
            let blocks_cpu: Vec<u32> = match gpu.cpu_mirror_readback.blocks_staging.as_ref() {
                Some(staging) => {
                    let staging = staging.clone();
                    let slice = staging.slice(..);
                    let data = slice.get_mapped_range();
                    let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
                    drop(data);
                    staging.unmap();
                    out
                }
                None => Vec::new(),
            };
            let voxels_cpu: Vec<u32> = match gpu.cpu_mirror_readback.voxels_staging.as_ref() {
                Some(staging) => {
                    let staging = staging.clone();
                    let slice = staging.slice(..);
                    let data = slice.get_mapped_range();
                    let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
                    drop(data);
                    staging.unmap();
                    out
                }
                None => Vec::new(),
            };

            let chunks_pair_count_u32 = gpu.cpu_mirror_readback.chunks_pair_count_u32;
            if chunks_pairs.len() as u64 != chunks_pair_count_u32 {
                bevy::log::warn!(
                    "vox-gpu-rewrite Q3 readback: chunks_buffer read size mismatch \
                     (got {} u32s, expected {})",
                    chunks_pairs.len(),
                    chunks_pair_count_u32,
                );
                gpu.cpu_mirror_populated = true;
                gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                gpu.cpu_mirror_readback.chunks_staging = None;
                gpu.cpu_mirror_readback.blocks_staging = None;
                gpu.cpu_mirror_readback.voxels_staging = None;
                return;
            }
            let chunk_count = (chunks_pair_count_u32 / 2) as usize;
            let mut chunks_cpu: Vec<u32> = Vec::with_capacity(chunk_count);
            for i in 0..chunk_count {
                chunks_cpu.push(chunks_pairs[i * 2]);
            }

            let chunks_len = chunks_cpu.len();
            let blocks_len = blocks_cpu.len();
            let voxels_len = voxels_cpu.len();

            // Mutate the main-world `WorldData`.
            let main_world: &mut bevy::ecs::world::World =
                &mut **main_world.into_inner();
            let Some(mut world_data) =
                main_world.get_resource_mut::<crate::world::data::WorldData>()
            else {
                bevy::log::warn!(
                    "vox-gpu-rewrite Q3 readback: main-world WorldData not present; \
                     dropping captured CPU mirror data this frame"
                );
                gpu.cpu_mirror_populated = true;
                gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                gpu.cpu_mirror_readback.chunks_staging = None;
                gpu.cpu_mirror_readback.blocks_staging = None;
                gpu.cpu_mirror_readback.voxels_staging = None;
                return;
            };
            world_data.chunks_cpu = chunks_cpu;
            world_data.blocks_cpu = blocks_cpu;
            world_data.voxels_cpu = voxels_cpu;
            world_data.block_hashing = crate::aadf::block_hash::BlockHashingHandler::new();
            world_data.seed_block_hashing();

            // 2026-05-19 horizon-parity AADF diagnostic — sample chunks +
            // blocks at distances along the cross-target SSIM gate's
            // camera view-ray and log AADF skip-bit decode for each.
            // Native + WASM both pass through this code (same readback);
            // the Playwright spec filters `[aadf-probe]` lines from
            // console output + native stdout, persists them to disk so
            // the orchestrator can diff native vs WASM without
            // copy-pasting log tails.
            //
            // Camera (cross-target gate pose):
            //   pos     = (3880, 497, 3514) voxels
            //   forward = (-0.924, -0.241, -0.297)
            //
            // Chunk word encoding (bits 30-31 = state; bits 0-29 = AADF
            // skip-distances for empty chunks, encoded as 6 × 5-bit
            // fields = (mx, px, my, py, mz, pz)).
            {
                let chunks_cpu = &world_data.chunks_cpu;
                let blocks_cpu = &world_data.blocks_cpu;
                let voxels_cpu = &world_data.voxels_cpu;
                let scx = world_data.size_in_chunks.x as usize;
                let scy = world_data.size_in_chunks.y as usize;
                let scz = world_data.size_in_chunks.z as usize;
                bevy::log::info!(
                    "[aadf-probe] world chunks {}×{}×{} \
                     chunks_cpu.len()={} blocks_cpu.len()={} voxels_cpu.len()={}",
                    scx, scy, scz,
                    chunks_cpu.len(),
                    blocks_cpu.len(),
                    voxels_cpu.len(),
                );
                let cam = [3880.0_f32, 497.0_f32, 3514.0_f32];
                let fwd = [-0.924_f32, -0.241_f32, -0.297_f32];
                for &dist in &[0.0_f32, 500.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0] {
                    let pxw = cam[0] + fwd[0] * dist;
                    let pyw = cam[1] + fwd[1] * dist;
                    let pzw = cam[2] + fwd[2] * dist;
                    if pxw < 0.0 || pyw < 0.0 || pzw < 0.0 {
                        bevy::log::info!(
                            "[aadf-probe] dist={} pos=({:.0},{:.0},{:.0}) OUT_OF_WORLD_NEGATIVE",
                            dist as u32, pxw, pyw, pzw,
                        );
                        continue;
                    }
                    let cx = (pxw as u32) / 16;
                    let cy = (pyw as u32) / 16;
                    let cz = (pzw as u32) / 16;
                    if cx >= scx as u32 || cy >= scy as u32 || cz >= scz as u32 {
                        bevy::log::info!(
                            "[aadf-probe] dist={} pos=({:.0},{:.0},{:.0}) chunk=({},{},{}) OUT_OF_WORLD",
                            dist as u32, pxw, pyw, pzw, cx, cy, cz,
                        );
                        continue;
                    }
                    let chunk_idx =
                        cx as usize + cy as usize * scx + cz as usize * scx * scy;
                    let chunk_word = chunks_cpu[chunk_idx];
                    let state = (chunk_word >> 30) & 0x3;
                    let mxd = chunk_word & 0x1F;
                    let pxd = (chunk_word >> 5) & 0x1F;
                    let myd = (chunk_word >> 10) & 0x1F;
                    let pyd = (chunk_word >> 15) & 0x1F;
                    let mzd = (chunk_word >> 20) & 0x1F;
                    let pzd = (chunk_word >> 25) & 0x1F;
                    let state_name = match state {
                        0 => "EMPTY",
                        1 => "FULL",
                        _ => "MIXED",
                    };
                    bevy::log::info!(
                        "[aadf-probe] dist={} pos=({:.0},{:.0},{:.0}) chunk=({},{},{}) \
                         word=0x{:08x} state={} chunk_aadf=[mx={} px={} my={} py={} mz={} pz={}]",
                        dist as u32, pxw, pyw, pzw, cx, cy, cz,
                        chunk_word, state_name, mxd, pxd, myd, pyd, mzd, pzd,
                    );
                    // For mixed chunks, peek at the first block's 2-bit
                    // AADF skip-distances + state. block_base is the
                    // 30-bit pointer in bits 0-29 of chunk_word.
                    if state >= 2 {
                        let block_base = chunk_word & 0x3FFFFFFF;
                        if (block_base as usize) < blocks_cpu.len() {
                            let block_word = blocks_cpu[block_base as usize];
                            let bstate = (block_word >> 30) & 0x3;
                            let bmx = block_word & 0x3;
                            let bpx = (block_word >> 2) & 0x3;
                            let bmy = (block_word >> 4) & 0x3;
                            let bpy = (block_word >> 6) & 0x3;
                            let bmz = (block_word >> 8) & 0x3;
                            let bpz = (block_word >> 10) & 0x3;
                            bevy::log::info!(
                                "[aadf-probe]   block[{}] word=0x{:08x} state={} \
                                 block_aadf=[mx={} px={} my={} py={} mz={} pz={}]",
                                block_base, block_word, bstate,
                                bmx, bpx, bmy, bpy, bmz, bpz,
                            );
                        }
                    }
                }
                let _ = voxels_cpu;
            }
            drop(world_data);

            gpu.cpu_mirror_populated = true;
            gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
            gpu.cpu_mirror_readback.chunks_staging = None;
            gpu.cpu_mirror_readback.blocks_staging = None;
            gpu.cpu_mirror_readback.voxels_staging = None;
            bevy::log::info!(
                "vox-gpu-rewrite Q3 readback: stage FullSetPending → Done — CPU \
                 mirror populated from GPU producer output: chunks_cpu.len() = {}, \
                 blocks_cpu.len() = {}, voxels_cpu.len() = {}",
                chunks_len,
                blocks_len,
                voxels_len,
            );
        }
    }
}

/// `RenderSystems::PrepareResources` system — the empty Phase-C prepare seam.
///
/// **W0 body — ensure-exists only.** W0's responsibility is to guarantee the
/// two construction resources are present in the render world; W1..W5 fill
/// the body with their allocate/resize/build-bind-group logic.
///
/// Inserts `ConstructionGpu::default()` and `ConstructionBindGroups::default()`
/// when missing; if both exist (the steady state from frame 2 onward), the
/// system returns immediately. The `Render` schedule re-runs every frame, so
/// keep the body cheap.
///
/// Runs in `PrepareResources` alongside `prepare_world_gpu` / `prepare_taa` /
/// `prepare_atmosphere` / `prepare_gi`. No ordering constraint vs. those in
/// W0 (the empty body cannot conflict); W1..W5 add `.before(...)` /
/// `.after(...)` as their bind groups gain real `WorldGpu` /
/// `ConstructionGpu` dependencies.
// Bevy systems legitimately exceed clippy's 7-argument ceiling (same as
// `prepare_frame_gpu`'s allow in `render/prepare.rs:302`).
#[allow(clippy::too_many_arguments)]
// Allowing the wide arg list: Phase-C followup #1 added one read-only
// world-metadata parameter (originally `ExtractedWorld`, now `WorldDataMeta`
// post-`02f`) for the `dense_voxel_types` GPU upload, pushing the count past
// clippy's default ceiling. The function is a single
// `RenderSystems::PrepareResources` body — every parameter is a Res / ResMut
// it legitimately needs.
#[allow(clippy::too_many_arguments)]
pub fn prepare_construction(
    mut commands: Commands,
    gpu: Option<ResMut<ConstructionGpu>>,
    bind_groups: Option<ResMut<ConstructionBindGroups>>,
    world_gpu: Option<ResMut<crate::render::prepare::WorldGpu>>,
    construction_pipelines: Option<Res<ConstructionPipelines>>,
    construction_config: Res<config::ConstructionConfig>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    construction_events: Option<Res<ConstructionEvents>>,
    // Phase-C followup #1 — read the dense voxel-type stream so we can build
    // `segment_voxel_buffer` on the runtime GPU producer path. Empty when the
    // test scene does not author a dense volume (legacy path). `02f` rearch:
    // moved from `ExtractedWorld` (deleted) to `WorldDataMeta` (minimal
    // metadata-only render-world mirror — see `02f` Decision 4).
    world_data_meta: Option<Res<crate::render::extract::WorldDataMeta>>,
    // vox-gpu-rewrite W5.2 — render-world mirror of `ModelData`. Present only
    // after the W5.1 `stage_model_data_buildonce` extract has run (which only
    // fires when the main-world install path inserts a `ModelData`); absent
    // on default-scene + entity-only / non-VOX runs.
    model_data: Option<Res<crate::render::extract::ModelDataRender>>,
) {
    // W0 seam: ensure-exists for both resources, then W1..W5 fill in their
    // family's allocations + bind groups on subsequent frames (when the
    // dependencies — `WorldGpu`, `ConstructionPipelines` — also exist).
    if gpu.is_none() {
        commands.insert_resource(ConstructionGpu::default());
        // First frame creates the resource; the *next* frame's pass through
        // this system fills its fields (W3 bound buffers etc.) once
        // `WorldGpu` is available.
        return;
    }
    if bind_groups.is_none() {
        commands.insert_resource(ConstructionBindGroups::default());
        return;
    }

    let mut gpu = gpu.unwrap();
    let mut bind_groups = bind_groups.unwrap();
    let Some(mut world_gpu) = world_gpu else { return; };
    let Some(construction_pipelines) = construction_pipelines else { return; };

    // === Phase-C followup #1 — runtime GPU producer pre-allocation ==========
    //
    // When `gpu_construction_enabled = true` AND the producer has not yet
    // run, allocate the FULL hash_map / segment_voxel_buffer /
    // hash_coefficients / block_voxel_count buffers (not the W2 placeholders
    // that the placeholder block below would otherwise create). This ensures
    // the `construction_world` bind group built later in this function binds
    // production-sized buffers, ready for the W1 chunk_calc dispatch the
    // gpu-producer block at the bottom of this function runs.
    //
    // The CPU mirror produced by `setup_test_grid` is still available via
    // `world_data_meta.{size_in_chunks, dense_voxel_types}` — we use
    // `dense_voxel_types` to build `segment_voxel_buffer`. When the dense
    // data is absent (sparse `.vox` path / legacy code paths), the GPU
    // producer cannot run and we fall back to the CPU upload (the
    // upload-skip in `prepare_world_gpu` is reversed by a follow-up check
    // there — but in practice every code path that sets
    // `gpu_construction_enabled = true` also authors a `DenseVolume`).
    // vox-gpu-rewrite W5.3-fix Stage 1.5 (2026-05-18) — the W5 install path
    // leaves `dense_voxel_types = Vec::new()` by design (the GPU producer
    // chain consumes `ModelData` instead of a dense CPU mirror). Before this
    // gate-widening, `want_gpu_producer` therefore evaluated `false` on the
    // W5 path → the pre-allocation block below was SKIPPED → the W2
    // placeholder block at `:1644-1721` left `hash_map = 16 B` (1 slot of
    // zero) and `hash_coefficients = 4 B` (1 u32 of zero). The chunk_calc
    // shader's hash computation degenerated to identically-zero for every
    // mixed block (`chunk_coefficients[i]` OOB-reads return zero per WebGPU
    // spec) → all mixed blocks raced for `hash_map[0]` via CAS → all-but-one
    // resolved to sentinel voxel-pointer `2` → rendered as scattered empty
    // holes. Diagnostic: `docs/orchestrate/vox-gpu-rewrite/06-diagnostic-inversion.md`.
    //
    // Fix (per `06-diagnostic-inversion.md:359-396`): widen the gate to also
    // fire when `model_data` is present. C# has no equivalent gate —
    // `BlockHashingHandler` is constructed unconditionally in
    // `WorldData.GenerateWorld` (`WorldData.cs:131-132`).
    let dense_data_ready = world_data_meta
        .as_deref()
        .is_some_and(|w| !w.dense_voxel_types.is_empty());
    let model_data_present = model_data.is_some();
    let want_gpu_producer = construction_config.gpu_construction_enabled
        && (dense_data_ready || model_data_present);
    if want_gpu_producer && !gpu.gpu_producer_has_run {
        // Pre-allocate REAL hash_map / segment_voxel_buffer /
        // hash_coefficients / block_voxel_count, replacing the
        // 1-element W2 placeholders the placeholder block below would
        // otherwise install. If these already exist (e.g. from a previous
        // partial dispatch), reuse them.
        let world_chunk_count = world_gpu.chunks_size_in_chunks.x
            * world_gpu.chunks_size_in_chunks.y
            * world_gpu.chunks_size_in_chunks.z;
        // hash-map: initial size from config; `BlockHashingHandler.cs:32`
        // = `1 << 18 = 262_144` slots, 16 B per slot.
        let hash_map_slots = construction_config.initial_hash_map_size as u64;
        if gpu.hash_map.as_ref().map(|b| b.size()).unwrap_or(0) < hash_map_slots * 16 {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_hash_map_gpu_producer"),
                size: hash_map_slots * 16,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            // web-vox-async-loading Q4 — stash label for debug-only assertion
            // in `populate_cpu_mirror_from_gpu_producer`.
            gpu.hash_map_label = Some("naadf_hash_map_gpu_producer");
            // wgpu storage buffers with `mapped_at_creation: false` have
            // implementation-defined contents (uninitialised on some
            // backends). The open-addressing CAS loop in `chunk_calc.wgsl`
            // depends on `voxel_pointer == EMPTY_BLOCK (0)` to claim a slot,
            // so the entire `hash_map` must be zeroed. NAADF C# explicitly
            // `Clear()`s the GPU buffer at `BlockHashingHandler.cs:74`. We
            // zero the full buffer in chunks (write_buffer staging is
            // chunked internally by wgpu).
            let zero_chunk = vec![0u32; 65536]; // 256 KiB per write
            let total_u32s = (hash_map_slots * 4) as usize;
            let mut written = 0usize;
            while written < total_u32s {
                let remaining = total_u32s - written;
                let n = remaining.min(zero_chunk.len());
                render_queue.write_buffer(
                    &buf,
                    (written * 4) as u64,
                    bytemuck::cast_slice(&zero_chunk[..n]),
                );
                written += n;
            }
            gpu.hash_map = Some(buf);
            bind_groups.construction_world = None;
        }
        // hash_coefficients: 65 u32s, the `31^(64-i)` table.
        if gpu.hash_coefficients.as_ref().map(|b| b.size()).unwrap_or(0) < 65 * 4 {
            let coeffs = hashing::hash_coefficients();
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_hash_coefficients_gpu_producer"),
                size: 65 * 4,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            render_queue.write_buffer(&buf, 0, bytemuck::cast_slice(&coeffs));
            gpu.hash_coefficients = Some(buf);
            gpu.hash_coefficients_label = Some("naadf_hash_coefficients_gpu_producer");
            bind_groups.construction_world = None;
        }
        // block_voxel_count: 2 u32 cursors, seeded at `[32, 64]` per
        // `chunkCalc.fx`'s `block_voxel_count[0]` = voxels cursor (starts at
        // 32 — first 32 u32s reserved for the all-empty placeholder block's
        // VoxelPtr=0), `block_voxel_count[1]` = blocks cursor (starts at 64
        // — first 64 entries reserved for the all-empty placeholder chunk's
        // BlockPtr=0). Matches `validate_gpu_construction`'s seed at
        // `block_voxel_count_init = vec![64u32, 64]` (the validate path uses
        // a slightly different seed of `[64,64]` per its choice; the
        // production GPU path uses `[32, 64]` to match
        // `aadf::construct::construct`'s allocation pattern more closely).
        //
        // Note: the existing `validate_gpu_construction` uses `[64, 64]`
        // because its 1×1×1 test scene's offsets are then trivially
        // calculable in its byte-equal compare. The production runtime is
        // not byte-compared at this point — it only needs to be functionally
        // correct, which `[64, 64]` already is.
        // web-vox-async-loading follow-up (2026-05-18) — re-allocate when
        // the existing buffer is a W2 placeholder. The placeholder and the
        // real gpu_producer buffer are byte-equivalent (both size=8, both
        // seeded `[64, 64]`, both STORAGE|COPY_DST|COPY_SRC), so a pure
        // size-check (`size() < 8`) treats the placeholder as already-good
        // and leaves the `w2_placeholder` label in place. On the web async
        // .vox path the embedded default-scene install precedes the .vox
        // bytes by N frames — the W2 placeholder block in this `prepare`
        // body therefore runs FIRST (during those N pre-`ModelData` frames)
        // and stamps the placeholder label. When `ModelData` finally lands
        // and `want_gpu_producer` flips true, the size-check skips
        // reallocation and `populate_cpu_mirror_from_gpu_producer`'s Q4
        // assertion fires on the lingering placeholder label. Re-checking
        // the label catches this case and re-labels (no buffer churn —
        // both buffers are identical, only `block_voxel_count_label`
        // changes). The native `--vox-e2e` path doesn't see this because
        // it routes through `GridPreset::Vox { path }` at Startup (no
        // pre-`.vox` default-embedded install).
        let needs_realloc = gpu
            .block_voxel_count
            .as_ref()
            .map(|b| b.size())
            .unwrap_or(0)
            < 8
            || gpu
                .block_voxel_count_label
                .is_some_and(|l| l.contains("w2_placeholder"));
        if needs_realloc {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_block_voxel_count_gpu_producer"),
                size: 8,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            render_queue.write_buffer(&buf, 0, bytemuck::cast_slice(&[64u32, 64u32]));
            gpu.block_voxel_count = Some(buf);
            gpu.block_voxel_count_label = Some("naadf_block_voxel_count_gpu_producer");
            bind_groups.construction_world = None;
        }
        // segment_voxel_buffer: built CPU-side from `dense_voxel_types`. For
        // a non-cubic world (e.g. the bevy-naadf 4×2×4 test grid) we **pad
        // to the cubic extent** = max(dim)^3 chunks, so the shader's
        // cubic-shape dispatch `(seg, seg, seg)` workgroups can safely read
        // every `chunk_index_in_segment = gx + gy*seg + gz*seg*seg` index
        // without going out of buffer. The padded entries are all-empty
        // (zero voxel-type) — the shader writes a uniform-empty `block`
        // state for those, and the over-dispatched `textureStore` writes
        // land at chunk positions outside the world texture (wgpu silently
        // ignores out-of-bounds texture writes) so they're a no-op.
        //
        // The padded buffer is `seg^3 * 2048` u32s. NAADF's real segmented
        // iteration is a memory/bandwidth optimisation; collapsing to one
        // dispatch over the cubic extent is functionally equivalent.
        // vox-gpu-rewrite W5.3-fix Stage 1.5 — when `model_data` is present
        // (W5 install path), the segment_voxel_buffer is allocated at the
        // per-segment cubic 128 MiB extent by the W5 block at `:1281-1314`
        // (NOT the dense-derived shape). Skip the dense-derived allocation
        // here so the two blocks don't fight over the same field. The W5
        // block's `gpu.segment_voxel_buffer.is_none()` guard then sees None
        // (since this block is the only other writer) and allocates the
        // proper 128 MiB buffer.
        if gpu.segment_voxel_buffer.as_ref().map(|b| b.size()).unwrap_or(0) <= 4
            && !model_data_present
        {
            let dense = &world_data_meta.as_deref().unwrap().dense_voxel_types;
            let size_in_chunks = [
                world_gpu.chunks_size_in_chunks.x,
                world_gpu.chunks_size_in_chunks.y,
                world_gpu.chunks_size_in_chunks.z,
            ];
            // Pad to cubic extent so the over-dispatch reads stay in bounds.
            let seg = size_in_chunks[0].max(size_in_chunks[1]).max(size_in_chunks[2]).max(1);
            let padded_size = [seg, seg, seg];
            // Build the segment buffer at the padded (cubic) extent, but
            // index the dense voxel data at the REAL world extent — padded
            // chunks read empty (out-of-real-world voxels return 0).
            let segment_data = build_segment_voxel_buffer_from_dense(
                dense,
                size_in_chunks,
                padded_size,
            );
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_segment_voxel_buffer_gpu_producer"),
                size: (segment_data.len() * 4) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            render_queue.write_buffer(&buf, 0, bytemuck::cast_slice(&segment_data));
            gpu.segment_voxel_buffer = Some(buf);
            gpu.segment_voxel_buffer_label = Some("naadf_segment_voxel_buffer_gpu_producer");
            bind_groups.construction_world = None;
        }
        let _ = world_chunk_count; // referenced for future segment-iteration sizing.
    }

    // === W3 — bound-queue family + bind groups ===============================
    //
    // Fixed-size allocation per `WorldBoundHandler.cs:44-47`:
    //   - boundQueueInfo:  32 × 3 × BoundQueueInfo (8 B) — 768 B.
    //   - boundGroupQueues: 32 × 3 × boundGroupCount × u32 — `96 * boundGroupCount` B.
    //   - boundGroupMasks:  boundGroupCount × 3 × u32 — `12 * boundGroupCount` B.
    //                       (We flatten the C# `Uint3` into 3 atomic<u32> slots
    //                       indexed `group * 3 + axis` — `bounds_calc.wgsl`
    //                       file header documents this.)
    //   - boundRefinedInfo: 3 × u32 — 12 B.
    //   - boundDispatchIndirect: 5 × u32 — 20 B, `INDIRECT|STORAGE|COPY_DST`.
    //
    // Build-once: only allocate when the buffers do not exist yet. The
    // size is fixed for the lifetime of the world (the C# allocates once at
    // `WorldBoundHandler::new` — `WorldBoundHandler.cs:38-51`).
    let chunk_count = world_gpu.chunks_size_in_chunks.x
        * world_gpu.chunks_size_in_chunks.y
        * world_gpu.chunks_size_in_chunks.z;
    let bound_group_count = bounds_calc::bound_group_count_of([
        world_gpu.chunks_size_in_chunks.x,
        world_gpu.chunks_size_in_chunks.y,
        world_gpu.chunks_size_in_chunks.z,
    ]);

    if gpu.bound_queue_info.is_none() {
        // wgpu rejects zero-size buffers; clamp every size to ≥1 element.
        let bgc = bound_group_count.max(1) as u64;
        let info_buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_bound_queue_info"),
            size: 32 * 3 * std::mem::size_of::<crate::render::gpu_types::GpuBoundQueueInfo>()
                as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // Seed: `boundQueueInfoNew[i*3+xyz] = {start: 0, size: i == 0 ? boundGroupCount : 0}`
        // — `WorldBoundHandler.cs:55-64`. The size-0 X/Y/Z queues hold every
        // group at startup; all higher bound sizes start empty.
        let mut info_seed: Vec<crate::render::gpu_types::GpuBoundQueueInfo> =
            Vec::with_capacity(32 * 3);
        for i in 0..32u32 {
            for _xyz in 0..3u32 {
                info_seed.push(crate::render::gpu_types::GpuBoundQueueInfo {
                    start: 0,
                    size: if i == 0 { bound_group_count } else { 0 },
                });
            }
        }
        render_queue.write_buffer(&info_buf, 0, bytemuck::cast_slice(&info_seed));

        let queues_buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_bound_group_queues"),
            size: 32 * 3 * bgc * 4,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // Zero-init (the regime-1 `add_initial_groups_to_bound_queue` shader
        // populates the size-0 queues).

        let masks_buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_bound_group_masks"),
            size: bgc * 3 * 4,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // Zero-init.

        // 2026-05-19 horizon-parity diagnostic — extended from 3 u32 to 16 u32.
        // [0..3] = original {start, count, packed_size_axis} prepare writes.
        // [3..6] = diagnostic: prepare logs {found_bound_size, found_xyz,
        //         found_size_atomic_load} every call. Lets the AADF probe2
        //         readback show WHICH queue prepare is picking vs what the
        //         queue ACTUALLY holds, surfacing whether atomic visibility
        //         across compute passes is broken on Dawn.
        // [6]    = diagnostic: atomicAdd counter; compute_group_bounds
        //         increments this once per workgroup that does real work
        //         (is_group_active=true + chunk_state==EMPTY). Lets us see
        //         per-round actual-work count vs prepare's chosen count.
        // [7..16] = reserved for future debug.
        let refined_buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_bound_refined_info"),
            size: 16 * 4,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let indirect_buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_bound_dispatch_indirect"),
            size: 5 * 4,
            usage: BufferUsages::STORAGE
                | BufferUsages::COPY_DST
                | BufferUsages::COPY_SRC
                | BufferUsages::INDIRECT,
            mapped_at_creation: false,
        });
        // Seed: `{GroupCountX=1, GroupCountY=1, GroupCountZ=1, _=0, _=0}` per
        // `WorldBoundHandler.cs:50`. `prepare_group_bounds` overwrites
        // `[0]` (GroupCountX) every frame; `[1]/[2]` stay 1.
        render_queue.write_buffer(
            &indirect_buf,
            0,
            bytemuck::cast_slice(&[1u32, 1u32, 1u32, 0u32, 0u32]),
        );

        gpu.bound_queue_info = Some(info_buf);
        gpu.bound_group_queues = Some(queues_buf);
        gpu.bound_group_masks = Some(masks_buf);
        gpu.bound_refined_info = Some(refined_buf);
        gpu.bound_dispatch_indirect = Some(indirect_buf);
        // Force bind-group rebuild on the next branch.
        bind_groups.construction_bounds_world = None;
        bind_groups.construction_bounds = None;
        bind_groups.bound_dispatch = None;
    }

    // Build the per-frame `GpuConstructionParams` uniform once (build-once;
    // the world topology does not change for the W3 regime-2 path on the
    // static test grid). The uniform is rewritten every frame in regime-2
    // through this same code path (W3 doesn't actually need to *update* it
    // per frame — `bound_group_queue_max_size` / `group_size_in_groups` /
    // `max_group_bound_dispatch` are fixed — but uploading once at startup
    // is cheap).
    if gpu.bounds_params_buffer.is_none() {
        let params = crate::render::gpu_types::GpuConstructionParams {
            size_in_chunks: [
                world_gpu.chunks_size_in_chunks.x,
                world_gpu.chunks_size_in_chunks.y,
                world_gpu.chunks_size_in_chunks.z,
            ],
            _pad0: 0,
            group_size_in_groups: bounds_calc::group_size_in_groups_of([
                world_gpu.chunks_size_in_chunks.x,
                world_gpu.chunks_size_in_chunks.y,
                world_gpu.chunks_size_in_chunks.z,
            ]),
            _pad1: 0,
            bound_group_queue_max_size: bound_group_count.max(1),
            hash_map_size: construction_config.initial_hash_map_size,
            segment_size_in_chunks: 4,
            max_group_bound_dispatch: construction_config.max_group_bound_dispatch,
            chunk_offset: [0, 0, 0],
            dispatch_offset: 0,
            frame_index: 0,
            changed_chunk_count: 0,
            changed_block_count: 0,
            changed_voxel_count: 0,
        };
        let buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_bounds_construction_params"),
            size: std::mem::size_of::<crate::render::gpu_types::GpuConstructionParams>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        render_queue.write_buffer(&buf, 0, bytemuck::bytes_of(&params));
        gpu.bounds_params_buffer = Some(buf);
        bind_groups.construction_bounds_world = None;
    }

    let _ = chunk_count; // referenced for future regime-3 sizing.

    // === Build W3 bind groups when missing ===================================
    if bind_groups.construction_bounds_world.is_none() {
        if let Some(params_buf) = gpu.bounds_params_buffer.as_ref() {
            let bgl = pipeline_cache
                .get_bind_group_layout(&construction_pipelines.construction_bounds_world_layout);
            let bg = render_device.create_bind_group(
                "naadf_construction_bounds_world_bind_group",
                &bgl,
                &BindGroupEntries::sequential((
                    world_gpu.chunks_buffer.as_entire_buffer_binding(),
                    params_buf.as_entire_buffer_binding(),
                )),
            );
            bind_groups.construction_bounds_world = Some(bg);
        }
    }
    if bind_groups.construction_bounds.is_none() {
        if let (Some(info), Some(queues), Some(masks), Some(refined)) = (
            gpu.bound_queue_info.as_ref(),
            gpu.bound_group_queues.as_ref(),
            gpu.bound_group_masks.as_ref(),
            gpu.bound_refined_info.as_ref(),
        ) {
            let bgl = pipeline_cache
                .get_bind_group_layout(&construction_pipelines.construction_bounds_layout);
            let bg = render_device.create_bind_group(
                "naadf_construction_bounds_bind_group",
                &bgl,
                &BindGroupEntries::sequential((
                    info.as_entire_buffer_binding(),
                    queues.as_entire_buffer_binding(),
                    masks.as_entire_buffer_binding(),
                    refined.as_entire_buffer_binding(),
                )),
            );
            bind_groups.construction_bounds = Some(bg);
        }
    }
    if bind_groups.bound_dispatch.is_none() {
        if let Some(indirect) = gpu.bound_dispatch_indirect.as_ref() {
            let bgl = pipeline_cache
                .get_bind_group_layout(&construction_pipelines.bound_dispatch_indirect_layout);
            let bg = render_device.create_bind_group(
                "naadf_bound_dispatch_bind_group",
                &bgl,
                &BindGroupEntries::sequential((indirect.as_entire_buffer_binding(),)),
            );
            bind_groups.bound_dispatch = Some(bg);
        }
    }

    // === W5 — generator_model upload + bind group ============================
    //
    // vox-gpu-rewrite W5.2: when `ModelDataRender` is present (the W5.1
    // extract fired), allocate the 3 model_data storage buffers + the
    // per-segment params uniform, upload the three model buffers ONCE, and
    // build the W5 `@group(0)` bind group. Build-once: every step gates on
    // `is_none()`, so this fires on the first frame all of {model_data,
    // generator_model_layout, segment_voxel_buffer} are ready and is a no-op
    // every frame after.
    //
    // Gating: requires
    //   - `model_data: Option<Res<ModelDataRender>>` → Some
    //   - `construction_pipelines.generator_model_layout` (always present per
    //     the `ConstructionPipelines::from_world` impl at `:337-344`)
    //   - `gpu.segment_voxel_buffer` → Some (the existing W1 block at
    //     `:888-1015` gates `want_gpu_producer` on `dense_voxel_types` being
    //     non-empty. The W5 install path leaves `dense_voxel_types =
    //     Vec::new()`, so `segment_voxel_buffer` will NOT get auto-allocated
    //     by the W1 block. The W5 block ALSO needs to allocate it — at
    //     per-segment cubic extent per the REVISED design decision.)
    //
    // See `docs/orchestrate/vox-gpu-rewrite/02-design.md` Decision:
    // "`segment_voxel_buffer` allocation in the W5 path — extend the
    //  existing block" (REVISED — per-segment cubic 128 MiB, NOT full-world
    //  cubic 134 GiB).
    if let Some(model_data) = model_data.as_deref() {
        // 1) Allocate `segment_voxel_buffer` if it's not already there. Per
        //    the REVISED design decision: size at per-segment cubic extent
        //    (`segment_chunks^3 × 2048 u32 × 4 B`). `segment_chunks =
        //    WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 = 16` → 16³ × 2048 × 4 =
        //    128 MiB (within wgpu Vulkan-baseline 256 MiB cap). The W5.3
        //    segment loop dispatches `chunk_calc` with `group_size_in_chunks
        //    = [16, 16, 16]` and reuses the buffer across all 512 segments,
        //    matching the C# `WorldData.cs:506`
        //    `DispatchCompute(worldGenSegmentSizeInChunks, ...)` shape.
        //
        //    Full-world cubic (`WORLD_SIZE_IN_CHUNKS^3 * 2048 * 4` ≈ 134 GiB)
        //    is REJECTED — past every realistic wgpu cap.
        // web-vox-async-loading follow-up (2026-05-18) — also re-allocate
        // when the existing buffer is the W2 placeholder (size = 4 B, used
        // only as a layout-required binding for `world_change.wgsl`). The
        // web async .vox path runs the embedded-default install first
        // (which leaves `dense_voxel_types = Vec::new()`, so the W2
        // placeholder block fires and installs a 4 B placeholder buffer);
        // when `ModelData` lands and this W5 block runs, an `is_none()`
        // guard would see Some(placeholder) and skip the production
        // 128 MiB allocation. The W5 dispatch then OOB-writes against the
        // 4 B buffer and the chunk_calc shader produces garbage.
        const SEGMENT_CHUNKS: u64 =
            (crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS as u64) * 4;
        let segment_voxel_buffer_full_size = SEGMENT_CHUNKS
            * SEGMENT_CHUNKS
            * SEGMENT_CHUNKS
            * (generator_model::CHUNK_DATA_U32S as u64)
            * 4;
        let segment_needs_realloc = gpu
            .segment_voxel_buffer
            .as_ref()
            .map(|b| b.size())
            .unwrap_or(0)
            < segment_voxel_buffer_full_size;
        if segment_needs_realloc {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_segment_voxel_buffer_w5"),
                size: segment_voxel_buffer_full_size,
                usage: BufferUsages::STORAGE
                    | BufferUsages::COPY_DST
                    | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            gpu.segment_voxel_buffer = Some(buf);
            gpu.segment_voxel_buffer_label = Some("naadf_segment_voxel_buffer_w5");
            // Force rebuild of every bind group that binds this buffer.
            bind_groups.construction_world = None;
            bind_groups.construction_generator_model = None;
        }

        // 2) Allocate + upload the 3 model_data storage buffers (build-once).
        if gpu.model_data_chunk_buffer.is_none() {
            let buf = generator_model::create_storage_buffer_u32(
                &render_device,
                &render_queue,
                "naadf_model_data_chunk",
                &model_data.data_chunk,
            );
            gpu.model_data_chunk_buffer = Some(buf);
            bind_groups.construction_generator_model = None;
        }
        if gpu.model_data_block_buffer.is_none() {
            let buf = generator_model::create_storage_buffer_u32(
                &render_device,
                &render_queue,
                "naadf_model_data_block",
                &model_data.data_block,
            );
            gpu.model_data_block_buffer = Some(buf);
            bind_groups.construction_generator_model = None;
        }
        if gpu.model_data_voxel_buffer.is_none() {
            let buf = generator_model::create_storage_buffer_u32(
                &render_device,
                &render_queue,
                "naadf_model_data_voxel",
                &model_data.data_voxel,
            );
            gpu.model_data_voxel_buffer = Some(buf);
            bind_groups.construction_generator_model = None;
        }

        // 3) Allocate the params uniform (zeroed initial; the W5.3 segment
        //    loop overwrites it 512 times per producer run).
        if gpu.model_data_params_buffer.is_none() {
            let zeroed: generator_model::GpuGeneratorModelParams =
                bytemuck::Zeroable::zeroed();
            let buf = generator_model::create_params_uniform(
                &render_device,
                &render_queue,
                &zeroed,
            );
            gpu.model_data_params_buffer = Some(buf);
            bind_groups.construction_generator_model = None;
        }

        // 4) Build the bind group when missing AND all 5 bindings exist.
        if bind_groups.construction_generator_model.is_none() {
            if let (Some(segv), Some(mdc), Some(mdb), Some(mdv), Some(params)) = (
                gpu.segment_voxel_buffer.as_ref(),
                gpu.model_data_chunk_buffer.as_ref(),
                gpu.model_data_block_buffer.as_ref(),
                gpu.model_data_voxel_buffer.as_ref(),
                gpu.model_data_params_buffer.as_ref(),
            ) {
                let bgl = pipeline_cache.get_bind_group_layout(
                    &construction_pipelines.generator_model_layout,
                );
                let bg = render_device.create_bind_group(
                    "naadf_construction_generator_model_bind_group",
                    &bgl,
                    &BindGroupEntries::sequential((
                        segv.as_entire_buffer_binding(),
                        mdc.as_entire_buffer_binding(),
                        mdb.as_entire_buffer_binding(),
                        mdv.as_entire_buffer_binding(),
                        params.as_entire_buffer_binding(),
                    )),
                );
                bind_groups.construction_generator_model = Some(bg);
            }
        }
    }

    // First-frame seed: when the bound-queue family has just been built AND
    // `WorldGpu`'s chunks texture is the CPU-built version, dispatch
    // `add_initial_groups_to_bound_queue` to seed the size-0 X/Y/Z queues +
    // the per-axis mask bits. This mirrors `WorldBoundHandler.Initialize`
    // (`WorldBoundHandler.cs:53-89`). The dispatch only runs when the W3
    // pipeline has compiled.
    //
    // Phase-C followup #1 — when the GPU producer is in play, the bounds
    // seed reads the chunks-texture `.x` state bits the GPU producer writes.
    // The producer dispatch runs in `naadf_gpu_producer_node` (render-graph,
    // BEFORE the W3 `naadf_bounds_compute_node`). The bounds-init seed below
    // runs HERE in `prepare_construction` (a render-world prepare system,
    // BEFORE the render-graph nodes run for the frame), so it actually fires
    // AFTER the producer-node's writes have landed only from frame 2 onward.
    //
    // To keep the seed in step with the producer: also gate the seed on
    // `gpu_producer_has_run` so it does not fire on a frame where chunks
    // is still empty (producer hasn't run yet). The producer flips the flag
    // when it runs in the render-graph; that flip is visible to
    // `prepare_construction` on the next frame.
    if construction_config.gpu_construction_enabled
        && bound_group_count > 0
        && !gpu.bounds_initialized
        && (!want_gpu_producer || gpu.gpu_producer_has_run)
    {
        let Some(initial_pipeline) = pipeline_cache
            .get_compute_pipeline(construction_pipelines.bounds_calc_pipeline_add_initial)
        else {
            return;
        };
        let (Some(world_bg), Some(bounds_bg)) = (
            bind_groups.construction_bounds_world.as_ref(),
            bind_groups.construction_bounds.as_ref(),
        ) else {
            return;
        };
        let mut encoder =
            render_device.create_command_encoder(&bevy::render::render_resource::CommandEncoderDescriptor {
                label: Some("naadf_bounds_calc_add_initial_seed"),
            });
        bounds_calc::dispatch_add_initial_groups(
            &mut encoder,
            initial_pipeline,
            world_bg,
            bounds_bg,
            bound_group_count,
        );
        render_queue.submit([encoder.finish()]);
        gpu.bounds_initialized = true;
    }

    // === W2 — change-staging family + bind group =============================
    //
    // Allocate per-frame upload buffers for `changedGroups` / `changedChunks`
    // / `changedBlocks` / `changedVoxels`. **Trimmed** initial size relative to
    // NAADF's defaults (`ChangeHandler.cs:53-55` — 2 M chunks, 2 M blocks, 5 M
    // voxels). The test grid never exceeds ~64 edits per frame; 8 KiB-class
    // buffers are sufficient. `world_change.wgsl` accepts any size — only the
    // per-frame `changed_*_count` scalars + dispatch shapes matter for
    // correctness. (Were this a production app with bigger edits, a
    // `GrowableBuffer<T>` would be in order; for the test scene the fixed
    // size suffices.)
    // Bug 4 fix (`docs/orchestrate/feature-completeness/03b-followup-editor-bugs-234.md`):
    // `set_voxels_batch` now emits one `changed_chunks` entry per chunk whose
    // chunk-layer AADF changed via the post-edit recompute. For large
    // `.vox`-loaded worlds (Oasis_Hard_Cover.vox: 93×34×84 = ~265 k chunks),
    // a single brush stroke can dirty up to the entire chunks layer. Bump
    // the static init size from 256 → 524 288 entries (524 288 × 8 B = 4 MiB,
    // still well inside any wgpu `max_buffer_size`). When the world has
    // fewer chunks than the cap, the extra bytes are unused — cheap.
    //
    // **Future**: switch to a `GrowableBuffer<u32>` when worlds may exceed
    // 524 k chunks (the chunks 3D texture wgpu `max_texture_dimension_3d`
    // ceiling is typically ~2048 per axis → 8 G chunks worst case; the
    // current GrowableBuffer for blocks/voxels is the right pattern). Not
    // in scope for the bug-2/3/4 fix.
    // `02f-followup` — Oasis-scale brush capacity. The pre-`02f-followup`
    // sizes (8192 u32s voxels / 4096 u32s blocks / 256 entries groups) were
    // calibrated for ≤63 voxel records / ≤63 block records / ≤32 groups per
    // frame. An r=30 erase sphere on Oasis (1488×544×1344) produces 72 chunks
    // + 63 blocks + 1823 voxels per frame, with the BFS group sweep expanding
    // 9 edited groups to ~2300 changed groups — every one of these
    // overshoots the old cap.
    //
    // Bumped to capacities that absorb a typical full-screen continuous
    // stroke without per-edit `Queue::write_buffer` OOB errors (which silently
    // dropped the W2 dispatch payload on Oasis pre-followup). Total static
    // VRAM ~28 MiB across the four buffers — trivial against an Oasis-scale
    // workload's existing 1.6 GiB voxels alloc.
    //
    // **Future**: as called out pre-followup, switch to `GrowableBuffer<u32>`
    // for unbounded stroke sizes. Static caps work for typical edits (the
    // empirical observation: a single brush frame on Oasis at r=400 produces
    // ~50k voxel records ≈ 200 KiB voxels payload, comfortably under the
    // new 16 MiB voxels cap). The OOB error mode below is the only correctness
    // failure; cap-overflow recovery is a future polish item.
    const W2_CHANGED_CHUNKS_INIT: u64 = 524_288;     // entries; 2×u32 = 8 B  → 4 MiB
    const W2_CHANGED_BLOCKS_INIT: u64 = 1_048_576;   // u32 entries           → 4 MiB
    const W2_CHANGED_VOXELS_INIT: u64 = 4_194_304;   // u32 entries           → 16 MiB
    const W2_CHANGED_GROUPS_INIT: u64 = 524_288;     // entries; 2×u32 = 8 B  → 4 MiB

    let render_world_changes = construction_events.as_ref();
    let needs_upload =
        render_world_changes.is_some_and(|c| c.has_pending_changes());

    if gpu.changed_chunks_dynamic.is_none() {
        let buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_changed_chunks_dynamic"),
            size: W2_CHANGED_CHUNKS_INIT * 8,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        gpu.changed_chunks_dynamic = Some(buf);
        bind_groups.construction_change = None;
    }
    if gpu.changed_blocks_dynamic.is_none() {
        let buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_changed_blocks_dynamic"),
            size: W2_CHANGED_BLOCKS_INIT * 4,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        gpu.changed_blocks_dynamic = Some(buf);
        bind_groups.construction_change = None;
    }
    if gpu.changed_voxels_dynamic.is_none() {
        let buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_changed_voxels_dynamic"),
            size: W2_CHANGED_VOXELS_INIT * 4,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        gpu.changed_voxels_dynamic = Some(buf);
        bind_groups.construction_change = None;
    }
    if gpu.changed_groups_dynamic.is_none() {
        let buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_changed_groups_dynamic"),
            size: W2_CHANGED_GROUPS_INIT * 8,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        gpu.changed_groups_dynamic = Some(buf);
        bind_groups.construction_change = None;
    }

    // Per-frame upload of the CPU-staged `ConstructionEvents` payload. Cheap
    // when empty (`needs_upload = false`); zero-cost no-op on no-edit frames.
    if needs_upload {
        if let Some(events) = render_world_changes {
            if !events.changed_chunks.is_empty() {
                if let Some(buf) = gpu.changed_chunks_dynamic.as_ref() {
                    render_queue.write_buffer(
                        buf,
                        0,
                        bytemuck::cast_slice(&events.changed_chunks),
                    );
                }
            }
            if !events.changed_blocks.is_empty() {
                if let Some(buf) = gpu.changed_blocks_dynamic.as_ref() {
                    render_queue.write_buffer(
                        buf,
                        0,
                        bytemuck::cast_slice(&events.changed_blocks),
                    );
                }
            }
            if !events.changed_voxels.is_empty() {
                if let Some(buf) = gpu.changed_voxels_dynamic.as_ref() {
                    render_queue.write_buffer(
                        buf,
                        0,
                        bytemuck::cast_slice(&events.changed_voxels),
                    );
                }
            }
            if !events.changed_groups.is_empty() {
                if let Some(buf) = gpu.changed_groups_dynamic.as_ref() {
                    render_queue.write_buffer(
                        buf,
                        0,
                        bytemuck::cast_slice(&events.changed_groups),
                    );
                }
            }
        }
        // Re-upload the construction params uniform with the current edit
        // counts so `apply_chunk_change` reads the right count for its
        // `global_id.x >= changed_chunk_count` guard.
        if let (Some(params_buf), Some(events)) =
            (gpu.bounds_params_buffer.as_ref(), render_world_changes)
        {
            let params = crate::render::gpu_types::GpuConstructionParams {
                size_in_chunks: [
                    world_gpu.chunks_size_in_chunks.x,
                    world_gpu.chunks_size_in_chunks.y,
                    world_gpu.chunks_size_in_chunks.z,
                ],
                _pad0: 0,
                group_size_in_groups: bounds_calc::group_size_in_groups_of([
                    world_gpu.chunks_size_in_chunks.x,
                    world_gpu.chunks_size_in_chunks.y,
                    world_gpu.chunks_size_in_chunks.z,
                ]),
                _pad1: 0,
                bound_group_queue_max_size: bound_group_count.max(1),
                hash_map_size: construction_config.initial_hash_map_size,
                segment_size_in_chunks: 4,
                max_group_bound_dispatch: construction_config.max_group_bound_dispatch,
                chunk_offset: [0, 0, 0],
                dispatch_offset: 0,
                frame_index: 0,
                changed_chunk_count: events.changed_chunk_count,
                changed_block_count: events.changed_block_count,
                changed_voxel_count: events.changed_voxel_count,
            };
            render_queue.write_buffer(params_buf, 0, bytemuck::bytes_of(&params));
        }
    }

    // Build the W2 `@group(1)` change bind group + the `construction_world`
    // (W1's 8-binding `@group(0)`) bind group when missing. W2's
    // `world_change.wgsl` consumes both.
    if bind_groups.construction_change.is_none() {
        if let (Some(g), Some(c), Some(b), Some(v)) = (
            gpu.changed_groups_dynamic.as_ref(),
            gpu.changed_chunks_dynamic.as_ref(),
            gpu.changed_blocks_dynamic.as_ref(),
            gpu.changed_voxels_dynamic.as_ref(),
        ) {
            let bgl = pipeline_cache
                .get_bind_group_layout(&construction_pipelines.construction_change_layout);
            let bg = render_device.create_bind_group(
                "naadf_construction_change_bind_group",
                &bgl,
                &BindGroupEntries::sequential((
                    g.as_entire_buffer_binding(),
                    c.as_entire_buffer_binding(),
                    b.as_entire_buffer_binding(),
                    v.as_entire_buffer_binding(),
                )),
            );
            bind_groups.construction_change = Some(bg);
        }
    }

    // Build the `construction_world` bind group (W1's 8-binding `@group(0)`)
    // — needed by `world_change.wgsl`. The actual `WorldGpu` blocks/voxels are
    // the production buffers; we wire them straight in. Bindings 3, 4, 5, 7
    // (block_voxel_count, segment_voxel_buffer, hash_map, hash_coefficients)
    // are not consumed by W2's shader but ARE bound (layout-required) — we
    // create small placeholder storage buffers + the existing
    // `bounds_params_buffer` for the params slot.
    if bind_groups.construction_world.is_none() {
        // Allocate placeholders for the unused-by-W2 bindings if absent.
        // (Allocating once and stashing on `ConstructionGpu` keeps the prepare
        // body cheap; we reuse the existing `bounds_params_buffer` for the
        // params uniform slot.)
        if gpu.block_voxel_count.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_block_voxel_count_w2_placeholder"),
                size: 8, // 2 × u32 — `block_voxel_count[0..2]`
                // COPY_SRC is required for `populate_cpu_mirror_from_gpu_producer`
                // to copy the W5 atomic cursors out via `CopyBufferToBuffer`.
                // Native wgpu validation was lenient with the flag missing;
                // WebGPU's stricter validation rejects the encode and the
                // follow-up `get_mapped_range` panics. See e2e/tests/
                // vox-loading.spec.ts.
                usage: BufferUsages::STORAGE
                    | BufferUsages::COPY_DST
                    | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            render_queue.write_buffer(&buf, 0, bytemuck::cast_slice(&[64u32, 64u32]));
            gpu.block_voxel_count = Some(buf);
            gpu.block_voxel_count_label = Some("naadf_block_voxel_count_w2_placeholder");
        }
        if gpu.segment_voxel_buffer.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_segment_voxel_buffer_w2_placeholder"),
                size: 4,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.segment_voxel_buffer = Some(buf);
            gpu.segment_voxel_buffer_label = Some("naadf_segment_voxel_buffer_w2_placeholder");
        }
        if gpu.hash_map.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_hash_map_w2_placeholder"),
                size: 16, // one HashValueSlot
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.hash_map = Some(buf);
            gpu.hash_map_label = Some("naadf_hash_map_w2_placeholder");
        }
        if gpu.hash_coefficients.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_hash_coefficients_w2_placeholder"),
                size: 4,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.hash_coefficients = Some(buf);
            gpu.hash_coefficients_label = Some("naadf_hash_coefficients_w2_placeholder");
        }

        if let (Some(params), Some(bvc), Some(segv), Some(hmap), Some(coeffs)) = (
            gpu.bounds_params_buffer.as_ref(),
            gpu.block_voxel_count.as_ref(),
            gpu.segment_voxel_buffer.as_ref(),
            gpu.hash_map.as_ref(),
            gpu.hash_coefficients.as_ref(),
        ) {
            // Web-WebGPU migration: chunks is now a storage buffer, not a
            // 3D texture, so there is no `TextureView` to mediate the
            // read-only vs read-write aliasing. Both the render-side
            // `world_layout` (ro) and the construction-side
            // `construction_world_layout` (rw) bind the same
            // `world_gpu.chunks_buffer` resource directly — wgpu inserts the
            // necessary STORAGE→STORAGE barriers between dispatches. (The
            // historical Phase-C followup #1 comment about separate
            // TextureViews is moot because storage buffers do not have
            // view-recorded access types.)
            let bgl = pipeline_cache
                .get_bind_group_layout(&construction_pipelines.construction_world_layout);
            let bg = render_device.create_bind_group(
                "naadf_construction_world_bind_group",
                &bgl,
                &BindGroupEntries::sequential((
                    world_gpu.chunks_buffer.as_entire_buffer_binding(),
                    world_gpu.blocks.buffer().as_entire_buffer_binding(),
                    world_gpu.voxels.buffer().as_entire_buffer_binding(),
                    bvc.as_entire_buffer_binding(),
                    segv.as_entire_buffer_binding(),
                    hmap.as_entire_buffer_binding(),
                    params.as_entire_buffer_binding(),
                    coeffs.as_entire_buffer_binding(),
                )),
            );
            bind_groups.construction_world = Some(bg);
        }
    }

    // === W4 wave-3 — entity-track GPU buffers + bind groups =================
    //
    // Allocate / re-upload the 6 W4 buffers when entities are enabled. Mirror
    // the W2 pattern: fixed-size production buffers + dynamic upload buffers,
    // both bound on `construction_entity_layout` @group(1). The world bind
    // group is rebuilt once with the production entity buffers replacing the
    // placeholders that `prepare_world_gpu` allocated.
    //
    // Gate on `construction_config.entities_enabled` so the no-entities path
    // is a single bool check per frame.
    if construction_config.entities_enabled {
        // Storage caps — sized for the 16384-instance default (the same
        // `WorldRender.cs:88` constant W4 uses). The chunk-update + history
        // upload buffers are sized for the max per-frame count.
        let max_entity_instances = construction_config.max_entity_instances.max(1);
        // 20 B per `GpuEntityChunkInstance`. Cap at 16× max_entity_instances
        // (one entity may overlap up to ~16 chunks in the worst case for
        // chunk-straddling entities of size ≤16 voxels).
        let entity_chunk_instances_cap = (max_entity_instances * 16) as u64;
        // `entity_voxel_data` size: 64 u32s per entity volume; we don't have
        // a hard cap on entity-type count, but for the test fixture 16 is
        // safe (the `EntityData` table is small). The buffer is re-uploaded
        // whenever `events.entity_voxel_data_dirty` fires.
        // History ring: `max_entity_instances * taa_ring_depth` slots.
        let taa_ring_depth = 16u64; // matches `TaaRingConfig::DEFAULT_DEPTH`.
        let history_ring_size = max_entity_instances as u64 * taa_ring_depth * 16; // 16 B per slot

        if gpu.entity_chunk_instances.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("w4_entity_chunk_instances_rw"),
                size: entity_chunk_instances_cap * 20,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            gpu.entity_chunk_instances = Some(buf);
            bind_groups.construction_entity = None;
            gpu.world_bind_group_has_entities = false;
        }
        if gpu.entity_instances_history.is_none() {
            // Phase-C followup #4 — gate the full
            // `max_entity_instances * taa_ring_depth * 16 B` allocation behind
            // `entity_history_enabled`. The Phase-D consumer
            // (TAA reprojection of moving entities) is not yet wired; the
            // production `shoot_ray` never reads this binding. Default-off
            // saves the ring's footprint (16384 * 16 * 16 B ≈ 4 MiB at the
            // current defaults; the C# default is ~128 MiB at 2_000_000
            // instances) while keeping the `world_data.wgsl` binding layout
            // satisfied via a 16 B (1-vec4) placeholder.
            let alloc_size = if construction_config.entity_history_enabled {
                history_ring_size
            } else {
                16
            };
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some(if construction_config.entity_history_enabled {
                    "w4_entity_instances_history_rw"
                } else {
                    "w4_entity_instances_history_rw_placeholder_phase_d_gated"
                }),
                size: alloc_size,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            gpu.entity_instances_history = Some(buf);
            bind_groups.construction_entity = None;
            gpu.world_bind_group_has_entities = false;
        }
        // `entity_voxel_data` — sized per the staged `entity_voxel_data` from
        // `ConstructionEvents`, re-allocated when the dirty flag fires.
        let voxel_data_size_bytes = (construction_events
            .as_ref()
            .map(|e| e.entity_voxel_data.len())
            .unwrap_or(0)
            .max(1) as u64)
            * 4;
        if gpu.entity_voxel_data.is_none()
            || construction_events
                .as_ref()
                .is_some_and(|e| e.entity_voxel_data_dirty)
        {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("w4_entity_voxel_data_rw"),
                size: voxel_data_size_bytes.max(4),
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            // Upload the staged voxel data (if any).
            if let Some(events) = construction_events.as_ref() {
                if !events.entity_voxel_data.is_empty() {
                    render_queue.write_buffer(
                        &buf,
                        0,
                        bytemuck::cast_slice(&events.entity_voxel_data),
                    );
                }
            }
            gpu.entity_voxel_data = Some(buf);
            bind_groups.construction_entity = None;
            gpu.world_bind_group_has_entities = false;
        }

        // Dynamic upload buffers (3) — re-uploaded every frame the entity
        // dispatch fires.
        if gpu.chunk_updates_dynamic.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("w4_chunk_updates_dynamic"),
                // 8 B per update (vec2<u32>); cap at 16× max_entity_instances.
                size: (max_entity_instances as u64 * 16) * 8,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.chunk_updates_dynamic = Some(buf);
            bind_groups.construction_entity = None;
        }
        if gpu.entity_chunk_instances_dynamic.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("w4_entity_chunk_instances_dynamic"),
                size: entity_chunk_instances_cap * 20,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.entity_chunk_instances_dynamic = Some(buf);
            bind_groups.construction_entity = None;
        }
        if gpu.entity_history_dynamic.is_none() {
            // Phase-C followup #4 — placeholder when history is disabled. The
            // bind-group layout requires the binding; `copy_entity_history`
            // is skipped by the node when disabled (see `entity_update.rs`).
            let alloc_size = if construction_config.entity_history_enabled {
                max_entity_instances as u64 * 16
            } else {
                16
            };
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("w4_entity_history_dynamic"),
                size: alloc_size,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.entity_history_dynamic = Some(buf);
            bind_groups.construction_entity = None;
        }
        // Params uniform.
        if gpu.entity_update_params_buffer.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("w4_entity_update_params"),
                size: std::mem::size_of::<entity_update::GpuEntityUpdateParams>() as u64,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.entity_update_params_buffer = Some(buf);
            // The params buffer is referenced by the W4 entity_world bind
            // group built below — that bind group lives outside the
            // `construction_entity` bind group (W4 uses `@group(0)` for
            // `chunks_rw + params`). The dispatch code in
            // `naadf_entity_update_node` builds the world bind group inline,
            // so no bind-group invalidation is needed here.
        }

        // Per-frame uploads — drained from `ConstructionEvents`.
        let entity_uploads_ready = construction_events
            .as_ref()
            .is_some_and(|e| e.has_entity_updates());
        if entity_uploads_ready {
            if let Some(events) = construction_events.as_ref() {
                if !events.entity_uploads.chunk_updates.is_empty() {
                    if let Some(buf) = gpu.chunk_updates_dynamic.as_ref() {
                        render_queue.write_buffer(
                            buf,
                            0,
                            bytemuck::cast_slice(&events.entity_uploads.chunk_updates),
                        );
                    }
                }
                if !events.entity_uploads.entity_chunk_instances.is_empty() {
                    if let Some(buf) = gpu.entity_chunk_instances_dynamic.as_ref() {
                        render_queue.write_buffer(
                            buf,
                            0,
                            bytemuck::cast_slice(&events.entity_uploads.entity_chunk_instances),
                        );
                    }
                }
                // Phase-C followup #4 — skip the per-frame upload when the
                // history consumer is gated off. The buffer is a 16 B
                // placeholder when disabled; writing into it is a waste
                // (and would overflow on `max_entity_instances > 1` uploads).
                if construction_config.entity_history_enabled
                    && !events.entity_uploads.entity_history.is_empty()
                {
                    if let Some(buf) = gpu.entity_history_dynamic.as_ref() {
                        render_queue.write_buffer(
                            buf,
                            0,
                            bytemuck::cast_slice(&events.entity_uploads.entity_history),
                        );
                    }
                }
                // Write the `EntityUpdateParams` uniform for this frame.
                if let Some(params_buf) = gpu.entity_update_params_buffer.as_ref() {
                    let params = entity_update::GpuEntityUpdateParams {
                        entity_instance_count: events.entity_uploads.entity_history.len() as u32,
                        entity_chunk_instance_count: events.entity_uploads.entity_chunk_instances.len() as u32,
                        taa_index: events.entity_taa_index,
                        update_count: events.entity_uploads.chunk_updates.len() as u32,
                        max_entity_instances: construction_config.max_entity_instances,
                        _pad0: 0,
                        _pad1: 0,
                        _pad2: 0,
                        // Web-WebGPU migration: chunks is a flat
                        // `array<vec2<u32>>` buffer; `update_chunks` needs
                        // the world's chunk extent to flatten chunk_pos.
                        size_in_chunks: [
                            world_gpu.chunks_size_in_chunks.x,
                            world_gpu.chunks_size_in_chunks.y,
                            world_gpu.chunks_size_in_chunks.z,
                        ],
                        _pad3: 0,
                    };
                    render_queue.write_buffer(params_buf, 0, bytemuck::bytes_of(&params));
                }
            }
        }

        // Build the W4 `@group(1)` construction-entity bind group when missing.
        if bind_groups.construction_entity.is_none() {
            if let (Some(cu), Some(eci), Some(eh), Some(eci_rw), Some(eih_rw)) = (
                gpu.chunk_updates_dynamic.as_ref(),
                gpu.entity_chunk_instances_dynamic.as_ref(),
                gpu.entity_history_dynamic.as_ref(),
                gpu.entity_chunk_instances.as_ref(),
                gpu.entity_instances_history.as_ref(),
            ) {
                let bgl = pipeline_cache.get_bind_group_layout(
                    &construction_pipelines.construction_entity_layout,
                );
                let bg = render_device.create_bind_group(
                    "naadf_construction_entity_bind_group",
                    &bgl,
                    &BindGroupEntries::sequential((
                        cu.as_entire_buffer_binding(),
                        eci.as_entire_buffer_binding(),
                        eh.as_entire_buffer_binding(),
                        eci_rw.as_entire_buffer_binding(),
                        eih_rw.as_entire_buffer_binding(),
                    )),
                );
                bind_groups.construction_entity = Some(bg);
            }
        }

        // Rebuild the renderer-side world bind group with the production W4
        // entity buffers in place of the `prepare_world_gpu` placeholders
        // (`15-design-c.md` §1.7 wave-3 integration). The renderer nodes bind
        // `WorldGpu::bind_group`, so a `world_gpu.bind_group = rebuilt`
        // mutation here propagates to every downstream pass — the
        // `ray_tracing.wgsl::shoot_ray` entity sub-traversal now reads the
        // production buffers. One-shot, guarded by
        // `gpu.world_bind_group_has_entities`. The layout descriptor is
        // rebuilt inline because `BindGroupLayoutDescriptor` equality is by
        // entry-set; the pipeline cache returns the same layout id as
        // `NaadfPipelines::world_layout`.
        if !gpu.world_bind_group_has_entities {
            if let (Some(eci_rw), Some(evd), Some(eih_rw)) = (
                gpu.entity_chunk_instances.as_ref(),
                gpu.entity_voxel_data.as_ref(),
                gpu.entity_instances_history.as_ref(),
            ) {
                use bevy::render::render_resource::{
                    binding_types::{
                        storage_buffer_read_only_sized, uniform_buffer_sized,
                    },
                    BindGroupLayoutEntries, ShaderStages,
                };
                use std::num::NonZeroU64;
                let world_meta_size = NonZeroU64::new(
                    std::mem::size_of::<crate::render::gpu_types::GpuWorldMeta>() as u64,
                )
                .unwrap();
                let world_layout_desc = bevy::render::render_resource::BindGroupLayoutDescriptor::new(
                    "naadf_world_bind_group_layout",
                    &BindGroupLayoutEntries::sequential(
                        ShaderStages::COMPUTE,
                        (
                            // Web-WebGPU migration: chunks is `array<vec2<u32>>`
                            // (ro on render-side).
                            storage_buffer_read_only_sized(false, None),
                            storage_buffer_read_only_sized(false, None),
                            storage_buffer_read_only_sized(false, None),
                            storage_buffer_read_only_sized(false, None),
                            uniform_buffer_sized(false, Some(world_meta_size)),
                            storage_buffer_read_only_sized(false, None),
                            storage_buffer_read_only_sized(false, None),
                            storage_buffer_read_only_sized(false, None),
                        ),
                    ),
                );
                let bgl = pipeline_cache.get_bind_group_layout(&world_layout_desc);
                let rebuilt = render_device.create_bind_group(
                    "naadf_world_bind_group_with_entities",
                    &bgl,
                    &BindGroupEntries::sequential((
                        world_gpu.chunks_buffer.as_entire_buffer_binding(),
                        world_gpu.blocks.buffer().as_entire_buffer_binding(),
                        world_gpu.voxels.buffer().as_entire_buffer_binding(),
                        world_gpu.voxel_types.buffer().as_entire_buffer_binding(),
                        world_gpu.world_meta.as_entire_buffer_binding(),
                        eci_rw.as_entire_buffer_binding(),
                        evd.as_entire_buffer_binding(),
                        eih_rw.as_entire_buffer_binding(),
                    )),
                );
                world_gpu.bind_group = rebuilt;
                gpu.world_bind_group_has_entities = true;
            }
        }
    }

    // === Phase-C followup #1 — GPU producer dispatch is in a render-graph node
    //
    // Concern #1 from `17-review-c.md`: `run_gpu_construction_startup` is
    // documentation-only. With this followup the chain
    // `chunk_calc.calc_block_from_raw_data` → `compute_voxel_bounds` →
    // `compute_block_bounds` runs against the production `WorldGpu`
    // buffers on the first frame all dependencies are ready.
    //
    // The dispatch lives in `naadf_gpu_producer_node` in the render-graph,
    // NOT here in `prepare_construction`. Reason: a render-graph node
    // uses the same `CommandEncoder` the renderer's reads come from, so
    // wgpu auto-inserts the STORAGE→STORAGE barrier between the
    // producer's writes and the renderer's reads. (Historical note:
    // before the web-WebGPU migration, this comment block flagged a
    // texture-aliasing hazard — `texture_storage_3d<rg32uint, read_write>`
    // writes not propagating to `texture_3d<u32>` reads across separate
    // submits; that hazard is moot now that both bindings reference the
    // same storage buffer.)
    //
    // `prepare_construction`'s job is now just to allocate the buffers +
    // build the bind group (above); the node consumes them.
    let _ = (world_data_meta, want_gpu_producer); // referenced in node.
}

/// Phase-C followup #1 — runtime GPU producer dispatch (render-graph node).
///
/// Runs the chunk_calc chain (`calc_block_from_raw_data` → `compute_voxel_bounds`
/// → `compute_block_bounds`) ONE TIME against the production `WorldGpu`
/// buffers, on the first frame all dependencies are compiled + allocated.
/// One-shot, gated by `ConstructionGpu::gpu_producer_has_run`.
///
/// Lives in the `Core3d` chain BEFORE `naadf_bounds_compute_node` so the W3
/// bounds-init seed can read the chunks `.x` state the chain produces. Uses
/// `RenderContext::command_encoder()` so wgpu/Vulkan auto-inserts the
/// STORAGE-write → SAMPLED-read texture barrier between this node's writes
/// and the renderer's reads (`prepare_construction`'s separate-encoder
/// dispatch pattern would NOT propagate the writes across submits — see
/// the comment block in `prepare_construction`'s GPU-producer section).
///
/// Skipped when:
/// - `gpu_construction_enabled = false` (E4 CPU fallback).
/// - The pipelines have not compiled yet (re-tried next frame).
/// - The bind group is not yet built (re-tried next frame).
/// - The producer has already run.
#[allow(clippy::too_many_arguments)]
pub fn naadf_gpu_producer_node(
    mut render_context: bevy::render::renderer::RenderContext,
    pipeline_cache: Res<bevy::render::render_resource::PipelineCache>,
    construction_pipelines: Option<Res<ConstructionPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_gpu: Option<ResMut<ConstructionGpu>>,
    construction_config: Option<Res<config::ConstructionConfig>>,
    // `02f` rearch: moved from `ExtractedWorld` (deleted) to `WorldDataMeta`
    // — the long-lived metadata-only mirror populated once at startup.
    world_data_meta: Option<Res<crate::render::extract::WorldDataMeta>>,
    // vox-gpu-rewrite W5.3 — needed by the W5 branch to rewrite the per-segment
    // `GpuGeneratorModelParams` and `GpuConstructionParams` uniforms 512 times
    // (one rewrite per segment). `RenderContext::command_encoder` does not
    // expose `write_buffer`; the staging-belt write APIs live on the queue.
    render_queue: Res<RenderQueue>,
    // vox-gpu-rewrite W5.3-fix Stage 1 — needed by the W5 branch to create a
    // fresh `CommandEncoder` per segment. `wgpu::Queue::write_buffer`
    // schedules writes BEFORE the next submit; a per-segment submit ensures
    // each segment's params are visible to its OWN dispatches (rather than
    // all 512 dispatches seeing the last segment's params, the pre-fix bug).
    render_device: Res<RenderDevice>,
    // vox-gpu-rewrite W5.3 — drives the three-way branch ladder. Present iff a
    // `.vox` file was loaded into the fixed world via
    // `install_vox_in_fixed_world` (W5.1).
    model_data: Option<Res<crate::render::extract::ModelDataRender>>,
) {
    let Some(config) = construction_config else { return; };
    if !config.gpu_construction_enabled {
        return;
    }
    let Some(mut gpu) = construction_gpu else { return; };
    if gpu.gpu_producer_has_run {
        return;
    }
    let Some(construction_pipelines) = construction_pipelines else { return; };
    let Some(construction_bind_groups) = construction_bind_groups else { return; };

    // Common-prerequisite pipelines (Algorithm 1 + bounds chain). Both branches
    // of the ladder below need these; resolve up-front.
    let Some(world_bg) = construction_bind_groups.construction_world.as_ref() else {
        return;
    };
    let (Some(p_calc), Some(p_voxel), Some(p_block)) = (
        pipeline_cache
            .get_compute_pipeline(construction_pipelines.chunk_calc_pipeline_calc_block),
        pipeline_cache
            .get_compute_pipeline(construction_pipelines.chunk_calc_pipeline_voxel_bounds),
        pipeline_cache
            .get_compute_pipeline(construction_pipelines.chunk_calc_pipeline_block_bounds),
    ) else {
        return;
    };

    // vox-gpu-rewrite W5.3 — three-way producer gate ladder
    // (`docs/orchestrate/vox-gpu-rewrite/02-design.md` § "Three-way producer
    // gate ordering"):
    //   (a) `ModelDataRender` present + W5 deps ready → per-segment generator
    //       + chunk_calc dispatch chain (the C# `WorldData.GenerateWorld` loop
    //       at `WorldData.cs:120-156`).
    //   (b) else `world_data_meta` has `dense_voxel_types` non-empty → the
    //       existing chunk-calc-only branch (CPU-built segment_voxel_buffer
    //       drives the chunk_calc chain; the default-scene path).
    //   (c) else → CPU upload fallback (early-return; the renderer reads the
    //       pre-built CPU mirror via `prepare_world_gpu`).
    if let Some(model_data) = model_data.as_deref() {
        // === (a) W5 branch — per-segment generator + chunk_calc =============
        //
        // Mirrors C# `NAADF/NAADF/World/Data/WorldData.cs:120-156`:
        //   for (z = 0; z < segs.Z; ++z)
        //       for (y = 0; y < segs.Y; ++y)
        //           for (x = 0; x < segs.X; ++x):
        //               worldGenerator.CopyToChunkData(segmentPosInChunks, ...);
        //               CalculateChunkBlocks(segmentPosInChunks);
        // Then ONCE after the loop:
        //   ComputeVoxelBounds; ComputeBlockBounds.
        let Some(p_gen) = pipeline_cache
            .get_compute_pipeline(construction_pipelines.generator_model_pipeline)
        else {
            return;
        };
        let Some(gen_bg) = construction_bind_groups
            .construction_generator_model
            .as_ref()
        else {
            return;
        };
        let Some(params_buf) = gpu.model_data_params_buffer.as_ref() else {
            return;
        };
        let Some(bounds_params_buf) = gpu.bounds_params_buffer.as_ref() else {
            return;
        };

        let world_size_in_voxels = [
            crate::WORLD_SIZE_IN_VOXELS.x,
            crate::WORLD_SIZE_IN_VOXELS.y,
            crate::WORLD_SIZE_IN_VOXELS.z,
        ];
        // Per-segment chunk extent. C# `WorldData.cs:73,143-145` uses
        // `worldGenSegmentSizeInChunks = WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4
        // = 16` per axis. The dispatch shape (for both generator_model and
        // chunk_calc.calc_block_from_raw_data) is `[16, 16, 16]` workgroups —
        // one per chunk in the segment.
        let segment_chunks: u32 = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
        let group_size_in_chunks =
            [segment_chunks, segment_chunks, segment_chunks];

        // vox-gpu-rewrite W5.3-fix Stage 1 — wgpu's `Queue::write_buffer`
        // writes are scheduled BEFORE the next `Queue::submit`; the writes
        // do NOT interleave with dispatches recorded in the same
        // command-encoder. Pre-fix, 512 write_buffer calls + 512
        // encoder.dispatch calls into ONE encoder + ONE submit meant ALL
        // dispatches saw the LAST write (segment 511's params) — so every
        // segment's chunk_calc.calc_block dispatch wrote to chunk position
        // [60, 4, 60] (the last segment's chunk_offset) instead of its own
        // offset. Result: 511 of the 512 segments' worth of generator output
        // was discarded; only the last segment's chunks landed at the
        // intended world position.
        //
        // Fix: per-segment fresh encoder + submit. `render_queue.write_buffer`
        // is now ordered with the per-segment submit; each submit sees only
        // its own segment's writes; each dispatch uses the correct params.
        //
        // Trade-off: 512 submits/frame instead of 1. The W5 producer runs
        // ONCE per app lifecycle (gated by `gpu_producer_has_run`), so this
        // is a one-time cost at startup, not a per-frame cost. C# behaves
        // identically (`WorldData.cs:120-156` submits per segment via the
        // DirectX immediate context — each `ApplyCompute()` + `DispatchCompute()`
        // is independently submitted with the latest parameter values).
        //
        // The bounds chain AFTER the loop continues to use the
        // `render_context` encoder, since it does NOT need per-segment
        // params rewrites (the bounds chain reads from blocks/voxels, not
        // params.chunk_offset).
        let mut segment_count: u32 = 0;
        for sz in 0..crate::WORLD_SIZE_IN_SEGMENTS.z {
            for sy in 0..crate::WORLD_SIZE_IN_SEGMENTS.y {
                for sx in 0..crate::WORLD_SIZE_IN_SEGMENTS.x {
                    let group_offset_in_chunks = [
                        sx * segment_chunks,
                        sy * segment_chunks,
                        sz * segment_chunks,
                    ];

                    // 1) Per-segment generator_model uniform — mirrors C#
                    //    `WorldGeneratorModel.cs:32-60` `CopyToChunkData`:
                    //      modelSizeInChunks   ← ModelData.size_in_chunks
                    //      sizeInVoxels        ← WorldData.actualSizeInVoxels
                    //      groupOffsetInChunks ← segmentPos * segmentChunks
                    //      groupSizeInChunksX/Y← per-segment chunk extent (16)
                    let gen_params = generator_model::GpuGeneratorModelParams {
                        size_in_voxels: world_size_in_voxels,
                        _pad0: 0,
                        model_size_in_chunks: model_data.size_in_chunks,
                        _pad1: 0,
                        group_offset_in_chunks,
                        group_size_in_chunks_x: segment_chunks,
                        group_size_in_chunks_y: segment_chunks,
                        _pad2: 0,
                        _pad3: 0,
                        _pad4: 0,
                    };
                    render_queue.write_buffer(
                        params_buf,
                        0,
                        bytemuck::bytes_of(&gen_params),
                    );

                    // 2) Per-segment construction params — mirrors C#
                    //    `WorldData.cs:492-503` `CalculateChunkBlocks`.
                    //
                    // vox-gpu-rewrite W5.3-fix Stage 1.5 (2026-05-18) —
                    // `bound_group_queue_max_size` is preserved at
                    // `bound_group_count.max(1)` (not the stale `1` from
                    // pre-Stage-1.5). chunk_calc.wgsl does NOT read this
                    // field, but the post-loop `add_initial_groups`
                    // dispatch DOES (`bounds_calc.wgsl:239 / :257-260`):
                    // its workgroup gate
                    // `if group_index >= params.bound_group_queue_max_size
                    //  { return; }` short-circuited 32767 of 32768
                    // workgroups when this field was `1`, leaving the
                    // chunk-level AADF acceleration structure unbuilt.
                    // Effect was perf-only (rays step chunk-by-chunk
                    // instead of skipping at chunk granularity); diagnostic
                    // at `06-diagnostic-inversion.md:477-507`.
                    let bound_group_count = bounds_calc::bound_group_count_of([
                        crate::WORLD_SIZE_IN_CHUNKS.x,
                        crate::WORLD_SIZE_IN_CHUNKS.y,
                        crate::WORLD_SIZE_IN_CHUNKS.z,
                    ]);
                    let construction_params = crate::render::gpu_types::GpuConstructionParams {
                        size_in_chunks: [
                            crate::WORLD_SIZE_IN_CHUNKS.x,
                            crate::WORLD_SIZE_IN_CHUNKS.y,
                            crate::WORLD_SIZE_IN_CHUNKS.z,
                        ],
                        _pad0: 0,
                        group_size_in_groups:
                            bounds_calc::group_size_in_groups_of([
                                crate::WORLD_SIZE_IN_CHUNKS.x,
                                crate::WORLD_SIZE_IN_CHUNKS.y,
                                crate::WORLD_SIZE_IN_CHUNKS.z,
                            ]),
                        _pad1: 0,
                        bound_group_queue_max_size: bound_group_count.max(1),
                        hash_map_size: config.initial_hash_map_size,
                        segment_size_in_chunks: segment_chunks,
                        max_group_bound_dispatch: config.max_group_bound_dispatch,
                        chunk_offset: group_offset_in_chunks,
                        dispatch_offset: 0,
                        frame_index: 0,
                        changed_chunk_count: 0,
                        changed_block_count: 0,
                        changed_voxel_count: 0,
                    };
                    render_queue.write_buffer(
                        bounds_params_buf,
                        0,
                        bytemuck::bytes_of(&construction_params),
                    );

                    // 3 + 4) Generator → segment_voxel_buffer +
                    //    chunk_calc.calc_block_from_raw_data, per-segment
                    //    encoder + submit (see comment block above).
                    let mut seg_encoder = render_device.create_command_encoder(
                        &CommandEncoderDescriptor {
                            label: Some("naadf_w5_segment_encoder"),
                        },
                    );
                    generator_model::dispatch_generator_model_with_encoder(
                        &mut seg_encoder,
                        p_gen,
                        gen_bg,
                        group_size_in_chunks,
                    );
                    chunk_calc::dispatch_calc_block_from_raw_data_world_sized(
                        &mut seg_encoder,
                        p_calc,
                        world_bg,
                        group_size_in_chunks,
                    );
                    render_queue.submit([seg_encoder.finish()]);

                    segment_count += 1;
                }
            }
        }

        // The bounds chain dispatches on the shared `render_context`
        // encoder — no per-segment params needed, so it can share the
        // single submit at the end of the frame.
        let encoder = render_context.command_encoder();

        // After the per-segment loop, run the bounds chain ONCE (mirrors C#
        // `WorldData.cs:158-210`'s post-loop `ComputeVoxelBounds` +
        // `ComputeBlockBounds` invocations).
        //
        // vox-gpu-rewrite W5.3-fix Stage 1 — for the W5 path
        // `world_data_meta.{blocks,voxels}_cpu_len` are 0 (the W5 install
        // path leaves the CPU mirror empty), so we cannot derive the actual
        // GPU output count without a mid-frame CPU readback (not possible
        // inside a render-graph node). C# DOES readback the cursor each
        // segment (`WorldData.cs:148-151`) and dispatches the bounds chain
        // with `(voxelCount/64, 1, 1)` / `(blockCount/64, 1, 1)`; the Rust
        // port must cover the full-world worst case in one shot.
        //
        // PRE-FIX: the dispatch helpers took a 1D `workgroups: u32` and the
        // call site clamped to wgpu's 65535/axis cap. That under-dispatched
        // by 32×–2046× and left the AADF bits empty on most of the world.
        //
        // POST-FIX: the dispatch helpers
        // (`chunk_calc::dispatch_compute_voxel_bounds` /
        // `dispatch_compute_block_bounds`) repack the 1D count into a 3D
        // shape (`split_3d_dispatch`); the WGSL entry points flatten
        // `(group_id, num_workgroups)` back into a 1D `block_index` /
        // `chunk_index`. Extra workgroups past the actual count read zero
        // blocks (the buffers are sized to worst-case in
        // `render/prepare.rs::prepare_world_gpu`) and are correct no-ops.
        //
        // Upper bound derivation: assume every chunk is mixed and every
        // block is mixed (the absolute worst case for the AADF bounds
        // chain). For the 256×32×256 chunk fixed world:
        //   world_chunks         = 2,097,152
        //   max_blocks_u64       = 134,217,728   (chunks * 64)
        //   max_voxels_u64       = 4,294,967,296 (max_blocks * 32)
        //   voxel_workgroups raw = 134,217,729   (voxels/32 + 1; one wg/mixed block)
        //   block_workgroups raw =   2,097,153   (blocks/64 + 1; one wg/chunk)
        //
        // `split_3d_dispatch` repacks these to 3D shapes within the 65535
        // per-axis cap; the WGSL flattens.
        let world_chunks = crate::WORLD_SIZE_IN_CHUNKS.x
            * crate::WORLD_SIZE_IN_CHUNKS.y
            * crate::WORLD_SIZE_IN_CHUNKS.z;
        let max_blocks_u64 = (world_chunks as u64) * 64;
        let max_voxels_u64 = max_blocks_u64 * 32;
        let voxel_workgroups =
            ((max_voxels_u64 / 32 + 1).max(1)).min(u32::MAX as u64) as u32;
        let block_workgroups =
            ((max_blocks_u64 / 64 + 1).max(1)).min(u32::MAX as u64) as u32;
        let voxel_dispatch = chunk_calc::split_3d_dispatch(voxel_workgroups);
        let block_dispatch = chunk_calc::split_3d_dispatch(block_workgroups);

        // 2026-05-19 — bounds chain runs as a single dispatch on both
        // native and web. (An earlier wasm-only split-dispatch experiment
        // tested whether Dawn was losing invocations from the large 134M
        // dispatch; SSIM moved by 0.02 at 16M batches and noise at 1M
        // batches → confirmed the dispatch completes fine. The
        // `params.dispatch_offset` field stays plumbed in case it's
        // useful for future batching needs; both shaders read it as 0.)
        let _ = (&render_device, &render_queue, bounds_params_buf);
        chunk_calc::dispatch_compute_voxel_bounds(
            encoder,
            p_voxel,
            world_bg,
            voxel_workgroups,
        );
        chunk_calc::dispatch_compute_block_bounds(
            encoder,
            p_block,
            world_bg,
            block_workgroups,
        );

        gpu.gpu_producer_has_run = true;
        info!(
            "vox-gpu-rewrite W5 — per-segment GPU producer chain DISPATCHED \
             ({} segments × (generator_model + calc_block); bounds chain ×1; \
             voxel_workgroups={voxel_workgroups} dispatched as 3D {:?} \
             (= {} total workgroups, covers {} requested), \
             block_workgroups={block_workgroups} dispatched as 3D {:?} \
             (= {} total workgroups, covers {} requested)).",
            segment_count,
            voxel_dispatch,
            voxel_dispatch[0] as u64 * voxel_dispatch[1] as u64 * voxel_dispatch[2] as u64,
            voxel_workgroups,
            block_dispatch,
            block_dispatch[0] as u64 * block_dispatch[1] as u64 * block_dispatch[2] as u64,
            block_workgroups,
        );
        return;
    }

    // === (b) chunk-calc-only branch (existing behaviour) ====================
    let Some(meta) = world_data_meta else { return; };
    if meta.dense_voxel_types.is_empty() {
        // === (c) CPU upload fallback ========================================
        // Source scene didn't author a `DenseVolume` AND no `ModelData` —
        // GPU producer is unsafe to run (the segment_voxel_buffer the
        // chunk_calc dispatch needs cannot be built from CPU data, AND
        // there's no model to generate from). Fall back to the CPU upload
        // path (the renderer reads the pre-built CPU mirror via
        // `prepare_world_gpu`).
        return;
    }
    let size_in_chunks = [
        meta.size_in_chunks.x,
        meta.size_in_chunks.y,
        meta.size_in_chunks.z,
    ];
    // Upper-bound the bound dispatches from the CPU mirror's sizes (each
    // mixed-block produces 32 u32s of voxel data; each mixed-chunk produces
    // 64 u32s of block data — the GPU output sizes match the CPU oracle).
    let cpu_blocks = meta.blocks_cpu_len;
    let cpu_voxels = meta.voxels_cpu_len;
    let voxel_workgroups = (cpu_voxels / 32 + 1).max(1);
    let block_workgroups = (cpu_blocks / 64 + 1).max(1);

    let encoder = render_context.command_encoder();
    // Step 2: calc_block_from_raw_data — Algorithm 1. Dispatch shape = real
    // world extent in chunks (one workgroup per chunk; the workgroup's 64
    // threads each handle one of the 64 blocks per chunk).
    chunk_calc::dispatch_calc_block_from_raw_data_world_sized(
        encoder,
        p_calc,
        world_bg,
        size_in_chunks,
    );
    // Step 3: compute_voxel_bounds — one workgroup per mixed block.
    chunk_calc::dispatch_compute_voxel_bounds(
        encoder,
        p_voxel,
        world_bg,
        voxel_workgroups,
    );
    // Step 4: compute_block_bounds — one workgroup per mixed chunk.
    chunk_calc::dispatch_compute_block_bounds(
        encoder,
        p_block,
        world_bg,
        block_workgroups,
    );

    gpu.gpu_producer_has_run = true;
    info!(
        "phase-c followup#1 — GPU producer chain DISPATCHED (size_in_chunks={:?}, \
         voxel_workgroups={}, block_workgroups={}). \
         Algorithm 1 is now the runtime producer for chunks/blocks/voxels.",
        size_in_chunks, voxel_workgroups, block_workgroups
    );
}

/// `Startup`-schedule one-shot driver — the Phase-C regime-1 announcement
/// (`15-design-c.md` §1.2 regime 1, §3 startup-schedule).
///
/// Lives in the main `App` so we can log + drive the main-world side of the
/// producer choice (whether to keep `WorldData::*_cpu` as the source for the
/// render-world upload, or hand off to the GPU chain). The **actual** runtime
/// GPU producer dispatch lives in the render sub-app in
/// [`prepare_construction`] (the W3 `add_initial_groups` first-frame seed
/// pattern is reused here): `prepare_construction` allocates every buffer +
/// builds every bind group + dispatches the chain in one place, the first
/// frame all dependencies are ready.
///
/// The chain dispatched in `prepare_construction` per
/// `WorldData.cs:120-156`'s `GenerateWorld` sequence + `15-design-c.md` §1.2:
///   1. `generator_model` per segment — currently bypassed for the bevy-naadf
///      test scene (the scene authors a `DenseVolume` directly rather than
///      using NAADF's `WorldGenerator`); `segment_voxel_buffer` is rebuilt
///      CPU-side from `WorldData::dense_voxel_types` and uploaded.
///   2. `chunk_calc.calc_block_from_raw_data` — Algorithm 1: hash + dedup +
///      atomic insert; writes `chunks.x` / `blocks` / `voxels`.
///   3. `chunk_calc.compute_voxel_bounds` — voxel-layer AADFs.
///   4. `chunk_calc.compute_block_bounds` — block-layer AADFs.
///   5. `bounds_calc.add_initial_groups_to_bound_queue` — seed the W3 bound
///      queues. (Already in the render-world prepare; the followup wires it
///      in the same conditional as steps 2-4.)
///
/// Phase-C followup #1 — Concern #1 in `17-review-c.md` was that the
/// production runtime was still reading CPU-uploaded
/// `WorldData::{chunks,blocks,voxels}_cpu`. With this followup the renderer
/// produces those GPU buffers via Algorithm 1 when
/// `gpu_construction_enabled = true` (the default). The CPU `construct()`
/// path still runs in `setup_test_grid` (it produces the CPU mirror used by
/// the oracle + editing path) — E4 fallback is preserved.
pub fn run_gpu_construction_startup(args: Res<crate::AppArgs>) {
    if !args.construction_config.gpu_construction_enabled {
        info!(
            "phase-c — gpu construction DISABLED; CPU `construct()` path \
             produces every chunks/blocks/voxels buffer the renderer reads."
        );
        return;
    }
    info!(
        "phase-c followup#1 — gpu construction ENABLED (default). The runtime \
         GPU dispatch chain (generator-bypass → chunk_calc.calc_block_from_raw_data \
         → compute_voxel_bounds → compute_block_bounds → bounds_calc.add_initial) \
         runs in `prepare_construction` on the first render frame the \
         dependencies (WorldGpu + ConstructionGpu + ConstructionPipelines) are \
         ready; the renderer then reads GPU-produced chunks/blocks/voxels \
         buffers. CPU `construct()` still runs at startup (oracle + fallback per E4)."
    );
}

/// The Phase-C `Plugin` — wires the empty seam into the `App` and the
/// `RenderApp` (`15-design-c.md` §3).
///
/// W0's wiring is intentionally minimal:
/// - main app: registers [`run_gpu_construction_startup`] in `Startup`.
/// - render sub-app: registers [`ConstructionPipelines`] as a GPU resource
///   (built `FromWorld` in `RenderStartup`, same pattern `NaadfPipelines`
///   uses — `render/mod.rs:94`), and registers [`prepare_construction`] in
///   `RenderSystems::PrepareResources`.
///
/// W0 does **not** insert any construction nodes into the `Core3d` chain —
/// the `render/mod.rs` chain has commented TODO markers showing where W2 /
/// W3 / W4 each land their node. The empty seam stays out of the chain so
/// W0's render-graph topology is byte-identical to pre-W0.
///
/// The construction-config resource is mirrored from main-world `AppArgs`
/// into the render sub-app the same way `TaaRingConfig` is mirrored
/// (`render/mod.rs:73-86`).
pub struct ConstructionPlugin;

/// 2026-05-19 horizon-parity AADF diagnostic — render-world resource that
/// drives a one-shot delayed readback of the W3 regime-2 bound_queue_info
/// buffer (96 × 8 B = 768 B) AND a fresh sample of chunks[] AT A LATER
/// FRAME than the initial cpu_mirror snapshot. The original
/// `populate_cpu_mirror_from_gpu_producer` snapshots chunks RIGHT AFTER the
/// W5 producer chain finishes — too early for regime-2 to have converged.
/// This probe waits N frames AFTER cpu_mirror_populated, then re-reads
/// chunks + bound_queue_info to capture the post-convergence state.
#[derive(Resource, Default)]
pub struct AadfDelayedProbe {
    /// Frames elapsed since cpu_mirror_populated flipped to true.
    pub frames_since_mirror: u32,
    /// `false` until the delayed readback has been ISSUED.
    pub readback_issued: bool,
    /// `false` until the delayed readback has been LOGGED.
    pub logged: bool,
    /// Which probe-pass this is (0 = at frame 30, 1 = at frame 500). The
    /// probe fires once at each, resets between.
    pub pass: u32,
    /// Staging buffer for bound_queue_info (small — 768 B).
    pub info_staging: Option<Buffer>,
    pub info_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Staging buffer for chunks[] (16 MiB at production scale).
    pub chunks_staging: Option<Buffer>,
    pub chunks_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Staging buffer for `bound_refined_info` (16 u32 = 64 B). Holds the
    /// per-call diagnostic from prepare_group_bounds + the per-workgroup
    /// "did expansion" counter from compute_group_bounds (2026-05-19
    /// horizon-parity diagnostic).
    pub refined_staging: Option<Buffer>,
    pub refined_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// 2026-05-19 — delayed AADF probe: waits 300 frames (~5 s @ 60 fps) post
/// cpu-mirror-population so the W3 regime-2 background loop has had time
/// to converge, then reads chunks[] AND bound_queue_info back to the CPU
/// and logs:
///   1. Sample chunks along the cross-target gate's camera view-ray with
///      decoded chunk-AADF skip bits — the LATE state (vs the
///      EARLY-state probe at cpu-mirror population time).
///   2. `bound_queue_info[size][axis].size` for all 96 entries. Pins
///      whether the regime-2 loop drained queue[0] (and re-enqueued to
///      queue[1+]) or got stuck somewhere.
pub fn aadf_delayed_probe(
    mut probe: ResMut<AadfDelayedProbe>,
    gpu: Option<Res<ConstructionGpu>>,
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    use bevy::render::render_resource::MapMode;
    use std::sync::atomic::Ordering;

    let Some(gpu) = gpu else { return; };
    if !gpu.cpu_mirror_populated { return; }
    if probe.pass >= 2 { return; }
    probe.frames_since_mirror = probe.frames_since_mirror.saturating_add(1);
    // pass 0 fires at frame 30 (early-convergence snapshot — also the
    // only one native typically reaches before exiting). pass 1 fires at
    // frame 500 (near-screenshot-time on web, lets us see if web
    // converges by then).
    let trigger_frame = if probe.pass == 0 { 30 } else { 200 };
    if probe.frames_since_mirror < trigger_frame { return; }
    let Some(world_gpu) = world_gpu else { return; };

    if !probe.readback_issued {
        let Some(info_src) = gpu.bound_queue_info.as_ref() else { return; };
        let Some(refined_src) = gpu.bound_refined_info.as_ref() else { return; };
        let chunks_src = &world_gpu.chunks_buffer;

        let info_size = 32u64 * 3 * 8; // 32 size-levels × 3 axes × 8 B (GpuBoundQueueInfo)
        let chunks_size = (world_gpu.chunks_size_in_chunks.x as u64)
            * (world_gpu.chunks_size_in_chunks.y as u64)
            * (world_gpu.chunks_size_in_chunks.z as u64)
            * 8;

        let info_staging = render_device.create_buffer(&BufferDescriptor {
            label: Some("aadf_delayed_probe_info_staging"),
            size: info_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let chunks_staging = render_device.create_buffer(&BufferDescriptor {
            label: Some("aadf_delayed_probe_chunks_staging"),
            size: chunks_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let refined_size = 16u64 * 4;
        let refined_staging = render_device.create_buffer(&BufferDescriptor {
            label: Some("aadf_delayed_probe_refined_staging"),
            size: refined_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = render_device.create_command_encoder(
            &CommandEncoderDescriptor { label: Some("aadf_delayed_probe_enc") },
        );
        enc.copy_buffer_to_buffer(info_src, 0, &info_staging, 0, info_size);
        enc.copy_buffer_to_buffer(chunks_src, 0, &chunks_staging, 0, chunks_size);
        enc.copy_buffer_to_buffer(refined_src, 0, &refined_staging, 0, refined_size);
        render_queue.submit([enc.finish()]);

        let info_done = probe.info_done.clone();
        info_staging.slice(..).map_async(MapMode::Read, move |r| {
            if r.is_err() {
                bevy::log::error!("[aadf-probe2] info map_async failed");
            }
            info_done.store(true, Ordering::Release);
        });
        let chunks_done = probe.chunks_done.clone();
        chunks_staging.slice(..).map_async(MapMode::Read, move |r| {
            if r.is_err() {
                bevy::log::error!("[aadf-probe2] chunks map_async failed");
            }
            chunks_done.store(true, Ordering::Release);
        });
        let refined_done = probe.refined_done.clone();
        refined_staging.slice(..).map_async(MapMode::Read, move |r| {
            if r.is_err() {
                bevy::log::error!("[aadf-probe2] refined map_async failed");
            }
            refined_done.store(true, Ordering::Release);
        });

        probe.info_staging = Some(info_staging);
        probe.chunks_staging = Some(chunks_staging);
        probe.refined_staging = Some(refined_staging);
        probe.readback_issued = true;
        bevy::log::info!(
            "[aadf-probe2 pass={}] readback ISSUED at frame {} (chunks_size={} B, info_size={} B)",
            probe.pass, probe.frames_since_mirror, chunks_size, info_size,
        );
        return;
    }

    // Issued — poll the device + wait for both callbacks.
    let _ = render_device.poll(bevy::render::render_resource::PollType::Poll);
    if !probe.info_done.load(Ordering::Acquire) { return; }
    if !probe.chunks_done.load(Ordering::Acquire) { return; }
    if !probe.refined_done.load(Ordering::Acquire) { return; }

    // All three mapped — decode + log + free.
    let Some(info_staging) = probe.info_staging.take() else { return; };
    let Some(chunks_staging) = probe.chunks_staging.take() else { return; };
    let Some(refined_staging) = probe.refined_staging.take() else { return; };

    let info_bytes_arc: Vec<u8> = info_staging.slice(..).get_mapped_range().to_vec();
    let chunks_bytes_arc: Vec<u8> = chunks_staging.slice(..).get_mapped_range().to_vec();
    let refined_bytes: Vec<u8> = refined_staging.slice(..).get_mapped_range().to_vec();
    info_staging.unmap();
    chunks_staging.unmap();
    refined_staging.unmap();

    // 2026-05-19 — decode bound_refined_info (16 u32).
    // [3]=found_bound_size, [4]=found_xyz, [5]=found_size_atomicload,
    // [6]=expansion_workgroup_counter, [7]=prepare_call_counter.
    let refined_u32 = |i: usize| -> u32 {
        u32::from_le_bytes(refined_bytes[i*4..(i+1)*4].try_into().unwrap())
    };
    let pass_tag_r = probe.pass;
    bevy::log::info!(
        "[aadf-probe2 pass={}] bound_refined_info: last_picked={{size={} axis={}}} \
         last_size_loaded={} expansion_workgroups_total={} prepare_calls_total={} \
         [0..3]=(start={},count={},packed_size_axis={})",
        pass_tag_r,
        refined_u32(3),
        refined_u32(4),
        refined_u32(5),
        refined_u32(6),
        refined_u32(7),
        refined_u32(0),
        refined_u32(1),
        refined_u32(2),
    );

    // Decode bound_queue_info (96 entries × 8 B each: start u32, size u32).
    let pass_tag = probe.pass;
    bevy::log::info!(
        "[aadf-probe2 pass={}] post-convergence bound_queue_info SIZE per (bound_size, axis):",
        pass_tag,
    );
    for size_level in 0u32..32 {
        let mut row = format!("[aadf-probe2 pass={}]   size={:2}: ", pass_tag, size_level);
        for axis in 0u32..3 {
            let qi = (size_level * 3 + axis) as usize;
            let off = qi * 8;
            let sz = u32::from_le_bytes(
                info_bytes_arc[off + 4..off + 8].try_into().unwrap(),
            );
            row.push_str(&format!(
                "{}={:5} ",
                ["X", "Y", "Z"][axis as usize],
                sz,
            ));
        }
        bevy::log::info!("{}", row);
    }

    // Decode chunks at the camera-view-ray sample positions (same sample
    // table as the EARLY probe — directly comparable).
    let scx = world_gpu.chunks_size_in_chunks.x as usize;
    let scy = world_gpu.chunks_size_in_chunks.y as usize;
    let scz = world_gpu.chunks_size_in_chunks.z as usize;
    // chunks_buffer is `array<vec2<u32>>` (W4 widening). Stride = 8 B.
    let read_chunk_x = |cx: u32, cy: u32, cz: u32| -> u32 {
        let i = (cx as usize)
            + (cy as usize) * scx
            + (cz as usize) * scx * scy;
        let off = i * 8;
        u32::from_le_bytes(chunks_bytes_arc[off..off + 4].try_into().unwrap())
    };
    let cam = [3880.0_f32, 497.0_f32, 3514.0_f32];
    let fwd = [-0.924_f32, -0.241_f32, -0.297_f32];
    for &dist in &[0.0_f32, 500.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0] {
        let pxw = cam[0] + fwd[0] * dist;
        let pyw = cam[1] + fwd[1] * dist;
        let pzw = cam[2] + fwd[2] * dist;
        if pxw < 0.0 || pyw < 0.0 || pzw < 0.0 { continue; }
        let cx = (pxw as u32) / 16;
        let cy = (pyw as u32) / 16;
        let cz = (pzw as u32) / 16;
        if cx >= scx as u32 || cy >= scy as u32 || cz >= scz as u32 { continue; }
        let word = read_chunk_x(cx, cy, cz);
        let state = (word >> 30) & 0x3;
        let mxd = word & 0x1F;
        let pxd = (word >> 5) & 0x1F;
        let myd = (word >> 10) & 0x1F;
        let pyd = (word >> 15) & 0x1F;
        let mzd = (word >> 20) & 0x1F;
        let pzd = (word >> 25) & 0x1F;
        let st = match state { 0 => "EMPTY", 1 => "FULL", _ => "MIXED" };
        bevy::log::info!(
            "[aadf-probe2 pass={}] dist={} chunk=({},{},{}) word=0x{:08x} state={} \
             chunk_aadf=[mx={} px={} my={} py={} mz={} pz={}]",
            pass_tag, dist as u32, cx, cy, cz, word, st, mxd, pxd, myd, pyd, mzd, pzd,
        );
    }

    bevy::log::info!(
        "[aadf-probe2 pass={}] DONE — logged post-convergence state",
        pass_tag,
    );

    // Reset for the next pass (or end if pass 1 just completed).
    probe.pass += 1;
    probe.readback_issued = false;
    probe.info_done.store(false, std::sync::atomic::Ordering::Release);
    probe.chunks_done.store(false, std::sync::atomic::Ordering::Release);
}

impl Plugin for ConstructionPlugin {
    fn build(&self, app: &mut App) {
        // Read the main-world `AppArgs.construction_config` once at
        // plugin-build time and mirror it into the render sub-app, same
        // pattern as `TaaRingConfig` (`render/mod.rs:73-86`).
        let construction_config = app
            .world()
            .get_resource::<crate::AppArgs>()
            .map(ConstructionConfig::from)
            .unwrap_or_default();

        // Phase-C wave-3 — main-world resource for the W4 entity track. Empty
        // by default; e2e binaries / user code that wants to render an entity
        // populates `instances` + `voxel_data` (and flags `voxel_data_dirty`).
        // The `extract_world_changes` system reads it.
        app.init_resource::<MainWorldEntities>();

        // Main-world `Startup` driver (regime-1, `15-design-c.md` §1.2). W0
        // body is the gated no-op above; W1 fills it.
        app.add_systems(Startup, run_gpu_construction_startup);
        // W2 — clear the per-frame `WorldData::pending_edits` queue after the
        // render world has consumed it via `extract_world_changes`. Runs in
        // the main-world `Last` schedule, so the next tick's `set_voxel` calls
        // start with a clean queue.
        app.add_systems(Last, clear_world_data_pending_edits);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            // Mirror the main-world construction config into the render sub-app.
            .insert_resource(construction_config)
            // 2026-05-19 horizon-parity AADF diagnostic — render-world
            // resource for the delayed bounds-info readback.
            .init_resource::<AadfDelayedProbe>()
            // Empty pipeline registry — W1..W5 add pipeline fields + a
            // proper `FromWorld` impl as they land.
            .init_gpu_resource::<ConstructionPipelines>()
            // W2 — render-world resource mirroring per-frame edit state;
            // populated in `ExtractSchedule` by `extract_world_changes`.
            .init_resource::<ConstructionEvents>()
            // Phase-C wave-3 — render-world W4 entity-handler state. Lives
            // in the render world so the extract can mutate the handler's
            // across-frame state without violating `Extract<>`'s read-only
            // main-world rule.
            .init_resource::<RenderWorldEntityState>()
            // Empty prepare seam — `init_resource`-only body. **Ordered
            // after `prepare_world_gpu`** so `WorldGpu` exists when the
            // Phase-C followup #1 runtime GPU producer dispatch fires
            // (the dispatch reads the production `WorldGpu::chunks/blocks/
            // voxels` and writes them via Algorithm 1).
            .add_systems(
                Render,
                prepare_construction
                    .in_set(RenderSystems::PrepareResources)
                    .after(crate::render::prepare::prepare_world_gpu),
            )
            // W2 — extract main-world `WorldData::pending_edits` to the
            // render-world `ConstructionEvents` resource.
            //
            // vox-gpu-rewrite W5.3-fix Stage 5 — D1 fix: after the W5 GPU
            // producer chain runs, copy the GPU buffer outputs back to the
            // main-world `WorldData::{chunks,blocks,voxels}_cpu` so the
            // CPU-side editor raycaster has data to traverse. One-shot,
            // gated by `gpu_producer_has_run && !cpu_mirror_populated`;
            // no-op on every other frame.
            .add_systems(
                ExtractSchedule,
                (extract_world_changes, populate_cpu_mirror_from_gpu_producer),
            )
            // 2026-05-19 horizon-parity AADF diagnostic.
            .add_systems(ExtractSchedule, aadf_delayed_probe);
    }
}

/// Phase-C followup #1 — runtime helper that builds the full-world
/// `segment_voxel_buffer` from a dense `u16` voxel-type stream
/// (`world_size_in_voxels.x*y*z` entries, indexed
/// `x + y*world_sx_v + z*world_sx_v*world_sy_v`).
///
/// `world_size_in_chunks` is the REAL world extent the dense buffer covers.
/// `segment_size_in_chunks` is the size of the segment to build (≥ world; for
/// non-cubic worlds, segment is padded to `max(world_dim)` so the shader's
/// cubic `(seg, seg, seg)` workgroup dispatch reads stay in bounds). Padded
/// chunks (outside the world) return 0 (all-empty) for every voxel.
///
/// The encoding matches [`build_segment_voxel_buffer`]: 2048 u32s per chunk
/// (64 blocks × 32 u32s/block; 2 voxels per u32 packed as `lo | (hi << 16)`);
/// each voxel encodes as `(1u << 15) | type` for non-empty, `0` for empty.
pub fn build_segment_voxel_buffer_from_dense(
    dense_voxel_types: &[u16],
    world_size_in_chunks: [u32; 3],
    segment_size_in_chunks: [u32; 3],
) -> Vec<u32> {
    let world_sx_v = world_size_in_chunks[0] * 16;
    let world_sy_v = world_size_in_chunks[1] * 16;
    let world_sz_v = world_size_in_chunks[2] * 16;
    let seg_chunks =
        (segment_size_in_chunks[0] * segment_size_in_chunks[1] * segment_size_in_chunks[2]) as usize;
    let total_u32s = seg_chunks * 2048;
    let mut out = vec![0u32; total_u32s];
    let voxel_at = |v: [u32; 3]| -> u16 {
        // Out-of-real-world voxel positions read as empty (padding chunks).
        if v[0] >= world_sx_v || v[1] >= world_sy_v || v[2] >= world_sz_v {
            return 0;
        }
        let idx = (v[0] + v[1] * world_sx_v + v[2] * world_sx_v * world_sy_v) as usize;
        if idx >= dense_voxel_types.len() {
            return 0;
        }
        let ty = dense_voxel_types[idx];
        if ty == 0 {
            0
        } else {
            crate::voxel::VOXEL_FULL_FLAG | (ty & crate::voxel::VOXEL_PAYLOAD_MASK)
        }
    };
    for cz in 0..segment_size_in_chunks[2] as usize {
        for cy in 0..segment_size_in_chunks[1] as usize {
            for cx in 0..segment_size_in_chunks[0] as usize {
                let chunk_index = cx
                    + cy * segment_size_in_chunks[0] as usize
                    + cz * segment_size_in_chunks[0] as usize
                        * segment_size_in_chunks[1] as usize;
                let chunk_base = chunk_index * 2048;
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let vi_lo = vi * 2;
                                let vi_hi = vi * 2 + 1;
                                let lvx = vi_lo % 4;
                                let lvy = (vi_lo / 4) % 4;
                                let lvz = vi_lo / 16;
                                let hvx = vi_hi % 4;
                                let hvy = (vi_hi / 4) % 4;
                                let hvz = vi_hi / 16;
                                let lo = voxel_at([
                                    (cx * 16 + bx * 4 + lvx) as u32,
                                    (cy * 16 + by * 4 + lvy) as u32,
                                    (cz * 16 + bz * 4 + lvz) as u32,
                                ]);
                                let hi = voxel_at([
                                    (cx * 16 + bx * 4 + hvx) as u32,
                                    (cy * 16 + by * 4 + hvy) as u32,
                                    (cz * 16 + bz * 4 + hvz) as u32,
                                ]);
                                out[block_base + vi] = (lo as u32) | ((hi as u32) << 16);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Build a `segment_voxel_buffer` from a `DenseVolume` segment matching the
/// byte layout that `chunkCalc.fx::calcBlockFromRawData` reads. Used by the
/// W1 GPU/CPU oracle test + by `--validate-gpu-construction`.
///
/// The encoding (per `chunkCalc.fx:120-121` + `compute_voxel_bounds`'s
/// `localIndex = lx + ly*4 + lz*16` voxel ordering, `chunkCalc.fx:205`):
///
/// - For each chunk in the segment (in chunk scan order `cx, cy, cz` →
///   `chunk_index = cx + cy*seg + cz*seg*seg`), the 64 blocks of the chunk
///   are at consecutive offsets `chunk_index * 2048 + block_index * 32` for
///   `block_index = 0..64`.
/// - The block at intra-chunk position `(bx, by, bz)` has
///   `block_index = bx + by*4 + bz*16`.
/// - The 64 voxels of a block are packed two per u32; voxel at intra-block
///   position `(vx, vy, vz)` has `voxel_index = vx + vy*4 + vz*16`. The u32
///   offset within the block is `voxel_index / 2`; the low half holds the
///   even-index voxel, the high half the odd-index.
/// - Each voxel encodes as `u16`: full voxel = `(1 << 15) | type`; empty = 0.
///
/// The `segment_size_in_chunks` is the chunk extent of the segment (NAADF
/// default 4 — the C# `WorldData.cs:73`). For the W1 test we use whatever the
/// volume's `size_in_chunks` is (so the test segment matches the test grid).
pub fn build_segment_voxel_buffer(
    volume: &crate::aadf::construct::DenseVolume,
    segment_size_in_chunks: u32,
) -> Vec<u32> {
    let seg = segment_size_in_chunks as usize;
    let total_u32s = seg * seg * seg * 2048;
    let mut out = vec![0u32; total_u32s];

    for cz in 0..seg {
        for cy in 0..seg {
            for cx in 0..seg {
                let chunk_index_in_segment = cx + cy * seg + cz * seg * seg;
                let chunk_base = chunk_index_in_segment * 2048;

                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;

                            // 64 voxels per block, packed two per u32, low
                            // half = even index.
                            for vi in 0..32 {
                                // Two voxels per pair.
                                let lo = voxel_at_block_local(
                                    volume,
                                    [cx, cy, cz],
                                    [bx, by, bz],
                                    vi * 2,
                                );
                                let hi = voxel_at_block_local(
                                    volume,
                                    [cx, cy, cz],
                                    [bx, by, bz],
                                    vi * 2 + 1,
                                );
                                out[block_base + vi] =
                                    (lo as u32) | ((hi as u32) << 16);
                            }
                        }
                    }
                }
            }
        }
    }

    out
}

/// Read voxel at intra-block index `voxel_idx` of block `(bx,by,bz)` in
/// chunk `(cx,cy,cz)`, encoded as the 16-bit `VoxelCell::Full` /
/// `VoxelCell::Empty` payload that `chunkCalc.fx` reads.
///
/// Out-of-bounds positions clamp to empty (type 0).
fn voxel_at_block_local(
    volume: &crate::aadf::construct::DenseVolume,
    chunk: [usize; 3],
    block: [usize; 3],
    voxel_idx: usize,
) -> u16 {
    // voxel intra-block position: vx + vy*4 + vz*16 = voxel_idx
    let vx = voxel_idx % 4;
    let vy = (voxel_idx / 4) % 4;
    let vz = voxel_idx / 16;
    let world_v = [
        (chunk[0] * 16 + block[0] * 4 + vx) as u32,
        (chunk[1] * 16 + block[1] * 4 + vy) as u32,
        (chunk[2] * 16 + block[2] * 4 + vz) as u32,
    ];
    let size = volume.size_in_voxels();
    if world_v[0] >= size[0] || world_v[1] >= size[1] || world_v[2] >= size[2] {
        return 0;
    }
    let ty = volume.voxel_at(world_v);
    if ty == crate::voxel::VoxelTypeId::EMPTY {
        0u16
    } else {
        // VoxelCell::Full encoding (`aadf::cell::VoxelCell::encode`): bit 15
        // set, low 15 bits = type. Matches `chunkCalc.fx`'s `voxel & 0x7FFF`
        // hash extraction + `>> 15` state detection.
        crate::voxel::VOXEL_FULL_FLAG | (ty.raw() & crate::voxel::VOXEL_PAYLOAD_MASK)
    }
}

/// W1 — entry point for `bevy-naadf e2e_render --validate-gpu-construction`.
///
/// Boots a headless render world, builds the GPU `chunk_calc.wgsl` pipelines,
/// runs Algorithm 1 + the two AADF passes against a small deterministic test
/// scene, and asserts the GPU output matches the CPU `aadf::construct::
/// construct` oracle byte-for-byte. Returns the number of bytes compared on
/// success, an error message on failure.
///
/// Used by `crates/bevy_naadf/src/bin/e2e_render.rs` when the
/// `--validate-gpu-construction` CLI flag is present. The flag-plumbing was
/// added by W0; W1 fills in the body here.
///
/// The validation scene is a 1×1×1 chunk world with one solid voxel — the
/// minimum geometry that exercises Algorithm 1's mixed-block / hash-dedup /
/// AADF-encode paths AND has a deterministic `VoxelPtr(0)` assignment on
/// both CPU and GPU (mixed-block dedup hits a single key, deterministic
/// regardless of HashMap iteration order). Bigger scenes diverge at the
/// `VoxelPtr` level (CPU `HashMap` iteration vs GPU
/// `hash & (mapSize - 1)`) — semantic equality is provable but not byte
/// equality; the W1 brief / `15-design-c.md` §1.6 assumption #7 flags this.
///
/// The runtime path here mirrors the `tests_w1::gpu_algorithm1_vs_cpu_bit_exact`
/// unit test exactly; the helper exists so both the test + the e2e CLI flag
/// run the same code.
pub fn validate_gpu_construction() -> Result<usize, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell, VoxelPtr};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;
    use crate::voxel::VoxelTypeId;

    // ── Boot headless render world ────────────────────────────────────────────
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

    let shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp sub-app available".into());
    };
    {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no RenderDevice")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no RenderQueue")?
        .clone();

    // ── Test scene: 1×1×1 chunk world, single mixed block ────────────────────
    let mut volume = DenseVolume::empty([1, 1, 1]);
    let ty = VoxelTypeId(7);
    volume.set([0, 0, 0], ty);

    let oracle = construct(&volume);

    // ── Allocate GPU buffers + uniform ────────────────────────────────────────
    let segment_size_in_chunks: u32 = 1;
    let size_in_chunks: [u32; 3] = volume.size_in_chunks;
    let segment_voxels = build_segment_voxel_buffer(&volume, segment_size_in_chunks);
    let hash_map_size_slots: u32 = 256;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let mk_storage = |label: &'static str, data: &[u32]| {
        let data = if data.is_empty() { &[0u32][..] } else { data };
        let size = (data.len() * 4) as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
        buffer
    };

    let gpu_blocks = mk_storage(
        "validate_blocks",
        &vec![0u32; oracle.blocks.len().max(64) + 64],
    );
    let gpu_voxels = mk_storage(
        "validate_voxels",
        &vec![0u32; oracle.voxels.len().max(32) + 32],
    );
    let gpu_block_voxel_count = mk_storage("validate_bvc", &block_voxel_count_init);
    let gpu_segment = mk_storage("validate_segment", &segment_voxels);
    let gpu_hash_map = mk_storage("validate_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("validate_coeffs", &coeffs);

    let params = GpuConstructionParams {
        size_in_chunks,
        _pad0: 0,
        group_size_in_groups: [1, 1, 1],
        _pad1: 0,
        bound_group_queue_max_size: 1,
        hash_map_size: hash_map_size_slots,
        segment_size_in_chunks,
        max_group_bound_dispatch: 0,
        chunk_offset: [0, 0, 0],
        dispatch_offset: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let params_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("validate_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

    // Web-WebGPU migration: chunks is an `array<vec2<u32>>` storage buffer
    // (was `Rg32Uint` 3D texture). 8 B per chunk pair; the W1 validation
    // path zeros every channel.
    let chunk_count_total =
        (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count_total];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("validate_chunks"),
        size: (chunk_count_total as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    // ── Queue + compile pipelines ─────────────────────────────────────────────
    let layout = construction_world_layout_descriptor();
    let (id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let a = queue_calc_block_pipeline_with_handle(
            cache,
            layout.clone(),
            shader_handle.clone(),
        );
        let b = queue_voxel_bounds_pipeline_with_handle(
            cache,
            layout.clone(),
            shader_handle.clone(),
        );
        let c = queue_block_bounds_pipeline_with_handle(
            cache,
            layout.clone(),
            shader_handle.clone(),
        );
        (a, b, c)
    };

    let mut pipelines: Option<Vec<bevy::render::render_resource::ComputePipeline>> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some(vec![a.clone(), b.clone(), c.clone()]);
            break;
        }
    }
    let pipelines = pipelines.ok_or("W1 pipelines did not compile")?;

    // ── Build bind group ──────────────────────────────────────────────────────
    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout);
    let bind_group = device.create_bind_group(
        "validate_bind_group",
        &bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            gpu_segment.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            params_buffer.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch the 3 passes ─────────────────────────────────────────────────
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("validate_calc_block"),
    });
    dispatch_calc_block_from_raw_data(
        &mut encoder,
        &pipelines[0],
        &bind_group,
        segment_size_in_chunks,
    );
    queue.submit([encoder.finish()]);

    // Read cursors to size the bounds dispatches faithfully.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("validate_cursor_staging"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("validate_cursor_readback"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_workgroups = cursor_pair[0] / 64;
    let block_workgroups = cursor_pair[1] / 64;

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("validate_bounds"),
    });
    dispatch_compute_voxel_bounds(&mut encoder, &pipelines[1], &bind_group, voxel_workgroups);
    dispatch_compute_block_bounds(&mut encoder, &pipelines[2], &bind_group, block_workgroups);
    queue.submit([encoder.finish()]);

    // ── Read back + compare ───────────────────────────────────────────────────
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("validate_readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("validate_readback_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };

    let gpu_blocks_out = read_u32(&gpu_blocks, (oracle.blocks.len().max(64) + 64) as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, (oracle.voxels.len().max(32) + 32) as u64);

    // Web-WebGPU migration: chunks is a flat `array<vec2<u32>>` storage
    // buffer (8 B per pair). The validation gate compares the `.x` (state)
    // channel only — that's what W1 writes; `.y` (entity pointer) stays zero.
    // Buffer→buffer copy doesn't need bytes_per_row padding.
    let chunk_count = size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2];
    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("validate_chunks_readback"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("validate_chunks_readback_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // Compare. The CPU oracle uses `VoxelPtr(0)` / `BlockPtr(0)` for its
    // first allocations; the GPU seeds the cursors at 64, so its first
    // mixed-chunk's BlockPtr = 64 and the first mixed-block's VoxelPtr = 32
    // (in u32-element units). Re-encode the oracle output with these shifts.
    let mut bytes_compared: usize = 0;
    for (i, &gpu_chunk) in gpu_chunks_out.iter().enumerate() {
        let expected = match ChunkCell::decode(oracle.chunks[i]) {
            ChunkCell::Mixed(ptr) => {
                ChunkCell::Mixed(crate::aadf::cell::BlockPtr(ptr.0 + 64)).encode()
            }
            other => other.encode(),
        };
        if gpu_chunk != expected {
            return Err(format!(
                "chunk[{}] mismatch: gpu={:#010x} expected={:#010x}",
                i, gpu_chunk, expected
            ));
        }
        bytes_compared += 4;
    }
    for (i, &c) in oracle.blocks.iter().enumerate() {
        let expected = match BlockCell::decode(c) {
            BlockCell::Mixed(VoxelPtr(v)) => {
                BlockCell::Mixed(VoxelPtr(v + 32)).encode()
            }
            other => other.encode(),
        };
        let g = gpu_blocks_out[64 + i];
        if g != expected {
            return Err(format!(
                "block[{}] mismatch: gpu={:#010x} expected={:#010x}",
                64 + i, g, expected
            ));
        }
        bytes_compared += 4;
    }
    for (i, &c) in oracle.voxels.iter().enumerate() {
        let g = gpu_voxels_out[32 + i];
        if g != c {
            return Err(format!(
                "voxel[{}] mismatch: gpu={:#010x} oracle={:#010x}",
                32 + i, g, c
            ));
        }
        bytes_compared += 4;
    }

    Ok(bytes_compared)
}

/// vox-gpu-rewrite — concrete byte-level diagnostic for GPU vs CPU chunk_calc
/// divergence (`12-diagnostic-byte-diff-concrete.md`).
///
/// Drives the same W5 chunk_calc chain as [`validate_gpu_construction`] but
/// across a sweep of progressively-larger fixtures, emitting the first
/// divergent u32 index per buffer (chunks / blocks / voxels) for each fixture
/// — both via raw byte-equality and via semantic pointer-following
/// comparison. The raw-byte form is sensitive to nondeterministic atomic
/// ordering (`atomicAdd` cursor races across mixed chunks) and will report
/// divergences that are semantically benign; the pointer-following form
/// reports only divergences in the actual referenced content.
///
/// Output: a multi-fixture report printed to stderr; the function returns
/// `Ok(report_string)` on completion regardless of whether divergences were
/// found (this is a DIAGNOSTIC, not a gate).
pub fn validate_gpu_construction_scaled() -> Result<String, String> {
    let mut report = String::new();
    report.push_str("=== vox-gpu-rewrite scaled byte-diff diagnostic ===\n\n");

    // Fixture sweep — three diversity modes per scale to triangulate where
    // bytes diverge:
    //   "uniform":  every chunk has identical mixed content (1 voxel at
    //               block-corner) — heavy dedup-hit traffic, all blocks
    //               funnel to one voxel slot.
    //   "diverse":  every chunk has a UNIQUE mixed content (different voxel
    //               position per chunk) — no dedup hits, every chunk claims
    //               its own slot. Stresses CAS slot-claim path.
    //   "mixed":    half chunks unique, half identical — both dedup-hit and
    //               new-slot CAS are exercised in the same dispatch.
    let fixtures: &[(&str, [u32; 3], FixtureMode)] = &[
        ("2x1x2-uniform",   [2, 1, 2],   FixtureMode::Uniform),
        ("4x1x4-mixed",     [4, 1, 4],   FixtureMode::Mixed),
        ("16x1x16-mixed",   [16, 1, 16], FixtureMode::Mixed),
        ("32x2x32-diverse", [32, 2, 32], FixtureMode::Diverse),
        ("64x4x64-mixed",   [64, 4, 64], FixtureMode::Mixed),
    ];

    for (label, dims, mode) in fixtures {
        report.push_str(&format!("--- fixture {label}: {:?} chunks, mode={:?} (single-dispatch) ---\n", dims, mode));
        match run_one_fixture_byte_diff(*dims, *mode) {
            Ok(s) => {
                report.push_str(&s);
                report.push('\n');
            }
            Err(e) => {
                report.push_str(&format!("FIXTURE FAILED: {e}\n\n"));
            }
        }
    }

    // Multi-segment dispatch sweep — mirrors the production W5 segment loop:
    // hash_map is allocated once + reused across all per-segment dispatches.
    // Each segment dispatches generator_model + calc_block_from_raw_data
    // for its own chunk_offset (the segment buffer is reused per segment).
    // This exercises cross-segment hash_map state accumulation that the
    // single-dispatch fixtures cannot reach.
    let multi_seg_fixtures: &[(&str, [u32; 3], u32, FixtureMode)] = &[
        // (label, world_chunks, segment_size_in_chunks, mode)
        ("4x4x4-seg2-mixed",   [4, 4, 4],   2, FixtureMode::Mixed),
        ("8x4x8-seg4-mixed",   [8, 4, 8],   4, FixtureMode::Mixed),
        ("16x4x16-seg4-mixed", [16, 4, 16], 4, FixtureMode::Mixed),
        ("32x4x32-seg4-mixed", [32, 4, 32], 4, FixtureMode::Mixed),
        ("64x16x64-seg16-mixed",[64, 16, 64], 16, FixtureMode::Mixed),
        // Closest to Oasis-class scale with a sane CPU oracle footprint.
        // 128x16x128 = 262144 chunks. CPU `DenseVolume::voxels` is
        // (128*16)*(16*16)*(128*16) = 2048*256*2048 ≈ 1 G voxels (~ 1 GB for
        // u16 voxel-types). Larger world sizes overflow u32 voxel-count
        // (e.g. 256x32x256 → 8.5 G voxels → u32 wrap → empty Vec → panic).
        ("128x16x128-seg16-mixed", [128, 16, 128], 16, FixtureMode::Mixed),
    ];

    for (label, dims, seg_size, mode) in multi_seg_fixtures {
        report.push_str(&format!(
            "--- fixture {label}: {:?} chunks, seg={seg_size}, mode={:?} (multi-segment) ---\n",
            dims, mode
        ));
        match run_one_fixture_multiseg_byte_diff(*dims, *seg_size, *mode) {
            Ok(s) => {
                report.push_str(&s);
                report.push('\n');
            }
            Err(e) => {
                report.push_str(&format!("FIXTURE FAILED: {e}\n\n"));
            }
        }
    }

    // ── Generator_model.wgsl byte-diff ──────────────────────────────────────
    // Compares the generator_model.wgsl GPU output against the
    // `generate_segment_cpu` Rust oracle (the bit-exact port). Both consume a
    // `ModelData` and produce the same `segment_voxel_buffer` layout that
    // chunk_calc reads. If THIS diverges, the bug is upstream of chunk_calc
    // (in generator_model itself).
    report.push_str("\n=== Generator_model.wgsl GPU vs CPU oracle byte-diff ===\n");
    let gen_fixtures: &[(&str, [u32; 3], [u32; 3], [u32; 3])] = &[
        // (label, model_size_in_chunks, group_size_in_chunks, group_offset_in_chunks)
        ("model-1x1x1-seg-1x1x1",   [1, 1, 1],   [1, 1, 1],  [0, 0, 0]),
        ("model-2x1x2-seg-4x4x4",   [2, 1, 2],   [4, 4, 4],  [0, 0, 0]),
        ("model-4x2x4-seg-8x4x8",   [4, 2, 4],   [8, 4, 8],  [0, 0, 0]),
        ("model-8x2x8-seg-16x16x16",[8, 2, 8],   [16, 16, 16],[0, 0, 0]),
        // Offset matters — test a non-zero offset to exercise the Y-clamp
        // and the world-extent gate.
        ("model-2x1x2-seg-4x4x4-off2-1-2", [2, 1, 2], [4, 4, 4], [2, 1, 2]),
    ];
    for (label, model_dims, seg_dims, off) in gen_fixtures {
        report.push_str(&format!(
            "--- gen fixture {label}: model={:?} seg={:?} off={:?} ---\n",
            model_dims, seg_dims, off
        ));
        match run_one_generator_model_byte_diff(*model_dims, *seg_dims, *off) {
            Ok(s) => {
                report.push_str(&s);
                report.push('\n');
            }
            Err(e) => {
                report.push_str(&format!("FIXTURE FAILED: {e}\n\n"));
            }
        }
    }

    // ── TILED ModelData: small model tiled into a larger world ─────────────
    // Mirrors production behavior: a small ModelData (a "tile") gets repeated
    // across the larger fixed world via the generator's `voxelPos % modelSize`
    // wraparound. The repeated tiles produce many chunks with identical
    // content, hitting the chunk_calc dedup path heavily.
    report.push_str("\n=== Tiled ModelData (small model, large world) generator+chunk_calc byte-diff ===\n");
    let tiled_fixtures: &[(&str, [u32; 3], [u32; 3])] = &[
        ("model-2x1x2_world-4x1x4",  [2, 1, 2], [4, 1, 4]),
        ("model-4x1x4_world-16x1x16",[4, 1, 4], [16, 1, 16]),
        ("model-4x2x4_world-16x4x16",[4, 2, 4], [16, 4, 16]),
        ("model-8x2x8_world-32x4x32",[8, 2, 8], [32, 4, 32]),
        // Closest to Oasis tile ratio: 93x34x84 model in 256x32x256 world =
        // ~3x4 horizontal tiles. Use 32x16x32 model in 96x16x96 world for
        // 3x3 = 9-tile equivalent.
        ("model-32x16x32_world-96x16x96",  [32, 16, 32], [96, 16, 96]),
    ];
    for (label, model_dims, world_dims) in tiled_fixtures {
        report.push_str(&format!(
            "--- tiled fixture {label}: model={:?} world={:?} ---\n",
            model_dims, world_dims
        ));
        match run_one_tiled_byte_diff(*model_dims, *world_dims) {
            Ok(s) => report.push_str(&s),
            Err(e) => report.push_str(&format!("FAILED: {e}\n")),
        }
        report.push('\n');
    }

    // ── Real Oasis model loaded from disk ───────────────────────────────────
    // Load the actual oasis_hard_cover.vox into a ModelData and exercise the
    // generator+chunk_calc chain on a few segments. Compares the per-segment
    // GPU output to the CPU oracle (generate_segment_cpu + construct of the
    // resulting DenseVolume). If THIS diverges where synthesized fixtures
    // didn't, the bug is triggered by the real-world model's specific
    // structure.
    report.push_str("\n=== Real Oasis ModelData + per-segment generator+chunk_calc byte-diff ===\n");
    let oasis_path = std::path::Path::new(
        "crates/bevy_naadf/assets/test/oasis_hard_cover.vox",
    );
    match load_oasis_model_data(oasis_path) {
        Ok(model) => {
            report.push_str(&format!(
                "Loaded Oasis ModelData: {}×{}×{} chunks (data_chunk={} data_block={} data_voxel={})\n",
                model.size_in_chunks[0], model.size_in_chunks[1], model.size_in_chunks[2],
                model.data_chunk.len(), model.data_block.len(), model.data_voxel.len(),
            ));
            // Run on a 4x4x4-chunk segment at offset (0,0,0) — should land
            // in the densely-populated corner of the model.
            for &(off, seg) in &[
                // Cover model interior — pick offsets inside the 93×34×84
                // model where content actually lives.
                ([0u32, 0, 0], [16u32, 16, 16]),
                ([16, 0, 16], [16, 16, 16]),
                ([32, 0, 32], [16, 16, 16]),
                ([48, 0, 48], [16, 16, 16]),
                ([60, 0, 60], [16, 16, 16]),
                // Edge-of-model segment to test wrap behaviour.
                ([76, 0, 64], [16, 16, 16]),
                // Segment that straddles the model's vertical extent (y=16+
                // crosses model y=2 boundary at chunk y=2).
                ([16, 0, 16], [16, 16, 16]),
            ] {
                report.push_str(&format!(
                    "--- oasis segment off={:?} seg={:?} ---\n", off, seg
                ));
                match run_oasis_segment_byte_diff(&model, off, seg) {
                    Ok(s) => report.push_str(&s),
                    Err(e) => report.push_str(&format!("FAILED: {e}\n")),
                }
                report.push('\n');
            }
        }
        Err(e) => {
            report.push_str(&format!("Failed to load Oasis VOX fixture: {e}\n"));
        }
    }

    eprintln!("{report}");
    Ok(report)
}

/// Scan an Oasis `ModelData` to find world voxel positions that are FULL in
/// the model AND fall within the fixed world's bounds. Returns up to `limit`
/// positions, distributed across the model's interior (not clustered).
///
/// Picks fixed positions deterministically (no RNG) for reproducibility.
fn discover_populated_oasis_voxels(
    model: &crate::aadf::generator::ModelData,
    world_voxels: [u32; 3],
    limit: usize,
) -> Vec<[u32; 3]> {
    let msc = model.size_in_chunks;
    let model_voxels = [msc[0] * 16, msc[1] * 16, msc[2] * 16];
    // Bound search to whichever is smaller — the model OR the world.
    let upper = [
        model_voxels[0].min(world_voxels[0]),
        model_voxels[1].min(world_voxels[1]),
        model_voxels[2].min(world_voxels[2]),
    ];
    let mut out = Vec::with_capacity(limit);
    // Probe by stepping every (dx, dy, dz) until we have `limit` populated
    // positions. Finer step sizes than naive max/N to ensure we sample the
    // model interior densely (Oasis is mostly air).
    let stride_x: u32 = (upper[0] / 16).max(8);
    let stride_y: u32 = (upper[1] / 24).max(4);
    let stride_z: u32 = (upper[2] / 16).max(8);
    let probe = |voxel_pos: [i64; 3]| -> u32 {
        // Inline the get_voxel_type_in_model logic (it's `fn` not `pub fn`,
        // so we replicate the few lines we need).
        if voxel_pos[0] < 0
            || voxel_pos[1] < 0
            || voxel_pos[2] < 0
            || voxel_pos[0] >= world_voxels[0] as i64
            || voxel_pos[1] >= world_voxels[1] as i64
            || voxel_pos[2] >= world_voxels[2] as i64
        {
            return 0;
        }
        let vx = voxel_pos[0] as u32;
        let vy = voxel_pos[1] as u32;
        let vz = voxel_pos[2] as u32;
        let model_extent_v = [msc[0] * 16, msc[1] * 16, msc[2] * 16];
        let vpim = [
            vx % model_extent_v[0],
            vy % model_extent_v[1],
            vz % model_extent_v[2],
        ];
        let model_index_y = vy / (msc[1] * 16);
        let cpim = [vpim[0] / 16, vpim[1] / 16, vpim[2] / 16];
        let chunk_index_in_model =
            (cpim[0] + cpim[1] * msc[0] + cpim[2] * msc[0] * msc[1]) as usize;
        let chunk = model.data_chunk[chunk_index_in_model];
        let mut ty: u32 = 0;
        let chunk_disc = chunk >> 30;
        if chunk_disc == 2 {
            let mbpic = [(vpim[0] % 16) / 4, (vpim[1] % 16) / 4, (vpim[2] % 16) / 4];
            let model_block_index = mbpic[0] + mbpic[1] * 4 + mbpic[2] * 16;
            let block_addr = ((chunk & 0x3FFF_FFFF) + model_block_index) as usize;
            let block = model.data_block[block_addr];
            let block_disc = block >> 30;
            if block_disc == 2 {
                let mvpic = [vpim[0] % 4, vpim[1] % 4, vpim[2] % 4];
                let model_voxel_index = mvpic[0] + mvpic[1] * 4 + mvpic[2] * 16;
                let voxel_addr =
                    ((block & 0x3FFF_FFFF) + model_voxel_index / 2) as usize;
                let voxel_comp = model.data_voxel[voxel_addr];
                ty = if model_voxel_index % 2 == 0 {
                    voxel_comp & 0x7FFF
                } else {
                    (voxel_comp >> 16) & 0x7FFF
                };
            } else if block_disc == 1 {
                ty = block & 0x3FFF_FFFF;
            }
        } else if chunk_disc == 1 {
            ty = chunk & 0x3FFF_FFFF;
        }
        if model_index_y > 0 {
            return 0;
        }
        ty
    };
    'outer: for vy in (0..upper[1]).step_by(stride_y as usize) {
        for vz in (0..upper[2]).step_by(stride_z as usize) {
            for vx in (0..upper[0]).step_by(stride_x as usize) {
                if probe([vx as i64, vy as i64, vz as i64]) != 0 {
                    out.push([vx, vy, vz]);
                    if out.len() >= limit {
                        break 'outer;
                    }
                }
            }
        }
    }
    // Pad with arbitrary positions if we didn't find enough — extremely
    // unlikely on Oasis but defensive against an empty-model future test.
    while out.len() < limit {
        out.push([0, 0, 0]);
    }
    out
}

/// vox-gpu-rewrite Stage 9 — production-scale voxels[] readback diagnostic.
///
/// Loads `oasis_hard_cover.vox` as `ModelData` and runs the FULL W5 producer
/// chain at production scale (256×32×256 chunk fixed world, 512 segments,
/// full bounds chain) in headless mode. Reads back `voxels[]` at TWO
/// checkpoints — post-producer (pre-bounds-calc) and post-bounds-calc — and
/// compares against the CPU oracle at ~25 sampled Oasis-populated voxel
/// positions.
///
/// The discriminating question this diagnostic answers (per
/// `docs/orchestrate/vox-gpu-rewrite/14-diagnostic-type-decode.md` follow-up):
///
/// - **If post-full-pipeline `voxels[]` is BYTE-EQUAL to CPU oracle** → the
///   bug is in the renderer's decode path (not in the producer chain).
/// - **If post-full-pipeline `voxels[]` DIFFERS from CPU oracle** → the bug
///   is in whichever stage corrupted it (`compute_voxel_bounds`'s leaf
///   writeback at `chunk_calc.wgsl:495-499`, or a downstream pass).
///
/// The Stage 6 diagnostic
/// (`docs/orchestrate/vox-gpu-rewrite/12-diagnostic-byte-diff-concrete.md`)
/// only sampled 6 Oasis segments at sub-production scale (16³-chunk segments
/// driven through their own fresh hash_map, NOT the production 512-segment
/// shared-hash-map + bounds chain pipeline). Stage 9 covers the gap.
///
/// CPU oracle methodology: for each sampled voxel position `(vx, vy, vz)`,
/// invoke `aadf::generator::get_voxel_type_in_model`-equivalent logic via
/// `generate_segment_cpu` over a single-chunk segment around the sample
/// position. The returned segment buffer holds the **producer-stage** voxel
/// type (the raw `voxel | (1<<15)` half-word emitted by `generator_model.wgsl`
/// before `chunk_calc` runs). That oracle's value is what the GPU's voxels[]
/// must contain at the leaf after the producer + bounds chain.
///
/// GPU pointer walk: at each sample position, decode the GPU's
/// `chunks[chunk_idx]`. If Mixed, dereference `blocks[BlockPtr +
/// block_idx_in_chunk]`. If Mixed, dereference `voxels[VoxelPtr + voxel_idx /
/// 2]`. Extract the half-word for the voxel's parity. Compare type-bit
/// equality.
///
/// The diagnostic is **DIAGNOSTIC ONLY** — it does not land a fix and does
/// not assert. It writes its report to stderr and returns `Ok(report)`. The
/// caller (in `bin/e2e_render.rs`) propagates the result as exit code 0; the
/// orchestration doc at
/// `docs/orchestrate/vox-gpu-rewrite/15-diagnostic-production-scale-readback.md`
/// summarises the findings.
pub fn validate_gpu_construction_production_scale() -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::generator::{generate_segment_cpu, CHUNK_DATA_U32S};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor,
        dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::generator_model::{
        create_params_uniform, create_storage_buffer_u32,
        dispatch_generator_model_with_encoder, generator_model_layout_descriptor,
        queue_generator_model_pipeline_with_handle, GpuGeneratorModelParams,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;

    let mut report = String::new();
    report.push_str(
        "=== vox-gpu-rewrite Stage 9 — production-scale voxels[] readback diagnostic ===\n\n",
    );

    // ── Load Oasis ModelData ─────────────────────────────────────────────────
    let oasis_path = std::path::Path::new(
        "crates/bevy_naadf/assets/test/oasis_hard_cover.vox",
    );
    let model = load_oasis_model_data(oasis_path)?;
    report.push_str(&format!(
        "Loaded Oasis ModelData: {}×{}×{} chunks (data_chunk={} data_block={} data_voxel={})\n",
        model.size_in_chunks[0],
        model.size_in_chunks[1],
        model.size_in_chunks[2],
        model.data_chunk.len(),
        model.data_block.len(),
        model.data_voxel.len(),
    ));

    // ── Fixed-world dispatch shape (mirrors production) ──────────────────────
    let world_chunks = [
        crate::WORLD_SIZE_IN_CHUNKS.x,
        crate::WORLD_SIZE_IN_CHUNKS.y,
        crate::WORLD_SIZE_IN_CHUNKS.z,
    ];
    let world_voxels = [
        crate::WORLD_SIZE_IN_VOXELS.x,
        crate::WORLD_SIZE_IN_VOXELS.y,
        crate::WORLD_SIZE_IN_VOXELS.z,
    ];
    let segment_chunks: u32 = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
    let world_segments = [
        crate::WORLD_SIZE_IN_SEGMENTS.x,
        crate::WORLD_SIZE_IN_SEGMENTS.y,
        crate::WORLD_SIZE_IN_SEGMENTS.z,
    ];
    let total_segments = world_segments[0] * world_segments[1] * world_segments[2];

    report.push_str(&format!(
        "Fixed-world: {}×{}×{} chunks ({}×{}×{} voxels), {}×{}×{} segments × {}³-chunk segments = {} total segments\n\n",
        world_chunks[0], world_chunks[1], world_chunks[2],
        world_voxels[0], world_voxels[1], world_voxels[2],
        world_segments[0], world_segments[1], world_segments[2],
        segment_chunks, total_segments,
    ));

    // ── Sample positions: discovered by scanning the Oasis model ─────────────
    //
    // The Oasis model is 1488×544×1344 voxels (93×34×84 chunks). When tiled
    // into the 4096×512×4096 world via `voxelPos % modelSize`, the model
    // repeats horizontally (3 X-tiles × 3 Z-tiles), and the Y-clamp
    // (`generator_model.fx:48`) zeros out everything above
    // `model_size_in_chunks.y * 16 = 544` voxels.
    //
    // Production world Y=512 voxels = 32 chunks. Model Y=544 voxels (>world
    // Y), so the Y-clamp does NOT fire on any world voxel — every world Y
    // position maps to model_index_y=0 (= the only copy in Y).
    //
    // Picking sample positions at arbitrary Y (e.g. Y=16) yields mostly
    // EMPTY voxels because the model's lower volume is mostly air. To make
    // the discriminating test meaningful we need positions that are KNOWN
    // FULL in the Oasis model — discovered by scanning the source ModelData
    // buffers for any voxel slot with a non-zero type.
    let sample_positions = discover_populated_oasis_voxels(&model, world_voxels, 25);
    report.push_str(&format!(
        "Sample positions: {} voxel positions, discovered by scanning Oasis ModelData for FULL voxels in the world's Y range.\n\n",
        sample_positions.len()
    ));
    let sample_positions: &[[u32; 3]] = sample_positions.as_slice();

    // ── Compute CPU oracle for every sample position ────────────────────────
    //
    // For each sample (vx,vy,vz), call `generate_segment_cpu` over a
    // single-chunk segment that contains the sample (chunk = vx/16, vy/16,
    // vz/16). Index into the returned segment buffer to retrieve the
    // half-word the producer would have written for that voxel.
    //
    // The segment buffer encoding (`generator_model.fx:70`):
    //   out[group_index * 2048 + local_index * 32 + i] = voxel1 | (voxel2 << 16);
    //   group_index = gx + gy*gscx + gz*gscx*gscy = 0 (single-chunk segment).
    //   local_index = lx + ly*4 + lz*16   (block position in chunk).
    //   i = voxel_pair_index_in_block (0..32; each pair = 2 voxels x even/odd).
    //
    // For a sample at world voxel (vx,vy,vz):
    //   chunk    = (vx/16, vy/16, vz/16)
    //   intra_c  = (vx%16, vy%16, vz%16)
    //   block    = intra_c / 4  → (bx,by,bz) in [0..4)
    //   intra_b  = intra_c % 4  → (lx,ly,lz) in [0..4)
    //   voxel_in_block_idx = lx + ly*4 + lz*16   (0..64)
    //   pair_idx = voxel_in_block_idx / 2
    //   parity   = voxel_in_block_idx % 2        (0=lo,1=hi)
    //
    // The producer-pass output `voxel_in_block_idx`-th voxel has half-word
    // `(out[block_index*32 + pair_idx] >> (16*parity)) & 0xFFFF`, where
    // `block_index = bx + by*4 + bz*16`.
    let cpu_oracle_halfwords: Vec<u16> = sample_positions
        .iter()
        .map(|&[vx, vy, vz]| {
            let cx = vx / 16;
            let cy = vy / 16;
            let cz = vz / 16;
            // Run generator over a single 1×1×1-chunk segment at that chunk.
            let seg = generate_segment_cpu(
                &model,
                [cx, cy, cz],
                [1, 1, 1],
                world_voxels,
            );
            // Decode the half-word at (vx,vy,vz).
            let lx = (vx % 16) % 4;
            let ly = (vy % 16) % 4;
            let lz = (vz % 16) % 4;
            let bx = (vx % 16) / 4;
            let by = (vy % 16) / 4;
            let bz = (vz % 16) / 4;
            let voxel_in_block_idx = lx + ly * 4 + lz * 16;
            let block_index = bx + by * 4 + bz * 16;
            let pair_idx = voxel_in_block_idx / 2;
            let parity = voxel_in_block_idx % 2;
            // group_index = 0 for a single-chunk segment.
            let chunk_base = 0usize;
            let pair_u32_idx = chunk_base + (block_index as usize) * 32 + pair_idx as usize;
            let pair_u32 = seg[pair_u32_idx];
            let half = if parity == 0 { pair_u32 & 0xFFFF } else { (pair_u32 >> 16) & 0xFFFF };
            half as u16
        })
        .collect();

    report.push_str("CPU oracle half-words computed for all sample positions.\n");
    let n_full = cpu_oracle_halfwords.iter().filter(|h| (**h & 0x8000) != 0).count();
    let n_empty = cpu_oracle_halfwords.len() - n_full;
    report.push_str(&format!(
        "  Oracle: {} positions FULL (bit-15 set), {} positions EMPTY.\n\n",
        n_full, n_empty,
    ));

    // ── Boot headless render world (Bevy MinimalPlugins + RenderPlugin) ─────
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

    let gen_shader =
        Shader::from_wgsl(GENERATOR_MODEL_SHADER_SRC, "shaders/generator_model.wgsl");
    let gen_shader_clone = gen_shader.clone();
    let gen_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(gen_shader);
    let calc_shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let calc_shader_clone = calc_shader.clone();
    let calc_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(calc_shader);

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp sub-app".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(gen_shader_handle.id(), gen_shader_clone);
        pc.set_shader(calc_shader_handle.id(), calc_shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no RenderDevice")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no RenderQueue")?
        .clone();

    let limits = device.limits();
    report.push_str(&format!(
        "Device: max_storage_buffer_binding_size = {} MiB; max_buffer_size = {} MiB.\n",
        limits.max_storage_buffer_binding_size / (1024 * 1024),
        limits.max_buffer_size / (1024 * 1024),
    ));

    // ── Allocate production-scale buffers ────────────────────────────────────
    //
    // Sizing mirrors `render/prepare.rs::prepare_world_gpu` when
    // `gpu_producer_enabled = true`:
    //   chunk_count       = 256 * 32 * 256 = 2,097,152
    //   blocks_alloc_len  = chunk_count * 64 = 134,217,728  (512 MiB)
    //   voxels_alloc_len  = chunk_count * 128 = 268,435,456 (1024 MiB)
    let chunk_count: u64 =
        (world_chunks[0] as u64) * (world_chunks[1] as u64) * (world_chunks[2] as u64);
    let blocks_alloc_len: u64 = chunk_count * 64;
    let voxels_alloc_len: u64 = chunk_count * 128;
    report.push_str(&format!(
        "Allocations: chunks={} pairs ({} MiB), blocks={} u32 ({} MiB), voxels={} u32 ({} MiB)\n",
        chunk_count, (chunk_count * 8) / (1024 * 1024),
        blocks_alloc_len, (blocks_alloc_len * 4) / (1024 * 1024),
        voxels_alloc_len, (voxels_alloc_len * 4) / (1024 * 1024),
    ));
    if (blocks_alloc_len * 4) > limits.max_storage_buffer_binding_size as u64
        || (voxels_alloc_len * 4) > limits.max_storage_buffer_binding_size as u64
        || (chunk_count * 8) > limits.max_storage_buffer_binding_size as u64
    {
        report.push_str(&format!(
            "  WARNING: an allocation exceeds device max_storage_buffer_binding_size ({} MiB).\n",
            limits.max_storage_buffer_binding_size / (1024 * 1024),
        ));
    }

    // Hash map sized as production does (`ConstructionConfig::initial_hash_map_size`
    // = 1<<20 = 1048576 slots, * 4 u32/slot = 16 MiB).
    let hash_map_size_slots: u32 = crate::render::construction::config::ConstructionConfig::default().initial_hash_map_size;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let mk_storage = |label: &'static str, data_len_u32: u64| {
        let size_bytes = data_len_u32 * 4;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: size_bytes,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Zero-initialise via a single 1 MiB chunk loop to bound peak memory.
        // Production buffers are NOT pre-zero'd because wgpu/Bevy sets up the
        // backing store; per Stage 8's Q4 verify, the post-allocation state is
        // de-facto zero on the user's machine. For diagnostic determinism we
        // zero them explicitly.
        const ZERO_CHUNK: usize = 1024 * 1024 / 4; // 1 MiB / 4 = 256 K u32s.
        let zero_chunk = vec![0u32; ZERO_CHUNK];
        let mut written: u64 = 0;
        while written < data_len_u32 {
            let remaining = (data_len_u32 - written) as usize;
            let n = remaining.min(zero_chunk.len());
            queue.write_buffer(
                &buffer,
                written * 4,
                bytemuck::cast_slice(&zero_chunk[..n]),
            );
            written += n as u64;
        }
        buffer
    };

    report.push_str("Allocating GPU buffers (zero-init via 1 MiB chunked writes)…\n");
    let gpu_blocks = mk_storage("prod_blocks", blocks_alloc_len);
    let gpu_voxels = mk_storage("prod_voxels", voxels_alloc_len);
    let chunks_buf = device.create_buffer(&BufferDescriptor {
        label: Some("prod_chunks"),
        size: chunk_count * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Zero-init chunks too.
    {
        const ZERO_PAIRS: usize = 1024 * 1024 / 8; // 1 MiB / 8 = 128K pairs.
        let zero_pairs = vec![[0u32; 2]; ZERO_PAIRS];
        let mut written_pairs: u64 = 0;
        while written_pairs < chunk_count {
            let remaining = (chunk_count - written_pairs) as usize;
            let n = remaining.min(zero_pairs.len());
            queue.write_buffer(
                &chunks_buf,
                written_pairs * 8,
                bytemuck::cast_slice(&zero_pairs[..n]),
            );
            written_pairs += n as u64;
        }
    }

    let gpu_block_voxel_count = device.create_buffer(&BufferDescriptor {
        label: Some("prod_bvc"),
        size: 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &gpu_block_voxel_count,
        0,
        bytemuck::cast_slice(&block_voxel_count_init),
    );

    let gpu_hash_map = mk_storage("prod_hashmap", (hash_map_size_slots as u64) * 4);
    // Hash map seed is zero — already zero'd by `mk_storage`. The W5 producer
    // re-initialises via the chunk_calc shader.
    let _ = hash_map_init;

    let gpu_coeffs = create_storage_buffer_u32(&device, &queue, "prod_coeffs", &coeffs);

    // Segment buffer — one segment's worth, 16³ chunks × 2048 u32s ≈ 32 MiB.
    let seg = segment_chunks as usize;
    let segment_buf_u32s = seg * seg * seg * (CHUNK_DATA_U32S as usize);
    let segment_buf = device.create_buffer(&BufferDescriptor {
        label: Some("prod_segment"),
        size: (segment_buf_u32s as u64) * 4,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&segment_buf, 0, bytemuck::cast_slice(&vec![0u32; segment_buf_u32s]));

    let model_chunk_buf =
        create_storage_buffer_u32(&device, &queue, "prod_model_chunk", &model.data_chunk);
    let model_block_buf =
        create_storage_buffer_u32(&device, &queue, "prod_model_block", &model.data_block);
    let model_voxel_buf =
        create_storage_buffer_u32(&device, &queue, "prod_model_voxel", &model.data_voxel);

    let gen_params = GpuGeneratorModelParams {
        size_in_voxels: world_voxels,
        _pad0: 0,
        model_size_in_chunks: model.size_in_chunks,
        _pad1: 0,
        group_offset_in_chunks: [0, 0, 0],
        group_size_in_chunks_x: segment_chunks,
        group_size_in_chunks_y: segment_chunks,
        _pad2: 0,
        _pad3: 0,
        _pad4: 0,
    };
    let gen_params_buf = create_params_uniform(&device, &queue, &gen_params);

    let calc_params = GpuConstructionParams {
        size_in_chunks: world_chunks,
        _pad0: 0,
        group_size_in_groups: [0, 0, 0], // bounds-calc-relevant; not used by chunk_calc
        _pad1: 0,
        bound_group_queue_max_size: 1,
        hash_map_size: hash_map_size_slots,
        segment_size_in_chunks: segment_chunks,
        max_group_bound_dispatch: 0,
        chunk_offset: [0, 0, 0],
        dispatch_offset: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let calc_params_buf = device.create_buffer(&BufferDescriptor {
        label: Some("prod_calc_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&calc_params_buf, 0, bytemuck::bytes_of(&calc_params));

    // ── Pipelines ────────────────────────────────────────────────────────────
    let gen_layout = generator_model_layout_descriptor();
    let calc_layout = construction_world_layout_descriptor();
    let (id_gen, id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        (
            queue_generator_model_pipeline_with_handle(
                cache,
                gen_layout.clone(),
                gen_shader_handle.clone(),
            ),
            queue_calc_block_pipeline_with_handle(
                cache,
                calc_layout.clone(),
                calc_shader_handle.clone(),
            ),
            queue_voxel_bounds_pipeline_with_handle(
                cache,
                calc_layout.clone(),
                calc_shader_handle.clone(),
            ),
            queue_block_bounds_pipeline_with_handle(
                cache,
                calc_layout.clone(),
                calc_shader_handle.clone(),
            ),
        )
    };

    let mut pipelines: Option<(
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
    )> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..128 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(g), Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_gen),
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some((g.clone(), a.clone(), b.clone(), c.clone()));
            break;
        }
    }
    let (p_gen, p_calc, p_voxel, p_block) =
        pipelines.ok_or("production pipelines did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let gen_bgl = cache.get_bind_group_layout(&gen_layout);
    let gen_bg = device.create_bind_group(
        "prod_gen_bg",
        &gen_bgl,
        &BindGroupEntries::sequential((
            segment_buf.as_entire_buffer_binding(),
            model_chunk_buf.as_entire_buffer_binding(),
            model_block_buf.as_entire_buffer_binding(),
            model_voxel_buf.as_entire_buffer_binding(),
            gen_params_buf.as_entire_buffer_binding(),
        )),
    );
    let calc_bgl = cache.get_bind_group_layout(&calc_layout);
    let calc_bg = device.create_bind_group(
        "prod_calc_bg",
        &calc_bgl,
        &BindGroupEntries::sequential((
            chunks_buf.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            segment_buf.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            calc_params_buf.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Per-segment producer loop (mirrors production W5
    //    `naadf_gpu_producer_node` `mod.rs:2454-2566`) ─────────────────────
    let group_size_in_chunks = [segment_chunks, segment_chunks, segment_chunks];
    let mut seg_count = 0u32;
    report.push_str("Dispatching 512 per-segment generator+chunk_calc dispatches…\n");
    for sz in 0..world_segments[2] {
        for sy in 0..world_segments[1] {
            for sx in 0..world_segments[0] {
                let chunk_offset = [
                    sx * segment_chunks,
                    sy * segment_chunks,
                    sz * segment_chunks,
                ];
                let gen_params = GpuGeneratorModelParams {
                    size_in_voxels: world_voxels,
                    _pad0: 0,
                    model_size_in_chunks: model.size_in_chunks,
                    _pad1: 0,
                    group_offset_in_chunks: chunk_offset,
                    group_size_in_chunks_x: segment_chunks,
                    group_size_in_chunks_y: segment_chunks,
                    _pad2: 0,
                    _pad3: 0,
                    _pad4: 0,
                };
                queue.write_buffer(&gen_params_buf, 0, bytemuck::bytes_of(&gen_params));
                let calc_params = GpuConstructionParams {
                    size_in_chunks: world_chunks,
                    _pad0: 0,
                    group_size_in_groups: [0, 0, 0],
                    _pad1: 0,
                    bound_group_queue_max_size: 1,
                    hash_map_size: hash_map_size_slots,
                    segment_size_in_chunks: segment_chunks,
                    max_group_bound_dispatch: 0,
                    chunk_offset,
                    dispatch_offset: 0,
                    frame_index: 0,
                    changed_chunk_count: 0,
                    changed_block_count: 0,
                    changed_voxel_count: 0,
                };
                queue.write_buffer(&calc_params_buf, 0, bytemuck::bytes_of(&calc_params));
                let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("prod_segment_enc"),
                });
                dispatch_generator_model_with_encoder(
                    &mut enc,
                    &p_gen,
                    &gen_bg,
                    group_size_in_chunks,
                );
                dispatch_calc_block_from_raw_data_world_sized(
                    &mut enc,
                    &p_calc,
                    &calc_bg,
                    group_size_in_chunks,
                );
                queue.submit([enc.finish()]);
                seg_count += 1;
                if seg_count.is_multiple_of(64) {
                    // Flush periodically so we don't accumulate too much in-flight.
                    device.poll(PollType::wait_indefinitely()).unwrap();
                }
            }
        }
    }
    // Final flush of the producer loop.
    device.poll(PollType::wait_indefinitely()).unwrap();
    report.push_str(&format!("  Producer loop done: {} segments dispatched.\n", seg_count));

    // Readback cursor after producer.
    let cursor_after_producer = readback_cursor(&device, &queue, &gpu_block_voxel_count);
    report.push_str(&format!(
        "  Cursors post-producer: block_voxel_count[0]={} (voxel-pair cursor), [1]={} (block-u32 cursor)\n\n",
        cursor_after_producer[0], cursor_after_producer[1],
    ));

    // ── CHECKPOINT A: read back post-producer (pre-bounds-calc) ─────────────
    report.push_str("=== CHECKPOINT A: post-W5-producer, pre-bounds-calc ===\n");
    let post_producer_results = sample_voxel_readback(
        &device,
        &queue,
        &chunks_buf,
        &gpu_blocks,
        &gpu_voxels,
        sample_positions,
        &cpu_oracle_halfwords,
        world_chunks,
    );
    report.push_str(&render_results_table(
        "post-producer",
        sample_positions,
        &cpu_oracle_halfwords,
        &post_producer_results,
    ));

    // ── Dispatch the bounds chain (mirrors production
    //    `mod.rs:2622-2634`) ───────────────────────────────────────────────
    let world_chunks_total = chunk_count;
    let max_blocks_u64 = world_chunks_total * 64;
    let max_voxels_u64 = max_blocks_u64 * 32;
    let voxel_workgroups =
        ((max_voxels_u64 / 32 + 1).max(1)).min(u32::MAX as u64) as u32;
    let block_workgroups =
        ((max_blocks_u64 / 64 + 1).max(1)).min(u32::MAX as u64) as u32;
    report.push_str(&format!(
        "Dispatching bounds chain: voxel_workgroups={} block_workgroups={}\n",
        voxel_workgroups, block_workgroups,
    ));
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("prod_bounds_enc"),
    });
    dispatch_compute_voxel_bounds(&mut enc, &p_voxel, &calc_bg, voxel_workgroups);
    dispatch_compute_block_bounds(&mut enc, &p_block, &calc_bg, block_workgroups);
    queue.submit([enc.finish()]);
    device.poll(PollType::wait_indefinitely()).unwrap();
    report.push_str("  Bounds chain complete.\n\n");

    // ── CHECKPOINT B: read back post-bounds-calc ────────────────────────────
    report.push_str("=== CHECKPOINT B: post-bounds-calc (after compute_voxel_bounds + compute_block_bounds) ===\n");
    let post_bounds_results = sample_voxel_readback(
        &device,
        &queue,
        &chunks_buf,
        &gpu_blocks,
        &gpu_voxels,
        sample_positions,
        &cpu_oracle_halfwords,
        world_chunks,
    );
    report.push_str(&render_results_table(
        "post-bounds",
        sample_positions,
        &cpu_oracle_halfwords,
        &post_bounds_results,
    ));

    // ── Cross-checkpoint diff analysis ──────────────────────────────────────
    report.push_str("\n=== Pattern analysis: post-producer vs post-bounds ===\n");
    let mut a_b_changed = 0usize;
    let mut a_match_b_mismatch = 0usize;
    let mut a_mismatch_b_match = 0usize;
    let mut both_match = 0usize;
    let mut both_mismatch = 0usize;
    for (i, &[vx, vy, vz]) in sample_positions.iter().enumerate() {
        let oracle = cpu_oracle_halfwords[i];
        let a = &post_producer_results[i];
        let b = &post_bounds_results[i];
        let a_match = a.matches_oracle(oracle);
        let b_match = b.matches_oracle(oracle);
        if a.gpu_voxel_halfword != b.gpu_voxel_halfword
            || a.gpu_chunk_u32 != b.gpu_chunk_u32
            || a.gpu_block_u32 != b.gpu_block_u32
        {
            a_b_changed += 1;
            report.push_str(&format!(
                "  pos=({vx},{vy},{vz}): A→B CHANGED  | chunk {:#010x}→{:#010x}  block {:#010x}→{:#010x}  voxel-half {:#06x}→{:#06x}\n",
                a.gpu_chunk_u32, b.gpu_chunk_u32,
                a.gpu_block_u32, b.gpu_block_u32,
                a.gpu_voxel_halfword, b.gpu_voxel_halfword,
            ));
        }
        match (a_match, b_match) {
            (true, true) => both_match += 1,
            (false, false) => both_mismatch += 1,
            (true, false) => a_match_b_mismatch += 1,
            (false, true) => a_mismatch_b_match += 1,
        }
    }
    report.push_str(&format!(
        "  Summary: A→B changed in {} of {} positions; both-match={} both-mismatch={} match→mismatch={} mismatch→match={}\n",
        a_b_changed, sample_positions.len(),
        both_match, both_mismatch, a_match_b_mismatch, a_mismatch_b_match,
    ));

    // ── Verdict ─────────────────────────────────────────────────────────────
    report.push_str("\n=== Verdict ===\n");
    let n_mismatch_b = post_bounds_results
        .iter()
        .enumerate()
        .filter(|(i, r)| !r.matches_oracle(cpu_oracle_halfwords[*i]))
        .count();
    let n_mismatch_a = post_producer_results
        .iter()
        .enumerate()
        .filter(|(i, r)| !r.matches_oracle(cpu_oracle_halfwords[*i]))
        .count();
    report.push_str(&format!(
        "  Post-producer mismatches: {} of {}\n",
        n_mismatch_a,
        sample_positions.len()
    ));
    report.push_str(&format!(
        "  Post-bounds mismatches: {} of {}\n",
        n_mismatch_b,
        sample_positions.len()
    ));
    if n_mismatch_b == 0 {
        report.push_str(
            "  → voxels[] is byte-correct post-full-pipeline at every sampled position.\n\
             → Bug must be in the RENDERER's decode path (or elsewhere outside the producer/bounds chain).\n",
        );
    } else if n_mismatch_a == 0 && n_mismatch_b > 0 {
        report.push_str(
            "  → voxels[] was correct after the producer, but DIVERGED after bounds-calc ran.\n\
             → Bug is in `compute_voxel_bounds` / `compute_block_bounds` writeback.\n",
        );
    } else if n_mismatch_a > 0 && n_mismatch_b > 0 {
        report.push_str(
            "  → voxels[] was ALREADY corrupted after the producer (Stage 6 gap).\n\
             → Bug is in W5 producer chain when run at full production shape (512 segments + shared hash_map).\n",
        );
    } else {
        report.push_str(
            "  → Producer broke voxels[] but bounds-calc somehow restored them?? Inspect per-position rows above.\n",
        );
    }

    let _ = total_segments;

    eprintln!("{report}");
    Ok(report)
}

/// Result of a single GPU pointer walk for one sample position. Captured by
/// [`sample_voxel_readback`] at each checkpoint.
struct VoxelReadback {
    /// Decoded `chunks[chunk_idx]` (low 32 bits of the vec2<u32> pair —
    /// matches the renderer's read pattern at `ray_tracing.wgsl`).
    gpu_chunk_u32: u32,
    /// Decoded `blocks[BlockPtr + block_idx_in_chunk]`. `None` if the chunk
    /// wasn't mixed.
    gpu_block_u32: u32,
    /// Decoded `voxels[VoxelPtr + voxel_in_block_idx / 2]`. `None` if the
    /// block wasn't mixed.
    gpu_voxel_pair_u32: u32,
    /// The half-word for this voxel's parity.
    gpu_voxel_halfword: u16,
    /// Hit kind: 0=empty-chunk, 1=uniform-full-chunk, 2=mixed-chunk-empty-block,
    /// 3=mixed-chunk-uniform-full-block, 4=mixed-chunk-mixed-block-empty-voxel,
    /// 5=mixed-chunk-mixed-block-full-voxel, 6=out-of-allocated-region.
    kind: u8,
}

impl VoxelReadback {
    fn matches_oracle(&self, oracle_half: u16) -> bool {
        // The oracle half-word is what `generate_segment_cpu` produces — the
        // post-producer pre-chunk_calc encoding (bit 15 = full, low 15 = type).
        // After chunk_calc/bounds, the FULL voxel half should equal the
        // oracle's full voxel half (type bits preserved). For an EMPTY oracle
        // half (bit 15 clear), the result is uniform empty or the AADF
        // half-word — either is "correct" downstream of the producer pass, so
        // we only assert correctness on FULL oracle positions.
        let oracle_full = (oracle_half & 0x8000) != 0;
        if !oracle_full {
            // Oracle says EMPTY here. The GPU may decode this position as
            // empty-chunk (kind 0), empty-block (kind 2), or
            // empty-voxel (kind 4). Anything ELSE is unexpected. We
            // tolerate AADF bits differing.
            matches!(self.kind, 0 | 2 | 4)
        } else {
            // Oracle says FULL with a specific type. The GPU must decode this
            // position as full with the SAME type — uniform-full chunk,
            // uniform-full block, or full voxel.
            let oracle_type = oracle_half & 0x7FFF;
            match self.kind {
                1 => self.gpu_chunk_u32 & 0x7FFF == oracle_type as u32,
                3 => self.gpu_block_u32 & 0x7FFF == oracle_type as u32,
                5 => {
                    let gpu_full = (self.gpu_voxel_halfword & 0x8000) != 0;
                    let gpu_type = self.gpu_voxel_halfword & 0x7FFF;
                    gpu_full && gpu_type == oracle_type
                }
                _ => false,
            }
        }
    }
}

/// Read back the 2 cursor counters (`block_voxel_count[0..2]`).
fn readback_cursor(
    device: &bevy::render::renderer::RenderDevice,
    queue: &bevy::render::renderer::RenderQueue,
    buf: &bevy::render::render_resource::Buffer,
) -> [u32; 2] {
    use bevy::render::render_resource::{
        BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
    };
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("prod_cur_staging"),
        size: 8,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("prod_cur_enc"),
    });
    enc.copy_buffer_to_buffer(buf, 0, &staging, 0, 8);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging.unmap();
    [v[0], v[1]]
}

/// Map back a single `u32`-element at a precise offset within a GPU storage
/// buffer. Used for surgical sampling of multi-GB buffers (no full readback —
/// 1 GiB voxels[] readback would push the OS to swap).
fn map_single_u32(
    device: &bevy::render::renderer::RenderDevice,
    queue: &bevy::render::renderer::RenderQueue,
    buf: &bevy::render::render_resource::Buffer,
    u32_index: u64,
) -> u32 {
    use bevy::render::render_resource::{
        BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
    };
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("prod_single_u32_staging"),
        size: 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("prod_single_u32_enc"),
    });
    enc.copy_buffer_to_buffer(buf, u32_index * 4, &staging, 0, 4);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let v: u32 = bytemuck::cast_slice::<_, u32>(&data)[0];
    drop(data);
    staging.unmap();
    v
}

/// Map back a single `vec2<u32>` (8-byte pair) at a precise pair-index within
/// a `chunks` storage buffer.
fn map_single_pair(
    device: &bevy::render::renderer::RenderDevice,
    queue: &bevy::render::renderer::RenderQueue,
    buf: &bevy::render::render_resource::Buffer,
    pair_index: u64,
) -> [u32; 2] {
    use bevy::render::render_resource::{
        BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
    };
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("prod_single_pair_staging"),
        size: 8,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("prod_single_pair_enc"),
    });
    enc.copy_buffer_to_buffer(buf, pair_index * 8, &staging, 0, 8);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let v: &[u32] = bytemuck::cast_slice(&data);
    let pair = [v[0], v[1]];
    drop(data);
    staging.unmap();
    pair
}

/// For each sample position, walk the GPU pointer chain (chunks → blocks →
/// voxels) and capture the decoded values + the leaf voxel half-word.
fn sample_voxel_readback(
    device: &bevy::render::renderer::RenderDevice,
    queue: &bevy::render::renderer::RenderQueue,
    chunks_buf: &bevy::render::render_resource::Buffer,
    gpu_blocks: &bevy::render::render_resource::Buffer,
    gpu_voxels: &bevy::render::render_resource::Buffer,
    positions: &[[u32; 3]],
    _oracle: &[u16],
    world_chunks: [u32; 3],
) -> Vec<VoxelReadback> {
    let mut out = Vec::with_capacity(positions.len());
    for &[vx, vy, vz] in positions {
        let cx = vx / 16;
        let cy = vy / 16;
        let cz = vz / 16;
        let chunk_idx = (cx as u64)
            + (cy as u64) * (world_chunks[0] as u64)
            + (cz as u64) * (world_chunks[0] as u64) * (world_chunks[1] as u64);
        let chunk_pair = map_single_pair(device, queue, chunks_buf, chunk_idx);
        let chunk_u32 = chunk_pair[0];

        // Decode chunk classification (`aadf::cell::ChunkCell::decode`):
        //   bit 31 set = mixed (low 30 = BlockPtr)
        //   bit 30 set = uniform-full (low 15 = type)
        //   else        = empty (low 30 = 6×5-bit AADF)
        const CELL_HAS_CHILDREN: u32 = 1 << 31;
        const CELL_UNIFORM_FULL: u32 = 1 << 30;
        const CELL_PAYLOAD_MASK: u32 = (1 << 30) - 1;
        if (chunk_u32 & CELL_HAS_CHILDREN) == 0 {
            // Empty or uniform-full chunk.
            if (chunk_u32 & CELL_UNIFORM_FULL) != 0 {
                out.push(VoxelReadback {
                    gpu_chunk_u32: chunk_u32,
                    gpu_block_u32: 0,
                    gpu_voxel_pair_u32: 0,
                    gpu_voxel_halfword: 0,
                    kind: 1,
                });
            } else {
                out.push(VoxelReadback {
                    gpu_chunk_u32: chunk_u32,
                    gpu_block_u32: 0,
                    gpu_voxel_pair_u32: 0,
                    gpu_voxel_halfword: 0,
                    kind: 0,
                });
            }
            continue;
        }
        // Mixed chunk: follow BlockPtr.
        let block_ptr = (chunk_u32 & CELL_PAYLOAD_MASK) as u64;
        let bx = (vx % 16) / 4;
        let by = (vy % 16) / 4;
        let bz = (vz % 16) / 4;
        let block_in_chunk = bx + by * 4 + bz * 16;
        let block_offset = block_ptr + block_in_chunk as u64;
        let block_u32 = map_single_u32(device, queue, gpu_blocks, block_offset);

        if (block_u32 & CELL_HAS_CHILDREN) == 0 {
            if (block_u32 & CELL_UNIFORM_FULL) != 0 {
                out.push(VoxelReadback {
                    gpu_chunk_u32: chunk_u32,
                    gpu_block_u32: block_u32,
                    gpu_voxel_pair_u32: 0,
                    gpu_voxel_halfword: 0,
                    kind: 3,
                });
            } else {
                out.push(VoxelReadback {
                    gpu_chunk_u32: chunk_u32,
                    gpu_block_u32: block_u32,
                    gpu_voxel_pair_u32: 0,
                    gpu_voxel_halfword: 0,
                    kind: 2,
                });
            }
            continue;
        }
        // Mixed block: follow VoxelPtr.
        let voxel_ptr = (block_u32 & CELL_PAYLOAD_MASK) as u64;
        let lx = (vx % 16) % 4;
        let ly = (vy % 16) % 4;
        let lz = (vz % 16) % 4;
        let voxel_in_block_idx = lx + ly * 4 + lz * 16;
        let pair_idx = voxel_in_block_idx / 2;
        let parity = voxel_in_block_idx % 2;
        let voxel_u32 = map_single_u32(device, queue, gpu_voxels, voxel_ptr + pair_idx as u64);
        let half = if parity == 0 { voxel_u32 & 0xFFFF } else { (voxel_u32 >> 16) & 0xFFFF };
        let kind = if (half & 0x8000) != 0 { 5 } else { 4 };
        out.push(VoxelReadback {
            gpu_chunk_u32: chunk_u32,
            gpu_block_u32: block_u32,
            gpu_voxel_pair_u32: voxel_u32,
            gpu_voxel_halfword: half as u16,
            kind,
        });
    }
    out
}

/// Format the per-sample readback table for inclusion in the report.
fn render_results_table(
    label: &str,
    positions: &[[u32; 3]],
    oracle: &[u16],
    results: &[VoxelReadback],
) -> String {
    let mut s = String::new();
    s.push_str(&format!("\n[{label} table — voxel position | oracle half | GPU chunk | GPU block | GPU voxel-pair | GPU voxel-half | kind | match?]\n"));
    for (i, &[vx, vy, vz]) in positions.iter().enumerate() {
        let r = &results[i];
        let oracle_half = oracle[i];
        let oracle_full = (oracle_half & 0x8000) != 0;
        let oracle_type = oracle_half & 0x7FFF;
        let m = r.matches_oracle(oracle_half);
        let kind_name = match r.kind {
            0 => "EMPTY-CHUNK",
            1 => "UNIFORM-FULL-CHUNK",
            2 => "MIXED-CHUNK / EMPTY-BLOCK",
            3 => "MIXED-CHUNK / UNIFORM-FULL-BLOCK",
            4 => "MIXED-CHUNK / MIXED-BLOCK / EMPTY-VOXEL",
            5 => "MIXED-CHUNK / MIXED-BLOCK / FULL-VOXEL",
            _ => "?",
        };
        let xor_half =
            (r.gpu_voxel_halfword as u32) ^ (oracle_half as u32);
        s.push_str(&format!(
            "  ({:>4},{:>3},{:>4}) | oracle={:#06x} {} (type={:#06x}) | chunk={:#010x} | block={:#010x} | vpair={:#010x} | vhalf={:#06x} (type={:#06x}) | {} | XOR-half={:#06x} | {}\n",
            vx, vy, vz,
            oracle_half, if oracle_full { "FULL " } else { "EMPTY" }, oracle_type,
            r.gpu_chunk_u32, r.gpu_block_u32, r.gpu_voxel_pair_u32, r.gpu_voxel_halfword,
            r.gpu_voxel_halfword & 0x7FFF,
            kind_name,
            xor_half,
            if m { "MATCH" } else { "MISMATCH" },
        ));
    }
    s
}

/// Per-chunk content diversity mode for the scaled byte-diff fixture sweep.
#[derive(Clone, Copy, Debug)]
enum FixtureMode {
    /// All mixed blocks across all chunks have identical content — dedup-hit
    /// path dominates.
    Uniform,
    /// Every chunk has UNIQUE mixed-block content — new-slot CAS dominates.
    Diverse,
    /// Half chunks uniform, half diverse — exercises both paths in one
    /// dispatch.
    Mixed,
}

/// Run the W5 chunk_calc chain on a single fixture and report byte-level +
/// semantic-content divergence vs the CPU oracle.
fn run_one_fixture_byte_diff(
    size_in_chunks: [u32; 3],
    mode: FixtureMode,
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;
    use crate::voxel::VoxelTypeId;

    // ── Build the fixture volume ──────────────────────────────────────────────
    // Each chunk gets ONE mixed block (block (0,0,0) of the chunk) with one
    // full voxel. The voxel's position inside the block depends on `mode`:
    //   Uniform: voxel at (0,0,0) of the block — every mixed block has
    //            identical content → all dedup-hit.
    //   Diverse: voxel position varies per chunk (cycles through 64 positions)
    //            → every mixed block claims a unique slot.
    //   Mixed:   even-parity chunks uniform, odd-parity chunks diverse.
    let mut volume = DenseVolume::empty(size_in_chunks);
    let sv = volume.size_in_voxels();
    for cz in 0..size_in_chunks[2] {
        for cy in 0..size_in_chunks[1] {
            for cx in 0..size_in_chunks[0] {
                let chunk_idx = cx + cy * size_in_chunks[0]
                    + cz * size_in_chunks[0] * size_in_chunks[1];
                let pos_in_block: u32 = match mode {
                    FixtureMode::Uniform => 0,
                    FixtureMode::Diverse => chunk_idx % 64,
                    FixtureMode::Mixed => {
                        if chunk_idx % 2 == 0 { 0 } else { (chunk_idx / 2) % 64 }
                    }
                };
                // Decode pos_in_block (0..64) into (lx, ly, lz) within the
                // 4×4×4 block.
                let lx = pos_in_block % 4;
                let ly = (pos_in_block / 4) % 4;
                let lz = pos_in_block / 16;
                // Block (0,0,0) of this chunk — its world voxel base is
                // (cx*16, cy*16, cz*16); add (lx, ly, lz).
                let vx = cx * 16 + lx;
                let vy = cy * 16 + ly;
                let vz = cz * 16 + lz;
                if vx < sv[0] && vy < sv[1] && vz < sv[2] {
                    volume.set([vx, vy, vz], VoxelTypeId(7));
                }
            }
        }
    }

    let oracle = construct(&volume);

    // ── Boot headless render world ────────────────────────────────────────────
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

    let shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp sub-app available".into());
    };
    {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no RenderDevice")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no RenderQueue")?
        .clone();

    // ── Allocate GPU buffers ──────────────────────────────────────────────────
    // For a fixture extent S = size_in_chunks, the segment buffer holds S^3
    // chunks × 2048 u32s per chunk = S^3 * 2048 u32s; the GPU uses
    // `params.segment_size_in_chunks` to index, so set it to max(size_in_chunks)
    // and dispatch the actual world extent via `_world_sized`.
    let segment_size_in_chunks: u32 = size_in_chunks[0].max(size_in_chunks[1]).max(size_in_chunks[2]);
    let segment_voxels = build_segment_voxel_buffer_for_world(&volume, segment_size_in_chunks);

    // Worst-case block/voxel sizing for the GPU buffers (cursor seed = 64 ;
    // chunks × 64 blocks/chunk for blocks ; chunks × 64 voxels/chunk ×
    // (1 u32 / 2 voxels) = chunks × 32 u32s for voxels). We pad generously.
    let chunk_count = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
    let max_blocks = 64 + chunk_count * 64;
    let max_voxels_u32 = 64 + chunk_count * 64 * 32; // worst case no dedup
    // Cap the hash map at 1M slots (16 MB) — large enough that any fixture's
    // expected unique-block count fits with low load factor, but small
    // enough that allocation never fails silently at million-chunk scale.
    // Production uses 256K initially per `ConstructionConfig::initial_hash_map_size`.
    let hash_map_size_slots: u32 = 1 << 20;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let mk_storage = |label: &'static str, data: &[u32]| {
        let data = if data.is_empty() { &[0u32][..] } else { data };
        let size = (data.len() * 4) as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
        buffer
    };

    let gpu_blocks = mk_storage("scaled_blocks", &vec![0u32; max_blocks]);
    let gpu_voxels = mk_storage("scaled_voxels", &vec![0u32; max_voxels_u32]);
    let gpu_block_voxel_count = mk_storage("scaled_bvc", &block_voxel_count_init);
    let gpu_segment = mk_storage("scaled_segment", &segment_voxels);
    let gpu_hash_map = mk_storage("scaled_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("scaled_coeffs", &coeffs);

    let params = GpuConstructionParams {
        size_in_chunks,
        _pad0: 0,
        group_size_in_groups: [1, 1, 1],
        _pad1: 0,
        bound_group_queue_max_size: 1,
        hash_map_size: hash_map_size_slots,
        segment_size_in_chunks,
        max_group_bound_dispatch: 0,
        chunk_offset: [0, 0, 0],
        dispatch_offset: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let params_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("scaled_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("scaled_chunks"),
        size: (chunk_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    // ── Pipelines ────────────────────────────────────────────────────────────
    let layout = construction_world_layout_descriptor();
    let (id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let a = queue_calc_block_pipeline_with_handle(cache, layout.clone(), shader_handle.clone());
        let b = queue_voxel_bounds_pipeline_with_handle(cache, layout.clone(), shader_handle.clone());
        let c = queue_block_bounds_pipeline_with_handle(cache, layout.clone(), shader_handle.clone());
        (a, b, c)
    };

    let mut pipelines: Option<Vec<bevy::render::render_resource::ComputePipeline>> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some(vec![a.clone(), b.clone(), c.clone()]);
            break;
        }
    }
    let pipelines = pipelines.ok_or("pipelines did not compile")?;

    // ── Bind group ───────────────────────────────────────────────────────────
    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout);
    let bind_group = device.create_bind_group(
        "scaled_bind_group",
        &bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            gpu_segment.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            params_buffer.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch chunk_calc over the actual world extent ─────────────────────
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("scaled_calc_block"),
    });
    dispatch_calc_block_from_raw_data_world_sized(
        &mut encoder,
        &pipelines[0],
        &bind_group,
        size_in_chunks,
    );
    queue.submit([encoder.finish()]);

    // Read cursor counts to size the bounds dispatches faithfully.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("scaled_cursor_staging"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("scaled_cursor_readback"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_workgroups = cursor_pair[0] / 64;
    let block_workgroups = cursor_pair[1] / 64;

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("scaled_bounds"),
    });
    dispatch_compute_voxel_bounds(&mut encoder, &pipelines[1], &bind_group, voxel_workgroups);
    dispatch_compute_block_bounds(&mut encoder, &pipelines[2], &bind_group, block_workgroups);
    queue.submit([encoder.finish()]);

    // ── Read back the three buffers ──────────────────────────────────────────
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("scaled_readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("scaled_readback_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };

    let gpu_blocks_out = read_u32(&gpu_blocks, max_blocks as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, max_voxels_u32 as u64);

    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("scaled_chunks_readback"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("scaled_chunks_readback_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // ── Diagnostic output ────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "cursors: voxel_pairs={} block_u32s={}\n",
        cursor_pair[0], cursor_pair[1]
    ));
    s.push_str(&format!(
        "CPU oracle: chunks={} blocks={} voxels={}\n",
        oracle.chunks.len(), oracle.blocks.len(), oracle.voxels.len()
    ));

    // === Part 1: semantic content comparison (pointer-following) =============
    // For each chunk_idx, decode both CPU and GPU chunk; if both mixed, walk
    // their respective block groups; for each block_idx in the chunk, if both
    // mixed, walk their respective voxel groups. Compare semantic content
    // (block type/aadf/voxel u32s) — not raw pointer values, since pointers
    // are nondeterministic across CPU vs GPU.
    let mut sem_chunk_mismatch: Option<(usize, ChunkCell, ChunkCell, u32, u32)> = None;
    let mut sem_block_mismatch: Option<(usize, usize, BlockCell, BlockCell, u32, u32)> = None;
    let mut sem_voxel_mismatch: Option<(usize, usize, usize, u32, u32)> = None;
    let mut mixed_chunks_cpu = 0usize;
    let mut mixed_chunks_gpu = 0usize;
    let mut mixed_blocks_cpu = 0usize;
    let mut mixed_blocks_gpu = 0usize;
    'outer: for ci in 0..chunk_count {
        let cpu_chunk = ChunkCell::decode(oracle.chunks[ci]);
        let gpu_chunk = ChunkCell::decode(gpu_chunks_out[ci]);
        // Classification mismatch (Empty/Uniform/Mixed) is a real divergence.
        let cpu_kind = chunk_kind(&cpu_chunk);
        let gpu_kind = chunk_kind(&gpu_chunk);
        if cpu_kind != gpu_kind {
            sem_chunk_mismatch = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
            break 'outer;
        }
        // For non-mixed: compare encoded u32 directly.
        match (cpu_chunk, gpu_chunk) {
            (ChunkCell::Mixed(cpu_ptr), ChunkCell::Mixed(gpu_ptr)) => {
                mixed_chunks_cpu += 1;
                mixed_chunks_gpu += 1;
                // Walk 64 blocks at each side's pointer.
                for bi in 0..64usize {
                    let cpu_b_raw = oracle.blocks.get(cpu_ptr.0 as usize + bi).copied().unwrap_or(0);
                    let gpu_b_raw = gpu_blocks_out.get(gpu_ptr.0 as usize + bi).copied().unwrap_or(0);
                    let cpu_b = BlockCell::decode(cpu_b_raw);
                    let gpu_b = BlockCell::decode(gpu_b_raw);
                    let cpu_bk = block_kind(&cpu_b);
                    let gpu_bk = block_kind(&gpu_b);
                    if cpu_bk != gpu_bk {
                        sem_block_mismatch = Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                        break 'outer;
                    }
                    match (cpu_b, gpu_b) {
                        (BlockCell::Mixed(cpu_v), BlockCell::Mixed(gpu_v)) => {
                            mixed_blocks_cpu += 1;
                            mixed_blocks_gpu += 1;
                            for vi in 0..32usize {
                                let cpu_vw = oracle.voxels.get(cpu_v.0 as usize + vi).copied().unwrap_or(0);
                                let gpu_vw = gpu_voxels_out.get(gpu_v.0 as usize + vi).copied().unwrap_or(0);
                                if cpu_vw != gpu_vw {
                                    sem_voxel_mismatch = Some((ci, bi, vi, cpu_vw, gpu_vw));
                                    break 'outer;
                                }
                            }
                        }
                        (BlockCell::UniformFull(t1), BlockCell::UniformFull(t2)) => {
                            if t1 != t2 {
                                sem_block_mismatch = Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                                break 'outer;
                            }
                        }
                        (BlockCell::Empty(a1), BlockCell::Empty(a2)) => {
                            if a1.d != a2.d {
                                sem_block_mismatch = Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                                break 'outer;
                            }
                        }
                        _ => {
                            sem_block_mismatch = Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                            break 'outer;
                        }
                    }
                }
            }
            (ChunkCell::UniformFull(t1), ChunkCell::UniformFull(t2)) => {
                if t1 != t2 {
                    sem_chunk_mismatch = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                    break 'outer;
                }
            }
            (ChunkCell::Empty(a1), ChunkCell::Empty(a2)) => {
                if a1.d != a2.d {
                    sem_chunk_mismatch = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                    break 'outer;
                }
            }
            _ => {}
        }
    }

    s.push_str(&format!(
        "mixed chunks: cpu={mixed_chunks_cpu} gpu={mixed_chunks_gpu} ; mixed blocks: cpu={mixed_blocks_cpu} gpu={mixed_blocks_gpu}\n"
    ));

    s.push_str("[semantic pointer-following diff]\n");
    if let Some((ci, cpu_c, gpu_c, cpu_raw, gpu_raw)) = sem_chunk_mismatch {
        s.push_str(&format!(
            "  CHUNK MISMATCH @ ci={ci}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x}\n",
            cpu_c, cpu_raw, gpu_c, gpu_raw, cpu_raw ^ gpu_raw
        ));
    } else {
        s.push_str("  chunks: all 'kind' classifications match\n");
    }
    if let Some((ci, bi, cpu_b, gpu_b, cpu_raw, gpu_raw)) = sem_block_mismatch {
        s.push_str(&format!(
            "  BLOCK MISMATCH @ ci={ci} bi={bi}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x} (bits {:032b})\n",
            cpu_b, cpu_raw, gpu_b, gpu_raw, cpu_raw ^ gpu_raw, cpu_raw ^ gpu_raw
        ));
    } else {
        s.push_str("  blocks: all referenced blocks match semantically\n");
    }
    if let Some((ci, bi, vi, cpu_v, gpu_v)) = sem_voxel_mismatch {
        s.push_str(&format!(
            "  VOXEL MISMATCH @ ci={ci} bi={bi} vi={vi}: cpu={:#010x} gpu={:#010x} XOR={:#010x} (bits {:032b})\n",
            cpu_v, gpu_v, cpu_v ^ gpu_v, cpu_v ^ gpu_v
        ));
    } else {
        s.push_str("  voxels: all referenced voxels match\n");
    }

    // === Part 2: raw byte equality (sensitive to atomic ordering) ============
    s.push_str("[raw u32 byte-equality (sensitive to nondeterministic atomicAdd ordering)]\n");
    let first_chunk_diff = oracle
        .chunks
        .iter()
        .zip(gpu_chunks_out.iter())
        .position(|(a, b)| a != b);
    if let Some(i) = first_chunk_diff {
        let a = oracle.chunks[i];
        let b = gpu_chunks_out[i];
        s.push_str(&format!(
            "  chunks[{i}]: cpu={a:#010x} gpu={b:#010x} XOR={:#010x}\n",
            a ^ b
        ));
    } else {
        s.push_str("  chunks: byte-equal\n");
    }
    // For blocks/voxels: account for the GPU's +64 / +64-into-voxels seed.
    // CPU oracle voxels[0..] live at GPU voxels[gpu_voxel_seed..]; CPU oracle
    // blocks[0..] live at GPU blocks[64..]. Compare the relative-offset views.
    let first_block_diff = oracle
        .blocks
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            let b = gpu_blocks_out.get(64 + i).copied().unwrap_or(0);
            (*a != b).then_some((i, *a, b))
        });
    if let Some((i, a, b)) = first_block_diff {
        s.push_str(&format!(
            "  blocks[64+{i}={}]: cpu={a:#010x} gpu={b:#010x} XOR={:#010x} (bits {:032b})\n",
            64 + i,
            a ^ b,
            a ^ b
        ));
    } else {
        s.push_str("  blocks: byte-equal (after +64 seed offset)\n");
    }
    let first_voxel_diff = oracle
        .voxels
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            let b = gpu_voxels_out.get(32 + i).copied().unwrap_or(0);
            (*a != b).then_some((i, *a, b))
        });
    if let Some((i, a, b)) = first_voxel_diff {
        s.push_str(&format!(
            "  voxels[32+{i}={}]: cpu={a:#010x} gpu={b:#010x} XOR={:#010x} (bits {:032b})\n",
            32 + i,
            a ^ b,
            a ^ b
        ));
    } else {
        s.push_str("  voxels: byte-equal (after +32 seed offset)\n");
    }

    Ok(s)
}

/// Multi-segment variant: dispatch chunk_calc per-segment with shared
/// hash_map. Mirrors the production W5 loop in `naadf_gpu_producer_node`.
fn run_one_fixture_multiseg_byte_diff(
    world_size_in_chunks: [u32; 3],
    segment_size_in_chunks: u32,
    mode: FixtureMode,
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;
    use crate::voxel::VoxelTypeId;

    // Validate parameters: world dims must be exact multiples of segment size.
    let sx = world_size_in_chunks[0];
    let sy = world_size_in_chunks[1];
    let sz = world_size_in_chunks[2];
    if sx % segment_size_in_chunks != 0
        || sy % segment_size_in_chunks != 0
        || sz % segment_size_in_chunks != 0
    {
        return Err(format!(
            "world size {:?} not divisible by segment size {segment_size_in_chunks}",
            world_size_in_chunks
        ));
    }
    let seg_count_x = sx / segment_size_in_chunks;
    let seg_count_y = sy / segment_size_in_chunks;
    let seg_count_z = sz / segment_size_in_chunks;
    let n_segments = seg_count_x * seg_count_y * seg_count_z;

    // ── Build fixture volume ─────────────────────────────────────────────────
    let mut volume = DenseVolume::empty(world_size_in_chunks);
    let sv = volume.size_in_voxels();
    for cz in 0..sz {
        for cy in 0..sy {
            for cx in 0..sx {
                let chunk_idx = cx + cy * sx + cz * sx * sy;
                let pos_in_block: u32 = match mode {
                    FixtureMode::Uniform => 0,
                    FixtureMode::Diverse => chunk_idx % 64,
                    FixtureMode::Mixed => {
                        if chunk_idx % 2 == 0 { 0 } else { (chunk_idx / 2) % 64 }
                    }
                };
                let lx = pos_in_block % 4;
                let ly = (pos_in_block / 4) % 4;
                let lz = pos_in_block / 16;
                let vx = cx * 16 + lx;
                let vy = cy * 16 + ly;
                let vz = cz * 16 + lz;
                if vx < sv[0] && vy < sv[1] && vz < sv[2] {
                    volume.set([vx, vy, vz], VoxelTypeId(7));
                }
            }
        }
    }

    let oracle = construct(&volume);

    // ── Boot headless ────────────────────────────────────────────────────────
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

    let shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no device")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no queue")?
        .clone();

    // ── Allocate buffers ─────────────────────────────────────────────────────
    let chunk_count = (sx * sy * sz) as usize;
    // Mode=Mixed / Diverse fixtures dedup-collapse to ≤64 unique block
    // patterns. The voxel buffer can be bounded much tighter than the
    // naive worst-case (chunks * 64 * 32 = 2 GB for the largest fixture).
    // Cap at the smaller of (worst-case, 256 MB-of-u32s) so the GPU
    // allocation always succeeds; the hash dedup keeps real usage well
    // under this.
    let max_blocks = (64 + chunk_count * 64).max(64);
    let max_voxels_u32_uncapped: usize = 64usize.saturating_add(chunk_count.saturating_mul(64 * 32));
    let max_voxels_u32 = max_voxels_u32_uncapped.min(256 * 1024 * 1024 / 4); // 256 MB
    let hash_map_size_slots: u32 = 1 << 20; // fixed 16 MB / 1M slots
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    // Segment buffer size: exactly one segment's worth.
    let seg = segment_size_in_chunks as usize;
    let segment_buf_u32s = seg * seg * seg * 2048;

    let mk_storage = |label: &'static str, data: &[u32]| {
        let data = if data.is_empty() { &[0u32][..] } else { data };
        let size = (data.len() * 4) as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
        buffer
    };

    let gpu_blocks = mk_storage("ms_blocks", &vec![0u32; max_blocks]);
    let gpu_voxels = mk_storage("ms_voxels", &vec![0u32; max_voxels_u32]);
    let gpu_block_voxel_count = mk_storage("ms_bvc", &block_voxel_count_init);
    let gpu_segment = mk_storage("ms_segment", &vec![0u32; segment_buf_u32s]);
    let gpu_hash_map = mk_storage("ms_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("ms_coeffs", &coeffs);

    let params_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("ms_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("ms_chunks"),
        size: (chunk_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    // ── Pipelines ───────────────────────────────────────────────────────────
    let layout = construction_world_layout_descriptor();
    let (id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        (
            queue_calc_block_pipeline_with_handle(cache, layout.clone(), shader_handle.clone()),
            queue_voxel_bounds_pipeline_with_handle(cache, layout.clone(), shader_handle.clone()),
            queue_block_bounds_pipeline_with_handle(cache, layout.clone(), shader_handle.clone()),
        )
    };
    let mut pipelines: Option<Vec<bevy::render::render_resource::ComputePipeline>> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some(vec![a.clone(), b.clone(), c.clone()]);
            break;
        }
    }
    let pipelines = pipelines.ok_or("pipelines did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout);
    let bind_group = device.create_bind_group(
        "ms_bind_group",
        &bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            gpu_segment.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            params_buffer.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Per-segment loop: write segment buffer + params + dispatch ──────────
    for sz_i in 0..seg_count_z {
        for sy_i in 0..seg_count_y {
            for sx_i in 0..seg_count_x {
                let chunk_offset = [
                    sx_i * segment_size_in_chunks,
                    sy_i * segment_size_in_chunks,
                    sz_i * segment_size_in_chunks,
                ];
                // Build segment voxel buffer for THIS segment's region.
                let seg_voxels = build_segment_voxel_buffer_for_region(
                    &volume,
                    chunk_offset,
                    segment_size_in_chunks,
                );
                queue.write_buffer(&gpu_segment, 0, bytemuck::cast_slice(&seg_voxels));

                let params = GpuConstructionParams {
                    size_in_chunks: world_size_in_chunks,
                    _pad0: 0,
                    group_size_in_groups: [1, 1, 1],
                    _pad1: 0,
                    bound_group_queue_max_size: 1,
                    hash_map_size: hash_map_size_slots,
                    segment_size_in_chunks,
                    max_group_bound_dispatch: 0,
                    chunk_offset,
                    dispatch_offset: 0,
                    frame_index: 0,
                    changed_chunk_count: 0,
                    changed_block_count: 0,
                    changed_voxel_count: 0,
                };
                queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

                // Per-segment encoder + submit (mirrors production W5 loop).
                let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("ms_segment_enc"),
                });
                dispatch_calc_block_from_raw_data_world_sized(
                    &mut enc,
                    &pipelines[0],
                    &bind_group,
                    [segment_size_in_chunks; 3],
                );
                queue.submit([enc.finish()]);
            }
        }
    }

    // Cursor readback.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("ms_cursor_st"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("ms_cursor_enc"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_workgroups = cursor_pair[0] / 64;
    let block_workgroups = cursor_pair[1] / 64;

    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("ms_bounds"),
    });
    dispatch_compute_voxel_bounds(&mut enc, &pipelines[1], &bind_group, voxel_workgroups);
    dispatch_compute_block_bounds(&mut enc, &pipelines[2], &bind_group, block_workgroups);
    queue.submit([enc.finish()]);

    // Readback all three buffers.
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("ms_rb"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("ms_rb_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let gpu_blocks_out = read_u32(&gpu_blocks, max_blocks as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, max_voxels_u32 as u64);

    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("ms_chunks_rb"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("ms_chunks_rb_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // ── Diagnostic ──────────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "n_segments={n_segments} cursors: voxel_pairs={} block_u32s={}\n",
        cursor_pair[0], cursor_pair[1]
    ));
    s.push_str(&format!(
        "CPU oracle: chunks={} blocks={} voxels={}\n",
        oracle.chunks.len(), oracle.blocks.len(), oracle.voxels.len()
    ));

    let mut sem_chunk_mismatch: Option<(usize, ChunkCell, ChunkCell, u32, u32)> = None;
    let mut sem_block_mismatch: Option<(usize, usize, BlockCell, BlockCell, u32, u32)> = None;
    let mut sem_voxel_mismatch: Option<(usize, usize, usize, u32, u32)> = None;
    let mut total_mismatches: u32 = 0;
    let mut first_mismatch_chunk: Option<usize> = None;
    'outer: for ci in 0..chunk_count {
        let cpu_chunk = ChunkCell::decode(oracle.chunks[ci]);
        let gpu_chunk = ChunkCell::decode(gpu_chunks_out[ci]);
        if chunk_kind(&cpu_chunk) != chunk_kind(&gpu_chunk) {
            if sem_chunk_mismatch.is_none() {
                sem_chunk_mismatch = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                first_mismatch_chunk = Some(ci);
            }
            total_mismatches += 1;
            continue;
        }
        match (cpu_chunk, gpu_chunk) {
            (ChunkCell::Mixed(cpu_ptr), ChunkCell::Mixed(gpu_ptr)) => {
                for bi in 0..64usize {
                    let cpu_b_raw = oracle.blocks.get(cpu_ptr.0 as usize + bi).copied().unwrap_or(0);
                    let gpu_b_raw = gpu_blocks_out.get(gpu_ptr.0 as usize + bi).copied().unwrap_or(0);
                    let cpu_b = BlockCell::decode(cpu_b_raw);
                    let gpu_b = BlockCell::decode(gpu_b_raw);
                    if block_kind(&cpu_b) != block_kind(&gpu_b) {
                        if sem_block_mismatch.is_none() {
                            sem_block_mismatch =
                                Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                            first_mismatch_chunk = first_mismatch_chunk.or(Some(ci));
                        }
                        total_mismatches += 1;
                        continue;
                    }
                    match (cpu_b, gpu_b) {
                        (BlockCell::Mixed(cpu_v), BlockCell::Mixed(gpu_v)) => {
                            for vi in 0..32usize {
                                let cpu_vw = oracle.voxels.get(cpu_v.0 as usize + vi).copied().unwrap_or(0);
                                let gpu_vw = gpu_voxels_out.get(gpu_v.0 as usize + vi).copied().unwrap_or(0);
                                if cpu_vw != gpu_vw {
                                    if sem_voxel_mismatch.is_none() {
                                        sem_voxel_mismatch = Some((ci, bi, vi, cpu_vw, gpu_vw));
                                        first_mismatch_chunk = first_mismatch_chunk.or(Some(ci));
                                    }
                                    total_mismatches += 1;
                                    break 'outer;
                                }
                            }
                        }
                        (BlockCell::UniformFull(t1), BlockCell::UniformFull(t2)) if t1 != t2 => {
                            if sem_block_mismatch.is_none() {
                                sem_block_mismatch =
                                    Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                            }
                            total_mismatches += 1;
                        }
                        (BlockCell::Empty(a1), BlockCell::Empty(a2)) if a1.d != a2.d => {
                            if sem_block_mismatch.is_none() {
                                sem_block_mismatch =
                                    Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                            }
                            total_mismatches += 1;
                        }
                        _ => {}
                    }
                }
            }
            (ChunkCell::UniformFull(t1), ChunkCell::UniformFull(t2)) if t1 != t2 => {
                if sem_chunk_mismatch.is_none() {
                    sem_chunk_mismatch =
                        Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                }
                total_mismatches += 1;
            }
            (ChunkCell::Empty(a1), ChunkCell::Empty(a2)) if a1.d != a2.d => {
                if sem_chunk_mismatch.is_none() {
                    sem_chunk_mismatch =
                        Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                }
                total_mismatches += 1;
            }
            _ => {}
        }
    }

    s.push_str("[semantic pointer-following diff]\n");
    if let Some((ci, cpu_c, gpu_c, cpu_raw, gpu_raw)) = sem_chunk_mismatch {
        s.push_str(&format!(
            "  CHUNK MISMATCH @ ci={ci}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x}\n",
            cpu_c, cpu_raw, gpu_c, gpu_raw, cpu_raw ^ gpu_raw
        ));
    } else {
        s.push_str("  chunks: all 'kind' match\n");
    }
    if let Some((ci, bi, cpu_b, gpu_b, cpu_raw, gpu_raw)) = sem_block_mismatch {
        s.push_str(&format!(
            "  BLOCK MISMATCH @ ci={ci} bi={bi}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x} (bits {:032b})\n",
            cpu_b, cpu_raw, gpu_b, gpu_raw, cpu_raw ^ gpu_raw, cpu_raw ^ gpu_raw
        ));
    } else {
        s.push_str("  blocks: all kinds match semantically\n");
    }
    if let Some((ci, bi, vi, cpu_v, gpu_v)) = sem_voxel_mismatch {
        s.push_str(&format!(
            "  VOXEL MISMATCH @ ci={ci} bi={bi} vi={vi}: cpu={:#010x} gpu={:#010x} XOR={:#010x} (bits {:032b})\n",
            cpu_v, gpu_v, cpu_v ^ gpu_v, cpu_v ^ gpu_v
        ));
    } else {
        s.push_str("  voxels: all referenced voxels match\n");
    }
    s.push_str(&format!(
        "  total semantic mismatches: {total_mismatches} (first @ chunk {:?})\n",
        first_mismatch_chunk
    ));
    Ok(s)
}

/// Drive `generator_model.wgsl` for one (model, segment) configuration and
/// byte-compare its `segment_voxel_buffer` output against the
/// `generate_segment_cpu` Rust oracle.
fn run_one_generator_model_byte_diff(
    model_size_in_chunks: [u32; 3],
    group_size_in_chunks: [u32; 3],
    group_offset_in_chunks: [u32; 3],
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::generator::{
        generate_segment_cpu, CHUNK_DATA_U32S,
    };
    use crate::render::construction::generator_model::{
        create_storage_buffer_u32, create_params_uniform,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        dispatch_generator_model_with_encoder, GpuGeneratorModelParams,
        GENERATOR_MODEL_SHADER_SRC,
    };

    // Build a "mixed" ModelData with varied per-chunk content. Use the model's
    // chunks/blocks/voxels layout from the encoding contract.
    let model = build_mixed_model_data(model_size_in_chunks);

    let world_size_in_voxels = [
        group_size_in_chunks[0] * 16 + group_offset_in_chunks[0] * 16,
        group_size_in_chunks[1] * 16 + group_offset_in_chunks[1] * 16,
        group_size_in_chunks[2] * 16 + group_offset_in_chunks[2] * 16,
    ];

    // CPU oracle.
    let cpu_out = generate_segment_cpu(
        &model,
        group_offset_in_chunks,
        group_size_in_chunks,
        world_size_in_voxels,
    );

    // ── Boot headless ────────────────────────────────────────────────────────
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

    let shader = Shader::from_wgsl(GENERATOR_MODEL_SHADER_SRC, "shaders/generator_model.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no device")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no queue")?
        .clone();

    // ── Allocate buffers ─────────────────────────────────────────────────────
    let total_chunks =
        group_size_in_chunks[0] * group_size_in_chunks[1] * group_size_in_chunks[2];
    let chunk_data_u32s = (total_chunks * CHUNK_DATA_U32S) as usize;
    let chunk_data_init = vec![0u32; chunk_data_u32s];

    let chunk_data_buf =
        create_storage_buffer_u32(&device, &queue, "gen_chunk_data_rw", &chunk_data_init);
    let model_chunk_buf =
        create_storage_buffer_u32(&device, &queue, "gen_model_chunk_ro", &model.data_chunk);
    let model_block_buf =
        create_storage_buffer_u32(&device, &queue, "gen_model_block_ro", &model.data_block);
    let model_voxel_buf =
        create_storage_buffer_u32(&device, &queue, "gen_model_voxel_ro", &model.data_voxel);

    let params = GpuGeneratorModelParams {
        size_in_voxels: world_size_in_voxels,
        _pad0: 0,
        model_size_in_chunks,
        _pad1: 0,
        group_offset_in_chunks,
        group_size_in_chunks_x: group_size_in_chunks[0],
        group_size_in_chunks_y: group_size_in_chunks[1],
        _pad2: 0,
        _pad3: 0,
        _pad4: 0,
    };
    let params_buf = create_params_uniform(&device, &queue, &params);

    // ── Pipeline ────────────────────────────────────────────────────────────
    let layout = generator_model_layout_descriptor();
    let id = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        queue_generator_model_pipeline_with_handle(cache, layout.clone(), shader_handle.clone())
    };
    let mut pipeline: Option<bevy::render::render_resource::ComputePipeline> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let Some(p) = cache.get_compute_pipeline(id) {
            pipeline = Some(p.clone());
            break;
        }
    }
    let pipeline = pipeline.ok_or("generator pipeline did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout);
    let bind_group = device.create_bind_group(
        "gen_bg",
        &bgl,
        &BindGroupEntries::sequential((
            chunk_data_buf.as_entire_buffer_binding(),
            model_chunk_buf.as_entire_buffer_binding(),
            model_block_buf.as_entire_buffer_binding(),
            model_voxel_buf.as_entire_buffer_binding(),
            params_buf.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch ────────────────────────────────────────────────────────────
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("gen_dispatch"),
    });
    dispatch_generator_model_with_encoder(&mut enc, &pipeline, &bind_group, group_size_in_chunks);
    queue.submit([enc.finish()]);

    // ── Readback ────────────────────────────────────────────────────────────
    let staging_size = (chunk_data_u32s as u64) * 4;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("gen_rb"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("gen_rb_enc"),
    });
    enc.copy_buffer_to_buffer(&chunk_data_buf, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let gpu_out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging.unmap();

    // ── Compare ─────────────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "CPU oracle: {} u32s ; GPU: {} u32s\n",
        cpu_out.len(),
        gpu_out.len()
    ));

    let first_diff = cpu_out
        .iter()
        .zip(gpu_out.iter())
        .enumerate()
        .find(|(_, (a, b))| a != b);
    if let Some((i, (a, b))) = first_diff {
        let xor = a ^ b;
        s.push_str(&format!(
            "  FIRST DIVERGENT INDEX: {i}  cpu={a:#010x}  gpu={b:#010x}  XOR={xor:#010x}  bits={xor:032b}\n",
        ));
        // Also count total divergent u32s.
        let n_diff: usize = cpu_out
            .iter()
            .zip(gpu_out.iter())
            .filter(|(a, b)| a != b)
            .count();
        let pct = n_diff as f64 * 100.0 / cpu_out.len() as f64;
        s.push_str(&format!(
            "  total divergent u32s: {n_diff} / {} ({pct:.2}%)\n",
            cpu_out.len()
        ));
        // Decode the divergent position back to (group, local, i) to localize.
        let chunk_data_per = CHUNK_DATA_U32S as usize;
        let group_index = i / chunk_data_per;
        let within = i % chunk_data_per;
        let local_index = within / 32;
        let pair_i = within % 32;
        let gx = (group_index as u32) % group_size_in_chunks[0];
        let gy = ((group_index as u32) / group_size_in_chunks[0])
            % group_size_in_chunks[1];
        let gz = (group_index as u32) / (group_size_in_chunks[0] * group_size_in_chunks[1]);
        s.push_str(&format!(
            "  decoded: group={gx},{gy},{gz} local_index={local_index} pair_i={pair_i}\n"
        ));
    } else {
        s.push_str("  GENERATOR BYTE-EQUAL: cpu == gpu across all u32s\n");
    }

    Ok(s)
}

/// Tiled-model variant: small `ModelData` gets tiled into a larger world via
/// the per-segment generator chain (matches production Oasis behavior). The
/// CPU oracle builds the tiled volume by decoding `generate_segment_cpu`
/// output for every segment, then runs `construct()` on the assembled
/// world-volume.
fn run_one_tiled_byte_diff(
    model_size_in_chunks: [u32; 3],
    world_size_in_chunks: [u32; 3],
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::aadf::generator::generate_segment_cpu;
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::generator_model::{
        create_storage_buffer_u32, create_params_uniform,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        dispatch_generator_model_with_encoder, GpuGeneratorModelParams,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;

    // Build the tile model — non-trivial mixed content so tiles actually
    // contribute geometry.
    let model = build_mixed_model_data(model_size_in_chunks);

    let world_size_in_voxels = [
        world_size_in_chunks[0] * 16,
        world_size_in_chunks[1] * 16,
        world_size_in_chunks[2] * 16,
    ];

    // Segment shape: matches production's `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS *
    // 4 = 16`. If the world is smaller than 16 per axis, use the full world
    // as one segment.
    let seg = world_size_in_chunks[0].min(world_size_in_chunks[1]).min(world_size_in_chunks[2]).min(16);
    if world_size_in_chunks[0] % seg != 0
        || world_size_in_chunks[1] % seg != 0
        || world_size_in_chunks[2] % seg != 0
    {
        return Err(format!(
            "world {:?} not divisible by seg {seg}",
            world_size_in_chunks
        ));
    }
    let seg_count = [
        world_size_in_chunks[0] / seg,
        world_size_in_chunks[1] / seg,
        world_size_in_chunks[2] / seg,
    ];
    let n_segments = seg_count[0] * seg_count[1] * seg_count[2];

    // ── CPU oracle: build full world by tiling, then construct() ────────────
    let mut volume = DenseVolume::empty(world_size_in_chunks);
    for sz_i in 0..seg_count[2] {
        for sy_i in 0..seg_count[1] {
            for sx_i in 0..seg_count[0] {
                let off = [sx_i * seg, sy_i * seg, sz_i * seg];
                let cpu_seg = generate_segment_cpu(
                    &model,
                    off,
                    [seg, seg, seg],
                    world_size_in_voxels,
                );
                // Stamp the segment's voxels into the global volume.
                decode_segment_voxels_into_volume(&cpu_seg, off, [seg, seg, seg], &mut volume);
            }
        }
    }
    let oracle = construct(&volume);

    // ── Boot headless ───────────────────────────────────────────────────────
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

    let gen_shader = Shader::from_wgsl(GENERATOR_MODEL_SHADER_SRC, "shaders/generator_model.wgsl");
    let gen_shader_clone = gen_shader.clone();
    let gen_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(gen_shader);
    let calc_shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let calc_shader_clone = calc_shader.clone();
    let calc_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(calc_shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(gen_shader_handle.id(), gen_shader_clone);
        pc.set_shader(calc_shader_handle.id(), calc_shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no device")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no queue")?
        .clone();

    // ── Buffers ─────────────────────────────────────────────────────────────
    let segment_buf_u32s = (seg as usize) * (seg as usize) * (seg as usize) * 2048;
    let chunk_count = (world_size_in_chunks[0] * world_size_in_chunks[1] * world_size_in_chunks[2]) as usize;
    let max_blocks = (64 + chunk_count * 64).max(64);
    let max_voxels_u32 = (64usize.saturating_add(chunk_count.saturating_mul(64 * 32))).min(256 * 1024 * 1024 / 4);
    let hash_map_size_slots: u32 = 1 << 20;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let segment_buf = create_storage_buffer_u32(&device, &queue, "tile_seg", &vec![0u32; segment_buf_u32s]);
    let model_chunk_buf =
        create_storage_buffer_u32(&device, &queue, "tile_mc", &model.data_chunk);
    let model_block_buf =
        create_storage_buffer_u32(&device, &queue, "tile_mb", &model.data_block);
    let model_voxel_buf =
        create_storage_buffer_u32(&device, &queue, "tile_mv", &model.data_voxel);
    let gen_params_buf = create_params_uniform(
        &device,
        &queue,
        &GpuGeneratorModelParams {
            size_in_voxels: world_size_in_voxels,
            _pad0: 0,
            model_size_in_chunks: model.size_in_chunks,
            _pad1: 0,
            group_offset_in_chunks: [0, 0, 0],
            group_size_in_chunks_x: seg,
            group_size_in_chunks_y: seg,
            _pad2: 0,
            _pad3: 0,
            _pad4: 0,
        },
    );

    let mk_storage = |label: &'static str, data: &[u32]| {
        let data = if data.is_empty() { &[0u32][..] } else { data };
        let size = (data.len() * 4) as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
        buffer
    };

    let gpu_blocks = mk_storage("tile_blocks", &vec![0u32; max_blocks]);
    let gpu_voxels = mk_storage("tile_voxels", &vec![0u32; max_voxels_u32]);
    let gpu_block_voxel_count = mk_storage("tile_bvc", &block_voxel_count_init);
    let gpu_hash_map = mk_storage("tile_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("tile_coeffs", &coeffs);

    let calc_params_buf = device.create_buffer(&BufferDescriptor {
        label: Some("tile_calc_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("tile_chunks"),
        size: (chunk_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    let gen_layout = generator_model_layout_descriptor();
    let calc_layout = construction_world_layout_descriptor();
    let (id_gen, id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        (
            queue_generator_model_pipeline_with_handle(cache, gen_layout.clone(), gen_shader_handle.clone()),
            queue_calc_block_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
            queue_voxel_bounds_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
            queue_block_bounds_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
        )
    };
    let mut pipelines: Option<(_, _, _, _)> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(g), Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_gen),
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some((g.clone(), a.clone(), b.clone(), c.clone()));
            break;
        }
    }
    let (p_gen, p_calc, p_voxel, p_block) =
        pipelines.ok_or("tiled pipelines did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let gen_bgl = cache.get_bind_group_layout(&gen_layout);
    let gen_bg = device.create_bind_group(
        "tile_gen_bg",
        &gen_bgl,
        &BindGroupEntries::sequential((
            segment_buf.as_entire_buffer_binding(),
            model_chunk_buf.as_entire_buffer_binding(),
            model_block_buf.as_entire_buffer_binding(),
            model_voxel_buf.as_entire_buffer_binding(),
            gen_params_buf.as_entire_buffer_binding(),
        )),
    );
    let calc_bgl = cache.get_bind_group_layout(&calc_layout);
    let calc_bg = device.create_bind_group(
        "tile_calc_bg",
        &calc_bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            segment_buf.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            calc_params_buf.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Per-segment dispatch loop (matches production W5) ───────────────────
    for sz_i in 0..seg_count[2] {
        for sy_i in 0..seg_count[1] {
            for sx_i in 0..seg_count[0] {
                let off = [sx_i * seg, sy_i * seg, sz_i * seg];
                // Generator uniform — model + segment offset.
                let gen_p = GpuGeneratorModelParams {
                    size_in_voxels: world_size_in_voxels,
                    _pad0: 0,
                    model_size_in_chunks: model.size_in_chunks,
                    _pad1: 0,
                    group_offset_in_chunks: off,
                    group_size_in_chunks_x: seg,
                    group_size_in_chunks_y: seg,
                    _pad2: 0,
                    _pad3: 0,
                    _pad4: 0,
                };
                queue.write_buffer(&gen_params_buf, 0, bytemuck::bytes_of(&gen_p));
                // Chunk_calc uniform — world size + segment offset.
                let calc_p = GpuConstructionParams {
                    size_in_chunks: world_size_in_chunks,
                    _pad0: 0,
                    group_size_in_groups: [1, 1, 1],
                    _pad1: 0,
                    bound_group_queue_max_size: 1,
                    hash_map_size: hash_map_size_slots,
                    segment_size_in_chunks: seg,
                    max_group_bound_dispatch: 0,
                    chunk_offset: off,
                    dispatch_offset: 0,
                    frame_index: 0,
                    changed_chunk_count: 0,
                    changed_block_count: 0,
                    changed_voxel_count: 0,
                };
                queue.write_buffer(&calc_params_buf, 0, bytemuck::bytes_of(&calc_p));

                let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("tile_seg_enc"),
                });
                dispatch_generator_model_with_encoder(&mut enc, &p_gen, &gen_bg, [seg, seg, seg]);
                dispatch_calc_block_from_raw_data_world_sized(&mut enc, &p_calc, &calc_bg, [seg, seg, seg]);
                queue.submit([enc.finish()]);
            }
        }
    }

    // Readback cursor + run bounds dispatches.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("tile_cur"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("tile_cur_enc"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_wg = cursor_pair[0] / 64;
    let block_wg = cursor_pair[1] / 64;
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("tile_bnd"),
    });
    dispatch_compute_voxel_bounds(&mut enc, &p_voxel, &calc_bg, voxel_wg);
    dispatch_compute_block_bounds(&mut enc, &p_block, &calc_bg, block_wg);
    queue.submit([enc.finish()]);

    // Readback buffers.
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("tile_rb"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("tile_rb_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let gpu_blocks_out = read_u32(&gpu_blocks, max_blocks as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, max_voxels_u32 as u64);

    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("tile_chunks_rb"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("tile_chunks_rb_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // ── Semantic diff ───────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "n_segments={n_segments} seg={seg} cursors: voxel_pairs={} block_u32s={}\n",
        cursor_pair[0], cursor_pair[1]
    ));
    s.push_str(&format!(
        "CPU oracle: chunks={} blocks={} voxels={}\n",
        oracle.chunks.len(), oracle.blocks.len(), oracle.voxels.len()
    ));

    let mut sem_chunk_mm: Option<(usize, ChunkCell, ChunkCell, u32, u32)> = None;
    let mut sem_block_mm: Option<(usize, usize, BlockCell, BlockCell, u32, u32)> = None;
    let mut sem_voxel_mm: Option<(usize, usize, usize, u32, u32)> = None;
    let mut total_mm: u32 = 0;
    let mut first_mismatch_chunk: Option<usize> = None;
    'outer: for ci in 0..chunk_count {
        let cpu_chunk = ChunkCell::decode(oracle.chunks[ci]);
        let gpu_chunk = ChunkCell::decode(gpu_chunks_out[ci]);
        if chunk_kind(&cpu_chunk) != chunk_kind(&gpu_chunk) {
            if sem_chunk_mm.is_none() {
                sem_chunk_mm = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                first_mismatch_chunk = Some(ci);
            }
            total_mm += 1;
            continue;
        }
        match (cpu_chunk, gpu_chunk) {
            (ChunkCell::Mixed(cp), ChunkCell::Mixed(gp)) => {
                for bi in 0..64usize {
                    let cb_raw = oracle.blocks.get(cp.0 as usize + bi).copied().unwrap_or(0);
                    let gb_raw = gpu_blocks_out.get(gp.0 as usize + bi).copied().unwrap_or(0);
                    let cb = BlockCell::decode(cb_raw);
                    let gb = BlockCell::decode(gb_raw);
                    if block_kind(&cb) != block_kind(&gb) {
                        if sem_block_mm.is_none() {
                            sem_block_mm = Some((ci, bi, cb, gb, cb_raw, gb_raw));
                            first_mismatch_chunk = first_mismatch_chunk.or(Some(ci));
                        }
                        total_mm += 1;
                        continue;
                    }
                    match (cb, gb) {
                        (BlockCell::Mixed(cv), BlockCell::Mixed(gv)) => {
                            for vi in 0..32usize {
                                let cv_w = oracle.voxels.get(cv.0 as usize + vi).copied().unwrap_or(0);
                                let gv_w = gpu_voxels_out.get(gv.0 as usize + vi).copied().unwrap_or(0);
                                if cv_w != gv_w {
                                    if sem_voxel_mm.is_none() {
                                        sem_voxel_mm = Some((ci, bi, vi, cv_w, gv_w));
                                        first_mismatch_chunk = first_mismatch_chunk.or(Some(ci));
                                    }
                                    total_mm += 1;
                                    break 'outer;
                                }
                            }
                        }
                        (BlockCell::UniformFull(a), BlockCell::UniformFull(b)) if a != b => {
                            if sem_block_mm.is_none() {
                                sem_block_mm = Some((ci, bi, cb, gb, cb_raw, gb_raw));
                            }
                            total_mm += 1;
                        }
                        (BlockCell::Empty(a1), BlockCell::Empty(a2)) if a1.d != a2.d => {
                            if sem_block_mm.is_none() {
                                sem_block_mm = Some((ci, bi, cb, gb, cb_raw, gb_raw));
                            }
                            total_mm += 1;
                        }
                        _ => {}
                    }
                }
            }
            (ChunkCell::UniformFull(a), ChunkCell::UniformFull(b)) if a != b => {
                if sem_chunk_mm.is_none() {
                    sem_chunk_mm = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                }
                total_mm += 1;
            }
            (ChunkCell::Empty(_), ChunkCell::Empty(_)) => {
                // Empty-chunk AADFs are set by bounds_calc (not run here).
            }
            _ => {}
        }
    }

    s.push_str(&format!(
        "  total semantic mismatches: {total_mm} (first @ chunk {:?}; Empty-chunk AADFs ignored)\n",
        first_mismatch_chunk
    ));
    if let Some((ci, cc, gc, cr, gr)) = sem_chunk_mm {
        s.push_str(&format!(
            "    CHUNK MM @ ci={ci}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x}\n",
            cc, cr, gc, gr, cr ^ gr,
        ));
    }
    if let Some((ci, bi, cb, gb, cr, gr)) = sem_block_mm {
        s.push_str(&format!(
            "    BLOCK MM @ ci={ci} bi={bi}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x} (bits {:032b})\n",
            cb, cr, gb, gr, cr ^ gr, cr ^ gr,
        ));
    }
    if let Some((ci, bi, vi, cw, gw)) = sem_voxel_mm {
        s.push_str(&format!(
            "    VOXEL MM @ ci={ci} bi={bi} vi={vi}: cpu={:#010x} gpu={:#010x} XOR={:#010x} (bits {:032b})\n",
            cw, gw, cw ^ gw, cw ^ gw,
        ));
    }

    Ok(s)
}

/// Decode a segment_voxel_buffer-formatted vector and stamp its decoded
/// voxels into the given DenseVolume at the given chunk offset.
fn decode_segment_voxels_into_volume(
    seg_buf: &[u32],
    chunk_offset: [u32; 3],
    group_size_in_chunks: [u32; 3],
    volume: &mut crate::aadf::construct::DenseVolume,
) {
    use crate::voxel::VoxelTypeId;
    let gscx = group_size_in_chunks[0] as usize;
    let gscy = group_size_in_chunks[1] as usize;
    let gscz = group_size_in_chunks[2] as usize;
    let sv = volume.size_in_voxels();
    for cz in 0..gscz {
        for cy in 0..gscy {
            for cx in 0..gscx {
                let chunk_index_in_segment = cx + cy * gscx + cz * gscx * gscy;
                let chunk_base = chunk_index_in_segment * 2048;
                let world_cx = chunk_offset[0] as usize + cx;
                let world_cy = chunk_offset[1] as usize + cy;
                let world_cz = chunk_offset[2] as usize + cz;
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let pair = seg_buf[block_base + vi];
                                let lo = (pair & 0xFFFF) as u16;
                                let hi = ((pair >> 16) & 0xFFFF) as u16;
                                for (slot, half) in [(0, lo), (1, hi)].iter() {
                                    let voxel_idx = vi * 2 + slot;
                                    let lx = voxel_idx % 4;
                                    let ly = (voxel_idx / 4) % 4;
                                    let lz = voxel_idx / 16;
                                    let vx = world_cx * 16 + bx * 4 + lx;
                                    let vy = world_cy * 16 + by * 4 + ly;
                                    let vz = world_cz * 16 + bz * 4 + lz;
                                    if vx >= sv[0] as usize || vy >= sv[1] as usize || vz >= sv[2] as usize {
                                        continue;
                                    }
                                    let ty_bits = *half & 0x7FFF;
                                    let full = *half & 0x8000 != 0;
                                    let ty = if full {
                                        VoxelTypeId(ty_bits)
                                    } else {
                                        VoxelTypeId::EMPTY
                                    };
                                    volume.set([vx as u32, vy as u32, vz as u32], ty);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Load the Oasis VOX file from disk as a `ModelData` (same way
/// `install_vox_in_fixed_world` does).
fn load_oasis_model_data(
    path: &std::path::Path,
) -> Result<crate::aadf::generator::ModelData, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let data = dot_vox::load_bytes(&bytes).map_err(|e| format!("parse: {e}"))?;
    let imp = crate::voxel::vox_import::parse_dot_vox_data(&data)
        .map_err(|e| format!("import: {e:?}"))?;
    Ok(crate::aadf::generator::ModelData {
        size_in_chunks: imp.world.size_in_chunks,
        data_chunk: imp.world.chunks,
        data_block: imp.world.blocks,
        data_voxel: imp.world.voxels,
    })
}

/// Drive `generator_model.wgsl` + `chunk_calc.wgsl` for one segment of a
/// real `ModelData`, then byte-diff the GPU output against a CPU oracle
/// built from the same segment's voxels.
fn run_oasis_segment_byte_diff(
    model: &crate::aadf::generator::ModelData,
    group_offset_in_chunks: [u32; 3],
    group_size_in_chunks: [u32; 3],
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell};
    use crate::aadf::construct::construct;
    use crate::aadf::generator::{generate_segment_cpu, CHUNK_DATA_U32S};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::generator_model::{
        create_storage_buffer_u32, create_params_uniform,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        dispatch_generator_model_with_encoder, GpuGeneratorModelParams,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;

    // The "world" size for purposes of voxel-position bounds-check =
    // (offset + segment) * 16 voxels per axis. This matches what production
    // passes to generator_model when running on the Oasis fixed world.
    let world_size_in_voxels = [
        (group_offset_in_chunks[0] + group_size_in_chunks[0]) * 16,
        (group_offset_in_chunks[1] + group_size_in_chunks[1]) * 16,
        (group_offset_in_chunks[2] + group_size_in_chunks[2]) * 16,
    ];

    // ── CPU oracle: generator output ────────────────────────────────────────
    let cpu_seg_voxels = generate_segment_cpu(
        model,
        group_offset_in_chunks,
        group_size_in_chunks,
        world_size_in_voxels,
    );

    // Build a DenseVolume of just this segment's voxels by decoding
    // `cpu_seg_voxels` (which is in the segment_voxel_buffer encoding:
    // chunk_index_in_segment * 2048 + local_index * 32 + i). Then run
    // construct() as the chunk_calc CPU oracle.
    let seg_volume = decode_segment_voxels_to_volume(&cpu_seg_voxels, group_size_in_chunks);
    let oracle = construct(&seg_volume);

    // ── Boot headless ────────────────────────────────────────────────────────
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

    let gen_shader = Shader::from_wgsl(GENERATOR_MODEL_SHADER_SRC, "shaders/generator_model.wgsl");
    let gen_shader_clone = gen_shader.clone();
    let gen_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(gen_shader);
    let calc_shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let calc_shader_clone = calc_shader.clone();
    let calc_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(calc_shader);

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(gen_shader_handle.id(), gen_shader_clone);
        pc.set_shader(calc_shader_handle.id(), calc_shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no device")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no queue")?
        .clone();

    // ── Allocate buffers ────────────────────────────────────────────────────
    let total_chunks =
        group_size_in_chunks[0] * group_size_in_chunks[1] * group_size_in_chunks[2];
    let chunk_data_u32s = (total_chunks * CHUNK_DATA_U32S) as usize;
    let chunk_count = total_chunks as usize;
    let max_blocks = (64 + chunk_count * 64).max(64);
    let max_voxels_u32 = (64 + chunk_count * 64 * 32).min(256 * 1024 * 1024 / 4);
    let hash_map_size_slots: u32 = 1 << 20;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let segment_buf = create_storage_buffer_u32(
        &device,
        &queue,
        "oasis_seg_buf",
        &vec![0u32; chunk_data_u32s],
    );
    let model_chunk_buf =
        create_storage_buffer_u32(&device, &queue, "oasis_model_chunk", &model.data_chunk);
    let model_block_buf =
        create_storage_buffer_u32(&device, &queue, "oasis_model_block", &model.data_block);
    let model_voxel_buf =
        create_storage_buffer_u32(&device, &queue, "oasis_model_voxel", &model.data_voxel);

    let gen_params = GpuGeneratorModelParams {
        size_in_voxels: world_size_in_voxels,
        _pad0: 0,
        model_size_in_chunks: model.size_in_chunks,
        _pad1: 0,
        group_offset_in_chunks,
        group_size_in_chunks_x: group_size_in_chunks[0],
        group_size_in_chunks_y: group_size_in_chunks[1],
        _pad2: 0,
        _pad3: 0,
        _pad4: 0,
    };
    let gen_params_buf = create_params_uniform(&device, &queue, &gen_params);

    // chunk_calc resources — note that chunks/blocks/voxels target the
    // segment as if it were a self-contained world of `group_size_in_chunks`.
    let mk_storage = |label: &'static str, data: &[u32]| {
        let data = if data.is_empty() { &[0u32][..] } else { data };
        let size = (data.len() * 4) as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
        buffer
    };

    let gpu_blocks = mk_storage("oasis_blocks", &vec![0u32; max_blocks]);
    let gpu_voxels = mk_storage("oasis_voxels", &vec![0u32; max_voxels_u32]);
    let gpu_block_voxel_count = mk_storage("oasis_bvc", &block_voxel_count_init);
    let gpu_hash_map = mk_storage("oasis_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("oasis_coeffs", &coeffs);

    let calc_params = GpuConstructionParams {
        size_in_chunks: group_size_in_chunks,
        _pad0: 0,
        group_size_in_groups: [1, 1, 1],
        _pad1: 0,
        bound_group_queue_max_size: 1,
        hash_map_size: hash_map_size_slots,
        segment_size_in_chunks: group_size_in_chunks[0]
            .max(group_size_in_chunks[1])
            .max(group_size_in_chunks[2]),
        max_group_bound_dispatch: 0,
        chunk_offset: [0, 0, 0],
        dispatch_offset: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let calc_params_buf = device.create_buffer(&BufferDescriptor {
        label: Some("oasis_calc_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&calc_params_buf, 0, bytemuck::bytes_of(&calc_params));

    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("oasis_chunks"),
        size: (chunk_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    // ── Pipelines ───────────────────────────────────────────────────────────
    let gen_layout = generator_model_layout_descriptor();
    let calc_layout = construction_world_layout_descriptor();
    let (id_gen, id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        (
            queue_generator_model_pipeline_with_handle(cache, gen_layout.clone(), gen_shader_handle.clone()),
            queue_calc_block_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
            queue_voxel_bounds_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
            queue_block_bounds_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
        )
    };
    let mut pipelines: Option<(
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
    )> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(g), Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_gen),
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some((g.clone(), a.clone(), b.clone(), c.clone()));
            break;
        }
    }
    let (p_gen, p_calc, p_voxel, p_block) =
        pipelines.ok_or("oasis pipelines did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();

    let gen_bgl = cache.get_bind_group_layout(&gen_layout);
    let gen_bg = device.create_bind_group(
        "oasis_gen_bg",
        &gen_bgl,
        &BindGroupEntries::sequential((
            segment_buf.as_entire_buffer_binding(),
            model_chunk_buf.as_entire_buffer_binding(),
            model_block_buf.as_entire_buffer_binding(),
            model_voxel_buf.as_entire_buffer_binding(),
            gen_params_buf.as_entire_buffer_binding(),
        )),
    );
    let calc_bgl = cache.get_bind_group_layout(&calc_layout);
    let calc_bg = device.create_bind_group(
        "oasis_calc_bg",
        &calc_bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            segment_buf.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            calc_params_buf.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch generator → chunk_calc → bounds ─────────────────────────────
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("oasis_dispatch"),
    });
    dispatch_generator_model_with_encoder(&mut enc, &p_gen, &gen_bg, group_size_in_chunks);
    dispatch_calc_block_from_raw_data_world_sized(&mut enc, &p_calc, &calc_bg, group_size_in_chunks);
    queue.submit([enc.finish()]);

    // Readback cursor for bounds.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("oasis_cur"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("oasis_cur_enc"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_wg = cursor_pair[0] / 64;
    let block_wg = cursor_pair[1] / 64;
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("oasis_bnd"),
    });
    dispatch_compute_voxel_bounds(&mut enc, &p_voxel, &calc_bg, voxel_wg);
    dispatch_compute_block_bounds(&mut enc, &p_block, &calc_bg, block_wg);
    queue.submit([enc.finish()]);

    // ── Verify generator output ALSO matches CPU oracle ─────────────────────
    let gpu_seg_voxels = {
        let size = (chunk_data_u32s as u64) * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("oasis_seg_rb"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("oasis_seg_rb_enc"),
        });
        enc.copy_buffer_to_buffer(&segment_buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let gen_first_diff = cpu_seg_voxels
        .iter()
        .zip(gpu_seg_voxels.iter())
        .enumerate()
        .find(|(_, (a, b))| a != b);

    // ── Readback chunks/blocks/voxels ────────────────────────────────────────
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("oasis_rb"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("oasis_rb_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let gpu_blocks_out = read_u32(&gpu_blocks, max_blocks as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, max_voxels_u32 as u64);
    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("oasis_chunks_rb"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("oasis_chunks_rb_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // ── Diagnostic ──────────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "cursors: voxel_pairs={} block_u32s={} ; CPU oracle: chunks={} blocks={} voxels={}\n",
        cursor_pair[0],
        cursor_pair[1],
        oracle.chunks.len(),
        oracle.blocks.len(),
        oracle.voxels.len(),
    ));

    if let Some((i, (a, b))) = gen_first_diff {
        let xor = a ^ b;
        s.push_str(&format!(
            "  GENERATOR DIVERGED @ u32[{i}]: cpu={a:#010x} gpu={b:#010x} XOR={xor:#010x}\n",
        ));
    } else {
        s.push_str("  generator: byte-equal ({} u32s)\n");
    }

    // Semantic pointer-following diff (same as the other multi-fixture path).
    let mut sem_chunk_mm: Option<(usize, ChunkCell, ChunkCell, u32, u32)> = None;
    let mut sem_block_mm: Option<(usize, usize, BlockCell, BlockCell, u32, u32)> = None;
    let mut sem_voxel_mm: Option<(usize, usize, usize, u32, u32)> = None;
    let mut total_mm: u32 = 0;
    'outer: for ci in 0..chunk_count {
        let cpu_chunk = ChunkCell::decode(oracle.chunks[ci]);
        let gpu_chunk = ChunkCell::decode(gpu_chunks_out[ci]);
        if chunk_kind(&cpu_chunk) != chunk_kind(&gpu_chunk) {
            sem_chunk_mm.get_or_insert((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
            total_mm += 1;
            continue;
        }
        match (cpu_chunk, gpu_chunk) {
            (ChunkCell::Mixed(cp), ChunkCell::Mixed(gp)) => {
                for bi in 0..64usize {
                    let cb_raw = oracle.blocks.get(cp.0 as usize + bi).copied().unwrap_or(0);
                    let gb_raw = gpu_blocks_out.get(gp.0 as usize + bi).copied().unwrap_or(0);
                    let cb = BlockCell::decode(cb_raw);
                    let gb = BlockCell::decode(gb_raw);
                    if block_kind(&cb) != block_kind(&gb) {
                        sem_block_mm.get_or_insert((ci, bi, cb, gb, cb_raw, gb_raw));
                        total_mm += 1;
                        continue;
                    }
                    match (cb, gb) {
                        (BlockCell::Mixed(cv), BlockCell::Mixed(gv)) => {
                            for vi in 0..32usize {
                                let cv_w = oracle.voxels.get(cv.0 as usize + vi).copied().unwrap_or(0);
                                let gv_w = gpu_voxels_out.get(gv.0 as usize + vi).copied().unwrap_or(0);
                                if cv_w != gv_w {
                                    sem_voxel_mm.get_or_insert((ci, bi, vi, cv_w, gv_w));
                                    total_mm += 1;
                                    break 'outer;
                                }
                            }
                        }
                        (BlockCell::UniformFull(a), BlockCell::UniformFull(b)) if a != b => {
                            sem_block_mm.get_or_insert((ci, bi, cb, gb, cb_raw, gb_raw));
                            total_mm += 1;
                        }
                        (BlockCell::Empty(a1), BlockCell::Empty(a2)) if a1.d != a2.d => {
                            sem_block_mm.get_or_insert((ci, bi, cb, gb, cb_raw, gb_raw));
                            total_mm += 1;
                        }
                        _ => {}
                    }
                }
            }
            (ChunkCell::UniformFull(a), ChunkCell::UniformFull(b)) if a != b => {
                sem_chunk_mm.get_or_insert((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                total_mm += 1;
            }
            (ChunkCell::Empty(_), ChunkCell::Empty(_)) => {
                // Chunk-AADFs for Empty chunks are set by `bounds_calc.wgsl`,
                // not by `chunk_calc.wgsl`. The CPU oracle (`construct()`)
                // computes chunk-AADFs via `compute_aadf_layer` as part of
                // its single function call, but this diagnostic runs only
                // chunk_calc + the block/voxel bounds — NOT the chunk
                // bounds. Therefore, Empty(AADF) differences here are EXPECTED
                // and NOT a bug.
            }
            _ => {}
        }
    }

    s.push_str(&format!(
        "  [semantic diff] total mismatches: {total_mm} (NB: Empty-chunk AADFs ignored — bounds_calc not run)\n"
    ));
    if let Some((ci, cc, gc, cr, gr)) = sem_chunk_mm {
        s.push_str(&format!(
            "    CHUNK MM @ ci={ci}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x}\n",
            cc, cr, gc, gr, cr ^ gr,
        ));
    }
    if let Some((ci, bi, cb, gb, cr, gr)) = sem_block_mm {
        s.push_str(&format!(
            "    BLOCK MM @ ci={ci} bi={bi}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x} (bits {:032b})\n",
            cb, cr, gb, gr, cr ^ gr, cr ^ gr,
        ));
    }
    if let Some((ci, bi, vi, cw, gw)) = sem_voxel_mm {
        s.push_str(&format!(
            "    VOXEL MM @ ci={ci} bi={bi} vi={vi}: cpu={:#010x} gpu={:#010x} XOR={:#010x} (bits {:032b})\n",
            cw, gw, cw ^ gw, cw ^ gw,
        ));
    }

    Ok(s)
}

/// Decode a `segment_voxel_buffer`-formatted vector back into a `DenseVolume`
/// of `group_size_in_chunks` extent. Inverse of `build_segment_voxel_buffer`.
fn decode_segment_voxels_to_volume(
    seg: &[u32],
    group_size_in_chunks: [u32; 3],
) -> crate::aadf::construct::DenseVolume {
    use crate::voxel::VoxelTypeId;
    let mut volume = crate::aadf::construct::DenseVolume::empty(group_size_in_chunks);
    let gscx = group_size_in_chunks[0] as usize;
    let gscy = group_size_in_chunks[1] as usize;
    let gscz = group_size_in_chunks[2] as usize;
    for cz in 0..gscz {
        for cy in 0..gscy {
            for cx in 0..gscx {
                let chunk_index_in_segment = cx + cy * gscx + cz * gscx * gscy;
                let chunk_base = chunk_index_in_segment * 2048;
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let pair = seg[block_base + vi];
                                let lo = (pair & 0xFFFF) as u16;
                                let hi = ((pair >> 16) & 0xFFFF) as u16;
                                for (slot, half) in [(0, lo), (1, hi)].iter() {
                                    let voxel_idx = vi * 2 + slot;
                                    let lx = voxel_idx % 4;
                                    let ly = (voxel_idx / 4) % 4;
                                    let lz = voxel_idx / 16;
                                    let vx = cx * 16 + bx * 4 + lx;
                                    let vy = cy * 16 + by * 4 + ly;
                                    let vz = cz * 16 + bz * 4 + lz;
                                    let ty_bits = *half & 0x7FFF;
                                    let full = *half & 0x8000 != 0;
                                    let ty = if full {
                                        VoxelTypeId(ty_bits)
                                    } else {
                                        VoxelTypeId::EMPTY
                                    };
                                    volume.set([vx as u32, vy as u32, vz as u32], ty);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    volume
}

/// Build a `ModelData` with diverse per-chunk content. Each chunk has
/// `(chunk_idx % 3)` classification: 0 = empty, 1 = uniform-full of type
/// `(chunk_idx % 256 + 1)`, 2 = mixed with one solid voxel at position
/// `(chunk_idx % 64)` of block (0,0,0).
fn build_mixed_model_data(model_size_in_chunks: [u32; 3]) -> crate::aadf::generator::ModelData {
    let chunk_count = (model_size_in_chunks[0]
        * model_size_in_chunks[1]
        * model_size_in_chunks[2]) as usize;
    let mut data_chunk = vec![0u32; chunk_count];
    let mut data_block: Vec<u32> = Vec::new();
    let mut data_voxel: Vec<u32> = Vec::new();
    for ci in 0..chunk_count {
        let kind = ci % 3;
        match kind {
            0 => {
                // empty
                data_chunk[ci] = 0u32;
            }
            1 => {
                // uniform-full
                let ty = ((ci % 255) + 1) as u32;
                data_chunk[ci] = (1u32 << 30) | (ty & 0x3FFF_FFFF);
            }
            _ => {
                // mixed: 64 blocks for this chunk, only block 0 is mixed
                // (one voxel at position `ci % 64` of block 0).
                let block_base = data_block.len() as u32;
                data_chunk[ci] = (2u32 << 30) | block_base;
                // Reserve 64 blocks.
                data_block.resize(data_block.len() + 64, 0u32);
                // Block 0: mixed, voxels base.
                let voxel_base = data_voxel.len() as u32;
                data_block[block_base as usize] = (2u32 << 30) | voxel_base;
                // 32 voxel-pairs = 64 voxels for block 0.
                data_voxel.resize(data_voxel.len() + 32, 0u32);
                let pos: u32 = (ci % 64) as u32;
                // Place full voxel type 0x42 at position `pos`. Pos 0..64
                // decodes to lx=pos%4, ly=(pos/4)%4, lz=pos/16 — voxel
                // index in block is pos. In data_voxel, voxel index ÷ 2 =
                // pair index; even slot is low half, odd slot is high half.
                let pair = pos / 2;
                let voxel_word = if pos % 2 == 0 {
                    (1u32 << 15) | 0x42 // even slot, low half
                } else {
                    ((1u32 << 15) | 0x42) << 16 // odd slot, high half
                };
                data_voxel[voxel_base as usize + pair as usize] = voxel_word;
            }
        }
    }
    crate::aadf::generator::ModelData {
        data_chunk,
        data_block: if data_block.is_empty() { vec![0] } else { data_block },
        data_voxel: if data_voxel.is_empty() { vec![0] } else { data_voxel },
        size_in_chunks: model_size_in_chunks,
    }
}

/// Build a segment voxel buffer for ONE segment (chunks at offset
/// `chunk_offset` with extent `segment_size_in_chunks` per axis). Only the
/// chunks inside the segment are populated; out-of-volume chunks (if the
/// segment partially extends beyond the volume) are zero.
fn build_segment_voxel_buffer_for_region(
    volume: &crate::aadf::construct::DenseVolume,
    chunk_offset: [u32; 3],
    segment_size_in_chunks: u32,
) -> Vec<u32> {
    let seg = segment_size_in_chunks as usize;
    let total_u32s = seg * seg * seg * 2048;
    let mut out = vec![0u32; total_u32s];
    let sc = volume.size_in_chunks;
    for lcz in 0..seg {
        let cz = chunk_offset[2] as usize + lcz;
        if cz >= sc[2] as usize { continue; }
        for lcy in 0..seg {
            let cy = chunk_offset[1] as usize + lcy;
            if cy >= sc[1] as usize { continue; }
            for lcx in 0..seg {
                let cx = chunk_offset[0] as usize + lcx;
                if cx >= sc[0] as usize { continue; }
                let chunk_index_in_segment = lcx + lcy * seg + lcz * seg * seg;
                let chunk_base = chunk_index_in_segment * 2048;
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let lo = voxel_at_block_local(
                                    volume, [cx, cy, cz], [bx, by, bz], vi * 2,
                                );
                                let hi = voxel_at_block_local(
                                    volume, [cx, cy, cz], [bx, by, bz], vi * 2 + 1,
                                );
                                out[block_base + vi] = (lo as u32) | ((hi as u32) << 16);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

fn chunk_kind(c: &crate::aadf::cell::ChunkCell) -> u8 {
    match c {
        crate::aadf::cell::ChunkCell::Empty(_) => 0,
        crate::aadf::cell::ChunkCell::UniformFull(_) => 1,
        crate::aadf::cell::ChunkCell::Mixed(_) => 2,
    }
}

fn block_kind(b: &crate::aadf::cell::BlockCell) -> u8 {
    match b {
        crate::aadf::cell::BlockCell::Empty(_) => 0,
        crate::aadf::cell::BlockCell::UniformFull(_) => 1,
        crate::aadf::cell::BlockCell::Mixed(_) => 2,
    }
}

/// Build `segment_voxel_buffer` for a fixture using the full world's
/// dimensions, indexed via `chunk_index = cx + cy*S + cz*S*S` where
/// `S = segment_size_in_chunks = max(size_in_chunks)`. Chunks outside the
/// volume's extent are left zero.
fn build_segment_voxel_buffer_for_world(
    volume: &crate::aadf::construct::DenseVolume,
    segment_size_in_chunks: u32,
) -> Vec<u32> {
    let seg = segment_size_in_chunks as usize;
    let sc = volume.size_in_chunks;
    // Size precisely: the max chunk_index_in_segment accessed is
    // `(sc[0]-1) + (sc[1]-1)*seg + (sc[2]-1)*seg*seg`, each chunk consumes
    // 2048 u32s. Add 1 for the inclusive bound.
    let max_chunk_idx = (sc[0] as usize - 1)
        + (sc[1] as usize - 1) * seg
        + (sc[2] as usize - 1) * seg * seg;
    let total_u32s = (max_chunk_idx + 1) * 2048;
    let mut out = vec![0u32; total_u32s];

    for cz in 0..(sc[2] as usize) {
        for cy in 0..(sc[1] as usize) {
            for cx in 0..(sc[0] as usize) {
                let chunk_index_in_segment = cx + cy * seg + cz * seg * seg;
                let chunk_base = chunk_index_in_segment * 2048;

                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let lo = voxel_at_block_local(
                                    volume,
                                    [cx, cy, cz],
                                    [bx, by, bz],
                                    vi * 2,
                                );
                                let hi = voxel_at_block_local(
                                    volume,
                                    [cx, cy, cz],
                                    [bx, by, bz],
                                    vi * 2 + 1,
                                );
                                out[block_base + vi] =
                                    (lo as u32) | ((hi as u32) << 16);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// W4 — entry point for `bevy-naadf e2e_render --entities`.
///
/// Runs `EntityHandler::update` for a small fixture (one moving entity in a
/// 4×4×4-chunk world, 2 frames of motion), asserts the uploads are
/// well-formed, and reports a short summary. Until wave-3 wires the
/// render-side dispatch, this is the load-bearing W4 e2e gate.
///
/// The fixture:
/// - World is 4×4×4 chunks = 64 chunks.
/// - One entity (size 8×8×8) at world position (16, 16, 16) — straddles
///   the chunk-boundary at every axis, so it overlaps 8 chunks (the
///   2×2×2 chunks at the boundary).
/// - Frame 2: same entity translated to (24, 16, 16); the entity now
///   overlaps a different but partially-overlapping set of 8 chunks, so
///   the handler must emit "clear" updates for the no-longer-overlapped
///   chunks + "set" updates for the new ones.
///
/// Asserts:
/// - Frame 1: 8 chunk_updates (the 2×2×2 overlapped chunks), 8 entity
///   chunk instances (dedup never fires because each chunk's instance
///   list is unique by chunk-id ordering), 1 entity_history entry.
/// - Frame 2: the chunk_updates include both old chunks (cleared) and
///   new chunks (set) — at least 8 updates.
/// Phase-C W2 — entry point for `e2e_render --edit-mode`.
///
/// Runs a CPU-side scripted edit end-to-end via the `set_voxel` API and
/// asserts:
///   1. The edit produces a non-empty `PendingEdits.batches`.
///   2. The CPU `process_edit_batch` produces well-formed `changed_chunks` +
///      `changed_blocks` / `changed_voxels` arrays.
///   3. The flood-fill CPU oracle (`change_handler::compute_change_groups`)
///      produces the expected `changed_groups` array for the edit's group.
///
/// This is a **CPU-side** end-to-end validation — equivalent to W4's
/// `validate_entity_handler` design. The GPU bit-exact validation lives in
/// the `world_change::tests` GPU test suite; this flag is for catching
/// integration-level regressions (the `set_voxel` → `process_edit_batch` →
/// `compute_change_groups` chain) without requiring a windowed GPU run.
pub fn validate_edit_mode() -> Result<String, String> {
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::voxel::{VoxelTypeId, CELL_DIM};
    use crate::world::data::WorldData;
    use bevy::prelude::UVec3;

    // Build a 4×2×4-chunk world matching the production test grid layout.
    let size_in_chunks = [4u32, 2, 4];
    let mut volume = DenseVolume::empty(size_in_chunks);
    // Put a single full voxel at (16, 4, 16) — chunk (1, 0, 1)'s corner — so
    // there's some non-empty geometry around the planned edit position.
    volume.set([16, 4, 16], VoxelTypeId(5));
    let built = construct(&volume);

    let mut world_data = WorldData {
        chunks_cpu: built.chunks,
        blocks_cpu: built.blocks,
        voxels_cpu: built.voxels,
        size_in_chunks: UVec3::from_array(size_in_chunks),
        bounding_box: crate::world::data::IAabb3 {
            min: bevy::prelude::IVec3::ZERO,
            max: bevy::prelude::IVec3::new(
                (size_in_chunks[0] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
                (size_in_chunks[1] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
                (size_in_chunks[2] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: volume.voxels.iter().map(|t| t.0).collect(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();
    // The pre-edit chunks_cpu — record its bytes to verify the edit changed
    // something.
    let pre_edit_chunks = world_data.chunks_cpu.clone();

    // Apply the scripted edit: set voxel (20, 12, 20) to a new emissive type.
    // This is in chunk (1, 0, 1), block (1, 3, 1), voxel (0, 0, 0).
    let new_type = VoxelTypeId(9);
    world_data.set_voxel(bevy::prelude::IVec3::new(20, 12, 20), new_type);

    if world_data.pending_edits.batches.is_empty() {
        return Err("set_voxel produced no edit batch".into());
    }
    if world_data.pending_edits.edited_groups.is_empty() {
        return Err("set_voxel produced no edited_groups".into());
    }
    let batch = &world_data.pending_edits.batches[0];
    if batch.changed_chunks.is_empty() {
        return Err("edit batch has no changed_chunks".into());
    }
    // NOTE (`02e-perframe-cpu-investigation.md`, 2026-05-16): edit paths no
    // longer set `world_data.dirty = true`. Per-edit changes flow through the
    // W2 delta-upload chain (`pending_edits.batches` → `naadf_world_change_node`);
    // the full-world re-extract that `dirty` triggers is redundant and was
    // causing the per-frame full-world upload bottleneck on Oasis-class worlds.
    // We continue to assert that the batch + chunks_cpu mutation happened
    // (already verified above) — those carry the actual per-edit change.
    if pre_edit_chunks == world_data.chunks_cpu {
        return Err("set_voxel did not mutate chunks_cpu".into());
    }

    // Run the CPU flood-fill — even though `size_in_chunks = [4, 2, 4]` gives
    // `bound_group_count = 0` on the W3 path (Y=2 not divisible by 4), the
    // `compute_change_groups` function handles this with an early-return at
    // the `size_in_groups > 0` check in `extract_world_changes`.
    let size_in_groups = [
        size_in_chunks[0] / 4,
        size_in_chunks[1] / 4,
        size_in_chunks[2] / 4,
    ];
    let flood_groups_len = if size_in_groups[0] > 0
        && size_in_groups[1] > 0
        && size_in_groups[2] > 0
    {
        let groups = change_handler::compute_change_groups(
            size_in_groups,
            &world_data.pending_edits.edited_groups,
        );
        groups.entries.len()
    } else {
        // `size_in_groups[1] == 0` on the 4×2×4 test grid — the W3 bound queues
        // are dormant on this grid; the flood-fill is correctly skipped.
        0
    };

    Ok(format!(
        "edit-mode PASS: 1 set_voxel call produced {} changed_chunks + {} \
         changed_blocks records + {} changed_voxels records; flood-fill produced \
         {} group entries (size_in_groups = {:?})",
        batch.changed_chunks.len(),
        batch.changed_blocks.len() / 65,
        batch.changed_voxels.len() / 33,
        flood_groups_len,
        size_in_groups,
    ))
}

/// `02f` rearch — **runtime-edit gate**. Complements [`validate_edit_mode`]
/// (which exercises the diagnostic `set_voxel` path) by hitting the
/// production runtime brush path: [`crate::world::data::WorldData::set_voxels_batch`].
///
/// **Why this gate exists.** The pre-`02f` CPU-oracle `--edit-mode` gate
/// missed the regression mode "edit landed in main-world `pending_edits`
/// but never crossed to the render world": that gate only exercised
/// `set_voxel` (the diagnostic oracle path) against a self-built
/// `WorldData`, with no extract pass + no render-graph dispatch in scope.
/// The `dirty=true never on edits` failure mode (`02e`/`03e` followup)
/// slipped through because `set_voxel` produces an edit batch INDEPENDENT
/// of the dirty flag, and the CPU oracle gate doesn't observe the
/// extract-to-render-world ferry.
///
/// **What this gate asserts.** Builds a minimal in-process world, calls
/// [`crate::world::data::WorldData::set_voxels_batch`] (the production
/// brush entry point — same code path the editor's `apply_edit_tool`
/// invokes), and asserts:
///
/// 1. The batch produces a non-empty `pending_edits.batches` AND
///    non-empty `pending_edits.edited_groups`.
/// 2. The runtime path's `changed_chunks` array is non-empty (proving
///    `process_edit_batch` ran and emitted records — the load-bearing W2
///    delta payload).
/// 3. The runtime path does NOT emit the synthetic whole-world AADF
///    refresh records (i.e. `changed_chunks.len()` is in the touched-chunk
///    range, NOT the whole-world range) — this confirms the runtime path
///    skips `recompute_chunk_layer_aadfs` (the diagnostic-only CPU rebuild
///    the `02f` rearch retires from the production hot path).
///
/// **What this gate does NOT verify** (out of scope for an in-process
/// gate): the GPU render-graph dispatch (`naadf_world_change_node` →
/// `apply_chunk_change.wgsl` + the 3 sibling shaders), the
/// extract-to-`ConstructionEvents` flow, the GPU buffer mutation, the
/// framebuffer luminance delta. Those need a windowed harness with
/// before/after screenshot comparison; out of this dispatch's scope.
/// **The asymmetric coverage is deliberate** — this gate closes the
/// regression hole the `02e`/`03e` followup left open (edit-doesn't-reach-
/// W2-batch) without re-implementing a full integration test.
pub fn validate_runtime_edit_mode() -> Result<String, String> {
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::voxel::{VoxelTypeId, CELL_DIM};
    use crate::world::data::WorldData;
    use bevy::prelude::UVec3;

    // 4×4×4-chunk world — bigger than the `--edit-mode` 4×2×4 fixture so
    // the brush's chunk-AABB distinguishes "touched chunks only" from
    // "whole-world" (4×4×4 = 64 chunks vs ~125 chunks the brush touches
    // at r=16 — the 4×4×4 fixture's 64 chunks is the WHOLE world).
    let size_in_chunks = [4u32, 4, 4];
    let mut volume = DenseVolume::empty(size_in_chunks);
    volume.set([16, 16, 16], VoxelTypeId(5));
    let built = construct(&volume);
    let total_chunks = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;

    let mut world_data = WorldData {
        chunks_cpu: built.chunks,
        blocks_cpu: built.blocks,
        voxels_cpu: built.voxels,
        size_in_chunks: UVec3::from_array(size_in_chunks),
        bounding_box: crate::world::data::IAabb3 {
            min: bevy::prelude::IVec3::ZERO,
            max: bevy::prelude::IVec3::new(
                (size_in_chunks[0] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
                (size_in_chunks[1] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
                (size_in_chunks[2] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: volume.voxels.iter().map(|t| t.0).collect(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();

    // Production brush path — same call shape the editor's brushes use
    // (`editor/tools.rs::paint_brush` / `cube_brush` / `sphere_brush`).
    // Three voxels in two adjacent chunks: tests the by-chunk grouping +
    // the multi-chunk batched dispatch.
    let new_type = VoxelTypeId(9);
    let edits = [
        (bevy::prelude::IVec3::new(20, 12, 20), new_type),
        (bevy::prelude::IVec3::new(21, 12, 20), new_type),
        (bevy::prelude::IVec3::new(36, 12, 36), new_type),
    ];
    world_data.set_voxels_batch(&edits);

    // Gate 1 — the runtime path produced a non-empty edit batch.
    if world_data.pending_edits.batches.is_empty() {
        return Err(
            "runtime-edit gate FAIL: set_voxels_batch produced no edit batch \
             — the W2 delta chain has no work; edits would silently never \
             reach the GPU. This is the regression mode the `02f` rearch \
             addresses (edit-doesn't-reach-W2-batch)."
                .into(),
        );
    }
    if world_data.pending_edits.edited_groups.is_empty() {
        return Err(
            "runtime-edit gate FAIL: set_voxels_batch produced no \
             edited_groups — the `compute_change_groups` BFS oracle in \
             `extract_world_changes` would skip with no work, and the W2 \
             GPU dispatch's `changed_groups_dynamic` would never populate."
                .into(),
        );
    }

    // Gate 2 — the batch carries `changed_chunks` records.
    let batch = &world_data.pending_edits.batches[0];
    if batch.changed_chunks.is_empty() {
        return Err(
            "runtime-edit gate FAIL: edit batch has no changed_chunks \
             records — `process_edit_batch` failed to emit per-touched-\
             chunk records. The W2 GPU dispatch's `apply_chunk_change` \
             pass would have no input."
                .into(),
        );
    }

    // Gate 3 — the runtime path is the production fast path, NOT the
    // diagnostic oracle. The runtime path's `changed_chunks` count is
    // bounded by the touched-chunk count (~2 here — two edits in chunk
    // (1,0,1) and one edit in chunk (2,0,2)). The diagnostic oracle path
    // would emit synthetic AADF-refresh entries for many more chunks
    // (up to ~total_chunks - touched). Assert the runtime path is NOT
    // accidentally invoking the oracle's whole-world recompute.
    let touched_chunks = batch.changed_chunks.len();
    // The runtime path touches at most the chunks listed in the edit set
    // (3 edits → at most 2 unique chunks). A whole-world recompute would
    // emit on the order of `total_chunks` (= 64) records. Assert the
    // ratio is small (touched / total ≤ 0.5 = 32) — well below the
    // whole-world threshold.
    if touched_chunks > total_chunks / 2 {
        return Err(format!(
            "runtime-edit gate FAIL: runtime path emitted {touched_chunks} \
             changed_chunks records for a brush that touched ≤2 chunks (out \
             of {total_chunks} total) — likely accidentally invoking the \
             diagnostic `recompute_chunk_layer_aadfs` whole-world rehash on \
             the runtime hot path, which the `02f` rearch retires."
        ));
    }

    // Gate 4 — chunks_cpu was mutated in place (proves the CPU mirror
    // patch landed; the editor's mouse-pick ray_traversal reads
    // chunks_cpu and would see stale state if this skipped).
    let pre_state = built_pre_edit_state(&volume, &edits);
    let any_chunk_mutated = pre_state.iter().enumerate().any(|(ci, &pre)| {
        ci < world_data.chunks_cpu.len() && world_data.chunks_cpu[ci] != pre
    });
    if !any_chunk_mutated {
        return Err(
            "runtime-edit gate FAIL: set_voxels_batch did not mutate any \
             chunks_cpu entry — the CPU mirror patch (cheap in-place; the \
             `02f` rearch directive bullet #5) did not land. \
             `WorldData::ray_traversal` would read stale state."
                .into(),
        );
    }

    Ok(format!(
        "runtime-edit gate PASS: set_voxels_batch produced {} batch(es) \
         with {} changed_chunks + {} changed_blocks + {} changed_voxels \
         records (out of {total_chunks} total chunks — runtime path \
         touched-only, NOT whole-world rehash); {} edited_groups for the \
         BFS oracle. CPU mirror patched in-place.",
        world_data.pending_edits.batches.len(),
        touched_chunks,
        batch.changed_blocks.len() / 65,
        batch.changed_voxels.len() / 33,
        world_data.pending_edits.edited_groups.len(),
    ))
}

/// `02f` runtime-edit gate helper — rebuild the `chunks_cpu` mirror that
/// `construct(&volume)` would have produced (the pre-edit baseline) so the
/// gate can diff against post-edit `chunks_cpu`. Implementation: re-run
/// `construct` and return its `chunks` (cheap; the test fixture is 4×4×4
/// chunks).
fn built_pre_edit_state(
    volume: &crate::aadf::construct::DenseVolume,
    _edits: &[(bevy::prelude::IVec3, crate::voxel::VoxelTypeId)],
) -> Vec<u32> {
    crate::aadf::construct::construct(volume).chunks
}

pub fn validate_entity_handler() -> Result<String, String> {
    use crate::aadf::entity::decompress_quaternion;
    use crate::render::construction::entity_handler::EntityHandler;
    use crate::render::gpu_types::EntityInstance;
    use bevy::math::Vec3;

    let mut handler = EntityHandler::new([4, 4, 4]);
    // Place the entity so it overlaps the 2×2×2 chunks at the boundary.
    // Chunks are 16 voxels wide; the entity is 8 voxels; positioning at
    // (12, 12, 12) means [12..20] × [12..20] × [12..20] which straddles
    // the (0,0,0)/(1,1,1) chunk boundaries.
    let frame_a = vec![EntityInstance {
        position: Vec3::new(12.0, 12.0, 12.0),
        quaternion: [0.0, 0.0, 0.0, 1.0],
        voxel_start: 0,
        entity: 0,
        size: [8, 8, 8],
    }];
    let uploads_a = handler.update(&frame_a);
    if uploads_a.entity_history.len() != 1 {
        return Err(format!(
            "frame A: expected 1 entity_history entry, got {}",
            uploads_a.entity_history.len()
        ));
    }
    if uploads_a.chunk_updates.is_empty() {
        return Err("frame A: expected non-zero chunk_updates".into());
    }
    if uploads_a.entity_chunk_instances.is_empty() {
        return Err("frame A: expected non-zero entity_chunk_instances".into());
    }
    let frame_a_overlap_count = uploads_a.chunk_updates.len();

    // Frame B — entity moved.
    let frame_b = vec![EntityInstance {
        position: Vec3::new(20.0, 12.0, 12.0),
        quaternion: [0.0, 0.0, 0.0, 1.0],
        voxel_start: 0,
        entity: 0,
        size: [8, 8, 8],
    }];
    let uploads_b = handler.update(&frame_b);
    if uploads_b.chunk_updates.is_empty() {
        return Err("frame B: expected non-zero chunk_updates".into());
    }

    // Verify the quaternion roundtrip on the history slot.
    let history = uploads_a.entity_history[0];
    let q_decoded = decompress_quaternion((history.data3, history.data4));
    // Identity quaternion compresses + decompresses with ~ component (0,0,0,1).
    if q_decoded[3].abs() < 0.99 {
        return Err(format!(
            "history slot quaternion did not roundtrip identity: w = {}",
            q_decoded[3]
        ));
    }

    Ok(format!(
        "frame A: {} chunk_updates, {} entity_chunk_instances, {} history; \
         frame B: {} chunk_updates",
        frame_a_overlap_count,
        uploads_a.entity_chunk_instances.len(),
        uploads_a.entity_history.len(),
        uploads_b.chunk_updates.len(),
    ))
}

#[cfg(test)]
mod tests {
    //! W5 — the load-bearing bit-exact `GPU vs CPU` oracle test
    //! (`15-design-c.md` §1.6, §2.1 W5 row, §4.5).
    //!
    //! Builds a small fixed `ModelData`, runs BOTH the GPU pipeline and the
    //! `crate::aadf::generator::generate_segment_cpu` oracle against the same
    //! inputs, maps the GPU `segment_voxel_buffer` back to the CPU, and
    //! asserts byte-for-byte equality with the oracle's output.
    //!
    //! The test uses the same headless `App + RenderPlugin` fixture pattern
    //! `world::buffer::tests` uses (`world/buffer.rs:227-264`): build a
    //! minimal `App` with `RenderPlugin`, `finish()` + `cleanup()` to make
    //! `RenderDevice`/`RenderQueue` available, then drive the W5 dispatch
    //! by hand. No render schedule runs — that would require a full plugin
    //! set + a window.
    //!
    //! Skips with a warning when no wgpu adapter is available (CI box without
    //! a GPU). The CPU-oracle determinism + Y-clamp + OOB tests in
    //! `crate::aadf::generator::tests` exercise the oracle independently of
    //! the GPU path.
    use super::generator_model::{
        create_params_uniform, create_storage_buffer_u32, dispatch_generator_model,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        GpuGeneratorModelParams, CHUNK_DATA_U32S, GENERATOR_MODEL_SHADER,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::aadf::generator::{generate_segment_cpu, ModelData};

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets, Handle};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    /// Build a headless render world (same plumbing as
    /// `world::buffer::tests::render_device_queue`). Returns the `App`, the
    /// device, the queue, and an inline-built `Handle<Shader>` for the W5
    /// generator-model WGSL.
    ///
    /// The test does NOT drive the standard `ExtractSchedule` (which would
    /// require all the `bevy_render::render_asset` Message types to be
    /// initialised — `MinimalPlugins` + `AssetPlugin` only initialise some
    /// of them). Instead, it populates the cache's shader registry directly
    /// via the `pub fn PipelineCache::set_shader` entry point, which is
    /// exactly what `extract_shaders` does internally.
    fn render_fixture() -> Option<(App, RenderDevice, RenderQueue, Handle<Shader>)> {
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

        // Build a stable handle out of an inserted shader in the main world's
        // `Assets<Shader>` — the handle's `AssetId` is what the pipeline
        // cache keys on.
        let shader = Shader::from_wgsl(
            GENERATOR_MODEL_SHADER_SRC,
            "shaders/generator_model.wgsl",
        );
        let shader_clone = shader.clone();
        let shader_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(shader);

        // Inject the shader directly into the pipeline cache (mirrors what
        // `extract_shaders` does at the end of `ExtractSchedule`).
        let render_app = app.get_sub_app_mut(RenderApp)?;
        {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.set_shader(shader_handle.id(), shader_clone);
        }

        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((app, device, queue, shader_handle))
    }

    /// Read back the first `count` `u32`s of `src`.
    fn readback_u32(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &bevy::render::render_resource::Buffer,
        count: u64,
    ) -> Vec<u32> {
        let size = count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("naadf_generator_readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("naadf_generator_readback_encoder"),
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

    /// W5's load-bearing test: GPU `generator_model.wgsl` output is
    /// **byte-for-byte equal** to `generate_segment_cpu` (the §1.6 oracle).
    ///
    /// Test setup: a 2×1×2 chunk model, uniform-full of type 0x42, queried
    /// over a 2×1×2 chunk segment. The model's voxel-level data path is
    /// exercised by the construct-and-run paths below; the Y-clamp does not
    /// fire (segment matches model height); the OOB short-circuit does not
    /// fire (segment matches world `sizeInVoxels`).
    ///
    /// Buffer size: 2×1×2 chunks × 2048 u32s = 8192 u32s = 32 KiB.
    #[test]
    fn generator_model_gpu_vs_cpu_bit_exact() {
        let Some((mut app, device, queue, shader_handle)) = render_fixture() else {
            eprintln!("no wgpu device — skipping W5 GPU/CPU bit-exact test");
            return;
        };

        // Fixed test inputs.
        let model = ModelData::uniform_full([2, 1, 2], 0x42);
        let group_offset_in_chunks = [0u32, 0, 0];
        let group_size_in_chunks = [2u32, 1, 2];
        let size_in_voxels = [32u32, 16, 32];

        // === CPU oracle =====================================================
        let cpu_out = generate_segment_cpu(
            &model,
            group_offset_in_chunks,
            group_size_in_chunks,
            size_in_voxels,
        );
        let total_chunks =
            (group_size_in_chunks[0] * group_size_in_chunks[1] * group_size_in_chunks[2])
                as u64;
        let chunk_data_u32s = total_chunks * CHUNK_DATA_U32S as u64;
        assert_eq!(cpu_out.len() as u64, chunk_data_u32s);

        // === GPU path =======================================================
        // Queue the pipeline + build the layout. We deliberately do NOT use
        // `app.init_gpu_resource::<ConstructionPipelines>()` here because that
        // pulls in the full `FromWorld` impl + the AssetServer load; for the
        // W5 isolated test, we go straight to the helpers + the inline shader.
        let layout = generator_model_layout_descriptor();
        let pipeline_id = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let pipeline_cache = render_app.world().resource::<PipelineCache>();
            queue_generator_model_pipeline_with_handle(
                pipeline_cache,
                layout.clone(),
                shader_handle.clone(),
            )
        };

        // Allocate the GPU buffers + uniform.
        let chunk_data_init = vec![0xDEAD_BEEFu32; chunk_data_u32s as usize];
        let gpu_chunk_data = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_segment_voxel_buffer_test",
            &chunk_data_init,
        );
        let gpu_model_chunk = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_chunk_test",
            &model.data_chunk,
        );
        let gpu_model_block = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_block_test",
            &model.data_block,
        );
        let gpu_model_voxel = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_voxel_test",
            &model.data_voxel,
        );
        let params = GpuGeneratorModelParams {
            size_in_voxels,
            _pad0: 0,
            model_size_in_chunks: model.size_in_chunks,
            _pad1: 0,
            group_offset_in_chunks,
            group_size_in_chunks_x: group_size_in_chunks[0],
            group_size_in_chunks_y: group_size_in_chunks[1],
            dispatch_offset: 0,
            _pad3: 0,
            _pad4: 0,
        };
        let gpu_params = create_params_uniform(&device, &queue, &params);

        // Drive the pipeline to compile, then dispatch. The pipeline cache's
        // background compile is gated by `synchronous_pipeline_compilation`
        // (set true above), so `App::update()` once should be enough to fully
        // resolve the pipeline.
        // Drive pipeline compilation manually — `PipelineCache::process_queue`
        // is `pub` and `synchronous_pipeline_compilation = true` on the
        // RenderPlugin above forces compile-on-call. One pass is usually
        // enough; cap at 64 defensively.
        let pipeline = {
            let render_app = app.get_sub_app_mut(RenderApp).unwrap();
            let mut got = None;
            for _ in 0..64 {
                let mut pipeline_cache =
                    render_app.world_mut().resource_mut::<PipelineCache>();
                pipeline_cache.process_queue();
                if let Some(p) = pipeline_cache.get_compute_pipeline(pipeline_id) {
                    got = Some(p.clone());
                    break;
                }
            }
            got.expect("W5 generator_model pipeline did not compile in 64 ticks")
        };

        // Build the bind group against the resolved layout.
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let pipeline_cache = render_app.world().resource::<PipelineCache>();
        let bind_group_layout = pipeline_cache.get_bind_group_layout(&layout);
        let bind_group = device.create_bind_group(
            "naadf_generator_model_bind_group_test",
            &bind_group_layout,
            &BindGroupEntries::sequential((
                gpu_chunk_data.as_entire_buffer_binding(),
                gpu_model_chunk.as_entire_buffer_binding(),
                gpu_model_block.as_entire_buffer_binding(),
                gpu_model_voxel.as_entire_buffer_binding(),
                gpu_params.as_entire_buffer_binding(),
            )),
        );

        dispatch_generator_model(
            &device,
            &queue,
            &pipeline,
            &bind_group,
            group_size_in_chunks,
        );

        // Map the GPU buffer back + compare byte-for-byte to the oracle.
        let gpu_out = readback_u32(&device, &queue, &gpu_chunk_data, chunk_data_u32s);
        assert_eq!(gpu_out.len(), cpu_out.len());

        // Find the first divergence (if any) for a useful failure message.
        if gpu_out != cpu_out {
            for (i, (&g, &c)) in gpu_out.iter().zip(cpu_out.iter()).enumerate() {
                if g != c {
                    panic!(
                        "W5 GPU vs CPU bit-exact divergence @ u32[{i}]: gpu={g:#010x} cpu={c:#010x}"
                    );
                }
            }
        }
        assert_eq!(gpu_out, cpu_out, "W5 GPU output must equal CPU oracle byte-for-byte");

        // Sanity: the test exercised a real workload, not an all-zeros buffer.
        // For the uniform-full model + matching segment, every u32 should pack
        // two type-0x42 voxels with the full flag set.
        let expected_voxel = 0x42u32 | (1u32 << 15);
        let expected_packed = expected_voxel | (expected_voxel << 16);
        assert_eq!(
            gpu_out[0], expected_packed,
            "uniform-full model should pack 0x{:04x}_{:04x} into every voxel pair",
            expected_voxel, expected_voxel
        );
        // And the buffer is not the placeholder 0xDEADBEEF init pattern.
        assert!(gpu_out.iter().all(|&u| u != 0xDEAD_BEEF));
        // Reference `GENERATOR_MODEL_SHADER` so the constant stays load-bearing
        // (a future rename of the asset path must trip the test compile).
        let _ = GENERATOR_MODEL_SHADER;
    }
}

#[cfg(test)]
mod tests_w1 {
    //! W1 — the load-bearing bit-exact `GPU vs CPU` oracle test
    //! (`15-design-c.md` §1.6, §2.1 W1 row, §4.1; `16-impl-c-W1.md`).
    //!
    //! Builds a small `DenseVolume`, runs the CPU oracle (`aadf::construct::
    //! construct`) AND the GPU `chunk_calc.wgsl` 3-entry-point chain (Algorithm
    //! 1 → voxel-AADFs → block-AADFs), maps GPU `blocks`/`voxels` + the chunks
    //! texture back to CPU, and asserts byte-equality.
    //!
    //! The W1 test is the load-bearing W1 deliverable per the brief: it
    //! exercises every shader entry point + the `BlockHashingHandler` Rust
    //! port + the `HashValueSlot` atomicity discipline.
    //!
    //! Note on pointer-assignment determinism: when the CPU
    //! `HashMap<[VoxelTypeId; 64], VoxelPtr>` and the GPU
    //! `open-addressing-by-hash` assign different `VoxelPtr` values to the
    //! same set of unique blocks (because Rust `HashMap` iterates in a hash-
    //! seed-randomised order, while the GPU assigns by hash-mod-mapsize), the
    //! blocks and voxels buffers may not be byte-equal at the *u32-content*
    //! level. The test mitigates this by exercising a small layer where the
    //! pointer space is trivially deterministic (single mixed block ⇒ both
    //! paths assign `VoxelPtr(0)`).

    use super::chunk_calc::{
        construction_world_layout_descriptor,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use super::hashing::hash_coefficients;
    use super::map_copy::{
        map_copy_layout_descriptor, queue_copy_map_pipeline_with_handle, GpuMapCopyParams,
        MAP_COPY_SHADER_SRC,
    };
    use crate::aadf::cell::{BlockCell, ChunkCell, VoxelCell, VoxelPtr};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::render::gpu_types::{GpuConstructionParams, GpuHashValueSlot};
    use crate::voxel::VoxelTypeId;

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets, Handle};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    /// Build a headless render world + inject the W1 chunk_calc + map_copy
    /// shaders. Returns the app, the device, queue, and `Handle<Shader>` for
    /// both W1 shaders.
    #[allow(clippy::type_complexity)]
    fn render_fixture_w1() -> Option<(App, RenderDevice, RenderQueue, Handle<Shader>, Handle<Shader>)> {
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

        let chunk_calc_shader = Shader::from_wgsl(
            CHUNK_CALC_SHADER_SRC,
            "shaders/chunk_calc.wgsl",
        );
        let chunk_calc_shader_clone = chunk_calc_shader.clone();
        let chunk_calc_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(chunk_calc_shader);
        let map_copy_shader = Shader::from_wgsl(
            MAP_COPY_SHADER_SRC,
            "shaders/map_copy.wgsl",
        );
        let map_copy_shader_clone = map_copy_shader.clone();
        let map_copy_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(map_copy_shader);

        let render_app = app.get_sub_app_mut(RenderApp)?;
        {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.set_shader(chunk_calc_handle.id(), chunk_calc_shader_clone);
            pipeline_cache.set_shader(map_copy_handle.id(), map_copy_shader_clone);
        }

        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((app, device, queue, chunk_calc_handle, map_copy_handle))
    }

    fn create_storage_u32(
        device: &RenderDevice,
        queue: &RenderQueue,
        label: &'static str,
        data: &[u32],
    ) -> bevy::render::render_resource::Buffer {
        let data = if data.is_empty() { &[0u32][..] } else { data };
        let size = (data.len() * 4) as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
        buffer
    }

    fn create_uniform<T: bytemuck::Pod>(
        device: &RenderDevice,
        queue: &RenderQueue,
        label: &'static str,
        data: &T,
    ) -> bevy::render::render_resource::Buffer {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: std::mem::size_of::<T>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(data));
        buffer
    }

    fn readback_u32(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &bevy::render::render_resource::Buffer,
        u32_count: u64,
    ) -> Vec<u32> {
        let size = u32_count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("w1_readback_staging"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_readback"),
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

    /// Read the entire Rg32Uint 3D chunks texture back to CPU as a flat `u32`
    /// vector of the `.x` channel (the construction-state channel; the `.y`
    /// channel is the W4 entity pointer + counter, zero in this test). Order
    /// is `cz * cx * cy + cy * cx + cx` (x-fastest), matching
    /// `WorldData.chunks_cpu`'s convention.
    fn readback_chunks_buffer(
        device: &RenderDevice,
        queue: &RenderQueue,
        chunks: &bevy::render::render_resource::Buffer,
        size: [u32; 3],
    ) -> Vec<u32> {
        // Web-WebGPU migration: chunks is an `array<vec2<u32>>` storage
        // buffer (8 B per pair). Buffer→buffer copy doesn't need the
        // 256-byte row alignment a 3D-texture readback required.
        let chunk_count = (size[0] * size[1] * size[2]) as u64;
        let staging_size = chunk_count * 8;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("w1_chunks_readback_staging"),
            size: staging_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_chunks_readback"),
        });
        encoder.copy_buffer_to_buffer(chunks, 0, &staging, 0, staging_size);
        queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let raw = slice.get_mapped_range();
        let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
        let out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
        drop(raw);
        staging.unmap();
        assert_eq!(out.len() as u64, chunk_count);
        out
    }

    /// Drive the pipeline cache through `process_queue` until either every
    /// id has compiled or the iteration cap fires.
    fn compile_pipelines(
        app: &mut App,
        ids: &[bevy::render::render_resource::CachedComputePipelineId],
    ) -> Option<Vec<bevy::render::render_resource::ComputePipeline>> {
        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        for _ in 0..64 {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.process_queue();
            let pipeline_cache = render_app.world().resource::<PipelineCache>();
            let mut got = Vec::with_capacity(ids.len());
            let mut all_ready = true;
            for id in ids {
                if let Some(p) = pipeline_cache.get_compute_pipeline(*id) {
                    got.push(p.clone());
                } else {
                    all_ready = false;
                    break;
                }
            }
            if all_ready {
                return Some(got);
            }
        }
        None
    }

    /// W1's load-bearing test: GPU `chunk_calc.wgsl` 3-entry-point chain
    /// produces blocks + voxels + chunks bit-equal to the CPU oracle
    /// `aadf::construct::construct`.
    ///
    /// The test uses a tiny 1×1×1 chunk world with a single mixed block so
    /// the `VoxelPtr` assignment is deterministic (the only mixed block gets
    /// `VoxelPtr(0)` on both paths — see file doc note on pointer-assignment
    /// determinism).
    #[test]
    fn gpu_algorithm1_vs_cpu_bit_exact() {
        let Some((mut app, device, queue, chunk_calc_handle, _map_copy_handle)) =
            render_fixture_w1()
        else {
            eprintln!("no wgpu device — skipping W1 GPU/CPU bit-exact test");
            return;
        };

        // === Tiny test scene: 1×1×1 chunk world, single mixed block =========
        let mut volume = DenseVolume::empty([1, 1, 1]);
        let ty = VoxelTypeId(7);
        // Put one solid voxel at the origin — the chunk + the (0,0,0) block
        // become mixed, the other 63 blocks stay empty. Exactly one Mixed
        // entry in the dedup table ⇒ `VoxelPtr(0)`.
        volume.set([0, 0, 0], ty);

        // === CPU oracle =====================================================
        let oracle = construct(&volume);
        assert_eq!(oracle.chunks.len(), 1);
        assert_eq!(oracle.blocks.len(), 64);
        assert_eq!(oracle.voxels.len(), 32);
        // Sanity: the chunk decodes Mixed.
        assert!(matches!(ChunkCell::decode(oracle.chunks[0]), ChunkCell::Mixed(_)));

        // === GPU setup ======================================================
        let segment_size_in_chunks: u32 = 1;
        let size_in_chunks: [u32; 3] = volume.size_in_chunks;

        // The segment-voxel-buffer: 1 chunk × 2048 u32s.
        let segment_voxels =
            super::build_segment_voxel_buffer(&volume, segment_size_in_chunks);
        assert_eq!(segment_voxels.len(), 2048);

        // The hash map — a power-of-two slot array. For 1 mixed block, even a
        // small size like 256 is comfortable headroom. Each slot is 16 B
        // (`GpuHashValueSlot`).
        let hash_map_size_slots: u32 = 256;
        let hash_map_init: Vec<u32> =
            vec![0u32; (hash_map_size_slots as usize) * 4]; // 4 u32 per slot.

        // The block-voxel-count cursor: [voxel_cursor, block_cursor]. NAADF
        // seeds it to [64, 64] (`WorldData.cs:129`) so `VoxelPtr(0)` /
        // `BlockPtr(0)` are reserved sentinels (the dedup-empty value
        // `EMPTY_BLOCK = 0` distinguishes from a real slot at offset 0). W1
        // uses the same seed.
        let block_voxel_count_init: Vec<u32> = vec![64, 64];

        // The hash coefficients table.
        let coeffs = hash_coefficients().to_vec();

        // Allocate GPU buffers.
        let gpu_blocks = create_storage_u32(
            &device,
            &queue,
            "w1_blocks",
            &vec![0u32; oracle.blocks.len().max(64) + 64], // extra headroom past oracle
        );
        let gpu_voxels = create_storage_u32(
            &device,
            &queue,
            "w1_voxels",
            &vec![0u32; oracle.voxels.len().max(32) + 32], // extra headroom
        );
        let gpu_block_voxel_count = create_storage_u32(
            &device,
            &queue,
            "w1_block_voxel_count",
            &block_voxel_count_init,
        );
        let gpu_segment_voxel_buffer = create_storage_u32(
            &device,
            &queue,
            "w1_segment_voxel_buffer",
            &segment_voxels,
        );
        let gpu_hash_map = create_storage_u32(
            &device,
            &queue,
            "w1_hash_map",
            &hash_map_init,
        );
        let gpu_hash_coefficients = create_storage_u32(
            &device,
            &queue,
            "w1_hash_coefficients",
            &coeffs,
        );
        let params = GpuConstructionParams {
            size_in_chunks,
            _pad0: 0,
            group_size_in_groups: [1, 1, 1],
            _pad1: 0,
            bound_group_queue_max_size: 1,
            hash_map_size: hash_map_size_slots,
            segment_size_in_chunks,
            max_group_bound_dispatch: 0,
            chunk_offset: [0, 0, 0],
            dispatch_offset: 0,
            frame_index: 0,
            changed_chunk_count: 0,
            changed_block_count: 0,
            changed_voxel_count: 0,
        };
        let gpu_params =
            create_uniform(&device, &queue, "w1_construction_params", &params);

        // The chunks resource — `array<vec2<u32>>` storage buffer
        // (web-WebGPU migration; was `Rg32Uint` 3D texture). 8 B per pair;
        // `.x` carries the construction state (this test's load-bearing
        // channel), `.y` is the entity pointer (zero in this no-entities
        // test). STORAGE | COPY_DST | COPY_SRC — COPY_SRC for readback.
        let chunk_count_total =
            (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
        let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count_total];
        let chunks_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("w1_chunks"),
            size: (chunk_count_total as u64) * 8,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

        // === Queue the three pipelines ======================================
        let layout = construction_world_layout_descriptor();
        let (id_calc_block, id_voxel_bounds, id_block_bounds) = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let cache = render_app.world().resource::<PipelineCache>();
            let a = queue_calc_block_pipeline_with_handle(
                cache,
                layout.clone(),
                chunk_calc_handle.clone(),
            );
            let b = queue_voxel_bounds_pipeline_with_handle(
                cache,
                layout.clone(),
                chunk_calc_handle.clone(),
            );
            let c = queue_block_bounds_pipeline_with_handle(
                cache,
                layout.clone(),
                chunk_calc_handle.clone(),
            );
            (a, b, c)
        };

        let pipelines = compile_pipelines(
            &mut app,
            &[id_calc_block, id_voxel_bounds, id_block_bounds],
        )
        .expect("W1 pipelines did not compile in 64 ticks");

        // === Build the bind group ===========================================
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let bgl = cache.get_bind_group_layout(&layout);
        let bind_group = device.create_bind_group(
            "w1_construction_world_bind_group",
            &bgl,
            &BindGroupEntries::sequential((
                chunks_buffer.as_entire_buffer_binding(),
                gpu_blocks.as_entire_buffer_binding(),
                gpu_voxels.as_entire_buffer_binding(),
                gpu_block_voxel_count.as_entire_buffer_binding(),
                gpu_segment_voxel_buffer.as_entire_buffer_binding(),
                gpu_hash_map.as_entire_buffer_binding(),
                gpu_params.as_entire_buffer_binding(),
                gpu_hash_coefficients.as_entire_buffer_binding(),
            )),
        );

        // === Dispatch the 3 passes ==========================================
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_dispatch_calc_block"),
        });
        super::chunk_calc::dispatch_calc_block_from_raw_data(
            &mut encoder,
            &pipelines[0],
            &bind_group,
            segment_size_in_chunks,
        );
        queue.submit([encoder.finish()]);
        // Per `WorldData.cs:202-207`: NAADF dispatches `voxelCount / 64`
        // workgroups for `compute_voxel_bounds` (one per allocated block —
        // including the seed-reserved block at base 0) and `blockCount / 64`
        // workgroups for `compute_block_bounds` (one per allocated chunk,
        // including the seed-reserved chunk at base 0).
        //
        // `voxelCount` after `calc_block_from_raw_data` = 64 (seed) + 64
        // (one mixed block × 64 voxels) = 128 → 128/64 = 2 voxel workgroups.
        // `blockCount` after construction = 64 (seed) + 64 (one mixed chunk
        // × 64 blocks) = 128 → 128/64 = 2 block workgroups.
        //
        // The shader's `groupID.x` then indexes into the buffer at
        // `chunkIndex * 64`, so workgroup 0 hits the seed slots (zeros in/zeros
        // out — idempotent) and workgroup 1 hits the real chunk's 64 blocks.
        //
        // We READ BACK from `block_voxel_count` to get the actual GPU-side
        // cursors (matches `WorldData.cs:158` — `dataBlockGpu.GetData(blockVoxelCount)`).
        let cursor_pair =
            readback_u32(&device, &queue, &gpu_block_voxel_count, 2);
        let voxel_count = cursor_pair[0];
        let block_count = cursor_pair[1];
        let voxel_workgroups = voxel_count / 64;
        let block_workgroups = block_count / 64;
        eprintln!(
            "W1 GPU cursors after calc_block: voxelCount={} blockCount={}",
            voxel_count, block_count,
        );

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_dispatch_bounds"),
        });
        super::chunk_calc::dispatch_compute_voxel_bounds(
            &mut encoder,
            &pipelines[1],
            &bind_group,
            voxel_workgroups,
        );
        super::chunk_calc::dispatch_compute_block_bounds(
            &mut encoder,
            &pipelines[2],
            &bind_group,
            block_workgroups,
        );
        queue.submit([encoder.finish()]);

        // === Read back + compare ============================================
        // Read back the FULL allocated buffers (we need the [64..] window for
        // blocks and the [32..] window for voxels — GPU seeds the cursors at
        // 64 voxels = 32 u32 + 64 blocks).
        let gpu_blocks_out = readback_u32(
            &device,
            &queue,
            &gpu_blocks,
            (oracle.blocks.len().max(64) + 64) as u64,
        );
        let gpu_voxels_out = readback_u32(
            &device,
            &queue,
            &gpu_voxels,
            (oracle.voxels.len().max(32) + 32) as u64,
        );
        let gpu_chunks_out = readback_chunks_buffer(
            &device,
            &queue,
            &chunks_buffer,
            size_in_chunks,
        );

        // Chunks: byte-equal to the oracle. `ChunkCell::Mixed` payload
        // (`BlockPtr`) IS deterministic on a single-mixed-chunk test (the
        // GPU's `atomicAdd(&block_voxel_count[1], 64)` starts from the seed
        // 64, so the first mixed chunk gets `block_pointer = 64`; the CPU
        // oracle's `blocks_buf.len()` also starts at 0 and grows by 64 per
        // mixed chunk — `BlockPtr(0)` on CPU. To make the test
        // pointer-assignment-deterministic, we shift the CPU oracle output
        // by +64 in its `BlockPtr`s (the GPU's seed). This is a known port
        // deviation, documented in `16-impl-c-W1.md`.
        //
        // Easier: re-encode the oracle's `ChunkCell`s with the GPU's
        // base-pointer convention (BlockPtr offset by +64 from the CPU
        // oracle's offset).
        let expected_chunk: u32 = match ChunkCell::decode(oracle.chunks[0]) {
            ChunkCell::Empty(_) => oracle.chunks[0],
            ChunkCell::UniformFull(_) => oracle.chunks[0],
            ChunkCell::Mixed(ptr) => {
                ChunkCell::Mixed(crate::aadf::cell::BlockPtr(ptr.0 + 64)).encode()
            }
        };
        assert_eq!(
            gpu_chunks_out[0], expected_chunk,
            "GPU chunk[0] {:#010x} != CPU oracle (+64 BlockPtr offset) {:#010x}",
            gpu_chunks_out[0], expected_chunk
        );

        // Blocks: GPU lays them out at offset 64 (the seed); we compare the
        // GPU's [64..128] range to the CPU's [0..64]. With `BlockCell::Mixed`
        // entries the `VoxelPtr` also shifts by +64 (block 0's voxel pointer
        // = 64 on GPU, 0 on CPU). Re-encode the CPU blocks with the +64
        // VoxelPtr shift.
        let expected_blocks: Vec<u32> = oracle
            .blocks
            .iter()
            .map(|&raw| {
                match BlockCell::decode(raw) {
                    BlockCell::Empty(_) | BlockCell::UniformFull(_) => raw,
                    BlockCell::Mixed(VoxelPtr(v)) => {
                        // Halve the bias: voxels[] is in u32-element offsets;
                        // GPU seeds block_voxel_count[0] to 64 voxels = 32
                        // u32-pairs. So VoxelPtr on GPU side is +32 u32 from
                        // CPU's VoxelPtr (which is also a u32-element offset
                        // per `aadf/cell.rs:78-82`).
                        BlockCell::Mixed(VoxelPtr(v + 32)).encode()
                    }
                }
            })
            .collect();
        let gpu_blocks_slice = &gpu_blocks_out[64..64 + oracle.blocks.len()];
        // Find first mismatch for a helpful failure message.
        for (i, (&g, &c)) in gpu_blocks_slice
            .iter()
            .zip(expected_blocks.iter())
            .enumerate()
        {
            assert_eq!(
                g, c,
                "GPU blocks[{}] {:#010x} != expected (shifted oracle) {:#010x}",
                64 + i, g, c
            );
        }

        // Voxels: GPU stores them starting at u32-offset 32 (voxel_count
        // seed = 64 voxels / 2 = 32 u32s). Compare against the oracle's
        // [0..oracle.voxels.len()].
        let gpu_voxels_slice = &gpu_voxels_out[32..32 + oracle.voxels.len()];
        for (i, (&g, &c)) in gpu_voxels_slice
            .iter()
            .zip(oracle.voxels.iter())
            .enumerate()
        {
            assert_eq!(
                g, c,
                "GPU voxels[{}] {:#010x} != oracle {:#010x}",
                32 + i, g, c
            );
        }

        // Sanity references so the test trips on rename / removal.
        let _ = GpuHashValueSlot {
            voxel_pointer: 0,
            use_count: 0,
            hash_raw: 0,
            _pad: 0,
        };
        let _ = ty;
        // VoxelCell reference (used implicitly via decode in the oracle).
        let _vc = VoxelCell::Full(VoxelTypeId(1));

        // Total bytes compared (chunks + blocks + voxels — slice form).
        let bytes_compared = (oracle.chunks.len() * 4)
            + (oracle.blocks.len() * 4)
            + (oracle.voxels.len() * 4);
        eprintln!(
            "W1 GPU/CPU bit-exact: {} bytes compared (chunks {} + blocks {} + voxels {} u32s)",
            bytes_compared,
            oracle.chunks.len(),
            oracle.blocks.len(),
            oracle.voxels.len(),
        );
    }

    /// W1 — `map_copy.wgsl::copy_map` rehash correctness.
    ///
    /// Seeds an old map of size 32 with a few non-empty slots at deterministic
    /// positions, runs `copy_map` over it into a new map of size 64, reads
    /// back the new map, and asserts every old-map slot has a corresponding
    /// new-map entry at `hash & (new_size - 1)` (the linear-probe re-hash
    /// starting point).
    #[test]
    fn map_copy_regrow_preserves_contents() {
        let Some((mut app, device, queue, _chunk_calc_handle, map_copy_handle)) =
            render_fixture_w1()
        else {
            eprintln!("no wgpu device — skipping W1 map_copy test");
            return;
        };

        // Hand-built old map: 32 slots, 3 occupied. Each slot is 4 u32s
        // (voxel_pointer, use_count, hash_raw, _pad).
        let old_size: u32 = 32;
        let new_size: u32 = 64;
        let mut old_map_u32 = vec![0u32; (old_size as usize) * 4];
        // Slot 1: voxel_pointer = 100, use_count = 7, hash_raw = 0x1234.
        // Slot 5: voxel_pointer = 200, use_count = 3, hash_raw = 0xABCD.
        // Slot 20: voxel_pointer = 300, use_count = 1, hash_raw = 0xDEAD.
        let seeds: [(usize, u32, u32, u32); 3] = [
            (1, 100, 7, 0x1234),
            (5, 200, 3, 0xABCD),
            (20, 300, 1, 0xDEAD),
        ];
        for &(slot, vp, uc, hr) in &seeds {
            old_map_u32[slot * 4 + 0] = vp;
            old_map_u32[slot * 4 + 1] = uc;
            old_map_u32[slot * 4 + 2] = hr;
        }

        let gpu_old = create_storage_u32(&device, &queue, "w1_mc_old", &old_map_u32);
        let gpu_new = create_storage_u32(
            &device,
            &queue,
            "w1_mc_new",
            &vec![0u32; (new_size as usize) * 4],
        );
        let params = GpuMapCopyParams {
            old_size,
            new_size,
            _pad0: 0,
            _pad1: 0,
        };
        let gpu_params = create_uniform(&device, &queue, "w1_mc_params", &params);
        let gpu_coeffs = create_storage_u32(&device, &queue, "w1_mc_coeffs", &[0u32; 1]);
        let gpu_v2h = create_storage_u32(&device, &queue, "w1_mc_v2h", &[0u32; 1]);
        let gpu_result = create_storage_u32(&device, &queue, "w1_mc_result", &[0u32; 1]);

        let layout = map_copy_layout_descriptor();
        let id_copy = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let cache = render_app.world().resource::<PipelineCache>();
            queue_copy_map_pipeline_with_handle(cache, layout.clone(), map_copy_handle.clone())
        };
        let pipelines = compile_pipelines(&mut app, &[id_copy])
            .expect("map_copy pipeline did not compile in 64 ticks");

        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let bgl = cache.get_bind_group_layout(&layout);
        let bind_group = device.create_bind_group(
            "w1_mc_bind_group",
            &bgl,
            &BindGroupEntries::sequential((
                gpu_old.as_entire_buffer_binding(),
                gpu_new.as_entire_buffer_binding(),
                gpu_params.as_entire_buffer_binding(),
                gpu_coeffs.as_entire_buffer_binding(),
                gpu_v2h.as_entire_buffer_binding(),
                gpu_result.as_entire_buffer_binding(),
            )),
        );

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_mc_dispatch"),
        });
        super::map_copy::dispatch_copy_map(&mut encoder, &pipelines[0], &bind_group, old_size);
        queue.submit([encoder.finish()]);

        let new_u32 = readback_u32(
            &device,
            &queue,
            &gpu_new,
            (new_size as u64) * 4,
        );

        // Verify each seed landed in the new map at hash_raw & (new_size-1)
        // OR a subsequent probe slot.
        for &(_slot, vp, uc, hr) in &seeds {
            let start = (hr & (new_size - 1)) as usize;
            let mut found = false;
            for probe in 0..50u32 {
                let candidate = ((start as u32 + probe) & (new_size - 1)) as usize;
                if new_u32[candidate * 4 + 0] == vp {
                    assert_eq!(new_u32[candidate * 4 + 1], uc, "use_count mismatch");
                    assert_eq!(new_u32[candidate * 4 + 2], hr, "hash_raw mismatch");
                    found = true;
                    break;
                }
            }
            assert!(found, "slot with vp={vp} hr={hr:#x} not found in new map");
        }
    }

    /// Phase-C followup #1 — verify the runtime GPU producer flip is active by
    /// default + the runtime-flip's `build_segment_voxel_buffer_from_dense`
    /// helper is byte-equivalent to the W1-test-validated
    /// `build_segment_voxel_buffer(&volume, …)` helper, AND the full GPU
    /// dispatch chain against this runtime-built segment buffer byte-matches
    /// the CPU oracle.
    ///
    /// This is the load-bearing "runtime-flip verification" test the brief
    /// asked for. We can't easily boot the full e2e `App` in a unit test
    /// (it needs a window + the asset loader), so we exercise the same
    /// dispatch chain `prepare_construction` runs at runtime, but driven by
    /// the test harness — proving:
    ///
    /// 1. `ConstructionConfig::default().gpu_construction_enabled` is `true`
    ///    (the runtime flip is on by default).
    /// 2. `build_segment_voxel_buffer_from_dense` (used by the runtime path)
    ///    produces the same `segment_voxel_buffer` as
    ///    `build_segment_voxel_buffer` (used by `validate_gpu_construction`
    ///    + the existing W1 oracle test).
    /// 3. `validate_gpu_construction()` succeeds — the W1 chain produces
    ///    Algorithm 1 output byte-equal to the CPU oracle. (This is the
    ///    same gate `e2e_render --validate-gpu-construction` runs at the
    ///    e2e harness level.)
    #[test]
    fn runtime_gpu_producer_runs_and_matches_cpu_oracle_in_default_mode() {
        // (1) Runtime flip is the default.
        let cfg = crate::render::construction::config::ConstructionConfig::default();
        assert!(
            cfg.gpu_construction_enabled,
            "Phase-C followup #1: `gpu_construction_enabled` MUST default to true so the \
             runtime GPU producer is active out-of-the-box"
        );

        // (2) The runtime-path `build_segment_voxel_buffer_from_dense` helper
        // is byte-equivalent to the validated `build_segment_voxel_buffer`.
        // Build the same single-voxel test volume the W1 oracle uses, then
        // compare both segment-buffer builders.
        let mut volume = crate::aadf::construct::DenseVolume::empty([1, 1, 1]);
        volume.set([0, 0, 0], crate::voxel::VoxelTypeId(7));
        let dense_u16: Vec<u16> = volume.voxels.iter().map(|t| t.0).collect();
        let runtime_buf =
            crate::render::construction::build_segment_voxel_buffer_from_dense(
                &dense_u16,
                volume.size_in_chunks,
                volume.size_in_chunks,
            );
        let validated_buf =
            crate::render::construction::build_segment_voxel_buffer(&volume, 1);
        assert_eq!(
            runtime_buf, validated_buf,
            "Phase-C followup #1: `build_segment_voxel_buffer_from_dense` must produce \
             byte-identical output to the W1-test-validated \
             `build_segment_voxel_buffer(&volume, segment_size)` helper (the validation \
             gate at `e2e_render --validate-gpu-construction` proves the latter is \
             byte-correct to the CPU oracle; this assertion chains both proofs together \
             so the runtime path inherits the same correctness)."
        );

        // (3) The full GPU dispatch chain — driven by the same shader entry
        // points the runtime path uses — produces output byte-equal to the
        // CPU oracle on the deterministic 1×1×1 test scene.
        match crate::render::construction::validate_gpu_construction() {
            Ok(bytes) => {
                assert!(
                    bytes >= 12,
                    "Phase-C followup #1: validate_gpu_construction compared only \
                     {bytes} bytes (expected ≥12: 4 chunks + 4 blocks + 4 voxels at \
                     the 1×1×1 fixture)"
                );
            }
            Err(msg)
                if msg.contains("no wgpu")
                    || msg.contains("no RenderApp")
                    || msg.contains("no RenderDevice")
                    || msg.contains("no RenderQueue") =>
            {
                // No GPU available in CI — the W1 oracle test
                // `gpu_algorithm1_vs_cpu_bit_exact` will have skipped too;
                // this test stays consistent with that policy.
                eprintln!(
                    "no wgpu device — skipping the GPU dispatch leg of the runtime-flip \
                     verification (the config-flip + helper-equivalence legs above \
                     still ran and passed)"
                );
            }
            Err(msg) => panic!(
                "Phase-C followup #1: validate_gpu_construction failed with: {msg}"
            ),
        }
    }
}

#[cfg(test)]
mod tests_w4 {
    //! W4 — GPU pipeline-compilation smoke + the load-bearing
    //! `entity_update_gpu_vs_cpu` shape gate.
    //!
    //! - `entity_update_pipelines_compile`: builds a headless render world,
    //!   queues all three `entity_update.wgsl` pipelines + the two W4
    //!   layouts, and asserts they all compile cleanly. Pure WGSL/layout
    //!   sanity — does not run the dispatches.
    //! - `chunks_format_widening_regression`: builds a small app, exercises
    //!   the chunks-format widening path through the existing
    //!   `prepare_world_gpu` indirectly (the existing 76 tests verify the
    //!   format-flip path in isolation).
    //! - `entity_update_gpu_vs_cpu`: dispatches the three entry points
    //!   against a small fixture, reads back the GPU-written
    //!   `entity_chunk_instances` + `entity_instances_history` buffers, and
    //!   asserts they match the CPU `EntityHandler::update` output
    //!   byte-for-byte.

    use super::entity_handler::EntityHandler;
    use super::entity_update::{
        construction_entity_layout_descriptor, dispatch_copy_entity_chunk_instances,
        dispatch_copy_entity_history, dispatch_update_chunks, entity_world_layout_descriptor,
        queue_copy_entity_chunk_instances_pipeline_with_handle,
        queue_copy_entity_history_pipeline_with_handle, queue_update_chunks_pipeline_with_handle,
        GpuEntityUpdateParams, ENTITY_UPDATE_SHADER_SRC,
    };
    use crate::render::gpu_types::{
        EntityInstance, GpuChunkUpdate, GpuEntityChunkInstance, GpuEntityInstanceHistory,
    };

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::math::Vec3;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::shader::Shader;
    use bevy::MinimalPlugins;

    fn boot_render_app() -> Option<(App, RenderDevice, RenderQueue, bevy::asset::Handle<Shader>)> {
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

        let shader = Shader::from_wgsl(ENTITY_UPDATE_SHADER_SRC, "shaders/entity_update.wgsl");
        let shader_clone = shader.clone();
        let shader_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(shader);
        let render_app = app.get_sub_app_mut(RenderApp)?;
        {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.set_shader(shader_handle.id(), shader_clone);
        }
        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((app, device, queue, shader_handle))
    }

    /// All three entity_update pipelines compile cleanly against the W4
    /// layouts. Pure layout/shader sanity.
    #[test]
    fn entity_update_pipelines_compile() {
        let Some((mut app, _device, _queue, shader_handle)) = boot_render_app() else {
            eprintln!("no wgpu device — skipping entity_update_pipelines_compile");
            return;
        };
        let world_layout = entity_world_layout_descriptor();
        let entity_layout = construction_entity_layout_descriptor();

        let (id_a, id_b, id_c) = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let cache = render_app.world().resource::<PipelineCache>();
            let a = queue_update_chunks_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            let b = queue_copy_entity_chunk_instances_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            let c = queue_copy_entity_history_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            (a, b, c)
        };

        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        let mut compiled = false;
        for _ in 0..64 {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.process_queue();
            let cache = render_app.world().resource::<PipelineCache>();
            if cache.get_compute_pipeline(id_a).is_some()
                && cache.get_compute_pipeline(id_b).is_some()
                && cache.get_compute_pipeline(id_c).is_some()
            {
                compiled = true;
                break;
            }
        }
        assert!(
            compiled,
            "W4 entity_update pipelines did not compile in 64 ticks"
        );
    }

    /// Load-bearing W4 gate: the GPU `entity_update.wgsl` output is
    /// byte-for-byte equal to the CPU `EntityHandler::update` output on a
    /// small deterministic fixture. Exercises all three entry points
    /// (update_chunks, copy_entity_chunk_instances, copy_entity_history).
    #[test]
    fn entity_update_gpu_vs_cpu() {
        let Some((mut app, device, queue, shader_handle)) = boot_render_app() else {
            eprintln!("no wgpu device — skipping entity_update_gpu_vs_cpu");
            return;
        };

        // CPU fixture — one entity in a 2×1×1-chunk world.
        let size_in_chunks = [2u32, 1, 1];
        let mut handler = EntityHandler::new(size_in_chunks);
        let instances = vec![EntityInstance {
            position: Vec3::new(12.0, 8.0, 8.0),
            quaternion: [0.0, 0.0, 0.0, 1.0],
            voxel_start: 0,
            entity: 0,
            size: [8, 4, 4],
        }];
        let cpu_uploads = handler.update(&instances);
        let update_count = cpu_uploads.chunk_updates.len() as u32;
        let chunk_instance_count = cpu_uploads.entity_chunk_instances.len() as u32;
        let instance_count = cpu_uploads.entity_history.len() as u32;

        // GPU side — allocate the chunks buffer + the two dynamic upload
        // buffers + the two output buffers. Web-WebGPU migration: chunks is
        // an `array<vec2<u32>>` storage buffer (was `Rg32Uint` 3D texture).
        let chunk_count =
            (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
        // Pre-write a non-zero `.x` channel so we can verify the entity
        // update preserves `.x`.
        let init_chunks: Vec<[u32; 2]> = (0..chunk_count)
            .map(|i| [0xAA00_0000u32 + i as u32, 0u32])
            .collect();
        let chunks_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("w4_chunks"),
            size: (chunk_count as u64) * 8,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&init_chunks));

        let mk_storage = |label: &'static str, bytes: &[u8]| {
            let buf = device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size: bytes.len().max(8) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if !bytes.is_empty() {
                queue.write_buffer(&buf, 0, bytes);
            } else {
                queue.write_buffer(&buf, 0, &[0u8; 8]);
            }
            buf
        };

        let chunk_updates_buf = mk_storage(
            "w4_chunk_updates_dynamic",
            bytemuck::cast_slice(&cpu_uploads.chunk_updates),
        );
        let entity_ci_dyn = mk_storage(
            "w4_entity_chunk_instances_dynamic",
            bytemuck::cast_slice(&cpu_uploads.entity_chunk_instances),
        );
        let entity_history_dyn = mk_storage(
            "w4_entity_history_dynamic",
            bytemuck::cast_slice(&cpu_uploads.entity_history),
        );
        // Output buffers — over-allocated.
        let entity_ci_rw_count = chunk_instance_count.max(1) as usize;
        let entity_ci_rw_zero =
            vec![GpuEntityChunkInstance::default(); entity_ci_rw_count];
        let entity_ci_rw = mk_storage(
            "w4_entity_chunk_instances_rw",
            bytemuck::cast_slice(&entity_ci_rw_zero),
        );
        // History ring sized `taa_index * max + entityInstanceID` cap; we use
        // taa_index = 0 so we only need `instance_count` entries.
        let history_rw_count = instance_count.max(1) as usize;
        let history_rw_zero = vec![GpuEntityInstanceHistory::default(); history_rw_count];
        let entity_history_rw = mk_storage(
            "w4_entity_history_rw",
            bytemuck::cast_slice(&history_rw_zero),
        );

        let params = GpuEntityUpdateParams {
            entity_instance_count: instance_count,
            entity_chunk_instance_count: chunk_instance_count,
            taa_index: 0,
            update_count,
            max_entity_instances: 64,
            _pad0: 0,
            _pad1: 0,
            dispatch_offset: 0,
            // Web-WebGPU migration: chunks is `array<vec2<u32>>`; the kernel
            // flattens chunk_pos with `size_in_chunks` as the stride basis.
            size_in_chunks,
            _pad3: 0,
        };
        let params_buf = device.create_buffer(&BufferDescriptor {
            label: Some("w4_params"),
            size: std::mem::size_of::<GpuEntityUpdateParams>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&params_buf, 0, bytemuck::bytes_of(&params));

        // Queue + compile pipelines.
        let world_layout = entity_world_layout_descriptor();
        let entity_layout = construction_entity_layout_descriptor();
        let (id_a, id_b, id_c) = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let cache = render_app.world().resource::<PipelineCache>();
            let a = queue_update_chunks_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            let b = queue_copy_entity_chunk_instances_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            let c = queue_copy_entity_history_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            (a, b, c)
        };
        let mut pipelines: Option<Vec<bevy::render::render_resource::ComputePipeline>> =
            None;
        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        for _ in 0..64 {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.process_queue();
            let cache = render_app.world().resource::<PipelineCache>();
            if let (Some(a), Some(b), Some(c)) = (
                cache.get_compute_pipeline(id_a),
                cache.get_compute_pipeline(id_b),
                cache.get_compute_pipeline(id_c),
            ) {
                pipelines = Some(vec![a.clone(), b.clone(), c.clone()]);
                break;
            }
        }
        let pipelines = pipelines.expect("entity_update pipelines did not compile");

        // Build bind groups.
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let world_bgl = cache.get_bind_group_layout(&world_layout);
        let entity_bgl = cache.get_bind_group_layout(&entity_layout);
        let world_bg = device.create_bind_group(
            "w4_world_bg",
            &world_bgl,
            &BindGroupEntries::sequential((
                chunks_buffer.as_entire_buffer_binding(),
                params_buf.as_entire_buffer_binding(),
            )),
        );
        let entity_bg = device.create_bind_group(
            "w4_entity_bg",
            &entity_bgl,
            &BindGroupEntries::sequential((
                chunk_updates_buf.as_entire_buffer_binding(),
                entity_ci_dyn.as_entire_buffer_binding(),
                entity_history_dyn.as_entire_buffer_binding(),
                entity_ci_rw.as_entire_buffer_binding(),
                entity_history_rw.as_entire_buffer_binding(),
            )),
        );

        // Dispatch.
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w4_dispatch"),
        });
        dispatch_update_chunks(
            &mut encoder,
            &pipelines[0],
            &world_bg,
            &entity_bg,
            update_count,
        );
        dispatch_copy_entity_chunk_instances(
            &mut encoder,
            &pipelines[1],
            &world_bg,
            &entity_bg,
            chunk_instance_count,
        );
        dispatch_copy_entity_history(
            &mut encoder,
            &pipelines[2],
            &world_bg,
            &entity_bg,
            instance_count,
        );
        queue.submit([encoder.finish()]);

        // Readback `entity_chunk_instances_rw` and compare to CPU.
        let read_bytes = |src: &bevy::render::render_resource::Buffer, n: u64| {
            let staging = device.create_buffer(&BufferDescriptor {
                label: Some("w4_readback_staging"),
                size: n,
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("w4_readback_enc"),
            });
            enc.copy_buffer_to_buffer(src, 0, &staging, 0, n);
            queue.submit([enc.finish()]);
            let slice = staging.slice(..);
            slice.map_async(MapMode::Read, |r| r.unwrap());
            device.poll(PollType::wait_indefinitely()).unwrap();
            let data = slice.get_mapped_range();
            let v: Vec<u8> = data.to_vec();
            drop(data);
            staging.unmap();
            v
        };
        let ci_bytes = read_bytes(
            &entity_ci_rw,
            (entity_ci_rw_count * std::mem::size_of::<GpuEntityChunkInstance>()) as u64,
        );
        let gpu_ci: &[GpuEntityChunkInstance] = bytemuck::cast_slice(&ci_bytes);
        for i in 0..(chunk_instance_count as usize) {
            assert_eq!(
                gpu_ci[i].data1, cpu_uploads.entity_chunk_instances[i].data1,
                "entity_chunk_instances[{i}].data1 mismatch"
            );
            assert_eq!(
                gpu_ci[i].data5, cpu_uploads.entity_chunk_instances[i].data5,
                "entity_chunk_instances[{i}].data5 mismatch"
            );
        }

        let hist_bytes = read_bytes(
            &entity_history_rw,
            (history_rw_count * std::mem::size_of::<GpuEntityInstanceHistory>()) as u64,
        );
        let gpu_hist: &[GpuEntityInstanceHistory] = bytemuck::cast_slice(&hist_bytes);
        for i in 0..(instance_count as usize) {
            assert_eq!(
                gpu_hist[i].data1, cpu_uploads.entity_history[i].data1,
                "entity_history[{i}].data1 mismatch"
            );
            assert_eq!(
                gpu_hist[i].data4, cpu_uploads.entity_history[i].data4,
                "entity_history[{i}].data4 mismatch"
            );
        }

        // Verify the chunks buffer: `.x` channel preserved, `.y` channel
        // got the entity pointer (= update.data2). Web-WebGPU migration:
        // chunks is `array<vec2<u32>>`; flat buffer→buffer copy.
        let staging_size = (chunk_count as u64) * 8;
        let chunks_staging = device.create_buffer(&BufferDescriptor {
            label: Some("w4_chunks_staging"),
            size: staging_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w4_chunks_readback"),
        });
        enc.copy_buffer_to_buffer(&chunks_buffer, 0, &chunks_staging, 0, staging_size);
        queue.submit([enc.finish()]);
        let slice = chunks_staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let raw = slice.get_mapped_range();
        let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
        // For each update, decode chunk_pos + verify `.x` preserved + `.y`
        // updated.
        for upd in &cpu_uploads.chunk_updates {
            let cx = upd.data1 & 0x7FF;
            let cy = (upd.data1 >> 11) & 0x3FF;
            let cz = upd.data1 >> 21;
            let chunk_idx_in_world =
                cx + cy * size_in_chunks[0] + cz * size_in_chunks[0] * size_in_chunks[1];
            let xy = pairs[chunk_idx_in_world as usize];
            let preserved_x = 0xAA00_0000u32 + chunk_idx_in_world;
            assert_eq!(
                xy[0], preserved_x,
                "chunks[{}].x not preserved: got {:#x} expected {:#x}",
                chunk_idx_in_world, xy[0], preserved_x
            );
            assert_eq!(
                xy[1], upd.data2,
                "chunks[{}].y not written: got {:#x} expected {:#x}",
                chunk_idx_in_world, xy[1], upd.data2
            );
        }
        drop(raw);
        chunks_staging.unmap();

        // Silence the unused-warn on GpuChunkUpdate.
        let _ = GpuChunkUpdate::default();
    }
}
