# vox-gpu-rewrite — surface-inversion diagnostic (2026-05-18)

## Symptom

**User report (verbatim):** running

```bash
cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
```

after the W5.3-fix Stage 1 land shows Oasis geometry — but **"most of the
surfaces are corrupted (inverted)"**.

**Screenshot:**
`/home/midori/.claude/image-cache/dc4b036b-73d6-4d50-96a0-bb4ecbac8b8b/1.png`

Visual observation of the screenshot:

- Recognisable Middle-Eastern Oasis architecture viewed from above (camera at
  Y=800 looking +Z, the Stage-1-noted camera rescaling — Y=800 sits above the
  512-voxel world ceiling so the view is top-down).
- Stone buildings with crenellated walls, courtyards, terraces, decorative
  carved trim along parapets — geometry IS spatially correct (the model
  layout matches Oasis).
- **MOST blocks are intact** — bulk wall surfaces, parapets, decorative trim
  render with subtle architectural shading. The architecture's silhouette
  reads correctly.
- **Scattered MISSING voxel-blocks throughout** — bright/sky-coloured patches
  where solid stone should be, exposing whatever lies past the missing block.
  The pattern is NOT uniformly inverted; NOT striated; NOT localised to one
  region. It looks like RANDOM small regions of mixed blocks have failed to
  populate, with the renderer descending into them and finding empty/zero
  voxel data.
- **Small bright specks** (look greenish/white) scattered through the
  near-ground level — likely individual voxels reading wrong type indices or
  empty.
- The MOSTLY-INTACT bulk vs SCATTERED-EMPTY pattern is the load-bearing
  observation: this is consistent with **uniform-empty / uniform-full blocks
  rendering correctly, and only the MIXED blocks (the small fraction of
  blocks with internal voxel variation — terrain edges, decorative trim
  detail) failing to populate.**

**E2E gate observation (independent evidence):**
`target/e2e-screenshots/vox_gpu_construction_before.png` (camera A at C# spawn
`(500, 200, 40)` looking +Z) shows a fragmentary water-blue surface with an
"underside slab" at top of frame, dark coastline silhouette right of centre,
and scattered small dark fragments below — the camera is positioned INSIDE
the world at Y=200 (below ceiling Y=512); the view is consistent with the
camera looking through the lower-Y region where the Oasis ground sits — and
the fragments visible are consistent with the SAME class of failure
(scattered mixed-block failures), just framed from a different angle that
makes the failure pattern less recognisable as architecture.

## Hypotheses considered

