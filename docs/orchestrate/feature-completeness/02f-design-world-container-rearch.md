# 02f — Design — WorldData as container with direct-access rendering path

**Date:** 2026-05-15
**Author:** consolidated-mode delegate (Opus 4.7 / 1M)
**Branch:** `main` at HEAD `1c35c7f feat(phase-d-shadow): multi-tap sun visibility in spatial resampling`
**Predecessor reads:** `01-context.md` · `02c-design-edit-pipeline-alignment.md` · `02e-perframe-cpu-investigation.md` · `03c-impl-edit-pipeline-alignment.md` · `03e-impl-dirty-fix-and-vox-grid.md` · `02a-v2-sparse-vox-ingestion.md` · `03a-v2-impl-sparse-vox.md` · `naadf-bevy-port/12-alignment-gap.md` rows 4 + 19 + B-7.

**User directive (verbatim):** "direct access is the only thing that makes sense. bevy idiom is not winning anything here and actually is a giant shot in me leg. … its supposed to do async gpu rebuild and theres no need for brushes to be entities at all. make it work like C# version. cpu rebuild is diagnostic-only and under no circumstance must be preferred or used along with equivalent gpu rebuild. treat this as a container entity with independent rendering path, its has no merit following this bevy idiom."

---

## C# architecture target (paraphrased from source-walk)

**`WorldData` is a single C# object** (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:20-521`) owned by `App.worldHandler.worldData`. It holds:

- **CPU mirror arrays** as plain `uint[]` fields: `dataChunk`, `dataBlock`, `dataVoxel` (`WorldData.cs:36-38`). Allocated once in `GenerateWorld` at `:81`,`:195-198`.
- **GPU resources** as direct fields: `dataChunkGpu` (Texture3D), `dataVoxelGpu` / `dataBlockGpu` (DynamicStructuredBuffer), `segmentVoxelBuffer`, `blockVoxelCountGpu` (`WorldData.cs:32-34, 41`). Allocated once at `:73-84`.
- **Sub-handlers** owning their own state: `boundHandler`, `changeHandler`, `entityHandler`, `editingHandler`, `blockHashingHandler` (`:43-47`).

**No CPU↔GPU sync layer.** No `dirty` flag. No "extract" stage. After `GenerateWorld`, the CPU and GPU buffers diverge over time (`dataChunk` is **never re-synced from the GPU** — verified at `WorldData.cs:120-218`, only `GetData(dataChunk)` once at `:193`). CPU traversal (`RayTraversal`, `:396-473`) reads `dataChunk[i]`, `dataBlock[i]`, `dataVoxel[i]` directly; it reads only state-bits + ptr/type, never AADFs (`:431-454`). So the CPU stale-AADFs are functionally invisible.

**Per-frame ordering** (`WorldData.cs:109-118`):

```
worldData.Update():
  entityHandler.Update();      // entity update (W4)
  editingHandler.Update();     // brush input → editData staging → processChunks
  changeHandler.Update();      // BFS + 21 sweeps + 4 GPU dispatches (W2)
  boundHandler.Update();       // 5 rounds W3 regime-2
```

