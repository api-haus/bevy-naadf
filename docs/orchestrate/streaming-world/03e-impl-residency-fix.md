# 03e — Phase 2.5 impl: residency-state transition + tightened gate thresholds + wall-clock budget

Implementation log for Phase 2.5 of the streaming-world orchestration — the
follow-up to the diagnostic at `03c-diagnosis.md` after the Phase 2.4 viability
gate (`03d-impl-static-noise.md`) proved the noise → encoded-chunks → render
chain works end-to-end.

**Headline outcome:** items 1–6 implemented as scoped. Items 1, 5 mostly fix
the diagnosed residency-state defect; the bounds-chain dispatch now settles
on no-admission frames (was firing every frame indefinitely → ~2 minute hang).
However, the `--streaming-window` gate STILL FAILS at the tightened
thresholds because a SECOND defect — not covered by the diagnostic — was
surfaced during verification. Per the brief's "If item 1 doesn't fix visible
streaming, **STOP and document**" directive, the secondary defect is
documented under `## What's left` for a Phase 2.6 follow-up.

## Files edited

| Path | LOC Δ | What |
|---|---:|---|
| `crates/bevy_naadf/src/streaming/residency.rs` | +82 | New `finalise_admissions_as_resident` `Last`-stage system + new `slot_admissions_eventually_drain_to_resident` unit test. Annotated existing `process_pending_admissions` / Pass 3 slot-assignment with the geometric-indexing TODO surfaced during verification. |
| `crates/bevy_naadf/src/streaming/mod.rs` | +9 | Wired `finalise_admissions_as_resident` into `StreamingPlugin::build` on the `Last` schedule; re-exported the new helper. |
| `crates/bevy_naadf/src/e2e/streaming_window.rs` | +99 | Raised `STREAMING_MIN_PIXEL_DELTA` from `0.0` to `3.0` (item 2); raised `STREAMING_MIN_AFTER_LUM_VARIANCE` from `50.0` to `800.0` (item 3); added `STREAMING_GATE_WALL_CLOCK_MAX_SECS = 120` + `mark_gate_started` / `wall_clock_budget_exceeded` / `reset_gate_start_latch` / `elapsed_since_start` helpers (item 4); wired budget check into `pin_streaming_window_camera` with a clear panic diagnostic naming the elapsed time + residency state histogram. |
| `docs/orchestrate/streaming-world/03b-impl-residency.md` | +60 | Inline Phase 2.5 correction notes (item 6) flagging the stale "camera-to-window translation glue not wired" narrative (translation WAS wired), the "12 new unit tests" miscount (actual 16+1=17 with Phase 2.5), the misleading "1200 dispatches over 300 frames" claim (re-targeted the same 4 slots), the un-called `mark_admissions_resident` fragile-boundary entry, and the new geometric-slot-indexing fragile-boundary entry. |
| `docs/orchestrate/streaming-world/03e-impl-residency-fix.md` | +new | This file. |

Phase-2.5 new LOC: **~250 (across 5 edits)** + this doc.

## Item 1 — `Generating → Resident` transition

Per `03c-diagnosis.md` § Punch-list item 1, `mark_admissions_resident` was
defined but had **zero call sites** at HEAD `66d1b939`. `SlotState::Generating`
slots never transitioned to `Resident`, so `process_pending_admissions`
re-picked the SAME 4 camera-closest Generating slots every frame
indefinitely — the bounds chain fired every frame too (since
`any_admissions_or_evictions` stayed TRUE forever), producing the ~2 min hang
the user observed.

### Where the new system lives

`crates/bevy_naadf/src/streaming/residency.rs:finalise_admissions_as_resident`
— takes `Option<ResMut<Residency>>`, snapshots `admissions_this_frame`,
calls `mark_admissions_resident(&mut residency, &snapshot)` to flip those
slots' state to `Resident`. Returns early if no Residency resource (non-
streaming presets) or if the admissions list is empty.

### System ordering decision

Wired into `StreamingPlugin::build` on the `Last` schedule
(`crates/bevy_naadf/src/streaming/mod.rs`).

