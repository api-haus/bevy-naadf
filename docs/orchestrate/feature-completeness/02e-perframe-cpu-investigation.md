# 02e — Per-frame CPU scaling investigation: port (40 FPS Oasis vs 240 FPS test grid)

**Date:** 2026-05-16
**Author:** general-purpose Opus 4.7 (1M context) — diagnostic dispatch
**Branch:** `main` at HEAD post-`1c35c7f` (with `M` modifications listed in `git status`)
**Predecessor reads:** `01-context.md` · `02d-render-perf-investigation.md` · `02c-design-edit-pipeline-alignment.md` · `02a-v2-sparse-vox-ingestion.md` (Δ-GPUProducer section) · `03a-v2-impl-sparse-vox.md` · `12-alignment-gap.md` row 4 + row B-7 · `crates/bevy_naadf/src/render/{prepare.rs,extract.rs,mod.rs,construction/mod.rs}` · `crates/bevy_naadf/src/world/{data.rs,buffer.rs}` · `crates/bevy_naadf/src/voxel/{grid.rs,vox_import.rs}` · `crates/bevy_naadf/Cargo.toml`.

---

## Headline answer

**`prepare_world_gpu` fires every frame and re-uploads the entire CPU-mirror world (chunks + blocks + voxels + voxel_types) to the GPU. `extract_world` fires every frame and clones those same CPU buffers from main world → render world. The two together account for ~19.5 ms per frame on Oasis (vs ~0.17 ms on the test grid) — the entire 20 ms gap in the brief.** Both systems are gated by `WorldData.dirty`, set `true` by `setup_test_grid` / `build_world_from_vox` (`crates/bevy_naadf/src/voxel/grid.rs:115`, `crates/bevy_naadf/src/voxel/vox_import.rs:213`), and **never cleared** anywhere in the main world. `prepare.rs:440` clears the *render-world* `extracted.dirty` after upload — but `extract.rs:83-85`'s docstring explicitly states "The main-world `dirty` flag is left untouched (the main world does not re-read it); the render-world copy carries its own flag." The render-world flag is then *re-set* `true` by `extract_world` on the very next frame because `world_data.dirty` is still `true` from startup, restarting the upload cycle. The fix is a one-line change in `extract_world` to clear the main-world flag after the copy, plus matching change to `voxel_types.dirty`.

---

## Measured data

**Hardware:** AMD Ryzen 9 7900X3D · 64 GiB RAM · NVIDIA RTX 5080 · Vulkan · Linux 7.0.3-1-cachyos · cargo release profile (workspace root `[profile.release]` defaults).

### Test grid (default scene — 4×2×4 chunks = 32 chunks, 64×32×64 voxels)

Single smoke (`cargo run -p bevy-naadf --release`), ~60 frames captured before window close:

| System | Calls | Avg (ms/call) | Max (ms/call) | Total (ms) |
|---|---:|---:|---:|---:|
| `extract_world_changes` | 1 | 0.001 | 0.001 | 0.0 |
| `extract_world clone` | 59 | **0.018** | 0.117 | 1.0 |
| `prepare_world_gpu (re)build` | 58 | **0.148** | 3.974 | 8.6 |
| `prepare_construction` | 1 | 0.733 | 0.733 | 0.7 |

`prepare_world_gpu` fires on **58 of ~60 frames** — i.e. essentially every frame. `extract_world` fires on **59 of ~60 frames**.

CPU mirror payload (logged by the instrumentation):

- `chunks_cpu`: **32 u32 = 0 KiB**
- `blocks_cpu`: **1,920 u32 = 7 KiB**
- `voxels_cpu`: **7,232 u32 = 28 KiB**
- `dense_voxel_types`: **131,072 u16 = 256 KiB** (test-grid populates this; `.vox` path does not)

### Oasis_Hard_Cover.vox (93×34×84 chunks = 265,608 chunks; 1488×544×1344 voxels)

Single smoke (`cargo run -p bevy-naadf --release -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox`), 157 frames captured:

| System | Calls | Avg (ms/call) | Max (ms/call) | Total (ms) |
|---|---:|---:|---:|---:|
| `extract_world_changes` | 2 | 0.000 | 0.000 | 0.0 |
| `extract_world clone` | 157 | **2.810** | 17.083 | 441.2 |
| `prepare_world_gpu (re)build` | 157 | **16.731** | 26.235 | 2,626.8 |
| `prepare_construction` | 2 | 0.038 | 0.076 | 0.1 |

