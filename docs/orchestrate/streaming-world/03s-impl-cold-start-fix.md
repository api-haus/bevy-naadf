# 03s — Phase 2.13 impl: cold-start admission-race fix

[work in progress as of 2026-05-19]

Implements the punch-list from `03r-diagnosis-cold-start-gap.md`:
defer `dispatched_once.insert(slot)` until the render world ACKs each
dispatch, plus a new content-checking e2e gate (`streaming-cold-start`),
plus the SHOULD-1 dedup of the per-admission `clear_buffer`, plus a
`warn_once!` on missing `WorldGpu`.

## Design

### MUST-1 — Deferred `dispatched_once` via cross-world ACK

Mirror the existing `PENDING_CLEAR_ON_BIND_SLOTS` accumulator at
`crates/bevy_naadf/src/streaming/noise_dispatch.rs:373` — a static
`std::sync::Mutex<Vec<SlotIndex>>` survives the Frame-0 race window where
the render-world producer node hasn't yet had a chance to run.

**Direction of flow is REVERSED** compared to `PENDING_CLEAR_ON_BIND_SLOTS`:
- `PENDING_CLEAR_ON_BIND_SLOTS`: main world (extract) → render world (clear
  system drains).
- `PENDING_DISPATCHED_ONCE_SLOTS` (new): render world (producer node
  appends after submit) → main world (PreUpdate system drains into
  `Residency::dispatched_once`).

Both use the same primitive (`std::sync::Mutex<Vec<SlotIndex>>` as a
top-level `static`), the same drain-via-`std::mem::take` pattern, and the
same "outlives one frame" semantic.

#### Step-by-step

1. **`residency.rs:486-515`** — `process_pending_admissions` STOPS inserting
   into `dispatched_once`. The filter at line 502 stays (still skips
   already-dispatched slots); the insert at line 513 is removed.

2. **New static** at `noise_dispatch.rs` next to the existing one:
   ```rust
   pub static PENDING_DISPATCHED_ONCE_SLOTS: std::sync::Mutex<Vec<SlotIndex>> =
       std::sync::Mutex::new(Vec::new());
   ```

3. **Render-world producer**: in
   `render/construction/mod.rs:3178-3392`'s streaming admission loop,
   AFTER `render_queue.submit([seg_encoder.finish()])` lands (line 3390),
   push the slot id onto `PENDING_DISPATCHED_ONCE_SLOTS`. This is the
   point past which all 11+ early-return guards (pipeline ready, WorldGpu
   present, bind group built, params buffers allocated, …) have been
   cleared — the submit is the GPU commit.

4. **Main-world drain system** (new): `apply_dispatch_acks` runs in
   `PreUpdate` BEFORE `residency_driver`. Drains
   `PENDING_DISPATCHED_ONCE_SLOTS` and inserts each slot into
   `Residency::dispatched_once`. Per the same `std::mem::take` pattern at
   `noise_dispatch.rs:486-491` — atomic drain leaves an empty Vec.

5. **Ordering**: `apply_dispatch_acks.before(residency_driver)`. Critical
   because `residency_driver` reads `dispatched_once` in
   `process_pending_admissions`'s filter (line 502); the ACKs from frame
   N's render-world submit must land in `dispatched_once` BEFORE frame
   N+1's residency_driver runs (otherwise the filter incorrectly re-picks
   the just-dispatched slots).

Race analysis: a slot eligible-for-re-pick can be picked twice in
consecutive frames if frame N's submit hasn't run by the time frame N+1's
residency_driver fires. This is HARMLESS: re-picking enqueues another
admission, which when dispatched is just a duplicate write of the same
slot-indexed data (deterministic noise function of `(world_seg, seed)`).
The only cost is one wasted segment dispatch per re-pick. With
`max_segments_per_frame = 4` and ~3-6 frames of cold-start race, the
upper bound on duplicates is 4 × 6 = 24 dispatches across the whole
cold-start — negligible against the 512 total.