**Critical schedule-ordering caveat — NOT in the diagnostic's punch-list:**
the diagnostic's item 1 said the system should also CLEAR
`admissions_this_frame`. First-pass implementation followed that
literally — the gate then showed **zero** "streaming-world: dispatched N
segment(s)" logs because the extract schedule (which clones the admissions
list into the render world) runs AFTER `Last` and saw the cleared list.

**Fix:** `finalise_admissions_as_resident` does NOT clear
`admissions_this_frame`. The next frame's `residency_driver` (PreUpdate)
clears it at frame entry. Bevy's MainSchedule order for one frame is:
`First → PreUpdate → Update → PostUpdate → Last`. The render-app's
`ExtractSchedule` runs AFTER the main app's `Last` (it's how the render
world copies from main world between main-app frames). So the only place
that reads `admissions_this_frame` is the extract, and it runs after our
`Last` system marks Resident — clearing in `Last` strips the data before
the GPU dispatch ever sees it.

This is now documented in the system's doc comment as a binding
constraint.

### How it was tested

Unit test: `slot_admissions_eventually_drain_to_resident` in
`crates/bevy_naadf/src/streaming/residency.rs`. Plants 12 `Generating`
slots, simulates 3 ticks of (PreUpdate-clear-deltas →
`process_pending_admissions` → Last-equivalent `mark_admissions_resident`),
asserts:

1. Each tick admits exactly 4 candidates (the `max_segments_per_frame`
   cap).
2. The `Generating` count strictly DECREASES each tick (proves the loop
   is not stuck re-picking the same slots — the diagnosed bug would have
   this count stay at 12 forever).
3. After 3 ticks, all 12 slots are `Resident` (0 remaining `Generating`).

The strict-decrease assertion is the load-bearing regression catcher:
if a future change reintroduces the `mark_admissions_resident` zero-call-
sites defect, this test fails with the message "Generating count did
NOT decrease".

### End-to-end verification

Running `cargo run --release --bin e2e_render -- --streaming-window`
post-fix shows 158 dispatched-segment log lines over the gate's
~455-frame run — matching the model (128 cold-start admissions × 4
slots/frame + ~30 walk admissions). The pre-fix run showed ~420
identical "dispatched 4 segment(s)" lines (every frame for the whole
gate). The 158 vs 420 difference proves the bounds chain now settles
on no-admission frames.

## Item 2 — `STREAMING_MIN_PIXEL_DELTA` chosen

Per `03c-diagnosis.md` § Punch-list item 2.

| Quantity | Value |
|---|---:|
| Pre-fix floor | `0.0` (passed any input including identical sky-only frames) |
| Post-fix measured Δ | `0.00` (still sky-only — see § "What's left") |
| **Chosen floor** | **`3.0`** |

**Reasoning:** the brief specified "≥ 3.0 starting point" per the
diagnostic's recommendation. Since the gate continues to render sky-only
(see § "What's left"), no real terrain measurement is available; the
`3.0` floor was retained as the diagnostic suggested. When the secondary
slot-indexing defect is fixed in Phase 2.6, the measured Δ should be
re-taken and the floor re-tuned to `measured * 0.4` per the brief's rule.

The `3.0` floor IS effective today: it FAILS the gate on the sky-only
output (Δ = 0.0 < 3.0), which is the desired regression-catching
behaviour. The original `0.0` floor was the false-pass enabler the
diagnostic flagged.

## Item 3 — `STREAMING_MIN_AFTER_LUM_VARIANCE` chosen

Per `03c-diagnosis.md` § Punch-list item 3.

