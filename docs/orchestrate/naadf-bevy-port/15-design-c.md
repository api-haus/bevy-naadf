# 15 — Phase C Architecture Design (canonical methodology completion)

## delegate-architect findings (2026-05-15)

Phase C is the deliberate completion of NAADF's *construction, maintenance and
dynamism* half (paper §3.2–3.6, the back half of the Method section). The
rendering half (Phases A, A-2, B) is already faithful, review-gated, and merged
to `main` HEAD `8995c88`. This design fans Phase C out into seven parallel
workstreams behind a **single seam-first extension surface**: a new
`crates/bevy_naadf/src/render/construction/` sub-module that owns *all* new GPU
construction state (`WorldGpu` extensions, parallel construction bind-group
layouts, the construction sub-graph and its dispatch ordering) so each
worktree-isolated workstream lands without touching the small set of
shared-render-graph entry points (`render/mod.rs`, `NaadfPipelines`, `WorldGpu`).
The CPU `aadf/construct.rs` 3-phase build stays as a bit-exact GPU validation
oracle + fallback (E4); GPU construction is the new default *producer*, the
traversal shader is producer-agnostic. Five new WGSL files (~15 entry points),
three new Bevy systems, three new growable buffer families, all consumed by the
new sub-module — and verified end-to-end by extending the existing
`cargo run --bin e2e_render` harness with editing/entity/regenerate gates that
assert GPU-vs-CPU buffer-bit equality on the 16³-chunk test grid.

Every file path, line range, symbol, struct size and `numthreads` value below
was verified by Read/Grep on disk against `/mnt/archive4/DEV/bevy-naadf/` and
`/mnt/archive4/DEV/NAADF/`. Nothing is invented. Where this design uses the
faithful-port "documented MonoGame↔wgpu deviation" exception (the
`STORAGE_READ_WRITE`+`INDIRECT` split, the `vec3`-then-scalar `vec4` alignment,
the entity-track 64-bit chunk widening), it is named and justified.

---

## 1. Seam-first extension design

### 1.1 Where the seam lives — `crates/bevy_naadf/src/render/construction/`

The Phase-C construction sub-module is **a sibling of `render/atmosphere.rs`,
`render/gi.rs`, `render/taa.rs`**, not a new top-level crate or a top-level
`aadf_gpu/` module. Justification:

- The construction work is *render-world* work: it dispatches compute against
  buffers owned by `WorldGpu` / `FrameGpu`, using the existing `NaadfPipelines`
  + `Core3d` schedule plumbing. Phase B already proves the
  "sub-module-under-`render/`" pattern (`atmosphere.rs`+`gi.rs`+`graph_b.rs` in
  `crates/bevy_naadf/src/render/mod.rs:16-23`). A new top-level `aadf_gpu/` or
  workspace crate would force a redundant `RenderApp` re-entry point.
- The CPU 3-phase build stays at `crates/bevy_naadf/src/aadf/construct.rs` —
  untouched. The GPU construction port is *not* a replacement for `aadf/`; it
  is a render-side producer that emits buffers in the same on-wire format. E4
  binds: the CPU path is the bit-exact oracle.
- The module is a **directory** (`render/construction/mod.rs` + per-shader files),
  not a single 2 000-line file, because seven workstreams will edit it in
  parallel and a directory split eliminates file-level merge conflicts on
  workstreams that are otherwise independent.

Layout:

```
crates/bevy_naadf/src/render/construction/
  mod.rs              ConstructionGpu + ConstructionBindGroups render-world
                      resources; the `prepare_construction` system that creates
                      / resizes / uploads all new buffers + builds all new bind
                      groups; pub re-exports of the node systems + helpers.
  config.rs           ConstructionConfig (initial hashMap size, growth factor,
                      maxGroupBoundDispatch, etc.) — read from AppArgs at
                      plugin-build time, same pattern as TaaRingConfig
                      (taa.rs:35). Single source of truth, both CPU buffer
                      sizing and shader specialisation key off it.
  chunk_calc.rs       The world-build node systems (initial Algorithm 1 build +
                      the local-AADF passes): naadf_chunk_calc_node,
                      naadf_compute_voxel_bounds_node,
                      naadf_compute_block_bounds_node. Driven by the build-time
                      / regenerate event path (NOT every-frame).
  bounds_calc.rs      The background per-frame chunk-AADF queue node systems:
                      naadf_bounds_prepare_node, naadf_bounds_compute_node
                      (×N_ROUNDS, NAADF runs 5 — `WorldBoundHandler.cs:113`).
                      One queue per frame is the load-bearing rate.
  world_change.rs     The edit-apply node systems gated on a per-frame edit
                      event: naadf_apply_chunk_change_node,
                      naadf_apply_block_change_node,
                      naadf_apply_voxel_change_node,
                      naadf_apply_group_change_node.
  map_copy.rs         The hash-map regrow node: naadf_map_copy_node. Gated on
                      a CPU-detected occupancy-threshold trigger from
                      ConstructionGpu's hash-map handler.
  generator_model.rs  The world-generator node: naadf_generator_model_node.
                      Driven by the build-time / regenerate event path; emits
                      into `segment_voxel_buffer`, then `chunk_calc` consumes.
  entity_update.rs    The entity-sync node systems: naadf_entity_chunk_update_node,
                      naadf_entity_history_copy_node. Driven by a per-frame
                      entity-events queue.
  hashing.rs          The CPU-side hash-coefficient table (the 65 `31^(64-i)`
                      values, `BlockHashingHandler.cs:50-55`) + occupancy
                      bookkeeping. A render-world resource (ConstructionGpu's
                      backing data). All CPU; no GPU systems.
  change_handler.rs   The CPU-side flood-fill (`ChangeHandler.UpdateWorld` —
                      paper §3.5: 7-round BFS, distance step 4, cap 28).
                      Runs in `ExtractSchedule` to mirror edit events from main
                      world; populates the `changed_*` upload buffers consumed
                      by `world_change.rs`. CPU; no GPU systems.
  entity_handler.rs   The CPU-side entity instance hashing / per-chunk pointer
                      build (`EntityHandler.cs:165-475`). Runs in
                      `ExtractSchedule`; populates the entity upload buffers
                      consumed by `entity_update.rs`. CPU; no GPU systems.
```

`render/mod.rs` change: one new `pub mod construction;` import + one
`prepare_construction` registration + a **single tagged insert point** in the
`.chain()` (§3) — that is the entirety of the seam touching the existing
render-graph file. `NaadfPipelines` gains a new sub-struct
`pipelines.rs::ConstructionPipelines` set by a separate `init_gpu_resource` (no
edits to existing pipeline declarations).

### 1.2 Render-graph scheduling — three temporal regimes, one sub-graph

NAADF's construction has *three* execution regimes:

1. **One-shot at startup** (`WorldData.GenerateWorld` — `WorldData.cs:120`):
   `worldGenerator.CopyToChunkData` per segment, then
   `chunkCalc.calcBlockFromRawData`, then `boundHandler.Initialize` seeds the
   bound queues, then `ChunkCopyToCpu` syncs GPU→CPU. Runs once before
   rendering starts. Sequence: **generator → chunk_calc (3 passes) → bounds_init**.
2. **Every-frame, throttled** (`WorldBoundHandler.Update` — `:91-121`): 5 rounds
   of `PrepareGroupBounds` + indirect-`ComputeGroupBounds`, gated by
   `maxGroupBoundDispatch`. The "one queue per frame" of paper §3.3.
3. **On-edit-event** (`ChangeHandler.UpdateWorld` — `:69`): CPU flood-fill →
   upload 4 `changed*Dynamic` buffers → dispatch `worldChange` (4 passes) →
   bound queues feed regime #2 the following frame.

The Bevy mapping respects all three:

- **Regime 1 (startup)** runs as a `Startup`-schedule system in the main app
  that drives the GPU dispatches *via direct command-encoder submission* (the
  same `RenderQueue::submit` pattern `prepare_world_gpu` uses today —
  `prepare.rs:168-180`). It does *not* sit in `Core3d`. It runs **once**,
  blocks startup until done (acceptable: NAADF blocks the same way —
  `WorldData.cs:152-153` prints a "Chunk Generation: X%" progress log). On
  completion, `WorldData.dirty = true` + a fresh `ConstructionGpu` resource are
  inserted, the CPU `aadf::construct::construct` is *also* run on the same
  source `DenseVolume`, the two outputs are asserted bit-equal in a `#[test]`
  / a debug-build assert path, then rendering begins.
- **Regime 2 (every-frame)** is a single `Core3d` node `naadf_bounds_compute_node`
  inserted **before `naadf_atmosphere_node`** (the first existing node —
  `render/mod.rs:226`). It owns its own sub-sequence:
  `bounds_prepare → indirect bounds_compute` looped `N_ROUNDS` times in one
  `RenderContext::add_command_buffer_generation_task`. Throttled by
  `ConstructionConfig.max_group_bound_dispatch` (NAADF: 512·64 — `WorldBoundHandler.cs:25`).
- **Regime 3 (on-edit-event)** is a `Core3d` node `naadf_world_change_node`
  inserted **directly after the bounds-compute node, before atmosphere**. Gated
  on `ConstructionEvents.has_pending_changes()` (an extracted render-world flag
  set by `extract_world_changes` if `ChangeHandler.changedGroupCount > 0` or
  any `changedChunk/Block/Voxel` count > 0). On a frame with no edits this node
  early-returns within microseconds (one bool check, no command encoding).

The final `Core3d` chain (the existing 14 nodes plus the 2 new construction
nodes) is shown in §3. Total chain length on a no-edit frame: 16 nodes,
construction nodes early-return cheaply.

### 1.3 Bind-group strategy — dedicated construction-mode layouts (parallel)

The reuse audit (`13-reuse-audit-c.md` "Borderline calls") flagged this as a
required design call: the existing `@group(0)` `world_layout`
(`pipelines.rs:298-312`) declares `blocks`/`voxels` as `storage_buffer_read_only_sized`
because the render passes only read them. wgpu's `BindGroupLayoutEntry` validation
**forbids the same buffer being bound `STORAGE_READ_WRITE` AND `STORAGE_READ_ONLY`
in one bind-group layout** — a layout is single-shot per visibility-stage. So
construction *cannot* reuse `world_layout`.

**Decision: a dedicated parallel `construction_world_layout` (not a
`@group(1)` extension).** Reasoning:

- A `@group(1)` extension would put the construction-only buffers (`hashMap`,
  `blockVoxelCount`, `segmentVoxelBuffer`, the queue family, the change family)
  on a *separate* group while leaving the rw `blocks`/`voxels`/`chunks` access
  on `@group(0)`. That still violates the wgpu rule — `@group(0)`'s
  `blocks`/`voxels` would need to be `STORAGE_READ_WRITE` for the construction
  pass, conflicting with the existing read-only declaration at the *layout
  descriptor* level. Wgpu validates per-layout, not per-pipeline.