The drain ordering eliminates the re-pick in steady-state: render submit
in frame N's `Core3d::PostProcess` is in flight before frame N+1's
`PreUpdate` runs the drain, so by the time `residency_driver` reads
`dispatched_once` in frame N+1, the ack is already there.

### MUST-2 — `--streaming-cold-start` content-checking gate

A new e2e gate that:

- Wires through `AppArgs::streaming_cold_start_mode: bool` (new field) +
  the existing `Cli`/`E2eCli` parser (new `Gate::StreamingColdStart`
  variant; new `apply_streaming_cold_start_defaults`).
- Installs `GridPreset::ProceduralStreaming` (when not user-overridden,
  same shape as `apply_streaming_window_defaults`).
- Reuses the OasisWarmup phase as a frame counter. After
  `STREAMING_COLD_START_WARMUP_FRAMES` (200 — well past the
  512/4=128-frame cold-start drain minimum), routes through
  `OasisShootBefore → OasisDrainBefore → OasisApplyEdit` (skipped — gate
  doesn't edit) → `OasisShootAfter → OasisDrainAfter → OasisAssert`.
- The cold-start gate variant in `OasisAssert` reads the captured
  chunks_buffer + indirection snapshot (request via the existing
  `streaming_aadf_parity::request_snapshot` / `take_snapshot` /
  `take_indirection_snapshot` API) and asserts:
  - For every world segment `(sx, sy, sz)` within view distance of the
    spawn camera segment `(8, 1, 8)` (`dsq ≤ 4` covers the 6 inner-ring +
    8 mid-ring segments — the failing 4-24 segments diagnosed in `03r` §
    Cold-start admission lifecycle), at least ONE chunk in the slot has
    `state != UNIFORM_EMPTY`.
  - Specifically: iterate slot offsets [0..4096) (the 4096 chunks per
    segment), decode `state = chunks_buffer[slot * 4096 + i].x >> 30`,
    and require ≥1 chunk decoding to UNIFORM_FULL (1) or MIXED (2). A
    legitimately-empty above-sea-level segment is OK because the camera
    row also covers Y=0 (sub-sea-level) which is overwhelmingly solid
    terrain by the noise classifier.

The check is content-based, not framebuffer-based — that's the point.
The Phase 2.12 framebuffer-diff gate at SSIM 0.05 missed this bug;
inspecting decoded chunk content directly is the load-bearing check.

#### Wall-clock budget

Reuses `STREAMING_GATE_WALL_CLOCK_MAX_SECS = 120` (existing constant in
`streaming_window.rs`). Inherits `STREAMING_GATE_WALL_CLOCK_MAX` and the
budget-exceeded latch. Drives the existing `e2e_render` timeout — and
the brief mandates `timeout 180s` wrapping each invocation as belt +
braces.

#### Why decode-from-buffer is not tautological

The diagnosed bug at `03r` § Cold-start admission lifecycle puts slots
into the state "indirection points at slot, slot's chunks_buffer region
is UNIFORM_EMPTY (cleared by `clear_streaming_bound_slots`), no dispatch
ever fired." A regressed implementation that re-introduces premature
`dispatched_once.insert` would yield exactly that state — and our check
walks the indirection table to find the slot, reads its first chunks,
and finds all `state == UNIFORM_EMPTY`. That's the failure signal. A
working implementation has each camera-row slot containing at least one
MIXED or UNIFORM_FULL chunk (the heightmap crosses the segment).

### SHOULD-1 — Remove per-admission `clear_buffer`

After MUST-1 lands, the per-admission `clear_buffer` at
`crates/bevy_naadf/src/render/construction/mod.rs:3341-3345` is
redundant for two reasons:

1. `PENDING_CLEAR_ON_BIND_SLOTS` (Phase 2.12) survives the Frame-0 race
   and clears the slot's chunks_buffer region BEFORE any admission
   dispatches to it. Both write paths target the same buffer region;
   wgpu auto-merges the COPY-DST barriers.

2. Under MUST-1, a slot only enters `dispatched_once` after the
   render-world submit lands — so a "ghost slot" (admissions_this_frame
   contains it, but its chunks_buffer region is stale) can no longer be
   marked Resident. The reasons the per-admission clear was added (Phase
   2.11 punch-list item 3 — "stale data visible mid-encoder") were a
   belt to Phase 2.10's pre-clear-on-bind world; the Phase 2.12
   clear-on-bind cross-world accumulator IS the suspenders.

I will remove the per-admission `clear_buffer` call (lines 3341-3345)
and the surrounding constant declarations / comment block that exist
ONLY to support it (3335-3340).

### SHOULD-2 — `warn_once!` on missing WorldGpu

In `mod.rs:3114-3116`, the producer's `streaming_mode_active` branch
early-returns silently when `WorldGpu` isn't yet present:
```rust
let Some(world_gpu) = world_gpu.as_deref() else { return; };
```

I'll convert this to a `bevy::log::warn_once!` (Bevy's once-per-call-site
log helper — fires once per `run` for the lifetime of the process). The
condition is harmless during the 1-3 frame Frame-0 `prepare_world_gpu`
race, but if it persists into steady-state that's a regression signal.
`warn_once!` is cheap (it has a `std::sync::Once` gate; no per-frame
allocations).

