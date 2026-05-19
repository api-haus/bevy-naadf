# Phase 2.14.f — impl: production-binary diagnostics wiring

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world`.

Builds on Phase 2.14.d ([`04d-impl-streaming-diagnostics.md`](./04d-impl-streaming-diagnostics.md))
and Phase 2.14.e ([`04e-impl-composition-tests.md`](./04e-impl-composition-tests.md)).
Per the audit ([`04-audit-primitives.md`](./04-audit-primitives.md) §
"Sequencing recommendation" item 5), this phase wires the analytical
surface from 2.14.d into the production binary so the user's load-bearing
requirement is satisfied:

> "the system must know if it HAS unfulfilled slots in the middle at
> startup, not via screenshots"

Three log channels land:

1. **Extended per-shift `info!`** at `residency.rs:~670` — already
   firing once per origin shift; now carries the analytical fields.
2. **Periodic `Last`-stage `info!`** — heartbeat at debounced cadence
   (every 10 frames pre cold-start, every 300 frames steady-state),
   plus a one-shot "cold-start complete at frame N" transition log.
3. **One-shot `warn!`** at frame 500 — fires once when the streamer
   still reports `unfulfilled > 0` past a 4× cold-start budget margin.

## Wiring design

### System stage choice — `Last`

Picked `Last` for the periodic logger system. Rationale:

- `PreUpdate` hosts the residency driver itself + `apply_dispatch_acks`,
  which mutate `Residency::frame_counter`, `dispatched_once`, and the
  cross-world ACK accumulators. Logging there would race against the
  same-frame mutations.
- `Update` is owned by user-facing tick logic (camera input,
  `track_and_pin_camera`); inserting a diagnostic system there
  pollutes a busy stage.
- `Last` is empty in the streaming layer (Phase 2.6 deleted the
  previous `Last`-stage `finalise_admissions_as_resident` system,
  per `02c-design-windowed-slot-map.md` § G.4 D4 — confirmed at
  `streaming/mod.rs:299-307`'s comment block). It's the natural
  end-of-frame slot for "look at what just happened and report".

The system reads `Res<Residency>` (immutable) + `ResMut<StreamingDiagnosticsLoggerState>`
(small POD resource carrying two `bool` latches).

### Cadence math

Encoded in the pure-data predicate `should_log_at_frame(frame, cold_start_complete) -> bool`:

```rust
let interval = if cold_start_complete {
    STEADY_LOG_INTERVAL_FRAMES   // 300
} else {
    COLD_START_LOG_INTERVAL_FRAMES   // 10
};
frame.is_multiple_of(interval)
```

Predicate is `const`-friendly (uses only `u64` arithmetic) and lives
outside `log_streaming_diagnostics`, so the cadence-math tests
(shape A in the brief — "extract a small pure function") can hit it
directly without an `App`.

### Transition-frame handling

The cold-start completion log fires when `diag.cold_start_complete`
flips from `false` to `true`. State is carried in
`StreamingDiagnosticsLoggerState::cold_start_seen_complete: bool`:

```rust
if diag.cold_start_complete && !state.cold_start_seen_complete {
    state.cold_start_seen_complete = true;
    bevy::log::info!("streaming-world: cold-start complete at frame {} ...", ...);
}
```

The latch never resets — by design. If a future eviction drops a slot
from `dispatched_once` and `cold_start_complete` flips back to false,
we do NOT re-log the transition the second time it completes (that
would be a steady-state shift, not a cold-start). The
per-shift `info!` line at `residency.rs:~670` covers shifts; the
periodic heartbeat covers steady-state.

### Warn-threshold latch

Symmetric design:

```rust
if !state.warn_threshold_fired
    && frame >= COLD_START_WARN_THRESHOLD_FRAMES
    && diag.camera_window_segments_unfulfilled > 0
{
    state.warn_threshold_fired = true;
    bevy::log::warn!("streaming-world: cold-start gap detected at frame {} ...", ...);
}
```

Once fired, the latch stays set for the rest of the session — a
persistent gap surfaces in the warn log exactly once, not every frame.

Threshold sizing: with window 16×2×16 = 512 and admit quota 4, the
ideal cold-start budget is `ceil(512 / 4) = 128` frames + 1 ACK drain
frame = 129. Picked 500 as ~4× the perfect-case budget — comfortably
past any expected legitimate cold-start window, but firing well within
the user's "I can see something is wrong" attention span at 60fps
(~8 seconds).

## SSoT audit

Per the brief, ran the standing-rule grep before adding any constant:

```bash
rg -n "const.*FRAME|const.*COLD|const.*BUDGET|const.*INTERVAL|const.*ADMIT" \
   crates/bevy_naadf/src/streaming/
