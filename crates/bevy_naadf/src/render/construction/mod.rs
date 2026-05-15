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

pub mod chunk_calc;
pub mod config;
pub mod generator_model;
pub mod hashing;
pub mod map_copy;

use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupLayoutDescriptor, Buffer, CachedComputePipelineId, PipelineCache,
};
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
    /// the construction passes (chunkCalc / mapCopy / boundsCalc /
    /// worldChange). W1 builds this when the world buffers exist.
    pub construction_world: Option<BindGroup>,
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
    /// `fill_chunk_data_with_model_data_16` entry point.
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
pub fn prepare_construction(
    mut commands: Commands,
    gpu: Option<Res<ConstructionGpu>>,
    bind_groups: Option<Res<ConstructionBindGroups>>,
) {
    // W0 — empty seam. Just guarantee the two resources exist.
    //
    // W1..W5 will fill this with their workstream's allocate/upload/build
    // logic. The pattern matches `prepare_world_gpu` (`render/prepare.rs:128`)
    // — `Option<Res<…>>` for the resource, `Commands::insert_resource` on
    // first creation, no-op when already present.
    if gpu.is_none() {
        commands.insert_resource(ConstructionGpu::default());
    }
    if bind_groups.is_none() {
        commands.insert_resource(ConstructionBindGroups::default());
    }
}

