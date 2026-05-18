# vox-gpu-rewrite — voxel encoding C# vs Rust diff (2026-05-18)

## Symptom recap

The user's production-binary symptoms after the W5 GPU producer chain runs
(against `Oasis_Hard_Cover.vox`, 93×34×84-chunk model tiled into the fixed
256×32×256-chunk world):

1. **Visual** — rendered Oasis architecture has "black spaces" that are not
   missing voxels but **back-face renders of inverted surfaces**. The
   chunk-level architectural layout (windows, walls, courtyards) is in the
   correct positions vs the CPU oracle, but most surfaces render dark with
   scattered bright/cream pixels at correct emissive positions plus scattered
   GREEN specks through stone walls.
2. **Edit-mode raycaster** — returns "no hit" for ANY cursor position, even
   when the cursor visibly overlays a voxel.
3. **Render distance suffers** — distant voxels do not render; consistent
   with wrong AADF distance fields → rays single-step or stop short.

The user's hypothesis: **"some glitch in minutia of encoding the voxels"** —
the same class of bug as the prior 1×1×1 edit fix at commit `9559dba`.

The three symptoms are NOT one bug. After exhaustive bit-level comparison
(this document) they are at least TWO orthogonal issues:

- **Symptom 2 has a separate, confirmed mechanical cause** (CPU mirror is
  unpopulated on the W5 install path — `install_vox_in_fixed_world` constructs
  an EMPTY `chunks_cpu`/`blocks_cpu`/`voxels_cpu` and the CPU raycaster
  reads from those empty buffers).
- **Symptoms 1 and 3** point at a GPU-side W5 producer issue. The round-4
  diagnostic narrowed it to "the per-block voxel data layer", not the
  chunks structure. **No bit-level encoding drift was found between C# HLSL
  and Rust WGSL** — every mask, shift, sentinel, discriminator and
  pair-packing convention matches byte-for-byte. The bug is most likely in
  the **hash-table state-machine ordering** of the chunk_calc dedup-hit
  path (consistent with round-4 candidate 3), not in the encoding format
  itself.

## Encoding layer 1: voxel (16 bits)

| Concept | HLSL (`generatorModel.fx`/`chunkCalc.fx`/`rayTracing.fxh`) | WGSL/Rust (`generator_model.wgsl`/`chunk_calc.wgsl`/`ray_tracing.wgsl`) | Match? |
|---|---|---|---|
| Voxel total width | 16 bits per voxel half-word | 16 bits per voxel half-word | ✓ |
| Type bits | bits 0–14 (15 bits) — masked via `& 0x7FFF` (`chunkCalc.fx:128, :40`, `rayTracing.fxh:142`) | bits 0–14 (15 bits) — masked via `& 0x7FFFu` (`chunk_calc.wgsl:361, :100, :102`, `ray_tracing.wgsl:383`) | ✓ |
| Full flag | bit 15 — set by `voxel \| (1 << 15)` when `type > 0` (`generatorModel.fx:67-68`) | bit 15 — set by `voxel \| (1u << 15u)` when `type > 0u` (`generator_model.wgsl:149-154`) | ✓ |
| Pair packing | `pair = low \| (high << 16)` (`generatorModel.fx:70`); even index in low, odd in high | `pair = low \| (high << 16u)` (`generator_model.wgsl:158`, `chunk_calc.wgsl:498`); even index in low, odd in high | ✓ |
| Pair extraction (chunk_calc) | `low = pair & 0xFFFF`, `high = pair >> 16` (`chunkCalc.fx:201`) | `low = pair & 0xFFFFu`, `high = pair >> 16u` (`chunk_calc.wgsl:467-471`) | ✓ |
| Pair extraction (renderer) | `(pair >> (16 * (idx & 1))) & 0xFFFF` (`rayTracing.fxh:128`) | `(pair >> (16u * (idx & 0x1u))) & 0xFFFFu` (`ray_tracing.wgsl:336`) | ✓ |
| Empty voxel | All-zero (`type = 0`, `flag = 0`) | All-zero | ✓ |
| Full voxel re-tag in renderer | `if (voxel >> 15) curNode.x \|= (1 << 30)` (`rayTracing.fxh:129-130`) | `if ((cur_node >> 15u) != 0u) cur_node = cur_node \| (1u << 30u)` (`ray_tracing.wgsl:339-341`) | ✓ |

**Verdict — layer 1: BYTE-EQUAL.**

## Encoding layer 2: block descriptor (32 bits)

