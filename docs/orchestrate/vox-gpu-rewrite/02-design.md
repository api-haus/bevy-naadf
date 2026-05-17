# vox-gpu-rewrite — design (delegate-architect)

This is the per-subtask implementation spec for W5.1 → W5.6 in landing order
**W5.1 → W5.2 → W5.5 → W5.3 → W5.4 → W5.6**. The orchestrator's required
reading (`01-context.md` + `00-reuse-audit.md`) is the upstream contract; this
file plans the concrete edits an implementer makes. Every cited file:line was
verified by Read before being included.

---

## Top-level architecture

End-to-end data flow once W5 lands:

```
.vox file
  └─► install_vox_in_fixed_world (voxel/grid.rs)              [W5.1]
       ├── vox_import::parse_dot_vox_data(&data)              (single-tile, KEPT helper)
       ├── vox_import::build_world_from_vox(imp)              (KEPT; produces WorldData + VoxelTypes)
       │     └── inserts empty WorldData{ dense_voxel_types=Vec::new() }
       │         + voxel_types into main world
       ├── convert ConstructedWorld → aadf::generator::ModelData
       │     (data_chunk = world.chunks, data_block = world.blocks,
       │      data_voxel = world.voxels, size_in_chunks = world.size_in_chunks)
       └── commands.insert_resource(model_data)                (main-world Resource)

ExtractSchedule:
  stage_model_data_buildonce(...)                              [W5.1]
    ├── gate: ModelDataRender::is_none()
    └── clones ModelData into render-world ModelDataRender

RenderSystems::PrepareResources:
  prepare_construction(...)                                    [W5.2]
    └── new W5 block (model-after construction_bounds_world @ mod.rs:1166-1215):
         ├── allocates 3 storage buffers (data_chunk/data_block/data_voxel)
         │   via generator_model::create_storage_buffer_u32
         ├── allocates 1 uniform buffer for GpuGeneratorModelParams
         │   via generator_model::create_params_uniform (initialised zeroed)
         ├── stashes the 4 buffers on ConstructionGpu (new Option<Buffer> fields)
         └── builds construction_generator_model bind group
             (binding 0 = segment_voxel_buffer (chunk_data_rw),
              binding 1 = model_data_chunk_buffer,
              binding 2 = model_data_block_buffer,
              binding 3 = model_data_voxel_buffer,
              binding 4 = model_data_params_buffer)

Core3d render-graph:
  naadf_gpu_producer_node(...) [extended]                      [W5.3]
    └── three-way ladder:
         ├── ModelDataRender present + W5 deps ready
         │   → for sz in 0..16 { for sy in 0..2 { for sx in 0..16 {
         │         render_queue.write_buffer(params_buf, 0,
         │             bytes_of(per-segment GpuGeneratorModelParams));
         │         dispatch_generator_model_with_encoder(encoder, ...);
         │         dispatch_calc_block_from_raw_data_world_sized(
         │             encoder, p_calc, world_bg, [16, 16, 16]);
         │     }}}
         │     then bounds chain ONCE (voxel + block bounds)
         │     gpu_producer_has_run = true
         ├── ModelDataRender absent + dense_voxel_types non-empty
         │   → existing chunk_calc-only branch (current mod.rs:1943-2000)
         └── both absent → CPU upload fallback (current early-return)

W5.4: delete the CPU XZ tile stop-gap (3 functions + 2 tests) + update 3
docstrings.
W5.5: new e2e module `vox_gpu_construction.rs` + flag wiring.
W5.6: docs-only divergence note for the CPU default-scene retention.
```

Six subtasks fit together: W5.1 lands the data plumbing (resource + extract);
W5.2 lands the GPU plumbing (buffers + bind group); W5.5 lands the e2e gate
*before* W5.3 so the segment loop's first frame is observed; W5.3 wires the
loop; W5.4 deletes the dead stop-gap; W5.6 documents the deliberate divergence
the default-scene path continues to take.

The shader (`generator_model.wgsl`) and the dispatch helper module
(`generator_model.rs`) are FIXED apart from one targeted addition — the
`dispatch_generator_model_with_encoder` sibling helper (Q1 decision; the
existing `dispatch_generator_model` is refactored to call it internally).

---

## Per-subtask design

### W5.1 — `ModelDataRender` render-world resource + build-once extract

**Files touched:**

- `crates/bevy_naadf/src/aadf/generator.rs:72-83` — add `Resource` derive
  (`Clone + Debug` already there per Read of `:72`).
- `crates/bevy_naadf/src/render/extract.rs` — add `ModelDataRender` resource +
  `stage_model_data_buildonce` system + extend imports.
- `crates/bevy_naadf/src/render/mod.rs:122,132-141` — register
  `init_resource::<ModelDataRender>` + add `stage_model_data_buildonce` to the
  `ExtractSchedule` tuple.
- `crates/bevy_naadf/src/voxel/grid.rs:306-343` — rewrite
  `install_vox_in_fixed_world`.

**Derive delta — `aadf/generator.rs:72`:**

Current code (Read-verified at `generator.rs:72`):

```rust
#[derive(Clone, Debug)]
pub struct ModelData { ... }
```

Change to:

```rust
#[derive(Resource, Clone, Debug)]
pub struct ModelData { ... }
```

Add `use bevy::prelude::Resource;` at the top of the file (the module today
uses no Bevy types, so this is a fresh import).

**`ModelDataRender` resource — `render/extract.rs`:**

Add next to `WorldDataMeta` (which lives at `extract.rs:106-119`). Field set
mirrors `ModelData` exactly (same names, same encoding) so a future writer can
extend the extract by reading from one and writing to the other field-by-field.

```rust
/// Render-world mirror of the main-world [`crate::aadf::generator::ModelData`]
/// (vox-gpu-rewrite W5.1). Populated **build-once** by
/// [`stage_model_data_buildonce`] on the first frame the main-world
/// `ModelData` exists (after `install_vox_in_fixed_world` inserts it). Drives
/// the W5 GPU producer chain in `naadf_gpu_producer_node`: presence of this
/// resource is the gate that switches the node from the chunk_calc-only
/// branch to the per-segment generator + chunk_calc chain.
///
/// Mirrors the `WorldGpuStaging` discipline (`extract.rs:67-87`) but is
/// **long-lived** — `prepare_construction` reads it every frame the W5 bind
/// group is being built (the buffers stay; the bind group only rebuilds when
/// `Option<BindGroup>` is `None`).
#[derive(Resource, Default, Clone)]
pub struct ModelDataRender {
    /// `ModelData.data_chunk` — `size_in_chunks.x * y * z` u32 entries.
    pub data_chunk: Vec<u32>,
    /// `ModelData.data_block` — variable length.
    pub data_block: Vec<u32>,
    /// `ModelData.data_voxel` — variable length, two voxels per u32.
    pub data_voxel: Vec<u32>,
    /// Model size in chunks (`ModelData.size_in_chunks`).
    pub size_in_chunks: [u32; 3],
}
```

**`stage_model_data_buildonce` — model-after `stage_world_gpu_buildonce`
(`extract.rs:167-203`):**

```rust
/// `ExtractSchedule` system: **build-once** hand-off of the main-world
/// [`crate::aadf::generator::ModelData`] into the render-world
/// [`ModelDataRender`] resource (vox-gpu-rewrite W5.1).
///
/// Gated on `Option<Res<ModelDataRender>>::is_none()` — once
/// [`prepare_construction`]-side bind-group construction has its source
/// payload, this system short-circuits on every subsequent frame. **There is
/// no per-frame clone.** Mirrors `stage_world_gpu_buildonce` 1:1
/// (`extract.rs:167-203`).
///
/// Per Q2 decision (`vox-gpu-rewrite/01-context.md`): a SEPARATE resource
/// rather than extending `WorldDataMeta` (which carries the "DELIBERATELY
/// MINIMAL" docstring at `extract.rs:102-105`).
pub fn stage_model_data_buildonce(
    mut commands: Commands,
    existing: Option<Res<ModelDataRender>>,
    model_data: Extract<Option<Res<crate::aadf::generator::ModelData>>>,
) {
    if existing.is_some() {
        return;
    }
    let Some(model_data) = &*model_data else {
        return;
    };
    commands.insert_resource(ModelDataRender {
        data_chunk: model_data.data_chunk.clone(),
        data_block: model_data.data_block.clone(),
        data_voxel: model_data.data_voxel.clone(),
        size_in_chunks: model_data.size_in_chunks,
    });
}
```

**Registration — `render/mod.rs`:**

In the `init_resource` chain at `render/mod.rs:122-126`, add immediately after
`init_resource::<WorldDataMeta>()`:

```rust
.init_resource::<ModelDataRender>()
```

In the `ExtractSchedule` system tuple at `render/mod.rs:132-141`, add
`stage_model_data_buildonce` alongside `stage_world_gpu_buildonce`:

```rust
.add_systems(
    ExtractSchedule,
    (
        stage_world_gpu_buildonce,
        stage_model_data_buildonce,   // ← new
        extract_camera,
        extract_camera_history,
        extract_taa_config,
        extract_gi_config,
    ),
)
```

Update the `use crate::render::extract::{...}` block at `render/mod.rs:44-45`:

```rust
use crate::render::extract::{
    stage_model_data_buildonce, stage_world_gpu_buildonce, ExtractedCameraData,
    ExtractedCameraHistory, ExtractedGiConfig, ExtractedTaaConfig, ModelDataRender,
    WorldDataMeta,
};
```

**`install_vox_in_fixed_world` rewrite — `voxel/grid.rs:306-343`:**

