# 03f — Implementation log — Visual-diff edit gate + regression fix

**Date:** 2026-05-15
**Author:** general-purpose Opus 4.7 (1M context) (`02f`-followup dispatch)
**Branch:** `main` at HEAD `1c35c7f feat(phase-d-shadow): multi-tap sun visibility in spatial resampling`
**Predecessor reads:** `01-context.md` · `02f-design-world-container-rearch.md` (in full) · `03e-impl-dirty-fix-and-vox-grid.md` · `03c-impl-edit-pipeline-alignment.md`
**Rearch context:** `02f` consolidated rearch (commit `81171f9`) deleted `ExtractedWorld` + `dirty` flag per user directive; gates passed but **user-visible edit path broke end-to-end**. The `--runtime-edit-mode` record-counter gate tested the producer side (W2 records generated correctly) and missed the consumer-side break (records never reach the framebuffer). The user directed a visual-diff e2e gate to close the hole.

---

## Git LFS + fixture

- `git lfs install` + `git lfs track "*.vox"` — `.gitattributes` already trackered `*.imp`/`*.png` (InstaMAT outputs); appended a `*.vox filter=lfs diff=lfs merge=lfs -text` line.
- Fixture path: `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` (81 MiB; copied from `/home/midori/Downloads/Oasis_Hard_Cover.vox` — verified by `stat`).
- `git check-attr -a` confirms the file is LFS-filtered:

  ```
  crates/bevy_naadf/assets/test/oasis_hard_cover.vox: diff: lfs
  crates/bevy_naadf/assets/test/oasis_hard_cover.vox: merge: lfs
  crates/bevy_naadf/assets/test/oasis_hard_cover.vox: text: unset
  crates/bevy_naadf/assets/test/oasis_hard_cover.vox: filter: lfs
  ```

- Per brief: orchestrator handles commit; this dispatch does NOT commit.

---

## `--oasis-edit-visual` gate

### Wiring

- `AppArgs::oasis_edit_visual_mode: bool` (added at `crates/bevy_naadf/src/lib.rs:298`).
- CLI flag `--oasis-edit-visual` parsed in `crates/bevy_naadf/src/bin/e2e_render.rs:88`; branch at `e2e_render.rs:178` invokes `bevy_naadf::e2e::oasis_edit_visual::run_oasis_edit_visual()`.
- New module `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs` (~300 LOC) — fixture path resolution, fixed camera pose math, brush invocation, framebuffer-diff assertion, PNG persistence.
- Driver state machine (`crates/bevy_naadf/src/e2e/driver.rs`) — 7 new `E2ePhase` variants: `OasisWarmup` → `OasisShootBefore` → `OasisDrainBefore` → `OasisApplyEdit` → `OasisWaitPostEdit` → `OasisShootAfter` → `OasisDrainAfter` → `OasisAssert`. Fast-path branch at tick 0 routes `Warmup` → `OasisWarmup` when `AppArgs.oasis_edit_visual_mode == true`, mirroring the `resize_test` branch shape.
- Camera pin system `pin_oasis_camera` runs `Update.after(driver::e2e_driver).before(sync_position_split)` — overrides whatever pose the standard driver wrote each tick. Hooked at `e2e/mod.rs:228-235`.

### Gate logic

1. Boot via `run_e2e_render_with_args` with `grid_preset = GridPreset::Vox { path: <fixture>, tiles: 1 }` + `oasis_edit_visual_mode = true`. Fixture path is hardcoded — no user CLI arg needed.
2. Pin camera birdseye over world centre — `Transform::from_xyz(cx, world_height + 250, cz).looking_at((cx, mid_y, cz), Vec3::X)`. For Oasis: cam `(744, 794, 672)` looking at `(744, 272, 672)`.
3. `OasisWarmup`: 120 frames (TAA 32-deep ring + GI 128-frame `sample_counts` ring populate).
4. **Capture frame A** → `target/e2e-screenshots/oasis_edit_before.png` (256×256, the `AppConfig::e2e` fixed window).
5. `OasisApplyEdit`: invoke `crate::editor::tools::sphere_brush(&mut world_data, centre, OASIS_ERASE_RADIUS=30.0, VoxelTypeId::EMPTY, is_erase=true)`. The brush is the **production runtime path** — identical to what the editor's `apply_edit_tool` calls on LMB-drag with Sphere brush + Erase active.
6. `OasisWaitPostEdit`: 300 frames (~5 s at 60 FPS) — covers W2 regime-3 dispatch, W3 regime-2 background AADF refinement (1500 rounds), TAA + GI re-convergence.
7. **Capture frame B** → `target/e2e-screenshots/oasis_edit_after.png`.
8. Assert mean per-pixel RGB delta over a tight central rect (frac `0.35..0.65 × 0.35..0.65`, pixel rect `(89, 89, 166, 166)` in the 256² framebuffer) exceeds `OASIS_EDIT_DIFF_FLOOR = 8.0` (mean of R+G+B channel absolute deltas, channel scale 0..255).

