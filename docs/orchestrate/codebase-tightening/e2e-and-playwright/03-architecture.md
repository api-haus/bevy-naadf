# D6 — e2e-and-playwright — architecture

**Author**: refactor-architect (codebase-tightening orchestration, D6 of 8).
**Date**: 2026-05-20.
**Input**: `01-context.md` (incl. 2026-05-20 addendum), `02-exploration.md`
(10 findings, 3 HIGH), D7's `02-exploration.md` §F7 + §`device_snapshot
deletion notes`, D3's `02-exploration.md` §F2 + §F6, `00-reuse-audit.md`
§2 D6 row + §3 DUP-6.

This is a significant task in computer graphics — every file:line ref has
been verified with Read/Grep against the working tree at commit `e042b88`
before being cited.

---

## refactor-architect findings (2026-05-20)

### 1. Findings addressed

The design covers every finding the explorer raised (10/10) + the
D6-side of the D7-coordinated `device_snapshot` deletion + the D3-side
coordination notes:

| # | severity | resolution |
|---|---|---|
| 1 | high | DELETE per 01-context addendum Resolution C — 1 988 LOC drop. |
| 2 | high | DECOMPOSE `e2e_driver`: `enum E2ePhase` collapses from 49 to 8 variants; per-gate flow becomes a `trait Gate` impl driven by a shared loop. |
| 3 | high | DUP-6 — extract `set_camera_pose` helper + `pin_active_gate_camera` system that resolves the active gate's pose via the `trait Gate` impls (replaces the `.after(pin_oasis_camera)` priority chain). |
| 4 | med | `Framebuffer::save_in_screenshots_dir(filename, gate_tag) -> Result<PathBuf, String>` in `e2e/framebuffer.rs` — delete the seven per-gate wrappers. |
| 5 | med | DELETE `--device-snapshot-native`, `--validate-gpu-construction-scaled`, `--validate-gpu-construction-production` short-circuits per coordinated deletions; remaining 14-flag ladder collapses to a `parse_e2e_command(args) -> Command` enum + a single `match`. |
| 6 | med | `add_e2e_systems` becomes a thin coordinator that registers ONE shared driver + ONE shared camera-pin system; per-gate `State` resources stay (each owned by its `Gate` impl) but only the active gate's state is consulted. |
| 7 | med | DELETE per 01-context addendum Resolution A — D6 owns the deletions of `device-snapshot.spec.ts`, `--device-snapshot-native`, `bin/diag_compare.rs`, `Cargo.toml [[bin]] diag_compare`, justfile `diag-native`/`diag-web`/`diag-compare`/`diag` recipes. D7 owns the production-side submodule + plugin wiring. |
| 8 | med | Verdict log moves into `trait Gate::verdict_log(&self, outcome) -> String`; driver owns AppExit + outcome stash uniformly. |
| 9 | low | Per-gate frame budgets keep their per-gate consts (the convergence requirements are genuinely different) but every per-gate `*_FRAMES` collapses into one `FrameBudget` value the gate's `Gate` impl returns. |
| 10 | low | The 6-flag mode-detection inside `e2e_driver` collapses to `state.active_gate: GateKind` (an `enum` carved off `AppArgs`) — set ONCE at app build time. |

**Coordination findings absorbed:**

- **D7 `device_snapshot` chain (Resolution A)** — design enumerates the
  D6-side delete list with exact `path:line` ranges (§3 step 1).
- **D3 F2 (`vox_gpu_oracle` CPU phase fate)** — design preserves the
  CPU + GPU phases; the gate is the `feedback-multiple-runs-rule-out-false-positives`-
  protected non-deterministic gate (per `01-context.md` Q2 — "CPU oracle stays").
  D3's `install_vox_sized_to_model` deletion is contingent on D6 dropping
  the gate; **D6 keeps the gate**, so D3 takes path option 2 (cfg-test
  the `tiled` helpers).
- **D3 F6 (horizon-camera constants)** — design notes the new home
  (D3 owns; either `voxel/grid.rs` or `camera/poses.rs`); D6's e2e gate
  imports from the new location once D3 moves them. Stub in §6.

**Findings skipped:** none — all 10 + 2 cross-domain items addressed.

### 2. Target-state architecture

#### Target file layout (post-refactor LOC estimates)

```
crates/bevy_naadf/src/e2e/
├── mod.rs                          ~280 LOC  (was 387 — del PBR refs;
│                                              shared registry collapsed)
├── driver.rs                       ~700 LOC  (was 1 956 — 49→8 enum,
│                                              per-gate flow extracted)
├── gate.rs                          ~90 LOC  (NEW — `trait Gate`,
│                                              `enum GateKind`, runner loop)
├── framebuffer.rs                  ~530 LOC  (was 514 — +1 helper)
├── gates.rs                         ~813 LOC  (untouched — keep verbatim;
│                                              user-tuned thresholds)
├── checks.rs                        ~183 LOC  (untouched)
├── readback.rs                      ~38 LOC   (untouched)
├── ssim.rs                          ~229 LOC  (untouched)
├── tracing_error_counter.rs         ~112 LOC  (untouched)
├── oasis_edit_visual.rs            ~390 LOC  (-63 LOC: del save_*, pin
│                                              system merges into runner)
├── small_edit_visual.rs            ~620 LOC  (-61 LOC: del save_*, pin)
├── small_edit_repro.rs             ~315 LOC  (-61 LOC: del save_*, pin)
├── vox_e2e.rs                      ~699 LOC  (untouched — standard gate)
├── vox_gpu_construction.rs         ~432 LOC  (-61 LOC: del save_*, pin)
├── vox_gpu_oracle.rs               ~635 LOC  (-61 LOC: del save_*, pin)
├── vox_horizon_parity.rs           ~190 LOC  (-56 LOC: del save_*, pin;
│                                              consts ref D3's new home)
├── vox_web_parity.rs               ~370 LOC  (-58 LOC: del save_*, pin)
├── pbr_debug_modes.rs              DELETED  (-218 LOC)
├── pbr_hard_edge.rs                DELETED  (-1 023 LOC)
└── pbr_visual.rs                   DELETED  (-747 LOC)

crates/bevy_naadf/src/bin/
├── e2e_render.rs                   ~250 LOC  (was 481 — ladder→match,
│                                              del device-snapshot,
│                                              del validate-* short-circuits)
└── diag_compare.rs                 DELETED  (-314 LOC)

e2e/tests/
└── device-snapshot.spec.ts         DELETED  (-122 LOC)

justfile diag-* recipes             DELETED  (-25 LOC: 4 recipes)
Cargo.toml [[bin]] diag_compare     DELETED  (-7 LOC)
```

**Net delta**: `-1 988` (PBR gates) `-314` (diag_compare) `-122`
(device-snapshot spec) `-25` (justfile) `-7` (Cargo entry) `-1 256`
(driver decomposition) `-360` (per-gate trims) ≈ **-4 072 LOC across
D6 + ~580 LOC indirect from coordinated D7/D8 deletes**. Even the
"keep, don't delete" refactor still extracts substantial structure
because the per-gate flow merges into the shared driver loop.

#### Finding 1: PBR gate orphan deletion (Resolution C)

**Current shape (verified):**

- `crates/bevy_naadf/src/e2e/pbr_debug_modes.rs:1-218` — orphaned.
- `crates/bevy_naadf/src/e2e/pbr_hard_edge.rs:1-1023` — orphaned.
- `crates/bevy_naadf/src/e2e/pbr_visual.rs:1-747` — orphaned.
- Verified `grep -n "pbr" crates/bevy_naadf/src/bin/e2e_render.rs` returns
  zero matches — no CLI flag exists.
- Verified `grep -n "pbr_debug_modes\|pbr_hard_edge\|pbr_visual"
  crates/bevy_naadf/src/e2e/mod.rs` returns zero matches at `e2e/mod.rs:24-38`
  — files are not registered in `pub mod`.
- The three files reference `args.pbr_*_mode` fields which D7's
  AppArgs at `lib.rs:283-462` does NOT contain (verified: zero
  `pbr_*_mode` fields in current `AppArgs` definition).
- `01-context.md` addendum Resolution C: **user confirmed delete**.

**Target shape:**

The three files vanish from the source tree. Zero compile/link impact
(already orphaned from `e2e/mod.rs`).

```bash
# verified clean — no callers anywhere:
rm crates/bevy_naadf/src/e2e/pbr_debug_modes.rs
rm crates/bevy_naadf/src/e2e/pbr_hard_edge.rs
rm crates/bevy_naadf/src/e2e/pbr_visual.rs
```

**Reuse choices:** none — pure deletion. PBR work lives on the PBR
branch per 01-context addendum master-branch identity statement.

**Behavioural delta:** none — files were never in the dispatch surface.
The audit's `00-reuse-audit.md §1.3` listed `pbr_hard_edge.rs:1023` as
the 9th-largest Rust file; that ghost vanishes from the LOC top-list.

#### Finding 7: `device_snapshot` chain — D6-side deletions (Resolution A)

**Current shape (verified):**

- `crates/bevy_naadf/src/bin/e2e_render.rs:137-143` — flag declaration
  (`let device_snapshot_native_mode = args.iter().any(...);` at line 143).
- `crates/bevy_naadf/src/bin/e2e_render.rs:364-375` — dispatch arm
  (the `else if device_snapshot_native_mode { … bevy_naadf::run_e2e_render() }`).
- `crates/bevy_naadf/src/bin/diag_compare.rs:1-314` — entire binary.
- `crates/bevy_naadf/Cargo.toml:39-41` — `[[bin]]` entry for `diag_compare`.
- `e2e/tests/device-snapshot.spec.ts:1-122` — Playwright spec.
- `justfile:196-213` — recipes `diag-native`, `diag-web`, `diag-compare`,
  `diag` (verified by `grep -n "diag-\|device-snapshot\|diag_compare"
  justfile` — 4 matches at lines 189, 194, 197, 210).
- `e2e/tests/vox-horizon-parity.spec.ts:122,147,158,187` — diagnostic
  sentinel-grep (forwards `[device-snapshot]` line into test report
  annotations). Per explorer F7: "diagnostic noise — stays, it just won't
  match anything after deletion."

**Target shape:**

All D6-side `device_snapshot` callers deleted. D7's implementor
removes the `diagnostics::device_snapshot` submodule + the
`DeviceSnapshotPlugin` wiring in `lib.rs:799` as the production-side
half of the same logical change.

`bin/e2e_render.rs` after deletion (the relevant slice):

```rust
let device_snapshot_native_mode = /* DELETED */;        // 7 LOC del at 137-143
// ...
} else if vox_web_parity_loaded_mode {                  // unchanged at 358-363
    bevy_naadf::e2e::vox_web_parity::run_vox_web_parity_loaded_phase()
} else if device_snapshot_native_mode {                 // 12 LOC del at 364-375
    /* DELETED */
} else if vox_horizon_native_mode {                     // unchanged
```

`vox-horizon-parity.spec.ts` sentinel-grep stays as-is (orphaned
matcher; safe per explorer side note 1).

**Reuse choices:** none — pure deletion.

**Behavioural delta:**
- The `just diag-native` / `just diag-web` / `just diag-compare` /
  `just diag` recipes stop existing — calling them errors out at the
  `just` parser. Acceptable: the diagnostics chain is being retired
  entirely per user directive.
- `vox-horizon-parity.spec.ts`'s annotation block no longer contains
  the `[device-snapshot]` line. Verified by reading the spec at
  cited lines: those annotations are non-load-bearing.

#### Finding 2: `driver.rs` god-function decomposition

**Current shape (verified):**

```rust
// driver.rs:58-248 — 49-variant flat enum
pub enum E2ePhase {
    Warmup, Motion, Settle, Shoot, Drain, Assert,
    // resize-test (11 variants)  LaunchSettle..ResizeAssert
    // oasis (8 variants)         OasisWarmup..OasisAssert
    // small-edit (8 variants)    SmallEditWarmup..SmallEditAssert
    // small-edit-repro (8 vars)  SmallEditReproWarmup..SmallEditReproAssert
    // vox-gpu-oracle (3 vars)    VoxGpuOracleWarmup/Shoot/Drain
    // vox-web-parity (3 vars)    VoxWebParityWarmup/Shoot/Drain
    Done,
}

// driver.rs:439-1679 — 1 240-LOC `match state.phase { ... }` body
// + 6 fast-path route-in blocks at 475-577.
```

Counted via `grep` against the live file:
- `shoot_primary_window(&mut commands);` — 11 sites in driver.rs.
- `Framebuffer::from_image(&image)` decode — 9 sites.
- `state.phase = E2ePhase::Done;` after AppExit::error — 16+ sites.
- `if let Some(image) = screenshot.0.take() {` drain — 8 sites.

**Target shape:**

A two-level state machine — outer `Phase` is gate-agnostic; the gate
provides per-step config. The eight gates the harness currently
dispatches become first-class `Gate` impl values.

New file `crates/bevy_naadf/src/e2e/gate.rs` (~90 LOC):

```rust
//! Per-gate trait absorbing the Warmup→Shoot→Drain→Save→Assert pattern.
//!
//! Each e2e gate provides its frame budgets, its camera pose, its edit
//! hook (if any), its assertion, and its verdict log. The shared driver
//! loop drives all of them through one `Phase` state machine.

use bevy::prelude::*;
use crate::camera::position_split::PositionSplit;
use crate::world::data::WorldData;
use super::framebuffer::Framebuffer;

/// Identifies which gate the run is dispatching. Set once at app build
/// time from `AppArgs`; held inside `E2eState` for the duration of the run.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GateKind {
    /// The default Warmup→Motion→Settle→Shoot→Drain→Assert flow that the
    /// resize / oasis / small-edit families don't take over (covers
    /// `baseline`, `--vox-e2e`, `--entities`, `--edit-mode`,
    /// `--validate-gpu-construction` post-app tails — i.e. the e2e modes
    /// where the standard region gate runs).
    #[default]
    Standard,
    Resize,                  // resize-blackness (genuinely distinct flow)
    OasisEdit,               // brush-edit gate over Oasis VOX
    VoxGpuConstruction,      // share-flow-with-OasisEdit + camera promote
    SmallEditVisual,         // brush + voxel-count + adj-rect gate
    SmallEditRepro,          // user-captured Oasis click repro
    VoxGpuOracle,            // single-capture; CPU vs GPU phase
    VoxWebParity,            // single-capture; skybox/loaded/horizon phase
}

/// Per-gate frame budget.
#[derive(Clone, Copy, Debug)]
pub struct FrameBudget {
    pub warmup: u32,
    /// `None` for single-capture gates with no edit phase.
    pub post_edit_wait: Option<u32>,
    pub drain: u32,
}

/// The trait every gate implements. Owned by the gate's `e2e/<gate>.rs`
/// module; consumed by the shared driver loop in `driver.rs`.
///
/// `&Self` methods (no `&mut self`) — gate config is static at boot.
/// State that mutates lives in the per-gate `State` resource the gate
/// declares.
pub trait Gate: Send + Sync + 'static {
    /// Which kind this gate is; used by the driver to discriminate
    /// edit-phase vs single-capture flows. (Could be derived from
    /// `post_edit_wait().is_some()` instead — architect leaves both
    /// signals available for explicit gate-shape branching.)
    fn kind(&self) -> GateKind;

    fn frame_budget(&self) -> FrameBudget;

    /// Compute the camera pose this gate pins. `world_data` is `None` if
    /// the gate's pose doesn't depend on world size (resize / horizon
    /// / web-parity); `Some` for the world-centre poses (oasis /
    /// small-edit / vox-gpu-oracle).
    ///
    /// Returns `None` if the gate's pose isn't computable yet (e.g. the
    /// world hasn't loaded) — the driver leaves the camera at whatever
    /// the standard pin wrote.
    fn camera_pose(&self, world_data: Option<&WorldData>) -> Option<Transform>;

    /// Apply the gate's edit (brush, camera promote, … ). `None` for
    /// single-capture gates. The driver calls this exactly once on the
    /// `Apply` phase.
    fn apply_edit(
        &self,
        _world_data: Option<&mut WorldData>,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Run the gate's assertion against the captured before/after
    /// framebuffer(s). `after` is `None` for single-capture gates;
    /// `before` is `None` for single-capture gates without a pre-edit
    /// capture.
    fn assert(
        &self,
        before: Option<&Framebuffer>,
        after: Option<&Framebuffer>,
    ) -> Result<String, String>;

    /// Format the gate's PASS verdict log (called only on `Ok`).
    /// Defaults to `Ok` payload — gates with extra config to surface
    /// override.
    fn verdict_log(&self, ok_msg: &str) -> String {
        ok_msg.to_string()
    }

    /// Filename pair this gate writes its captures to. For single-capture
    /// gates `(before, after)` is `(None, Some(_))`. The driver calls
    /// `Framebuffer::save_in_screenshots_dir(filename, gate_tag)` for each.
    fn capture_filenames(&self) -> (Option<&'static str>, &'static str);

    /// Per-gate log prefix (used by save + assert log lines).
    fn log_tag(&self) -> &'static str;
}
```

New / simplified `enum E2ePhase` at `driver.rs:58-...` (down from 49 to
8 variants):

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum E2ePhase {
    /// Static warmup, dispatched on the per-gate `FrameBudget.warmup`.
    /// For `GateKind::Standard` this is the original Warmup→Motion→Settle
    /// flow; the runner branches on `state.kind == Standard` and routes
    /// to the inline motion sub-state-machine. (Kept inline because the
    /// Motion phase is genuinely unique to Standard — no other gate has
    /// it.)
    #[default]
    Warmup,
    /// `Standard` only — the camera-orbit sweep.
    Motion,
    /// `Standard` only — re-pin at readback pose.
    Settle,
    /// Spawn the screenshot. Used for both pre-edit + post-edit captures
    /// (single-capture gates run it once; edit gates twice).
    Shoot,
    /// Drain the async capture. Distinguishes the two captures via the
    /// `taken_before: bool` sub-state.
    Drain,
    /// Apply the per-gate edit. `Gate::apply_edit` is called exactly
    /// once. Edit-gates only.
    Apply,
    /// Per-edit-gate post-edit wait — `FrameBudget.post_edit_wait`.
    /// Edit-gates only.
    PostEditWait,
    /// Per-gate assert + verdict + AppExit write.
    Assert,
    /// AppExit written.
    Done,
}
```

The Resize family (11 variants — `LaunchSettle..ResizeAssert`) does NOT
fold into the shared shape. Wayland resize is structurally unique and
its captures aren't bidirectional. Resize keeps its inline state
machine; the design DOES move it behind a `GateKind::Resize`
discriminator (so the standard flow's loop doesn't pollute its
arms) but the variants stay. **Verified architect-side**: trying
to express Resize as Warmup→Shoot→Drain×3 would force a 3-shot capture
abstraction that no other gate needs — cleaner to carve Resize out as
the named exception.

Post-refactor, `E2ePhase` has:
- 8 gate-agnostic variants (Warmup, Motion, Settle, Shoot, Drain,
  Apply, PostEditWait, Assert) + Done = 9 variants.
- 11 Resize-only variants (kept verbatim) prefixed `Resize*`.

Total: **20 variants** (down from 49). Eight gates worth of inline
state machines (`Oasis*`, `SmallEdit*`, `SmallEditRepro*`,
`VoxGpuOracle*`, `VoxWebParity*`) — **30 variants — DELETED**.

New driver loop shape (~250 LOC vs current 1 240):

```rust
pub fn e2e_driver(
    mut state: ResMut<E2eState>,
    mut outcome: ResMut<E2eOutcome>,
    mut screenshot: ResMut<E2eScreenshot>,
    mut captures: ResMut<GateCaptures>,        // NEW — replaces 6 per-gate State
    mut resize_test: ResMut<ResizeTestState>,
    world_data: Option<ResMut<crate::world::data::WorldData>>,
    diagnostics: Res<DiagnosticsStore>,
    pipeline_scan: Res<PipelineScanResult>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    active_gate: Res<ActiveGate>,              // NEW — `Res<Box<dyn Gate>>` analogue
    app_args: Option<Res<crate::AppArgs>>,
) {
    if active_gate.kind() == GateKind::Resize {
        return run_resize_state_machine(/* … */); // verbatim today's resize body
    }
    if active_gate.kind() == GateKind::Standard {
        return run_standard_state_machine(/* … */); // Warmup→Motion→Settle→…
    }
    // Per-gate trait-driven flow:
    let budget = active_gate.frame_budget();
    let has_edit = budget.post_edit_wait.is_some();
    match state.phase {
        E2ePhase::Warmup => {
            state.phase_ticks += 1;
            if state.phase_ticks >= budget.warmup {
                screenshot.0 = None;
                state.phase = E2ePhase::Shoot;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::Shoot => {
            shoot_primary_window(&mut commands);
            state.phase = E2ePhase::Drain;
            state.phase_ticks = 0;
        }
        E2ePhase::Drain => {
            state.phase_ticks += 1;
            if let Some(image) = screenshot.0.take() {
                let fb = decode_or_fail(&image, /* … */);
                save_gate_capture(&active_gate, &fb, captures.before.is_none());
                if captures.before.is_none() && has_edit {
                    captures.before = Some(fb);
                    state.phase = E2ePhase::Apply;
                } else {
                    captures.after = Some(fb);
                    state.phase = E2ePhase::Assert;
                }
                state.phase_ticks = 0;
            } else if state.phase_ticks >= budget.drain {
                fail_gate(&active_gate, "screenshot never delivered", &mut exit,
                          &mut outcome, &mut state);
            }
        }
        E2ePhase::Apply => {
            match active_gate.apply_edit(world_data.as_deref_mut()) {
                Ok(()) => {
                    state.phase = E2ePhase::PostEditWait;
                    state.phase_ticks = 0;
                }
                Err(msg) => fail_gate(/* … */),
            }
        }
        E2ePhase::PostEditWait => {
            state.phase_ticks += 1;
            if state.phase_ticks >= budget.post_edit_wait.expect("edit gate") {
                screenshot.0 = None;
                state.phase = E2ePhase::Shoot;
                state.phase_ticks = 0;
            }
        }
        E2ePhase::Assert => {
            let result = active_gate.assert(captures.before.as_ref(),
                                            captures.after.as_ref());
            run_assert_verdict(&active_gate, result, &mut exit, &mut outcome,
                               &mut state);
        }
        E2ePhase::Done => {}
        _ => unreachable!("non-Standard/Resize gates do not enter Motion/Settle"),
    }
}
```

**Reuse choices:**
- `Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>` —
  Bevy idiom for the unique-camera mutation pattern (already used at
  `driver.rs:465`).
- `Framebuffer::from_image` — already in `e2e/framebuffer.rs`.
- `shoot_primary_window` — already in `e2e/readback.rs`.
- `Res<DiagnosticsStore>` / `Res<PipelineScanResult>` — already wired
  by `add_e2e_systems`.
- No new third-party dep — `trait Gate` is plain dyn-compatible Rust.

**Behavioural delta:**
- The frame-budget consts each gate currently exports stay in place
  (gates need them for their `FrameBudget` value); only the inlined
  duplication of the loop shape collapses.
- The `Apply` phase shape changes from "`if !applied { call edit;
  applied = true; }`" to "`Apply` is entered exactly once" — the
  one-shot guarantee is enforced by the phase transition, not by a
  per-gate `edit_applied: bool`. The fields on the per-gate `State`
  structs go away (`OasisEditVisualState.edit_applied`, etc.).
  **Exception**: `vox_gpu_construction` reads `oasis.edit_applied` to
  promote camera pose A→B; that signal moves to a dedicated
  `VoxGpuConstructionState.camera_promoted: bool` set by the gate's
  `apply_edit` (which is a camera-promote stub, not a real brush).

Verification — every currently-passing gate must still pass. Per the
brief: "non-deterministic gates (`oasis_edit_visual`, `vox_gpu_oracle`)
need ≥2 runs in verification" — the implementor runs the relevant
gate ≥2× after each step. The CLAUDE.md verification surface is
preserved.

#### Finding 3: DUP-6 camera-pin consolidation (`pin_active_gate_camera`)

**Current shape (verified):**

Seven `pin_*_camera` systems all share the pattern (verified by reading
each file at the cited lines):

```rust
pub fn pin_<gate>_camera(
    args: Option<Res<crate::AppArgs>>,
    /* sometimes */ world_data: Option<Res<WorldData>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.<gate_flag> { return; }
    /* sometimes */ let Some(world_data) = world_data else { return; };
    let pose = compute_pose(...);
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
}
```

Plus three driver-internal sites (`driver.rs:586-589, 612-614, 637-639`).
Plus the `pin_resize_test_camera` (`driver.rs:286-297`).

Verified `grep -n "PositionSplit::from_world" crates/bevy_naadf/src/e2e/`
— 11 matches across 6 gate files + driver.

`add_e2e_systems` (`e2e/mod.rs:248-281`) registers all seven pin systems
with an explicit `.after(pin_oasis_camera)` priority chain (lines 259-279).

**Target shape:**

ONE shared `Update` system, driven by the `ActiveGate` resource. The
`set_camera_pose` helper extracts the 3-line write.

In `e2e/gate.rs`:

```rust
/// Write `pose` to the camera's `Transform` + recompute `PositionSplit`.
/// Replaces the 3-line `**transform = pose; **position_split =
/// PositionSplit::from_world(pose.translation);` write that 11 sites
/// currently duplicate verbatim.
pub fn set_camera_pose(
    camera: &mut (Mut<Transform>, Mut<PositionSplit>),
    pose: Transform,
) {
    *camera.0 = pose;
    *camera.1 = PositionSplit::from_world(pose.translation);
}

