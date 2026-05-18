# vox-gpu-rewrite — concrete CPU vs GPU byte divergence (2026-05-18)

This diagnostic supersedes `11-diagnostic-buffer-byte-diff.md` (which inferred
from cursor ratios). It produces **concrete byte-level data** by running both
the GPU W5 producer chain and the CPU oracle on the same fixtures across a
scale sweep, then diffing the output u32-by-u32.

The headline finding: **after exhaustive byte-level comparison across every
W5 producer-chain component, no divergence was found at any tested scale
including the actual Oasis VOX fixture loaded from disk.** The chunk_calc
chain (`calc_block_from_raw_data` + `compute_voxel_bounds` +
`compute_block_bounds`) and the generator chain (`generator_model.wgsl`)
both produce byte-equivalent output to their CPU oracles.

This **eliminates** the W5 producer chain itself as the source of symptoms
1 (visual inversion) and 3 (render distance suffers). The bug must be in a
component the diagnostic did not exercise — most likely the **chunk-layer
AADF pass** (`bounds_calc.wgsl`), the production buffer-allocation glue in
`prepare_construction`, or the downstream renderer's read addressing.

## Setup

### New diagnostic mode

Added `--validate-gpu-construction-scaled` to `e2e_render` (entry point:
`crates/bevy_naadf/src/bin/e2e_render.rs:91-93` flag parsing,
`crates/bevy_naadf/src/bin/e2e_render.rs:139-153` short-circuit dispatch).

The dispatch entry point is
`crate::render::construction::validate_gpu_construction_scaled` at
`crates/bevy_naadf/src/render/construction/mod.rs:3473` (start of the
`validate_gpu_construction_scaled` function — runs four fixture sweeps in
sequence).

The diagnostic boots a headless render world (the same pattern as the
existing `validate_gpu_construction`), allocates GPU storage buffers,
dispatches the W5 chain components, reads back via
`BufferUsages::MAP_READ` + `map_async` + `PollType::wait_indefinitely`, and
compares the GPU readback against the corresponding CPU oracle.

### Fixture sweeps

Four sweeps run sequentially:

**1. Single-segment chunk_calc fixtures** (`run_one_fixture_byte_diff` at
`mod.rs:3567`). Synthesizes a `DenseVolume`, builds the
`segment_voxel_buffer` via the existing `build_segment_voxel_buffer_for_world`
helper, dispatches `chunk_calc::calc_block_from_raw_data_world_sized` over the
full world extent, then `compute_voxel_bounds` + `compute_block_bounds`. CPU
oracle: `aadf::construct::construct(&volume)`.

Per-chunk content mode:
- `Uniform`: every chunk has identical mixed content (1 voxel at block
  corner) — heavy dedup-hit traffic.
- `Diverse`: every chunk has unique content (cycles through 64 voxel
  positions) — heavy new-slot CAS traffic.
- `Mixed`: half uniform, half diverse — both paths exercised.

Sizes tested: `2x1x2`, `4x1x4`, `16x1x16`, `32x2x32`, `64x4x64` (up to 16384
chunks, 1M+ block u32s claimed).

**2. Multi-segment chunk_calc fixtures**
(`run_one_fixture_multiseg_byte_diff` at `mod.rs:4051`). Mirrors the
production W5 segment loop in
`crates/bevy_naadf/src/render/construction/mod.rs:2454-2566`: the same
hash_map is shared across N per-segment dispatches; the segment_voxel_buffer
is overwritten per-segment; each segment uses its own encoder + submit.

Sizes tested: `4x4x4-seg2`, `8x4x8-seg4`, `16x4x16-seg4`, `32x4x32-seg4`,
`64x16x64-seg16`, **`128x16x128-seg16`** (262144 chunks × 64 segments,
16,777,280 block u32s claimed — half the Oasis-scale 256×32×256 footprint).

**3. Generator_model.wgsl byte-diff** (`run_one_generator_model_byte_diff`
at `mod.rs:4459`). Compares the WGSL shader's `segment_voxel_buffer` output
against the bit-exact CPU oracle
`crate::aadf::generator::generate_segment_cpu`.

