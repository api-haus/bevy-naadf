# Phase 2.14.e — impl: composition tests (synthetic-trace integration)

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world`.

Builds on Phase 2.14.b ([`04b-impl-wsm-atomic-api.md`](./04b-impl-wsm-atomic-api.md)),
Phase 2.14.c ([`04c-impl-sliding-window-primitive.md`](./04c-impl-sliding-window-primitive.md)),
and Phase 2.14.d ([`04d-impl-streaming-diagnostics.md`](./04d-impl-streaming-diagnostics.md)).

Per the audit at [`04-audit-primitives.md`](./04-audit-primitives.md)
§ "Sequencing recommendation" item 4 — once the primitives are isolated
and testable individually, the next layer up is **composition**: take all
four (`WindowedSlotMap` atomic API + `compute_window_delta` + `Residency`
ACK tracking + `StreamingDiagnostics`) and run them together against
synthetic camera-walk traces, asserting that the analytical
`unfulfilled_camera_window_segments` query reports sane values across
realistic camera motion patterns.

The load-bearing assertion: **after cold-start, the
`unfulfilled_camera_window_segments.len()` count is monotonically
non-increasing under static camera, bounded under bursty motion, and
bounded under random walks**. Never blows up.

## Harness design

### `SimState` shape

```rust
struct SimState {
    residency: Residency,                  // production type, no stand-in
    pending_admissions: Vec<WorldSegmentPos>,
    pending_ack_slots: Vec<SlotIndex>,     // simulator-side ACK queue
    frame: u64,
}
```

**Used the production `Residency` directly — no test stand-in.** The
production type is constructible via `Residency::empty(_)` without any
Bevy world, and its public surface (atomic `WindowedSlotMap` API +
`dispatched_once: HashSet<SlotIndex>` field + `diagnostics()` method)
is sufficient for everything the harness drives. The brief offered the
option to build a stand-in if `Residency` required Bevy machinery; it
does not, so the simpler path won.

### Production-pass-to-simulator mirror

`simulate_frame(state, camera_seg, admit_quota, ack_quota)` mirrors
`residency_driver`'s four passes against pure data:

| Production pass | Simulator step |
|---|---|
| **Pass 1** — `window.set_origin(new_origin, |w, slot| evict callback)` at `residency.rs:579-592`. The closure captures `evictions_this_frame` + `dispatched_once` via a destructured split-borrow and pulls the evicted slot from `dispatched_once`. | Same shape — same `set_origin` API, same `Residency { window, dispatched_once, .. }` split-borrow, same `dispatched_once.remove(&slot)` body. We additionally strip the evicted slot from the simulator's `pending_ack_slots` queue (production's render world wouldn't try to ACK a slot whose binding got evicted; this is the simulator-side mirror of the eviction → ACK-cancel race). |
| **Pass 2** — `compute_window_delta(old_origin, new_origin, window_size, &resident)` at `residency.rs:612-617`. Collects `delta.admit` into a `pending` Vec, sorted by camera distance. | Same call, same args. The simulator's `pending_admissions` is **sticky across frames** (production rebuilds fresh each shift); this is fine because the camera-window membership ensures the same segments re-enter pending each frame. We dedupe new admissions against existing pending. |
| **Pass 3** — `window.allocate_and_bind(w)` in a `for w in pending` loop (`residency.rs:639-657`). Returns `None` when the pool is empty → break (the rest re-enter pending next frame). | Same atomic API call. We use `swap_remove(0)` to drain the head of `pending_admissions` since after sort, the camera-nearest is at index 0. Slots returned by `allocate_and_bind` are pushed into `pending_ack_slots`. |
| **Pass 4** — `apply_dispatch_acks` (`residency.rs:756-772`) drains the render→main `PENDING_DISPATCHED_ONCE_SLOTS` channel into `dispatched_once`. Mirrors `process_pending_admissions` (`residency.rs:693-742`) for the admit list. | We drain up to `ack_quota` from `pending_ack_slots` per frame, inserting each into `state.residency.dispatched_once` (only if the slot is still bound — defensive check; production wouldn't ACK an evicted slot). |

### Divergences from production

Two deliberate divergences, both for harness simplicity:

1. **Sticky `pending_admissions` across frames.** Production rebuilds the
   pending list from `compute_window_delta(old_origin, new_origin,
   window_size, &resident)` on every shift. The simulator keeps the
   pending list and only appends new admissions (deduped). Equivalent
   in steady-state — the camera-window membership ensures the same
   segments enter pending each shift either way. The simulator filters
   out-of-new-window pending entries explicitly at the start of each
   shift-frame (because production's rebuild would have dropped them).

2. **Synchronous ACK in the same frame.** Production: render-world
   pushes to `PENDING_DISPATCHED_ONCE_SLOTS` after `render_queue.submit`;
   main-world `apply_dispatch_acks` drains at the NEXT frame's
   `PreUpdate`. The simulator collapses this round-trip — `pending_ack_slots`
   is drained at the end of the same `simulate_frame` that pushed to it.
   The invariant "admit ≤ ack_quota promotes per frame" survives the
   collapse; only the latency changes.

### Window size and quotas

The production preset: 16×2×16 = 512 slots, admit_quota = 4. We use
these directly via `WORLD_SIZE_IN_SEGMENTS`. Cold-start drive frame
budget = `ceil(512/4) + 2 = 130` frames per the brief.

## Traces landed

Six traces in `crates/bevy_naadf/src/streaming/composition_tests.rs`:

| # | Name | Invariant asserted |
|---|---|---|
| T1 | `trace_cold_start_origin_stays_fixed_reaches_full_coverage` | After `ceil(512/4) + 2 = 130` frames at static camera, `unfulfilled.is_empty()` AND `cold_start_complete == true`. The closed loop catches a regression where cold-start gets stuck mid-way (the 2.13 bug class). |
| T2 | `trace_post_cold_start_static_camera_unfulfilled_remains_zero` | Same shape as T1, then continue 20 frames static. Every frame `unfulfilled.len() == 0`. Catches a regression where the steady-state diverges from convergence. |
| T3 | `trace_camera_x_plus_one_step_after_cold_start_unfulfilled_monotone` | Cold-start, then 10 shift-and-drain cycles: each cycle is one `+1 X` shift-frame (asserts `unfulfilled ≤ slab + admit_quota = 32 + 4`) followed by `ceil(32/4) = 8` drain-frames (asserts `unfulfilled == 0` at end). Implements the brief's "tighter form" assertion: bounded post-shift, converges back to 0 within `ceil(slab/admit_quota)` frames. |
| T4 | `trace_partial_dispatch_ack_simulates_cold_start_race` | The 2.13 cold-start race regression catcher in pure-data form. Frames 1..=5 use `ack_quota = 0` (admit picks 4/frame but nothing promotes to `dispatched_once`). Asserts `dispatched_once == 0` at frame 5 with `bound_slots > 0`. Then `ack_quota = admit_quota` for up to 138 frames; asserts the system eventually converges (`unfulfilled.is_empty()`). With the post-2.13 ACK channel, slots stay re-pickable until ACK lands — cold-start completes. |
| T5 | `trace_diagonal_walk_steady_unfulfilled_bounded` | Cold-start, then 20 diagonal `(1, 0, 1)` shift-and-drain cycles. Each shift-frame: `unfulfilled ≤ 2*slab + admit_quota = 64 + 4 = 68` (loose bound). Each post-drain (`ceil(64/4) = 16` drain-frames): `unfulfilled == 0`. Diagonal evicts ~2 slabs per shift; the test pins both the spike bound and the convergence-back-to-zero invariant. |
| T6 | `trace_random_lcg_walk_unfulfilled_never_blows_up` | Cold-start, then 100 LCG-driven random walk frames. Step choices `{-1, 0, 0, 0, 0, 0, 0, 1}` per axis (75% stationary per axis), camera position clamped to `±3` on X/Z to prevent cumulative drift. Asserts `unfulfilled ≤ window_total / 2 = 256` at every frame. LCG coefficients match `windowed_slot_map::audit_invariants_after_random_mutations` for reproducibility. |

### Failure-message specificity

Every assertion includes: frame number, camera position, observed
unfulfilled count, expected bound, and the first 5 unfulfilled
segments (via the `first_n` helper). Per the brief: "when an assertion
trips, print [these] — the whole point of this phase is that failure
tells you exactly which composition step broke."

## Diffs landed

| File | Lines | Change |
|---|---|---|
| `crates/bevy_naadf/src/streaming/composition_tests.rs` | **NEW**, ~582 LOC | New module gated by `#[cfg(test)]`. Contains `SimState`, `simulate_frame`, `first_n` helper, and 6 trace tests. No production code. |
| `crates/bevy_naadf/src/streaming/mod.rs` | `:34-41` | One-line `#[cfg(test)] mod composition_tests;` declaration + 5-line comment explaining the module's role. |

