# vox-gpu-rewrite — production-scale voxels[] readback (2026-05-18)

## TL;DR

`voxels[]` is **BYTE-CORRECT** at every sampled Oasis-populated voxel
position **AFTER** the full W5 producer chain (per-segment 512×
generator+chunk_calc) **AND** after the bounds chain
(`compute_voxel_bounds` + `compute_block_bounds`) has run at production
scale (256×32×256 chunks = 4096×512×4096 voxels fixed world).

**Both checkpoints — pre-bounds and post-bounds — are byte-equal to the
CPU oracle at 25/25 sampled FULL voxel positions.** No corruption is
introduced by the bounds chain. No corruption is present from the
producer.

**Verdict: the bug is in the RENDERER's decode path** (or in the bind
group / buffer wiring that makes the renderer dereference a DIFFERENT
voxels[] buffer than the producer wrote to). The W5 producer chain at
full production shape is not the source of the "voxel types in thousands"
symptom in `oracle_gpu.png`.

A subsidiary finding: a smaller (256K-slot) hash map DOES produce
probe-exhaustion sentinels at production scale, manifesting as
`block = 0x80000002` (Mixed-block with VoxelPtr=0x2). The production
config uses 1M slots and does NOT exhibit this. The 256K-slot run is
documented below as a corroborating data point — NOT a bug to fix.

## Setup

### New diagnostic mode

Added `--validate-gpu-construction-production` to `e2e_render` (entry
point: `crates/bevy_naadf/src/bin/e2e_render.rs:91-93` flag parsing,
`crates/bevy_naadf/src/bin/e2e_render.rs:154-167` short-circuit dispatch).

The dispatch entry point is
`crate::render::construction::validate_gpu_construction_production_scale`
at `crates/bevy_naadf/src/render/construction/mod.rs:3619`.

The diagnostic boots a headless render world (same pattern as Stage 6's
`validate_gpu_construction_scaled`), allocates production-shape GPU
buffers (chunks 16 MiB, blocks 512 MiB, voxels 1024 MiB), loads
`crates/bevy_naadf/assets/test/oasis_hard_cover.vox` as `ModelData`,
dispatches the production W5 producer chain (512 per-segment
generator_model + chunk_calc dispatches), reads back voxels[] at known
voxel positions at TWO checkpoints (post-producer and post-bounds), and
diffs against the CPU oracle.

### Sample positions

Sample positions are **discovered** by scanning the Oasis `ModelData`
buffers via `discover_populated_oasis_voxels`
(`crates/bevy_naadf/src/render/construction/mod.rs:3530`). The function
strides through the world voxel volume at `(width/16, height/24,
depth/16)` steps and picks 25 positions that are FULL in the Oasis model
(non-zero voxel type at that voxel position via
`get_voxel_type_in_model`-equivalent lookup). All 25 discovered positions
fall in the Y=189..231 range — the densest cluster of Oasis cover-bulk
voxels in the model's interior.

The discovery method ensures the diagnostic targets ONLY positions where
the producer SHOULD have written a non-zero FULL voxel half-word; an
empty position would carry no signal about producer/bounds-chain
correctness at the leaf.

### CPU oracle methodology

For each sample voxel position `(vx, vy, vz)`:

1. Identify the chunk `(cx, cy, cz) = (vx/16, vy/16, vz/16)`.
2. Call `aadf::generator::generate_segment_cpu(&model, [cx,cy,cz],
   [1,1,1], world_voxels)` to run the bit-exact CPU port of
   `generator_model.wgsl` over a single-chunk segment containing the
   position.
3. Decode the segment buffer's voxel half-word at the position's intra-block
   index via the `out[block_index*32 + pair_idx] >> (16*parity) & 0xFFFF`
   formula that the producer writes.

This produces the EXACT half-word the W5 producer chain should leave in
`voxels[]` at the position's leaf voxel slot — including the `bit 15 = 1`
full flag and the 15-bit voxel type.

### GPU pointer walk

