# D7 — app-and-camera exploration

**Date**: 2026-05-20
**Scope**: `crates/bevy_naadf/src/{lib,main}.rs`, `crates/bevy_naadf/src/camera/{mod,position_split}.rs`, `crates/bevy_naadf/src/diagnostics.rs`.
**Empirical LOC** (verified `wc -l`): lib 1146 + main 54 + camera/mod 287 + camera/position_split 177 + diagnostics 711 = **2 375 LOC**. (Brief said main = 75; actual = 54. Audit said camera total 464 → 287 + 177 = 464 OK.)

This explorer was told D7's implementor lands LAST (after D1–D6, D8). Findings below are framed as "target shape D7 needs once the other domains have settled their plugins."

---

## Findings

### F1 — `build_app_with_args` is a monolithic 336-LOC system-registration ladder (severity: high)

**Location**: `crates/bevy_naadf/src/lib.rs:638-973`.

**Current state.** A single `pub fn` body with ten distinct sections, separated only by comments, that:
1. inserts `DlssProjectId` (lines 647-650),
2. assembles a `Window` struct + wasm overrides (lines 653-674),
3. inserts `AppArgs` + `CameraHistory` + the `DefaultPlugins` chain with three `.set()` overrides (lines 679-735),
4. adds the seven currently-extracted plugins as a tuple (lines 737-764),
5. branches on `cfg.add_free_camera` to wire `FreeCameraPlugin` + three camera systems with `.chain()` (lines 770-784),
6. branches on `!cfg.add_e2e_systems` to wire `DiagnosticsPlugin` (788-790),
7. unconditionally wires `DeviceSnapshotPlugin` (line 799 — see F7),
8. wires `load_dev_font` + `setup_test_grid` + the async-vox pump + the wasm-only `web_vox` startup + native dnd listener (lines 803-864),
9. branches on `args.spawn_test_entity` to wire the W4 fixture-entity startup (lines 874-876),
10. branches on `cfg.add_e2e_systems` vs `cfg.add_hud` to wire (a) the e2e systems OR (b) `setup_camera` + the camera-history update + the HUD/editor/settings/`AppMode` state machine block (lines 877-971).

The 71-line `if cfg.add_hud { … }` block alone (`lib.rs:900-971`) initialises **3 resources** (`SettingsState`, `SettingsDrag`, `EditorState`), 1 state (`AppMode`), and wires **15 systems** across `Startup`/`OnEnter`/`OnExit`/`Update` with hand-written `.after()` ordering, hand-written `.chain()`, and 4 `.run_if(in_state(...))` predicates. None of these systems belong to `lib.rs` semantically — every one is owned by `editor/` (D2), `settings.rs` (D2), `hud.rs` (D2), or `app_mode.rs` (D2/D7-cross).

**Why it's a problem.**
- Adding a system means editing one of 20+ specific lines deep inside this function and getting the `.after()` / state condition / `.chain()` placement exactly right. Each of D2/D3/D5/D6/D8's refactors needs to *touch this central registry* if they reorganise their own systems. That is the *opposite* of the IoC seam `01-context.md` Q1 asks for.
- The function violates the seam already established by `WorldPlugin`, `NaadfRenderPlugin`, `ConstructionPlugin`, `BakedMaterialPlugin`, `TextureArrayPlugin`, `DiagnosticsPlugin`, `DeviceSnapshotPlugin` — those seven plugins exist *precisely so* their authoring crates own their systems. The remaining ~336 LOC of inline `add_systems` are precisely the systems whose `Plugin`s have not been extracted yet.
- The `cfg.add_hud / cfg.add_free_camera / cfg.add_e2e_systems` ladder is correctly described as four deliberate e2e deltas (`AppConfig` doc at `lib.rs:570-592`), but the *implementation* of those deltas is scattered across **5 different `if`-branches** in `build_app_with_args` rather than collected on the `AppConfig::e2e()` vs `AppConfig::windowed()` side. The data-vs-code separation is broken.