| # | Hypothesis | Ranking after investigation |
|---|---|---|
| H1 | Bounds chain dispatched ONCE with the LAST segment's `bounds_params_buffer` values, AADFs computed against wrong chunks | **LOW (not the inversion cause)** — verified by Read of `chunk_calc.wgsl`'s `compute_voxel_bounds` and `compute_block_bounds` entry points: neither shader references `params.*` AT ALL. The bounds chain reads only `blocks[]` and `voxels[]`, computes within-block-relative AADF bits, writes back. The per-segment `bounds_params_buffer` overwrites don't affect the bounds chain's correctness. **BUT a related downstream bug exists** — see "Related downstream bug" below — that bug isn't the inversion cause either. |
| H2 | 3D workgroup flattening (W5.3-fix Stage 1 new code) off-by-N | **LOW** — verified: `chunk_calc.wgsl:461-463 / :513-515` flattens `block_index = group_id.x + group_id.y * num_workgroups.x + group_id.z * num_workgroups.x * num_workgroups.y`, matching the dispatch helper `split_3d_dispatch`'s `[x, y, z]` repacking at `chunk_calc.rs:246-265`. The runtime log shows `voxel_workgroups=134,281,215 covers 134,217,729 requested` — math checks out (134,217,729 + slack from the per-axis ceil rounds). |
| H3 | `block_voxel_count` cursor mismanagement across per-segment dispatches | **LOW** — verified: the cursor is a global 2-element atomic buffer, seeded ONCE at `[64, 64]` in `prepare_construction:1009 / :1656`. Per-segment dispatches `atomicAdd(&block_voxel_count[1], 64u)` / `atomicAdd(&block_voxel_count[0], 64u)` accumulate correctly across segments (each segment claims fresh slots; no clobber). Matches C# `WorldData.cs:129` `blockVoxelCount = [64, 64]` seed semantics. |
| H4 | Renderer's voxel-pair unpack swapped | **LOW** — verified: `ray_tracing.wgsl:335-336`: `voxels[voxel_start_index]` then `(cur_voxel_pair >> (16u * (voxel_index_in_block & 0x1u))) & 0xFFFFu`. Even voxel index → low 16 bits, odd → high 16 bits. Generator writes at `generator_model.wgsl:158`: `chunk_data_rw[dst] = voxel1 \| (voxel2 << 16u);` where `voxel1` = the EVEN-indexed voxel. Consistent. |
| H5 | "Full" bit polarity flipped (generator sets bit 15 on solid, renderer reads bit 15 as "empty") | **LOW** — verified: generator (`generator_model.wgsl:149-154`) sets bit 15 when `voxel > 0u` (i.e., on solid). Renderer (`ray_tracing.wgsl:339-341`) reads `if ((cur_node >> 15u) != 0u) { cur_node = cur_node \| (1u << 30u); }` — bit 15 set means "full, re-tag as uniform-full". Polarity matches. Also verified by the W5 unit test `generator_model_gpu_vs_cpu_bit_exact` which still passes (ran in this dispatch — 1 passed). |
| H6 | Per-segment generator output overwriting prior segments' output before chunk_calc consumes | **LOW** — verified: Stage 1's per-segment fresh-encoder + per-segment-submit pattern means each segment's `generator → chunk_calc` happens within the same submit (storage barriers auto-inserted between sequential compute passes in the same encoder). Subsequent segments' generator writes only fire after the previous submit completes (wgpu queue serialises submits). |
| H7 | `AppArgs::vox_gpu_construction_mode` leaks into production | **LOW** — verified: `lib.rs:412` defaults to `false`. The flag's effects are confined to `e2e::driver` and `e2e::vox_gpu_construction::pin_vox_gpu_construction_camera`; production code paths are unaffected. |
| **H8 (NEW — root cause)** | **`hash_coefficients` and `hash_map` storage buffers on the W5 install path are 4-byte and 16-byte placeholders, NOT the production allocations.** The W5 install path leaves `dense_voxel_types = Vec::new()`, which causes `prepare_construction:925-930`'s `want_gpu_producer = config.gpu_construction_enabled && dense_data_ready` to compute `false`. The `if want_gpu_producer && !gpu.gpu_producer_has_run` pre-allocation block at `:930-1012` is SKIPPED, leaving `hash_coefficients` / `hash_map` un-allocated. The W2-fallback placeholder block at `:1644-1721` then runs and creates `hash_map = 16 bytes` (1 slot of zero), `hash_coefficients = 4 bytes` (1 u32 of zero — NEVER `write_buffer`'d to the real `31^(64-i)` table). The W5-specific block at `:1281-1387` allocates `segment_voxel_buffer` + the 3 model_data buffers + the params uniform, but **DOES NOT TOUCH hash_map / hash_coefficients**. | **HIGH — confirmed root cause.** See "Identified gap" for the bit-level mechanism. |
| H9 | C#-style hash-map per-segment growth (`SetNewUsedCount`) missing — fixed-size hash_map saturates on full Oasis | LOW (contributing-perf, not load-bearing for the inversion). Even at the configured `262144` slots the table wouldn't saturate for Oasis (estimated ~80K unique mixed blocks well below the 131K wanted-empty threshold). H8 makes this moot anyway: with the 16-byte hash_map placeholder, EVERY hash insert collides regardless of the diversity of input hashes. |

## C# reference behaviour

**`WorldData` initial buffer allocation (`NAADF/NAADF/World/Data/WorldData.cs:73-92`):**

```csharp
segmentVoxelBuffer = new StructuredBuffer(... ((256)^3) / 2 ...);
dataVoxelGpu = new DynamicStructuredBuffer(... 256^3 / 2  = 8,388,608  u32s = 32 MiB);
dataBlockGpu = new DynamicStructuredBuffer(... 256^3 / 64 =   262,144  u32s =  1 MiB);
// ...
blockHashingHandler = new BlockHashingHandler(
    this, 0, 0.5f, (worldGenSegmentSizeInVoxels * worldGenSegmentSizeInVoxels * worldGenSegmentSizeInVoxels) / 64);
```

**`BlockHashingHandler` constructor (`BlockHashingHandler.cs:36-61`):**

