# 02 — Design: procedural-noise streaming world

Architect: `delegate-architect` (2026-05-18). Brief: `docs/orchestrate/streaming-world/01-context.md`. Reuse palette: `00-reuse-audit.md`.

This document is the design contract for the impl agent. Section ordering is required reading order. The `## Δ-StreamingResidency`, `## Decisions & rejected alternatives`, and `## Assumptions made` sections after `## Design` are load-bearing — the polished design alone does not carry the implicit decisions behind it.

---

## Design

### A. Residency manager (greenfield)

**New module:** `crates/bevy_naadf/src/streaming/mod.rs` + `residency.rs` (~250 LOC) + `chunk_source.rs` (~120 LOC). Mounted under a new `StreamingPlugin` registered by `lib.rs::build_app` after `ConstructionPlugin` (line `crates/bevy_naadf/src/lib.rs:670`).

#### A.1 Window geometry — the residency window IS the world container

Per Q2 (per-segment residency, `SEGMENT_CHUNKS = WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 = 16`) and the audit's row-5 "Bypass for infinite worlds" call, the residency window equals the existing fixed-world container. The container does not shrink, but its `world_origin_in_segments: IVec3` shifts under a moving camera. The shifting origin re-maps **which world-segment** lives at **which window-local slot**.

- World container = `WORLD_SIZE_IN_SEGMENTS = UVec3(16, 2, 16)` segments (`crates/bevy_naadf/src/lib.rs:217`) = **512 slots total**.
- Per-slot footprint = one segment = `16³` chunks = `4096³ × 4 B / 2` u32 voxels = **128 MiB** of `segment_voxel_buffer` (`crates/bevy_naadf/src/render/construction/mod.rs:1527-1547`).
- The `WORLD_SIZE_IN_SEGMENTS.y = 2` axis is shallow on purpose — the streaming window is a **flat cylinder/AABB** in X/Z and only 2 segments tall in Y.

The chosen window shape is therefore a **`16 × 2 × 16`-segment AABB centred on the camera's segment**, sliding in X/Z only. Y is full-height (both Y segments always resident) because the world is `WORLD_SIZE_IN_SEGMENTS.y = 2` deep — splitting that axis costs more than it saves.

#### A.2 Indirection table — dense `Vec<Option<WorldChunkPos>>`

Per Q2 (per-segment residency), the indirection is **segment-keyed**, not chunk-keyed. We need two maps:

```rust
// In crates/bevy_naadf/src/streaming/residency.rs:

/// A world-segment coordinate in `IVec3` chunk-coords / `SEGMENT_CHUNKS`.
/// Newtype because `IVec3` flies around the codebase already (PositionSplit,
/// AABBs, edits) and the unit confusion would be lethal — these are *segments*,
/// not chunks, not voxels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldSegmentPos(pub IVec3);

/// Window-local segment index, `[0, WORLD_SIZE_IN_SEGMENTS.x * y * z) = [0, 512)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotIndex(pub u32);

#[derive(Resource)]
pub struct Residency {
    /// The world-origin offset in segments. Shifted when the camera crosses
    /// a segment boundary. World segment `s` lives at window-local slot
    /// `slot_xyz = s.0 - origin` when that diff is in [0, WORLD_SIZE_IN_SEGMENTS).
    pub origin: IVec3,

    /// slot_index → resident WorldSegmentPos (or None if the slot is empty
    /// /pending). Dense Vec of length `WORLD_SIZE_IN_SEGMENTS.x * y * z = 512`.
    pub slot_to_world: Vec<Option<WorldSegmentPos>>,

    /// Reverse index: WorldSegmentPos → SlotIndex. Small HashMap (≤ 512
    /// entries by construction). The forward map is dense (slot-indexed
    /// reads from the per-frame system); the reverse map is sparse + small.
    pub world_to_slot: HashMap<WorldSegmentPos, SlotIndex>,

    /// Per-slot generation state (Empty / Pending(task) / Ready). Drives the
    /// "what to dispatch this frame" loop.
    pub slot_state: Vec<SlotState>,
}