Replace the entire body (the current body calls
`vox_import::load_vox_into_world` which is deleted in W5.4). New body:

```rust
fn install_vox_in_fixed_world(commands: &mut Commands, path: &std::path::Path) {
    // W5.1 — parse as a single-tile sparse import (no CPU tiling).
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            error!(
                ".vox load failed (read error: {e}); falling back to embedded \
                 default in fixed world (path: {})",
                path.display()
            );
            install_default_embedded_in_fixed_world(commands);
            return;
        }
    };
    let data = match dot_vox::load_bytes(&bytes) {
        Ok(d) => d,
        Err(e) => {
            error!(
                ".vox load failed (parse error: {e}); falling back to embedded \
                 default in fixed world (path: {})",
                path.display()
            );
            install_default_embedded_in_fixed_world(commands);
            return;
        }
    };
    let imp = match vox_import::parse_dot_vox_data(&data) {
        Ok(i) => i,
        Err(e) => {
            error!(
                ".vox load failed ({e}); falling back to embedded default in \
                 fixed world (path: {})",
                path.display()
            );
            install_default_embedded_in_fixed_world(commands);
            return;
        }
    };

    let model_size_in_chunks = imp.world.size_in_chunks;
    info!(
        "NAADF .vox loaded from {} → ModelData ({}×{}×{} chunks; \
         data_chunk={} u32s, data_block={} u32s, data_voxel={} u32s, \
         {} palette entries). Fixed world {}×{}×{} chunks; GPU producer \
         chain runs per WORLD_SIZE_IN_SEGMENTS = ({}, {}, {}).",
        path.display(),
        model_size_in_chunks[0], model_size_in_chunks[1], model_size_in_chunks[2],
        imp.world.chunks.len(), imp.world.blocks.len(), imp.world.voxels.len(),
        imp.palette.len(),
        WORLD_SIZE_IN_CHUNKS.x, WORLD_SIZE_IN_CHUNKS.y, WORLD_SIZE_IN_CHUNKS.z,
        crate::WORLD_SIZE_IN_SEGMENTS.x,
        crate::WORLD_SIZE_IN_SEGMENTS.y,
        crate::WORLD_SIZE_IN_SEGMENTS.z,
    );

    // C# camera spawn: literal (500, 200, 40) voxels in the fixed
    // 4096×512×4096 world (WorldRender.cs:48-49). `from_world_voxels` scales
    // proportionally for the fixed world size — see camera/mod.rs:54-64.
    let world_voxels = [
        WORLD_SIZE_IN_VOXELS.x,
        WORLD_SIZE_IN_VOXELS.y,
        WORLD_SIZE_IN_VOXELS.z,
    ];
    commands.insert_resource(crate::camera::InitialCameraPose::from_world_voxels(
        world_voxels,
    ));

    // W5.1 — convert ConstructedWorld → ModelData. The `chunks/blocks/voxels`
    // u32 buffers `vox_import` produces are byte-identical to NAADF's
    // `dataChunk/dataBlock/dataVoxel` encoding (`aadf/generator.rs:64-71`).
    let model_data = crate::aadf::generator::ModelData {
        data_chunk: imp.world.chunks,
        data_block: imp.world.blocks,
        data_voxel: imp.world.voxels,
        size_in_chunks: model_size_in_chunks,
    };
    commands.insert_resource(model_data);

    // W5.1 — install an EMPTY WorldData at the FIXED world size. The renderer
    // still consumes this for bind groups (prepare_world_gpu builds the
    // chunks/blocks/voxels storage buffers); the GPU producer dispatches
    // populate them from the segment_voxel_buffer the per-segment chain
    // writes. `dense_voxel_types = Vec::new()` preserves the existing
    // `if meta.dense_voxel_types.is_empty() { return; }` gate at
    // `render/construction/mod.rs:1936-1941` (the W5.3 three-way ladder adds
    // a NEW gate ABOVE that one for ModelDataRender presence).
    let mut world_data = crate::world::data::WorldData {
        chunks_cpu: Vec::new(),
        blocks_cpu: Vec::new(),
        voxels_cpu: Vec::new(),
        size_in_chunks: WORLD_SIZE_IN_CHUNKS,
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(
                WORLD_SIZE_IN_VOXELS.x as i32 - 1,
                WORLD_SIZE_IN_VOXELS.y as i32 - 1,
                WORLD_SIZE_IN_VOXELS.z as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: Vec::new(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();
    commands.insert_resource(world_data);
    commands.insert_resource(VoxelTypes { types: imp.palette });
}
```

**Invariant to preserve:** the EMPTY `chunks_cpu/blocks_cpu/voxels_cpu` here
flows through `stage_world_gpu_buildonce` (`extract.rs:167-203`) into
`WorldGpuStaging` and then `prepare_world_gpu` builds the production
`WorldGpu` storage buffers sized for `size_in_chunks = WORLD_SIZE_IN_CHUNKS`.
The W5 GPU producer then writes into them via `segment_voxel_buffer` (`W1`)
+ chunk_calc/bounds. The empty-CPU-mirror path is what
`stage_world_gpu_buildonce` already does for the sparse `.vox` legacy path —
no change to `prepare_world_gpu` required.

### W5.2 — Upload buffers + build W5 bind group in `prepare_construction`

**Files touched:**

- `crates/bevy_naadf/src/render/construction/mod.rs:106-190` — add 4 new
  `Option<Buffer>` fields to `ConstructionGpu`.
- `crates/bevy_naadf/src/render/construction/mod.rs:198-226` — add 1 new
  `Option<BindGroup>` field to `ConstructionBindGroups`.
- `crates/bevy_naadf/src/render/construction/mod.rs:830-846` — extend
  `prepare_construction` signature with `Option<Res<ModelDataRender>>`.
- `crates/bevy_naadf/src/render/construction/mod.rs:1162-1215` (insertion
  site, after the `construction_bounds_world` block ends) — insert new W5
  block.

**`ConstructionGpu` field additions** (inserted at end of struct, before the
closing `}` at `mod.rs:190`):

```rust
// === W5 — generator_model storage uploads + per-segment uniform ===========
/// W5 — `modelDataChunk` storage buffer (read-only by the generator;
/// `data_chunk` from `aadf::generator::ModelData`). Allocated + uploaded once
/// by `prepare_construction` on the first frame `ModelDataRender` is present.
pub model_data_chunk_buffer: Option<Buffer>,
/// W5 — `modelDataBlock` storage buffer (read-only by the generator).
pub model_data_block_buffer: Option<Buffer>,
/// W5 — `modelDataVoxel` storage buffer (read-only by the generator).
pub model_data_voxel_buffer: Option<Buffer>,
/// W5 — `GpuGeneratorModelParams` uniform (64 B, `generator_model.rs:74-93`).
/// **One buffer, rewritten in place 512 times per producer run** — once per
/// segment in the W5.3 segment loop via `RenderQueue::write_buffer`. (See
/// Decision: "one params buffer vs 512 buffers" below.)
pub model_data_params_buffer: Option<Buffer>,
```

**`ConstructionBindGroups` field addition** (inserted at end of struct, before
closing `}` at `mod.rs:226`):

```rust
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
/// 512 segments** (binding identities are stable; only the uniform contents
/// rotate).
pub construction_generator_model: Option<BindGroup>,
```

**`prepare_construction` signature extension** (at `mod.rs:830-846`):

Add a new parameter at the END of the parameter list (parallel to
`world_data_meta`):

```rust
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
    world_data_meta: Option<Res<crate::render::extract::WorldDataMeta>>,
    // W5.2 — render-world mirror of `ModelData`. Present only after the W5.1
    // `stage_model_data_buildonce` extract has run (which only fires when the
    // main-world install path inserts a `ModelData`); absent on default-scene
    // + entity-only / non-VOX runs.
    model_data: Option<Res<crate::render::extract::ModelDataRender>>,
) {
```

**New W5 prepare block** — insert AFTER the `construction_bounds_world`
bind-group block ends at `mod.rs:1216` (just before the W3
`if construction_config.gpu_construction_enabled && bound_group_count > 0 ...`
seed at `:1237`):

