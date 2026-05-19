# Phase 2.14.c — impl: `compute_window_delta` primitive extraction

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world`.

Builds on Phase 2.14.b at
[`04b-impl-wsm-atomic-api.md`](./04b-impl-wsm-atomic-api.md). Per the audit
([`04-audit-primitives.md`](./04-audit-primitives.md) §"Proposed primitive
extractions" item 2), the "old vs new origin → (evict, admit)" computation
was split across `WindowedSlotMap::set_origin` (evict half) and
`residency_driver` Pass 2 (admit half, three nested loops). This phase
collapses both halves into one pure-compute primitive with its own unit
tests.

## Design choices

### Option A vs Option B for `set_origin` integration — chose **Option A**

The brief offered two integration shapes:

- **Option A** — `set_origin` keeps its 2.14.b signature
  (`fn set_origin<F>(&mut self, new_origin: IVec3, on_evict: F) -> usize`),
  and Pass 2 in `residency.rs` calls `compute_window_delta` directly to
  obtain the admit list. The pre-shift `world_to_slot.iter()` loop inside
  `set_origin` is left untouched. **Pure refactor of Pass 2 only.**
- **Option B** — `set_origin` returns `WindowDelta`, computing both halves
  inside the primitive and exposing them to the caller. Pass 2 consumes
  `delta.admit`; the eviction callback flow stays but the return shape
  changes. **API change to a method that just landed in 2.14.b.**

**Chose Option A.** Reasons:

1. **Preserves 2.14.b's API surface.** All 21 `windowed_slot_map` tests
   ported from Phase 2.6 → 2.14.b assert against the exact return shape
   (`usize`, the number of evictions). Switching to `WindowDelta`
   would have rewritten all 6 `set_origin*` tests (T9, T10, T11, T12,
   T20, plus the two new invariant tests) for a marginal benefit.

2. **The eviction-half computation inside `set_origin` reads
   `world_to_slot.iter()` directly** — efficient (no
   `HashSet<WorldSegmentPos>` construction). Routing it through
   `compute_window_delta` would have required either (a) constructing a
   throwaway `HashSet` from `world_to_slot.keys()` (allocation), or
   (b) widening the primitive's signature to accept an iterator (heavier
   API surface). Neither pays for itself when the in-place iteration is
   already 3 lines.

3. **Pass 2 was the actual victim of the split** — the three-nested-loop
   admit computation lived directly in `residency_driver`, not behind any
   primitive, so it was uniquely vulnerable to a regression (no unit test
   exercised the iteration order or the filter logic in isolation). The
   admit-half extraction alone delivers the audit's stated value.

4. **The brief acknowledged "Implementer's call; either is acceptable
   as long as Pass 2 no longer has its own three-nested-loop
   computation."** Option A satisfies that constraint without churning
   2.14.b's surface.

### Iteration order — explicitly pinned

`compute_window_delta` iterates the new window in `for lz / for ly /
for lx` (X-fastest) order, matching the pre-extraction Pass 2 shape.
This is documented in the module docstring + the impl comment + the
`partial_diagonal_shift` test's spatial assertion. Downstream
slot-assignment (`process_pending_admissions` re-sorts by
camera-distance — order-independent) does NOT depend on the iteration
order, but the cold-start admission sort + the `oasis-edit-visual`
e2e pixel-diff gate (Phase 2.14.g) would surface a regression if the
order changed silently.

### Old origin parameter — held but unused

`compute_window_delta` takes `old_origin: WorldSegmentPos` even though
the impl ignores it. The argument is part of the contract for symmetry
+ future enrichment (e.g. fast-path no-op short-circuit when
`old_origin == new_origin`). Pass 2 captures `old_origin` BEFORE
`set_origin` mutates the residency, even though it doesn't affect the
result — that's the future-proof shape per the audit's design.

## Diffs landed

| File | Lines | Change |
|---|---|---|
| `crates/bevy_naadf/src/streaming/sliding_window.rs` | **NEW**, 339 LOC (165 impl + module docs + 6 unit tests) | New pure-compute primitive module. `pub struct WindowDelta { evict, admit }` + `pub fn compute_window_delta(old_origin, new_origin, window_size, currently_bound) -> WindowDelta`. No Bevy World types, no `&mut`, no GPU types — pure compute over `WorldSegmentPos` + `IVec3` + `HashSet`. |
| `crates/bevy_naadf/src/streaming/mod.rs` | `:32`, `:58` | `pub mod sliding_window;` declaration + `pub use sliding_window::{compute_window_delta, WindowDelta};` re-export. |
| `crates/bevy_naadf/src/streaming/residency.rs` | `:369-376` (capture old_origin), `:398-426` (Pass 2 admit replacement) | Pass 2: removed the three-nested-loop admit computation. Now calls `compute_window_delta(old_origin, new_origin, window_size, &resident)` and consumes `delta.admit`. The pre-shift `old_origin` is captured before `set_origin` runs (`old_origin = residency.origin()` at `:373`). The remaining `pending.sort_by_key(camera_distance_squared)` pass is unchanged. |

No changes to `windowed_slot_map.rs` — its `set_origin` method is left
verbatim per Option A. The 21 existing tests are unaffected.

## Tests added

Six new unit tests live in `sliding_window.rs::tests`. Per the
audit's invariant list + the brief's two extras (full-shift +
partial-diagonal). All tests use small synthetic windows (commonly
`UVec3::new(4, 2, 4)` → 32 cells) for deterministic hand-computed
expected values.

| Name | Asserts |
|---|---|
| `identity_no_shift_admits_all_unbound_in_window` | `old_origin == new_origin` + empty `currently_bound` → empty `evict`, `admit` covers every segment in the window exactly once. Each cell in the new window AABB appears in `admit`. |
| `translation_x_plus_one_evicts_leftmost_admits_rightmost` | `+1` X shift over fully-bound window → `evict.len() == y * z`, `admit.len() == y * z`. Every eviction at OLD `world.x == old_origin.x` (the leftmost-X slab). Every admission at NEW `world.x == new_origin.x + window_size.x - 1` (the rightmost-X slab). |
| `evict_admit_disjoint` | Arbitrary `(2, 0, 1)` shift over fully-bound window. `HashSet`-intersection of `evict` and `admit` is empty. |
| `closure_invariant` | `(1, 0, 1)` shift over fully-bound window. Every segment in the new window is in EXACTLY ONE of `(currently_bound \ evict)` or `admit`. Uses XOR to assert mutual exclusivity + total coverage. |
| `full_shift_no_overlap_evicts_all_admits_all` | Shift by exactly `window_size.x` on X (no overlap). Every previously-bound segment evicts; every new window cell admits. `evict_set == currently_bound`. |
| `partial_diagonal_shift` | `(1, 0, 1)` shift, 4×2×4 window. Hand-computed expected counts: evict-count = admit-count = `(y*z) + (y*x) - y = 8 + 8 - 2 = 14`. Spatial check: every eviction on OLD `local.x == 0` OR OLD `local.z == 0` slab; every admission on NEW rightmost-X OR backmost-Z slab. |

## Behaviour-preservation evidence

The refactor must produce byte-identical evict/admit sets at the call
sites. Continued-passing tests demonstrate this:

| Suite | Tests that exercise admit/evict logic | Outcome |
|---|---|---|
| `streaming::windowed_slot_map::tests` (21 tests) | `set_origin_no_shift_returns_zero`, `set_origin_full_evict_fires_callback_for_all_pairs`, `set_origin_partial_shift_preserves_in_window`, `set_origin_rebuilds_indirection_correctly`, `set_origin_no_in_flight_after_full_evict`, `set_origin_idempotent_under_re_derivation`, `audit_invariants_after_random_mutations`, `allocate_and_bind_free_segment_round_trips` | All 21 continue to pass (full suite green). |
| `streaming::residency::tests` (10 tests) | `slot_admissions_eventually_drain_to_resident`, `process_pending_admissions_does_not_mark_dispatched_once`, `is_cold_start_complete_tracks_full_admission` | All 10 continue to pass. |
| `e2e::streaming_window::tests` (in-process residency-shape tests, NOT runtime gate) | `pin_translates_world_to_window_local_origin_shifted`, `pin_translation_is_idempotent_under_re_derivation` | Both continue to pass (they call `window.set_origin(IVec3, |_,_| {})` directly — unchanged surface). |
| New `streaming::sliding_window::tests` (6 tests) | All 6 listed above | All pass on first run. |

Whole-workspace lib test count: **269 passed; 0 failed; 1 ignored**
(== 263 from Phase 2.14.b + 6 new sliding_window tests). Exactly the
target the brief specified.

## Verification

Per the brief — each gate run ONCE with a wall-clock timeout. No
`cargo run --bin bevy-naadf` (forbidden per CLAUDE.md). No e2e gates
(those are Phase 2.14.g).

```text
cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world

