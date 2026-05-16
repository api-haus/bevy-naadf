# 02c — Design — Editing pipeline C# alignment

**Date:** 2026-05-15
**Author:** delegate-architect
**Branch:** `main` at HEAD post-`1c35c7f`
**Brief source:** orchestrator's "investigate c# editing through and through and design a proper alignment for our rewrite" dispatch.
**Benchmark target (binding):** C# sustains 130 FPS rendering 4×4 Oasis_Hard_Cover.vox (16 × ~265k chunks) with a continuous big brush. Port cannot sustain framerate on a single Oasis with a continuous r=16 brush.

---

## Overview

**C# model.** Brushes write per-voxel into a thread-local `editData[2048]` window per touched chunk (a sparse CPU staging buffer keyed by chunk index). At the end of `EditingHandler.Update`, ONE `processChunks` call runs `Parallel.For` over the edited-chunks list, hashes each chunk's 64 blocks, free-lists old voxel slots, packs `changedChunks`/`changedBlocks`/`changedVoxels` and writes the new chunk-state in-place into `dataChunk` via `WorldData.SetChunk` (which also calls `changeHandler.AddChangedChunk` when the empty/non-empty content boundary flips). `ChangeHandler.UpdateWorld` then runs all 21 BFS+addBounds sweeps on the queue (queue+`distanceFloodFill` are class state, mutated in place) and dispatches the 4 GPU passes — chunk → block → voxel → group. **The CPU `dataChunk` is never re-synced from the GPU; the CPU mirror's chunk-layer AADFs go stale, but C#'s CPU `RayTraversal` doesn't read AADF bits during descent (only state bits + ptr/type) so the stale AADFs are functionally invisible to CPU consumers.**

**Port model.** Brushes enumerate the full brush AABB voxel-by-voxel (no chunk inside/mixed split), build a flat `Vec<(IVec3, VoxelTypeId)>`, and call `WorldData::set_voxels_batch` once. That call: (a) decodes each touched chunk's current 2048-u32 window via `build_chunk_edit_window_from_world`; (b) applies per-voxel mutations into the window; (c) runs `process_edit_batch` once (per-block hash classify, no dedup, fresh slot append per mixed block); (d) appends to `voxels_cpu`/`blocks_cpu` (slots never reused); (e) **rewrites every chunk-layer AADF over the WHOLE world** via `recompute_chunk_layer_aadfs` (Bug 4 fix) and emits a synthetic `changed_chunks` upload entry per AADF-changed chunk. The render-world extract drains all batches once per frame, runs the full BFS+21-sweep CPU `compute_change_groups`, and the W2 GPU node dispatches the 4 passes.

**Gap.** Three algorithmic divergences compound:
1. **`recompute_chunk_layer_aadfs` per edit-frame is `O(N_chunks × 31 × 3)`** — ~5–10 ms per call on a single Oasis (~265k chunks), ~80–160 ms on a 4×4 Oasis grid (~4.2M chunks). C# never runs this; chunk-AADF refresh is incremental via the W3 regime-2 self-perpetuating queue. **The port pays this cost every brush-fire frame.**
2. **Per-batch fresh voxel/block slot appends** — `voxels_cpu` and `blocks_cpu` grow without bound across a stroke. C# free-lists old slots (`freeVoxelSlots.Enqueue`, `freeBlockSlots.Enqueue`) and reuses them. On a continuous stroke the port's CPU buffers grow ~1–2 MiB/frame on Oasis.
3. **Brush over-iteration** — the port walks the full brush-AABB voxel grid (`O(r³)`) regardless of which chunks the brush actually intersects. C# splits chunks into `inside` (fast `Array.Fill(2048)` per chunk) vs `mixed` (per-voxel test only on chunks straddling the brush boundary). At r=16, port does ~17k voxel ops; C# does ~17k voxel ops on mixed chunks only + ~125 × `Array.Fill` for inside chunks (negligible).