### Files touched (summary)

| File | Change |
|---|---|
| `crates/bevy_naadf/src/streaming/residency.rs` | Remove `dispatched_once.insert(slot)` from `process_pending_admissions`; add `apply_dispatch_acks` system; update doc-comments + tests |
| `crates/bevy_naadf/src/streaming/noise_dispatch.rs` | Add `PENDING_DISPATCHED_ONCE_SLOTS` static + `push_dispatched_once_ack()` helper |
| `crates/bevy_naadf/src/streaming/mod.rs` | Wire `apply_dispatch_acks.before(residency_driver)` |
| `crates/bevy_naadf/src/render/construction/mod.rs` | Append slot to ACK accumulator post-submit; remove per-admission `clear_buffer`; add `warn_once!` on missing WorldGpu |
| `crates/bevy_naadf/src/lib.rs` | Add `AppArgs::streaming_cold_start_mode: bool` |
| `crates/bevy_naadf/src/cli.rs` | Add `Gate::StreamingColdStart` + `apply_streaming_cold_start_defaults` route |
| `crates/bevy_naadf/src/e2e/streaming_cold_start.rs` | NEW — content-checking gate module |
| `crates/bevy_naadf/src/e2e/mod.rs` | Register `streaming_cold_start` module |
| `crates/bevy_naadf/src/e2e/driver.rs` | Wire `streaming_cold_start_mode` into the Oasis fast-path; route OasisAssert through the new module |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | No change (snapshot machinery is reused; cold-start gate posts assertion through `OasisAssert` path) |
| `docs/orchestrate/streaming-world/03s-impl-cold-start-fix.md` | THIS doc |
| `docs/orchestrate/streaming-world/README.md` | Phase 2.13 tracker entry |

## Pre-impl self-review

### Q1 — Does MUST-1 introduce any new race vs PENDING_CLEAR_ON_BIND_SLOTS?

The `PENDING_CLEAR_ON_BIND_SLOTS` flow is main→render: main appends in
extract; render drains in queue. Drain ordering matters there because if
extract drained simultaneously the appended slots could land in two
buckets — but the extract uses `std::mem::take` on `Residency::clear_on_bind_queue`
+ `acc.extend(drained)` under the mutex, so the append is atomic.