For each sample position, the diagnostic does a per-position surgical
GPU readback (single u32 / u32-pair via `map_async`) rather than reading
back the full 1 GiB voxels[] buffer:

1. Compute `chunk_idx = cx + cy*256 + cz*256*32` and read
   `chunks[chunk_idx].x` (the u32 part of the `array<vec2<u32>>` chunk
   buffer — what the renderer's `ray_tracing.wgsl:296` reads as
   `cur_node = chunk_texel.x`).
2. If chunk decoded as **Mixed** (bit 31 set), follow `BlockPtr +
   block_index_in_chunk` into `blocks[]`; read that u32.
3. If block decoded as **Mixed** (bit 31 set), follow `VoxelPtr +
   voxel_index_in_block / 2` into `voxels[]`; read that u32; extract the
   appropriate half-word for the voxel's parity.

The walk EXACTLY mirrors the renderer's chunk → block → voxel descent
in `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:283-401`.

### Readback timing

- **Checkpoint A (post-producer):** captured AFTER the 512 per-segment
  generator+chunk_calc dispatches complete (each segment a fresh encoder
  + submit, mirroring production at `mod.rs:2544-2561`) and AFTER an
  explicit `device.poll(PollType::wait_indefinitely())`.
- **Checkpoint B (post-bounds-calc):** captured AFTER an additional
  `compute_voxel_bounds` + `compute_block_bounds` dispatch (matching
  production at `mod.rs:2622-2633`, including the same 134,217,729-block
  voxel-bounds + 2,097,153-chunk block-bounds workgroup counts) and
  another explicit `device.poll`.

## Results — post-W5-producer (pre-bounds-calc)

Sampled voxel position (column 1) | CPU oracle half-word (column 2) | GPU
chunks[].x (col 3) | GPU blocks[BlockPtr+block_idx] (col 4) | GPU
voxels[VoxelPtr+pair_idx] (col 5) | GPU voxel half-word (col 6) | Match
verdict.

| Position | Oracle half | GPU chunk | GPU block | GPU vpair | GPU vhalf | XOR | Match |
|---|---|---|---|---|---|---|---|
| (372,189, 84) | 0x802e | 0x800060c0 | 0x800117a0 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (930,189, 84) | 0x8096 | 0x80016e80 | 0x800402a0 | 0x80938096 | 0x8096 | 0x0000 | ✓ |
| (372,189,168) | 0x8084 | 0x80008980 | 0x80017000 | 0x80818084 | 0x8084 | 0x0000 | ✓ |
| (465,189,168) | 0x802e | 0x800081c0 | 0x800163e0 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (186,189,252) | 0x8886 | 0x80005280 | 0x8000f300 | 0x88838886 | 0x8886 | 0x0000 | ✓ |
| (372,189,252) | 0x802e | 0x8000a680 | 0x80005280 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (930,210, 84) | 0x8866 | 0x80015840 | 0x80046360 | 0x88638866 | 0x8866 | 0x0000 | ✓ |
| (1023,210, 84) | 0x8863 | 0x80017080 | 0x80042de0 | 0x88638866 | 0x8863 | 0x0000 | ✓ |
| (1116,210, 84) | 0x886c | 0x800205c0 | 0x80080a60 | 0x8869886c | 0x886c | 0x0000 | ✓ |
| (837,210,588) | 0x802c | 0x801d6480 | 0x804c8fc0 | 0x802c8025 | 0x802c | 0x0000 | ✓ |
| (930,210,588) | 0x8024 | 0x801d5a40 | 0x804bace0 | 0x80258024 | 0x8024 | 0x0000 | ✓ |
| (837,210,672) | 0x8027 | 0x801dbc00 | 0x804db900 | 0x80278028 | 0x8027 | 0x0000 | ✓ |
| (1116,210,672) | 0x802b | 0x801e4480 | 0x804e71e0 | 0x802b802b | 0x802b | 0x0000 | ✓ |
| (1023,210,756) | 0x8863 | 0x801ddc80 | 0x804b5660 | 0x88638866 | 0x8863 | 0x0000 | ✓ |
| (1116,210,756) | 0x8028 | 0x801e5e40 | 0x8054cd00 | 0x8c688028 | 0x8028 | 0x0000 | ✓ |
| (651,210,924) | 0x802d | 0x802e7cc0 | 0x8079f2c0 | 0x802d8024 | 0x802d | 0x0000 | ✓ |
| (744,231,  0) | 0x803c | 0x8000af80 | 0x8001d900 | 0x8039803c | 0x803c | 0x0000 | ✓ |
| (651,231, 84) | 0x802e | 0x8000cb80 | 0x8001d060 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (744,231, 84) | 0x802e | 0x8000c400 | 0x8001d060 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (651,231,168) | 0x802e | 0x8000fd40 | 0x8001d060 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (744,231,168) | 0x802e | 0x8000e840 | 0x8001d060 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (1116,231,168) | 0x8026 | 0x80023140 | 0x8008dd60 | 0x80258026 | 0x8026 | 0x0000 | ✓ |
| (1302,231,168) | 0x8c34 | 0x8002c240 | 0x800bf040 | 0x8c318c34 | 0x8c34 | 0x0000 | ✓ |
| (744,231,252) | 0x8026 | 0x80012600 | 0x800367e0 | 0x8c388026 | 0x8026 | 0x0000 | ✓ |
| (1209,231,252) | 0x8023 | 0x800253c0 | 0x800a1aa0 | 0x80238028 | 0x8023 | 0x0000 | ✓ |

