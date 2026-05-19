# Phase 2.14.a — Primitive audit (read-only)

Scope: streaming-world primitives + 9 failing `windowed_slot_map` unit tests.
Mode: READ-ONLY. No code edits, no `cargo build`. One `cargo test` run only.

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world`.

## Test triage — 9 failing windowed_slot_map tests

Single root cause shared by all 9 failures. Confirmed by one `cargo test
--workspace --lib windowed_slot_map` run.

**Root cause (HIGH confidence)**: invariant `I2` at
`crates/bevy_naadf/src/streaming/windowed_slot_map.rs:375-382` asserts
`free_list.len() + world_to_slot.len() == capacity`. This invariant
ignores the **in-flight** state that `allocate()` → `bind()` creates:
`allocate()` pops from `free_list` (free shrinks by 1) without inserting
into `world_to_slot` (bound does NOT grow). At that moment
`free + bound == capacity - 1`, which `audit_invariants` will trip the
next time any other mutator runs.

A second, symmetric in-flight state exists between `unbind()` and the
caller's follow-up `free()` (per the design at `02c-design-windowed-
slot-map.md`, `unbind()` returns the slot to the caller without pushing
it back to the free list — so for that window the slot is neither bound
nor free).

The implementer's escalation note in `03s-impl-cold-start-fix.md:421`
("free + bound != capacity invariant") matches what we observed: every
single failure prints `I2: free + bound must equal capacity` with a
sum strictly less than 512.

Per-test breakdown:

| # | Test | Invariant asserted | Observed (free / bound / cap) | Hypothesis |
|---|---|---|---|---|
| 1 | `allocate_free_round_trips` (`:508`) | `I2: free + bound == cap` (`:375`) | 413 / 0 / 512 | After 100×`allocate`, the test calls `free` on the first held slot. Audit fires at end of `free` (`:177`). State: `free=412+1=413`, `bound=0` (never bound). 99 slots are in-flight (held by the caller). I2 doesn't count them. **Real primitive bug — invariant is wrong, not the test.** |
| 2 | `unbind_clears_indirection` (`:544`) | `I2` (`:375`) | 511 / 0 / 512 | `allocate(s)` + `bind(w,s)` + `unbind(w)`. After unbind: `free=511`, `bound=0`. Slot is in caller's hand (return value of unbind). 1 in-flight. **Same root cause.** |
| 3 | `unbind_returns_slot_for_caller_disposition` (`:559`) | `I2` (`:375`) | 511 / 0 / 512 | Identical flow to #2. **Same root cause.** |
| 4 | `set_origin_full_evict_returns_all_pairs` (`:587`) | `I2` (`:375`) | 507 / 0 / 512 | 5×(`allocate`+`bind`) then `set_origin(...)` evicts all 5; evictions are RETURNED, not freed (`windowed_slot_map.rs:296-303` strips bindings without pushing to `free_list`). After: `free=507`, `bound=0`, 5 in-flight. **Same root cause.** |
| 5 | `set_origin_partial_shift_preserves_in_window` (`:604`) | `I2` (`:375`) | 496 / 15 / 512 | 16×(`allocate`+`bind`), then `set_origin(1,0,0)` evicts the segment at local-x=0. After: `free=496`, `bound=15`, 1 in-flight (the evicted slot). **Same root cause.** |
| 6 | `set_origin_rebuilds_indirection_correctly` (`:627`) | `I2` (`:375`) | 496 / 15 / 512 | Same shape as #5 — single eviction returns 1 in-flight slot. **Same root cause.** |
| 7 | `set_origin_idempotent_under_re_derivation` (`:806`) | `I2` (`:375`) | 507 / 4 / 512 | 5×(`allocate`+`bind`), `set_origin(1,0,0)` evicts 1, returns it (in-flight). Second `set_origin(1,0,0)` is the no-op fast-path BUT it still calls `audit_invariants` at `:265`. State: `free=507`, `bound=4`, 1 in-flight. **Same root cause.** |
| 8 | `bind_panics_on_double_bind_world` (`:657`) | Expected panic "already bound to" (`:204-205`); got panic from I2 (`:375`) | 510 / 1 / 512 | 2×`allocate` (2 in-flight), then `bind(w, s1)` audits at exit (`:223-224`). State: `free=510`, `bound=1`, 1 still in-flight (s2). I2 fires before the second `bind` can reach its own debug_assert. **Same root cause — also masks the legitimate test signal.** |
| 9 | `audit_invariants_after_random_mutations` (`:706`) | `I2` (`:375`) | 509 / 1 / 512 (sample) | LCG-driven sequence of allocate/bind/unbind/set_origin/free that randomly leaves slots in-flight. **Same root cause.** |

### Confirmation that this is the test's failure mode, not a different bug

The audit error message at `:378-381` literally prints `(free={}, bound={}, cap={})` and every panic above has `free + bound < cap` by exactly the count of slots whose `allocate()` was not followed by a matching `bind()`, OR whose `unbind()` return value was not followed by a matching `free()`. The arithmetic is exact in every failure.

The 11 tests that PASS are the ones that never expose an in-flight slot: `new_empty_state`, `allocate_returns_slots_in_order_starting_from_zero`, `allocate_returns_none_when_pool_empty` (this test pops 512 slots but never triggers another mutator — `allocate` does not call audit), `bind_round_trip_via_lookup`, `bind_updates_indirection`, `indirection_buffer_length_equals_capacity`, `pack_round_trip_x_fastest`, `set_origin_no_shift_returns_empty_vec` (BUT this passes only because nothing is in-flight at the moment of the no-op), `bind_panics_on_out_of_window` (panics in `bind` BEFORE the audit runs), `bind_panics_on_double_bind_slot` (panics inside `bind` BEFORE audit), `free_panics_on_bound_slot` (panics inside `free` BEFORE audit).

### The fix is either-or

Either:
1. Add a third invariant accountant — track `in_flight: u32` counter incremented by `allocate()` / decremented by `bind()` / `free()` — and change I2 to `free + bound + in_flight == cap`. The `unbind → free` window collapses if `unbind` also increments `in_flight` and `free` decrements it.
2. Make `allocate` + `bind` atomic — `pub fn allocate_and_bind(world_seg) -> Option<SlotIndex>` — and forbid the bare `allocate()` API. Same for `unbind` + `free` collapsed into one method `free_segment(world_seg)` that returns nothing. This is a bigger API change and affects `residency.rs:432-450` callers.

Option 1 is the lighter touch. Option 2 closes the design hole that allowed the bug to exist. Either way, this is a real primitive bug, not a stale test.

## Primitive inventory

| Concept | Verdict | File:line / functions |
|---|---|---|
| **Pool of free slots** | EXTRACTED (with bug above) | `WindowedSlotMap::{allocate, free, free_count}` at `crates/bevy_naadf/src/streaming/windowed_slot_map.rs:156-178`. Closed-API. |
| **Indirection table (world↔slot bidirectional + GPU buffer)** | EXTRACTED | `WindowedSlotMap::{bind, unbind, lookup_slot, lookup_world, iter_bound, indirection_buffer, pack}` at `windowed_slot_map.rs:128-353`. Closed-API. |
| **Sliding window translation (compute evict/admit sets from old vs new origin)** | PARTIAL | `WindowedSlotMap::set_origin` at `windowed_slot_map.rs:262-328` computes evict set. **Admit set is computed elsewhere** in `residency_driver` Pass 2 at `residency.rs:392-422` — three nested `for lz/ly/lx` loops + a HashSet check. The "given old vs new origin, produce (evict, admit)" pure-compute primitive is split across two files. |
| **Slot-assignment policy (sorted admission list + N free + dispatched_once → pick K)** | ADHOC | `process_pending_admissions` at `residency.rs:486-535`. Iterates `window.iter_bound()` + filters `dispatched_once` + sorts by camera-distance-squared + takes first `cap`. Closed-form pure compute but lives directly in the system function. |
| **Dispatch lifecycle state machine (Empty/Generating/Resident/Evicted)** | ADHOC | Scattered across **3 files**: (a) main world reads `window.free_list` + `window.iter_bound()` + `Residency::admissions_this_frame` + `Residency::dispatched_once` (`residency.rs:486-535`); (b) cross-world ACK in `noise_dispatch.rs:414-425` (`PENDING_DISPATCHED_ONCE_SLOTS`); (c) main-world ACK drain in `apply_dispatch_acks` at `residency.rs:549-565`. The "state machine" is **implicit** per `residency.rs:23-31` comment block — there is no `enum SlotState` anymore. |
| **Camera-window coverage (camera pose → expected resident set)** | ADHOC | `target_origin_for_camera_seg` at `residency.rs:239-244` returns origin; the expected resident set is then the row-major iteration at `residency.rs:405-422`. There is **no `expected_resident_set(camera_pose) -> HashSet<WorldSegmentPos>` function** and no `unfulfilled_segments(camera_pose) -> Vec<WorldSegmentPos>` function. |
| **Clear-on-bind cross-world accumulator** | EXTRACTED (as plain static Mutex<Vec>) | `PENDING_CLEAR_ON_BIND_SLOTS` at `noise_dispatch.rs:373-374`. The pattern is twin'd with `PENDING_DISPATCHED_ONCE_SLOTS` (`:414-415`). Could be a `CrossWorldSlotChannel<Direction>` primitive but the two flow directions are different (main→render vs render→main); current ad-hoc shape is acceptable. |

## Internal observability — what exists today

- `streaming.diagnostics()` / `streaming.health()` method — **ABSENT.**
- Counters `slots_in_state(SlotState)` — **ABSENT.** Closest is `Residency::is_cold_start_complete()` at `residency.rs:208-210` (returns `bool` from `dispatched_once.len() == 512`).
- `unfulfilled_camera_window_segments(camera_pose) -> Vec<WorldSegment>` — **ABSENT.**
- `slots_pending_dispatch_ack() -> usize` — **ABSENT.** Would be `PENDING_DISPATCHED_ONCE_SLOTS.lock().len()` but not exposed.
- Startup-time check after N frames that no camera-window segments are unfulfilled — **ABSENT** for production; only the e2e gate `--gate streaming-cold-start` (`crates/bevy_naadf/src/e2e/streaming_cold_start.rs`) does this.
- `debug_assert!` calls in streaming layer — present **only inside `WindowedSlotMap`** (`windowed_slot_map.rs:164, 169, 189, 196, 203, 210, 340, 363-455`). Zero `debug_assert!` in `residency.rs`, `noise_dispatch.rs`, `camera.rs`.
- Structured `info!` / `warn!` / `error!` lines in residency layer — exactly **one** at `residency.rs:456` (`info!("streaming-world residency shift: cam_seg=…, new_origin=…, evictions=…, bound_segments=…, admissions_this_frame=…")` — fires on every origin shift). Plus the SHOULD-2 `warn_once!` for missing `WorldGpu` at `render/construction/mod.rs:3113-3129` (outside the streaming module).
- Per-state slot counters (Empty / Generating / Resident / Evicted) — **ABSENT.** Counts are recoverable via `window.free_count()` (= Empty), `window.iter_bound().count() - dispatched_once.len()` (= Generating), `dispatched_once.len()` (= Resident), `evictions_this_frame.len()` (= Evicted-this-frame). All recoverable, none exposed as a named method.
- "Cold-start incomplete after N frames" alarm — **ABSENT.**
- `PENDING_DISPATCHED_ONCE_SLOTS` queue depth observation — **ABSENT.**

Summary: the layer ships **one info log per shift** and `is_cold_start_complete() -> bool`. Everything else the user asked for ("the system must know if it HAS unfulfilled slots in the middle at startup") would need to be added.

## Proposed primitive extractions

The user's redirect was specifically "data structure primitives and testing them independently — sliding window, slot accessment, etc." The audit verdict: the bugs and the missing observability map cleanly onto three extractions.

### 1. `WindowedSlotMap` — fix the existing primitive's invariant (no new type)

Not a new primitive — just a fix to the existing one.

- **API surface (unchanged):** `new, capacity, origin, window_size, is_in_window, local_of, lookup_slot, lookup_world, iter_bound, indirection_buffer, free_count, allocate, free, bind, unbind, set_origin, pack`.
- **Invariants to fix:** add `in_flight: u32` counter (or equivalent: `free_list.len() + bound + in_flight == capacity`). Increment on `allocate`, decrement on `bind`. Increment on `unbind`'s return path, decrement on `free`. The 9 failing tests then pass without modification.
- **Optional API additions:**
  - `in_flight_count() -> u32` — for diagnostics.
  - `audit_invariants_pub(&self)` — extracted from `cfg(debug_assertions)`-only into an `&self` query callable from tests + diagnostics. Currently `audit_invariants` is `cfg(debug_assertions)` + private.
- **LOC estimate:** ~30 (struct field + 3 line-changes per mutator + 1 line per audit clause + the 9 tests then pass).
- **Test invariants it would newly carry:** `allocate + bind` strictly preserves `free + bound + in_flight == capacity`; same for `unbind + free`; same for `set_origin`. Re-run the existing 9 failing tests as the regression catcher.

### 2. `SlidingWindowDelta` — pure compute "old vs new origin → (evict, admit)"

Currently spread across `WindowedSlotMap::set_origin` (evict half) + `residency_driver` Pass 2 (admit half). Pulling it into one pure function makes camera-window coverage testable without `Residency` setup.

- **Name:** `compute_window_delta(old_origin, new_origin, window_size, currently_bound: &HashSet<WorldSegmentPos>) -> WindowDelta { evict: Vec<WorldSegmentPos>, admit: Vec<WorldSegmentPos> }`.
- **Closed API surface (1 function, 1 struct):**
  - `compute_window_delta(...)`.
  - `pub struct WindowDelta { pub evict, pub admit }`.
- **Invariants the test suite would carry:**
  - Identity: `compute_window_delta(o, o, _, bound)` → empty evict + admit set covering only initially-unbound segments. (Test against current `set_origin`'s no-op fast-path.)
  - Translation: shifting origin by `+1` on X evicts the leftmost-X slab and admits the rightmost-X slab (size = `y * z`).
  - Coverage: `evict.intersection(admit).is_empty()`.
  - Closure: every segment in `[new_origin, new_origin + window_size)` is either in `currently_bound \ evict` or in `admit`.
- **LOC estimate:** ~60 impl + ~80 tests.

### 3. `StreamingDiagnostics` — analytical-invariant surface (see next section)

Treated separately below.

### 4. (REJECTED proposal) `DispatchLifecycle` enum + state-transition primitive

Considered, rejected. The state-machine is already implicit and Phase 2.6 deliberately collapsed it (`residency.rs:23-31` documents this). Re-introducing an enum is a regression. The state is recoverable analytically from `WindowedSlotMap` + `dispatched_once` + cross-world accumulator; the right move is to **expose a query** (`StreamingDiagnostics::slots_in_state(...)`), not re-add a state field.

### 5. (REJECTED proposal) `SlotAssignmentPolicy` extraction

Considered, rejected. The pick-logic in `process_pending_admissions` (`residency.rs:486-535`) is 30 lines of straight-line compute (filter / map / sort / take). Extracting it would not surface a meaningful invariant the inline code doesn't already test. The bug class it would protect against (re-pick of dispatched slots) is already covered by the new `process_pending_admissions_does_not_mark_dispatched_once` test at `residency.rs:817`. Pulling 30 lines into a new file is not worth the import noise.

## Proposed StreamingDiagnostics surface

A `Resource` or method on `Residency` that exposes the analytical state the user asked for. Cheap-enough to call every frame in debug builds; cheap-enough to call once-per-second in release for a status line.

Naming proposal: `Residency::diagnostics(&self, camera_pose: WorldSegmentPos) -> StreamingDiagnostics`. Method (not Resource) — the snapshot is computed on demand from the existing `Residency` fields; storing it as a Resource would just be a stale mirror.

```rust
pub struct StreamingDiagnostics {
    // counters (every-frame cheap)
    pub free_slots: u32,
    pub bound_slots: u32,
    pub in_flight_slots: u32,       // <- exposes the I2 hidden state
    pub dispatched_once_slots: u32,
    pub generating_slots: u32,      // bound - dispatched_once
    pub pending_clear_on_bind: usize,
    pub pending_dispatch_acks: usize,
    pub frame_counter: u64,