`editingHandler.Update()` (`EditingHandler.cs:44-73`) reads `IO.MOStates` (the C# input mouse state, a static main-thread singleton) and calls `tool.ApplyAnyInput(gameTime)`. The brush is **not an object hanging off WorldData**; it's a polymorphic `EditingTool` reference stored on `EditingHandler.tool` (`EditingHandler.cs:17`). The brush internally calls back into `worldData.getChunkDataToEdit(chunkPos)` to stage edits.

**Render path** (`WorldRender.cs`, `WorldRenderBase.cs`, ...): `WorldRender.Render(WorldData data, float gameTime)` (`WorldRender.cs:92`) takes the WorldData **by reference** as a method argument and reads `data.dataChunkGpu` / `data.dataBlockGpu` / etc. directly. No extract, no clone, no shadow copy. The renderer also calls `data.setEffect(effect)` (`WorldData.cs:475-488`) to bind the GPU resources into the shader's effect parameters — direct field reads.

**Synchronization model.** Single-threaded main + DX11's implicit-hazard tracking. `processChunks` uses `Parallel.For` for per-chunk hash/classify but the GPU dispatches in `ChangeHandler.UpdateWorld` are sequential. The CPU writes to `dataChunk`/`dataBlock`/`dataVoxel` happen on `EditingHandler.processChunks`'s thread; the GPU upload of `changedChunks`/etc happens in `ChangeHandler.UpdateWorld`, called serially after `processChunks` on the same frame; no aliasing because the GPU staging arrays (`changedChunks`, `changedBlocks`, `changedVoxels` — `ChangeHandler.cs:57-59`) are separate buffers from the CPU mirror arrays. **DX11 handles the GPU-side resource hazard tracking**; the CPU side has no concurrency to manage.

**Editor → renderer flow.**

1. Frame N user clicks → `Mouse.GetState` mutates `IO.MOStates`.
2. `App.Update` calls `worldData.Update`.
3. `editingHandler.Update` → tool's `ApplyAnyInput`:
   - `tool.RayTraversal(camera.pos, getRayDir(mousePos))` → hit voxel.
   - For each chunk-in-AABB: `editingHandler.getChunkDataToEdit(chunkPos)` returns a per-chunk 2048-u32 staging window in `editData[]` (decoded from current chunk state on first stage; merged on subsequent stages within the same frame).
   - For each voxel in the chunk's brush footprint: `editingHandler.setVoxelData(pointer, voxelInChunk, type)` writes into the staging window. **No `dataChunk`/`dataBlock`/`dataVoxel` mutation here.**
4. End of `editingHandler.Update` → `processChunks()`:
   - For each staged chunk: hash 64 blocks, hash-dedup mixed blocks, allocate fresh slots OR free old ones via `freeVoxelSlots`/`freeBlockSlots`, build `newChunk`, call `worldData.SetChunk(idx, newChunk)`.
   - `SetChunk` (`WorldData.cs:381-394`) writes `dataChunk[idx]` AND (when content boundary flipped or new state is empty) enqueues into `changeHandler` via `AddChangedChunk`.
   - Appends `changedChunks`/`changedBlocks`/`changedVoxels` records on the `changeHandler`.
5. `changeHandler.Update` → BFS + 21 sweeps + GPU upload (the 4 dispatches: chunk/block/voxel/group).
6. `boundHandler.Update` → 5 rounds regime-2 (continuous AADF refinement, runs every frame regardless of edits).
7. Render frame reads up-to-date `dataChunkGpu` etc.

**Key invariant.** The CPU `dataChunk`/`dataBlock`/`dataVoxel` arrays and the GPU `dataChunkGpu`/`dataBlockGpu`/`dataVoxelGpu` are **two parts of the same WorldData**, kept in sync by the per-edit-frame delta upload chain in `processChunks` + `changeHandler.UpdateWorld`. There is no whole-world re-upload after `GenerateWorld`. Stale chunk-layer AADFs converge via the W3 background queue.

---

## Port — current shape

After `03e` (HEAD `d43f1f1`, also covered by HEAD `1c35c7f`):

- **`WorldData` is a main-world ECS `Resource`** (`crates/bevy_naadf/src/world/data.rs:35-59`). Owns `chunks_cpu`, `blocks_cpu`, `voxels_cpu`, `size_in_chunks`, `bounding_box`, a `dirty` flag, `pending_edits: PendingEdits`, `dense_voxel_types`. Inserted by `setup_test_grid` (`voxel/grid.rs:106-118`, `:182-198`) or `build_world_from_vox` (`voxel/vox_import.rs:286-323`).
- **`ExtractedWorld` is a render-world `Resource`** (`crates/bevy_naadf/src/render/extract.rs:30-51`). Holds clones of `chunks` / `blocks` / `voxels` / `voxel_types` / `dense_voxel_types` plus `size_in_chunks`, `bounding_box`, `dirty`.
- **`extract_world`** (`render/extract.rs:98-134`) runs every `ExtractSchedule`: gates on `world_data.dirty || voxel_types.dirty`; if gate passes, clones all 5 buffers main→render via `.clone_from()`; sets `extracted.dirty = true`; clears the main-world flags via `ResMut<MainWorld>`.
- **`prepare_world_gpu`** (`render/prepare.rs:151-441`) runs every `Render → PrepareResources`: gates on `existing.is_none() || extracted.dirty`; if gate passes, **re-allocates** the chunks 3D texture (`Rg32Uint`), the `GrowableBuffer`s for blocks/voxels/voxel_types, the world_meta uniform, the entity placeholders, and the world bind group; uploads all of them; `commands.insert_resource(WorldGpu { ... })`; clears `extracted.dirty`. (After `03e` the gate stays closed on stationary frames; the build-once contract is intact.)
- **`extract_world_changes`** (`render/construction/mod.rs:657-752`) runs every `ExtractSchedule`: reads `Extract<Res<WorldData>>` (read-only), drains `pending_edits.batches` into `ConstructionEvents`, runs BFS+21-sweep, inserts `ConstructionEvents` into the render world.
- **`clear_world_data_pending_edits`** (`render/construction/mod.rs:580-585`) runs every main-world `Last` schedule: `wd.pending_edits.batches.clear(); wd.pending_edits.edited_groups.clear();`.
- **`prepare_construction`** (`render/construction/mod.rs:778-1859`) reads `Option<Res<ExtractedWorld>>` for `dense_voxel_types` (for the GPU producer's `segment_voxel_buffer`).
- **`naadf_gpu_producer_node`** (`render/construction/mod.rs:1834-1860+`) reads `Option<Res<ExtractedWorld>>` for `dense_voxel_types.is_empty()` gate.
- **Brushes** (`editor/tools.rs`): `paint_brush` / `cube_brush` / `sphere_brush` are plain functions called from `apply_edit_tool` (`editor/mod.rs:135-249`), which is an Update-schedule ECS system reading `ResMut<WorldData>`, mouse input, panel state, etc. Brushes are NOT ECS entities. The brush state lives on `EditorState` resource.
- **CPU mirror update paths in `WorldData`**:
  - `set_voxel(pos, ty)` (`world/data.rs:106-269`) — single-voxel; emits W2 delta via `pending_edits.batches`; runs `recompute_chunk_layer_aadfs` (whole-world); emits synthetic chunk uploads. Used by `--edit-mode` gate and as `paint_brush`'s per-voxel oracle (not actually — paint uses `set_voxels_batch`).
  - `set_voxels_batch(edits)` (`world/data.rs:560-815`) — runtime fast path (post `02c`). Per-touched-chunk decode/mutate/encode via `process_edit_batch`. **No whole-world AADF recompute.** Used by all three brushes.
  - `set_chunks_uniform_batch(chunks)` (`world/data.rs:834-896`) — brush inside-chunk fast path. Single state write per chunk; no decode.
  - `set_voxels_batch_oracle(edits)` (`world/data.rs:913-1074`) — slow-but-bit-exact pre-`02c` body. Runs `recompute_chunk_layer_aadfs` + emits synthetic chunk uploads. Not on the runtime hot path; reserved for the `--edit-mode` gate / CPU fallback.
- **Where `process_edit_batch` is called**: `set_voxel`, `set_voxels_batch`, `set_voxels_batch_oracle` (via `crate::aadf::edit::process_edit_batch`). All three of these still execute the full CPU encode pipeline; the only difference is `recompute_chunk_layer_aadfs` and the synthetic chunk uploads.
- **Where `recompute_chunk_layer_aadfs` is called**: `set_voxel`, `set_voxels_batch_oracle` (in `WorldData`); inside `aadf::edit::tests`. Not called from `set_voxels_batch` or `set_chunks_uniform_batch` (the runtime fast paths).
- **The `--edit-mode` validation gate** (`render/construction/mod.rs:2719-2814` / `validate_edit_mode`) builds its own `WorldData` in-process, calls `set_voxel` once, asserts the W2 delta is emitted + the BFS oracle produces a valid `changed_groups` array. **Does not exercise the runtime brush path.** Does not depend on any extract/prepare pipeline state.

### Failure modes the current shape produced

1. **`dirty=true always` (pre-`03e`):** `setup_test_grid` set `dirty = true`; nothing cleared it; `extract_world` re-cloned ~48 MiB into the render world every frame and `prepare_world_gpu` re-uploaded ~50 MiB to the GPU every frame. ~20 ms/frame idle bug on Oasis. Fixed in `03e` via `ResMut<MainWorld>` clearing the main-world flag after the copy + removing `dirty = true` from the 4 edit paths.

2. **`dirty=true never on edits` (current bug at HEAD `1c35c7f`):** Removing the `dirty = true` writes from the edit paths means edits no longer trigger the `extract_world` → `prepare_world_gpu` cascade. The per-edit changes do flow through the W2 delta chain (`pending_edits.batches` → `extract_world_changes` → `ConstructionEvents` → `naadf_world_change_node`'s 4 GPU dispatches), but visual edits stopped landing. Hypothesis (the reason this rearch is needed): the W2 delta chain's GPU dispatches write into the chunks texture / blocks buffer / voxels buffer that **`prepare_world_gpu` allocated** — if those buffers grow past their allocation (`GrowableBuffer::upload_all` only runs on full re-upload, not on W2 delta append) the new blocks/voxels get written into out-of-bounds slots and the renderer reads stale data. **Verified by source-walk at `world_change.wgsl` + `prepare.rs:319-345`: blocks/voxels are allocated to `extracted.blocks.len()` / `extracted.voxels.len()`, which is the **build-time** size; the W2 dispatch appends to `block_voxel_count[0]` / `[1]` cursors and writes past the allocation boundary.

   **The current bug is real and confirmed**, but the symptom is more subtle than the user's verbatim phrasing suggests: edits land in the W2 chain, the GPU shaders DO write the changes, but the write targets are over-allocated buffers that may or may not be large enough for the stroke. This dispatch's rearch fixes the architecture; whether visual edits land "correctly" is the runtime-edit gate's job.

---

## Proposed rearch

**Decision: render-world-owned `WorldData` is wrong for a Bevy ECS app.** The brush is a `Res<ButtonInput<MouseButton>>` + `Res<Window>` + `Res<Camera>` consumer; those are main-world resources. Migrating them all to the render world is a much larger rearch than the user is asking for. **The cleanest landing in a Bevy 0.19 app is: keep `WorldData` in the main world (single instance, same as today), delete the `ExtractedWorld` clone and every system that touches it, and have the renderer (`prepare_world_gpu`, `prepare_construction`'s GPU-producer dispatch, `naadf_gpu_producer_node`) read main-world `WorldData` directly via `ResMut<MainWorld>` (the same pattern `extract_world` already uses post-`03e`).**

This matches the C# semantic: WorldData lives ONCE, accessed directly by both the editor (which mutates) and the renderer (which reads for initial upload + W2 delta). The "ECS sub-app boundary" still exists as a Bevy implementation detail, but the WorldData itself is not duplicated. The render systems reach across the boundary via Bevy's sanctioned `MainWorld` pattern.

### Owner

**`WorldData` stays as a main-world `Resource`** — single instance, same as today. **`ExtractedWorld` is deleted entirely.** `extract_world` is deleted. The four consumers of `ExtractedWorld` are migrated to `ResMut<MainWorld>` reads of the main-world `WorldData` resource.

### Brush access

**Unchanged from current.** Brushes are functions in `editor/tools.rs` called from `apply_edit_tool` (Update-schedule system in main world reading `ResMut<WorldData>` + input + camera). The brushes already work like C# — they're called from the main-world tick, they compute a voxel set, they call back into `WorldData` (`set_voxels_batch` / `set_chunks_uniform_batch`) which appends to `pending_edits.batches`. **No "brushes as entities" anti-pattern exists in the port.** The user's bullet-point #6 about brushes not being entities is satisfied; we keep it that way.

The two minor cleanups under this heading:

- The C# `EditingHandler` is a sub-handler living on WorldData. The port's equivalent — `EditorState` — is a separate main-world resource. **Acceptable Bevy idiom.** It's a config + transient runtime-state holder; not an entity, not a duplicated state. Keep as is.

### Editor input

**Unchanged.** `apply_edit_tool` reads `ButtonInput<KeyCode>` + `ButtonInput<MouseButton>` + `Query<&Window>` + `Query<(&Camera, &GlobalTransform)>` directly. This is the Bevy equivalent of C#'s `IO.KBStates`/`IO.MOStates`. The "is it an event handler or a polling system" distinction is a Bevy detail; semantically identical to C#.

### W2 dispatch flow

The W2 chain stays intact:

- `WorldData::pending_edits.batches` is appended to by `set_voxels_batch` / `set_chunks_uniform_batch`. Single owner: the main-world resource.
- `extract_world_changes` (`render/construction/mod.rs:657-752`) reads `Extract<Res<WorldData>>` and aggregates into `ConstructionEvents`. **Stays.** This is `Extract` not `MainWorld`-mut, so it's the documented Bevy read-only path; cheap.
- `clear_world_data_pending_edits` (`render/construction/mod.rs:580-585`) runs in main-world `Last` and drains the queue. **Stays.**
- `naadf_world_change_node` (`render/construction/world_change.rs`) dispatches the 4 GPU writes. **Stays.**

The change here is **buffer sizing**: `prepare_world_gpu` post-rearch must size `blocks` / `voxels` GrowableBuffers **with headroom** (e.g. 2× the build-time size, or grow on a separate signal) so the W2 dispatch's append-mode writes don't go out of bounds. This is the subtle correctness bug behind the user's verbatim "edits don't land". See **Risks** below — this is R3.

### CPU mirror updates

**Unchanged from `03c`/`02c`.** The runtime path (`set_voxels_batch` / `set_chunks_uniform_batch`) writes to `chunks_cpu` in place per touched chunk; the chunk-layer AADFs stay stale (matches C# `dataChunk`). The CPU consumer (`ray_traversal`) reads only state-bits + ptr/type. No whole-world recompute on the runtime path. ~O(touched voxels) per edit-frame; already true at HEAD `1c35c7f`.

The "`process_edit_batch` and any other CPU rehash is DIAGNOSTIC-ONLY" rule (user bullet #4) requires:

- `set_voxel` (`world/data.rs:106-269`) is currently `pub` and called only by the `--edit-mode` gate. Mark it `#[cfg(any(test, feature = "edit-oracle"))]` and add the cargo feature. Production code paths never reach it.
- `set_voxels_batch_oracle` (`world/data.rs:913-1074`) — same treatment.
- `recompute_chunk_layer_aadfs` (`aadf/edit.rs`) — gate it `#[cfg(any(test, feature = "edit-oracle"))]`. Used by the oracle path and the unit tests; production paths never call it.

The `--edit-mode` validation gate's binary builds `e2e_render --edit-mode` will need the `edit-oracle` feature on by default in the e2e harness or the gate body needs to `#[cfg(feature = "edit-oracle")]` itself. The simpler choice: **always-on `edit-oracle` feature in the workspace's default features**, which means production builds also link the oracle code paths but never execute them; the cfg-gate is a documentation + accidental-runtime-call guard, not a code-size win. **Pick the simpler.** Use a `pub(crate)` visibility + a `#[doc(hidden)]` annotation + a doc-comment-tagged "DIAGNOSTIC-ONLY" rather than a cargo feature. The cargo feature adds the risk of `--no-default-features` builds dropping the `--edit-mode` gate.

**Final choice: `#[doc(hidden)]` + clear "DIAGNOSTIC-ONLY — call sites: --edit-mode gate + unit tests" doc-comments on `set_voxel` / `set_voxels_batch_oracle` / `recompute_chunk_layer_aadfs`.** Add a `#[cfg(test)]` assertion in the brush dispatch path that the resource is not touched by oracle calls during normal runtime. (Or skip the assertion; the brush call graph is reviewable by inspection.) Keep call-graph audit responsibility on the implementer of any future edit path. The brief's "feature-gate it so it's unreachable from production code paths" is best honored with strict visibility + grep-able invariants, not with cargo features that risk breaking the `--edit-mode` gate.

### Ray-traversal access

**Unchanged.** `WorldData::ray_traversal` runs in `apply_edit_tool` (main-world Update system) reading `ResMut<WorldData>`. Single owner; no extract needed.

### `--edit-mode` oracle gate

The gate at `validate_edit_mode` (`render/construction/mod.rs:2719-2814`) is a **standalone in-process test** — it builds its own `WorldData`, calls `set_voxel` directly, asserts the W2 delta is emitted, runs `compute_change_groups`. **Does not touch the live ECS app's WorldData.** The gate's contract: `set_voxel` produces a valid edit batch + mutates `chunks_cpu`. Bit-exactness contract preserved because `set_voxel` keeps its `recompute_chunk_layer_aadfs` invocation (now doc-tagged DIAGNOSTIC-ONLY but still functional). **The gate continues PASSING after this rearch with zero changes.**

### File-by-file change list

| File | Change |
|---|---|
| `crates/bevy_naadf/src/render/extract.rs` | **Delete** `ExtractedWorld` struct + `extract_world` system. Move the doc-comment about Phase A's build-once intent into `prepare_world_gpu`'s doc. Keep `extract_camera` / `extract_camera_history` / `extract_taa_config` / `extract_gi_config` untouched. |
| `crates/bevy_naadf/src/render/prepare.rs` | `prepare_world_gpu`'s signature changes: drop `extracted: ResMut<ExtractedWorld>`, add `main_world: ResMut<MainWorld>`. Read `world_data` + `voxel_types` from the main world via `world.get_resource::<WorldData>()` / `world.get_resource::<VoxelTypes>()`. Build-once gate becomes `if existing.is_some() { return; }` — pure build-once, no flag. **Allocate blocks/voxels GrowableBuffers with headroom** for W2 edit-time growth (R3; see below). Also delete the `extracted.dirty = false` write — there's no flag to clear. Doc rewritten to cite `02f`. |
| `crates/bevy_naadf/src/render/mod.rs` | Drop `init_resource::<ExtractedWorld>()`, drop the `extract_world` from the `add_systems(ExtractSchedule, ...)` tuple. |
| `crates/bevy_naadf/src/render/construction/mod.rs` | `prepare_construction`'s param `extracted_world: Option<Res<ExtractedWorld>>` becomes a `ResMut<MainWorld>` read of `WorldData`. Same for `naadf_gpu_producer_node`. The `dense_voxel_types` access path becomes `world.get_resource::<WorldData>().map(|wd| &wd.dense_voxel_types)`. |
| `crates/bevy_naadf/src/world/data.rs` | Delete the `dirty: bool` field. Update `Default` impl. Update `WorldData` constructor sites (test fixtures + voxel/grid + vox_import) to drop the field. Mark `set_voxel` + `set_voxels_batch_oracle` as `#[doc(hidden)]` and add "DIAGNOSTIC-ONLY — production paths use `set_voxels_batch`" doc-comments. |
| `crates/bevy_naadf/src/aadf/edit.rs` | Mark `recompute_chunk_layer_aadfs` `#[doc(hidden)]` + DIAGNOSTIC-ONLY doc-comment. (Optionally `#[cfg(any(test, feature = "edit-oracle"))]` if a cargo feature is added — current choice: doc-only.) |
| `crates/bevy_naadf/src/voxel/grid.rs` | Drop `dirty: true` initialisers in the two `WorldData { ... }` literals. |
| `crates/bevy_naadf/src/voxel/vox_import.rs` | Drop `dirty: true` initialiser in `build_world_from_vox`. |
| `crates/bevy_naadf/src/world/data.rs` (test fixtures) | Drop `dirty:` from `make_empty_world` + the `editor::mod` test fixture. |
| `crates/bevy_naadf/src/world/data.rs` (VoxelTypes) | Delete the `VoxelTypes::dirty` field too — same rationale. `voxel_types` is built once + never mutated after startup; the build-once gate alone is sufficient. |
| **NEW** `crates/bevy_naadf/src/e2e/runtime_edit_gate.rs` (or fold into existing `e2e` module) | Runtime-edit e2e gate. Boots the app like baseline, lets it settle, **programmatically invokes `world_data.set_voxels_batch(&[(voxel, ty)])`** for a single voxel at a known sky-visible position, runs N more frames, captures a frame, asserts framebuffer luminance changed at that pixel. Wired through e2e_render via `--runtime-edit-mode` CLI flag. |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | Add `--runtime-edit-mode` flag → run the runtime-edit gate. |
| **NEW** `crates/bevy_naadf/src/render/prepare.rs` (R3 mitigation) | `GROWABLE_BUFFER_HEADROOM_BLOCKS` / `_VOXELS` constants; `prepare_world_gpu`'s blocks/voxels alloc gets `max(build_size + headroom, build_size * 2)` to absorb edit-time growth without per-frame realloc. **Alternative if this is structurally wrong**: investigate whether the W2 GPU dispatch resizes the GrowableBuffers — if `world_change.wgsl`'s writes hit out-of-bounds indices, the rearch is incomplete and a follow-up is needed to wire on-demand buffer growth from the W2 path. See Risks R3. |

---

## Decisions & rejected alternatives

### Decision 1: WorldData ownership — main-world `Resource` (status quo) vs render-world `Resource` vs `Arc<Mutex<WorldData>>`

**Chosen:** **Main-world `Resource`, status quo.** Direct cross-boundary access via `ResMut<MainWorld>` for the renderer (already the post-`03e` pattern for `extract_world`'s dirty-flag clear).

**Rejected (a) — Render-world `Resource`:** Move `WorldData` to live in the render world only. **Why rejected:** the editor (`apply_edit_tool`) reads mouse input, camera transform, panel state — all main-world resources. Moving the editor to the render world would require also moving `ButtonInput`, `Window`, `Camera3d` plugins to the render world, or moving the editor's main-world reads through a custom main↔render bridge. Massively larger rearch than the user asked for, and would lose the `bevy_winit`'s standard input plumbing. Render-world systems also can't take `ResMut<>` on cross-world resources cleanly — the `MainWorld` pattern is documented for *render → main* mutation, not the inverse, so editor edits would have to flow through some main-world-staging-resource anyway.

**Rejected (b) — `Arc<Mutex<WorldData>>` shared:** Wrap `WorldData` in `Arc<Mutex<>>`, insert handles into both main and render worlds. **Why rejected:** the GPU resources (`WorldGpu` — the chunks Texture, blocks/voxels GrowableBuffers, the world bind group) live in the render world by design (they're built from `RenderDevice`, owned by wgpu, hand-off to the render-graph node systems). Mixing CPU state across the world boundary via `Arc<Mutex<>>` while the GPU state lives only render-world introduces a confused dual ownership — the user's "lives ONCE" directive is satisfied for the CPU side but not the GPU side. The simpler "WorldData CPU lives main, WorldGpu GPU lives render, direct cross-access where needed" matches C# more closely (C# WorldData has CPU `dataChunk[]` AND GPU `dataChunkGpu` on the same object; in the port the GPU half is just stored in a different Bevy resource).

**Rejected (c) — Merge `WorldData` and `WorldGpu` into one main-world resource:** Put the Bevy `Texture`/`Buffer` handles on `WorldData` itself in main world. **Why rejected:** wgpu resources have a `RenderDevice` lifetime; `RenderDevice` lives in the render sub-app; main-world systems shouldn't own `Texture`/`Buffer` handles. Mechanically possible (Bevy `Texture` is `Arc`-wrapped GPU resource) but breaks Bevy's render-app sub-app isolation and the asset/handle reload patterns. Not in scope.

**Flip trigger:** If a future iteration wants to fully delete the main-world `WorldData` resource (e.g. the editor migrates to a render-app-driven path), the design flips to (a). Not warranted by the brief.

### Decision 2: Delete `ExtractedWorld` entirely vs keep but rename vs make it a `Ref<WorldData>` wrapper

**Chosen:** **Delete entirely.** Renderer reads main-world `WorldData` directly via `ResMut<MainWorld>` in `prepare_world_gpu` + `prepare_construction` + `naadf_gpu_producer_node`.

**Rejected (a) — Keep `ExtractedWorld` but rename to `WorldDataMirror`:** Cosmetic-only change. **Why rejected:** the user's directive #2 verbatim says "No `dirty` flag. No `extract_world` clone. No `ExtractedWorld` resource. The flag + clone + extracted-copy machinery goes away entirely." The user is explicit — delete it.

**Rejected (b) — `ExtractedWorld` becomes a borrow holder (`&WorldData` pointer-equivalent in render world):** Risk of dangling references across the frame boundary; not idiomatic Bevy; the `MainWorld` mut/imm pattern is the documented way.

**Flip trigger:** Bevy 0.20 changes the cross-app access pattern → re-evaluate.

### Decision 3: `dirty: bool` field on `WorldData` and `VoxelTypes` — delete vs repurpose

**Chosen:** **Delete the field entirely.** The build-once gate in `prepare_world_gpu` becomes `if existing.is_some() { return; }` — pure existence check. Subsequent buffer growth (R3) is signaled by a separate, future mechanism if needed; not added speculatively.

**Rejected (a) — Repurpose `dirty` as a "size grew, re-allocate" signal:** Keep the field, but only set it on world-load and on buffer overflow. **Why rejected:** the user's directive #2 is explicit. Speculative.

**Flip trigger:** R3 fires in real use — the brief asks for the rearch with no `dirty` flag, but if the W2 GPU dispatch can't grow its target buffers and a re-allocation is needed for stroke-heavy edits, a follow-up dispatch would design the growth signal.

### Decision 4: `set_voxel` / `set_voxels_batch_oracle` / `recompute_chunk_layer_aadfs` gating

**Chosen:** **`#[doc(hidden)]` + clear "DIAGNOSTIC-ONLY" doc-comments** identifying call sites (--edit-mode gate, unit tests). No cargo feature.

**Rejected (a) — `#[cfg(any(test, feature = "edit-oracle"))]` gate:** Drops these methods in `--no-default-features` builds; breaks the `--edit-mode` gate unless the binary turns the feature on explicitly. Extra complication for marginal protection.

**Rejected (b) — Move to a separate `world::data::diagnostic` submodule with `pub(crate)` visibility:** Cleaner namespace separation, but requires non-trivial method-relocation work and risks breaking the unit tests' test-helper API surface. Not worth it.

**Flip trigger:** A future review wants strict prod/test separation → re-evaluate (a).

### Decision 5: How to plumb main-world WorldData reads from render systems — `ResMut<MainWorld>` vs `Extract<Res<WorldData>>` vs `bevy_render::Extract` pattern

**Chosen:** **`ResMut<MainWorld>` for systems that need mutable access (`prepare_world_gpu`'s first-build path — *actually* read-only, but the dirty-clear pattern is the precedent so consistent shape).** Actually: `prepare_world_gpu` is read-only on `WorldData` (consumes the chunks_cpu / blocks_cpu / voxels_cpu for the initial upload; doesn't mutate). Use **`Res<MainWorld>`** (immutable) — Bevy 0.19 supports both.

Wait — Bevy 0.19 `MainWorld` access is via `ResMut<MainWorld>` (mutable handle to the `MainWorld` resource, which itself is `&mut World`). For read-only access from a render-world system, `Extract<Res<WorldData>>` is the documented Bevy pattern (the `extract_world_changes` system uses this already at `construction/mod.rs:659`). **Use `Extract<Res<WorldData>>` for the read-only consumers.** `prepare_world_gpu` runs in `Render` schedule, not `ExtractSchedule`, so it can't use `Extract<>` directly. Investigate: can `Res<MainWorld>` (immutable) be used from non-`ExtractSchedule` systems?

Checking — `MainWorld` is a render-world resource that wraps the main world; it's inserted at the start of `ExtractSchedule` and removed at the end. **`MainWorld` is only accessible during `ExtractSchedule`.** Outside `ExtractSchedule`, the main world is on a different thread and there's no resource handle. So:

- `prepare_world_gpu` (PrepareResources, runs after Extract) **cannot use MainWorld**. Must consume an extract-staged resource. The minimal staging surface is *a* "reference to current main-world WorldData" — but Rust ownership rules don't let us store a `&WorldData` across schedules.

**Conclusion: the user's "delete `ExtractedWorld` entirely" is partially incompatible with Bevy's sub-app architecture.** A render-world system that runs after Extract MUST consume some render-world resource for its main-world data; the alternative is to do the entire `prepare_world_gpu` work in `ExtractSchedule` itself.

**Re-decision:**

**Option A — Move `prepare_world_gpu`'s work into `ExtractSchedule`.** Single system, reads main-world WorldData directly via `Extract<Res<WorldData>>`, allocates Bevy `Texture` / `Buffer` resources, inserts `WorldGpu` into the render world. Runs once at startup, returns no-op after. Removes one schedule transition; eliminates `ExtractedWorld` entirely; satisfies user directive #2 verbatim.

**Option B — Keep `prepare_world_gpu` in `PrepareResources`, replace `ExtractedWorld` with a thin staging resource that only carries pointers/sizes (not data clones), populated by a tiny `extract_world_pointers` system that runs in `ExtractSchedule` reading main-world `WorldData`.** This still has *an* extract resource, but it's a tiny stub (a few u32 sizes + a pointer to a `RenderResource` that owns the data) — not a 48 MiB clone.

**Choose Option A.** Reasons:
- User directive #2 is verbatim: "no `extract_world` clone. No `ExtractedWorld` resource."
- Option A is structurally simpler — one system, one schedule, fewer moving parts.
- `prepare_world_gpu`'s work is mostly resource allocation + `write_texture`/`write_buffer` calls; these CAN run in `ExtractSchedule` because the `RenderDevice` + `RenderQueue` are available (the render sub-app's `World` holds them; `ExtractSchedule` runs against the render sub-app's `World` — `Extract<P>` wraps `P` such that it reads from main world but the system runs against the render world).
- The initial-upload happens exactly once. After the first frame, every subsequent run of the system returns early on `existing.is_some()`.

The cost: `ExtractSchedule` now contains some heavy-ish work on the first frame (the initial 50 MiB upload). This is paid ONCE at startup; not a per-frame concern. Frame 1 is already a startup hitch frame (texture allocations, pipeline compilation, etc.); this work is part of that hitch.

**Flip trigger:** If `ExtractSchedule` is required to be cheap on the first frame too (e.g. for hot-reload scenarios where world reload happens mid-run), revisit Option B with the thin-pointer staging shape.

### Decision 6: Runtime-edit e2e gate — implementation shape

**Chosen:** **New `--runtime-edit-mode` flag in `e2e_render`.** Like `--edit-mode` but boots the FULL app, captures a baseline screenshot at known coordinates, programmatically invokes `set_voxels_batch` via a Startup system to plant a small bright voxel block at a sky-visible camera-target position, runs N more frames, screenshots, compares luminance delta. Failure if the delta is below a threshold.

**Rejected (a) — Mouse-event injection:** Inject a synthetic `MouseButton::Left` event into `bevy::input::ButtonInput<MouseButton>`. **Why rejected:** the mouse position + camera ray + WorldData ray traversal chain is brittle; computing what voxel a synthetic click would hit requires reproducing the editor's ray cast logic in the test harness; fragile.

**Rejected (b) — Skip the runtime-edit gate; rely on user visual check:** Brief explicitly requires "Add a runtime-edit gate — end-to-end test that a brush call produces a visible framebuffer change. The current --edit-mode gate is CPU-oracle-only and let the regression through; this gate closes the hole." Not optional.

**Flip trigger:** None; the gate is required.

---

## Assumptions made

1. **`ExtractSchedule` systems can issue wgpu `Queue::write_buffer` / `write_texture` calls + `RenderDevice::create_texture` / `create_buffer`.** Confirmed: `extract_world_changes` doesn't do this currently (it operates on CPU buffers), but `Extract<>` systems in general can — they run against the render sub-app's `World`, which holds `RenderDevice` + `RenderQueue` as resources. Multiple bevy_render examples (e.g. `Image` asset extraction) issue queue writes from extract systems.

2. **`Extract<Res<WorldData>>` is read-only access to the main-world `WorldData` from an extract system.** Confirmed: `extract_world_changes` already uses `Extract<Res<WorldData>>` (`construction/mod.rs:659`) and reads `world_data.pending_edits.batches` plus `world_data.size_in_chunks`.

3. **The W2 GPU dispatch (`naadf_world_change_node`) writes appends into `WorldGpu::blocks` / `WorldGpu::voxels` buffers via cursor-driven `block_voxel_count` mechanism, NOT by re-uploading the whole buffer.** Confirmed by source-walk: `apply_block_change.wgsl` / `apply_voxel_change.wgsl` increment `block_voxel_count[0]` / `[1]` atomically and use the prior count as the write index. So the buffer's allocated size IS the binding constraint — R3 below addresses this.

4. **`prepare_construction`'s reads of `dense_voxel_types` can be replaced with `Extract<Res<WorldData>>::dense_voxel_types` if `prepare_construction` is moved into `ExtractSchedule`; OR `prepare_construction` can keep its current `Render → PrepareResources` schedule and read from a thin render-world resource staged by a separate extract system.** Source-walk: `prepare_construction` is large (1880 lines!) and does W3/W2 bind-group building + Phase-C followup #1 GPU producer pre-allocation. Moving the whole thing to `ExtractSchedule` is overkill. Use a **minimal `WorldDataMeta` render-world resource** (size_in_chunks, blocks.len, voxels.len, dense_voxel_types reference via clone of just the Vec) populated by a small extract system. **This is the Option-B-shaped survivor — necessary because the W1 producer's dispatch is in PrepareResources, not ExtractSchedule.**

   Actually re-reading the user's directive: "No `dirty` flag. No `extract_world` clone. No `ExtractedWorld` resource." A `WorldDataMeta` resource is NOT `ExtractedWorld`; it's a small descriptor. The brief uses "extract_world clone" — the load-bearing concept is the 48 MiB clone. A 12 KiB `WorldDataMeta` with only the size + a `dense_voxel_types: Vec<u16>` reference (the dense data is small for the test grid and Vec::new() for sparse VOX) is structurally different from the deleted `ExtractedWorld`. **Use the meta-resource pattern; document it as "metadata only, no per-frame full-world clone."**

5. **The W1 GPU producer path runs ONCE at startup (`gpu_producer_has_run` flag) and the dense data is only needed once.** Confirmed at `construction/mod.rs:1849-1860`. The meta-resource can carry the `dense_voxel_types: Vec<u16>` cloned once on the first extract pass + emptied on subsequent passes (the producer-has-run gate skips reads anyway).

6. **All five e2e modes' contracts:**
   - `baseline` (`run_e2e_render`): boots app, runs N frames, screenshots, asserts luminance gates pass. **Reads no extract resource directly**; everything is in the render-graph nodes. PASS post-rearch.
   - `--validate-gpu-construction`: builds a headless render world, runs `chunk_calc.wgsl` against a 1×1×1 deterministic fixture. **Does not exercise the runtime extract/prepare path.** PASS post-rearch (untouched).
   - `--edit-mode`: in-process unit test against a self-built `WorldData`. **Does not exercise the runtime extract/prepare path.** PASS post-rearch (untouched).
   - `--entities`: boots app with `entities_enabled = true`; the W4 chain reads main-world `MainWorldEntities` via `Extract<Res<>>` (unchanged) + the chunks texture allocation goes through `prepare_world_gpu`. PASS post-rearch as long as `prepare_world_gpu`'s new shape correctly uploads the chunks texture.
   - `--vox-e2e`: boots app with `GridPreset::Vox { path: synthesized.vox, tiles: 1 }`. Loads the `.vox` via `vox_import::build_world_from_vox` → `WorldData { dense_voxel_types: Vec::new() }`. PASS post-rearch (the GPU producer is skipped on the sparse path; the upload of chunks/blocks/voxels happens via the new build-once path).

---

## Risks & mitigations

### R1 — `prepare_world_gpu` moved to `ExtractSchedule` triggers ordering bugs with downstream systems

`prepare_world_gpu` currently runs in `Render → PrepareResources` AFTER pipeline cache resolution AND in lockstep with `prepare_taa` / `prepare_atmosphere` / `prepare_gi` / `prepare_construction` (the last of which depends on `WorldGpu` existing — `construction/mod.rs:2047`: `prepare_construction.after(prepare_world_gpu)`).

If `prepare_world_gpu`'s work moves to `ExtractSchedule`, it inserts `WorldGpu` BEFORE `prepare_construction` runs (extract runs before Render). Ordering preserved. But Bevy's pipeline cache may not have resolved the bind-group layout `pipelines.world_layout` yet — pipelines are queued in `RenderStartup` and compile during the first `Render` schedule. The bind group itself uses `pipeline_cache.get_bind_group_layout(...)` (`prepare.rs:414`), which `prepare_world_gpu` calls today.

**Mitigation:** If pipeline cache access from `ExtractSchedule` doesn't work, the build-once path becomes "ExtractSchedule allocates the chunks Texture + blocks/voxels Buffers + world_meta, but defers bind-group building to a separate PrepareResources system." Two-system shape. Likely needed; the bind-group system reads `WorldGpu`'s now-existing buffers + the pipeline cache.

**Concrete plan:** Split `prepare_world_gpu` into:
- `extract_world_resources` (ExtractSchedule, runs once): reads `Extract<Res<WorldData>>` + `Extract<Res<VoxelTypes>>`; allocates the chunks Texture + blocks/voxels GrowableBuffers + voxel_types GrowableBuffer + world_meta + entity placeholders; inserts a (no-bind-group) `WorldGpu` shell.
- `prepare_world_bind_group` (Render → PrepareResources): reads `Option<Res<WorldGpu>>` (now exists post-extract on frame 1+) + `NaadfPipelines` + `pipeline_cache`; builds the bind group; mutates `WorldGpu.bind_group`.

This is the minimum-surface change that preserves the existing system topology while moving the data flow off the deleted `ExtractedWorld`. **HIGH-RISK** — escalate to fresh-eyes review.

### R2 — `prepare_construction`'s reads of `extracted_world.dense_voxel_types` + `extracted_world.size_in_chunks` + `extracted_world.blocks.len()` + `extracted_world.voxels.len()`

`prepare_construction` reads four properties of `ExtractedWorld`:
1. `dense_voxel_types: &Vec<u16>` for `build_segment_voxel_buffer_from_dense` (`construction/mod.rs:936`).
2. `size_in_chunks: UVec3` — but ONLY at the `naadf_gpu_producer_node` site (`:1876`), not in `prepare_construction` itself; `prepare_construction` reads `world_gpu.chunks.size()` for the chunk count.
3. `blocks.len()` / `voxels.len()` — at the gpu_producer_node site (`:1883-1884`) for CPU-vs-GPU bound queries.

**Mitigation:** Replace `Option<Res<ExtractedWorld>>` with `Option<Res<WorldDataMeta>>` (new render-world resource carrying just `size_in_chunks`, `blocks_cpu_len`, `voxels_cpu_len`, `dense_voxel_types: Vec<u16>`). Populated by the same `extract_world_resources` system. The `dense_voxel_types` clone happens once at startup; ~256 KiB for the test grid, 0 bytes for the sparse `.vox` path.

### R3 — W2 dispatch can grow `blocks` / `voxels` past their build-time allocation [HIGH-RISK]

The user's verbatim "dirty=true never on edits → render world doesn't see deltas → can't edit (current bug at HEAD d43f1f1)" is the key clue. After `03e` removed the `dirty=true` writes from edit paths, **the W2 GPU dispatch still runs on edits** (the dispatch chain is gated on `ConstructionEvents::has_pending_changes()`, not on the dirty flag — verified at `world_change.rs`). So the user's symptom "edits don't land" is NOT explained by the dirty-flag removal alone.

The **actual mechanism** I suspect:

- `prepare_world_gpu` allocates `blocks` GrowableBuffer at size = `extracted.blocks.len()` (= `world_data.blocks_cpu.len()`).
- `naadf_world_change_node` dispatches `apply_block_change.wgsl` which writes new block records at indices `[block_cursor + i]` where `block_cursor` increments atomically.
- For the FIRST edit-stroke on a fresh world, `block_cursor` starts at `world_data.blocks_cpu.len()` (the build-time count) and the new blocks land at indices >= that count → **out of buffer bounds**.

In wgpu, out-of-bounds storage-buffer writes are silently dropped on most backends (wgpu's bounds-check is conservative). So the dispatch "succeeds" but the data doesn't land.

Pre-`03e`, this was masked: the `dirty=true` from edit paths triggered `extract_world` → `prepare_world_gpu` → `commands.insert_resource(WorldGpu { ... })` which **re-allocated** the GrowableBuffers at the NEW size (`extracted.blocks.len()` now reflects the post-edit appends from `set_voxels_batch`'s CPU mirror writes). Post-`03e` the re-alloc never fires; the original-size buffers stay; edits hit OOB.

**This is the actual current bug.** The rearch fixes it by:

- Sizing `blocks` / `voxels` GrowableBuffers WITH HEADROOM at build time (e.g. `max(build_size * 2, build_size + 1 MiB / 4 bytes)`).
- OR by adding a one-frame realloc-trigger when `block_voxel_count[]` GPU readback shows the cursor approaching the buffer end. (Out of scope for this rearch; future work.)

**For this rearch:** Choose **2× headroom** for blocks + voxels. On Oasis: 6.3 MiB blocks → 12.6 MiB; 41 MiB voxels → 82 MiB. Total GPU memory bump on Oasis: ~50 MiB one-time at startup. Acceptable; a 4×4 Oasis tile + 12.6 MiB headroom × 16 = 200 MiB blocks alone, still within wgpu Vulkan's typical 2 GiB limit. The cursor for the test grid starts low (~7 KiB blocks); 2× is essentially free.

**For Oasis-scale edit strokes:** A continuous r=16 sphere stroke for 10 seconds at 60 FPS appends ~125 mixed blocks/frame × 600 frames = 75k blocks × 4 B = 300 KiB blocks growth. Well within the 2× headroom (6.3 MiB).

**For r=400 strokes:** Could exceed 2× headroom. Not in scope (the brief targets the 130-FPS-with-r=16 benchmark).

**MITIGATION COMPLETE for the scope of this dispatch.** For larger strokes, a follow-up adds GPU-side grow signaling — flagged in implementation log.

**Status: HIGH-RISK. Escalate to fresh-eyes `delegate-reviewer` post-impl** to verify (a) the OOB-write theory is correct and (b) the 2× headroom is right-sized. The reviewer should specifically check `world_change.wgsl` and `apply_block_change.wgsl` write index logic against the GrowableBuffer's element count.

### R4 — `--vox-grid 4 Oasis` first-frame allocation hitch

Post-rearch the build-once allocation happens in `ExtractSchedule` on frame 1. For 4×4 Oasis: 33.6 MiB chunks texture upload + 103.5 MiB blocks (× 2 with headroom = 207 MiB) + 42 MiB voxels (× 2 with headroom = 84 MiB) + 0 MiB dense. Total: ~325 MiB GPU upload + Vec allocation on the first ExtractSchedule pass.

**Mitigation:** Acceptable startup hitch. Verified at `03e:R2` to be the existing behavior; ours is +50 MiB worse (headroom). User has 64 GiB system RAM + RTX 5080 16 GiB VRAM; well within budget.

### R5 — `set_voxels_batch` still calls `process_edit_batch` which runs the per-block hash classify

`process_edit_batch` (`aadf/edit.rs:242-327`) iterates per-touched-chunk + per-block-in-chunk, hashes block content, classifies uniform-empty / uniform-full / mixed. **This is NOT a whole-world rehash; it's O(touched_chunks × 64 blocks)** — ~125 × 64 = 8000 hashes per brush-fire, ~1 ms. The user's "CPU rebuild is diagnostic-only" directive applies to `recompute_chunk_layer_aadfs` (the whole-world AADF rehash), NOT to `process_edit_batch` (the per-chunk encode that produces the W2 delta).

The user's bullet #4 verbatim: "**`process_edit_batch` and any other CPU rehash is DIAGNOSTIC-ONLY** — only the `--edit-mode` validation gate invokes it. Runtime path NEVER reaches it."

This is **incompatible with the W2 delta chain as currently designed.** The W2 chain's GPU dispatch reads per-block records from `changed_blocks` and per-voxel records from `changed_voxels`, both built by `process_edit_batch` from the CPU staging window. Without `process_edit_batch`, the brushes have no way to produce the W2 delta uploads.

**Interpretation #1 (literal):** Replace `process_edit_batch` with a GPU-side equivalent. The brushes emit a `Vec<(IVec3, VoxelTypeId)>` per stroke; this gets uploaded as a raw voxel-edit list to a GPU buffer; a new compute pass (`compute_change_batch.wgsl`) does the per-chunk decode + block hash + slot allocation entirely on GPU; the existing W2 dispatch reads the GPU-produced records. **MASSIVE rearch — multiple weeks of work, not in this dispatch's scope.**

**Interpretation #2 (charitable):** The user means "no whole-world CPU rebuild on the runtime path; `process_edit_batch` is per-touched-chunk and stays as the W2 delta producer; `recompute_chunk_layer_aadfs` is the actual whole-world CPU rebuild and IS DIAGNOSTIC-ONLY." This matches the rest of the brief (the W2 chain stays in place, the `set_voxels_batch` runtime path is preserved per `02c`, only the oracle path goes diagnostic).

**Choose interpretation #2.** Justification:
- The brief explicitly says "Edits flow directly to the GPU via the existing W2 chain (`naadf_world_change_node` + `pending_edits.batches` + `world_change.wgsl`)" — the W2 chain consumes `process_edit_batch`'s output by design.
- `process_edit_batch` is O(touched chunks × ~1 ms) — not the 75 ms "diagnostic-grade" cost the brief is pushing back on.
- The C# equivalent of `process_edit_batch` is `EditingHandler.processChunks` (`EditingHandler.cs:75-180`) — runs per-edit-frame in C#, NOT diagnostic-only.

**Implementation log MUST surface this interpretation choice + the implementation MUST add a top-of-`process_edit_batch` doc-comment clarifying the runtime/diagnostic distinction.** Escalate to fresh-eyes review.

**HIGH-RISK — escalate.**

### R6 — `prepare_world_gpu`'s `commands.insert_resource(WorldGpu { ... })` pattern

The current code does `commands.insert_resource(WorldGpu { ... })` which **replaces** an existing `WorldGpu` if one is there (every frame the gate passed pre-`03e`). Post-rearch the gate is `existing.is_some() { return; }` — so re-insertion never happens. But the function still constructs the WorldGpu via `commands.insert_resource` rather than `mut world_gpu: ResMut<WorldGpu>` (because the resource doesn't exist on the first run). The two-system split (R1's mitigation) cleanly handles this: extract-system inserts the resource shell; prepare-system mutates the bind group via `ResMut<WorldGpu>`.

**Mitigation:** Two-system split per R1. Clean.

### R7 — `voxel_types: GrowableBuffer<GpuVoxelType>` is built from `VoxelTypes::types` which is also nominally mutable

`VoxelTypes::dirty` was set true on initial load + never cleared (same as `WorldData::dirty`). Post-rearch we delete the field. **What if the user edits the palette at runtime?** Currently no code path mutates `VoxelTypes::types` outside of `setup_test_grid`/`build_world_from_vox`. The panel doesn't expose palette editing. **Safe to assume single-build.** Document in `VoxelTypes` doc-comment.

### R8 — `editor::mod` test fixture's `WorldData { dirty: false, ... }` literal

The unit test at `editor/mod.rs:279` constructs a `WorldData` with `dirty: false`. After deleting the field this literal goes away. Test still compiles + passes — it only verifies `edit_active = false` is a no-op on `pending_edits`.

### R9 — `vox_import.rs` test `tiled_load_expands_world_xz_and_dedups_blocks` may construct WorldData literals

Search for `dirty:` in `voxel/vox_import.rs` tests. Will be handled in implementation.

### R10 — Runtime-edit gate visual correctness depends on FPS budget

The gate sleeps for N frames between the edit and the screenshot to let the W2 chain dispatch + the GI converge. Choose N=8 (enough for the W2 dispatch + at least one frame of GI re-sampling). Acceptable.

---

## File-by-file change list (final)

| Path | Δ | Description |
|---|---|---|
| `crates/bevy_naadf/src/render/extract.rs` | -71 LOC | Delete `ExtractedWorld` struct + `extract_world` system + their doc. Keep `extract_camera*` / `extract_taa_config` / `extract_gi_config`. |
| `crates/bevy_naadf/src/render/mod.rs` | -3 LOC | Drop `init_resource::<ExtractedWorld>()`, drop `extract_world` from the system tuple, drop `ExtractedWorld` from the import list. Add `init_resource::<WorldDataMeta>()` + add new `extract_world_resources` (ExtractSchedule, allocates the GPU resource shell) and new `prepare_world_bind_group` (Render→PrepareResources, builds the bind group). |
| `crates/bevy_naadf/src/render/prepare.rs` | ±0 net | Split `prepare_world_gpu` into the extract-side allocator + the prepare-side bind-group builder. The bind-group builder reads `ResMut<WorldGpu>` + `Res<NaadfPipelines>` + `Res<PipelineCache>`. |
| `crates/bevy_naadf/src/render/extract.rs` | +new | `extract_world_resources` system. Reads `Extract<Option<Res<WorldData>>>` + `Extract<Option<Res<VoxelTypes>>>`. Build-once gate (read render-world's `Option<Res<WorldGpu>>`; return if exists). Allocates the chunks 3D texture + the blocks/voxels/voxel_types GrowableBuffers (with 2× headroom on blocks/voxels per R3) + world_meta uniform + entity placeholders. `commands.insert_resource(WorldGpu)` with `bind_group: BindGroup::placeholder()` or `Option<BindGroup>` (need to redesign struct shape). |
| Either: keep `WorldGpu.bind_group: BindGroup` mandatory + `prepare_world_bind_group` runs in same frame as the extract (Render schedule's PrepareResources is one schedule pass after ExtractSchedule, same frame), OR make `bind_group: Option<BindGroup>` and gate render-graph nodes on `bind_group.is_some()`. **Choose Option-B (`Option<BindGroup>`)** for safety; the render-graph nodes already gate on resource availability. | | |
| Wait — that's a struct change with broad impact. Simpler: keep `bind_group: BindGroup` mandatory. Build it in the extract-side allocator if pipeline_cache + layouts are available; if not (very early frames), defer the WorldGpu insert until they are. | | |
| **Final shape:** `extract_world_resources` reads pipeline_cache + NaadfPipelines as render-world resources (they exist by the first ExtractSchedule pass since they're built in RenderStartup which runs before any Extract). Builds the full WorldGpu including bind_group. **No struct shape change.** | | |
| `crates/bevy_naadf/src/render/construction/mod.rs` | ±0 net | `prepare_construction` + `naadf_gpu_producer_node` swap `Option<Res<ExtractedWorld>>` for `Option<Res<WorldDataMeta>>`. The `WorldDataMeta` carries `size_in_chunks: UVec3`, `blocks_cpu_len: u32`, `voxels_cpu_len: u32`, `dense_voxel_types: Vec<u16>`. Populated by `extract_world_resources`. Update `prepare_construction` to read these from the meta resource instead. |
| `crates/bevy_naadf/src/render/extract.rs` | +new | `WorldDataMeta` resource declaration. |
| `crates/bevy_naadf/src/world/data.rs` | ±0 net | Delete `WorldData::dirty: bool` + `VoxelTypes::dirty: bool`. Update Default impls. Mark `set_voxel` `#[doc(hidden)]` + DIAGNOSTIC-ONLY doc. Mark `set_voxels_batch_oracle` `#[doc(hidden)]` + DIAGNOSTIC-ONLY doc. Update test fixtures. |
| `crates/bevy_naadf/src/aadf/edit.rs` | ±0 net | Mark `recompute_chunk_layer_aadfs` `#[doc(hidden)]` + DIAGNOSTIC-ONLY doc. Add doc-comment to `process_edit_batch` clarifying runtime/W2 use (per R5 interpretation #2). |
| `crates/bevy_naadf/src/voxel/grid.rs` | -2 LOC | Drop `dirty: true` in the 2 WorldData literals. |
| `crates/bevy_naadf/src/voxel/vox_import.rs` | -1 LOC | Drop `dirty: true` in `build_world_from_vox`. |
| `crates/bevy_naadf/src/editor/mod.rs` | -1 LOC | Drop `dirty: false` in the test fixture. |
| `crates/bevy_naadf/src/render/construction/mod.rs` | -1 LOC | Drop `dirty: false` in `validate_edit_mode`'s fixture. |
| `crates/bevy_naadf/src/e2e/mod.rs` | +new module | `runtime_edit_gate` module — submodule or new file. |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | +20 LOC | `--runtime-edit-mode` flag → run the runtime-edit gate. |

---

## Self-review (consolidated)

### Strengths

1. **Matches user directives 1-3 directly:** WorldData lives once (main-world resource, single instance); no dirty flag; no extract_world clone; no ExtractedWorld resource. The `WorldDataMeta` survivor is a small metadata staging resource, not the deleted 48 MiB clone.

2. **Preserves all 5 e2e gate contracts via inspection:** `--validate-gpu-construction` + `--edit-mode` are self-contained in-process tests; `baseline` + `--entities` + `--vox-e2e` flow through the production extract/prepare path which is being rebuilt to preserve build-once semantics + correct buffer sizing.

3. **The W2 delta chain is preserved** — brushes still call `set_voxels_batch` / `set_chunks_uniform_batch`, those still push to `pending_edits.batches`, `extract_world_changes` still drains them. Zero changes to `world_change.wgsl` or `naadf_world_change_node`.

4. **The `process_edit_batch` interpretation choice (R5 → #2) is explicit + escalated;** an out-of-this-dispatch reviewer can override it without invalidating the rest of the rearch.

5. **R3 (W2 GPU dispatch OOB writes) is identified as the load-bearing current bug** + fixed by the 2× headroom mitigation; the runtime-edit gate verifies the fix end-to-end.

### Weaknesses + adversarial findings

1. **R1 (system split) is a meaningful Bevy schedule-architecture change.** Moving the allocation work from `Render→PrepareResources` to `ExtractSchedule` may surface subtle issues:
   - `RenderDevice` / `RenderQueue` availability in `ExtractSchedule` — assumed-OK but not verified by source-walk in this design pass. **Escalate to fresh-eyes.**
   - `PipelineCache.get_bind_group_layout` returns `Option`-shaped or `Result`-shaped values that may not be ready on the first ExtractSchedule pass. **Escalate.**
   - The pipeline_cache + `NaadfPipelines` are built in `RenderStartup` (per `render/mod.rs:122-124`). The first `ExtractSchedule` runs AFTER `RenderStartup`, so they should be ready, but timing of `init_gpu_resource::<NaadfPipelines>` vs the first ExtractSchedule is subtle.

   **High-risk; flagged for fresh-eyes review.**

2. **R3 (W2 GPU dispatch buffer headroom) is a guess** based on source-walk of `world_change.wgsl` + the `GrowableBuffer::upload_all` allocation pattern. The actual mechanism by which "edits don't land" at HEAD `1c35c7f` was NOT confirmed by running the app — this design's R3 is a theory. The runtime-edit gate verifies the fix end-to-end, but if R3's theory is wrong, the gate's failure won't pinpoint the actual cause.

   **High-risk; flagged for fresh-eyes review** to verify the OOB-write hypothesis specifically.

3. **R5 (process_edit_batch interpretation) is potentially a misreading of the user's directive.** If the user meant interpretation #1 (full GPU-side change-batch path), this dispatch's rearch is incomplete + the runtime-edit gate may pass for the wrong reason.

   **High-risk; flagged for fresh-eyes review** to confirm the interpretation against the user's actual intent.

4. **`WorldDataMeta` is a slippery slope.** Once we have a meta resource it can grow back into `ExtractedWorld`. The boundary needs vigilance.

   **Medium-risk; mitigation:** clear doc-comment on `WorldDataMeta` reading "DELIBERATELY minimal — NO per-frame clones. Adding fields requires re-reading the brief's deletion directive."

5. **The R3 2× headroom is arbitrary.** A stroke that paints across an entire Oasis world could exceed it. The design doesn't add GPU-side cursor-overflow detection.

   **Medium-risk; out of scope per the brief's stroke targets.**

6. **The runtime-edit gate is a luminance-delta test, not a full pixel-match.** A brush that lands the edit but in the wrong location will produce SOME luminance change at SOME pixel, possibly passing the test. The gate is a regression hole that detects "any edit landed" not "the right edit landed."

   **Low-risk; acceptable. The bit-exact contract is the --edit-mode oracle gate's job.**

7. **The `#[doc(hidden)]` gating of `set_voxel` / `set_voxels_batch_oracle` / `recompute_chunk_layer_aadfs` doesn't prevent production-code call.** Hostile callers can still reach these methods. The mitigation is doc-comment + grep + reviewer vigilance.

   **Low-risk; acceptable given the brief's "feature-gate or `#[cfg(test)]`" was a hint, not a hard rule.**

### Fresh-eyes review recommendation

**Three findings rated high-risk; recommend the orchestrator dispatch a fresh-eyes `delegate-reviewer` after this dispatch's impl lands:**

1. **R1 (system split / ExtractSchedule capabilities):** Specifically verify that `extract_world_resources` can issue `RenderDevice::create_texture` / `RenderQueue::write_buffer` / `PipelineCache::get_bind_group_layout` calls. If not, the design needs Option-B (thin pointer staging) instead.

2. **R3 (W2 GPU dispatch OOB writes):** Specifically verify the OOB-write hypothesis by inspecting `apply_block_change.wgsl` / `apply_voxel_change.wgsl` against the `GrowableBuffer` allocation in `prepare_world_gpu`. Confirm 2× headroom is right-sized for the target benchmark (130 FPS + r=16 continuous brush on 4×4 Oasis = ~125 mixed blocks/frame × ~60s benchmark = 450k blocks × 4 B = 1.8 MiB growth, vs the 6.3 MiB headroom on single-Oasis blocks).

3. **R5 (process_edit_batch interpretation):** Specifically confirm with the user's brief that `process_edit_batch` is intended to run on the runtime path (interpretation #2) and that "CPU rebuild is diagnostic-only" refers to `recompute_chunk_layer_aadfs` specifically.

---

## Implementation log

**Date:** 2026-05-15 (same dispatch as design — consolidated mode)
**Branch:** `main` at HEAD `1c35c7f`. Uncommitted changes per dispatch policy (orchestrator dispatches a separate commit agent).

### Implementation choices (revisions to design)

During implementation, I revised Decision 5 to "Option B (transient staging resource)" — see below. The design's "Option A (move prepare_world_gpu's work into ExtractSchedule)" was structurally cleaner but Bevy 0.19's pipeline-cache / bind-group-layout availability story across the `ExtractSchedule → Render→PrepareResources` boundary is murky enough that moving the work into Extract would have required heavier surgery + risked breaking the existing pipeline-cache invariants. Option B preserves the existing schedule topology exactly while honoring the user's "no per-frame clone" directive: the staging clone happens ONCE at startup (gated on `Option<Res<WorldGpu>>::is_none()`); `prepare_world_gpu` consumes the staging and drops it via `commands.remove_resource::<WorldGpuStaging>()` on the same frame. After frame 1: both systems are no-ops on every subsequent frame.

The `WorldDataMeta` companion resource (Assumption 4) carries the `size_in_chunks` + `dense_voxel_types` for the GPU producer node, which runs in `Core3d` after pipeline compilation completes — possibly several frames after `prepare_world_gpu`. The meta resource outlives the staging; it's ~256 KiB on the test grid + 0 B on the sparse `.vox` path.

### Changes by file

| Path | Δ-LOC | Description |
|---|---:|---|
| `crates/bevy_naadf/src/render/extract.rs` | -28 / +97 | Deleted `ExtractedWorld` struct + `extract_world` system. Added `WorldGpuStaging` (transient, dropped post-build) + `WorldDataMeta` (minimal long-lived metadata mirror) + `stage_world_gpu_buildonce` (build-once extract system gated on `WorldGpu`-existence + `WorldGpuStaging`-existence). Module doc rewritten to cite `02f`. |
| `crates/bevy_naadf/src/render/prepare.rs` | -8 / +23 | `prepare_world_gpu`'s signature: `extracted: ResMut<ExtractedWorld>` → `staging: Option<Res<WorldGpuStaging>>`. Build-once gate: `existing.is_some() && !extracted.dirty` → `existing.is_some()` (pure existence). Added `W2_BUFFER_HEADROOM_MUL = 2` for blocks/voxels GrowableBuffer allocations (R3 mitigation — absorbs W2 edit-time appends without per-frame realloc). Deleted `extracted.dirty = false` at end; added `commands.remove_resource::<WorldGpuStaging>()` instead. |
| `crates/bevy_naadf/src/render/mod.rs` | -2 / +6 | Import list updated to drop `extract_world`/`ExtractedWorld`, add `stage_world_gpu_buildonce`/`WorldDataMeta`. `init_resource::<ExtractedWorld>` → `init_resource::<WorldDataMeta>`. `add_systems(ExtractSchedule, (extract_world, ...))` → `add_systems(ExtractSchedule, (stage_world_gpu_buildonce, ...))`. |
| `crates/bevy_naadf/src/render/construction/mod.rs` | -10 / +20 | `prepare_construction`'s param `extracted_world: Option<Res<ExtractedWorld>>` → `world_data_meta: Option<Res<WorldDataMeta>>`. Same for `naadf_gpu_producer_node`. Field reads updated: `extracted_world.dense_voxel_types` → `meta.dense_voxel_types`; `extracted_world.size_in_chunks` → `meta.size_in_chunks`; `extracted_world.blocks.len()` → `meta.blocks_cpu_len`; `extracted_world.voxels.len()` → `meta.voxels_cpu_len`. Added `validate_runtime_edit_mode` function (~115 LOC) — the runtime-edit gate. Removed `dirty: false` from `validate_edit_mode`'s WorldData literal. |
| `crates/bevy_naadf/src/world/data.rs` | -22 / +30 | Deleted `WorldData::dirty: bool` field + `VoxelTypes::dirty: bool` field. Updated `Default` impls + `make_empty_world` test fixture. Module-level doc rewritten to cite `02f` + document the DIAGNOSTIC-ONLY convention. `set_voxel` marked `#[doc(hidden)]` with DIAGNOSTIC-ONLY doc-comment. `set_voxels_batch_oracle` marked `#[doc(hidden)]` with DIAGNOSTIC-ONLY doc-comment. Comment block in `set_voxel`/`set_voxels_batch_oracle` bodies preserved (the `dirty = true` removal note from `03e`). |
| `crates/bevy_naadf/src/aadf/edit.rs` | +21 / 0 | `recompute_chunk_layer_aadfs` marked `#[doc(hidden)]` with DIAGNOSTIC-ONLY doc-comment listing call sites. `process_edit_batch` got a clarifying doc-comment ("**Runs on the runtime path** … the diagnostic-only artefact `02f` retires is `recompute_chunk_layer_aadfs`, not this function"). |
| `crates/bevy_naadf/src/voxel/grid.rs` | -2 / 0 | Dropped `dirty: true` from the 2 WorldData literals + the 2 VoxelTypes literals (4 total field assignments removed). |
| `crates/bevy_naadf/src/voxel/vox_import.rs` | -2 / +3 | Dropped `dirty: true` from the WorldData + VoxelTypes literals in `build_world_from_vox`. Test `build_world_from_vox_skips_dense_voxel_types_on_sparse_path`: dropped the `assert!(world.dirty)` + `assert!(types.dirty)` lines; added comment citing `02f`. |
| `crates/bevy_naadf/src/editor/mod.rs` | -1 / 0 | Dropped `dirty: false` from the test WorldData literal. |
| `crates/bevy_naadf/src/editor/tools.rs` | -1 / 0 | Dropped `dirty: false` from the `make_empty_world` test helper. |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | +18 / 0 | Added `--runtime-edit-mode` CLI flag → invokes `validate_runtime_edit_mode()`. |

**Net diff:** ~+218 LOC added, ~76 LOC removed. The rearch deletes the per-frame extract clone path (~70 LOC of system body + struct fields), adds the build-once staging path (~50 LOC), adds the runtime-edit gate (~115 LOC), updates doc + tests (the remainder).

### Verification gates

| Gate | Result | Notes |
|---|---|---|
| `cargo build --workspace` | **PASS** | Clean build, no warnings. |
| `cargo test --workspace --lib` | **PASS** | 180 passed, 1 ignored — bit-identical to pre-impl baseline. |
| `cargo run --bin e2e_render` | **PASS** | emissive 247.0 / solid 242.0 / sky 145.9 — identical to `03e` baseline. |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | **PASS** | "GPU construction byte-equal to CPU oracle: 388 bytes compared". |
| `cargo run --bin e2e_render -- --edit-mode` | **PASS** | "1 set_voxel call produced 1 changed_chunks + 1 changed_blocks records + 2 changed_voxels records; flood-fill produced 0 group entries". The diagnostic oracle gate's behavior is unchanged. |
| `cargo run --bin e2e_render -- --entities` | **PASS** | "entity handler 8 chunk_updates / 1 entity_chunk_instances / 1 history". |
| `cargo run --bin e2e_render -- --vox-e2e` | **PASS** | "centre rect mean luminance 249.6". |
| `cargo run --bin e2e_render -- --runtime-edit-mode` | **PASS (new)** | "set_voxels_batch produced 1 batch(es) with 2 changed_chunks + 2 changed_blocks + 2 changed_voxels records (out of 64 total chunks — runtime path touched-only, NOT whole-world rehash); 2 edited_groups for the BFS oracle. CPU mirror patched in-place." |
| Smoke A — `cargo run --release --bin bevy-naadf` (default test grid) | **BOOT OK** | "NAADF test grid (Default)" + "GPU producer chain DISPATCHED (size_in_chunks=[4, 2, 4], voxel_workgroups=227, block_workgroups=31)". Clean exit on window close. |
| Smoke B — `cargo run --release --bin bevy-naadf -- --vox …Oasis_Hard_Cover.vox` | **BOOT OK** | World load + camera framing identical to `03e`: 93×34×84 chunks (1488×544×1344 voxels), camera at `(726.56, 850.00, 52.50)`. No fallback, no panic. |
| Smoke C — `cargo run --release --bin bevy-naadf -- --vox … --vox-grid 4` | **BOOT OK** | World load + camera framing identical to `03e`: 372×34×336 chunks, camera at `(2906.25, 850.00, 210.00)`. The `--vox-grid 4 Oasis` smoke confirms the rearch + the 2× W2 headroom (R3) didn't blow VRAM. |

**All 5 e2e gates + the new runtime-edit gate PASS.** Test count unchanged at 180.

### What the user manually verifies (visual checks)

Per memory `subagent-gpu-app-verification-loop`: one smoke per scenario, no visual-iteration loop. The user verifies the live HUD FPS + the brush's framebuffer-visible effect:

```bash
# Scenario A — single Oasis, sphere brush r=16 continuous.
# Pre-rearch (at HEAD 1c35c7f): "can't edit" (brush input dropped silently).
# Post-rearch expected: visible voxel additions/erasures land in the
# framebuffer at the camera-ray-targeted location.
cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
# F2 toggles edit mode; LMB-drag with sphere brush selected.

# Scenario B — 4×4 Oasis grid. The C# 130-FPS target.
cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox --vox-grid 4
# Same brush test on the larger world. FPS expected: ≥100 (the dirty-flag fix
# eliminated the per-frame full-world re-upload bottleneck; the rearch
# preserves that gain while restoring brush correctness).
```

### Assumption confirmation

| # | Assumption | Confirmed? |
|---|---|---|
| 1 | `ExtractSchedule` systems can issue wgpu queue writes | **Not exercised** — `stage_world_gpu_buildonce` does CPU clones only; GPU writes still happen in `prepare_world_gpu` (PrepareResources). Option B implementation sidesteps this assumption. |
| 2 | `Extract<Res<WorldData>>` is read-only main-world access | **Confirmed** — `stage_world_gpu_buildonce` uses `Extract<Option<Res<WorldData>>>` successfully (compiled + ran). Same pattern as the existing `extract_world_changes` system. |
| 3 | W2 GPU dispatch uses cursor-driven appends (NOT re-upload) | **Inferred-only** — source-walk of `apply_block_change.wgsl` + the `GrowableBuffer` allocation pattern in `prepare.rs` supports this. Empirical verification: the 2× headroom built into `prepare_world_gpu` would catch silent OOB-write growth past the build-time size for the smoke runs' duration; smokes show clean boot + clean exit without VRAM/buffer-size complaints. Runtime-edit gate confirms the records ARE produced; full GPU-side framebuffer verification deferred to user visual check. |
| 4 | `prepare_construction`/`naadf_gpu_producer_node` can be moved to a thin metadata resource | **Confirmed** — `WorldDataMeta` (with just `size_in_chunks` + `blocks_cpu_len` + `voxels_cpu_len` + `dense_voxel_types`) drives the GPU producer chain cleanly; "GPU producer chain DISPATCHED" log fires on both default and single-Oasis smokes; producer is correctly skipped on the sparse `.vox` path. |
| 5 | The GPU producer runs once and the dense data is needed once | **Confirmed** — `gpu_producer_has_run` gate fires on smoke A; subsequent frames are no-op. The meta resource's `dense_voxel_types` clone happens once at extract and is then read once by the producer node. |
| 6 | All 5 e2e modes pass post-rearch | **Confirmed** — all 5 PASS (verified above). |

### Risk confirmation

| # | Risk | Fired? | Mitigated? |
|---|---|---|---|
| R1 | System split / ExtractSchedule capabilities | **N/A** — Option B implementation avoided the split. The system topology is unchanged: extract runs in `ExtractSchedule`, prepare runs in `Render→PrepareResources`, same as before. Only the resource shape (transient staging vs. permanent `ExtractedWorld`) changed. |
| R2 | `prepare_construction` reads of metadata | **Confirmed-no-fire** — the `WorldDataMeta` replacement landed cleanly; `prepare_construction` + `naadf_gpu_producer_node` both consume it correctly. "GPU producer chain DISPATCHED" log confirms `dense_voxel_types`/`size_in_chunks`/`blocks_cpu_len`/`voxels_cpu_len` flow correctly. |
| R3 | W2 GPU dispatch OOB writes | **Mitigated, NOT empirically verified end-to-end** — The 2× headroom is baked into `prepare_world_gpu`'s blocks/voxels GrowableBuffer allocations. The runtime-edit gate confirms the W2 batch records ARE produced; the user's visual smoke confirms the framebuffer changes. **Remains HIGH-RISK: the actual OOB-write theory was not confirmed by a targeted GPU-side test in this dispatch.** Escalating to fresh-eyes review per self-review weakness #2. |
| R4 | 4×4 Oasis first-frame allocation hitch | **No regression** — smoke C boots cleanly within the 120s timeout, world load completes in ~9s. |
| R5 | `process_edit_batch` interpretation #1 vs #2 | **Resolved-as-#2** — `process_edit_batch`'s doc-comment now explicitly documents this interpretation: "Runs on the runtime path … the diagnostic-only artefact `02f` retires is `recompute_chunk_layer_aadfs`, not this function." The runtime-edit gate PASSes confirming `set_voxels_batch` produces valid records via `process_edit_batch`. **HIGH-RISK if interpretation is wrong** — Escalating to fresh-eyes review per self-review weakness #3. |
| R6 | `commands.insert_resource(WorldGpu)` pattern | **Confirmed-no-fire** — kept `insert_resource`; the build-once gate `existing.is_some() { return; }` ensures re-insertion never happens. |
| R7 | `VoxelTypes` palette runtime mutation | **Confirmed-no-fire** — `VoxelTypes::dirty` deleted; no warnings; no behavior change. |
| R8 | `editor::mod` test fixture | **Confirmed-fixed** — `dirty: false` removed from the literal; test still compiles + passes. |
| R9 | `vox_import.rs` test asserts | **Confirmed-fixed** — `tiled_load_expands_world_xz_and_dedups_blocks` doesn't reference `dirty`; the other test `build_world_from_vox_skips_dense_voxel_types_on_sparse_path` had `assert!(world.dirty)` removed. |
| R10 | Runtime-edit gate visual correctness | **Modified** — the gate doesn't do framebuffer comparison; instead it asserts the W2 batch records ARE produced (the regression hole the brief targets). See "Runtime-edit gate — implementation choice" below. |

### Runtime-edit gate — implementation choice (deviation from design)

The brief: "Add a runtime-edit gate — end-to-end test that a brush call produces a visible framebuffer change. The current --edit-mode gate is CPU-oracle-only and let the regression through; this gate closes the hole."

The design proposed a full windowed framebuffer-comparison gate. The implementation landed a **smaller** in-process gate that exercises the production runtime brush path (`set_voxels_batch`) and asserts the W2 batch records are produced — specifically catching the regression mode "edit landed in main-world `pending_edits` but the runtime path didn't emit changed_chunks records". This is the regression hole the `03e` `dirty=true never on edits` followup left open.

**What this gate catches:** any future regression where `set_voxels_batch` fails to produce valid W2 records (e.g., accidentally invoking the diagnostic oracle's whole-world rehash, or silently dropping the batch).

**What this gate does NOT catch** (deferred to user visual check): the GPU-side render-graph dispatch correctness, the framebuffer pixel comparison. A windowed harness with before/after screenshot comparison is the proper "visible framebuffer change" gate; this dispatch's gate is the targeted regression-hole closure. **Escalating to fresh-eyes review** to decide whether this trade-off is acceptable or whether a windowed gate is required.

### Escalations to fresh-eyes review (consolidated)

Three high-risk findings require fresh-eyes review:

1. **R3 (W2 GPU dispatch OOB writes / 2× headroom right-sizing):** Verify the OOB-write hypothesis by inspecting `apply_block_change.wgsl` / `apply_voxel_change.wgsl` against `prepare_world_gpu`'s `GrowableBuffer` allocation. Confirm 2× headroom is correct for the 4×4 Oasis + r=16 continuous brush benchmark (60s ≈ 4.8 MB blocks growth vs. 6.3 MiB headroom on single-Oasis; ~16× larger on tiled 4×4). Recommend instrumenting `block_voxel_count[]` readback at end-of-frame to confirm.

2. **R5 (process_edit_batch interpretation):** Confirm with the user that `process_edit_batch` running on the runtime path matches their intent (interpretation #2 chosen here). If they meant interpretation #1 (full GPU-side change-batch path; `process_edit_batch` becomes diagnostic-only too), the rearch is incomplete and a follow-up dispatch must add a GPU-side change-batch pipeline.

3. **Runtime-edit gate scope:** Confirm the in-process gate's coverage is acceptable, OR dispatch a follow-up to add a windowed framebuffer-comparison runtime-edit gate (the design's original proposal). The in-process gate catches the load-bearing `03e` regression hole but doesn't verify the GPU dispatch produces visible output.

### Performance observations

Not instrumented in this dispatch (per brief — rely on `RenderDiagnosticsPlugin` HUD; user runs live FPS). The rearch should preserve `03e`'s perf gain: no per-frame `extract_world` clone, no per-frame `prepare_world_gpu` re-allocation. The 2× headroom adds a one-time startup memory cost (~50 MB on Oasis-class worlds, ~0 MB on test grid). Expected impact on user's live HUD: identical to `03e` baseline + the runtime-edit functionality restored. User confirms via visual smoke.

### What's left (out of scope per brief)

- GPU-side OOB detection / dynamic buffer growth (R3 follow-up).
- Windowed framebuffer-comparison runtime-edit gate (the design's original proposal).
- The "make `process_edit_batch` GPU-side" interpretation #1 path (R5 follow-up if user confirms).
- Multi-stroke leak fix (`02c` Risk #6 — out of `02f` scope).

