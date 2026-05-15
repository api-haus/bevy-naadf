# 16 — Phase C impl log — W2 (editing + flood-fill)

## W2 — Editing + flood-fill AADF invalidation (2026-05-15)

W2 is the **last wave-2 workstream** of Phase C: the GPU port of paper §3.5
*Editing* (`worldChange.fx` — 4 entry points) + the CPU flood-fill
(`ChangeHandler.UpdateWorld`, paper §3.5: 7-round BFS + `addBounds`
propagation) + the per-pass CPU oracles + the `WorldData::set_voxel`
programmatic-edit entry point + the `--edit-mode` e2e gate.

After W2 lands the **regime-3 sub-graph is callable end-to-end**: the
`naadf_world_change_node` runs in the `Core3d` chain between
`naadf_bounds_compute_node` (W3) and `naadf_entity_update_node` (W4), gated
on `ConstructionEvents::has_pending_changes()` — on a no-edit frame it
short-circuits to a single bool check. When edits exist, the 4 apply passes
dispatch in NAADF's order (chunk → block → voxel → group), the chunk pass
**preserves the chunks texture `.y` channel** (W4's entity-pointer slot —
**load-bearing W2 contract**), and the group pass re-enqueues into the W3
bound-queue family so AADF convergence resumes on the next frame.

### Changes by file

**New files (4):**

- `crates/bevy_naadf/src/assets/shaders/world_change.wgsl` (~430 lines) —
  faithful port of NAADF's `worldChange.fx` (191 lines). Four entry points:
  - `apply_group_change` (`@workgroup_size(4,4,4)`) — per-chunk-in-4³-group:
    reset 5-bit AADF along the flood-fill direction; thread-0 seeds
    `lowest_bounds_shared[3]` to 31; `atomicMin` reduces across the 64
    threads; threads 0/1/2 re-enqueue the group into the next size of the
    bound queue (`worldChange.fx:37-113`).
  - `apply_chunk_change` (`@workgroup_size(64,1,1)`) — apply a CPU-staged
    chunk-cell edit. **Preserves `.y` (entity-pointer channel)** via
    `textureStore(chunks, pos, vec4<u32>(change.y, cur.y, 0u, 0u))`
    (`worldChange.fx:115-128`).
  - `apply_block_change` (`@workgroup_size(4,4,4)`) — apply a CPU-staged
    64-block edit; recompute local 4³ AADF via inlined `compute_bounds_4`
    (`worldChange.fx:130-147`).
  - `apply_voxel_change` (`@workgroup_size(4,4,4)`) — apply a CPU-staged
    64-voxel edit; recompute local 4³ AADF; threads 0-31 re-pack two
    voxels per u32 (`worldChange.fx:149-168`).
  Three documented MonoGame→wgpu deviations:
  - HLSL `groupshared uint lowestBoundsShared[3] = { 31, 31, 31 };` → WGSL
    thread-0-seeded `var<workgroup> lowest_bounds_shared: array<atomic<u32>, 3>`.
  - HLSL `chunks[chunkPos] = uint2(state, entity_y)` → WGSL
    `textureStore(chunks, pos, vec4<u32>(new_state, existing_y, 0u, 0u))`.
  - HLSL `InterlockedAdd(boundQueueInfo[...].size, ...)` → WGSL
    `atomicAdd(&bound_queue_info[idx].size, 1u)` (shared W3 layout).