#[derive(Clone, Debug)]
pub enum SlotState {
    Empty,
    Generating,                  // CPU noise task in flight
    Encoded(Box<EncodedSegment>),// noise → EncodedSegment done; awaiting GPU upload
    Resident,                    // uploaded to GPU; participating in rendering
}
```

**Justification for dense Vec over open-addressed HashMap.** The slot table is hit once per shift per slot (≤ 512 entries, max ~32 shifts per frame at the assumed camera speed). Even a 512-entry direct-mapped Vec is faster than a HashMap and trivially `IntoIter`-able by the eviction sweep. `HashMap` is reserved for the reverse map (random-access "is this world-segment resident?") where the small size still wins over linear scan.

#### A.3 World-origin shift geometry

Each `PreUpdate` the residency system compares the camera's current `WorldSegmentPos` against the previous frame's value. When the segment changes:

1. **Compute target resident set.** The camera segment determines the new `origin` such that the camera segment lives at slot index `(WORLD_SIZE_IN_SEGMENTS.x / 2, _, WORLD_SIZE_IN_SEGMENTS.z / 2)` (center of the X/Z window). Y is unconstrained because `WORLD_SIZE_IN_SEGMENTS.y = 2`.
2. **Compute evictions.** For each slot whose `Some(world_seg)` no longer satisfies `is_in_window(world_seg, new_origin)`, mark `SlotState::Empty`. Remove from `world_to_slot`.
3. **Compute admissions.** For each world-segment in the target set not in `world_to_slot`, assign it to a freed slot. Mark `SlotState::Generating`.

Window predicate (X/Z only — Y is full-height):

```rust
fn is_in_window(s: WorldSegmentPos, origin: IVec3) -> bool {
    let d = s.0 - origin;
    d.x >= 0 && d.x < WORLD_SIZE_IN_SEGMENTS.x as i32
        && d.y >= 0 && d.y < WORLD_SIZE_IN_SEGMENTS.y as i32
        && d.z >= 0 && d.z < WORLD_SIZE_IN_SEGMENTS.z as i32
}
```

The window is an AABB (not a sphere) because the GPU slot table is rectangular and rebinding bind groups on partial-circle resident sets carries no win.

#### A.4 VRAM budget enforcement — one-shot allocation

Per `00-reuse-audit.md` borderline call #1: the **residency slab is a one-shot allocation at app boot**. `GrowableBuffer<T>` is NOT used for the slab.

The existing `segment_voxel_buffer` (`crates/bevy_naadf/src/render/construction/mod.rs:1527-1547`) is **already** the residency slab — per-segment cubic 128 MiB. Streaming reuses it verbatim.

What is inside the slab (managed by residency):
- `segment_voxel_buffer` — 128 MiB at `SEGMENT_CHUNKS³ × 2048 u32 × 4 B`. **One segment's worth of u32 voxels.** Currently scratch for the per-segment generator+chunk_calc; streaming inherits this layout.
- The W5 producer's **output** (`chunks_cpu / blocks_cpu / voxels_cpu` worth of state for the slot) lives in the existing `WorldGpu.chunks_buffer / blocks / voxels` buffers, NOT in the segment_voxel_buffer. Those are sized per the W5 fixed-world worst case at startup by `render/prepare.rs:172` and are themselves the long-term residency state.

What stays outside the slab (kept growable / palette-style):
- The voxel-type palette (`VoxelTypes`).
- Residency metadata (`Residency::slot_to_world`, `slot_state`) — main-world resource, CPU.
- W2 `changed_*_dynamic` record buffers — already `GrowableBuffer`-style, sized for the largest per-frame batch.
- `model_data_chunk_buffer / block_buffer / voxel_buffer` — these stay allocated even though streaming bypasses the W5 generator (see § C below); the `ConstructionBindGroups` lifecycle requires them present (`mod.rs:1596-1614`).

**Default for `--vram-budget-mib`:**

The accounting (from `prepare.rs` + production-scale numbers visible in the W5 producer):

| Buffer | Size | Why |
|---|---|---|
| `segment_voxel_buffer` | **128 MiB** | `16³ × 2048 u32` per the W5 alloc at `mod.rs:1527-1547` |
| `WorldGpu.chunks_buffer` (`Rg32Uint`) | **~64 MiB** | `256 × 32 × 256 × 2 u32 × 4 B = 64 MiB` |
| `WorldGpu.blocks` (worst case) | **~256 MiB** | Vulkan-baseline cap; sized by `render/prepare.rs:172` 2× headroom |
| `WorldGpu.voxels` (worst case) | **~256 MiB** | Same shape |
| `hash_map` | ~4 MiB | `1<<18 × 16 B` |
| `bound_*` queues + masks + indirect | ~24 MiB | `mod.rs:1315-1391` |
| **Subtotal slab** | **~732 MiB** | One full resident world container at production fixed-world shape |

Default: `--vram-budget-mib 1024` (1 GiB). Covers the 732 MiB slab + headroom for `model_data` buffers (~32 MiB for noise-source bookkeeping if present, ~0 if streaming bypasses) + the W2 record buffers + palette. Documented as "the streaming-window default that covers the full `WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)` resident set at production scale".

**Resident-segments arithmetic:** `segments_resident = floor(vram_budget_mib / per_segment_mib)`. With `per_segment_mib ≈ 128 + (blocks+voxels per-segment proportional share)`, one full container (512 segments at the production fixed-world shape) costs the ~732 MiB above and is the only configuration this session targets. **The budget knob's *primary purpose* this session is to validate the budget-check assertion fires (gate G), not to shrink the window** — shrinking the window means changing `WORLD_SIZE_IN_SEGMENTS`, which is forbidden by the drift-guard test at `lib.rs:920-946`.

#### A.5 Failure modes

1. **Camera moves faster than CPU noise generates chunks.** Slots stay `SlotState::Generating` / `SlotState::Encoded` past the window's lifetime → on the next shift, those slots are forcibly evicted (their in-flight task results are dropped on receive). The slot becomes Empty in the new window position. Visually: the segment is empty (skybox bleed) until generation catches up. **No stall.** **No hard speed cap.** The user can fly through faster than generation; the cost is empty patches.
2. **Insufficient VRAM.** Pre-flight check at startup compares the configured budget against the slab total. Below threshold → `panic!` with a clear message. **Hard pre-flight check**, not runtime soft cap.
3. **CPU pool exhaustion.** `bevy_tasks::AsyncComputeTaskPool` provides natural backpressure; we bound the in-flight generator-task count to `pool.thread_num()` so the pool is never queue-saturated. Excess admissions wait one frame.

### B. Noise → chunk adapter

**New module:** `crates/bevy_naadf/src/streaming/noise_source.rs` (~180 LOC).

#### B.1 Where it runs — CPU on bevy_tasks pool

The noise source runs on **`AsyncComputeTaskPool`** (Bevy's CPU thread pool for non-realtime work). Per the reference project (`/mnt/archive4/DEV/bevy_voxel_world/bevy_voxel_world/crates/voxel_plugin/src/noise/terrain.rs:106-160`), `FastNoise2Terrain` calls `node.gen_uniform_grid_3d(...)` synchronously on whatever thread the task runs on; the node is `Send + Sync` (`crates/voxel_noise/src/native.rs:128-129`) so per-task ownership is fine. We follow the same shape:

- Construct **one shared `NoiseNode`** at startup (held by `ChunkSource`).
- For each newly-admitted slot, spawn an `AsyncComputeTaskPool` task that:
  1. Calls `gen_uniform_grid_3d` over the segment's voxel extent.
  2. Classifies each voxel into a `VoxelTypeId` (a simple noise-threshold cutoff for `SimpleTerrain`: `noise > 0.0 → solid, else empty`; planet-graph paths add SDF + sea-level interpretation).
  3. Encodes into an `EncodedSegment` via `encode_one_segment` (defined below).
  4. Returns the `EncodedSegment` over a `crossbeam_channel` (or `bevy_tasks::Task<EncodedSegment>` polled per-frame).

**Why CPU and not GPU compute:** the existing W5 `generator_model` GPU pipeline is bit-exact to a fixed `ModelData` lookup — adding a noise-sampling code-path to `generator_model.wgsl` is a brand-new shader. Per the reuse-audit's row 3 + 4: the W5 GPU path generates voxels and `chunk_calc` then re-hashes them, taking the per-segment cubic 128 MiB scratch (one segment at a time, serially per the per-segment-submit ordering bug at `mod.rs:2427-2453`). For procedural streaming we want **N segments generated in parallel** while `chunk_calc` runs on **one segment at a time**. CPU pool gives us the parallel-N for free; GPU compute would either need a re-architected `generator_model.wgsl` (port it from `ModelData` lookup to `gen_single_3d`-equivalent FBM in WGSL — not done by any noise crate this project ships) or a sequential per-segment dispatch matching the current W5 driver, which gives no parallelism over the serial generator path. **The W5 GPU producer is bypassed entirely in the streaming preset.** See § C for what replaces it.

#### B.2 Per-chunk-local dedup encoder

Per `00-reuse-audit.md` borderline call #2 + Q3 ("per-resident-chunk-local dedup, no global/no per-window dedup state"), `aadf::construct(&DenseVolume)` is **NOT called** per chunk — it carries a shared dedup HashMap that would silently still dedup across calls if its inner body were extracted naively.

Add to `crates/bevy_naadf/src/aadf/construct.rs` (after the existing `construct` fn):

```rust
/// Per-chunk encoded buffers — analogue of `ConstructedWorld` for one chunk.
pub struct EncodedChunk {
    /// 1 u32 — the chunk-cell encoding (Empty / UniformFull(ty) / Mixed(ptr)).
    pub chunk_word: u32,
    /// 64 block-cell u32s when the chunk is Mixed; empty Vec otherwise.
    pub blocks: Vec<u32>,
    /// Variable: 32 voxel-pair u32s per Mixed block; empty when chunk is uniform.
    pub voxels: Vec<u32>,
}