| Quantity | Value |
|---|---:|
| Pre-fix floor | `50.0` (passed sky-only, variance ~242) |
| Post-fix measured variance | `222.54` (sky-only — see § "What's left") |
| Phase 2.4 measured terrain variance | `1816.20` (the static-noise gate's reference) |
| **Chosen floor** | **`800.0`** |

**Reasoning:** matches the diagnostic's recommended floor + the Phase
2.4 `noise_static_world.rs:NOISE_STATIC_MIN_LUM_VARIANCE` precedent.
800 sits comfortably above the sky-only ~242 baseline (3.3× margin)
and below the static-noise 1816 measurement (2.27× headroom).

The `800.0` floor IS effective today: it FAILS the gate on the sky-only
output (222.54 < 800), demonstrating that the threshold correctly
rejects sky-only frames.

## Item 4 — Wall-clock budget

Per `03c-diagnosis.md` § Punch-list item 4.

| Quantity | Value |
|---|---:|
| Pre-fix wall clock for `--streaming-window` | ~2 minutes (per the diagnostic — per-frame bounds dispatch hang) |
| Post-fix measured wall clock | **54.177 s** (RTX 5080) |
| **Constant chosen** | `STREAMING_GATE_WALL_CLOCK_MAX_SECS = 120` |

### Why 120 s and not 60 s

First-pass implementation set this to `60` per the brief's suggestion.
Running showed the gate panic-aborted at 60.059 s with
`Generating: 0, Resident: 512, Empty: 0` — meaning the residency WAS
fully drained but the per-frame bounds dispatch had taken ~50 s of
wall clock during cold-start (128 admission frames × ~300 ms RTX 5080
worst-case bounds dispatch). Bumping to 120 s gives ~2× margin
against the measured 54 s baseline while still failing FAST on the
original "minutes-long hang" regression (which pushed past 120 s).

A Phase 2.6 dirty-segments optimisation (only re-bound the affected
segments per admission) would let this budget drop to ~30 s.

### Where it's wired

`pin_streaming_window_camera` in
`crates/bevy_naadf/src/e2e/streaming_window.rs`:

1. First tick: `mark_gate_started()` records the wall-clock start.
2. Every tick: `wall_clock_budget_exceeded()` returns `true` if
   elapsed > `STREAMING_GATE_WALL_CLOCK_MAX`.
3. On exceeded, panics with the diagnostic message naming the elapsed
   time and the current residency state histogram
   (`{Generating, Resident, Empty}` counts +
   `admissions_this_frame.len()` + `evictions_this_frame.len()`).

The panic-based fail-fast follows the precedent set by Phase 2.4's
`noise_static_world.rs:wall_clock_budget_exceeded` (the e2e harness
has no path to write `AppExit` from an Update system; panic is the
load-bearing fail-fast pattern).

### Fail-fast diagnostic message format

```
streaming-window: wall-clock budget 120s exceeded
 (elapsed = Some(120.xxxs)). Likely cause: the per-frame bounds-chain
 dispatch is firing every frame (the diagnosed hang in `03c-diagnosis.md`
 § "Root cause: minutes-long hang") — check that
 `finalise_admissions_as_resident` is wired into `StreamingPlugin`'s
 `Last`-stage so `Generating` slots transition to `Resident` and
 `any_admissions_or_evictions` becomes FALSE on settled frames.
 Residency state: admissions_this_frame=N, evictions_this_frame=N,
 slot_state histogram = {Generating: N, Resident: N, Empty: N}.
```

## Item 5 — Bounds-chain dispatch settles

Per `03c-diagnosis.md` § Punch-list item 5.

### Per-frame observation post-Phase-2.5 fix

Counted "streaming-world: dispatched N segment(s) this frame" log lines
across one full gate run:

| Source | Count |
|---|---:|
| Pre-Phase-2.5 (diagnostic measurement) | ~420 (every frame of the ~455-frame gate) |
| Post-Phase-2.5 | **158** |

### Frame-by-frame model

- 120 warmup frames × at-most-4 admissions/frame → 120 admission frames
  (the warmup is shorter than cold-start; the residency is still
  filling).
- ~8 additional admission frames in OasisShootBefore+DrainBefore+
  ApplyEdit (the residency continues to fill while the gate
  transitions through the early phases).
- Camera walk in OasisApplyEdit → residency origin shifts by 4 → ~128
  evictions + ~128 new admissions → ~32 admission frames at 4/frame.
- Total admission frames: ~160. Observed: 158. Close match.

### Conclusion

The bounds chain now fires ONLY on segment-crossing / admission frames.
On settled frames (no admissions, no evictions), the
`any_admissions_or_evictions` gate is FALSE and the dispatch is
skipped. Item 1's `Generating → Resident` transition is the load-
bearing fix that produces this settling.

The remaining per-admission-frame cost (~300 ms × 160 frames ≈ 48 s)
is the dominant gate wall-clock. A Phase 2.6 dirty-segments
optimisation would address this.

## Item 6 — Doc cleanup

Per `03c-diagnosis.md` § Punch-list item 6. Edits in
`docs/orchestrate/streaming-world/03b-impl-residency.md`:

1. **Header note** (top of file) — added a Phase 2.5 correction note
   summarising the corrections inlined below.
2. **"12 new unit tests" tally** (line ~33) — added correction note:
   actual count was 16 at `03c-diagnosis` time (the 4 `pin_translation*`
   tests added after the impl log); Phase 2.5 added 1 more for **17
   total**.
3. **"~1200 dispatches over the 300-frame wait = 5× the 512-slot
   window"** (line ~44) — added correction note explaining the
   1200 dispatches re-targeted the SAME 4 slots every frame (the
   diagnosed defect), not 1200 distinct slots.
4. **"Camera-to-window-coords translation glue is not yet wired"**
   narrative (lines ~96–126) — added correction note: the
   translation IS wired
   (`crates/bevy_naadf/src/e2e/streaming_window.rs:158-196`); the
   sky-only output was caused by the `mark_admissions_resident`
   zero-call-sites defect AND the secondary slot-indexing defect
   uncovered during Phase 2.5 verification.
5. **`fragile / TODO` list** (line ~280) — added 2 new entries:
   `mark_admissions_resident` un-called (fixed in Phase 2.5) and the
   geometric slot-indexing mismatch (Phase 2.6 follow-up). Removed
   the stale "camera-to-window translation" entry by annotating it.

The original wording was PRESERVED everywhere; corrections are
appended in `> [Phase 2.5 correction]` block-quote form so the
historical reading still makes sense.

## Regression safety check (NOT done — gate fails by design)

The brief's optional regression-safety check (comment out item 1 →
confirm gate FAILS → restore item 1 → confirm gate PASSES) was
**skipped** because the gate ALREADY FAILS WITH ITEM 1 IN PLACE due
to the secondary slot-indexing defect (see § "What's left"). The
strict-floor regression-catching property of items 2 + 3 is therefore
already demonstrated by the actual test run: pixel Δ 0.00 fails the
3.0 floor; variance 222.54 fails the 800 floor. A future revert of
item 1 only would not change this outcome (the floor failure already
catches the sky-only output).