**4. Tiled ModelData** (`run_one_tiled_byte_diff` at `mod.rs:4870`). Small
model gets tiled into a larger world via the generator's `voxelPos %
modelSize` wraparound — mirrors production's Oasis tiling. Up to
`32x16x32` model tiled into `96x16x96` world (147,456 chunks, 9-tile
multiplier).

**5. Real Oasis ModelData segments** (`run_oasis_segment_byte_diff` at
`mod.rs:4625`). Loads `crates/bevy_naadf/assets/test/oasis_hard_cover.vox`
via the same `parse_dot_vox_data → ModelData` path that
`install_vox_in_fixed_world` uses (`grid.rs:317-429`), then runs
generator+chunk_calc on 16×16×16-chunk segments. CPU oracle:
`generate_segment_cpu` → decode back to a `DenseVolume` → `construct()`.

### Comparison methodology

Three independent comparisons per fixture:

1. **Raw u32 byte-equality**: index-by-index `cpu[i] == gpu[i]` for chunks /
   blocks / voxels. Sensitive to nondeterministic `atomicAdd` cursor
   ordering between mixed chunks — at multi-mixed-chunk scale the GPU's
   pointer values will differ from the CPU's even when the semantic
   content is identical. Reports first divergent u32 index + XOR mask.

2. **Semantic pointer-following diff**: walk the chunk → block → voxel
   tree on BOTH sides; compare each chunk's `ChunkCell::decode` kind; for
   `Mixed` chunks, dereference the block pointer and compare each
   `BlockCell::decode` kind; for `Mixed` blocks, dereference the voxel
   pointer and compare the 32 u32s of packed voxels. Reports the first
   index at each layer where the semantic content diverges. Insensitive to
   pointer values.

3. **Generator output byte-diff**: for fixtures that exercise the
   generator, compares the CPU oracle (`generate_segment_cpu`) against the
   GPU shader's `segment_voxel_buffer` output u32-by-u32.

Empty-chunk AADF differences are EXPECTED and IGNORED in semantic
comparisons for the Oasis-segment and tiled fixtures — those AADFs are set
by `bounds_calc.wgsl` (the chunk-layer AADF pass), which the diagnostic
does NOT exercise (only the chunk_calc family runs).

## Results — full sweep

### Single-segment chunk_calc

| Fixture | chunks first-divergent-index (raw) | blocks first-divergent (raw) | voxels first-divergent (raw) | semantic match? |
|---|---|---|---|---|
| `2x1x2-uniform` | 0 (ptr seed) | 64 (+64 seed) | byte-equal | **YES** |
| `4x1x4-mixed` | 0 (ptr seed) | 64 (+64 seed) | 64 (atomic-add reorder) | **YES** |
| `16x1x16-mixed` | 0 (ptr seed) | 64 (+64 seed) | 32 (atomic-add reorder) | **YES** |
| `32x2x32-diverse` (2048 mixed chunks) | 0 (ptr seed) | 64 (+64 seed) | 32 (atomic-add reorder) | **YES** |
| `64x4x64-mixed` (16384 mixed chunks, 1,048,640 block-u32s claimed) | 0 (ptr seed) | 64 (+64 seed) | 32 (atomic-add reorder) | **YES** |

The raw-byte differences are accounted for by:
- The GPU seeds `block_voxel_count = [64u32, 64]` so the first
  `atomicAdd(&block_voxel_count[1], 64u)` returns 64 (not 0); CPU
  `construct()` starts at 0. Pointer values shift by exactly +64 for
  blocks and +32 for voxels (the existing
  `validate_gpu_construction` test already documents and accounts for
  this).
- At multi-mixed-chunk scale, the order in which workgroups' `atomicAdd`
  calls land is nondeterministic; chunk[0]'s `BlockPtr` is not always 64
  (the first cursor slot) — any workgroup could claim slot 0.

**Semantic pointer-following comparison: PASSES at every scale tested.**

### Multi-segment chunk_calc (production-style per-segment dispatch)

| Fixture | n_segments | cursors voxel_pairs / block_u32s | CPU oracle blocks / voxels | semantic match? |
|---|---:|---|---|---|
| `4x4x4-seg2` | 8 | 2112 / 4160 | 4096 / 1024 | **YES** |
| `8x4x8-seg4` | 4 | 4160 / 16448 | 16384 / 2048 | **YES** |
| `16x4x16-seg4` | 16 | 4160 / 65600 | 65536 / 2048 | **YES** |
| `32x4x32-seg4` | 64 | 4160 / 262208 | 262144 / 2048 | **YES** |
| `64x16x64-seg16` | 16 | 4160 / 4194368 | 4194304 / 2048 | **YES** |
| **`128x16x128-seg16` (262144 chunks, 64 segments)** | 64 | 4160 / **16777280** | 16777216 / 2048 | **YES** |

The cursor counts match the CPU oracle exactly: GPU claimed
`16777280 - 64 = 16777216` block-u32s, oracle has 16777216 blocks.
**Multi-segment chunk_calc with shared hash_map across 64 dispatches
produces byte-equivalent semantic output to the CPU oracle at 262,144-chunk
scale.**

### Generator_model.wgsl

| Fixture | u32s compared | result |
|---|---:|---|
| `model-1x1x1-seg-1x1x1` | 2048 | **BYTE-EQUAL** |
| `model-2x1x2-seg-4x4x4` | 131,072 | **BYTE-EQUAL** |
| `model-4x2x4-seg-8x4x8` | 524,288 | **BYTE-EQUAL** |
| `model-8x2x8-seg-16x16x16` | 8,388,608 | **BYTE-EQUAL** |
| `model-2x1x2-seg-4x4x4-off2-1-2` (non-zero chunk offset, Y-clamp triggered) | 131,072 | **BYTE-EQUAL** |

The GPU `generator_model.wgsl` produces output that matches the bit-exact
CPU oracle `generate_segment_cpu` byte-for-byte across all 5 tested
configurations including 8.4M-u32 segments and non-zero `group_offset_in_chunks`.

### Tiled ModelData

| Fixture | n_segments | cursors voxel_pairs / block_u32s | CPU oracle | semantic match? |
|---|---:|---|---|---|
| `model-2x1x2_world-4x1x4` | 16 | 128 / 320 | 256 blocks, 32 voxels | **YES** |
| `model-4x1x4_world-16x1x16` | 256 | 384 / 5184 | 5120 / 160 | **YES** |
| `model-4x2x4_world-16x4x16` | 16 | 704 / 10304 | 10240 / 320 | **YES** |
| `model-8x2x8_world-32x4x32` | 64 | 2752 / 43072 | 43008 / 1344 | **YES** |
| **`model-32x16x32_world-96x16x96`** (9-tile Oasis-like ratio) | 36 | 4160 / **3145600** | 3145536 / 2048 | **YES** |

The 9-tile fixture matches the per-tile dedup behavior that doc 11 inferred
for Oasis (the 9 tiles' identical-content blocks dedup-collapse to one
voxel slot each).

### Real Oasis ModelData (loaded from `oasis_hard_cover.vox`)

Model dimensions per the loader: **93×34×84 chunks** (data_chunk=265,608
u32s, data_block=1,617,216 u32s, data_voxel=10,498,368 u32s, matching doc
11's CPU-oracle measurements exactly).

| Segment offset | seg size | cursor voxel_pairs / block_u32s | CPU oracle | semantic match? |
|---|---|---|---|---|
| `(0,0,0)` | 16³ | 127040 / 21376 | 21312 / 63488 | **YES** |
| `(16,0,16)` | 16³ | 81536 / 11264 | 11200 / 40736 | **YES** |
| `(32,0,32)` | 16³ | 600256 / 33152 | 33088 / 300096 | **YES** |
| `(48,0,48)` | 16³ | 173056 / 21376 | 21312 / 86496 | **YES** |
| `(60,0,60)` | 16³ | 11904 / 4096 | 4032 / 5920 | **YES** |
| `(76,0,64)` | 16³ (edge segment) | 1408 / 256 | 192 / 672 | **YES** |

For every Oasis segment tested, the cursor counts match the CPU oracle
exactly (after subtracting the 64-element seed), and the
generator-segment-voxel-buffer GPU output is byte-equal to the CPU
`generate_segment_cpu` oracle.

## Pattern analysis

After 27 distinct fixture configurations spanning:
- Single-segment chunk_calc at 4-16384 mixed chunks.
- Multi-segment chunk_calc at 64 mixed chunks to 262,144 mixed chunks across
  4-64 per-segment dispatches with shared hash_map.
- generator_model.wgsl in isolation at up to 8.4M u32 segments.
- Tiled model dispatch with 9-tile Oasis-like multiplication factor.
- Real Oasis VOX file loaded from disk at six interior segment positions.

**Not a single semantic byte divergence was found.** The CPU oracle's
`chunks → blocks → voxels` tree is structurally and content-wise identical
to the GPU producer chain's output, after accounting for the documented
+64/+32 cursor seed shift and nondeterministic per-workgroup
`atomicAdd` ordering.

This refutes the doc 11 hypothesis that "the dedup-hit pointer-resolution
path is the source of symptoms 1+3". Doc 11 inferred from cursor ratios
that there was a dedup-pointer-resolution divergence; this diagnostic walks
the actual content via pointer-following and finds none.

## Localized bug — where is it NOT

Concrete elimination based on the diagnostic data:

1. **`generator_model.wgsl`**: NOT the bug. Byte-equal to
   `generate_segment_cpu` on every tested fixture.

2. **`chunk_calc.wgsl::calc_block_from_raw_data`**: NOT the bug. Semantic
   pointer-following byte-equal to `construct()` from 4 mixed chunks to
   262,144 mixed chunks (16.7M block u32s, 4M+ voxel-pair atomicAdd
   claims).

3. **`chunk_calc.wgsl::compute_voxel_bounds`**: NOT the bug. The
   per-voxel AADFs in mixed blocks (which are written by
   `compute_voxel_bounds`) match `aadf::construct::encode_block_voxels`'s
   output byte-for-byte across all fixtures with non-empty mixed blocks.

4. **`chunk_calc.wgsl::compute_block_bounds`**: NOT the bug. Same
   reasoning as #3 at the block-AADF level.

5. **Per-segment hash_map state accumulation**: NOT the bug. Multi-segment
   dispatch with shared hash_map across 64 per-segment dispatches produces
   correct semantic output at every tile and every chunk.

6. **The per-segment encoder+submit loop**: NOT the bug. The diagnostic
   uses the same one-encoder-per-segment-submit pattern as the production
   code (`construction/mod.rs:2544-2561`) and produces correct output.

## Localized bug — where it remains possible

The bug must be in one of the following (NOT exercised by this
diagnostic):

### Candidate A — `bounds_calc.wgsl` (chunk-layer AADF iterative builder)

The chunk-layer AADFs in `Empty` chunk cells (5-bit fields at shifts
0,5,10,15,20,25 in the chunk word) are written by `bounds_calc.wgsl`'s
chain (`add_initial_groups_to_bound_queue` → `prepare_group_bounds` →
`compute_group_bounds`, all dispatched in a separate render-graph node
from the W5 producer, see `mod.rs:1659-1671`). The diagnostic ignores
Empty-chunk AADF differences and does not dispatch `bounds_calc.wgsl` at
all.

The bounds_calc chain is **iterative across frames** (impl log
`03-impl.md:874` — "bounds_calc chain fills in missing AADFs over
subsequent frames"). On Oasis-scale worlds (256×32×256 → 32,768 bound
groups), convergence could take **many** frames. During convergence the
chunk-AADFs are still zero or partial, and the renderer's
`ray_tracing.wgsl::shoot_ray` at `ray_tracing.wgsl:368-378` reads
`(cur_node >> shift_chunk.x) & 0x1Fu` → reads 0 → fails to skip past
empty chunks → single-steps → runs out of march budget → symptom 3
(render distance suffers).

If the chain never converges (because Oasis-scale exceeds some queue cap
or has a logic bug at that scale), the symptom would be permanent.

The diagnostic explicitly does **not** test the bounds_calc chain
because:

- It requires the W3 indirect-dispatch infrastructure
  (`bound_dispatch_indirect`, `bound_queue_info` atomic counters,
  `bound_group_queues` ring buffer).
- Convergence is multi-frame and indirect — building a self-contained
  fixture is non-trivial.
- The existing test surface for bounds_calc lives at
  `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` and
  is unit-test-scale only.

**File:line for the candidate-A code surface**:
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` (entire file)
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` (entire file —
  the dispatch wiring + parameter shape)
- `crates/bevy_naadf/src/render/construction/mod.rs:1659-1671` — the W3
  add_initial_groups dispatch site.
- `crates/bevy_naadf/src/render/construction/mod.rs:2622-2634` — the W5
  branch's bounds-chain dispatch. It dispatches only
  `compute_voxel_bounds` + `compute_block_bounds`. The chunk-AADF chain
  runs separately via the W3 path.

This is the most likely remaining suspect because (a) the diagnostic
covers everything else in the W5 producer chain, (b) the chunk-AADF chain
is the renderer's primary "skip past empty space" mechanism, and (c) doc
11's symptoms 1+3 (visual inversion + render distance) both have AADF
failure modes as plausible mechanisms.

### Candidate B — Buffer allocation / bind group setup

`prepare_construction` allocates buffers for the W5 path via different
sizing math than the legacy path. The chunks buffer is `chunk_count * 8`
bytes (`array<vec2<u32>>` storage, 8 B per pair); the blocks/voxels
buffers come from `prepare_world_gpu`'s W5-aware sizing path
(`render/prepare.rs::prepare_world_gpu`). If any of these are sized
based on `world_data_meta.{blocks,voxels}_cpu_len` (which is 0 for the
W5 install path per `voxel/grid.rs:409-425`), the buffers may be undersized
and the GPU writes silently truncate (WebGPU's defined no-op behavior on
OOB storage buffer writes).

**File:line**: `crates/bevy_naadf/src/render/prepare.rs::prepare_world_gpu`
(whole function — check whether its blocks/voxels sizing uses
`meta.dense_voxel_types`/`cpu_blocks_len`/etc., which would be empty on
the W5 install path).

### Candidate C — Renderer addressing

The renderer's `ray_tracing.wgsl::shoot_ray` reads `chunks[chunk_idx]` as
`array<vec2<u32>>`, with `chunk_idx = chunk_pos.x + chunk_pos.y *
size_in_chunks.x + chunk_pos.z * size_in_chunks.x * size_in_chunks.y`
(at `ray_tracing.wgsl:290-294`). If `size_in_chunks` is anything other
than the actual world's chunk extent at the renderer's read-time
(e.g., the model's 93×34×84 vs the world's 256×32×256), the index would
land in the wrong row and the renderer would read garbage.

**File:line**: `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:290-294`
and the binding for `size_in_chunks` in `render/render_params.rs`
(or wherever `GpuRenderParams` is populated for the W5 path).

## Recommended fix (NOT to be implemented in this dispatch)

Per the diagnostic discipline of this dispatch, no fix is to be landed.

The recommended next dispatch:

1. **Force-readback the chunks buffer after a fixed number of frames at
   Oasis scale** to see if the chunk-layer AADFs are zero, partially
   populated, or fully populated. Compare against the CPU `construct()`
   oracle's chunk AADFs on the same Oasis fixture. If the GPU chunk-AADFs
   are zero or stuck-partial after N frames where N is large enough that
   convergence "should" be done, **Candidate A is confirmed**. The
   intervention is then to either (a) verify the `bounds_calc` chain
   actually dispatches each frame at Oasis scale, (b) raise
   `max_group_bound_dispatch` so convergence completes faster, or (c)
   diagnose a logic bug in the chain that prevents convergence at the
   32,768-bound-group scale.

   A concrete implementation: extend the D1 readback at
   `populate_cpu_mirror_from_gpu_producer` to ALSO emit per-frame
   stats on what fraction of Empty chunks have non-zero AADFs, and run
   the `--vox-gpu-construction` gate with 100+ warmup frames.

2. **Inspect `prepare_world_gpu` for W5-path buffer-sizing dependencies on
   `world_data_meta.*_cpu_len`** which are 0 on the W5 install path. If
   any allocation is undersized, fix it to size off the full world's
   chunk-count × 64 worst case (already documented at
   `mod.rs:2599-2607`).

3. **Add a renderer read-side instrumentation pass** that, on a small
   fixture, captures what `shoot_ray` reads from `chunks[chunk_idx]` at
   the producer's known-correct positions and verifies the reads land on
   the producer-written values.

## Confidence level

**HIGH** — concrete byte-level data across 27 distinct fixtures including the
actual Oasis VOX file loaded from disk. The diagnostic has eliminated the
W5 producer chain components (`generator_model.wgsl` and `chunk_calc.wgsl`'s
three entry points) as the source of symptoms 1+3. The remaining candidates
are narrow: chunk-layer AADF pass (Candidate A — strongest), production
buffer-allocation glue (Candidate B), or renderer read addressing
(Candidate C).

**The diagnostic mode landed in this dispatch is permanent** — re-run via
`cargo run --release --bin e2e_render -- --validate-gpu-construction-scaled`
at any time. The full report is ~310 lines and prints to stderr.

## Cross-references

- Diagnostic implementation: `crates/bevy_naadf/src/render/construction/mod.rs:3473-...`
  (entry point `validate_gpu_construction_scaled`).
- CLI wiring: `crates/bevy_naadf/src/bin/e2e_render.rs:91-93` (flag), `:139-153`
  (short-circuit dispatch).
- Production W5 segment loop (the system the diagnostic mirrors):
  `crates/bevy_naadf/src/render/construction/mod.rs:2454-2566`.
- Production W5 bounds-chain dispatch (the dispatch the diagnostic
  observed is missing the chunk-AADF chain):
  `crates/bevy_naadf/src/render/construction/mod.rs:2622-2634`.
- Pre-existing W1 byte-equality gate (still passes — `388 bytes compared`):
  `validate_gpu_construction` at
  `crates/bevy_naadf/src/render/construction/mod.rs:3071`.
- Prior inferential diagnostic this replaces:
  `docs/orchestrate/vox-gpu-rewrite/11-diagnostic-buffer-byte-diff.md`.
- Prior encoding-comparison diagnostic (refuted bit-level encoding drift):
  `docs/orchestrate/vox-gpu-rewrite/10-diagnostic-encoding-comparison.md`.