/// `Update` system: pin the active gate's camera pose every tick.
/// Replaces the seven `pin_*_camera` systems + the three driver-inline
/// camera writes.
///
/// Registered `.after(e2e_driver)` so the gate's pose lands AFTER the
/// driver's standard-flow pose write (which only runs for
/// `GateKind::Standard`) but BEFORE `sync_position_split` consumes the
/// `Transform`.
pub fn pin_active_gate_camera(
    active_gate: Res<ActiveGate>,
    world_data: Option<Res<WorldData>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(pose) = active_gate.camera_pose(world_data.as_deref()) else {
        return;
    };
    let pose_t = (camera.0.clone(), camera.1.clone());
    let _ = pose_t;
    set_camera_pose(&mut camera, pose);
}
```

The seven `pin_*_camera` fns in the per-gate modules go away. Each
gate's `Gate::camera_pose` impl returns its `Transform` (constants /
world-centre derivation already lives in each gate's module).

The `.after(pin_oasis_camera)` priority chain in
`add_e2e_systems:248-281` collapses to ONE registration:

```rust
.add_systems(Update, (
    driver::e2e_driver,
    gate::pin_active_gate_camera.after(driver::e2e_driver),
).before(crate::camera::sync_position_split))
```

The `pin_resize_test_camera` in `driver.rs:286-297` also goes away —
the Resize gate's `Gate::camera_pose` returns
`e2e_resize_test_camera_transform()`. The three driver-inline writes
at `driver.rs:586-589, 612-614, 637-639` (in Warmup/Motion/Settle of
the Standard flow) stay — they implement the `Standard` gate's orbit
animation, which is `t`-parameterised and doesn't fit
`camera_pose(world_data)`'s signature. Architect-side decision: the
Standard `GateKind::Standard` carries the orbit logic inline because
factoring it would force `camera_pose(world_data, phase, phase_ticks)`
and no other gate uses those parameters. **Standard is the named
exception**, like Resize.

**Reuse choices:**
- `Mut<T>` deref pattern — Bevy idiom for system-parameter mutable
  refs (already used everywhere in the e2e/).
- The active-gate-resolution is the `ActiveGate` resource (a
  `Box<dyn Gate>` analogue — see §"Decisions").

**Behavioural delta:**
- Camera write semantics identical to the current 11 sites.
- Ordering identical: `e2e_driver` → `pin_active_gate_camera` →
  `sync_position_split`.
- Frame timing identical (write every Update tick — `BEV-5 Added<>/Changed<>`
  filter is NOT applied here per explorer side note 3: the per-gate
  ordering chain is load-bearing for "more-specific-gate beats
  less-specific-gate"; with a single pin system + a single active
  gate, that priority resolution moves to the
  `parse_e2e_command → ActiveGate` decision at app build time, where it
  belongs).

#### Finding 4: `save_*_screenshot` duplication

**Current shape (verified):**

Seven near-identical fns:
- `e2e/oasis_edit_visual.rs:442-453` — `save_oasis_screenshot`.
- `e2e/small_edit_visual.rs::save_small_edit_screenshot` (called from
  `driver.rs:1173, 1249`).
- `e2e/small_edit_repro.rs::save_small_edit_repro_screenshot` (called
  from `driver.rs:1357, 1435`).
- `e2e/vox_gpu_oracle.rs:675-686` — `save_oracle_screenshot`.
- `e2e/vox_web_parity.rs:417-428` — `save_parity_screenshot`.
- `e2e/vox_horizon_parity.rs:235-246` — `save_horizon_screenshot`.
- `e2e/vox_gpu_construction.rs::save_vox_gpu_construction_screenshot`.

Each is:

```rust
pub fn save_<gate>_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!("e2e_render --<gate>: screenshot saved to {}", path.display()),
        Err(e) => eprintln!("e2e_render --<gate>: {filename} save failed: {e}"),
    }
}
```

Only the log prefix differs.

**Target shape:**

One helper on `Framebuffer`:

```rust
// e2e/framebuffer.rs — appended to existing impl block at line ~370
impl Framebuffer {
    /// Save `self` to `target/e2e-screenshots/<filename>`. Best-effort;
    /// logs to stdout/stderr with `gate_tag` for grep-ability, returns
    /// the resolved path on success. Replaces seven per-gate
    /// `save_*_screenshot` wrappers.
    pub fn save_in_screenshots_dir(
        &self,
        filename: &str,
        gate_tag: &str,
    ) -> Result<std::path::PathBuf, String> {
        let path = std::path::Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
        match self.save_png(&path) {
            Ok(()) => {
                println!("e2e_render --{gate_tag}: screenshot saved to {}", path.display());
                Ok(path)
            }
            Err(e) => {
                eprintln!("e2e_render --{gate_tag}: {filename} save failed: {e}");
                Err(e)
            }
        }
    }
}
```

All seven per-gate wrapper fns deleted. ~12 call sites in `driver.rs`
update to `fb.save_in_screenshots_dir(filename, gate.log_tag())?;`.

The driver helper `save_gate_capture` from the §Finding 2 sketch wraps
this with `Gate::log_tag` + `Gate::capture_filenames`.

**Reuse choices:**
- `std::path::Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename)`
  — already the convention in every gate.
- `Framebuffer::save_png` — already at `e2e/framebuffer.rs:374-405`,
  the load-bearing API.

**Behavioural delta:** none — the log format is identical
("`e2e_render --<gate>: screenshot saved to <path>`"). The
`gate_tag` parameter takes the per-gate string literal (e.g.
`"oasis-edit-visual"`).

#### Finding 5 + 10: `bin/e2e_render.rs` CLI ladder + `AppArgs` flag-bag

**Current shape (verified):**

`bin/e2e_render.rs:81-148` — 18 flag declarations.
`bin/e2e_render.rs:174-207` — 3 short-circuit early-returns
(`vox_gpu_oracle_mode`, `vox_web_parity_mode`, `ssim_compare_mode`).
`bin/e2e_render.rs:213-239` — 2 diagnostic short-circuit early-returns
(`validate_gpu_construction_scaled`,
`validate_gpu_construction_production`).
`bin/e2e_render.rs:241-412` — 250-line `if/else if` ladder routing
the 13 remaining modes.
`bin/e2e_render.rs:419-478` — 4 post-app validation tails (orthogonal
to gate dispatch).

The mode-detection inside `e2e_driver` (`driver.rs:475-577`) is 6
`app_args.as_deref().is_some_and(|a| a.<flag>)` lookups + 6 route-in
branches.

`AppArgs` (`lib.rs:283-462`) has 11 mode/phase flags that are
mutually-exclusive in practice but the type system doesn't enforce it.

**Target shape:**

A `parse_e2e_command` function that turns CLI args into a
`Command` enum, replacing the if-else-if ladder with a single
`match`.

```rust
// bin/e2e_render.rs — new top-level
enum Command {
    /// Run the e2e harness with the named gate active. Maps directly
    /// to `bevy_naadf::e2e::gate::GateKind` (D6 owns the type).
    /// Carries the `AppArgs` already populated.
    Boot { gate: GateKind, args: bevy_naadf::AppArgs },
    /// `--ssim-compare` — no Bevy boot, pure PNG diff.
    SsimCompare,
    /// Top-level multi-process gate that spawns sub-phase subprocesses
    /// and exits with their composite result.
    VoxGpuOracleCompare,
    VoxWebParityCompare,
}

