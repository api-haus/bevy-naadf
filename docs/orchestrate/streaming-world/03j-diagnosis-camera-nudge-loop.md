# 03j — Diagnosis: streaming preset enters endless-reposition loop on first interactive camera nudge

Read-only investigation of the user-reported bug:

> `cargo run --release --bin bevy-naadf -- --grid-preset procedural-streaming ...`
> "oh, it does populate but then as soon as i nudge the camera it enters
> endless-reposition loop"

Cold-start works (Phase 2.8 deferred-bounds-flush). The `--gate
streaming-window` gate PASSES (pixel Δ 82.46, variance 2336.37). The bug
fires only under **interactive** camera input.

## Camera position state map

Where the camera's "current position" lives and how it flows across systems
in the **production binary** (`AppConfig::windowed()` — `add_free_camera =
true`, `add_e2e_systems = false`).

```
+----------------------------------------------------------------+
| FreeCameraPlugin::run_freecamera_controller                    |
|   schedule: RunFixedMainLoop::BeforeFixedMainLoop              |
|   read : Transform (current), AccumulatedMouseMotion, ButtonInput
|   write: transform.translation += velocity.x * dt * right      |
|          + velocity.y * dt * up                                |
|          + velocity.z * dt * forward                           |
|   (free_camera.rs:377-380 — ADDITIVE; treats Transform as the  |
|    sole authoritative world position)                          |
+----------------------------------------------------------------+
                          |
                          v Transform.translation (ABSOLUTE WORLD VOXELS)
+----------------------------------------------------------------+
| residency_driver                                               |
|   schedule: PreUpdate                                          |
|   read : PositionSplit (set last frame's Update),              |
|          Residency.window.origin()                             |
|   write: Residency.window.set_origin(...),                     |
|          admissions_this_frame, evictions_this_frame           |
|   FORMULA (residency.rs:238-240):                              |
|     world_voxel = pos_int + origin * SEGMENT_VOXELS            |
|     cam_seg     = world_voxel.div_euclid(SEGMENT_VOXELS)       |
|   ASSUMES pos_int is window-local (per doc-comment :230-237).  |
+----------------------------------------------------------------+
                          |
                          v Residency.origin shifts as camera moves
+----------------------------------------------------------------+
| sync_position_split                                            |
|   schedule: Update (lib.rs:780-787, added unconditionally)     |
|   read : Transform.translation                                 |
|   write: PositionSplit (pos_int = floor(translation),          |
|          pos_frac = fractional remainder)                      |
|   Pure function of Transform — does NOT translate to           |
|   window-local.                                                |
+----------------------------------------------------------------+
                          |
                          v PositionSplit (== absolute world voxels)
+----------------------------------------------------------------+
| naadf renderer (extract → prepare → render-graph)              |
|   reads PositionSplit + the window-indirection buffer.         |
|   Per Phase 2.6 design `02c` § A — renderer treats the camera  |
|   position as window-local and derives                         |
|     chunks_buffer[ pack(pos / 16) ]                            |
|   then indirection translates to slot.                         |
+----------------------------------------------------------------+

pin_streaming_window_camera (e2e/streaming_window.rs:270-346)
  schedule: Update, .after(pin_oasis_camera) .before(sync_position_split)
  IS WIRED ONLY in `add_e2e_systems` path (e2e/mod.rs:245-279).
  NOT PRESENT in the production binary.
  When present it (a) computes Pose A/B as ABSOLUTE WORLD from the
  CAMERA_WALKED latch, (b) calls translate_world_to_window_local to
  subtract origin*SEGMENT_VOXELS, (c) writes BOTH Transform and
  PositionSplit directly. Stateless re-derivation each tick — no drift.
```