The realignment: **eliminate `recompute_chunk_layer_aadfs` from the runtime path** by trusting the W3 self-perpetuating queue (matches C#); **add C#'s chunk inside/mixed split** to the brushes; **add slot reuse** to `process_edit_batch` (or accept the leak as a slower but correctness-preserving compromise). The `--edit-mode` oracle gate keeps its current behaviour (1 set_voxel call); the brushes use the runtime path. The bit-exact W2 GPU oracle gates (in `world_change::tests`) are unaffected because they test `apply_*_change.wgsl` semantics, not the CPU staging.

---

## C# editing pipeline — full trace

### Stage 0 — Frame entry: `App.Update` → `WorldData.Update`

`WorldData.Update` (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:109-118`) runs once per frame in this exact order:

```
entityHandler.Update(gameTime, taaIndex);
editingHandler.Update(gameTime);   // ← brush input + processChunks
changeHandler.Update();            // ← BFS + 21 addBounds rounds + 4 GPU dispatches
boundHandler.Update();             // ← 5 rounds × regime-2 (W3, the background AADF refinement)
```

`gameTime` arrives in **milliseconds** (`App.cs:111`, confirmed `App.cs` line via the Bug-2/3 fix log at `03b-followup-editor-bugs-234.md:67-70`).

### Stage 1 — Brush input → `editData` staging

The currently-selected `EditingTool` runs in `EditingHandler.Update` (`EditingHandler.cs:44-49`). For Sphere brush (`EditingToolSphere.cs:27-104`):

1. **Pick ray** (`Sphere.cs:32-38`): `WorldData.RayTraversal` from camera position along cursor ray; on hit, `pos = lerp(hitPos, pos, time-based lerp)` (`Sphere.cs:42-48`).
2. **Continuous gate** (`Sphere.cs:50-51`): `if (!isContinuous && Old.LeftButton == Pressed) return;` — return early on every frame after the first if not continuous.
3. **Chunk-AABB classification** (`Sphere.cs:53-74`):
   - Compute `minChunkPos`/`maxChunkPos` from `pos ± radius`, clamped.
   - For every chunk in the AABB: compute `distToPosSqr = |chunk_center − pos|²`.
   - If `distToPosSqr < radiusInsideSqr` → `chunksToEditInside[]` (chunk entirely inside the sphere — full-fill path).
   - Else if `distToPosSqr < radiusOutsideSqr` → `chunksToEditMixed[]` (chunk straddles the sphere boundary — per-voxel test path).
   - Else skip (chunk entirely outside).
   - `radiusInsideSqr = max(0, radius - |(7.5,7.5,7.5)|)²` (the chunk corner-to-center distance is √(3·7.5²) ≈ 13).
   - `radiusOutsideSqr = (radius + |(7.5,7.5,7.5)|)²`.
4. **Mixed-chunk path** (`Sphere.cs:75-90`, `Parallel.For` per chunk):
   - `pointer = editingHandler.getChunkDataToEdit(chunkPos)` (`EditingHandler.cs:182-211`): if this chunk isn't already staged, atomically allocate a fresh 2048-u32 slot in `editData[]` and call `worldData.FillChunkData(chunkIndex, editData, offset)` to **decode the chunk's current state into the slot**. Returns the slot pointer.
   - Loop over all 4096 intra-chunk voxel positions; for each, test `distSqr < radiusSqr`; if inside, call `editingHandler.setVoxelData(pointer, voxelPosInChunk, isErase ? 0 : ((1<<15) | selectedTypeRenderIndex))` (`EditingHandler.cs:228-242`). **`setVoxelData` writes ONE voxel half-word in the `editData` window. It does NOT touch `dataChunk`/`dataBlock`/`dataVoxel`. It does NOT enqueue anything.**
5. **Inside-chunk path** (`Sphere.cs:91-100`):
   - `pointer = editingHandler.getChunkDataToEdit(chunkPos)` as above.
   - `Array.Fill(editingHandler.editData, type | (type << 16), pointer, 2048)` — one memset over 2048 u32s. ZERO per-voxel cost.

**`getChunkDataToEdit` mechanics** (`EditingHandler.cs:182-211`):
- The `editChunkDataPointer: uint[chunkCount]` array maps `chunk_index → offset_in_editData` (or `0xFFFFFFFF` if not yet staged). Per-edit lookup is O(1) and **dedup happens automatically across multiple brush-fires within one frame**.
- On a stage miss: append to `editedChunks: List<int>`, advance `editDataCount += 2048`, possibly grow `editData` array.
- On a stage hit: return the existing pointer; the new write merges into the same chunk's slot.

**Cost per brush-fire, r=16, sphere, 1 Oasis (single chunk surface):**
- ~125 chunks touched (sphere r=16 covers ~5×5×5 chunks).
- ~5–15 inside-chunks (`Array.Fill(2048)` × 5–15 = trivially fast, <50 μs total).
- ~110–120 mixed-chunks × 4096 voxel tests × ~10 ns per test = ~5 ms per brush-fire on a single thread (but `Parallel.For` distributes across cores: ~1 ms on 8 cores).

### Stage 2 — `processChunks` (per-frame digest)

`EditingHandler.processChunks` (`EditingHandler.cs:75-180`) runs ONCE per frame at the end of `editingHandler.Update`.

- `Parallel.For(0, editedChunks.Count, ...)` (one task per uniquely-staged chunk this frame).
- Per task (`EditingHandler.cs:82-167`):
  1. For each of the chunk's 64 blocks: hash the 32-u32 block content via `blockHashingHandler.getHashOfBlock`, classify uniform-full vs mixed.
  2. For mixed blocks: `blockHashingHandler.AddBlock(hash, …, out isNew)` — open-addressing CAS-on-GPU-hashmap dedup. On `isNew`, claim a fresh voxel slot AND emit a `changedVoxels` upload entry (`EditingHandler.cs:111-117`).
  3. For chunks whose previous state was Mixed, walk their old 64 blocks; for each old Mixed block, call `DeleteBlock(hash, oldVoxelPointer)` — when the refcount hits zero, push the slot into `freeVoxelSlots` (`EditingHandler.cs:127-144`). **This is the slot reuse path that the port lacks.**
  4. If all 64 new blocks are uniform-same → chunk is Uniform; else claim a fresh block slot via `worldData.SetBlocks` and emit a `changedBlocks` upload entry.
  5. Build `newChunk = state-encoded ptr | content`; call `worldData.SetChunk(chunkIndex, newChunk)` (`WorldData.cs:381-394`):
     - Write `dataChunk[chunkIndex] = chunkData` (CPU mirror state).
     - If the old chunk was Mixed and the new isn't → `freeBlockSlots.Enqueue(oldBlockPtr)`.
     - **If the empty/non-empty content boundary flipped, OR the new state is empty → `changeHandler.AddChangedChunk(chunkIndex)`** (otherwise no group-queue enqueue).
- After the parallel loop: reset `editChunkDataPointer[]` entries to `0xFFFFFFFF`; clear `editedChunks`; reset `editDataCount = 0`.

**`AddChangedChunk` mechanics** (`ChangeHandler.cs:257-278`):
- Compute `groupPos = chunkPos / 4` and `groupIndex`.
- Under lock: if `distanceFloodFill[groupIndex] == 0x3FFFFFFF` (untouched), set it to `0x80000000` (reset-completely marker), push to `changedGroups[]`, enqueue `groupPosComp` into `floodFillQueue`.

**Cost of `processChunks` on a continuous Oasis brush, r=16:**
- ~125 chunks × per-chunk block hashing (64 blocks × 32 u32s + 64 dedup lookups) = ~150 µs/chunk × 125 / 8 cores = ~2.3 ms.
- Plus `Array.Copy(dataVoxel, pointer, changedVoxels, ...)` per new voxel slot = trivial.
- Total per-frame `processChunks` ≈ 3–4 ms on 8 cores.

### Stage 3 — `ChangeHandler.UpdateWorld` (per-frame BFS+addBounds+GPU dispatch)

`ChangeHandler.UpdateWorld` (`ChangeHandler.cs:69-255`) runs once per frame after `processChunks` (via `worldData.Update` ordering at `WorldData.cs:115-116`).

- **Loop 1 — BFS-expand** (`ChangeHandler.cs:73-110`): drain `floodFillQueue` (queue is **class state, NOT reset per call** — the queue mixes the directly-edited groups from this frame's `AddChangedChunk` calls AND any leftover from prior frames). For each dequeued group, walk the 27-neighbour 3³ shell; for any untouched neighbour, set `distanceFloodFill[next] = curDist + 4`, push to `changedGroups[]`, **and re-enqueue only if curDist < 28** (cap-28 reach: ~7 hops).
- **Loop 2 — addBounds propagation** (`ChangeHandler.cs:124-174`): for each of 7 rounds, run 3 sub-passes (one per axis). Each pass iterates `changedGroups[originalChangedGroupCount..]` (the BFS-touched groups, NOT the directly-edited ones), examines the ±axis neighbour, calls `addBounds` which conditionally bumps the per-axis 5-bit AADF by `4 << boundsLocation`. **All 7 rounds in one call.**
- **Pack output** (`ChangeHandler.cs:175-183`): for each `changedGroups[i]`, emit `(groupPosComp, distance)` where `distance = 0xC0000000` for directly-edited groups OR `distanceFloodFill[g]` for BFS-touched groups. After packing, reset `distanceFloodFill[g] = 0x3FFFFFFF` (so next frame's BFS starts fresh on these groups).
- **GPU dispatches** (`ChangeHandler.cs:203-249`): chunk → block → voxel → group, in that order. Each `if (count > 0) { SetData; ApplyCompute; DispatchCompute; }`. **Reset all 4 counts to 0 at the end** (`ChangeHandler.cs:251-254`).

**Cost of `UpdateWorld` on a small batch (r=16 brush, ~125 chunks ≈ ~30 groups):**
- BFS-expand: 27 × 30 BFS hops × O(1) per hop = trivial (~30 µs).
- addBounds: 7 × 3 × ~300 group-cells × O(1) per cell = trivial (~50 µs).
- GPU SetData + dispatch: ~50 µs.
- Total: <1 ms.

### Stage 4 — `WorldBoundHandler.Update` (background AADF refinement, regime-2)

`WorldBoundHandler.Update` (`WorldBoundHandler.cs:91-121`) runs **5 rounds per frame** (the `for (int i = 0; i < 5; ++i)` at line 113). Each round runs:
1. `PrepareGroupBounds` (`boundsCalc.fx:51-93`, 1 thread): scan `boundQueueInfo[0..32×3]` (32 bound sizes × 3 axes); find the lowest non-empty queue slot. Cap the dispatch count at `maxGroupBoundDispatch` (user-tunable, default 512×N). Record dispatch params in `boundRefinedInfo`. Pop groups from the queue.
2. `ComputeGroupBounds` (`boundsCalc.fx:118-193`, indirect dispatch): for each group, refine its per-axis 5-bit AADF via `addBoundsGroup` (read neighbour chunk, compare bounds, bump current).
3. **Self-perpetuating** (`boundsCalc.fx:174-191`): at the end of the dispatch, **re-enqueue the same group at `boundSize+1`** into `boundQueueInfo[next].size`. The queue grows itself: a group keeps refining its AADF up through size 31 over many frames.
4. The initial seed comes from `apply_group_change`'s tail (`worldChange.fx:91-112`): every BFS-touched group is enqueued at the lowest of its 3 axis bounds.

**Critical:** the C# `WorldBoundHandler` does **NOT** run "regime-2 over all groups per frame" as `03b-followup-editor-bugs-234.md:189` Risk #8 suggests. It runs 5 rounds × {prepare + indirect dispatch on whatever the BFS-seeded queue holds, refined incrementally by self-enqueueing}. The "whole world refresh" emerges OVER MANY FRAMES, not per-frame.

**Cost:** ~5 × {prepare ~10 µs + dispatch ~10–100 µs depending on `maxGroupBoundDispatch`} = ~0.5 ms per frame.

### Stage 5 — Where does the CPU `dataChunk` get its AADFs refreshed?

**It doesn't, after `GenerateWorld`.** `dataChunk` is filled once at world-build time (`WorldData.cs:81 + 183 + 193`) and never re-synced from GPU. After editing:
- `SetChunk` updates the chunk's `state | content` (`WorldData.cs:381-394`).
- For chunks that **don't** flip the empty/non-empty boundary, `SetChunk` does NOT call `AddChangedChunk` (`WorldData.cs:392-393`) — the chunk's AADFs in the CPU mirror are never updated, even on the GPU.

**Why this is OK in C#:** `RayTraversal` (`WorldData.cs:396-473`) descends chunk → block → voxel **using only state bits + pointers** (`WorldData.cs:431-454`). The AADF bits in `dataChunk[i]` are **never read** during traversal — `boundsInDir` is recomputed from `voxelPosInChunk` (cell-local position), not from the AADF. So stale CPU AADFs are functionally invisible to the CPU consumer.

The port's CPU `ray_traversal` (`crates/bevy_naadf/src/world/data.rs:294-478`) is a faithful port of this — line `:374` computes `bounds_in_dir` from `voxel_pos_in_chunk`, never from AADF bits. **The Bug-4-A diagnosis at `03b-followup-editor-bugs-234.md:29-35` claiming "stale CPU AADFs make ray_traversal misroute" is incorrect** — the AADFs are not read.

The real Bug 4 was Bug 4-B (`03b-followup-editor-bugs-234.md:31-33`): the **GPU chunks texture** retains stale saturated AADFs (from `addInitialGroupsToBoundQueue`'s seed of `0xFFFFFFFF`/`AADF=31` cells far from geometry) that overshoot the new geometry. The W2 regime-3 `apply_group_change` only refines BFS-touched groups; far-away saturated AADFs stay stale until the W3 regime-2 self-perpetuating queue gets around to them (over many frames). The Bug-4 fix's `recompute_chunk_layer_aadfs` + synthetic `changed_chunks` entries force-refresh the GPU AADFs in one shot.

---

## Port editing pipeline — full trace

### Stage 0 — Frame entry

Bevy's `Update` schedule runs (`crates/bevy_naadf/src/lib.rs`):
1. Panel input (`adjust_panel`, `mouse_interact_panel`).
2. `editor::apply_edit_tool` (after `update_panel_text`).
3. (Other Update systems.)
4. `Last` schedule: `clear_world_data_pending_edits` (drains the per-frame queue).
5. Render world `ExtractSchedule`: `extract_world_changes` (mirrors `pending_edits` → `ConstructionEvents` + runs `compute_change_groups`).
6. Render world `Core3d` graph: `naadf_world_change_node` (the W2 GPU dispatch — chunk → block → voxel → group, per-frame edit-event-gated).
7. Render world `Core3d` graph: `naadf_bounds_compute_node` (the W3 regime-2 — runs 5 rounds per frame per `ConstructionConfig.n_bounds_rounds`).

### Stage 1 — Brush input

`editor::apply_edit_tool` (`crates/bevy_naadf/src/editor/mod.rs:135-249`):
1. F2 toggles `edit_active`.
2. Bail if panel owns the click.
3. Cursor → world ray via `screen_to_ray` + `WorldData::ray_traversal`.
4. If LMB pressed: lerp `state.pos` toward hit (`dt_ms = delta_secs × 1000` per Bug-2/3 fix at `editor/mod.rs:212`).
5. Cube/Sphere `is_continuous` early-return.
6. Dispatch to `tools::*_brush`.

`tools::sphere_brush` (`crates/bevy_naadf/src/editor/tools.rs:117-145`):
1. Compute `brush_aabb` over `pos ± radius` — voxel-coord AABB clamped to world.
2. **Iterate the full voxel AABB** (`O(r³)` voxels): for each voxel, test `|v - pos|² < r²`; if inside, push `(voxel_pos, target_type)` into `edits: Vec<(IVec3, VoxelTypeId)>`.
3. Call `world_data.set_voxels_batch(&edits)`.

**Cost of brush iteration, r=16 sphere:**
- AABB volume = `(2×16+1)³` ≈ 36k voxel tests.
- Sphere volume ≈ 17k voxels actually inside.
- For Paint: each inside voxel additionally calls `WorldData::get_voxel_type` (3-layer descent, ~30 instructions). 17k × 30 ≈ 0.5M instructions ≈ 0.2 ms.
- For Cube/Sphere: just push to Vec. ~17k pushes ≈ 0.1 ms.
- **Continuous-fire frame-budget on Oasis: ~0.3 ms here.**

**Cost at r=64 sphere:** AABB = 274k voxel tests, sphere volume ≈ 1M voxels inside. Vec grows to ~1M entries ≈ 16 MiB heap.
**Cost at r=400 sphere:** AABB = 514M voxel tests — Vec OOM.

The brush has NO C#-style chunk inside/mixed split — every voxel of the brush AABB pays the per-voxel test, even for chunks entirely outside the brush sphere.

### Stage 2 — `WorldData::set_voxels_batch` (the hot path)

`set_voxels_batch` (`crates/bevy_naadf/src/world/data.rs:567-759`):

1. **Group by chunk** (`:583-607`): allocate `HashMap<[u32; 3], Vec<([u32; 3], u16)>>`, insert all N edits. Cost: ~17k HashMap operations × ~50 ns = ~0.85 ms for r=16 sphere.
2. **Per-chunk window decode** (`:614-641`): for each touched chunk:
   - Allocate a 2048-u32 slice in `edit_data: Vec<u32>` (`:615`: `vec![0; chunk_count * 2048]`). For ~125 chunks: 256k u32s = 1 MiB heap allocation per brush-fire.
   - `build_chunk_edit_window_from_world` (`crates/bevy_naadf/src/aadf/edit.rs:362-415`): walk the chunk's state → blocks → voxels and decode into 2048 u32s. For a Mixed chunk this is 64 block reads + (for each Mixed block) 32 voxel-pair reads = up to 2048 random reads.
   - `set_voxel_in_window` (`aadf/edit.rs:421-447`) per per-chunk edit: ~17k/125 = ~140 calls per chunk, each O(1).
   - **Cost per chunk:** ~5 µs decode + ~2 µs writes = ~7 µs/chunk × 125 chunks = ~0.9 ms.
3. **`process_edit_batch`** (`:650-655`, calls `aadf/edit.rs:242-327`): for each of 125 chunks: hash 64 blocks (test uniform), for mixed blocks append 32 u32s to `batch.changed_voxels` + bump `v_cursor += 32`. No dedup, no slot reuse. Cost per chunk: ~10 µs hash + ~5 µs per mixed block × ~10 mixed blocks ≈ ~60 µs/chunk × 125 = ~7.5 ms.
4. **Apply to CPU buffers** (`:659-687`): push N voxel u32s into `voxels_cpu`, resize `blocks_cpu` and `apply_block_edit_cpu` (which runs `compute_aadf_layer([4,4,4], 3, ...)` per block). For ~1250 mixed blocks (125 chunks × 10 mixed/chunk avg): ~1250 × `compute_aadf_layer` calls of cost ~5 µs each = ~6 ms.
5. **`recompute_chunk_layer_aadfs` over the WHOLE world** (`:702-708`, calls `aadf/edit.rs:483-542`):
   - `compute_aadf_layer(size_in_chunks, 31, …)` over the entire chunks layer.
   - `O(N_chunks × 31 × 3 axes × inner cost)`.
   - For a single Oasis (265k chunks): ~25 M ops × ~3 ns/op ≈ **75 ms** (release build, single-threaded, per the prior log's "~5–10 ms" estimate may be optimistic for cache-unfriendly large layers).
   - For a 4×4 Oasis grid (4.2M chunks): ~400 M ops ≈ **1.2 s** per edit-frame.
6. **Synthetic `changed_chunks` for every AADF-changed chunk** (`:710-743`): every empty chunk whose AADF differs from before gets appended. On the first brush-fire of a session, **every empty chunk in the world** can show up here (the construction-time AADFs were max-cap, but the BFS reach + the brush's distance forces shrinks across half the world). For Oasis that's up to 200k synthetic entries × 2 u32s = 1.6 MiB upload per frame.

**Total `set_voxels_batch` cost on a single Oasis, r=16:**
- Steps 1–4: ~15 ms.
- Step 5: ~75 ms.
- Step 6: ~1.6 MiB upload bandwidth.
- **~90 ms per brush-fire frame** — frame budget exceeded by 5×.

**Total on 4×4 Oasis grid, r=16:**
- Steps 1–4: ~15 ms (brush-touched chunks don't scale with grid).
- Step 5: ~1.2 s (whole-world recompute).
- **~1.2 s per brush-fire frame** — catastrophic.

### Stage 3 — Per-frame digest

`clear_world_data_pending_edits` (`render/construction/mod.rs:580-585`) — drains `pending_edits` at the end of every frame, AFTER `extract_world_changes` has consumed it. Cost: O(1).

### Stage 4 — `extract_world_changes` (render-world extract)

`extract_world_changes` (`render/construction/mod.rs:657-752`):
1. Aggregate all `pending_edits.batches[*].changed_*` into one `ConstructionEvents` (one `Extend` per batch — but typically one batch per frame from `set_voxels_batch`).
2. **Run `compute_change_groups`** (`change_handler.rs:127-292`) on the unique edited group positions. Same BFS + 21-sweep algorithm as C#, all in one call. Cost: ~50 µs per ~30 groups.

### Stage 5 — W2 GPU dispatch

`naadf_world_change_node` (`render/construction/world_change.rs:362-448`): edit-event-gated, runs the 4 dispatches if `has_pending_changes()`. Cost on GPU: <1 ms typical.

### Stage 6 — W3 regime-2 dispatch

`naadf_bounds_compute_node` (per `bounds_calc.rs`, 5 rounds × prepare+indirect): identical self-perpetuating shape as C#. Cost: ~0.5 ms.

---

## Divergences — algorithmic gap

| # | Stage | C# behavior | Port behavior | Per-edit-frame cost contribution | Realignment direction |
|---|---|---|---|---|---|
| 1 | Brush voxel enumeration | **Chunk inside/mixed split**: inside-chunks get `Array.Fill(editData, type, 2048)` (one memset); mixed-chunks iterate 4096 voxels with per-voxel distance test. (`Sphere.cs:65-100`, `Cube.cs:62-101`, `Paint.cs:54-82`.) | **No split**: iterate the full brush AABB voxel-by-voxel, accumulate into `Vec<(IVec3, VoxelTypeId)>`. (`editor/tools.rs:117-145`.) | r=16: ~0.2 ms (modest); r=64: ~20 ms; r=400: **OOM via Vec**. | Adopt C#'s inside/mixed split. New API: `set_voxels_in_region(region, predicate_fn)` OR pre-classify chunks and feed `WorldData` two distinct entry points (`set_chunk_uniform` + `set_voxels_batch`). |
| 2 | CPU mirror chunk-layer AADF maintenance | **Never refreshed after `GenerateWorld`**. CPU `RayTraversal` doesn't read AADF bits, so staleness is invisible. (`WorldData.cs:431-454`, `WorldData.cs:381-394`.) | **Full-world `compute_aadf_layer` per edit-batch** via `recompute_chunk_layer_aadfs` (Bug-4 fix, `aadf/edit.rs:483-542`). | Oasis (265k chunks): **~75 ms/frame**. 4×4 Oasis: **~1.2 s/frame**. | **Remove `recompute_chunk_layer_aadfs` from the runtime path.** The W3 regime-2 self-perpetuating queue already refreshes far-away AADFs over many frames — same as C#. Move the recompute to the oracle path (`--edit-mode` / `gpu_construction_enabled=false` fallback). |
| 3 | GPU chunks-texture stale-AADF refresh | **W3 self-perpetuating queue** (`boundsCalc.fx:174-191`): each refined group re-enqueues at the next bound size. AADFs converge over many frames at `maxGroupBoundDispatch` rate. | Same self-perpetuating queue is implemented in the port's `bounds_calc.wgsl`, BUT the Bug-4 fix bypasses it by force-uploading the whole-world recompute. | Bypassed currently. | Trust the W3 queue. If the visible-stale-AADF artifact returns post-removal, tune `n_bounds_rounds` (currently 5) and `max_group_bound_dispatch` (default 512) upward to accelerate convergence on the first few post-edit frames. The user can already drive these via the panel (`max_group_bound_dispatch` is a knob). |
| 4 | Slot reuse on edits | **Free-lists** (`freeVoxelSlots: ConcurrentQueue<uint>`, `freeBlockSlots: ConcurrentQueue<uint>`). Old voxel slots are reclaimed when `DeleteBlock` refcounts to zero (`EditingHandler.cs:127-144`). Old block slots reclaimed when chunk leaves Mixed state (`WorldData.cs:388-389`). | **No reuse.** Every edit appends fresh slots. (`aadf/edit.rs:289` `v_cursor += 32`, `:312` `b_cursor += 64`.) Comment at `aadf/edit.rs:223-227` acknowledges this as a "simplified port". | Continuous r=16 brush at 60 fps: ~125 mixed blocks/frame × 32 u32s + 64 u32s × 125 = ~24 KiB/frame growth on `voxels_cpu` + `blocks_cpu`. **Over 60 s of stroke: ~1.4 MiB growth.** Multi-stroke editing: linear leak. Not a per-frame catastrophe but a long-session memory leak + GPU upload bloat. | Long-term: port `BlockHashingHandler.DeleteBlock` / `freeBlockSlots` / `freeVoxelSlots` (the dedup hash map + refcounts). Short-term (this design): accept the leak; the user can restart between sessions; the algorithmic match to C# is more important. |
| 5 | Voxel-block hash dedup | **CAS-on-hashmap** (`BlockHashingHandler.AddBlock` at `EditingHandler.cs:108`): identical 64-voxel content packs to the same slot. Compression factor logged at `WorldData.cs:106`. | **No dedup** (`aadf/edit.rs:221-227`). Identical content gets a fresh slot every time. | Continuous brush re-painting the same area: 2× to 5× redundant `voxels_cpu` growth depending on stroke pattern. | Defer (matches Decision 5 below). The C# dedup uses a GPU CAS hashmap; porting it is W1-style work. Short-term: leak. |
| 6 | `processChunks` parallelism | **`Parallel.For` over edited chunks** (`EditingHandler.cs:82`): per-chunk re-hash runs on `ThreadPool`. | **Serial**: `set_voxels_batch` loops over chunks single-threaded (`world/data.rs:618-641`, `aadf/edit.rs:252-324`). | r=16 brush, ~125 chunks: serial ~7.5 ms vs 8-core parallel ~1 ms. **6.5 ms/frame budget gap.** | Within `set_voxels_batch`, replace the per-chunk loop with `bevy_tasks::compute_task_pool().scope` (or `rayon::par_iter`). The per-chunk work is read-only on `chunks_cpu` (decode) + emits to per-chunk-local output (`window_slice`); aggregation at the end is sequential. Sanctioned divergence — already noted in `01-context.md` Q&A row 7 ("set_voxels_batch" is itself a sanctioned perf divergence). |
| 7 | BFS pacing | **All 7 rounds per `UpdateWorld` call** (`ChangeHandler.cs:124`). Queue+`distanceFloodFill` are class state; per-call drains the queue completely. | **Same — all 7 rounds in one `compute_change_groups` call** (`change_handler.rs:127-292`). | No divergence; identical cost. | None. (Note: the `12-alignment-gap.md` row 19 line "7 rounds × 3 axes per round = 21 sweeps" matches both impls; the cap-28 distance step is also bit-faithful.) |
| 8 | `set_voxel` (single-voxel API) | Per-voxel `setVoxelData` is staging-only — same merge into per-chunk `editData` slot (`EditingHandler.cs:228-242`). Real work batched into one `processChunks` per frame. | `set_voxel` (`world/data.rs:106-263`) **does the full pipeline per call** — decode window, mutate, encode, apply, **`recompute_chunk_layer_aadfs`**. Each call is O(N_chunks). | Test/oracle path only (the `--edit-mode` validation gate calls once; the brushes use `set_voxels_batch`). Not a runtime-hot-path cost contributor today. | Either leave (`--edit-mode` gate is single-call and tolerates the cost) or extract a shared internal helper that both `set_voxel` and `set_voxels_batch` invoke without the AADF recompute. The redesign retires the recompute from the runtime path anyway, so this divergence is incidental. |
| 9 | GPU chunks-texture `apply_chunk_change` upload volume | Per edit-frame: ~`changedChunkCount = edited-chunks-count` (~125 for r=16) entries × 8 bytes = ~1 KiB. | Per edit-frame post Bug-4 fix: up to **whole-world AADF-changed chunks** (~200k for a freshly-loaded Oasis's first edit) × 8 bytes = **~1.6 MiB** upload. The static buffer was bumped from 256 → 524288 entries (`mod.rs` per Bug-4 fix log). | Bandwidth: 1.6 MiB × 60 fps × 16 grid copies = ~1.5 GiB/s upload during continuous editing. **Likely the second-biggest cost contributor after the CPU recompute.** | Removing `recompute_chunk_layer_aadfs` removes this upload. Bump the static buffer back to ~256 (or grow on demand) once the runtime path no longer needs the synthetic entries. |

---

## Redesign — module layout + algorithm

### Files touched

| Path | Change |
|---|---|
| `crates/bevy_naadf/src/world/data.rs` | Split `set_voxels_batch` into two paths: a **runtime fast path** (no whole-world AADF recompute, no synthetic chunk uploads) and an **oracle path** (the current behaviour, gated behind a flag — used by `--edit-mode` and CPU-fallback). Add a NEW `set_chunk_uniform(chunk_pos: [u32; 3], ty: Option<VoxelTypeId>)` helper for the brush inside-chunk fast path (writes the chunk's full-fill in one go, single `changed_chunks` entry, ZERO block/voxel decode/encode work). Mark `set_voxel` as oracle-only via doc + rename to `set_voxel_oracle` (or leave the API — the `--edit-mode` gate at `mod.rs:2757` is the only runtime caller). |
| `crates/bevy_naadf/src/aadf/edit.rs` | `process_edit_batch` gets a slot-reuse companion (or keep the fresh-append behaviour for the first cut, document the leak). `recompute_chunk_layer_aadfs` stays — used by the oracle path only. |
| `crates/bevy_naadf/src/editor/tools.rs` | Re-implement `paint_brush` / `cube_brush` / `sphere_brush` with the **C# chunk inside/mixed split**. Each tool now classifies chunks into 3 buckets (outside / inside / mixed) and emits one of two API calls per chunk: `set_chunk_uniform` (for inside, with the target type or `EMPTY`) or `set_voxels_in_chunk` (for mixed — passing only the chunk's voxel mutations). |
| `crates/bevy_naadf/src/render/construction/mod.rs` | (optional, second-pass) shrink `W2_CHANGED_CHUNKS_INIT` back to 256 once the synthetic per-edit chunk uploads stop. |

### New runtime path — `set_voxels_batch_runtime` (pseudocode)

The runtime path mirrors C# `EditingHandler.processChunks` semantics: per-chunk hash/classify, no whole-world AADF recompute, no synthetic chunk uploads.

```rust
impl WorldData {
    /// Runtime-path batch edit. Mirrors C# `processChunks` semantics:
    /// - Per touched chunk: decode → mutate → encode (process_edit_batch).
    /// - Emit ONE changed_chunks entry per touched chunk (the new state).
    /// - **No whole-world AADF recompute** — trust the W3 self-perpetuating
    ///   queue to refresh stale AADFs over subsequent frames, same as C#.
    /// - **No synthetic AADF-changed chunk uploads.**
    ///
    /// Sanctioned divergence from C#:
    /// - The simplified port appends fresh voxel/block slots (no free-list reuse).
    /// - The per-chunk processing loop uses bevy_tasks for parallelism.
    pub fn set_voxels_batch(&mut self, edits: &[(IVec3, VoxelTypeId)]) {
        // 1. Group by chunk (same as today, lines 583-607).
        let mut by_chunk = group_edits_by_chunk(edits, self.size_in_chunks);
        if by_chunk.is_empty() { return; }

        // 2. Per-chunk window decode + voxel mutations (parallel via bevy_tasks).
        //    Each chunk produces a fresh 2048-u32 window.
        let chunk_windows: Vec<(ChunkPos, [u32; 2048])> =
            bevy_tasks::ComputeTaskPool::get().scope(|s| {
                for (chunk_pos, per_chunk_edits) in by_chunk.drain() {
                    s.spawn(async move {
                        let mut window = build_chunk_edit_window_from_world(
                            &self.chunks_cpu, &self.blocks_cpu, &self.voxels_cpu,
                            chunk_index(chunk_pos, self.size_in_chunks),
                        );
                        for (voxel_in_chunk, ty) in per_chunk_edits {
                            set_voxel_in_window(&mut window, voxel_in_chunk, ty);
                        }
                        (chunk_pos, window)
                    });
                }
            });

        // 3. Run process_edit_batch ONCE — sequential, but per-chunk work is
        //    pre-decoded so this is the "merge" pass.
        let v_cursor = self.voxels_cpu.len() as u32;
        let b_cursor = self.blocks_cpu.len() as u32;
        let (batch, _, _) = process_edit_batch(
            flat_concat(&chunk_windows),
            &chunk_windows.iter().enumerate()
                .map(|(i, (p, _))| (*p, (i * 2048) as u32))
                .collect::<Vec<_>>(),
            v_cursor, b_cursor,
        );

        // 4. Apply to CPU buffers — push voxels, apply_block_edit_cpu for blocks,
        //    update chunks_cpu[ci] in place per batch.changed_chunks entry.
        //    Same as current set_voxels_batch lines 659-687.

        // 5. SetChunk-equivalent: for each batch.changed_chunks entry, check if
        //    the empty/non-empty boundary flipped (C# WorldData.cs:392-393).
        //    If yes → push to pending_edits.edited_groups.
        for entry in &batch.changed_chunks {
            let chunk_idx = decode_chunk_index(entry[0], self.size_in_chunks);
            let old_state = self.chunks_cpu[chunk_idx] >> 30;
            let new_state = entry[1] >> 30;
            let cur_has_content = old_state != 0;
            let new_has_content = new_state != 0;
            self.chunks_cpu[chunk_idx] = entry[1];

            if cur_has_content != new_has_content || !new_has_content {
                // SetChunk's AddChangedChunk gate — only enqueue if boundary
                // flipped OR new state is empty.
                let chunk_pos = unpack_chunk_pos(entry[0]);
                self.pending_edits.edited_groups.push([
                    chunk_pos[0] / 4, chunk_pos[1] / 4, chunk_pos[2] / 4,
                ]);
            }
        }

        // 6. NO recompute_chunk_layer_aadfs. NO synthetic changed_chunks.

        self.pending_edits.batches.push(batch);
        self.dirty = true;
    }
}
```

### New fast path — `set_chunks_uniform_batch`

For brushes that touch inside-chunks (the C# `chunksToEditInside[]` path), we can short-circuit the entire process_edit_batch pipeline. Each inside-chunk's new state is uniform (uniform full of `ty` or uniform empty); we write the `chunks_cpu[ci]` value directly + emit one `changed_chunks` entry.

```rust
impl WorldData {
    /// Mark a set of chunks as uniform-state. Used by brush inside-chunk path
    /// (C# `chunksToEditInside[]` + `Array.Fill(editData, type, 2048)`).
    /// One `changed_chunks` entry per chunk; ZERO block/voxel uploads.
    pub fn set_chunks_uniform_batch(
        &mut self,
        chunks: &[([u32; 3], Option<VoxelTypeId>)],
    ) {
        let mut batch = EditBatch::default();
        for &(chunk_pos, ty) in chunks {
            let ci = chunk_index(chunk_pos, self.size_in_chunks);
            if ci >= self.chunks_cpu.len() { continue; }

            // Encode the new chunk state. Uniform Full → state 1 | type;
            // Uniform Empty → state 0 (no AADF).
            let new_state = match ty {
                Some(VoxelTypeId::EMPTY) | None => 0u32,
                Some(t) => 1u32 << 30 | (t.raw() as u32 & 0x7FFF),
            };
            // If the previous chunk was Mixed, the block slots leak (matches
            // current set_voxels_batch behaviour — no free-list).

            let old_has_content = (self.chunks_cpu[ci] >> 30) != 0;
            let new_has_content = (new_state >> 30) != 0;
            self.chunks_cpu[ci] = new_state;
            batch.changed_chunks.push([pack_chunk_pos(chunk_pos), new_state]);

            // SetChunk's AddChangedChunk gate.
            if old_has_content != new_has_content || !new_has_content {
                self.pending_edits.edited_groups.push([
                    chunk_pos[0] / 4, chunk_pos[1] / 4, chunk_pos[2] / 4,
                ]);
            }
        }
        if !batch.changed_chunks.is_empty() {
            self.pending_edits.batches.push(batch);
            self.dirty = true;
        }
    }
}
```

### Realigned brushes — chunk inside/mixed split

```rust
// editor/tools.rs

fn sphere_chunk_classify(
    chunk_pos: IVec3,
    pos: Vec3,
    radius: f32,
) -> ChunkClass {
    // C# Sphere.cs:69-74.
    let chunk_center = chunk_pos.as_vec3() * 16.0 + Vec3::splat(8.0);
    let dist_sqr = (chunk_center - pos).length_squared();
    let chunk_corner_diag = (Vec3::splat(7.5)).length(); // √(3·7.5²) ≈ 13
    let r_inside_sqr = (radius - chunk_corner_diag).max(0.0).powi(2);
    let r_outside_sqr = (radius + chunk_corner_diag).powi(2);
    if dist_sqr < r_inside_sqr {
        ChunkClass::Inside
    } else if dist_sqr < r_outside_sqr {
        ChunkClass::Mixed
    } else {
        ChunkClass::Outside
    }
}

pub fn sphere_brush(
    world_data: &mut WorldData,
    pos: Vec3, radius: f32, ty: VoxelTypeId, is_erase: bool,
) {
    let target = if is_erase { None } else { Some(ty) };
    let (min_chunk, max_chunk) = brush_chunk_aabb(world_data, pos, radius);

    let mut inside_chunks: Vec<([u32; 3], Option<VoxelTypeId>)> = Vec::new();
    let mut mixed_chunk_edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();
    let r2 = radius * radius;
    let target_id = target.unwrap_or(VoxelTypeId::EMPTY);

    for cz in min_chunk.z..=max_chunk.z {
        for cy in min_chunk.y..=max_chunk.y {
            for cx in min_chunk.x..=max_chunk.x {
                let chunk_pos = IVec3::new(cx, cy, cz);
                match sphere_chunk_classify(chunk_pos, pos, radius) {
                    ChunkClass::Outside => continue,
                    ChunkClass::Inside => {
                        inside_chunks.push((
                            [cx as u32, cy as u32, cz as u32],
                            target,
                        ));
                    }
                    ChunkClass::Mixed => {
                        // Per-voxel test only on mixed chunks.
                        let chunk_origin = chunk_pos * 16;
                        for lz in 0..16 {
                            for ly in 0..16 {
                                for lx in 0..16 {
                                    let voxel = chunk_origin + IVec3::new(lx, ly, lz);
                                    let d = voxel.as_vec3() + Vec3::splat(0.5) - pos;
                                    if d.length_squared() < r2 {
                                        mixed_chunk_edits.push((voxel, target_id));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if !inside_chunks.is_empty() {
        world_data.set_chunks_uniform_batch(&inside_chunks);
    }
    if !mixed_chunk_edits.is_empty() {
        world_data.set_voxels_batch(&mixed_chunk_edits);
    }
}
```

`cube_brush` follows the same shape with `radiusInside = max(0, radius - 16)` and `radiusOutside = max(0, radius + 16)` per `Cube.cs:58-59`. `paint_brush` keeps the per-voxel `get_voxel_type` check (no inside-chunk path because Paint only replaces non-empty voxels — it never knows a whole chunk is "all paintable" without inspecting each voxel; we could add a `chunks_uniform_repaint` for fully-uniform-non-empty chunks but it's a corner case).

### Oracle path — `set_voxels_batch_oracle` (validation only)

The `--edit-mode` gate and any future "CPU-only" e2e (`gpu_construction_enabled=false`) need the old behaviour: full chunk-layer AADF recompute + synthetic chunk uploads, so the bit-exact post-edit state of `chunks_cpu` matches the pre-edit GPU construction.

```rust
impl WorldData {
    /// Oracle-path batch edit. Same as the current set_voxels_batch behaviour
    /// (Bug-4 fix preserved): runs recompute_chunk_layer_aadfs + emits
    /// synthetic changed_chunks entries for every AADF-changed chunk.
    ///
    /// Used by:
    /// - `--edit-mode` e2e validation gate (single set_voxel call).
    /// - CPU-fallback rendering when `gpu_construction_enabled = false`
    ///   (no W3 queue refines AADFs, so the CPU must keep the chunks_cpu
    ///   AADFs converged itself for the CPU-rendered output to be correct).
    pub fn set_voxels_batch_oracle(&mut self, edits: &[(IVec3, VoxelTypeId)]) {
        // Current set_voxels_batch body (world/data.rs:567-759) — UNCHANGED.
    }
}
```

The `--edit-mode` gate at `render/construction/mod.rs:2757` calls `world_data.set_voxel(...)` (single voxel). We can leave that path identical (`set_voxel` keeps its current `recompute_chunk_layer_aadfs` for bit-exactness with the gate's expectations) — the gate is a one-shot, cost is irrelevant. The brushes don't call `set_voxel` so the cost stays out of the runtime hot path.

---

## CPU-mirror consistency contract

### Post-redesign `chunks_cpu` state

| Bit | Meaning | Maintained by runtime path? | Read by which CPU consumer? |
|---|---|---|---|
| 30-31 | State (Empty / UniformFull / Mixed) | YES — `set_voxels_batch_runtime` writes `chunks_cpu[ci] = new_state` per `process_edit_batch` output entry | `ray_traversal` (`world/data.rs:382`), `get_voxel_type` (`:502`), `build_chunk_edit_window_from_world` (`aadf/edit.rs:370`) |
| 0-29 (Mixed) | Block ptr | YES — same | Same descent paths |
| 0-29 (UniformFull) | VoxelTypeId | YES — same | Same |
| 0-29 (Empty) | Packed 6×5-bit AADFs | **NO — stays stale post-edit** (matches C#). The W3 GPU queue refines GPU-side AADFs over many frames; the CPU mirror's AADF bits are never re-synced. | **No CPU consumer reads these bits** (verified — `ray_traversal:374`, `get_voxel_type:502`, `build_chunk_edit_window_from_world:367-415`). Stale AADFs are functionally invisible. |

### Contract surface

- **`WorldData::ray_traversal`** — reads state + ptr/type only. Always correct post-edit (boundary-flipping edits get `SetChunk`'s state update).
- **`WorldData::get_voxel_type`** — same.
- **`build_chunk_edit_window_from_world`** — same; the decode only reads state + ptr.
- **`compute_change_groups`** (BFS) — reads no `chunks_cpu` content; pure positional flood fill over `edited_groups`.
- **`extract_world_changes`** — extracts `pending_edits.batches` + runs BFS; no `chunks_cpu` reads.
- **W2 GPU dispatch** — consumes the upload arrays from `ConstructionEvents`; reads the GPU chunks texture (which is what `apply_*_change` mutates).

### Faithful-port preservation

The contract matches C# exactly:
- C# `dataChunk[i]` carries pre-edit AADFs for empty cells; same as port post-redesign.
- C# `SetChunk` writes only state+ptr; same as port post-redesign (`set_voxels_batch_runtime` step 5 writes `chunks_cpu[ci] = entry[1]` directly, and `entry[1]` is `process_edit_batch`'s output which has state bits + ptr but **zero AADF bits in its low 30**).
- C# `RayTraversal` ignores AADF bits; same as port.

### The Bug-4 fix's correctness contract — preserved how?

Bug 4 was the **GPU-side** chunks-texture stale-AADF artifact (visible "painted shapes terminate at axis boundaries depending on view angle"). With the redesign:
- Removing `recompute_chunk_layer_aadfs` removes the synthetic GPU upload that force-refreshed the GPU AADFs in one shot.
- The W3 regime-2 queue resumes the C#-style incremental refresh (as it did before Bug-4 was diagnosed but apparently not fast enough for the first few post-edit frames on freshly-loaded Oasis with saturated `AADF=31` cells).
- **Mitigation:** the user can crank `max_group_bound_dispatch` upward via the existing panel knob to accelerate convergence. The default 512×N may need bumping to e.g. 2048 for Oasis-class worlds.
- **Verification:** the Bug-4 manual visual test (`03b-followup:155-166`) must re-PASS post-redesign. If it doesn't, the W3 dispatch rate or the seed-on-edit strategy needs tuning.

The redesign accepts that **for a few frames post-edit, far-away AADFs may be stale on the GPU side too** — exactly what C# accepts. This is the algorithmic match.

---

## Test plan

### Preserved gates

1. **`cargo test --workspace --lib`** — all 173 existing lib tests continue to PASS. The Bug-4 unit tests at `aadf/edit.rs::tests::recompute_chunk_layer_aadfs_shrinks_stale_post_edit` and `:recompute_chunk_layer_aadfs_idempotent_on_converged_world` stay green — `recompute_chunk_layer_aadfs` is preserved as a function (used by the oracle path); only its caller chain changes.
2. **`set_voxels_batch_byte_equals_per_voxel_loop`** (`world/data.rs:862`) — relaxed to "effective-per-voxel-state equivalence" already; stays as-is.
3. **e2e `cargo run --bin e2e_render`** (baseline) — unaffected (no editor in e2e harness).
4. **e2e `--validate-gpu-construction`** — unaffected (no editor).
5. **e2e `--edit-mode`** — preserves the `set_voxel` → `set_voxel_oracle` path; the gate at `render/construction/mod.rs:2757` keeps its current behaviour. Gate stays PASS.
6. **e2e `--entities`** — unaffected.
7. **e2e `--vox-e2e`** — unaffected.

### New unit tests

8. **`sphere_brush_chunk_inside_path_uses_set_chunks_uniform`** — Build a 4×2×4 world, place a sphere r=24 at the center of a chunk so 1 chunk is fully inside the sphere; assert the resulting `pending_edits.batches[0].changed_blocks.is_empty()` (no block-level work for the inside chunk) AND `changed_voxels.is_empty()` (no voxel uploads for inside chunks). Assert `chunks_cpu[centre_chunk] >> 30 == 1` (UniformFull) and low 15 bits = `ty`.
9. **`sphere_brush_chunk_outside_path_skipped`** — Build a 4×2×4 world, place a sphere r=8 at one corner; assert chunks > 16 voxels away are NOT touched (`chunks_cpu` slice diff). Pre/post chunks_cpu only diffs in the brush-AABB chunks.
10. **`runtime_path_does_not_emit_whole_world_uploads`** — Build a 4×2×4 world; call `set_voxels_batch` with 1 voxel edit; assert `pending_edits.batches[0].changed_chunks.len() == 1` (not the whole world). This verifies the synthetic-AADF-upload path is gone.
11. **`oracle_path_byte_exact_to_pre_redesign`** — Build a 4×2×4 world; call `set_voxels_batch_oracle(&[(v, ty)])`; assert `chunks_cpu` matches the pre-redesign `set_voxels_batch` output byte-for-byte. This pins the oracle path's bit-exactness for the `--edit-mode` gate.
12. **`set_chunks_uniform_batch_basic`** — Build a 2×2×2 world; call `set_chunks_uniform_batch(&[([0,0,0], Some(VoxelTypeId(5)))])`; assert `chunks_cpu[0] == ChunkCell::UniformFull(VoxelTypeId(5)).encode()`.

### New microbenchmarks (Criterion or `cargo bench`)

13. **`bench_sphere_brush_r16_oasis`** — load Oasis_Hard_Cover.vox, simulate a sphere r=16 brush fire; assert wall-clock < 5 ms (target: <1 ms for steady-state continuous). Pre-redesign measurement: ~90 ms.
14. **`bench_sphere_brush_r64_oasis`** — same with r=64; assert < 20 ms. Pre-redesign: estimated >500 ms.
15. **`bench_sphere_brush_r16_oasis_4x4`** — 4×4 Oasis grid; assert < 8 ms. Pre-redesign: ~1.2 s.

### Manual visual gate (user)

16. Run `cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox`, F2 → editor, sphere r=16, hold LMB, drag across geometry — frame rate should sustain at the C#-equivalent 130 FPS (or close enough to remove the "catastrophically slow" complaint). The Bug-4 visual symptom (painted shapes terminate at axis boundaries) must NOT regress.
17. Repeat on a 4×4 Oasis grid (if `--vox` supports multiple loads or a configured grid) — frame rate stays usable.

---

## Decisions & rejected alternatives

### Decision 1: Runtime path vs oracle path — split via TWO APIs ✅ vs runtime-flag toggle vs delete oracle entirely

**Chosen:** Add `set_voxels_batch_oracle` as a parallel method on `WorldData` for the slow-but-bit-exact path; keep `set_voxels_batch` as the runtime fast path. The `--edit-mode` gate calls `set_voxel` (which keeps `recompute_chunk_layer_aadfs` for the existing test's expectations); the brushes call the new `set_voxels_batch`. CPU-only rendering (if it ever gets re-enabled via `gpu_construction_enabled=false`) calls `set_voxels_batch_oracle`.

**Rejected (a) — runtime flag:** A `WorldData::edit_mode: EditMode { Runtime, Oracle }` field that `set_voxels_batch` switches on. **Why rejected:** the flag is global mutable state with cross-test contamination risk, and the bifurcation is structural (different post-conditions on `chunks_cpu`), not just a perf switch.

**Rejected (b) — delete the oracle entirely:** Remove `recompute_chunk_layer_aadfs` outright; the W3 queue is enough. **Why rejected:** the `--edit-mode` gate's existing failure mode involves checking `chunks_cpu` byte changes (`mod.rs:2772`); removing the AADF recompute weakens this gate's signal. Keeping the oracle as a sibling preserves the gate's invariant.

**Flip trigger:** If a future change makes the oracle path's bit-exact `chunks_cpu` invariant unreachable (e.g., the GPU's chunks texture format diverges), the oracle becomes meaningless and gets retired.

### Decision 2: In-place CPU patches vs deferred CPU rebuild vs CPU eliminated entirely ✅ chosen "in-place per-chunk patches, no whole-world rebuild"

**Chosen:** In-place — `set_voxels_batch_runtime` writes `chunks_cpu[ci] = new_state` directly per `process_edit_batch` output entry, and the chunk's AADF bits stay stale (matches C#). No deferred CPU rebuild, no whole-world rehash.

**Rejected (a) — deferred CPU rebuild:** spawn a background task to recompute AADFs across all edited chunks, drain into `pending_edits` over frames. **Why rejected:** this re-introduces the same cost (~75 ms for Oasis whole-world AADF) just spread; the AADFs aren't read by CPU consumers anyway so the rebuild is wasted work.

**Rejected (b) — CPU eliminated entirely:** never write to `chunks_cpu` from edits; rely on the GPU + an occasional GPU→CPU sync for `ray_traversal`. **Why rejected:** `ray_traversal` is per-frame editor hover (cheap) but a GPU→CPU sync per frame is expensive (waits for GPU + DMA). C# also keeps the CPU mirror; matching C# is faithful.

**Flip trigger:** if the CPU `ray_traversal` is moved to the GPU (a `GpuRayTraversal` that the editor calls), the CPU `chunks_cpu` mirror could be deleted entirely. Not in this design's scope.

### Decision 3: AADF maintenance — runtime CPU recompute / GPU readback / lazy / hybrid ✅ chosen "trust W3 GPU self-perpetuating queue (lazy)"

**Chosen:** Lazy — let the W3 regime-2 queue refine AADFs incrementally over frames (matches C#'s WorldBoundHandler behavior verified at `boundsCalc.fx:174-191`). Tune `max_group_bound_dispatch` upward if the visible-stale-AADF artifact returns.

**Rejected (a) — runtime CPU recompute (status quo):** `recompute_chunk_layer_aadfs` per edit-frame. **Why rejected:** 75 ms/frame on Oasis is the dominant cost; eliminating it is the headline perf win.

**Rejected (b) — GPU readback:** at the end of each frame, read back the GPU chunks texture into `chunks_cpu`. **Why rejected:** GPU→CPU sync costs ~1–2 ms (DMA + fence wait); the readback is unnecessary since CPU consumers don't read AADF bits anyway.

**Rejected (c) — hybrid:** CPU recompute only on chunks near the BFS-edge (where regime-2 takes the longest). **Why rejected:** the cost of computing "which chunks need it" is the same as the recompute itself (one full pass over the chunks layer). Plus, no CPU consumer reads the AADF bits — the hybrid is solving a non-problem on the CPU side; the only side it matters on is GPU, which the W3 queue handles.

**Flip trigger:** if `cargo run --bin bevy-naadf -- --vox Oasis_Hard_Cover.vox` post-redesign shows persistent Bug-4-style visual artifacts after `max_group_bound_dispatch` is cranked, add a one-shot GPU-side dispatch that re-seeds the W3 queue for far-away chunks (matches C#'s `addInitialGroupsToBoundQueue` at `boundsCalc.fx:38-48`, which is called once on world load — we may want to re-call it after large edits).

### Decision 4: BFS pacing — verified ✅ both C# and port run all 7 rounds per frame in one call

**Verification:** C# `ChangeHandler.UpdateWorld:124-174` runs `for (int i = 0; i < 7; ++i)` inside `UpdateWorld`, one call per frame. Port `change_handler::compute_change_groups:217-269` runs `for _iter in 0..7` inside one call per `extract_world_changes` invocation, also per frame. **Identical pacing.** No change needed.

**Rejected (a) — pace over 7 frames:** distribute the addBounds work. **Why rejected:** C# doesn't do this; faithful-port rule applies.

### Decision 5: Bug 1 (async) — does the redesign retire it?

**Chosen:** Yes — the redesign **retires Bug 1 as currently scoped**. Post-redesign cost estimates: r=16 sphere on Oasis = <5 ms (target <1 ms with the parallelism win from Decision 6). At <5 ms per frame, continuous editing sustains 60+ FPS — well within the user's target. Async-edit infrastructure (futures + pending markers, drain over frames) is no longer load-bearing.

**Rejected — keep Bug 1 backlogged:** maintain the deferred async-edit work item. **Why rejected:** if synchronous edits hit the C# 130 FPS target, async edits become an irrelevance, not a separate dispatch. The deferred section in `docs/orchestrate/feature-completeness/README.md:75-84` should be retired or rescoped to "if r=400 brushes become a workflow, revisit; otherwise close."

**Flip trigger:** if post-implementation profiling shows the runtime path still costs >5 ms for typical brushes (r ≤ 32) on Oasis-class worlds, Bug 1 resurfaces — but the algorithm-level gap should be closed first.

### Decision 6: `recompute_chunk_layer_aadfs` — keep, modify, or remove? ✅ KEEP (used by oracle path); REMOVE FROM RUNTIME PATH

**Chosen:** Keep the function definition + its tests (used by `set_voxels_batch_oracle`); remove its call from `set_voxels_batch_runtime` and `set_voxel`'s runtime call.

**Rejected (a) — remove entirely:** delete the function. **Why rejected:** the oracle path needs it for bit-exact `chunks_cpu` equality with the C#-canonical "construct + edit + reconstruct" reference. The `--edit-mode` gate's `pre_edit_chunks == post_edit_chunks` check at `mod.rs:2772` benefits from the AADF-converged post-state.

**Rejected (b) — keep in runtime, make it incremental:** only recompute AADFs for chunks within BFS reach of any edit (subset of the world). **Why rejected:** the W3 GPU queue already does this incrementally; the CPU duplication adds no value (CPU consumers don't read AADF bits). Match C# — don't recompute on CPU.

**Flip trigger:** if the W3 GPU queue proves chronically too slow on cold-start large worlds (visible stale AADFs > 2 seconds after each edit), reintroduce a **bounded** CPU recompute as a fallback (e.g., only when GPU regime-2 is disabled). Not currently observed.

### Decision 7: Parallelism in `set_voxels_batch_runtime` — chosen `bevy_tasks::ComputeTaskPool` ✅ vs serial vs rayon

**Chosen:** Use `bevy_tasks::ComputeTaskPool::get().scope(|s| { s.spawn(...) })` for the per-chunk decode + mutate loop. Matches Bevy 0.19 conventions, available without new deps.

**Rejected (a) — serial:** keep the loop single-threaded. **Why rejected:** C# uses `Parallel.For` per chunk (`EditingHandler.cs:82`); matching this is faithful AND saves 5–7 ms on r=16 brushes.

**Rejected (b) — rayon:** add `rayon` dep. **Why rejected:** Bevy 0.19 already ships `bevy_tasks` with a compute pool; no new dep needed.

**Flip trigger:** if `bevy_tasks` overhead per spawned task dominates for small batches (~5 chunks), fall back to serial below a threshold.

### Decision 8: Chunk inside/mixed classification math ✅ chosen "verbatim port of C# `radiusInsideSqr` / `radiusOutsideSqr`"

**Chosen:** Sphere: `radiusInsideSqr = max(0, radius - 13)²`, `radiusOutsideSqr = (radius + 13)²` (where 13 = ||(7.5,7.5,7.5)||). Cube: `radiusInside = max(0, radius - 16)`, `radiusOutside = radius + 16`. Verbatim per `Sphere.cs:59-60`, `Cube.cs:58-59`.

**Rejected — simplified bound:** use a conservative box-vs-sphere clipping. **Why rejected:** C# uses the corner-diag distance as the cushion; matching this exactly preserves the C# semantic and avoids a class of "almost-inside" edge cases.

---

## Assumptions made

1. **C# `ChangeHandler.UpdateWorld` runs all 7 addBounds rounds in one call per frame.** Verified by reading `ChangeHandler.cs:124` (the `for (int i = 0; i < 7; i++)` loop is inside `UpdateWorld`). Would be invalidated by a future C# refactor that paces rounds; out of port's control.
2. **C# `BlockHashingHandler.DeleteBlock` + `freeVoxelSlots` is the slot reuse mechanism.** Inferred from `EditingHandler.cs:127-144` (the only call site I see) and the field declarations at `WorldData.cs:39`. Not bit-verified — C# may have additional GC paths I didn't inspect. The port's lack of slot reuse is a known sanctioned divergence; this assumption doesn't affect the design.
3. **C# CPU `dataChunk` is never re-synced from GPU after `GenerateWorld`.** Verified by grep across the C# tree showing only 2 callers of `dataChunkGpu.GetData(dataChunk)`, both in `GenerateWorld`. Would be invalidated by a missed sync path in code I didn't read.
4. **C# CPU `RayTraversal` doesn't read AADF bits.** Verified by reading `WorldData.cs:396-473` line-by-line; `boundsInDir` at `:433`+`:442` is computed from intra-cell position, not from AADF bits. Same for the port at `world/data.rs:294-478`. Solid.
5. **The C# `WorldBoundHandler` runs 5 rounds × {prepare+indirect dispatch} per frame, on whatever the BFS-seeded + self-perpetuating queue holds.** Verified by reading `WorldBoundHandler.cs:91-121` + `boundsCalc.fx:51-93,118-193`. The Risk-#8 claim at `03b-followup-editor-bugs-234.md:189` ("C# runs regime-2 over all groups per frame") is **incorrect** — refuted by direct shader read.
6. **The W3 GPU self-perpetuating queue is fast enough at default `max_group_bound_dispatch` to refresh post-edit AADFs without visible artifacts.** **Not yet verified** — this is the key assumption the redesign rests on. The Bug-4 fix log claims it wasn't fast enough on Oasis cold-start; that was diagnosed as needing the CPU recompute. The mitigation (tune `max_group_bound_dispatch`) is documented as a manual visual gate. **Flip:** if the artifact returns and tuning doesn't fix it, the design needs a one-shot CPU pass to re-seed regime-2's queue with the BFS-far-region groups.
7. **Bevy 0.19 `bevy_tasks::ComputeTaskPool` parallelism overhead is < 100 µs/spawn.** Bevy's ECS docs claim this; not measured for this design. If overhead dominates for ~5-chunk batches, the design switches to serial below a threshold.
8. **The chunk inside/mixed classification math (`radiusInsideSqr` etc.) ports byte-for-byte from C#.** Verified by reading the three brushes (`Sphere.cs:59-60`, `Cube.cs:58-59`, `Paint.cs:48`). Paint doesn't have an inside path because Paint only replaces non-empty voxels (no "uniform full-fill" semantic available).
9. **Removing `recompute_chunk_layer_aadfs` from the runtime path does NOT break the `set_voxels_batch_byte_equals_per_voxel_loop` test.** That test is already relaxed to "effective-per-voxel-state equivalence" (per Test #5 deviation note at `03b-impl-editor.md:336-343`), so byte-exact `chunks_cpu` is not its invariant. AADF differences in `chunks_cpu` won't fail this test.
10. **The brushes are the only runtime callers of the edit pipeline.** Verified by grep — the only non-test callers of `set_voxel` / `set_voxels_batch` are `editor/tools.rs` (brushes) and the `--edit-mode` validation gate (one call). No production code loops `set_voxel`.

---

## Risks & mitigations

| # | Risk | Mitigation |
|---|---|---|
| 1 | Bug-4 visual artifact regresses (painted shapes terminate at axis boundaries on cold-start) | Tune `max_group_bound_dispatch` upward (panel knob exists). If insufficient, add a one-shot post-edit dispatch that re-seeds regime-2's queue with far-from-BFS groups (matches `boundsCalc.fx:38-48` initial seed but called on demand). |
| 2 | `bevy_tasks` parallelism overhead dominates for small batches (~5 chunks) | Threshold the parallel path: if `chunk_count < 8`, run serially. (Bevy task spawn ~100 µs is roughly the cost of decoding 5 chunks serially.) |
| 3 | `set_voxels_batch_oracle` accidentally invoked from runtime code (perf regression) | Method names enforce intent (`_oracle` suffix). Add a debug-build `#[cfg(debug_assertions)]` log on every `set_voxels_batch_oracle` call so any unintentional runtime invocation surfaces. |
| 4 | The `--edit-mode` gate's `pre_edit_chunks == post_edit_chunks` byte-diff (`mod.rs:2772`) breaks because `set_voxel` now routes through a runtime path | Keep `set_voxel` calling the **oracle** internals (preserves the gate). The runtime split applies to `set_voxels_batch`, not `set_voxel`. Document this clearly in the function-level doc. |
| 5 | Chunk inside/mixed classification thresholds (`radiusInsideSqr` etc.) get the math wrong | Unit-test the classifier with hand-computed boundary cases: chunk just-barely-inside, chunk just-barely-outside, chunk at the boundary corner. The math is mechanical; the test pin is the brush-AABB-coverage assertion in Test #8. |
| 6 | `set_chunks_uniform_batch` leaks block-slot pointers when overwriting Mixed chunks | The existing `set_voxels_batch` already leaks slots (no free-list). The new method matches that behavior — same sanctioned divergence. Document at the method's doc. |
| 7 | Removing per-edit whole-world chunk-uploads breaks a downstream consumer | Verified: the only consumer of `changed_chunks` is `apply_chunk_change.wgsl` (the GPU pass). No CPU side reads `changed_chunks` for any other purpose. Safe. |
| 8 | The `W2_CHANGED_CHUNKS_INIT = 524288` static buffer cap is now over-provisioned (waste) | Optional second-pass: shrink back to 256 (or use a `GrowableBuffer`). Not load-bearing; defer to a separate cleanup PR. |
| 9 | The deferred Bug-1 (async edits) section in `README.md:75-84` becomes stale | Update or retire that section as part of the impl PR. Note explicitly that async is no longer required given the algorithmic alignment. |
| 10 | Continuous brush at r > 100 still stalls (millions of voxels) | The inside-chunk path scales O(chunks_inside) — at r=100, that's ~100³/16³ ≈ 250 chunks inside, all fast `set_chunks_uniform`. Only mixed-chunks pay per-voxel cost, and there are O(r²) of them (the brush surface). r=100 mixed-chunks ≈ ~300, ~300 × 4096 voxel-tests ≈ 1.2M tests ≈ 5 ms. r=400: ~5000 mixed chunks × 4096 tests ≈ 20M ≈ 80 ms — acceptable for a rare extreme brush, no longer catastrophic. |
| 11 | A future render-path divergence requires AADF-converged CPU mirror | The oracle path stays available. If `gpu_construction_enabled=false` is ever revived for CPU-only rendering, the editor switches to `set_voxels_batch_oracle` automatically (gated on the config flag). |
| 12 | The `recompute_chunk_layer_aadfs_shrinks_stale_post_edit` unit test no longer reflects production behavior (the function is only called by oracle path now) | Test stays valid (it's a unit test of the function's correctness, not a behavior assertion on `set_voxels_batch`). Add a doc comment on the test linking to this design's Decision 1 (the function survives for the oracle path). |

---

## Out of scope for this design

- **World-padding to keep brush AABB inside the world** — orthogonal to the edit pipeline; the brushes already clamp via `brush_aabb`.
- **Streaming retrofit** (large-world streaming where chunks load/unload on demand) — not currently planned; the design assumes the whole world fits in RAM.
- **Dynamic world resize** — out of scope.
- **GI shader / render-pipeline changes** — explicitly forbidden by the brief.
- **`naadf_gpu_producer_node` / `gpu_producer_skip_upload` changes** — explicitly forbidden.
- **Async edits (Bug 1)** — retired by this design (see Decision 5). The deferred section in `README.md:75-84` should be retired or rescoped.
- **GPU CAS hashmap for `BlockHashingHandler`** (the C# slot dedup) — this is W1-style construction work; out of scope for editing. The simplified port's "fresh slot per mixed block, no dedup, no free-list" stays.
- **Free-list slot reuse for `voxels_cpu` / `blocks_cpu`** — long-session memory leak; sanctioned divergence (Divergence #4 above). A future pass can port C#'s `freeVoxelSlots` / `freeBlockSlots` queues.
- **CPU-fallback rendering (gpu_construction_enabled=false)** — the oracle path is preserved as a hook for this, but no `--no-gpu-edits` e2e mode is added in this design.
- **One-shot post-edit regime-2 reseed** — listed as a Risk-#1 mitigation but only implemented if the visual gate fails.
- **Brush gizmo / sphere wireframe at hover** — UI concern, out of scope.
- **Undo / redo** — out of scope.
- **`set_voxel_oracle` rename** — keep `set_voxel` as the public name; document that it's oracle-shaped (slow + bit-exact). The `--edit-mode` gate is the only runtime caller; cost is irrelevant.
- **Multi-stroke recording / playback** — out of scope.