fn parse_e2e_command(args: &[String]) -> Command {
    if args.iter().any(|a| a == "--ssim-compare") { return Command::SsimCompare; }
    if args.iter().any(|a| a == "--vox-gpu-oracle") { return Command::VoxGpuOracleCompare; }
    if args.iter().any(|a| a == "--vox-web-parity") { return Command::VoxWebParityCompare; }
    // Mutually-exclusive gate flags: the FIRST matching flag wins. The
    // user can't legitimately set two; if they do, the table order is
    // the resolution priority.
    let (gate, mut app_args) = parse_gate_args(args);
    Command::Boot { gate, args: app_args }
}

fn parse_gate_args(args: &[String]) -> (GateKind, AppArgs) {
    // 14-row table-driven gate flag parse. Each row: (cli_flag, GateKind,
    // app_args_mutator). The post-app `--validate-gpu-construction*` and
    // `--entities` and `--edit-mode` and `--runtime-edit-mode` are post-app
    // validation tails — NOT gate selectors — they don't show up here.
    /* ... ~30 LOC ... */
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let post_app_validations = parse_post_app_validations(&args); // 4 flags

    let app_exit = match parse_e2e_command(&args) {
        Command::SsimCompare => return ExitCode::from(run_ssim_compare(&args)),
        Command::VoxGpuOracleCompare =>
            return ExitCode::from(bevy_naadf::e2e::vox_gpu_oracle::run_vox_gpu_oracle_compare()),
        Command::VoxWebParityCompare =>
            return ExitCode::from(bevy_naadf::e2e::vox_web_parity::run_vox_web_parity_compare()),
        Command::Boot { gate, args } => {
            install_resize_test_windowrule_if_needed(gate);
            let app_exit = bevy_naadf::run_e2e_render_with_args(args);
            cleanup_resize_test_windowrule_if_needed(gate);
            app_exit
        }
    };

    let e2e_code = app_exit_to_code(app_exit);
    run_post_app_validations(post_app_validations, e2e_code)
}
```

`bin/e2e_render.rs` shrinks from 481 → ~250 LOC.

**Note on the diagnostic short-circuits**: `--validate-gpu-construction-scaled`
and `--validate-gpu-construction-production` (lines 213-239) call directly
into `bevy_naadf::render::construction::validate_*` and short-circuit
the Bevy boot. The architect's call: these are **D5's territory**, not
D6's. The current binary entry happens to host them because they
piggyback on the e2e_render CLI surface. The two functions exist
in `render/construction/mod.rs:4928,5290,5621` — D5's architect is
already proposing structural extraction of those. D6 keeps the CLI
short-circuits in place but isolates them via a
`parse_post_app_validations` helper so D5's later move (extracting
`validate_*` to `render/construction/validation/`) only touches the
imports.

The 4 post-app validation tails (`if validate_gpu_construction`, `if
entities_mode`, `if edit_mode`, `if runtime_edit_mode` at lines
419-478) compose orthogonally with any gate dispatch and stay as
post-app tails — NOT inside the gate enum. The `PostAppValidation`
helper struct collects the 4 booleans + dispatches them.

**On the `AppArgs` flag-bag (Finding 10):**

The architect explored two alternatives:
- **(a)** Add `AppArgs.e2e_gate: Option<GateKind>` and leave the 11
  per-mode booleans as a deprecated layer that the new field replaces.
- **(b)** Replace the 11 booleans with `AppArgs.e2e_gate: GateKind`
  outright — every consumer reads the enum instead of a boolean.

Choice: **option (a) for the D6 refactor**, **option (b) deferred to
D7's later impl phase**. Rationale:
- `AppArgs` is in `lib.rs` (D7 territory). The structural change to
  delete fields is D7's call.
- D6 can land its `enum GateKind` + driver decomposition without
  removing `AppArgs` fields; the gate's `Gate::camera_pose` reads the
  enum, the legacy `args.<flag>` reads still work for the production
  binary's flag-of-no-effect noise.
- The eventual `AppArgs.e2e_gate: GateKind` migration is a D6+D7
  coordinated change that the architect documents here (§"D7
  coordination notes") for D7's later impl phase.

**Reuse choices:**
- `enum Command` / `enum GateKind` — plain Rust idiomatic.
- Bevy already provides no relevant abstraction (mode dispatch is
  pre-Bevy).
- No external crate.

**Behavioural delta:**
- Every existing CLI flag still works (`--oasis-edit-visual`,
  `--vox-gpu-oracle-cpu`, etc.). The 18 → 14 reduction is `--ssim-compare`
  (kept), `--vox-gpu-oracle` and `--vox-web-parity` (kept as
  top-level compare commands), the 3 PBR flags (DELETED — Resolution C),
  `--device-snapshot-native` (DELETED — Resolution A). The 4 post-app
  tail flags are unchanged.
- Logging unchanged (the `parse_*` functions emit the same
  startup messages).
- Exit codes unchanged.

#### Finding 6: `add_e2e_systems` god-init

**Current shape (verified):**

`e2e/mod.rs:204-296` — 92-line `pub fn add_e2e_systems(app: &mut App)`.

Inserts 6 per-gate `State` resources (lines 226-234) regardless of
which gate runs. Adds 7 pin-camera systems with explicit
`.after(pin_oasis_camera)` priority chain (lines 248-281).

**Target shape:**

`add_e2e_systems` collapses to:

```rust
pub fn add_e2e_systems(app: &mut App, gate: GateKind) {
    let pipeline_scan = PipelineScanResult::default();
    let active_gate = gate::active_gate_for(gate);          // -> Box<dyn Gate>

    app
        .insert_resource(WinitSettings {
            focused_mode: UpdateMode::Continuous,
            unfocused_mode: UpdateMode::Continuous,
        })
        .insert_resource(pipeline_scan.clone())
        .insert_resource(active_gate)                       // NEW
        .init_resource::<readback::E2eScreenshot>()
        .init_resource::<driver::E2eState>()
        .init_resource::<driver::E2eOutcome>()
        .init_resource::<driver::ResizeTestState>()         // only used by Resize
        .init_resource::<driver::GateCaptures>()            // NEW — replaces 5 per-gate State
        .init_resource::<tracing_error_counter::TracingErrorCounter>()
        .add_systems(Startup, setup_e2e_camera)
        .add_systems(
            Update,
            (
                driver::e2e_driver,
                gate::pin_active_gate_camera.after(driver::e2e_driver),
            )
                .before(crate::camera::sync_position_split),
        );

    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .insert_resource(pipeline_scan)
            .add_systems(Render,
                scan_pipeline_errors_render_system.after(RenderSystems::Render));
    }
}
```

The 7 separate `pin_*_camera` system entries collapse to ONE entry.

The 6 per-gate `State` resources (`OasisEditVisualState`,
`SmallEditVisualState`, `SmallEditReproState`, `VoxGpuOracleState`,
`VoxWebParityState`) collapse into ONE `GateCaptures` resource the
driver owns:

```rust
// driver.rs
#[derive(Resource, Default)]
pub struct GateCaptures {
    /// Pre-edit / pre-promote capture (None for single-capture gates).
    pub before: Option<Framebuffer>,
    /// Post-edit / single capture.
    pub after: Option<Framebuffer>,
    /// Per-gate auxiliary stash. `vox_gpu_construction` uses this for
    /// `camera_promoted: bool`; `small_edit_visual` for the
    /// `voxel_count_{before,after}` + `world_size_voxels` triple. Empty
    /// for the other gates.
    pub aux: GateAuxState,
}