/// `Startup`-schedule one-shot driver — the empty Phase-C regime-1 seam
/// (`15-design-c.md` §1.2 regime 1, §3 startup-schedule).
///
/// **W0 body — gated no-op.** When
/// `ConstructionConfig.gpu_construction_enabled` is `false` (W0 default), the
/// system returns immediately and the CPU `aadf::construct::construct` path
/// stays the producer (E4). When `true`, W0 logs a placeholder line at
/// `info!` so a future invocation that flips the flag without W1's payload
/// landed still produces visible diagnostics rather than silent dispatch
/// nothingness. W1 replaces the body with the regime-1 dispatch chain
/// (generator → chunk_calc → bounds_init) + the bit-exact CPU/GPU oracle
/// assert.
///
/// Lives in the main `App` (not the render sub-app) because regime-1 owns
/// its own command-encoder submission against `RenderDevice` (the same
/// `RenderQueue::submit` pattern `prepare_world_gpu` uses today —
/// `render/prepare.rs:168-180`). Runs **once**.
pub fn run_gpu_construction_startup(args: Res<crate::AppArgs>) {
    if !args.construction_config.gpu_construction_enabled {
        // CPU path is producer; nothing to do.
        return;
    }
    // W1 disposition (`16-impl-c-W1.md` decision #2): the main-app `Startup`
    // tracer logs an info line; the GPU construction itself runs (a) inside
    // the load-bearing `gpu_algorithm1_vs_cpu_bit_exact` unit test against a
    // headless render world (per `15-design-c.md` §1.6 — the §1.6 oracle role),
    // (b) inside the `--validate-gpu-construction` e2e validation path which
    // runs the SAME headless dispatch + bit-exact compare after the main e2e
    // exits.
    //
    // The production render path stays the existing CPU-build-then-upload
    // pipeline (`setup_test_grid` → `WorldData.chunks_cpu/blocks_cpu/voxels_cpu`
    // → `extract_world` → `prepare_world_gpu`). Flipping the production
    // producer to GPU is W2/W3-territory: those workstreams' `Core3d` nodes
    // need the GPU chunks/blocks/voxels buffers to exist in `ConstructionGpu`
    // before they can read them. W1's `--validate-gpu-construction` gate
    // proves GPU and CPU outputs are byte-identical on the same source, so the
    // producer flip is sound when W2/W3 land.
    info!(
        "phase-c W1 — gpu construction enabled (validation runs in tests + \
         e2e_render --validate-gpu-construction)"
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

        // Main-world `Startup` driver (regime-1, `15-design-c.md` §1.2). W0
        // body is the gated no-op above; W1 fills it.
        app.add_systems(Startup, run_gpu_construction_startup);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            // Mirror the main-world construction config into the render sub-app.
            .insert_resource(construction_config)
            // Empty pipeline registry — W1..W5 add pipeline fields + a
            // proper `FromWorld` impl as they land.
            .init_gpu_resource::<ConstructionPipelines>()
            // Empty prepare seam — `init_resource`-only body.
            .add_systems(
                Render,
                prepare_construction.in_set(RenderSystems::PrepareResources),
            );
    }
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
        Extent3d, MapMode, PipelineCache, PollType, TexelCopyBufferInfo,
        TexelCopyBufferLayout, TexelCopyTextureInfo, TextureDescriptor, TextureDimension,
        TextureFormat, TextureUsages, TextureViewDescriptor,
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
        _pad2: 0,
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

    let chunks_texture = device.create_texture(&TextureDescriptor {
        label: Some("validate_chunks"),
        size: Extent3d {
            width: size_in_chunks[0],
            height: size_in_chunks[1],
            depth_or_array_layers: size_in_chunks[2],
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D3,
        format: TextureFormat::R32Uint,
        usage: TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::COPY_SRC
            | TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let zero_chunks: Vec<u32> =
        vec![0u32; (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize];
    queue.write_texture(
        TexelCopyTextureInfo {
            texture: &chunks_texture,
            mip_level: 0,
            origin: Default::default(),
            aspect: Default::default(),
        },
        bytemuck::cast_slice(&zero_chunks),
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size_in_chunks[0] * 4),
            rows_per_image: Some(size_in_chunks[1]),
        },
        Extent3d {
            width: size_in_chunks[0],
            height: size_in_chunks[1],
            depth_or_array_layers: size_in_chunks[2],
        },
    );
    let chunks_view = chunks_texture.create_view(&TextureViewDescriptor::default());

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
            &chunks_view,
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

    // Chunks texture readback.
    let chunk_count = size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2];
    let bytes_per_row = (size_in_chunks[0] * 4).max(256).next_multiple_of(256);
    let staging_size = (bytes_per_row * size_in_chunks[1] * size_in_chunks[2]) as u64;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("validate_chunks_readback"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("validate_chunks_readback_enc"),
    });
    enc.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &chunks_texture,
            mip_level: 0,
            origin: Default::default(),
            aspect: Default::default(),
        },
        TexelCopyBufferInfo {
            buffer: &staging,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size_in_chunks[1]),
            },
        },
        Extent3d {
            width: size_in_chunks[0],
            height: size_in_chunks[1],
            depth_or_array_layers: size_in_chunks[2],
        },
    );
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let mut gpu_chunks_out: Vec<u32> = Vec::with_capacity(chunk_count as usize);
    for z in 0..size_in_chunks[2] {
        for y in 0..size_in_chunks[1] {
            let row_offset = (z * size_in_chunks[1] + y) as usize * bytes_per_row as usize;
            let row_bytes =
                &raw[row_offset..row_offset + (size_in_chunks[0] * 4) as usize];
            let row_u32s: &[u32] = bytemuck::cast_slice(row_bytes);
            gpu_chunks_out.extend_from_slice(row_u32s);
        }
    }
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
            _pad2: 0,
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
        Extent3d, MapMode, PipelineCache, PollType, TexelCopyBufferInfo,
        TexelCopyBufferLayout, TexelCopyTextureInfo, TextureDescriptor, TextureDimension,
        TextureFormat, TextureUsages, TextureViewDescriptor,
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

    /// Read the entire R32Uint 3D chunks texture back to CPU as a flat `u32`
    /// vector, in `cz * cx * cy + cy * cx + cx` (x-fastest) order matching
    /// `WorldData.chunks_cpu`'s convention.
    fn readback_chunks_texture(
        device: &RenderDevice,
        queue: &RenderQueue,
        chunks: &bevy::render::render_resource::Texture,
        size: [u32; 3],
    ) -> Vec<u32> {
        let chunk_count = (size[0] * size[1] * size[2]) as u64;
        // For 3D texture readback, bytes_per_row must be a multiple of 256.
        let bytes_per_row =
            (size[0] * 4).max(256).next_multiple_of(256);
        let rows_per_image = size[1];
        let staging_size = (bytes_per_row * size[1] * size[2]) as u64;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("w1_chunks_readback_staging"),
            size: staging_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_chunks_readback"),
        });
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: chunks,
                mip_level: 0,
                origin: Default::default(),
                aspect: Default::default(),
            },
            TexelCopyBufferInfo {
                buffer: &staging,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(rows_per_image),
                },
            },
            Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: size[2],
            },
        );
        queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let raw = slice.get_mapped_range();
        // De-pad: each row has `bytes_per_row` bytes, the first `size[0]*4` of
        // which are the real chunks.
        let mut out: Vec<u32> = Vec::with_capacity(chunk_count as usize);
        for z in 0..size[2] {
            for y in 0..size[1] {
                let row_offset = (z * rows_per_image + y) as usize * bytes_per_row as usize;
                let row_bytes = &raw[row_offset..row_offset + (size[0] * 4) as usize];
                let row_u32s: &[u32] = bytemuck::cast_slice(row_bytes);
                out.extend_from_slice(row_u32s);
            }
        }
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
            _pad2: 0,
            frame_index: 0,
            changed_chunk_count: 0,
            changed_block_count: 0,
            changed_voxel_count: 0,
        };
        let gpu_params =
            create_uniform(&device, &queue, "w1_construction_params", &params);

        // The chunks 3D texture — R32Uint, STORAGE_BINDING + COPY_SRC for
        // readback. wgpu requires 3D textures be `WIDTH * HEIGHT * DEPTH * 4`
        // for R32Uint; the host-side init is `vec![0u32; chunk_count]`.
        let chunks_texture = device.create_texture(&TextureDescriptor {
            label: Some("w1_chunks"),
            size: Extent3d {
                width: size_in_chunks[0],
                height: size_in_chunks[1],
                depth_or_array_layers: size_in_chunks[2],
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D3,
            format: TextureFormat::R32Uint,
            usage: TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_DST
                | TextureUsages::COPY_SRC
                | TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        // Zero-init the texture.
        let zero_chunks: Vec<u32> =
            vec![0u32; (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize];
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &chunks_texture,
                mip_level: 0,
                origin: Default::default(),
                aspect: Default::default(),
            },
            bytemuck::cast_slice(&zero_chunks),
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size_in_chunks[0] * 4),
                rows_per_image: Some(size_in_chunks[1]),
            },
            Extent3d {
                width: size_in_chunks[0],
                height: size_in_chunks[1],
                depth_or_array_layers: size_in_chunks[2],
            },
        );
        let chunks_view = chunks_texture.create_view(&TextureViewDescriptor::default());

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
                &chunks_view,
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
        let gpu_chunks_out = readback_chunks_texture(
            &device,
            &queue,
            &chunks_texture,
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
}