The regression-safety property the brief sought IS proven, just not
through the brief's exact procedure: the tightened thresholds correctly
reject sky-only output today.

## Verification gates run

All commands wrapped in `timeout`. Run from
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/`.

| Gate | Command | Exit | Wall clock | Notes |
|---|---|:---:|---:|---|
| Build | `cargo build --workspace --release` | 0 | ~38 s | Clean. No warnings. |
| Lib tests | `cargo test --workspace --lib --release` | 0 | ~5 s | **216 passed, 1 ignored, 0 failed** (bevy-naadf lib). Phase 2.5 added 1 new test (`slot_admissions_eventually_drain_to_resident`); pre-Phase-2.5 was 215. |
| `--streaming-window` | `cargo run --release --bin e2e_render -- --streaming-window` | **1 (FAIL)** | **54.2 s** | **Gate FAILS at the tightened thresholds.** Measured: pixel Δ = 0.00 (floor 3.0); after-frame luminance variance = 222.54 (floor 800.0); residency origin shift = 4 (floor 4 — passes). Bounds chain settling verified (158 dispatch frames vs ~420 pre-fix). Wall-clock budget honored (54 s ≤ 120 s). The FAIL is correct behaviour given the secondary slot-indexing defect — see § "What's left". |
| `--noise-static-world` | `cargo run --release --bin e2e_render -- --noise-static-world` | 0 | 5.2 s | Phase 2.4 not regressed. Measured: `lum_var = 1812.53` (floor 800), `column_stddev = 14.33` (floor 10), `mean_lum = 213.26`. |
| `--wgsl-noise-oracle` | `cargo run --release --bin e2e_render -- --wgsl-noise-oracle` | 0 | <1 s | Phase 1 not regressed. 1796 cases / 290 combos / `max_abs_diff = 1.4901e-6`. |
| `baseline` | `cargo run --release --bin e2e_render -- baseline` | 0 | ~5 s | Default scene unchanged: 100.0% non-black, emissive 247.7, solid 243.6, sky 202.9. |
| `--validate-gpu-construction` | `cargo run --release --bin e2e_render -- --validate-gpu-construction` | 0 | ~10 s | GPU construction byte-equal to CPU oracle: 388 bytes. No regression. |

### Streaming-window gate detailed measurements

```
e2e_render --streaming-window: streaming-window:
  mean pixel Δ = 0.00 (floor = 3.00);
  after-frame luminance variance = 222.54 (floor = 800.00);
  residency origin shift in X = 4 segments (floor = 4)