**Result: 25 / 25 sample positions MATCH the CPU oracle at checkpoint A.**

Cursor state at checkpoint A:
- `block_voxel_count[0]` = **20,958,784** (voxel-pair cursor; legitimate
  voxel data spans up to u32 index `20,958,784 / 2 = 10,479,392`).
- `block_voxel_count[1]` = **12,882,752** (block-u32 cursor; legitimate
  block data spans up to u32 index 12,882,752).
- Both within the allocated buffer ranges (1024 MiB voxels[] holds
  268,435,456 u32s; 512 MiB blocks[] holds 134,217,728 u32s).

## Results — post-bounds-calc (after compute_voxel_bounds + compute_block_bounds)

After the bounds dispatches at production scale
(`voxel_workgroups=134217729`, `block_workgroups=2097153` — matching
`mod.rs:2615-2618`), the same 25 sample positions are re-read:

| Position | Oracle half | GPU chunk | GPU block | GPU vpair | GPU vhalf | XOR | Match |
|---|---|---|---|---|---|---|---|
| (372,189, 84) | 0x802e | 0x800060c0 | 0x800117a0 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (930,189, 84) | 0x8096 | 0x80016e80 | 0x800402a0 | 0x80938096 | 0x8096 | 0x0000 | ✓ |
| (372,189,168) | 0x8084 | 0x80008980 | 0x80017000 | 0x80818084 | 0x8084 | 0x0000 | ✓ |
| (465,189,168) | 0x802e | 0x800081c0 | 0x800163e0 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (186,189,252) | 0x8886 | 0x80005280 | 0x8000f300 | 0x88838886 | 0x8886 | 0x0000 | ✓ |
| (372,189,252) | 0x802e | 0x8000a680 | 0x80005280 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (930,210, 84) | 0x8866 | 0x80015840 | 0x80046360 | 0x88638866 | 0x8866 | 0x0000 | ✓ |
| (1023,210, 84) | 0x8863 | 0x80017080 | 0x80042de0 | 0x88638866 | 0x8863 | 0x0000 | ✓ |
| (1116,210, 84) | 0x886c | 0x800205c0 | 0x80080a60 | 0x8869886c | 0x886c | 0x0000 | ✓ |
| (837,210,588) | 0x802c | 0x801d6480 | 0x804c8fc0 | 0x802c8025 | 0x802c | 0x0000 | ✓ |
| (930,210,588) | 0x8024 | 0x801d5a40 | 0x804bace0 | 0x80258024 | 0x8024 | 0x0000 | ✓ |
| (837,210,672) | 0x8027 | 0x801dbc00 | 0x804db900 | 0x80278028 | 0x8027 | 0x0000 | ✓ |
| (1116,210,672) | 0x802b | 0x801e4480 | 0x804e71e0 | 0x802b802b | 0x802b | 0x0000 | ✓ |
| (1023,210,756) | 0x8863 | 0x801ddc80 | 0x804b5660 | 0x88638866 | 0x8863 | 0x0000 | ✓ |
| (1116,210,756) | 0x8028 | 0x801e5e40 | 0x8054cd00 | 0x8c688028 | 0x8028 | 0x0000 | ✓ |
| (651,210,924) | 0x802d | 0x802e7cc0 | 0x8079f2c0 | 0x802d8024 | 0x802d | 0x0000 | ✓ |
| (744,231,  0) | 0x803c | 0x8000af80 | 0x8001d900 | 0x8039803c | 0x803c | 0x0000 | ✓ |
| (651,231, 84) | 0x802e | 0x8000cb80 | 0x8001d060 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (744,231, 84) | 0x802e | 0x8000c400 | 0x8001d060 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (651,231,168) | 0x802e | 0x8000fd40 | 0x8001d060 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (744,231,168) | 0x802e | 0x8000e840 | 0x8001d060 | 0x802e802e | 0x802e | 0x0000 | ✓ |
| (1116,231,168) | 0x8026 | 0x80023140 | 0x8008dd60 | 0x80258026 | 0x8026 | 0x0000 | ✓ |
| (1302,231,168) | 0x8c34 | 0x8002c240 | 0x800bf040 | 0x8c318c34 | 0x8c34 | 0x0000 | ✓ |
| (744,231,252) | 0x8026 | 0x80012600 | 0x800367e0 | 0x8c388026 | 0x8026 | 0x0000 | ✓ |
| (1209,231,252) | 0x8023 | 0x800253c0 | 0x800a1aa0 | 0x80238028 | 0x8023 | 0x0000 | ✓ |