# 1. workspace build
timeout 180s cargo build --workspace 2>&1 | tail -100
# → "Finished `dev` profile [optimized + debuginfo] target(s) in 17.04s"
# → GREEN

# 2. sliding_window unit suite
timeout 180s cargo test --workspace --lib sliding_window 2>&1 | tail -120
# → "test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 264 filtered out"
# → GREEN — all 6 sliding_window tests pass

# 3. full library test suite
timeout 300s cargo test --workspace --lib 2>&1 | tail -60
# → "test result: ok. 269 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
# → GREEN — pre-existing 263 pass count + 6 new sliding_window tests = 269 (matches budget)
```

windowed_slot_map subset (regression check):

```text
timeout 120s cargo test --workspace --lib windowed_slot_map 2>&1 | tail -30
# → "test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 249 filtered out"
# → GREEN — Phase 2.14.b atomic-API surface still intact
```

## Out-of-scope findings

None. The Phase 2.14.b atomic API reads cleanly with the primitive
delegating only the Pass 2 admit half; nothing else surfaced during
the refactor that needed flagging.

One minor observation (not actionable here): the `compute_window_delta`
`old_origin` parameter is unused in the current impl. A future
fast-path `if old_origin == new_origin { return WindowDelta { evict:
vec![], admit: ... } }` short-circuit could exploit it — but that's
a Phase 2.14.f+ enrichment, not a behaviour-preserving refactor.