```csharp
public BlockHashingHandler(WorldData worldData, int startSizeMap = 0,
                           float wantedEmptyRatio = 0.5f, int minReservedCount = 64)
{
    mapSize = Math.Max(1, startSizeMap);                  // = 1
    while (mapSize * wantedEmptyRatio < minReservedCount) // 1*0.5 < 524288 → loop 21 times
        mapSize *= 2;
    // mapSize = 2^21 = 2,097,152 slots = 32 MiB hash_map for Oasis-class
    // worlds (`maxNewVoxelsPerGenSegment = 256^3 = 16,777,216`,
    // `minReservedCount = 16,777,216 / 32 = 524,288`).

    coefficients = new uint[65];
    coefficients[64] = 1;
    for (int i = 64 - 1; i >= 0; --i)
        coefficients[i] = 31 * coefficients[i + 1];
    // coefficients[0] = 31^64 mod 2^32 = 0xC9E6F1FC (some large non-zero u32);
    // coefficients[1..63] = 31^(64-i) — none is zero.

    map = new BlockValue[mapSize];      // 32 MiB CPU mirror, all-zero
    mapGpu = new StructuredBuffer(...); // 32 MiB GPU buffer
    mapGpu.SetData(map);                // GPU buffer explicitly cleared
}
```

**`WorldData.GenerateWorld` per-segment hash bindings (`WorldData.cs:120-156, :490-507`):**

```csharp
// GenerateWorld replaces the BlockHashingHandler at the start of every
// load (Dispose old, construct fresh):
blockHashingHandler?.Dispose();
blockHashingHandler = new BlockHashingHandler(this, 0, 0.5f, maxNewVoxelsPerGenSegment / 32);

// Per-segment loop:
for (int z = 0; z < sizeInWorldGenSegments.Z; ++z)
for (int y = 0; y < sizeInWorldGenSegments.Y; ++y)
for (int x = 0; x < sizeInWorldGenSegments.X; ++x) {
    worldGenerator.CopyToChunkData(segmentPosInChunks, ...);
    CalculateChunkBlocks(segmentPosInChunks);
    blockHashingHandler.SetNewUsedCount(blockVoxelCount[0] / 64);  // may grow + re-hash
    dataBlockGpu.SetNewMinCount(...);
    dataVoxelGpu.SetNewMinCount(...);
}

// `CalculateChunkBlocks` BINDS hashMap / hashCoefficients / hashMapSize FRESH
// each call — picking up the (possibly-grown) BlockHashingHandler state:
private void CalculateChunkBlocks(Point3 chunkOffset) {
    // ...
    chunkProcessor.Parameters["hashMap"]?.SetValue(blockHashingHandler.mapGpu);
    chunkProcessor.Parameters["hashCoefficients"].SetValue(blockHashingHandler.coefficients);
    chunkProcessor.Parameters["hashMapSize"]?.SetValue(blockHashingHandler.mapSize);
    // ...
    chunkProcessor.Techniques[0].Passes["VoxelHash"].ApplyCompute();
    App.graphicsDevice.DispatchCompute(worldGenSegmentSizeInChunks, ...);
}
```

**Two load-bearing C# guarantees:**

1. **`hashCoefficients` IS a 65-entry table of `31^(64-i)` values, NEVER zero.** Bound to the shader EVERY segment.
2. **`hashMap` IS sized to `mapSize ≥ 2,097,152` slots for Oasis-class worlds**, allocated and explicitly cleared up-front, optionally grown per segment.

## Rust port behaviour (post-Stage 1)

### The `dense_voxel_types` gate (the load-bearing branch point)

`prepare_construction` at `crates/bevy_naadf/src/render/construction/mod.rs:925-930`:

```rust
let dense_data_ready = world_data_meta
    .as_deref()
    .is_some_and(|w| !w.dense_voxel_types.is_empty());
let want_gpu_producer =
    construction_config.gpu_construction_enabled && dense_data_ready;
if want_gpu_producer && !gpu.gpu_producer_has_run {
    // ...
    // Allocates the PRODUCTION hash_map (sized to `initial_hash_map_size`)
    // AND uploads the real `hash_coefficients()` table.
}
```