/// Encode one 16³-voxel chunk into the three-layer NAADF format with
/// **per-chunk-local dedup only** (the HashMap is constructed inside the fn,
/// dropped at return — no cross-chunk reuse).
pub fn encode_one_chunk(voxel_types: &[VoxelTypeId; 16 * 16 * 16]) -> EncodedChunk {
    /* port the inner per-chunk body of `construct`; the HashMap is local;
       no &mut HashMap parameter, no shared cursor */
}
```

A `EncodedSegment` is `Vec<EncodedChunk>` of length `SEGMENT_CHUNKS³ = 4096`. Per-chunk-local dedup means each chunk's `voxels` vec is independently slot-pointer-zeroed: the W2 upload (§ C) rebases the pointers as it writes them into the residency buffers.

#### B.3 Per-chunk timing + parallelism

The relevant numbers we have (from `voxel_noise` edge-coherency tests + `crates/bevy_naadf/src/render/construction/mod.rs:2425-2566` per-segment cost as a referent):

- `voxel_noise::gen_uniform_grid_3d` on a 32³ sample (from `test_simple_terrain` etc) runs sub-millisecond on release builds (assumption — not benchmarked here, see § Assumptions).
- Per-chunk extent is 16³ = 4096 samples. Per-segment = 16³ chunks × 16³ voxels/chunk = 4096³ voxels = ~16.7 M samples.
- **`per_segment_ms` estimate: ~30 ms** on a 6-core CPU (assumption — see § Assumptions; tighter than chunk-by-chunk because `gen_uniform_grid_3d` is FastNoise2's optimised SIMD path, far faster than 4096 chunk-by-chunk calls).
- Encoding (`encode_one_chunk` × 4096 chunks per segment) ≈ comparable to W5 CPU oracle's `generate_segment_cpu` (`crates/bevy_naadf/src/aadf/generator.rs:239-335`) cost shape: 2048 u32s/chunk × 4096 chunks = 8.4 M u32 writes ≈ tens of ms wall-time, parallelisable across the pool.

**Little's Law:**

```
chunks_per_frame_max = pool_threads × (frame_ms / per_chunk_ms_estimate)
                    = ~6              × (16 ms / 0.01 ms)
                    = ~9600 chunks / frame headroom
```

But segments-per-frame, not chunks-per-frame, is the rate that matters (parallelism unit is one segment):

```
segments_per_frame_max ≈ pool_threads × (frame_ms / per_segment_ms_estimate)
                       = 6             × (16 / 30)
                       = ~3 segments / frame
```

For a 60 fps demo with one fully populated 512-segment window at startup, the cold-start window fills in `512 / 3 ≈ 170 frames ≈ 3 s`. Once warm, only the per-shift delta (per-edge `16 × 2 × 1 = 32` segments per axis shift) generates → comfortably ≤ 1 frame at the upper bound. **No hard cap on traversal speed** (per A.5); fast traversal degrades visually to empty patches, not stalls.

The pool's thread count is queried via `AsyncComputeTaskPool::get().thread_num()` at startup; we do not hardcode 6.

#### B.4 The exact `gen_uniform_grid_3d` call

For a per-chunk generator (`WorldChunkPos = WorldSegmentPos.0 * SEGMENT_CHUNKS + (cx, cy, cz)` per chunk inside the segment), the noise call for one segment-sized 256³ voxel block is **one call**, not 4096:

```rust
// In streaming/noise_source.rs:
fn generate_segment(node: &NoiseNode, seg: WorldSegmentPos, seed: i32) -> Vec<f32> {
    const VOX_PER_AXIS: i32 = (SEGMENT_CHUNKS * 16) as i32; // 256
    let voxel_step: f32 = 1.0; // 1 voxel = 1 noise-sample
    let frequency: f32 = 0.02; // matches voxel_noise SimpleTerrain preset test default

    let world_voxel_origin = seg.0 * (SEGMENT_CHUNKS as i32) * 16;
    let x_off = (world_voxel_origin.x as f32) * frequency;
    let y_off = (world_voxel_origin.y as f32) * frequency;
    let z_off = (world_voxel_origin.z as f32) * frequency;
    let step  = voxel_step * frequency;

    let mut out = vec![0.0f32; (VOX_PER_AXIS * VOX_PER_AXIS * VOX_PER_AXIS) as usize];
    node.gen_uniform_grid_3d(
        &mut out,
        x_off, y_off, z_off,
        VOX_PER_AXIS, VOX_PER_AXIS, VOX_PER_AXIS,
        step, step, step,
        seed,
    );
    out
}
```

The output is **X-fastest** indexing per FastNoise2 (`crates/voxel_noise/src/lib.rs:185-199`). After the noise call we walk the buffer in `dense_voxel_index(x, y, z)` order (X-fastest), threshold each sample into a `VoxelTypeId` (`SimpleTerrain`: `noise > 0.0 → TY_GROUND`, else `EMPTY`; planet-SDF preset: subtract sphere-SDF + threshold), then group 16³ voxel windows into chunks for `encode_one_chunk`.

### C. GPU upload — reuse W2 (with one targeted bypass)

Per the reuse-audit's row #7 and Q1/Q2 the streaming layer **synthesises records into `WorldData.pending_edits.batches`** (`crates/bevy_naadf/src/world/data.rs:80, 1370-1379`) — the existing W2 chain (`extract_world_changes → naadf_world_change_node`'s 4 GPU compute passes) handles upload byte-identically.

#### C.1 Synthesis path

When an `EncodedSegment` (a `Vec<EncodedChunk>` of length 4096) becomes ready:

1. The residency system computes window-local chunk-coords for every chunk in the segment: `(slot_x * SEGMENT_CHUNKS + cx, slot_y * SEGMENT_CHUNKS + cy, slot_z * SEGMENT_CHUNKS + cz)` where `slot_x/y/z` derive from `slot_index` via `WORLD_SIZE_IN_SEGMENTS` strides.
2. Per chunk, pack `(window_local_pos, new_chunk_word)` via `aadf::edit::pack_chunk_pos` (`crates/bevy_naadf/src/aadf/edit.rs:203-205`). The packing `pos.x | y<<11 | z<<21` is the exact W2 format.
3. The chunk's `Vec<EncodedChunk>.blocks` (when Mixed) is rebased: the local block-pointer becomes a global block-pointer via the W2 cursor accounting in `process_edit_batch` (`aadf/edit.rs:250-338`). Same for voxels.
4. The 4096 records are pushed into a single `EditBatch` (`changed_chunks: Vec<[u32; 2]>`, `changed_blocks: Vec<u32>`, `changed_voxels: Vec<u32>`).
5. `WorldData.pending_edits.batches.push(batch)`. The next `extract_world_changes` system runs the W2 chain over it.

This produces **N chunks × 4 GPU compute passes** (= 4096 × 4 = 16384 record-driven invocations per segment) which is exactly the bandwidth the existing W2 chain already supports on a brush stroke. The driver does not invent a new upload path.

#### C.2 Window-local vs world-chunk-coord translation

The W2 packing is `(cx:11, cy:10, cz:11)` (`aadf/edit.rs:62-69` for unpacking, `:203-205` for packing). Per Q1, the residency layer translates **`IVec3 world-chunk-coord → u32 window-local-coord`** before packing:

```rust
fn world_chunk_to_window_local(p: WorldChunkPos, origin_in_chunks: IVec3) -> Option<[u32; 3]> {
    let d = p.0 - origin_in_chunks; // origin_in_chunks = origin_in_segments * SEGMENT_CHUNKS
    if d.x < 0 || d.y < 0 || d.z < 0 { return None; }
    if d.x >= WORLD_SIZE_IN_CHUNKS.x as i32
       || d.y >= WORLD_SIZE_IN_CHUNKS.y as i32
       || d.z >= WORLD_SIZE_IN_CHUNKS.z as i32 { return None; }
    Some([d.x as u32, d.y as u32, d.z as u32])
}
```

The packing caps at `2048 × 1024 × 2048` chunks; `WORLD_SIZE_IN_CHUNKS = (256, 32, 256)` is comfortably under. **No shader-side packing changes** (Q1).

#### C.3 Eviction — empty-record writes

Eviction is the **inverse** of admission: the residency system synthesises an `EditBatch` whose `changed_chunks` entries have `new_state = 0` (an Empty `ChunkCell` per `aadf/cell.rs`'s `ChunkCell::Empty(0).encode()`). The same W2 chain writes the Empty-state across the evicted slot's 4096 chunks.

Empty-state writes do NOT need `changed_blocks` / `changed_voxels` payloads (the chunk's discriminator alone determines no-block-storage). This is a fraction of an admit's record size — a full eviction batch is `4096 × 2 u32 = 32 KiB` of `changed_chunks`. Cheap.

#### C.4 No new GPU upload pipeline

Reusing W2 means the impl agent **does not** write a new shader, **does not** add a `RenderQueue::write_buffer` per-segment fast path, **does not** touch `naadf_gpu_producer_node`. The W5 generator chain at `mod.rs:2384-2566` is **dead code** in the streaming preset: `model_data` is `None`, `dense_voxel_types` is empty, so the three-way ladder falls through to branch (c) early-return (`mod.rs:2384-2566` body's final `else`). See § D for how we ensure that.

### D. Driver — invert the once-at-startup gate

#### D.1 New per-frame residency driver system

**New system:** `streaming::residency::residency_driver` (main world), scheduled in `PreUpdate` after `camera::sync_position_split` (which sets `PositionSplit` from `Transform`). System order:

```
PreUpdate:
  camera::sync_position_split (existing, lib.rs:691)
  streaming::residency::residency_driver (NEW)
    1. read camera PositionSplit → WorldSegmentPos
    2. if camera_seg != prev_camera_seg:
         compute target_origin, evictions, admissions
         spawn admission tasks on AsyncComputeTaskPool
         push eviction EditBatch into WorldData.pending_edits
    3. drain finished admission tasks (poll Task<EncodedSegment>):
         synthesise EditBatch from EncodedSegment
         push into WorldData.pending_edits
         mark slot SlotState::Resident