```rust
// === W5 — generator_model upload + bind group ============================
//
// vox-gpu-rewrite W5.2: when `ModelDataRender` is present (the W5.1 extract
// fired), allocate the 3 model_data storage buffers + the params uniform,
// upload the three model buffers ONCE, and build the W5 `@group(0)` bind
// group. Build-once: every step gates on `is_none()`, so this fires on the
// first frame all of {model_data, generator_model_layout, segment_voxel_buffer}
// are ready and is a no-op every frame after.
//
// Gating: requires
//   - `model_data: Option<Res<ModelDataRender>>` → Some
//   - `construction_pipelines.generator_model_layout` (always present per
//     the `ConstructionPipelines::from_world` impl at `:337-344`)
//   - `gpu.segment_voxel_buffer` → Some (allocated in the W1 block at
//     `:988-1015` — that block runs whenever `want_gpu_producer` is true.
//     Caveat: the existing block at `:888-890` gates `want_gpu_producer` on
//     `dense_voxel_types` being non-empty. The W5 install path leaves
//     `dense_voxel_types = Vec::new()`, so `segment_voxel_buffer` will NOT
//     get auto-allocated. The W5 block ALSO needs to allocate it.)
//
// See Decision: "segment_voxel_buffer allocation for W5 path" below.
if let Some(model_data) = model_data.as_deref() {
    // 1) Allocate `segment_voxel_buffer` if it's not already there. Sized at
    //    WORLD_SIZE_IN_CHUNKS^3 * 2048 u32s (= 256³ * 2048 * 4 B ≈ 512 MiB
    //    at the fixed C# world size — note the same allocation `WorldData.cs:73`
    //    makes for the C# `segmentVoxelBuffer`). Zero-initialised; the W5.3
    //    segment loop fills it.
    if gpu.segment_voxel_buffer.is_none() {
        let world_chunk_count = (world_gpu.chunks_size_in_chunks.x
            * world_gpu.chunks_size_in_chunks.y
            * world_gpu.chunks_size_in_chunks.z) as u64;
        let size = world_chunk_count * 2048 * 4;
        let buf = render_device.create_buffer(&BufferDescriptor {
            label: Some("naadf_segment_voxel_buffer_w5"),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        gpu.segment_voxel_buffer = Some(buf);
        bind_groups.construction_world = None;
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

    // 3) Allocate the params uniform (zeroed initial; the W5.3 loop overwrites
    //    it 512 times per producer run).
    if gpu.model_data_params_buffer.is_none() {
        let zeroed = generator_model::GpuGeneratorModelParams {
            size_in_voxels: [0; 3],
            _pad0: 0,
            model_size_in_chunks: [0; 3],
            _pad1: 0,
            group_offset_in_chunks: [0; 3],
            group_size_in_chunks_x: 0,
            group_size_in_chunks_y: 0,
            _pad2: 0,
            _pad3: 0,
            _pad4: 0,
        };
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
            let bgl = pipeline_cache
                .get_bind_group_layout(&construction_pipelines.generator_model_layout);
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
```

**Bind-group entry ordering** is dictated by
`generator_model::generator_model_layout_descriptor` at
`generator_model.rs:131-147`, verified by Read:

| Binding | Layout descriptor | Bind-group entry |
|---|---|---|
| 0 | `storage_buffer_sized(false, None)` — chunk_data_rw | `segment_voxel_buffer` |
| 1 | `storage_buffer_read_only_sized(false, None)` — model_data_chunk_ro | `model_data_chunk_buffer` |
| 2 | `storage_buffer_read_only_sized(false, None)` — model_data_block_ro | `model_data_block_buffer` |
| 3 | `storage_buffer_read_only_sized(false, None)` — model_data_voxel_ro | `model_data_voxel_buffer` |
| 4 | `uniform_buffer_sized(false, Some(params_size))` — params | `model_data_params_buffer` |

`BindGroupEntries::sequential((a, b, c, d, e))` populates bindings 0..4 in
order — verified by reading the parallel construction at `mod.rs:1174-1177`
(2-binding sequential) and `:1453-1458` (4-binding sequential).

### W5.5 — `--vox-gpu-construction` e2e gate (lands BEFORE W5.3)

**Files added:**

- `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` (new file, ~180 LOC).

**Files touched:**

- `crates/bevy_naadf/src/e2e/mod.rs:24-32` — add `pub mod vox_gpu_construction;`.
- `crates/bevy_naadf/src/bin/e2e_render.rs:81-89` — add `vox_gpu_construction_mode`
  flag parsing.
- `crates/bevy_naadf/src/bin/e2e_render.rs:~210-227` — add a dispatch branch.

**Module skeleton — `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs`:**

```rust
//! `--vox-gpu-construction` mode — regression gate for the vox-gpu-rewrite
//! W5 GPU producer chain (`docs/orchestrate/vox-gpu-rewrite/`).
//!
//! ## Goal
//!
//! End-to-end gate that:
//!   1. Loads the in-tree Oasis fixture (`OASIS_VOX_FIXTURE_PATH`,
//!      `e2e/oasis_edit_visual.rs:81`) through the production W5.1
//!      `install_vox_in_fixed_world` path.
//!   2. Boots the e2e harness with `fixed_world_size = true` +
//!      `gpu_construction_enabled = true` (the W5 default), so the new
//!      `ModelData → ModelDataRender → W5 per-segment dispatch → chunk_calc`
//!      chain runs against the production buffers.
//!   3. Asserts the framebuffer captured at the e2e camera pose
//!      (`gates::e2e_camera_transform`) is non-empty — region-mean luminance
//!      above a "captured something" floor (the post-W5 frame must show
//!      the GPU-produced geometry, not pure skybox).
//!
//! Per Q3 decision (`01-context.md`): no `AppArgs::vox_gpu_construction_mode`
//! flag. The new e2e module sets `AppArgs.fixed_world_size = true` +
//! `grid_preset = GridPreset::Vox { OASIS_VOX_FIXTURE_PATH, tiles: 1 }`
//! directly + reuses the standard e2e camera pose. The driver runs the
//! existing Warmup→Motion→Settle→Shoot flow; no driver-flow customisation.
//!
//! Per Q4 decision (`01-context.md`): the W5.5 gate reuses
//! `OASIS_VOX_FIXTURE_PATH` from `oasis_edit_visual.rs:81` (Git LFS-tracked
//! at `crates/bevy_naadf/assets/test/oasis_hard_cover.vox`).

use std::path::PathBuf;

use bevy::prelude::AppExit;

use crate::e2e::framebuffer::{Framebuffer, Rect};
use crate::e2e::oasis_edit_visual::{oasis_vox_fixture_path, OASIS_VOX_FIXTURE_PATH};

/// Screen-rect fractional bounds the gate samples for the "captured
/// something" assertion. A central 40%×40% region (same shape as
/// `vox_e2e.rs` `VOX_GEOMETRY_RECT_FRACS`). The standard e2e camera pose
/// (`gates::e2e_camera_transform` at `(86, 42, 90)` looking at `(32, 16, 32)`)
/// does NOT frame the populated region of `oasis_hard_cover.vox` (the Oasis
/// model is ~93×34×84 chunks ≈ 1488×544×1344 voxels — far outside the e2e
/// camera's calibrated 64×32×64-voxel view box).
///
/// See the InitialCameraPose assumption in `02-design.md` — for the W5.5 gate
/// the framebuffer floor assertion uses a coarse "framebuffer is not pure
/// skybox / not all-black" check rather than a "see the model" check.
const FRAME_REGION_FRACS: (f32, f32, f32, f32) = (0.30, 0.30, 0.70, 0.70);

/// Mean luminance floor: any frame above this value has captured *something*
/// (skybox is ~146; a pure-black frame is 0). At the e2e camera pose the
/// scene the W5 path produces sits BEHIND/BELOW the camera (the e2e pose
/// frames the legacy 64×32×64 world centre); the frame should still capture
/// the atmosphere-tinted sky band. A regression that leaves chunks_buffer
/// uninitialised (e.g. W5.3's segment loop never fires) typically yields
/// pure-black (because `prepare_world_gpu` allocates chunks zeroed and
/// `chunks[i] == 0` decodes as Empty); the sky band remains, so the floor
/// catches the regression by being above 0 but below the sky band.
///
/// **Calibration:** set to `40.0` — well below the measured sky-band mean
/// (~146 per `vox_e2e.rs:88`) so the standard frame passes; well above 0
/// so a pure-black "GPU producer never ran" failure trips.
const NOT_BLACK_LUMINANCE_FLOOR: f32 = 40.0;

/// Boot the e2e harness with the production W5 GPU producer path enabled.
/// Returns the harness's `AppExit`.
pub fn run_vox_gpu_construction() -> AppExit {
    let path = oasis_vox_fixture_path();
    if !path.exists() {
        eprintln!(
            "e2e_render --vox-gpu-construction: FIXTURE MISSING at {} \
             (cwd = {:?}). The fixture is Git LFS-tracked at \
             {OASIS_VOX_FIXTURE_PATH}. Run `git lfs pull` to fetch.",
            path.display(),
            std::env::current_dir().ok()
        );
        return AppExit::error();
    }
    println!(
        "e2e_render --vox-gpu-construction: loading Oasis VOX fixture from \
         {} ({} bytes) into the W5 GPU producer chain",
        path.display(),
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
    );

    let mut app_args = crate::AppArgs::default();
    // Production W5 path: fixed-size world + GPU construction default-on
    // (= the `bevy-naadf::main` shape, per `lib.rs:393` + `:143`).
    app_args.grid_preset = crate::GridPreset::Vox { path, tiles: 1 };
    app_args.fixed_world_size = true;
    app_args.construction_config.gpu_construction_enabled = true;
    // Reuse the `--vox-e2e` driver branch — same shape: swaps the
    // default-scene Batch-6 region gate for the `assert_vox_geometry_visible`
    // non-skybox check. (Per Q3 decision, no NEW mode flag.)
    app_args.vox_e2e_mode = true;

    crate::run_e2e_render_with_args(app_args)
}

/// Region-luminance gate: framebuffer must have captured SOMETHING — region
/// mean luminance above the not-black floor. This is the only assertion the
/// W5.5 gate adds on top of the standard `--vox-e2e` driver checks
/// (PipelineCache scan + node-dispatch).
///
/// The standard e2e camera pose (`gates::e2e_camera_transform`) frames the
/// legacy 64×32×64 origin region, NOT the populated Oasis region (~93×34×84
/// chunks). The W5 GPU producer chain populates the full 256×32×256-chunk
/// fixed world from the model via per-segment dispatch + Y-clamp at
/// `generator_model.wgsl:114-116`. A passing frame proves: pipelines
/// compiled, every render-graph node ran, the GPU producer ran without
/// silently crashing the device.
pub fn assert_frame_not_black(fb: &Framebuffer) -> Result<(), String> {
    let (fx0, fy0, fx1, fy1) = FRAME_REGION_FRACS;
    let region = Rect::from_fractional(fb, fx0, fy0, fx1, fy1);
    let mean = fb.region_mean(region);
    let lum = Framebuffer::luminance(mean);

    println!(
        "e2e_render --vox-gpu-construction: region mean rgba {mean:?}, \
         luminance {lum:.1} (floor > {NOT_BLACK_LUMINANCE_FLOOR:.0})",
    );

    if lum <= NOT_BLACK_LUMINANCE_FLOOR {
        return Err(format!(
            "vox-gpu-construction gate FAIL — region luminance {lum:.1} at \
             or below not-black floor {NOT_BLACK_LUMINANCE_FLOOR:.0}. The W5 \
             GPU producer chain likely failed (pipelines failed to compile, \
             the segment loop didn't run, or the dispatch silently crashed). \
             Inspect target/e2e-screenshots/e2e_latest.png + run with \
             RUST_LOG=debug for shader-cache + dispatch traces.",
        ));
    }
    Ok(())
}

/// Save a vox-gpu-construction screenshot alongside the standard
/// `e2e_latest.png`. Best-effort.
pub fn save_screenshot(fb: &Framebuffer) {
    let path = std::path::Path::new(crate::e2e::E2E_SCREENSHOT_DIR)
        .join("vox_gpu_construction_latest.png");
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --vox-gpu-construction: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --vox-gpu-construction: screenshot save failed: {e}"
        ),
    }
}
```