#[derive(Default)]
pub enum GateAuxState {
    #[default]
    None,
    SmallEditCounts { count_before: u64, count_after: u64, world_size: [u32; 3] },
    VoxGpuConstruction { camera_promoted: bool },
}
```

`tracing_error_counter::TracingErrorCounter` stays as its own
resource — it's not per-gate, it's a cross-cutting log filter.

`ResizeTestState` stays — the Resize gate's three captures need a
distinct stash shape (initial/after_a/after_b), and the Resize state
machine is the named exception.

**Reuse choices:**
- `app.insert_resource(active_gate)` where `active_gate: Box<dyn Gate>`
  — Bevy supports this; the design wraps it in a thin `ActiveGate`
  newtype for type-erasure-friendly Res<> queries.
- Per-gate `State` resources where they carry config — STAY (oasis's
  `OASIS_*_FRAMES` consts, small-edit's `voxel_count_*`). They migrate
  into the `GateAuxState` only when they're capture-flow state, not
  config.

**Behavioural delta:**
- Memory footprint shrinks — 5 unused State resources stop being
  inserted on every run.
- Init cost shrinks (one less system in the Update schedule per
  ex-`pin_*_camera`).
- System ordering identical (`e2e_driver` → pin → `sync_position_split`).

#### Finding 8: scattered assertion / verdict log

**Current shape (verified):**

Each gate's `Assert` arm in the driver has the shape (verified at
`driver.rs:1087-1147` Oasis, 1281-1329 SmallEdit, 1466-1493
SmallEditRepro, 1538-1573 VoxGpuOracle, 1591-1672 VoxWebParity):

```rust
E2ePhase::<Gate>Assert => {
    let before = state.before.take();
    let after = state.after.take();
    let result = match (before, after) {
        (Some(a), Some(b)) => gate_assert(&a, &b).map(|msg| println!(...)),
        _ => Err("driver bug ..." .to_string()),
    };
    match &result {
        Ok(()) => {
            println!("e2e_render: <gate> PASS — <config-specific summary>");
            exit.write(AppExit::Success);
        }
        Err(msg) => {
            eprintln!("e2e_render: FAIL —\n{msg}");
            exit.write(AppExit::error());
        }
    }
    outcome.gate_result = Some(result);
    state.phase = E2ePhase::Done;
}
```

The verdict format drift (some include `floor`, some `radius`,
some `world_size`) is per-gate hand-typed at each `println!`.

**Target shape:**

One `run_assert_verdict` helper at the driver level + `Gate::assert`
+ `Gate::verdict_log` on the trait:

```rust
fn run_assert_verdict(
    gate: &dyn Gate,
    result: Result<String, String>,    // String = the "ok message" the
                                       //          gate's assert returns
    exit: &mut MessageWriter<AppExit>,
    outcome: &mut E2eOutcome,
    state: &mut E2eState,
) {
    let unit_result = match &result {
        Ok(ok_msg) => {
            let verdict = gate.verdict_log(ok_msg);
            println!("e2e_render: {verdict}");
            exit.write(AppExit::Success);
            Ok(())
        }
        Err(msg) => {
            eprintln!("e2e_render: FAIL —\n{msg}");
            exit.write(AppExit::error());
            Err(msg.clone())
        }
    };
    outcome.gate_result = Some(unit_result);
    state.phase = E2ePhase::Done;
}
```

Each gate's `Gate::verdict_log` impl owns its verbose summary
(referencing its own consts). Example for OasisEditVisual:

```rust
impl Gate for OasisEditVisual {
    /* … */
    fn verdict_log(&self, ok_msg: &str) -> String {
        format!(
            "oasis-edit-visual PASS — {} warmup + {} post-edit wait frames; \
             erase sphere @ r={:.1} voxels; {ok_msg}",
            OASIS_WARMUP_FRAMES,
            OASIS_POST_EDIT_WAIT_FRAMES,
            OASIS_ERASE_RADIUS,
        )
    }
}
```

**Reuse choices:**
- `MessageWriter<AppExit>` — Bevy idiom, already used at
  `driver.rs:467`.
- Each gate's existing `assert_*_landed` fn stays — only the call
  shape changes (the driver passes results through `Gate::assert`).

**Behavioural delta:**
- PASS log lines are the same text (each gate's `verdict_log`
  reproduces today's per-gate format).
- FAIL log lines are the same text (the per-gate `assert` returns
  the existing error messages).
- Exit codes the same.

#### Finding 9: per-gate frame-budget consts

**Current shape (verified):**

- `e2e/mod.rs:55-193` — 14 top-level constants (E2E_WARMUP_FRAMES,
  E2E_MOTION_FRAMES, etc.).
- Per-gate constants in 6 modules (OASIS_*_FRAMES, SMALL_EDIT_*_FRAMES,
  ORACLE_*_FRAMES, PARITY_*_FRAMES).
- `vox_horizon_parity.rs:110,113` already aliases
  `super::vox_web_parity::PARITY_WARMUP_FRAMES` — verified.

**Target shape:**

Per-gate consts STAY (they're calibrated values; changing them needs
user sign-off per the explorer side-note 4 — "the threshold values
were tuned by the user"). Each gate's `Gate::frame_budget()` returns a
`FrameBudget` composed of its existing consts:

```rust
// oasis_edit_visual.rs (post-refactor)
impl Gate for OasisEditVisual {
    fn frame_budget(&self) -> FrameBudget {
        FrameBudget {
            warmup: OASIS_WARMUP_FRAMES,
            post_edit_wait: Some(OASIS_POST_EDIT_WAIT_FRAMES),
            drain: OASIS_DRAIN_FRAMES,
        }
    }
    /* … */
}
```

The driver's `match` arms read `budget.warmup` / `budget.drain` /
`budget.post_edit_wait` instead of `super::oasis_edit_visual::OASIS_*_FRAMES`.

The Standard gate's `FrameBudget`:

```rust
impl Gate for StandardGate {
    fn frame_budget(&self) -> FrameBudget {
        FrameBudget {
            warmup: E2E_WARMUP_FRAMES,
            post_edit_wait: None,    // Standard has no edit phase
            drain: E2E_DRAIN_FRAMES,
        }
    }
}
```

Standard's Motion + Settle phases stay inline (they're Standard-only).

**Reuse choices:** none new — every const stays at its declaration
site. Only the `FrameBudget` aggregate is new.

**Behavioural delta:** none — values identical run-to-run.

### 3. Migration steps

Ordered, granular. Each step keeps the surviving gates green.

#### Step 1 — DELETE: device_snapshot e2e-side + diag_compare + PBR gates

**Edits:**
- `crates/bevy_naadf/src/bin/e2e_render.rs:137-143` — delete the
  `device_snapshot_native_mode` flag declaration + comment.
- `crates/bevy_naadf/src/bin/e2e_render.rs:364-375` — delete the
  `else if device_snapshot_native_mode { … }` dispatch arm.
- `crates/bevy_naadf/src/bin/diag_compare.rs` — delete the entire file
  (314 LOC).
- `crates/bevy_naadf/Cargo.toml:34-41` — delete the `[[bin]] name =
  "diag_compare"` block.
- `e2e/tests/device-snapshot.spec.ts` — delete the entire file (122 LOC).
- `justfile:185-213` — delete `diag-native`, `diag-web`, `diag-compare`,
  `diag` recipes + the section comment.
- `crates/bevy_naadf/src/e2e/pbr_debug_modes.rs` — delete (218 LOC).
- `crates/bevy_naadf/src/e2e/pbr_hard_edge.rs` — delete (1 023 LOC).
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` — delete (747 LOC).