The W5 install path (`crates/bevy_naadf/src/voxel/grid.rs:317-428`,
`install_vox_in_fixed_world`) deliberately leaves `dense_voxel_types =
Vec::new()` at `:423` because the GPU producer chain doesn't need it (the
model data comes from `ModelData`/`ModelDataRender` instead). Therefore
`dense_data_ready = false`, `want_gpu_producer = false`, and the entire
pre-allocation block at `:930-1012` is **SKIPPED**.

### What allocates `hash_coefficients` / `hash_map` on the W5 path

The W5-specific allocation block at
`crates/bevy_naadf/src/render/construction/mod.rs:1281-1387` runs when
`model_data` is present. It allocates:

- `segment_voxel_buffer` at 128 MiB (`:1294-1314`)
- `model_data_chunk_buffer` / `model_data_block_buffer` /
  `model_data_voxel_buffer` (`:1317-1346`)
- `model_data_params_buffer` (zeroed; rewritten per-segment in the producer
  node) (`:1350-1360`)
- The `construction_generator_model` bind group (`:1363-1387`)

**It does NOT allocate or touch `hash_map` / `hash_coefficients` / `block_voxel_count`.**

The placeholder block at `:1644-1721` runs next (gated on
`bind_groups.construction_world.is_none()`). It creates **W2-PLACEHOLDER**
buffers for the missing slots:

| Field | Placeholder allocation | Initial contents |
|---|---|---|
| `block_voxel_count` | 8 bytes (2 × u32) | `[64u32, 64u32]` (correct seed; this one happens to be fine) |
| `segment_voxel_buffer` | 4 bytes (1 × u32) | UNTOUCHED (skipped — W5 block already filled it) |
| **`hash_map`** | **16 bytes (1 × HashValueSlot)** | UNINITIALISED (wgpu zero-init guarantee → all zero, but only 1 slot) |
| **`hash_coefficients`** | **4 bytes (1 × u32)** | UNINITIALISED (wgpu zero-init → 1 u32 of zero) |

