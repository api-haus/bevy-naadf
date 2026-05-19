# Phase 2.14.g — E2E gate regression sweep

Regression catcher for the Phase 2.14 bottom-up refactor (WSM atomic API
collapse in 2.14.b + `compute_window_delta` extraction in 2.14.c +
`StreamingDiagnostics` analytical surface in 2.14.d + composition tests in
2.14.e + production log wiring in 2.14.f).

No code changes. No new tests. Three existing e2e gates invoked once each,
in strict pass-or-stop order.

## Invocation

Working directory:
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world`

Each gate wrapped in `timeout 180s`. Order:
1. `--gate streaming-cold-start` (Phase 2.13 cold-start content gate)
2. `--gate streaming-window` (legacy Phase 2 procedural-streaming gate)
3. `--gate oasis-edit-visual` (Phase 03f cross-feature visual gate)

Stop-on-first-failure protocol per agent brief.

## Gate results

| Gate | Status | Wall-clock | Last assertion (one line) |
|---|---|---|---|
| `streaming-cold-start` | **PASS** | ~17 s | `streaming-cold-start gate PASS — cold-start admission drain produced non-empty content in every camera-row segment (dsq ≤ 2 ring at spawn pose); Phase 2.13 deferred-dispatched_once ACK pipeline holding` |
| `streaming-window` | **PASS** | ~6 s | `streaming-window gate PASS — mean pixel Δ = 46.22 (floor 3.00); after-frame luminance variance = 2341.60 (floor 800.00); residency origin shift in X = 4 segments (floor 4); max per-frame walk time = 22.0 ms; mid-walk non-sky centre ratio = 0.785 (floor 0.300)` |
| `oasis-edit-visual` | **PASS** | ~6 s | `oasis-edit-visual gate PASS — rect mean per-pixel RGB Δ = 17.93 (floor 8.00); erase sphere @ r = 30.0 voxels; rect luminance before/after = 169.2 / 154.3` |

Total wall-clock for the sweep (release builds cached after gate 1):
~30 s end-to-end. Each gate exited 0.

## Failure detail

None. All three gates exited 0 on first invocation. No retries performed.

## Regression verdict

The Phase 2.14 bottom-up refactor (WSM atomic API + `compute_window_delta` +
`StreamingDiagnostics` + composition tests + production log wiring) **preserved
runtime behaviour** across the three e2e gates that exercise the streaming
layer and the cross-feature voxel-edit path. Specifically:

- `streaming-cold-start` (the Phase-2.13 content gate that walks the
  camera-row segments' `chunks_buffer` snapshot and asserts every camera-row
  segment has at least one non-EMPTY chunk after cold-start drain): all 14
  segments at `dsq ≤ 2` ring have non-empty content. The deferred
  `dispatched_once` ACK pipeline introduced in 2.13 still drains as
  expected after the 2.14.b atomic-API collapse to `allocate_and_bind` /
  `free_segment`. Cold-start completes at frame 259 (well under the gate's
  warmup ceiling).
- `streaming-window` (the legacy procedural-streaming gate): pixel-Δ 46.22
  (floor 3.00, ~15× above floor), variance 2341.60 (floor 800.00, ~2.9×
  above), X-axis origin shift = 4 segments (matches floor exactly), max
  per-frame walk time 22 ms (cap 50 ms), mid-walk non-sky ratio 0.785
  (floor 0.300). All five strict thresholds still hold; the residency layer
  walks the window correctly and produces visible terrain change as the
  camera moves.
- `oasis-edit-visual` (the Phase 03f visual gate): rect mean per-pixel RGB
  Δ 17.93 against an 8.00 floor (~2.2× above), framebuffer capture before
  and after a 30-voxel-radius erase sphere shows the documented luminance
  drop (169.2 → 154.3). The brush-edit code path (which the 2.14 work did
  not touch) still propagates the edit to the renderer; no cross-feature
  regression from the WSM/Diagnostics extractions.

The 2.14 work was bottom-up primitive surgery (data-structure invariants +
extraction of pure-compute helpers + analytical observability) with the
explicit design rule that it must not change production behaviour. The
gates confirm that rule held end-to-end.

## What this DOES NOT verify

- **Interactive UX of the live binary.** Camera flight smoothness, free-look
  control feel, edit-brush UX, FPS at sustained traversal, audio-feedback
  loops — all the things that need a human at the keyboard — are out of
  scope here. Per `bevy-naadf/CLAUDE.md`, the live visual check is the user's
  job; `cargo run --bin bevy-naadf` is explicitly forbidden as an agent
  verification step. The three gates above are an automated lower-bound on
  correctness: they catch *regression* in the specific behaviours each gate
  asserts (cold-start content, sliding-window pixel change, voxel-edit
  framebuffer change), and nothing more.
- **Streaming behaviour beyond the 1024-voxel X-walk in `streaming-window`.**
  Long traversals, diagonal walks, vertical traversal, traversal across
  origin-shift boundaries at high frame rate, fast camera teleports — not
  covered by any of the three gates. The composition tests in 2.14.e cover
  these in pure-data form (without GPU) but not as a live-binary smoke.
- **GPU producer chain regressions** outside the cold-start window. The
  `streaming-cold-start` gate inspects content at one steady-state moment;
  it does not assert anything about per-frame producer-chain stability
  during continuous traversal. The `streaming-window` gate's per-frame
  walk-time bound (≤ 50 ms) is a coarse safety belt only.
- **Diagnostic logging output.** The new production `info!`/`warn!` lines
  from 2.14.f are visible in the `streaming-window` gate's tail (the
  shift-trigger `info!` lines log `cam_seg`, `new_origin`, `evictions`,
  `bound_segments`, `admissions_this_frame`, `cold_start_complete`,
  `unfulfilled`, `in_flight`, `dispatched_once`) but no e2e gate asserts
  on log content. Log-content regression catchers are out of scope for
  this sweep.

## Notes from the run

- The `streaming-cold-start` gate logged shift events with the new extended
  field set from 2.14.f (`cold_start_complete=false`, `unfulfilled=512`,
  `in_flight=0`, `dispatched_once=0`) on the initial admission — confirms
  the production logging surface is wired and emitting the expected fields.
- The `streaming-window` gate observed multiple origin shifts during the
  X-walk (cam_seg 8→9→10→11→12), each logging the expected per-shift
  diagnostic including `in_flight=0` (no stuck slots).
- Build was already cached for the second + third gate invocations; only
  gate 1 paid a `cargo build --release` cost.

## Status

GREEN. Phase 2.14 refactor closes with no e2e regression.