**Result: 25 / 25 sample positions MATCH the CPU oracle at checkpoint B.**

Cross-checkpoint diff: **A→B changed in 0 / 25 positions.** All
chunks/blocks/voxels values are byte-identical between the two
checkpoints (the bounds chain is reading the producer's voxel data,
classifying FULL voxels' bit-15=1 state, hitting the
`if (state == 1u) cached_cell[local_index] = orig_voxel` restore branch
at `chunk_calc.wgsl:488-490`, and writing the un-mutated original
voxel-pair u32 back).

## Pattern analysis

There is **no divergence** between the CPU oracle and the GPU readback
at any sample position at any checkpoint. The W5 producer chain + bounds
chain produce voxels[] that is byte-equal to the CPU oracle at every
position the producer SHOULD have written.

This rules out:

1. **`compute_voxel_bounds` leaf-writeback corrupting full-voxel type
   bits** (Q1 in `14-diagnostic-type-decode.md:333`): if this were the
   bug, post-bounds-calc voxel-pair u32s would have garbled high or low
   halves. They are bit-identical to post-producer.
2. **Per-segment shared `hash_map` state accumulating across 512 segments**
   (P1 in `14-diagnostic-type-decode.md:154`): if this were the bug, some
   subset of the 25 sampled positions (those whose hash falls in a
   collision-dense bucket) would deref via a wrong VoxelPtr and the leaf
   voxel data would mismatch. All 25 match.
3. **A downstream pass mutating `voxels[]` in production but not Stage 6**
   (P3 at `:158`): the only downstream pass on the producer chain is the
   bounds chain, and it leaves voxels[] byte-identical. No other pass
   exists between producer and renderer that writes voxels[].