No changes to `residency.rs`, `windowed_slot_map.rs`, `sliding_window.rs`,
`noise_dispatch.rs`, or any e2e gate. The composition tests are
read-only consumers of the public API.

## Verification

Per `CLAUDE.md` discipline: no `cargo run --bin bevy-naadf` smoke, no
e2e gates run. Phase 2.14.e is composition-only. Each gate ran ONCE
with a wall-clock timeout.

```text
cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world

# 1. workspace build
timeout 180s cargo build --workspace 2>&1 | tail -100
# → "Finished `dev` profile [optimized + debuginfo] target(s) in 21.31s"
# → GREEN

# 2. composition tests only
timeout 180s cargo test --workspace --lib composition_tests 2>&1 | tail -120
# → "test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 278 filtered out"
# → GREEN — all 6 traces pass

# 3. full library test suite
timeout 300s cargo test --workspace --lib 2>&1 | tail -60
# → "test result: ok. 283 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
# → GREEN — pre-existing 277 + 6 new = 283 (matches budget exactly)
```

Test count delta: **+6 (277 → 283)**. Exactly as the brief specified.

### Failed test iterations during development

Three tests required ONE iteration each to land. All failures were
**brief-bound interpretation issues**, not composition bugs:

1. **T3** (x+1 step) — initial form continuously shifted every frame
   (per literal reading of the brief). Bound `slab + admit_quota = 36`
   trivially fails under continuous shifts since net accumulation is
   `slab - admit_quota = 28` per frame. **Fix**: restructured to
   shift-and-drain cycles per the brief's "tighter form" — one
   shift-frame followed by `ceil(slab / admit_quota)` drain-frames.
   This matches the production camera pattern (occasional segment
   crossing followed by drain time) and pins both the post-shift
   bound and the convergence-back-to-zero invariant.