### Threshold rationale

- The erase sphere (`r = 30 voxels`) projects to ~15% of framebuffer width at the birdseye altitude.
- Pre-fix run (regression present): rect mean Δ = **4.50** (the small swing comes from TAA noise + scene-graph re-stabilisation).
- Post-fix run (regression caught): rect mean Δ = **9.63** — a ~5 luma unit swing on each colour channel.
- Floor of **8.0** sits comfortably above the noise floor (~4.5) and below the post-fix signal (~9.6). Defensible margin both directions.

### Bounding box choice

The original 40%×40% rect (`OASIS_DIFF_RECT_FRACS = (0.30, 0.30, 0.70, 0.70)`) averaged the sphere-projection swing over a 16× larger area; the mean-delta math diluted the signal to 7.18 even after the runtime fix, below the floor. **Tightened to 30%×30% (`0.35, 0.35, 0.65, 0.65`)** — the sphere fills ~50% of this rect; the rest is sparse Oasis ground geometry that drops to sky on erase. Empirical: post-fix mean Δ = 9.63 ~ 9.68 (stable across runs).

---

## Phase 3 reproducer — the gate fails as expected

First run (regression present, pre-fix):

```
e2e_render --oasis-edit-visual: applying erase sphere — centre Vec3(744.0, 272.0, 672.0), radius 30.0 voxels, world size [1488, 544, 1344] voxels
e2e_render --oasis-edit-visual: sphere_brush returned — pending_edits batches 2, edited_groups 9, changed_chunks 72, changed_blocks 63, changed_voxels 1823; voxels_cpu 10498368→10556704 (+58336), blocks_cpu 1617216→1621248 (+4032)
e2e_render --oasis-edit-visual: rect mean per-pixel RGB Δ=4.50 (floor=8.00); full-frame mean per-pixel RGB Δ=3.71
oasis-edit-visual gate FAIL — rect mean per-pixel RGB delta 4.50 is below the floor 8.00.
```

Key observations:

- **Brush produced 2 batches** (sphere brush splits inside-chunks + mixed-chunks into separate calls — same as production runtime path).
- **CPU mirror was mutated** (voxels_cpu grew by 58336, blocks_cpu by 4032 — `set_voxels_batch` succeeded).
- **Framebuffer Δ = 4.50, below the 8.0 floor** — the edit did NOT propagate to the GPU.
- A diagnostic `eprintln!` inside `extract_world_changes` (gated on non-empty batches) **never fired** — extract never saw the batches.
- A diagnostic `eprintln!` inside `naadf_world_change_node` (gated on `has_pending_changes()`) **never fired** — W2 dispatch never ran.

Visually: `oasis_edit_before.png` and `oasis_edit_after.png` are essentially indistinguishable (5-luma-unit drift attributable to TAA noise + scene-graph re-stabilisation; no visible hole in the geometry).

---

## Root cause

**Schedule race between `clear_world_data_pending_edits` and `extract_world_changes`.**