```

The existing `extract_world_changes` system + `naadf_world_change_node` (regime-3) runs every frame regardless, so as long as we push into `pending_edits` the GPU upload propagates with no further plumbing.

#### D.2 Disable the W5 once-at-startup gate

The W5 driver (`crates/bevy_naadf/src/render/construction/mod.rs:2454-2566`) runs **only** when `ModelDataRender` is present (the `if let Some(model_data) = model_data.as_deref()` gate at `mod.rs:2384`). The streaming-window preset **does not install `ModelDataRender`** (`GridPreset::ProceduralStreaming` skips the `model_data` insertion entirely) so:
- Branch (a) at `mod.rs:2384` is skipped.
- Branch (b) at the same site fires only if `world_data_meta.dense_voxel_types` is non-empty; the streaming install path leaves it empty, just like `install_default_embedded_in_fixed_world` already does (`crates/bevy_naadf/src/voxel/grid.rs:136-228`).
- Branch (c) early-returns. The W5 producer never runs.

**No code is deleted from `mod.rs`.** The W5 path stays available for `GridPreset::Vox` and the existing default-scene gate. The streaming preset just goes through neither branch.

#### D.3 Per-segment-submit ordering

The W5 per-segment-submit constraint at `mod.rs:2427-2453` is **inherited**: when the residency driver issues uploads via the W2 chain, each `pending_edits.batches` entry maps to one full W2 compute-pass run in `naadf_world_change_node`. The W2 chain already submits per-frame (regime-3) without the W5 per-segment-encoder-per-submit pattern (because the W2 records are `RenderQueue::write_buffer`'d into the `changed_*_dynamic` buffers and the 4-pass dispatch reads them post-write, all on the same encoder). **The streaming driver does not break the W5 ordering bug fix** — it does not touch the W5 code path at all.

#### D.4 Interaction with the once-at-startup gate

For the streaming preset, the W5 startup gate **never fires** by construction (no `model_data`, no `dense_voxel_types`). The `gpu_producer_has_run` flag stays `false` indefinitely — harmless (it gates only the dead W5 producer branch).

The initial resident set at app boot is computed by the residency driver on its **first** `PreUpdate` run: `prev_camera_seg = None` triggers the full admission of every segment in the camera-centred window. The 512-segment cold-start fills in ~170 frames per § B.3.

### E. Coordinate widening (residency-only `i32`)

Per Q1, residency manager keys chunks on `IVec3` (i32) world-chunk-coords. The GPU bind layout stays window-local. No `i64` / `f64` widening.

#### E.1 Representations

```rust
// In streaming/residency.rs:

/// World-segment coord in IVec3 (i32). Newtype to disambiguate from
/// WorldChunkPos and from "voxel IVec3" used in PositionSplit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldSegmentPos(pub IVec3);

/// World-chunk coord (one segment = 16³ chunks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldChunkPos(pub IVec3);