- The parallel-layout approach (one layout for construction, the existing one
  for rendering, bound to the *same wgpu buffers*) is what NAADF effectively
  does — its HLSL declares `RWStructuredBuffer<uint> blocks` in `chunkCalc.fx:34`
  and `StructuredBuffer<uint> blocks` (read-only) in `rayTracing.fxh`, two
  different layout slots over the same underlying buffer. The wgpu translation
  is two bind-group-layout-descriptors over the same `Buffer` handles.

Concrete construction `@group(0)` layout (`construction/mod.rs`'s
`construction_world_layout`):

| binding | wgsl type | rust binding helper | shader |
|---|---|---|---|
| 0 | `texture_storage_3d<r32uint, read_write>` chunks_rw | `storage_texture_3d` | chunkCalc, boundsCalc, worldChange, entityUpdate |
| 1 | `storage_buffer<array<u32>>` blocks_rw | `storage_buffer_sized(false, None)` | chunkCalc, worldChange |
| 2 | `storage_buffer<array<u32>>` voxels_rw | `storage_buffer_sized(false, None)` | chunkCalc, worldChange, generatorModel |
| 3 | `storage_buffer<array<u32>>` block_voxel_count_rw | `storage_buffer_sized(false, None)` | chunkCalc, generatorModel |
| 4 | `storage_buffer_read_only<array<u32>>` segment_voxel_buffer | `storage_buffer_read_only_sized` | chunkCalc |
| 5 | `storage_buffer<array<HashValue>>` hash_map_rw | `storage_buffer_sized(false, None)` | chunkCalc, mapCopy |
| 6 | uniform `ConstructionParams` | `uniform_buffer_sized` | every construction pass |

A second construction layout `construction_bounds_layout` (`@group(1)` for
`boundsCalc`):

| binding | wgsl type | shader |
|---|---|---|
| 0 | `storage_buffer<array<BoundQueueInfo>>` bound_queue_info_rw | boundsCalc |
| 1 | `storage_buffer<array<u32>>` bound_group_queues_rw | boundsCalc |
| 2 | `storage_buffer<array<atomic<u32>>>` bound_group_masks_rw_x3 | boundsCalc, worldChange |
| 3 | `storage_buffer<array<u32>>` bound_refined_info_rw | boundsCalc |
| 4 | `storage_buffer<array<u32>>` bound_dispatch_indirect_rw | boundsCalc (also bound INDIRECT — see split below) |

**The wgpu `STORAGE_READ_WRITE` × `INDIRECT` exclusivity split (D-D in
`12-alignment-gap.md` §3) repeats here.** `bound_dispatch_indirect` is written
by `boundsCalc.prepareGroupBounds` (`:92` — `boundGroupQueueDispatchCount.Store(0, ...)`)
and consumed indirect by the next `boundsCalc.computeGroupBounds` dispatch.
Mirror the Phase-B Batch 4 fix (`sample_refine_dispatch_layout` —
`pipelines.rs:531-540`): put `bound_dispatch_indirect` in its own one-binding
layout used **only** by `bounds_prepare` for writing; the indirect consumer
binds the same buffer as the indirect-args source (no shader binding) on the
`bounds_compute` dispatch.

Third construction layout `construction_change_layout` (`@group(1)` for
`worldChange`):

| binding | wgsl type | shader |
|---|---|---|
| 0 | `storage_buffer_read_only<array<vec2<u32>>>` changed_groups | applyGroupChange |
| 1 | `storage_buffer_read_only<array<vec2<u32>>>` changed_chunks | applyChunkChange |
| 2 | `storage_buffer_read_only<array<u32>>` changed_blocks | applyBlockChange |
| 3 | `storage_buffer_read_only<array<u32>>` changed_voxels | applyVoxelChange |

Fourth construction layout `construction_entity_layout` (`@group(1)` for
`entityUpdate`):

| binding | wgsl type | shader |
|---|---|---|
| 0 | `storage_buffer_read_only<array<vec2<u32>>>` chunk_updates_dynamic | updateChunks |
| 1 | `storage_buffer_read_only<array<EntityChunkInstance>>` entity_chunk_instances_dynamic | copyEntityChunkInstances |
| 2 | `storage_buffer_read_only<array<vec4<u32>>>` entity_history_dynamic | copyEntityHistory |
| 3 | `storage_buffer<array<EntityChunkInstance>>` entity_chunk_instances_rw | updateChunks |
| 4 | `storage_buffer<array<vec4<u32>>>` entity_instances_history_rw | copyEntityHistory |

All four construction layouts are owned by a new `ConstructionPipelines`
sub-resource (`render/construction/mod.rs`), which is registered as a separate
`init_gpu_resource` in `render/mod.rs` alongside the existing
`NaadfPipelines`. `NaadfPipelines` is **not edited** — this is the seam.

### 1.4 `WorldGpu` extension — what new state goes where

`WorldGpu` (`prepare.rs:47-62`) is the production GPU world state. Phase C
extends it with a single new field `ConstructionGpu` (a `Resource` in its own
right, not nested into `WorldGpu`, to keep the seam textbook):

```rust
// render/construction/mod.rs
#[derive(Resource)]
pub struct ConstructionGpu {
    // === Algorithm 1 inputs / outputs (workstream W1) ===
    pub segment_voxel_buffer: GrowableBuffer<u32>,     // chunkCalc.fx:38
    pub block_voxel_count:    Buffer,                  // 2× u32 atomic cursors — chunkCalc.fx:37
    pub hash_map:             GrowableBuffer<HashValue>, // BlockHashingHandler — chunkCalc.fx:39, mapCopy.fx:13
    pub hash_coefficients:    Buffer,                  // 65× u32, NEVER grows — BlockHashingHandler.cs:50

    // === Bound-queue family (workstream W3) ===
    pub bound_queue_info:        Buffer,               // 32*3 × BoundQueueInfo, fixed — WorldBoundHandler.cs:44
    pub bound_group_queues:      Buffer,               // 32*3*boundGroupCount × u32, fixed — :46
    pub bound_group_masks:       Buffer,               // boundGroupCount × Uint3, fixed atomic — :47
    pub bound_refined_info:      Buffer,               // 3 × u32, fixed — :45
    pub bound_dispatch_indirect: Buffer,               // 5 × u32 INDIRECT — :49

    // === Change-staging family (workstream W2) ===
    pub changed_groups_dynamic:  GrowableBuffer<[u32; 2]>,   // ChangeHandler.cs:56
    pub changed_chunks_dynamic:  GrowableBuffer<[u32; 2]>,   // :57
    pub changed_blocks_dynamic:  GrowableBuffer<u32>,        // :58
    pub changed_voxels_dynamic:  GrowableBuffer<u32>,        // :59

    // === Entity family (workstream W4, only allocated if entities enabled) ===
    pub entity_chunk_instances:        Option<GrowableBuffer<GpuEntityChunkInstance>>, // EntityHandler.cs:148
    pub entity_voxel_data:             Option<GrowableBuffer<u32>>,                    // :147
    pub entity_instances_history:      Option<GrowableBuffer<[u32; 4]>>,               // :149
    pub chunk_updates_dynamic:         Option<GrowableBuffer<[u32; 2]>>,               // entityUpdate.fx:3
    pub entity_chunk_instances_dynamic: Option<GrowableBuffer<GpuEntityChunkInstance>>,
    pub entity_history_dynamic:        Option<GrowableBuffer<[u32; 4]>>,
}

#[derive(Resource)]
pub struct ConstructionBindGroups {
    pub construction_world:  BindGroup,
    pub construction_bounds: BindGroup,
    pub construction_change: BindGroup,
    pub construction_entity: Option<BindGroup>,
    pub bound_dispatch:      BindGroup,
}
```

Buffer growth policy follows `GrowableBuffer::reserve` (`world/buffer.rs:96-160`):
factor 2, `copy_buffer_to_buffer` on grow. Initial sizes are taken from NAADF's
constants verbatim (`ChangeHandler.cs:53-55` — 2 M chunks, 2 M blocks, 5 M
voxels — for change buffers; `WorldBoundHandler.cs:44-47` for bounds).
`segment_voxel_buffer` initial size = `(segmentSizeInChunks)³ × 2048`
(`WorldData.cs:73` — 2048 voxels per chunk).

`WorldGpu` itself gains **one** new field: an `Arc<wgpu::Buffer>` clone of the
chunks texture (already exists) and clones of `blocks` / `voxels` buffer
handles so the new construction bind groups can be built against the same
underlying buffers without changing `WorldGpu`'s ownership semantics. Wgpu
buffers are `Arc<>`-backed by `RenderDevice`; cloning a `Buffer` is a refcount
bump, not a copy. **No `WorldGpu` struct member is moved or renamed; the
existing bind group is unaffected.** New behaviour: `chunks` texture's
`TextureUsages` adds `STORAGE_BINDING` (currently `TEXTURE_BINDING | COPY_DST`
— `prepare.rs:160`), so construction can write to it via
`texture_storage_3d<r32uint, read_write>`. This is the **one production
`prepare.rs` change** the seam requires; it is a usage-flag widening, not a
behaviour change for existing readers.

### 1.5 GPU struct registry — `offset_of!` guard discipline

Every new GPU struct adopts the **`offset_of!` guard pattern** established by
`18-taa-fidelity.md` fix #1 (`gpu_types.rs:575-576`). The Phase-B `vec3`-then-scalar
hazard (D-A in `12-alignment-gap.md` §3) recurred three times before the guard
was instituted; Phase C audits every new struct. Full list in §5; here is the
discipline:

- Every new `#[repr(C)]` GPU struct gets a `const _: () = assert!(size_of == N)`
  guard.
- Every field where a `vec3` is followed by a `f32`/`u32` (the hazard pattern)
  gets a `const _: () = assert!(offset_of!(S, field) == N)` and `% 16 == 0`
  guard.
- Every WGSL struct in the construction shaders is annotated with the matching
  offset comment.
- The mechanical-offset-assert harness from `12-alignment-gap.md` B-6 is not
  required for Phase C — the `offset_of!` guard catches the hazard at compile
  time, which is the cheaper equivalent of the runtime harness. (If the
  runtime harness is added in parallel, the seam absorbs it without change.)

### 1.6 CPU-as-oracle integration (E4)

The CPU `aadf::construct::construct(volume: &DenseVolume) -> ConstructedWorld`
(`aadf/construct.rs:128`) is **the validation oracle**. The discipline:

- **Test harness (a new `#[test]` in `aadf/construct.rs`)** that constructs the
  exact `GridPreset::Default` test grid through *both* paths in a headless
  `App` (the existing e2e binary's `build_app(AppConfig::e2e())` already
  provides this), then maps the GPU `blocks` / `voxels` / chunks-texture
  buffers to the CPU and asserts byte-equality with the CPU oracle's
  `chunks` / `blocks` / `voxels` vectors.
- **A `--validate-gpu-construction` flag on `e2e_render`** that runs the same
  byte-equality assertion on the production startup-construction path before
  emitting `AppExit::Success`. This is the load-bearing GPU-side gate: every
  workstream's PR runs `cargo run --bin e2e_render -- --validate-gpu-construction`
  + `cargo test`.
- **Oracle mapping** — which CPU function is the truth for which GPU pass:

| GPU pass (workstream) | CPU oracle function | bit-exact buffer compared |
|---|---|---|
| `chunkCalc.calcBlockFromRawData` (W1) | `construct::construct` (`aadf/construct.rs:128`) | chunks texture + blocks + voxels |
| `chunkCalc.computeVoxelBounds` (W1) | `bounds::compute_aadf` over voxel cells (`aadf/bounds.rs:55`) called from `construct` Phase 3 | voxels (the 2-bit AADFs inside the packed pairs) |
| `chunkCalc.computeBlockBounds` (W1) | `bounds::compute_aadf` over block cells from `construct` Phase 3 | blocks (the 2-bit AADFs) |
| `boundsCalc.computeGroupBounds` (W3) | `bounds::compute_aadf` over chunk cells, 5-bit AADF, `max_dist = 31` | chunks texture (the 5-bit AADFs) — note this is the *converged* state after enough `boundsCalc` frames |
| `worldChange.applyChunkChange` (W2) | a *new* `aadf::edit::apply_chunk_edit_cpu` helper that mirrors the GPU shader's bit-twiddling | chunks texture |
| `worldChange.applyBlockChange` (W2) | a *new* `aadf::edit::apply_block_edit_cpu` helper | blocks |
| `worldChange.applyVoxelChange` (W2) | a *new* `aadf::edit::apply_voxel_edit_cpu` helper | voxels |
| `generatorModel.fillChunkDataWithModelData16` (W5) | a *new* `aadf::generator::generate_segment_cpu` helper | segment_voxel_buffer |
| `entityUpdate.*` (W4) | a *new* `aadf::entity::EntityData::process_cpu` (`EntityHandler.Update` port) | entity_chunk_instances + chunks-texture `.y` channel |

The "new" helpers in W2/W4/W5 are small (each is a CPU translation of one
HLSL pass, no algorithmic novelty); each ships in its workstream as the
GPU pass's test oracle and an optional CPU fallback for environments where
GPU construction is disabled (a config knob — `ConstructionConfig.cpu_fallback`).

### 1.7 Entity-track impact on the seam — pre-emptive chunk widening (decision: NO)

NAADF's entity track widens the chunk 3D texture format from `R32Uint` to
`Rg64Uint` (NAADF: `Rg32Uint`, `settings.fxh:14-18` — `#define CHUNKTYPE uint2`).
This is *the* breaking change to the chunk format; every existing render pass
that reads the chunk texture would have to be updated.

**Decision: the entity track (W4) owns the widening.** Not pre-emptive.

Reasoning:

- Pre-widening adds a `.y = 0` ignore-this-channel discipline to every
  existing read of the chunk texture. WGSL's `textureLoad(chunks, pos, 0)`
  returns a `vec4<u32>` regardless of format (`r32uint` → `.x` only,
  `rg32uint` → `.x` + `.y`), and the existing render shaders read
  `chunks[chunkPos]` (or `textureLoad(chunks, chunkPos).x`) as a scalar.
  Pre-widening forces a sweep of every render-side shader to confirm the read
  pattern still works under the wider format, which means W2 / W3 / W5 / W6
  block on a wholesale render-shader audit before they can land. That makes
  W4 a *prerequisite* to the entire wave plan, eliminating its parallelism.
- Keeping the widening inside W4 means W4 owns its own breaking change:
  `chunks` texture format changes from `R32Uint` to `Rg32Uint`, every
  reader is updated to read `.x` explicitly (one mechanical edit per WGSL
  read site), and the production binding type changes. **This is a
  single-workstream-internal change**, contained behind the seam.
- The entity track is the *one* W where shipping the chunk-widening is
  load-bearing — without it, entities cannot be addressed per-chunk. W2 / W3
  / W5 / W6 each work fine on `R32Uint` chunks (NAADF's `#ifdef ENTITIES`
  branches in `chunkCalc.fx:170-174`, `boundsCalc.fx:140-145`,
  `worldChange.fx:77-81`, etc. are all surrounded by the existing
  single-channel code). The C# code paths the design ports are the
  non-`#ifdef ENTITIES` branches *first*; the entity branches are an
  internal addition by W4.
- The final integration step (§2 below) gates: the W4 PR is **the last** of
  the wave-2 PRs to merge, and its merge contains both the format flip and
  every `.x` read-site edit in one atomic change. Wave-3 work on entities
  builds on that merged state.

What W4 includes that the seam must accommodate but not provide:
- The `chunks` texture `TextureFormat` becomes `Rg32Uint` (verified the wgpu
  format is supported as a `STORAGE_BINDING + TEXTURE_BINDING` texture).
- Every existing `textureLoad(chunks, ...)` read site (`ray_tracing.wgsl`,
  `naadf_first_hit.wgsl`, `naadf_global_illum.wgsl`, `spatial_resampling.wgsl`,
  `naadf_atmosphere.wgsl` if any) gains an explicit `.x` selection.
- The `construction_world_layout`'s `chunks_rw` binding flips from
  `texture_storage_3d<r32uint, read_write>` to
  `texture_storage_3d<rg32uint, read_write>`.
- `WorldGpu`'s chunks texture descriptor (`prepare.rs:158-161`) updates its
  format.

The seam contract: every wave-2 / wave-3 workstream codes against
`R32Uint`-typed chunks; W4 sweeps the `.x` selections and the format flip in
its own merge; subsequent workstreams that depend on entity data (W4-dependent
W in wave 3) inherit the wider format.

### 1.8 Per-frame construction params uniform (`GpuConstructionParams`)

Every construction pass needs a small set of per-frame parameters that NAADF
passes individually through `Effect.Parameters` (verified in
`WorldBoundHandler.cs:97-111`, `ChangeHandler.cs:188-200`). These all collapse
to one uniform:

```rust
// render/construction/mod.rs (Rust)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuConstructionParams {
    pub size_in_chunks:      [u32; 3],  // chunkSizeX/Y/Z
    pub _pad0:               u32,
    pub group_size_in_groups: [u32; 3], // groupSizeX/Y/Z (== sizeInChunks / 4)
    pub _pad1:               u32,
    pub bound_group_queue_max_size: u32, // == boundGroupCount = chunkCount / 64
    pub hash_map_size:       u32,       // current power-of-two hashMap capacity
    pub segment_size_in_chunks: u32,    // 4 (NAADF default — WorldData.cs:73)
    pub max_group_bound_dispatch: u32,  // ConstructionConfig
    pub chunk_offset:        [u32; 3],  // chunkOffsetX/Y/Z (per-segment)
    pub _pad2:               u32,
    pub frame_index:         u32,
    pub changed_chunk_count: u32,
    pub changed_block_count: u32,
    pub changed_voxel_count: u32,
}
// 80 bytes = 5 × 16-byte rows
const _: () = assert!(std::mem::size_of::<GpuConstructionParams>() == 80);
const _: () = assert!(std::mem::offset_of!(GpuConstructionParams, frame_index) == 64);
// (No vec3-then-scalar hazard: every 3-tuple is explicitly padded to 16 bytes
// at the Rust level.)
```

This goes in `gpu_types.rs` (the existing pattern — every GPU struct lives
together), but its WGSL counterpart lives in `construction/mod.rs`'s WGSL
import surface.

---

## 2. Workstream decomposition + wave plan

Seven workstreams, three waves. Wave 1 is two independent foundational tracks
(generator infra + Algorithm 1) that can start the day Phase C opens. Wave 2
is the bulk of the work — five tracks that fan out behind the seam, three of
which depend on Wave 1's Algorithm-1 deliverable. Wave 3 is the integration
step.

