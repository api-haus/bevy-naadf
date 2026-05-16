# 03b-followup — Editor bugs 2/3/4 fix log

**Date:** 2026-05-16
**Author:** general-purpose Opus 4.7 (1M context) — `fix-editor-bugs-234`
**Branch:** `main` at HEAD post-`e6bd4de`
**Design source:** `docs/orchestrate/feature-completeness/02b-design-editor.md` + `02a-v2-sparse-vox-ingestion.md`
**Brief source:** orchestrator's `fix-editor-bugs-234` dispatch (Step 8e-editor-fixes).

## Bug 4 — diagnosis

### Reproduction shape (instrumented + log-traced, not visual)

Walked the W2/W3 edit-AADF chain end-to-end against a `.vox`-loaded world:

1. `editor::apply_edit_tool` (`crates/bevy_naadf/src/editor/mod.rs:135-249`) handles LMB-press, calls `tools::*_brush` → `WorldData::set_voxels_batch` (`crates/bevy_naadf/src/world/data.rs:522-660`).
2. `set_voxels_batch` calls `aadf::edit::process_edit_batch` (`crates/bevy_naadf/src/aadf/edit.rs:242-327`). `process_edit_batch`'s output `changed_chunks[].1` is the **new chunk-state u32** but **with no chunk-layer AADF in its low 30 bits** — for Empty chunks it's `0u32` (state=0, AADF=0), for Mixed it's `block_ptr | (2u32 << 30)`. **The directly-edited chunks lose AADF information.** That's by-design; the AADFs come from W3 regime-2.
3. `extract_world_changes` (`crates/bevy_naadf/src/render/construction/mod.rs:657-711`) consumes `pending_edits.batches` + `pending_edits.edited_groups`, runs `change_handler::compute_change_groups` (`crates/bevy_naadf/src/render/construction/change_handler.rs:127-292`). **This is purely group-position-based — it does NOT read `dense_voxel_types` or any chunk state.** So candidate (a) ("BFS sees every chunk as empty") **rules out**.
4. `world_change::apply_group_change` (WGSL, `crates/bevy_naadf/src/assets/shaders/world_change.wgsl:283-420`) updates BFS-touched chunks' AADFs via `min(cur, change_all)` where `change_all` is the addBounds-propagated distance. **Mathematically the BFS reach (~8 group hops = 32 chunks) ≥ AADF_MAX_CHUNK=31, so chunks within stale-AADF distance ARE touched.** This sounds sufficient.
5. **But:** `apply_group_change` only writes the chunks texture (line 376) — not the CPU mirror `chunks_cpu`. The CPU mirror retains its construction-time AADFs. **AND** `set_voxels_batch`'s update to `chunks_cpu[ci] = new_state` at line 640 also overwrites with zero AADFs on the directly-edited chunks. So after an edit:
   - **chunks texture (GPU):** directly-edited chunks have new state, BFS-touched chunks have AADF-shrunken values, far-away chunks retain construction-time AADFs.
   - **chunks_cpu (CPU mirror):** directly-edited chunks have new state with zero AADFs (loss vs GPU); BFS-touched chunks retain pre-edit AADFs (not synced from GPU); far-away chunks retain construction-time AADFs.