impl WorldSegmentPos {
    pub fn chunk_origin(self) -> WorldChunkPos {
        WorldChunkPos(self.0 * (SEGMENT_CHUNKS as i32))
    }
}
```

The `Residency::origin` (§ A.2) is an `IVec3` in **segment** units. The "GPU-side world origin" derived from it is `origin_in_chunks = Residency::origin * SEGMENT_CHUNKS` (in chunks) and `origin_in_voxels = origin_in_chunks * 16` (in voxels).

#### E.2 Origin as a `Resource`

```rust
#[derive(Resource, Default)]
pub struct StreamingWorldOrigin {
    pub origin_in_segments: IVec3, // mirror of Residency::origin
    pub origin_in_chunks: IVec3,
    pub origin_in_voxels: IVec3,
}
```

Updated by the residency driver every time `Residency::origin` shifts. Read by:
- The W2 record-synthesis path (window-local translation per § C.2).
- The camera shader-uniform path: `PositionSplit::pos_int` is in **voxels**. The shader sees `pos_int - origin_in_voxels` as a camera position relative to the residency window's origin — this is the "shader sees only window-local" guarantee (Q1).

Caveat: the shader-uniform integration in `crates/bevy_naadf/src/render/prepare.rs` already uploads `PositionSplit` to the GPU. We add a **second** `IVec3` field `world_origin_in_voxels_int` to the per-frame camera uniform; the shader subtracts it from `pos_int` before any DDA. This keeps the existing `PositionSplit` math intact and adds **one subtraction** per shader to make the camera window-local.

#### E.3 Unit-conversion rule

The `PositionSplit` camera is in **voxels** (`pos_int: IVec3` voxels + `pos_frac: Vec3` sub-voxel). The streaming layer is in **segments**. Conversion:

```rust
fn camera_to_segment(p: &PositionSplit) -> WorldSegmentPos {
    WorldSegmentPos(p.pos_int.div_euclid(IVec3::splat(
        SEGMENT_CHUNKS as i32 * 16
    )))
}
```

`div_euclid` (not `/`) so negative coords land on the correct segment (`-1` voxels → segment `-1`, not segment `0`).

### F. New `GridPreset` variant + CLI

#### F.1 `GridPreset::ProceduralStreaming`

**Edit `crates/bevy_naadf/src/lib.rs:65-78`:**

```rust
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub enum GridPreset {
    #[default]
    Default,
    Vox { path: std::path::PathBuf },
    /// Procedurally-generated streaming world (this dispatch). See
    /// `docs/orchestrate/streaming-world/02-design.md`.
    ProceduralStreaming {
        /// Noise preset id, mapped to `voxel_noise::NoisePreset`.
        noise_preset: u32,
        /// Random seed for FastNoise2.
        seed: i32,
    },
}
```

Name justification: `ProceduralStreaming` — `Procedural` distinguishes it from a future `VoxStreaming` variant (pre-made world streaming) the design must not preclude (§ H); `Streaming` distinguishes it from a hypothetical `Procedural` variant that one-shots a noise-built world via the existing W5 producer.

**Edit `crates/bevy_naadf/src/voxel/grid.rs::setup_test_grid` (line 104)** to add a match arm:

```rust
GridPreset::ProceduralStreaming { noise_preset, seed } => {
    install_procedural_streaming_world(&mut commands, *noise_preset, *seed);
}
```

`install_procedural_streaming_world` (~80 LOC, same file) inserts an empty `WorldData` (size = `WORLD_SIZE_IN_CHUNKS`, dense_voxel_types empty), the `VoxelTypes` palette, the streaming `ChunkSource` resource (a `NoiseNode` + seed wrapped in `Arc` for the task pool), the `Residency` resource, the `StreamingWorldOrigin` resource, and the `InitialCameraPose` (spawned at the world's voxel-XZ-centre, `Y = ~surface_height_estimate + 30` voxels).

#### F.2 CLI

**Edit `crates/bevy_naadf/src/lib.rs::AppArgs` (line 259):**

```rust
pub struct AppArgs {
    // ... existing fields ...

    /// VRAM budget for the streaming residency slab (MiB). Default 1024.
    /// See `docs/orchestrate/streaming-world/02-design.md` § A.4.
    pub vram_budget_mib: u32,
}
```

Default: `1024` (1 GiB), per § A.4.

**Edit `crates/bevy_naadf/src/bin/bevy-naadf.rs` + `crates/bevy_naadf/src/bin/e2e_render.rs`** to parse:
- `--streaming-window` (boolean, sets `grid_preset = ProceduralStreaming{..}`).
- `--noise-preset <SimpleTerrain|PlanetTerrain|SurfaceDetail>` (default `SimpleTerrain`).
- `--seed <i32>` (default `1337`, matches the `voxel_noise` test default).
- `--vram-budget-mib <N>` (default `1024`).

The existing `e2e_render.rs:71-130` flag-parsing block is hand-rolled (`args.iter().any(|a| a == "--flag")`); add 4 more lines following the same pattern. **No `clap` dependency added.**

### G. New `--streaming-window` e2e gate

**New module:** `crates/bevy_naadf/src/e2e/streaming_window.rs` (~280 LOC, modelled exactly on `e2e/oasis_edit_visual.rs:1-200`). Add a new flag handler in `crates/bevy_naadf/src/bin/e2e_render.rs` after the existing `oasis_edit_visual_mode` branch at line 249.

#### G.1 Gate driver phases

Modelled on the `OasisWarmup → OasisShoot → OasisApplyEdit → OasisWait → OasisShoot → OasisAssert` shape:

1. **`StreamingBoot`** — install `GridPreset::ProceduralStreaming { noise_preset: SimpleTerrain, seed: 1337 }`, spawn the e2e camera at the world's central segment, position (`WORLD_SIZE_IN_VOXELS.x / 2`, surface + 30, `WORLD_SIZE_IN_VOXELS.z / 2`).
2. **`StreamingWarmupA`** — 200 frames (covers the ~170-frame cold-start + TAA convergence). Asserts that by frame 170, `Residency::slot_state` is all `Resident` (sanity check on the cold-start arithmetic).
3. **`StreamingShootA`** — capture framebuffer A → `target/e2e-screenshots/streaming_before_shift.png`. Run **assertion A**: framebuffer mean luminance over central rect > floor (`STREAMING_TERRAIN_LUMINANCE_FLOOR = 30.0`) — terrain is visible (not skybox).
4. **`StreamingShift`** — programmatically translate the camera by `(SEGMENT_CHUNKS × 16 × 2, 0, 0)` voxels (2 segments along X). This crosses 2 segment boundaries, forcing eviction of 2 columns of segments along the trailing X-edge and admission of 2 columns along the leading X-edge.
5. **`StreamingWarmupB`** — 200 frames (admission + W2 4-pass propagation + TAA reconverge over the new content).
6. **`StreamingShootB`** — capture framebuffer B → `streaming_after_shift.png`. Run **assertions B, C, D**:
   - **B.** Mean luminance over central rect of B > floor → terrain renders at the new camera position.
   - **C.** Mean luminance over a rect *behind* the camera (corresponding in voxel-coords to the *old* camera position, now outside the resident window) is **at-or-below skybox floor** (the evicted region is no longer in the resident window → ray-marching into it hits empty → skybox). Concretely: compute screen-space rect via camera-relative projection of the OLD camera origin (`prev_origin + (-1, 0, 0) * shift`) and assert mean luminance < `STREAMING_EVICTED_LUMINANCE_CEILING = 50.0` (or even below the skybox baseline, ~35-45).
   - **D.** Query `RenderDevice::limits()` + the actual `segment_voxel_buffer.size()` + the sum of `WorldGpu` chunks/blocks/voxels buffer sizes + the W2 records buffer sizes. Assert `total_residency_slab_mib` is within `±5%` of `--vram-budget-mib` (the budget knob is honoured within slab-allocation rounding tolerance).

#### G.2 Assertion thresholds

- `STREAMING_TERRAIN_LUMINANCE_FLOOR = 30.0` — terrain mean luminance is comfortably above the skybox (~35-45 over a small rect of pure sky; terrain w/ shading is typically 80-180). 30 sits just below the skybox floor so the test fails if NO terrain renders.
- `STREAMING_EVICTED_LUMINANCE_CEILING = 50.0` — slightly above pure skybox to allow GI bleed from adjacent resident segments. A regression that leaves the evicted slot's u32-encoded chunk state stuck (instead of writing zeros) would render the old terrain → luminance lifts back to the 80-180 terrain range → test fails.
- `STREAMING_VRAM_TOLERANCE = 0.05` — 5% tolerance, absorbs the per-buffer alignment padding wgpu adds (typically `next_multiple_of(buffer_size, 4)` or 256-byte alignment for storage).

#### G.3 e2e_render wiring

**Edit `crates/bevy_naadf/src/bin/e2e_render.rs:101`** add:

```rust
let streaming_window_mode = args.iter().any(|a| a == "--streaming-window");
```

**Edit the dispatch ladder at `e2e_render.rs:179-320`**, insert a new branch (after the existing `oasis_edit_visual_mode` branch at line 249):

```rust
} else if streaming_window_mode {
    bevy_naadf::e2e::streaming_window::run_streaming_window()
}
```

The new `run_streaming_window` function follows the `run_oasis_edit_visual` shape (line 182): build `AppArgs::default()`, override `grid_preset = GridPreset::ProceduralStreaming{..}`, override `vram_budget_mib`, call `bevy_naadf::run_e2e_render_with_args(app_args)`.

#### G.4 Module + flag registration

- New module exported from `crates/bevy_naadf/src/e2e/mod.rs` (mirror `pub mod oasis_edit_visual;` line).
- `AppArgs.streaming_window_mode: bool` field (line 376 area), so the e2e harness driver knows which phase ladder to run (per the `vox_gpu_construction_mode` pattern).

### H. Forward-compatibility seams (groundwork only — not wired)

#### H.1 `trait ChunkSource`

**New module:** `crates/bevy_naadf/src/streaming/chunk_source.rs` (~50 LOC, this session's implementation has exactly ONE impl):

```rust
/// A source that produces an encoded segment for a given world-segment coord.
/// One impl this session (NoiseChunkSource); future impls: VoxChunkSource,
/// MinecraftChunkSource.
pub trait ChunkSource: Send + Sync + 'static {
    /// Produce an `EncodedSegment` for `seg`. Blocks; called on the
    /// AsyncComputeTaskPool, not the main thread.
    fn produce(&self, seg: WorldSegmentPos) -> EncodedSegment;
}