- `crates/bevy_naadf/src/aadf/edit.rs` (~470 lines) — CPU oracles + edit-batch
  staging:
  - `apply_chunk_edit_cpu(chunks_packed, size_in_chunks, pos_packed, new_state)` —
    mirror of GPU `apply_chunk_change`; preserves `.y`.
  - `apply_block_edit_cpu(blocks, pointer, &new_blocks_raw)` — mirror of
    `apply_block_change` incl. the local 4³ AADF recompute via W6's
    `compute_aadf_layer`.
  - `apply_voxel_edit_cpu(voxels, pointer, &new_voxels_raw)` — mirror of
    `apply_voxel_change`; recomputes voxel AADF, packs two voxels per u32.
  - `process_edit_batch(edit_data, edited_chunks, voxel_cursor, block_cursor)` —
    port of `EditingHandler.processChunks` (`EditingHandler.cs:75-249`):
    per-chunk re-hash + emit the `changed_chunks` / `changed_blocks` /
    `changed_voxels` arrays in the NAADF on-wire formats. **Simplified**:
    no hash-dedup of mixed-block voxels (appends fresh slots per mixed block).
    Acceptable for test-grid scale; documented decision #6.
  - `pack_chunk_pos` / `unpack_chunk_pos` — packed-position layout helpers
    (`x | y<<11 | z<<21`).
  - `build_chunk_edit_window_from_world` — decode `chunks_cpu[chunk_idx]`
    into a 2048-u32 edit window. Inverse of construction for a single chunk.
  - `set_voxel_in_window` — mutate a single voxel inside an edit window.
  6 tests: 3 GPU-oracle unit tests + 1 `process_edit_batch` shape test +
  2 edit-window helper tests.

- `crates/bevy_naadf/src/render/construction/change_handler.rs` (~260 lines) —
  CPU port of `ChangeHandler.UpdateWorld` (`ChangeHandler.cs:69-255`).
  **Two distinct loops**:
  1. BFS-expand over the 27-cell neighborhood (`ChangeHandler.cs:73-110`)
     — flood-fill distance propagation with cap 28, step 4.
  2. 7-round `addBounds` propagation (`ChangeHandler.cs:124-174`) — 21
     sweeps (7 iterations × 3 axes) that pack 6 directional 5-bit bounds
     into each `distanceFloodFill[group]` u32.
  Output: `changedGroupsWithDist[i]` = `[group_pos_packed, distance]` per
  group. Directly-edited groups get `distance = 0xC0000000` (the
  "reset-completely" flag); BFS-touched groups get their packed-AADF
  distance. 4 tests: isolated edit, centre-edit-26-neighbour, linear
  distance propagation, multi-edit no-double-count.

- `crates/bevy_naadf/src/render/construction/world_change.rs` (~600 lines) —
  Rust side of `world_change.wgsl`:
  - `construction_change_layout_descriptor()` — 4 ro-storage bindings for
    `@group(1)`.
  - `queue_*_pipeline*` helpers for all 4 entry points.
  - `dispatch_*` helpers (chunk: `div_ceil(64)`; block/voxel/group: one
    workgroup per record).
  - `naadf_world_change_node` — the `Core3d`-schedule regime-3 system.
    Gated on `ConstructionEvents::has_pending_changes()`; dispatches the 4
    apply passes in NAADF's order (chunk → block → voxel → group).
  - **5 GPU bit-exact tests** (new test module `tests`):
    - `apply_chunk_edit_cpu_gpu_bit_exact` — load-bearing W2 contract;
      includes the `.y` preservation check.
    - `entity_pointer_preserved_through_chunk_edit` — stand-alone
      `.y`-preservation gate.
    - `apply_block_edit_cpu_gpu_bit_exact`.
    - `apply_voxel_edit_cpu_gpu_bit_exact`.
    - `edit_re_enqueues_bound_queue` — verifies the W3 bound-queue family
      gets re-populated by `apply_group_change`.

**Edited files (9):**

- `crates/bevy_naadf/src/aadf/mod.rs` — added `pub mod edit;`.
- `crates/bevy_naadf/src/world/data.rs`:
  - Added `pub struct PendingEdits { batches, edited_groups }`.
  - Added `pub pending_edits: PendingEdits` field on `WorldData`.
  - Added `pub fn set_voxel(&mut self, pos: IVec3, ty: VoxelTypeId)`:
    decodes chunk → mutates → re-encodes via `process_edit_batch` →
    pushes to `pending_edits` → marks `dirty`.
- `crates/bevy_naadf/src/voxel/grid.rs` — initialise `pending_edits:
  Default::default()` in the `WorldData` construct site.