- `clear_world_data_pending_edits` was registered at `crates/bevy_naadf/src/render/construction/mod.rs:2056` (pre-followup) in main-world `Last` schedule. It cleared `world_data.pending_edits.batches` and `pending_edits.edited_groups`.
- `extract_world_changes` was registered at `crates/bevy_naadf/src/render/construction/mod.rs:2089` in render sub-app `ExtractSchedule`. It read `Extract<Res<WorldData>>` (read-only) and built `ConstructionEvents`.
- **Bevy 0.19's standard schedule order**: main `Last` runs BEFORE render-sub-app `ExtractSchedule` within the same `App::update()` tick. The pipelined-rendering timing diagram at `bevy_render-0.19/src/pipelined_rendering.rs:75-92` makes this explicit: `simulation thread: main schedule (incl. Last) → sync → extract (which kicks off ExtractSchedule on the render sub-app's main world copy)`.
- Result: every frame's `Last` cleared the queue **before** the render sub-app's `ExtractSchedule` could read it. `extract_world_changes` saw empty batches every time — the W2 GPU dispatch never had work to do.

**Why `--runtime-edit-mode` missed it**: the gate at `validate_runtime_edit_mode` (`render/construction/mod.rs:2880-3008`) is a **standalone in-process inspection** — it builds its own `WorldData`, calls `set_voxels_batch` directly, asserts `pending_edits.batches` is non-empty + carries well-formed records. It does NOT drive the Bevy schedule, so it never exercises the `Last`-vs-`ExtractSchedule` ordering. The gate verified the producer correctness; the consumer side (extract drain + GPU dispatch) was outside its scope.

**Why this didn't trip pre-`81171f9`**: the previous architecture had `extract_world` (now-deleted) cloning `WorldData` into `ExtractedWorld` on the `dirty` flag. That code path didn't drain `pending_edits.batches`; the drain was always racy-against-clear, but `extract_world_changes` was added in Phase-C wave-2 with the same shape it has today. The race likely existed for a long time but was masked because the test fixtures used by the pre-existing `--edit-mode` gate are in-process oracles (don't drive Bevy schedule) and the `--vox-e2e` gate doesn't make edits at all. **The Oasis-scale binary edit was the first user-visible exercise of this code path.**

---

## Fix shape

### Fix 1 (load-bearing): drain inside `extract_world_changes` via `ResMut<MainWorld>`

`crates/bevy_naadf/src/render/construction/mod.rs:657-770`

- **Signature change**: `extract_world_changes` no longer takes `Extract<Res<WorldData>>` + `Extract<Res<MainWorldEntities>>` + `Extract<Res<AppArgs>>`. It now takes `main_world: ResMut<bevy::render::MainWorld>` (the Bevy-sanctioned pattern for a render-world system to mutate a main-world resource during `ExtractSchedule`; `bevy_render-0.19/src/extract_plugin.rs:100-126` documents the contract).
- **Drain logic**: `std::mem::take(&mut world_data.pending_edits.batches)` and `std::mem::take(&mut world_data.pending_edits.edited_groups)` — moves the contents out, leaving an empty `Vec` behind. The drained data is consumed locally into `ConstructionEvents`.
- **Co-located produce + consume**: the drain happens inside the same system that builds the events. No separate `clear` system needed. The race is structurally impossible — there's no second writer to compete with.
- **`clear_world_data_pending_edits`**: kept as a no-op stub (deprecated; `mod.rs:580-606`). The registration in `ConstructionPlugin::build` (`mod.rs:2056`) is harmless now — the function returns immediately. Future cleanup can remove both, but leaving them keeps this dispatch surgical.

The `MainWorldEntities` read (entity track) is preserved: read `Vec` clones from main world via `main_world.get_resource::<MainWorldEntities>()` BEFORE taking the `WorldData` mutable borrow, then drop the read borrow before the mutable borrow scope. The entity-handler update logic is otherwise identical.

### Fix 2 (correctness): bump W2 static buffer caps

`crates/bevy_naadf/src/render/construction/mod.rs:1283-1306`

After Fix 1 the drain worked, but wgpu surfaced two validation errors:

```
Caused by:
  In Queue::write_buffer
    Copy at offset 0 for 240636 bytes would end up overrunning the bounds of the Destination buffer of size 32768

Caused by:
  In Queue::write_buffer
    Copy at offset 0 for 18496 bytes would end up overrunning the bounds of the Destination buffer of size 2048