Naming convention: workstream `Wn` ↔ branch `feat/phase-c-Wn-<slug>` ↔ worktree
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-c-Wn-<slug>/`. All
branched from local `main` at the Phase-C-start commit. Worktree usage is
mandatory — code-mutating parallel workstreams **only** safe because each is in
a separate worktree (§01-context §2e E3, the user's explicit override of the
serialise-code-mutations rule).

### 2.1 The workstream table

| workstream | wave | deps | scope | branch / worktree | touched files | verification gate |
|---|---|---|---|---|---|---|
| **W0 — Seam construction** (`render/construction/` skeleton + `ConstructionPipelines` + `ConstructionGpu` empty shell + `prepare_construction` + the `prepare_construction` placement in `render/mod.rs` + `GpuConstructionParams` + the chunks-texture `STORAGE_BINDING` usage-flag widening + the e2e `--validate-gpu-construction` flag plumbing) | **1** | none | small | `feat/phase-c-W0-seam` / `phase-c-W0-seam` | NEW: `render/construction/mod.rs`, `config.rs`. EDIT: `render/mod.rs` (one mod import, one resource init, one node insert site, one prepare-system registration); `render/gpu_types.rs` (add `GpuConstructionParams`); `render/prepare.rs` (chunks texture usage flag widening — `:160`); `Cargo.toml` (no change); `lib.rs` (add `taa_ring_depth`-style `AppArgs.construction_config` plumbing if needed); `src/bin/e2e_render.rs` (the `--validate-gpu-construction` flag, default off). | `cargo build` + `cargo test` + `cargo run --bin e2e_render` exits 0 (image unchanged — empty construction sub-module is a no-op). |
| **W5 — World generator** (`generatorModel.fx` → `generator_model.wgsl`; ports paper-gap item #6 + the Wave-1 input to W1's GPU build) | **1** | W0 | medium | `feat/phase-c-W5-generator` / `phase-c-W5-generator` | NEW: `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl`; `render/construction/generator_model.rs`; `aadf/generator.rs` (the CPU oracle `generate_segment_cpu`); a `voxel/grid.rs`-equivalent `ModelData` consumer trait. EDIT: `render/construction/mod.rs` (add the pipeline + node registration); `render/pipelines.rs::ConstructionPipelines` (add `generator_model_pipeline`). | `cargo test` (new CPU-oracle `#[test]` + GPU-vs-CPU buffer-bit equality on a small `ModelData`); `cargo run --bin e2e_render -- --validate-gpu-construction` passes. |
| **W6 — O(3·d·n) AADF rewrite** (`bounds.rs::compute_aadf` → synchronised-iteration neighbour-merge; ports paper-gap item #5) | **1** | none (CPU only) | medium | `feat/phase-c-W6-aadf-merge` / `phase-c-W6-aadf-merge` | EDIT: `crates/bevy_naadf/src/aadf/bounds.rs` (replace the per-cell expansion with the neighbour-merge algorithm — keep the existing `compute_aadf` signature, swap the body); add a `compute_aadf_layer` batched-form that produces all AADFs for a layer in one O(3·d·n) pass for use by `construct.rs`; expand the existing 5-test suite to cover the same scenarios under the new algorithm. EDIT: `aadf/construct.rs` Phase 3 (lines ~190-280) to call `compute_aadf_layer` once per layer instead of `compute_aadf` per cell. | `cargo test` (all existing `bounds.rs` tests + `construct.rs` tests stay green — same outputs, faster); a new `#[test] bench_construction_speedup` that constructs a 16³ chunk grid and asserts the new path is ≥10× faster than the legacy per-cell path (NAADF's claim is "O(3·d·n) linear" — the old path is O(d² · n) effectively). |
| **W1 — GPU Algorithm 1** (`chunkCalc.fx` → `chunk_calc.wgsl` with the three entry points + `mapCopy.fx` → `map_copy.wgsl` + the `BlockHashingHandler` Rust port + the startup regime-1 dispatch; ports paper-gap item #1) | **2** | W0, W5 | **large** | `feat/phase-c-W1-algorithm1` / `phase-c-W1-algorithm1` | NEW: `assets/shaders/chunk_calc.wgsl` (3 entry points + the `GetVoxelPointer` open-addressing function); `assets/shaders/map_copy.wgsl` (2 entry points); `render/construction/chunk_calc.rs`; `render/construction/map_copy.rs`; `render/construction/hashing.rs` (the 65 `31^(64-i)` coefficient table + occupancy tracker); the startup-regime-1 driver system `run_gpu_construction_startup` in `render/construction/mod.rs`. EDIT: `pipelines.rs::ConstructionPipelines` (5 new pipelines + 2 layouts already declared by W0); `render/mod.rs` (none — the startup driver is a one-shot `Startup` system, not in the chain). | `cargo test` (the new GPU-vs-CPU bit-exact test on `GridPreset::Default`); `cargo run --bin e2e_render -- --validate-gpu-construction` passes; the existing `e2e_render` gates (`emissive > 120`, `solid > 150`, `sky ∈ [10,230]`) stay green — the producer changed, the renderer did not. |
| **W3 — Background AADF queue** (`boundsCalc.fx` → `bounds_calc.wgsl`; ports paper-gap item #3) | **2** | W0, W1 (consumes the `chunks` buffer W1 produces) | medium | `feat/phase-c-W3-bounds-queue` / `phase-c-W3-bounds-queue` | NEW: `assets/shaders/bounds_calc.wgsl` (3 entry points: addInitialGroupsToBoundQueue, prepareGroupBounds, computeGroupBounds); `render/construction/bounds_calc.rs` (the `naadf_bounds_prepare_node` + `naadf_bounds_compute_node` regime-2 systems, 5-rounds-per-frame loop). EDIT: `pipelines.rs::ConstructionPipelines` (3 new pipelines); `render/mod.rs` (one `.chain()` insert — the new node placed before `naadf_atmosphere_node`). | `cargo test` (CPU-converged-state vs GPU-after-N-frames bit-exact comparison — run the bounds queue in the e2e harness for enough frames that all chunks have converged, then map and assert); `cargo run --bin e2e_render` exits 0; the e2e gates stay green. |
| **W2 — Editing + flood-fill invalidation** (`worldChange.fx` → `world_change.wgsl` + the CPU flood-fill `ChangeHandler` port; ports paper-gap item #2) | **2** | W0, W1, W3 (consumes the bounds queue infra) | **large** | `feat/phase-c-W2-editing` / `phase-c-W2-editing` | NEW: `assets/shaders/world_change.wgsl` (4 entry points: applyGroupChange, applyChunkChange, applyBlockChange, applyVoxelChange); `render/construction/world_change.rs` (the on-edit-event regime-3 node); `render/construction/change_handler.rs` (the CPU flood-fill — `ChangeHandler.UpdateWorld` port); `aadf/edit.rs` (the CPU oracles for the 4 worldChange passes + the `EditingHandler.processChunks` port — per-chunk re-hash + the `changed*` array fill). EDIT: `pipelines.rs::ConstructionPipelines` (4 new pipelines); `render/mod.rs` (one `.chain()` insert after the bounds node, gated on the edit-event flag); main-world `WorldData` gains a `pub fn set_voxel(&mut self, pos: IVec3, ty: VoxelTypeId)` programmatic-edit entry point (called by an extended e2e harness — see verification). | `cargo test` (oracle equality after one edit; oracle equality after a flood-fill that traverses a 63³-chunk volume — verifies the BFS distance propagation matches the C#); `cargo run --bin e2e_render -- --edit-mode` exits 0 (new mode: warmup → apply a scripted edit → render N more frames → screenshot, assert the edited region appears in the screenshot via a known-rectangle gate). |
| **W4 — Dynamic entities** (`entityUpdate.fx` → `entity_update.wgsl` + the per-chunk entity pointer + entity instance buffer + per-entity AADF voxel volumes + the `EntityHandler` CPU port + the chunk-format `Rg32Uint` widening + the `shoot_ray` entity sub-traversal + `.x` selection sweep on every chunk-read site; ports paper-gap item #4 — paper contribution #4 in full) | **2** | W0, W1 (independent of W2/W3 — entities are *traversal-time* additions, not editing) | **large** | `feat/phase-c-W4-entities` / `phase-c-W4-entities` | NEW: `assets/shaders/entity_update.wgsl` (3 entry points: updateChunks, copyEntityChunkInstances, copyEntityHistory); `render/construction/entity_update.rs`; `render/construction/entity_handler.rs` (the `EntityHandler.Update` CPU port — overlap counting + prefix-sum + dedup-hash); `aadf/entity.rs` (`EntityData` CPU build of per-entity AADF voxel volume + `compress_quaternion` smallest-three encoding port); `render/gpu_types.rs::GpuEntityChunkInstance` + `GpuEntityInstance` + offset guards. EDIT: `render/prepare.rs:158-161` (chunk texture format `R32Uint` → `Rg32Uint`); every WGSL shader that reads `chunks` (`ray_tracing.wgsl`, `naadf_first_hit.wgsl`, `naadf_global_illum.wgsl`, `spatial_resampling.wgsl`, the construction shaders from W1/W3 — explicit `.x` access on every read); `assets/shaders/ray_tracing.wgsl` adds the entity sub-traversal branch (the `#ifdef ENTITIES` path from `rayTracing.fxh`); `ConstructionConfig.entities_enabled`. | `cargo test` (entity-instance CPU/GPU bit-equality; per-entity AADF voxel volume CPU/GPU bit-equality; chunk-format read regression — every existing render-side bit-exact test stays green under the new format); `cargo run --bin e2e_render -- --entities` exits 0 (new mode: scene includes one moving entity, gates check it appears in the screenshot at a known position). |
| **Final integration step** (the merge agent) | **3** | every Wn | small (single thin sweep) | back to `main` | one merge per workstream branch, in **dependency order** (W0, W5/W6 in parallel, W1, W3, W4, W2 — see §2.2 ordering); after each merge: `cargo build`, `cargo test`, `cargo run --bin e2e_render`. Final-step touches: `docs/orchestrate/naadf-bevy-port/16-impl-c.md` (a Phase-B-style impl log skeleton committed empty so each workstream's impl agent appends to it on merge — same convention as `04/07/10-impl*.md`); `README.md` checklist; `RESUME.md` (post-Phase-C entry). | Full `cargo test` (every workstream's `#[test]`s + the new GPU/CPU oracle tests pass on `main`); `cargo run --bin e2e_render -- --validate-gpu-construction` exits 0; `cargo run --bin e2e_render -- --edit-mode` exits 0; `cargo run --bin e2e_render -- --entities` exits 0; a fresh-eyes `delegate-reviewer` pass + user interactive confirmation. |

### 2.2 Wave + dependency rationale

- **Wave 1 (parallel, 3 worktrees concurrent):** W0 is the smallest seam-only
  PR — it lands first as a single fast review. W5 (worldgen) and W6 (the AADF
  rewrite) are independent of each other and of every other Phase-C
  workstream's *code surface*: W6 is pure-CPU `aadf/bounds.rs`, W5 is a
  greenfield WGSL + a greenfield `generator_model.rs` file. They can land in
  any order after W0. The critical-path is W0 → (W5 ‖ W6).
- **Wave 2 (parallel, 4 worktrees concurrent):** W1 is **the foundational
  Wave-2 track** — it produces the GPU chunks/blocks/voxels that W3 and W2 read.
  W3 and W2 depend on W1's merge; W4 is independent of W2/W3 (entities are
  traversal-time, not editing-time) but depends on W1 to know the chunk
  format. **The intra-Wave-2 critical path is W1 → (W3 ‖ W4) → W2** because
  W2's flood-fill re-enqueues into the bound queues W3 owns. (The C#
  `ChangeHandler.UpdateWorld` writes to `boundQueueInfo` / `boundGroupQueues`
  directly — `:197-200`.) So W3 must land before W2.
- **Wave 3 (serial, 1 merge agent):** the final integration step. Sequenced
  merges of every Wn into `main`, with `cargo test` + `cargo run --bin e2e_render`
  after each. Order: W0 → W5 → W6 → W1 → W3 → W4 → W2. The W4 merge is the
  one with cross-cutting render-side edits (the chunk-format widening's `.x`
  sweep); it lands *between* W3 and W2 so that W2's worldChange compute
  shaders can be written to the `Rg32Uint` format from the start. The
  alternative — W4 last — would force a follow-up sweep of W2's just-merged
  WGSL.

### 2.3 The final integration step in detail

The merge agent runs:

1. Fetch each workstream branch, run `cargo build` + `cargo test` against
   `main` HEAD before any merge.
2. For each workstream in the §2.2 order:
   - `git checkout main && git merge --no-ff feat/phase-c-Wn-<slug>` in a
     worktree dedicated to integration
     (`.claude/worktrees/phase-c-integration/`, branched from `main`).
   - Resolve any conflict — by §1 the seam contract is that conflicts should
     only occur in the construction sub-module's mod-file (`render/construction/mod.rs`'s
     bind-group / pipeline / node registrations) and in `render/mod.rs`'s
     `.chain()` (one new node per workstream). Any conflict outside these
     two files is an unexpected-shared-edit and triggers a halt + escalation
     to the orchestrator.
   - Run `cargo build`, `cargo test`, `cargo run --bin e2e_render -- --validate-gpu-construction`,
     and on the final two merges (W4, W2) also `cargo run --bin e2e_render -- --entities`
     and `cargo run --bin e2e_render -- --edit-mode`.
   - Append a short merge-summary section to `16-impl-c.md`.
3. After the final merge, run the full test+e2e suite once more and emit a
   PR-ready summary. No production code is written by this agent; it is a
   merger + tester.

### 2.4 Risk inventory

**R1 — `WorldGpu`'s chunks texture format flip (W4's widening) breaks every
existing render shader that reads the chunks texture.** Mitigation: W4 owns
the entire `.x`-selection sweep in its own atomic merge (§1.7). The risk
materialises only if a wave-2 workstream other than W4 forgets the seam
contract and tries to widen the chunk texture inline — the merge agent guards
this by gating any chunk-texture-format change *only* on W4's branch.

**R2 — W1 → W3 ordering breaks if W1's bit-exact CPU/GPU oracle reveals a
construction bug late in the merge sequence.** Mitigation: W1 carries the
GPU-vs-CPU bit-exact `#[test]` *and* the `--validate-gpu-construction` e2e
flag, so the bug surfaces inside W1's own PR — not at the integration step.
The e2e flag is the §1.6 oracle: it MUST exit 0 on W1's merge before any
wave-2 workstream merges. If the bit-exact test fails, the merge agent halts
and re-dispatches W1's impl agent.

**R3 — The bound-queue family grows beyond the buffer ceiling W3 allocates,
silently overwriting queue entries.** Mitigation: NAADF runs a strict
fixed-size queue (`32 * 3 * boundGroupCount` — `WorldBoundHandler.cs:46`)
because the chunk count is fixed; the same fixed allocation is correct in
the port for a fixed world size. W3's `#[test]` asserts the queue never
overruns on `GridPreset::Default` (16³ chunks → boundGroupCount = 64 →
queue size = 6 144 u32s — well below the `max_buffer_size` ceiling). The
risk only matters if a future workstream extends to dynamic world resizing
(out of scope per E1; the port keeps the fixed test grid).

---

## 3. Render-graph extension diagram

Existing `Core3d` chain (`render/mod.rs:223-244`, 14 nodes):

```
[atmosphere] → [first_hit] → [taa_reproject] → [sample_refine_clear]
→ [ray_queue] → [global_illum] → [sample_refine_valid_history]
→ [sample_refine_count_valid] → [sample_refine_count_invalid]
→ [sample_refine_buckets] → [spatial_resampling] → [denoise]
→ [calc_new_taa_sample] → [final_blit]
```

Phase C extends it with **two new construction nodes inserted at the top of
the chain**, before any render work, and a separate **Startup-schedule
construction pipeline** that runs once before the first render frame:

```
                                    ┌─────────────────────────────────┐
                                    │  STARTUP SCHEDULE (one-shot)    │
                                    │                                 │
                                    │  run_gpu_construction_startup:  │
                                    │    1. generator_model           │
                                    │       (per segment, W5)         │
                                    │    2. chunk_calc.calcBlock      │
                                    │       FromRawData (W1)          │
                                    │    3. chunk_calc.computeVoxel   │
                                    │       Bounds (W1)               │
                                    │    4. chunk_calc.computeBlock   │
                                    │       Bounds (W1)               │
                                    │    5. bounds_calc.addInitial    │
                                    │       GroupsToBoundQueue (W3)   │
                                    │    6. (optional) chunk_copy_to_ │
                                    │       cpu — entities only (W4)  │
                                    │  + GPU/CPU oracle assert        │
                                    └────────────────┬────────────────┘
                                                     │
                                                     ▼
            ┌──────────────────  Core3d render schedule (every frame)  ──────────────────┐
            │                                                                            │
            │  [bounds_compute  ◄── W3, regime-2: 5 prepare+indirect_compute rounds]     │
            │           │           gated by ConstructionConfig.max_group_bound_dispatch │
            │           ▼                                                                │
            │  [world_change   ◄── W2, regime-3: 4 dispatches gated on edit events]      │
            │           │           gated by ConstructionEvents.has_pending_changes()    │
            │           ▼                                                                │
            │  [entity_update  ◄── W4, regime-3: 3 dispatches gated on entity events]    │
            │           │           gated by ConstructionConfig.entities_enabled         │
            │           ▼                                                                │
            │  [atmosphere] → [first_hit] → [taa_reproject] → [sample_refine_clear]      │
            │       → [ray_queue] → [global_illum] → [sample_refine_valid_history]       │
            │       → [sample_refine_count_valid] → [sample_refine_count_invalid]        │
            │       → [sample_refine_buckets] → [spatial_resampling] → [denoise]         │
            │       → [calc_new_taa_sample] → [final_blit]                               │
            └────────────────────────────────────────────────────────────────────────────┘
```

Why insert *before* atmosphere, not after `final_blit`:
- The bounds queue should run with the *previous* frame's chunk state already
  observable to atmosphere/first-hit — but the AADFs are only used by ray
  traversal, which sits inside atmosphere/first-hit/GI. Running construction
  before atmosphere means the first frame after a construction update is the
  frame the new AADFs are visible. (NAADF runs it in `Update`, before
  `RenderInternal` — `WorldData.cs:114-117` then `WorldRenderBase.cs:205`.)
- It also means the construction nodes can be skipped cheaply on no-edit
  frames without affecting downstream render ordering.

Edges in `render/mod.rs::add_systems(Core3d, …)`:

```rust
// New chain (with the Phase-C insertions at the top):
.add_systems(
    Core3d,
    (
        // Phase-C construction nodes (regime-2 + regime-3) — run before any
        // render work so atmosphere/first-hit see the up-to-date chunk state.
        naadf_bounds_compute_node,       // W3
        naadf_world_change_node,         // W2 (gated on edit events)
        naadf_entity_update_node,        // W4 (gated on entities_enabled)
        // Existing 14 nodes — unchanged.
        naadf_atmosphere_node,
        naadf_first_hit_node,
        naadf_taa_reproject_node,
        // ... (the existing 12 — verbatim from render/mod.rs:226-239)
        naadf_final_blit_node,
    )
        .chain()
        .in_set(Core3dSystems::PostProcess)
        .before(tonemapping),
);
```

That is the **entire `render/mod.rs` edit** for Phase C (modulo the imports +
`prepare_construction` registration). 3 lines of new node names in the
`.chain()` tuple, prepended.

The startup-schedule one-shot construction lives in
`construction::run_gpu_construction_startup` — a `Startup` system in the
**main** app (not the render sub-app, because it owns its own command-encoder
submission against `RenderDevice` and runs synchronously). It is wired by
`construction::Plugin::build`:

```rust
// render/construction/mod.rs
pub struct ConstructionPlugin;
impl Plugin for ConstructionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, run_gpu_construction_startup);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return; };
        render_app
            .init_gpu_resource::<ConstructionPipelines>()
            .add_systems(
                Render,
                prepare_construction.in_set(RenderSystems::PrepareResources),
            );
        // The Core3d chain insert is in render/mod.rs — kept there so the
        // single .chain() tuple is the entire chain.
    }
}
```

`main.rs` adds the plugin alongside `NaadfRenderPlugin`.

---

## 4. Per-shader detail

Five new WGSL files. Every entry point, `numthreads` value, and binding access
mode was verified against the NAADF HLSL on disk.

### 4.1 `chunk_calc.wgsl` — W1

- **C# source:** `Content/shaders/world/data/chunkCalc.fx` (265 lines, verified).
- **Entry points** (3 + 1):

| WGSL entry | `numthreads` | C# entry | role |
|---|---|---|---|
| `calc_block_from_raw_data` | `(4,4,4)` | `calcBlockFromRawData` (:117-181) | Algorithm 1: per-block hash, dedup via `get_voxel_pointer`, groupshared `is_all_blocks_equal`, atomic `block_voxel_count[1]` append, write `chunks[chunkPos]` + (if mixed) `blocks[insertBlockIndex+localIndex]`. |
| `compute_voxel_bounds` | `(64,1,1)` | `computeVoxelBounds` (:193-217) | Voxel-layer 2-bit AADFs via the groupshared `ComputeBounds4` 3-iteration loop; reads `cachedCell[64]`. |
| `compute_block_bounds` | `(64,1,1)` | `computeBlockBounds` (:219-241) | Block-layer 2-bit AADFs via the same `ComputeBounds4` loop. |
| `chunk_copy_to_cpu` | `(64,1,1)` | `chunkCopyToCpu` (:183-191) | GPU→CPU sync of a chunk-range to a flat staging buffer. Only used when entities are enabled; W4 may consume / W1 ships the shader but the dispatch is W4's. |

- **Bindings:** `@group(0)` = `construction_world_layout` (chunks_rw, blocks_rw,
  voxels_rw, block_voxel_count_rw, segment_voxel_buffer_ro, hash_map_rw,
  construction_params_uniform). No `@group(1)`.
- **Dispatch shape:**
  - `calc_block_from_raw_data`: `dispatch(segmentSizeInChunks, segmentSizeInChunks, segmentSizeInChunks)` — one workgroup per chunk in the segment. For the 16³-chunk test grid with `segment_size_in_chunks=4`, 64 dispatches of `(4,4,4)` groups (NAADF runs a per-segment loop —
    `WorldData.cs:136-156`).
  - `compute_voxel_bounds`: `dispatch(block_count, 1, 1)` — one workgroup per
    block; per-thread one voxel.
  - `compute_block_bounds`: `dispatch(chunk_count, 1, 1)` — one workgroup per
    chunk; per-thread one block.
- **Correctness notes:**
  - The **`GetVoxelPointer` open-addressing loop** (`chunkCalc.fx:57-115`)
    uses HLSL `InterlockedCompareExchange` with a 250-probe cap; WGSL has
    `atomicCompareExchangeWeak` which returns a `__atomic_compare_exchange_result`
    struct (`{ old_value: u32, exchanged: bool }`). The port reads
    `.old_value` and `.exchanged` separately. The `0x80000000`-tagged
    pending-pointer busy-wait (`:88-92`) translates: read `atomicLoad`, loop
    while `(loaded & 0x80000000u) != 0u` with a 2000-iteration cap.
  - **`HashValue` struct** must use the WGSL `atomic<u32>` discipline: only
    the `voxel_pointer` field is the atomic CAS target; `use_count` is
    `atomic<u32>` (atomicAdd at `:72`); `hash_raw` is plain `u32` (written
    after the slot is claimed). `13-reuse-audit-c.md` §borderline calls
    flagged this; WGSL forces declaring the buffer as
    `array<HashValueSlot>` where `struct HashValueSlot { voxel_pointer: atomic<u32>, use_count: atomic<u32>, hash_raw: u32 }`.
  - The **probe-cap deviation** (paper §3.2: 100 / 75 %; C#: 250 / 50 %) is
    a CPU-config-knob in `ConstructionConfig`. Default: NAADF's values (250
    probes, 50 % occupancy `wanted_empty_ratio`) — faithful per Q3.
  - The HLSL `#ifdef ENTITIES` branch at `:170-174` (`chunks[chunkPos] = uint2(state, 0)` vs `chunks[chunkPos] = state`) is omitted on the `R32Uint` format; W4 swaps it for the `vec2<u32>(state, 0u)` write when the format flips to `Rg32Uint`.

### 4.2 `bounds_calc.wgsl` — W3

- **C# source:** `Content/shaders/world/data/boundsCalc.fx` (209 lines, verified).
- **Entry points** (3):

| WGSL entry | `numthreads` | C# entry | role |
|---|---|---|---|
| `add_initial_groups_to_bound_queue` | `(64,1,1)` | `addInitialGroupsToBoundQueue` (:39-48) | One-shot seed of every 4³-chunk group into the size-0 X/Y/Z queues. Called in regime-1 startup. |
| `prepare_group_bounds` | `(1,1,1)` | `prepareGroupBounds` (:51-93) | Single-thread queue picker: scans `bound_queue_info[0..32*3]` for a non-empty queue, picks up to `max_group_bound_dispatch` work-items, writes the slice into `bound_refined_info`, advances the queue start, writes the indirect-dispatch count into `bound_dispatch_indirect[0]`. |
| `compute_group_bounds` | `(4,4,4)` | `computeGroupBounds` (:118-193) | Per-group: 64 chunks processed in parallel; per-chunk, expand its 5-bit AADF by one cell along the queue's axis (via `add_bounds_group` + `check_matching_bounds` from a ported `bounds_common.wgsl` include); thread-0 re-enqueues the group into the next-bound-size queue. |

- **Bindings:** `@group(0)` = `construction_world_layout` (only needs `chunks_rw`
  + the params uniform — but the layout is shared with chunk_calc / world_change
  for bind-group reuse, so the other binding slots are present but unused by
  the entry point); `@group(1)` = `construction_bounds_layout`
  (bound_queue_info_rw, bound_group_queues_rw, bound_group_masks_rw,
  bound_refined_info_rw); `bound_dispatch_indirect` is on a third one-binding
  layout `bound_dispatch_indirect_layout` consumed only by `prepare_group_bounds`
  (the wgpu `STORAGE_READ_WRITE`×`INDIRECT` split, §1.3).
- **Dispatch shape:**
  - regime-1: `dispatch(boundGroupCount / 64, 1, 1)` over
    `add_initial_groups_to_bound_queue`.
  - regime-2 per round (5× per frame): `dispatch(1,1,1)` for
    `prepare_group_bounds`, then `dispatch_indirect(bound_dispatch_indirect, 0)`
    for `compute_group_bounds`.
- **Correctness notes:**
  - The **per-axis `bound_group_masks` atomic** (`boundsCalc.fx:135` —
    `boundGroupMasks[groupIndex][boundXYZ] &= ~(1 << boundSize)`) is a
    `vec3<u32>` storage with per-component atomic access. WGSL forbids
    `atomic<vec3<u32>>` directly; the port stores it as 3 separate
    `atomic<u32>` arrays indexed by axis (or a single `array<atomic<u32>>` of
    length `groupCount * 3` indexed `groupIndex * 3 + axis`). Mechanical
    translation; verified the C# only ever atomically updates one axis at a
    time at any call site (`:135`, `:179` — `boundGroupMasks[groupIndex][boundXYZ]`).
  - `addBoundsGroup` (`boundsCalc.fx:95-116`) reads a neighbour chunk's
    `chunks[neighbourChunkPos]` and does the `checkMatchingBounds` 5-bit AADF
    inequality test, then conditionally increments the current chunk's AADF
    in the queue's axis. Ported function-by-function into `bounds_common.wgsl`
    (the WGSL counterpart of `boundsCommon.fxh` — needed by W1 and W2 too;
    landed in the W1 PR).
  - The `frame_index` decrement in `WorldBoundHandler.Update` (`:96`) is
    just a counter for diagnostics — port as a uniform write, not used by
    the shader's correctness.

### 4.3 `world_change.wgsl` — W2

- **C# source:** `Content/shaders/world/data/worldChange.fx` (191 lines, verified).
- **Entry points** (4):

| WGSL entry | `numthreads` | C# entry | role |
|---|---|---|---|
| `apply_group_change` | `(4,4,4)` | `applyGroupChange` (:37-113) | Per chunk in a 4³ group: reset the chunk's 5-bit AADF to the change-distance the flood-fill assigned, re-enqueue the group into the right size of the bound queue. |
| `apply_chunk_change` | `(64,1,1)` | `applyChunkChange` (:115-128) | Apply a CPU-staged chunk-cell edit (the `changedChunks` buffer): write `chunks[chunkPos] = change.y`. |
| `apply_block_change` | `(4,4,4)` | `applyBlockChange` (:130-147) | Apply a CPU-staged 64-block edit: write 64 blocks at `changedBlocks[groupID.x*65]` base, recompute the local 4³ AADF via `ComputeBounds4`. |
| `apply_voxel_change` | `(4,4,4)` | `applyVoxelChange` (:149-168) | Apply a CPU-staged 64-voxel edit: write 32 `uint`s of packed voxels, recompute the local 4³ AADF via `ComputeBounds4`. |

- **Bindings:** `@group(0)` = `construction_world_layout` (chunks_rw, blocks_rw,
  voxels_rw, params); `@group(1)` = `construction_change_layout`
  (changed_groups_ro, changed_chunks_ro, changed_blocks_ro, changed_voxels_ro);
  `@group(2)` = `construction_bounds_layout` (`apply_group_change` reads the
  bound-queue infra to re-enqueue).
- **Dispatch shape:**
  - `apply_chunk_change`: `dispatch((changedChunkCount + 63) / 64, 1, 1)`.
  - `apply_block_change`: `dispatch(changedBlockCount, 1, 1)` — one workgroup
    per edited 64-block chunk.
  - `apply_voxel_change`: `dispatch(changedVoxelCount, 1, 1)` — one
    workgroup per edited 64-voxel block.
  - `apply_group_change`: `dispatch(changedGroupCount, 1, 1)` — one workgroup
    per 4³-chunk group flagged by the flood-fill.
- **Correctness notes:**
  - `apply_group_change`'s `lowestBoundsShared[3]` groupshared array
    (`worldChange.fx:35`) is an `array<atomic<u32>, 3>` in WGSL, with
    `atomicMin` ops at `:86-88`.
  - The CPU flood-fill (`ChangeHandler.UpdateWorld` — `ChangeHandler.cs:69-255`)
    has **two distinct loops**: the BFS-expand (`:73-110`) over the 27-cell
    neighborhood, and the 7-round `addBounds` propagation
    (`:124-174`) that steps distances by 4. Both are ported into
    `change_handler.rs::compute_change_groups` (CPU). The output is the
    `changedGroupsWithDist` buffer's `Uint2` layout: `(pos.x | y<<11 | z<<21, distance)`.
  - The `changedChunks` / `changedBlocks` / `changedVoxels` buffer formats
    are NAADF-specific: `changedChunks` is `Uint2[]` of `(chunkPos, newCellValue)`;
    `changedBlocks` is `uint[]` of `(pointer, 64 packed block uints)` per edit
    (`:133-135`); `changedVoxels` is `uint[]` of `(pointer, 32 packed voxel uints)`
    per edit (`:152-154`). The CPU `EditingHandler.processChunks`
    (`EditingHandler.cs:75-249`) is the canonical source — port verbatim into
    `aadf/edit.rs::process_edit_batch`.

### 4.4 `map_copy.wgsl` — W1

- **C# source:** `Content/shaders/world/data/mapCopy.fx` (70 lines, verified).
- **Entry points** (2):

| WGSL entry | `numthreads` | C# entry | role |
|---|---|---|---|
| `copy_map` | `(64,1,1)` | `copyMap` (:19-43) | Linear-probe re-hash every occupied slot of `old_map` into the larger `new_map`, max 50 probes. |
| `test_hash` | `(1,1,1)` | `testHash` (:45-57) | Compute the 64-voxel hash on the CPU-staged `voxelsToHash[32]`, write to `resultHash[0]`. Used by the CPU-side hash sanity check; not used in the production startup path. |

- **Bindings:** A `map_copy_layout` `@group(0)`: `old_map_ro`, `new_map_rw`,
  `params_uniform` (carries `old_size` + `new_size`); plus `hash_coefficients_uniform`
  and `voxels_to_hash_uniform` for `test_hash`.
- **Dispatch shape:** `copy_map`: `dispatch((oldSize + 63) / 64, 1, 1)`. Run
  whenever the CPU-side `BlockHashingHandler` (Rust: `hashing.rs`) detects
  occupancy > `wanted_empty_ratio` * `mapSize`.
- **Correctness notes:** WGSL has no equivalent of HLSL's `InterlockedCompareExchange`
  with the "old value" out-param under one call — but `atomicCompareExchangeWeak`
  returns `{ old_value, exchanged }`. Same pattern as W1's `get_voxel_pointer`.

### 4.5 `generator_model.wgsl` — W5

- **C# source:** `Content/shaders/world/generator/generatorModel.fx` (80 lines, verified).
- **Entry points** (1):

| WGSL entry | `numthreads` | C# entry | role |
|---|---|---|---|
| `fill_chunk_data_with_model_data_16` | `(4,4,4)` | `fillChunkDataWithModelData16` (:54-72) | Per chunk: 32 iterations × 2 voxels per iteration → 64 voxels = 32 packed `uint`s into `chunk_data[group_index * 2048 + local_index * 32 + i]`. |

- **Bindings:** `generator_model_layout` `@group(0)`: `chunk_data_rw`
  (== `segment_voxel_buffer`), `model_data_chunk_ro`, `model_data_block_ro`,
  `model_data_voxel_ro`, `params_uniform` (carries the world/model sizes +
  offsets).
- **Dispatch shape:** `dispatch(groupSizeInChunks.x, groupSizeInChunks.y, groupSizeInChunks.z)` —
  one workgroup per chunk in the segment.
- **Correctness notes:** The HLSL `modelIndexY > 0 ⇒ type = 0` clamp
  (`generatorModel.fx:48-49`) is a one-tile-vertical limit; port verbatim.

### 4.6 `entity_update.wgsl` — W4

- **C# source:** `Content/shaders/world/data/entityUpdate.fx` (60 lines, verified).
- **Entry points** (3):

| WGSL entry | `numthreads` | C# entry | role |
|---|---|---|---|
| `update_chunks` | `(64,1,1)` | `updateChunks` (:15-24) | Apply per-chunk entity-pointer + counter updates: `chunks[chunkPos] = vec2<u32>(chunks[chunkPos].x, update.y)`. Requires the `Rg32Uint` chunk format. |
| `copy_entity_chunk_instances` | `(64,1,1)` | `copyEntityChunkInstances` (:26-33) | Bulk copy the per-frame `entity_chunk_instances_dynamic` upload buffer into the GPU `entity_chunk_instances` buffer. |
| `copy_entity_history` | `(64,1,1)` | `copyEntityHistory` (:35-42) | Write one slot of the entity history ring (`taa_index * 16384` base, indexed by `entityInstanceID`). |

- **Bindings:** `@group(0)` = `construction_world_layout` (chunks_rw + params);
  `@group(1)` = `construction_entity_layout`. 16384 is `WorldRender.cs:88`'s
  per-frame entity-instance cap; port as a `ConstructionConfig` constant.
- **Dispatch shape:**
  - `update_chunks`: `dispatch((updateCount + 63) / 64, 1, 1)`.
  - `copy_entity_chunk_instances`: `dispatch((entityChunkInstanceCount + 63) / 64, 1, 1)`.
  - `copy_entity_history`: `dispatch((entityInstanceCount + 63) / 64, 1, 1)`.

---

## 5. GPU struct registry

All new structs follow the `offset_of!` guard discipline (§1.5). Sizes are in
bytes.

### 5.1 `GpuConstructionParams` (80 B = 5×16)

Field-by-field layout (defined in §1.8). No `vec3`-then-scalar hazard: every
3-tuple is explicitly padded to 16 bytes at the Rust level.

Guards:
```rust
const _: () = assert!(std::mem::size_of::<GpuConstructionParams>() == 80);
const _: () = assert!(std::mem::offset_of!(GpuConstructionParams, group_size_in_groups) == 16);
const _: () = assert!(std::mem::offset_of!(GpuConstructionParams, frame_index) == 64);
```

### 5.2 `GpuHashValue` (12 B = 3×4)

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuHashValue {
    pub voxel_pointer: u32,  // atomic-CAS target — chunkCalc.fx HashValue
    pub use_count:     u32,  // atomicAdd target
    pub hash_raw:      u32,
}
const _: () = assert!(std::mem::size_of::<GpuHashValue>() == 12);
```
- WGSL counterpart wraps `voxel_pointer` and `use_count` in `atomic<u32>` for
  the construction-pass binding (one storage-buffer declaration per shader-side
  usage; the buffer underlying is one `wgpu::Buffer`).
- No `vec3` hazard.
- A 12-byte struct in `array<HashValue>` storage gets a 16-byte stride in WGSL
  (`array` elements are aligned to 16 bytes for non-`vec4` types). **This is
  the second documented `vec3<u32>` storage-buffer alignment deviation**
  (`12-alignment-gap.md` §3 D-A class): the Rust struct is 12 B, the WGSL
  stride is 16 B, so the Rust mirror is actually `#[repr(C)] pub struct GpuHashValueSlot { pub value: GpuHashValue, pub _pad: u32 }` (16 B). The mirror struct's
  alignment guard:
```rust
const _: () = assert!(std::mem::size_of::<GpuHashValueSlot>() == 16);
const _: () = assert!(std::mem::offset_of!(GpuHashValueSlot, value) == 0);
```

### 5.3 `GpuBoundQueueInfo` (8 B = 2×4)

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuBoundQueueInfo {
    pub start: u32,
    pub size:  u32,
}
const _: () = assert!(std::mem::size_of::<GpuBoundQueueInfo>() == 8);
```
- WGSL `array<BoundQueueInfo>` is 8 B stride (a 2-u32 struct).
- No hazard.

### 5.4 `GpuChangedGroup` (8 B = 2×4) — alias `vec2<u32>`

`changedGroupsDynamic` is C# `Uint2[]` of `(pos | y<<11 | z<<21, distance)`.
Port as `[u32; 2]` directly — no fielded struct needed for upload.

### 5.5 `GpuEntityChunkInstance` (20 B = `Uint4` + u32) — W4

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuEntityChunkInstance {
    pub data1: u32,   // posX | (posY & 0x7FF) << 21
    pub data2: u32,   // posZ | (posY >> 11) << 21 | (sizeZ >> 4) << 29
    pub data3: u32,   // quaternion.x compressed
    pub data4: u32,   // quaternion.y compressed | voxelStart << 12
    pub data5: u32,   // entity | sizeX << 14 | sizeY << 21 | (sizeZ & 0xF) << 28
}
const _: () = assert!(std::mem::size_of::<GpuEntityChunkInstance>() == 20);
```
- WGSL `array<EntityChunkInstance>` stride: 20 B with no vec3, fine. (No
  alignment pad needed — all `u32` fields.)
- Verified against `EntityHandler.cs:32-36` (`EntityChunkInstanceGpu`).

### 5.6 `GpuEntityInstanceHistory` (16 B = `Uint4`) — W4

A 4×u32 ring slot, indexed `taa_index * 16384 + entityInstanceID`. Aliased as
`[u32; 4]` — no fielded struct needed.

### 5.7 `GpuBoundGroupMask` (3×u32, per-axis) — W3

Stored as `array<atomic<u32>>` length `bound_group_count * 3`, indexed
`group_index * 3 + axis`. The C# `Uint3` ScalarBuffer is a 12-B struct in the
HLSL but on wgpu the 16-B-stride array of `atomic<u32>` triples is replaced by
three separate `atomic<u32>` arrays of length `bound_group_count`. **Avoids
the `vec3`-then-scalar hazard outright.**

### 5.8 Existing struct changes (re-verified, none impact Phase C's data)

`GpuGiParams.taa_jitter` (offset 280 — `gpu_types.rs:574-576`) is unchanged.
`GpuRenderParams._pad0a/_pad0b` (offsets 8/12) is unchanged. The Phase-C
additions are all in new structs; no existing GPU struct is widened.

---

## 6. Decisions & rejected alternatives

Architect-prompt mandatory section. Every load-bearing call below is
re-derivable only with the same context; an implementer reading the §1–§5
polished design alone would re-derive many of these differently.

### 6.1 Seam location: `render/construction/` sub-module (chosen) vs `aadf_gpu/` top-level vs separate workspace crate

- **Chose `crates/bevy_naadf/src/render/construction/`** because the work is
  render-world work that needs `RenderDevice` / `PipelineCache` / `Core3d`
  schedule access — all things `render/` already plumbs. The Phase-B
  precedent (`render/atmosphere.rs`, `render/gi.rs`, `render/graph_b.rs`) is
  the established pattern, and it kept the seam to a single edit point on
  `render/mod.rs`. The directory split (sub-directory not a 2 000-line file)
  preserves parallel-workstream merge-conflict freedom.
- **Rejected `crates/bevy_naadf/src/aadf_gpu/`** — it would force a *second*
  render-app entry point (or shared resources cross-imported between
  `aadf_gpu/` and `render/`), doubling the seam surface for no architectural
  benefit. The semantic argument that "construction belongs near the AADF
  data structure not near the render pipeline" is real but loses to the
  practical argument that 100 % of new GPU compute work needs `render/`'s
  existing wiring.
- **Rejected a separate workspace crate** — would force splitting `WorldGpu`
  across crate boundaries (the existing crate owns it; a new crate would have
  to import or re-export it). Phase-A Q4 binds: single crate, modules under
  `src/`. Workspace crate would also break the worktree-parallelism model —
  every workstream would have to coordinate `Cargo.lock` edits.
- **Flip-the-decision fact:** if the user later wanted GPU construction to
  be usable by *other* Bevy apps (a reusable voxel-construction plugin
  crate), the seam would split off into its own crate at that point — the
  sub-module's interface (`ConstructionGpu` resource, `ConstructionPlugin`)
  is deliberately self-contained to make that future extraction mechanical.

### 6.2 Bind-group strategy: parallel `construction_world_layout` (chosen) vs `@group(1)` extension vs read_only-everywhere

- **Chose a parallel construction-mode `@group(0)` layout** because wgpu's
  `BindGroupLayoutEntry` validation is *layout*-level, not pipeline-level
  (`13-reuse-audit-c.md` "Borderline calls" verified). The same underlying
  `Buffer` handle is bound through two different layout descriptors — one
  read-only (the existing `world_layout`, consumed by render passes), one
  read-write (the new `construction_world_layout`, consumed by construction
  passes). Wgpu accepts this; the buffer's `BufferUsages` only needs to
  include both `STORAGE` flavors (already true for `GrowableBuffer` —
  `world/buffer.rs:36-38`).
- **Rejected a dedicated `@group(1)` extension** — would not fix the wgpu
  layout-level rw/ro conflict on `blocks`/`voxels`/`chunks` (the C# read-write
  bindings on construction passes — `chunkCalc.fx:33-35` — make them rw under
  any group number). The conflict is wgpu's layout validation, not group
  collision. A `@group(1)` extension would *additionally* increase the
  bind-group count of every construction pipeline by 1, which is cost without
  benefit.
- **Rejected widening `world_layout` to read_write everywhere** — would
  require every render pipeline to bind read_write storage when it only
  semantically reads. Wgpu would complain about pipeline-vs-layout mismatch
  on read-only declarations in the WGSL. Also a regression of intent: the
  read-only declaration in render passes is *documentation* that the renderer
  never writes the world buffers.
- **Flip-the-decision fact:** if wgpu added a `@uniformity(non_aliasing)`
  hint or relaxed the layout-level constraint to pipeline-level (as Vulkan
  does), the parallel-layout split would still be the clean idiomatic
  Bevy/wgpu pattern, but the layout could be unified.

### 6.3 Construction-pass scheduling: regime-1 startup + regime-2 Core3d + regime-3 Core3d-with-gate (chosen) vs full-startup-batch vs always-in-Core3d

- **Chose three-regime scheduling** because NAADF's three temporal regimes
  are load-bearingly different (§1.2) — *initial build is one-shot blocking
  before any render starts*; *AADF maintenance is paced one-queue-per-frame*;
  *editing is event-driven*. Collapsing any pair loses real behaviour: a
  full-startup-batch would force AADF construction to be eager (recomputing
  the entire 32-deep queue at once), losing the "non-stalling background
  recompute" property that lets large worlds stay editable. An always-in-Core3d
  approach would run the regime-1 dispatches every frame, costing milliseconds.
- **Rejected full-startup-batch (run all three regimes at startup, then keep
  re-running regime 2 on the Core3d chain)** — works on a 16³-chunk test
  grid but does not generalise to the paper's tera-voxel scenes (`14-paper-gap.md`
  §5). The paper makes a point of "construction is not time-critical;
  modified cells are queued separately per layer, AADFs computed in the
  background during rendering" — that is the *contribution* of paper §3.3,
  and the port loses it if regime 1 is conflated with regime 2.