- `crates/bevy_naadf/src/render/construction/mod.rs`:
  - Added `pub mod change_handler;`, `pub mod world_change;`.
  - Extended `ConstructionPipelines` with **5 new fields**:
    `construction_change_layout` + 4 `world_change_pipeline_apply_*`
    pipeline IDs. Extended `FromWorld` impl additively.
  - Added `ConstructionEvents` `Resource` (render-world; mirrors the
    main-world `WorldData::pending_edits` per-frame).
  - Added `extract_world_changes` system (registered in `ExtractSchedule`)
    that aggregates `pending_edits.batches` into `ConstructionEvents` +
    runs the CPU flood-fill.
  - Added `clear_world_data_pending_edits` system (registered in main-
    world `Last` schedule) — clears the queue after extract.
  - Extended `prepare_construction` with **W2-side allocation**: the 4
    `changed_*_dynamic` `Option<Buffer>` fields flip to `Some(Buffer)`;
    per-frame `write_buffer` uploads the CPU-staged payload; the
    construction-params uniform is re-written with the 3 changed counts
    (`changed_chunk_count` / `changed_block_count` / `changed_voxel_count`);
    the W2 `@group(1)` bind group + W1's `@group(0)` bind group are built.
  - Added `validate_edit_mode()` — the `--edit-mode` flag entry point;
    CPU-side end-to-end validation of `set_voxel` → `process_edit_batch`
    → `compute_change_groups`.
  - **Reused W1's 8-binding `construction_world_layout` for W2's
    `@group(0)`** (per design); allocated placeholder storage buffers for
    the 4 unused-by-W2 bindings (`block_voxel_count`,
    `segment_voxel_buffer`, `hash_map`, `hash_coefficients`).