| Concept | HLSL | WGSL/Rust | Match? |
|---|---|---|---|
| Total width | 32-bit `uint` | 32-bit `u32` | ✓ |
| Discriminator bits | bits 30–31 (top 2 bits) — read via `>> 30` (`chunkCalc.fx:228`) | bits 30–31 — read via `>> 30u` (`chunk_calc.wgsl:520, :486`) | ✓ |
| `disc = 0` meaning | `BLOCK_STATE_UNIFORM_EMPTY` (`chunkCalc.fx:6`) | `BLOCK_STATE_UNIFORM_EMPTY = 0u` (`chunk_calc.wgsl:236`) | ✓ |
| `disc = 1` meaning | `BLOCK_STATE_UNIFORM_FULL` (`chunkCalc.fx:7`) | `BLOCK_STATE_UNIFORM_FULL = 1u` (`chunk_calc.wgsl:237`) | ✓ |
| `disc = 2` meaning | `BLOCK_STATE_CHILD` (mixed) (`chunkCalc.fx:5`) | `BLOCK_STATE_CHILD = 2u` (`chunk_calc.wgsl:235`) | ✓ |
| Payload mask | `& 0x3FFFFFFF` (low 30 bits) (`generatorModel.fx:34, 39`; `chunkCalc.fx`; `rayTracing.fxh:118`) | `& 0x3FFFFFFFu` (`generator_model.wgsl:87, 96`, `chunk_calc.wgsl:322`, `ray_tracing.wgsl:322`) | ✓ |
| Uniform-full payload | low 15 bits = type (`chunkCalc.fx:140`) — payload is `firstVoxelType` (already masked to 15 bits at `:128`) | low 15 bits = type (`chunk_calc.wgsl:381`) — payload is `first_voxel_type` (already masked at `:361`) | ✓ |
| Mixed payload | bits 0–29 = block ptr (= voxel ptr base for blocks) | bits 0–29 = block ptr / voxel ptr base | ✓ |
| 2-bit AADF positions | bits 0–11; per-direction at shifts `0,2,4,6,8,10` (`rayTracing.fxh:78`) | shifts `0,2,4,6,8,10` (`chunk_calc.wgsl`, `bounds_common.wgsl`, `ray_tracing.wgsl:215-218`) | ✓ |
| Renderer mixed-detect | `cur_node >> 31` (top bit) (`rayTracing.fxh:115, 122`) | `(cur_node >> 31u) != 0u` (`ray_tracing.wgsl:319, 326`) | ✓ (`disc=2` ⇔ bit 31 set ⇔ `>>31 == 1`) |
| Renderer hit-detect | `cur_node & 0x40000000` (bit 30) (`rayTracing.fxh:140`) | `(cur_node & 0x40000000u) != 0u` (`ray_tracing.wgsl:382`) | ✓ |
| Block uniform-empty encoding | `firstVoxelType \| (0 << 30) = 0` when type==0 (`chunkCalc.fx:140`) | `first_voxel_type \| (0u << 30u) = 0u` when type==0 (`chunk_calc.wgsl:375-381`) | ✓ |
| Block uniform-full encoding | `firstVoxelType \| (1 << 30) = ty \| 0x40000000` (`chunkCalc.fx:140`) | `first_voxel_type \| (1u << 30u)` (`chunk_calc.wgsl:381`) | ✓ |
| Block mixed encoding | `voxel_ptr \| (2 << 30)` (`chunkCalc.fx:142`) | `voxel_ptr \| (2u << 30u)` (`chunk_calc.wgsl:383`) | ✓ |

**Verdict — layer 2: BYTE-EQUAL.**

## Encoding layer 3: chunk descriptor (32 bits)

Same shape as block descriptor.

| Concept | HLSL | WGSL/Rust | Match? |
|---|---|---|---|
| Total width | 32-bit `uint` | 32-bit `u32` | ✓ |
| Discriminator bits | bits 30–31 — read via `>> 30` (`generatorModel.fx:30, 35, 42, 45`) | bits 30–31 — read via `>> 30u` (`generator_model.wgsl:81, 90, 104, 108`) | ✓ |
| `disc = 0/1/2` mapping | empty / uniform-full / mixed (same constants as block) | same | ✓ |
| 5-bit AADF positions | bits 0–29; per-direction at shifts `0,5,10,15,20,25` (`rayTracing.fxh:79`) | shifts `0,5,10,15,20,25` (`bounds_calc.wgsl:146-167`, `ray_tracing.wgsl:220-224`) | ✓ |
| Renderer write (chunks-pair, `.x` channel) | `chunks[chunkPos] = state` or `uint2(state, 0)` (`chunkCalc.fx:170-174`) | `chunks[chunk_idx] = vec2<u32>(state, 0u)` (`chunk_calc.wgsl:427`) | ✓ (W4 widening identical) |
| Renderer write (chunks-pair, `.y` preservation) | `uint2(new_state, chunks[pos].y)` (`worldChange.fx:124`) | `vec2<u32>(change.y, cur.y)` (`world_change.wgsl:454`) | ✓ |
| Flatten layout | C# uses `Texture3D<>` with `chunks[chunkPos]` 3D index | x-fastest flat: `idx = x + y*sx + z*sx*sy` (`chunk_calc.wgsl:424-426`, `ray_tracing.wgsl:290-294`, `common.wgsl:32-34`) | ✓ (semantically identical: producer + consumer use identical flatten formula) |

**Verdict — layer 3: BYTE-EQUAL.**

## Encoding layer 4: AADF storage format

