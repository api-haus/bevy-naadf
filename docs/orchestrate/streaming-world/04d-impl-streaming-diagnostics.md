# Phase 2.14.d — impl: `StreamingDiagnostics` analytical surface

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world`.

Builds on Phase 2.14.b ([`04b-impl-wsm-atomic-api.md`](./04b-impl-wsm-atomic-api.md))
and Phase 2.14.c ([`04c-impl-sliding-window-primitive.md`](./04c-impl-sliding-window-primitive.md)).
Per the audit ([`04-audit-primitives.md`](./04-audit-primitives.md) §
"Proposed `StreamingDiagnostics` surface"), this phase adds the analytical
self-reporting surface the user demanded:

> "the case with unfulfilled slots in the middle must be catched analytically
> — the system must know if it HAS unfulfilled slots in the middle at
> startup, not via screenshots"

This phase delivers the **query surface only** — `StreamingDiagnostics`
struct, three methods on `Residency`, two free functions in
`noise_dispatch`, plus 8 unit tests. **No production wiring** (no new log
lines, no startup-time check system, no plugin extension). Wiring is
Phase 2.14.f.

## Design choices

### File placement — chose to keep everything in `residency.rs`

The brief offered two options: keep in `residency.rs` (lowest surface) or
extract into a new `streaming/diagnostics.rs`. Chose to keep in
`residency.rs` for two reasons:

1. **The methods are on `Residency`.** Extracting the struct definition
   while leaving the methods inside `impl Residency` would split the
   concern across two files for no gain. The audit's "lowest surface"
   reading (user pick Q2) wins this trade.
2. **Module already imports the right types.** `WorldSegmentPos`,
   `SlotIndex`, `WindowedSlotMap`, `HashSet` are all already in scope.
   A new module would re-import them and add another `pub mod` line
   without changing any callers.

The `StreamingDiagnostics` struct is `pub use`-d from `streaming::mod.rs`
alongside `Residency` / `SlotIndex` / `WorldSegmentPos`.

### `frame_counter` field — used existing

`Residency::frame_counter` already exists at `residency.rs:82` (tick'd by
`residency_driver` at line 328). The brief offered three options
(add-new-with-system, defer, use-existing). Used existing — no schedule
touch required.

### `in_flight_slots` computation

Computed as `capacity.saturating_sub(free).saturating_sub(bound)`. Under
the Phase 2.14.b atomic API this is structurally 0; the saturating-sub
form guards against a transient inconsistency during a non-PreUpdate
diagnostics read (which can't happen with the current usage, but
diagnostics are best-effort telemetry — a regression that re-introduces
in-flight escape must not crash the diagnostics call).

### `generating_slots` computation

`bound.saturating_sub(dispatched_once)`. The saturating form covers the
same edge case (e.g. a debug reader that snapshots `dispatched_once`
after `bound` shrinks during eviction).

### Unfulfilled-set iteration order

`for lz / for ly / for lx` (X-fastest). Matches the existing convention
used by `super::sliding_window::compute_window_delta` (Phase 2.14.c) and
`WindowedSlotMap::pack`. Test T3 (`diagnostics_partial_bind_some_unfulfilled`)
pins the exact expected set membership but is order-independent (uses
HashSet for membership assertions); the order is observable only
indirectly via the returned `Vec`.

### Cross-world accumulator tests serialization

The two `pending_*_count_reflects_state` tests touch process-global
`std::sync::Mutex<Vec<SlotIndex>>` statics. Cargo runs unit tests on
multiple threads by default. Added a per-test-module
`StdMutex<()>` (`CROSS_WORLD_ACC_TEST_GUARD`) that both tests lock for
the duration of their cross-world reads, plus a `drain_cross_world_accumulators`
helper that clears both globals at start and end of each test. This
keeps the diagnostics tests deterministic without pulling in
`serial_test` as a new dev-dep.

### Deviations from the audit's proposed shape

None of substance. The struct fields match the audit verbatim. The
audit lists `Residency::unfulfilled_camera_window_segments(&self, cam_pose: WorldSegmentPos)`;
per Q3 ("Full 512"), the implementation drops the `cam_pose` argument
— the window position already encodes the camera-near range via
`set_origin`, so a separate camera pose argument is redundant. The
audit's signature was a placeholder; the user's Q3 answer made the
parameterless form correct.

## Public surface added

In `crates/bevy_naadf/src/streaming/residency.rs`:

```rust
pub struct StreamingDiagnostics {
    pub free_slots: u32,
    pub bound_slots: u32,
    pub in_flight_slots: u32,
    pub dispatched_once_slots: u32,
    pub generating_slots: u32,
    pub pending_clear_on_bind: usize,
    pub pending_dispatch_acks: usize,
    pub frame_counter: u64,
    pub cold_start_complete: bool,
    pub camera_window_segments_total: u32,
    pub camera_window_segments_unfulfilled: u32,
    pub unfulfilled_camera_window_segments: Vec<WorldSegmentPos>,
}