**Suggested direction (NOT a design).**
Pull every inline `add_systems(...)` into a Plugin owned by the relevant domain's module:
- D2: `EditorPlugin` (`editor::EditorPlugin`) — owns `EditorState`, `setup_editor_hud`, `refresh_palette_swatches`, `handle_hud_clicks`, `scroll_palette_with_wheel`, `drag_palette_scrollbar`, `update_palette_scrollbar`, `update_editor_hud`, `apply_edit_tool`. Self-gated by reading the `AppMode` state inside `.run_if`.
- D2: `SettingsPlugin` — owns `SettingsState`, `SettingsDrag`, `setup_settings`, `show_settings`, `hide_settings`, `adjust_settings`, `mouse_interact_settings`, `update_settings_text`. Self-gated on `AppMode::Settings`.
- D2 (or D7): `HudPlugin` — owns `HudText`, `setup_hud`, `update_hud`.
- D7 own: `AppModePlugin` — owns `AppMode` state init + `toggle_settings_on_escape` + `suspend_camera_input` / `restore_camera_input` `OnEnter`/`OnExit` schedules. This is the cross-cutting glue between `editor` and `settings` so it stays in D7.
- D7 own: `CameraPlugin` — owns `setup_camera` + `sync_position_split` + `apply_initial_camera_pose_changes` + `toggle_dlss` + `update_camera_history` (currently in D4's `render/taa.rs` but it's a *main-world* system writing the `CameraHistory` resource from `Transform`, semantically a camera-side concern — D4's architect needs to agree).
- D7 own: `DevFontPlugin` (or fold into `HudPlugin`) — owns `ROBOTO_REGULAR_BYTES` + `DevFont` + `load_dev_font`.
- D7 own: `Phase-C-test-entity` startup belongs with D5's `ConstructionPlugin` (it populates `MainWorldEntities`); `lib.rs:1052-1095`'s `spawn_phase_c_test_entity` should move to `render/construction/` and be self-gated on `args.spawn_test_entity` via `.run_if`.

`AppConfig`'s four delta flags then collapse to a single Bevy idiom: each gated plugin is added unconditionally; the *plugin* internally reads `Res<AppConfig>` (insert it as a resource before `add_plugins`) and self-skips when it shouldn't run. OR — preserving the existing config-pattern — each plugin is added conditionally at the *single* `build_app` site, but their bodies are entirely owned by their authoring module.

Post-refactor target shape (architect refines):
```rust
pub fn build_app_with_args(cfg: AppConfig, args: AppArgs) -> App {
    let mut app = App::new();
    app.insert_resource(args.clone())
        .insert_resource(cfg)
        .add_plugins(default_plugins_for(&cfg, &args))     // ~30 LOC helper
        .add_plugins((
            DevFontPlugin,
            WorldPlugin,
            NaadfRenderPlugin,
            ConstructionPlugin,
            BakedMaterialPlugin,
            TextureArrayPlugin,
            VoxelIoPlugin,            // new D3 owner — `setup_test_grid` + async pump + web_vox + dnd
            CameraPlugin { add_free_camera: cfg.add_free_camera },
            DiagnosticsPlugin,        // press-P only — no longer wired only outside e2e (Plugin internally `.run_if(no_e2e)`)
        ));
    if cfg.add_hud { app.add_plugins((HudPlugin, AppModePlugin, EditorPlugin, SettingsPlugin)); }
    if cfg.add_e2e_systems { app.add_plugins(E2ePlugin); }
    app
}
```
That's ~20 LOC + ~10 LOC of `default_plugins_for`. The remaining ~310 LOC migrates *into* the owning crates.

**Out-of-scope ripple.**
- Every other domain's implementor must land their `Plugin` (D2's `EditorPlugin` / `SettingsPlugin` / `HudPlugin`, D3's voxel-io plugin, D5's `spawn_phase_c_test_entity` re-home) before D7 can call them. The brief acknowledges this ("D7 lands LAST").
- D4's `render::taa::update_camera_history` reads `Transform` from the main-world camera entity — moving it under `CameraPlugin` is a cross-domain rename. D4's architect must agree (or D7 leaves it where it is and just adds the `.after(sync_position_split)` ordering elsewhere).
- `voxel::grid::setup_test_grid` registration is currently in `lib.rs:814` with the wasm `startup_fetch_default_vox.before(setup_test_grid)` ordering at line 840. D3's architect already proposes a voxel-io plugin owning this — D7 just deletes the inline lines.

---

### F2 — `GiSettings` has triple-canonical authoring across lib.rs + settings.rs + gpu_types.rs (severity: high)

**Location**:
- Canonical Rust struct definition: `crates/bevy_naadf/src/lib.rs:108-185` (declared in `lib.rs`).
- KNOBS function-pointer table that exposes the fields to the settings UI: `crates/bevy_naadf/src/settings.rs:172-219` (D2 owns).
- GPU uniform mirror fields: `crates/bevy_naadf/src/render/gpu_types.rs:87` (`GpuRenderParams.max_ray_steps_primary`), `gpu_types.rs:539` (`GpuGiParams.spatial_iter_count`) (D4 owns).
- Default values quadrupled: `lib.rs:191-228` (struct defaults) + `settings.rs:174,184,194,202,210` (KNOBS row defaults) + WGSL `ray_tracing.wgsl:122-126` (`MAX_RAY_STEPS_*` consts) + the doc-comments at `lib.rs:223-228` that also state them in prose.

**Why it's a problem.**
- This is the audit's **SSoT-1**. 5 source-of-truth locations for the same 5 numbers (`120/100/120/80/60`). Cross-checked: every site DOES agree at HEAD, but the only enforcement is the unit tests at `settings.rs:898-904` (`KNOBS_RAY_STEPS_*_DEFAULTS_AGREE`-style asserts), which pin KNOBS defaults to `GiSettings::default()` defaults — they do NOT pin the WGSL consts or the doc-comments. So 3 of the 5 sites are unguarded.
- D7 owns the canonical *struct*. D2 owns the *KNOBS adapter*. D4 owns the *GPU uniform layout*. None of them on their own can fix the divergence — it requires a coordinated change.
- Putting `GiSettings` in `lib.rs` (the app spine) rather than in `settings.rs` or its own module sends the wrong architectural signal: every domain that touches it now imports from the spine. `lib.rs:34` already shows `editor::hud`, `settings`, etc. importing it as `crate::GiSettings`.