- **Rejected always-in-Core3d** — same problem: regime 1's `GenerateWorld`
  is meaningfully a one-shot operation (the per-segment loop in
  `WorldData.cs:136-156`), not a per-frame pass. Running it every frame would
  waste a full per-segment construction dispatch per frame even when nothing
  changed.
- **Flip-the-decision fact:** if the user later wanted runtime world
  resizing (a "build a new segment on demand" feature), regime 1 would move
  to an on-event regime-3-style gate; the three-regime structure already
  accommodates this with a fourth gate, no refactor.

### 6.4 Entity-track chunk-format pre-widening: NO (chosen) vs YES

- **Chose to put the chunk-format widening *inside* W4** (§1.7). The
  argument is concrete: pre-widening forces every wave-2 workstream to verify
  its WGSL works on a `Rg32Uint` format that *no other workstream uses yet*.
  The `.x` selection sweep is a single mechanical change, and bundling it
  with W4 means it lands in one merge where every renderer-side reader is
  audited together.
- **Rejected pre-widening (W0 widens the chunks texture format)** — would
  push the sweep cost onto W0, the seam PR that is supposed to be small and
  fast. The "build everyone against the widened format" claim sounds clean
  but in practice forces every wave-2 workstream's WGSL to be re-verified
  on a format they have no semantic use for. Also: W0 has no entities to
  verify the wider format with, so the test surface is "did I not break
  the renderer?" — exactly what the integration step catches anyway.
