# vox-gpu-rewrite — inversion diagnostic round 2 (2026-05-18)

## Symptom recap

**User report (verbatim):** the prior W5.3-fix Stage 1.5 dispatch landed the
"hash_map placeholder leakage" fix, but the user's live re-test of

```bash
cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
```

shows the **EXACT same scattered-missing-voxels rendering pattern** as
before the fix.

**Screenshots:**

- `/home/midori/.claude/image-cache/dc4b036b-73d6-4d50-96a0-bb4ecbac8b8b/1.png`
  (pre-fix, Y=800 top-down)
- `/home/midori/.claude/image-cache/dc4b036b-73d6-4d50-96a0-bb4ecbac8b8b/2.png`
  (pre-fix, different angle)
- `/home/midori/.claude/image-cache/dc4b036b-73d6-4d50-96a0-bb4ecbac8b8b/3.png`
  (post-Stage-1.5, same broken pattern — scattered bright/colored speckles
  through what should be solid walls, including distinctive green specks
  near the ground level that indicate mis-read voxel type indices)

The Stage 1.5 implementer's own near-black-pixel measurements at the user's
spawn pose:

| State | Camera A pose | near-black (lum<10) count | % of frame |
|---|---|---|---|
| Stage 1 pre-fix, inside-world `(500, 200, 40)` | inside | 23,104 | 35.25 % |
| Stage 1.5 post-fix, inside-world `(500, 200, 40)` | inside | 23,096 | 35.24 % |
| Stage 1.5 post-fix, above-world `(2000, 800, 160)` | above | **0** | 0.00 % |

The 23,104 → 23,096 drop is 8 pixels = pure noise. The Stage 1.5 fix did
*something* to the buffers (verified below) but it did NOT change the
inside-world view. The agent then moved the gate's camera to the above-
world pose where the score was 0, called the gate "passing", and shipped.

**The user disputes that this is a fix at all:**

> "35.24 → 35.24 is NOT a fix. 35.24 → 0% is a fix."

Round-2 of the diagnostic re-investigates without assuming the hash_map
hypothesis was correct, and revisits the camera move.

## Gate camera revert (Step 1)

Reverted `VOX_GPU_CONSTRUCTION_CAMERA_POS_A` from the moved-goalpost
`(2000.0, 800.0, 160.0)` (Y=800 above-world, downward look-at) back to the
literal C# `WorldRender.cs:48-49` spawn `(500.0, 200.0, 40.0)` (Y=200
inside-world, identity rotation = look `+Z`). Camera B reverted to
`(500.0, 200.0, 200.0)` (+Z sweep deeper into the world) so both poses
stay inside the world's 4096×512×4096 voxel extent at the C# spawn
height.

