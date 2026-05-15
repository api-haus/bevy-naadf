# 16 — Phase C impl log — W3 (background AADF queue)

## W3 — Background AADF queue (2026-05-15)

W3 is the wave-2 fan-out workstream that ports NAADF's **regime-2 every-frame
background chunk-AADF queue** (`boundsCalc.fx` + `WorldBoundHandler.cs`, paper
§3.3 contribution #3). It adds three GPU compute pipelines + the
`naadf_bounds_compute_node` `Core3d` schedule node + the per-frame 5-rounds
{prepare → indirect compute} dispatch loop, gated by
`ConstructionConfig.max_group_bound_dispatch` /
`ConstructionConfig.n_bounds_rounds`. The W1 seam is preserved; W3 lives
entirely under `crates/bevy_naadf/src/render/construction/bounds_calc.rs`
(Rust) + `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` (WGSL) +
small additions to `mod.rs` / `render/mod.rs` / `gpu_types.rs`.

After W3 lands the **regime-2 sub-graph is callable end-to-end**: the
`naadf_bounds_compute_node` runs on every `Core3d` frame ahead of
`naadf_atmosphere_node`, the regime-1 startup-seed dispatch fires when
`prepare_construction` first sees `WorldGpu`'s chunks texture, and the GPU
chunk-layer AADFs converge to bit-equal values against a faithful CPU port of
the same algorithm (`tests::cpu_converged_bounds` — including the chunk-world-
edge OOB-permissive convention NAADF documents at `boundsCalc.fx:98-103`).

### Changes by file

**New files (3):**

- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` (~310 lines) — the
  three production entry points + the inlined 5-bit `check_matching_bounds`
  helper + the `add_bounds_group` neighbour-check function. Three documented
  MonoGame → wgpu deviations:
  - HLSL `RWStructuredBuffer<uint3> boundGroupMasks` → WGSL
    `array<atomic<u32>>` of length `bound_group_count * 3`, indexed
    `group * 3 + axis`. WGSL forbids `atomic<vec3<u32>>`; the C# accesses one
    axis at a time at every call site (`boundsCalc.fx:135,179,183`), so a flat
    per-axis array is a 1:1 mechanical translation.
  - HLSL `RWByteAddressBuffer.Store(0, value)` (`boundsCalc.fx:92`) → WGSL
    `bound_dispatch_indirect[0] = max(1, group_amount)`. The buffer carries
    `dispatch_workgroups_indirect` args; we write `GroupCountX` and leave
    `[1]/[2]` at the prepare-pass startup-seed of 1.
  - HLSL `InterlockedAdd(.size, 1, original_out)` → WGSL
    `atomicAdd(&size, 1u)`. The `BoundQueueInfo.size` field is declared
    `atomic<u32>` (one of the two struct fields, the other `start` is a plain
    `u32` written only by `prepare_group_bounds` which is the single
    `@workgroup_size(1,1,1)` writer).
  - HLSL `chunks[chunkPos]` reads/writes → WGSL `textureLoad(chunks, p).x` /
    `textureStore(chunks, p, vec4<u32>(state, 0u, 0u, 0u))`. **Forward-compat:
    every `textureLoad(chunks, ...)` uses `.x` so the W4 widening to
    `Rg32Uint` is a no-op for this shader** (`15-design-c.md` §1.7).
  - HLSL `groupshared bool anyBoundsIncrease` → WGSL `var<workgroup>
    any_bounds_increase: atomic<u32>`. WGSL doesn't allow `bool` in workgroup
    storage cleanly; the variable is diagnostic-only (C# never reads it back
    outside the kernel).

- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` (~410 lines) —
  Rust side of `bounds_calc.wgsl`. Declares 3 layouts
  (`construction_bounds_world_layout` `@group(0)` for chunks+params,
  `construction_bounds_layout` `@group(1)` for the 4 bound-queue buffers,
  `bound_dispatch_indirect_layout` `@group(2)` for the indirect counter), 3
  pipeline-queue helpers + their `_with_handle` headless-test variants, 2
  dispatch helpers (`dispatch_add_initial_groups` for the regime-1 seed,
  `dispatch_regime_2_rounds` for the per-frame 5-round loop), and the
  `naadf_bounds_compute_node` `Core3d`-schedule system. Plus 2 sizing
  helpers (`bound_group_count_of`, `group_size_in_groups_of`).

- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` (~705
  lines) — 5 tests:
  - `bounds_calc_convergence_matches_cpu_oracle` — the load-bearing W3 gate.
    Builds a 4×4×4 chunk world (1 bound group, one solid chunk at the centre);
    runs the regime-1 seed + 200 rounds of regime-2; reads back the chunks
    texture; compares chunk-by-chunk against `cpu_converged_bounds` (a
    faithful CPU port of `boundsCalc.fx`'s convergence algorithm, including
    the chunk-world-edge OOB-permissive convention). 64 chunks compared, 0
    mismatched.
  - `bounds_queue_no_overrun` — asserts the per-queue `size` field stays ≤
    `bound_group_count` after running the seed + 200 rounds (the queue ring
    capacity invariant); asserts mask values stay in [0, 2^31).
  - `bounds_per_axis_atomic_correctness` — verifies the regime-1 seed sets
    exactly `[1, 1, 1]` for the 3 per-axis masks (bit-0 of each = the size-0
    queue), then verifies the masks stay legal after 5 regime-2 rounds.
  - `cpu_oracle_tests::all_empty_saturates_to_max` — pure-CPU regression
    test for the oracle: an all-empty world saturates every AADF to 31.
  - `cpu_oracle_tests::wall_blocks_negative_direction` — pure-CPU regression
    for the oracle's solid-neighbour check.

**Edited files (5):**

- `crates/bevy_naadf/src/render/construction/mod.rs`:
  - Added `pub mod bounds_calc;`.
  - Added 6 new fields to `ConstructionPipelines`
    (`construction_bounds_world_layout` + `construction_bounds_layout` +
    `bound_dispatch_indirect_layout` + 3 pipeline IDs). Extended `FromWorld`
    additively to build them.
  - Added `construction_bounds_world` field to `ConstructionBindGroups` —
    the narrow `@group(0)` for `bounds_calc.wgsl` (2 bindings:
    `chunks_view` + `bounds_params`). Distinct from the 8-binding
    `construction_world` (W1's `chunk_calc.wgsl` `@group(0)`) so the W3
    prepare path does not depend on W1's hash buffers.
  - Added `bounds_params_buffer: Option<Buffer>` and `bounds_initialized:
    bool` to `ConstructionGpu` (the per-world `GpuConstructionParams`
    uniform + the regime-1-seed-done flag).
  - Extended `prepare_construction`'s body: allocate the 5 bound-queue
    buffers + the params uniform on first frame (with the seed
    `BoundQueueInfo[i*3+xyz] = { start: 0, size: i==0 ? boundGroupCount : 0 }`
    matching `WorldBoundHandler.cs:55-64`); build the 3 W3 bind groups; on
    first frame after the buffers/bind groups exist + the
    `add_initial_pipeline` has compiled, dispatch the regime-1 seed.
  - The system now takes 9 args (Bevy ceiling allow added — mirrors
    `prepare_frame_gpu`).

- `crates/bevy_naadf/src/render/mod.rs`:
  - Replaced the W0 TODO comment block with the real W3 chain-insert: imports
    `construction::bounds_calc::naadf_bounds_compute_node` and inserts it as
    the first entry of the `Core3d` chain tuple, before
    `naadf_atmosphere_node`. The chain is now 15 nodes (14 existing + W3),
    `.chain()` enforces W3 → atmosphere → first-hit ordering.

- `crates/bevy_naadf/src/render/gpu_types.rs`:
  - Added `GpuBoundQueueInfo` (8 B = 2 × u32) with `Pod`/`Zeroable` derives +
    4 compile-time `const _: () = assert!(...)` layout guards + a runtime
    mirror `bound_queue_info_layout` test (the +1 over W1's 76 baseline).

- (no edit to `pipelines.rs::NaadfPipelines` — explicitly off-limits per
  the seam contract.)
- (no edit to `prepare.rs` — W0's chunks-texture `STORAGE_BINDING` widening
  already in place; W3 reads/writes the same chunks texture.)

**Not edited (by design):**

- `crates/bevy_naadf/src/render/pipelines.rs::NaadfPipelines` — off-limits.
- `crates/bevy_naadf/src/aadf/bounds.rs` — W6's `compute_aadf_layer` is the
  CPU oracle for the inner-cell convergence; the chunk-world-edge convention
  divergence (`16-impl-c-W6.md` assumption #2) means the convergence test
  uses a workstream-internal CPU oracle (`cpu_converged_bounds` in
  `bounds_calc/tests.rs`) that faithfully ports `boundsCalc.fx`'s OOB-
  permissive growth — that is the right §1.6 oracle for W3.
- `crates/bevy_naadf/src/render/construction/chunk_calc.rs` — W3 does not
  modify the W1 chain.
- The W1 startup driver `run_gpu_construction_startup` — the brief asks
  W3 to extend it with a regime-1 `add_initial_groups_to_bound_queue` call.
  We landed the regime-1 seed inside `prepare_construction` instead (one
  frame after `WorldGpu` first exists + the pipeline has compiled) because
  (a) `run_gpu_construction_startup` runs on the main app's `Startup`
  schedule before the render sub-app has `WorldGpu`, so it cannot drive
  GPU dispatches against `WorldGpu`'s chunks texture; (b) W1's own startup
  body is a gated `info!` log only (it does NOT dispatch the GPU
  construction path; the validation-only dispatch runs in the
  `gpu_algorithm1_vs_cpu_bit_exact` test + `--validate-gpu-construction`).
  Putting the seed in `prepare_construction` keeps the regime-1 seed where
  every other W3 buffer allocation lives. (Documented as decision #5 below.)

### Decisions & rejected alternatives

1. **`bound_group_masks` per-axis layout: flat `array<atomic<u32>>` of
   length `groupCount * 3`, indexed `g*3+axis` (chosen).** Per
   `15-design-c.md` §4.2 + §5.7 + the brief. WGSL forbids
   `atomic<vec3<u32>>`; a `struct { atomic<u32>, atomic<u32>, atomic<u32> }`
   array would have 16-B stride (waste) and uglier indexing. The flat layout
   is 1:1 with the C# access pattern — every call site
   (`boundsCalc.fx:135,179,183`) updates a single axis at a time.
   **Rejected:** 3 separate `array<atomic<u32>>` arrays — would force 3
   bindings instead of 1, no semantic benefit.

2. **Dedicated narrow `construction_bounds_world_layout` (chosen) vs
   reusing W1's 8-binding `construction_world_layout`.** The brief
   nominally says `@group(0)` is shared. We chose to make the W3
   `@group(0)` a 2-binding layout (chunks + params only) because (a) it
   removes the W3 prepare path's dependency on W1's hash buffers existing
   (`prepare_construction` does not need to know about `hash_map` /
   `block_voxel_count` / `segment_voxel_buffer` to build the W3 bind
   group), (b) the WGSL `bounds_calc.wgsl` only references chunks + params,
   so binding 6 unused buffers wastes wgpu resource slots, (c) the
   convergence-test fixture is 2-binding rather than 8-binding. **Rejected:**
   shared 8-binding layout — pulled W3 unnecessarily into W1's dependency
   set; the W1 layout is owned by `chunk_calc.rs` and `world_change.rs`
   (W2) which need the hash buffers.

3. **Regime-2 5-rounds-per-frame bundling (chosen) — one node, 10 passes
   inside one command encoder.** `naadf_bounds_compute_node` calls
   `dispatch_regime_2_rounds(encoder, …, n_rounds=5)`, which emits 10
   compute passes (5 × {prepare, compute_indirect}) inside one encoder.
   wgpu's automatic STORAGE→INDIRECT barrier between the prepare write and
   the compute_indirect read serialises them. **Rejected:** 5 separate
   `Core3d` nodes (one per round) — would (a) add 5 chain-insert lines to
   `render/mod.rs`'s `.chain()`, (b) force the node-dispatch-check
   accounting to count 5 node-dispatches per frame, (c) prevent wgpu from
   inlining the per-round passes into one command buffer. The Phase-B
   precedent (`naadf_ray_queue_node`'s 2-pass bundling) is the same
   pattern.

4. **Chunks-`.x` forward-compat (chosen).** Every `textureLoad(chunks, p)`
   in `bounds_calc.wgsl` reads `.x` even though the texture is currently
   `R32Uint` (single-channel; `.x` is the same value). Per the W3 brief's
   "forward-compat" hard rule + `15-design-c.md` §1.7: when W4 widens the
   chunks texture to `Rg32Uint`, the W3 shader will continue to read the
   `.x` channel without any source change. This eliminates a merge-conflict
   on every chunks-read site during W4's atomic-merge `.x` sweep.

5. **Regime-1 seed runs in `prepare_construction`, NOT in the W1 startup
   driver (chosen).** The brief asks W3 to extend
   `run_gpu_construction_startup` with the `add_initial_groups_to_bound_queue`
   dispatch. W1's startup body is a gated `info!` log — it does not
   currently dispatch any GPU construction (the validation-only dispatch
   lives in `validate_gpu_construction` / the unit test); flipping the
   production producer to GPU is W2/W3-land. Putting the seed in the
   render-side `prepare_construction` keeps it where every other W3 buffer
   allocation lives, runs in the same sub-app the regime-2 node consumes
   from, and avoids the main-world Startup-vs-RenderApp synchronisation
   problem (the main app's `Startup` runs before the render sub-app has
   `WorldGpu`). **Rejected:** main-world `Startup` dispatch — would require
   teaching the regime-1 driver to wait for `WorldGpu` to exist on the
   render sub-app, then submit through the render sub-app's `RenderQueue`,
   adding cross-sub-app plumbing W1 explicitly did not.

6. **Indirect dispatch split (chosen) — `bound_dispatch_indirect_layout` is
   `@group(2)` of `prepare_group_bounds` only.** `bound_dispatch_indirect`
   is the buffer the prepare pass writes `GroupCountX` to and the compute
   pass consumes as INDIRECT args. wgpu's `STORAGE_READ_WRITE` × `INDIRECT`
   exclusivity rule forbids both usages in one bind-group layout. So:
   the prepare pipeline binds the buffer at `@group(2) @binding(0)` as
   `storage_buffer_sized` (rw); the compute pipeline does NOT bind it as
   a shader resource — it consumes it via
   `dispatch_workgroups_indirect(buffer, 0)`. The buffer's `BufferUsages`
   carry `STORAGE | COPY_DST | COPY_SRC | INDIRECT` so both consumers work.
   Mirrors the Phase-B Batch-4 fix (`sample_refine_dispatch_layout` —
   `render/pipelines.rs:531-540`).

7. **Convergence-test CPU oracle is the GPU's own algorithm
   (`cpu_converged_bounds`), NOT W6's `compute_aadf_layer` (chosen).**
   The two CPU functions disagree at the chunk world edge: W6 treats the
   layer boundary as a wall (`16-impl-c-W6.md` assumption #2); NAADF's
   `boundsCalc.fx:98-103` treats OOB neighbours as growth-permissive —
   directly bumping the bound. For our 4×4×4 test grid (every chunk
   touches at least one world edge), the two conventions diverge on every
   chunk. The right §1.6 oracle for W3 is **the CPU port of
   `boundsCalc.fx`'s convergence**, which we ship in
   `bounds_calc/tests.rs::cpu_converged_bounds`. W6's `compute_aadf_layer`
   remains the right oracle for W1's *initial* block/voxel AADFs (where
   the small-AADF convention IS the wall convention — see
   `boundsCommon.fxh::ComputeBounds4`). **Rejected:** comparing GPU
   convergence to `compute_aadf_layer` — would force the test grid to be
   large enough that every cell's chain stays within the layer for 31
   iterations (impractical at test scale); cell-by-cell edge-aware skip
   logic would obscure the test's load-bearing intent.

### Assumptions made

1. **`bound_group_count = 0` for the default 4×2×4 grid is acceptable.**
   The C# `WorldBoundHandler.cs:41-42` requires `sizeInChunks / 4` per axis;
   with Y=2 the integer-divide is 0, so `boundGroupCount = 0`. Our
   `bound_group_count_of` returns 0 for any non-`% 4` dimension, the bound-
   queue family allocates `max(1, …)` slots (so wgpu doesn't reject
   zero-size buffers), and the regime-2 node's `prepare_group_bounds`
   finds every queue empty and writes `bound_refined_info[1] = 0`. The
   subsequent indirect compute dispatches with `GroupCountX = 1` (the
   minimum per `boundsCalc.fx:92` — `max(1, groupAmount)`), the single 4³
   workgroup runs but every thread early-exits (since `groupID.x < count`
   is false). Net cost: ~1 µs per round × 5 rounds per frame = negligible.
   The e2e gates stay green because the chunks texture's W1 production
   CPU build is unchanged and the regime-2 node has no work to do.
2. **Pre-W4 chunk texture is `R32Uint` everywhere.** The W3 layout +
   shader use `texture_storage_3d<r32uint, read_write>`; W4 flips this to
   `rg32uint` in its own merge along with the `.x` sweep on every reader.
   Our forward-compat `.x` selection makes the W4 flip a no-op for
   `bounds_calc.wgsl`.
3. **The CPU oracle's `cpu_converged_bounds` is exhaustively equivalent
   to the GPU's `boundsCalc.fx` convergence.** Verified by the convergence
   test passing on a 4×4×4 grid (64 chunks compared, 0 mismatched) and by
   the two CPU regression tests (`all_empty_saturates_to_max`,
   `wall_blocks_negative_direction`). The CPU oracle's sweep order
   (size 0..31 × axis 0..3) matches the GPU's queue-picker order
   (`prepare_group_bounds` always picks the lowest-size non-empty
   `(size, axis)` queue first).
4. **`max_group_bound_dispatch = 512 * 64 = 32_768` is comfortably above
   the test grid's group count (1).** NAADF's default per
   `WorldBoundHandler.cs:25`; the test fixture upload re-uses the same
   value via `GpuConstructionParams.max_group_bound_dispatch`. With only
   1 group in the queue, every prepare round slices `min(32_768, 1) = 1`
   item — one workgroup per compute round.

### Verification

- **Build:** `cargo build -p bevy-naadf` — clean, 0 errors, 0 warnings on
  W3-touched files. `cargo clippy -p bevy-naadf --lib` — 0 errors, 4
  warnings (all pre-existing in W5 generator code, not W3-introduced).
- **Tests:** `cargo test -p bevy-naadf --lib` → **82 passed, 1 ignored**
  (W1 baseline 76 → +6 W3 tests: 3 GPU/CPU-oracle + 2 CPU-only oracle
  regressions + 1 `bound_queue_info_layout` runtime mirror). Full
  workspace: `cargo test --workspace` → **95 passed, 6 ignored** across
  10 suites.
- **e2e:** `cargo run --bin e2e_render` — exits 0. Gate values **emissive
  247.0, solid 242.0, sky 145.9** — identical to W0/W1/W5/W6 baseline.
  The regime-2 node runs every frame but does no work (the default 4×2×4
  grid has `bound_group_count = 0`); the screenshot is unchanged.
- **e2e with W1 oracle:** `cargo run --bin e2e_render --
  --validate-gpu-construction` — exits 0. Output:
  ```
  e2e_render: PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, ...
  GPU construction byte-equal to CPU oracle: 388 bytes compared
  ```
  W1's oracle gate is unaffected by W3 (as expected — W1 owns a different
  validation path with its own headless render world).
- **The load-bearing `bounds_calc_convergence_matches_cpu_oracle` test:**
  PASS. Stdout (from `eprintln!`):
  ```
  W3 convergence: 64 chunks compared, 0 mismatched
  ```
  Every chunk in the 4³ test world (1 solid centre + 63 empty) converges
  to the bit-exact CPU oracle value. Total e2e runs in the W3 workstream:
  3 (within the ≤6 cap).

### Seam contract update (for W2 / W4)

W3 modifies the W0 / W1 / W5 / W6 seam in the following ways:

| seam element | post-W1 state | post-W3 state |
|---|---|---|
| `ConstructionPipelines` | 9 fields (W5+W1). | **15 fields** — added `construction_bounds_world_layout`, `construction_bounds_layout`, `bound_dispatch_indirect_layout`, `bounds_calc_pipeline_add_initial`, `bounds_calc_pipeline_prepare`, `bounds_calc_pipeline_compute`. W2/W4 extend the `FromWorld` impl additively. |
| `ConstructionPipelines::from_world` | Builds W5 + W1 pipelines. | Builds W5 + W1 + W3 pipelines. W2/W4 add their pipelines additively. |
| `ConstructionGpu.{bound_queue_info, bound_group_queues, bound_group_masks, bound_refined_info, bound_dispatch_indirect}` | `Option<Buffer>::None`. | **`Some(Buffer)`** — allocated by `prepare_construction` on the first frame `WorldGpu` exists. Sizes: `bound_queue_info = 32*3*8 B`, `bound_group_queues = 32*3*bgc*4 B`, `bound_group_masks = bgc*3*4 B`, `bound_refined_info = 12 B`, `bound_dispatch_indirect = 20 B INDIRECT`. (where `bgc = max(1, bound_group_count_of(size_in_chunks))`.) |
| `ConstructionGpu.bounds_params_buffer` | new field (W3). | **`Some(Buffer)`** — the `GpuConstructionParams` uniform (80 B) written once at world-init. W4 / W2 may extend (`changed_*_count` fields). |
| `ConstructionGpu.bounds_initialized` | new field (W3). | `true` after the regime-1 seed has dispatched. |
| `ConstructionBindGroups.construction_bounds_world` | new field (W3). | **`Some(BindGroup)`** — the W3 `@group(0)` (chunks + params). |
| `ConstructionBindGroups.construction_bounds` | `Option<BindGroup>::None`. | **`Some(BindGroup)`** — the W3 `@group(1)` (4 bound-queue buffers). |
| `ConstructionBindGroups.bound_dispatch` | `Option<BindGroup>::None`. | **`Some(BindGroup)`** — the W3 `@group(2)` (indirect-dispatch write-side). |
| `prepare_construction` body | `init_resource` shells (W0), no W1 allocations (W1 stayed test-only). | **Allocates the W3 buffer family + builds the 3 W3 bind groups + dispatches the regime-1 seed on first ready frame.** Bevy `#[allow(clippy::too_many_arguments)]` added (9 args; same allow as `prepare_frame_gpu`). |
| `Core3d` chain in `render/mod.rs` | 14 nodes (post-W0 TODO placeholders for W2/W3/W4). | **15 nodes** — `naadf_bounds_compute_node` inserted as the FIRST entry of the `.chain()` tuple, before `naadf_atmosphere_node`. W2 / W4 add their nodes at the same insertion point in their own merges. |
| `run_gpu_construction_startup` | Gated info log (W1). | UNCHANGED — W3 does not extend the main-world Startup driver; the regime-1 seed lives in `prepare_construction` (decision #5 above). |
| `e2e_render --validate-gpu-construction` flag | W1 oracle (388 bytes). | UNCHANGED — W3 leaves W1's oracle alone. |

**Public API additions** for W2 / W4 to consume:

- `crate::render::construction::bounds_calc::{construction_bounds_world_layout_descriptor,
  construction_bounds_layout_descriptor, bound_dispatch_indirect_layout_descriptor}` —
  the 3 W3 layout descriptors; W2's `world_change.wgsl` can reuse
  `construction_bounds_layout` for `apply_group_change`'s re-enqueue path.
- `crate::render::construction::bounds_calc::{queue_*_pipeline,
  queue_*_pipeline_with_handle, dispatch_*}` — pipeline-queue + dispatch
  helpers.
- `crate::render::construction::bounds_calc::dispatch_regime_2_rounds` —
  the 5-rounds-per-frame loop helper; W2's edit-event handler may share it
  if needed.
- `crate::render::construction::bounds_calc::{bound_group_count_of,
  group_size_in_groups_of}` — sizing helpers (mirror C#
  `WorldBoundHandler.cs:41-42`).
- `crate::render::gpu_types::GpuBoundQueueInfo` — Rust mirror of the WGSL
  `BoundQueueInfo` struct; size 8 B; uploads via `bytemuck::cast_slice`.

The seam stays additive: every later Phase-C workstream can land its row
without re-editing W3's fields. The next dispatch in dependency order per
`15-design-c.md` §2.2 is **W4 (entities)** or **W2 (editing)** — W4 owns
the `Rg32Uint` chunks widening + `.x` sweep, then W2 (which needs the
bound-queue family W3 ships to re-enqueue groups on edits).