**Rationale:** These deletions are independent of the structural
refactor. They land first so the LOC win is realised + the structural
steps don't waste cycles on dead code. PBR gates are already
orphaned; `device_snapshot` chain is user-confirmed delete (Resolution
A). D7 lands its production-side deletions (`diagnostics.rs:155-711`
+ `lib.rs:799`) AFTER D6's e2e-side deletions per orchestrator
sequencing.

**Post-step state:**
- `wc -l crates/bevy_naadf/src/e2e/` ≈ `-1 988` LOC.
- `crates/bevy_naadf/src/bin/` ≈ `-314` LOC.
- `e2e/tests/` ≈ `-122` LOC.
- `cargo build --workspace` succeeds — `device_snapshot_native_mode`
  declaration + dispatch removed in one commit.
- `cargo build --workspace` LIKELY-still-builds even before D7's
  `diagnostics::device_snapshot` deletion lands — the only D6→D7
  coupling was `bin/diag_compare.rs` referencing `diagnostics::
  device_snapshot` schema types, and that binary is deleted in this
  step. After this step lands, D7 can delete the submodule without
  any D6-side dangling refs. **Verified**: `grep -n
  "diagnostics::device_snapshot" crates/bevy_naadf/src/` post-step
  returns matches only in `lib.rs:799` (D7 deletes that).

**Verification:**
- `cargo build --workspace` — proves the e2e_render binary still
  builds without the dispatched-but-deleted `device_snapshot_native_mode`.
- `cargo test --workspace --lib` — proves no lib-level test referenced
  the deleted spec / binary.
- `cargo run --bin e2e_render -- --oasis-edit-visual` — proves a
  representative non-deleted gate still runs (run ≥2× per the
  feedback-multiple-runs rule).
- `cargo run --bin e2e_render -- --vox-gpu-construction` — proves
  the vox-gpu-construction gate still runs.
- `cargo run --bin e2e_render -- --vox-horizon-native` — proves the
  horizon-parity gate (which carried the `[device-snapshot]` sentinel
  forwarder) still runs and its annotation block's orphaned sentinel
  matcher doesn't crash the spec.
- `cd e2e && npx playwright test --headed` — proves the remaining
  4 Playwright specs (wasm-smoke, sw-chrome-extension, vox-loading,
  vox-horizon-parity) still pass. Verifies channel `chrome` per
  user memory `feedback-playwright-channel-google-chrome-stable`.

#### Step 2 — INTRODUCE: `e2e/gate.rs` trait + `GateKind` enum + `set_camera_pose` helper + `Framebuffer::save_in_screenshots_dir`