Note: `lib.rs:787` adds `sync_position_split` to `Update` even when
`add_free_camera = false`. With `FreeCameraPlugin` on, line 780-783 adds it
alongside `toggle_dlss`. There is no explicit `.before/.after` on
`residency_driver`, but `PreUpdate` precedes both `RunFixedMainLoop` and
`Update` in `MainScheduleOrder::default()`
(`bevy_app/src/main_schedule.rs:222-235`: `First → PreUpdate →
RunFixedMainLoop → Update → SpawnScene → PostUpdate → Last`). So
`residency_driver` reads the PositionSplit from the PREVIOUS frame's
`sync_position_split`, which reflected the PREVIOUS frame's
post-FreeCamera Transform.

## Hypothesis verdicts

### H1: Pin system mutates Transform additively, not re-derives from absolute world position.

**Refuted (for the e2e pin — but Confirmed for the production camera controller).**

The e2e pin `pin_streaming_window_camera` (`e2e/streaming_window.rs:270-346`)
re-derives Transform from a fixed Pose A/B world position each tick. It is
NOT additive — `streaming_window_pose(walked)` returns a fresh `Transform`
from world constants (cx, cy_base, cz) and the
`translate_world_to_window_local` helper is stateless
(`e2e/streaming_window.rs:247-258`). The unit test
`pin_translation_is_idempotent_under_re_derivation`
(`e2e/streaming_window.rs:614-626`) pins this property.

HOWEVER, the bug in the production binary is essentially this hypothesis
applied to the **FreeCamera controller**: `run_freecamera_controller`
(`bevy_camera_controller-0.19.0-rc.1/src/free_camera.rs:377-380`) does
`transform.translation += velocity.{xyz} * dt * {right/up/forward}`, then no
production-side system re-derives a window-local Transform. The `pos_int`
that lands in `residency_driver` IS the absolute world voxel, which the
driver then mis-interprets as window-local and adds the origin offset on
top of (residency.rs:238-240).

### H2: System ordering bug.

**Refuted.** Order is deterministic: `PreUpdate` (residency_driver) →
`RunFixedMainLoop::BeforeFixedMainLoop` (run_freecamera_controller) →
`Update` (sync_position_split). `residency_driver` reads the last frame's
PositionSplit; FreeCamera writes Transform; sync_position_split writes
this frame's PositionSplit. No tearing or feedback loop within a frame.
The one-frame lag between input and residency response is harmless — same
shape Phase 2.6 designed for. The bug fires regardless of frame ordering;
it's a coordinate-system mismatch, not a scheduling race.

### H3: `Residency::origin` recomputes per frame from current Transform with no hysteresis.

**Confirmed.** `residency.rs:277-280`:

```rust
let do_shift = match residency.last_camera_seg {
    None => true,
    Some(prev) => prev != cam_seg_world.0,
};
```

There IS hysteresis at segment granularity (only re-shift when
`cam_seg_world` changes). But under the bug, `cam_seg_world` changes EVERY
FRAME after the first segment crossing — not because the camera crossed a
new boundary, but because the origin grew in the previous frame and
`camera_segment_pos(pos_int, origin)` adds it back. The hysteresis check
is bypassed by the feedback.

Concrete trace (camera spawns at world (2048, 288, 2048), origin = (0,0,0),
last_camera_seg = None):

| Frame | Transform.x | pos_int.x | origin.x (before) | world_voxel.x = pos_int + origin*256 | cam_seg.x | last_camera_seg.x | do_shift | new_origin.x |
|------:|------------:|----------:|------------------:|-------------------------------------:|----------:|------------------:|:--------:|-------------:|
| 1     | 2048        | 2048      | 0                 | 2048                                 | 8         | None              | true     | 0            |
| ...   | (no move)   | 2048      | 0                 | 2048                                 | 8         | 8                 | false    | 0            |
| K     | 2304        | 2304      | 0                 | 2304                                 | 9         | 8                 | true     | 1            |
| K+1   | 2305        | 2305      | 1                 | 2305 + 256 = 2561                    | 10        | 9                 | true     | 2            |
| K+2   | 2306        | 2306      | 2                 | 2306 + 512 = 2818                    | 11        | 10                | true     | 3            |
| K+3   | 2307        | 2307      | 3                 | 2307 + 768 = 3075                    | 12        | 11                | true     | 4            |