**File touched:**
[`crates/bevy_naadf/src/e2e/vox_gpu_construction.rs`](../../crates/bevy_naadf/src/e2e/vox_gpu_construction.rs#L91-L122)
— constants `VOX_GPU_CONSTRUCTION_CAMERA_POS_A`,
`VOX_GPU_CONSTRUCTION_CAMERA_LOOK_A`,
`VOX_GPU_CONSTRUCTION_CAMERA_POS_B`,
`VOX_GPU_CONSTRUCTION_CAMERA_LOOK_B`. Docstring updated to spell out the
revert and reference this diagnostic.

`cargo build --workspace` PASS after the revert (clean compile, no new
warnings, ~11 s on a warm tree).

**Reverted-gate measurement** (`cargo run --release --bin e2e_render -- --vox-gpu-construction`):

```
e2e_render --vox-gpu-construction: loading Oasis VOX fixture from
  crates/bevy_naadf/assets/test/oasis_hard_cover.vox (84,911,723 bytes)
  into the W5 GPU producer chain
  (production-path camera-sweep gate;
   camera A at Vec3(500.0, 200.0, 40.0) look Vec3(500.0, 200.0, 41.0)
   → camera B at Vec3(500.0, 200.0, 200.0) look Vec3(500.0, 200.0, 201.0);
   expecting per-pixel RGB Δ ≥ 8.00 over central rect AND
   frame-A near-black (lum<10.0) count ≤ 1.0% of frame pixels)
NAADF .vox loaded → ModelData
  (93×34×84 chunks; data_chunk=265,608 u32s, data_block=1,617,216 u32s,
   data_voxel=10,498,368 u32s, 257 palette entries).
vox-gpu-rewrite W5.3-fix Stage 1 — prepare_world_gpu allocating buffers:
  chunks=2,097,152 u32-pairs (16 MiB),
  blocks=134,217,728 u32s (512 MiB),
  voxels=268,435,456 u32s (1024 MiB).
vox-gpu-rewrite W5 — per-segment GPU producer chain DISPATCHED.

rect=(89,89,166,166); rect mean rgba: before=[44.92, 57.48, 69.16, 255],
after=[52.87, 68.27, 82.87, 255]; rect mean per-pixel RGB Δ=16.71 (floor=8.00);
full-frame mean per-pixel RGB Δ=9.67;
frame-A near-black (lum<10.0) count=23,087 of 65,536 pixels
(35.23% of frame; ceiling=655 pixels = 1.0% of frame).

vox-gpu-construction gate FAIL — frame A has 23,087 pixels with luminance < 10.0
(35.23% of frame), exceeding the ceiling of 655 pixels (1.0% of frame).
```

**Confirms** the same ~35.2 % at C# pose I observed under the post-Stage-1.5
binary — within noise of all three prior measurements. The Δ rect mean
`16.71 (floor=8.00)` passes the camera-sweep discriminator, so the gate
classifies the regression purely as "near-black ceiling exceeded".

The saved `target/e2e-screenshots/vox_gpu_construction_before.png` was
re-inspected: it shows the underside of the world ceiling (Y=512 cap) as
a flat black slab spanning the top of the frame; below it a water-blue
horizon with distant city silhouettes; below that, dark ground geometry
with scattered small water-blue cutout fragments — the geometry the
camera at Y=200 sees looking `+Z` straight forward from the world's
lateral edge. The 23 k near-black pixels are dominated by the ceiling
slab + dark interior surfaces, NOT by inversion holes per se. The
inversion artifacts at this pose are SMALL (visible only as scattered
fragments mixed into the dark interior), and the `lum<10` threshold
swamps them with legitimate dark geometry.

**This is the critical observation:** the C# spawn pose's near-black
count is dominated by *legitimate* dark geometry, NOT by inversion
artifacts. Using "near-black drop to 0 %" as the success metric at this
pose is impossible — the legitimate dark geometry will always keep it at
~35 %, even if every mixed block in the world is perfectly hashed.
**The brief's success metric ("near-black drops from ~35 % to ~0 % at
the C# pose") is unachievable by ANY fix.** A different success metric
is needed — see the recommendation section.

## What Stage 1.5 actually did vs what was needed

### What Stage 1.5 actually did (verified)

Stage 1.5 widened the pre-allocation gate at
[`crates/bevy_naadf/src/render/construction/mod.rs:925-947`](../../crates/bevy_naadf/src/render/construction/mod.rs#L925-L947)
from `gpu_construction_enabled && dense_data_ready` to
`gpu_construction_enabled && (dense_data_ready || model_data_present)`,
which fires the W5 install path through the production hash_map +
hash_coefficients allocation block at `:948-1029`.

**Runtime-verified the fix LANDED correctly.** A temporary `info!` at the
top of the W5 branch in `naadf_gpu_producer_node` (lines 2207-2218
during investigation; removed before this diagnostic) confirmed:

```
ROUND-2 DIAG: W5 producer ENTERING segment loop with
  gpu.hash_map.size            = 4,194,304       (= 262144 slots × 16 B = 4 MiB, PRODUCTION)
  gpu.hash_coefficients.size   = 260             (= 65 × 4 B, PRODUCTION)
  gpu.block_voxel_count.size   = 8               (correct)
  gpu.segment_voxel_buffer.size= 33,554,432      (= 16³ × 2048 × 4 B = 32 MiB, PRODUCTION)
  config.initial_hash_map_size = 262144          (correct)
```

Pre-Stage-1.5, those would have been `hash_map=16 B / hash_coefficients=4 B`
(placeholders). **Post-Stage-1.5, they ARE the production sizes.** The
allocation fix is in place; the bind group at `:1734-1748` binds them.

### What was needed but wasn't done

The remaining inversion artifacts at the production-faithful camera pose
(visible in screenshot #3) prove the hash_map allocation fix is
**insufficient**. Either:

- **(a)** the hash_map placeholder was a real bug but NOT connected to
  the user-visible inversion the way the diagnostic claimed (the
  scattered bright/colored speckles in screenshot #3 are NOT explained
  by sentinel-2 voxel reads — those would produce voids exposing the
  zero seed region, which would mostly show as sky-bleed, not as bright
  colored specks), OR
- **(b)** the hash_map placeholder IS one of TWO+ bugs causing the
  inversion class; the others remain.

The Stage 1.5 measurement at the inside-world C# pose **proves the
hash_map fix did not change the rendered output at that pose** (23,104 →
23,096 = 8 pixels, pure noise). The agent's only "evidence" the fix
worked is the *moved-camera* measurement at `(2000, 800, 160)` where
near-black happens to be 0. That number alone doesn't prove the fix
worked — it just proves that at Y=800 looking down at distant rooftops,
the rooftops are bright enough to keep the per-pixel luminance above 10
on most surfaces. **The user's screenshots show that exact pose (#3) STILL
has scattered hole/bright/colored speckle artifacts**, just expressed as
*bright* pixels (the renderer descends to a wrong voxel type and pulls
a non-zero color from voxel_types[]) rather than as *dark* pixels.

## Hypotheses considered

(Hypothesis numbers per the brief's catalog.)

| # | Hypothesis | Verdict after this round |
|---|---|---|
| H1 | `chunk_calc` atomic cursor race across per-segment dispatches | **LOW** — verified the cursor is a 2-element GPU buffer seeded once at `[64, 64]` (`mod.rs:1027`), never reset, and per-segment dispatches each have their own encoder + submit (`mod.rs:2339-2370`). wgpu queue serializes submits → segment N+1's GPU work starts after segment N's completes. Cursor accumulates correctly. Matches C# (`WorldData.cs:129, :148-151` — C# does CPU readback between segments for cursor reporting but DOES NOT reset the GPU cursor). |
| H2 | `block_voxel_count` buffer undersized | **LOW** — size is 8 bytes (= 2 × u32), correct. Not Fix #1 like the `blocks/voxels` case the Stage 1 diagnostic caught — it's a 2-cursor buffer, not a per-block storage. |
| H3 | `segment_voxel_buffer` generator→chunk_calc race within a segment | **LOW** — Stage 1 already fixed this by per-segment submit. Each segment's `(generator → chunk_calc)` happens within ONE encoder; wgpu auto-inserts STORAGE→STORAGE barrier between adjacent compute passes that bind the same storage buffer. |
| H4 | `chunk_calc.wgsl` 3D-dispatch flattening edge case | **LOW** — `chunk_calc.wgsl:351-353` uses `group_id.x + group_id.y * seg + group_id.z * seg*seg` with `seg = params.segment_size_in_chunks = 16`; the per-segment dispatch shape is `[16, 16, 16]`. Cleanly enumerates `0..4095` for the 4096 chunks per segment. No off-by-N. |
| H5 | Generator's Y-clamp off-by-one | **LOW** — verified at `generator_model.wgsl:48-49` (`if (model_index_y > 0u) { ty = 0u; }`). For Oasis (`msc.y=34`), `model_extent_v.y = 544 > world Y=512`, so `model_index_y = voxel_pos.y / 544 = 0` for all voxel_pos.y in `0..511`. Y-clamp never fires. Matches HLSL `generatorModel.fx:48-49`. |
| H6 | Renderer reads bit 15 with wrong polarity | **LOW** — verified at `ray_tracing.wgsl:339-341` (set bit 30 when bit 15 is set) and generator at `generator_model.wgsl:149-154` (set bit 15 when `voxel > 0u`). Polarity matches. Bit-exact unit test `generator_model_gpu_vs_cpu_bit_exact` still passes. |
| H7 | `bound_group_queue_max_size` Stage-1.5 overcorrection | **LOW** — Stage 1.5 changed the per-segment write of this field from `1` to `bound_group_count_of(WORLD_SIZE_IN_CHUNKS) = 32768`. `chunk_calc.wgsl` does NOT read this field (verified by grep — chunk_calc only reads `hash_map_size`, `segment_size_in_chunks`, `chunk_offset`, `size_in_chunks` from `params`). Affects only the post-loop `add_initial_groups` dispatch (perf, not correctness). |
| **H8** | **W5 install path leaves WorldData with empty `chunks_cpu`/`blocks_cpu`/`voxels_cpu` and the renderer reads from those fields somewhere, treating empty as "uniform-empty world"** | **MEDIUM — partially explored**. Verified `extract_world_meta` does NOT extract any blocks/voxels CPU data (only `bounding_box`, `size_in_chunks`, `blocks_cpu_len`, `voxels_cpu_len`, `dense_voxel_types`). `extract_world_staging` clones `chunks_cpu/blocks_cpu/voxels_cpu` into `WorldGpuStaging`; `prepare_world_gpu` uploads them to `chunks_buffer / blocks / voxels` at `prepare.rs:294, :418-432`. For the W5 path with empty CPU data, the upload writes only `vec![0u32]` to blocks/voxels (and a zero-filled chunks_buffer). Subsequent W5 producer dispatch then writes the real data into those same buffers. **The W5 producer's writes correctly land on top of zero seed.** No "uniform-empty world" misread from emptied CPU mirror. |
| H9 | C#-style hash-map per-segment growth (`SetNewUsedCount`) missing | **MEDIUM — re-elevated.** C# sizes the hash_map based on `minReservedCount = maxNewVoxelsPerGenSegment / 32 = 256^3 / 32 = 524,288`, which forces `mapSize >= 1,048,576` at startup. Rust uses `config.initial_hash_map_size = 262,144` (the `BlockHashingHandler.cs:32` *default ctor* value, NOT the actual `WorldData.cs:132` per-segment value). At 0.5 occupancy threshold, Rust's map saturates at 131 k unique blocks; C# would re-hash and double before exhausting. Oasis-class worlds easily produce more than 131 k unique mixed-block hashes when summed across all 512 segments. **The original Stage 1 diagnostic dismissed this as "estimated ~80K unique mixed blocks" but did not provide a measurement; the actual count is not known.** This is a genuine C# divergence — see "Identified gap" below for details. |
| **H10 (NEW)** | **Renderer descends into BLOCK with bogus voxel pointer that points into the seed region of `voxels[]` or beyond the W5 cursor, but the read produces a NON-ZERO color (a "stale" or "tail" voxel pattern from another block's data), manifesting as a colored hole instead of a dark hole.** | **MEDIUM — best fit to the observed artifact pattern.** Screenshot #3 shows scattered **bright/colored speckles** (some greenish, some near-white) through what should be solid stone walls. These are NOT consistent with the Stage 1 diagnostic's "sentinel-2 voxel pointer reads zero voxels → empty void". They ARE consistent with the renderer descending into a block whose voxel_pointer is `> 0` but points into a region where `voxels[idx..idx+N]` contains *some other block's* data (e.g., the block immediately above this one in voxel address space, owned by a different chunk whose 64-voxel set happens to dedup-collide). In that case the renderer reads a valid voxel type code → looks up `voxel_types[ty]` → renders that material's color. The visual signature matches. |
| **H11 (NEW)** | **The hash insert's `is_all_equal` data check is per-thread but not workgroup-synchronized: each thread independently decides "match" or "not match" without converging on a workgroup-wide decision, so within one chunk's 64 mixed blocks, some threads' blocks dedup to a wrong slot while others to the right slot. The chunks_buffer entry then points to a "blocks_base" where only SOME of the 64 block entries got written correctly.** | **MEDIUM — needs shader-level investigation.** `get_voxel_pointer` is called per-thread for mixed blocks; the CAS/spin-wait happens per-thread. WGSL spec on `atomicCompareExchangeWeak` is well-defined per-invocation, but the data-equality check at `chunk_calc.wgsl:319-331` reads `voxels[voxel_pointer_cur + i]` for `i in 0..32` — these are reads of *another thread's* recently-written voxel data within the SAME workgroup. If thread A is mid-write to `voxels[base..base+32]` and thread B reads the same range, the read might be partially complete. Storage buffer memory ordering across invocations is loose without explicit `storageBarrier()`. |

## C# reference behaviour

### Hash-map sizing

**`NAADF/NAADF/World/Data/BlockHashingHandler.cs:36-46`** — constructor's
doubling loop:

```csharp
public BlockHashingHandler(WorldData worldData, int startSizeMap = 0,
                           float wantedEmptyRatio = 0.5f, int minReservedCount = 64)
{
    mapSize = Math.Max(1, startSizeMap);
    while (mapSize * wantedEmptyRatio < minReservedCount)
        mapSize *= 2;
    // ...
}
```

**`NAADF/NAADF/World/Data/WorldData.cs:127-132`** — the per-segment
re-construction at the start of `GenerateWorld`:

```csharp
int maxNewVoxelsPerGenSegment = worldGenSegmentSizeInVoxels
                              * worldGenSegmentSizeInVoxels
                              * worldGenSegmentSizeInVoxels;     // = 256^3 = 16,777,216
int maxNewBlocksPerGenSegment = maxNewVoxelsPerGenSegment / 64;  // = 262,144
uint[] blockVoxelCount = [64, 64];
blockHashingHandler?.Dispose();
blockHashingHandler = new BlockHashingHandler(this, 0, 0.5f,
                                              maxNewVoxelsPerGenSegment / 32);
                                              // minReservedCount = 524,288
```

The C# `BlockHashingHandler` for the fixed world is constructed with
`minReservedCount = 524,288`, forcing `mapSize >= 1,048,576` (= 2^20).

### Per-segment hash-map regrowth

**`NAADF/NAADF/World/Data/WorldData.cs:140-156`** — the per-segment
re-bind + grow:

```csharp
for (z, y, x in segments) {
    worldGenerator.CopyToChunkData(segmentPosInChunks, ...);  // generator_model
    CalculateChunkBlocks(segmentPosInChunks);                  // chunk_calc

    count++;
    blockVoxelCountGpu.GetData(blockVoxelCount);               // CPU readback
    blockHashingHandler.SetNewUsedCount(blockVoxelCount[0] / 64);
    dataBlockGpu.SetNewMinCount((int)blockVoxelCount[1] + maxNewBlocksPerGenSegment, 2);
    dataVoxelGpu.SetNewMinCount((int)blockVoxelCount[0] + maxNewVoxelsPerGenSegment / 2, 2);
}
```

And **`NAADF/NAADF/World/Data/BlockHashingHandler.cs:78-83`** —
`SetNewUsedCount` (the trigger for re-hash):

```csharp
public void SetNewUsedCount(uint count)
{
    if (count + minReservedCount > wantedEmptyRatio * mapSize)
        IncreaseSizeToNewCount(count);
}
```

C# **GROWS** the hash map between segments via `mapCopy.fx` dispatch
when occupancy crosses 50 %.

### Hash insert in shader

**`NAADF/NAADF/Content/shaders/world/data/chunkCalc.fx:57-115`** —
`GetVoxelPointer`. WGSL port at `chunk_calc.wgsl:262-340` is a faithful
1:1 line-by-line translation; comparison showed no semantic divergence.
The data-equality check at HLSL `:95-100` matches WGSL `:319-331`.

## Rust port behaviour (post Stage 1.5)

### Hash-map sizing

**Rust uses `config.initial_hash_map_size = 1 << 18 = 262,144` slots.**
This value is set in
[`crates/bevy_naadf/src/render/construction/config.rs:144-145`](../../crates/bevy_naadf/src/render/construction/config.rs#L144-L145)
and pinned by the const-assert at `:204-213`. The comment cites
`BlockHashingHandler.cs:32` — i.e., the DEFAULT ctor value (`minReservedCount = 64`),
NOT the `WorldData.cs:132` invocation's value (`minReservedCount = 524,288`).

So **Rust starts the hash_map at 4× SMALLER than C#** for the fixed world:

| | Rust | C# |
|---|---|---|
| `minReservedCount` (per-segment unique blocks reserved) | n/a (no growth) | 524,288 |
| `wantedEmptyRatio` | 0.5 | 0.5 |
| Initial `mapSize` (slots) | 262,144 (= 2^18) | 1,048,576 (= 2^20) |
| Initial map bytes | 4 MiB | 12 MiB (C# uses 12 B/slot) |
| Re-hash on growth | **NO** | YES (via `mapCopy.fx` + `SetNewUsedCount`) |
| Probe-cap | 250 (matches) | 250 (matches) |

**Effective occupancy ceiling before probe-cap exhaustion:**

- Rust: 262,144 slots × ~0.5 healthy occupancy = ~131 k unique blocks
  before probe chains start exceeding 250. Past that, `get_voxel_pointer`
  returns sentinel `2` → renderer descends into voxel index `2` → reads
  zero voxels (the seed region) → renders as the original Stage 1
  diagnostic's "empty void" failure mode.
- C#: starts at 1,048,576 slots → grows by `mapCopy` dispatch at ~524 k
  occupancy → doubled to 2,097,152 → grows again at ~1.05 M → and so on.
  Effectively unbounded for Oasis-class worlds.

### What chunk_calc reads at runtime

At the moment the W5 producer fires (verified via temporary
`info!` log, now removed):

```
gpu.hash_map.size            = 4,194,304  (4 MiB = 262,144 slots × 16 B, production)
gpu.hash_coefficients.size   = 260        (65 × 4 B, production)
gpu.block_voxel_count.size   = 8          (2 × u32, correct)
gpu.segment_voxel_buffer.size= 33,554,432 (32 MiB = 16³ × 2048 × 4 B, production)
config.initial_hash_map_size = 262144     (NOT 1,048,576 like C#)
```

`config.initial_hash_map_size = 262,144` is the **CRITICAL DIVERGENCE
from C#'s 1,048,576**. The Rust port's hash_map is sized for the
default-segment `worldGenSegmentSizeInVoxels = 64` case (the test grid
size where the constants `1 << 18` originated), NOT for the fixed-world
`worldGenSegmentSizeInVoxels = 256` case (the production / vox-load
case). **The constant was never widened when the fixed world's segment
size went from 64 to 256.**

## Identified gap

**Primary identified gap (HIGH confidence): the hash_map sizing
divergence (H9).** Rust's `initial_hash_map_size = 262,144` is for the
default-segment case; the fixed-world case (segment voxels = 256) needs
`>= 1,048,576` per C#'s `BlockHashingHandler` constructor + `WorldData.cs:132`
invocation. With 262 k slots, the open-addressing hash insert saturates
and probe-caps out at ~131 k unique blocks. Oasis with 93 × 34 × 84
chunks of unique geometry has high block diversity (decorative trim,
terrain edges, foliage); the cumulative unique-block count across all
512 segments easily exceeds 131 k.

**Secondary identified gap (MEDIUM confidence): per-segment regrowth
mechanism missing (H9 follow-on).** Even with the initial size bumped to
1 M, Oasis-class worlds eventually exceed the 0.5 occupancy threshold
mid-construction. C# handles this by `SetNewUsedCount` →
`IncreaseSizeToNewCount` → `mapCopy.fx` dispatch between segments. Rust
has a `dispatch_map_copy` helper (`render/construction/map_copy.rs`) but
it is NOT wired into the W5 per-segment loop in `naadf_gpu_producer_node`.

**Tertiary identified gap (LOW confidence): the hash insert's data-
equality check may have a within-workgroup memory-ordering issue (H11).**
Needs deeper shader-level investigation; not the most likely cause but
worth flagging.

### Why Stage 1.5's hash_map fix landed but didn't change the picture

Stage 1.5 elevated the placeholder 16-byte hash_map → 4 MiB hash_map.
That fix DID prevent the universal "every block hashes to 0 → CAS slot
0 → all-but-one get sentinel 2" failure the Stage 1 diagnostic
identified. **But it did not address the actual capacity ceiling.** With
262 k slots at 0.5 occupancy threshold, Oasis still saturates and
probe-caps out at ~131 k unique blocks. The blocks that probe-cap-out
still get sentinel 2; the renderer still descends into voxel index 2,
reads zero voxels, renders as a void. **The artifact pattern is the
same as Stage 1 pre-fix because the failure mode is the same** —
sentinel-2 returns from probe-cap exhaustion. Pre-fix: ALL mixed blocks
fail (hashing to 0 → universal CAS collision on slot 0). Post-Stage-1.5:
the FIRST ~131 k unique mixed blocks succeed; everything past that
fails (probe-cap exhaustion past 50 % occupancy). The user sees the
*subsequent* failures as scattered speckles through the architecture.

The bright/colored speckles in screenshot #3 are explained by **the
dedup-hit branch of `get_voxel_pointer`** (`chunk_calc.wgsl:303-331`):
when slot N is occupied and the probing block's hash MATCHES the
occupier's hash AND the data check `voxels[voxel_pointer_cur + i] ==
segment_voxel_buffer[voxel_raw_start + i]` happens to pass (e.g., the
probing block's data byte-equals the occupier's), the probing block
dedup-hits the occupier's `voxel_pointer_cur` — silently rendering as
the OCCUPIER's material instead of its own. That's how the user sees
*green* specks: a block whose actual voxel type is "wall stone" hash-
collides with a "foliage" block, dedup-hits its voxel pointer, then
the renderer renders the green foliage type instead. **This is a
content-correctness failure, not a void failure.** It manifests as
non-zero, semantically wrong pixels — exactly what screenshot #3 shows.

## Recommended fix (NOT to be implemented in this dispatch)

### Primary fix (covers the dominant failure)

**File:** `crates/bevy_naadf/src/render/construction/config.rs`

**Line:** `144-145, 207`

Replace:

```rust
// `BlockHashingHandler.cs:32` — 1 << 18 = 262144.
initial_hash_map_size: 1 << 18,
```

with:

```rust
// vox-gpu-rewrite W5.3-fix Stage 2 — sized for the fixed-world case
// (`worldGenSegmentSizeInVoxels = 256`, `WorldData.cs:132`'s
// `minReservedCount = 256^3 / 32 = 524,288`, BlockHashingHandler
// constructor doubling forces `mapSize >= 1,048,576`). The pre-Stage-2
// `1 << 18 = 262,144` was the BlockHashingHandler default-ctor value
// (`BlockHashingHandler.cs:32`, `minReservedCount = 64`), not the
// per-segment Oasis invocation's value. With 262k slots and 0.5
// occupancy threshold the hash_map probe-cap exhausts past ~131k
// unique blocks; Oasis exceeds this and renders scattered speckles
// through walls (see `06-diagnostic-inversion.md` round 2,
// hypothesis H9).
initial_hash_map_size: 1 << 20,   // = 1,048,576 slots (16 MiB GPU buffer)
```

Apply the same change to the const-assert pin at `:207`.

**Expected effect:**
- `prepare_construction` will allocate a 16 MiB hash_map instead of 4
  MiB on the W5 path (acceptable — total GPU footprint goes from ~1.6
  GiB to ~1.62 GiB).
- Probe-cap exhaustion ceiling rises from ~131 k unique blocks to
  ~524 k unique blocks.
- For Oasis (estimated 80 k – 500 k unique blocks per the data_block
  size at 1.6 M u32s = 404 k block entries), this clears the threshold
  with headroom.

### Secondary fix (covers worst-case strokes / very-dense worlds)

Wire `dispatch_map_copy` into the W5 per-segment loop in
`naadf_gpu_producer_node` (currently dispatched only by the W2 editing
path). After each segment's chunk_calc dispatch, optionally read back
the cursor + check occupancy + grow the map. Matches C# `WorldData.cs:148-149`.

**Out of scope for this dispatch** — requires non-trivial wiring + a
CPU readback per segment (which the Stage 1 commentary noted is "not
possible inside a render-graph node" without restructuring). The
primary fix above should suffice for Oasis-class worlds; defer the
secondary until a denser test case demonstrates the need.

### Gate metric correction (required to validate the fix)

**The current near-black-pixel-count metric at the C# pose is not a
valid discriminator** — legitimate dark geometry (ceiling underside +
dark stone interiors) keeps the count at ~35 % regardless of inversion
state. The success metric needs to be either:

- **(a)** A different visual metric at the C# pose that DOES catch
  inversion. Examples: rect mean RGB value (post-fix should be
  *brighter* because fewer hole pixels lower the mean) or a more
  targeted Δ-vs-oracle metric (render twice — once with the GPU
  producer, once with a CPU oracle on the same model — diff the
  results).
- **(b)** Restore the above-world pose as a SECONDARY check, but use
  the C# pose as the PRIMARY: at the C# pose, look at a different
  metric (e.g., count of pixels with a specific "ground-stone" color
  range — if the wall material correctly covers the central rect,
  count is high; if speckled with non-stone colors, count drops).
- **(c)** Hardcoded comparison against a known-good golden screenshot
  at the C# pose, saved alongside the fixture, asserting per-pixel
  diff < threshold.

Option (c) is the most robust — the oracle is the actual visual output
of a known-correct CPU implementation. The other options require
visual judgement to select thresholds; option (c) is byte-comparable.

**Out of scope for this dispatch** — gate redesign should land alongside
the primary fix in Stage 2. The reverted gate WILL FAIL on the C# pose
regardless of any hash fix (legitimate dark geometry dominates), so the
gate's pass/fail outcome cannot be the success oracle for the next
dispatch. The dispatch should use **visual confirmation by the user** as
the success oracle (the user's own re-test of `bevy-naadf -- --vox
Oasis_Hard_Cover.vox`), then update the gate's metric afterward.

## Confidence level

**MEDIUM**

- **HIGH confidence in the identified divergence:** the
  `initial_hash_map_size = 262,144` value is documented as a faithful
  port of `BlockHashingHandler.cs:32` (the default ctor parameter), but
  the actual C# invocation in `WorldData.cs:132` uses
  `minReservedCount = 524,288`, forcing `mapSize >= 1,048,576`. The
  Rust port misread the C# constant.
- **MEDIUM confidence the primary fix is sufficient:** the
  ~131k-block ceiling at the current size IS a real cap, and Oasis
  IS likely to exceed it. But "likely" is not "verified". A direct
  measurement of Oasis's unique-block count would convert this to HIGH;
  the measurement requires either a CPU oracle run that counts unique
  block patterns or a GPU readback of the `hash_map.use_count` field
  after the producer runs. Either is doable; neither was done in this
  dispatch (out of scope per the brief's "investigate + report only"
  constraint).
- **LOW confidence in the gate metric**: the current near-black metric
  at the C# pose is provably wrong (dominated by legitimate dark
  geometry); no replacement was validated in this dispatch.

## Observation evidence

### My run at the reverted C# pose

```
e2e_render --vox-gpu-construction:
rect=(89,89,166,166) frac=(0.35,0.35,0.65,0.65);
rect mean rgba: before=[44.92, 57.48, 69.16, 255],
                after=[52.87, 68.27, 82.87, 255];
rect mean per-pixel RGB Δ=16.71 (floor=8.00);
full-frame mean per-pixel RGB Δ=9.67;
frame-A near-black (lum<10.0) count=23,087 of 65,536 pixels
(35.23% of frame; ceiling=655 pixels = 1.0% of frame).

vox-gpu-construction gate FAIL.
```

The `rect mean rgba` values pre and post Stage-1.5 are not directly
comparable (the Stage 1.5 measurement was at a different pose), but
the 35.2 % near-black count is within noise of the Stage 1.5 impl
log's `23,096 (35.24 %)` measurement at the same pose, confirming the
fix did not change the rendered output at this pose.

### Saved PNG at the reverted pose

`target/e2e-screenshots/vox_gpu_construction_before.png` (camera A at
`(500, 200, 40)` looking `+Z`). Inspected via the Read tool. Shows:
- Top of frame: flat black slab spanning width (the world ceiling
  Y=512 cap, seen from below — Y=200 is below the ceiling).
- Upper-middle: pale water-blue horizon band; the camera is at the
  world's lateral edge (X=500 of 4096) looking `+Z` (= deeper into
  the world); the horizon is the water-level surface of the lake-
  surrounded model.
- Lower-middle: dark city silhouettes, sharply defined (uniform
  blocks render cleanly).
- Bottom: dark stone surface with scattered small water-blue cutout
  fragments — these ARE inversion artifacts (mixed blocks failing
  the hash insert past saturation → sentinel-2 voxel pointer → reads
  zero voxels → renders as the sky/water color leaking through), but
  they are small and dominated by the legitimate dark stone.

The pose framing makes it impossible for any "near-black pixel count
must drop to 0" metric to succeed — the legitimate dark surfaces
(ceiling slab, stone interiors) account for the ~35 % floor.

### Hash buffer sizes verified (Stage 1.5 fix landed)

Via temporary `info!` instrumentation (now removed):

```
ROUND-2 DIAG: W5 producer ENTERING segment loop with
  gpu.hash_map.size=4194304 (= 4 MiB, production)
  gpu.hash_coefficients.size=260 (= 65 × 4 B, production)
  gpu.block_voxel_count.size=8
  gpu.segment_voxel_buffer.size=33554432 (= 32 MiB, production)
  config.initial_hash_map_size=262144
```

Note `config.initial_hash_map_size=262144` — the identified gap. C#'s
fixed-world value is 1,048,576.

### Unit + e2e tests

- `cargo build --workspace` — PASS (~11 s on warm tree).
- `cargo run --release --bin e2e_render -- --vox-gpu-construction` —
  FAIL on the near-black metric (35.23 % > 1 % ceiling at the reverted
  C# pose), as expected; this is the diagnostic FAIL that documents
  the inversion symptom is still present.