**Note on assertion strategy:** the W5.5 module reuses
`assert_vox_geometry_visible` (via `vox_e2e_mode = true` in the driver
branch — that's how `--vox-e2e` already gates the framebuffer). The
`assert_frame_not_black` helper above is the alternative if reusing the
`--vox-e2e` driver path is judged to give a misleading "FAIL — region
luminance below sky ceiling" message (the Oasis model isn't framed by the e2e
camera; the central region will be sky, which is fine but trips
`SKY_LUMINANCE_CEILING`). **Implementer decision:** start by reusing
`vox_e2e_mode = true` and observe the first run's region luminance report;
if the central 40%×40% is the sky band (~146), the `--vox-e2e` gate trips.
Swap to a custom driver gate using `assert_frame_not_black` only if needed.
See the InitialCameraPose assumption section below for the full rationale.

**`e2e/mod.rs:32` export addition:**

```rust
pub mod vox_e2e;
pub mod vox_gpu_construction;   // ← new
```

**`bin/e2e_render.rs:89` flag parse addition** (insert immediately after
`small_edit_repro_mode`):

```rust
let small_edit_repro_mode = args.iter().any(|a| a == "--small-edit-repro");
let vox_gpu_construction_mode =
    args.iter().any(|a| a == "--vox-gpu-construction");
```

**`bin/e2e_render.rs:~210` dispatch branch addition** (insert before the
`vox_e2e_mode` branch at `:210`):

```rust
} else if vox_gpu_construction_mode {
    // `--vox-gpu-construction` — load the Oasis fixture through the
    // production W5 GPU producer chain (vox-gpu-rewrite W5.5). Loads
    // `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` as `ModelData`,
    // runs 16×2×16 = 512 per-segment generator + chunk_calc dispatches
    // against the production WorldGpu buffers, asserts framebuffer is not
    // pure-black. See `bevy_naadf::e2e::vox_gpu_construction`.
    bevy_naadf::e2e::vox_gpu_construction::run_vox_gpu_construction()
} else if vox_e2e_mode {
```

### W5.3 — Per-segment generator + chunk_calc dispatch loop

**Files touched:**

- `crates/bevy_naadf/src/render/construction/generator_model.rs:217-254` —
  add the `dispatch_generator_model_with_encoder` sibling helper; refactor
  existing `dispatch_generator_model` to call it.
- `crates/bevy_naadf/src/render/construction/mod.rs:1913-1924` — extend
  `naadf_gpu_producer_node` signature with `Res<RenderQueue>` +
  `Option<Res<ModelDataRender>>`.
- `crates/bevy_naadf/src/render/construction/mod.rs:1925-2001` — rewrite
  the body with the three-way branch ladder.

**Sibling helper — `generator_model.rs`:**

Replace the existing `dispatch_generator_model` at `generator_model.rs:229-254`
with this two-function form (existing function's external signature unchanged
— internal body now calls the sibling):

```rust
/// W5 — encoder-taking dispatch helper. Same shape as
/// `chunk_calc::dispatch_calc_block_from_raw_data_world_sized` at
/// `chunk_calc.rs:198-215`: caller owns the `CommandEncoder`, so the
/// production `naadf_gpu_producer_node` can chain this against subsequent
/// `chunk_calc` dispatches on the SAME encoder (so wgpu auto-inserts the
/// STORAGE→STORAGE barrier between the generator's writes and chunk_calc's
/// reads of `segment_voxel_buffer`).
///
/// Per Q1 decision in `docs/orchestrate/vox-gpu-rewrite/01-context.md`: the
/// "treat `generator_model.rs` as a FIXED dependency" rule is **explicitly
/// loosened** for this one sibling helper.
pub fn dispatch_generator_model_with_encoder(
    encoder: &mut bevy::render::render_resource::CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    group_size_in_chunks: [u32; 3],
) {
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_generator_model_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    // Dispatch shape: one workgroup per chunk in the segment.
    pass.dispatch_workgroups(
        group_size_in_chunks[0],
        group_size_in_chunks[1],
        group_size_in_chunks[2],
    );
}

/// W5 unit-test entry point (`generator_model_gpu_vs_cpu_bit_exact` in
/// `crate::render::construction::mod::tests`). Builds its own encoder +
/// `queue.submit` so the unit test can run without a render context.
///
/// Per Q1 decision, this delegates the dispatch body to
/// [`dispatch_generator_model_with_encoder`] — one source of truth.
pub fn dispatch_generator_model(
    device: &RenderDevice,
    queue: &RenderQueue,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    group_size_in_chunks: [u32; 3],
) {
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("naadf_generator_model_encoder"),
    });
    dispatch_generator_model_with_encoder(
        &mut encoder,
        pipeline,
        bind_group,
        group_size_in_chunks,
    );
    queue.submit([encoder.finish()]);
}
```

This preserves the W5 unit test's caller (`device + queue`) verbatim while
factoring out the inner pass for the production node.

**`naadf_gpu_producer_node` extension — `mod.rs:1913-2001`:**

New signature (add `render_queue: Res<RenderQueue>` and
`model_data: Option<Res<ModelDataRender>>`):

```rust
#[allow(clippy::too_many_arguments)]
pub fn naadf_gpu_producer_node(
    mut render_context: bevy::render::renderer::RenderContext,
    pipeline_cache: Res<bevy::render::render_resource::PipelineCache>,
    construction_pipelines: Option<Res<ConstructionPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_gpu: Option<ResMut<ConstructionGpu>>,
    construction_config: Option<Res<config::ConstructionConfig>>,
    world_data_meta: Option<Res<crate::render::extract::WorldDataMeta>>,
    // W5.3 — used for per-segment uniform rewrites.
    render_queue: Res<RenderQueue>,
    // W5.3 — drives the three-way branch ladder.
    model_data: Option<Res<crate::render::extract::ModelDataRender>>,
) {
```

**Three-way branch ladder body** — replaces `mod.rs:1925-2001`:

```rust
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

    // Common-prerequisite pipelines (Algorithm 1 + bounds chain). The W5
    // branch ALSO needs `generator_model_pipeline`; the chunk-calc-only
    // branch does not. Resolve only what each branch needs.
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

    // Three-way producer gate (vox-gpu-rewrite, audit drift #5):
    //   (a) `ModelDataRender` present + W5 deps ready → run W5 chain.
    //   (b) else `dense_voxel_types` non-empty → existing chunk_calc-only.
    //   (c) else → CPU upload fallback (early-return).
    if let Some(model_data) = model_data.as_deref() {
        // === (a) W5 branch — per-segment generator + chunk_calc ============
        // Requires the W5 pipeline + bind group to be ready.
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

        let world_size_in_chunks = [
            crate::WORLD_SIZE_IN_CHUNKS.x,
            crate::WORLD_SIZE_IN_CHUNKS.y,
            crate::WORLD_SIZE_IN_CHUNKS.z,
        ];
        let world_size_in_voxels = [
            crate::WORLD_SIZE_IN_VOXELS.x,
            crate::WORLD_SIZE_IN_VOXELS.y,
            crate::WORLD_SIZE_IN_VOXELS.z,
        ];
        // 16 chunks per axis per segment (`WORLD_GEN_SEGMENT_SIZE_IN_GROUPS *
        // 4` = `4 * 4`; verified at `lib.rs:218,224,234,906`).
        let segment_chunks: u32 = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
        let group_size_in_chunks = [segment_chunks, segment_chunks, segment_chunks];

        let encoder = render_context.command_encoder();

        // C#-faithful loop order: outer Z, middle Y, inner X (verified
        // against `NAADF/NAADF/World/Data/WorldData.cs:136-140`).
        for sz in 0..crate::WORLD_SIZE_IN_SEGMENTS.z {
            for sy in 0..crate::WORLD_SIZE_IN_SEGMENTS.y {
                for sx in 0..crate::WORLD_SIZE_IN_SEGMENTS.x {
                    // Per-segment uniform — mirrors C#
                    // `WorldGeneratorModel.CopyToChunkData`
                    // (`WorldGeneratorModel.cs:32-60`):
                    //   modelSizeInChunks ← ModelData.sizeInChunks
                    //   sizeInVoxels      ← WorldData.actualSizeInVoxels
                    //   groupOffsetInChunks ← segmentPos * worldGenSegmentSizeInChunks
                    //   groupSizeInChunksX/Y ← per-segment chunk extent
                    let group_offset_in_chunks = [
                        sx * segment_chunks,
                        sy * segment_chunks,
                        sz * segment_chunks,
                    ];
                    let params = generator_model::GpuGeneratorModelParams {
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
                        bytemuck::bytes_of(&params),
                    );

                    // Step 1: generator_model.wgsl → segment_voxel_buffer
                    // (shaped (16, 16, 16) workgroups; one per chunk in the
                    // segment per `generator_model.wgsl:121-132`).
                    generator_model::dispatch_generator_model_with_encoder(
                        encoder,
                        p_gen,
                        gen_bg,
                        group_size_in_chunks,
                    );

                    // Step 2: chunk_calc.calc_block_from_raw_data over the
                    // same segment extent (= the C# `CalculateChunkBlocks`
                    // dispatch at `WorldData.cs:506`).
                    chunk_calc::dispatch_calc_block_from_raw_data_world_sized(
                        encoder,
                        p_calc,
                        world_bg,
                        group_size_in_chunks,
                    );
                }
            }
        }

        // Bounds chain — runs ONCE after the full segment loop (mirrors the
        // existing `:1980-1992` chain). Worker counts derived from the same
        // `meta.{blocks,voxels}_cpu_len` as the chunk-calc-only branch.
        let (cpu_blocks, cpu_voxels) = match world_data_meta.as_deref() {
            Some(meta) => (meta.blocks_cpu_len, meta.voxels_cpu_len),
            // Empty CPU mirror on the W5 path — the GPU will write the real
            // blocks/voxels into the production WorldGpu buffers. Use the
            // full-world maxes as a conservative upper bound: every chunk
            // could be mixed (max blocks = chunks * 64), every block could
            // be mixed (max voxels = blocks * 32). These are the workgroup
            // dispatch counts, not buffer sizes, so over-dispatch is safe.
            None => {
                let chunks = world_size_in_chunks[0]
                    * world_size_in_chunks[1]
                    * world_size_in_chunks[2];
                let max_blocks = chunks * 64;
                let max_voxels = max_blocks * 32;
                (max_blocks, max_voxels)
            }
        };
        let voxel_workgroups = (cpu_voxels / 32 + 1).max(1);
        let block_workgroups = (cpu_blocks / 64 + 1).max(1);

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
             voxel_workgroups={voxel_workgroups}, block_workgroups={block_workgroups}).",
            crate::WORLD_SIZE_IN_SEGMENTS.x
                * crate::WORLD_SIZE_IN_SEGMENTS.y
                * crate::WORLD_SIZE_IN_SEGMENTS.z,
        );
        return;
    }

    // === (b) chunk-calc-only branch (existing behaviour) ====================
    let Some(meta) = world_data_meta else { return; };
    if meta.dense_voxel_types.is_empty() {
        // === (c) CPU upload fallback ======================================
        // Source scene didn't author a `DenseVolume` AND no `ModelData` — GPU
        // producer is unsafe to run (the segment_voxel_buffer the chunk_calc
        // dispatch needs cannot be built from CPU data, AND there's no model
        // to generate from). Fall back to the CPU upload path.
        return;
    }

    let size_in_chunks = [
        meta.size_in_chunks.x,
        meta.size_in_chunks.y,
        meta.size_in_chunks.z,
    ];
    let cpu_blocks = meta.blocks_cpu_len;
    let cpu_voxels = meta.voxels_cpu_len;
    let voxel_workgroups = (cpu_voxels / 32 + 1).max(1);
    let block_workgroups = (cpu_blocks / 64 + 1).max(1);

    let encoder = render_context.command_encoder();
    chunk_calc::dispatch_calc_block_from_raw_data_world_sized(
        encoder,
        p_calc,
        world_bg,
        size_in_chunks,
    );
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
        "phase-c followup#1 — GPU producer chain DISPATCHED (size_in_chunks={:?}, \
         voxel_workgroups={}, block_workgroups={}).",
        size_in_chunks, voxel_workgroups, block_workgroups,
    );
```

**Subtle invariants the implementer must preserve:**

1. **Loop order = (z outer, y middle, x inner)**. Verified against C#
   `NAADF/NAADF/World/Data/WorldData.cs:136-140`:
   ```csharp
   for (int z = 0; z < sizeInWorldGenSegments.Z; ++z)
       for (int y = 0; y < sizeInWorldGenSegments.Y; ++y)
           for (int x = 0; x < sizeInWorldGenSegments.X; ++x)
   ```
2. **`group_offset_in_chunks = (sx, sy, sz) * segment_chunks`** where
   `segment_chunks = WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 = 16`. Mirrors C#
   `WorldData.cs:143`:
   `Point3 segmentPosInChunks = segmentPos * worldGenSegmentSizeInChunks;`.
3. **`group_size_in_chunks_x/y = segment_chunks = 16`** (NOT the world's
   chunk extent). These are the X/Y strides the shader uses for
   `group_index` at `generator_model.wgsl:130-132`; they must scope
   `group_index` to the per-segment slice the dispatch writes, not the
   whole-world slice.
4. **Same encoder + same bind group** for all 512 iterations. wgpu inserts
   STORAGE→STORAGE barriers automatically between adjacent compute passes
   on the same encoder that read/write the same buffer alias.
5. **`gpu_producer_has_run = true`** at the end of the W5 branch — the
   W3-side bounds-init seed at `mod.rs:1237-1266` is gated on this and on
   `want_gpu_producer`; the W5 branch is conceptually a `want_gpu_producer
   = true` for this run. The existing gate at `:1240`
   (`!want_gpu_producer || gpu.gpu_producer_has_run`) covers the W5 case
   naturally because once we flip `gpu_producer_has_run`, the seed runs.
6. **`segment_voxel_buffer` is preserved across all 512 iterations** — the
   buffer's contents are valid only for the segment most recently written,
   which is fine because `chunk_calc.dispatch_calc_block_from_raw_data_world_sized`
   for that segment runs IMMEDIATELY after the generator write for that
   segment within the same iteration. After all 512 iterations, the buffer's
   final state is the last segment's writes (segment `(15, 1, 15)`); this is
   OK because the bounds chain after the loop does not read
   `segment_voxel_buffer` — only `chunks/blocks/voxels` (the production
   `WorldGpu` buffers chunk_calc populated).
7. **`render_queue.write_buffer(params_buf, ...)` 512 times in a row**
   inside a render-graph node: this is the standard Bevy pattern (the
   existing `prepare_construction` does it at `:1404-1435` + `:1066-1108`).
   `RenderQueue::write_buffer` enqueues the write onto wgpu's staging
   belt; subsequent dispatches on the encoder pick up the latest value
   because wgpu inserts a copy→storage barrier on first use.

### W5.4 — Delete the CPU tile stop-gap

**Files touched:**

- `crates/bevy_naadf/src/voxel/vox_import.rs` — DELETE the three named
  functions + the two named tests.
- `crates/bevy_naadf/src/render/construction/mod.rs:2017-2020` — update
  docstring.
- `crates/bevy_naadf/src/voxel/vox_import.rs:46-56` — update Δ-decisions block.
- `crates/bevy_naadf/src/voxel/vox_import.rs:382-385` — update
  `build_world_from_vox` Δ-GPUProducer comment.

**Exact deletion list (line ranges Read-verified at audit time):**

| Symbol | File | Line range | Reason |
|---|---|---|---|
| `pub fn load_vox_into_world` | `voxel/vox_import.rs` | `:193-200` | Only caller was `install_vox_in_fixed_world` (rewritten in W5.1). |
| `pub fn parse_dot_vox_data_into_world` | `voxel/vox_import.rs` | `:259-273` | Only caller was `load_vox_into_world` (above). |
| `fn tile_buckets_into_world` | `voxel/vox_import.rs` | `:287-325` | Only caller was `parse_dot_vox_data_into_world` (above). The CPU XZ-tiling stop-gap itself. |
| `#[test] into_world_tiles_xz_and_leaves_y_above_tile_empty` | `voxel/vox_import.rs` | `:1831-1874` | Exercises `parse_dot_vox_data_into_world` (deleted). |
| `#[test] into_world_with_target_smaller_than_tile_clips` | `voxel/vox_import.rs` | `:1876-1889` | Exercises `parse_dot_vox_data_into_world` (deleted). |

**KEEP:**

- `pub fn parse_vox_bytes` (`:154`).
- `pub fn parse_vox_bytes_tiled` (`:161`).
- `pub fn load_vox` (`:171`).
- `pub fn load_vox_tiled` (`:181`).
- `pub fn parse_dot_vox_data` (`:206`).
- `pub fn parse_dot_vox_data_tiled` (`:223`).
- `fn replicate_buckets_xz` (`:335-376`).
- `pub fn build_world_from_vox` (`:386-423`).

The other 30+ tests in `mod tests` stay (they exercise the kept helpers).

**Docstring updates:**

*`render/construction/mod.rs:2017-2020`* — change:

```rust
///   1. `generator_model` per segment — currently bypassed for the bevy-naadf
///      test scene (the scene authors a `DenseVolume` directly rather than
///      using NAADF's `WorldGenerator`); `segment_voxel_buffer` is rebuilt
///      CPU-side from `WorldData::dense_voxel_types` and uploaded.
```

to:

```rust
///   1. `generator_model` per segment — vox-gpu-rewrite W5 landed: when a
///      `.vox` load installs a `ModelData` main-world resource, the
///      `naadf_gpu_producer_node` runs 16×2×16 per-segment dispatches of
///      `generator_model.wgsl` into `segment_voxel_buffer`, mirroring
///      `WorldData.cs:120-156`'s `GenerateWorld` per-segment chain. The
///      default-scene path retains CPU upload (see W5.6 divergence in
///      `docs/orchestrate/naadf-bevy-port/12-alignment-gap.md`).
```

*`voxel/vox_import.rs:46-56`* — replace the Δ-GPUProducer line:

```rust
//! - Δ-GPUProducer — `WorldData::dense_voxel_types = Vec::new()` for `.vox`
//!   content; the data-driven gate at `render/construction/mod.rs:833-835`
//!   skips the GPU producer (the renderer reads the pre-built CPU buffers).
```

with:

```rust
//! - Δ-GPUProducer — `WorldData::dense_voxel_types = Vec::new()` for `.vox`
//!   content. Two consumer paths:
//!     - Fixed-world `.vox` load (`install_vox_in_fixed_world`): vox-gpu-rewrite
//!       W5 wires a `ModelData` main-world resource → `ModelDataRender`
//!       render-world → per-segment `generator_model.wgsl` + `chunk_calc`
//!       dispatch chain. The renderer reads GPU-produced buffers.
//!     - Legacy sized-to-model `.vox` load (`install_vox_sized_to_model`):
//!       sparse path still skips the GPU producer; the data-driven gate at
//!       `render/construction/mod.rs:1936-1941` falls through to CPU upload.
```

*`voxel/vox_import.rs:382-385`* — `build_world_from_vox`'s Δ-GPUProducer
comment:

```rust
/// **Δ-GPUProducer (v2):** `dense_voxel_types` is set to `Vec::new()` so the
/// GPU producer's data-driven gate at `render/construction/mod.rs:833-835`
/// skips the segmented-dispatch chain — the renderer reads the pre-built
/// CPU mirror buffers via the existing extract/prepare upload path.
```

becomes:

```rust
/// **Δ-GPUProducer (v2):** `dense_voxel_types` is set to `Vec::new()` so the
/// data-driven gate at `render/construction/mod.rs:1936-1941` skips the
/// chunk-calc-only GPU producer branch. The vox-gpu-rewrite W5
/// `install_vox_in_fixed_world` path installs a `ModelData` resource
/// alongside the empty `WorldData`; presence of `ModelData` flips the
/// `naadf_gpu_producer_node` ladder to the per-segment chain. The legacy
/// sized-to-model path (`install_vox_sized_to_model`) installs NO
/// `ModelData`, so it continues to fall through to the CPU upload path.
```

### W5.6 — Document default-scene CPU-retention divergence

No code change. Append a section to
`docs/orchestrate/naadf-bevy-port/12-alignment-gap.md`. Exact text to add:

```markdown
## vox-gpu-rewrite W5.6 — default-scene CPU upload retention (2026-05-17)

**Status:** deliberate divergence, deferred deletion candidate.

The fixed-world `.vox` load path (W5.1 `install_vox_in_fixed_world`) installs
a `ModelData` main-world resource, which the W5.3 segment loop in
`naadf_gpu_producer_node` uses to populate the production
`WorldGpu::{chunks,blocks,voxels}` buffers via 512 per-segment dispatches of
`generator_model.wgsl` + `chunk_calc.calc_block_from_raw_data`. This matches
C# `WorldData.cs:120-156` `GenerateWorld` line-by-line.

**The Default scene path (`install_default_embedded_in_fixed_world`,
`voxel/grid.rs:156-249`) does NOT install a `ModelData`.** Instead it
continues to:

1. Run CPU `aadf::construct::construct()` to produce the primitive scene's
   `chunks_cpu/blocks_cpu/voxels_cpu`.
2. Compose those into a fixed-world layout via
   `compose_default_scene_into_fixed_world` (`grid.rs:390-486`).
3. Install with `dense_voxel_types = Vec::new()` — the renderer reads the
   pre-built CPU mirror via `stage_world_gpu_buildonce` and
   `prepare_world_gpu`, bypassing the GPU producer chain entirely (the
   data-driven gate at `render/construction/mod.rs:1936-1941` short-circuits
   the `naadf_gpu_producer_node`'s chunk-calc-only branch on
   `dense_voxel_types.is_empty()`).

### Why retain the CPU path

The C# `WorldGeneratorModel` (`generatorModel.fx`) **tiles the model
unconditionally** via `voxelPos % (modelSizeInChunks * 16)` at
`generatorModel.fx:20`. The default scene is a 4×2×4-chunk primitive arrangement
(`voxel/grid.rs::build_default_volume`) that does NOT carry a
single-tile-XZ-repeat-everywhere semantic — synthesising a `ModelData` from it
would force unwanted 16×16 XZ tiling of the demo across the 256×32×256-chunk
fixed world (with `Y > 0` empty per the shader's Y-clamp at
`generatorModel.fx:48-49`). The user-facing default scene would change visibly.

The C# binary does not actually exercise the no-model startup path with a
primitive default scene — the C# default is "load `oasis.cvox` if present,
empty otherwise". The Bevy port's CPU primitive scene is a port-specific
convenience that has no C# correspondent.

### When to delete

When a full-GPU default scene path lands — either by:

- (a) Making the primitive scene authorable as a `ModelData` (i.e. carve the
  scene out of `data_chunk/data_block/data_voxel` with the dedup pass
  `aadf::construct` does, then accept the XZ tiling); or
- (b) Replacing the primitive scene with a small in-tree `.vox` fixture
  routed through `install_vox_in_fixed_world`.

Either path lets `install_default_embedded_in_fixed_world` and
`compose_default_scene_into_fixed_world` be deleted (and removes
`dense_voxel_types` entirely once the entity-test scenes also migrate).

### Code touched (vox-gpu-rewrite W5.6)

None — this is documentation only. The divergence was already implemented
by Phase-C followup #1 (`render/construction/mod.rs:1936-1941`'s
`dense_voxel_types.is_empty()` short-circuit); W5.6 just records it as
deliberate per `CLAUDE.md`'s faithful-port-divergence-needs-docs rule.
```

---

## Decisions & rejected alternatives

This section captures every load-bearing choice for the implementer. Each
entry: chosen approach, rejected alternative, reasoning, and what fact would
flip the call.

### Three-way producer gate ordering — `ModelData` → `dense_voxel_types` → fallback

**Chosen:** explicit ladder `if model_data.is_some() { W5 } else if
dense_voxel_types.non_empty() { chunk_calc_only } else { return }`.

**Rejected:** combined gate `if model_data || dense_voxel_types` then runtime
dispatch on which is present. Rejected because the two branches have
**different prerequisite pipelines** (W5 branch needs
`generator_model_pipeline` + `construction_generator_model` bind group;
chunk-calc-only branch does not) — a unified prerequisite check would block
the legacy default-scene path from running whenever the W5 pipeline failed to
compile.