The AADFs are stored **interleaved with type/state bits inside the SAME `u32`**
(not in a separate buffer). For empty cells the AADF bits ARE the cell's
payload; for non-empty cells the AADF bits are zero / unused.

| Concept | HLSL | WGSL/Rust | Match? |
|---|---|---|---|
| **Voxel-layer AADF storage** | 2-bit fields at shifts `0,2,4,6,8,10` in the same 16-bit voxel half-word as the type. The 5 used bits (12-bit AADF + 1-bit full flag = 13 bits) leave bits 13–14 unused. | Identical layout (`chunk_calc.wgsl:482, 466-499`; `world_change.wgsl:546-555`) | ✓ |
| **Block-layer AADF storage** | 2-bit fields at shifts `0,2,4,6,8,10` in the same 32-bit block word as the disc + payload. State bits 30–31 must be 0 (uniform-empty) for the AADF bits to be readable. | Identical (`chunk_calc.wgsl:529, 518-538`; `world_change.wgsl:489-504`; `bounds_common.wgsl:36-41, 92-110`) | ✓ |
| **Chunk-layer AADF storage** | 5-bit fields at shifts `0,5,10,15,20,25` in the chunk word. State bits 30–31 must be 0. | Identical (`bounds_calc.wgsl:131-167, 374-394`; `ray_tracing.wgsl:368-378`) | ✓ |
| **Renderer AADF read (voxel/block)** | `(cur_node >> shift) & 0x3` for each of x/y/z (`rayTracing.fxh:132`) | `(cur_node >> shift_voxel_block.x) & 0x3u` (`ray_tracing.wgsl:344-348`) | ✓ |
| **Renderer AADF read (chunk)** | `(cur_node >> shift) & 0x1F` (`rayTracing.fxh:138`) | `(cur_node >> shift_chunk.x) & 0x1Fu` (`ray_tracing.wgsl:374-378`) | ✓ |
| **Shift selection by ray sign** | `shift = isNegative.x ? 0 : 2` for voxel/block x (`rayTracing.fxh:78`); `shift = isNegative.x ? 0 : 5` for chunk x (`rayTracing.fxh:79`) | `select(2u, 0u, is_negative.x == 1u)` for voxel/block x (`ray_tracing.wgsl:215-218`); `select(5u, 0u, is_negative.x == 1u)` for chunk x (`ray_tracing.wgsl:220-224`) | ✓ |
| **Direction-mask exclusion (back-pointer)** | `MASK_MX..MASK_PZ = 0x3D, 0x3E, 0x37, 0x3B, 0x1F, 0x2F` (`boundsCommon.fxh:6-11`) | Same constants (`bounds_common.wgsl:36-41`, `chunk_calc.wgsl:127-132`, `world_change.wgsl:160-165`, `bounds_calc.wgsl:131-136`) | ✓ |
| **`compute_bounds_4` direction offsets** | `-1, +1, -4, +4, -16, +16` for 6 sides (`boundsCommon.fxh:44-60`) | Same (`bounds_common.wgsl:142-187`, `chunk_calc.wgsl:196-225`, `world_change.wgsl:239-268`) | ✓ |
| **`compute_bounds_4` bounds_location values** | `0, 2, 4, 6, 8, 10` (`boundsCommon.fxh:44-60`) | Same (same files) | ✓ |
| **`compute_bounds_4` state_location/state_mask for voxels** | `15, 0x1` (`chunkCalc.fx:208`) | `15u, 0x1u` (`chunk_calc.wgsl:482`, `world_change.wgsl:555`) | ✓ |
| **`compute_bounds_4` state_location/state_mask for blocks** | `30, 0x3` (`chunkCalc.fx:233`) | `30u, 0x3u` (`chunk_calc.wgsl:529`, `world_change.wgsl:497`) | ✓ |
| **`bounds_calc` chunk-AADF growth direction** | `boundXYZ * 10 + 0` for -dir, `boundXYZ * 10 + 5` for +dir (`boundsCalc.fx:156-157`) | `bound_xyz * 10u + 0u` and `+ 5u` (`bounds_calc.wgsl:388-394`) | ✓ |

**Verdict — layer 4: BYTE-EQUAL.**

The "AADF storage format" question reduces to "are the bits stored where the
renderer reads them from?" — and yes, the producer writes and the renderer
reads at the same bit positions on both sides.

## Encoding layer 5: hash table

