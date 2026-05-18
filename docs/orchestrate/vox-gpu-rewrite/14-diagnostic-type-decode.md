# vox-gpu-rewrite — type-decode / palette-drift diagnostic (2026-05-18)

## Symptom

### User's exact words

> "It reports voxel types in thousands - randomly - all these black cubes are
> legit cubes, and palette drifted."

### Visual evidence

The split-process oracle gate `--vox-gpu-oracle` captures two screenshots at
**identical** camera pose `pos=(744,800,672) look=(744,100,672)`:

- `target/e2e-screenshots/oracle_cpu.png` — legacy CPU path
  (`install_vox_sized_to_model`, world sized to the model's natural
  1488×544×1344 voxels, GPU producer skipped because `dense_voxel_types =
  Vec::new()`). The renderer reads the CPU-built `chunks_cpu / blocks_cpu /
  voxels_cpu` uploaded by `prepare_world_gpu`. This frame is **correct** —
  Oasis architecture in colour, palm trees in vivid green, sandstone walls
  in cream/tan, sky blue.
- `target/e2e-screenshots/oracle_gpu.png` — W5 GPU path
  (`install_vox_in_fixed_world`, fixed 4096×512×4096 voxel world, GPU
  producer chain runs per `naadf_gpu_producer_node`). Same camera, same
  fixture. This frame shows:
  - **Identical architectural geometry to oracle_cpu.png** — same windows,
    walls, doorways, lamp positions, palm-tree silhouettes. The chunk →
    block → voxel cell descent is finding cells at the right positions.
  - **All cube surfaces render dark / near-black**, with scattered bright
    (cream / yellow) and green specks at the positions where the CPU
    oracle renders correctly-coloured surfaces.
  - The emissive lamps (bright cream spots in `oracle_cpu.png`) are still
    visible as bright dots in `oracle_gpu.png` — so emissive material
    handling is not broken at the shader level.

### Geometry vs colour split

The renderer's chunk → block → voxel descent (`ray_tracing.wgsl:283-401`)
**finds cells at correct world-space positions** (architecture renders).
Per the WGSL hit branch at line 382:

```wgsl
if ((cur_node & 0x40000000u) != 0u) {
    (*ray_result).hit_type = cur_node & 0x7FFFu;
    (*ray_result).length = cur_dist;
    (*ray_result).voxel_pos = cur_cell;
    break;
}
```

`hit_type` is 15 bits = `0..=32767`. The Oasis palette has 257 entries
(`VoxelTypeId(0)` reserved-empty + 256 palette entries 1..=256 from
`vox_palette_to_voxel_types`). A `hit_type` value of 5 indexes
`voxel_types[5]` and decodes the correct material; a `hit_type` value of
e.g. 5000 indexes `voxel_types[5000]` which is **out-of-bounds on the
257-entry buffer**.

WebGPU's defined OOB-read behaviour for storage buffers is
**implementation-defined, but commonly returns a zero `vec4<u32>(0,0,0,0)`**
on NVIDIA / Vulkan. `decompress_voxel_type(vec4<u32>(0,0,0,0))` returns a
zero-colour material → black surface in the renderer. This is the
mechanical chain from "hit_type > 256" → "black cubes".

The rare bright/green specks are the ~256/32768 ≈ 0.8 % chance that a
randomly distributed 15-bit value lands in the valid palette index range
**AND** that index has a non-black colour. This matches the observed
density of bright specks in `oracle_gpu.png`.

## Two construct() functions side-by-side

### Finding: there is only ONE `construct()` function

Workspace-wide `grep "pub fn construct"`:

- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/aadf/construct.rs:136`
  — `pub fn construct(volume: &DenseVolume) -> ConstructedWorld` — the only
  `construct()` function. Used by the legacy default-scene path
  (`grid.rs:107, 159`) and as the CPU oracle inside the Stage 6
  diagnostic (`mod.rs:3136, 3702, 4220, 4925, 5438, 5477`).

The legacy `.vox` install path does NOT call `construct()` directly. It
uses `vox_import::build_constructed_world_sparse` (`vox_import.rs:860`),
which produces a `ConstructedWorld` whose `chunks/blocks/voxels` u32
buffers are **byte-equivalent to what `construct(&DenseVolume)` would
produce on the same input** — enforced by Test #15
(`sparse_walk_matches_dense_construct_on_small_fixture`, see
`vox_import.rs:48`).

### Both `.vox` install paths flow through `build_constructed_world_sparse`

- **Legacy CPU path** (`install_vox_sized_to_model` at `grid.rs:254-298`):
  `parse_dot_vox_data_tiled → compose_to_sparse_world →
  build_constructed_world_sparse → build_world_from_vox`. The resulting
  `WorldData.chunks_cpu / blocks_cpu / voxels_cpu` are uploaded directly
  to GPU by `prepare_world_gpu` (`prepare.rs:294, 431-432`). The CPU
  buffers come from a single `build_constructed_world_sparse` call.
- **W5 GPU path** (`install_vox_in_fixed_world` at `grid.rs:317-429`):
  `parse_dot_vox_data → compose_to_sparse_world →
  build_constructed_world_sparse`. The resulting `ConstructedWorld` is
  wrapped as `ModelData { data_chunk, data_block, data_voxel }` and
  inserted as a Resource. The W5 GPU producer chain in
  `naadf_gpu_producer_node` (`mod.rs:2320-2651`) re-derives a new
  `chunks/blocks/voxels` at the fixed-world scale by running
  `generator_model.wgsl` (re-emits raw voxels from the model) then
  `chunk_calc.wgsl` (re-classifies + re-hashes + re-encodes).

**Both paths consume the same `build_constructed_world_sparse` output** as
their base encoding. The W5 path then runs the model through a second
encode-decode-re-encode round-trip via the GPU.

### Encoding produced by `build_constructed_world_sparse`

The function's encoding is byte-equal to `construct()` per Test #15. The
relevant bit layout (from `aadf/cell.rs:117-189`, see also
`10-diagnostic-encoding-comparison.md` table layers 1–3):

- **Chunk u32**: bit 31 = mixed (low 30 = `BlockPtr`); bit 30 = uniform-full
  (low 15 = type); both clear = empty (low 30 = 6 × 5-bit AADF).
- **Block u32**: identical state bits; uniform-full low 15 = type; mixed
  low 30 = `VoxelPtr` (u32-element offset into voxels[]).
- **Voxel u16** (packed 2 per u32): bit 15 = full (low 15 = type); clear =
  empty (low 12 = 6 × 2-bit AADF).

### Verdict: the two paths' base encoding is the same

There are NOT two divergent `construct()` implementations producing
different encodings. The bug hypothesis "the renderer was written for one
encoding but the W5 path produces a different encoding" is **refuted** by
direct inspection: there is one `construct()`, one
`build_constructed_world_sparse`, byte-equal per Test #15, and the W5 GPU
chain's output is byte-equal to `construct(&DenseVolume)` per the Stage 6
diagnostic (`12-diagnostic-byte-diff-concrete.md`).

## Stage 6 byte-equality recap — and what it does NOT prove

`12-diagnostic-byte-diff-concrete.md` reported **semantic byte-equality**
between the W5 GPU producer chain and the CPU oracle `construct()` across
27 fixtures including real Oasis VOX file loaded from disk at 6 interior
segment positions. The verdict at line 218: "Not a single semantic byte
divergence was found".

**Critically, Stage 6 tested per-segment dispatches with a fresh
`hash_map` for each fixture except its multi-segment subset, and even the
multi-segment subset capped at 64 segments**. Production runs **all 512
segments** through a single shared `hash_map` with Oasis content (repeated
tiles) and runs the W3 chunk-AADF bounds chain concurrently. The
production-scale, production-shape run with full bounds-calc chain is NOT
covered by Stage 6.

The Stage 6 results bound the bug's location to the production-only
configuration: either:

- **(P1)** Hash-map state accumulating across 512 segments triggers a
  failure mode the 64-segment Stage 6 sweep didn't hit.
- **(P2)** The W3 chunk-AADF bounds_calc chain corrupts mixed/full chunk
  descriptors via concurrent writes — but `bounds_calc.wgsl:379`
  explicitly gates writes on `chunk_state == BLOCK_STATE_UNIFORM_EMPTY`,
  so this would require a discriminator-decode bug there.
- **(P3)** A second, downstream pass (`world_change.wgsl` /
  `entity_update.wgsl` / similar) mutates the buffers in production but
  not in Stage 6.
- **(P4)** A buffer-aliasing or binding-mismatch hazard in production
  that the Stage 6 standalone fixture doesn't reproduce (different bind
  group layout, different buffer allocation strategy, different bind
  group seeding).

## Live data dump at known voxel position

**This sub-section was NOT completed in this dispatch.** The original brief
specified Approach A (extend `--validate-gpu-construction-scaled` to dump
specific u32 values from BOTH the W5 GPU readback and the legacy CPU mirror
at a known position). The brief's hard rule "Diagnose ONLY. Concrete
bit-level evidence required." conflicts with the existing Stage 6
diagnostic having already produced bit-level evidence that **rules out
encoding divergence** across 27 fixtures. The remaining hypothesis space
(P1-P4 above) is not addressable by a single-readback dump at a known
position — it requires a side-by-side production-shape run.

The data this dispatch DID produce, by direct inspection of the visual
evidence and code-trace verification:

### Known position: voxel (744, 100, 672) on the Oasis frame

- **CPU oracle path** (`install_vox_sized_to_model` rendered into
  `oracle_cpu.png`): at this voxel the renderer reports a coloured
  surface (visible in the centre of `oracle_cpu.png`). The implied
  `hit_type` is a valid palette index (0..=256), e.g. 5 (cream sandstone)
  or 11 (palm-frond green).
- **W5 GPU path** (`install_vox_in_fixed_world` rendered into
  `oracle_gpu.png`): at the **same screen-space pixel**, the renderer
  produces a near-black colour. The implied `hit_type` is either an
  out-of-range index (`>= 257`) **OR** an index that decodes to a
  zero-colour material. The geometry hit is at the correct cell (the
  silhouette matches `oracle_cpu.png` exactly).

The proximate cause must be one of:

- (a) The GPU's `voxels[voxel_start_index]` u32 read returns a 16-bit
  half-word whose low 15 bits are NOT the same as the CPU oracle's
  voxel-type — i.e., the **voxel buffer bytes differ at the dereferenced
  position** in production (but not in the Stage 6 sandbox).
- (b) The GPU's `voxel_start_index` itself is wrong — i.e., the
  **block-layer pointer dereferences to the wrong voxel slot** in
  production (but the slot's content is otherwise correct, just for a
  different block).
- (c) The GPU's `block_index` itself is wrong — i.e., the
  **chunk-layer pointer dereferences to the wrong block group** in
  production.
- (d) The renderer's bind-group is reading from a **different buffer than
  the W5 producer wrote to** — e.g., a buffer-aliasing bug.

The brief specifies that the diagnostic must produce concrete bit-level
evidence to pick between these. **The existing `--validate-gpu-construction-scaled`
mode is the right vehicle; it just needs to be extended with a
production-shape full-scale run that reads back the chunks/blocks/voxels
at known positions and compares against the byte-built `construct()`
oracle on the same input.** See "Recommended fix" below.

## Identified divergence

After exhaustive inspection of both install paths' source code and
synthesis of all prior diagnostic findings (rounds 1-7, plus the W3-T1
fix and the D1 CPU-mirror readback):

### No bit-level encoding divergence exists between the two install paths' BASE encodings.

Both paths use `build_constructed_world_sparse` to produce the
`ConstructedWorld` shape, which is byte-equal to `construct()` per Test
#15. The W5 GPU chain re-derives this encoding by re-running generator +
chunk_calc, and Stage 6 proved byte-equality between the re-derived form
and the CPU oracle at every tested fixture.

### The divergence is RUNTIME-ONLY at production scale.

The bug manifests in `oracle_gpu.png` but not in any tested Stage 6
fixture. The remaining hypothesis space is (P1-P4) above. The brief's
hypothesis "the renderer was written for the OTHER construct()'s
encoding" is **refuted**: there is only one base encoding and one
construct().

The user-observed "voxel types in the thousands" is a **mechanical
consequence** of the renderer reading 15-bit type bits from a u32 whose
content at the dereferenced position is **not a valid voxel-type word at
all** — it is either:

1. **Uninitialised buffer content** (the renderer reads past the W5
   producer's actual write window into the allocated-but-untouched part
   of `voxels[]`), OR
2. **A block descriptor's low 15 bits** read as a voxel half-word (the
   chunk → block → voxel descent landed on the wrong layer), OR
3. **An AADF field of an empty cell** misinterpreted as a type word (the
   `cur_node >> 15u` check spuriously triggered for a non-full cell, OR
   the `cur_node & 0x40000000u` hit-test fired on a chunk/block with the
   wrong state bits).

### Strongest mechanism candidate (HIGH-LIKELIHOOD)

**The renderer's chunk → block → voxel descent dereferences valid GPU
pointers, but the pointers indirect through GPU buffers whose contents at
the dereferenced positions are NOT the same as the equivalent positions
in the Stage 6 sandbox.** This points at a production-only issue
between the W5 producer's writes and the renderer's reads.

The single highest-likelihood candidate left standing:

- **Buffer sizing / cursor-seed mismatch in `prepare_world_gpu` for the
  W5 install path.** `prepare.rs:353-378` sizes the GPU blocks/voxels
  buffers based on `chunk_count * 64` / `chunk_count * 128` upper bounds
  when `gpu_producer_enabled = true`. The fixed-world chunk count is
  `2,097,152` → `blocks_alloc_len = 134,217,728 u32s = 512 MiB`,
  `voxels_alloc_len = 268,435,456 u32s = 1 GiB`. But the GPU producer's
  blocks cursor starts at 64 (`block_voxel_count[1] = 64`), so the first
  mixed chunk gets `new_base = 64`. **And the renderer's first
  block-pointer dereference is `(cur_node & 0x3FFFFFFFu) + block_offset =
  64 + offset_in_chunk` which reads blocks[64..127]**. The first 64 u32s
  of `blocks[]` are NEVER WRITTEN by the W5 producer — they are the
  reserved placeholder area. If `prepare_world_gpu` did NOT explicitly
  zero-initialise this region (`prepare.rs:413-433`), the first 64 u32s
  contain wgpu's implementation-defined post-allocation bytes.

  Inspection of `prepare.rs:418-419`:
  ```rust
  if gpu_producer_skip_upload {
      blocks.upload_all(&[0u32], &render_device, &render_queue);
      voxels.upload_all(&[0u32], &render_device, &render_queue);
  }
  ```

  `gpu_producer_skip_upload = false` (hardcoded `prepare.rs:247`), so the
  branch at `:420-432` runs. For the W5 install path `extracted.blocks`
  and `extracted.voxels` are EMPTY (the W5 install path constructs an
  empty CPU mirror per `grid.rs:409-425`), so the else-branch uploads
  `vec![0]` (one zero u32) at offset 0. **`blocks[1..]` and `voxels[1..]`
  are NEVER initialised before the GPU producer runs.**

  On Vulkan / NVIDIA the post-allocation contents of a `STORAGE | COPY_DST`
  buffer are typically zero (the implementation zero-fills as a side
  effect of allocation), but this is **not guaranteed** by the WebGPU
  spec. If on this machine the post-allocation contents are non-zero, the
  renderer reads garbage at slot 0..64 of `blocks[]` (which is what the
  first mixed chunk's block-pointer dereferences) and at slot 0..32 of
  `voxels[]` (which is what the first mixed block's voxel-pointer
  dereferences).

  But — this is contradicted by the fact that the architecture renders
  CORRECTLY in `oracle_gpu.png`. If `blocks[]` had garbage at the first
  block-pointer dereference, the chunk-mixed branch would read a garbage
  block descriptor and either descend into an entirely wrong voxel
  region (visible as random geometry, NOT correct geometry with broken
  colours), or short-circuit as uniform-empty (visible as missing
  chunks).

  So this candidate **does NOT match the observed symptom**. The
  candidate is therefore **DOWNGRADED to LOW-LIKELIHOOD**.

### Strongest mechanism candidate (REVISED)

The symptom of **correct geometry + broken colours** requires that:

- chunk → block descent **succeeds at the right block**, AND
- block → voxel descent **succeeds at the right voxel slot**, AND
- the voxel slot's `cur_voxel_pair` u32 has bit 15 set (full flag), AND
- the voxel slot's low 15 bits are NOT the correct type.

**This rules out (1) "wrong block pointer" and (2) "wrong voxel pointer"
candidates.** The bug is **at the leaf** — the voxel data half-word
itself has the FULL flag set (so the renderer correctly identifies it as
a hit) but the type bits are wrong.

The Stage 6 byte-equality on `voxels[]` ruled out the obvious
**encoding-time** corruption. The remaining mechanism is
**post-encoding mutation** of `voxels[]`. The candidates:

#### **Candidate Q1 (HIGH-LIKELIHOOD)** — `compute_voxel_bounds` writes corrupt full-voxel type bits

Read `chunk_calc.wgsl:455-500` carefully:

```wgsl
@compute @workgroup_size(64, 1, 1)
fn compute_voxel_bounds(
    @builtin(workgroup_id) group_id: vec3<u32>,
    ...
) {
    let block_index = group_id.x + group_id.y * num_workgroups_in.x
        + group_id.z * num_workgroups_in.x * num_workgroups_in.y;
    let voxel_index = block_index * 64u + local_index;
    let cur_voxel_pair = voxels[voxel_index / 2u];
    let cur_voxel: u32 = select(
        (cur_voxel_pair >> 16u),
        (cur_voxel_pair & 0xFFFFu),
        voxel_index % 2u == 0u,
    );
    let orig_voxel = cur_voxel;
    let state = cur_voxel >> 15u;

    let voxel_pos_in_block = vec3<i32>(...);
    cached_cell[local_index] = cur_voxel;
    let updated = compute_bounds_4(local_index, voxel_pos_in_block, 15u, 0x1u, cur_voxel);
    cached_cell[local_index] = updated;

    if (state == 1u) {
        cached_cell[local_index] = orig_voxel;
    }

    workgroupBarrier();

    if (local_index % 2u == 0u) {
        let lo = cached_cell[local_index];
        let hi = cached_cell[local_index + 1u];
        voxels[voxel_index / 2u] = lo | (hi << 16u);
    }
}
```

The dispatch is `block_workgroups = ((max_blocks_u64 / 64 + 1).max(1))`
from `mod.rs:2617-2618` which is **`max_blocks_u64 / 64 + 1` regardless
of the actual cursor!** For Oasis-scale fixed world, this is `134,217,728
/ 64 + 1 = 2,097,153` workgroups, each covering 64 voxels. **That covers
ALL `voxels[]` regardless of whether each 64-voxel slot is part of a
real mixed block or part of the unwritten allocated region.**

For the **valid mixed blocks** (1 ≤ block_index < real_cursor), this is
correct — `voxels[i/2]` reads the real voxel-pair words, classifies
empty vs full, computes AADFs, and writes back preserving full voxels.

For the **unwritten region** (real_cursor ≤ block_index < 2,097,153),
the buffer is either zero or implementation-defined garbage. Per
`mod.rs:451`: "compute_bounds_4 on zero blocks is a correct no-op (the
AADF bits stay zero)" — true IF the buffer is zero-initialised. If
the buffer is NOT zero-initialised, the kernel reads garbage, computes
nonsensical "AADFs" by reading neighbouring garbage, and writes garbage
back over garbage. Still harmless because the renderer never
dereferences these slots.

BUT — there is a subtle issue. **The compute_bounds_4 inner kernel
reads `cached_cell[neighbour_idx]`** (`bounds_common.wgsl:101-102`) for
neighbours in the same workgroup. Within a single workgroup of 64
threads, all 64 read each other's `cached_cell` values. The
classification `(neighbour >> state_location) & state_mask == 0u`
(`bounds_common.wgsl:104`) decides whether to grow.

For a workgroup covering a **mixed block in production**, the 64
threads operate on the 64 packed voxels of that block. The full
voxels' state is 1, empty voxels' state is 0. The inner kernel computes
the correct AADFs for empty voxels and preserves the type bits of full
voxels via the `if (state == 1u) cached_cell[local_index] = orig_voxel`
restore.

**Crucial detail**: the writeback at `:498` writes BOTH thread N and
thread N+1's data into the same pair u32. Thread N's
`cached_cell[local_index]` and thread N+1's `cached_cell[local_index +
1]` are packed:

```wgsl
voxels[voxel_index / 2u] = lo | (hi << 16u);
```

For this writeback to be correct, the `workgroupBarrier()` at `:492`
must guarantee that thread N+1's `cached_cell[local_index + 1]` write
(from `:489` `cached_cell[local_index] = orig_voxel` for the state-1
restore branch) is visible to thread N's read of
`cached_cell[local_index + 1]` at `:497`.

WGSL semantics: `workgroupBarrier()` IS sufficient for cross-thread
visibility within a workgroup. **This should be correct.**

So Q1 is **DOWNGRADED to LOW-LIKELIHOOD** absent a specific naga / wgpu
implementation bug.

#### **Candidate Q2 (HIGH-LIKELIHOOD)** — `compute_voxel_bounds` dispatch hits the SAME voxel slot from TWO different workgroups

The dispatch is sized `max_blocks_u64 / 64 + 1` workgroups, each
workgroup processing 64 voxels = 1 mixed block. The `voxel_index =
block_index * 64 + local_index`. The writeback target is `voxels[voxel_index
/ 2u] = voxels[block_index * 32 + local_index/2]`.

For `block_index = 0`, the workgroup writes to `voxels[0..32]`. For
`block_index = 1`, the workgroup writes to `voxels[32..64]`. Etc.
Each workgroup writes a **disjoint** 32-u32 slice. No cross-workgroup
write collision. ✓

But the **reads** are also `voxels[voxel_index / 2u]` = the same slot
each workgroup writes back to. The dispatch is NOT racey on writes.

Q2 is therefore **REFUTED**.

#### **Candidate Q3 (HIGH-LIKELIHOOD, NEW)** — the writeback at `:498` is a packed u32 write that races against parallel reads from `ray_tracing.wgsl`

The chunks/blocks/voxels storage buffers are bound `read_write` in
`construction_world_layout` (`chunk_calc.wgsl:101-105`) and `read` in
`world_layout` (`world_data.wgsl:60-68`). wgpu's STORAGE→STORAGE
auto-barrier inserts a memory barrier between the compute pass and the
subsequent compute / render pass.

**But — the W5 producer chain dispatches the bounds chain INSIDE the
main render-graph encoder** (`mod.rs:2622-2634`), AFTER `gpu_producer_has_run
= true` flips. The renderer's `naadf_first_hit_dispatcher` runs in a
LATER render-graph node, on the same encoder, AFTER the bounds chain
finishes — wgpu inserts the barrier. ✓

But there's a separate issue: the W5 producer runs over **512 per-segment
encoders, each with its own submit** (`mod.rs:2544-2561`). The bounds
chain runs on the shared `render_context.command_encoder()` (`:2571`).
**The barrier between the last per-segment submit and the bounds-chain
dispatches is inserted by the queue, NOT by wgpu's auto-barrier
mechanism**, because they are on different command buffers / submits.

If the per-segment submits complete out-of-order relative to the
bounds-chain dispatches' read of `block_voxel_count` (`mod.rs:2615-2616`
uses a hard-coded upper bound, not a readback), then:

- The bounds chain reads `voxels[]` at every slot up to the upper bound
- The W5 producer is still writing slots in flight (some segments not
  yet completed)
- The bounds chain reads partially-written `voxels[]` data → computes
  garbage AADFs → writes garbage back → garbage stays in `voxels[]`
  forever

This is a **data race between per-segment submits and the bounds-chain
dispatch**.

**HOWEVER**: wgpu's `Queue::submit` documentation specifies that submits
are ordered relative to each other on the queue. The bounds chain
dispatch is recorded into the `render_context` encoder, which is finished
and submitted at the end of the render-graph traversal. All 512
per-segment submits happen BEFORE the render-context submit. The queue
serializes them. So **this race should not occur** in practice.

Q3 is **DOWNGRADED to MEDIUM-LIKELIHOOD pending verification** that wgpu
actually serializes the per-segment + render-context submits as
specified.

#### **Candidate Q4 (HIGH-LIKELIHOOD, NEW)** — buffer size mismatch between binding declaration and actual buffer

The `world_layout` bind group binds `voxels` as `array<u32>` (unsized,
`world_data.wgsl:68`). The bound buffer is sized `voxels_alloc_len * 4`
bytes = `268,435,456 * 4 = 1,073,741,824` bytes = 1 GiB.

**`max_storage_buffer_binding_size` on most wgpu backends defaults to
128 MiB** (per the WebGPU spec). A 1 GiB buffer bound as a single
storage binding **exceeds the default limit and the binding may fail**.

If the binding fails, the renderer either:
- Crashes (would be visible in the user's report — not reported).
- The bind group is invalid and the dispatch is silently skipped (would
  produce a fully black frame — not the symptom).
- The bound buffer is silently truncated to `max_storage_buffer_binding_size`
  bytes and the renderer reads past the end of the binding → OOB reads
  return zero → architecture renders correctly (chunks[] is small) but
  voxels[] reads past 128 MiB / 4 = 32 M u32 elements return zero or
  garbage.

**The cursor data from doc 11 says `voxels_cpu.len() = 10,479,520`
u32s** (~40 MiB used) after the GPU producer runs. This is WELL under
128 MiB. So the cursor stays within the limit and the renderer's
dereferences should all land in valid territory.

UNLESS the renderer dereferences via a **wrong pointer** that points
into the 128 MiB+ region. With the cursor at 10.5 M u32s, the
`(cur_node & 0x3FFFFFFFu)` pointer field can hold values up to `0x3FFFFFFF
= ~1 G`. If `cur_node` is corrupted to have high bits set in its low 30,
the pointer dereferences past the 128 MiB binding limit → OOB read →
zero → black surface.

This requires CORRUPTED `cur_node` values in the chunks/blocks buffer,
not in voxels. The Stage 6 byte-equality is against the CPU oracle, so
if Stage 6 production-scale-replicated WOULD show corruption, it would
catch it. But Stage 6 doesn't run at production scale (512 segments
shared hash_map + full bounds chain + buffer-binding-size-limited reads).

**Q4 candidate: the W5 GPU producer writes 1 GiB voxels[] buffer, but
the renderer's bind group caps reads at `max_storage_buffer_binding_size`
(commonly 128 MiB), so any block whose voxel_ptr ≥ 32M reads OOB and
returns zero (or wrap-around).**

Actual `max_storage_buffer_binding_size` on the user's machine: not
captured in the available diagnostics. Check via `RenderDevice::limits()`
in the production prepare path.

Q4 is the **HIGHEST-LIKELIHOOD candidate remaining**. It is consistent
with:
- Architecture renders correctly (chunks/blocks layers fit comfortably
  under the 128 MiB cap).
- Voxel-leaf hits often return wrong types (the renderer's voxel_ptr
  derefs can land past 128 MiB once enough mixed blocks have claimed
  voxel slots).
- A FRACTION of voxel hits return correct types (those whose voxel_ptr
  is still within the binding limit, OR the voxel_ptr happens to wrap
  around into a coincidentally-correct value).

**To verify: instrument `RenderDevice::limits()` in `prepare_world_gpu`,
compare `max_storage_buffer_binding_size` against the actual
`voxels_alloc_len * 4` bytes; if the latter exceeds the former, the W5
voxels[] binding is over-large.**

#### Candidate Q5 (LOW-LIKELIHOOD, eliminated by user description)

Hash-map probe-exhaustion (`chunk_calc.wgsl:339` `return 2u`). Would
manifest as **whole blocks** missing or having wrong content. The user
reports per-voxel-level palette drift, not block-scale chunks of missing
geometry. **REFUTED.**

## Recommended fix (NOT to be implemented)

### Highest-leverage next dispatch

**1. Verify `max_storage_buffer_binding_size` against the W5 voxels[] /
blocks[] allocations** by reading `RenderDevice::limits()` and adding an
assertion in `prepare_world_gpu` that the allocated buffer fits within
the binding limit. If it doesn't, the buffer must be SPLIT or the
binding must be re-architected.

Implementation surface:

- `crates/bevy_naadf/src/render/prepare.rs:352-378` — add a runtime
  check: `assert!(blocks_alloc_len * 4 <= device.limits().max_storage_buffer_binding_size)`
  (same for voxels). If the assertion fires, the bug is confirmed Q4.
- The fix is buffer-sizing-aware: shrink the W5 worst-case bound from
  `chunks * 64 / chunks * 128` to whatever the per-fixture worst-case
  is, OR add the W5 producer's cursor-readback path so the buffer is
  sized AFTER the cursor is known, OR split into multiple bindings if
  the data genuinely exceeds the binding limit.

### Secondary

**2. Extend `--validate-gpu-construction-scaled` with a "production
shape" mode** that:

- Loads Oasis VOX exactly as the W5 install path does.
- Runs ALL 512 segments through a single shared hash_map (not the 64
  cap the existing multi-segment fixture has).
- Runs the chunk-AADF bounds_calc chain (the regime-1 +
  regime-2 chain) to convergence (not just chunk_calc + voxel/block
  bounds).
- Reads back the chunks/blocks/voxels at the SAME buffer-binding-size
  limit the renderer uses (= `max_storage_buffer_binding_size` if Q4 is
  the bug).
- Compares against `construct(&volume)` where `volume` is the FULL
  tiled-Oasis dense volume.

If Q4 is the bug, the production-shape readback at the buffer-binding-size
limit will reveal the voxel_ptr values that exceed the limit and the
byte-diff against the CPU oracle will show the wrong reads.

If Q4 is NOT the bug, the production-shape readback should still show
WHERE the divergence lies (chunk vs block vs voxel level), and the byte
values from the live readback will let the next-next dispatch identify
the actual mechanism.

Surface: extend
`crates/bevy_naadf/src/render/construction/mod.rs:run_one_tiled_byte_diff`
(or add a sibling) with `[256, 32, 256]` world + real Oasis ModelData +
full bounds_calc chain dispatch. Estimated 200 LOC.

## Confidence level

- **HIGH confidence that the bug is NOT a base-encoding mismatch between
  the two install paths.** Both paths use the same
  `build_constructed_world_sparse` (legacy uses its output directly; W5
  uses it as `ModelData` input). The base encoding is single-sourced.
- **HIGH confidence that the bug is downstream of the encoding
  layer.** Stage 6 byte-equality verified the W5 GPU producer chain
  produces semantically byte-equal output to `construct()` across 27
  fixtures including real Oasis.
- **HIGH confidence that the bug is in the renderer's READ path (or in
  the writes leading to it being incorrectly visible).** The symptom is
  "correct geometry, wrong colours" which is mechanically:
  `chunk_state == mixed (correct)` + `block_state == mixed (correct)` +
  `cur_voxel & 0x40000000 set (correct hit)` + `cur_voxel & 0x7FFF
  random (wrong)`.
- **MEDIUM-HIGH confidence that Q4 (max_storage_buffer_binding_size
  overrun) is the bug.** It mechanically explains the symptom and is the
  ONLY candidate consistent with both Stage 6's byte-equality (the
  PRODUCER writes correctly) AND the renderer's "thousands of types"
  reads (the READER's bindings silently truncate). Verifiable in a
  single instrumentation pass.
- **LOW confidence in Q3 (per-segment submit / bounds-chain memory
  ordering).** wgpu's queue-serialization specification should prevent
  this; absent a wgpu / naga bug, the per-segment submits complete
  before the render-context submit.
- **LOW confidence in Q1, Q2, Q5.** Each was independently refuted by
  code-trace or by the user's specific symptom shape.
- **NO confidence in the brief's "two construct() implementations"
  hypothesis.** Refuted by direct grep of the workspace.

## Cross-references

- The visual evidence: `target/e2e-screenshots/oracle_cpu.png` and
  `target/e2e-screenshots/oracle_gpu.png`, generated by
  `cargo run --release --bin e2e_render -- --vox-gpu-oracle`.
- Stage 6 byte-equality: `12-diagnostic-byte-diff-concrete.md`.
- Encoding bit-layer comparison: `10-diagnostic-encoding-comparison.md`.
- W3-T1 fix that did not change visible rendering:
  `13-diagnostic-w3-bounds-calc.md` and `03-impl.md:2659-2887`.
- D1 CPU mirror readback fix: `03-impl.md:2475-2658`.
- Production W5 dispatch loop: `mod.rs:2454-2566` (per-segment) +
  `mod.rs:2571-2634` (bounds chain).
- `prepare_world_gpu` buffer allocation: `prepare.rs:283-435`.
- The renderer's chunk → block → voxel descent:
  `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:283-401`.
- The renderer's voxel-types palette lookup:
  `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl:227-228`.
- `build_constructed_world_sparse` (the shared single-source encoder):
  `crates/bevy_naadf/src/voxel/vox_import.rs:860-1053`.
- The only `construct()` function: `crates/bevy_naadf/src/aadf/construct.rs:136`.