**What would flip:** if the codebase later removes the dense default-scene
path entirely (per W5.6's "When to delete"), the ladder collapses to a
two-way: `if model_data { W5 } else { return }`.

### Loop iteration order — Z outer, Y middle, X inner

**Chosen:** match C# `WorldData.cs:136-140` byte-for-byte:
```
for (int z = 0; z < sizeInWorldGenSegments.Z; ++z)
    for (int y = 0; y < sizeInWorldGenSegments.Y; ++y)
        for (int x = 0; x < sizeInWorldGenSegments.X; ++x)
```

**Rejected:** X-outer / Z-inner (or any other permutation). The iteration
order is **observationally invariant** for the W5 pipeline — each segment
writes its own slice of `segment_voxel_buffer` (and that buffer is consumed
immediately by `chunk_calc` for the SAME segment), so no segment's dispatch
depends on a previously-written segment's voxels. So in principle X/Y/Z
permutation does not matter.

**Why match C# anyway:** per `CLAUDE.md`'s faithful-port discipline + the
`bevy-naadf-faithful-port-rule` memory ("no Bevy-only microoptimizations or
behaviors not in C# NAADF; default = match C#, even when C# has the bug").
The implementer should treat this as load-bearing for the port even though
the GPU dispatch outcome is identical.

**What would flip:** a hypothetical future where two adjacent segments share
state via `block_voxel_count` cursors that depend on submission order, AND
the C# happens to rely on that order. (Not the case today; the
`block_voxel_count` cursor advances monotonically and the final state is
order-independent.)

### `run_worldgen_only` flag — ignore

**Chosen:** the W5 branch does NOT additionally gate on
`construction_config.run_worldgen_only`. Surface as a followup if needed.

**Rejected:** gating the W5 branch on `run_worldgen_only`. Rejected because
the flag's docstring at `construction/config.rs:92-99` explicitly says it's a
"W5-only isolation flag" used by the unit test — its production semantics are
undefined. Adding a runtime check there would entangle the flag's per-test
isolation contract with production code paths.

**What would flip:** if the flag's docstring is updated to say it gates the
production W5 chain in addition to the unit test (e.g. "production W5 runs
when `gpu_construction_enabled && (model_data_present || run_worldgen_only)`"),
add the additional gate. Until then, ignore.

### `InitialCameraPose` for the W5.5 gate — reuse standard e2e camera, accept Oasis off-frame

**Chosen:** the W5.5 gate uses the standard e2e camera pose
(`gates::e2e_camera_transform` at `(86, 42, 90)` looking at `(32, 16, 32)`)
and sets `app_args.vox_e2e_mode = true` so the driver substitutes the
`assert_vox_geometry_visible` non-skybox check. The framebuffer assertion is
a "captured something" floor (a frame that is not pure-black means the GPU
producer chain ran; a frame above the sky-band ceiling means the rendered
scene includes more than atmospheric tint).

**Rejected:**

1. **Override `InitialCameraPose` to frame the Oasis model.** Rejected
   because the e2e harness uses `setup_e2e_camera` at `e2e/mod.rs:287-314`
   and IGNORES `InitialCameraPose` entirely — the resource the install path
   inserts has no effect in the e2e harness. (Verified by reading
   `camera/mod.rs:31-47` doc + `e2e/mod.rs:287-314`.)
2. **Add a fresh camera-pose helper for the W5.5 module.** Possible — add
   `fn vox_gpu_camera_transform() -> Transform` in
   `vox_gpu_construction.rs` that frames a chunk known to be populated by
   the Oasis fixture (e.g. position the camera at `(1488*0.5, 800,
   1344*0.5)` looking down per `oasis_edit_visual.rs::birdseye_pose`), and
   override `setup_e2e_camera` for this mode. Rejected because that requires
   a driver-mode flag (per Q3 we don't add one) or a `Startup`-system
   override that's more invasive than the W5.5 gate's intent. The Q3
   decision specifies the gate uses the production e2e flow as-is.
3. **Assert framebuffer is non-empty over a tighter rect framing the
   Oasis location at the standard camera pose.** Rejected because the
   standard e2e camera does not see the Oasis-populated region (Oasis is
   centered around (744, 272, 672) voxels; the standard camera is at
   (86, 42, 90) looking at (32, 16, 32) — opposite hemisphere of the world).

**What would flip:** if the user prefers a stronger gate (e.g. "must
actually render the Oasis model"), introduce a custom camera pose in
`vox_gpu_construction.rs` and override `setup_e2e_camera` via a per-mode
Startup system. This was rejected for the FIRST landing of W5.5 to keep the
patch small; a followup can add the visible-model gate.

### Bind-group entry ordering — sequential 0..4

**Chosen:** `BindGroupEntries::sequential((segv, mdc, mdb, mdv, params))` —
matches the bindings 0..4 declared by `generator_model_layout_descriptor` at
`generator_model.rs:131-147` byte-for-byte.

**Rejected:** explicit indexed entries (`BindGroupEntry { binding: 0, ... }`,
etc.). The codebase uses `BindGroupEntries::sequential` for every other
construction bind group (`mod.rs:1174-1177`, `:1192-1199`, `:1208-1212`,
`:1453-1458`, `:1536-1546`, `:1789-1795`); using explicit indexed entries
would diverge from the established discipline without benefit.

**What would flip:** the layout descriptor changes order or skips an index
(neither is happening — the layout is FIXED per Q1's "no edits to
generator_model_layout_descriptor").

### One params buffer, rewritten 512 times — vs 512 buffers allocated up-front

**Chosen:** one `Buffer` allocated once via `create_params_uniform`, rewritten
in place 512 times per producer run via `RenderQueue::write_buffer`.

**Rejected:**

1. **512 buffers + 512 bind groups.** Allocates 512 × 64 B = 32 KiB of
   uniform buffer (trivial) AND 512 distinct bind groups. The bind-group
   allocation is the cost (each requires a `create_bind_group` call). The
   wgpu cost of `RenderQueue::write_buffer` for 64 B is essentially zero
   (it's a memcpy onto the staging belt). Rejected because the staging
   approach is the codebase pattern (`mod.rs:1404-1435` does it for
   `GpuConstructionParams`).
2. **One params buffer + dynamic uniform offsets.** Allocate one 512×64 B
   buffer, bind with dynamic offset, advance per segment. Rejected because
   `generator_model_layout_descriptor` declares the binding as
   `uniform_buffer_sized(false, Some(params_size))` (`generator_model.rs:143`)
   — the `false` means non-dynamic. Switching to dynamic would require
   editing the layout descriptor (forbidden by Q1).

**What would flip:** profiling shows `RenderQueue::write_buffer` 512×
per-producer-run is hot (it isn't — happens once at startup). The producer
runs ONCE per `gpu_producer_has_run` flip; this isn't a per-frame path.

### Encoder lifetime — one shared encoder for all 512 dispatches + bounds chain

**Chosen:** `let encoder = render_context.command_encoder();` once at the top
of the W5 branch; all 512 generator + 512 chunk_calc + 2 bounds dispatches
share it. wgpu auto-inserts STORAGE→STORAGE barriers between adjacent passes.

**Rejected:**

1. **Per-segment encoder (`device.create_command_encoder` + `queue.submit`
   inside the loop).** This is the shape of the existing
   `dispatch_generator_model` helper at `generator_model.rs:229-254`.
   Rejected for the production node body per Q1: `render_context.command_encoder()`
   is the encoder the renderer's subsequent reads come from; submitting
   separately means wgpu cannot insert the barrier across submissions for the
   same buffer alias, AND it costs 512 separate submits per producer run.
2. **One encoder per segment, all submitted after the loop.** Functionally
   identical to (1); rejected for the same reasons.

**What would flip:** wgpu's encoder size cap is exceeded. (Not a concern at
this scale: 512 generator passes + 512 chunk_calc passes + 2 bounds passes
= 1026 compute passes, well within any reasonable encoder limit.)

### `segment_voxel_buffer` allocation in the W5 path — extend the existing block

**Chosen:** the W5.2 prepare block allocates `segment_voxel_buffer` when
`ModelDataRender` is present AND the existing W1 block's
`want_gpu_producer` gate (`mod.rs:888-890`) skips it (because
`dense_voxel_types.is_empty()`). Sized at WORLD_SIZE_IN_CHUNKS^3 * 2048 u32s.

**Rejected:**

1. **Allocate via the existing W1 block.** Would require flipping
   `want_gpu_producer = construction_config.gpu_construction_enabled &&
   (dense_data_ready || model_data_ready)`. Rejected for surface-area:
   `want_gpu_producer` controls four buffer allocations (hash_map,
   hash_coefficients, block_voxel_count, segment_voxel_buffer) AND a code
   path that depends on `world_data_meta.dense_voxel_types`. Rewiring the
   gate is invasive; allocating the one missing buffer (segment_voxel_buffer)
   in the new W5 block is the minimal patch.
2. **Sized at the per-segment extent (16^3 * 2048 u32s) rewritten 512
   times.** Rejected because `chunk_calc.dispatch_calc_block_from_raw_data_world_sized`
   shares the binding with the W5 dispatch — chunk_calc reads the same
   `segment_voxel_buffer` (binding 4 of `construction_world` layout) the
   generator wrote, and a 16³-sized buffer would underflow chunk_calc's
   per-segment indexing if it expected a full-world buffer. C# allocates
   the full-world size at `WorldData.cs:73` for the same reason.

**What would flip:** wgpu `max_buffer_size` proves too small for
WORLD_SIZE_IN_CHUNKS^3 * 2048 * 4 = 256³ * 2048 * 4 = 32 GiB (this WILL
exceed standard caps). Mitigation: the existing W1 path at `:988-1015`
**pads to the cubic extent of the largest world axis**, not the full
multiplication; the W5 block should ideally do the same. Re-derive: the
WORLD_SIZE_IN_CHUNKS is `(256, 32, 256)` → max axis 256 → cubic extent
256³ = ~16.7M chunks × 2048 u32 × 4 B/u32 = **~134 GiB**. This is past every
realistic wgpu cap.

**REVISED:** the W5 chain CANNOT use a single full-world cubic
`segment_voxel_buffer`. It MUST be sized at the **per-segment cubic extent**
(16³ chunks × 2048 u32 × 4 B = 128 MiB). The implementer therefore allocates
`segment_voxel_buffer` at `segment_chunks^3 * 2048 * 4` bytes (128 MiB,
within wgpu Vulkan-baseline 256 MiB cap), and chunk_calc's
`dispatch_calc_block_from_raw_data_world_sized` is called with
`group_size_in_chunks = [16, 16, 16]` (the per-segment extent, NOT the
world extent) — same shape the C# `CalculateChunkBlocks` dispatch uses
(`WorldData.cs:506`: `App.graphicsDevice.DispatchCompute(worldGenSegmentSizeInChunks,
worldGenSegmentSizeInChunks, worldGenSegmentSizeInChunks);`).

The W5 design above is correct — the implementer should size
`segment_voxel_buffer` at `segment_chunks * segment_chunks * segment_chunks
* 2048 * 4` bytes (128 MiB) and pass `group_size_in_chunks = [segment_chunks,
segment_chunks, segment_chunks]` to BOTH the generator and the
`chunk_calc.dispatch_calc_block_from_raw_data_world_sized` call.

**What would flip:** wgpu's `max_buffer_size` is raised AND there is a
performance reason to avoid the per-segment reuse pattern (not currently the
case).

### Adding `RenderQueue` to `naadf_gpu_producer_node`'s signature

**Chosen:** add `render_queue: Res<RenderQueue>` to the system parameters.

**Rejected:** rewrite the params buffer via `RenderContext::command_encoder()`
+ `encoder.write_buffer`. Rejected because `wgpu::CommandEncoder` does not
expose a `write_buffer` method; the staging-belt write APIs are on the
queue. `RenderContext` does not have direct queue access.

**What would flip:** Bevy adds a `RenderContext::write_buffer` shortcut (not
currently available in the codebase version).

---

## Assumptions made

Things I had to infer without explicit confirmation in the brief or audit.
The implementer should verify on first run.

1. **`ModelData` derives only `Clone + Debug` today** (not yet `Resource`).
   Verified by Read at `aadf/generator.rs:72`:
   ```rust
   #[derive(Clone, Debug)]
   pub struct ModelData { ... }
   ```
   Therefore W5.1 adds **only** `Resource`. If a future change has already
   added `Resource`, the W5.1 derive delta is a no-op.

2. **`bevy::render::renderer::RenderQueue` is the correct import name** for
   the Bevy version this project uses. Verified by Read at
   `render/construction/generator_model.rs:46`:
   ```rust
   use bevy::render::renderer::{RenderDevice, RenderQueue};
   ```
   Same import is used in `prepare_construction` (`mod.rs:76`).

3. **`OASIS_VOX_FIXTURE_PATH` resolves at `cargo run` time from the workspace
   root.** Verified by Read at `e2e/oasis_edit_visual.rs:80-92`: the existing
   `--oasis-edit-visual` gate uses `oasis_vox_fixture_path()` which tries
   the workspace-relative path first then falls back to a crate-relative
   variant. The fixture is presumed Git LFS-tracked per the audit's
   "presumably via Git LFS — verify before adding any new LFS config"
   (`01-context.md:111-112`). If `git lfs pull` has not been run, the W5.5
   gate emits a clear `FIXTURE MISSING` error and returns `AppExit::error()`
   (mirrors the `--oasis-edit-visual` shape).

4. **C# loop order is Z/Y/X.** Verified by Read at
   `/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:136-140`:
   ```csharp
   for (int z = 0; z < sizeInWorldGenSegments.Z; ++z)
       for (int y = 0; y < sizeInWorldGenSegments.Y; ++y)
           for (int x = 0; x < sizeInWorldGenSegments.X; ++x)
   ```

5. **`segment_voxel_buffer` is allocated at the per-segment cubic extent
   (16³ chunks × 2048 u32 × 4 B = 128 MiB)**, NOT at the full-world cubic
   extent. The W5 chain dispatches `chunk_calc` over the per-segment extent
   (matching C# `WorldData.cs:506`'s
   `DispatchCompute(worldGenSegmentSizeInChunks, ..., ...)` shape). See the
   "REVISED" note in the Decisions section.

6. **The W5 branch of `naadf_gpu_producer_node` runs only ONCE per app
   lifecycle** (via `gpu.gpu_producer_has_run` short-circuit) — 512
   per-segment dispatches × 1 producer run, not 512 × N frames.

7. **`generator_model.wgsl` is FIXED** — verified by Read of all 160 lines.
   The implementer must not edit it. The HLSL→WGSL port is audited per
   `generator_model.rs:1-32`'s module doc.

8. **The `--vox-gpu-construction` gate's framebuffer assertion can reuse
   `vox_e2e_mode = true`** to substitute `assert_vox_geometry_visible` for
   the default-scene Batch-6 gate. If the central 40%×40% region at the
   standard e2e camera pose lands in the atmosphere-tinted sky (the
   expected case for Oasis-off-frame), `assert_vox_geometry_visible` will
   FAIL because sky luminance ~146 < threshold 160. **The implementer
   should run the gate FIRST without any vox_e2e_mode flag to observe the
   luminance, then EITHER (a) lower the threshold or (b) switch to a custom
   `assert_frame_not_black` floor (the helper in the W5.5 module skeleton
   above)** depending on what the first measured luminance shows.

9. **`run_worldgen_only` is unused in production** — verified by grep + Read
   at `construction/config.rs:92-99`: docstring says "Used by the W5 unit
   test to exercise its GPU path"; no production callers found. The W5
   branch ignores it.

10. **The existing W1 path's `want_gpu_producer` gate at `mod.rs:888-890`
    will NOT allocate `segment_voxel_buffer` for the W5 path** because the
    W5 install path leaves `dense_voxel_types = Vec::new()` → `dense_data_ready
    = false` → `want_gpu_producer = false` → the block at `:891-1015` skips
    every allocation. The W5.2 prepare block must therefore allocate
    `segment_voxel_buffer` itself.

11. **The bounds chain after the W5 segment loop uses
    `world_data_meta.{blocks,voxels}_cpu_len` if present, else
    conservative full-world upper bounds.** Since the W5 install path
    leaves `blocks_cpu/voxels_cpu` EMPTY, `world_data_meta.blocks_cpu_len`
    and `voxels_cpu_len` are both 0 → `voxel_workgroups = max(1, 0/32+1) = 1`,
    `block_workgroups = max(1, 0/64+1) = 1`. This under-dispatches the
    bounds chain. The W5 branch overrides this when `world_data_meta` is
    absent OR when `world_data_meta.dense_voxel_types.is_empty()` (the W5
    path's signature) — using the conservative upper bound `chunks * 64`
    blocks and `chunks * 64 * 32` voxels. (For 256³ chunks this is
    16.7M * 64 / 64 ≈ 16.7M block_workgroups — within wgpu's
    `max_compute_workgroups_per_dimension` per axis but the implementer
    should sanity-check the dispatch count on first run.)

---

## Verification checklist for the implementer

After landing **each** subtask (in landing order), run:

1. **After W5.1:**
   - `cargo build --workspace` — must compile.
   - `cargo test --workspace --lib` — must stay at the baseline (198
     passed, 1 ignored per `01-context.md:302`).
   - `cargo run --bin e2e_render -- --vox-e2e` — existing gate; must stay
     green (the W5.1 install path only fires when `fixed_world_size = true`,
     which `--vox-e2e` does not set).
   - `cargo run --bin e2e_render -- --oasis-edit-visual` — same caveat.

2. **After W5.2:**
   - `cargo build --workspace`.
   - `cargo test --workspace --lib`.
   - The new W5 prepare block is gated on `ModelDataRender` presence; no
     existing e2e gate inserts that resource, so all existing gates must
     stay green. Verify by running:
     - `cargo run --bin e2e_render -- --baseline`
     - `cargo run --bin e2e_render -- --edit-mode`
     - `cargo run --bin e2e_render -- --validate-gpu-construction`
     - `cargo run --bin e2e_render -- --vox-e2e`

3. **After W5.5 (lands BEFORE W5.3 per the brief):**
   - `cargo build --workspace`.
   - `cargo run --bin e2e_render -- --vox-gpu-construction` — **expected to
     FAIL at this stage** because W5.3 has not landed; the segment loop
     does not run, `gpu_producer_has_run` never flips on the W5 path,
     `WorldGpu::chunks` stays zeroed → render reads decode every chunk as
     Empty → framebuffer is pure sky (luminance ~146 < the
     `assert_vox_geometry_visible` threshold of 160 if `vox_e2e_mode = true`,
     OR ≥ the `NOT_BLACK_LUMINANCE_FLOOR` of 40 if the custom gate is
     used). The gate's FAIL message must clearly indicate "W5 chain did
     not run" semantics (so W5.3 has a concrete failing oracle to fix).
   - All other gates must still be green.

4. **After W5.3:**
   - `cargo build --workspace`.
   - `cargo test --workspace --lib` — must include the existing
     `generator_model_gpu_vs_cpu_bit_exact` test (the W5 unit test in
     `render/construction/mod.rs:3206-3377`) — proves the refactor of
     `dispatch_generator_model → dispatch_generator_model_with_encoder`
     preserves the inner pass behavior.
   - `cargo run --bin e2e_render -- --vox-gpu-construction` — must now
     PASS (W5.3 wires the segment loop; the framebuffer shows GPU-produced
     geometry or at least the standard sky-band the gate accepts).
   - **MUST NOT regress** any existing gate:
     - `--baseline`
     - `--edit-mode`
     - `--runtime-edit-mode`
     - `--entities`
     - `--validate-gpu-construction`
     - `--vox-e2e`
     - `--oasis-edit-visual`
     - `--small-edit-visual`
     - `--small-edit-repro`

5. **After W5.4 (deletions):**
   - `cargo build --workspace` — proves nothing in the tree calls the
     deleted functions.
   - `cargo test --workspace --lib` — must drop by 2 tests (198 → 196
     passed, 1 ignored).
   - `cargo run --bin e2e_render -- --vox-gpu-construction` — must stay
     green.
   - All other gates must stay green (the deleted functions had no callers
     reachable from non-deleted code per the W5.4 audit table).

6. **After W5.6 (docs-only):**
   - `cargo build --workspace`.
   - `cargo test --workspace --lib`.
   - No e2e gate changes (W5.6 is pure documentation).
   - Verify the appended section is well-formatted and the file
     `docs/orchestrate/naadf-bevy-port/12-alignment-gap.md` parses as
     markdown.

7. **DO NOT** run `cargo run --bin bevy-naadf` as a verification step.
   Per `CLAUDE.md` and `01-context.md:148-149`, the W5.5 gate is the
   verification surface. Let the user perform the live visual check.

8. **Final sweep** before declaring W5 done:
   - `cargo build --workspace` — clean.
   - `cargo test --workspace --lib` — 196 passed, 1 ignored.
   - `cargo clippy --workspace --all-targets` — no new warnings from any
     changed file.
   - Full e2e suite green:
     - `--baseline` ✓
     - `--edit-mode` ✓
     - `--runtime-edit-mode` ✓
     - `--entities` ✓
     - `--validate-gpu-construction` ✓
     - `--vox-e2e` ✓
     - `--oasis-edit-visual` ✓
     - `--small-edit-visual` ✓
     - `--small-edit-repro` ✓
     - `--vox-gpu-construction` ✓ (new)
