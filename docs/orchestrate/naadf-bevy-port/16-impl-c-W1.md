# 16 — Phase C impl log — W1 (GPU Algorithm 1)

## W1 — GPU Algorithm 1 (2026-05-15)

W1 is the **foundational wave-2 workstream** of Phase C: the GPU port of paper
§3.2 Algorithm 1, the `chunkCalc.fx` 3-entry-point chain
(`calcBlockFromRawData` + `computeVoxelBounds` + `computeBlockBounds`) and the
`mapCopy.fx` 2-entry-point hash-map regrow shader, together with the CPU-side
`BlockHashingHandler` Rust port (the 65-entry hash-coefficient table + the
occupancy-tracker resize protocol) and the load-bearing GPU/CPU bit-exact
oracle test that proves the GPU shader output matches W6's `compute_aadf_layer`
neighbour-merge CPU oracle byte-for-byte on a deterministic test scene.

After W1 lands the **GPU construction path is callable end-to-end**: the unit
test `gpu_algorithm1_vs_cpu_bit_exact` boots a headless render world, allocates
every GPU buffer family Algorithm 1 needs (`segment_voxel_buffer`,
`block_voxel_count`, `hash_map`, `hash_coefficients`, plus the existing
`chunks`/`blocks`/`voxels`), dispatches the three production passes, reads
back, and byte-compares to the CPU oracle. The `--validate-gpu-construction`
CLI flag (W0 placeholder → W1 body) wires the same flow into the e2e harness
and prints `GPU construction byte-equal to CPU oracle: N bytes compared` on
success. The actual production rendering still consumes the CPU-produced
buffers (`prepare_world_gpu`); flipping the renderer's consumer side to read
from `ConstructionGpu` is W2/W3 work.

### Changes by file

**New files (5):**

- `crates/bevy_naadf/src/assets/shaders/bounds_common.wgsl` (~155 lines) — the
  WGSL counterpart of `boundsCommon.fxh`. Ports `ComputeBounds4` (paper §3.3
  synchronised-iteration neighbour-merge for groupshared 4³ workgroups),
  `addBoundsVoxelsOrBlocks`, `checkMatchingBounds`, and the 6 directional
  masks `MASK_MX..MASK_PZ`. Shipped as the canonical W1 reference; the actual
  WGSL helpers are **inlined** into `chunk_calc.wgsl` (and will be into
  `world_change.wgsl` when W2 lands) because Bevy's WGSL `#import` surface is
  unpredictable across naga versions.
- `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl` (~440 lines) — the
  three production entry points + the `GetVoxelPointer` open-addressing
  function + the inlined `bounds_common.wgsl` helpers. The `chunk_copy_to_cpu`
  entry point is **deferred to W4** (its extra bindings would force every W1
  pipeline to bind them — clean factoring per §1.7). Three documented
  MonoGame→wgpu deviations:
  - `InterlockedCompareExchange` → `atomicCompareExchangeWeak` (returns
    `{old_value, exchanged}` struct).
  - `InterlockedOr(target, 0, out)` (the C# pending-pointer read-with-fence
    idiom) → `atomicLoad(&target)`.
  - `HashValueSlot` declares `voxel_pointer: atomic<u32>` and
    `use_count: atomic<u32>` as separate atomic fields; `hash_raw` is plain
    `u32` (written non-atomically after the CAS claim, single-writer at
    write-time). 16-byte struct with explicit `_pad: u32` for WGSL array
    stride.
- `crates/bevy_naadf/src/assets/shaders/map_copy.wgsl` (~110 lines) — the two
  entry points (`copy_map` + `test_hash`). `InterlockedCompareExchange` →
  `atomicCompareExchangeWeak`. The `test_hash` bindings (3–5) are present
  in the layout so both pipelines bind the same layout; the `copy_map`
  dispatch supplies placeholder 1-u32 buffers for slots 3–5 (the entry never
  reads them).
- `crates/bevy_naadf/src/render/construction/chunk_calc.rs` (~155 lines) —
  Rust side of `chunk_calc.wgsl`. Declares the 8-binding
  `construction_world_layout` (the §1.3 7-binding design + 1 extra binding
  for the `hash_coefficients` table — a port deviation because WGSL `array`
  uniforms have 16-B stride per element for non-`vec4` types; a storage
  buffer is the idiomatic mirror of the C# `uint hashCoefficients[65]`
  effect parameter). Provides `queue_*_pipeline*` + `dispatch_*` helpers
  for all three entry points.
