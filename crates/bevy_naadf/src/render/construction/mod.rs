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
pub mod extract;
pub mod generator_model;
pub mod hashing;
pub mod map_copy;
pub mod producer;
pub mod readback;
pub mod shader_drift_guard;
pub mod test_fixture;
pub mod validation;
pub mod world_change;

use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, Buffer, BufferDescriptor, BufferUsages,
    PipelineCache,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::render::pipelines::NaadfPipelines;

pub use chunk_calc::build_segment_voxel_buffer_from_dense;
pub use config::ConstructionConfig;
pub use extract::{extract_world_changes, MainWorldEntities, RenderWorldEntityState};
pub use producer::naadf_gpu_producer_node;
pub use readback::{
    populate_cpu_mirror_from_gpu_producer, CpuMirrorReadback, ReadbackStage,
    READBACK_STALL_BUDGET_FRAMES,
};
pub use validation::{
    validate_edit_mode, validate_entity_handler, validate_gpu_construction,
    validate_gpu_construction_production_scale, validate_gpu_construction_scaled,
    validate_runtime_edit_mode,
};

/// The render-world `Resource` holding every Phase-C buffer family
/// (`15-design-c.md` §1.4).
///
/// **W0 — empty shell.** Every buffer field is `Option<Buffer>` initialised
/// to `None`. Each workstream populates its own family:
///
/// - **W1** (Algorithm 1): `segment_voxel_buffer`, `block_voxel_count`,
///   `hash_map`, `hash_coefficients`.
/// - **W3** (background AADF queue): `bound_queue_starts` + `bound_queue_sizes`, `bound_group_queues`,
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
    /// `boundQueueInfo.start` (`WorldBoundHandler.cs:44`) — 32*3 × u32 of the
    /// per-queue start cursor. 2026-05-19 wasm-chunk-aadf-determinism fix:
    /// the C# `RWStructuredBuffer<BoundQueueInfo>` packed `(start, size)`
    /// struct was split into two top-level flat buffers so Tint emits the
    /// proven-working `array<atomic<u32>>` lowering for the cross-pass
    /// atomic `size` field on Dawn/WebGPU. Algorithm unchanged; user-approved
    /// faithful-port divergence (layout-only, same class as the existing
    /// chunks-buffer split). Fixed-size. W3.
    pub bound_queue_starts: Option<Buffer>,
    /// `boundQueueInfo.size` — 32*3 × atomic<u32> of the per-queue element
    /// count. 2026-05-19 wasm-chunk-aadf-determinism split — see
    /// `bound_queue_starts` above. Fixed-size. W3.
    pub bound_queue_sizes: Option<Buffer>,
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
    /// Read-only mirror of `chunks` used by `compute_group_bounds` for ALL
    /// chunk-AADF reads (own at `bounds_calc.wgsl:523`, neighbour at `:273`).
    /// Refreshed via `copy_buffer_to_buffer(chunks, chunks_mirror, full_size)`
    /// once before round 0 (W5-seed) and between every subsequent round.
    /// Load-bearing on both targets — see `bounds_calc.rs::naadf_bounds_compute_node`
    /// docblock for the full mechanism. Same size as chunks_buffer
    /// (`array<vec2<u32>>`, stride 8 B).
    pub chunks_mirror_buffer: Option<Buffer>,
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
    pipelines: Option<Res<NaadfPipelines>>,
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
    let Some(pipelines) = pipelines else { return; };

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
        let needs_realloc = gpu
            .block_voxel_count
            .as_ref()
            .map(|b| b.size())
            .unwrap_or(0)
            < 8;
        if needs_realloc {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_block_voxel_count_gpu_producer"),
                size: 8,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            render_queue.write_buffer(&buf, 0, bytemuck::cast_slice(&[64u32, 64u32]));
            gpu.block_voxel_count = Some(buf);
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
            bind_groups.construction_world = None;
        }
        let _ = world_chunk_count; // referenced for future segment-iteration sizing.
    }

    // === W3 — bound-queue family + bind groups ===============================
    //
    // Fixed-size allocation per `WorldBoundHandler.cs:44-47`:
    //   - bound_queue_starts: 32 × 3 × u32 — 384 B. (2026-05-19 web fix
    //     split from `boundQueueInfo`'s packed `(start, size)` struct.)
    //   - bound_queue_sizes:  32 × 3 × atomic<u32> — 384 B. (other half of
    //     the split; declared `array<atomic<u32>>` on the WGSL side to
    //     restore cross-pass atomic visibility on Dawn/WebGPU.)
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

    if gpu.bound_queue_starts.is_none() {
        // wgpu rejects zero-size buffers; clamp every size to ≥1 element.
        let bgc = bound_group_count.max(1) as u64;
        // 2026-05-19 wasm-chunk-aadf-determinism fix: the C# `BoundQueueInfo
        // { start: u32, size: u32 }` packed struct is split into two top-
        // level flat buffers (`bound_queue_starts` + `bound_queue_sizes`)
        // so Tint emits the proven-working `array<atomic<u32>>` lowering
        // on Dawn/WebGPU for the cross-pass-atomic `size` field. Each
        // buffer holds 32 (sizes) × 3 (axes) = 96 × u32 = 384 B.
        const BOUND_QUEUE_BUFFER_BYTES: u64 = 32 * 3 * 4;
        let starts_buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_bound_queue_starts"),
            size: BOUND_QUEUE_BUFFER_BYTES,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let sizes_buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_bound_queue_sizes"),
            size: BOUND_QUEUE_BUFFER_BYTES,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // Seed: `boundQueueInfoNew[i*3+xyz] = {start: 0, size: i == 0 ? boundGroupCount : 0}`
        // — `WorldBoundHandler.cs:55-64`. The size-0 X/Y/Z queues hold every
        // group at startup; all higher bound sizes start empty. The
        // `GpuBoundQueueInfo` struct is still used for CPU-side seed
        // construction (it's just a (start, size) pair); we split into the
        // two flat arrays here at upload time.
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
        let starts_seed: Vec<u32> =
            info_seed.iter().map(|s| s.start).collect();
        let sizes_seed: Vec<u32> = info_seed.iter().map(|s| s.size).collect();
        render_queue.write_buffer(&starts_buf, 0, bytemuck::cast_slice(&starts_seed));
        render_queue.write_buffer(&sizes_buf, 0, bytemuck::cast_slice(&sizes_seed));

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

        gpu.bound_queue_starts = Some(starts_buf);
        gpu.bound_queue_sizes = Some(sizes_buf);
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

    // 2026-05-20 brute-force iter-2 (HP) — allocate `chunks_mirror_buffer`
    // (same size as `chunks_buffer`). Read-only mirror that W3's
    // `compute_group_bounds` reads from for own + neighbour AADF.
    // copy_buffer_to_buffer(chunks, chunks_mirror) is dispatched between
    // each W3 round in `naadf_bounds_compute_node` on wasm; native uses
    // chunks-rw directly and writes to chunks_mirror once at startup so
    // the binding has SOMETHING valid.
    if gpu.chunks_mirror_buffer.is_none() {
        let chunks_size = world_gpu.chunks_buffer.size();
        let buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_chunks_mirror_w3_brute_force_iter2"),
            size: chunks_size,
            usage: BufferUsages::STORAGE
                | BufferUsages::COPY_DST
                | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        gpu.chunks_mirror_buffer = Some(buf);
        bind_groups.construction_bounds_world = None;
        bevy::log::info!(
            "[aadf-probe] brute-force iter-2 HP: allocated chunks_mirror_buffer size={} B",
            chunks_size,
        );
    }

    let _ = chunk_count; // referenced for future regime-3 sizing.

    // === Build W3 bind groups when missing ===================================
    if bind_groups.construction_bounds_world.is_none() {
        if let (Some(params_buf), Some(chunks_mirror_buf)) = (
            gpu.bounds_params_buffer.as_ref(),
            gpu.chunks_mirror_buffer.as_ref(),
        ) {
            let bgl = pipeline_cache
                .get_bind_group_layout(&pipelines.construction_bounds_world_layout);
            let bg = render_device.create_bind_group(
                "naadf_construction_bounds_world_bind_group",
                &bgl,
                &BindGroupEntries::sequential((
                    world_gpu.chunks_buffer.as_entire_buffer_binding(),
                    params_buf.as_entire_buffer_binding(),
                    chunks_mirror_buf.as_entire_buffer_binding(),
                )),
            );
            bind_groups.construction_bounds_world = Some(bg);
        }
    }
    if bind_groups.construction_bounds.is_none() {
        if let (Some(starts), Some(queues), Some(masks), Some(refined), Some(sizes)) = (
            gpu.bound_queue_starts.as_ref(),
            gpu.bound_group_queues.as_ref(),
            gpu.bound_group_masks.as_ref(),
            gpu.bound_refined_info.as_ref(),
            gpu.bound_queue_sizes.as_ref(),
        ) {
            let bgl = pipeline_cache
                .get_bind_group_layout(&pipelines.construction_bounds_layout);
            // 2026-05-19 web fix — 5 bindings: `bound_queue_starts` (0) /
            // `bound_group_queues` (1) / `bound_group_masks` (2) /
            // `bound_refined_info` (3) / `bound_queue_sizes` (4).
            let bg = render_device.create_bind_group(
                "naadf_construction_bounds_bind_group",
                &bgl,
                &BindGroupEntries::sequential((
                    starts.as_entire_buffer_binding(),
                    queues.as_entire_buffer_binding(),
                    masks.as_entire_buffer_binding(),
                    refined.as_entire_buffer_binding(),
                    sizes.as_entire_buffer_binding(),
                )),
            );
            bind_groups.construction_bounds = Some(bg);
        }
    }
    if bind_groups.bound_dispatch.is_none() {
        if let Some(indirect) = gpu.bound_dispatch_indirect.as_ref() {
            let bgl = pipeline_cache
                .get_bind_group_layout(&pipelines.bound_dispatch_indirect_layout);
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
    //   - `pipelines.generator_model_layout` (always present per
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
                    &pipelines.generator_model_layout,
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
            .get_compute_pipeline(pipelines.bounds_calc_pipeline_add_initial)
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
                .get_bind_group_layout(&pipelines.construction_change_layout);
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
        }
        if gpu.segment_voxel_buffer.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_segment_voxel_buffer_w2_placeholder"),
                size: 4,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.segment_voxel_buffer = Some(buf);
        }
        if gpu.hash_map.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_hash_map_w2_placeholder"),
                size: 16, // one HashValueSlot
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.hash_map = Some(buf);
        }
        if gpu.hash_coefficients.is_none() {
            let buf = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_hash_coefficients_w2_placeholder"),
                size: 4,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.hash_coefficients = Some(buf);
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
                .get_bind_group_layout(&pipelines.construction_world_layout);
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
                    &pipelines.construction_entity_layout,
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
        // `gpu.world_bind_group_has_entities`. D4 owns the layout shape; the
        // named `rebuild_world_bind_group_with_entities` helper lives in
        // `render/prepare.rs` so this site is purely a *caller* — the cross-
        // domain seam is one function name (Resolution D follow-up Step 5).
        if !gpu.world_bind_group_has_entities {
            if let (Some(eci_rw), Some(evd), Some(eih_rw)) = (
                gpu.entity_chunk_instances.as_ref(),
                gpu.entity_voxel_data.as_ref(),
                gpu.entity_instances_history.as_ref(),
            ) {
                let rebuilt = crate::render::prepare::rebuild_world_bind_group_with_entities(
                    &render_device,
                    &pipeline_cache,
                    &pipelines,
                    &world_gpu,
                    eci_rw,
                    evd,
                    eih_rw,
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

        // Phase-C wave-3 — `--entities` fixture spawner. Self-gates on
        // `AppArgs::spawn_test_entity` so registration is unconditional; the
        // scheduler skips the system body when the flag is `false`. Runs
        // after `voxel::grid::setup_test_grid` so the world dimensions are
        // known before the fixture computes its demo-relative position.
        app.add_systems(
            Startup,
            test_fixture::spawn_phase_c_test_entity
                .after(crate::voxel::grid::setup_test_grid)
                .run_if(|args: Res<crate::AppArgs>| args.spawn_test_entity),
        );

        // Main-world `Startup` driver (regime-1, `15-design-c.md` §1.2). W0
        // body is the gated no-op above; W1 fills it.
        app.add_systems(Startup, run_gpu_construction_startup);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            // Mirror the main-world construction config into the render sub-app.
            .insert_resource(construction_config)
            // Construction pipelines now live on `NaadfPipelines` (Resolution D
            // — W0 seam retired). `NaadfRenderPlugin::build` registers
            // `NaadfPipelines` once via `init_gpu_resource`; no second-register
            // needed here.
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
                (
                    extract_world_changes,
                    populate_cpu_mirror_from_gpu_producer
                        .run_if(bevy::ecs::schedule::common_conditions::resource_exists::<
                            ConstructionGpu,
                        >)
                        .run_if(bevy::ecs::schedule::common_conditions::resource_exists::<
                            crate::render::prepare::WorldGpu,
                        >),
                ),
            );
    }
}

// `build_segment_voxel_buffer_from_dense` moved to `chunk_calc.rs` (Step 7 —
// canonical home with the rest of the W1 encode/dispatch chain). Re-exported
// at the top of this file so production callers
// (`bevy_naadf::render::construction::build_segment_voxel_buffer_from_dense`)
// resolve unchanged.