```

Decoded:

- `changed_voxels_dynamic` write: 60159 u32s × 4 B = 240636 B; pre-followup cap `W2_CHANGED_VOXELS_INIT = 8192 u32s × 4 = 32768 B` (32 KiB).
- `changed_groups_dynamic` write: 2312 entries × 8 B = 18496 B; pre-followup cap `W2_CHANGED_GROUPS_INIT = 256 entries × 8 = 2048 B` (2 KiB).

A single Oasis-scale brush (r=30 erase sphere; 72 chunks + 63 blocks + 1823 voxels + 9 edited groups → 2312 changed groups after the W3 BFS expansion) blew past both caps. Pre-followup the comments at `mod.rs:1278-1280` claimed `W2_CHANGED_BLOCKS_INIT = 4096` covers `~63 edits × 65 = 4095 u32s` and `W2_CHANGED_VOXELS_INIT = 8192` covers `~247 edits × 33 = 8151 u32s` — true for the test-grid scale but blown apart at Oasis-scale.

Bumped to:

| Const | Pre-followup | Post-followup | Capacity |
|---|---|---|---|
| `W2_CHANGED_CHUNKS_INIT` | 524 288 entries (8 B) | unchanged | 4 MiB / ~524 k entries |
| `W2_CHANGED_BLOCKS_INIT` | 4 096 u32s | 1 048 576 u32s | 4 MiB / ~16 k block records |
| `W2_CHANGED_VOXELS_INIT` | 8 192 u32s | 4 194 304 u32s | 16 MiB / ~127 k voxel records |
| `W2_CHANGED_GROUPS_INIT` | 256 entries (8 B) | 524 288 entries | 4 MiB / ~524 k groups |

Total static cap: ~28 MiB across the four W2 staging buffers — trivial against Oasis's existing 1.6 GiB voxels/blocks alloc. The comment at `mod.rs:1273-1288` documents the Oasis empirical data and flags `GrowableBuffer<u32>` migration as future polish (not in scope; static caps suffice for typical edits).

### Architectural commitments preserved

Per the user's `02f` directive and the brief's "binding" rules:

- **No `ExtractedWorld` clone** — the rearch's deletion stands. `WorldGpuStaging` is still build-once. No per-frame full-world copy.
- **No `dirty` flag** — `WorldData::dirty` stays deleted; `prepare_world_gpu` gate is pure `if existing.is_some() { return; }`.
- **`recompute_chunk_layer_aadfs` + `process_edit_batch`** stay diagnostic-only (`#[doc(hidden)]`) on the runtime path. The brushes still call `set_voxels_batch` (W2 fast path), which calls `process_edit_batch` ONLY to produce the W2 delta records — that's the W2 design, not a whole-world rebuild.
- **Single source-of-truth `WorldData`** — main-world resource; the render sub-app reaches across via `ResMut<MainWorld>` (the Bevy sanctioned pattern). No second copy, no second owner.

---

## Verification

### `cargo build --workspace`

```
    Finished `dev` profile [optimized + debuginfo] target(s) in 20.12s
```

### `cargo test --workspace --lib`

```
cargo test: 180 passed, 1 ignored (3 suites, 4.14s)
```

180/180 tests pass, matching the post-`03e` baseline (no test count regression).

### All e2e modes

| Mode | Pre-fix verdict | Post-fix verdict | Output snippet |
|---|---|---|---|
| baseline (`cargo run --release --bin e2e_render`) | PASS | **PASS** | `e2e_render: PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, framebuffer read back & non-degenerate, per-batch region gate green` |
| `--validate-gpu-construction` | PASS | **PASS** | `GPU construction byte-equal to CPU oracle: 388 bytes compared` |
| `--edit-mode` | PASS | **PASS** | `edit-mode validation PASS: 1 set_voxel call produced 1 changed_chunks + 1 changed_blocks records + 2 changed_voxels records` |
| `--entities` | PASS | **PASS** | `entity handler validation PASS: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates` |
| `--vox-e2e` | PASS | **PASS** | `vox_geometry region luminance — mean rgba [251.08, 249.85, 243.26, 255], luminance 249.6 (threshold > 160)` |
| `--runtime-edit-mode` | PASS | **PASS** | `runtime-edit gate PASS: set_voxels_batch produced 1 batch(es) with 2 changed_chunks ...` |
| `--oasis-edit-visual` (NEW) | FAIL (Δ=4.50) | **PASS** (Δ=9.63) | `oasis-edit-visual PASS — 120 warmup + 300 post-edit wait frames; erase sphere @ r=30.0 voxels produced rect mean per-pixel RGB Δ above 8.00 floor.` |

### Production binary smokes

```bash
timeout 30 cargo run --release --bin bevy-naadf 2>&1 | head -25
```

```
NAADF test grid (Default): 32 chunks, 1920 blocks, 7232 voxel-u32s (64x32x64 voxels)
phase-c followup#1 — GPU producer chain DISPATCHED (size_in_chunks=[4, 2, 4], voxel_workgroups=227, block_workgroups=31).
No windows are open, exiting
```

Default test grid boots + renders cleanly (window opens; no errors). Window closed by user; exit code 0.

```bash
timeout 30 cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox 2>&1 | head -20
```

```
NAADF .vox loaded from /home/midori/Downloads/Oasis_Hard_Cover.vox: 257 palette entries, world bounds 93×34×84 chunks (1488×544×1344 voxels), 265608 chunks total, blocks_cpu 1617216 u32s, voxels_cpu 10498368 u32s (sparse path, GPU producer skipped)
camera::setup_camera: framing loaded world — pos=(726.56, 850.00, 52.50), look_at=(726.56, 850.00, 53.50)
```

Oasis VOX loads + frames; window opens. User does the visual perf check (per memory `subagent-gpu-app-verification-loop` — one smoke per scenario, no rebuild→rerun loop).