**Suggested direction (NOT a design).**
- Move `GiSettings` and `Default for GiSettings` out of `lib.rs` into its own home — most naturally `settings.rs` (it IS the settings-knob payload), but a new `src/gi_settings.rs` is also clean. D7's group note (below) coordinates with D2 on the new home.
- Express the WGSL `MAX_RAY_STEPS_*` consts as shader-defs sourced from `GiSettings::default()` at pipeline specialisation time, OR delete those consts entirely once the uniform plumbing is universal (D4 architect's call). The doc-comment values in `lib.rs:152-184,219-228` then become single-source.
- Treat the KNOBS table as a *view* over `GiSettings` and bind the defaults to `GiSettings::default()` at compile time via a builder/macro — this is D2's `BEV-4` reflect-driven settings restructure. D7's piece is just "make the struct movable."

**Out-of-scope ripple.**
- Every `extract.rs:470`, `prepare.rs:843`, `gi.rs:383`, etc. import becomes `use crate::settings::GiSettings;` instead of `use crate::GiSettings;`. Trivial, but ~15 sites.
- D4's `gpu_types.rs` SSoT-1 fix has to land before D7 can claim SSoT-1 is closed.

---

### F3 — `WindowConfig` constructor ladder + scattered window-config switching in `run_e2e_render_with_args` (severity: medium)

**Location**:
- `WindowConfig` constructors: `crates/bevy_naadf/src/lib.rs:485-568` (4 constructors: `windowed()`, `e2e()`, `e2e_horizon()`, `e2e_resize_test()`).
- Window-config switching: `crates/bevy_naadf/src/lib.rs:993-1022` (`run_e2e_render_with_args` mutates `cfg.window` based on which `AppArgs` flag is set: `resize_test → e2e_resize_test`, `small_edit_repro_mode → ad-hoc inline construction`, `vox_horizon_native_phase → e2e_horizon`).

**Why it's a problem.**
- The mode→window-config mapping is hand-written in 3 different if-branches at the call site, not in `WindowConfig`'s impl block. `small_edit_repro_mode` even inlines a `WindowConfig { resolution: Some((..)), … }` literal at lines 1006-1015 — a fifth window config the `WindowConfig::` constructor surface doesn't have a name for.
- This is the audit's "ladder of mode-specific window sizes" pattern. `AppArgs` already has 9+ `*_mode` / `*_phase` flag fields driving e2e dispatch (`lib.rs:336-438`); the window-config mapping is a derived view over those flags but it lives separately.
- New e2e modes (and the project has added many — 11 distinct AppArgs flags) now require *two* edits: one to add the flag, one to add the window-config branch.

**Suggested direction (NOT a design).**
- Add the small-edit-repro inline literal as a fifth named `WindowConfig::e2e_small_edit_repro()` constructor — minimal, source-stable.
- Consider a `fn window_for_e2e_args(args: &AppArgs) -> WindowConfig` that owns the mode→window-config mapping in one place. `run_e2e_render_with_args` then becomes `cfg.window = window_for_e2e_args(&args)`.
- Or simpler still: each e2e mode flag carries a `WindowConfig` in its driver-side module (each mode is its own `e2e/<mode>.rs` already — push the constant there), and the dispatch logic at `lib.rs:996-1022` becomes a single match.

**Out-of-scope ripple.**
- None — `WindowConfig` is D7-internal. The e2e-mode flags it switches on are already in `AppArgs`.

---

### F4 — `AppArgs` carries 13 `_mode` / `_phase` flags that drive e2e dispatch + 3 production flags, with no structural separation (severity: medium)

**Location**: `crates/bevy_naadf/src/lib.rs:283-462`.

**Current state.** `AppArgs` has 19 fields. Counting:
- **Production-meaningful**: `grid_preset`, `taa`, `taa_ring_depth`, `gi`, `construction_config` (5 fields — the actual app configuration).
- **Phase-C test fixture**: `spawn_test_entity` (1 field — used in both production binary and e2e).
- **E2E-mode flags**: `resize_test`, `vox_e2e_mode`, `oasis_edit_visual_mode`, `small_edit_visual_mode`, `small_edit_repro_mode`, `vox_gpu_construction_mode`, `vox_gpu_oracle_cpu_phase`, `vox_gpu_oracle_gpu_phase`, `vox_web_parity_skybox_phase`, `vox_web_parity_loaded_phase`, `vox_horizon_native_phase` (11 flags — each is set true by exactly one e2e dispatch branch in `bin/e2e_render.rs` and read by the corresponding e2e module).

`AppArgs::default()` (lines 441-462) initialises all 11 flags to `false`. Every production launch of `main.rs` carries 11 dead booleans.

**Why it's a problem.**
- The struct mixes app-configuration with e2e-dispatch-marker fields. Type-level honesty: a production `main.rs` literally cannot meaningfully set any of `vox_e2e_mode..vox_horizon_native_phase`.
- Adding an e2e mode means widening `AppArgs` (every consumer sees the new field), updating `Default`, updating `bin/e2e_render.rs` to set it, and (per F3) updating `run_e2e_render_with_args` to map it to a window config. This is a multi-file touch for what should be a one-file change.
- The `_mode` / `_phase` naming asymmetry across 11 flags is itself a smell: some are described as "modes" (Mode 2 logic) and others as "phases" (subprocess phases of a parent comparison gate). The audit's domain-coupling argument is that these belong in `e2e/`, not in `lib.rs`.

**Suggested direction (NOT a design).**
- Split into `AppArgs` (the 6 production-meaningful fields) + `E2eMode` enum (the 11 mode/phase flags, as variants of one enum since at most one is ever true).
- `e2e/` then owns `E2eMode` and the dispatch from `bin/e2e_render.rs` becomes `match parse_e2e_mode(args) { … }` → `run_with_app(build_app_with_args(cfg, args, mode))`.
- This is also D6's territory (the e2e harness owns mode dispatch), so D7's architect coordinates with D6.

**Out-of-scope ripple.**
- `bin/e2e_render.rs` mode-dispatch (D6) needs to be rewritten to match-on the enum.
- The 11 `e2e/<mode>.rs` modules each read one of those flags off `AppArgs` (e.g. `e2e/oasis_edit_visual.rs` reads `args.oasis_edit_visual_mode`) — they migrate to reading off the enum.

---

### F5 — `SSoT-2`: WORLD_SIZE_IN_SEGMENTS/CHUNKS/VOXELS hand-derived 3× with test enforcement (severity: medium)

**Location**: `crates/bevy_naadf/src/lib.rs:241-260` (constants) + `lib.rs:1131-1145` (`tests::fixed_world_size_constants_agree`).

**Current state.** Three `pub const UVec3`s are independently typed-out:
```
WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)
WORLD_GEN_SEGMENT_SIZE_IN_GROUPS = 4        // u32
WORLD_SIZE_IN_CHUNKS = (256, 32, 256)       // hand-typed = SEGMENTS * GROUPS * 4
WORLD_SIZE_IN_VOXELS = (4096, 512, 4096)    // hand-typed = CHUNKS * 16
```
The relationship is enforced *only* at unit-test time. The comment at `lib.rs:255-256` explains why: *"Hardcoded rather than computed because glam's UVec3 ops are not const."*

**Why it's a problem.**
- The hand-typing pattern is one source-of-truth + two pre-multiplied copies. Changing the world size (which the project has done historically — see `feature-completeness/01-context.md` references) means editing three places + remembering the test exists.
- Verified the constants are consumed by `voxel/grid.rs` at ~12 sites and `render/construction/mod.rs:2245,2252` — none of them divide back out from `WORLD_SIZE_IN_VOXELS`, they just consume directly. So the duplication serves no consumer.
- `glam::UVec3`'s `*` is not const, but a manual `const fn` over three `u32`s producing a `UVec3` IS const. The "not const" claim is a half-truth — there is a workaround.

**Suggested direction (NOT a design).**
- Replace with: keep `WORLD_SIZE_IN_SEGMENTS` + `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS` as the only base constants. Derive `WORLD_SIZE_IN_CHUNKS` and `WORLD_SIZE_IN_VOXELS` via a `const fn mul_uvec3(v: UVec3, k: u32) -> UVec3 { UVec3::new(v.x * k, v.y * k, v.z * k) }` helper (component-wise mul *is* const-friendly when written in terms of `u32 * u32`).
- The `fixed_world_size_constants_agree` test becomes redundant (the equality is by construction). Keep it as a sanity assertion against the C# canonical values (`UVec3::new(256, 32, 256)` and `UVec3::new(4096, 512, 4096)`) — that's the only check still needed.

**Out-of-scope ripple.**
- None. The three constants stay `pub`, the signatures stay the same.

---

### F6 — `apply_initial_camera_pose_changes` has stateful drift-detection that is hard to unit-test (severity: medium)

**Location**: `crates/bevy_naadf/src/camera/mod.rs:179-213`.

**Current state.** The system holds a `Local<Option<Transform>>` tracking the last-applied pose, and compares the live `Transform.translation`/`rotation` against it to decide whether to re-apply on a `InitialCameraPose` resource change. Thresholds: `translation_drift > 1.0`, `rotation_drift > 0.01` (squared length + angle, hardcoded magic numbers — `mod.rs:196`).

**Why it's a problem.**
- The thresholds `1.0` (squared length! so an actual move of 1 unit) and `0.01` rad are unnamed magic numbers — neither has a const, neither is doc-justified. The 1 vs 1.0 framing is also implicit (squared-length comparison).
- The `Local<Option<Transform>>` makes the system stateful and order-dependent: any test exercising it has to construct the local state correctly. The current `tests` module (`mod.rs:240-287`) tests `from_world_voxels` algebra but not the drift logic.
- Detecting "user has moved the camera since spawn" via translation/rotation deltas against a stored anchor is itself a workaround for not having a `Changed<FreeCamera>`-style flag or an explicit `UserHasMovedCamera` event. The Bevy-idiomatic alternative is to write an event whenever `FreeCamera` actually moves the camera and have this system listen for it. Until then this works but it's a smell.

**Suggested direction (NOT a design).**
- Extract `DRIFT_TRANSLATION_THRESHOLD_SQ: f32 = 1.0` and `DRIFT_ROTATION_THRESHOLD_RAD: f32 = 0.01` as named consts at the module head; doc-comment the units. Cheap.
- Architect's judgement call: replace the drift check with an explicit "camera has been moved by user" signal (event or marker component). The current 36-line drift detector then collapses to a single `if camera.has::<UserMovedCamera>() { return; }`. Out-of-scope ripple: `FreeCameraPlugin` is third-party (Bevy's), so the architect would either add a small "translator" system that converts free-camera movement events to a project-owned event, or stick with the threshold approach.