**Edits:**
- `crates/bevy_naadf/src/e2e/gate.rs` (NEW) — add `trait Gate`, `enum
  GateKind`, `struct FrameBudget`, `fn set_camera_pose`, `fn
  pin_active_gate_camera` (system), `struct ActiveGate(Box<dyn Gate>)`
  resource type. ~90 LOC.
- `crates/bevy_naadf/src/e2e/mod.rs:27` — add `pub mod gate;`.
- `crates/bevy_naadf/src/e2e/framebuffer.rs` — add
  `pub fn save_in_screenshots_dir(&self, filename: &str, gate_tag: &str)
  -> Result<PathBuf, String>` on `impl Framebuffer`. ~16 LOC.

**Rationale:** Lands the abstractions ahead of the per-gate migration
so each per-gate impl can land independently in step 3+.

**Post-step state:**
- The `gate.rs` module is in the tree but no one uses it yet —
  `cargo build` warns "unused"; the trait is not yet impl'd by any
  gate.
- `Framebuffer::save_in_screenshots_dir` exists but isn't called yet.
- The seven `save_*_screenshot` wrapper fns still exist.

**Verification:**
- `cargo build --workspace` — proves the trait + helper compile.
- `cargo test --workspace --lib` — proves nothing existing broke.
- No e2e gates need to run — pure additive change.

#### Step 3 — MIGRATE: per-gate `Gate` impls + delete `save_*_screenshot` wrappers + delete `pin_*_camera` systems

**Edits (per-gate, applied to all 8 gates that survive):**

For each of `oasis_edit_visual.rs`, `small_edit_visual.rs`,
`small_edit_repro.rs`, `vox_gpu_construction.rs`, `vox_gpu_oracle.rs`,
`vox_web_parity.rs`, `vox_horizon_parity.rs`, `vox_e2e.rs`:

- Add `impl Gate for <GateStruct> { … }` block in the gate's module.
- Delete the per-gate `pin_*_camera` fn (replaced by `Gate::camera_pose`).
- Delete the per-gate `save_*_screenshot` fn (replaced by
  `Framebuffer::save_in_screenshots_dir(filename, gate.log_tag())`).
- Update the per-gate `assert_*_landed` fn to take `before`/`after` as
  `Option<&Framebuffer>` (already the inner shape after `state.before.take()`).
  Most existing assert fns take `(&Framebuffer, &Framebuffer)` — wrap them
  by the `Gate::assert` impl that does the `Option` unwrap.

Also:
- `crates/bevy_naadf/src/e2e/driver.rs` — update the call sites in the
  driver to use `Framebuffer::save_in_screenshots_dir` directly via
  the gate's `log_tag` (interim — the per-gate `match` arms still
  exist; this step is just changing the save-fn calls inside them).
- `crates/bevy_naadf/src/e2e/mod.rs:248-281` — KEEP the existing
  `.after(pin_oasis_camera)` chain for now (deleted in step 4). But
  delete the now-orphan `pin_*_camera` system registrations whose
  fns we just deleted (i.e. delete every pin entry in the tuple).
  REPLACE with `gate::pin_active_gate_camera.after(driver::e2e_driver)`.

Wait — there's a structural ordering issue. We can't delete the
per-gate pin entries until step 4 lands the `ActiveGate` resource
the `pin_active_gate_camera` system reads. The implementor must
either:
- (a) Land step 4 (the driver decomposition + ActiveGate resource)
  BEFORE step 3's per-gate `pin_*_camera` deletions, OR
- (b) Use a two-substep sequence where step 3a lands the per-gate
  `impl Gate` blocks (additive — old fns still present), step 3b
  lands the `ActiveGate` insertion + the new `pin_active_gate_camera`
  registration in `add_e2e_systems` (changes the active pin path but
  keeps the old per-gate fns alongside), step 3c deletes the
  per-gate `pin_*_camera` fns + their registrations (the cleanup).

Architect recommends **(b)** — finer steps; each substep keeps gates
green. The implementor breaks step 3 into 3a/3b/3c at impl time.

**Rationale:** The trait-impl migration is the biggest mechanical
change. Doing it gate-by-gate (one gate at a time) keeps each commit
small. The save-fn replacement and pin-fn deletion are bundled
because they share the same per-gate module.

**Post-step state:**
- Every gate that the harness dispatches has a `Gate` impl.
- `pin_active_gate_camera` runs in place of 7 separate pin systems.
- `Framebuffer::save_in_screenshots_dir` is called from the driver;
  the 7 wrapper fns are gone.

