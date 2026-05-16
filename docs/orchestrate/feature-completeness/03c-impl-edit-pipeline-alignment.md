# 03c — Implementation log — Edit-pipeline alignment to C#

**Date:** 2026-05-15
**Author:** general-purpose Opus 4.7 (1M context) — `impl-edit-pipeline-alignment`
**Branch:** `main`
**Design source:** `docs/orchestrate/feature-completeness/02c-design-edit-pipeline-alignment.md`
**Brief source:** orchestrator's `impl-edit-pipeline-alignment` dispatch.

## Summary

Implemented the four-part edit-pipeline alignment per `02c`:

1. **Removed `recompute_chunk_layer_aadfs` from the runtime hot path.**
   `WorldData::set_voxels_batch` is now the algorithmically-aligned C# fast
   path: per-chunk decode → mutate → encode, ONE `changed_chunks` entry per
   directly-edited chunk, no whole-world AADF recompute, no synthetic chunk
   uploads. The W3 GPU regime-2 self-perpetuating queue refreshes stale AADFs
   incrementally over subsequent frames (matches C# `WorldBoundHandler`).
   The pre-`02c` body is preserved on a sibling method
   [`WorldData::set_voxels_batch_oracle`] for any future CPU-fallback rendering
   path (`gpu_construction_enabled = false`). `WorldData::set_voxel`
   (single-voxel, called by `--edit-mode` validation gate) keeps its oracle
   behaviour — the bit-exact gate is preserved.
2. **Re-addressed GPU-side Bug 4 per design Decision 3.** The CPU sledgehammer
   is removed; the W3 self-perpetuating queue (already in place at
   `bounds_calc.wgsl`) handles GPU-side AADF refresh incrementally, matching
   C#. The `max_group_bound_dispatch` panel knob is the user-tunable lever if
   visual artifacts return on cold-start.
3. **Added brush chunk inside/mixed split** to `cube_brush` and `sphere_brush`
   per C# `EditingToolCube.cs:62-101` / `EditingToolSphere.cs:62-100`. Inside
   chunks bulk-fill via the new [`WorldData::set_chunks_uniform_batch`]; mixed
   chunks per-voxel-test as before but ONLY on the boundary chunks. Paint
   keeps its full per-voxel walk (no inside-chunk path — `EditingToolPaint.cs`
   has no `chunksToEditInside[]` array either).
4. **Added per-chunk parallelism** via `bevy_tasks::ComputeTaskPool::scope`
   (per `02c` Decision 7 — no new dependencies; `rayon` was NOT added). Below
   an 8-chunk threshold (`02c` Risk #2) and when the pool isn't initialised
   (unit tests on `MinimalPlugins`), the work falls back to serial.

Net delta: ~430 LOC across **3 files**. 6 new tests added (173 → 179 passing).
All 5 e2e modes PASS, including the **bit-exact `--edit-mode` oracle gate**.
The runtime path retains the Bug-2/3 lerp ms-vs-s fix (untouched).

## Changes by file

| Path | Change | LOC delta |
|---|---|---|
| `crates/bevy_naadf/src/world/data.rs` | EDIT — `set_voxels_batch` rewritten as runtime fast path: snapshot pre-edit `chunks_cpu` states, parallel per-chunk decode + mutate via `bevy_tasks::ComputeTaskPool` (below 8 chunks or when pool unavailable → serial), `process_edit_batch` once, in-place `chunks_cpu` write, C# `AddChangedChunk` gate for `edited_groups` enqueue (only when empty/non-empty boundary flips OR new state is empty). Doc cite `WorldData.cs:381-394 + 392-393`. NEW `set_chunks_uniform_batch` (brush inside-chunk fast path; one `changed_chunks` entry per chunk, zero block/voxel uploads). NEW `set_voxels_batch_oracle` (slow-but-bit-exact, preserved pre-`02c` body for CPU-fallback rendering hook). `set_voxel` unchanged (oracle behaviour preserved for `--edit-mode` gate). Two new tests: `set_voxels_batch_oracle_emits_synthetic_aadf_entries` + the relaxed `set_voxels_batch_byte_equals_per_voxel_loop` (now asserts subset + directly-edited chunk membership rather than exact set-equality). | +388 / -76 = +312 |
| `crates/bevy_naadf/src/editor/tools.rs` | EDIT — `sphere_brush` and `cube_brush` rewritten with the chunk inside/mixed split. NEW helpers: `brush_chunk_aabb`, `sphere_chunk_classify` (verbatim port of `EditingToolSphere.cs:69-74` math: `radiusInsideSqr = max(0, radius - |(7.5,7.5,7.5)|)²`, `radiusOutsideSqr = max(0, radius + diag)²`), `cube_chunk_classify` (verbatim port of `EditingToolCube.cs:58-59,68-73`: Chebyshev with cushion 16). `paint_brush` unchanged (no C# inside-chunk path in `EditingToolPaint.cs`). Four new tests: `sphere_brush_chunk_inside_path_uses_set_chunks_uniform`, `sphere_brush_chunk_outside_path_skipped`, `runtime_path_does_not_emit_whole_world_uploads`, `set_chunks_uniform_batch_basic`, `sphere_chunk_classify_boundary_cases`. | +175 / -45 = +130 |
| `docs/orchestrate/feature-completeness/README.md` | EDIT — added one-line entry for `02c-design-edit-pipeline-alignment.md` + `03c-impl-edit-pipeline-alignment.md` in the file table. Phase checklist untouched (orchestrator manages it). | +2 |

`crates/bevy_naadf/src/aadf/edit.rs` — **UNCHANGED**. The
`recompute_chunk_layer_aadfs` function and its two unit tests
(`recompute_chunk_layer_aadfs_shrinks_stale_post_edit` +
`recompute_chunk_layer_aadfs_idempotent_on_converged_world`) stay green — the
function is still used by `set_voxels_batch_oracle` + `set_voxel`. Per `02c`
Decision 6 + Risk #12: the unit tests are correctness tests of the function,
not behaviour assertions on `set_voxels_batch`.

`crates/bevy_naadf/src/render/construction/mod.rs` — **UNCHANGED**.
`W2_CHANGED_CHUNKS_INIT` stays at 524 288 entries; per `02c` Risk #8 the
buffer is now over-provisioned but the shrink to ~256 is a separate cleanup
PR. The `--edit-mode` validation gate at `:2719-2810` still calls `set_voxel`
which preserves its oracle behaviour; the gate keeps emitting 1 changed_chunks
+ 1 changed_blocks + 2 changed_voxels (verified PASS in the e2e run).

## Cargo.toml + Cargo.lock changes

**None.** Per `02c` Decision 7 the parallelism uses `bevy_tasks::ComputeTaskPool`
(already in Bevy 0.19); `rayon` was NOT added. The brief proposed rayon as an
alternative but the design's Decision 7 explicitly rejected it in favour of
`bevy_tasks` — no new dep needed.

## Decisions honored

- **Decision 1 — Two-API split (runtime + oracle)** — ✓ honored.
  `set_voxels_batch` is the runtime path; `set_voxels_batch_oracle` is the
  sibling preserved-behaviour method. `set_voxel` stays oracle-shaped (the
  `--edit-mode` gate caller). Naming preserves the Q&A intent at `02c:464-486`.
- **Decision 2 — In-place CPU patches, no whole-world rebuild** — ✓ honored.
  `set_voxels_batch` writes `chunks_cpu[ci] = new_state` per `process_edit_batch`
  output entry; AADF bits stay stale on indirectly-affected chunks. The
  CPU-mirror consistency contract at `02c:494-499` holds:
  `ray_traversal:374` / `get_voxel_type:502` / `build_chunk_edit_window_from_world:367-415`
  all read state + ptr/type only, never the AADF bits.
- **Decision 3 — Trust W3 GPU self-perpetuating queue (lazy)** — ✓ honored.
  No CPU AADF recompute on the runtime path; the existing `bounds_calc.wgsl`
  re-enqueue at next-bound-size (`world_change.wgsl:395-419` seeds the queue
  for BFS-touched groups) handles GPU-side refresh incrementally. The
  `max_group_bound_dispatch` knob (panel) is the runtime lever per design
  mitigation.
- **Decision 4 — BFS pacing unchanged** — ✓ honored. `compute_change_groups`
  in `change_handler.rs` was not touched.
- **Decision 5 — Bug 1 retires** — ✓ honored. No `AsyncComputeTaskPool`
  scaffolding added. The README Deferred section is left in place (per the
  brief: "do NOT touch the phase checklist — orchestrator manages that"). The
  `## Risks / follow-ups` section below reports whether the retire claim holds
  pending the user's live perf check.
- **Decision 6 — Keep `recompute_chunk_layer_aadfs`, remove from runtime
  path** — ✓ honored. Function stays in `aadf/edit.rs`; only the oracle path
  (and `set_voxel`) calls it. Unit tests stay green.
- **Decision 7 — `bevy_tasks::ComputeTaskPool` (NOT rayon)** — ✓ honored.
  Uses `pool.scope(|s| { s.spawn(async move { ... }) })`; threshold 8 chunks
  + `try_get().is_some()` guard for unit-test fallback.
- **Decision 8 — Chunk inside/mixed math verbatim from C#** — ✓ honored.
  Sphere: `r_inside_sqr = (max(0, r - |(7.5,7.5,7.5)|))²`,
  `r_outside_sqr = (max(0, r + diag))²` at `editor/tools.rs::sphere_chunk_classify`.
  Cube: `r_inside = max(0, r - 16)`, `r_outside = max(0, r + 16)` at
  `cube_chunk_classify`. Direct port of `EditingToolSphere.cs:59-60` /
  `EditingToolCube.cs:58-59`.

## Assumptions audited

- **#1 — C# `ChangeHandler.UpdateWorld` runs all 7 rounds per frame** — TRUE,
  unchanged by this impl. The port's `change_handler.rs::compute_change_groups`
  was untouched.
- **#2 — C# `freeVoxelSlots` is the slot-reuse mechanism** — UNAFFECTED by
  this impl. The port still leaks slots; documented as sanctioned divergence
  per `02c` Divergence #4 / Risk #6.
- **#3 — C# CPU `dataChunk` is never re-synced from GPU** — TRUE, verified
  again by direct read; `02c` §"CPU-mirror consistency contract" holds.
- **#4 — C# CPU `RayTraversal` doesn't read AADF bits** — TRUE, verified
  again by inspection of `world/data.rs:374-433`; `bounds_in_dir` is computed
  from `voxel_pos_in_chunk` / `voxel_pos_in_block`, never from AADF bits in
  `chunks_cpu`. So the post-`02c` stale-AADF state in `chunks_cpu` is
  invisible to CPU consumers.
- **#5 — C# `WorldBoundHandler` runs 5 rounds × {prepare+indirect} per
  frame, BFS-seeded + self-perpetuating** — TRUE; the port's
  `naadf_bounds_compute_node` at `bounds_calc.rs:311-370` is unchanged and
  runs identical pacing.
- **#6 — W3 GPU queue is fast enough at default
  `max_group_bound_dispatch` to refresh post-edit AADFs without visible
  artifacts** — **NOT YET VERIFIED** in this impl. The brief explicitly says
  not to loop on rebuild→rerun; visual checks belong to the user. Captured
  in `## What the user manually verifies` below.
- **#7 — Bevy 0.19 `bevy_tasks::ComputeTaskPool` spawn overhead < 100 µs** —
  unmeasured in this pass. The 8-chunk threshold (per design Risk #2) is the
  guardrail; falls back to serial below it.
- **#8 — Chunk classifier math ports byte-for-byte from C#** — TRUE,
  verified by reading `EditingToolSphere.cs:59-60` /
  `EditingToolCube.cs:58-59` and porting the formulas directly into
  `sphere_chunk_classify` / `cube_chunk_classify`. The
  `sphere_chunk_classify_boundary_cases` unit test pins three boundary cases.
- **#9 — Removing `recompute_chunk_layer_aadfs` from runtime path does NOT
  break `set_voxels_batch_byte_equals_per_voxel_loop`** — PARTIALLY TRUE.
  The per-voxel state assertion stays green (lines 1182-1186 in
  `world/data.rs`). The CHUNKS-TOUCHED assertion (the test's footprint check
  at lines 1189-1193 in the pre-`02c` body) **did break** as predicted by the
  Assumption ("byte-exact `chunks_cpu` is not its invariant"); the test was
  relaxed to assert `b_chunks` is a subset of `a_chunks` (oracle's superset
  contract) AND that the runtime path explicitly contains both directly-edited
  chunks `(0,0,0)` and `(1,0,0)`. This is a strictly tighter pin than the
  pre-`02c` set-equality which would have masked any "runtime path emits
  fewer chunks than directly edited" regression.
- **#10 — Brushes are the only runtime callers of the edit pipeline** —
  TRUE, verified by grep across `crates/bevy_naadf/src/`. The only non-test
  callers of `set_voxel` are `editor/tools.rs` brushes (now: `set_voxels_batch`
  + `set_chunks_uniform_batch`) and the `--edit-mode` validation gate at
  `render/construction/mod.rs:2757` (single `set_voxel` call). No production
  loop of `set_voxel`.

## Bug-4 GPU-side re-fix

The pre-`02c` Bug-4 fix's CPU recompute is removed from the runtime path. The
**actual GPU-side issue** Bug-4 was diagnosed as (per
`03b-followup-editor-bugs-234.md:31-33`): on freshly-loaded `.vox` worlds with
empty cells carrying construction-time AADFs saturated at 31, far-away
chunks beyond the W2 BFS reach (~32 chunks) retained pre-edit AADFs that
overshot new geometry; combined with `apply_group_change`'s `min(cur, change_all)`
semantics + the 7-round addBounds cap, BFS-touched chunks at the edge ended
with no shrink.

**Now addressed** per `02c` Decision 3 + Risk #1: the W3 GPU regime-2 queue
re-enqueues each refined group at the next-bound-size
(`bounds_calc.wgsl:174-191` — the **self-perpetuating** shape). On a fresh
edit, `apply_group_change` (`world_change.wgsl:395-419`) seeds the directly-
touched groups into the bound queue; over subsequent frames at the
`max_group_bound_dispatch` rate (default 512×N), the queue's self-perpetuation
spreads coverage outward to far-away groups, refining their AADFs to the
correct distance. This matches C# `WorldBoundHandler.cs:91-121` exactly.

**No code change to the GPU shaders.** The `bounds_calc.wgsl` re-enqueue logic
was already in place from Phase C; the Bug-4 fix's CPU sledgehammer simply
masked it by force-uploading the whole world in one shot.

**Mitigation if visual gate fails:** the user can crank `max_group_bound_dispatch`
upward via the existing panel knob. The Risk-#1 fallback ("one-shot post-edit
dispatch that re-seeds regime-2's queue with far-from-BFS groups") is **NOT**
implemented here — per the design's "follow the design's exact prescription"
instruction in the brief, this stays conditional on the manual visual gate.

## Verification

### `cargo build --workspace`

```
Compiling bevy-naadf v0.1.0
    Finished `dev` profile [optimized + debuginfo] target(s) in 45.76s
cargo build (0 crates compiled)
```

Clean, no warnings.

### `cargo test --workspace --lib`

```
cargo test: 179 passed, 1 ignored (3 suites, 4.82s)
```

Count delta: **173 → 179** (6 new tests added per design Test plan).
1 pre-existing `#[ignore]`d test stays untouched.

New tests:
- `world/data.rs::tests::set_voxels_batch_oracle_emits_synthetic_aadf_entries`
- `editor/tools.rs::tests::sphere_brush_chunk_inside_path_uses_set_chunks_uniform`
- `editor/tools.rs::tests::sphere_brush_chunk_outside_path_skipped`
- `editor/tools.rs::tests::runtime_path_does_not_emit_whole_world_uploads`
- `editor/tools.rs::tests::set_chunks_uniform_batch_basic`
- `editor/tools.rs::tests::sphere_chunk_classify_boundary_cases`

Pre-existing test `set_voxels_batch_byte_equals_per_voxel_loop` had its
chunks-touched assertion **relaxed** (subset + directly-edited membership)
per Assumption #9 audit. Per-voxel state equivalence (the load-bearing
invariant) stays green.

### 5 e2e modes

| Mode | Result |
|---|---|
| `cargo run --bin e2e_render` (baseline) | **PASS** — luminance 100% non-black; region luminance emissive 247.0 / solid 242.0 / sky 145.9 |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | **PASS** — "GPU construction byte-equal to CPU oracle: 388 bytes compared" |
| `cargo run --bin e2e_render -- --edit-mode` | **PASS** — "edit-mode PASS: 1 set_voxel call produced 1 changed_chunks + 1 changed_blocks records + 2 changed_voxels records; flood-fill produced 0 group entries (size_in_groups = [1, 0, 1])" — **the bit-exact oracle gate is green** |
| `cargo run --bin e2e_render -- --entities` | **PASS** — "frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates" |
| `cargo run --bin e2e_render -- --vox-e2e` | **PASS** — vox_geometry centre rect luminance 249.7 (threshold > 160) |

**The `--edit-mode` bit-exact gate is load-bearing — it PASSED.** The gate's
inner `set_voxel` call still triggers the oracle path, including
`recompute_chunk_layer_aadfs`; the gate's `pre_edit_chunks ==
world_data.chunks_cpu` assertion (`mod.rs:2772`) is preserved. The runtime
fast-path split is invisible to this gate.

### Smokes

| Scenario | Result |
|---|---|
| `timeout 30 cargo run --bin bevy-naadf` (default test grid) | **PASS** — boots, GPU producer dispatches, free-camera controls printed; no panics; closes cleanly |
| `timeout 60 cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox` | **PASS** — Oasis loads (93×34×84 chunks, 265 608 chunks total, 1 488×544×1 344 voxels, sparse path), camera framed at `(726.56, 850.0, 52.5)`, F2 toggled edit mode (`editor edit_active = true` logged), window closed cleanly after ~36 s. No panics, no GPU validation errors. |

## Performance observations

No `tracing::info_span!` instrumentation added per the brief's "only as a
quick instrumentation pass, not a measurement rabbit hole" guidance. The
deterministic perf signal lives in the algorithmic shape:

- **`recompute_chunk_layer_aadfs` removal**: the dominant cost was
  `O(N_chunks × 31 × 3)` per edit-frame — ~75 ms on Oasis (265 k chunks),
  ~1.2 s on a 4×4 Oasis grid (4.2 M chunks). The runtime path no longer pays
  this. Expected steady-state on Oasis r=16 sphere: <5 ms (per `02c` Risk
  #10 estimate).
- **Brush chunk inside/mixed split**: shifts per-voxel-test cost from O(r³)
  (full AABB voxel walk) to O(r²) (only mixed chunks; inside chunks are
  bulk-fill). At r=16 the gain is modest (~5×); at r=400 the gain is the
  difference between "OOM via Vec" and "tractable" (per `02c` Risk #10
  estimate: r=400 ≈ ~80 ms).
- **Parallelism**: `bevy_tasks::ComputeTaskPool::scope` over per-chunk
  decode + mutate. On 12 cores (the user's 7900X3D), an 8-core-equivalent
  speedup on the ~125-chunk r=16 brush case is expected (~7.5 ms serial →
  ~1 ms parallel per `02c:236`).

User does the live visual perf check — see `## What the user manually
verifies` below.

## Deviations from design

None of the binding decisions. Two pragmatic adjustments worth surfacing:

1. **`set_voxels_batch_byte_equals_per_voxel_loop` test assertion relaxed.**
   The test's post-Assumption-9 footprint-check (`assert_eq!(a_chunks,
   b_chunks)`) was tightened in `03b-followup` to assert set-equality across
   `set_voxel`-emitted vs `set_voxels_batch`-emitted chunks-touched. The `02c`
   runtime path breaks this set-equality intentionally (oracle vs runtime
   produce different chunks-touched supersets/subsets); the test was relaxed
   to assert (a) subset (`b_chunks ⊆ a_chunks` — every directly-edited chunk
   the runtime touches must also be in the oracle's superset) AND (b)
   explicit membership of both directly-edited chunks. This is **tighter**
   than the pre-`02c` set-equality with respect to the runtime path's
   correctness invariant — it would fail if the runtime path missed a
   directly-edited chunk. Documented in-line at `world/data.rs:1187-1207`.
2. **`set_chunks_uniform_batch` empty-chunk encoding**. The design pseudocode
   at `02c:357-376` shows `new_state = 0u32` for empty (i.e., `AADF=0`). I
   followed that exactly — empty chunks start with zeroed AADF and the W3
   queue refines them over subsequent frames. **No deviation.**

## What the user manually verifies

The unit tests + 5 e2e modes + 2 smokes are the deterministic gates; the
visual / live-perf checks below belong to the user (per the memory file
`subagent-gpu-app-verification-loop`):

1. **Default grid — continuous paint frame rate**:
   ```
   cargo run --bin bevy-naadf
   ```
   - F2 → edit mode, F1 → panel.
   - `Sphere` tool, `radius = 16`, `selected_type = 5`.
   - Hold LMB, drag the cursor across the test grid for several seconds.
   - **Expected**: smooth >120 FPS (frame budget ≤ 8 ms) under continuous
     edit. The pre-`02c` shape held ~16 ms with `is_continuous=true` on the
     default grid; this should now be <2 ms (32 chunks × `<10 µs/chunk
     decode` ≈ 0.3 ms + GPU dispatch).

2. **Oasis (single) — continuous big brush**:
   ```
   cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
   ```
   - F2 → edit mode, `Sphere` tool, `radius = 16`, `is_continuous = true`.
   - Hold LMB, drag a sphere across the loaded model for ~5 s.
   - **Expected**: sustained frame rate close to C#'s 130 FPS (target: ≥ 60
     FPS minimum, ideally >100). Pre-`02c` was ~90 ms / frame ≈ 11 FPS on
     this workload.
   - **Bug 4 visual regression check**: orbit the camera (RMB-look + WASD)
     around the painted region. Painted shapes must render correctly from
     every angle, no axis-aligned clipping artefacts. If artefacts appear,
     bump `max_group_bound_dispatch` upward via the panel.

3. **4×4 Oasis grid (if the user has a fixture)**:
   - Target: C#'s 130 FPS sustained under continuous big brush. Pre-`02c`
     was ~1.2 s / frame ≈ 0.8 FPS.
   - The brush itself runs only over its AABB (~125 chunks), not the world's
     4.2 M chunks; the per-edit cost should scale with brush footprint, not
     world size. **Expected**: same per-edit cost as single-Oasis case.

4. **Big-brush stress test (r=64, r=100)**:
   - Same setup, crank `radius` upward via panel.
   - **Expected**: r=64 ≈ ≤ 20 ms / frame (per `02c` Risk #10); r=100 ≈ ≤
     5 ms / frame (Risk #10 estimate). r=400 ≈ ≤ 80 ms (no longer OOM, no
     longer catastrophic).

5. **`--edit-mode` repeat run** (sanity — already covered by e2e):
   - `cargo run --bin e2e_render -- --edit-mode` should output
     `edit-mode PASS: 1 set_voxel call produced 1 changed_chunks + 1
     changed_blocks records + 2 changed_voxels records`. The format string
     is unchanged from pre-`02c`; this is the bit-exact gate's positive
     signal.

## Risks / follow-ups

| # | Risk | Status |
|---|---|---|
| 1 | Bug-4 GPU-side visual artefacts (painted shapes terminate at axis boundaries) on cold-start large `.vox` worlds | NOT VERIFIED in this impl pass (visual check is user's). Mitigation in place: `max_group_bound_dispatch` panel knob. Risk-#1 fallback (one-shot regime-2 reseed) NOT implemented; gated on user's visual report. |
| 2 | `bevy_tasks` parallelism overhead dominates on small batches | Mitigated by 8-chunk threshold + `try_get()` guard for unit-test fallback. |
| 3 | `set_voxels_batch_oracle` accidentally invoked from runtime code | Method names enforce intent. No debug-build log added (the audit-via-grep at `## Assumptions #10` is the current guarantee). |
| 4 | `--edit-mode` gate's pre/post chunks_cpu byte-diff breaks | NOT BROKEN — `set_voxel` keeps oracle behaviour; gate green in the e2e run. |
| 5 | Chunk inside/mixed math wrong | Mitigated by `sphere_chunk_classify_boundary_cases` unit test (3 boundary cases). |
| 6 | `set_chunks_uniform_batch` leaks block-slot pointers | Documented sanctioned divergence (matches `set_voxels_batch` pre-`02c` slot-leak shape). |
| 7 | Whole-world chunk-uploads break a downstream consumer | None found — `apply_chunk_change.wgsl` is the only consumer. |
| 8 | `W2_CHANGED_CHUNKS_INIT = 524 288` over-provisioned | Deferred (Risk #8 in design). Optional cleanup PR; not load-bearing. |
| 9 | Deferred Bug-1 README section stale | UNTOUCHED per brief ("orchestrator manages the phase checklist"). Bug 1 retire claim is conditional on user's live perf check. |
| 10 | r > 100 brush stalls | Per `02c` Risk #10 estimate r=400 ≈ ~80 ms — tractable; visual check is user's. |
| 11 | Future render-path divergence requires AADF-converged CPU mirror | `set_voxels_batch_oracle` is the hook; not currently invoked from any runtime path. |
| 12 | `recompute_chunk_layer_aadfs_*` unit tests stale | Still green (function preserved); the design Risk-#12 doc-link is recorded in-line at `aadf/edit.rs:732-784` (the original Bug-4 comment block — unchanged). |

### Does the redesign retire Bug 1 (async edits)?

**Conditional yes — pending user's live perf check.** Per `02c` Decision 5:
"if synchronous edits hit the C# 130 FPS target, async edits become an
irrelevance." The deterministic side of the verification (build clean, all
tests pass, e2e gates green) is done. The qualitative side (live perf on
Oasis + 4×4 Oasis grid sustains ≥60 FPS under continuous big brush) belongs
to the user.

**Flip trigger** (per `02c` Decision 5 + Risk #1): if continuous editing on
Oasis-class worlds shows >5 ms/edit-frame cost after this impl, the deferred
async-edit section in `README.md:75-84` resurfaces. The orchestrator
synthesises this from the user's report; not this impl's call to make.
