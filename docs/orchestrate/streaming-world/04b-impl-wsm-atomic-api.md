# Phase 2.14.b — impl: WindowedSlotMap atomic API

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world`.
Builds on the audit at
[`04-audit-primitives.md`](./04-audit-primitives.md) (root cause of the 9
`windowed_slot_map` failures: I2 invariant ignored the in-flight state between
`allocate→bind` and `unbind→free`).

User decision (from Q&A, verbatim in the orchestrator brief): **Atomic API
(collapse).** Replace `allocate` + `bind` with `allocate_and_bind(world_seg)`;
replace `unbind` + `free` with `free_segment(world_seg)`. Removes the
in-flight state by design — no possibility of allocate-without-bind or
unbind-without-free.

## Design choices

### Option A vs B for `set_origin`'s eviction path — chose **Option A (per-eviction callback)**

The original `set_origin(new_origin) -> Vec<(WorldSegmentPos, SlotIndex)>`
returned evicted pairs for the caller to `free()` separately. That was the
THIRD source of in-flight state: between the return-from-`set_origin` and the
caller's follow-up `free()`, the slots were neither bound nor in the free
list. The audit caught this in tests #4, #5, #6, #7 (failing with `free +
bound = capacity - N` where N = number of evicted-but-not-yet-freed slots).

The brief offered two reshapes:
- **Option A — callback:** `set_origin(new_origin, on_evict: FnMut(WorldSegmentPos, SlotIndex)) -> usize`.
  Callback fires while the slot is still tracked (caller can do per-eviction
  bookkeeping); `set_origin` internally pushes the slot back to the free pool
  AFTER the callback returns. Returns the number of evictions that fired.
- **Option B — finalizer guard:** `set_origin(new_origin) -> WindowDeltaPending<'_>`
  that holds the evicted list; iterating the guard auto-frees each slot.

**Chose Option A.** Three reasons:

1. **Caller signature is simpler.** The residency_driver Pass 1 caller body
   (record eviction in `evictions_this_frame`, drop from `dispatched_once`)
   ports verbatim into the closure body. The original two-pass loop
   (`for (_w, slot) in &evicted { evictions.push(*slot); dispatched.remove(slot); }`
   followed by `for (_w, slot) in evicted { window.free(slot); }`) collapses
   to a single closure.

2. **No lifetime juggling.** A guard would borrow `&mut self.window`
   for the duration of the iteration, which prevents the caller from
   touching `residency.evictions_this_frame` / `residency.dispatched_once`
   without a split-borrow dance. The closure form sidesteps this — the
   caller uses a destructured `Residency { window, evictions_this_frame,
   dispatched_once, .. }` mut-ref split-borrow at the call site
   (`residency.rs:413`), and the closure captures the non-window fields
   directly.

3. **Matches the existing inline pattern.** Other streaming code uses
   inline callbacks for per-element work (e.g. `for (w, slot) in evicted`).
   The shape is familiar; a guard type would be a new idiom in this
   subsystem.

The brief's strong-recommendation matched Option A; the on-site Pass 1/4
analysis confirmed it.

### I2 invariant — restored to original form

With the atomic API, no slot is ever popped-but-not-bound or
unbound-but-not-freed. The audit invariant I2 (`free_list.len() +
world_to_slot.len() == capacity`) is therefore exact at every mutator
boundary, and the audit at `windowed_slot_map.rs:395-403` reads exactly
as it did in Phase 2.6 (no `in_flight` counter, no third accountant).

A new `debug_assert_eq!` at the END of `set_origin` (post the indirection
rebuild) double-checks the post-condition `free + bound == capacity`
explicitly — this is the strongest form of the "no in-flight escape"
guarantee the brief asked for.

### `allocate_and_bind` failure-mode policy

The brief specified three None conditions:
- pool empty;
- `world_seg` already bound;
- `world_seg` outside the current window.

All three are checked BEFORE the pool is touched. Order is: in-window →
not-already-bound → pool-non-empty. If any fails, state is unchanged. The
order matters for the test surface — the dedicated `allocate_and_bind_is_atomic_under_pool_empty`
test had to use a 1×1×1 tiny window to expose the pool-empty branch
in isolation (otherwise the already-bound branch fires first for any
candidate in a fully-bound window).

The previous Phase 2.6 API panicked on out-of-window binds + double-binds.
Under the atomic API, the failures become non-panicking None returns — the
residency_driver upstream already filters these cases out (the
`resident.contains(&w)` check at Pass 2 + the row-major iteration limited
to in-window positions), so the primitive's defensive return is just
preserving state if the caller misses it.