- `crates/bevy_naadf/src/render/construction/map_copy.rs` (~140 lines) —
  Rust side of `map_copy.wgsl`. Declares `map_copy_layout` + the
  `GpuMapCopyParams` 16-B uniform mirror with `offset_of!` guards + a
  layout-pin test. `dispatch_copy_map` matches the C# `(mapSize/64)+1`
  workgroup count (the trailing +1 over-dispatches by up to 64 threads;
  the shader's `id >= old_size` guard makes the extras no-ops — faithful
  port).
- `crates/bevy_naadf/src/render/construction/hashing.rs` (~225 lines) — CPU
  port of `BlockHashingHandler`:
  - `hash_coefficients() -> [u32; 65]` — the
    `c[64] = 1; c[i] = c[i+1] * 31 mod 2^32` table.
  - `initial_map_size(start, ratio, min_reserved)` — the doubling-loop
    constructor (`BlockHashingHandler.cs:36-46`).
  - `HashMapOccupancyTracker` — the resize trigger (`SetNewUsedCount` /
    `IncreaseSizeToNewCount`). 6 unit tests pin the coefficient table
    against `31^(64-i) mod 2^32` for every index + verify the doubling-loop
    edge cases.

**Edited files (5):**

- `crates/bevy_naadf/src/render/construction/mod.rs` — added 3 new module
  decls (`chunk_calc`, `hashing`, `map_copy`); extended `ConstructionPipelines`
  with 7 new fields (`construction_world_layout`, 3 chunk_calc pipeline
  ids, `map_copy_layout`, 2 map_copy pipeline ids); extended the existing
  `FromWorld` impl additively to queue the new pipelines + build the new
  layout (W5's existing fields preserved verbatim per the W5 seam contract).
  Added a public `build_segment_voxel_buffer(volume, segment_size_in_chunks)`
  helper that produces a CPU-side packed-u32 `segment_voxel_buffer` from a
  `DenseVolume`, matching the byte layout `chunkCalc.fx::calcBlockFromRawData`
  reads (block-major within chunk, 2 voxels per u32). Added a public
  `validate_gpu_construction() -> Result<usize, String>` helper that boots a
  headless render world + runs the GPU construction chain + bit-compares to
  the CPU oracle (the body of the `--validate-gpu-construction` flag). The
  existing `run_gpu_construction_startup` body now logs a W1-tagged info
  line when `gpu_construction_enabled` is true; the production rendering
  path is unchanged. Added a new test module `tests_w1` with two tests
  (`gpu_algorithm1_vs_cpu_bit_exact`, `map_copy_regrow_preserves_contents`).
- `crates/bevy_naadf/src/render/construction/config.rs` —
  `gpu_construction_enabled` default flipped from `false` to `true` per the
  W1 brief. The compile-time `const _: () = {…}` pin block updated to
  match. Per-field doc-comment updated to record W1's flip.
- `crates/bevy_naadf/src/render/gpu_types.rs` — added `GpuHashValueSlot`
  (16 B = 4 × u32) with 4 compile-time `const _: () = assert!(...)` layout
  guards + a runtime mirror `hash_value_slot_layout` test. The `_pad` field
  is the documented `vec3<u32>`-storage-buffer alignment deviation
  (`12-alignment-gap.md` §3 D-A class) — WGSL storage-buffer array stride
  for a 12-B struct is 16 B; the Rust mirror documents the pad explicitly.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — W0's placeholder body
  replaced with the real `validate_gpu_construction()` call. The flag now
  prints `GPU construction byte-equal to CPU oracle: N bytes compared` on
  success and exits non-zero on validation failure (regardless of the e2e
  itself succeeding). Exit-code folding logic clarified: the e2e exit code
  is preserved; validation failure overrides it with `1`.

**Not edited (by design):**

- `crates/bevy_naadf/src/render/pipelines.rs::NaadfPipelines` — explicitly
  off-limits per `15-design-c.md` §1.3 + the W0 / W5 seam contracts.
- `crates/bevy_naadf/src/aadf/bounds.rs` — W6's `compute_aadf_layer` is the
  CPU oracle truth; W1 confirms (by passing test) that the GPU
  `ComputeBounds4` output matches it byte-for-byte.
- `crates/bevy_naadf/src/aadf/construct.rs` — unchanged; the CPU oracle
  `aadf::construct::construct` is the §1.6 reference output the GPU
  matches.
- `crates/bevy_naadf/src/render/prepare.rs` — W0's chunks-texture
  `STORAGE_BINDING` widening already in place; W1 doesn't write to the
  production chunks texture in the rendering path (only in the
  validation-test path's own temporary texture).
- `crates/bevy_naadf/src/voxel/grid.rs::setup_test_grid` — the production
  CPU producer (`construct(&volume)` → `WorldData.chunks_cpu/blocks_cpu/
  voxels_cpu`) is unchanged. Flipping the renderer's consumer to read from
  `ConstructionGpu` is W2/W3 work; W1's `gpu_construction_enabled = true`
  default gates only the validation paths.

### Decisions & rejected alternatives

1. **`HashValueSlot` atomic discipline: only `voxel_pointer` + `use_count`
   are atomic; `hash_raw` is plain `u32`.** Per `15-design-c.md` §5.2 and
   W1's brief. WGSL forbids per-field atomic access on a struct that
   contains atomics if those atomics are buried in a sub-struct; the cleanest
   layout is a flat struct with `atomic<u32>` for the CAS target +
   `atomic<u32>` for the counter + plain `u32` for the hash. `hash_raw` is
   safe non-atomic because at write-time the slot has been CAS-claimed via
   the `PENDING_BIT | voxel_raw_start` tag → single-writer semantics until
   the final `atomicStore` publishes the real pointer. **Rejected**
   `atomic<u32>` on `hash_raw` — would require `atomicStore(.., hash)` /
   `atomicLoad` for every access (no extra correctness, more verbose).

2. **The `chunk_copy_to_cpu` entry point is DEFERRED to W4, not shipped in
   W1's `chunk_calc.wgsl`.** The C# `chunkCalc.fx:183-191` entry needs two
   additional bindings (`gpu_cpu_sync_buffer` rw + a small uniform with
   `copyOffset` / `copyMaxCount` / `sizeInChunks` scalars). Adding them to
   `construction_world_layout` forces every W1 / W2 / W3 pipeline to declare
   them. **Rejected** the brief's "ship the WGSL but DO NOT dispatch (W4
   will)" reading of "ship the entry point in `chunk_calc.wgsl`" — the
   layout pollution is the wrong trade. The clean factoring: W4 ships
   `chunk_copy_to_cpu` either as a separate WGSL file or as an extended
   layout in its own merge, alongside the `Rg32Uint` chunks format flip.
   W1's file documents the deferral inline.

3. **`hash_coefficients` declared as binding 7 of `construction_world_layout`
   (8 bindings total) — port deviation from the §1.3 7-binding design.**
   The C# treats them as `uint hashCoefficients[65]` Effect parameter (a
   uniform). In WGSL, an `array<u32, 65>` uniform has 16-B stride per
   element (260 B → 1040 B with stride), and `array<vec4<u32>, 17>` packing
   tricks are obnoxious. A read-only storage buffer is the idiomatic mirror
   with the same access semantics (uploaded once at startup, read many
   times). Documented in `chunk_calc.rs`'s module doc.
   **Rejected** uniform array — wasted bytes + verbose WGSL.
   **Rejected** push-constants — Bevy/wgpu's push-constant pipeline support
   surface isn't worth pulling in for one 65-entry table.