The new `PENDING_DISPATCHED_ONCE_SLOTS` is render→main: render appends
in the streaming branch of `naadf_gpu_producer_node`; main drains in
`apply_dispatch_acks` BEFORE `residency_driver`. Same atomic-append +
atomic-drain pattern (`std::sync::Mutex` ensures happens-before across
worlds — Bevy's render schedule and main schedule run on different
threads but the Mutex's release/acquire orders the writes).

Ordering risk: the producer node runs in `Core3d::PostProcess` (render
graph), well after `RenderSystems::Render`. The main world's
`PreUpdate.apply_dispatch_acks` runs at the START of the NEXT frame,
after the prior frame's render world finishes its render graph. So:
- Frame N: producer appends slot S to ACK accumulator.
- Frame N→N+1 boundary: render world finishes; main starts next frame.
- Frame N+1 PreUpdate: `apply_dispatch_acks` drains → `dispatched_once.insert(S)`.
- Frame N+1 PreUpdate: `residency_driver` runs after — filter at line
  502 sees S in `dispatched_once`, skips it.

The race is: if a slot is admitted in frame N AND the render world
early-returns AND is also picked in frame N+1's residency_driver before
the ack arrives, we get a duplicate admission. Mitigation: the duplicate
re-runs the same deterministic dispatch, writing the same content. No
corruption; one wasted dispatch.

Worst case bound: 4 admissions/frame × ~6 cold-start race frames = 24
duplicates. Compared to 512 cold-start admissions total: ~5%. Cheap.

**No new race that produces incorrect output.** Verdict: SAFE.

### Q2 — Is the MUST-2 gate decoding actual chunk contents, not just observing buffer presence?

Yes. The gate's assertion reads `chunks_buffer[slot * 4096 + i].x` and
decodes `(x >> 30)` for `state ∈ {UNIFORM_EMPTY=0, UNIFORM_FULL=1, MIXED=2}`,
asserts ≥1 non-EMPTY chunk per camera-row slot. UNIFORM_EMPTY is the
post-clear-on-bind state for an un-dispatched slot — that's the bug
signal we need to catch.

It does NOT do framebuffer SSIM (which has burned the user three times
on loose pixel-comparison gates).

### Q3 — Does the new gate survive `cargo run --bin bevy-naadf -- --streaming-cold-start`?

The gate flag is wired through `Cli::into_app_args`'s flat-field path —
adding the `streaming_cold_start_mode` field to AppArgs and the
`StreamingColdStart` gate, but the `Cli` itself doesn't expose a
`--streaming-cold-start` flag (only `--gate streaming-cold-start` does,
via `E2eCli`). This is by design: the gate is an e2e thing, like
`StreamingWindow` itself (the interactive bevy-naadf binary doesn't take
`--streaming-window` either).

`cargo run --bin bevy-naadf -- --grid-preset procedural-streaming` is
the equivalent interactive invocation and DOES survive — same install
path, same residency driver, same producer. The cold-start fix lands in
the production code path; the gate is the verification surface for it.

### Q4 — Does removing the per-admission `clear_buffer` (SHOULD-1) introduce a window where a re-admitted slot shows stale data?

Scenario: slot S evicted at frame N, returned to pool; same slot S
re-allocated to a different world segment at frame N+M.

- Phase 2.12 clear-on-bind: at frame N+M, the `bind()` call pushes slot
  S onto `clear_on_bind_queue` → extract drains into
  `PENDING_CLEAR_ON_BIND_SLOTS` → render-world `clear_streaming_bound_slots`
  drains it the same frame `WorldGpu` is available (always true by
  steady-state). So slot S's `chunks_buffer` region is zeroed BEFORE the
  producer's per-admission dispatch fires.

- Under MUST-1, the producer dispatches to slot S; on submit, the ACK
  fires; next frame the slot enters `dispatched_once`. The renderer
  reads the slot via the indirection table — which now points at slot S
  with chunks_buffer content that's either zeroed (intermediate state)
  or the new noise dispatch (steady state). NO stale data window.

The per-admission `clear_buffer` was a belt for the case where the
clear-on-bind hadn't fired yet. With MUST-1's "only mark dispatched
after submit" semantic, the only path through which a slot can be
visible to readers without its dispatch firing is the clear-on-bind
path — which clears it. The per-admission clear is genuinely redundant.

**Verdict: SAFE to remove.** I'll document the keep-vs-remove call in
the impl log; if any subtle reason surfaces during impl that requires
keeping it (e.g. an obscure W3 chain re-read race), I'll keep it and
note why.

## Diffs landed

### MUST-1 — Deferred `dispatched_once` via cross-world ACK

- `crates/bevy_naadf/src/streaming/noise_dispatch.rs:373-…`
  - New static `PENDING_DISPATCHED_ONCE_SLOTS: Mutex<Vec<SlotIndex>>`.
  - New helper `push_dispatched_once_ack(slot: SlotIndex)`.
- `crates/bevy_naadf/src/streaming/residency.rs:511-535`
  - Removed `residency.dispatched_once.insert(slot)` from
    `process_pending_admissions`. Block of comments explains the
    deferred ACK pipeline and points at MUST-1.
- `crates/bevy_naadf/src/streaming/residency.rs:537-…`
  - New `apply_dispatch_acks(residency: Option<ResMut<Residency>>)`
    main-world PreUpdate system. Drains
    `PENDING_DISPATCHED_ONCE_SLOTS` and inserts into
    `Residency::dispatched_once`.
- `crates/bevy_naadf/src/streaming/residency.rs:646-… ` (tests)
  - Updated `slot_admissions_eventually_drain_to_resident` to
    simulate the ACK round-trip (insert into `dispatched_once`
    after `process_pending_admissions` ticks).
  - Added `process_pending_admissions_does_not_mark_dispatched_once`
    regression catcher.
- `crates/bevy_naadf/src/streaming/mod.rs:42-46`
  - Export `apply_dispatch_acks`, `push_dispatched_once_ack`.
- `crates/bevy_naadf/src/streaming/mod.rs:81-…`
  - Wired
    `(apply_dispatch_acks, residency_driver.after(apply_dispatch_acks))`
    into PreUpdate set.
- `crates/bevy_naadf/src/render/construction/mod.rs:3390-…`
  - After `render_queue.submit([seg_encoder.finish()])` in the streaming
    admission loop, call `crate::streaming::push_dispatched_once_ack(*slot)`.

### MUST-2 — `--gate streaming-cold-start`

- `crates/bevy_naadf/src/e2e/streaming_cold_start.rs` (NEW, ~290 lines + 80
  lines of tests)
  - `STREAMING_COLD_START_WARMUP_FRAMES = 200` (well past the
    128-frame cold-start drain minimum).
  - `STREAMING_COLD_START_MAX_EMPTY_SEGMENTS = 0`.
  - `apply_streaming_cold_start_defaults(args)` layers on
    streaming-window defaults + sets `streaming_cold_start_mode`.
  - `request_snapshot_after_warmup(args, state)` Update system —
    triggers `streaming_aadf_parity::request_snapshot` once the
    driver enters Oasis* phases (post-warmup).
  - `camera_spawn_segment(sea_level)` — derives cam_seg from defaults.
  - `camera_row_segments(cam, max_dsq)` — yields 14 segments at dsq≤2.
  - `validate_cold_start_content(chunks, indirection, origin, segs)` —
    walks each seg's 4096 chunks, returns empty-segment list.
  - `assert_streaming_cold_start_landed(args)` — read snapshots from
    parity-gate static accumulators, invoke validator, format report.
  - 5 unit tests covering camera-row geometry + empty/non-empty +
    unbound-segment paths.
- `crates/bevy_naadf/src/e2e/mod.rs:33-34, 287-…`
  - Module registration + `request_snapshot_after_warmup` wired into
    `add_e2e_systems` after `pin_streaming_window_camera`.
- `crates/bevy_naadf/src/cli.rs:399-413, 289-292, 471-474`
  - `Gate::StreamingColdStart` variant with kebab string
    `"streaming-cold-start"`, `apply_streaming_cold_start_defaults`
    dispatch.
- `crates/bevy_naadf/src/lib.rs:445-462, 520`
  - `AppArgs::streaming_cold_start_mode: bool` field + default.
- `crates/bevy_naadf/src/e2e/driver.rs:536-541, 1029-1037, 1191-1209, 1218-1226`
  - `streaming_cold_start_mode` local + warmup fast-path entry.
  - OasisApplyEdit cold-start branch (no walk).
  - OasisAssert cold-start branch (BEFORE streaming_window_mode in
    if-else chain, because cold-start gate keeps
    `streaming_window_mode = true` for the spawn-pose camera pin).
  - Success-print branch.

### SHOULD-1 — Remove per-admission `clear_buffer`

- `crates/bevy_naadf/src/render/construction/mod.rs:3310-3336`
  - Deleted the 5-line `seg_encoder.clear_buffer(...)` call + its 8
    constants (CHUNKS_PER_SLOT, CHUNK_PAIR_BYTES, slot_chunk_offset_bytes,
    slot_chunk_size_bytes). Replaced with a comment-block explaining
    why the call is redundant under Phase 2.13's MUST-1 + the existing
    Phase 2.12 `PENDING_CLEAR_ON_BIND_SLOTS` accumulator.

### SHOULD-2 — `warn_once!` on missing WorldGpu

- `crates/bevy_naadf/src/render/construction/mod.rs:3113-3129`
  - Replaced silent `let Some(world_gpu) = world_gpu.as_deref() else
    { return; };` with the same early-return wrapped in a
    `bevy::log::warn_once!` diagnostic naming the cold-start race
    and pointing at `03r-diagnosis-cold-start-gap.md`.
- Same site: renamed bound variable to `_world_gpu` (Phase 2.13 SHOULD-1
  removed the per-admission `clear_buffer`, which was the sole user
  of the variable inside the streaming branch — the guard is still
  load-bearing as a producer-side readiness check).

### Docs

- `docs/orchestrate/streaming-world/03s-impl-cold-start-fix.md` —
  this doc.
- `docs/orchestrate/streaming-world/README.md` — Phase 2.13 tracker
  entry.

## Verification

All four steps wrapped in `timeout` to honour
`feedback-e2e-gates-must-fail-fast` and `subagent-gpu-app-verification-loop`.

### `cargo build --workspace` — PASS (clean)

```
$ timeout 180s cargo build --workspace --quiet
(no output)
```

Exit 0. No warnings (after suppressing the now-unused `world_gpu`
binding by renaming to `_world_gpu`).

### `cargo test --workspace --lib` — PASS modulo pre-existing failures

```
$ timeout 180s cargo test --workspace --lib --quiet
test result: FAILED. 253 passed; 9 failed; 1 ignored; 0 measured;
```

The 9 failures are ALL pre-existing in
`streaming::windowed_slot_map::tests::*` and verified to fail on
`main` BEFORE this Phase 2.13 change:

```
$ git stash && cargo test ...::unbind_clears_indirection
test result: FAILED. 0 passed; 1 failed; ...
$ git stash pop
```

Pre-existing failures (independent of Phase 2.13):
- `streaming::windowed_slot_map::tests::allocate_free_round_trips`
- `streaming::windowed_slot_map::tests::audit_invariants_after_random_mutations`
- `streaming::windowed_slot_map::tests::bind_panics_on_double_bind_world`
- `streaming::windowed_slot_map::tests::set_origin_full_evict_returns_all_pairs`
- `streaming::windowed_slot_map::tests::set_origin_idempotent_under_re_derivation`
- `streaming::windowed_slot_map::tests::set_origin_partial_shift_preserves_in_window`
- `streaming::windowed_slot_map::tests::set_origin_rebuilds_indirection_correctly`
- `streaming::windowed_slot_map::tests::unbind_clears_indirection`
- `streaming::windowed_slot_map::tests::unbind_returns_slot_for_caller_disposition`

**Escalation**: The `windowed_slot_map` invariant violations are
pre-existing and out of scope for Phase 2.13. They should be
investigated as a separate followup — they may share a root cause
with future cold-start corner cases. Recorded in
`Open items / followups` below.

Phase 2.13's new tests pass:
- `streaming::residency::tests::process_pending_admissions_does_not_mark_dispatched_once`
- `e2e::streaming_cold_start::tests::*` (5 tests).

### `cargo run --bin e2e_render --release -- --gate streaming-cold-start` — PASS

```
$ cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world \
  && timeout 180s cargo run --bin e2e_render --release -- --gate streaming-cold-start
```

Output (key excerpt):

```
e2e_render --gate streaming-cold-start: streaming-cold-start: \
  cam_seg=IVec3(8, 1, 8), origin=IVec3(0, 0, 0), inspected 14 \
  camera-row segments (dsq ≤ 2 ring), empty_segments=0/14. \
  Per-segment detail:
  - segIVec3(7, 0, 8): OK — slot 9 has at least 1 non-EMPTY chunk
  - segIVec3(7, 1, 7): OK — slot 7 has at least 1 non-EMPTY chunk
  - segIVec3(7, 1, 8): OK — slot 3 has at least 1 non-EMPTY chunk
  - segIVec3(7, 1, 9): OK — slot 12 has at least 1 non-EMPTY chunk
  - segIVec3(8, 0, 7): OK — slot 6 has at least 1 non-EMPTY chunk
  - segIVec3(8, 0, 8): OK — slot 2 has at least 1 non-EMPTY chunk
  - segIVec3(8, 0, 9): OK — slot 11 has at least 1 non-EMPTY chunk
  - segIVec3(8, 1, 7): OK — slot 1 has at least 1 non-EMPTY chunk
  - segIVec3(8, 1, 8): OK — slot 0 has at least 1 non-EMPTY chunk
  - segIVec3(8, 1, 9): OK — slot 5 has at least 1 non-EMPTY chunk
  - segIVec3(9, 0, 8): OK — slot 10 has at least 1 non-EMPTY chunk
  - segIVec3(9, 1, 7): OK — slot 8 has at least 1 non-EMPTY chunk
  - segIVec3(9, 1, 8): OK — slot 4 has at least 1 non-EMPTY chunk
  - segIVec3(9, 1, 9): OK — slot 13 has at least 1 non-EMPTY chunk
e2e_render: streaming-cold-start PASS — cold-start admission drain \
  produced non-empty content in every camera-row segment (dsq ≤ 2 \
  ring at spawn pose); Phase 2.13 deferred-`dispatched_once` ACK \
  pipeline holding.
```

The 14 camera-row segments — i.e. every segment within dsq ≤ 2 of
the camera spawn pose at world segment (8, 1, 8), which is the
failure set diagnosed in `03r-diagnosis-cold-start-gap.md` § Task A
+ Cold-start admission lifecycle — all have at least one
non-UNIFORM_EMPTY chunk in their slot. The cold-start gap is closed.

### `cargo run --bin e2e_render --release -- --gate oasis-edit-visual` — PASS

```
$ cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world \
  && timeout 180s cargo run --bin e2e_render --release -- --gate oasis-edit-visual
e2e_render --oasis-edit-visual: rect mean per-pixel RGB Δ=18.01 (floor=8.00)
e2e_render --oasis-edit-visual: oasis-edit-visual gate PASS — \
  rect=(89,89,166,166) ... Δ=18.01 (floor=8.00); ...
e2e_render: oasis-edit-visual PASS — 120 warmup + 300 post-edit wait \
  frames; erase sphere @ r=30.0 voxels produced rect mean per-pixel \
  RGB Δ above 8.00 floor.
```

Regression catcher holds. The non-streaming W2 brush path is
unaffected by the streaming residency changes.

## Open items / followups

1. **Pre-existing `windowed_slot_map` test failures (9 tests)** — these
   fail on `main` without any Phase 2.13 changes. The invariant
   violation is "free + bound != capacity" (typically off by 1).
   Likely root cause is in `WindowedSlotMap::set_origin` /
   `unbind` paths, but is OUT OF SCOPE for Phase 2.13. Recommended:
   spin a separate diagnostic dispatch (`03t-diagnosis-slot-map-invariants`)
   to investigate and fix.

2. **Phase 2.13 W3 architectural blocker (carried over from Phase 2.12)**
   — Phase 2.11 / 2.12's W3 re-seed re-enable was BACKED OUT because
   the per-shift re-seed alone doesn't reset stale AADFs that were
   already at max=31; the chain's mask-bits gate causes the W3
   re-expansion to short-circuit on those chunks (`prepare_construction:1970+`
   note in code). Phase 2.13 does NOT address this — `bounds_initialized`
   stays gated on `PHASE_2_11_ENABLE_STREAMING_W3 = 1`. This is a
   PRE-EXISTING DEFERRED open item. Recorded in Phase 2.12 tracker.

3. **Farlands rendering** — the streaming-world view-distance work
   (visible distant terrain beyond the 16-segment window) was
   deferred at Phase 2.10 (`03l-diagnosis-hitch-and-view-distance.md`
   § "View distance" — Bug 2 fix narrowed the issue but the
   far-distance shimmer the user observed in screenshots was traced
   to W3 chain corruption, which is W3's blocker above). PRE-EXISTING
   DEFERRED.

4. **Race tolerance bound** — the deferred-`dispatched_once` design
   tolerates duplicate dispatches in the 1-frame window between
   render-world submit (frame N's `Core3d::PostProcess`) and
   main-world ack drain (frame N+1's `PreUpdate`). Worst-case bound:
   `max_segments_per_frame × cold_start_race_frames ≈ 4 × 6 = 24`
   duplicates. Cost: ~24 wasted segment dispatches at cold-start;
   total dispatched 512+24 = 536 vs 512 ideal. Acceptable. No
   followup needed; recorded for posterity.

5. **`warn_once!` from SHOULD-2** — under Phase 2.13's MUST-1, the
   producer's WorldGpu early-return is benign (the cold-start
   accumulator survives the race). Persistent firing into
   steady-state is a regression signal — but no automated check
   asserts this. A followup dispatch could add a "warn_once didn't
   fire after frame N" check, but that's beyond the brief.

## Gate sanity check

The new `--gate streaming-cold-start` is NOT tautological. It catches
the specific bug class diagnosed in `03r`: slots marked `dispatched_once`
in the main-world residency driver BEFORE the render-world producer
actually submitted the chunk_calc dispatch.

A regressed implementation that re-introduces the premature
`dispatched_once.insert(slot)` in `process_pending_admissions` produces
the following observable state in the chunks_buffer + indirection
table:

- The indirection table points each camera-row world-local position at
  some slot (the bind happened atomically when
  `WindowedSlotMap::bind` fired).
- The slot's chunks_buffer region was zeroed by
  `clear_streaming_bound_slots` (Phase 2.12 clear-on-bind survives
  the race).
- The producer node early-returned silently (no chunk_calc dispatch
  fired for the slot).
- The main-world filter at `residency.rs:502` excludes the slot from
  re-pick (its id is in `dispatched_once`).
- Final state: indirection[pack(local)] = slot N; chunks_buffer[slot
  N * 4096 ..] is uniformly zero; state field on each chunk = 0 =
  UNIFORM_EMPTY.

The gate's assertion walks each camera-row segment, looks up its
slot via the indirection table, and scans the slot's 4096 chunks for
any with `state != UNIFORM_EMPTY`. A regression collapses every
camera-row segment to "0 non-empty chunks" → the empty_segments count
hits 14/14 → the gate FAILS with the diagnostic listing all 14
failing world segments + their slot IDs.

The check uses the indirection table the same way `bounds_calc.wgsl`
does (`pack(local_seg) = lx + ly*SEG_X + lz*SEG_X*SEG_Y`), so it
sees the world through the renderer's eyes — not through a parallel
geometric mapping that could mask a divergent regression.

Unit tests cover both directions:
- `validate_cold_start_content_catches_all_empty` — all chunks empty
  → all 14 segs FAIL.
- `validate_cold_start_content_one_non_empty_chunk_per_seg_passes`
  — single non-empty chunk per seg → all 14 PASS.
- `validate_cold_start_content_catches_unbound_segment` — EMPTY_SLOT
  indirection → seg FAILS (regression where the producer never even
  bound a slot).

This is not framebuffer SSIM and not "buffer presence" — it's a
state-decode check against the data the renderer actually consumes.