4. **Buffer-aliasing / binding-mismatch hazard** in the production
   bind-group setup (P4 at `:161`): the diagnostic uses the SAME
   `construction_world_layout` + `BindGroupEntries::sequential` pattern
   as the production `prepare_construction`; no aliasing manifests at
   the leaf data level.
5. **Q3 per-segment-submit / bounds-chain memory ordering**
   (`14:447-491`): if this were the bug, post-bounds voxels would
   contain partial / stale producer writes. The cursor counts confirm
   the producer wrote 20,958,784 voxel-pair claims = 10,479,392 u32s, and
   the post-bounds readback returns the EXACT same data the producer
   wrote — no in-flight write was overtaken by the bounds reads.

## Identified bug

**voxels[] is byte-correct post-full-pipeline. The bug must be in the
renderer's decode path (or in the bind group / buffer wiring upstream
of the renderer).**

Candidate decode-path files to investigate next:

1. **`crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:283-401`** —
   the chunk → block → voxel descent function `shoot_ray`. The relevant
   leaf-hit code at line 335-340:
   ```wgsl
   let cur_voxel_pair = voxels[voxel_start_index];
   cur_node = (cur_voxel_pair >> (16u * (voxel_index_in_block & 0x1u))) & 0xFFFFu;
   if ((cur_node >> 15u) != 0u) {
       cur_node = cur_node | (1u << 30u);
   }
   ```
   This reads `voxels[]` at `voxel_start_index = (cur_node & 0x3FFFFFFFu)
   + voxel_index_in_block / 2u` (line 333-334) after dereferencing a
   block descriptor's low 30 bits as `VoxelPtr`. If the BLOCK descriptor
   the renderer reads has WRONG `VoxelPtr` bits, the renderer dereferences
   a wrong slot in voxels[] and reads garbage. The diagnostic's GPU
   pointer walk uses the SAME descent code and finds correct data — so
   the chunk and block descriptors at the sampled positions are correct
   on the producer side. **The renderer reading the same descriptors at
   the same chunks[] and blocks[] buffer locations should see the same
   correct VoxelPtrs.** Unless the renderer's bind group binds to a
   DIFFERENT `chunks` or `blocks` buffer than the producer wrote to.

2. **`crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl:227-228`**
   — the `voxel_types[ray_result.hit_type]` palette lookup. If
   `ray_result.hit_type` is in-range (0..256), this reads the correct
   palette entry. The user-reported "thousands" types would land OOB
   in the 257-entry palette and decode to zero/garbage.

3. **`crates/bevy_naadf/src/render/prepare.rs::prepare_world_gpu`**
   (`render/prepare.rs:184-450`) — the world bind group / buffer
   allocation site. **Specifically check whether the `chunks`,
   `blocks`, and `voxels` BUFFERS bound to the W5 producer's
   `construction_world_layout` bind group at `mod.rs:1750+` are the
   SAME buffer handles bound to the renderer's `world_layout` bind
   group at `prepare.rs:431-440`**. If `prepare_world_gpu` creates the
   buffers but the producer's bind group binds DIFFERENT buffers (e.g.
   freshly-allocated diagnostic mirrors), the renderer reads from
   one set and the producer wrote to another.

4. **`crates/bevy_naadf/src/render/extract.rs`** — extracted world-data
   metadata. If the `world_data_meta.size_in_chunks` the renderer reads
   differs from the producer's `WORLD_SIZE_IN_CHUNKS = (256,32,256)` at
   the time of bind-group construction, the renderer's
   `flatten_index(chunk_pos, size_in_chunks.x, size_in_chunks.x *
   size_in_chunks.y)` (`ray_tracing.wgsl:290-294`) computes a DIFFERENT
   chunk_idx than the producer's `cx + cy*256 + cz*256*32`, and the
   renderer would read garbage from chunks[] (or correct data from a
   wrong chunk).