## Diffs landed

| File | Lines | Change |
|---|---|---|
| `crates/bevy_naadf/src/streaming/windowed_slot_map.rs` | full rewrite (~830 lines) | Removed public `allocate` / `free` / `bind` / `unbind`. Added `allocate_and_bind(world_seg)` / `free_segment(world_seg)`. Re-signatured `set_origin(new_origin, on_evict: FnMut(WorldSegmentPos, SlotIndex)) -> usize`. Rewrote 11 test fns + added 2 invariant tests + dropped 2 obsolete tests (T15 slot-double-bind, T16 free-on-bound-slot — structurally impossible under atomic API). Module docstring updated with Phase 2.14.b rationale at the top. I2 audit text updated to reference the atomic API guarantee. |
| `crates/bevy_naadf/src/streaming/residency.rs` | `~376-394` (Pass 1), `~431-450` (Pass 3), `~715-720` (unit test), `~750-755` (unit test), `~821-826` (unit test) | Pass 1: collapsed `set_origin → evictions loop → free loop` (three sequential loops) to a single `set_origin(new_origin, |_w, slot| { ... })` call with split-borrow capture of `evictions_this_frame` + `dispatched_once`. Pass 3: replaced `window.allocate() + window.bind(w, slot)` two-step with `window.allocate_and_bind(w)`. Three unit-test sites: same allocate+bind → allocate_and_bind collapse. |
| `crates/bevy_naadf/src/e2e/streaming_window.rs` | `:957`, `:994` | Two `set_origin(IVec3, ...)` test sites updated to pass a no-op callback `\|_, _\| {}` (neither call site has bound segments, so the callback never fires). |

## Tests rewritten

Maps the Phase 2.6 T1..T20 surface onto the Phase 2.14.b atomic surface.
"Old name" is the test fn that was in `windowed_slot_map.rs` pre-edit;
"new name" is the test fn that's there now.

| # | Old name | New name | One-line intent |
|---|---|---|---|
| T1 | `new_empty_state` | `new_empty_state` | Empty constructor state — capacity, free count, origin, indirection all zero/empty. |
| T2 | `allocate_returns_slots_in_order_starting_from_zero` | `allocate_and_bind_returns_slots_in_order_starting_from_zero` | First N `allocate_and_bind` calls return SlotIndex(0..N) deterministically. |
| T3 | `allocate_returns_none_when_pool_empty` | `allocate_and_bind_after_exhaustion_returns_none` | After full-window binding, any further `allocate_and_bind` returns None without state change. |
| T4 (formerly failed) | `allocate_free_round_trips` | `allocate_and_bind_free_segment_round_trips` | Bind 100 segments + free all → free_count returns to capacity. |
| T5 | `bind_updates_indirection` | `allocate_and_bind_updates_indirection` | `allocate_and_bind` writes `indirection[pack(local)] = slot.0`. |
| T6 | `bind_round_trip_via_lookup` | `allocate_and_bind_round_trip_via_lookup` | `allocate_and_bind` produces forward + reverse lookups that agree. |
| T7 (formerly failed) | `unbind_clears_indirection` | `free_segment_clears_indirection` | `free_segment` clears the indirection entry AND pushes slot to pool atomically. |
| T8 (formerly failed) | `unbind_returns_slot_for_caller_disposition` | `free_segment_returns_slot_and_pushes_to_pool` | `free_segment` returns the slot index AND has already pushed it to the free pool (atomicity). |
| T8b (new) | — | `free_segment_returns_none_for_unbound_world` | `free_segment` on an unbound world segment is a no-op returning None. |
| T9 | `set_origin_no_shift_returns_empty_vec` | `set_origin_no_shift_returns_zero` | `set_origin(origin)` is a fast-path no-op: returns 0, callback never fires. |
| T10 (formerly failed) | `set_origin_full_evict_returns_all_pairs` | `set_origin_full_evict_fires_callback_for_all_pairs` | Full-window eviction fires callback N times AND leaves every slot in the free pool. |
| T11 (formerly failed) | `set_origin_partial_shift_preserves_in_window` | `set_origin_partial_shift_preserves_in_window` | Partial shift evicts only out-of-window pairs; remaining bindings stay bound at new local coords. |
| T12 (formerly failed) | `set_origin_rebuilds_indirection_correctly` | `set_origin_rebuilds_indirection_correctly` | Post-shift, indirection[pack(local_of(w))] == slot for every remaining bound pair. |
| T13 | `bind_panics_on_out_of_window` | `allocate_and_bind_returns_none_out_of_window` | Out-of-window candidate → None, state unchanged (no panic). |
| T14 (formerly failed) | `bind_panics_on_double_bind_world` | `allocate_and_bind_returns_none_on_double_bind_world` | Already-bound candidate → None, original binding intact (no panic). |
| T15 | `bind_panics_on_double_bind_slot` | DROPPED — obsolete | Caller can no longer choose the slot; double-bind-slot is structurally impossible. Property is implicit in T2 (each call returns a distinct slot). |
| T16 | `free_panics_on_bound_slot` | DROPPED — obsolete | Caller can no longer pass a bound slot to a free-by-slot API; replaced by T8b. |
| T17 | `indirection_buffer_length_equals_capacity` | `indirection_buffer_length_equals_capacity` | Buffer length matches capacity. |
| T18 (formerly failed) | `audit_invariants_after_random_mutations` | `audit_invariants_after_random_mutations` | LCG-driven 200-op fuzz over allocate_and_bind / free_segment / set_origin. Audit fires at every mutator exit. |
| T19 | `pack_round_trip_x_fastest` | `pack_round_trip_x_fastest` | `pack` agrees with `Residency::slot_index_of` across the full window. |
| T20 (formerly failed) | `set_origin_idempotent_under_re_derivation` | `set_origin_idempotent_under_re_derivation` | Second shift to the same new_origin is a no-op (callback never fires; indirection unchanged). |