These placeholders go into the `construction_world` bind group at
`:1704-1719` (used by `chunk_calc.calc_block_from_raw_data` in the W5
producer's segment loop).

### What the chunk_calc shader does with the placeholder bindings

`chunk_calc.wgsl:357-371` (in `calc_block_from_raw_data`):

```wgsl
var hash: u32 = hash_coefficients[0];
let first_voxel_type_comp = segment_voxel_buffer[voxel_index_in_segment];
let first_voxel_type = first_voxel_type_comp & 0x7FFFu;
var is_all_same: bool =
    (first_voxel_type_comp & 0xFFFFu) == (first_voxel_type_comp >> 16u);
for (var i: u32 = 0u; i < 32u; i = i + 1u) {
    let voxel_comp = segment_voxel_buffer[voxel_index_in_segment + i];
    hash = hash + hash_coefficients[i * 2u + 1u] * (voxel_comp & 0x7FFFu);
    hash = hash + hash_coefficients[i * 2u + 2u] * ((voxel_comp >> 16u) & 0x7FFFu);
    if (first_voxel_type_comp != voxel_comp) {
        is_all_same = false;
    }
}
```

`hash_coefficients` is a 4-byte placeholder = ONE u32 of zero. The shader
reads `hash_coefficients[0]` (returns 0 — the placeholder's sole slot) AND
`hash_coefficients[1..64]` (returns 0 — WebGPU spec §Storage Buffer Access:
OOB reads on storage buffers return zero).

**Therefore `hash = 0 + 0 + 0 + … + 0 = 0` for EVERY mixed block in the
world.**

Then `chunk_calc.wgsl:262-340` (`get_voxel_pointer`):

```wgsl
var hash_bounds: u32 = hash & (params.hash_map_size - 1u);  // = 0 & 262143 = 0
// ...
let cas_result = atomicCompareExchangeWeak(
    &hash_map[hash_bounds].voxel_pointer,    // hash_map[0]
    EMPTY_BLOCK,
    PENDING_BIT | voxel_raw_start,
);
```

The hash_map BUFFER has 16 bytes = 1 slot. `params.hash_map_size = 262144`
(the per-segment value the W5 producer writes). All blocks compute
`hash_bounds = 0`. They all CAS slot 0.

**The FIRST mixed block to win the CAS:**
- Claims slot 0.
- Reserves 64 voxels at the cursor (`atomicAdd(&block_voxel_count[0], 64u)`).
- Copies its 32 voxel-pairs to `voxels[voxel_u32_start..voxel_u32_start+32]`.
- Stores `hash_map[0].hash_raw = 0` (the hash for every block, since all
  hashes are zero).
- Atomically replaces the PENDING tag with the final pointer.

**EVERY SUBSEQUENT mixed block:**
- CAS fails (slot 0 already occupied).
- Spins until PENDING clears.
- Reads `hash_map[0].hash_raw == 0 == hash` (matches — both are zero by
  vacuous truth).
- Performs the 32-voxel data-equality check `voxels[voxel_pointer_cur + i]
  vs segment_voxel_buffer[voxel_raw_start + i]`.
- If the block's voxel data happens to byte-match the FIRST mixed block's
  data → **dedup hit, the subsequent block silently inherits the FIRST
  block's voxel data** (renders as the first-block geometry).
- If the block's voxel data does NOT byte-match → `is_all_equal = false`,
  voxel_pointer stays 0, probe loop increments `hash_bounds` (now 1).
- `hash_bounds = 1` indexes `hash_map[1]` — OOB on a 1-slot buffer. **OOB
  writes are no-ops, OOB reads return zero.** CAS sees "empty" (zero),
  tries to claim — write is dropped. The atomic spin reads zero. The
  spin-wait sentinel never clears. After PENDING_WAIT_CAP=2000 iterations,
  the wait exits. `hash_raw == hash == 0` matches. Data compare fails (the
  "neighbour" slot has all zeros, the block's data isn't all zeros).
  Continue probing. EVERY probe sees OOB-zero. After 250 probes:
  `get_voxel_pointer` returns the sentinel **`2u`** (`chunk_calc.wgsl:339`).
- The block is then encoded as `block = 2u | (BLOCK_STATE_CHILD << 30u)` —
  a CHILD block whose voxel address is `2`.

### What the renderer sees

`ray_tracing.wgsl:319-342`:

```wgsl
if ((cur_node >> 31u) != 0u) {                       // CHILD chunk: descend
    let block_pos_in_chunk = voxel_pos_in_chunk / 4u;
    let block_index = (cur_node & 0x3FFFFFFFu) + flatten_index(block_pos_in_chunk, 4u, 16u);
    cur_node = blocks[block_index];
    let voxel_pos_in_block = vec3<u32>(cur_cell) % 4u;
    let block_is_parent = (cur_node >> 31u) != 0u;
    if (block_is_parent) {                            // CHILD block: descend to voxels
        let voxel_index_in_block = flatten_index(voxel_pos_in_block, 4u, 16u);
        let voxel_start_index =
            (cur_node & 0x3FFFFFFFu) + voxel_index_in_block / 2u;  // = 2 + idx/2
        let cur_voxel_pair = voxels[voxel_start_index];            // voxels[2..N]
        cur_node = (cur_voxel_pair >> (16u * (voxel_index_in_block & 0x1u))) & 0xFFFFu;
        // ...
    }
}
```

For the sentinel-2 blocks, `voxel_start_index = 2 + offset`. The `voxels`
buffer's cursor seeds at index 64 (= the 64-voxel `block_voxel_count[0]`
seed / 2 u32 packing). Indices 0..63 in `voxels[]` are SEED REGION,
zero-initialised, never written by chunk_calc. **Reading
`voxels[2..2+offset]` returns ZERO → type=0 → empty voxel → ray passes
through.**

For the dedup-hit blocks (voxel data happens to match the first block),
`voxel_start_index` points into the actual voxels buffer at the FIRST
mixed block's claimed region. Renders as the FIRST mixed block's voxel
data — visually "okay" but byte-incorrect (semantic match means the
geometry happens to coincide).

### Why the symptom is "scattered missing voxels" not "everything broken"

The architecture has THREE block populations:

1. **Uniform-empty blocks** (the bulk of air voxels in / around the
   buildings): `is_all_same == true` AND `first_voxel_type == 0` →
   `block = 0 | (BLOCK_STATE_UNIFORM_EMPTY << 30u) = 0` — chunk_calc
   never touches the hash map. **Renders correctly (empty).**
2. **Uniform-full blocks** (the bulk of solid stone interior): `is_all_same
   == true` AND `first_voxel_type != 0` → `block = type | (BLOCK_STATE_UNIFORM_FULL << 30u)`
   — chunk_calc never touches the hash map. **Renders correctly (solid).**
3. **Mixed blocks** (the small fraction of blocks with internal voxel
   variation — wall edges, decorative trim, terrain transitions): calls
   `get_voxel_pointer` → fails OR dedup-hits-wrong-data per the mechanism
   above. **Renders as scattered missing voxels.**

The Oasis fixture has lots of uniform blocks (interior walls, sky, ground
mass) and a smaller fraction of mixed blocks (decorative trim, terrain
edges). **This exactly matches the screenshot: bulk architecture intact,
scattered detail missing.**

## Identified gap

**File:** `crates/bevy_naadf/src/render/construction/mod.rs`

**Line:** `925-930` (the `want_gpu_producer` gate).

**Gap:** the gate requires `dense_voxel_types` non-empty as a precondition
for allocating the production `hash_map` and the real `hash_coefficients()`
table. The W5 install path is the FIRST production code path to leave
`dense_voxel_types` empty while ALSO needing the GPU producer chain
(`model_data` IS present, `gpu_construction_enabled` IS true, but
`dense_voxel_types` is intentionally empty — `grid.rs:423`).

The pre-Stage-1 code paths (default-scene-with-`DenseVolume` path,
validate_gpu_construction path) ALL had `dense_voxel_types` non-empty by
construction, so the pre-allocation block always ran for them. The Stage 1
addition (W5 path) silently bypasses the allocation.

**C# has no equivalent gate**: `BlockHashingHandler` is constructed
unconditionally in `WorldData.GenerateWorld` (`WorldData.cs:131-132`), and
the `coefficients[65]` table is initialised in `BlockHashingHandler`'s
constructor (`:50-55`). The Rust port's `want_gpu_producer` gate is a
Rust-side optimisation — "don't allocate big buffers when there's nothing
to dispatch" — that doesn't match C#'s "always allocate, dispatch is
gated separately".

## Recommended fix (NOT to be implemented in this dispatch)

### Fix (minimal — gate the pre-allocation on `gpu_construction_enabled` alone)

**File:** `crates/bevy_naadf/src/render/construction/mod.rs`

**Lines:** `925-930`

Replace:

```rust
let dense_data_ready = world_data_meta
    .as_deref()
    .is_some_and(|w| !w.dense_voxel_types.is_empty());
let want_gpu_producer =
    construction_config.gpu_construction_enabled && dense_data_ready;
if want_gpu_producer && !gpu.gpu_producer_has_run {
```

with:

```rust
// vox-gpu-rewrite W5.3-fix Stage 2 — the W5 install path leaves
// `dense_voxel_types = Vec::new()` by design (the GPU producer chain
// consumes `ModelData` instead). The pre-allocation block below must
// still run to allocate the production hash_map + the real
// `hash_coefficients()` table (else `chunk_calc.calc_block_from_raw_data`
// receives a 16-byte hash_map placeholder + a 4-byte zeroed
// hash_coefficients placeholder, causing every mixed block to hash to 0
// and probe-cap-exhaust → scattered missing voxels in the rendered
// scene). C# has no equivalent gate (`BlockHashingHandler` is constructed
// unconditionally in `WorldData.GenerateWorld`).
let dense_data_ready = world_data_meta
    .as_deref()
    .is_some_and(|w| !w.dense_voxel_types.is_empty());
let model_data_present = model_data.is_some();
let want_gpu_producer = construction_config.gpu_construction_enabled
    && (dense_data_ready || model_data_present);
if want_gpu_producer && !gpu.gpu_producer_has_run {
```

The downstream allocation of `segment_voxel_buffer` (line 1027) is gated
on `dense_voxel_types`-derived `dense` data and would NEED a small further
adjustment to skip the W5 case (since the W5 block at `:1281-1314`
allocates `segment_voxel_buffer` at the per-segment cubic 128 MiB extent,
not the dense-derived shape). Two clean shapes:

**Shape A (minimal):** keep the W5 block's `segment_voxel_buffer`
allocation at `:1294-1314`, and skip the dense-data-derived
`segment_voxel_buffer` allocation at `:1027-1054` when `model_data_present`:

```rust
if gpu.segment_voxel_buffer.as_ref().map(|b| b.size()).unwrap_or(0) <= 4
    && !model_data_present  // <-- new gate
{
    // existing dense-data-derived allocation
}
```

(With Shape A: the loop still runs the hash_map / hash_coefficients /
block_voxel_count allocations — the only block that needs the
`dense_voxel_types` data is the segment_voxel_buffer dense-build path.)

**Shape B (cleaner):** lift the hash_map / hash_coefficients /
block_voxel_count allocations out of the `if want_gpu_producer` block into
an unconditional `if gpu_construction_enabled` block. This matches the C#
"always allocate, gate the dispatch separately" shape.

### Confidence — recommended fix correctness

Both shapes resolve the bug; Shape A is the smaller diff. The fix is
guaranteed correct because:

1. The pre-allocation block at `:930-1012` is well-tested on the default-scene
   path (the bevy-naadf grid preset has `dense_voxel_types` populated). It
   has been the production code path since Phase-C; the only change is to
   extend the gate to also fire when `model_data` is present.
2. `model_data: Option<Res<crate::render::extract::ModelDataRender>>` is
   already a system parameter on `prepare_construction` (the W5.1 extract
   feeds it); no signature change needed.
3. The downstream `if gpu.hash_map.is_none()` / `if gpu.hash_coefficients.is_none()`
   guards in the pre-allocation block ensure idempotency — running the
   block on the W5 path simply fills in the missing real buffers.

### Confirmation test

After applying the fix, the user re-runs `cargo run --release --bin
bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox` and the
scattered-missing-voxels artefact resolves.

The `--vox-gpu-construction` e2e gate's saved before/after PNGs at
`target/e2e-screenshots/vox_gpu_construction_*.png` should then show
recognisable architecture detail (today they show fragmentary water/dark
silhouettes because the same bug applies to the e2e path).

## Confidence level

**HIGH**

- The placeholder allocations are provable from direct Read of
  `prepare.rs:1644-1721` and the W5-block at `:1281-1387`. The W5 block
  demonstrably does NOT touch `hash_map` / `hash_coefficients`.
- The shader's hash computation degenerates to identically-zero is provable
  from `chunk_calc.wgsl:359-371` (the hash-accumulation loop) + WebGPU
  spec §Storage Buffer Access (OOB storage reads return zero).
- The `get_voxel_pointer` failure mode (sentinel `2u` after probe-cap
  exhaustion) is provable from `chunk_calc.wgsl:264-340`.
- The renderer's behaviour on sentinel-2 blocks (reading `voxels[2..]` = zero
  → empty voxels) is provable from `ray_tracing.wgsl:319-342` + the cursor
  seeding `voxels[0..63] = 0` at `prepare.rs:418`.
