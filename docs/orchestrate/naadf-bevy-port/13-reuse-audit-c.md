# 13 — Reuse Audit: Phase C — GPU World Construction & Editing

**delegate-auditor findings (2026-05-15)**

Scope: what already exists in `bevy-naadf` (`crates/bevy_naadf/src/`) that covers,
partially covers, or could be extended for **GPU-side world / AADF construction and
editing**. Also covers what NAADF's C# reference does for construction (`chunkCalc.fx`,
`boundsCalc.fx`, `boundsCommon.fxh`, `worldChange.fx`, `mapCopy.fx`, `BlockHashingHandler.cs`,
`WorldBoundHandler.cs`, `ChangeHandler.cs`) vs. what is already in Rust.

---

## Candidate table

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| CPU AADF construction (`construct`, `DenseVolume`, `ConstructedWorld`, `BlockClass/ChunkClass` classifications) | `crates/bevy_naadf/src/aadf/construct.rs:1–573` | Full CPU-side Algorithm 1 re-derivation: 3-phase dense→chunk/block/voxel pipeline with `HashMap`-based block dedup; produces bit-identical output to what `chunkCalc.fx` would emit; 4 `#[test]` functions covering empty, uniform, mixed, and dedup cases | **extend** | The 3-phase construction logic (classify blocks, classify chunks, encode with AADFs) is the direct CPU mirror of `chunkCalc.fx`'s three passes (`calcBlockFromRawData`, `computeVoxelBounds`, `computeBlockBounds`); porting to GPU means translating each phase to a compute entry point and replacing the `HashMap` with open-addressing `InterlockedCompareExchange` atomic probing exactly as in the C# `GetVoxelPointer` |
| AADF cuboid expansion (`compute_aadf`, `CellBox`, `slice_empty`) | `crates/bevy_naadf/src/aadf/bounds.rs:1–229` | CPU alternating-axis AADF expansion; faithfully maps to `boundsCommon.fxh:ComputeBounds4` (the 3-iteration `GroupMemoryBarrierWithGroupSync` loop); 5 `#[test]` functions covering corner, inner, capped, wall, and cuboid-validity cases | **extend** | `ComputeBounds4` in `boundsCommon.fxh` is `compute_aadf` expressed as a GPU groupshared loop; the logic is structurally identical — `GroupMemoryBarrierWithGroupSync` replaces Rust's sequential passes, `cachedCell[64]` replaces the `is_empty` closure captures. The WGSL port of `ComputeBounds4` can be derived line-for-line from `bounds.rs` + `boundsCommon.fxh` |
| Cell encode/decode types (`ChunkCell`, `BlockCell`, `VoxelCell`, `Aadf6`, `BlockPtr`, `VoxelPtr`, `pack_voxels`, `unpack_voxel`) | `crates/bevy_naadf/src/aadf/cell.rs:1–347` | All bit-layout encode/decode helpers for chunk (5-bit AADF), block (2-bit AADF), and voxel (packed 2-per-u32) cells; already verified bit-exact against the C# (`02-research.md` §1.1.2); 12 `#[test]` round-trips | **reuse** | The GPU construction shaders in `chunkCalc.fx` use identical bit layouts; the Rust encode/decode is the authoritative reference for writing the equivalent WGSL helper functions or validating them. The constants (`CELL_HAS_CHILDREN`, `CELL_UNIFORM_FULL`, `VOXEL_FULL_FLAG`, `AADF_BITS_CHUNK`, etc.) are already in `src/voxel/mod.rs` and will flow directly into the WGSL definitions |
| `GrowableBuffer<T>` with `reserve` / `write` / `upload_all` / `reserve_discard` | `crates/bevy_naadf/src/world/buffer.rs:1–399` | Generic growable wgpu storage buffer; `reserve` does `copy_buffer_to_buffer` on growth (the `DynamicStructuredBuffer` + `CopyData` equivalent); `GROWTH_FACTOR=2`; already tested against a real wgpu device | **reuse** | The `blocks`, `voxels`, and hash-map buffers needed by the GPU construction pipeline all require exactly this grow-and-copy semantic (`BlockHashingHandler.IncreaseSizeToNewCount` / `mapCopy.fx`). Phase C needs only to call `reserve` on the existing buffers before issuing construction dispatches — no new buffer abstraction needed |
| World GPU resources and upload path (`WorldGpu`, `prepare_world_gpu`) | `crates/bevy_naadf/src/render/prepare.rs:47–120` | The render-world resource holding `chunks` (3D texture), `blocks` / `voxels` / `voxel_types` (`GrowableBuffer`s), `world_meta` uniform, and `bind_group_world`; currently built once on CPU dirty flag | **extend** | Phase C replaces the CPU-built `upload_all` path in `prepare_world_gpu` with GPU compute dispatches writing into `blocks` and `voxels` directly. The `WorldGpu` struct is the right holder for any new construction-phase GPU resources (`hashMap` buffer, `blockVoxelCount` atomic, `segmentVoxelBuffer`, `boundQueueInfo`, `boundGroupQueues`, `boundGroupMasks`); only the resource fields and `prepare_world_gpu`'s body change |
| Render-graph compute pass pattern (`graph.rs` / `graph_b.rs` node systems) | `crates/bevy_naadf/src/render/graph.rs:74–end`, `crates/bevy_naadf/src/render/graph_b.rs:65–end` | 14 existing `Core3d` schedule system nodes (compute + render pass); all follow the pattern: obtain `PipelineCache`, dispatch workgroups, wrap in `time_span`; multi-pass staging already demonstrated (5 passes in `sample_refine`, indirect dispatch in `ray_queue_calc`) | **reuse** | Phase C construction passes (`calcBlockFromRawData`, `computeVoxelBounds`, `computeBlockBounds`) and the background AADF queue (`prepareGroupBounds`, `computeGroupBounds`) are additional `Core3d` node systems with the same pattern — `PipelineCache` lookup, compute pass, dispatch. The indirect-dispatch pattern from `naadf_ray_queue_node` (writing then reading an indirect-args buffer) is directly reusable for `boundsCalc`'s `boundGroupQueueDispatchCount` |
| Bind-group layout infrastructure (`NaadfPipelines`, `binding_types` helpers, `@group(0)` world layout) | `crates/bevy_naadf/src/render/pipelines.rs:1–100` | `NaadfPipelines` holds all bind-group-layout descriptors; the `@group(0)` world layout already exposes `chunks` (RW 3D texture needed for construction), `blocks`, `voxels` as storage; `binding_types::{storage_buffer_sized, texture_3d, uniform_buffer_sized}` used throughout | **extend** | The GPU construction shaders need `RWTexture3D chunks`, `RWStructuredBuffer blocks/voxels`, plus new bindings (`hashMap`, `blockVoxelCount`, `segmentVoxelBuffer`, `boundQueueInfo`, etc.). A new `@group(0)` variant for construction (write-access) or a dedicated construction bind-group layout slots into `NaadfPipelines` following the existing pattern; the helper types are all already imported |