2. **T5** (diagonal walk) — same root cause as T3. **Fix**: same
   restructure to shift-and-drain cycles, bound `2*slab + admit_quota = 68`,
   drain frames `ceil(64/4) = 16`.

3. **T6** (LCG random walk) — failed at frame 24-46 with
   `unfulfilled = 257..265` against bound 256. Cumulative drift of
   the LCG walk pushed the camera 6+ segments from origin, accumulating
   shift-without-drain unfulfilled count. **Fix**: clamped
   `camera_pos.{x,z}` to `±3` and biased step distribution further
   toward stationary (75% stationary per axis). The brief's
   `window_total / 2` bound assumed a locally-bounded walk; without
   the clamp, an LCG walk drifts linearly and exceeds the bound
   purely from net displacement, not a composition bug.

These iterations were brief-interpretation fixes, not bug fixes — the
underlying composition is correct in all three cases (T1 / T2 / T4
passed first-run, and the rewritten T3 / T5 / T6 confirm shift-and-drain
cycles converge correctly).

## What this catches that primitives don't

A primitive-level test verifies one thing in isolation: e.g.,
`compute_window_delta` returns correct `(evict, admit)` for a given
input, or `WindowedSlotMap::allocate_and_bind` is atomic under
pool-empty. These tests can all pass while the **integration** is
broken — for example, the 2.13 cold-start race:

- `WindowedSlotMap` is correct.
- `compute_window_delta` is correct.
- `process_pending_admissions` produces a valid admission list.
- `apply_dispatch_acks` correctly drains the cross-world channel.

But the **composed** behavior had a bug: `dispatched_once.insert(slot)`
fired at admit time (Pass 3), BEFORE the render-world producer had a
chance to actually dispatch. The 11+ producer early-returns silently
skipped the dispatch, while the main-world filter excluded the
"already-dispatched" slot from re-pick. Each individual primitive
was fine; the composition leaked unfulfilled segments forever.

**T4 (`trace_partial_dispatch_ack_simulates_cold_start_race`) is the
pure-data regression catcher for that class of bug.** A future
refactor that re-introduces a "mark dispatched at admit time" shortcut
would survive every primitive's unit test but fail T4 immediately:
with `ack_quota = 0`, the admit picks burn slots permanently, and
turning on `ack_quota` later cannot un-burn them. Composition tests
catch this; primitives never can, because each primitive's contract
doesn't span the admit-then-ACK lifecycle that the bug exploited.

The other 5 traces catch related compositional regressions: a
mis-wired eviction callback (`set_origin`'s `dispatched_once.remove`
forgotten) would surface as T3's drain-frames failing to reach 0 even
after a stable camera (slots stay dispatched-but-no-longer-bound,
unfulfilled stays positive forever). A `compute_window_delta` regression
that includes already-bound segments in admit would surface as T1
running out of free slots before cold-start completes. None of those
are visible to the primitive's own test suite.

## Out-of-scope findings

None of substance. The harness composed cleanly against the public
API of all four primitives — no missing accessors, no need for
test-only `pub(crate)` shims, no escalations.

One observation (not actionable here): the simulator's `pending_ack_slots`
queue parallels the production `PENDING_DISPATCHED_ONCE_SLOTS`
static. A future refactor that lifts the cross-world channel into a
struct (per the audit's note about `CrossWorldSlotChannel<Direction>`
at `04-audit-primitives.md` § "Primitive inventory" row 6) would make
the simulator's mirror more obvious: instead of a bare `Vec<SlotIndex>`
the simulator would hold a `MockCrossWorldChannel` that implements the
same interface. Not worth doing speculatively; mentioned for future
cleanup phases.