After the FIRST real segment crossing (frame K) origin.x advances by +1
**every frame** as long as the player gives a non-zero velocity (or even
zero velocity if the next tick's `pos_int` is still in the prior segment —
wait, hysteresis would catch zero-velocity case at frame K+1 because
`last_camera_seg.x == cam_seg.x` would re-evaluate — no, by frame K+1
`last_camera_seg = 9, cam_seg = 10` — DIFFERENT, do_shift triggers,
origin grows again, AND the next frame `last_camera_seg = 10, cam_seg =
11` — still different. The feedback drifts at a rate of one
segment/frame indefinitely).

### H4: There's no absolute-world Transform Resource at all.

**Confirmed.** The codebase has exactly one piece of state that knows the
camera's "true world position": the `Transform` itself. There is no
separate `CameraAbsolutePosition` resource, no `WorldPosition` component
on the camera entity, nothing. `PositionSplit` is derived FROM Transform
each frame (`position_split.rs:114-119`).

A `Resource`-style absolute-world tracker exists only in two surrogate
forms — neither suitable as the load-bearing source-of-truth:

- `Residency::last_camera_seg` is segment-level only (no sub-segment
  precision).
- `e2e/streaming_window.rs:streaming_window_pose(walked)` is a fixed
  pose computed from world constants + the CAMERA_WALKED bool — works
  for the gate's two-pose test but isn't a general absolute-world
  tracker.

## Root cause

When the user nudges the camera in the production binary, `FreeCamera`
adds the input delta to `Transform.translation` (an absolute-world voxel
coordinate). No system in the production path re-translates the
Transform into window-local before `residency_driver` and the renderer
consume it. `residency_driver` at
`crates/bevy_naadf/src/streaming/residency.rs:238-240` then computes

```rust
let world_voxel = camera_pos_int + residency_origin * SEGMENT_VOXELS;
```

assuming `camera_pos_int` is window-local (per its doc comment at lines
230-237). Once any real segment crossing has bumped `residency.origin`
above zero, this formula double-counts the origin: the absolute Transform
already encodes the camera's world position, and the driver adds the
origin offset on top. Every subsequent frame the inferred `cam_seg`
drifts by an additional +(origin_delta) segments, `target_origin_for_camera_seg`
chases it, the origin grows by +1 segment, the inferred `cam_seg`
overshoots by another +1 next frame, ad infinitum. The 4-segments-per-
frame admission/eviction churn locks the GPU producer node in continuous
dispatch and the visible world appears to slide past the user without
their input matching.

## Why the e2e gate doesn't reproduce

The `--gate streaming-window` gate wires
`pin_streaming_window_camera` into `Update`
(`crates/bevy_naadf/src/e2e/mod.rs:270-271`), which runs only because the
gate uses `AppConfig::e2e()` with `add_e2e_systems = true`
(`crates/bevy_naadf/src/lib.rs:631-639`). That same config sets
`add_free_camera = false`, so `FreeCameraPlugin` is omitted entirely. On
each Update tick the e2e pin OVERWRITES `Transform.translation` and
`PositionSplit` from a stateless re-derivation:
`streaming_window_pose(camera_has_walked())` returns an ABSOLUTE-WORLD
pose computed from world constants, then
`translate_world_to_window_local(world_pose, residency)` subtracts
`origin * SEGMENT_VOXELS` (`crates/bevy_naadf/src/e2e/streaming_window.rs:340-345`).
So in the gate the Transform consumed by `residency_driver` IS
window-local, and the `pos_int + origin * SEGMENT_VOXELS` formula
correctly recovers the absolute world voxel. The unit test
`pin_translation_is_idempotent_under_re_derivation`
(`crates/bevy_naadf/src/e2e/streaming_window.rs:614-626`) pins exactly
this property. The bug is invisible to the gate because the gate has
already implemented the missing fix — for itself only.