/// The noise-source impl (this session's only impl).
pub struct NoiseChunkSource {
    pub node: std::sync::Arc<voxel_noise::NoiseNode>,
    pub seed: i32,
    pub frequency: f32,
}

impl ChunkSource for NoiseChunkSource {
    fn produce(&self, seg: WorldSegmentPos) -> EncodedSegment {
        /* generate_segment() + classify + encode_one_chunk × 4096 */
    }
}
```

The residency driver holds `Box<dyn ChunkSource>` (or `Arc<dyn ChunkSource>` for cheap clone into tasks). Future `.vox`-streaming swap is a `Box::new(VoxChunkSource::new(path))` — no residency-layer changes.

#### H.2 Future cross-segment chunk eviction

The current `Residency::slot_to_world` is per-segment. To support per-chunk eviction later, the data structure that would change is `slot_state: Vec<SlotState>` → `slot_state: Vec<Vec<ChunkSlotState>>` (per-chunk-within-segment). The `EncodedSegment = Vec<EncodedChunk>` shape already supports this — the per-chunk encoding seam is the future eviction unit. **What we DO NOT hardcode:** the "one slot = one segment" assumption is confined to `streaming/residency.rs`; the W2 upload synthesis (§ C) writes per-chunk records, not per-segment records, so the upload layer needs zero changes for per-chunk eviction.

The W5 `segment_voxel_buffer` (one slot's scratch space) IS hardcoded to one segment — that's fine, it's a scratch buffer, not the residency state. Per-chunk eviction would just reduce how much of it is touched per generate-pass.

### File-level diff sketch

| Action | Path | Approx LOC | Why |
|---|---|---|---|
| new file | `crates/bevy_naadf/src/streaming/mod.rs` | ~50 | Module root + `StreamingPlugin` |
| new file | `crates/bevy_naadf/src/streaming/residency.rs` | ~250 | Residency manager, driver system, shift math |
| new file | `crates/bevy_naadf/src/streaming/noise_source.rs` | ~180 | `NoiseChunkSource`, `generate_segment` |
| new file | `crates/bevy_naadf/src/streaming/chunk_source.rs` | ~50 | `trait ChunkSource` + `EncodedSegment` shape |
| new file | `crates/bevy_naadf/src/e2e/streaming_window.rs` | ~280 | `--streaming-window` e2e gate |
| edit | `crates/bevy_naadf/src/aadf/construct.rs` | +~120 | Add `encode_one_chunk` per § B.2 |
| edit | `crates/bevy_naadf/src/voxel/grid.rs` | +~80 | `install_procedural_streaming_world` + arm at `setup_test_grid:104` |
| edit | `crates/bevy_naadf/src/lib.rs:65-78` | +~10 | `GridPreset::ProceduralStreaming` variant |
| edit | `crates/bevy_naadf/src/lib.rs:259` | +~6 | `AppArgs.vram_budget_mib` + `streaming_window_mode` |
| edit | `crates/bevy_naadf/src/lib.rs:655-682` | +1 | Register `StreamingPlugin` in `build_app` |
| edit | `crates/bevy_naadf/src/e2e/mod.rs` | +1 | `pub mod streaming_window;` |
| edit | `crates/bevy_naadf/src/bin/e2e_render.rs:101, 249` | +6 | Flag-parse + dispatch branch |
| edit | `crates/bevy_naadf/Cargo.toml` | +1 | `voxel_noise = { path = "../voxel_noise" }` |
| **no edit** | `crates/bevy_naadf/src/render/construction/mod.rs:2454-2566` | 0 | Streaming preset bypasses W5 via the existing three-way gate |
| **no edit** | `crates/bevy_naadf/src/aadf/edit.rs` | 0 | W2 packing/extract reused as-is |
| **no edit** | `crates/bevy_naadf/src/render/construction/world_change.rs` | 0 | W2 GPU pipelines reused as-is |
| **no edit** | any `.wgsl` shader | 0 | Per Q1, no shader-side packing changes |

Total new LOC: ~810. Total touched-LOC across edits: ~225. Zero shader changes. Zero W5 changes. Zero W2 shader/pipeline changes — only an additional synthesiser of W2-format records.

---

## Δ-StreamingResidency

This design diverges from C# NAADF and is approved per `01-context.md` Q&A Step 4.

**The divergence:** C# NAADF (`NAADF/World/Data/WorldData.cs:120-156`) runs a **one-shot `GenerateWorld` at startup**: the `for (z) for (y) for (x)` segment loop dispatches the entire `WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)` world once. After it, the GPU world contains every voxel forever; the camera roams within a fixed `4096³ × 512` voxel container; the only mutations are user edits via `EditingHandler.processChunks`.

This design replaces that one-shot generation with a **per-frame sliding-window residency driver** (`streaming::residency::residency_driver`, § D.1). The driver checks each frame whether the camera has crossed a segment boundary; if so, it evicts the segments outside the new window AABB and admits the segments inside it. World content is generated **lazily** as the camera reaches each region.

**User-stated motivation** (`01-context.md`):

> "lets implement a way to open or stream large .vox files (2.1G - upper limit of vox file) … we have to be able to demo large voxel worlds … we would implement a world import process for that. this would also require a sliding window approach, for streaming in worlds larger than VRAM allows under specified VRAM budget, generating infinite worlds, a large coordinate system."

**C# surfaces replaced:**

- `NAADF/World/Data/WorldData.cs:120-156` (`GenerateWorld`) — the orchestrator. Replaced by `streaming::residency::residency_driver`.
- `NAADF/World/Generator/WorldGeneratorModel.cs:11-22` (`CopyToChunkData(Point3 chunkPos, ...)` — the per-segment dispatch entry) — the per-segment generator. Replaced by `streaming::noise_source::NoiseChunkSource::produce + encode_one_chunk` running on the CPU pool.

**What this divergence preserves:**

- The W2 edit chain (`EditingHandler.processChunks` analogue in `WorldData::set_voxels_batch`) is **untouched**. Streaming admissions piggyback the same per-chunk record format brush edits already use.
- The `(cx:11, cy:10, cz:11)` packing in `world_change.wgsl` is **preserved** (Q1).
- The `WORLD_SIZE_IN_*` fixed-world constants (`lib.rs:209-250`) are **preserved**. The world container does not change shape; only its origin shifts.
- The W5 once-at-startup producer is **preserved as dead code in the streaming preset** — `GridPreset::Default` and `GridPreset::Vox` still take that path.

**Approval status:** per `01-context.md` § Q&A Step 4 (the user explicitly motivated the divergence). The four load-bearing decisions (Q1 i32 residency-only widening, Q2 per-segment residency unit, Q3 per-chunk-local dedup, Q4 voxel_noise backend) are recorded and this design reflects all four.

---

## Decisions & rejected alternatives

### D.1 Indirection table: dense `Vec<Option<...>>` vs `HashMap`
- **Chosen:** Dense `Vec<Option<WorldSegmentPos>>` of length 512, plus a sparse `HashMap<WorldSegmentPos, SlotIndex>` for the reverse lookup.
- **Rejected:** `HashMap<WorldSegmentPos, SlotIndex>` only.
- **Reason:** the forward map is hit once per frame per slot in eviction-sweep iteration order; a 512-Vec is faster, simpler, and naturally `IntoIter`-able. The HashMap is reserved for the reverse lookup where its O(1) random-access beats a 512-element linear scan. Hybrid wins.
- **Fact that flips this:** if `WORLD_SIZE_IN_SEGMENTS` widens to 100k+ slots in a future per-chunk-eviction extension, switch the forward map to a `HashMap`-equivalent.

### D.2 Where noise runs: CPU pool vs GPU compute
- **Chosen:** CPU `AsyncComputeTaskPool`. One `NoiseChunkSource` task per admitted segment.
- **Rejected:** A new GPU compute pipeline that consumes a `gen_uniform_grid_3d`-equivalent noise buffer.
- **Reason:** (a) parallelism — the CPU pool runs N segments in parallel for free; the GPU path inherits the W5 per-segment-submit serialisation. (b) implementation cost — porting FastNoise2's FBM + domain-warp to WGSL is a brand-new shader nobody in this workspace has, vs reusing the proven `voxel_noise` crate as the reference project already does. (c) memory — keeping the noise output on the CPU side lets us run `encode_one_chunk` immediately without a `RenderDevice` readback round-trip.
- **Fact that flips this:** if profile data shows the CPU pool can't keep up at the target traversal speed (≥ 1 segment / frame at 60 fps), the GPU compute path becomes attractive — but at that point we'd want `voxel_noise` to ship a WGSL backend or use a separate JFA-style generator.

### D.3 Upload path: synthesise W2 records vs new bulk `RenderQueue::write_buffer`
- **Chosen:** Synthesise W2 record batches (`pending_edits.batches`). 4096 records per segment admission; same path brush edits use.
- **Rejected:** A new bulk-upload code path that `RenderQueue::write_buffer`s the entire segment's worth of `blocks` / `voxels` data in one shot.
- **Reason:** zero new GPU code, zero new shader, zero new bind-group; the W2 chain is proven byte-exact and the throughput cost (4 compute passes × 4096 records) is what brushes already pay on a large `set_voxels_batch`. The bulk path would need a per-segment offset table on the GPU, a new bind-group layout, and proof of equivalence with the W2 record semantics.
- **Fact that flips this:** if the per-frame W2 cost dominates the frame time at full window shift (e.g., 32 segments admitted in 1 frame = 32 × 4 compute passes = 128 dispatches), a fast-path bulk upload becomes valuable. The current 1-frame cap on shift size leaves room for this if needed.

### D.4 Window shape: AABB vs sphere/cylinder
- **Chosen:** Rectangular AABB matching `WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)`. X/Z slide, Y fixed at full-height.
- **Rejected:** Sphere around camera (Manhattan-distance), Cylinder (radius-by-XZ + full Y).
- **Reason:** the existing GPU bind layout assumes a rectangular world container (`chunks_buffer` is sized `WORLD_SIZE_IN_CHUNKS.x * y * z`, indexed by `(cx, cy, cz)` in `world_change.wgsl`). A sphere would force per-slot validity bits in the bind layout — a shader change. The AABB matches what the GPU already binds.
- **Fact that flips this:** if a future per-chunk eviction unit makes the partial-population pattern worth a shader change, a sphere becomes an option.

### D.5 Window-shift trigger: per-segment vs per-chunk vs per-voxel
- **Chosen:** Per-segment trigger. Window shifts only when the camera crosses a segment boundary (256-voxel grid cell).
- **Rejected:** Per-chunk trigger (16-voxel boundary), per-voxel hysteresis.
- **Reason:** Q2 mandate (residency unit = segment). Per-chunk would cost ~16× more shift evaluations + ~16× tighter eviction batches per second.
- **Fact that flips this:** if Q2 changes to per-chunk in a future session, the trigger granularity follows naturally.

### D.6 Encoding strategy: extract `aadf::construct` inner body vs fresh `encode_one_chunk`
- **Chosen:** Fresh `encode_one_chunk(&[VoxelTypeId; 4096]) -> EncodedChunk` per `00-reuse-audit.md` borderline call #2.
- **Rejected:** Lift the inner body of `aadf::construct` into a `pub(crate)` helper.
- **Reason:** `construct`'s inner body uses a *shared* HashMap parameter for cross-chunk dedup. Extracting naively risks silent cross-chunk dedup on a future caller (correctness regression that's hard to spot). A fresh fn owning its own local HashMap is bulletproof.
- **Fact that flips this:** if global content-addressed storage is added in a future session (a `BlockHashingHandler`-like across-segments cache), extract the inner body and pass the shared cache explicitly.

### D.7 Failure mode under fast traversal: stall vs empty-patches vs hard cap
- **Chosen:** Empty patches (visual degradation, no stall).
- **Rejected:** Stall the frame loop until generation completes; hard cap on camera speed.
- **Reason:** the demo target (per the user's brief) is "demo large voxel worlds" — frame stalls under traversal are catastrophic for a demo. Visual artefacts are recoverable as the user slows down.
- **Fact that flips this:** for a recorded demo path where the camera trajectory is known in advance, a pre-generation pass (prefetch the path's resident sets) is preferable to both stalls and gaps.

### D.8 GridPreset variant name
- **Chosen:** `ProceduralStreaming { noise_preset: u32, seed: i32 }`.
- **Rejected:** `Streaming { source: ChunkSourceKind }`, `Procedural { ... }` (without streaming).
- **Reason:** the brief explicitly motivates "streaming = procedural this session, but pre-made world later" (§ H). Naming this variant `Streaming` would force the future pre-made-world variant to rename; naming it `Procedural` would make the future `VoxStreaming` variant feel different when it shouldn't. `ProceduralStreaming` is explicit + the obvious sibling for `VoxStreaming`.
- **Fact that flips this:** if § H's future trait-based `ChunkSourceKind` enum lands first, the variant collapses to `Streaming { source: ChunkSourceKind }`.

### D.9 Default `--vram-budget-mib`
- **Chosen:** `1024` (1 GiB).
- **Rejected:** `512` (forces a half-window), `2048` (waste).
- **Reason:** the full `WORLD_SIZE_IN_SEGMENTS = (16, 2, 16) = 512`-slot resident slab costs ~732 MiB at production fixed-world shape (§ A.4). 1024 covers it + headroom for W2 records, model buffers, palette. Below 512 the budget assertion would fail at startup — useful as a test target but a bad default. Above 2048 wastes VRAM on systems that don't have it (the design target is `≥ 4 GiB VRAM` GPUs).
- **Fact that flips this:** if `WORLD_SIZE_IN_SEGMENTS` grows in a future session, recompute the per-segment-slab × 512.

### D.10 The W5 once-at-startup gate: invert vs disable
- **Chosen:** **Disable** for streaming preset (the existing three-way gate ladder at `mod.rs:2384` naturally falls through to "do nothing" when `model_data` and `dense_voxel_types` are both empty).
- **Rejected:** Invert the gate (make it per-frame, run only for newly-resident segments via a `gpu_producer_needs_run: Vec<SlotIndex>` queue).
- **Reason:** the W5 GPU producer is bypassed entirely by streaming (per § B's CPU-pool choice). Inverting would either (a) duplicate the work the CPU path already does + the upload work the W2 chain already does, or (b) require an alternate "streaming-mode-only" branch that re-uses W5's segment_voxel_buffer but skips chunk_calc — a lot of code surface for a path the CPU pool already covers. Cleaner: streaming bypasses W5 entirely; let the gate stay one-shot for the `Vox` / `Default` presets that need it.
- **Fact that flips this:** if profile data forces the GPU compute path (D.2), the gate-inversion plan becomes the necessary follow-up.

### D.11 Slot iteration order during admission
- **Chosen:** Spawn admission tasks **in camera-distance order** (closest segments first). A small `min-heap` over `SlotIndex` keyed by `|slot_world_pos - camera_world_pos|`.
- **Rejected:** Arbitrary iteration order (slot-index order or HashMap iteration).
- **Reason:** if the cold-start (~170 frames) is in arbitrary order, the user sees terrain populate randomly across the visible window. Camera-distance order populates the centre first, walking outward — more visually coherent during the cold-start frames.
- **Fact that flips this:** if profile data shows the heap ordering costs more than it saves on visual quality, fall back to slot-index order.

---

## Assumptions made

1. **`per_segment_ms` noise-generation timing (§ B.3).** Estimated at ~30 ms on a 6-core CPU. Not benchmarked directly. Derived by extrapolating: `voxel_noise`'s `test_simple_terrain` is 32³ = 32k samples in sub-millisecond release time → 256³ = 16.7 M samples is ~512× larger; assuming sub-linear scaling (FastNoise2's SIMD path) ~30 ms is a plausible upper bound. **Impl agent action:** add a `#[bench]` in `streaming/noise_source.rs::tests` that times `generate_segment` over a representative segment; if > 100 ms, reduce the per-frame admission count or revisit D.2.