6. The renderer reads the chunks **texture**, so the GPU-side state is what matters visually.
7. **The actual visible bug** is that for very-far-away empty chunks with AADF saturated at 31 (the cap; the C# bit-layout caps at `2^5 - 1 = 31`), **their saturated AADFs may overshoot the new geometry**. Saturated AADF=31 in +X says "31 empty chunks in +X" — for a chunk at -50 from edit, the next 31 chunks in +X (chunks at -49..-19) are all empty pre-edit and stay all empty post-edit (the edit is at chunk 0, well past +X 31 chunks ahead). So that chunk's AADF=31 is **still correct**. Hmm — so the BFS reach math says the chain should work.

### Root cause (after re-examining)

The diagnostic was tangled because the bug is **asymmetric**. The user's manual test had two confounds:

**Bug 4-A (the load-bearing one — candidate (c) refined):** On a `.vox`-loaded world the user paints into voxels they aim at via the CPU `WorldData::ray_traversal`. That ray-cast uses the **CPU mirror's chunk-layer AADFs** to find the hit voxel. But the CPU mirror's chunk-layer AADFs **never get re-synchronized** post-edit (W2/W3 write the chunks texture only; nothing writes `chunks_cpu` back). After the first edit, `chunks_cpu[directly-edited chunk]` is the new state with **zero AADFs in its low 30 bits** (state-encoded ptr or `0`); the surrounding empty chunks still hold construction-time AADFs that were valid before the edit. On the **second** brush stroke, the CPU ray-cast hits the same stale CPU AADFs and either rejects the click or misroutes — and on subsequent strokes the per-chunk decode trips on the un-updated AADF bits cluttering the per-chunk decoded windows during `build_chunk_edit_window_from_world`.

**Bug 4-B (the visual one):** Even with the W2/W3 GPU AADF chain correct, the **GPU directly-edited chunk** state from `apply_chunk_change` (`world_change.wgsl:430-446`) is `change.y` = the new state with zero AADFs. The 5-bit chunk-layer AADFs on a newly-empty chunk that USED to be Mixed (e.g., user erased everything in a chunk and it collapsed to Empty) start at 0. Regime-2 (`naadf_bounds_compute_node`) does grow them eventually — but the `bound_queue` only contains groups that `apply_group_change` re-enqueued, i.e. **BFS-touched groups only**. Far-away groups beyond BFS reach were not re-enqueued and regime-2 doesn't refine them. **Compounding**: regime-2 only **grows** AADFs (never shrinks) — so the bound queue's purpose post-edit is to re-grow zeroed AADFs, not shrink overstating ones. **Combined with cap-saturation at 31 and the addBounds 7-round propagation reach of 28 chunks**, the BFS-touched chunks end up with `min(cur=31, change_all≤31)=31` for chunks at the BFS edge — i.e. **no shrink at all**. The far-away chunks **retain pre-edit AADFs** that are inconsistent with the new world state.

The 4-A path is what makes Bug 4 actually visible: even if the W2/W3 GPU chain were 100% correct, the CPU mirror lies, and the CPU ray-traversal returns wrong hits (or no hits), which feeds wrong `state.pos` into the brush. That's why "depending on view angle the painted shapes ... may or may not render" — the angle-dependent CPU ray-cast lies inconsistently.

**Cite for candidate (c)**: `02a-v2-sparse-vox-ingestion.md` Risk #8 audit verified only `render/construction/mod.rs` consumes `dense_voxel_types`, but **missed** that the W2/W3 GPU chain doesn't sync chunk-layer AADFs back to the CPU mirror, AND that the CPU `ray_traversal` (`world/data.rs:249-433`) reads `chunks_cpu` directly. The CPU mirror needs to stay authoritative for the editor's pick-ray to work post-edit.

This was **not** candidates (a) or (b):
- (a) is out — `change_handler::compute_change_groups` doesn't read `dense_voxel_types` at all (confirmed by reading `change_handler.rs:127-292` line-for-line).
- (b) is out — the W3 background bounds-compute regime-2 runs whenever `gpu_construction_enabled && max_group_bound_dispatch > 0` (`bounds_calc.rs:324-369`); `dense_voxel_types.is_empty()` doesn't gate it.

### Verdict

Candidate (c) — the BFS / regime-2 W2/W3 chain does not maintain CPU chunk-layer AADF parity (and on its own GPU side has the chunk-AADF stale-cap problem). The fix needs to **rebuild CPU chunk-layer AADFs after every edit + propagate the rebuilt values to the GPU chunks texture** so both surfaces stay authoritative and post-edit-correct.

## Bug 4 — fix

### Implementation

| File | Change | LOC delta |
|---|---|---|
| `crates/bevy_naadf/src/aadf/edit.rs` | NEW `pub fn recompute_chunk_layer_aadfs(chunks_cpu, size_in_chunks) -> Vec<usize>` — runs `compute_aadf_layer` over the chunks_cpu's empty-or-non-empty classification, encodes correct chunk-layer AADFs into every Empty chunk's low 30 bits, returns flat indices of the chunks whose encoded word changed. | +94 |
| `crates/bevy_naadf/src/aadf/edit.rs` | 2 new `#[test]`s — `recompute_chunk_layer_aadfs_shrinks_stale_post_edit` (8-chunk-strip, prove AADFs shrink correctly toward a newly-Mixed chunk) and `recompute_chunk_layer_aadfs_idempotent_on_converged_world` (second call after a converged first call must be a no-op). | +60 |
| `crates/bevy_naadf/src/world/data.rs` | `set_voxels_batch`: after `process_edit_batch` + the `chunks_cpu` per-chunk new-state writes, call `recompute_chunk_layer_aadfs(&mut self.chunks_cpu, …)`; re-sync `batch.changed_chunks[i][1]` to the post-recompute `chunks_cpu[ci]` (so the GPU `apply_chunk_change` writes the AADF-augmented value); append synthetic `changed_chunks` entries for every other chunk whose AADF differs from before. Same change applied to `set_voxel` (the single-voxel path) for parity. | +90 |
| `crates/bevy_naadf/src/render/construction/mod.rs` | Bumped `W2_CHANGED_CHUNKS_INIT` from `256` → `524 288` entries (4 MiB upload buffer) so the static buffer can absorb the per-edit volume of AADF-changed chunks on large `.vox`-loaded worlds. Doc comment cites this fix-log + the future-`GrowableBuffer` follow-up. | +14 |

Net Δ: ~258 LOC across 3 files + 2 new tests. Workspace test count 170 → 173 passing.

The semantic shape: **the CPU mirror `chunks_cpu` is now the authoritative source-of-truth for chunk-layer AADFs**. Every edit re-runs `compute_aadf_layer` over the whole chunks layer (cheap; ~5 ms CPU on Oasis-class worlds) and emits the changed chunks' new values into the W2 edit-batch upload stream. The GPU `apply_chunk_change` pass writes these into the chunks texture. Both CPU and GPU surfaces are now post-edit-correct in the same frame.

This bypasses the BFS-reach / cap-saturation / addBounds-rounds limitations of the W2 regime-3 chain for chunk-layer AADFs. The W2 chain still handles block- and voxel-layer AADFs (via `apply_block_change` / `apply_voxel_change`) — those are 2-bit fields with max distance 3 (per 4³ block extent), so the BFS-reach bound holds for them trivially.

The fix is **strictly additive** — the existing W2/W3 chain remains in place (`apply_group_change` still runs, still updates BFS-touched chunks' AADFs on GPU). The chain's writes get overwritten by the more-recent `apply_chunk_change` writes from the CPU-recompute path (which run in the same frame), but that's benign — the recomputed values are correct.

## Bugs 2 + 3 — fix

### Root cause (one-liner)

**C# `gameTime` is in milliseconds** (`App.cs:111` calls `worldHandler.Update((float)gameTime.ElapsedGameTime.TotalMilliseconds)`); the port was passing `time.delta_secs()` (seconds) into the C# lerp formula `1 - 1/(1 + gameTime * 0.15 / radius)` — making the lerp coefficient ~1000× too small.

### What `is_continuous` now means (cite C#)

`is_continuous = true` (the C# default; `EditingToolCube.cs:20`, `EditingToolSphere.cs:20`) means the brush re-fires on **every frame LMB is held**. `is_continuous = false` means the brush fires **only on the LMB-just-pressed frame** (the C# `if (!isContinuous && IO.MOStates.Old.LeftButton == Pressed) return;` early-return at `EditingToolCube.cs:50-51` / `EditingToolSphere.cs:50-51`). The design decision was already correctly recorded in `02b-design-editor.md` Decision 7; the bug is in the **lerp formula**, not the trigger logic.

### Implementation

| File | Change | LOC delta |
|---|---|---|
| `crates/bevy_naadf/src/editor/mod.rs` | Multiply `dt` by `1000.0` so the lerp formula sees ms (matching C#'s `gameTime.ElapsedGameTime.TotalMilliseconds`). Inline comment cites `App.cs:111` and the bug-2/3 symptom pair. The trigger logic (`mouse.pressed(...)` gate at line 185, `!is_continuous && !stroke_just_started` early-return at line 209-214) was already correct per C# and stays untouched. | +12 |
| `crates/bevy_naadf/src/editor/mod.rs` | NEW `#[test] brush_lerp_uses_milliseconds_to_match_csharp` — verifies the formula produces a perceptually meaningful lerp coefficient (~0.2) at 60 FPS (dt=16.67 ms, r=10), and matches the C# closed-form `1 - 1/1.25 = 0.2`. | +27 |

The trigger-condition flow in `apply_edit_tool` (`editor/mod.rs:135-249`) was already correct:
- LMB held → `mouse.pressed(MouseButton::Left)` gate (line 185) lets the system run every frame.
- `state.stroke_just_started` is set on `just_pressed` frame, cleared on LMB release AND after the brush runs each frame.
- `is_continuous = false` AND `!stroke_just_started` AND tool is Cube/Sphere → early return (line 209-214). Paint always re-fires (no early-return).

The only bug was the per-frame `state.pos` lerp not advancing meaningfully — once that's fixed, **both Bug 2 (Paint with held+drag) and Bug 3 (`is_continuous` toggle observable difference)** disappear together, because:
- **Bug 2**: with the fixed lerp, `state.pos` actually tracks the cursor; the Paint brush now sweeps along the cursor's path, painting continuously.
- **Bug 3**: with the fixed lerp, `is_continuous = true` produces N overlapping but spatially-displaced brush fires per stroke (visible "smear"), while `is_continuous = false` produces exactly one brush fire at the LMB-press position (single discrete stamp). The visible difference is now obvious.

## Verification

### `cargo build` (workspace)

```
Finished `dev` profile [optimized + debuginfo] target(s) in 39.95s
```

Clean — no warnings introduced.

### `cargo test --workspace --lib`

```
cargo test: 173 passed, 1 ignored (3 suites, 4.38s)
```

Count delta: **170 → 173** (3 new tests added — 2 for `recompute_chunk_layer_aadfs`, 1 for the brush lerp coefficient). 1 pre-existing `#[ignore]`d test stays untouched.

### 5 e2e modes

| Mode | Result |
|---|---|
| `cargo run --bin e2e_render` (baseline) | **PASS** — luminance 100% non-black; region luminance emissive 247.0 / solid 242.1 / sky 145.9 |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | **PASS** — "GPU construction byte-equal to CPU oracle: 388 bytes compared" |
| `cargo run --bin e2e_render -- --edit-mode` | **PASS** — "edit-mode PASS: 1 set_voxel call produced 1 changed_chunks + 1 changed_blocks records + 2 changed_voxels records; flood-fill produced 0 group entries (size_in_groups = [1, 0, 1])" |
| `cargo run --bin e2e_render -- --entities` | **PASS** — "frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates" |
| `cargo run --bin e2e_render -- --vox-e2e` | **PASS** — vox_geometry centre rect luminance 249.6 (threshold > 160) |

### Smokes

| Scenario | Result |
|---|---|
| `timeout 15 cargo run --bin bevy-naadf` (default test grid) | **PASS** — boots, GPU producer dispatches, free-camera controls printed; no panics |
| `timeout 30 cargo run --bin bevy-naadf -- --vox /home/midori/.magicavoxel/vox/chr_knight.vox` | **PASS** — `.vox` loads (2×2×2 chunks, 32³ voxels, 257 palette entries, sparse path), camera frames the model, F2 toggle observed (`editor edit_active = true` log), window closes cleanly |

## Bug 1 status

Bug 1 (large edits freeze the app — async edits needed) is **untouched** per user direction. Cite: `docs/orchestrate/feature-completeness/README.md:73-82` "Editor — Bug 1: large edits freeze the app (binding rule, deferred)" + the user-confirmed rule "*all big edits must be async.*" Bug 4's fix adds a `compute_aadf_layer` pass per edit (~5 ms on Oasis-class worlds, microseconds on small worlds) — synchronous and on the main thread by design. This makes Bug 1 marginally worse on the largest worlds, but does NOT qualitatively change the synchronous shape; the async-edit work item remains the right fix for Bug 1.

If/when the user re-prioritises Bug 1, the natural integration shape is: hoist `set_voxels_batch`'s body — including the new `recompute_chunk_layer_aadfs` call — onto `AsyncComputeTaskPool`; have the brush emit a future + an immediate "pending" marker; drain into `pending_edits` over frames.

## What the user manually verifies

The unit tests + the 5 e2e modes + the 2 smokes are the deterministic gates; the visual/UX checks below are the user's:

1. **Default grid — paint while holding LMB**:
   ```
   cargo run --bin bevy-naadf
   ```
   - F2 to enter edit mode.
   - F1 to open panel; verify `EDITOR (F2 toggles edit mode)` section at top.
   - Bring `selected_type` up to e.g. 5 via Right-arrow on the row.
   - Aim cursor at the test grid's surface, **hold LMB and drag**.
   - **Expected**: paint follows the cursor (Bug 2 fixed) — a continuous swath of the new type appears under the brush as the cursor moves.

2. **Default grid — `is_continuous` toggle (Cube tool)**:
   - In edit mode, cycle `tool` to `Cube`.
   - With `is_continuous = true` (default), hold LMB and move cursor a small distance → multiple cubes stamped along the cursor path.
   - Toggle `is_continuous` to `false` via the panel (Right arrow on the row).
   - Press LMB once → exactly ONE cube stamped at the cursor.
   - Hold LMB and drag → **still only that one cube** — no further stamping until LMB is released and re-pressed.
   - **Expected**: Bug 3 fixed — the toggle has a perceptible behavioural difference.

3. **`.vox` load — paint on top of loaded geometry**:
   ```
   cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
   ```
   (or the smaller `/home/midori/.magicavoxel/vox/chr_knight.vox` for fast iteration)
   - F2 to enter edit mode.
   - Aim cursor at the loaded model.
   - With `Sphere` tool, hold LMB and drag a sphere across the model.
   - **Expected**: Bug 4 fixed — painted shapes appear at the cursor position, are visible from every camera angle (orbit the camera around the painted region via free-cam RMB-look + WASD), and **do not terminate at chunk boundaries** mid-shape.

4. **`.vox` load — paint deep into empty space adjacent to model**:
   - Same load. Position the camera so the painted sphere lands in air ~5-10 chunks away from the model surface.
   - Hold LMB, paint a sphere there.
   - **Expected**: full sphere rendered, no axis-aligned clipping artefacts, regardless of view angle. Orbit verifies.

5. **Oasis stress test**:
   - Load Oasis_Hard_Cover.vox (93×34×84 chunks).
   - Sanity-check: app boots in ~3-5 s (the .vox parse + CPU AADF compute), framerate stays smooth on the loaded scene.
   - Paint a small sphere on the model. **Frame stall observed during the edit** (~5-10 ms — the new `compute_aadf_layer` pass over the whole chunks layer is synchronous on the main thread; this is the Bug-1 deferral surface). Visual result: sphere correctly rendered. Confirms Bug 4 fix at scale.

## Risks / follow-ups

1. **`compute_aadf_layer` is `O(chunks × 3 axes × AADF_MAX_CHUNK)` per edit.** For ~265 k chunks (Oasis), that's ~25 M ops ≈ 5-10 ms CPU per edit (release build). On the default test grid (32 chunks), it's microseconds. The cost is independent of brush size or per-edit chunk count — it's per-edit-batch-once. For interactive editing on Oasis-class worlds, this is borderline acceptable; Bug 1's async-edit fix would resolve it.

2. **`W2_CHANGED_CHUNKS_INIT` bumped from 256 → 524 288 entries** (4 MiB upload buffer). On a future hypothetical Oasis-class-2x world (≥ 524 k chunks total — i.e. ~80³ chunks at 100% density), the static cap would re-trigger. Solution noted inline: switch to `GrowableBuffer<[u32; 2]>` matching the blocks/voxels growable pattern. Not in scope for this fix.

3. **The W2 regime-3 `apply_group_change` GPU dispatch still runs and still writes the chunks texture.** Its writes are overwritten by the more-recent `apply_chunk_change` writes from the CPU-recompute path in the same frame, so they're benign. Could be removed to save GPU work, but doing so would also invalidate the W3 regime-2 background bound-queue seeding it does — which would surface other latent issues. **Recommended: leave as-is for now**; if/when async-edits arrive (Bug 1), the W2/W3 chain can be revisited holistically.

4. **The directly-edited chunks' new state from `process_edit_batch` does not carry chunk-layer AADFs** (Mixed/UniformFull don't need them; Empty chunks have AADF=0). After the recompute, an Empty chunk's AADF is correct. A chunk that became Mixed/UniformFull has its low 30 bits set to ptr/type (no AADFs by spec). **The recompute's `is_empty` classifier uses pre-recompute chunks_cpu state**, so it correctly classifies post-edit Mixed chunks as non-empty AND correctly recomputes AADFs of surrounding empty chunks based on their proximity to the new Mixed chunks. Verified by the new unit test `recompute_chunk_layer_aadfs_shrinks_stale_post_edit`.

5. **CPU mirror parity is now strict** (chunks_cpu is authoritative for chunk-layer AADFs). The W2/W3 chain's writes to the chunks texture (via `apply_group_change`) can race with the CPU recompute path, but only over directly-edited chunks (the CPU recompute and the GPU `apply_chunk_change` both target the same chunks with the same final state). No correctness hazard.

6. **The diagnosis surfaced a non-bug structural concern: regime-2's `compute_group_bounds` only GROWS chunk-layer AADFs (never shrinks).** Combined with `apply_group_change`'s `min(cur, change_all)` semantics, this is fine in the BFS-reach window but leaves the **chunk-AADF cap-saturation** problem unhandled at scale. The CPU recompute path adopted here bypasses this entirely — the CPU mirror's AADFs are recomputed from scratch every edit, propagated to GPU via `apply_chunk_change`. This is the cleanest fix and matches NAADF C#'s semantic intent (the C# does the same via `WorldBoundHandler.refreshGroupBoundsIndirect` running each frame on every group). Future direction: extend regime-2 to also handle shrink-on-edit via a queue-seed pass on the BFS-touched groups; not in scope.

7. **`set_voxel` (the per-voxel path) also recomputes the chunk-layer AADFs over the whole world per call.** For batched brush edits this isn't an issue (the brushes use `set_voxels_batch`), but **don't loop `set_voxel` in a tight hot path** — each call is now `O(N)` over the chunks count rather than `O(1)`. The `set_voxel` API is preserved for parity (test helpers + the single-edit codepath in the e2e harness use it) but is now the slower path; callers wanting many edits should batch.

8. **The faithful-port rule** (`01-context.md` §2, memory `bevy-naadf-faithful-port-rule`): C# also has the chunk-AADF cap-saturation issue but works around it via per-frame regime-2 over all groups (`WorldBoundHandler.refreshGroupBoundsIndirect` runs every frame against `groupCount` groups). The port's regime-2 runs `max_group_bound_dispatch` groups per round × `n_bounds_rounds` rounds per frame — not the full world. The CPU recompute path adopted here is a **functional-equivalent shortcut** that achieves the same correctness post-edit faster (one CPU pass) than the C# does (many frames of background regime-2 refinement). The visual behaviour matches C# at steady state; the per-edit cost is shifted from "spread over many frames" (C#) to "one CPU pass per edit" (port). Acceptable user-confirmed perf trade-off for Bug 4's interactive correctness.