---

## Top reuse recommendation

**`GrowableBuffer<T>` + the existing `WorldGpu` resource + the `Core3d` compute-node
pattern** are the highest-leverage reuse surface for Phase C.

`GrowableBuffer` already provides the exact grow-and-copy semantic that `BlockHashingHandler`
and `mapCopy.fx` implement in the C# (`IncreaseSizeToNewCount` / `copyMap`): allocate a
new larger buffer, `copy_buffer_to_buffer` the old contents, swap. Every buffer Phase C
needs — `blocks`, `voxels` (already in `WorldGpu`), plus a new `hashMap` typed as
`GrowableBuffer<HashValue>` — can use this abstraction directly with no modification.

The CPU construction (`aadf/construct.rs`) is the second-most important reuse surface: its
three phases map directly onto the three `chunkCalc.fx` compute entry points, so the
WGSL design phase has a precise Rust reference to write against and can validate GPU output
against CPU output in tests.

The `Core3d` compute-node pattern (graph.rs / graph_b.rs) means the construction
dispatches integrate into the render loop without any new infrastructure — they are just
additional `Core3d` schedule systems, same as the atmosphere / GI / denoiser passes.

---

## Borderline calls

**`aadf/construct.rs` — reuse vs. extend.** Classified as "extend" because the GPU port
replaces the core dedup mechanism (Rust `HashMap` → `InterlockedCompareExchange`
open-addressing atomic probing, max 250 probes) and adds `groupshared` synchronisation
barriers between the three phases. The overall 3-phase shape and the classify/encode
logic are faithfully reusable as the WGSL design reference, but the CPU code is not
directly compiled to GPU — it is extended by expressing the same algorithm in WGSL. If
the auditor's brief were "is there WGSL compute shader code to extend?", the answer would
be "no, greenfield WGSL"; if the brief is "is there Rust logic to guide and validate the
GPU design?", the answer is "strong reuse." The verdict "extend" reflects the second
reading (which is the load-bearing one for design guidance).

**`NaadfPipelines` / bind-group layout — extend vs. greenfield.** The existing `@group(0)`
world layout is read-only (`storage, read`) for the render passes. GPU construction needs
write access (`storage, read_write`) on the same buffers. wgpu forbids mixing a
`STORAGE_READ_WRITE` binding with a `read`-only binding in the same bind-group layout, so
Phase C will need a parallel construction-mode `@group(0)` layout (or a separate
construction bind group). This does not make the existing layout irrelevant — the helper
types, the `NaadfPipelines` resource, and the `BindGroupLayoutEntries` builder pattern
are all reused — but it does mean that the layout descriptor itself cannot be shared
verbatim. Classified "extend" is correct; if the wgpu constraint were not present it
would flip to "reuse."

**`prepare_world_gpu` — extend.** Currently a build-once CPU-upload function. For Phase C,
`prepare_world_gpu` would gain two modes: the existing CPU-upload path (initial world
build, or when the GPU construction path is disabled) and a GPU-dispatch path (segment
construction dispatches). The function signature and resource ownership do not need to
change, but the body grows substantially. A design that moves the GPU construction
dispatches to a separate system rather than extending `prepare_world_gpu` would also be
valid; the verdict depends on the Phase C design agent's architectural preference.

