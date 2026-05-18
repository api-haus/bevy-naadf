# 03f — Impl log: Phase 2.6 (`WindowedSlotMap`)

Author: delegate-impl (Phase 2.6).
Status: landed, all gates green.

Implements the design at
`docs/orchestrate/streaming-world/02c-design-windowed-slot-map.md`. After
this lands, the `--streaming-window` e2e gate PASSES for the first time —
visible terrain at the post-shift camera position, not skybox.

## Files added / edited

| Path | LOC Δ | What |
|---|---:|---|
| `crates/bevy_naadf/src/streaming/windowed_slot_map.rs` | +740 (new) | The `WindowedSlotMap` primitive + 20 unit tests (T1–T20). |
| `crates/bevy_naadf/src/streaming/mod.rs` | +12 / −10 | Register the new module; drop `SlotState` / `mark_admissions_resident` / `finalise_admissions_as_resident` re-exports; register `upload_window_indirection` in `Render::Queue`. |
| `crates/bevy_naadf/src/streaming/residency.rs` | +145 / −335 | Collapse `slot_to_world` + `world_to_slot` + `slot_state` triple into `window: WindowedSlotMap`. Drop `SlotState` enum + `mark_admissions_resident` / `finalise_admissions_as_resident`. Add `dispatched_once: HashSet<SlotIndex>` to track "Generating vs Resident" implicitly per D4. Rewrite `residency_driver` Pass 1 (`set_origin` returning evicted pairs → `free()` each + strip from `dispatched_once`), Pass 2 (target minus `iter_bound()`), Pass 3 (`allocate` + `bind`). |
| `crates/bevy_naadf/src/streaming/noise_dispatch.rs` | +56 | Add `window_indirection: Vec<u32>` + `window_origin: IVec3` to `StreamingExtractRender`. Add `upload_window_indirection` system (writes the 2 KB indirection buffer at `Render::Queue`). |
| `crates/bevy_naadf/src/render/gpu_types.rs` | +11 / −2 | Replace `_pad0` field on `GpuWorldMeta` with `streaming_active: u32`. Layout stride unchanged (the slot was already padding). |
| `crates/bevy_naadf/src/render/pipelines.rs` | +8 | Extend renderer-side `world_layout` with binding 8 = `window_indirection` (storage, read-only). |
| `crates/bevy_naadf/src/render/prepare.rs` | +30 / −2 | Add `window_indirection_placeholder: Buffer` to `WorldGpu`; allocate 1-u32 placeholder; thread `streaming_active` into `world_meta` via the extract resource; include placeholder in the world bind group. |
| `crates/bevy_naadf/src/render/construction/chunk_calc.rs` | +8 | Extend `construction_world_layout` with binding 8 = `window_indirection`. |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | +7 | Extend `construction_bounds_world_layout` with binding 2 = `window_indirection`. |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +280 | Add `window_indirection_buffer: Option<Buffer>` to `ConstructionGpu`. Allocate in `prepare_construction` (2 KB on streaming-active, lazy). Update `construction_world` / `construction_bounds_world` bind-group builds to include binding 8 (production buffer or placeholder). Add separate streaming-mode renderer-side bind-group rebuild path. Update Pass-3 streaming dispatch to compute `local = world_seg - origin` from the extract's `window_origin` field. Update 5 test-fixture bind groups to include the placeholder. |
| `crates/bevy_naadf/src/render/construction/world_change.rs` | +10 | Add window_indirection placeholder to W2 test fixture's world bind group. |
| `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` | +8 | Add window_indirection placeholder to W3 test fixture. |
| `crates/bevy_naadf/src/e2e/streaming_window.rs` | +35 / −22 | Migrate `slot_state` histogram in the wall-clock-budget panic to derive Generating/Resident/Empty from the new implicit lifecycle. Migrate 3 unit tests from `res.origin = ...` field assign to `res.window.set_origin(...)`. |
| `crates/bevy_naadf/src/e2e/driver.rs` | +2 / −2 | Migrate `r.origin.x` → `r.origin().x` (method, not field). |
| `crates/bevy_naadf/src/assets/shaders/world_data.wgsl` | +85 | Add `streaming_active: u32` to `GpuWorldMeta`; add `@binding(8) window_indirection`; add `streaming_chunk_index` + `streaming_chunk_load` helpers. |
| `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl` | +5 / −10 | Import the new helpers; replace flat-coord `chunks[…]` read at line ~290 with `streaming_chunk_load(chunk_pos)`. |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | +74 | Add `@binding(2) window_indirection`; add `streaming_chunk_index_bc` + `streaming_chunk_load_bc` helpers (using `arrayLength(&window_indirection) <= 1u` as streaming-active gate, since this shader doesn't bind `world_meta`). Update 2 read sites + 1 write site (with EMPTY_SLOT guard). |
| `crates/bevy_naadf/src/assets/shaders/world_change.wgsl` | +70 | Same pattern as bounds_calc.wgsl — add binding 8 + helpers + update read/write sites in `apply_group_change` + `apply_chunk_change`. |
| `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl` | +57 | Same pattern — add binding 8 + helper, update the `chunks[chunk_idx] = …` write at line ~424 with EMPTY_SLOT guard. |

Net: +1610 / −400 ≈ +1210 LOC across 19 edits (15 file edits + 1 new file + 3 test-fixture extensions).

## `WindowedSlotMap` test results

- **All 20 tests landed** as specified in `02c` § H (T1–T20), no deviation
  from spec. T18's pseudo-random sequence uses a deterministic LCG instead
  of a `rand` crate dependency (LCG seeded at 0xC0FF_EE42; sequence
  reproducible). T13/T14/T15/T16 are `#[cfg(debug_assertions)]`-gated
  (release builds elide the assertion the test triggers).
- Overall test count: workspace lib tests pass at **232/232** (Phase 2.5
  baseline 215 + 20 new − 3 retired Phase-2.5-only assertions on
  `SlotState`).

## Verification gates run

| Gate | Cmd | Exit | Wall-clock | Notes |
|---|---|---|---|---|
| Build (release) | `cargo build --workspace --release` | 0 | 44 s | Clean. |
| Lib tests (release) | `cargo test --workspace --lib --release` | 0 | 4.3 s | 232/232 pass; 1 ignored, no regressions. |
| `baseline` | `cargo run --release --bin e2e_render -- baseline` | 0 | ~7 s | Default preset bit-equivalent (region luminance: emissive 247.7, solid 243.7, sky 202.9). |
| `--wgsl-noise-oracle` | … | 0 | <1 s | Phase 1 oracle: max_abs_diff 1.49e-6. |
| `--noise-static-world` | … | 0 | 7.3 s | Phase 2.4 variance 1804.90 (floor 800), stddev 14.42 (floor 10). |
| `--validate-gpu-construction` | … | 0 | ~4 s | 388 bytes compared, byte-equal. |
| **`--streaming-window`** | … | **0** | **67.6 s** | **PASS** (first time). Mean pixel Δ = **83.01** (floor 3.00); after-frame luminance variance = **2325.94** (floor 800.00); residency origin shift in X = 4 segments (floor 4). Wall-clock 67.6 s, well under the 120 s budget. |

All gates green, including the load-bearing `--streaming-window`. The
pixel delta of 83 (~28× the 3.0 floor) and variance of 2326 (~2.9× the
800 floor) both sit comfortably above the strict thresholds — the gate
now has a wide passing margin instead of the previous false-pass/regression
flip-flop.

## Sequencing pinch — how steps 7, 9, 10 were handled

Landed all three (renderer-side `NaadfPipelines::world_layout`,
construction-side `naadf_world_bind_group_layout` rebuild, prepare-side
`WorldGpu::window_indirection_placeholder` allocation + bind group) as
ONE edit batch BEFORE the first `cargo build`. The pipeline cache
descriptor-set equality is by entry-set, so all three layouts needed to
arrive simultaneously.

**Outcome**: no pipeline-cache validation errors. The first build after
the sequencing-pinch edits failed only on unrelated method-vs-field
issues (`r.origin.x` callers needing `r.origin()`) and a missed
`storage_buffer_read_only_sized` import — neither caused by descriptor
mismatch.

## Surprises during implementation

1. **Construction-side shaders (chunk_calc / world_change / bounds_calc)
   don't import `world_data.wgsl`.** The design's "shared helper"
   strategy assumed they did. Fix: inlined the helper functions in each
   construction shader (`streaming_chunk_index_cc` /
   `streaming_chunk_index_wc` / `streaming_chunk_index_bc`), and used
   `arrayLength(&window_indirection) <= 1u` as the streaming-active gate
   rather than reading `world_meta.streaming_active` (which only
   ray_tracing.wgsl binds). Non-streaming presets bind a 1-u32
   placeholder; the gate short-circuits to the flat-coord pass-through.

2. **The streaming dispatch's `chunk_offset` math source.** The design
   said the math values stay the same — `[lx*16, ly*16, lz*16]` — but
   the design's pseudo-code derived `(lx, ly, lz)` from
   `Residency::local_of(slot.0)` (the OLD geometric mapping). Under
   pool-driven allocation, `slot.0` no longer maps geometrically. Fix:
   added `window_origin: IVec3` to `StreamingExtractRender` and compute
   `local = world_seg - origin` directly in the dispatch loop. The
   indirection table then handles the slot translation transparently
   in the shader.

3. **`SlotIndex` needed `Hash`** to be a `HashSet` key for the new
   `dispatched_once` tracking. Added `Hash` to the existing
   `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` — no impact on
   downstream consumers (the type was already `Eq`).

4. **5 test fixtures needed window_indirection placeholders** — the
   construction-side test fixtures (W1, W2, W3 + 2 validate sites: 388
   `validate_gpu_construction` + scaled-mode + tile-mode + oasis-mode
   tests) all build their own bind groups manually and were missing
   binding 8 / binding 2 after the layout extension. Added 1-u32
   placeholder buffers at each site.

5. **Renderer-side world bind-group rebuild needs a SECOND path for
   streaming.** The existing rebuild path was gated on
   `construction_config.entities_enabled`. Since streaming and entities
   are mutually exclusive at install time, a separate streaming-mode
   rebuild block was added (uses the entity placeholders + the
   production indirection buffer). Re-uploads `world_meta` with
   `streaming_active = 1` in case prepare_world_gpu ran on a frame
   before the streaming extract had populated the flag.

6. **`Residency::origin` becoming a method broke 4 direct field
   accesses.** Internal sites in `e2e/streaming_window.rs` (3 unit
   tests + 1 production read) and `e2e/driver.rs` (2 reads) were
   updated. The 3 unit tests used `res.origin = ...` (field assign)
   which now must go through `res.window.set_origin(...)`. Migration was
   mechanical.

## Deviations from design

- **Helper-function strategy**: design § E specified ONE helper per
  function (`streaming_chunk_index` / `streaming_chunk_load`) declared
  in `world_data.wgsl` and shared. **Inlined per-shader** because the
  construction shaders don't import `world_data.wgsl`. Functionally
  equivalent (each is a verbatim copy of the helper); the impl agent
  inlined 4 copies (`world_data.wgsl` + bc / wc / cc in the three
  construction shaders).

- **`streaming_active` gating on construction side**: design used
  `world_meta.streaming_active`. The construction shaders don't bind
  `world_meta`. Used `arrayLength(&window_indirection) <= 1u` as a
  runtime substitute (placeholder is 1-element on non-streaming; full
  512-element on streaming). This is functionally equivalent — the
  shader-side gate behaves the same. Adds one ALU per chunk_calc invocation
  to compute `arrayLength`; this is negligible compared to chunk_calc's
  workload.

- **`dispatched_once` HashSet instead of repurposing slot_state**: the
  design (D4) said the lifecycle is implicit via `bound ∩
  admissions_this_frame`. The implementation reified this slightly:
  added a `dispatched_once: HashSet<SlotIndex>` so
  `process_pending_admissions` can filter out already-dispatched slots
  across multiple ticks (the Phase-2.5 drain semantics). Without this
  tracking, the same 4 closest slots would re-enter
  `admissions_this_frame` every tick. The HashSet is cleared on eviction
  (via the `set_origin` callback chain).

## What's left

- **Optional regression-safety tightening (OQ.4)**: post-impl measured
  pixel delta = 83.01 (~28× the 3.0 floor) and variance = 2325.94
  (~2.9× the 800 floor). These are well above the strict floors and
  match the design's expected magnitudes for a working streaming
  preset. **Did NOT raise the thresholds** in this dispatch per the
  brief's directive — that's a separate consideration (the user can
  tighten via OQ.4 follow-up after observing several runs).

- **Phase-3 hand-off (biome composition / multi-noise mixing)**: the
  `WindowedSlotMap` primitive cleanly separates pool / mapping /
  indirection concerns. The biome-per-segment work can extend
  `StreamingExtractRender` with per-segment biome IDs (an additional
  field alongside `window_origin` / `window_indirection`) and the
  noise-dispatch loop can switch noise parameters per admission via
  the existing `build_noise_terrain_params` helper. The indirection
  table is biome-agnostic — the GPU indirection translation works
  identically for multi-biome streaming.

- **Phase 2.5's `slot_admissions_eventually_drain_to_resident`
  regression catcher**: re-cast against the new implicit lifecycle.
  Now asserts "bound ∧ !dispatched_once" count strictly decreases per
  tick. Same load-bearing semantics, new bookkeeping source.
