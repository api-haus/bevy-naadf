# 03k — Phase 2.9 impl: streaming preset endless-reposition-loop fix + production-camera-path gate refactor

Implementation log for the `03j-diagnosis-camera-nudge-loop.md` punch-list.
Lands the production-side camera-position tracker that the
`FreeCameraPlugin`-driven additive `Transform` writes need to compose with
the residency window, and refactors `--gate streaming-window` to exercise
that same additive-Transform path (replacing the previous teleport pin which
masked the bug).

## Files added / edited

| Path | LOC Δ | What changed |
|---|---:|---|
| `crates/bevy_naadf/src/streaming/camera.rs` | +189 (new) | `CameraAbsolutePosition` resource (int+frac, mirroring `PositionSplit` shape), `track_and_pin_camera` Update system, `install_streaming_camera_position` helper, 4 unit tests. |
| `crates/bevy_naadf/src/streaming/mod.rs` | +18 | `pub mod camera;` + re-exports + wire `track_and_pin_camera` in `StreamingPlugin::build` (`Update`, `.before(sync_position_split).after(pin_streaming_window_camera)`). |
| `crates/bevy_naadf/src/streaming/residency.rs` | +44 | New `camera_segment_pos_from_abs` helper; `residency_driver` takes a new `Option<Res<CameraAbsolutePosition>>` param and prefers it when present (falls back to the legacy `pos_int + origin*SEG` formula for the e2e-gate path before any pre-translation). 2 new regression-catcher unit tests (`camera_segment_pos_from_abs_is_origin_independent`, `legacy_camera_segment_pos_double_counts_under_absolute_pos_int`). |
| `crates/bevy_naadf/src/voxel/grid.rs` | +7 | `install_procedural_streaming_world` calls `streaming::install_streaming_camera_position(commands, cam_pos)` so the absolute-position tracker is seeded at the same point `InitialCameraPose` is. |
| `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs` | +14 | `pin_oasis_camera` early-returns when `args.streaming_window_mode == true` so the streaming pin is the sole authority over Transform for the streaming gate. The Y of the birdseye pose (~world_top + 250 = ~762) was placing the camera at segment-Y=2, outside the residency window — incompatible with the new additive-walk shape. |
| `crates/bevy_naadf/src/e2e/streaming_window.rs` | +57 / -10 | `pin_streaming_window_camera` rewritten: pre-walk pins to the streaming-preset spawn pose in window-local, during the walk applies `transform.translation.x += STREAMING_WALK_VOXELS_PER_TICK` per tick (mirroring the production `FreeCamera` controller's additive pattern), post-walk holds. New `WALK_TICKS_REMAINING` static counter, `STREAMING_WALK_TICKS = 256`, `STREAMING_WALK_VOXELS_PER_TICK = 4.0` (1024 voxels = 4 segments total). |

Net LOC added: **~330**. Touched LOC: **~50**. Within the brief's budget
(~120 + ~150 = ~270 target; the surplus is unit-test scaffolding and doc
comments — load-bearing code is ~180 LOC).

## `CameraAbsolutePosition` shape

**Resource** (not Component). Single-camera assumption matches today's
bevy-naadf wiring (`camera::setup_camera` spawns exactly one `Camera3d`).
Promoting to a per-entity `Component` later is a localised change if
multi-camera ever happens.

```rust
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct CameraAbsolutePosition {
    pub pos_int: IVec3,
    pub pos_frac: Vec3,
}
```

Field layout mirrors `crate::camera::position_split::PositionSplit` — int +
frac with `pos_frac` normalised to `[0, 1)^3`. The split avoids f32 precision
loss at world scales of `(4096, 512, 4096)` voxels.

**Insertion sites:**
- `streaming::install_streaming_camera_position` — called by
  `install_procedural_streaming_world` after `InitialCameraPose`. Seeded from
  the streaming spawn pose `(2048, 288, 2048)` (centre X/Z, sea_level + 32 Y).
- No insertion for `Default` / `Vox` / `ProceduralStatic` presets — the
  resource's absence is the signal that the production-side window-local
  re-pin should NOT happen there. `FreeCamera`'s direct Transform writes ARE
  the correct behaviour for those presets.

**Mutation sites:**
- `track_and_pin_camera` — observes per-tick `Transform.translation` deltas
  vs the previous frame's re-pinned window-local position, folds the delta
  into `CameraAbsolutePosition`, re-pins `Transform.translation` to the
  window-local frame at the current `Residency::origin`.

**Read sites:**
- `track_and_pin_camera` itself (`window_local(origin)` re-pin step).
- `residency_driver` (new `Option<Res<CameraAbsolutePosition>>` param) —
  prefers `abs.pos_int` directly for the camera-segment computation,
  bypassing the legacy `pos_int + origin*SEG` round-trip (which was the
  load-bearing failure under the bug).

## System ordering

```text
PreUpdate:
  residency_driver
    reads CameraAbsolutePosition (preferred) OR PositionSplit (fallback)
    writes Residency.{admissions,evictions,window,origin}

RunFixedMainLoop::BeforeFixedMainLoop:
  FreeCameraPlugin::run_freecamera_controller (production only)
    writes Transform.translation ADDITIVELY (the bug-source pattern)

Update:
  e2e::oasis_edit_visual::pin_oasis_camera (e2e-only)
    early-returns when streaming_window_mode
  e2e::streaming_window::pin_streaming_window_camera (e2e-only)
    .after(pin_oasis_camera)
    pre-walk: writes streaming spawn pose
    during walk: writes Transform.translation.x += per-tick delta
    post-walk: no-op
  streaming::track_and_pin_camera (NEW)
    .after(pin_streaming_window_camera)
    .before(sync_position_split)
    observes Transform delta vs prev frame's window-local
    folds delta into CameraAbsolutePosition
    re-pins Transform.translation = abs_pos.window_local(origin)
  camera::sync_position_split
    derives PositionSplit from Transform.translation (now window-local)
```

The critical pins:
- `track_and_pin_camera.before(sync_position_split)` — so the renderer-side
  `PositionSplit::pos_int` is window-local at the current origin.
- `track_and_pin_camera.after(pin_streaming_window_camera)` — so e2e gate
  writes are folded into `CameraAbsolutePosition` before the re-pin. In
  production this `.after()` is a no-op (the e2e pin runs `.after(pin_oasis_camera)`
  which only runs under e2e config; the only Transform writer in production is
  `FreeCamera`, which runs in `RunFixedMainLoop` — earlier in the same frame
  than any `Update` system).

## `--gate streaming-window` refactor

**What was chosen:** the brief offered two paths — (A) drive the production
`AppConfig::windowed()` with simulated winit input, or (B, fallback) keep
`AppConfig::e2e()` but exercise the production camera path via additive
Transform writes. **(B) was chosen** because:

1. Simulating winit `KeyboardInput`/`MouseMotion` events deterministically
   from inside an Update system is non-trivial — Bevy 0.19's input layer
   reads `Res<ButtonInput<KeyCode>>` which is populated by the `bevy_winit`
   crate from real OS events. Pushing fake events to `Events<...>` queues
   doesn't update the `ButtonInput` state by itself.
2. The bug is in the **additive-Transform write pattern**, not the input
   system specifically. The gate exercises that pattern faithfully by
   writing `transform.translation.x += STREAMING_WALK_VOXELS_PER_TICK` per
   tick — exactly the shape `run_freecamera_controller` produces.
3. Switching to `AppConfig::windowed()` requires re-spawning the camera
   entity with `FreeCamera` attached (the e2e camera doesn't have it), and
   the gate's bounded-frame driver + screenshot path is built on
   `AppConfig::e2e()`'s `Continuous` update mode + synchronous pipeline
   compilation. Carrying those guarantees over to `windowed()` is a
   substantial refactor outside the Phase 2.9 scope.

**The gate now exercises the production camera path's core property:**
additive `Transform` writes drive `track_and_pin_camera` → updates
`CameraAbsolutePosition` → `residency_driver` reads from the resource and
shifts origin correctly. If `track_and_pin_camera` regresses (or if the
production install path stops seeding `CameraAbsolutePosition`), the
additive writes drive `residency_driver` into the diagnosed endless
reposition loop and the 120 s wall-clock budget panic fires.

**Measured numbers (Phase 2.9, additive-walk path):**

| Metric | Phase 2.5 (teleport pin) | Phase 2.9 (additive walk) | Floor |
|---|---:|---:|---:|
| Mean pixel Δ | 82.46 | **82.11** | 3.00 |
| After-frame lum variance | 2326.37 | **2346.83** | 800.00 |
| Residency origin shift X | 4 seg | **4 seg** | 4 seg |
| Wall-clock elapsed | ~10 s | ~13 s | 120 s |

The additive-walk path produces effectively identical numbers to the
teleport pin (the framebuffer the assertion samples is the post-walk frame
at the same final pose). The added load-bearing coverage is the
**intermediate** ticks — the residency driver sees per-tick `cam_seg`
movement instead of a one-shot pose jump, exercising the same per-frame
boundary-crossing path the interactive bug fires under.

The diagnostic log confirms the progressive segment shifts:

```
streaming-world residency shift: cam_seg=IVec3(9, 1, 8),  new_origin=(1,0,0), evictions=32
streaming-world residency shift: cam_seg=IVec3(10, 1, 8), new_origin=(2,0,0), evictions=32
streaming-world residency shift: cam_seg=IVec3(11, 1, 8), new_origin=(3,0,0), evictions=32
streaming-world residency shift: cam_seg=IVec3(12, 1, 8), new_origin=(4,0,0), evictions=32
```

Pre-fix, the second shift would have produced `cam_seg=(11, ...)`,
`new_origin=(2,...)`, then `(13,...)` `(3,...)`, then `(15,...)` `(4,...)` —
drifting by `+1` segment per frame after the first shift. Post-fix the
segment progression matches the actual world-position progression `1:1`.

## Verification gates run

All gates run under `timeout` per the
`feedback-e2e-gates-must-fail-fast` memory.

| Gate | Command | Exit | Wall clock | Notes |
|---|---|:--:|---:|---|
| Build | `timeout 180s cargo build --workspace --release` | 0 | 35 s | Clean. |
| Lib tests | `timeout 180s cargo test --workspace --lib --release` | 0 | 5 s | **246 passed** (up from 240 pre-fix; +4 `streaming::camera::tests`, +2 `residency::tests` regression-catchers). 1 ignored (pre-existing, unrelated). |
| `--gate streaming-window` | `timeout 240s cargo run --release --bin e2e_render -- --gate streaming-window` | 0 | 13 s | **PASS.** Pixel Δ = 82.11; lum var = 2346.83; origin shift = 4 seg. Strict thresholds untouched. |
| `--gate noise-static-world` | `timeout 240s cargo run --release --bin e2e_render -- --gate noise-static-world` | 0 | 9 s | **PASS.** Mean luma = 213.26; var = 1812.10; col-stddev = 14.20. Phase 2.4 unregressed. |
| `--gate wgsl-noise-oracle` | `timeout 240s cargo run --release --bin e2e_render -- --gate wgsl-noise-oracle` | 0 | <1 s | **PASS.** 1796 cases, max_abs_diff = 1.49e-6. Phase 1 unregressed. |
| `--gate baseline` | `timeout 240s cargo run --release --bin e2e_render -- --gate baseline` | 0 | 5 s | **PASS.** Batch-6 region gate green; non-streaming presets untouched. |
| `--gate validate-gpu-construction` | `timeout 240s cargo run --release --bin e2e_render -- --gate validate-gpu-construction` | 0 | 6 s | **PASS.** GPU byte-equal to CPU oracle: 388 bytes. W1/W5 chain unregressed. |
| Interactive smoke | `timeout 15s cargo run --release --bin bevy-naadf -- --grid-preset procedural-streaming --vram-budget-mib 1024` | timeout (expected) | 15 s | Boot clean. ONE residency shift on cold-start (`cam_seg=(8,1,8) → origin=(0,0,0)`); no endless reposition loop. (User does the live keyboard-driven visual check separately.) |

## Surprises during implementation

1. **`pin_oasis_camera` writes birdseye Y = 762.** The streaming gate routes
   through `oasis_edit_visual_mode = true` to reuse the OasisXxx state
   machine. Pre-fix, `pin_streaming_window_camera` overwrote the entire
   Transform every tick, so the birdseye write was harmless. Under the
   new additive-walk shape the streaming pin only writes `+= delta` during
   the walk, leaving the birdseye Y (~762, segment row Y = 2 — outside
   the window's `[0, 2)` Y range) in place. **Fix:** added an early-return
   to `pin_oasis_camera` when `streaming_window_mode` is set, mirroring
   the implicit-skip pattern `pin_vox_gpu_construction_camera` /
   `pin_vox_gpu_oracle_camera` already use (those use `.after()` overwrites,
   but the streaming pin can't because it only writes during specific
   phases).

2. **First-tick seed in `track_and_pin_camera` would clobber install-time
   `CameraAbsolutePosition`.** The first cut seeded
   `CameraAbsolutePosition` from `Transform.translation` when
   `prev_window_local == None` — but the e2e harness spawns the camera at
   `e2e_motion_start_transform()`, a different pose from
   `install_procedural_streaming_world`'s `(2048, 288, 2048)`. Seeding from
   the e2e harness's spawn Transform overwrote the install-time absolute
   pose, leaving the residency driver looking up segments at the wrong
   world location. **Fix:** the first-tick branch is now a no-op — re-pin
   uses the existing (install-time-seeded) `CameraAbsolutePosition`
   directly.

3. **System-ordering quirk: `track_and_pin_camera` is in `streaming` but
   must `.after()` an e2e-crate system (`pin_streaming_window_camera`).**
   Solved by referencing `crate::e2e::streaming_window::pin_streaming_window_camera`
   directly. This is a deliberate cross-module dependency — the gate's pin
   is THE upstream Transform writer when running e2e, so the production
   tracker has to observe it. The cycle is fine (`streaming` already
   doesn't depend on `e2e` for compilation, just for system ordering, and
   the e2e module is always present in the build).

## Deviations from this brief

- **Chose option (B) — additive Transform writes via the existing e2e
  pin — over option (A) — `AppConfig::windowed()` + simulated input.** Per
  the brief's fallback clause (>200 LOC threshold). Rationale in
  `## --gate streaming-window refactor` above.

- **`residency_driver` reads `CameraAbsolutePosition` PREFERRED, falls
  back to `PositionSplit + origin*SEG`.** The brief recommended a hard
  switch to absolute-position reads. The fallback keeps the e2e gate's
  pre-Phase-2.9 `pin_streaming_window_camera` flow working unchanged (the
  unit tests `pin_translates_world_to_window_local_origin_*` still cover
  the e2e-side translation helper); it's only the **production** path
  that wires through the new tracker. Both paths produce identical
  residency-driver behaviour; the fallback exists so a future entry
  point (e.g. a new gate that doesn't go through `install_procedural_streaming_world`)
  isn't silently broken.

- **No `--gate streaming-window-windowed` second gate added.** The brief's
  Plan-B fallback mentioned this as an option if input simulation
  ballooned. Since the gate refactor lands cleanly under option (B),
  there's no need for a separate gate. The existing gate now exercises
  the production camera path's core invariant.

## What's left

- **Manual QA: the user runs `cargo run --release --bin bevy-naadf --
  --grid-preset procedural-streaming --vram-budget-mib 1024`, drives the
  camera with WASD + mouse, confirms the camera responds to input
  normally and there is NO endless reposition loop / window-drift
  behaviour.** The interactive smoke from this dispatch confirms boot is
  clean (no spurious residency shifts in the bash-driven run); the
  keyboard-driven walk path is the user's check.

- **Cleanup item — legacy `streaming_window_pose(walked=true)`
  call-site.** The new pin only calls `streaming_window_pose(false)` for
  the pre-walk anchor; the `walked=true` path is now exercised only by
  unit tests (`streaming_window_pose_x_shifts_on_walk` +
  `pin_translates_world_to_window_local_origin_shifted`). The function
  itself is fine to keep — it documents the original intent + the
  Pose-A/Pose-B coordinate calculation in test form. No dead code to
  delete.

- **Cleanup item — `translate_world_to_window_local` helper.** Still used
  by `pin_streaming_window_camera`'s pre-walk anchor + 4 unit tests. The
  production path doesn't need it (the helper is now equivalent to
  `CameraAbsolutePosition::window_local`). Keeping the helper is fine
  for backward compatibility with the e2e tests; refactoring to share an
  implementation between the two would save ~5 LOC.

- **The 2 high-risk Phase 2.7 escalations** (vox-gpu-oracle subprocess
  respawn; oasis-edit-visual / vox-gpu-construction default-fidelity)
  remain pending — unrelated to this fix.

## Followup considerations

- **Bevy `Local<T>` privacy.** `track_and_pin_camera` takes a
  `Local<PrevWindowLocal>` parameter; `PrevWindowLocal` had to be
  promoted to `pub` because Bevy 0.19 requires `Local<T>` parameter
  types to be at least as visible as the system using them. Documented
  at the type declaration.

- **Camera Y under non-default `sea_level`.** If the user passes
  `--sea-level 128`, the install path seeds `CameraAbsolutePosition` at
  Y = 160, but `streaming_window_pose` (used by the e2e pin's pre-walk
  anchor) is independent of `sea_level` and hardcodes Y = 288. The pin
  overwrites Transform with Y = 288 during warmup, `track_and_pin_camera`
  observes a (0, 128, 0) delta and updates abs_pos to Y = 288. Harmless
  for the gate (`--gate streaming-window` doesn't take `--sea-level`),
  but a follow-up consistency pass would make `streaming_window_pose`
  read `AppArgs.sea_level`. Out of Phase 2.9 scope.