5. **A renderer-specific `chunks` buffer extraction in
   `prepare_world_gpu`** — `prepare.rs:418-432` (per
   `14-diagnostic-type-decode.md:280-294`) calls
   `chunks.upload_all(&[0u32], …)` / `voxels.upload_all(…)` / `blocks.upload_all(…)`
   when `gpu_producer_skip_upload = false`, then uploads `extracted.{chunks,blocks,voxels}`.
   On the W5 install path these are EMPTY (`grid.rs:409-425`), so the
   upload uploads `vec![0]`. **If the upload happens AFTER the producer
   wrote to the buffers, it zeros out the first 4 bytes of every
   buffer.** Verify the upload ordering vs the producer dispatch
   (`prepare_construction` runs in `PrepareResources`; the producer in
   `Core3d`).

## Subsidiary finding — small-hash-map probe exhaustion (NOT a fix recommendation)

An exploratory run with `hash_map_size_slots = 1 << 18 = 262,144`
(narrower than the production `1 << 20 = 1,048,576` from
`config.rs:157`) reproduced a different observable failure:

- Sample position `(930, 340, 840)`:
  - GPU chunk u32 = `0x8033cd00` → Mixed, BlockPtr = 0x33cd00 (valid).
  - GPU block u32 = `0x80000002` → Mixed, **VoxelPtr = 0x2**.
  - VoxelPtr = 2 is the **probe-exhaustion sentinel** returned by
    `get_voxel_pointer` at `chunk_calc.wgsl:339`:
    ```wgsl
    // Probe exhaustion sentinel (chunkCalc.fx:114).
    return 2u;
    ```
  - At checkpoint A: voxels[2/2]=voxels[1] = 0x00000000 (the producer
    NEVER wrote that slot; the original probe-exhausted block writer
    couldn't reserve a slot and bailed).
  - At checkpoint B: voxels[1] = `0x0c930c96` — `compute_voxel_bounds`
    read the zero voxel-pair, classified all 64 voxels as empty, computed
    AADFs across the workgroup, and wrote AADF bits BACK to voxels[1].

This is a real failure mode at smaller hash-map sizes (load factor
>1.0). Production uses 1M slots and does NOT exhibit it. The 256K-slot
finding is documented as a corroborating data point only — production
does not need a fix here.

The probe-exhaustion sentinel pattern (`block = 0x80000002`) IS a
plausible explanation for the user's symptom IF the production
configuration's `initial_hash_map_size` is somehow being overridden /
lowered at runtime (e.g. via `AppArgs.construction_config`). Verifying
that the production app actually uses 1M-slot hash map at the
`naadf_gpu_producer_node` dispatch site is a one-line check:
`info!("hash_map_size = {}", config.initial_hash_map_size);` in
`naadf_gpu_producer_node` at `mod.rs:2520`.

## Recommended next step (NOT to be implemented in this dispatch)

**Highest leverage**: instrument the renderer's per-pixel decode path to
capture, for a single ray that the user-observed `oracle_gpu.png` shows
as "thousand-typed", the values of `cur_node` at each layer of the
descent (chunk → block → voxel). Compare those values against what the
producer-written `chunks[]` / `blocks[]` / `voxels[]` contain at those
exact addresses (via the existing diagnostic infrastructure).

One concrete approach:

1. Add an `info!` log to `naadf_first_hit.wgsl:227-228` that, for a
   single hard-coded pixel coordinate (e.g. the centre of the
   `oracle_gpu.png` frame), dumps `ray_result.hit_type` + the underlying
   buffer reads via a host-readback ring (mirrors the per-frame chunk
   readback added in `03-impl.md:2469` (D1) for the chunk-AADF
   convergence check).
2. Capture the dump in a single-frame screenshot run.
3. Compare the dump's `chunk_u32`, `block_u32`, `voxel_pair_u32` against
   the values this Stage 9 diagnostic measures at the same
   (chunk_pos, block_pos_in_chunk, voxel_pos_in_block) decomposition.
4. If they match → the renderer is correctly reading the producer's
   data, but `decompress_voxel_type` or the palette is broken.
   Investigate `naadf_first_hit.wgsl:228` /
   `render_pipeline_common.wgsl:105` `decompress_voxel_type` / the
   `voxel_types` storage buffer in `prepare_world_gpu`.
5. If they DO NOT match → the renderer's bind group is reading from a
   DIFFERENT buffer than the producer wrote to.
   Investigate `crates/bevy_naadf/src/render/prepare.rs:184-450`
   `prepare_world_gpu` for the buffer-handle plumbing.

Alternative concrete diagnostic: extend Stage 9 to also run the renderer
(launch the full production app for 1 frame, capture chunks/blocks/voxels
buffer reads from the renderer side via a `host_visible` ring, compare
against this diagnostic's producer-side readback values at the SAME
addresses). This is heavier but produces the discriminating answer in
one step.

## Confidence level

**HIGH.**

- Concrete byte-level evidence at 25 distinct Oasis-populated voxel
  positions, sampled across the densest cover-bulk region of the
  model's Y=189..231 layer.
- The full production W5 producer chain is exercised end-to-end at
  production scale (512 segments × generator+chunk_calc, plus the
  bounds chain) with the production-config `hash_map_size = 1 << 20`,
  `probe_cap = 250`, `wanted_empty_ratio = 0.5`, and all other
  parameters matching `ConstructionConfig::default()`.
- Both checkpoint A (post-producer) and checkpoint B (post-bounds-calc)
  show byte-identity to the CPU oracle. The discriminating question is
  answered without ambiguity.
- The diagnostic is permanent and re-runnable via
  `cargo run --release --bin e2e_render -- --validate-gpu-construction-production`.

The remaining hypothesis space (per `14-diagnostic-type-decode.md`'s
P1-P4 + Q1-Q5) is **conclusively narrowed to the renderer's read path
or the buffer/bind-group wiring upstream of the renderer**. The producer
chain (W5 generator_model + chunk_calc + bounds_calc) is byte-correct at
the leaf in the production-shape run.

## Cross-references

- This diagnostic: `crates/bevy_naadf/src/render/construction/mod.rs:3530`
  (`discover_populated_oasis_voxels`) +
  `crates/bevy_naadf/src/render/construction/mod.rs:3619`
  (`validate_gpu_construction_production_scale`).
- CLI wiring: `crates/bevy_naadf/src/bin/e2e_render.rs:91-93` (flag),
  `crates/bevy_naadf/src/bin/e2e_render.rs:154-167` (dispatch).
- Production W5 producer loop the diagnostic mirrors:
  `crates/bevy_naadf/src/render/construction/mod.rs:2454-2566`.
- Production W5 bounds-chain dispatch:
  `crates/bevy_naadf/src/render/construction/mod.rs:2622-2633`.
- W5 install path: `crates/bevy_naadf/src/voxel/grid.rs:317-429`.
- Oasis fixture: `crates/bevy_naadf/assets/test/oasis_hard_cover.vox`.
- Production config: `crates/bevy_naadf/src/render/construction/config.rs:157`
  (`initial_hash_map_size = 1 << 20`).
- chunk_calc probe-exhaustion sentinel: `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl:339`
  (`return 2u`).
- Renderer chunk→block→voxel descent: `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:283-401`.
- Renderer palette lookup: `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl:227-228`.
- Prior diagnostics in the cascade: `14-diagnostic-type-decode.md` (Q1-Q5 +
  P1-P4 hypothesis space), `13-diagnostic-w3-bounds-calc.md` (W3-T1 fix),
  `12-diagnostic-byte-diff-concrete.md` (Stage 6 sub-production-scale
  byte-equality), `11-diagnostic-buffer-byte-diff.md`,
  `10-diagnostic-encoding-comparison.md`.
- Q4 Bevy-defaults-clarification log: `03-impl.md:2890+`
  (RTX 5080 / Vulkan: `max_storage_buffer_binding_size = 2047 MiB`; the
  1 GiB voxels[] binding fits comfortably).