| Concept | C# (`BlockHashingHandler.cs`, `chunkCalc.fx`, `mapCopy.fx`) | Rust/WGSL (`chunk_calc.wgsl`, `gpu_types.rs`) | Match? |
|---|---|---|---|
| Slot struct fields | `voxelPointer: uint`, `useCount: uint`, `hashRaw: uint` (`chunkCalc.fx:13-20`) | `voxel_pointer: atomic<u32>`, `use_count: atomic<u32>`, `hash_raw: u32` (`chunk_calc.wgsl:61-69`) | ✓ (semantically; atomic typing is a WGSL access discipline only) |
| Slot stride | **12 bytes** (3 × `uint`, no padding) — `StructuredBuffer<HashValue>` natural alignment | **16 bytes** (3 × `u32` + 4-byte `_pad`) — WGSL/Rust convention (`gpu_types.rs:646-659`) | DIVERGENT but INTERNALLY CONSISTENT — Rust/WGSL both stride at 16; C# both stride at 12. Documented at `gpu_types.rs:638-643`. |
| Empty-slot sentinel | `voxelPointer == 0` (`EMPTY_BLOCK = 0x0`, `chunkCalc.fx:4`) | `voxel_pointer == 0u` (`EMPTY_BLOCK = 0x0u`, `chunk_calc.wgsl:234`) | ✓ |
| Pending-claim sentinel | `voxelPointer == 0x80000000 \| voxelRawStart` (top bit set) (`chunkCalc.fx:67, 89`) | `voxel_pointer == PENDING_BIT \| voxel_raw_start` (`chunk_calc.wgsl:239, 275, 310`) | ✓ |
| Final-claim layout | `voxelPointer = originalIndex` (= `block_voxel_count[0] / 2`, in u32-offset units) (`chunkCalc.fx:74, 81`) | `voxel_pointer = voxel_u32_start` (= `original_index >> 1u`) (`chunk_calc.wgsl:291, 301`) | ✓ |
| CAS protocol | `InterlockedCompareExchange(target, EMPTY_BLOCK, PENDING\|raw_start, original)` | `atomicCompareExchangeWeak(&target, EMPTY_BLOCK, PENDING_BIT\|voxel_raw_start).old_value` (`chunk_calc.wgsl:272-277`) | ✓ |
| Probe cap | 250 iterations (`chunkCalc.fx:62`) | 250 iterations (`PROBE_CAP = 250u`, `chunk_calc.wgsl:238`) | ✓ |
| Probe-exhaustion sentinel | `return 2` (`chunkCalc.fx:114`) | `return 2u` (`chunk_calc.wgsl:339`) | ✓ |
| Spin-wait cap | 2000 iterations (`chunkCalc.fx:89`) | 2000 iterations (`PENDING_WAIT_CAP = 2000u`, `chunk_calc.wgsl:240`) | ✓ |
| Read-with-fence (spin) | `InterlockedOr(target, 0, out)` (HLSL idiom for sequentially-consistent read) (`chunkCalc.fx:88, 91`) | `atomicLoad(&target)` (`chunk_calc.wgsl:307, 313`) | ✓ |
| Cursor seed | `blockVoxelCount = [64, 64]` (`WorldData.cs:129`) — voxels cursor starts at 64, blocks cursor at 64 | `block_voxel_count = [64u32, 64u32]` (`construction/mod.rs:1027`) | ✓ |
| Hash polynomial | `coef[64] = 1; coef[i] = 31 * coef[i+1]` (`BlockHashingHandler.cs:50-55`) | Same (`hashing::hash_coefficients`) | ✓ |
| Hash accumulator | `hash = coef[0] + sum(coef[i*2+1] * (low & 0x7FFF) + coef[i*2+2] * (high & 0x7FFF))` (`chunkCalc.fx:126-134`) | Same (`chunk_calc.wgsl:359-371`) | ✓ |
| Dedup-hit equality test | Compare 32 u32s `segmentVoxelBuffer[raw+i] == voxels[ptr+i]` (full 32-bit comparison including bit 15 full flag) (`chunkCalc.fx:97-99`) | Same (`chunk_calc.wgsl:321-326`) | ✓ |
| Dedup-hit hash gate | `if (hashMap[bounds].hashRaw == hash) { ... }` (`chunkCalc.fx:94`) | `if (hash_map[hash_bounds].hash_raw == hash) { ... }` (`chunk_calc.wgsl:319`) | ✓ |
| `hash_raw` write ordering | non-atomic write BEFORE `InterlockedExchange(voxel_pointer, originalIndex)` (`chunkCalc.fx:79-81`) | non-atomic write BEFORE `atomicStore(&voxel_pointer, voxel_u32_start)` (`chunk_calc.wgsl:299-301`) | ✓ same ordering on both sides |

**Verdict — layer 5: BYTE-EQUAL (modulo the documented 12B-vs-16B slot
stride, which is internally consistent on each side).**

## Renderer decode side-by-side

The renderer (`rayTracing.fxh::shootRay` vs `ray_tracing.wgsl::shoot_ray`)
follows the chunk → block → voxel descent with identical bit tests at every
level. Side-by-side:

| Step | HLSL (`rayTracing.fxh`) | WGSL (`ray_tracing.wgsl`) |
|---|---|---|
| Chunk fetch | `curNode = chunks[chunkPos]` (3D texture, `:105`) | `chunk_texel = chunks[chunk_idx]; cur_node = chunk_texel.x` (flat array, `:295-296`) |
| Mixed-chunk test | `if (curNode.x >> 31)` (bit 31; equivalent to `disc==2`) (`:115`) | `if ((cur_node >> 31u) != 0u)` (`:319`) |
| Mixed-chunk descent | `block_idx = (curNode.x & 0x3FFFFFFF) + FLATTEN_INDEX(blockPos, 4, 16); curNode.x = blocks[block_idx]` (`:117-119`) | Same (`:321-323`) |
| Mixed-block test | `bool blockIsParent = curNode.x >> 31` (`:122`) | `let block_is_parent = (cur_node >> 31u) != 0u` (`:326`) |
| Mixed-block descent | `voxelStart = (curNode.x & 0x3FFFFFFF) + voxelIdx/2; voxelPair = voxels[voxelStart]; curNode.x = (voxelPair >> (16*(voxelIdx&1))) & 0xFFFF` (`:125-128`) | Same (`:332-336`) |
| Full-voxel re-tag | `if (curNode.x >> 15) curNode.x \|= (BLOCK_STATE_UNIFORM_FULL << 30)` (`:129-130`) | `if ((cur_node >> 15u) != 0u) cur_node = cur_node \| (1u << 30u)` (`:339-341`) |
| AADF read (voxel/block) | `boundsInDir = ((curNode.x >> shiftMaskVoxelAndBlocks) & 0x3)` (`:132`) | Same with `shift_voxel_block` (`:344-348`) |
| AADF read (chunk uniform-empty) | `boundsInDir = (isNegative ? voxelPosInChunk : 15u - voxelPosInChunk) + 16u * ((curNode.x >> shiftMaskChunk) & 0x1F)` (`:138`) | Same with `shift_chunk` (`:369-378`) |
| Hit test | `if (curNode.x & 0x40000000) { rayResult.type = curNode.x & 0x7FFF; ... break; }` (`:140-146`) | Same (`:382-387`) |
| DDA step | `distForIntersect = (1 + boundsInDir - (1 - mask) * abs(isNegative - frac(curPos))) * invRayDirAbs` (`:147`) | Same (`:392-395`) |

**Verdict — renderer decode: BYTE-EQUAL.** Every bit test, every shift,
every mask matches the C# source.

## Edit-mode raycaster decode side-by-side

**THIS SECTION HOLDS THE LOAD-BEARING DIAGNOSTIC for symptom 2.**

The edit-mode raycaster is **CPU-side** in the Rust port —
`WorldData::ray_traversal` at `world/data.rs:406-590` (port of C#
`WorldData.RayTraversal:396-473`). It reads from `self.chunks_cpu`,
`self.blocks_cpu`, `self.voxels_cpu` — **the CPU mirror**, NOT the GPU
buffers.

| Step | C# (`WorldData.cs:396-473`) | Rust (`world/data.rs:406-590`) |
|---|---|---|
| Chunk fetch | `dataChunk[chunkIdx]` (CPU mirror) | `self.chunks_cpu[chunk_idx]` |
| Mixed-chunk test | `curNode >> 31` | `chunk_state == 2` (= `cur_node >> 30 == 2`, equivalent) |
| Block fetch | `dataBlock[block_idx]` | `self.blocks_cpu[block_idx]` |
| Voxel fetch | `dataVoxel[voxel_base_pair]` | `self.voxels_cpu[pair_idx]` |
| Full-voxel detect | `(half & 0x8000) != 0` | `(half & 0x8000) != 0` (`:534`) |
| Hit promotion | `curNode = (1 << 30) \| (half & 0x7FFF)` | `cur_node = (1 << 30) \| (half & 0x7FFF)` (`:537`) |
| Hit test | `curNode & 0x4000_0000` | `cur_node & 0x4000_0000` (`:549`) |

The CPU raycaster's bit-level decode is faithful to the C# CPU raycaster.

### The actual symptom-2 bug

`install_vox_in_fixed_world` (`voxel/grid.rs:317-429`) is the W5 install path.
At `:409-425` it constructs the `WorldData` Resource the editor uses:

```rust
let mut world_data = WorldData {
    chunks_cpu: Vec::new(),        // <-- EMPTY!
    blocks_cpu: Vec::new(),        // <-- EMPTY!
    voxels_cpu: Vec::new(),        // <-- EMPTY!
    size_in_chunks: WORLD_SIZE_IN_CHUNKS,
    bounding_box: IAabb3 { ... },
    pending_edits: Default::default(),
    dense_voxel_types: Vec::new(),
    block_hashing: ...,
};
world_data.seed_block_hashing();   // <-- no-op on empty buffers
commands.insert_resource(world_data);
```

The GPU producer populates GPU buffers; **the CPU mirror stays empty**.
`WorldData::ray_traversal` at `world/data.rs:479-481` hits:

```rust
if chunk_idx >= self.chunks_cpu.len() {
    return None;
}
```

`chunks_cpu.len() = 0` for every position → `chunk_idx >= 0` is true for any
non-negative chunk_idx → **every ray_traversal call returns `None`**.

This is the **exact symptom #2 mechanism**. Every edit-mode raycast misses.
It is unrelated to the W5 GPU producer's encoding — it is a missing CPU
mirror initialization on the W5 install path.