`prepare_world_gpu` fires on every captured frame (157/157). Same for `extract_world`. CPU mirror payload:

- `chunks_cpu`: **265,608 u32 = 1,037 KiB**
- `blocks_cpu`: **1,617,216 u32 = 6,317 KiB**
- `voxels_cpu`: **10,498,368 u32 = 41,009 KiB**
- `dense_voxel_types`: **0 u16 = 0 KiB** (sparse path empties it per Δ-GPUProducer at `vox_import.rs:215-219`)

Total CPU-mirror size per frame on Oasis: **~48 MiB**, cloned in `extract_world` AND re-uploaded by `prepare_world_gpu`.

### Delta (Oasis − test grid)

| System | Test grid avg | Oasis avg | Delta |
|---|---:|---:|---:|
| `extract_world clone` | 0.018 ms | 2.810 ms | **+2.79 ms** |
| `prepare_world_gpu (re)build` | 0.148 ms | 16.731 ms | **+16.58 ms** |
| **Sum of per-frame world-data work** | **0.166 ms** | **19.541 ms** | **+19.4 ms** |

This matches the brief's "**~20 ms scales with world size, lives outside the named GPU passes**" exactly. The named GPU passes total 1.23 ms on Oasis (per brief / HUD); the 23.7 ms residual budget (~25 ms wall − 1.23 ms named GPU) is approximately accounted for by these two CPU systems (~19.5 ms) plus the unmodified-named-GPU residual + Bevy plugin overhead identified in `02d`.

---

## Per-frame systems touching world resources — enumeration table