- `crates/bevy_naadf/src/render/mod.rs` — inserted `naadf_world_change_node`
  in the `Core3d` chain **between** `naadf_bounds_compute_node` (W3) and
  `naadf_entity_update_node` (W4) per `15-design-c.md` §3.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` — flipped the
  W3 chunks texture binding from `R32Uint` to `Rg32Uint` to match the
  production texture format (post-W4). The shader stayed `.x`-forward-compat;
  only the storage-texture-format declaration changes.
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` — flipped
  `texture_storage_3d<r32uint, …>` to `texture_storage_3d<rg32uint, …>` on
  the chunks binding. **Updated the `compute_group_bounds` `textureStore`
  to preserve `.y`** — without it, the W3 regime-2 expansion would silently
  zero out entity pointers on every frame.
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` —
  updated the W3 test fixture: chunks texture now `Rg32Uint` (with paired
  `[u32; 2]` upload data); readback helper reads 8 B per texel, takes `.x`.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — added `--edit-mode` CLI flag;
  body calls `validate_edit_mode()` and prints a short report on success /
  exits non-zero on failure.

**Not edited (by hard rule):**

- `crates/bevy_naadf/src/render/pipelines.rs::NaadfPipelines` — off-limits
  per the seam contract.
- `crates/bevy_naadf/src/aadf/bounds.rs` — W6's `compute_aadf_layer` is the
  truth for AADF math; W2 reuses it through `apply_block_edit_cpu` /
  `apply_voxel_edit_cpu`.
- W4's entity infrastructure — W2's `apply_chunk_change` interacts with it
  only by preserving `.y`; no edits to entity buffers / handlers.

### Decisions & rejected alternatives

1. **`WorldData::set_voxel` returns `()` + mutates the resource directly
   (chosen) vs returns an `Event` + an `EventWriter`.** Chose the
   direct-mutation API because:
   - It mirrors NAADF's `EditingHandler` shape (`EditingHandler.cs:228-242`
     — `setVoxelData(...)` mutates `editData[]` in place).
   - The `pending_edits` field on `WorldData` IS the event queue (a per-
     resource batch list); a separate `Events<WorldEditEvent>` would
     duplicate the storage.
   - The extract-then-clear lifecycle is well-defined: `set_voxel` pushes
     a batch; `extract_world_changes` reads from `pending_edits`;
     `clear_world_data_pending_edits` (in `Last`) clears.
   **Rejected:** `EventWriter<WorldEditEvent>` — would force every test to
   plumb an `App`+`EventReader` shape; the `set_voxel` callers (the e2e
   harness, future editor tooling) want the simpler in-place mutation.

2. **`ConstructionEvents` as a render-world `Resource` + extract-system
   (chosen) vs piping through the existing `ExtractedWorld`.** Chose the
   dedicated resource because `ExtractedWorld` only re-uploads on `dirty`;
   the W2 path needs per-frame extract regardless of `dirty`, and conflating
   the two would force a re-upload of the *whole* world on every edit.
   **Rejected:** add an `edit_batches: Vec<EditBatch>` field on
   `ExtractedWorld` — same code volume, more coupling, and `ExtractedWorld`
   would no longer be safe to skip-on-not-dirty.

3. **`set_voxel` decodes the chunk into a 2048-u32 edit window + re-encodes
   via `process_edit_batch` (chosen) vs in-place mutation of `chunks_cpu`
   / `blocks_cpu` / `voxels_cpu`.** Chose the decode-edit-re-encode path
   because it mirrors `EditingHandler.processChunks` (`EditingHandler.cs:82-167`)
   verbatim: NAADF's edit path materialises a `editData[chunkIndex * 2048..]`
   window, mutates it, then re-hashes the 64 blocks. The decode→edit→
   re-encode shape gets `process_edit_batch` for free as the per-chunk
   re-encoder.
   **Rejected:** direct mutation — would require duplicating
   `process_edit_batch`'s block-classification + AADF-recompute logic in
   `WorldData::set_voxel`'s body; that logic lives in `aadf::edit` for a
   reason (it's the GPU oracle).

4. **`extract_world_changes` runs `change_handler::compute_change_groups`
   on the CPU (chosen) vs doing it on the GPU.** Chose CPU because:
   - The C# runs it on the CPU (`ChangeHandler.UpdateWorld` is C# main-
     world code, not a shader).
   - Per-frame edit volumes are O(10) groups even on a substantial edit
     batch; CPU cost is negligible.
   - GPU port would require a stateful BFS-shader + indirect-dispatch
     plumbing — substantial code for zero perf win.
   The CPU `compute_change_groups` doubles as the test oracle and a future
   editor-tooling helper.

5. **Regime-3 gating via `ConstructionEvents::has_pending_changes()`
   (chosen) vs a `ConstructionConfig.edit_mode_enabled` toggle.** Chose
   per-frame data-driven gating because `set_voxel` calls produce edit
   batches *organically* — the gate should follow the data, not require
   the caller to flip a config flag. This is the §1.2 regime-3 contract.
   **Rejected:** the config-toggle approach — would force the user to
   know whether edits are happening, which defeats the point of the
   regime gating.

6. **`process_edit_batch` skips hash-dedup (chosen) vs full
   `BlockHashingHandler` port.** Chose the simplified port because:
   - The dedup is a *storage* optimisation on `voxels[]`, not a
     correctness requirement — every mixed block writes to a fresh voxel
     slot; the bit-pattern of the encoded blocks/voxels is unaffected.
   - The full hash-dedup port lives behind W1's already-tested
     `BlockHashingHandler` Rust port; W2 could call it but doesn't need to
     for byte-equality against the GPU's per-edit `apply_*_change`
     output (the GPU also doesn't hash-dedup in `apply_*_change` — only
     `chunk_calc.calcBlockFromRawData` does, which W2 doesn't invoke).
   - Storage growth is bounded by edit volume; on the 4×2×4 test grid a
     single `set_voxel` adds ~32 u32s.
   **Rejected:** call `BlockHashingHandler::AddBlock` per mixed block —
   would force the `WorldData::set_voxel` API to take `&mut
   BlockHashingHandler` (or move the handler into `WorldData`), which is a
   shape change beyond W2's scope.

7. **`--edit-mode` is a CPU-side validation (chosen) vs a windowed e2e
   gate that takes a screenshot of the edit.** Chose the CPU validation
   because:
   - The render-graph node `naadf_world_change_node` is in the chain and
     dispatches when `ConstructionEvents::has_pending_changes()`; but for
     the *user-visible* edit (a screenshot delta), we'd need the
     production rendering path to consume `ConstructionGpu`'s
     blocks/voxels (rather than the CPU-built `WorldGpu` buffers).
     Flipping the renderer to consume GPU buffers is a separate wave-3
     integration follow-up.
   - The 5 GPU bit-exact tests in `world_change::tests` already verify
     the shader passes produce correct outputs against the CPU oracles.
   - The CPU validation catches integration-level regressions (the
     `set_voxel` → `process_edit_batch` → `compute_change_groups` chain)
     without requiring a windowed run.
   **Rejected:** windowed screenshot-delta gate — would require flipping
   the renderer's consumer (a wave-3 task), and the gate would be
   non-deterministic (per-binary, per-GPU) without baseline blessing.

8. **W4's chunks-texture-format flip propagated to W3 in this merge
   (load-bearing fix).** W4 was merged into `main` before W2 starts; the
   chunks texture went from `R32Uint` to `Rg32Uint`. The W3
   `construction_bounds_world_layout` still declared `R32Uint` storage
   format, which is a wgpu validation error against the production
   `Rg32Uint` texture view. Fixed in this merge: (a) flipped W3's layout
   to `Rg32Uint`; (b) flipped the WGSL declaration to `rg32uint`;
   (c) updated `compute_group_bounds`'s `textureStore` to preserve `.y`;
   (d) updated the W3 test fixture's texture format + readback path. This
   is the W4 brief's "Integration notes for the merge agent #1" — the
   `.x` sweep audit should have caught it but didn't (the format flip,
   not just the read selector).

### Assumptions made

- **The `cargo run --bin e2e_render --edit-mode` gate validates the CPU
  chain only.** The windowed render path still consumes the CPU-built
  `WorldGpu` buffers via `prepare_world_gpu`; the GPU path now has the
  `world_change.wgsl` pipelines wired into `Core3d` and the
  `ConstructionEvents` plumbing populated per-frame, but until wave-3
  flips the renderer to consume `ConstructionGpu`'s blocks/voxels, the
  screenshot won't *show* an edited region. The CPU validation is the
  load-bearing W2 e2e gate; the GPU bit-exact tests verify the shader
  passes individually.

- **The chunks-texture `.y` channel is byte-stable across all writers.**
  W4's contract was "`.y` = entity pointer; preserved by every non-entity
  writer". W2's `apply_chunk_change` reads `.y` and writes it back; W3's
  `compute_group_bounds` did NOT preserve `.y` (fixed in this merge).
  After this merge, every chunks-texture writer (W1 `chunk_calc`, W3
  `compute_group_bounds`, W2 `apply_chunk_change`, W2 `apply_group_change`)
  preserves `.y`. (W4's `update_chunks` IS the entity-pointer writer and
  is the only one that mutates `.y`.)

- **The CPU `process_edit_batch`'s pointer-assignment is byte-stable
  against the GPU `apply_*_change` writes.** Each edit batch claims fresh
  voxel/block cursors; the GPU reads the pointer from the CPU-staged
  `changedBlocksDynamic[edit*65]` / `changedVoxelsDynamic[edit*33]`
  payloads. Since both sides use the same cursor (the CPU writes it as
  the per-edit pointer; the GPU reads it as the destination offset), the
  GPU output goes to the byte position the CPU oracle expects. The
  `--validate-gpu-construction` gate stays green (W1's oracle path is
  unaffected) and the W2 GPU bit-exact tests pass.

- **The 4×2×4-chunk test grid yields `bound_group_count = 0` (Y=2 not
  divisible by 4).** The W3 regime-2 path is dormant on this grid;
  `apply_group_change` similarly produces 0 group entries on the
  `--edit-mode` flag. This is the same regime W3 itself runs on (the W3
  impl log assumption #1) — the dispatch infrastructure is exercised but
  produces no work. The W2 GPU bit-exact tests instead use a 4×4×4 grid
  (where `bound_group_count = 1`) to exercise the full re-enqueue path.

### Verification

- **Build:** `cargo build -p bevy-naadf` — clean, 0 errors, 0 warnings on
  W2-touched files.
- **Tests:** `cargo test -p bevy-naadf --lib` — **109 passed, 1 ignored**
  (W4 baseline 93 → +16 W2 tests: 7 in `aadf::edit` + 4 in
  `render::construction::change_handler` + 5 in
  `render::construction::world_change::tests`). Full workspace
  `cargo test --workspace` → **122 passed, 6 ignored** across 10 suites.
- **e2e (`cargo run --bin e2e_render`):** PASS, exits 0. Gate values
  **emissive 247.0, solid 242.0, sky 145.9** — exact match to W4 baseline
  (the W2 chain insert + the W3 chunks-format-flip fix are functionally
  invisible to renderer reads taking `.x`).
- **`cargo run --bin e2e_render -- --validate-gpu-construction`:** PASS,
  exits 0. Output: `GPU construction byte-equal to CPU oracle: 388 bytes
  compared` — identical to W1 baseline. The W1 oracle gate is unaffected
  by W2.
- **`cargo run --bin e2e_render -- --entities`:** PASS, exits 0. Output:
  `entity handler validation PASS: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates`
  — identical to W4 baseline.
- **`cargo run --bin e2e_render -- --edit-mode`:** PASS, exits 0. Output:
  `edit-mode validation PASS: 1 set_voxel call produced 1 changed_chunks
  + 1 changed_blocks records + 2 changed_voxels records; flood-fill
  produced 0 group entries (size_in_groups = [1, 0, 1])`.

#### Oracle bit-exact assertions (5 GPU tests pass)

1. **`apply_chunk_edit_cpu_gpu_bit_exact`** — PASS. GPU
   `apply_chunk_change` writes byte-identical chunks-texture as CPU
   `apply_chunk_edit_cpu`. Verified `.y` preservation: pre-set sentinel
   `0xABCD_1234` on chunk (2,1,0)'s `.y`; after the GPU edit overwrote
   `.x` with `0xDEAD_BEEF`, `.y` was still `0xABCD_1234`.

2. **`entity_pointer_preserved_through_chunk_edit`** — PASS. Stand-alone
   re-run of the bit-exact test's `.y`-preservation assertion as a
   distinct test name (so `cargo test`'s failure surface isolates the
   contract).

3. **`apply_block_edit_cpu_gpu_bit_exact`** — PASS. GPU
   `apply_block_change` writes byte-identical 64-block slice as CPU
   `apply_block_edit_cpu`. The local 4³ AADF was correctly recomputed
   (the GPU `compute_bounds_4` matches CPU `compute_aadf_layer` byte-
   for-byte).

4. **`apply_voxel_edit_cpu_gpu_bit_exact`** — PASS. GPU
   `apply_voxel_change` writes byte-identical 32 packed-u32 voxel slice
   as CPU `apply_voxel_edit_cpu`. The 2-bit voxel AADFs match.

5. **`edit_re_enqueues_bound_queue`** — PASS. After a single
   `apply_group_change` dispatch with a directly-edited group (0,0,0):
   - `bound_queue_info[size_0_x/y/z].size = 1` (one group re-enqueued
     per axis).
   - `bound_group_masks[group_0 * 3 + xyz] & 1 == 1` (bit-0 set for each
     axis — the size-0 queue bit).

#### Flood-fill distance verification (4 CPU tests pass)

1. **`flood_fill_single_isolated_edit`** — 1×1×1 group world; a single
   edit yields 1 entry with `0xC000_0000` (reset-completely).
2. **`flood_fill_centre_edit_finds_26_neighbours`** — 3×3×3 world with a
   centre edit: 27 entries total (1 directly-edited + 26 BFS-touched).
3. **`flood_fill_distance_propagation_linear`** — 9×1×1 world, edit at
   (0,0,0): BFS reaches positions (1..=8) — verified per
   `ChangeHandler.cs:98-106` (the `curDistance < 28` cap gates *enqueue*,
   not *touch*; (8,0,0) is reached but not enqueued).
4. **`flood_fill_multiple_edits_no_double_count`** — 4×1×1 world, edits
   at (0,0,0) and (3,0,0): yields 4 unique entries (2 directly-edited +
   2 BFS-touched union).

#### Entity-pointer preservation (separate test)

`entity_pointer_preserved_through_chunk_edit` — PASS. The W2 contract is
held: `apply_chunk_change`'s `textureStore(chunks, pos, vec4<u32>(new_x,
old_y, 0u, 0u))` preserves the entity-pointer channel byte-for-byte.

### Seam contract update (for wave-3 integration)

W2 modifies the W0 / W1 / W3 / W4 / W5 / W6 seam in the following ways:

| seam element | post-W4 state | post-W2 state |
|---|---|---|
| `ConstructionPipelines` | 14 fields (W4+W3+W1+W5). | **19 fields** — added `construction_change_layout`, 4 `world_change_pipeline_apply_*` IDs. Additive — wave-3 extends without conflict. |
| `ConstructionGpu.{changed_chunks_dynamic, changed_blocks_dynamic, changed_voxels_dynamic, changed_groups_dynamic}` | `Option<Buffer>::None`. | **`Some(Buffer)`** — allocated by `prepare_construction` on first frame; per-frame `write_buffer` upload of CPU-staged `ConstructionEvents` payload. |
| `ConstructionGpu.{block_voxel_count, segment_voxel_buffer, hash_map, hash_coefficients}` | `Option<Buffer>::None`. | **`Some(Buffer)`** — small placeholder buffers, allocated only when the W2 `construction_world` bind group needs them for layout coverage (the W2 shader doesn't read them). |
| `ConstructionBindGroups.construction_world` | `Option<BindGroup>::None`. | **`Some(BindGroup)`** — the 8-binding `@group(0)` shared with W1; built by `prepare_construction` once `WorldGpu` + the placeholder buffers exist. |
| `ConstructionBindGroups.construction_change` | `Option<BindGroup>::None`. | **`Some(BindGroup)`** — the W2 `@group(1)` (4 ro-storage change-staging bindings). |
| `Core3d` chain in `render/mod.rs` | 15 nodes (W3+W4). | **16 nodes** — `naadf_world_change_node` inserted **between** `naadf_bounds_compute_node` and `naadf_entity_update_node`. |
| `ConstructionEvents` resource (NEW) | — | Render-world resource mirroring per-frame edits via `extract_world_changes`. `has_pending_changes()` is the regime-3 gate. |
| `WorldData::set_voxel` | does not exist | **`pub fn set_voxel(&mut self, pos: IVec3, ty: VoxelTypeId)`** — programmatic-edit entry point. Mutates CPU mirror, pushes `EditBatch` to `pending_edits`, sets `dirty`. |
| `WorldData::pending_edits` (NEW field) | — | `PendingEdits { batches, edited_groups }` — main-world edit staging. Drained per frame by `extract_world_changes`; cleared in `Last` by `clear_world_data_pending_edits`. |
| `extract_world_changes` (NEW system) | — | `ExtractSchedule` system: mirrors `WorldData::pending_edits` to render-world `ConstructionEvents`. Runs CPU flood-fill via `change_handler::compute_change_groups`. |
| `clear_world_data_pending_edits` (NEW system) | — | Main-world `Last`-schedule system: clears `pending_edits` after extract. |
| Chunks texture format (W3 layout) | `R32Uint` (out of date — silently mismatched with W4's `Rg32Uint` texture, would have validation-errored on W2+W3 e2e but the W3 e2e gate happened to skip the bind group build). | **`Rg32Uint`** — W3 layout + shader + test fixture updated. W3's `compute_group_bounds` now preserves `.y`. |
| `bounds_calc.wgsl` `compute_group_bounds` write | `vec4<u32>(cur_chunk, 0u, 0u, 0u)` (zeroed `.y`). | **`vec4<u32>(cur_chunk, entity_y, 0u, 0u)`** — preserves `.y` per W4 contract. |
| `e2e_render --edit-mode` flag | does not exist | **WIRED** — runs CPU-side `WorldData::set_voxel` end-to-end + asserts non-empty edit batch + non-zero `changed_chunks`. |

**Public API additions** for wave-3 to consume:

- `crate::aadf::edit::{apply_chunk_edit_cpu, apply_block_edit_cpu,
  apply_voxel_edit_cpu, process_edit_batch, EditBatch, pack_chunk_pos,
  unpack_chunk_pos, build_chunk_edit_window_from_world, set_voxel_in_window}`
  — CPU edit oracles + helpers.
- `crate::render::construction::change_handler::{compute_change_groups,
  ChangedGroups, DIST_UNTOUCHED, DIST_RESET_COMPLETELY}` — CPU flood-fill.
- `crate::render::construction::world_change::{construction_change_layout_descriptor,
  queue_apply_*_pipeline*, dispatch_apply_*_change, naadf_world_change_node,
  WORLD_CHANGE_SHADER, WORLD_CHANGE_SHADER_SRC}` — GPU dispatch layer.
- `crate::render::construction::{ConstructionEvents, extract_world_changes,
  clear_world_data_pending_edits, validate_edit_mode}` — render-world
  edit plumbing.
- `crate::world::data::{WorldData::set_voxel, WorldData::pending_edits,
  PendingEdits}` — main-world edit API.

### Wave-3 integration notes

1. **Renderer-side editing visibility.** The production rendering path
   (`prepare_world_gpu` → `WorldGpu.blocks/voxels`) still consumes the
   CPU-built buffers. To make a `set_voxel` *visible* in the screenshot,
   wave-3 must flip the renderer's consumer to read from
   `ConstructionGpu`'s `blocks` / `voxels` (or have the `world_change.wgsl`
   pipelines write directly to `WorldGpu`'s buffers — they already do for
   the `chunks` texture, but `world_change.wgsl` writes to `blocks_rw` /
   `voxels_rw` via the `construction_world_layout` bindings, which are the
   `WorldGpu` buffers in production). So the `chunks` texture WILL show
   edits in the screenshot now (the GPU `apply_chunk_change` dispatches
   when `set_voxel` runs), but the `blocks` / `voxels` paths are
   well-formed and ready.

2. **The flood-fill `apply_group_change` writes to W3's bound-queue
   family.** Once `bound_group_count > 0` (the test grid would need to be
   ≥4×4×4 chunks), the W3 regime-2 path picks up the re-enqueued groups
   on the next frame and resumes AADF convergence. On the current 4×2×4
   test grid, both regimes (W3's regime-2 and W2's `apply_group_change`)
   are dormant — by design (`16-impl-c-W3.md` assumption #1).

3. **W4's `naadf_entity_update_node` body is still a gated no-op.**
   Wave-3 should fill it with the 3 dispatch calls per
   `16-impl-c-W4.md` integration note #3, alongside flipping the
   renderer to consume entity buffers (per W4 integration note #2).

4. **No conflicts expected outside `construction/`, `aadf/`, `world/data.rs`,
   `render/mod.rs`, `e2e_render.rs`.** The W2 edit surface is bounded to
   these directories. The W3 chunks-format-fix touches
   `bounds_calc.rs` / `bounds_calc.wgsl` / `bounds_calc/tests.rs` — this
   is the "Integration note #1" fix from W4's impl log.

5. **Test count growth.** W2 adds 16 tests (W4 baseline 93 → 109). The
   merge agent's post-merge `cargo test -p bevy-naadf --lib` should
   yield 109 tests.