impl Residency {
    pub fn slot_counters(&self) -> (u32, u32, u32, u32) { ... }
    pub fn unfulfilled_camera_window_segments(&self) -> Vec<WorldSegmentPos> { ... }
    pub fn diagnostics(&self) -> StreamingDiagnostics { ... }
}
```

In `crates/bevy_naadf/src/streaming/noise_dispatch.rs`:

```rust
pub fn pending_clear_on_bind_count() -> usize { ... }
pub fn pending_dispatch_ack_count() -> usize { ... }
```

Both noise_dispatch helpers return 0 on lock poisoning (best-effort
telemetry must not propagate panics).

In `crates/bevy_naadf/src/streaming/mod.rs`: re-exported
`pending_clear_on_bind_count`, `pending_dispatch_ack_count`,
`StreamingDiagnostics` alongside the existing public surface.

## Diffs landed

| File | Lines | Change |
|---|---|---|
| `crates/bevy_naadf/src/streaming/residency.rs` | `~211–323` (new), `~325–388` (new), `~1050+` (tests) | Added `impl Residency::{slot_counters, unfulfilled_camera_window_segments, diagnostics}`. Added `pub struct StreamingDiagnostics { 12 fields }`. Added 8 unit tests under `mod tests`. No changes to existing methods. |
| `crates/bevy_naadf/src/streaming/noise_dispatch.rs` | `~427+` (post-`push_dispatched_once_ack`) | Added `pub fn pending_clear_on_bind_count()` + `pub fn pending_dispatch_ack_count()`. Lock-poisoning returns 0 (no panic). |
| `crates/bevy_naadf/src/streaming/mod.rs` | `:44–52`, `:53–58` | `pub use noise_dispatch::{pending_clear_on_bind_count, pending_dispatch_ack_count, ...}` + `pub use residency::{StreamingDiagnostics, ...}`. |

No changes to `WindowedSlotMap`, `sliding_window`, `residency_driver`,
`StreamingPlugin`, or any e2e gate. The only callsites that observe the
new surface are the 8 unit tests added in this phase.

## Tests added

| # | Name | Intent |
|---|---|---|
| T1 | `diagnostics_on_empty_residency_reports_all_unfulfilled` | Fresh Residency: `free=cap, bound=0, in_flight=0, dispatched_once=0, cold_start_complete=false, unfulfilled=cap`. Every window segment reports as unfulfilled. |
| T2 | `diagnostics_fully_fulfilled_reports_none_unfulfilled` | All 512 cells bound + dispatched: `cold_start_complete=true, generating=0, unfulfilled=0`. Full window fulfillment. |
| T3 | `diagnostics_partial_bind_some_unfulfilled` | Bind 16 segments, dispatch 5: `bound=16, dispatched=5, generating=11`. Unfulfilled list contains every segment EXCEPT the 5 dispatched. Verifies set membership (HashSet equality). |
| T4 | `diagnostics_after_set_origin_window_segments_reflect_new_origin` | After `set_origin((1,0,0), ...)`: every unfulfilled segment lies within the new window AABB; the evicted (0,0,0) is NOT in the list. |
| T5 | `slot_counters_o1_matches_diagnostics` | `slot_counters() == (d.free_slots, d.bound_slots, d.in_flight_slots, d.dispatched_once_slots)`. Hot-path consistency. |
| T6 | `unfulfilled_camera_window_segments_within_window_only` | Every returned segment is inside `[origin, origin + window_size)`. Sanity invariant. |
| T7 | `pending_clear_on_bind_count_reflects_state` | Drain → 0 → push 3 → 3 → drain → 0. Locked against the serialization guard. |
| T8 | `pending_dispatch_ack_count_reflects_state` | Same shape via `push_dispatched_once_ack` helper. Locked against the serialization guard. |

T7 + T8 share `CROSS_WORLD_ACC_TEST_GUARD: StdMutex<()>` so they
serialize on the cross-world accumulator reads (statics are
process-global, cargo runs unit tests on multiple threads by default).

## Verification

Per `CLAUDE.md` discipline: no `cargo run --bin bevy-naadf` smoke, no e2e
gates run. Phase 2.14.d is primitive-only. Each gate ran ONCE with a
wall-clock timeout.

```text
cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world