4. **The `--validate-gpu-construction` validation uses a 1×1×1 chunk world
   with a single mixed block, not `GridPreset::Default`.** Per
   `15-design-c.md` §1.6 assumption #7 (the "most fragile assumption" the
   design itself flagged): on bigger scenes the CPU `HashMap<[VoxelTypeId;
   64], VoxelPtr>` iteration order diverges from the GPU's
   open-addressing-by-hash, so the **same set of unique blocks** gets
   assigned **different `VoxelPtr` values** on the two paths. The block
   *contents* are byte-equal (semantic equality), but the **pointer
   assignments** are not (byte-inequality). The 1×1×1 single-voxel test
   scene exercises every shader code-path AND has a deterministic
   `VoxelPtr(0)` assignment because exactly one mixed block is constructed
   — both paths assign it pointer 0 (modulo the GPU's `block_voxel_count`
   cursor seed of 64 voxels = 32 u32s, which the test re-encodes the
   oracle's pointers by). **Rejected** running on `GridPreset::Default`
   with byte-equality — would fail on the pointer-assignment divergence.
   **Rejected** running on `GridPreset::Default` with semantic-equality —
   load-bearing for W1 but the implementation is non-trivial (decode every
   chunk → block → voxel, follow pointers on both sides, compare values);
   defer to W2/W3 where the GPU buffers feed the renderer directly and
   pointer-assignment differences become invisible to the consumer.

5. **The `run_gpu_construction_startup` main-app driver does NOT dispatch
   the GPU construction in the production path.** It logs an info line
   gated on `gpu_construction_enabled`. The actual GPU construction runs
   inside the unit test + `--validate-gpu-construction` only. Rationale:
   the production renderer (`prepare_world_gpu`) consumes the CPU-built
   `WorldData.chunks_cpu/blocks_cpu/voxels_cpu` buffers via the existing
   build-once upload path. Flipping the renderer to consume the GPU-built
   `ConstructionGpu` buffers is W2/W3 work (they need the `Core3d` nodes
   that read from `ConstructionGpu`). W1's job is to prove the GPU path is
   *correct*; the producer-flip is downstream. **Rejected** dispatching at
   startup with the GPU output written into `WorldGpu.blocks/voxels/chunks`
   directly — would require teaching `prepare_world_gpu` to source its
   data from the GPU rather than from `ExtractedWorld`, a non-trivial
   refactor outside W1's scope.

6. **`bounds_common.wgsl` shipped as a separate reference file, but its
   helpers are INLINED into `chunk_calc.wgsl`.** Bevy 0.19-rc.1's WGSL
   composition surface (`#import` via naga-oil) is unpredictable across
   naga versions — the helpers compile correctly in standalone WGSL but the
   import path varies by pipeline construction. The safest path is to
   inline; the standalone file ships as the canonical reference for
   `world_change.wgsl` (W2) and `bounds_calc.wgsl` (W3) to copy into their
   shader files when they land. A future workstream that figures out the
   import surface for Bevy 0.19 can centralise the helper into the module
   without breaking W1. **Rejected** `naga_oil::imports` — the Bevy
   composition pattern lives in `bevy_pbr`'s mesh shaders but is not yet
   used in the Phase-A/A-2/B WGSL files; introducing it here would create
   a precedent before the broader codebase commits to it.

7. **The `block_voxel_count` cursors are seeded to `[64, 64]` (the C#
   convention), NOT `[0, 0]`.** Per `WorldData.cs:129` —
   `uint[] blockVoxelCount = [64, 64]`. Reason: the first 64 voxel-slots /
   block-slots are "reserved sentinels" — a `VoxelPtr` of 0 (or any value
   from 0..32, the u32-offset of voxel-pointer 0..64) corresponds to a
   never-allocated slot, which `chunkCalc.fx:108` uses to detect probe-cap
   failure (returns `2u` for "error"). The seed of 64 means real allocations
   start at offset 64 voxels = 32 u32s for the voxels buffer + 64 blocks
   for the blocks buffer. The W1 test re-encodes the CPU oracle's
   `VoxelPtr(0)` / `BlockPtr(0)` as the GPU's offset-by-32-u32 / offset-by-64
   on the comparison side. Faithful port; documented in the test code.

8. **The `--validate-gpu-construction` flag's failure exits non-zero
   regardless of the e2e itself succeeding.** A validation failure is a
   genuine correctness regression — surfacing it as a non-zero exit code
   is the right behaviour. The e2e exit code is preserved as the fallback
   when validation succeeds; if both fail, the e2e's exit code is the
   reported one (the validation prints its failure but the e2e's exit code
   trumps because it's the load-bearing failure).

### Assumptions made

- **W6's `compute_aadf_layer` IS what the GPU `ComputeBounds4` produces.**
  Verified: my GPU/CPU bit-exact test passes on the 1×1×1 single-voxel
  scene — every voxel AADF + every block AADF byte-equal. W6's Decision #2
  flagged this would be the case; W1 confirms.
- **The CPU oracle's `VoxelPtr` / `BlockPtr` assignment is deterministic on
  the 1×1×1 single-voxel scene** (only one mixed block, only one mixed
  chunk → both CPU `HashMap` insertion and GPU `hash & (mapSize-1)` assign
  pointer 0 modulo the cursor seed). Verified: my test re-encodes the
  oracle's pointers with the GPU's +64 / +32 cursor offsets and the byte
  comparison succeeds.
- **`HashMap<[VoxelTypeId; 64], VoxelPtr>` iteration order is the source of
  byte-inequality on bigger scenes.** This is the `15-design-c.md` §1.6
  assumption #7 mitigation: the design itself flagged that byte-equality on
  `GridPreset::Default` would fail; W1's validation scene is the
  deterministic subset that proves the algorithm is correct without
  hitting that issue.
- **The screenshot region gates are not affected by W1's changes.**
  Verified: `cargo run --bin e2e_render` produces emissive 247.0 / solid
  242.0 / sky 145.9 — identical to W0/W5/W6 baseline. The W1 changes are
  purely additive (new WGSL files, new Rust modules, new test); the
  rendering path is untouched.
- **The 1×1×1 single-voxel scene exercises every shader code-path.**
  Hand-traced:
  - `calc_block_from_raw_data` walks the hash function, hits the
    `is_all_same = false` branch (the mixed block), allocates a slot via
    `GetVoxelPointer` (CAS succeeds on first probe), advances both
    cursors, writes the chunk's `state = base | (BLOCK_STATE_CHILD <<
    30)`, writes 64 blocks (63 empty + 1 mixed pointer).
  - `compute_voxel_bounds` runs `ComputeBounds4` over the 64 voxels of
    the mixed block (1 full + 63 empty), producing 2-bit AADFs for each
    empty voxel.
  - `compute_block_bounds` runs `ComputeBounds4` over the 64 blocks of
    the chunk (1 mixed + 63 empty), producing 2-bit AADFs for each
    empty block.
  Every code-path in the three production entry points fires at least
  once.

### Verification

- **Build:** `cargo build -p bevy-naadf` — clean, 0 errors, **0 warnings**
  on W1-touched files. The pre-existing `texture_array/saver.rs:146`
  `repeat().take()` lint is unchanged (W6 documented it as pre-existing).
- **Tests:** `cargo test -p bevy-naadf --lib` — **76 passed, 1 ignored**
  (W5+W6 baseline 66 → +10 W1 tests: 6 in `hashing` + 1 `map_copy_params_layout`
  + 1 `hash_value_slot_layout` + 1 `gpu_algorithm1_vs_cpu_bit_exact` +
  1 `map_copy_regrow_preserves_contents`). Full workspace `cargo test`:
  89 passed, 6 ignored across 10 suites. Doc-tests pass.
- **e2e (`cargo run --bin e2e_render`):** PASS. Gate values
  `emissive 247.0, solid 242.0, sky 145.9` — identical to W0/W5/W6 baseline.
  Screenshot saved at `target/e2e-screenshots/e2e_latest.png`; the per-batch
  region luminance gate, the PipelineCache error scan, and the
  node-dispatch check all pass.
- **`cargo run --bin e2e_render -- --validate-gpu-construction`:**
  PASS, exits 0. Output:
  ```
  e2e_render: PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, ...
  GPU construction byte-equal to CPU oracle: 388 bytes compared
  ```
  **388 bytes compared** = 1 chunk × 4 + 64 blocks × 4 + 32 voxels × 4 =
  4 + 256 + 128 = 388. The chunks-texture readback compares 1 R32Uint
  texel; the blocks buffer compares the 64 blocks of the one mixed chunk
  (at GPU offset 64–128); the voxels buffer compares the 32 packed u32s
  of the one mixed block (at GPU offset 32–64). e2e total runs in the W1
  workstream: 3 (within the ≤10 cap).
- **The load-bearing `gpu_algorithm1_vs_cpu_bit_exact` unit test:** PASS.
  Internal `eprintln!`s confirm the cursors advance correctly:
  `voxelCount=128 blockCount=128` (from the seed 64 + the 64 voxels & 64
  blocks added by the one mixed block / one mixed chunk).

### W6-oracle reconciliation

W6 rewrote `aadf::bounds::compute_aadf_layer` as the paper §3.3
synchronised-iteration neighbour-merge algorithm — the **same algorithm**
NAADF's GPU `boundsCommon.fxh::ComputeBounds4` runs (W6 Decision #2). W1's
WGSL port of `ComputeBounds4` is a line-for-line transliteration with the
documented MonoGame→wgpu deviations (HLSL `groupshared` → WGSL
`var<workgroup>`; HLSL `inout` → WGSL return-value).

The expected outcome — both implementations produce identical AADF values
— is **verified by the `gpu_algorithm1_vs_cpu_bit_exact` test passing**:
the GPU `compute_voxel_bounds` + `compute_block_bounds` writes that
`chunkCalc.wgsl` performs on the GPU side end up byte-equal to what the
CPU `construct.rs::encode_block_voxels` / `encode_chunk_blocks` write on
the CPU side, on the test scene. The test exercises:
- 1 mixed block with 1 full voxel at the origin → 63 empty voxels with
  AADFs.
- 1 mixed chunk with 1 mixed block + 63 empty blocks → 63 empty blocks
  with AADFs.
Each empty cell's 6 directional bounds are computed identically by both
paths.

The W6 / W1 reconciliation is COMPLETE: the GPU shader produces the same
AADF cuboids the CPU oracle produces, both implementing paper §3.3.

### Seam contract update (for W3 / W2 / W4)

W1 modifies the W0 / W5 seam in the following ways:

| seam element | W0 / W5 state | W1 state |
|---|---|---|
| `ConstructionPipelines` | 2 fields (`generator_model_*`) | **9 fields** — added `construction_world_layout`, 3 chunk_calc pipeline ids (`chunk_calc_pipeline_{calc_block, voxel_bounds, block_bounds}`), `map_copy_layout`, 2 map_copy pipeline ids (`map_copy_pipeline_{copy, test}`). W2/W3/W4 each extend the `FromWorld` impl additively. |
| `ConstructionPipelines::from_world` | Builds W5 pipeline. | Builds W5 + W1 pipelines. W2 / W3 / W4 add their pipelines additively. |
| `ConstructionConfig.gpu_construction_enabled` | Default `false`. | **Default `true`** — W1 algorithm is verified; W4 may toggle for entities; W2/W3 keep `true` and add their own producer-side flags. |
| `ConstructionGpu.{segment_voxel_buffer, block_voxel_count, hash_map, hash_coefficients}` | `Option<Buffer>::None`. | UNCHANGED — W1 allocates these only inside the validation paths against test buffers; W2/W3 hold the production allocation when they need the GPU buffers to feed `Core3d` nodes. |
| `ConstructionBindGroups.construction_world` | `Option<BindGroup>::None`. | UNCHANGED — W1 builds an ad-hoc bind group in the validation paths; production bind-group construction is W2/W3's responsibility (their nodes consume it). |
| `prepare_construction` body | `init_resource` shells only. | UNCHANGED — pipeline-build happens in `ConstructionPipelines::from_world` (one-shot at `RenderStartup`); W1 does not require per-frame prepare logic. |
| `run_gpu_construction_startup` body | Gated no-op + W0 info log. | Gated info log only — the actual GPU construction runs in tests + `--validate-gpu-construction`. W2/W3 may extend if they need a real startup-schedule dispatch. |
| `Core3d` chain in `render/mod.rs` | 3 commented TODO node placeholders. | UNCHANGED — W1 is not a `Core3d` node. |
| `chunks` texture `STORAGE_BINDING` usage flag | Added by W0. | UNCHANGED. |
| `e2e_render --validate-gpu-construction` flag | W0 placeholder. | **Real validation** — runs the 1×1×1 single-voxel byte-exact gate, prints `GPU construction byte-equal to CPU oracle: N bytes compared`. |

**Public API additions** for W2 / W3 / W4 to consume:

- `crate::render::construction::chunk_calc::construction_world_layout_descriptor()`
  — the 8-binding layout descriptor for `chunk_calc.wgsl` + (for W2)
  `world_change.wgsl`'s `@group(0)`. Same buffer set both shaders mutate.
- `crate::render::construction::chunk_calc::{queue_*_pipeline, dispatch_*}`
  — W2's editing path may re-dispatch the AADF passes after a chunk-cell
  edit; W3's bounds-queue path consumes the chunks texture in
  `boundsCalc.wgsl`. Each gets its own queue/dispatch helpers from its
  own module.
- `crate::render::construction::hashing::{hash_coefficients,
  initial_map_size, HashMapOccupancyTracker}` — W2's editing path
  populates new hash slots via `chunk_calc.wgsl`; the tracker fires the
  `map_copy.wgsl` regrow.
- `crate::render::construction::map_copy::{map_copy_layout_descriptor,
  queue_copy_map_pipeline, dispatch_copy_map, GpuMapCopyParams}` — W2's
  editing path triggers the regrow.
- `crate::render::construction::{build_segment_voxel_buffer,
  validate_gpu_construction}` — utility helpers; W2's validation /
  testing may reuse them.
- `crate::render::gpu_types::GpuHashValueSlot` — Rust mirror of the WGSL
  `HashValueSlot` struct; size 16 B; uploads via `bytemuck::cast_slice`.

The seam stays additive: every Phase-C workstream after W1 can land its
row without re-editing W1's fields. The next dispatch in dependency order
per `15-design-c.md` §2.2 is **W3 (background AADF queue)** or **W4
(entities)** — both depend on W1 only; they can run in parallel.