```

Hits in scope:

| Match | File | Verdict |
|---|---|---|
| `const ADMIT_QUOTA: usize = 4;` | `streaming/composition_tests.rs:230` | **Test-local only** — comment says "mirrors `Residency::max_segments_per_frame` default". This is the composition-tests harness's local mirror of the CLI default at `lib.rs:543` (`max_segments_per_frame: 4`). NOT a candidate for SSoT consolidation with the new logger constants (different semantics — `ADMIT_QUOTA` is the streaming admission rate, not a log-cadence interval). Left untouched. |

Other greps confirmed no pre-existing log-cadence / frame-interval /
warn-threshold constants in the streaming layer. The three constants
in this phase have no SSoT collisions.

Decision: introduce the three new constants inline at the top of
`streaming/mod.rs` (the only file that uses them — they're consumed
exclusively by `should_log_at_frame` + `log_streaming_diagnostics`).

Did NOT create `streaming/constants.rs`. The brief offered this as an
option "if you'd be adding three constants"; the audit shows the three
constants are tightly coupled to one system in one file, and creating
a constants module would split the cohesion. If a future phase grows
this set past ~5 constants, the file extraction becomes worthwhile;
at three, inline beats `pub use constants::*`.

## Diffs landed

| File | Range | Change |
|---|---|---|
| `crates/bevy_naadf/src/streaming/residency.rs` | `:679-697` (was `:663-671`) | Extended the per-shift `info!` line — added `cold_start_complete`, `unfulfilled`, `in_flight`, `dispatched_once` fields. Pre-call to `residency.diagnostics()` captured locally. Severity unchanged (`info!`). |
| `crates/bevy_naadf/src/streaming/mod.rs` | `:71-225` (new) | Added three constants (`COLD_START_LOG_INTERVAL_FRAMES`, `STEADY_LOG_INTERVAL_FRAMES`, `COLD_START_WARN_THRESHOLD_FRAMES`, `WARN_UNFULFILLED_TRUNCATE`), free function `should_log_at_frame`, resource `StreamingDiagnosticsLoggerState`, system `log_streaming_diagnostics`. |
| `crates/bevy_naadf/src/streaming/mod.rs` | `:316-317` | Registered `StreamingDiagnosticsLoggerState` resource + `log_streaming_diagnostics` system in `Last` schedule. |
| `crates/bevy_naadf/src/streaming/mod.rs` | `:349+` (new test module) | `#[cfg(test)] mod diagnostics_logger_tests` — 6 new unit tests. |

No changes to:

- `WindowedSlotMap` (Phase 2.14.b primitive).
- `sliding_window` (Phase 2.14.c primitive).
- `StreamingDiagnostics` struct or its methods (Phase 2.14.d surface).
- `composition_tests` (Phase 2.14.e).
- E2e gate code (`--gate streaming-cold-start` etc).
- Any shader / GPU resource / bind layout.
- Dispatch ACK mechanics (`apply_dispatch_acks`).

## Tests added

| # | Name | Shape | Intent |
|---|---|---|---|
| T1 | `cadence_pre_cold_start_emits_every_10_frames` | A (pure-data predicate) | Pin the 10-frame cadence pre cold-start. Spot-check frames 0-29 (3 intervals). Asserts `COLD_START_LOG_INTERVAL_FRAMES == 10` so a constant change trips the test. |
| T2 | `cadence_post_cold_start_emits_every_300_frames` | A (pure-data predicate) | Pin the 300-frame cadence post cold-start. Spot-check frames 0, 1, 299, 300, 600, 301. Asserts `STEADY_LOG_INTERVAL_FRAMES == 300`. |
| T3 | `cadence_cold_start_state_uses_short_interval` | A (pure-data predicate) | Regression catcher for a bool inversion in `should_log_at_frame`. Frame 10 fires under cold-start state (10 % 10 == 0) but NOT in steady (10 % 300 != 0). |
| T4 | `warn_fires_once_at_threshold_when_unfulfilled_nonzero` | B (`App`-driven) | Build a Bevy `App` with `Residency::empty()` at `frame_counter = 500`. First `update()` must flip `warn_threshold_fired`. Second `update()` must keep it set (one-shot semantics). |
| T5 | `cold_start_transition_latch_flips_on_completion` | B (`App`-driven) | Build a Bevy `App` with a fully-fulfilled `Residency` (every cell bound + dispatched). First `update()` flips `cold_start_seen_complete`. Second `update()` keeps it set. |
| T6 | `logger_early_returns_when_residency_absent` | B (`App`-driven) | Build a Bevy `App` with NO `Residency` resource. `update()` must run without panic and leave both latches clear. Pin the non-streaming preset safety. |

Per the brief, picked shape-A for the cadence-math invariants
(narrowest blast radius, fastest test) and shape-B for the three
state-machine invariants that require `Res<Residency>` plumbing. No
`tracing-test` dev-dep added — the brief warned against it, and the
state-machine assertions on the latch booleans are sufficient (a
content-of-log assertion would add nothing the latch invariants
don't already cover).

## Verification

Per `CLAUDE.md` discipline: no `cargo run --bin bevy-naadf` smoke, no
e2e gates run. Each gate ran ONCE with a wall-clock timeout per the
brief.

```text
cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world

# 1. workspace build
timeout 180s cargo build --workspace 2>&1 | tail -50
# → "Finished `dev` profile [optimized + debuginfo] target(s) in 19.16s"
# → GREEN

# 2. full library test suite
timeout 300s cargo test --workspace --lib 2>&1 | tail -10
# → "test result: ok. 289 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
# → GREEN — pre-existing 283 + 6 new = 289 (matches the budget exactly)

# 2b. diagnostics-logger-specific
timeout 120s cargo test --workspace --lib streaming::diagnostics_logger 2>&1 | tail -15
# → "test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 284 filtered out"
# → GREEN — 6/6 of the newly-added tests
```

Test count delta: **+6 (283 → 289)**. Matches the brief's "at least 3
tests total" requirement with margin (we delivered two more `App`-
driven cases — the cold-start transition latch test and the
non-streaming-preset safety test).