## Punch-list for the fix dispatch

Goal: make the production binary behave the same way the e2e gate does —
the renderer always sees a window-local Transform; the residency driver
always sees a Transform whose `pos_int + origin * SEGMENT_VOXELS`
correctly recovers the absolute world voxel.

The cleanest shape that does NOT depend on the e2e crate (the e2e pin is
test-fixture code; production cannot reach back into it) is **option (b)
from `03b-impl-residency.md:266-280` Hand-off** — a production-side pin
analogous to the e2e one, but reading the absolute-world position from a
new tracker resource the FreeCamera-aware input layer writes.

The minimal shape that lands a correct fix:

1. **Introduce `CameraAbsolutePosition` resource (or component on the
   camera entity)** in `crates/bevy_naadf/src/camera/mod.rs` near the
   `InitialCameraPose` definition (`camera/mod.rs:48-65`). Newtype around
   `Vec3` (or `(IVec3, Vec3)` if we want to mirror `PositionSplit`'s
   int+frac split for large worlds — recommended for f32 precision at
   world scale 4096+ voxels). Seeded from `InitialCameraPose` in
   `setup_camera`. **~15 LOC.**

2. **Add `apply_free_camera_to_absolute_position` system** that mirrors
   `run_freecamera_controller`'s additive deltas onto
   `CameraAbsolutePosition` instead of (or alongside) the Transform.
   Schedule: same as the FreeCamera plugin —
   `RunFixedMainLoop::BeforeFixedMainLoop`, `.after(run_freecamera_controller)`.
   Reads the post-controller Transform delta vs the previous frame, adds
   it to the absolute tracker, then RESETS the Transform back to
   window-local (`absolute - origin * SEGMENT_VOXELS`).

   Alternative shape (cleaner): the controller writes Transform as
   normal; this new system reads the absolute tracker, advances it by
   `(transform.translation - previous_window_local_transform_we_stored)`,
   then immediately reprojects the Transform back to window-local. The
   Transform serves only as a transient delta channel between the
   controller and this system.

   Schedule gated on `Residency` resource being present (the streaming
   preset). **~40 LOC.**

3. **Add `pin_streaming_production_camera` system** at
   `crates/bevy_naadf/src/streaming/residency.rs` (next to
   `residency_driver`, or in a sibling file `streaming/camera_pin.rs`).
   Equivalent to the e2e `pin_streaming_window_camera` minus the wall-
   clock budget and Pose A/B latch — it reads `CameraAbsolutePosition`
   and `Residency.window.origin()`, writes
   `Transform.translation = absolute - origin * SEGMENT_VOXELS` and the
   matching `PositionSplit`. Stateless re-derivation each tick.
   Schedule: `Update`, `.before(crate::camera::sync_position_split)` (so
   the corrected Transform is what `sync_position_split` consumes). Per
   the existing precedent at `crates/bevy_naadf/src/e2e/mod.rs:279`.
   Gated on `Residency` resource being present. **~30 LOC.**

4. **Wire systems 2 + 3 in `StreamingPlugin::build`** at
   `crates/bevy_naadf/src/streaming/mod.rs:64-100`. The plugin already
   early-returns the inner systems when `Residency` is absent (per the
   `Option<ResMut>` pattern documented at
   `crates/bevy_naadf/src/streaming/residency.rs:255-259`), so adding
   the new systems is purely additive. **~10 LOC.**

5. **Update `residency_driver`'s doc-comment** at
   `crates/bevy_naadf/src/streaming/residency.rs:230-241` — the assertion
   "the camera Transform / `PositionSplit::pos_int` is **window-local**"
   was true ONLY for the e2e gate. With the production pin in place it
   becomes universally true. Strike the "(Phase 2.5 — `pin_streaming_window_camera`
   pre-translates...)" parenthetical and replace with a note about the
   production pin. **~5 LOC.**