**Verification (per gate, after each gate's substep):**
- `cargo build --workspace`.
- `cargo test --workspace --lib` (the per-gate assert fns have unit
  tests, e.g. `small_edit_visual.rs` has the `count_non_empty_voxels`
  tests).
- Run the affected gate ≥2× (≥3× for `vox_gpu_oracle` per `feedback-
  multiple-runs-rule-out-false-positives`).

Per gate, the verifying invocation:
- `cargo run --bin e2e_render -- --oasis-edit-visual` (≥2×)
- `cargo run --bin e2e_render -- --small-edit-visual` (≥2×)
- `cargo run --bin e2e_render -- --small-edit-repro` (≥2×)
- `cargo run --bin e2e_render -- --vox-gpu-construction` (≥2×)
- `cargo run --bin e2e_render -- --vox-gpu-oracle` (≥3× — non-deterministic)
- `cargo run --bin e2e_render -- --vox-web-parity` (≥2×)
- `cargo run --bin e2e_render -- --vox-horizon-native` (≥2×)
- `cargo run --bin e2e_render -- --vox-e2e` (≥2×)
- `cargo run --bin e2e_render` (baseline ≥2×)

#### Step 4 — DECOMPOSE: `driver.rs` — collapse 49→20 variants, extract per-gate flows, introduce `GateCaptures` resource

**Edits:**
- `crates/bevy_naadf/src/e2e/driver.rs:58-248` — replace the 49-variant
  `enum E2ePhase` with the new 20-variant shape (9 standard/shared +
  11 Resize).
- `crates/bevy_naadf/src/e2e/driver.rs:439-1679` — replace the
  1 240-LOC body with the new dispatching loop that consults
  `ActiveGate`. The Resize state machine arms stay verbatim (just
  renamed to `Resize*` prefix); the Standard arms stay verbatim
  (Warmup/Motion/Settle); the per-gate arms (Oasis*, SmallEdit*,
  SmallEditRepro*, VoxGpuOracle*, VoxWebParity*) collapse into the
  generic Warmup/Shoot/Drain/Apply/PostEditWait/Assert arms driven by
  `Gate` trait methods.
- `crates/bevy_naadf/src/e2e/driver.rs:286-297` — delete
  `pin_resize_test_camera` (replaced by `ResizeGate::camera_pose`).
- `crates/bevy_naadf/src/e2e/driver.rs` — introduce `struct
  GateCaptures { before, after, aux }` + `enum GateAuxState` resource.
- `crates/bevy_naadf/src/e2e/mod.rs:204-296` (`add_e2e_systems`) —
  replace the 5 per-gate `State.init_resource` calls with one
  `GateCaptures.init_resource()` + the `ActiveGate` resource
  insertion. Remove the 7-system pin chain in favour of the single
  `pin_active_gate_camera` registration (this was a step-3b precursor;
  full removal lands here).
- Per-gate `State` resources where they only carried before/after
  framebuffers + edit_applied → delete. The few resources that
  carried gate-specific aux (small-edit's `voxel_count_*`, vgc's
  `camera_promoted`) migrate into `GateAuxState`.

**Rationale:** This is the biggest single edit. The driver shape is
the bottleneck for every other simplification. After this step the
driver is ~700 LOC (down from 1 956) and the per-gate State
resources collapse to 1 + Resize.

**Post-step state:**
- `wc -l driver.rs` ≈ 700.
- `enum E2ePhase` has 20 variants.
- One generic Warmup→Shoot→Drain→Apply→PostEditWait→Assert loop
  drives all per-gate flows except Standard + Resize.
- The 6 `app_args.as_deref().is_some_and(|a| a.<flag>)` fast-path
  branches in `driver.rs:475-577` collapse to ONE `match
  active_gate.kind()` at the top of the system body.

**Verification (after the single big edit lands):**
- `cargo build --workspace`.
- `cargo test --workspace --lib`.
- ALL gates ≥2× (≥3× for `--vox-gpu-oracle`):
  - `cargo run --bin e2e_render` (baseline)
  - `cargo run --bin e2e_render -- --resize-test` (on Hyprland —
    the impl agent on a different compositor flags this as a
    skip-with-environment-justification)
  - `cargo run --bin e2e_render -- --oasis-edit-visual`
  - `cargo run --bin e2e_render -- --small-edit-visual`
  - `cargo run --bin e2e_render -- --small-edit-repro`
  - `cargo run --bin e2e_render -- --vox-gpu-construction`
  - `cargo run --bin e2e_render -- --vox-gpu-oracle`
  - `cargo run --bin e2e_render -- --vox-web-parity`
  - `cargo run --bin e2e_render -- --vox-horizon-native`
  - `cargo run --bin e2e_render -- --vox-e2e`
- Compare PNGs from before/after pixel-by-pixel via `--ssim-compare`:
  every gate's PNG should be SSIM ≥ 0.99 against its pre-refactor
  baseline (the refactor is structural; pixels should be identical
  modulo TAA/GI shimmer).

#### Step 5 — CLI dispatch refactor: `bin/e2e_render.rs` ladder → `parse_e2e_command` match

**Edits:**
- `crates/bevy_naadf/src/bin/e2e_render.rs:67-481` — replace the
  ladder with `parse_e2e_command(args) -> Command` + a single `match`.
- Extract `parse_gate_args`, `parse_post_app_validations`,
  `run_post_app_validations`, `install_resize_test_windowrule_if_needed`,
  `cleanup_resize_test_windowrule_if_needed` as helpers.
- `bin/e2e_render.rs` shrinks from 481 → ~250 LOC.

**Rationale:** The ladder is the most-visible quality-of-life rot in
the binary. After step 4 lands the gate abstraction, this step is
mechanical translation.

**Post-step state:**
- Adding a new gate is now: (1) add a `Gate` impl in its module, (2)
  add a row to the `parse_gate_args` table, (3) add a `GateKind`
  variant. Three small edits in three places, not 18 in one giant
  ladder.

**Verification:**
- `cargo build --workspace`.
- All gates ≥2× (≥3× for `--vox-gpu-oracle`).
- `cargo run --bin e2e_render -- --validate-gpu-construction` (proves
  post-app validation tail still works).
- `cargo run --bin e2e_render -- --edit-mode` (proves post-app tail).
- `cargo run --bin e2e_render -- --runtime-edit-mode` (proves
  post-app tail).
- `cargo run --bin e2e_render -- --entities` (proves post-app tail).
- `cargo run --bin e2e_render -- --ssim-compare <a.png> <b.png>`
  (proves the no-boot short-circuit still works).

### 4. What stays / what changes / what's removed

**Stays unchanged (inside D6 scope):**

| path | reason |
|---|---|
| `e2e/checks.rs` (183 LOC) | Load-bearing PipelineCache scan; calibrated. |
| `e2e/framebuffer.rs:1-369` (most of it) | User-tuned thresholds, calibrated predicates (`check_not_degenerate`, `check_luminance_alive`, `mean_pixel_delta`, `region_luminance`, …). Only `save_in_screenshots_dir` is added. |
| `e2e/gates.rs` (813 LOC) | Camera transform fns + per-batch region gates + `CURRENT_BATCH`. User-tuned. |
| `e2e/readback.rs` (38 LOC) | `Screenshot::primary_window` + observer. |
| `e2e/ssim.rs` (229 LOC) | SSIM wrapper around `image_compare`. |
| `e2e/tracing_error_counter.rs` (112 LOC) | Custom `Layer<Registry>` for the `tracing_error_count` metric. |
| `e2e/vox_e2e.rs` (699 LOC) | Synthesised-fixture gate — pure standard flow. Only the `Gate` impl is new. |
| `e2e/oasis_edit_visual.rs` constants (lines 56-157) | Frame budgets + thresholds — user-tuned. |
| `e2e/small_edit_visual.rs` (most of it) | The CPU snapshot helpers (`count_non_empty_voxels`, edit-apply, asserts) — load-bearing. |
| Per-gate camera-pose constants (HORIZON_*, PARITY_*, ORACLE_*, VOX_GPU_CONSTRUCTION_*, SMALL_EDIT_REPRO_CAM_*) | Calibrated; gates' `Gate::camera_pose` reads them. |
| `e2e/tests/wasm-smoke.spec.ts`, `vox-loading.spec.ts`, `vox-horizon-parity.spec.ts`, `sw-chrome-extension.spec.ts` | Live Playwright tests; D6 doesn't touch them (except the sentinel-grep cleanup in `vox-horizon-parity.spec.ts:122-187` — optional, deferred). |
| `e2e/playwright.config.ts` (66 LOC) | Channel `chrome`, headed-only — per user memory binding. |
| `e2e/helpers/console-collector.ts` | Shared TS helper. |
| `bin/e2e_render.rs` `--ssim-compare`, `--vox-gpu-oracle`, `--vox-web-parity` top-level commands | Logic preserved (structure refactored). |
| `bin/e2e_render.rs:419-478` — the 4 post-app validation tails | Behaviour preserved (helper extraction only). |

**Changes (inside D6 scope):**

| path | change |
|---|---|
| `e2e/mod.rs` | `add_e2e_systems` collapses 92→~35 LOC. PBR mod entries removed. |
| `e2e/driver.rs` | `enum E2ePhase` 49→20 variants. Body 1 240→~250 LOC. New `GateCaptures` + `GateAuxState` resources. |
| `e2e/framebuffer.rs` | Add `save_in_screenshots_dir`. |
| `e2e/oasis_edit_visual.rs` | Add `impl Gate`. Del `pin_oasis_camera`, `save_oasis_screenshot`. |
| `e2e/small_edit_visual.rs` | Add `impl Gate`. Del `pin_small_edit_camera`, `save_small_edit_screenshot`. |
| `e2e/small_edit_repro.rs` | Add `impl Gate`. Del `pin_small_edit_repro_camera`, `save_small_edit_repro_screenshot`. |
| `e2e/vox_gpu_construction.rs` | Add `impl Gate`. Del `pin_vox_gpu_construction_camera`, `save_vox_gpu_construction_screenshot`. |
| `e2e/vox_gpu_oracle.rs` | Add `impl Gate`. Del `pin_vox_gpu_oracle_camera`, `save_oracle_screenshot`. |
| `e2e/vox_web_parity.rs` | Add `impl Gate`. Del `pin_vox_web_parity_camera`, `save_parity_screenshot`. |
| `e2e/vox_horizon_parity.rs` | Add `impl Gate`. Del `pin_vox_horizon_camera`, `save_horizon_screenshot`. Constants `HORIZON_CAMERA_POS`/`HORIZON_CAMERA_ROT` reference D3's new home (post D3 F6 — interim, the import line changes from `crate::e2e::vox_horizon_parity` to the new path). |
| `e2e/vox_e2e.rs` | Add `impl Gate`. (No pin to delete — vox_e2e uses Standard pose.) |
| `bin/e2e_render.rs` | Ladder→`parse_e2e_command` match. -240 LOC. Remove `--device-snapshot-native`. |

**Removed (inside D6 scope, including coordinated deletions):**

| path | LOC | reason |
|---|---|---|
| `e2e/pbr_debug_modes.rs` | 218 | Resolution C — orphan, master is C# port. |
| `e2e/pbr_hard_edge.rs` | 1 023 | Resolution C. |
| `e2e/pbr_visual.rs` | 747 | Resolution C. |
| `bin/diag_compare.rs` | 314 | Resolution A — D7 deletes the producer. |
| `e2e/tests/device-snapshot.spec.ts` | 122 | Resolution A. |
| `Cargo.toml [[bin]] diag_compare` | 7 | Resolution A. |
| `justfile` recipes `diag-native`, `diag-web`, `diag-compare`, `diag` | ~25 | Resolution A. |
| `e2e/oasis_edit_visual.rs::save_oasis_screenshot` + `pin_oasis_camera` | ~30 | DUP-6 + DUP-4. |
| `e2e/small_edit_visual.rs::save_small_edit_screenshot` + `pin_small_edit_camera` | ~30 | Same. |
| `e2e/small_edit_repro.rs::save_small_edit_repro_screenshot` + `pin_small_edit_repro_camera` | ~30 | Same. |
| `e2e/vox_gpu_construction.rs::save_*` + `pin_*` | ~30 | Same. |
| `e2e/vox_gpu_oracle.rs::save_oracle_screenshot` + `pin_vox_gpu_oracle_camera` | ~30 | Same. |
| `e2e/vox_web_parity.rs::save_parity_screenshot` + `pin_vox_web_parity_camera` | ~30 | Same. |
| `e2e/vox_horizon_parity.rs::save_horizon_screenshot` + `pin_vox_horizon_camera` | ~30 | Same. |
| `driver.rs::pin_resize_test_camera` | ~12 | Subsumed by `ResizeGate::camera_pose`. |
| `OasisEditVisualState`, `SmallEditVisualState`, `SmallEditReproState`, `VoxGpuOracleState`, `VoxWebParityState` Resource defs | ~60 total | Subsumed by `GateCaptures` + `GateAuxState`. |
| `driver.rs::E2ePhase` 30 per-gate variants (Oasis*, SmallEdit*, SmallEditRepro*, VoxGpuOracle*, VoxWebParity*) | ~80 | Subsumed by the generic 6 phases. |
| `driver.rs` per-gate match arms | ~600 | Subsumed by the generic loop. |
| `driver.rs:475-577` — 6 fast-path route-in blocks | ~100 | Subsumed by `match active_gate.kind()` at the top of `e2e_driver`. |

### 5. Open conflicts

**None.**

All 10 explorer findings + 2 cross-domain coordination items resolve
within the design. The `device_snapshot` chain deletion is user-confirmed
(addendum Resolution A). PBR deletion is user-confirmed (addendum
Resolution C). The architect did not propose any forbidden moves
(no behavioural divergence from C# beyond what user already approved
at the addendum level).

---

## Decisions & rejected alternatives

### D1 — `Box<dyn Gate>` vs `enum Gate` (sum type)

**Chosen**: `trait Gate` + `Box<dyn Gate>` (object-safe dyn).

**Considered**: An `enum Gate { Standard, Resize, OasisEdit(OasisEdit),
… }` sum type with each variant's data inline.

**Rejected because**: the sum-type variant would force every `Gate`
method to be a `match self { … }` table — exactly the
per-gate-inlining pattern this refactor exists to eliminate. The
`dyn`-trait approach lets each gate's module own its impl block;
adding a new gate is a 1-file change. The `Send + Sync + 'static`
bounds are satisfied by every existing gate (all-Resource state).

**Cost**: one `Box` allocation per app boot. Negligible.

### D2 — Standard and Resize as named exceptions

**Chosen**: 2 of the 8 gates do NOT use the generic Warmup→Shoot→
Drain→Apply→PostEditWait→Assert loop. Standard keeps its Motion+Settle
inline; Resize keeps its 11-variant state machine.

**Considered**: forcing Resize into the generic shape by making it
"3 captures, no edit, 3 inline `Shoot/Drain` cycles."

**Rejected because**: Resize's `hyprctl` window-resize dispatch is
structurally a 3-shot fan-out the other gates don't have. Forcing it
into the generic shape would require parameterising `FrameBudget` over
`Vec<u32>` warmup values + a `CaptureSlot` enum + a 3-place `GateCaptures`
— each abstraction increasing every other gate's complexity for one
gate's special case. **Standard** has the orbit camera, which uses a
`t`-parameterised pose that no other gate needs. The cost of carving
Standard + Resize out is 11 named variants total — acceptable.

### D3 — Per-gate frame budget consts stay where they are

**Chosen**: each gate's `Gate::frame_budget()` reads its existing
per-gate consts (OASIS_WARMUP_FRAMES, ORACLE_WARMUP_FRAMES, …).

**Considered**: collapsing the per-gate consts into a single shared
budget table at the e2e/mod.rs level.

**Rejected because**: the values are calibrated per gate
(oasis 120-frame warmup ≠ vox-gpu-oracle 60-frame warmup, by deliberate
design per explorer side-note 4 — "the threshold values were tuned
by the user"). Centralising them would invite changes that break
calibration. Keep the consts at their declaration sites.

### D4 — `AppArgs` flag-bag stays for D6's phase

**Chosen**: D6 introduces `enum GateKind` (in `e2e/gate.rs`) but does
NOT remove the 11 mode/phase booleans from `AppArgs`.

**Considered**: replacing the 11 booleans with
`AppArgs.e2e_gate: GateKind` outright in D6's impl phase.

**Rejected because**: `AppArgs` is D7's territory (`lib.rs:283-462`).
D6 can land its design without touching D7's struct shape. The
`ActiveGate` resource is set by the e2e harness from the legacy
boolean reads at `add_e2e_systems` time; once D7's impl phase
introduces the `e2e_gate` enum, the wiring shifts to read the enum.
Layered, low-risk migration.

### D5 — Diagnostic short-circuits (`--validate-gpu-construction-scaled`, `--validate-gpu-construction-production`) stay in `bin/e2e_render.rs`

**Chosen**: keep them in the binary for now.

**Considered**: extracting them into a separate `bin/diag_construction.rs`
binary so e2e_render's surface is gate-only.

**Rejected because**: they're 2-3 LOC each at the binary level (a
flag parse + a fn call + an ExitCode return); extracting them is
churn for no real win. D5's architect may reshape them as part of
the `validate_*` extraction — D6 stays out of D5's way.

---

## Assumptions made

1. **D7 ships its production-side `device_snapshot` deletion AFTER D6
   ships the e2e-side**. The orchestrator's sequencing is D6 before
   D7; the architect respects it. If D7 lands first, `bin/diag_compare.rs`
   would error-out the build (it references `diagnostics::device_snapshot::
   DeviceSnapshot` types). The implementor MUST land D6 step 1 first.
2. **D3 chooses option 2 for its F2** (cfg-test the tiled helpers,
   keep the `--vox-gpu-oracle` gate). If D3 deletes the gate instead,
   D6's design adapts trivially: remove the `VoxGpuOracle` `GateKind`
   variant + `vox_gpu_oracle.rs` `impl Gate` + delete the
   `vox_gpu_oracle.rs` file.
3. **D3 picks a non-e2e home for the horizon-camera constants
   (F6)** — either `camera/poses.rs` or `voxel/grid.rs`. D6's
   `vox_horizon_parity.rs` updates its `use` line accordingly; the
   constants themselves move OUT of D6.
4. **The implementor uses fish-shell-compatible paths**. The brief's
   note about cwd-resets between bash calls applies — every cited
   path in this design is absolute under `/mnt/archive4/DEV/bevy-naadf/`.
5. **The `vox_e2e_mode` and `entities_mode` flags in `e2e_driver`'s
   `Assert` phase** (driver.rs:679-688) remain as post-app
   compositional flags — they alter the Standard gate's assertion
   surface, not the gate selection. The design preserves this:
   `StandardGate::assert` reads `AppArgs.vox_e2e_mode` and
   `AppArgs.spawn_test_entity` to pick the right region gate.

## D7 / D3 coordination notes

### D7 coordination (the `lib.rs` side)

D6's design depends on D7 for two changes:

1. **`AppArgs.e2e_gate: GateKind` (eventual)** — D7's impl phase
   introduces a single enum field replacing the 11 booleans. D6
   accommodates this by reading `args.e2e_gate` instead of the 11
   `args.<flag>` calls inside `parse_gate_args`. **D7 should add
   `GateKind` re-export at the lib root** so the binary can name it
   without an `e2e::` qualifier.
2. **`run_e2e_render_with_args` window-config switching (D7 F3)** —
   D6 doesn't touch `lib.rs:993-1022`. D7's design can collapse the
   mode→window-config mapping into a `WindowConfig::for_gate(gate:
   GateKind) -> WindowConfig` helper.

D6's impl phase lands its parts independently. D7's later impl phase
absorbs them via the `AppArgs.e2e_gate` introduction.

### D3 coordination (horizon-camera constants)

`vox_horizon_parity.rs:71-82` defines `HORIZON_CAMERA_POS` +
`HORIZON_CAMERA_ROT` constants currently imported by `voxel/web_vox.rs:287-288`
and `voxel/grid.rs:571-572` (per D3 F6). The dependency arrow
inverts in D3's design: constants move OUT of e2e/ into D3's
domain (D3 architect chose `voxel/grid.rs` or `camera/poses.rs`).
D6's `vox_horizon_parity.rs` then `use`s them from the new home.

**Coordination order**: D3 architect lands the move; D6 implementor's
step 3 updates the `use` line in `vox_horizon_parity.rs`. If D3
hasn't landed when D6 step 3 runs, leave the import as-is
(`pub` constants remain at their D6 location) and let D3's impl
delete the originals later.

### D5 coordination (`validate_gpu_construction*` extraction)

D5's design at `render/construction/` may extract `validate_gpu_construction`,
`validate_gpu_construction_scaled`, `validate_gpu_construction_production_scale`
into a `render/construction/validation/` submodule. D6's
`bin/e2e_render.rs` calls these via `bevy_naadf::render::construction::
validate_*`. If D5 reorganises the module path, D6's binary's
`use` lines update accordingly — mechanical, low-risk.

D6 deliberately does NOT propose moving these functions out of the
binary's surface — they're D5's territory. D6 keeps the call sites
narrow (one `Option<Validation>` enum + one `run_validation` helper)
so D5's rename touches one line.

---

## Side notes / observations / complaints

1. **The PBR deletion is much smaller than expected at the
   wiring-edits level**. The PBR files are already orphaned — `pub mod`
   entries are gone from `e2e/mod.rs`, no `--pbr-*` flags exist in
   `bin/e2e_render.rs`. The delete is pure `rm` for 3 files; zero
   import-edits, zero registration-edits. The 1 988 LOC win is
   real but the user already did the structural part. The architect
   notes this so the implementor doesn't waste cycles hunting
   for ghost dispatch entries that don't exist.

2. **`vox-horizon-parity.spec.ts:122-187`'s `[device-snapshot]`
   sentinel-grep is non-load-bearing** — the spec collects
   the sentinel into its annotation block for diagnostic context,
   not as a load-bearing assertion. After D7 deletes the producer,
   the matcher just won't match anything. Leaving it in place is
   fine; deleting the 4 grep blocks is a 30-LOC optional cleanup
   the implementor may bundle with step 1 or defer entirely. The
   design treats it as deferrable.

3. **`bin/e2e_render.rs:213-239`'s two `validate_gpu_construction*`
   short-circuits look like D5's territory but currently live in
   D6's binary**. The brief implies D6 may move them; the architect
   judged not to — they're tiny (3-4 LOC each at the dispatch
   level), and D5's reorganisation may move the callees but the
   callers stay simple. Future D5 work shouldn't have to fight a
   D6 reorganisation here.