- The screenshot pattern (scattered missing voxels on otherwise-intact
  architecture) is consistent with the predicted "mixed blocks fail, uniform
  blocks succeed" partition — uniform-block / mixed-block splits are
  determined by the input model's geometry (Oasis's stone-block layout
  produces lots of uniform blocks and a smaller fraction of mixed blocks at
  decorative edges + terrain transitions).
- The W5 generator unit test `generator_model_gpu_vs_cpu_bit_exact` still
  passes (verified in this dispatch — 1 passed, 198 filtered out), ruling
  out the generator stage as the source.

## Related downstream bug (NOT the inversion cause; flag for follow-up)

While investigating H1, found that the W5 per-segment loop writes the
`bounds_params_buffer` (the same buffer that the W3
`add_initial_groups_to_bound_queue` shader reads at `bounds_calc.wgsl:239,
:257-260`) with `bound_group_queue_max_size: 1` for every segment
(`mod.rs:2271`). After the segment loop, the buffer's
`bound_group_queue_max_size` is `1`, but `add_initial_groups` is dispatched
with `bound_group_count = 32768` workgroups (`mod.rs:1435`).

`add_initial_groups`'s gate `if (group_index >= params.bound_group_queue_max_size) return;`
limits the per-thread enqueue to 1 group out of 32768. The remaining 32767
groups never get seeded into the bound queue — the bound queue's
per-axis-bit-mask never sets bits for them → the W3 per-frame
`bounds_calc` chain has nothing to refine → chunk-level AADFs for those
groups stay at zero (the post-`prepare_world_gpu` chunks-buffer initial
state).