2. **`SEGMENT_CHUNKS` is `16` permanently.** Derived from `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS = 4` * 4. The drift-guard test at `lib.rs:920-946` pins it. If a future session changes the constant, the residency window size + per-segment-slab arithmetic both change.

3. **`bevy_tasks::AsyncComputeTaskPool` exists in Bevy 0.19.** Verified in `bevy_voxel_world` reference (`voxel_plugin` uses `rayon` instead, but bevy_tasks ships with Bevy 0.19). If the API has changed shape in 0.19-rc.1, the impl agent uses the equivalent (`bevy::tasks::AsyncComputeTaskPool`).

4. **The W2 `extract_world_changes` system runs every frame, regardless of whether `pending_edits.batches.is_empty()`.** Inferred from `crates/bevy_naadf/src/world/data.rs:1366-1379` + reuse-audit row 7 "per-frame `WorldData::pending_edits.batches` → `extract_world_changes`". If the system early-returns when batches are empty, the streaming driver's per-shift push works fine; if it skips on empty, no harm done either.

5. **`RenderQueue::write_buffer` is fine to call on streaming records without the per-segment-submit ordering bug that the W5 driver hit.** The W5 bug was specifically about multiple `write_buffer` calls **to the same uniform buffer** between dispatches **on the same encoder** (`mod.rs:2427-2453`). Streaming writes to the `changed_*_dynamic` buffers (the W2 record buffers, populated by `extract_world_changes` once per frame), then issues 4 compute passes (regime-3) that read them — same pattern as a brush edit, no ordering issue.

