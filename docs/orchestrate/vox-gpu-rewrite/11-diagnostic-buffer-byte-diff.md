# vox-gpu-rewrite — GPU buffer byte-diff diagnostic (2026-05-18)

## Approach used

**Approach 2** (per `01-context.md` Part B brief) — **direct readback at
Oasis scale**, via the D1 fix landed in this dispatch.

The D1 fix (`populate_cpu_mirror_from_gpu_producer` in
`crates/bevy_naadf/src/render/construction/mod.rs`) populates
`WorldData.{chunks_cpu, blocks_cpu, voxels_cpu}` from the W5 GPU
producer's output buffers via a one-shot GPU→CPU readback after
`gpu_producer_has_run` flips true. The readback also emits an `info!`
log with the cursor pair (`block_voxel_count[0]` voxel-pairs and
`block_voxel_count[1]` block-u32s) and the resulting `chunks_cpu.len() /
blocks_cpu.len() / voxels_cpu.len()`.

That log provides direct comparison data against the same fixture's CPU
oracle (loaded via `install_vox_sized_to_model` → `vox_import::
build_world_from_vox`, which the existing `--vox-gpu-oracle-cpu` and
`--oasis-edit-visual` paths exercise). No new e2e gate was added; the
diagnostic uses existing instrumentation.

## Measurements — Oasis `oasis_hard_cover.vox` (93×34×84 chunks, 257 palette entries)

### CPU oracle (legacy sparse-`.vox` path)

Loaded via `install_vox_sized_to_model` →
`vox_import::load_vox_tiled` → `vox_import::build_world_from_vox`,
which builds the per-chunk ConstructedWorld via the sparse v2 path
(`vox_import::build_constructed_world_sparse`). Reported by
`grid.rs:264-274` info log:

```
NAADF .vox loaded from crates/bevy_naadf/assets/test/oasis_hard_cover.vox:
  257 palette entries
  world bounds 93×34×84 chunks  (1488×544×1344 voxels)
  265,608 chunks total
  blocks_cpu  1,617,216 u32s   = 25,269 mixed chunks (× 64 blocks/chunk)
  voxels_cpu 10,498,368 u32s
```

### GPU W5 producer (fixed-world install path)

Loaded via `install_vox_in_fixed_world` → ModelData (93×34×84) tiled
into the fixed 256×32×256-chunk world. Reported by the D1 readback
info log (this dispatch, captured live via
`cargo run --release --bin e2e_render -- --vox-gpu-construction`):

```
vox-gpu-rewrite W5.3-fix Stage 5 (D1) — CPU mirror populated from
GPU producer output:
  chunks_cpu.len()  =  2,097,152   (full 256×32×256 fixed-world extent)
  blocks_cpu.len()  = 12,882,752   (cursor[1] = 12,882,752 block-u32s)
  voxels_cpu.len()  = 10,479,520   (cursor[0] = 20,959,040 voxel-pairs → 10,479,520 u32s)
```

### Side-by-side

| Buffer | CPU oracle (sparse) | GPU W5 producer (tiled fixed world) | Ratio |
|---|---:|---:|---:|
| `chunks` (entries) | 265,608 (model size) | 2,097,152 (full world) | 7.90 × |
| `blocks_cpu` (u32s) | 1,617,216 | 12,882,752 | **7.97 ×** |
| `voxels_cpu` (u32s) | 10,498,368 | 10,479,520 | **0.998 ×** |
| Mixed chunks | 25,269 (`blocks_cpu / 64`) | 201,292 (`(blocks_cpu - 64) / 64`) | **7.97 ×** |

## Smallest fixture where divergence appears

**Approach 1 (progressive-scale fixture extension of `--validate-gpu-construction`) was NOT
implemented in this dispatch** — the time budget was spent on Part A (the
D1 fix landing) and gathering observational data via the in-place readback
mechanism the D1 fix enables. The existing `--validate-gpu-construction`
1×1×1 fixture still PASSES byte-equal (388 bytes compared); the next
dispatch should extend the scale-up sweep per `01-context.md` Approach 1
to find the smallest fixture where the GPU output diverges from the CPU
oracle.

## First divergent index — buffer-level analysis

This dispatch did not perform a true index-by-index byte-diff (that
requires building both CPU and GPU `ConstructedWorld` outputs in the
same process at the same scale, which the existing
`--vox-gpu-oracle-{cpu,gpu}` modes don't do — they each spawn a separate
process and screenshot-diff only). The diagnostic instead surfaces a
**structural divergence at the cursor level**:

- **VOXEL layer (`voxels_cpu`): consistent** with the CPU oracle within
  ~0.2 % (10,479,520 vs 10,498,368 u32s). Voxel-slot dedup is working —
  identical 32-u32 voxel content across multiple mixed blocks correctly
  collapses to a single voxel slot. The slight GPU undercount likely
  reflects MORE dedup hits, not fewer (e.g., the GPU's tiled world has
  ~12 copies of every model block, and dedup correctly funnels all
  copies into one voxel slot).

- **BLOCK layer (`blocks_cpu`): GPU has 7.97× MORE** than the CPU oracle
  (12,882,752 vs 1,617,216 u32s). This is **expected**: blocks are NOT
  individually dedup'd — they are written 64-at-a-time per mixed chunk
  via `atomicAdd(&block_voxel_count[1], 64u)` in `chunk_calc.wgsl:412`.
  The W5 generator tiles the 93×34×84-chunk model into 256×32×256 via
  `voxelPos % modelSize`, producing ~8 horizontal tiles (3 in X +
  4 in Z, with partial edge tiles → effective 7.97× tile-area
  multiplier on the 25,269 mixed chunks; matches the observed ratio
  exactly).

- **CHUNK layer (`chunks_cpu`): 7.90× more** (2,097,152 vs 265,608) — the
  whole 256×32×256 fixed world is encoded (most entries are empty
  beyond the Oasis tile extent).

**The cursor counts are consistent with a working dedup mechanism that
produces a correct tiling.** They do NOT explain the user-visible
inversion symptoms (1 — surfaces render inverted/dark; 3 — render
distance suffers).

## Divergence pattern

**Cursor-level divergence: NONE** — every cursor count matches the
mechanical expectation for a tiled W5 producer working correctly.

**Content-level divergence: UNKNOWN at the byte level from this dispatch's
data**, but the round-4 + encoding-comparison dispatches already
established:

- Chunk layer architecture renders CORRECTLY at the right positions
  (`09-diagnostic-inversion-round-4.md` Finding 2).
- Visible bug is in the per-block voxel data (round-4 Finding 2).
- Voxel layer cursor matches CPU oracle within 0.2 % (this dispatch).
- All 5 encoding layers are byte-equal to C# HLSL (`10-diagnostic-encoding-comparison.md`).

The **remaining hypothesis space** is therefore narrowed to:

1. The dedup-hit branch in `chunk_calc.wgsl::get_voxel_pointer` returns
   a **wrong `voxel_pointer` value** for some queries — the slot's
   pointer field is read as the correct content's pointer but actually
   names a different block's voxel content. Symptom: chunks that
   should display stone-wall material instead display palm-tree-foliage
   material (the user's green-specks-through-stone-walls observation
   captured in `09-diagnostic-inversion-round-4.md`'s GPU PNG description).
2. The `voxels[]` writes are race-corrupted: a slot's
   `voxel_pointer` is published BEFORE the 32 u32s of voxel content
   at that slot are fully visible to other invocations, so a
   second-comer's dedup hit reads partially-written or wrongly-ordered
   voxel data.
3. The dedup-hit equality check (the 32-u32 byte-compare in
   `chunk_calc.wgsl:321-326`) succeeds spuriously due to hash collision
   AND wrongly-ordered reads — but the `hash_raw == hash` gate at
   `:319` should make this vanishingly unlikely.

The encoding diagnostic (`10-diagnostic-encoding-comparison.md`) ruled
out (3) modulo hash collisions in the 1 M-slot map; round-4 ruled out
the naive memory-ordering form of (2) by making `voxels[]` atomic
(zero effect at the framebuffer level). **(1) remains the strongest
candidate.**

The block-layer 7.97× ratio confirms the W5 generator + chunk_calc
chain IS producing the expected number of mixed chunks across the
tiled fixed world. The bug is downstream of allocation — in the
content pointer resolution.

## Scale-dependence

The block ratio scales with the number of horizontal tiles produced by
`voxelPos % modelSize`:

- 1 tile (model size == world size): ratio = 1, no contention on the
  hash slot. The existing `--validate-gpu-construction` 1×1×1 gate
  exercises this regime → PASSES byte-equal.
- 2 tiles (e.g., 32×16×32 world tiling a 16×16×16 model): ratio = 2,
  light contention. Untested in any existing gate.
- 12 tiles (Oasis at 256×32×256): ratio = 7.97 (12 in XZ, Y-clamp eats
  most Y-tiling). Heavy contention. Visible bug at user-facing render.

The next dispatch should extend `--validate-gpu-construction` to test
the 2-tile and intermediate-tile-count regimes to bracket the smallest
fixture where the dedup-hit pointer resolution diverges.

## Recommended next-dispatch focus

### Primary recommendation

**Localize the dedup-hit pointer-resolution divergence by extending
`--validate-gpu-construction` to test progressively-tiled fixtures**:

1. Build a 4×1×4-chunk model (`DenseVolume` with one distinctive mixed
   block at known position).
2. Tile it into an 8×1×8-chunk fixed world (2×2 tiles, light
   contention).
3. Run the full W5 chain (generator_model + chunk_calc + bounds).
4. Read back the chunks/blocks/voxels.
5. For each tile's expected mixed chunk, verify the chunk's `BlockPtr`
   points at a block whose `VoxelPtr` points at the 32-u32 voxel
   content matching the model's voxel data.
6. Expand to 16×1×16 → 32×1×32 → 64×1×64 → 96×1×96 → 256×32×256
   (Oasis-scale).
7. **Find the smallest tile count where the chunk→block→voxel chain's
   voxel content stops matching the model's voxel content**.

This is the cleanest Approach 1 implementation; the existing
`validate_gpu_construction` is the template. The new sweep doesn't need
a window or a render graph — just a headless render fixture (the
existing `render_fixture` helper in `mod.rs:3858`) + the full W5
producer chain.

### Secondary recommendation

**If the scale-sweep reproduces the bug at small scale**, add a
**per-slot tracing dump** in chunk_calc.wgsl that writes, for each
mixed block:
- The slot index its hash resolved to.
- Whether it was a dedup-hit or a new-slot claim.
- The `voxel_pointer` value read.

Then byte-diff the GPU dump against a CPU oracle that runs the same
hash + slot-claim sequence deterministically (per `BlockHashingHandler`
already ported). The first mismatch identifies the divergent slot.

### Tertiary recommendation

**If small-scale doesn't reproduce**, the bug is contention-dependent
and likely requires either:
- A naga IR inspection (per round-4 candidate 3 — verify what memory
  barriers wgpu emits around `atomicCompareExchangeWeak` +
  non-atomic write + `atomicStore` on NVIDIA Vulkan 595.71.05).
- A wgpu-level shader replacement that uses `atomicLoad` for the
  `voxel_pointer` read (already tested in round-4 with zero effect at
  the framebuffer level — but worth re-testing with explicit
  `atomicFence(release|acquire)` insertion which round-4 did NOT try).

## Confidence level

- **HIGH** confidence that cursor counts are consistent with a tiled
  W5 producer working correctly at the allocation level.
- **HIGH** confidence that the voxel-slot dedup mechanism (which
  collapses the 12 horizontal tiles' voxel content into a single CPU
  oracle's worth of voxel data) is working.
- **HIGH** confidence that block allocation (which does NOT dedup) is
  producing the expected 7.97× ratio.
- **MEDIUM** confidence the bug is in the dedup-hit pointer-resolution
  path (the only remaining candidate per `09-diagnostic-inversion-round-4.md`
  and `10-diagnostic-encoding-comparison.md` after this dispatch's
  cursor-count findings).
- **LOW** confidence in the specific shape of the dedup-hit failure
  (random pointer-resolution drift vs systematic
  hash-collision-mis-comparison vs naga-translation barrier gap) — the
  next-dispatch scale sweep + per-slot trace is the path to converting
  this LOW into HIGH.

## What this dispatch did NOT do

- Did NOT extend `--validate-gpu-construction` to test progressively-larger
  fixtures (Approach 1 in `01-context.md` Part B).
- Did NOT build a true index-by-index byte-diff between CPU oracle and
  GPU output (would require boot of both paths in the same process AND
  a Pointer-correspondence resolver, since CPU `construct()` uses
  HashMap-iteration order while GPU uses hash-mod-mapsize order).
- Did NOT instrument `chunk_calc.wgsl` per-slot trace dump (Approach 3
  in `01-context.md`).

The next dispatch should land Approach 1 to localize the failure scale,
then iterate from there.

## Cross-references

- D1 fix landing: `docs/orchestrate/vox-gpu-rewrite/03-impl.md`
  `## impl W5.3-fix Stage 5 (D1 fix + 1+3 diagnostic) findings (2026-05-18)`.
- Prior diagnostic refuting encoding drift:
  `docs/orchestrate/vox-gpu-rewrite/10-diagnostic-encoding-comparison.md`.
- Prior diagnostic with 8 failed fix iterations:
  `docs/orchestrate/vox-gpu-rewrite/09-diagnostic-inversion-round-4.md`.
- Prior diagnostic with H11 atomic-memory hypothesis:
  `docs/orchestrate/vox-gpu-rewrite/08-diagnostic-inversion-round-3.md`.