# 1. workspace build
timeout 180s cargo build --workspace 2>&1 | tail -50
# → "Finished `dev` profile [optimized + debuginfo] target(s) in 23.85s"
# → GREEN

# 2. diagnostics-filtered tests (matches 5 of 8 — the others use
#    different names like `pending_*` and `slot_counters_*`)
timeout 180s cargo test --workspace --lib diagnostics 2>&1 | tail -40
# → "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 273 filtered out"
# → GREEN

# 2b. residency::tests (catches all 8 new + 13 pre-existing residency tests)
timeout 180s cargo test --workspace --lib streaming::residency 2>&1 | tail -40
# → "test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 257 filtered out"
# → GREEN — 21 = 13 pre-existing + 8 new

# 3. full library test suite
timeout 300s cargo test --workspace --lib 2>&1 | tail -30
# → "test result: ok. 277 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
# → GREEN — pre-existing 269 + 8 new = 277 (matches budget exactly)
```

Test count delta: **+8 (269 → 277)**. Exactly as the brief specified.

## Deferred to 2.14.f

Items from the audit's `## Proposed StreamingDiagnostics surface` that
this phase deliberately did NOT land — per the brief's "Strict
out-of-scope (DO NOT touch)":

- **No new `info!`/`warn!`/`warn_once!` lines.** The existing `info!`
  at `residency.rs:473-481` (logs every shift) is unchanged. The
  audit's wishlist of "extending the info! line with unfulfilled count +
  in-flight + cold-start state" is queued for Phase 2.14.f.
- **No startup-time check system.** The audit proposed a `Last`-stage
  system that logs `Residency::diagnostics(camera_segment_pos())` at
  frames 50 / 200 / 500 (with `warn!` if frame-500 unfulfilled > 0).
  Not added. The diagnostics surface is the building block; the periodic
  logging is Phase 2.14.f's deliverable.
- **No `StreamingPlugin` extension.** `streaming/mod.rs`'s
  `Plugin::build` is unchanged. Adding the periodic-logging system is
  Phase 2.14.f.
- **No e2e gate rewiring.** `streaming-cold-start` still does its
  in-process content scan; it does NOT call `Residency::diagnostics`
  yet. Rewiring is Phase 2.14.f or .g.
- **No composition tests.** Multi-primitive walks (camera-track-driven
  trace asserting `unfulfilled` monotonically decreases) are Phase
  2.14.e.

The `frame_counter` field on `StreamingDiagnostics` reads from the
existing `Residency::frame_counter` field — no new system added, no
schedule touched. Brief noted this option ("If `Residency` already has
a frame counter field, use it"); did.

## Out-of-scope findings

None of substance. One small observation worth flagging for a future
phase but NOT actionable here:

- **`PENDING_CLEAR_ON_BIND_SLOTS` and `PENDING_DISPATCHED_ONCE_SLOTS`
  share the exact same shape** (`pub static M: Mutex<Vec<SlotIndex>>`
  with one direction APPEND from world A + DRAIN by world B). They are
  twin'd statics with twin'd helper APIs. A future
  `CrossWorldSlotChannel<Direction>` primitive could collapse both into
  one generic implementation. The audit noted this at
  `04-audit-primitives.md` § "Primitive inventory" table row
  "Clear-on-bind cross-world accumulator" and judged the ad-hoc shape
  acceptable; this implementation does not alter that judgment. The
  observation is recorded here so a future cleanup pass can pick it up.