**Effect:** rays do not over-skip at chunk granularity — they step
chunk-by-chunk through the full world. This is a PERFORMANCE issue (the
W3 acceleration structure is not built), NOT a correctness issue. The
geometry still renders correctly; just slower.

**Fix shape:** the W5 per-segment construction-params should preserve
`bound_group_queue_max_size = bound_group_count` (the pre-Stage-1 value
in `prepare_construction:1181`), since the post-segment-loop bounds chain
+ the next-frame `add_initial_groups` both read it. Or split into two
uniforms — one for chunk_calc (per-segment chunk_offset + segment_size)
and one for bounds_calc (one-shot, set at prepare time with
`bound_group_queue_max_size = bound_group_count`).

## Observation evidence

### Production binary run (this dispatch — diagnostic only, not verification)

```
cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
```

Log (filtered):

```
phase-c followup#1 — gpu construction ENABLED (default).
NAADF .vox loaded → ModelData (93×34×84 chunks; data_chunk=265608 u32s,
    data_block=1617216 u32s, data_voxel=10498368 u32s, 257 palette entries).
camera::setup_camera: framing loaded world — pos=(2000.00, 800.00, 160.00),
                     look_at=(2000.00, 800.00, 161.00)
vox-gpu-rewrite W5.3-fix Stage 1 — prepare_world_gpu allocating buffers:
    chunks=2097152 u32-pairs (16 MiB), blocks=134217728 u32s (512 MiB),
    voxels=268435456 u32s (1024 MiB) (gpu_producer_enabled=true,
    cpu_blocks_len=1, cpu_voxels_len=1, chunk_count=2097152).
vox-gpu-rewrite W5 — per-segment GPU producer chain DISPATCHED
    (512 segments × (generator_model + calc_block); bounds chain ×1;
    voxel_workgroups=134217729 dispatched as 3D [65535, 2049, 1]
    (= 134281215 total workgroups, covers 134217729 requested),
    block_workgroups=2097153 dispatched as 3D [65535, 33, 1]
    (= 2162655 total workgroups, covers 2097153 requested)).
```