## Log-output example

The user-facing surface this phase delivers. A typical run of
`cargo run --release --bin bevy-naadf -- --procedural-streaming`
produces approximately:

```text
# Frame 0 — first tick. residency_driver fires the initial
# `set_origin` for the camera-centered window. The per-shift `info!`
# line (now extended) emits. The `Last`-stage periodic logger ALSO
# emits on frame 0 because `frame % 10 == 0`.
INFO streaming-world residency shift: cam_seg=IVec3(0, 0, 0), \
     new_origin=IVec3(-8, 0, -8), evictions=0, bound_segments=512, \
     admissions_this_frame=4, cold_start_complete=false, \
     unfulfilled=512, in_flight=0, dispatched_once=0
INFO streaming-world: f=1 | free=0 bound=512 dispatched_once=0 | \
     generating=512 in_flight=0 | cold_start=false unfulfilled=512 | \
     pending_clear=512 pending_acks=0

# Frame 10 — periodic logger fires (10 % 10 == 0). ~36-40 slots
# dispatched by now (4 per frame for 9 frames + 1 frame of grace).
INFO streaming-world: f=10 | free=0 bound=512 dispatched_once=36 | \
     generating=476 in_flight=0 | cold_start=false unfulfilled=476 | \
     pending_clear=0 pending_acks=4

# Frame 100 — periodic logger fires (100 % 10 == 0). At 4/frame
# steady the system has dispatched ~100*4 = 400 slots.
INFO streaming-world: f=100 | free=0 bound=512 dispatched_once=400 | \
     generating=112 in_flight=0 | cold_start=false unfulfilled=112 | \
     pending_clear=0 pending_acks=4

# Frame 128 — cold-start complete (perfect-case budget hits here).
# The transition log fires. The next periodic-logger tick will be
# at frame 130 (still in cold-start cadence because the predicate
# evaluated cold_start_complete=false at frame 120; at frame 130
# the predicate returns 130 % 300 != 0 = false — so the next
# periodic emit is at frame 300).
INFO streaming-world: cold-start complete at frame 129 \
     (unfulfilled=0, bound=512, dispatched_once=512)

# Frame 300 — first post-cold-start periodic heartbeat (300 % 300 == 0).
# Steady state. No camera movement → no shift log between 130 and 300.
INFO streaming-world: f=300 | free=0 bound=512 dispatched_once=512 | \
     generating=0 in_flight=0 | cold_start=true unfulfilled=0 | \
     pending_clear=0 pending_acks=0

# Frame 600 — second post-cold-start heartbeat.
INFO streaming-world: f=600 | free=0 bound=512 dispatched_once=512 | \
     generating=0 in_flight=0 | cold_start=true unfulfilled=0 | \
     pending_clear=0 pending_acks=0
```

If the streamer were stuck (e.g. a regression that re-introduced the
cold-start gap fixed by Phase 2.13), the user would instead see:

```text
WARN streaming-world: cold-start gap detected at frame 500 — \
     4 unfulfilled camera-window segments after \
     COLD_START_WARN_THRESHOLD_FRAMES=500. First 4 segments: \
     [WorldSegmentPos(IVec3(0, 0, 0)), WorldSegmentPos(IVec3(1, 0, 0)), \
      WorldSegmentPos(IVec3(0, 1, 0)), WorldSegmentPos(IVec3(1, 1, 0))]
```

— the analytical version of "I see sky-coloured holes near the camera
at startup", emitted exactly once even if the gap persists.

## Out-of-scope findings

None of substance. One small observation worth flagging for a future
phase but NOT actionable here:

- **The periodic logger's frame counter source.** Currently reads
  `diag.frame_counter`, which is populated from `Residency::frame_counter`.
  That counter is incremented inside `residency_driver` at
  `residency.rs:518`. Edge case: if `residency_driver` early-returns
  (e.g. no camera yet on frame 0 of a non-e2e preset before the
  camera spawns), `frame_counter` does NOT tick. The periodic logger
  would then re-log frame 0 every tick until the camera lands. This
  is harmless (the warn-latch + cold-start-complete-latch prevent
  log floods for the one-shot events) but could in theory spam the
  periodic heartbeat at frame 0 for several ticks. Not worth fixing
  here — the production camera path lands within 1-2 ticks at
  startup; if a future regression makes the camera lazy, the
  duplicated `f=0` log lines are themselves a useful signal.