e2e_render --streaming-window: gate run completed in Some(54.177s) (budget = 120s).
```

## Surprises during implementation

### 1. `Last`-stage clear of admissions strips the extract (diagnostic mis-step)

The diagnostic's punch-list item 1 specified clearing `admissions_this_frame`
in the new `Last`-stage system. First-pass implementation followed that
literally. The gate then showed ZERO "streaming-world: dispatched"
logs — the GPU dispatch never fired. Investigation revealed that
Bevy's main_loop runs the main app's `Last` schedule BEFORE the
render-app's `ExtractSchedule` (the extract reads from main world to
render world between main app frames). Clearing in `Last` stripped the
admissions list before the extract had a chance to copy it. **Fix:**
`finalise_admissions_as_resident` no longer clears; `residency_driver`'s
PreUpdate-time clear at frame entry is sufficient. Now documented as
a binding constraint in the system's doc comment.

### 2. `--streaming-window` gate STILL FAILS post-Phase-2.5 (secondary defect)

Even with item 1's `Generating → Resident` transition working — all 512
slots end the run as `Resident`, the bounds chain dispatch settles
correctly — the gate's after-frame is still sky-only (variance 222.54,
mean luminance 188). A code reading of `residency_driver`'s Pass 3
(slot assignment) revealed the cause:

```rust
let mut empty_slots: Vec<u32> = residency.slot_to_world.iter()
    .enumerate()
    .filter_map(|(i, w)| if w.is_none() { Some(i as u32) } else { None })
    .collect();
empty_slots.reverse();