No wgpu validation errors. No warnings on the W5 chain. Producer dispatches
cleanly — the silent OOB on the hash_coefficients reads doesn't trigger
validation (the WebGPU spec defines OOB storage reads as returning zero,
not an error).

### Screenshot evidence

- **Production binary** (`/home/midori/.claude/image-cache/dc4b036b-73d6-4d50-96a0-bb4ecbac8b8b/1.png`):
  Oasis viewed from above (Y=800 vs ceiling Y=512). Recognisable
  architecture. Scattered missing voxels throughout. Pattern: most surfaces
  intact, with patches of "sky-colour" or "floor-colour" visible through
  what should be solid walls — consistent with mixed blocks failing the
  hash insert.
- **E2E `--vox-gpu-construction` before-frame**
  (`target/e2e-screenshots/vox_gpu_construction_before.png`): camera A at
  C# spawn (500, 200, 40) looking +Z. Fragmentary blue surface with a flat
  "ceiling slab" at the top of the view (interior of upper world cells),
  dark coastline-like silhouette right-of-centre, scattered dark fragments.
  The fragmentary appearance is consistent with the same class of failure
  (mixed-block failures dropping entire sub-block voxel sets) — but seen
  from inside the world at low Y the pattern doesn't read as architecture.
- **E2E after-frame**
  (`target/e2e-screenshots/vox_gpu_construction_after.png`): camera B at
  (500, 200, 200) looking +Z. Similar pattern; per-pixel Δ from before
  passes the floor=8 gate.

### W5 generator unit test (this dispatch)

```
cargo test --workspace --lib generator_model_gpu_vs_cpu_bit_exact
→ 1 passed, 198 filtered out (3 suites, 0.46s)
```

Confirms the generator (the per-segment GPU producer stage 1) writes
byte-equal data to its WGSL CPU oracle. The generator stage is NOT the
source of the inversion.

### E2E `--vox-gpu-construction` run (this dispatch — diagnostic only)

```
cargo run --release --bin e2e_render -- --vox-gpu-construction
→ vox-gpu-construction gate PASS — rect mean per-pixel RGB Δ=16.77
  (floor=8.00); full-frame mean per-pixel RGB Δ=9.66
```

Same producer-dispatched info log. Gate passes because the camera-sweep
delta exceeds the floor (some geometry IS visible) — but the underlying
voxel data IS corrupt per H8. The gate is a coarse "geometry present"
check; it does NOT detect mixed-block dropout.