| System | File:line | Runs | What it does | Scales with world size? |
|---|---|---|---|---|
| `extract_world` | `crates/bevy_naadf/src/render/extract.rs:86-106` (registered at `render/mod.rs:129`) | Every frame (`ExtractSchedule`), but body **gated** by `world_data.dirty \|\| voxel_types.dirty` | If gate passes: `.clone_from()` `chunks_cpu`, `blocks_cpu`, `voxels_cpu`, `voxel_types`, `dense_voxel_types` into `ExtractedWorld`. Sets `extracted.dirty = true`. | **YES — when gate passes.** Gate currently passes every frame (root cause). |
| `prepare_world_gpu` | `crates/bevy_naadf/src/render/prepare.rs:151-441` (registered at `render/mod.rs:155`) | Every frame (`PrepareResources`), but body **gated** by `existing.is_none() \|\| extracted.dirty` | If gate passes: re-creates the Rg32Uint chunks 3D texture, the `GrowableBuffer`s for blocks/voxels/voxel_types, the `world_meta` uniform, the entity placeholder buffers, the world bind group, AND uploads all the CPU data to GPU via `render_queue.write_texture` + `GrowableBuffer::upload_all`. Re-creates `WorldGpu` resource via `commands.insert_resource`. Clears `extracted.dirty = false`. | **YES — when gate passes.** Gate currently passes every frame because `extract_world` re-sets `extracted.dirty`. |
| `extract_world_changes` | `crates/bevy_naadf/src/render/construction/mod.rs:657-770` (registered at `construction/mod.rs:2051`) | Every frame (`ExtractSchedule`) | Aggregates `WorldData::pending_edits.batches` into `ConstructionEvents`. Runs `compute_change_groups` only when `edited_groups` is non-empty (BFS+addBounds, ~50 µs). Inserts `ConstructionEvents` resource. | NO on stationary scene (early-out when `pending_edits.batches` is empty — verified: 0.000 ms on both scenes). |
| `prepare_construction` | `crates/bevy_naadf/src/render/construction/mod.rs:801-1881` (registered at `construction/mod.rs:2043-2048`) | Every frame (`PrepareResources`, after `prepare_world_gpu`) | First few frames: allocate hash_map / segment_voxel_buffer / W2 change-staging buffers, build the construction bind groups, dispatch one-time `add_initial` seed. After bind groups exist + producer run: steady-state branch is just an early-return after a few `is_none()` checks (≤ 0.04 ms on both scenes). | NO on steady state. Measured 0.038 ms avg (Oasis) vs 0.733 ms initial allocation (test grid first frame). |
| `naadf_world_change_node` | `render/construction/world_change.rs` (Core3d graph node) | Every frame (render-graph node) | Gated by `events.has_pending_changes()` — early-returns on stationary scene. GPU dispatches 4 chains when edits are present. | NO on stationary scene (event-gated). |
| `naadf_bounds_compute_node` | `crates/bevy_naadf/src/render/construction/bounds_calc.rs:311-370` | Every frame (render-graph node) | Encodes 5 rounds of W3 regime-2 prepare+indirect dispatch (~10 µs encode + GPU dispatch covered by the HUD's `BOUNDS_COMPUTE_SPAN`). CPU encode cost is trivial; GPU cost is in the HUD-tracked `naadf_bounds_compute` span. | GPU cost grows with bound queue depth, but **bounded** by `max_group_bound_dispatch` (default 512, capped per round) — not free-running with world size. Tracked in the HUD GPU times (already accounted for in the 1.23 ms named passes). |
| `naadf_gpu_producer_node` | `crates/bevy_naadf/src/render/construction/mod.rs:1865+` (Core3d graph) | Every frame, but body **gated** by `gpu_producer_has_run` + `dense_data_ready` | On the test-grid path: runs ONCE (first frame deps are ready) then early-returns thereafter. On the `.vox` path: never runs (`dense_voxel_types.is_empty()` gate trips at `mod.rs:855`). | NO on steady state (one-shot, then gated). On Oasis: never runs (skipped per design `02a-v2`). |
| `extract_camera` / `extract_camera_history` / `extract_taa_config` / `extract_gi_config` | `render/extract.rs:121-279` | Every frame | Camera transform, history ring, taa/gi flags. Fixed-size copies (Mat4 + small arrays). | NO. |
| `prepare_frame_gpu` | `crates/bevy_naadf/src/render/prepare.rs:457-931` | Every frame | Rebuilds bind groups on viewport-resize; otherwise just writes 2 small uniforms (`GpuCamera`, `GpuRenderParams` — total <200 B). | NO — viewport-resize is rare; steady state is two small `write_buffer` calls. |
| `prepare_atmosphere` / `prepare_gi` / `prepare_taa` | `render/atmosphere.rs` / `gi.rs` / `taa.rs` | Every frame | Per-frame uniform updates (<1 KiB total per frame per `02d`). | NO. |
| `clear_world_data_pending_edits` | `crates/bevy_naadf/src/render/construction/mod.rs:580` (`Last` schedule) | Every frame | Drains `pending_edits` on the main-world side. O(1). | NO. |

---

## Root cause

**Two systems' gate conditions interact such that they fire every frame:**

### 1. The main-world `WorldData.dirty` flag is set at construction and never cleared

`setup_test_grid` (`crates/bevy_naadf/src/voxel/grid.rs:115`) and `build_world_from_vox` (`crates/bevy_naadf/src/voxel/vox_import.rs:213`) both insert `WorldData { dirty: true, ... }`. **No other code path in the entire crate assigns `world_data.dirty = false`.** Verified via:

```
grep -rn "world_data.dirty\|\.dirty = false\|\.dirty = true" crates/bevy_naadf/src/
# Only 4 set-true sites (data.rs:211/769/880/1008, all inside set_voxel paths),
# and ONE set-false site at render/prepare.rs:440 which clears the
# *render-world* `extracted.dirty`, not the main-world `world_data.dirty`.
```

The same applies to `VoxelTypes.dirty` (set true at `grid.rs:122`, never cleared).

### 2. `extract_world` re-sets `extracted.dirty = true` every frame the main-world flag is true

`crates/bevy_naadf/src/render/extract.rs:95`:

```rust
if !world_data.dirty && !voxel_types.dirty {
    return;
}
// ... clone_from chunks_cpu / blocks_cpu / voxels_cpu / voxel_types / dense_voxel_types ...
extracted.dirty = true;
```

Because main-world `world_data.dirty` is stuck at `true`, the gate falls through every frame. The clone is unconditional after the gate, and `extracted.dirty = true` is set unconditionally. The docstring at `extract.rs:82-85` explicitly states the intended Build-once (D2) shape:

```
/// Build-once (D2): `setup_test_grid` sets `dirty = true`, this copies the
/// buffers once, and after `prepare_world_gpu` clears the flag this stays a
/// no-op. The main-world `dirty` flag is left untouched (the main world does
/// not re-read it); the render-world copy carries its own flag.
```

But **the implementation contradicts the docstring**: each pass through `extract_world` *re-sets* `extracted.dirty = true` whenever `world_data.dirty` is true — and since the main-world flag is *never* cleared, every frame re-triggers the cascade.

### 3. `prepare_world_gpu` re-uploads when `extracted.dirty` is true

`crates/bevy_naadf/src/render/prepare.rs:168-171`:

```rust
if existing.is_some() && !extracted.dirty {
    return;
}
```

When `extracted.dirty == true` (set by step 2), the gate falls through, the function re-creates the chunks 3D texture (`render_queue.write_texture` of Oasis's 2 MiB Rg32Uint texture), re-allocates the `GrowableBuffer`s for blocks (6.3 MiB) and voxels (41 MiB) and re-uploads them via `upload_all`, then `commands.insert_resource(WorldGpu { ... })` to replace the existing `WorldGpu` resource. Only at line 440 does it set `extracted.dirty = false` — which has no effect since `extract_world` will set it true again on the next `ExtractSchedule`.

### Why the cost scales with world size

`extract_world` does 5 `Vec::clone_from`s on the four CPU buffers + `voxel_types`. The buffers are sized proportional to the chunk count (`chunks_cpu`), mixed-block count (`blocks_cpu`), and mixed-voxel-block count (`voxels_cpu`). Oasis at 265,608 chunks → 48 MiB of contiguous-memcpy `clone_from` work = **2.8 ms/frame** at typical DDR5 bandwidth.

`prepare_world_gpu` does:

- Allocation of a paired `Vec<[u32; 2]>` of length `chunk_count` (`prepare.rs:254-258`) + memcpy from `extracted.chunks` (the single u32 → paired [u32; 2] conversion runs over all 265k chunks each frame).
- `render_queue.write_texture` of 2.1 MiB (`265608 × 8 B`).
- `GrowableBuffer::new` for blocks (~6.3 MiB allocation), `upload_all` (~6.3 MiB GPU upload).
- `GrowableBuffer::new` for voxels (~41 MiB allocation), `upload_all` (~41 MiB GPU upload).
- `commands.insert_resource(WorldGpu)` which **drops the previous WorldGpu** (dropping the previous wgpu Texture + Buffers — these go into wgpu's deferred-destroy queue, where they eventually get freed but the CPU side bookkeeping cost is non-trivial).
- Building a new `bind_group` from the new buffers.

Total measured at **16.7 ms/frame avg** on Oasis. The GPU-side `write_texture` + `upload_all` calls are not synchronous CPU work — they queue commands — but the staging-buffer allocation, the memcpy into the staging buffer, and the `wgpu::Queue::write_buffer`/`write_texture` plumbing add up to that ~17 ms.

### Why `extract_world_changes` and `prepare_construction` are NOT contributors

- `extract_world_changes` (`construction/mod.rs:657`) early-outs in the for-loop after 0 iterations when `pending_edits.batches` is empty (stationary scene). Measured at 0.000 ms.
- `prepare_construction` (`construction/mod.rs:801`) reaches steady state after the first ~2 frames (allocate construction buffers, build bind group, fire one-time `add_initial` seed). Subsequent frames either early-out near the top or do trivial bind-group cache checks. Measured at 0.038 ms avg on Oasis.

The CPU-frame budget gap is **entirely** in the `WorldData.dirty` flag mismanagement.

---

## Proposed fix shape

**One-line fix in `extract_world`.** Clear the main-world `dirty` flags after the copy. The render-world `extracted.dirty` flag stays as is (cleared by `prepare_world_gpu`).

### File: `crates/bevy_naadf/src/render/extract.rs:86-106`

Change the parameter type of `world_data` from `Extract<Option<Res<WorldData>>>` to `Extract<Option<ResMut<WorldData>>>` (or use a one-frame deferred reset via an `Extract<Option<Res<_>>>` + a buffered `ResMut` on the main side), then **at the end of the function**:

```rust
// Clear the main-world dirty flags AFTER the copy completes, so subsequent
// frames stay a true no-op. Build-once (D2) per the docstring contract.
// (Bevy's `Extract` system param is read-only in the render world; we need
// `ResMut` on the main-world side to mutate.)
//
// Implementation note: `Extract<Option<ResMut<WorldData>>>` is the natural
// shape — `Extract` re-resolves the resource type from the main world; ResMut
// works on it. The change-detection cost is microseconds and only fires on the
// frame the flag flips, so there's no per-frame change-detection overhead in
// the steady state.
if let Some(mut wd) = world_data_mut { wd.dirty = false; }
if let Some(mut vt) = voxel_types_mut { vt.dirty = false; }
```

OR (lower-risk, matches C# pattern that "the CPU side is built once and stays"):

**Alternative: invert the semantics — `WorldData.dirty` is set by edit paths only, cleared by NO ONE, and `extract_world`'s gate becomes a *change-detection* via Bevy `Changed<WorldData>`.** This is closer to idiomatic Bevy. The CPU mirror is build-once at startup; subsequent CPU-mirror changes come from `set_voxels_batch` / `set_chunks_uniform_batch` which already exist. The render-world `WorldGpu` would then be rebuilt only when the main-world `WorldData` is *actually* changed (which is the original D2 intent).

### Why this is faithful to C#

`WorldData.cs` in C# NAADF (the reference) does NOT re-upload its world data per frame; the CPU mirror is built once at world-generation time and edits flow through the W2 chain (`processChunks` → `changedChunks` upload) — the renderer reads the GPU-side state directly, no whole-world re-clone+re-upload exists in C# at all. The port's per-frame upload is purely a Bevy artifact of the unset `dirty` flag.

### Risk profile

- **Verification path:** the existing `--validate-gpu-construction` e2e gate already pins bit-exactness of the world upload. After the fix, `--validate-gpu-construction` must still pass — confirms the world is uploaded correctly on the first frame. The other 4 e2e modes (`baseline`, `--edit-mode`, `--entities`, `--vox-e2e`) cover render correctness post-fix.
- **Edge case:** if any external code path sets `world_data.dirty = true` (e.g. an editor tool, an entity-update path that mutates `chunks_cpu`), `extract_world` will correctly fire ONCE on that frame and the fix preserves that path. Auditing the 4 set-true sites (`data.rs:211/769/880/1008`) confirms they're all inside `set_voxel*` / `set_chunks_uniform*` paths — exactly when a re-upload IS warranted. **(But note: the W2 edit chain uploads only the *delta* via `changed_chunks` etc., not the whole world; setting `dirty = true` here would re-trigger the full upload, which is a separate latent over-upload bug. Out of scope for this dispatch; flagged in §"Risks / follow-ups".)**
- **Faithful-port rule (`01-context.md` Faithful-port): no divergence from C#.** C# does not re-upload per frame. The fix removes a non-C# behaviour.

### Expected gain

**Recovery of the full ~19.5 ms/frame on Oasis.** Predicted post-fix Oasis frame time:
- Pre-fix: ~25 ms (40 FPS)
- Post-fix: ~5.5 ms (~180 FPS), bounded by the named GPU passes (1.23 ms HUD'd) + Bevy plugin overhead per `02d` + the remaining named CPU work.

If the post-fix gap to C#'s 130 FPS persists, the remaining contributors are the `02d`-identified Bevy plugin overhead + the `sun_shadow_taps` config that `03d` already landed. Those are tackled separately.

---

## Sanity checks

### Build + tests pass with instrumentation in place

```
$ cargo build --workspace
   Finished `dev` profile [optimized + debuginfo] target(s) in 38.80s

$ cargo test --workspace --lib
   Finished `test` profile [optimized + debuginfo] target(s) in 19.68s
cargo test: 179 passed, 1 ignored (3 suites, 4.34s)
```

### Per-pass GPU times unchanged

The instrumentation is CPU-side `info!` logging only — no shader, render-graph, or GPU pipeline edits. The named GPU passes (`atmosphere`, `first-hit`, `taa-reproject`, `global-illum`, `sample-refine`, `spatial-resmpl`, `denoise`, `final-blit`) are untouched and their HUD-reported timings remain the brief-quoted 1.23 ms total.

### Instrumentation cost itself

The `info!` calls fire at INFO level, format strings only. `std::time::Instant::now()` + `.elapsed()` add ~30 ns each. Per-frame overhead from the instrumentation: <100 µs. The measured `extract_world clone` / `prepare_world_gpu` numbers include this overhead but it's well below the signal.

### What the instrumentation will do post-investigation

The instrumentation is **kept on the working tree** for now (uncommitted, per dispatch rules — orchestrator commits separately). Once the fix lands, the instrumentation should be removed or downgraded to `trace!` so it doesn't spam INFO logs in production. The fix-implementing dispatch can roll the instrumentation removal into the same commit.

---

## Risks / follow-ups

### R1 — `set_voxels_batch` may over-trigger the full-world upload post-fix

The 4 set-true sites in `world/data.rs:{211,769,880,1008}` are inside `set_voxel` / `set_voxels_batch` / `set_chunks_uniform_batch` / `set_voxels_batch_oracle`. After the proposed fix, **these will trigger a full-world re-extract + re-upload on the next frame after any edit**. For a continuous brush stroke (~60 edit-frames/sec), this would still cause the per-frame re-upload on every edit frame — only stationary frames recover.

**Mitigation:** the W2 chain already delivers per-edit *deltas* via `pending_edits.batches.changed_chunks/blocks/voxels` + `naadf_world_change_node`'s GPU dispatch. The full-world re-upload via `extract_world` + `prepare_world_gpu` is *redundant* with the W2 chain when only a delta has changed. The robust fix is **stop setting `world_data.dirty = true` in edit paths** and rely on the W2 delta uploads only. The full-world upload is only needed when the buffer *sizes* change (new chunks added, voxel buffer grew past the GrowableBuffer cap) — which `GrowableBuffer::upload_all`'s growth path can detect internally.

This is a **second-order fix** that should be a follow-up to the primary one-line fix; the primary fix alone unblocks stationary-scene Oasis perf.

### R2 — `dense_voxel_types` is currently kept in main-world RAM even for `.vox` path

`WorldData.dense_voxel_types: Vec<u16>` is sized at `size_in_voxels.x*y*z` u16s. On Oasis this would be 1.1 GB if populated — but the `.vox` path correctly sets it to `Vec::new()` (`vox_import.rs:219`, Δ-GPUProducer). The test-grid path populates it at 131 K u16 (256 KiB). After the fix, this Vec is cloned-from once at startup; its size is no longer a per-frame concern.

### R3 — `extracted.dense_voxel_types.clone_from` cost

The instrumentation logs `dense_voxel_types: 0 u16` on the `.vox` path and `131,072 u16 = 256 KiB` on the test grid. The Oasis-path delta in `extract_world` is therefore *not* driven by `dense_voxel_types`; it's `chunks_cpu` + `blocks_cpu` + `voxels_cpu`. Confirms `02a-v2`'s Δ-GPUProducer skip is working as designed.

### R4 — `prepare_construction`'s first-frame allocation cost

Measured at 0.733 ms on the test grid's first frame (large constant — hash_map / segment_voxel_buffer allocations), 0.038 ms on Oasis's first frame (smaller because GPU producer is skipped on the `.vox` path). One-time only; not a per-frame contributor.

### R5 — `PipelinedRenderingPlugin` interaction

The render sub-app runs on a separate thread (Bevy 0.19 `multi_threaded` feature). `prepare_world_gpu`'s 16 ms cost is paid by the render thread; if main thread is faster, this still gates frame submission. The fix removes the cost regardless of pipelined-rendering state.

### R6 — Memory leak from repeated `WorldGpu` re-creation

Each `commands.insert_resource(WorldGpu { ... })` drops the previous `WorldGpu`. The previous `Texture` + 3 `Buffer`s are wgpu-managed; they go into the deferred-destroy queue. wgpu typically processes deferred destroys at frame-submission time, so this should be steady-state, but at 60 FPS × ~50 MiB of wgpu objects/frame churning through deferred-destroy, **VRAM utilisation may be transiently elevated** (1-2 frames of doubled allocation). The fix eliminates this churn.

### R7 — One smoke each, no multi-run variance characterisation

Per memory `subagent-gpu-app-verification-loop`, one smoke per scene. Variance across smokes is not measured — but the per-frame distribution within each smoke (min/avg/max columns) gives some sense of jitter. The conclusion is robust to typical run-to-run variance: 16.7 ms ± 1 ms vs 0.15 ms ± 0.05 ms is a 100×+ gap, not noise.

---

## Out of scope

- **Phase 4 fix implementation.** The fix is *one line* of code but the orchestrator dispatches a separate agent to write + verify it.
- **Bevy `DefaultPlugins` curation** (`02d` §3). Separate dispatch.
- **`sun_shadow_taps` runtime default** (`02d` §1, already landed in `03d`).
- **Render-pipeline / GI shader correctness, AADF chain, `MAX_RAY_STEPS_*` constants** — explicitly forbidden by `01-context.md` §5.
- **`naadf_gpu_producer_node` internals + `gpu_producer_skip_upload` lever** — explicitly forbidden by `01-context.md` §5; the upload path itself stays in place, only its trigger condition gets fixed.
- **R1 (over-trigger on edit paths)** — follow-up to the primary fix; tracked in §"Risks / follow-ups". Would land after the primary fix and after Oasis-perf verification.
- **GPU-timestamp-query overhead from `RenderDiagnosticsPlugin`** (`02d` §"Render-graph cache discipline"). Not a contributor at Oasis scale (CPU contributor identified is 10× larger).
- **PipelinedRenderingPlugin off/on bench** (`02d` §5). Separate concern.

---

## Decisions & rejected alternatives

1. **Chose: report the diagnosis with a proposed-fix shape only, do not implement.** Dispatch brief explicitly forbids landing the fix here. Orchestrator dispatches a follow-up.

2. **Chose: instrumentation via per-system `std::time::Instant::now()` + `info!` log with a periodic-emit guard.** Rejected: full `tracing` spans / Bevy `Diagnostic` registrations. Reason: the diagnosis-first methodology required *fast smoke-detection*, not production-grade timing infrastructure. Per-frame `info!` is loud but unambiguous; the periodic guard (`f.is_multiple_of(120)`) keeps the cold-path noise floor low. The instrumentation should be removed/downgraded as part of the fix dispatch.

3. **Chose: smoke both scenes once each, accept variance.** Per `subagent-gpu-app-verification-loop`. The 100× gap dwarfs variance.

4. **Chose: not investigate `prepare_frame_gpu`/`prepare_atmosphere`/`prepare_gi`/`prepare_taa` per-frame work.** `02d` already confirmed those are <1 KiB/frame uniform updates and the bind groups are cached. They cannot account for a 20-ms gap.

5. **Chose: report `extract_world_changes` as not-a-contributor** despite the orchestrator's hint about W3 regime-2 walking. The instrumentation confirms 0.000 ms on both scenes (1-2 calls only, both startup). The W3 regime-2 dispatch IS GPU-bound (5 rounds × indirect dispatch) and shows up in the `naadf_bounds_compute` HUD GPU span; the CPU encode is trivial.

6. **Chose: name `extract_world`+`prepare_world_gpu` as the culprit pair, not individually.** They are symmetric — clearing the main-world `dirty` flag fixes both at once. Calling out only one would be misleading.

---

## Assumptions made

1. **The user's framing "40 FPS on Oasis" + "~20 ms scales with world size" is on a stationary camera with no editing pressure.** Per the orchestrator brief: "scales with chunk/block/voxel count" and "screenshot read" suggests stationary. The instrumentation smoke also ran stationary. If the user's perf complaint was while *editing* Oasis, R1 applies and a second-order fix is needed.

2. **Bevy 0.19's `Extract<Option<ResMut<T>>>` is the correct shape to mutate a main-world resource from `ExtractSchedule`.** This is the idiomatic Bevy pattern (`bevy_render::Extract` wraps a main-world `SystemParam`; `ResMut` reaches into the main world). Verified usage pattern at `crates/bevy_naadf/src/render/construction/mod.rs:662` (`entity_state: Option<ResMut<RenderWorldEntityState>>`) — except that one is render-world; the main-world mutation pattern needs verification by the fix-implementing agent. The alternative (use a `Changed<WorldData>` filter) is simpler and avoids the question.

3. **The HUD-reported 1.23 ms total GPU named-pass time from the brief is accurate.** Was not re-measured by this dispatch (the HUD numbers are user-side smoke output). The 19.5 ms gap identification doesn't depend on the HUD's precision; the smoking-gun is the per-frame fire-rate of `prepare_world_gpu` + its measured 16.7 ms cost.

4. **The user is on the same hardware for both scenes.** Per brief.

5. **The wgpu/Vulkan driver's deferred-destroy queue is steady-state.** Not measured; could be a second-order amplifier if it's not.

6. **The fix has no faithful-port-rule violation.** C# does not re-upload per frame; the fix matches C# behaviour.