By contrast, the legacy `install_vox_sized_to_model` path (`voxel/grid.rs:254-315`)
goes through `WorldData::from_dense_volume` or `WorldData::from_imported_world`
which populates `chunks_cpu`/`blocks_cpu`/`voxels_cpu` with the CPU
construction output. The edit-mode raycaster works on the legacy path
because the CPU mirror is non-empty.

## chunk_calc encoding side-by-side

The W5 chain: generator writes `segment_voxel_buffer` (raw types + full
flags); chunk_calc reads it, hashes + dedups + writes into the chunks /
blocks / voxels buffers.

| Step | HLSL (`chunkCalc.fx`) | WGSL (`chunk_calc.wgsl`) |
|---|---|---|
| Per-block voxel pair read | `voxels[voxelIndexInSegment + i]` (`:127, 132`) | `segment_voxel_buffer[voxel_index_in_segment + i]` (`:360, 365`) |
| Hash low half | `hash += coef[i*2+1] * (voxelComp & 0x7FFF)` (`:133`) | `hash += coef[i*2+1] * (voxel_comp & 0x7FFFu)` (`:366`) |
| Hash high half | `hash += coef[i*2+2] * ((voxelComp >> 16) & 0x7FFF)` (`:134`) | `hash += coef[i*2+2] * ((voxel_comp >> 16u) & 0x7FFFu)` (`:367`) |
| All-same check (init) | `(firstVoxelTypeComp & 0xFFFF) == (firstVoxelTypeComp >> 16)` (`:129`) | Same (`:362-363`) |
| All-same per-iter | `isAllSame && (firstVoxelTypeComp == voxelComp)` (`:135`) | `if (first_voxel_type_comp != voxel_comp) is_all_same = false;` (`:368-370`) — logically equivalent |
| Per-block state encoding | `firstVoxelType \| (firstVoxelType == 0 ? 0 : 1) << 30` (`:140`) | `first_voxel_type \| (select(1u, 0u, first_voxel_type == 0u) << 30u)` (`:375-381`) — same: 0 if type=0, `\|0x40000000` if type>0 |
| Per-block child encoding | `GetVoxelPointer(hash, voxelIndexInSegment) \| 2 << 30` (`:142`) | `get_voxel_pointer(hash, voxel_index_in_segment) \| (2u << 30u)` (`:383`) |
| Per-chunk all-same gate | `if (block != referenceBlock \|\| !isAllSame) isAllBlocksEqual = false;` (`:154-155`) | Same (`:396-398`) — uses `atomicStore` since WGSL needs atomic for cross-thread workgroup state |
| Per-chunk uniform encoding | `firstVoxelType \| (firstVoxelType == 0 ? 0 : 1) << 30` (`:161-163`) | Same (`:405-410`) |
| Per-chunk child encoding (`InterlockedAdd` cursor) | `InterlockedAdd(blockVoxelCount[1], 64, insertBlockIndex); state = insertBlockIndex \| 2 << 30` (`:166-167`) | `atomicAdd(&block_voxel_count[1], 64u); atomicStore(&insert_block_index, new_base); state = new_base \| (2u << 30u)` (`:412-414`) |
| Chunks write | `chunks[chunkPos] = state` (or `uint2(state, 0)` with ENTITIES) (`:170-174`) | `chunks[chunk_idx] = vec2<u32>(state, 0u)` (`:427`) |
| Blocks write | `blocks[insertBlockIndex + localIndex] = block` (`:180`) | `blocks[base + local_index] = block` (`:434`) |
| GetVoxelPointer slot claim | `InterlockedCompareExchange(target, 0, 0x80000000 \| raw_start, original)` (`:67`) | `atomicCompareExchangeWeak(&target, EMPTY_BLOCK, PENDING_BIT \| voxel_raw_start).old_value` (`:272-277`) |
| Voxels copy (new slot) | `voxels[originalIndex + i] = segmentVoxelBuffer[voxelRawStart + i]` for i in 0..32; **originalIndex = block_voxel_count[0] / 2** (`:74-78`) | Same (`:291-295`); `voxel_u32_start = original_index >> 1u` |
| hash_raw write | `hashMap[hashBounds].hashRaw = hash` (non-atomic, BEFORE the atomicStore) (`:79`) | Same (`:299`) |
| Atomic-store final voxel_pointer | `InterlockedExchange(target, originalIndex, _)` (`:81`) | `atomicStore(&target, voxel_u32_start)` (`:301`) |
| Spin-wait on PENDING | `InterlockedOr(target, 0, out); while ((out & 0x80000000) != 0 && ++c < 2000) ...` (`:88-92`) | Same shape (`:307-314`) |
| Dedup-hit equality | `isAllEqual = true; for i: isAllEqual &= segmentVoxelBuffer[raw+i] == voxels[ptr+i];` (`:97-99`) | Same (`:319-326`) |

**Verdict — chunk_calc encoding: BYTE-EQUAL.**

Every dispatch parameter, every bit test, every cursor unit conversion
matches the C# source. The cursor seed `[64, 64]` matches C#'s
`WorldData.cs:129`. The voxel-index-to-u32-index conversion (`originalIndex
/= 2`) matches the WGSL `>> 1u`.