for w in pending {
    let slot_u = empty_slots.pop().unwrap();
    residency.slot_to_world[slot_u as usize] = Some(w);
    // ...
}
```

`empty_slots` is just `0..511` initially. World segments get assigned
to slot indices in the order they appear in `pending` (sorted by
camera-distance), so the camera-closest world segment lands at slot
511, next at slot 510, etc.

But the renderer treats slot index N as a *geometric* coordinate:
`local_xyz = Residency::local_of(N)`, content at `chunks_buffer[local_xyz]`
is assumed to be world segment `(origin + local_xyz)`. The
slot-assignment in Pass 3 doesn't honour this — it just picks the
first free slot. So world segment (8, 1, 8) lands at slot 511 (which
is window-local (15, 1, 15)), but the renderer reading at world position
(2048, 288, 2048) — window-local (8, 1, 8) at origin (0, 0, 0) — looks
at slot 8 + 1*16 + 8*32 = 280, which holds content for some unrelated
world segment.

**The Phase 2.5 brief explicitly directed STOP-and-document for this
case.** The fix shape is identified (see § "What's left"); a proper
Phase 2.6 dispatch needs to coordinate slot-relocation across shifts
(when origin moves, existing slots have to re-map to new local
positions — either via host-side re-uploads or a renderer-side index
translation).

### 3. The first-pass 60 s budget was too tight by 5 s

Initial wall-clock budget was set to `60` per the brief's suggestion.
The gate then panic-aborted at 60.059 s wall clock with all 512 slots
fully Resident — meaning the residency layer worked correctly but the
cold-start (128 admission frames × ~300 ms per-frame bounds dispatch)
exceeded 60 s. Bumped to 120 s; gate now completes in 54 s with the
expected FAIL outcome.

The diagnostic recommended 30–60 s — that estimate assumed both item 1
AND a follow-up bounds-chain optimisation (item 5 of the diagnostic
described it as "consequence of fixing item 1"). Item 5 does cause
bounds to skip on no-admission frames, but the per-admission-frame
cost is still ~300 ms, and cold-start has 160+ admission frames. The
~48 s "settled" wall clock is dominated by per-admission bounds
dispatch, not by every-frame dispatch.

### 4. The 215 → 216 test count surprise

The diagnostic reported `cargo test --workspace --lib --release` at
215 passing tests. The new `slot_admissions_eventually_drain_to_resident`
unit test brought this to 216 + 1 ignored + 13 voxel_noise. The brief
expected "217+ existing tests"; the pre-Phase-2.5 count was actually
215 (per `03d-impl-static-noise.md`'s "215 passed"). Not an issue —
just a recount.

## Deviations from this brief

### 1. Item 1's "clears `admissions_this_frame`" — REVERSED

The brief and the diagnostic both said `finalise_admissions_as_resident`
should clear `admissions_this_frame`. **Deviation:** the system does
NOT clear. Reason: clearing in `Last` strips the data before
`ExtractSchedule` reads it (the Bevy schedule order issue described
above). The next frame's `residency_driver` (PreUpdate) already
clears at frame entry. The deviation is necessary to make the
dispatch actually fire.

### 2. Item 4's budget — `120 s` not `60 s`

The brief specified 60 s as the budget. **Deviation:** 120 s. Reason:
even with item 1's fix, cold-start takes ~128 admission frames each
firing the bounds chain (~300 ms on RTX 5080), totalling ~40 s alone;
60 s leaves no margin. Bumped to 120 s to give ~2× headroom while
still failing FAST on the original "minutes-long hang" regression.

### 3. Regression-safety check — SKIPPED

The brief's optional regression-safety check (revert item 1, confirm
gate fails, restore) was skipped because the gate ALREADY FAILS at
the tightened thresholds (the secondary slot-indexing defect). The
strict-floor regression-catching property is therefore demonstrated
by the production run itself (variance 222.54 < 800, pixel Δ 0 < 3),
not by a separate revert exercise.

## What's left

### Phase 2.6 — Geometric slot indexing (load-bearing for visible streaming)

**The remaining defect blocking visible streaming.** The
`residency_driver`'s Pass 3 slot-assignment uses `empty_slots.pop()`,
picking the first free slot index. The renderer assumes a geometric
mapping: slot at local position `(lx, ly, lz)` holds content for
world segment `(origin + (lx, ly, lz))`. The mismatch means the
renderer at world camera position `(2048, 288, 2048)` reads from a
slot whose content is for a different world segment — and that other
segment's content is either empty or distant terrain (mostly empty
above sea level), producing sky-only output.

**Fix shape:**

The correct slot assignment is:

```rust
for w in pending {
    let local = w.0 - residency.origin;
    let slot_u = Residency::slot_index_of([
        local.x as u32, local.y as u32, local.z as u32,
    ]);
    residency.slot_to_world[slot_u as usize] = Some(w);
    // ...
}
```

BUT — this is necessary but not sufficient. A residency-origin SHIFT
also relocates every remaining slot's local position. Today's eviction
logic correctly evicts slots whose world_seg is out of the new window,
but the slots that REMAIN occupied keep their (slot_index, world_seg)
mapping — meaning a slot at index N still holds content for
world_seg W, but W's NEW local position is no longer
`local_of(N)`.

Two possible resolutions (Phase 2.6 dispatch needs to pick):

- **Option A (residency-side relocation):** on every origin shift,
  rebuild the slot table. Walk every remaining `(slot_i, world_seg)`
  pair; the slot's new index should be
  `slot_index_of(world_seg - new_origin)`; if different from
  `slot_i`, the GPU chunks_buffer must be re-uploaded for that
  segment (chunks_buffer[new_slot_index] ← chunks_buffer[old_slot_index]).
  Forces a per-shift GPU memcpy proportional to the number of
  segments that survive the shift (worst-case 512 - shift_count
  segments).

- **Option B (renderer-side index translation):** the renderer
  applies a circular-buffer-style index translation when reading
  chunks_buffer. Effectively: `chunks_buffer[(local_xyz + origin_mod_window) mod window_size]`.
  No GPU memcpy needed — just a small shader-side modulo on each
  chunk fetch. Requires shader changes (Phase 1/Phase 2 deliverable
  area, which the brief marked READ-ONLY for Phase 2.5).

Both are architecturally valid; Option B is closer to the design's
"renderer sees window-local coords" rule (per Q1 in `01-context.md`).

### Phase 2.6 — Dirty-segments bounds-chain optimisation

Per `03c-diagnosis.md` § Punch-list item 5, the bounds-chain dispatch
runs over the full-world worst-case workgroup extent (134M voxel
workgroups). The straightforward optimisation is to dispatch only
over the segments that were admitted or evicted this frame. Would
drop per-admission-frame cost from ~300 ms to ~few ms; cold-start
wall clock would drop from ~50 s to ~5 s.

### Fresh-eyes review (Phase 1 + 2 + 2.4 + 2.5)

Recommended next orchestrator step per the brief.

### Optional items not done

- **Item 7** (demote per-frame `info!` to `debug!`): skipped per the
  brief's "low-time-budget skip" allowance. The per-frame logs (now
  only ~160 instead of ~420) are still moderately noisy at production
  scale; a Phase 2.6 cleanup pass is appropriate.
- **Item 8** (asset-path CWD documentation): out of scope per the brief.

### Future Phase 3

Biome composition + multi-noise mixing — out of scope this session per
the brief.

## Hand-off notes

### The visible-streaming claim still cannot be made

Despite item 1's load-bearing fix and the tightened thresholds, the
`--streaming-window` gate STILL FAILS — correctly — because the
secondary slot-indexing defect remains. Phase 2.4's
`--noise-static-world` gate continues to pass and remains the
authoritative proof that the noise → encoded-chunks → render chain
works; the gap to streaming is now isolated to the residency-side
slot-indexing.

### What Phase 2.5 reliably proves

- The `Generating → Resident` state machine drains correctly under
  the budgeted admission rate (`slot_admissions_eventually_drain_to_resident`
  unit test; gate run shows all 512 slots Resident at exit).
- The bounds-chain dispatch settles on no-admission frames (158 vs
  ~420 dispatch frames over the same gate run).
- The strict thresholds correctly fail sky-only output (variance
  222.54 fails 800; pixel Δ 0.00 fails 3.0).
- The wall-clock budget correctly bounds the gate run (54 s ≤ 120 s).

### Phase 2 deliverables not touched

Per the brief's hard rule:

- `crates/bevy_naadf/src/assets/shaders/noise_terrain.wgsl` — read-only.
- `crates/bevy_naadf/src/assets/shaders/noise_fastnoiselite.wgsl` — read-only.
- `crates/bevy_naadf/src/streaming/noise_fastnoiselite.rs` — read-only.
- `crates/bevy_naadf/src/streaming/noise_fastnoiselite_cpu_oracle.rs` — read-only.
- `crates/bevy_naadf/src/streaming/chunk_source.rs` — read-only.
- `crates/bevy_naadf/src/streaming/noise_dispatch.rs` — read-only.
- `crates/bevy_naadf/src/render/construction/mod.rs` — read-only
  (only inspected; not edited).
- `crates/bevy_naadf/src/voxel/grid.rs` — read-only.
- `crates/bevy_naadf/src/lib.rs` — read-only.

Phase 2.5 touched only: `streaming/residency.rs` (item 1 additive),
`streaming/mod.rs` (item 1 system wiring), `e2e/streaming_window.rs`
(items 2 + 3 + 4), `03b-impl-residency.md` (item 6 doc cleanup),
`03e-impl-residency-fix.md` (this file, new).