6. **Add a regression-catcher unit test** in
   `crates/bevy_naadf/src/streaming/residency.rs::tests`. Plant a
   `Residency` with origin = (4, 0, 0) and `last_camera_seg = (10, 1, 8)`
   (the camera segment after a 4-segment +X walk). Set a window-local
   `PositionSplit` with `pos_int = (2048, 288, 2048)` (the matching
   window-local position after translation). Run a one-tick
   `residency_driver` equivalent (call its inner logic via a `pub(crate)`
   helper, or inline the formula) and assert `cam_seg_world == (12, 1, 8)`
   and `do_shift == false` — proving that under correct window-local
   input the driver does NOT keep shifting origin. **~20 LOC.**

Estimated total: **~120 LOC** across 3 source files and 1 doc edit.

## What test would catch this

The `--gate streaming-window` cannot catch this even if we extended its
walk distance — it bypasses the FreeCamera controller entirely. Two
deliverables together would land the regression net:

(a) **Extend `--gate streaming-window` to also run a
"free-camera-style" walk variant** that exercises the additive Transform
path. Concretely: in a second sub-phase after `OasisShootAfter`, replace
the `streaming_window_pose(walked=true)` write with a sequence of small
`Transform.translation += (1.0, 0.0, 0.0)` writes — one per tick for ~300
ticks — to simulate WASD movement. Assert at the end that
`Residency.origin` has shifted by AT MOST `walk_distance /
SEGMENT_VOXELS = 1` segments (the bug under diagnosis would show 300+
segments of drift). This catches the production-path bug without
requiring real keyboard input (we drive `Transform` directly the way the
FreeCamera controller does — additively, in absolute world coords).

(b) **A unit test on `residency_driver`'s segment-detection
idempotence** (item 6 of the punch-list above) — proves that given a
correct window-local pos_int, repeated `residency_driver` ticks at the
same camera position do NOT shift origin. Today's
`slot_admissions_eventually_drain_to_resident` test
(`crates/bevy_naadf/src/streaming/residency.rs:506-558`) doesn't drive
the camera-segment-detection path; the new test would.

(a) is the gate that catches the lived-experience regression; (b) is the
fine-grained guard that locates a future regression to the specific
formula line. Ship both.

## Hard one-off observation (Task D)

Ran `timeout 10s cargo run --release --bin bevy-naadf -- --grid-preset
procedural-streaming --vram-budget-mib 1024` once. Captured (truncated):

```
streaming-world: ProceduralStreaming preset installed — noise_preset=0,
  seed=1337, sea_level=256.0, terrain_amplitude=64.0, vram_budget_mib=1024,
  max_segments_per_frame=4; camera spawn at Vec3(2048.0, 288.0, 2048.0)
  looking at Vec3(2148.0, 240.0, 2048.0)
streaming-world residency shift: cam_seg=IVec3(8, 1, 8),
  new_origin=IVec3(0, 0, 0), evictions=0, bound_segments=512,
  admissions_this_frame=4
streaming-world: dispatched 4 segment(s) this frame (0 evictions);
  bounds chain deferred (latched dirty=true).
[... ~130 identical "dispatched 4 segment(s)" log lines ...]
```

Confirms: (1) the camera spawns at absolute world (2048, 288, 2048); (2)
`residency_driver` evaluates `cam_seg=(8,1,8)` on tick 1 and computes
`new_origin=(0,0,0)` — origin matches because origin starts at zero so
window-local==world for this single tick; (3) exactly ONE "residency
shift" log line fires in the bash-driven run (no keyboard input → no
interactive nudge → no second segment crossing → no loop). The bug
trace in the H3 verdict only fires when the user crosses a segment
boundary, which bash cannot simulate.

The bash run does not reproduce; the analysis stands on code reading.