## Prior 1×1×1 edit bug history

Commit `9559dba` (May 17, 2026):
**"fix(edit)+feat(e2e): port BlockHashingHandler + idempotent AADF shader"**

The user's prior bug — "small (radius=1) cube/sphere brush placements
rendered as inside-out / partially-opaque shapes with pitch-black faces" —
was a different class of bug than the current W5 inversion:

> Root cause: the GPU `apply_voxel_change` / `apply_block_change` kernels
> are *additive* on the per-axis 2-bit AADF fields — `compute_bounds_4`
> runs 3 iterations, each adding `+1` per axis when the neighbour is
> empty and the matching-bounds check passes. The algorithm assumes
> empty-cell inputs start at AADF=0; once the input carries already-
> computed AADFs (the case in this port because `build_chunk_edit_window`
> copies `voxels_cpu` verbatim), repeated additions overflow the 2-bit
> fields and corrupt the AADF state...

The fix had two parts:
1. **Port of `BlockHashingHandler`** so the CPU side does proper voxel-slot
   dedup with refcounting + free-list reuse (the C# `EditingHandler` path).
2. **WGSL idempotency** in `apply_block_change` and `apply_voxel_change`:
   zero-out empty cells before running `compute_bounds_4`, so repeated runs
   don't compound AADF bumps.

`compute_voxel_bounds` and `compute_block_bounds` in `chunk_calc.wgsl`
(the W5 chain's AADF pass) **DO NOT have this zero-reset**, but they
don't need it either: chunk_calc's voxel copy writes raw model voxels
(no AADF bits set, only type + full flag) so the input to
`compute_voxel_bounds` always starts at AADF=0. **The class precedent
DOES NOT APPLY here** — there is no second run of compute_voxel_bounds
over already-augmented voxels in the W5 chain.

git log search for related commits:

```
$ git log --all --oneline --grep -E "invert|edit|voxel|encod"
```

Results: `9559dba`, `5ef2d14`, `117a8ca`, `e6bd4de` — all editor-class
fixes, none touching the W5 construction encoding.

```
$ git log --all --oneline -S "1u << 15u"
$ git log --all --oneline -S "1 << 15"
```

Results: `912c984` (W5 initial — set the full flag), `53a4c8f` (W1 — copy
through chunk_calc), `9559dba` (fix uses `>> 15` in state test). No commit
landed a "swap bit 15 with another bit" or "mask off bit 15" change. The
full-flag bit position is consistent across all commits.

```
$ git log --all --oneline -S "0x7FFF"
$ git log --all --oneline -S "0x8000"
```

Results show consistent use: 0x7FFF = 15-bit type mask, 0x8000 = full
flag. No edit ever changed these.

**Conclusion**: prior fix is in a sibling kernel family (the edit-time
W2 chain), not the W5 producer chain. The fix is not directly relevant
to the current bug.

## Identified divergence(s)

After exhaustive bit-level comparison, **NO ENCODING DRIFT was found
between the C# HLSL and Rust WGSL paths**:

- Layer 1 (voxel 16-bit): BYTE-EQUAL.
- Layer 2 (block 32-bit): BYTE-EQUAL.
- Layer 3 (chunk 32-bit): BYTE-EQUAL.
- Layer 4 (AADF storage interleaved): BYTE-EQUAL.
- Layer 5 (hash table): BYTE-EQUAL (modulo 12B-vs-16B slot stride which
  is internally consistent on each side).
- Renderer decode: BYTE-EQUAL.
- chunk_calc encoding: BYTE-EQUAL.

The **one confirmed bug** found by this diagnostic is unrelated to GPU
encoding:

### Bug D1 — CPU mirror unpopulated on W5 install path (CAUSES SYMPTOM 2)

**Location**: `crates/bevy_naadf/src/voxel/grid.rs:409-425` in
`install_vox_in_fixed_world`.

**Mechanism**: The function constructs a `WorldData` Resource with
empty `chunks_cpu` / `blocks_cpu` / `voxels_cpu` buffers. The CPU-side
`WorldData::ray_traversal` (used by the editor's mouse-pick) returns
`None` immediately when `chunk_idx >= self.chunks_cpu.len()` (i.e.,
always, since `chunks_cpu.len() == 0`). The editor's brush cannot
find any voxel to anchor to.

**Predicted impact**: matches user symptom 2 ("edit-mode raycaster
returns 'no hit' for ANY voxel position") exactly.

**This is NOT an encoding bug** — it is a missing population of the CPU
mirror on the W5 install path. The CPU mirror was historically built by
the CPU construction path; the W5 GPU path replaces that with GPU
buffers but doesn't mirror the output back to CPU.

### Symptoms 1 and 3 (visual inversion + render distance) — NOT explained by encoding drift

The round-4 diagnostic (`09-diagnostic-inversion-round-4.md`) established
that:
- Chunk-layer architecture renders CORRECTLY (windows + walls in right
  positions in the GPU phase).
- The visible bug is in the per-block voxel data layer.
- Memory ordering (`voxels[]` and `hash_raw` made atomic) had zero effect.
- Hash-map capacity (bumped 32× to 8M slots) had zero effect.
- Bounds chain (skipped entirely) had zero effect.
- Disabling dedup gave pure-empty world (every probe exhausts).

The round-4 candidate 3 hypothesis — **the dedup-hit's slot-claim state
machine has a subtle write-write race during the PENDING → final transition
that visibly manifests on NVIDIA Vulkan despite the WGSL/naga translation
appearing correct** — is the only candidate not yet refuted by experiment
and not contradicted by this encoding diagnostic.

This is NOT an encoding bug. It is a memory-ordering bug in the slot-claim
state machine that produces the SAME ENCODING BYTES at the wrong moment for
the dedup-comparison reader.

## Recommended fix (NOT to be implemented)

### Fix for D1 (symptom 2 — edit raycaster never hits)

`crates/bevy_naadf/src/voxel/grid.rs:409-425`:

**Approach A (minimal-effort, CORRECTNESS-FAITHFUL)**: After the W5 GPU
producer chain runs and populates GPU buffers, run a one-shot
`mapCopy.fx`-equivalent GPU→CPU sync to populate `chunks_cpu` /
`blocks_cpu` / `voxels_cpu` from the GPU buffers. The C# does this in
`WorldData.cs:158-198` (the `dataChunkGpu.GetData(dataChunk)` and
similar `CopyFromStructuredBufferLarge` calls). This requires:
1. A post-W5 sync system that triggers AFTER `gpu_producer_has_run`
   flips to true.
2. The sync system reads the 3 GPU buffers back to CPU vectors and
   assigns them into `WorldData.chunks_cpu` / `blocks_cpu` /
   `voxels_cpu` on the main world.
3. `WorldData::seed_block_hashing()` is then re-run on the populated
   buffers (currently it is a no-op on the empty buffers).

**Approach B (cheaper, partial)**: Replace the CPU `ray_traversal` for
the W5 install path with a GPU-driven raycaster that reads the GPU
buffers. NAADF has `WorldData.cs::RayTraversal` (CPU) + the GPU
`shootRay` (renderer). The editor could submit a one-shot single-thread
compute dispatch that runs `shoot_ray` against the GPU buffers and reads
back a `RayResult` struct. This avoids the CPU mirror entirely. The C#
editor uses CPU raycasting (because dataChunk/dataBlock/dataVoxel are
mirrored back), so Approach A is more C#-faithful.

### Fix for symptoms 1 + 3 (visual inversion + render distance) — DEFER

Per the round-4 candidates section, the next diagnostic dispatch should:
1. **Long-warmup convergence test** (Candidate 1) — re-run the oracle gate
   with 1000+ frames warmup to confirm or rule out "GPU eventually
   converges to CPU output". If it converges, the bug is warmup duration
   (likely W3 chunk-AADF queue takes more frames than the e2e harness
   allows for the 256×32×256 world).
2. **GPU readback diagnostic** (Candidate 2) — add a per-pixel readback
   of the produced `chunks` / `blocks` / `voxels` buffers and compare
   against the CPU oracle's CPU buffers for overlap-region chunks. Move
   from "render compare" to "data compare" to localize the divergence to
   a specific buffer + specific cell.
3. **Fence-instrumented chunk_calc** (Candidate 3) — manually unroll
   `get_voxel_pointer` and insert explicit `atomicFence` calls between
   slot-claim phases. If this resolves the issue, the bug is naga
   missing a barrier in the `atomicCompareExchangeWeak` → non-atomic
   write → `atomicStore` sequence.

**None of these are encoding fixes.** This diagnostic refutes the
"encoding bit drift" hypothesis after layer-by-layer comparison.

## Confidence level

- **HIGH confidence: D1 (CPU mirror unpopulated) is the cause of symptom 2.**
  Direct code inspection at `voxel/grid.rs:409-425` shows the empty
  initialization; `world/data.rs:479-481` shows the immediate-`None` gate;
  the chain is mechanical.
- **HIGH confidence: no bit-level encoding drift exists between C# HLSL
  and Rust WGSL** in any of the 5 layers, the renderer, the edit raycaster,
  or the chunk_calc producer. Every mask, shift, sentinel, discriminator,
  and pair-packing convention was checked side-by-side and matches.
- **LOW confidence: any specific candidate for symptoms 1 + 3.** The
  round-4 diagnostic already refuted the top candidates (atomicity, hash
  capacity, bounds chain, dedup disable, per-segment submit collapse).
  This encoding diagnostic ADDS the further refutation: no bit-level
  drift exists.
- **MEDIUM-LOW confidence: the bug is in the chunk_calc dedup-hit
  state-machine ordering**, possibly downstream of WGSL/naga atomic
  semantics on NVIDIA Vulkan. Round-4 candidate 3 + this diagnostic's
  refutation of every alternative is the strongest remaining hypothesis,
  but specific instrumentation has not yet been performed.