6. **The shader-uniform path can accept an additional `IVec3` field for `world_origin_in_voxels_int` without invalidating existing pin/layout tests (§ E.2).** Adding a field to a `[repr(C)]` uniform mirror typically requires a layout test update. The impl agent will need to edit the relevant gpu_types struct + its pin test. The brief did not enumerate which struct; `crates/bevy_naadf/src/render/gpu_types.rs` is the likely site.

7. **`AsyncComputeTaskPool::get().thread_num()` returns the OS thread count.** Used for the parallelism estimate in § B.3. If the pool is configured smaller (Bevy default is `n - 1` cores), the estimate's 6 thread approximation may be off; the order-of-magnitude conclusion (admit-rate >> traversal rate) holds.

8. **The `voxel_noise` crate compiles on the target platform without further work.** `crates/voxel_noise/Cargo.toml` builds `fastnoise2 v0.4` "build-from-source", which requires a C++ compiler. The bevy-naadf workspace already builds it as a workspace member; adding it as a direct dep of `bevy_naadf` should not introduce new build issues. If the CI runs on a target where this fails, the dep flips to `cfg(not(target_arch = "wasm32"))` per the reference project's pattern (`bevy_voxel_world/crates/voxel_plugin/Cargo.toml:25-26`).

9. **The streaming preset spawns its initial camera looking at the **central** segment, so the initial residency window is centered on a populated area.** The "cold-start visible" assertion (G.1 phase 2) assumes this; if the camera spawns at world-corner `(0, 0, 0)`, the windowed residency excludes 7/8ths of the cold-start admissions and the visible terrain percentage in the framebuffer is much lower. Spawn at `(WORLD_SIZE_IN_VOXELS.x / 2, surface_estimate, WORLD_SIZE_IN_VOXELS.z / 2)`.

10. **The skybox luminance in this renderer is ≤ 50 over a small framebuffer rect.** Inferred from `e2e/oasis_edit_visual.rs`'s use of `mean_pixel_delta` with thresholds in the 8-100 range. If the skybox is hotter (e.g. emissive 200+ in HDR), assertion C (`STREAMING_EVICTED_LUMINANCE_CEILING = 50.0`) needs raising. The impl agent runs the warmup capture once first, reads the skybox luminance, then sets the constant.