Pre-edit count: 20 tests (11 passing, 9 failing). Post-edit count: 21 tests
(all passing). Net: +1 test (added T8b; dropped T15 + T16; added 2 invariant
tests below, so 20 - 2 + 1 + 2 = 21).

## Tests added

The brief required two additional invariant tests on top of the rewrites:

| Name | Asserts |
|---|---|
| `allocate_and_bind_is_atomic_under_pool_empty` | When the pool is empty AND the candidate is in-window AND not bound, `allocate_and_bind` returns None and leaves bidirectional mapping + indirection buffer untouched. Uses a 1×1×1 tiny window to expose the pool-empty branch in isolation. |
| `set_origin_no_in_flight_after_full_evict` | Post-`set_origin` with a callback that does nothing, `free_count() + iter_bound().count() == capacity` strictly. Pins the "no slot escapes from both free_list and world_to_slot" guarantee. |

Note: the original Phase 2.6 audit at `windowed_slot_map.rs:395-403` also
fires this invariant via debug_assert at every mutator exit; the test is
the explicit unit-suite version of that property.

## Verification

Per the brief — each gate run ONCE with a wall-clock timeout.

```
cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world

# 1. workspace build
timeout 180s cargo build --workspace 2>&1 | tail -100
# → "Finished `dev` profile [optimized + debuginfo] target(s) in 34.61s"
# → GREEN

# 2. windowed_slot_map test suite
timeout 180s cargo test --workspace --lib windowed_slot_map 2>&1 | tail -120
# → "test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 243 filtered out"
# → GREEN — all 21 windowed_slot_map tests pass (was 11/20 before; gain of +10 — 9 fixed + 2 new + 1 added — minus 2 dropped obsolete)

# 3. full library test suite
timeout 300s cargo test --workspace --lib 2>&1 | tail -60
# → "test result: ok. 263 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
# → GREEN — pre-existing 253 pass count + 9 newly-fixed + 1 net test delta = 263 (matches budget)
```

Per `CLAUDE.md` discipline: no `cargo run --bin bevy-naadf` smoke,
no e2e gates run. Phase 2.14.b is primitive-only.

## Out-of-scope findings

None. The audit's other identified concerns (dispatch lifecycle ACK
mechanics, `StreamingDiagnostics` surface, `compute_window_delta`
extraction) were not touched. The Pass 1 + Pass 3 code reads cleanly
under the atomic API; nothing surfaced during the refactor that needed
flagging.

One small style note (not an actionable finding): the residency.rs Pass 1
now uses an explicit destructured split-borrow at the `set_origin` call
site. This is the canonical idiom for projecting disjoint &mut fields
into a closure scope; it's clearer than the alternative of staging the
work into intermediate buffers and applying them in a second pass. The
existing residency.rs Pass 4 and `apply_dispatch_acks` patterns don't
need this pattern (no captured field is split across the `&mut self`
they use).