---

## Greenfield

The following have no existing coverage in the port and are completely greenfield for
Phase C:

1. **`chunkCalc.fx` → WGSL compute shaders.** Three entry points:
   - `calcBlockFromRawData` (`[numthreads(4,4,4)]`) — the hash-based block construction,
     `GetVoxelPointer` open-addressing with `InterlockedCompareExchange`, groupshared
     `insertBlockIndex`/`isAllBlocksEqual`, atomic `blockVoxelCount` append.
   - `computeVoxelBounds` (`[numthreads(64,1,1)]`) — voxel AADF pass using
     `ComputeBounds4` on `cachedCell[64]` groupshared.
   - `computeBlockBounds` (`[numthreads(64,1,1)]`) — block AADF pass, same pattern.
   - `chunkCopyToCpu` (`[numthreads(64,1,1)]`) — GPU→CPU sync (may use wgpu
     buffer-map instead of a dedicated shader).

2. **`boundsCalc.fx` → WGSL background chunk-AADF queue.** Five entry points:
   - `addInitialGroupsToBoundQueue` — queue initialisation.
   - `prepareGroupBounds` (`[numthreads(1,1,1)]`) — queue consumer, picks next work
     item; writes indirect-dispatch args.
   - `computeGroupBounds` — processes a 4³-chunk group, calls `ComputeBounds4` for
     the chunk level.
   - `addBoundsGroup` — cross-group neighbour merge.
   - Associated `BoundQueueInfo[32*3]` queue, `boundGroupQueues`, `boundGroupMasks`,
     `boundRefinedInfo` buffers — all new.

3. **`boundsCommon.fxh:ComputeBounds4` → WGSL helper.** The groupshared
   alternating-axis AADF pass currently exists only in C# / HLSL. The Rust
   `compute_aadf` in `bounds.rs` is the logic reference but the WGSL groupshared
   implementation is greenfield (different execution model: groupshared `cachedCell[64]`,
   `GroupMemoryBarrierWithGroupSync` instead of sequential passes).

4. **`worldChange.fx` → WGSL flood-fill invalidation.** Four entry points:
   - `applyGroupChange` (`[numthreads(4,4,4)]`) — resets chunk AADFs in a 4³ group
     after an edit; re-enqueues into `boundsCalc` queues.
   - `ApplyChunkChange` / `ApplyBlockChange` / `ApplyVoxelChange` — apply CPU-staged
     edits to the GPU buffers and recompute local AADFs via `ComputeBounds4`.
   - Requires `changedGroupsDynamic`, `changedChunksDynamic`, `changedBlocksDynamic`,
     `changedVoxelsDynamic` staging buffers (CPU→GPU upload paths).

5. **`mapCopy.fx:copyMap` → WGSL hash-map regrow.** Linear-probing rehash from old
   to new map on occupancy overflow; requires the `HashValue` struct on GPU.

6. **`BlockHashingHandler` CPU orchestration → Bevy system(s).** The C# class manages
   hash-map occupancy tracking, resize triggering, coefficient generation
   (`31^(64-i) mod 2³²`), and the CPU-side `AddBlock` fallback. The Bevy equivalent
   is one or more systems (or resource methods) managing a `hashMap: GrowableBuffer<HashValue>`
   resource, tracking occupancy, triggering `mapCopy` dispatches at 50 % fill
   (`wantedEmptyRatio = 0.5`), and pre-computing the 65 `hashCoefficients` at startup.

7. **`WorldBoundHandler` CPU orchestration → Bevy system(s).** Manages the 32×3
   `boundQueueInfo` queue, the 5-rounds-per-frame dispatch loop, the
   `maxGroupBoundDispatch` throttle, and the per-axis `boundGroupMasks`. Entirely new;
   no existing Bevy system covers it.

8. **`ChangeHandler` + `EditingHandler` → Bevy system(s).** The BFS flood-fill
   (`distanceFloodFill`, `floodFillQueue`, `addBounds`/`checkMatchingBoundCell`),
   the `changedGroupsWithDist` upload, the per-edited-chunk `processChunks` pipeline
   (hash each changed block, free old voxel slots, write changed arrays). All greenfield.

9. **`segmentVoxelBuffer` upload path.** The dense per-segment voxel input buffer
   (`WorldGenerator`-produced or procedurally authored) that `chunkCalc.fx` reads.
   Currently the CPU construction reads directly from `DenseVolume::voxels`; the GPU
   path needs this buffer uploaded as a `StructuredBuffer<uint>` before dispatch.
   Distinct from the existing `voxels` buffer (which holds the deduplicated output).

**Summary of greenfield scope:** Phase C is a large, self-contained compute track —
approximately 5 new WGSL files (~15 entry points total), 3 new `GrowableBuffer`-typed
GPU resources (`hashMap`, `boundGroupQueues`/`boundGroupMasks`/`boundQueueInfo`,
`changedGroupsDynamic`), and 3–4 new Bevy systems / resource types. The existing port
provides strong scaffolding (buffer abstraction, node pattern, bit-layout reference) but
no GPU construction code whatsoever.