- **Flip-the-decision fact:** if W4 turned out to need merging *before* a
  wave-2 workstream that itself widens the format (it does not), pre-widening
  would re-emerge. The §2.2 merge order (W4 between W3 and W2) is verified
  against the C# dependency graph and is stable.

### 6.5 Workstream split granularity: 7 workstreams (chosen) vs 4 macro-workstreams vs 12 micro-workstreams

- **Chose 7 workstreams** to match the seven natural code-units of paper-gap
  items #1–#6: W0 (seam — required infra), W5 (worldgen, item #6), W6 (AADF
  rewrite, item #5), W1 (Algorithm 1, item #1), W3 (bounds queue, item #3),
  W2 (editing, item #2), W4 (entities, item #4). Each Wn maps 1:1 to a
  paper-gap item or the seam. This makes assignment / review / merge order
  obvious and audit-able. The wave structure follows the dependency DAG
  the paper-gap analysis already produced.
- **Rejected 4 macro-workstreams** (e.g. "construction" = W0+W1+W3+W5,
  "editing" = W2+W6, "entities" = W4) — too coarse for the worktree-parallel
  model; each macro-PR would be 1 500+ lines, too large for fresh-eyes
  review per project discipline.
- **Rejected 12 micro-workstreams** (one shader per workstream + one CPU
  oracle per workstream) — too fine; the shader-and-oracle pairing is
  load-bearing, and splitting them creates artificial merge ordering. W1's
  `chunk_calc.wgsl` and its CPU oracle path through `aadf/construct.rs` are
  the same logical change.
- **Flip-the-decision fact:** if `cargo test` time on a worktree grew past
  a project budget (it currently runs ~10 s), W1 might split into
  `chunk_calc` (the 3 entry points) and `hashing` (the open-addressing
  table + map_copy) — purely a build-time concern, not an architectural
  one.

### 6.6 Wave structure: 3 waves, 2-4-1 (chosen) vs 2 waves vs 4 waves

- **Chose 3 waves** because the paper-gap dependency graph has exactly two
  edges: W1 depends on (W0 ∧ W5); (W2 ∧ W3 ∧ W4) depend on W1. That is two
  depth-edges in the DAG, hence three waves. Wave 1 holds 3 workstreams
  (W0, W5, W6) because W5/W6 are independent of everything except W0; wave
  2 holds 4 (W1, W2, W3, W4) because W1 is foundational *within* the wave
  but the others can land in parallel after it merges; wave 3 is the merge
  agent.
- **Rejected 2 waves** (W0 in wave 1, everything else in wave 2 — relying
  on intra-wave-2 sequencing) — loses the "wave-1 work is genuinely
  independent and can start immediately" property that the user's directive
  ("trivial with worktrees on a rust codebase") implies. W5/W6 are useful
  work to land in parallel with W0; the 3-wave structure makes that
  explicit.
- **Rejected 4 waves** (splitting W2's flood-fill from its apply passes
  into a separate wave) — the flood-fill and apply passes belong in the
  same PR (the CPU `change_handler.rs` and the GPU `world_change.wgsl` are
  one logical unit). Splitting would create unnecessary cross-PR
  dependencies.
- **Flip-the-decision fact:** if the integration step (wave 3) revealed
  cross-workstream conflicts beyond the seam contract, a wave 2.5 of fixup
  PRs would be added. The seam-first design exists to prevent this.

### 6.7 W4 entity-track scope: full entity track in one workstream (chosen) vs split entity track into 2 workstreams

- **Chose to bundle W4's entire entity track into one workstream** because
  the chunk-format widening (§1.7) and the entity-instance handling are
  inseparable: the widened format *exists to carry* the entity pointer.
  Splitting them would force one of the splits to leave the format flip
  un-merged, blocking renderer access to the entity data — wholesale
  defeating the entity track. The size penalty (W4 is a "large" workstream)
  is the price of the bundling.
- **Rejected splitting W4 into "format widening" + "entity instance
  handling"** — the format widening is **only useful** if entities ship at
  the same time; merging it standalone is dead code that affects every
  render pass. The bundle is unavoidable.
- **Flip-the-decision fact:** if the entity track itself were to land in
  multiple stages (e.g. static entity-instance buffers in stage 1, moving
  entities in stage 2), the second stage would be a small follow-up
  workstream rather than splitting W4. The current scope is "implement
  paper contribution #4 in full" — one workstream.

### 6.8 CPU oracle integration: assertion-in-startup (chosen) vs assertion-on-debug-only vs no oracle

- **Chose to run the GPU/CPU bit-exact assertion in the e2e binary under a
  `--validate-gpu-construction` flag, default off in `cargo run --bin
  bevy-naadf`, enabled in W1's PR-gate `cargo run --bin e2e_render
  --validate-gpu-construction`.** This matches NAADF's own dev-time
  validation pattern (`ChunkCopyToCpu` followed by `Array.Copy` to the CPU
  mirror — `WorldData.cs:167-188`) but folds the assertion into the
  test/e2e harness rather than production code. Production startup is
  GPU-only; dev builds with `--validate-gpu-construction` add the readback
  and assert.
- **Rejected always-on assertion** — would force the CPU build to run on
  every startup, doubling startup time on the user's path (`cargo run --bin
  bevy-naadf`). The CPU build is O(n) per voxel; for the 16³-chunk test
  grid it is fast (< 100 ms), but it would still be visible.
- **Rejected debug-cfg-only assertion** — `cargo test --release` would
  skip the validation, and the project runs release-mode `cargo run`
  routinely. The e2e-flag gate is explicit and inspectable.
- **Flip-the-decision fact:** if a future test-only world preset were
  added that the CPU oracle cannot handle (e.g. a multi-GB volume that
  exceeds CPU RAM), the assertion would become opt-in per preset rather
  than per-flag. The current test scene is small (16³ chunks = ~1 MB);
  the oracle path is comfortably ahead.

---

## 7. Assumptions made

Architect-prompt mandatory section. Each item is a place the brief / prior
docs underspecify; if any of these turns out wrong, the design needs
revisiting at the orchestrator-synthesis pause.

1. **The `chunks` texture's `STORAGE_BINDING` usage flag** is the *only*
   production-side production-code change Phase C makes to existing code
   outside `render/construction/` and one `.chain()` insert in `render/mod.rs`.
   Assumption: wgpu's `Rgba8UnormSrgb`-style format restrictions do not apply
   to `R32Uint` 3D textures as storage textures (the wgpu standard requires
   `r32uint` to be a valid storage-texture format with `STORAGE_BINDING +
   TEXTURE_BINDING` usage — verified in the wgpu spec but not by running a
   test build). If this turns out to require a *render-attachment*-style
   workaround, W0's chunks-flag widening becomes more involved.

2. **`AppArgs`-style configuration plumbing extends to `ConstructionConfig`
   the same way it extended to `TaaRingConfig`** (`render/mod.rs:73-85`,
   `taa.rs::TaaRingConfig`). Assumption: the orchestrator accepts another
   plugin-build-time config resource without preferring a single combined
   `Settings` resource. If preferred, W0's seam absorbs that with no
   architectural impact.

3. **The hash-map's initial size is taken from
   `BlockHashingHandler.cs:36-46`'s formula:** `max(1, startSizeMap)` doubled
   until `mapSize * wantedEmptyRatio ≥ minReservedCount`. For the
   16³-chunk-grid `minReservedCount = maxNewVoxelsPerGenSegment / 32 = 4 096 / 32 = 128`,
   so the initial map size is 256 (256 * 0.5 = 128). Assumption: this is
   correct for `segment_size_in_chunks = 4` (NAADF's value); if Phase C
   changes the segment size, the initial size recomputes.

4. **The entity-instance cap of 16 384** (`WorldRender.cs:88`'s ring depth
   inheriting from `taa_index * 16384`) is held constant. Assumption: the
   test scene W4 ships uses a small number of entities (single-digit count)
   and 16 384 is comfortable headroom. If the test scene grows entities,
   the cap is a `ConstructionConfig` knob — no architectural change.

5. **The `e2e_render` binary's `--validate-gpu-construction`, `--edit-mode`,
   `--entities` flags can be added as clap arguments to `AppConfig::e2e()`'s
   builder without breaking the existing harness (§e2e-render-test §9
   `AppConfig` extensibility).** Assumption: the existing `AppConfig::e2e()`
   constructor cleanly accepts new boolean fields. Reading the e2e doc this
   is true.

6. **Wgpu's `texture_storage_3d` macro with `Rg32Uint` is supported as a
   `read_write` storage texture format.** Per the wgpu spec, `rg32uint`
   should be supported but it requires `Features::TEXTURE_FORMAT_NV12` —
   actually no, `rg32uint` is a base WebGPU format. Verified in the wgpu
   spec; W4's impl agent verifies with the first build.

7. **The CPU `aadf/construct.rs` 3-phase build's `block_dedup: HashMap<[VoxelTypeId; CELL_CHILDREN], VoxelPtr>`** produces the same `VoxelPtr` assignment order
   that NAADF's GPU `chunkCalc.calcBlockFromRawData` produces deterministically
   on a 16³-chunk grid. Strict assumption: the open-addressing hash function
   `H = c₀ + Σcᵢ·vᵢ` produces probe-order that, on small grids with no
   collisions, gives sequential `VoxelPtr` values that match the Rust
   `HashMap`'s insertion-order *for a deterministic iteration order*.

   This is the **load-bearing assumption** for the E4 oracle. If the
   `VoxelPtr` assignment differs between CPU and GPU on the test grid (e.g.
   because the GPU hashes multiple identical blocks in parallel and assigns
   them sequential pointers in race-resolution order while the CPU
   `HashMap` assigns based on iteration order), the bit-exact byte-equality
   assertion at startup fails.

   **Mitigation:** the assertion in §1.6 might need relaxation from "byte
   equality" to "semantic equality" (every chunk decodes to the same
   `ChunkClass` + every block to the same `BlockClass` + the voxel
   *contents* at the assigned pointers match, even if the pointers
   differ). The W1 impl agent verifies this on the first build; if byte
   equality fails but semantic equality holds, the design upgrades the
   §1.6 oracle to "semantic equality" with a note in the impl log. This
   is the **most fragile assumption** in the design and the most likely
   to trigger a design tweak.

8. **Test-suite scope: every new `#[test]` in a workstream's PR runs under
   `cargo test --workspace` (the existing convention)**. Assumption: no
   workstream's tests require a GPU device (only the e2e binary does, per
   `e2e-render-test §2.3`). All CPU-side oracles + flood-fill tests + AADF
   rewrite tests run pure-CPU; only the bit-exact GPU-vs-CPU assertion at
   e2e startup requires a GPU, and it lives in the e2e binary path, not in
   `cargo test`.

9. **The user's "teams" execution model maps onto separate
   `TaskTool`/dispatch invocations per workstream**, each given a dedicated
   worktree path. Assumption: the orchestrator's dispatch infrastructure
   exposes a `team` primitive that creates a named dispatch lane per
   workstream. If it does not, the fallback is sequential dispatches per
   workstream (still per-worktree-isolated, just not concurrent). The
   wave structure (§2.2) constrains the *minimum* number of dispatches
   either way.

10. **The `delegate-reviewer` review-gate for Phase C reviews the full
    seven-workstream merge plus the integration step in one pass**, not
    per-workstream. Assumption: the project's review discipline allows a
    multi-PR review (the Phase-B precedent — `11-review-b.md` reviewed the
    full 6-batch Phase-B impl in one pass — confirms this). If per-workstream
    review is preferred, the integration step gates on per-workstream
    review-PASS rather than a single Phase-C review-PASS.

---

**End of Phase-C design.** The next dispatch is the **W0 impl agent** —
seam-only, smallest scope, lands first. Subsequent dispatches follow §2.1's
wave plan.