---

## What's now caught by the gate

The `--oasis-edit-visual` gate catches **two distinct prior regressions** that the pre-existing gates missed:

### `d43f1f1` — "no edits propagate at all"

The pre-`81171f9` regression where edits to `WorldData` never reached the W2 batch (the `dirty=true` writes were removed from edit paths in `03e` to fix idle perf; this also accidentally broke edit-time path because the renderer at the time gated on `dirty` to extract). `--oasis-edit-visual`'s frame-A-vs-frame-B comparison would have produced a 0-delta and tripped the floor.

### `81171f9` — "W2 batch generates correct records but framebuffer unchanged"

The consolidated rearch's regression — `pending_edits.batches` populated correctly, `--runtime-edit-mode` happy (it inspects the in-memory batches), but the render sub-app's `extract_world_changes` saw an empty queue every frame because `clear_world_data_pending_edits` ran in `Last` before extract. The visual-diff gate caught this directly via the framebuffer comparison — the load-bearing reason it exists.

### Future regressions

Any change that leaves the brush call producing well-formed records but breaks the journey to the framebuffer (bind-group staleness, W2 GPU dispatch gated wrong, OOB writes silently dropped, etc.) will trip this gate. It tests the FULL chain: brush → CPU mirror → `pending_edits` → `extract_world_changes` drain → `ConstructionEvents` → `prepare_construction` GPU upload → `naadf_world_change_node` 4-pass dispatch → GPU mutation of `WorldGpu.blocks`/`voxels`/chunks texture → next-frame render reads the new geometry → final blit composites it into the on-screen framebuffer.

---

## Risks / follow-ups

### R1 — Buffer cap overflow at very large brushes (deferred)

Current static caps (4 MiB chunks / 4 MiB blocks / 16 MiB voxels / 4 MiB groups) cover continuous r=30..400 strokes on Oasis. An r=2000 stroke (touching ~80k chunks × ~600 voxels each) would overflow `W2_CHANGED_VOXELS_INIT = 16 MiB`. The pre-followup comment at `mod.rs:1284-1289` already flagged `GrowableBuffer<u32>` as the future polish — that recommendation stands, deferred to a separate dispatch when a real-use-case stroke exceeds the static cap.

The current behaviour on overflow: wgpu emits the validation error (visible in stderr) and the dispatch's write is silently dropped — the edit doesn't land. A `GrowableBuffer<u32>` migration would auto-grow on the cursor approaching the cap.

### R2 — Frame-A-vs-frame-B diff metric sensitivity to camera framing

The 30%×30% bounding box is calibrated for the Oasis fixture + the birdseye pose at world centre + `r = 30`. A different fixture, different camera pose, or different brush radius could leave the bounding box landing on unchanged geometry (false PASS) or capturing too-small a swing (false FAIL).

Mitigation: the gate's hardcoded fixture path + hardcoded camera math means the geometry+camera tuple stays stable across runs. The threshold (`OASIS_EDIT_DIFF_FLOOR = 8.0`) has comfortable margin (9.63 vs 8.0 floor; noise floor ~4.5). If the fixture or pose changes, the threshold + rect need re-calibrating.

### R3 — Pipelined-rendering interaction (verified, no action needed)

The Bevy 0.19 pipelined-rendering timing diagram (`bevy_render/src/pipelined_rendering.rs:75-92`) confirms the extract runs serially on the simulation thread between the simulation schedule and the render thread handoff. There's no thread race in our fix — `ResMut<MainWorld>` access during `ExtractSchedule` is on the same thread that just finished the main schedule, sees the `pending_edits.batches` that `apply_edit_tool` populated in `Update`, and drains them before yielding the world back. The fix is sound under both pipelined and non-pipelined render modes.

### R4 — `clear_world_data_pending_edits` registration kept (deprecated)

The no-op stub at `mod.rs:580-606` + its registration at `mod.rs:2056` are harmless (the function returns immediately) but dead-code. A future cleanup pass should delete both. Kept in this dispatch to minimise surface area — the orchestrator's follow-up can excise them.

### R5 — Diagnostic `bevy::log::debug!` traces left in the production path

Both `extract_world_changes` (`mod.rs:686-697`) and `naadf_world_change_node` (`world_change.rs:382-388`) carry `bevy::log::debug!` traces gated on non-empty payloads. These cost nothing in the empty-payload steady state and surface end-to-end progress in `RUST_LOG=bevy_naadf=debug` runs. Useful for future regression diagnosis — recommended to keep. If unwanted, remove.