**Out-of-scope ripple.**
- If the event-driven alternative is chosen, the `FreeCamera`-movement-translator system might need a new home (D7 or D2's territory).

---

### F7 — `DeviceSnapshotPlugin` (the `device_snapshot` submodule in `diagnostics.rs`) is cited by e2e harness — user "DELETE outright" directive collides with the e2e seam constraint (severity: high — needs orchestrator/user adjudication)

**Location**:
- The submodule: `crates/bevy_naadf/src/diagnostics.rs:155-711` (557 LOC — lines 175-711, plus the section divider 155-174).
- The plugin wiring: `crates/bevy_naadf/src/lib.rs:799`.
- **Out-of-scope external callers**:
  - `crates/bevy_naadf/src/bin/e2e_render.rs:139-143` (declares `device_snapshot_native_mode`), `e2e_render.rs:364-375` (dispatches `--device-snapshot-native` mode by calling `bevy_naadf::run_e2e_render()`).
  - `crates/bevy_naadf/src/bin/diag_compare.rs` — the **entire 314-LOC binary** exists to diff two `DeviceSnapshot` JSON outputs. Module doc at `diag_compare.rs:1-21` is explicit about this.
  - `e2e/tests/device-snapshot.spec.ts` — Playwright spec that boots the WASM build and captures the `[device-snapshot]` sentinel info-line from the browser console, writing the JSON to disk.

**Why it's a problem (and why this is a constraint conflict).**

The user brief says, verbatim:

> The device-snapshot capture submodule (~560 LOC) — USER DIRECTIVE: DELETE outright (everything-else-can-go).

But `01-context.md` Q2 also says:

> Do NOT delete or rename anything cited by `e2e/`, `bin/e2e_render.rs`, or `e2e/tests/*.ts` without zero-callers verification.

`device_snapshot` IS cited by all three forbidden surfaces. The user override at the brief level postdates the audit, but the constraint at `01-context.md` Q2 also postdates the audit (it's the user's verbatim Q2 quote: "*everything else flagged as investigation residual: DELETE outright*" — including `diagnostics::device_snapshot` explicitly listed in the bulleted set at `01-context.md:53`). So the override IS the user's explicit decision — but the implementor's actual delete-list MUST also include:

- the `--device-snapshot-native` branch in `bin/e2e_render.rs:139-143` + `:364-375`,
- the entire `bin/diag_compare.rs` binary + its `Cargo.toml` `[[bin]]` entry,
- `e2e/tests/device-snapshot.spec.ts`,
- any `justfile` recipes referencing the above (architect: grep `diag-compare`),
- `docs/orchestrate/wasm-chunk-aadf-nondeterminism/` references (orchestrator's call — that orchestration is "completed" per the audit's side note #6).

**Suggested direction (NOT a design).**
The architect should:
1. Verify the delete-list is exactly: `diagnostics.rs:155-711` + `lib.rs:799` (the plugin wiring) + `e2e_render.rs:137-143,364-375` (the `device_snapshot_native_mode` declaration + dispatch branch) + the entire `bin/diag_compare.rs` file + the `[[bin]]` entry in `crates/bevy_naadf/Cargo.toml` for `diag_compare` + `e2e/tests/device-snapshot.spec.ts`.
2. Confirm via `git log -- crates/bevy_naadf/src/diagnostics.rs e2e/tests/device-snapshot.spec.ts` that no commits in the last 14 days are "actively iterating" on the snapshot — if they are, escalate to user.
3. Stage the deletion in D7's implementor session — but the cross-binary references in `bin/e2e_render.rs` mean D7's implementor MUST coordinate with D6 (which owns the e2e harness) to land both deletions in the same commit. Otherwise the `device_snapshot_native_mode` dispatch branch will be a dangling import.

**Out-of-scope ripple.**
- `bin/e2e_render.rs` (D6 territory — see §F4's e2e mode split).
- `bin/diag_compare.rs` (D8 / build-binary territory — the asset-pipeline audit row says "audit whether anything still consumes it. If it's a dead CLI partner of `device_snapshot`, delete." That's exactly the case here.)
- `e2e/tests/device-snapshot.spec.ts` (D6).
- The Playwright test fixture path `target/diagnostics/device-snapshot-web.json` and any CI config referencing it.

**Resulting D7 scope after this deletion**: `diagnostics.rs` shrinks from 711 → 148 LOC (press-P only). `lib.rs:799` line goes away. The remaining `DiagnosticsPlugin` is the trivial press-P registration at `diagnostics.rs:147-153`.

---

### F8 — Inline conditional camera/system wiring duplicates the `sync_position_split` registration (severity: low)

**Location**: `crates/bevy_naadf/src/lib.rs:770-784`.

**Current state.**
```rust
if cfg.add_free_camera {
    app.add_plugins(FreeCameraPlugin).add_systems(
        Update,
        (
            camera::toggle_dlss,
            camera::apply_initial_camera_pose_changes,
            camera::sync_position_split,
        )
            .chain(),
    );
} else {
    app.add_systems(Update, camera::sync_position_split);
}
```

**Why it's a problem.**
- `sync_position_split` is registered in *both* arms of the `if/else`. The else-arm comment correctly notes it must still run (pure function of `Transform`), but the structure makes the duplication invisible — a future edit to "make the chain run in `LateUpdate`" would only catch one branch.
- The chain ordering matters: `apply_initial_camera_pose_changes` writes `Transform` + `PositionSplit`; `sync_position_split` then re-derives `PositionSplit` from `Transform`. The `.chain()` correctly enforces this. The else-arm has no chain (only one system), but `update_camera_history` registered at `lib.rs:895-898` *also* runs after `sync_position_split` — that ordering constraint is enforced in *yet a third place*.
- This is the "multi-place ordering" smell the F1 plugin extraction is supposed to absorb.

**Suggested direction (NOT a design).**
Inside `CameraPlugin::build`:
```rust
app.add_systems(Update, sync_position_split);
app.add_systems(Update, update_camera_history.after(sync_position_split));
if self.add_free_camera {
    app.add_plugins(FreeCameraPlugin)
        .add_systems(Update, (toggle_dlss, apply_initial_camera_pose_changes)
            .before(sync_position_split));
}
```
One registration of `sync_position_split`, one ordering edge, the conditional only adds the free-camera systems.

**Out-of-scope ripple.**
- `update_camera_history` move (see F1 — D4 owns the file but the system is main-world).

---

### F9 — Press-P diagnostics dump uses `String::new()` + 11 `writeln!`/`push_str` calls — formatting could be a `Display` impl on a `Diagnostics` struct (severity: low)

**Location**: `crates/bevy_naadf/src/diagnostics.rs:40-143`.

**Current state.** The 103-line system body interleaves data access (queries) with manual `writeln!(buf, "...")` formatting. Each diagnostic field gets its own `writeln!` or `push_str` — 11 of them, plus 4 fallback `push_str` branches when a query fails.

**Why it's a problem.**
- The function does two things: (a) gather diagnostic data, (b) render it to a string. The two are tangled — every formatting edit touches a line that also accesses a query.
- Testing is impossible: the function takes `Res<...>` + `Query` parameters and writes only to `info!`, which means there's no way to unit-test "given this state, the dump matches this snapshot." Compare to the `device_snapshot::DeviceSnapshot` struct (which IS testable via serde_json — and which we're deleting per F7, so the example is moot).

**Suggested direction (NOT a design).**
- Extract a `DiagnosticsDumpSnapshot { camera_translation: Vec3, cursor_voxel_hit: Option<…>, args: AppArgs, … }` struct, gather it in one pass, format via `impl Display`. The system body becomes ~20 LOC of gathering + `info!("{}", snapshot)`. Snapshot becomes unit-testable.
- Or — judgment call — leave it. 103 LOC of straight-line dump code is fine for a single-purpose debug-key handler; the abstraction adds indirection for no real readability win. The architect's call.

**Out-of-scope ripple.**
- None — this is internal to `diagnostics.rs`.

---

### F10 — Unused / dead-end exports in `lib.rs` after extraction would simplify the public surface (severity: low)

**Location**: `crates/bevy_naadf/src/lib.rs`.

**Current state.** After F1 extraction, `lib.rs` would still re-export (or transitively expose) a long list of `pub` items that exist only because every other module needs them: `AppArgs`, `AppConfig`, `WindowConfig`, `GiSettings`, `GridPreset`, `DevFont`, `WORLD_SIZE_IN_*`, `DEFAULT_TAA_RING_DEPTH`, `build_app`, `build_app_with_args`, `run_e2e_render`, `run_e2e_render_with_args`. The `lib.rs:13-25` module list is also `pub` for everything.

**Why it's a problem.**
- `lib.rs` as the "spine" file is the right place for `AppConfig` + `build_app` + the module declarations. It is NOT the right place for `GiSettings` (F2), `GridPreset` (D3's territory — the variant `GridPreset::Vox` carries voxel-loader path data), or `DevFont` (best moved into a `HudPlugin` / dedicated font module).
- The `pub use` re-export discipline isn't established. Some types are reachable only via `crate::AppArgs` (defined at lib root); others via `crate::voxel::GridPreset`-style paths. After F1 lands, this inconsistency becomes more visible.

**Suggested direction (NOT a design).**
- After F2's `GiSettings` move, audit `lib.rs`'s `pub` surface. Consider keeping `lib.rs` to: `AppArgs`, `AppConfig`, `WindowConfig`, `build_app`, `build_app_with_args`, `run_e2e_render*`, the module list. Move `GridPreset`, `GiSettings`, `DevFont`, `WORLD_SIZE_IN_*` constants to their owning modules and `pub use` selectively if needed.
- This is a low-impact polish; architect's discretion. The behaviour change is zero.

**Out-of-scope ripple.**
- ~20 `use crate::X;` statements update to `use crate::path::to::X;`. Compiler enforces.

---

## Confirmed / refuted audit suspicions

The brief listed 3 initial suspicions in §"Initial audit suspicions for D7." Verdicts:

1. **`lib.rs` 1146 LOC, `build_app_with_args` is a 336-line monolith** — **CONFIRMED.** The function spans lines 638-973 = 336 lines. F1 above. The suspicion's "drop `lib.rs` to ~500 LOC" estimate is plausible: removing the entire 71-LOC `add_hud` block + the ~30-LOC inline camera registration + ~30 LOC of inline voxel-io wiring + the 5-LOC device-snapshot wiring + `spawn_phase_c_test_entity` (44 LOC) + `load_dev_font` (10 LOC) gets us to roughly 1146 - 190 ≈ 956 LOC. Further savings from extracting `GiSettings` (78 LOC) + `AppArgs` field comments (~80 LOC of doc-comment if the e2e mode flags split out per F4) get us to ~800. To hit "~500 LOC" we'd also need `AppConfig`/`WindowConfig` to migrate to their own file — possible but architect's call.

2. **`GiSettings` triple-canonical** — **CONFIRMED.** F2 above. Verified all 5 sites exist as audit claimed. The brief's "D7 owns the canonical Rust struct" framing is correct; D2 + D4 handle their side of the SSoT.

3. **`diagnostics.rs` mixes press-P (148 LOC) + device-snapshot (~560 LOC), delete the device-snapshot submodule outright** — **CONFIRMED on the structure** (the two surfaces share zero types and zero callers in either direction within `diagnostics.rs`). **CONSTRAINT-CONFLICT FLAGGED on the deletion**: the device-snapshot submodule IS cited by `bin/e2e_render.rs`, `bin/diag_compare.rs`, and `e2e/tests/device-snapshot.spec.ts`. The user directive is "delete outright" — but the implementor must delete those four call-sites in lockstep. See F7 for the full delete-list. The architect must explicitly enumerate the cross-binary/cross-test deletions before D7's impl runs.

---

## Plugin-decomposition sketch

Target shape for `build_app_with_args` (architect refines — this is the explorer's first-cut sketch, not the design):

```rust
pub fn build_app_with_args(cfg: AppConfig, args: AppArgs) -> App {
    let mut app = App::new();
    app.insert_resource(args.clone()).insert_resource(cfg);

    // DefaultPlugins + its three .set() overrides — extract to helper:
    app.add_plugins(default_plugins_for(&cfg, &args));

    // Core engine plugins (already extracted, just re-listed):
    app.add_plugins((
        bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
        bevy::render::diagnostic::RenderDiagnosticsPlugin,
        WorldPlugin,
        NaadfRenderPlugin,
        ConstructionPlugin,
        BakedMaterialPlugin,
        TextureArrayPlugin,
    ));

    // D7-owned plugins (NEW):
    app.add_plugins((
        DevFontPlugin,               // ROBOTO bytes + load_dev_font
        CameraPlugin,                // setup_camera + sync_position_split +
                                     //   apply_initial_camera_pose_changes + toggle_dlss +
                                     //   FreeCameraPlugin (gated on cfg.add_free_camera) +
                                     //   update_camera_history (moved from render/taa.rs)
        DiagnosticsPlugin,           // press-P only (device_snapshot deleted per F7)
    ));

    // D2/D3 plugins (their architects land them):
    app.add_plugins(VoxelIoPlugin);  // setup_test_grid + async pump + web_vox + native dnd
    if cfg.add_hud {
        app.add_plugins((HudPlugin, AppModePlugin, EditorPlugin, SettingsPlugin));
    }

    // D6 plugin:
    if cfg.add_e2e_systems { app.add_plugins(E2ePlugin); }

    app
}
```

Each plugin's body owns its `Resource` init, `State` init, and `add_systems` calls. The `cfg.add_free_camera` / `cfg.add_hud` / `cfg.add_e2e_systems` flags are read by the plugins from `Res<AppConfig>` for self-gating, OR each plugin takes a config struct on construction (`CameraPlugin { add_free_camera: cfg.add_free_camera }`). Architect picks the convention.

Open questions the architect must resolve:

- Which is owned by D2 vs D7: `HudPlugin` (the FPS HUD at `src/hud.rs`) — it's tiny (245 LOC), conceptually "always-on diagnostic overlay," sibling to `EditorPlugin`/`SettingsPlugin`. Both placements are defensible. The brief assigns `hud.rs` to D2 in the LOC table; D7 stays a thin wirer.
- `spawn_phase_c_test_entity` (`lib.rs:1052-1095`): brief implies this leaves D7. Architect confirms it lands in D5's `render/construction/` next to `MainWorldEntities`.
- `update_camera_history` (`render/taa.rs`): moving it to `CameraPlugin` would be a cross-D4 edit. Either D4's architect agrees, or D7 only adds the ordering edge from outside (`app.add_systems(Update, update_camera_history.after(sync_position_split))` lives in `CameraPlugin` without moving the function definition).

---

## SSoT coordination notes

**SSoT-1 (`max_ray_steps_*` family).** D7 owns `GiSettings` (the canonical Rust struct at `lib.rs:108-185`). The audit lists the divergence as crosscutting `D4 + D7 + D2`. D7's piece:

- Move `GiSettings` out of `lib.rs` (currently at `lib.rs:108-231` including `Default`) into `crates/bevy_naadf/src/settings.rs` (or a new `crates/bevy_naadf/src/gi_settings.rs`). This is the canonical home.
- D2's KNOBS table (`settings.rs:172-219`) then has the struct definition in the same file — the function-pointer table can become `&[Knob<GiSettings>]`-shape or a `Reflect` derive (BEV-4 — D2's call).
- D4 owns the GPU-side mirror (`gpu_types.rs::GpuRenderParams.max_ray_steps_primary`, `GpuGiParams.{max_ray_steps_secondary,sun,sun_secondary,visibility,spatial_iter_count}`); D4's architect proposes how the values flow from `GiSettings::default()` into either WGSL `#define` shader-defs (compile-time) or uniform fields (runtime — current behaviour).
- D7's deliverable: move the struct, retain `pub use` re-export from `lib.rs` if needed for source-stability. Punt the reflect-driven KNOBS rework + the WGSL shader-def question to D2 + D4.

**SSoT-2 (`WORLD_SIZE_IN_CHUNKS/VOXELS/SEGMENTS`).** D7 owns this entirely. F5 above. Replace with `const fn` derivation; the existing test becomes a sanity check against the C# canonical values. No coordination needed with other domains — they only consume the constants.

---

## device_snapshot deletion notes

Per the user directive at the brief level (and `01-context.md:53`), the device-snapshot submodule deletes outright. Verified delete-list (line numbers verified by Read at this exploration session):

| file | line range | content |
|---|---|---|
| `crates/bevy_naadf/src/diagnostics.rs` | 155-711 | The submodule + its section divider. Includes `pub mod device_snapshot {`, all schema types (`DeviceSnapshot`, `SnapshotAdapterInfo`, `SnapshotLimits`, `SnapshotDownlevel`, `LimitDelta`, `SnapshotBuild`, `PendingRenderSnapshot`, `PendingMainSnapshot`), the capture + extract + emit systems, all helpers (`snapshot_adapter_info`, `features_to_names`, `features_to_bits`, `limits_to_snapshot`, `snapshot_downlevel`, `compute_limit_deltas`, `build_facts`), and `DeviceSnapshotPlugin`. Also the module doc-comment lines 12-24 (the "Device snapshot" half of the two-surface description). |
| `crates/bevy_naadf/src/lib.rs` | 799 | `app.add_plugins(diagnostics::device_snapshot::DeviceSnapshotPlugin);` line. Comments at 792-798 explaining the snapshot also go. |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | 137-143 | `device_snapshot_native_mode` flag declaration + comment. |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | 364-375 | The `--device-snapshot-native` dispatch branch in the e2e mode ladder. |
| `crates/bevy_naadf/src/bin/diag_compare.rs` | entire file (314 LOC) | The whole binary exists only to diff two `DeviceSnapshot` JSON outputs. |
| `crates/bevy_naadf/Cargo.toml` | grep for `[[bin]] name = "diag_compare"` | The `[[bin]]` entry that builds `diag_compare`. |
| `e2e/tests/device-snapshot.spec.ts` | entire file | Playwright spec that captures the `[device-snapshot]` sentinel and writes web JSON. |
| `justfile` (or scripts) | grep for `diag-compare`, `device-snapshot` | Any recipes referencing the above. (Not verified by this explorer — D7 architect verifies.) |

**Caller audit evidence**: searched `grep -rn "device_snapshot\|DeviceSnapshot" crates e2e scripts` at exploration time. Results (annotated):
- `lib.rs:799` — the wiring line above.
- `diagnostics.rs:1-24,155-711` — the submodule itself + its module-doc-comment.
- `bin/e2e_render.rs:139,143,364,367` — the four cross-binary references above.
- `bin/diag_compare.rs:2,6` — the binary's module-doc + DEFAULT path comment.
- `e2e/tests/device-snapshot.spec.ts:12,88` — Playwright spec.
- No other callers in `crates/bevy_naadf/src/**` outside `diagnostics.rs` itself.

**Cross-domain coordination**: D6 owns `bin/e2e_render.rs` + `e2e/tests/`; D8 owns `bin/diag_compare.rs` (asset-pipeline domain — per the audit's row at `00-reuse-audit.md` D8). Both must land their deletions either (a) in lockstep with D7's commit, or (b) BEFORE D7 lands so D7's `diagnostics.rs` truncation doesn't break a build. The user-decided sequencing has D7 LAST, so (b) is the natural ordering: D6 deletes the e2e_render dispatch branch + the Playwright spec, D8 deletes `bin/diag_compare.rs` + the `[[bin]]` entry, then D7 deletes the submodule + the `lib.rs:799` line.

**Resulting `diagnostics.rs`** after deletion: ~148 LOC of press-P-only code. The remaining `DiagnosticsPlugin` is the trivial 7-LOC registration at `diagnostics.rs:147-153`. Module doc-comment shrinks to the press-P-only half. The press-P dump itself stays at `diagnostics.rs:40-143` (covered by F9 if architect wants the further refactor).

---

## Side notes / observations / complaints

1. **F7 is the biggest unknown.** The user's "delete outright" instruction is unambiguous at the brief level, but it requires deleting cross-domain assets (a Playwright spec, an `e2e_render` mode, an entire binary). If the user genuinely meant "delete from D7 and let D6/D8 deal with their side" then the explorer agent is doing the right thing flagging this. If the user meant "delete all of it everywhere" then this is just the call-graph work the architect would have done anyway. Recommend the orchestrator restate the directive to the architect with the full delete-list visible.

2. **The brief said `main.rs` is 75 LOC; actual is 54.** Probably the brief grabbed the LOC from the audit which had `wc -l` count blanks + comments differently, or the file was recently trimmed. Mentioning so the architect doesn't waste cycles looking for 21 phantom lines.

3. **`AppArgs::default()` ships with `taa: true` even though the doc-comment at `lib.rs:289` says "always `false` in Phase A".** The default was flipped to `true` somewhere along the way; the comment lies. This is a minor doc-rot but it's in D7's domain so flagging.

4. **`hud.rs` is at the lib-root (`crates/bevy_naadf/src/hud.rs`) but `editor::hud` is a sibling module (`crates/bevy_naadf/src/editor/hud.rs`).** Two `hud` modules at different depths. The audit row puts the root-`hud.rs` in D2's territory and the editor-`hud.rs` also in D2 — so D7's plugin sketch should defer both to D2 or one to D7 if convenient. Mild naming confusion; architect's call.

5. **The `cfg.add_e2e_systems` flag does double duty.** It both gates the e2e driver registration (`lib.rs:877-878`) AND the LogPlugin custom-layer install (`lib.rs:701-708`) AND the `!cfg.add_e2e_systems` skip of `DiagnosticsPlugin` (`lib.rs:788`) AND the `!cfg.add_e2e_systems` skip of the native dnd listener (`lib.rs:861`). Four different effects from one flag. Not necessarily wrong, but the design pressure on the flag is high — each new e2e gate adds more bound effects. The plugin-extraction in F1 naturally distributes these gates to each plugin's `build` method.

6. **`build_app_with_args` is private**. `build_app` is `pub fn` (line 628) but `build_app_with_args` is also `pub fn` (line 638) — both are public. The function-pair pattern (one defaults, one takes args) is fine, but the visibility audit might prefer making `build_app_with_args` `pub(crate)` and exposing only `build_app` once F4's `E2eMode` enum split lands. Architect's call.

7. **The brief's LOC tally for D7 (2 396) closely matches actual (2 375).** Within rounding. The audit was accurate.

8. **`feature-completeness/01-context.md` constraint cited at brief F2 — "Do NOT delete `MAX_RAY_STEPS_*` consts"**. The audit specifically calls this out as deliberate retention (the WGSL `const`s are kept for shader-side fallback). This means the SSoT-1 closure path is NOT "delete the WGSL consts and inject as shader-defs"; it's more likely "WGSL consts stay; runtime path overrides them via uniform fields; the agreement is enforced by tests." That's a D4 architect call — D7 only does the struct move.

9. **Equal-footing complaint**: D7's brief framing as "the spine that every other domain wires through" + "D7's implementor runs LAST" + "you are ONE of 8 parallel explorers" creates a Chesterton's-fence problem. If D1's architect proposes folding `GridPreset::Vox` into a new `VoxelIoPlugin`-internal type, and D2's architect proposes the same for `GiSettings`, and D3's architect for the wasm `?ui=hide` flag (currently inserts `UiHiddenOverride` from `voxel/web_vox.rs`), D7 ends up writing an architect doc that references 7 other architect docs that don't yet exist. The parallel-architect phase will need a synchronisation pass. Suggestion: D7's architect dispatch should explicitly wait for D1–D6 + D8 architect docs to land, OR receive a brief that says "draft a design that depends on the other architect docs through `<placeholder>` references; the orchestrator resolves placeholders before D7's implementor runs."

10. **Subjective**: this codebase is genuinely well-structured under the bloat. The `Plugin`-extraction work for D7 is mechanical — every system has a clear owner, every resource has a clear scope, the architectural anchors (`AppConfig`, `AppArgs`) are well-documented. The 1146-LOC `lib.rs` reads more as "this was the staging area where each phase landed a new chunk that should have been extracted at the time" than as foundation rot. Compare to `render/construction/mod.rs:11043` (D5's territory) which IS foundation rot. D7's refactor is straightforward; D5's is the hard one.