    // analytical (one-shot at startup / diagnostic logging)
    pub cold_start_complete: bool,
    pub camera_window_segments_total: u32,
    pub camera_window_segments_unfulfilled: u32,
    pub unfulfilled_camera_window_segments: Vec<WorldSegmentPos>,
}
```

### Methods

| Method | Returns | Call frequency | Cost |
|---|---|---|---|
| `Residency::diagnostics(&self, cam_pose)` | `StreamingDiagnostics` | Every frame debug / once-per-second release | O(window_size) for unfulfilled scan = O(512), trivial |
| `Residency::slot_counters(&self)` | `(free, bound, in_flight, dispatched_once)` | Every frame | O(1) |
| `Residency::is_cold_start_complete(&self)` | `bool` | Already exists at `residency.rs:208-210` | O(1) |
| `Residency::unfulfilled_camera_window_segments(&self, cam_pose) -> Vec<WorldSegmentPos>` | List | One-shot at startup / on shift | O(window_size) = O(512) |
| `streaming::pending_dispatch_ack_count() -> usize` | `usize` | Every frame | O(1) under Mutex — cheap |
| `streaming::pending_clear_on_bind_count() -> usize` | `usize` | Every frame | O(1) under Mutex — cheap |

### Wiring

Plain methods on `Residency` (no new resource). Two free functions on `noise_dispatch` for the cross-world accumulator depths (read under their existing Mutex; trivial). Optionally a `StreamingPlugin` post-startup system that logs the analytical snapshot at frames 50 / 200 / 500 — calling `Residency::diagnostics` directly. The user's "must know if it HAS unfulfilled slots in the middle at startup, not via screenshots" requirement is satisfied by `unfulfilled_camera_window_segments` returning a non-empty list at frame 200 → log it as `warn!`.

For the production binary's interactive smoke, a `Last`-stage system that periodically logs `Residency::diagnostics(camera_segment_pos())` at a debounced interval (e.g. every 60 frames once cold-start completes, every 10 frames before) gives the user a continuous self-report of what the streamer thinks it has and what it's missing.

## Sequencing recommendation

Bottom-up per the user's redirect.

1. **Phase 2.14.b — fix `WindowedSlotMap` invariant.**
   - Add `in_flight` counter (or atomic-pair API).
   - Update I2 to `free + bound + in_flight == capacity`.
   - 9 existing failing tests must pass without modification.
   - Add 2 new tests: `allocate_then_audit_via_no_op_set_origin_passes` + `unbind_without_free_passes_audit`.
   - Estimated LOC: +30 impl, +20 tests.

2. **Phase 2.14.c — extract `compute_window_delta` primitive.**
   - Pure function in `streaming/sliding_window.rs` (new file, ~60 LOC).
   - 4 unit-test invariants per "Proposed primitive extractions" §2.
   - Call site in `residency_driver` Pass 2 reduces to `let delta = compute_window_delta(...)` + iterate.
   - **No production behaviour change** — the existing logic produces the same evict/admit sets; this is a refactor.

3. **Phase 2.14.d — `StreamingDiagnostics` surface.**
   - 5 methods per spec above.
   - 1 startup diagnostic system in `StreamingPlugin` that logs `unfulfilled_camera_window_segments` at frame 200 (configurable threshold).
   - 1 unit test per method.
   - Estimated LOC: +120 impl, +80 tests.

4. **Phase 2.14.e — composition tests.**
   - Take the now-isolated primitives + run them together against a synthetic camera-walk trace.
   - No GPU, no Bevy app — just the data structures driven by a script of `(camera_seg, frames_elapsed)` tuples.
   - Assert: at every frame, `unfulfilled_camera_window_segments` either decreases or holds steady; never increases (after cold-start).
   - Estimated LOC: ~150 (new module `streaming/tests_composition.rs`).

5. **Phase 2.14.f — promote diagnostics into production logging.**
   - Wire `StreamingDiagnostics` into the existing `info!` line at `residency.rs:456` (it currently logs 5 fields; we extend to include unfulfilled count + in-flight + cold-start state).
   - Add an explicit `warn!` if at frame 500 the unfulfilled count is non-zero — the analytical version of "user sees a sky-coloured hole in the camera-near segments at startup."

6. **Phase 2.14.g — only after the above** — re-run the e2e gate suite. Phase 2.13's gates (`--gate streaming-cold-start`, `--gate streaming-window`, `--gate oasis-edit-visual`) become regression catchers for this lower-level work, not the primary signal.

Dependencies: 2 depends on 1 (the fix unblocks running the test suite cleanly). 3 depends on 1. 4 depends on 2 + 3. 5 depends on 4 (composition tests gate the integration). 6 depends on everything.

## Open questions for the user

1. **Atomic API vs in-flight counter?** Phase 2.14.b can go either way. Option A: add `in_flight` counter + update I2 (smallest surface). Option B: collapse `allocate + bind` into one method (`allocate_and_bind`), collapse `unbind + free` into one (`free_segment`), forbidding the bare two-step. Option B closes the design hole permanently but changes the caller signature in `residency_driver` Pass 3 (`residency.rs:431-450`) — 5 lines of caller change. Either reasonable; user pick determines whether the in-flight state remains a thing at all.

2. **Where does `StreamingDiagnostics` live — method on `Residency`, free function, or new `DiagnosticsPlugin`?** Method on `Residency` is lowest surface; `DiagnosticsPlugin` is more idiomatic Bevy. The user's "system must know" suggests a periodic logging system that needs a place to live — that pushes toward a plugin extension. Default proposal: method on `Residency` + one logging system inside `StreamingPlugin` that calls it. If the user wants a separate `StreamingDiagnosticsPlugin` to make the logging configurable independent of streaming, that's a 10-line refactor later.

3. **`unfulfilled` predicate scope — full window or camera-near ring only?** The cold-start bug specifically burned the camera-nearest 4-24 segments (dsq ≤ 2 ring). The `streaming-cold-start` e2e gate (`e2e/streaming_cold_start.rs:62-67`) inspects the 14 segments at `dsq ≤ 2`. Should `unfulfilled_camera_window_segments` default to the full 512 slots or to a configurable ring? Default proposal: full 512; the caller's `Vec` filter handles the ring case.

4. **Borderline: do we need a separate `DispatchLifecycle` primitive?** I rejected it above. The user's wording ("dispatch lifecycle state machine") may have meant a more structural type. Verdict: the existing implicit state is correct and the new `StreamingDiagnostics::slots_in_state` query gives the same observability without re-introducing a `SlotState` enum. Flag this for confirmation before Phase 2.14.d lands.