4. **The `Standard` gate carries the orbit camera + the per-batch
   region gates + the entity-pixel gate + the vox_e2e geometry
   gate**. That's a LOT for "the default gate", but they're all
   readback-format-aware framebuffer checks; consolidating them
   into one `Gate` impl is consistent. The architect notes this
   so the implementor knows `StandardGate::assert` is going to
   contain `run_assertions(...)` from `driver.rs:1849-1956` (110
   LOC) — that's by design, not bloat.

5. **The brief framed `oasis_edit_visual` and `vox_gpu_oracle` as
   the non-deterministic gates needing ≥2 runs**. Verified: those
   gates have W2 GPU dispatch propagation + GI/TAA convergence
   (Oasis) and atomic-cursor nondeterminism (VoxGpuOracle).
   `small_edit_visual` has the same W2+W3+TAA convergence as Oasis;
   `vox_web_parity` has the loaded vs skybox SSIM compare which is
   stochastic too. The architect recommends the implementor run
   ALL non-Standard gates ≥2× as the safer default, not just the
   2 the brief named.

6. **The `Single<...>` system-parameter shape `&mut Transform, &mut
   PositionSplit, With<Camera3d>` is the canonical e2e camera
   mutation pattern** — used at 9+ sites (the 7 `pin_*_camera`
   systems + the driver's 4 inline writes). The design preserves it
   in `pin_active_gate_camera`. If the project ever upgrades Bevy to
   a version with a different `Single` API, the design changes ONE
   place.

7. **Equal-footing complaint**: the explorer's `02-exploration.md`
   side-note 2 flagged that `00-reuse-audit.md §3.2 DUP-6`'s
   description was incomplete (it mentioned `camera-history` which
   isn't actually written by the e2e pin systems). Verified — the
   audit was slightly wrong; the design correctly scopes DUP-6 to
   pose + PositionSplit + the args-gate preamble. The audit's
   "~270 LOC" estimate for DUP-6 also turned out to overstate the
   raw boilerplate — actually closer to ~210 LOC across the 7 pin
   systems plus the 4 driver-inline sites. The structural win is
   still big: ONE pin system instead of 8 named ones + the chain
   ordering.

8. **Subjective**: this is the cleanest refactor in the 8-domain
   set. The e2e harness was always going to refactor well because
   every gate already follows the same shape; the explorer's job
   was to find the seams, not to invent abstractions. The `Gate`
   trait + the generic loop is the obvious move; the only real
   design call was Standard + Resize as named exceptions. D6 lands
   in 5 atomic steps with explicit verification gates after each.

9. **What's load-bearing about D7's later impl phase consuming this
   design**: D7 will need a `bevy_naadf::e2e::gate::GateKind`
   re-export at the lib root, plus the `parse_e2e_command(&args)
   -> Command` function would ideally move into `lib.rs` (so both
   `main.rs` and `bin/e2e_render.rs` can use it). The architect
   doesn't ship that move in D6 — D6 keeps `parse_e2e_command` in
   the binary — but flags it for D7's architect.

10. **No new `Cargo.toml` deps**. The design uses only Bevy-built-in
    abstractions (`Single`, `Res`, `MessageWriter`, `Resource`, the
    `Update` schedule) + `image_compare` / `image` / `tracing` /
    `serde_json` already present. Per the explorer's confirmed
    audit suspicion 3, no new third-party crate is needed.
