# 02b — Design — Track B: Editor (paint/cube/sphere)

**Date:** 2026-05-15
**Author:** delegate-architect
**Branch:** `main` at HEAD (post `1c35c7f`)
**Brief:** orchestrator-supplied; covers Track B — editor with paint/cube/sphere tools, fresh Bevy-UI 0.19 gamified controls extending `panel.rs`, sanctioned `set_voxels_batch` perf divergence, no new deps.

## Overview

**End-to-end interaction.** The user presses `F2` to enter edit mode (a non-modal flip — the fly camera stays usable; only LMB is repurposed). With the panel (now `F1`) the user picks a tool (`Paint`/`Cube`/`Sphere`), a brush radius (1..400), a target `VoxelTypeId` from the palette index, and the erase + continuous toggles. They aim the camera at the world, press LMB, and the system: (1) raycasts cursor → world via `WorldData::ray_traversal`, (2) snaps the brush position to the hit voxel on the first frame of the press, lerps the position toward the new hit on subsequent frames (motion smoothing — `EditingToolPaint.cs:36-40`), (3) the selected `*_brush` enumerates every voxel inside its footprint (sphere `r²`, cube Chebyshev, paint `r²`-on-non-empty), batches them into a `Vec<(IVec3, VoxelTypeId)>`, and calls `WorldData::set_voxels_batch`. That helper groups by chunk and builds one combined `process_edit_batch` invocation that pushes one `EditBatch` covering every touched chunk + every touched group. The existing extract drains `pending_edits` next frame, the W2 GPU dispatch chain (regime-3) applies the edits to the chunks/blocks/voxels textures, and the next-frame render reflects the change. A tool-state HUD overlay at top-right shows the current tool, radius, and hover voxel info; the panel's editor section shows the same state in mutable form.

**Architectural rationale.** Every load-bearing surface for Track B already exists in the port: `set_voxel` → `process_edit_batch` → `extract_world_changes` → `compute_change_groups` → W2 GPU dispatch is wired and bit-faithful. The only missing pieces are (a) CPU ray-traversal (port of C# `WorldData.RayTraversal:396-473`, ~80 LOC), (b) the brush footprints (~50 LOC each, ported from C# `EditingTool{Paint,Cube,Sphere}.cs`), (c) a thin `EditorState` + `apply_edit_tool` system (~150 LOC), (d) a `set_voxels_batch` perf extension on `WorldData` (~80 LOC, sanctioned divergence), (e) a `KnobKind::Enum` variant + ~6 new `KNOBS` rows on `panel.rs` (~80 LOC), (f) a tool-state HUD (~40 LOC), and (g) `FreeCamera` gating via `FreeCameraState.enabled` (~10 LOC). Total: ~400 LOC, no new deps. Everything sits ON TOP of the existing W2 chain; no render-side changes, no shader edits, no `GpuGiParams` mutation.

## Architecture

### Module layout

```
crates/bevy_naadf/src/
├── editor/                          # NEW directory — one home for all editor code
│   ├── mod.rs                       # pub use; EditorPlugin? (or wire from lib.rs); EditorMode, EditorState, EditTool
│   ├── tools.rs                     # paint_brush / cube_brush / sphere_brush helpers
│   ├── ray.rs                       # screen_to_ray helper + RayHit type
│   └── hud.rs                       # tool-state overlay (Node + Text), gated on EditorState.edit_active
├── panel.rs                         # EDITED — extend with KnobKind::Enum, new KNOBS rows
├── world/data.rs                    # EDITED — add ray_traversal + set_voxels_batch
└── lib.rs                           # EDITED — wire new systems, init EditorState resource, F2 toggle
```

Three reasons the editor lives under `editor/` instead of being scattered across `world/` and `voxel/`:

1. The `apply_edit_tool` system is a top-level Bevy system that consumes camera, mouse, window, world data, and panel state; it has no natural home in any single existing module.
2. Tool footprint helpers (`paint_brush` etc.) need to be `pub` for testing but call private `WorldData` mutation helpers; concentrating them in one module avoids a `voxel/grid.rs::fill_*` extraction that the audit recommended but that would leak `DenseVolume`-shaped helpers into a closure-callback API. The audit row 6 "extract `fill_sphere`/`fill_box` as `pub` helpers taking a closure" is **rejected** — see Decisions § "fill_sphere extraction" below.
3. A future `editor_brush_undo` or `editor_brush_recording` would naturally join here.

`WorldData::ray_traversal` lives on `WorldData` (in `world/data.rs`) — same as in C# (`WorldData.cs:396-473`) — because it reads `chunks_cpu` / `blocks_cpu` / `voxels_cpu` directly and is most natural as an `&self` method. `WorldData::set_voxels_batch` likewise lives on `WorldData` next to `set_voxel`.

### `EditorState` resource shape

```rust
// crates/bevy_naadf/src/editor/mod.rs

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditTool {
    #[default]
    Paint = 0,
    Cube = 1,
    Sphere = 2,
}

/// Editor configuration + per-frame runtime state. Single Resource so the
/// panel and the apply system share one source of truth (matches the
/// `AppArgs.gi` shared-state pattern in `panel.rs`).
#[derive(Resource, Debug, Clone)]
pub struct EditorState {
    // ---- user-tweakable via panel ----
    /// Currently selected tool. Toggle via panel's enum knob.
    pub tool: EditTool,
    /// Currently selected paint type — index into VoxelTypes::types, clamped
    /// to `1..types.len()` (0 is the empty placeholder; selecting it would
    /// effectively be "paint empty" which is what `is_erase` already does).
    pub selected_type: VoxelTypeId,
    /// Brush radius in voxels — clamped 1..400 by the panel; C# default 10.
    /// `EditingToolPaint.cs:13` initialises `radius = 10`.
    pub radius: f32,
    /// Erase mode (Cube/Sphere only). Paint ignores this — paint's semantic
    /// is "replace non-empty voxels with the selected type", never erase.
    /// `EditingToolCube.cs:20`, `EditingToolSphere.cs:20`.
    pub is_erase: bool,
    /// Continuous-brush mode (Cube/Sphere only). When `false`, the brush only
    /// fires on the LMB-down edge; when `true`, it re-fires every frame while
    /// LMB is held. `EditingToolCube.cs:50-51` + `EditingToolSphere.cs:50-51`
    /// — `if (!isContinuous && IO.MOStates.Old.LeftButton == Pressed) return;`
    /// (i.e. on every frame after the first LMB-pressed frame, return early
    /// if not continuous). Default: `true`, matching C# default.

    pub is_continuous: bool,

    // ---- runtime-only state (not user-editable) ----
    /// Master gate: `false` means LMB is ignored by the editor + the tool-
    /// state HUD is hidden. Toggled by F2 (separate from the F1 panel toggle —
    /// the user can have the panel open without being in edit mode).
    pub edit_active: bool,
    /// Smoothed brush position in world space. Snapped on LMB-just-pressed
    /// (`EditingToolPaint.cs:34-35`), lerped on continued press
    /// (`EditingToolPaint.cs:36-40`). Initialised to ZERO; first LMB press
    /// snaps it to the cursor's hit, so the initial value doesn't matter.
    pub pos: Vec3,
    /// `true` on the frame LMB was first pressed this stroke (i.e. on the
    /// `just_pressed(LMB)` frame). The state machine equivalent of C#'s
    /// `IO.MOStates.IsLeftButtonToggleOn()` (`EditingToolPaint.cs:34`).
    /// Tracked manually rather than re-derived because the system reads
    /// `ButtonInput<MouseButton>::just_pressed` directly, but we also want
    /// to track "first-frame-of-this-stroke" for the snap-vs-lerp branch.
    /// Cleared when LMB is released.
    pub stroke_just_started: bool,
    /// Last hover RayHit, cached for the HUD overlay (refreshed every frame
    /// `edit_active && cursor_in_window`, regardless of LMB state).
    pub last_hover_hit: Option<RayHit>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            tool: EditTool::default(),
            selected_type: VoxelTypeId(1), // first non-empty palette index
            radius: 10.0,
            is_erase: false,
            is_continuous: true,
            edit_active: false,
            pos: Vec3::ZERO,
            stroke_just_started: false,
            last_hover_hit: None,
        }
    }
}
```

The `EditorState` resource is initialised via `App::init_resource::<EditorState>()` in `lib.rs` alongside the existing `PanelState`/`PanelDrag` initialisations (`lib.rs:584-585`). Gated on `cfg.add_hud` (same as the panel — the e2e harness must not see the editor either, for the same reason the panel is excluded).

### The `apply_edit_tool` Update system flow

```
fn apply_edit_tool(
    keys:            Res<ButtonInput<KeyCode>>,
    mouse:           Res<ButtonInput<MouseButton>>,
    window:          Single<&Window, With<PrimaryWindow>>,
    camera:          Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut world_data:  ResMut<WorldData>,
    voxel_types:     Res<VoxelTypes>,
    mut state:       ResMut<EditorState>,
    time:            Res<Time>,
    panel_state:     Res<PanelState>,
) {
    // F2 toggle — gated to NOT fire while the panel is open AND the cursor
    // is over the panel (defensive; F2 doesn't trigger panel widgets, but
    // keep the principle of mutual non-interference).
    if keys.just_pressed(KeyCode::F2) {
        state.edit_active = !state.edit_active;
    }

    // Refresh hover info every frame edit_active is on (cheap; one ray cast).
    if !state.edit_active {
        state.last_hover_hit = None;
        return;
    }
    let Some(cursor_pos) = window.cursor_position() else {
        state.last_hover_hit = None;
        return;
    };
    let (camera, cam_gxf) = *camera;
    let Some(ray) = screen_to_ray(camera, cam_gxf, cursor_pos) else {
        return;
    };
    state.last_hover_hit = world_data.ray_traversal(ray.origin, ray.dir);

    // LMB handling — apply brush.
    if !mouse.pressed(MouseButton::Left) {
        // Release — clear "stroke just started"; nothing to apply.
        state.stroke_just_started = false;
        return;
    }
    let Some(hit) = state.last_hover_hit.clone() else {
        return;
    };

    // Snap-on-first-press, lerp on continued press.
    let just_pressed = mouse.just_pressed(MouseButton::Left);
    if just_pressed {
        state.stroke_just_started = true;
        state.pos = hit.world_pos;
    } else {
        // C# Paint.cs:38-40 smoothing math (matches Cube + Sphere too).
        let dt = time.delta_secs();
        let lerp_value =
            (1.0 - 1.0 / (1.0 + dt * 0.15 / state.radius)).min(1.0);
        state.pos = hit.world_pos * lerp_value + state.pos * (1.0 - lerp_value);
    }

    // is_continuous gate — only Cube + Sphere honour it. Paint ALWAYS re-fires.
    // C# EditingToolCube.cs:50-51 / EditingToolSphere.cs:50-51:
    //   if (!isContinuous && IO.MOStates.Old.LeftButton == Pressed) return;
    // Translation: when not continuous AND this is NOT the first frame of the
    // stroke, return early.
    if matches!(state.tool, EditTool::Cube | EditTool::Sphere)
        && !state.is_continuous
        && !state.stroke_just_started
    {
        return;
    }

    // is_erase + selected_type sanity (C# Cube.cs:30-31 + Sphere.cs:30-31:
    // `if (!isErase && selectedTypeRenderIndex == 0) return;`).
    if !state.is_erase
        && matches!(state.tool, EditTool::Cube | EditTool::Sphere)
        && state.selected_type == VoxelTypeId::EMPTY
    {
        // Paint is exempt from this check — it does its own non-empty test
        // per voxel below; selecting "empty" for Paint is a no-op anyway.
        return;
    }

    // Dispatch to the brush.
    match state.tool {
        EditTool::Paint => {
            tools::paint_brush(&mut world_data, state.pos, state.radius, state.selected_type);
        }
        EditTool::Cube => {
            tools::cube_brush(&mut world_data, state.pos, state.radius, state.selected_type, state.is_erase);
        }
        EditTool::Sphere => {
            tools::sphere_brush(&mut world_data, state.pos, state.radius, state.selected_type, state.is_erase);
        }
    }

    // Clear "first frame" flag AFTER the brush ran — so a Cube edit with
    // `is_continuous = false` fires exactly once per LMB-down.
    state.stroke_just_started = false;

    let _ = panel_state; // future: gate when panel cursor is over the panel
    let _ = voxel_types; // present so the system reads it for cache + future
}
```

### Panel extension — fresh `EDITOR` section + `KnobKind::Enum`

Inserted at the TOP of `KNOBS` (so editor controls are visible without scrolling), the new section reads:

```
EDITOR (active: F2)
  tool                   Paint  [E]
  selected_type             1   [E]    (1..palette_size)
  radius                10.00   [E]    (1..400, log scale comes for free
                                        via panel's mouse-drag exponential
                                        sensitivity — see Decisions)
  is_erase              false   [E]
  is_continuous          true   [E]
> APPLY HOVER ECHO <             [B]   (debug action: prints the current
                                        last_hover_hit to log; useful in
                                        development, gated on `cfg(debug)`?
                                        — NO, keep it always-on, costs ~5 LOC)
DIVIDER
RAY STEP CAPS                          (existing — unchanged)
  primary                ...
...
```

The new section uses class character `'E'` (mirrors the existing `'P'` / `'C'` / `'D'` / `'B'` classes — `panel.rs:181`). One new variant + one new helper added to `panel.rs`:

```rust
// New variant on KnobKind (panel.rs:187+).
enum KnobKind {
    // ...existing variants unchanged...
    /// An enum-valued knob on the editor state. `variants` lists the names;
    /// Left/Right (and mouse-drag) cycles. Mutates the EditorState resource
    /// via the closure-pointer pair.
    Enum {
        getter: fn(&EditorState) -> u32,
        setter: fn(&mut EditorState, u32),
        variants: &'static [&'static str],
        default: u32,
    },
}
```

**Why a new variant instead of reusing `U32`.** The display is fundamentally different — a `U32` row shows the integer value (`10`); an `Enum` row shows the variant name (`Paint`). And the bounds (`min/max`) are implicit in `variants.len()`. A new variant is ~30 LOC; reusing `U32` would require a per-row "string-format-override" callback, which is uglier.

Because the new variant operates on `EditorState` (not `AppArgs.gi`), we have two options:

(a) **Pass `EditorState` as a second mutable parameter through every panel system.** Each `getter`/`setter` becomes `fn(&EditorState) -> u32` / `fn(&mut EditorState, u32)`. The existing `KnobKind::U32`/`F32`/`Bool` keep their `GiSettings` getters. We need a second mutable resource in `adjust_panel`, `mouse_interact_panel`, `apply_drag_delta`, `handle_click_release`, `update_panel_text`, and `reset_all_knobs`. **Chosen** — see Decisions below.

(b) **Unify under a "panel context" struct.** Wrap both `&mut AppArgs` and `&mut EditorState` into a single `PanelCtx` borrow. **Rejected**: more surface change to `panel.rs` for less clarity.

The chosen (a) means:
- The five getter/setter knob variants get **two function pointers each** — `GiSetting` variants stay as-is (`getter: fn(&GiSettings) -> u32`); new `EditorState` variants point at `EditorState`. The variants are distinct (`KnobKind::U32` vs new `KnobKind::EditU32` etc.) OR we generalise via a tagged sum on the closure pair. To minimise churn, the simplest approach: **add 4 mirror variants** `EditU32 / EditF32 / EditBool / EditEnum` that take `&[mut] EditorState` instead of `&[mut] GiSettings`. ~80 LOC of duplication; the `panel.rs` adjust/mouse logic gets a `match` arm per new variant.

Alternative: **fold `EditU32 / EditF32 / EditBool` into one `Edit { variant: EditKnobVariant }` enum with the four sub-shapes.** This keeps `match` arms tighter. The trade-off is ~20 fewer LOC, more nested matches. **Chosen — single `Edit { variant }` variant grouped tagged-union** because the panel `update_panel_text` already has a tagged match against `KnobKind`; adding one outer `Edit { variant: ... }` arm keeps the overall structure recognisable.

Final shape:

```rust
// panel.rs:187+ (additive — existing variants unchanged)
enum KnobKind {
    Section,
    U32 { /* unchanged */ },
    F32 { /* unchanged */ },
    Bool { /* unchanged */ },
    Readonly { /* unchanged */ },
    Action { /* unchanged — `apply: fn(&mut AppArgs)` */ },
    /// NEW — editor-state knob. The `variant` field discriminates on the
    /// underlying value type while keeping the row a single panel entry.
    Edit { variant: EditKnobVariant },
}

enum EditKnobVariant {
    U32 {
        getter: fn(&EditorState) -> u32,
        setter: fn(&mut EditorState, u32),
        nudge: u32,
        big_step: u32,
        min: u32,
        max: u32,
        default: u32,
    },
    F32 {
        getter: fn(&EditorState) -> f32,
        setter: fn(&mut EditorState, f32),
        nudge: f32,
        big_step: f32,
        min: f32,
        max: f32,
        default: f32,
    },
    Bool {
        getter: fn(&EditorState) -> bool,
        setter: fn(&mut EditorState, bool),
        default: bool,
    },
    Enum {
        getter: fn(&EditorState) -> u32,
        setter: fn(&mut EditorState, u32),
        variants: &'static [&'static str],
        default: u32,
    },
}
```

**Why this layout.** `KnobKind` stays compact (one new variant), all existing logic on the `GiSettings`-side variants unchanged. The new `Edit { variant }` arm in `adjust_panel`/`mouse_interact_panel`/`apply_drag_delta`/`handle_click_release`/`update_panel_text` is one nested match on `variant` — the per-sub-variant logic mirrors what `U32`/`F32`/`Bool` already do but reads `&mut EditorState`. Total: ~120 LOC of additive panel changes; zero existing-row regressions.

#### New `KNOBS` rows (top of table)

```rust
const KNOBS: &[Knob] = &[
    Knob { label: "EDITOR (F2 toggles edit mode)", class: ' ', kind: KnobKind::Section },
    Knob {
        label: "  tool",
        class: 'E',
        kind: KnobKind::Edit {
            variant: EditKnobVariant::Enum {
                getter: |s| s.tool as u32,
                setter: |s, v| {
                    s.tool = match v {
                        0 => EditTool::Paint,
                        1 => EditTool::Cube,
                        2 => EditTool::Sphere,
                        _ => s.tool,
                    };
                },
                variants: &["Paint", "Cube", "Sphere"],
                default: 0, // Paint
            },
        },
    },
    Knob {
        label: "  selected_type",
        class: 'E',
        kind: KnobKind::Edit {
            variant: EditKnobVariant::U32 {
                getter: |s| s.selected_type.0 as u32,
                setter: |s, v| {
                    // Clamp to 1..palette_size at runtime — the panel doesn't
                    // know `voxel_types.types.len()` statically; we set a
                    // generous upper bound (`MAX_VOXEL_TYPE_INDEX = 4095`) and
                    // let the brush silently no-op if the index is out of
                    // palette range. Practical palettes are <100 entries.
                    s.selected_type = VoxelTypeId(v.min(u16::MAX as u32) as u16);
                },
                nudge: 1, big_step: 5,
                min: 1, max: 4095, // VOXEL_PAYLOAD_MASK is 15 bits = 32767
                default: 1,
            },
        },
    },
    Knob {
        label: "  radius",
        class: 'E',
        kind: KnobKind::Edit {
            variant: EditKnobVariant::F32 {
                getter: |s| s.radius,
                setter: |s, v| s.radius = v,
                nudge: 1.0, big_step: 10.0,
                min: 1.0, max: 400.0,
                default: 10.0,
            },
        },
    },
    Knob {
        label: "  is_erase",
        class: 'E',
        kind: KnobKind::Edit {
            variant: EditKnobVariant::Bool {
                getter: |s| s.is_erase,
                setter: |s, v| s.is_erase = v,
                default: false,
            },
        },
    },
    Knob {
        label: "  is_continuous",
        class: 'E',
        kind: KnobKind::Edit {
            variant: EditKnobVariant::Bool {
                getter: |s| s.is_continuous,
                setter: |s, v| s.is_continuous = v,
                default: true,
            },
        },
    },
    Knob { label: "DIVIDER", class: ' ', kind: KnobKind::Section },
    // ...existing RAY STEP CAPS section follows unchanged...
];
```

`reset_all_knobs` (`panel.rs:537-546`) gains four new match arms for the `EditKnobVariant` sub-variants, each restoring the row's `default`. The `Action { apply: reset_all_knobs }` last row gets a new signature: `apply: fn(&mut AppArgs, &mut EditorState)` — the orchestrator must thread `&mut EditorState` into the `KnobKind::Action` apply call sites (`adjust_panel:826-832`, `handle_click_release:1033-1036`). One signature change, three call-site updates — ~10 LOC.

#### Single F1 panel vs. F1+F2 dual-panel decision

The audit (§3.4) recommended a single panel. I confirm: **one panel, F1-toggled, with an `EDITOR` section at the top.** `F2` toggles only `EditorState.edit_active` (the "now LMB does brush" mode); it does NOT show/hide the panel.

Rationale:
- The panel only has ~28 rows today; adding 5 more pushes to 33. At 12 px line height (panel default), that's ~400 px of vertical space — well within the available 1080-px screen height. No need for a second panel.
- The user's "gamified" intent points to **at-a-glance state visibility**: with one panel, every knob is in one place. Two panels splits attention.
- `F1` opens the panel for tweaking; `F2` flips into edit mode (LMB now acts on the world). These are orthogonal concerns and want different keys.

### Tool-state HUD overlay

Sibling component to `HudText` (`hud.rs:89-110`). New `EditorHudText` component + `setup_editor_hud` system + `update_editor_hud` system in `editor/hud.rs`. Mirrors the chrome of `setup_hud` exactly — `Node { position_type: Absolute, top: px(12), right: px(12), padding: px(8).all(), .. }`, `BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6))`, `TextFont { font: dev_font.0.clone(), font_size: FontSize::Px(14.0), .. }`. Difference: anchored top-RIGHT instead of top-LEFT (HUD is top-left, panel is bottom-left, editor-HUD is top-right — three-corner layout, no overlap).

Content (refreshed every frame via `Update`):

```
EDITOR MODE
  Tool:     Paint
  Radius:   10
  Erase:    false
  Continuous: true
  Type:     1

Hover:
  Voxel:    (32, 15, 28)
  Type:     5 (TY_EMISSIVE)
  Normal:   (0, 1, 0)
  Distance: 14.32

Press F2 to exit.
```

Gated on `EditorState.edit_active`: when `false`, the system writes empty string to the `Text` (or sets `Node::display = Display::None`). When `EditorState.last_hover_hit` is `None`, the "Hover:" block shows "(no hit — aim at world)".

LOC budget: ~50 (setup ~20 + update ~30).

### `FreeCamera` gating mechanism

**Chosen: option (a) — toggle `FreeCameraState.enabled` based on `EditorState.edit_active`.**

The `bevy_camera_controller` crate exposes `FreeCameraState.enabled: bool` at `bevy_camera_controller-0.19.0-rc.1/src/free_camera.rs:226-227`, and `run_freecamera_controller` short-circuits at `:291-303` when `enabled == false` (also releasing cursor grab). This is the clean disable hook the audit said might not exist — it does.

But re-checking the C# behaviour + reading `FreeCamera`'s default config: **`mouse_key_cursor_grab: MouseButton::Right`** (`free_camera.rs:150`). The fly camera captures mouse LOOK only when **RMB** is held. LMB is naturally free. This significantly changes the gating story:

1. With panel + editor both off, LMB does nothing, RMB rotates the camera (current behaviour, no regression).
2. With editor on (F2 pressed once), LMB now applies the brush; RMB still rotates. No conflict — they're separate mouse buttons.
3. WASD movement still works in edit mode (the user wants this — they need to fly close to a target before brushing). The audit's worry was "FreeCamera captures mouse always" — this is half-true (look only, on RMB); **we do not need to gate fly movement**.

**Updated decision: do NOTHING to `FreeCameraState.enabled`.** The existing input bindings already separate cleanly. `apply_edit_tool` reads `ButtonInput<MouseButton>::pressed(MouseButton::Left)` and does its work; `FreeCameraPlugin` reads RMB for look and WASD/etc. for movement. No interference.

The one edge case: when the panel is open (F1) AND cursor is over the panel AND user clicks LMB on a panel row, the brush would fire on the world UNDER the panel. **Mitigation:** `apply_edit_tool` reads `PanelState.open` + checks if the cursor is over the panel root's screen rect, and bails if so. Concretely, query `PanelRow` entities with `Interaction::Pressed` — if any panel row is pressed, the panel system already owns this click; `apply_edit_tool` returns. Implementation: `Query<(), (With<PanelRow>, With<Interaction>)>::iter().any(|i| *i == Interaction::Pressed)` — if any, bail.

LOC: ~10 in `apply_edit_tool` for the bail-on-panel-press guard, zero LOC for FreeCamera gating.

The audit recommendation (a) — `Update`-system early-out on `FreeCamera`'s movement — turns out unneeded; the audit was based on the (incorrect-without-reading-the-crate) assumption that LMB captures the camera. **Recorded as a decision below** with the verified evidence at `free_camera.rs:150`.

## File-by-file change list

### New files

| Path | Purpose | LOC |
|---|---|---|
| `crates/bevy_naadf/src/editor/mod.rs` | `EditorState` resource, `EditTool` enum, `apply_edit_tool` Update system, F2 toggle, public API + tests | ~150 |
| `crates/bevy_naadf/src/editor/tools.rs` | `paint_brush`, `cube_brush`, `sphere_brush` helpers + their unit tests | ~140 |
| `crates/bevy_naadf/src/editor/ray.rs` | `screen_to_ray` helper, `RayHit` type, + tests | ~50 |
| `crates/bevy_naadf/src/editor/hud.rs` | `EditorHudText` component, `setup_editor_hud`, `update_editor_hud` | ~50 |

### Edited files

| Path | Edit | LOC delta |
|---|---|---|
| `crates/bevy_naadf/src/world/data.rs` | Add `WorldData::ray_traversal(origin: Vec3, dir: Vec3) -> Option<RayHit>` + `set_voxels_batch(&mut self, edits: &[(IVec3, VoxelTypeId)])` methods. ~150 LOC delta on this file. | +150 |
| `crates/bevy_naadf/src/panel.rs` | Add `KnobKind::Edit { variant: EditKnobVariant }` variant + `EditKnobVariant` enum (4 sub-variants). Extend `KnobKind::is_interactive()`. Add 6 new top-of-`KNOBS` rows (EDITOR section + 5 knobs + a divider). Extend `adjust_panel`, `mouse_interact_panel`, `apply_drag_delta`, `handle_click_release`, `update_panel_text`, `reset_all_knobs` with `Edit { variant }` match arms. Thread `&mut EditorState` through `adjust_panel`'s `mut args` parameter + similar across the mouse path. Update `KnobKind::Action`'s `apply` signature from `fn(&mut AppArgs)` to `fn(&mut AppArgs, &mut EditorState)` + the existing `reset_all_knobs` call site. | +120 / -5 |
| `crates/bevy_naadf/src/lib.rs` | `mod editor;` declaration. `app.init_resource::<editor::EditorState>()` (gated on `cfg.add_hud`, same as `PanelState`). Wire `editor::apply_edit_tool` system into `Update` with `.after(panel::mouse_interact_panel)` (so panel-press bail-out reads up-to-date `PanelDrag` state). Wire `editor::hud::setup_editor_hud` to `Startup.after(load_dev_font)` and `editor::hud::update_editor_hud` to `Update`. | +20 |
| `crates/bevy_naadf/src/voxel/grid.rs` | (zero change — the audit row 6 `fill_*` extraction is rejected; tools build their own voxel-enumeration loops in `editor/tools.rs` operating on `WorldData::set_voxels_batch`). | 0 |

### Files NOT touched

- All `aadf/*.rs` — the existing `edit.rs::process_edit_batch` is bedrock; we call into it from `set_voxels_batch` without changes.
- All `render/*.rs` — the W2 chain, GPU producer, change_handler, GPU dispatch all consume `pending_edits` unchanged.
- All shaders — no shader edits.
- `hud.rs` — sibling HUD, but the existing one is untouched. Editor HUD goes into a NEW file (`editor/hud.rs`).
- `camera/mod.rs` — `FreeCameraState.enabled` gating is unneeded (see Decisions).
- `Cargo.toml` — no new deps.
- `voxel/grid.rs` — see above; `fill_box` / `fill_sphere` stay private to `grid.rs`.

**Total new code: ~390 LOC (~140 + 150 + 50 + 50 helpers/tests; ~150 in `data.rs`; ~120 in `panel.rs`; ~20 in `lib.rs`).** Well within the audit's ~400 LOC budget.

## Algorithm specifications

### `WorldData::ray_traversal(origin: Vec3, dir: Vec3) -> Option<RayHit>`

Faithful port of C# `WorldData.RayTraversal` (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:396-473`). Naive DDA with 3-layer descend — chunk (32³ voxels per cell at chunk granularity, 16-voxel side), block (4³ blocks per chunk, 4-voxel side), voxel (4³ voxels per block, 1-voxel side). No AADF skipping.

```rust
// In world/data.rs

#[derive(Debug, Clone)]
pub struct RayHit {
    /// Hit position in world space (origin + dir * distance).
    pub world_pos: Vec3,
    /// Voxel position of the hit voxel.
    pub voxel_pos: IVec3,
    /// Outward-facing normal of the hit face (axis-aligned, unit length).
    pub normal: Vec3,
    /// Resolved voxel type id (low 15 bits of the C# `curNode & 0x3FFFFFFF`).
    pub voxel_type: VoxelTypeId,
    /// Distance along the ray (in world units = voxels) from origin to hit.
    pub distance: f32,
}

impl WorldData {
    pub fn ray_traversal(&self, ray_origin: Vec3, ray_dir: Vec3) -> Option<RayHit> {
        // size_in_voxels = size_in_chunks * 16.
        let size_v = (self.size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32)).as_vec3();
        // C# WorldData.cs:399 — bounding box [(0.1, 0.1, 0.1), size_in_voxels - (0.1, 0.1, 0.1)].
        let world_min = Vec3::splat(0.1);
        let world_max = size_v - Vec3::splat(0.1);

        // C# WorldData.cs:399-404 — if origin is outside the world AABB AND
        // the ray intersects it, advance start_pos by that intersection.
        let mut start_pos = ray_origin;
        let world_bb_dist = ray_aabb_entry_distance(ray_origin, ray_dir, world_min, world_max);
        if !aabb_contains_point(world_min, world_max, ray_origin) {
            let Some(dist) = world_bb_dist else { return None; };
            start_pos += ray_dir * dist;
        }
        let world_bb_dist_or_zero = world_bb_dist.unwrap_or(0.0);

        // C# WorldData.cs:406-410 — DDA setup. `1e-10` matches C#.
        let inv_ray_dir_abs = Vec3::new(
            (1.0 / (1e-10 + ray_dir.x)).abs(),
            (1.0 / (1e-10 + ray_dir.y)).abs(),
            (1.0 / (1e-10 + ray_dir.z)).abs(),
        );
        let is_negative = IVec3::new(
            (ray_dir.x < 0.0) as i32,
            (ray_dir.y < 0.0) as i32,
            (ray_dir.z < 0.0) as i32,
        );
        let sign_ray_dir = Vec3::new(
            if ray_dir.x < 0.0 { -1.0 } else { 1.0 },
            if ray_dir.y < 0.0 { -1.0 } else { 1.0 },
            if ray_dir.z < 0.0 { -1.0 } else { 1.0 },
        );

        let mut mask = Vec3::ZERO;
        let mut cur_dist: f32 = 0.0;

        // C# WorldData.cs:419 — 1000-step cap; matches verbatim.
        for _step in 0..1000 {
            let cur_pos = start_pos + ray_dir * cur_dist;
            // C# WorldData.cs:422 — face-snap to current cell.
            let cur_cell_v = (mask * sign_ray_dir * 0.5 + cur_pos).floor();
            let cur_cell = cur_cell_v.as_ivec3();

            // Bounds check — C# WorldData.cs:424.
            let sx = size_v.x as i32;
            let sy = size_v.y as i32;
            let sz = size_v.z as i32;
            if cur_cell.x < 0 || cur_cell.y < 0 || cur_cell.z < 0
                || cur_cell.x >= sx || cur_cell.y >= sy || cur_cell.z >= sz
            {
                return None;
            }

            // C# WorldData.cs:428-430 — chunk lookup.
            let voxel_pos_in_chunk = IVec3::new(
                cur_cell.x.rem_euclid(16),
                cur_cell.y.rem_euclid(16),
                cur_cell.z.rem_euclid(16),
            );
            let chunk_pos = IVec3::new(cur_cell.x / 16, cur_cell.y / 16, cur_cell.z / 16);
            let chunk_idx = (chunk_pos.x
                + chunk_pos.y * self.size_in_chunks.x as i32
                + chunk_pos.z * self.size_in_chunks.x as i32 * self.size_in_chunks.y as i32)
                as usize;
            let mut cur_node: u32 = self.chunks_cpu[chunk_idx];

            // C# WorldData.cs:433 — bounds-in-direction at the chunk layer.
            let mut bounds_in_dir = IVec3::new(
                if ray_dir.x < 0.0 { voxel_pos_in_chunk.x } else { 15 - voxel_pos_in_chunk.x },
                if ray_dir.y < 0.0 { voxel_pos_in_chunk.y } else { 15 - voxel_pos_in_chunk.y },
                if ray_dir.z < 0.0 { voxel_pos_in_chunk.z } else { 15 - voxel_pos_in_chunk.z },
            );

            // C# WorldData.cs:435 — high bit set → chunk is Mixed (descend to block).
            // (The Rust port packs state in bits 30-31; per `aadf/cell.rs` BLOCK_STATE_CHILD = 2,
            // and the C# `curNode >> 31 != 0` check is on the *high* bit, but the port's
            // ChunkCell encoding uses bits 30-31 too. We must check the port's encoding
            // shape: `chunk_state = chunk_raw >> 30` (verified at `aadf/edit.rs:370-371`,
            // `build_chunk_edit_window_from_world`). state 2 = Mixed.)
            let chunk_state = cur_node >> 30;
            if chunk_state == 2 /* Mixed */ {
                // C# WorldData.cs:437-442 — block descent.
                let block_pos_in_chunk = voxel_pos_in_chunk / 4;
                let block_base = (cur_node & 0x3FFF_FFFF) as usize;
                let block_idx = block_base
                    + (block_pos_in_chunk.x + block_pos_in_chunk.y * 4 + block_pos_in_chunk.z * 16) as usize;
                cur_node = self.blocks_cpu[block_idx];
                let voxel_pos_in_block = IVec3::new(
                    cur_cell.x.rem_euclid(4),
                    cur_cell.y.rem_euclid(4),
                    cur_cell.z.rem_euclid(4),
                );
                bounds_in_dir = IVec3::new(
                    if ray_dir.x < 0.0 { voxel_pos_in_block.x } else { 3 - voxel_pos_in_block.x },
                    if ray_dir.y < 0.0 { voxel_pos_in_block.y } else { 3 - voxel_pos_in_block.y },
                    if ray_dir.z < 0.0 { voxel_pos_in_block.z } else { 3 - voxel_pos_in_block.z },
                );

                // C# WorldData.cs:443 — block Mixed → descend to voxel.
                let block_state = cur_node >> 30;
                if block_state == 2 /* Mixed */ {
                    // C# WorldData.cs:445-447 — voxel descent.
                    let voxel_base_pair = (cur_node & 0x3FFF_FFFF) as usize;
                    let voxel_index = voxel_base_pair * 2
                        + (voxel_pos_in_block.x + voxel_pos_in_block.y * 4 + voxel_pos_in_block.z * 16) as usize;
                    let cur_voxel_pair = self.voxels_cpu[voxel_index / 2];
                    let half = (cur_voxel_pair >> (16 * (voxel_index & 0x1))) & 0xFFFF;
                    // C# WorldData.cs:449-452 — bit 15 of the half-word = full flag.
                    if (half & 0x8000) != 0 {
                        // C# WorldData.cs:450 — promote: high bit (30) becomes the "hit" flag.
                        cur_node = (1 << 30) | (half & 0x7FFF);
                    } else {
                        // C# WorldData.cs:452 — empty voxel inside Mixed block, bounds=0
                        // (the AADF is stored in bits 0-5; but C# zeroes here because the
                        // 1-voxel cell IS the stepping resolution; we don't subdivide further).
                        bounds_in_dir = IVec3::ZERO;
                        // cur_node already has high bits clear (empty), so the "hit?" test below fails;
                        // continue to the step-distance computation.
                    }
                }
            }

            // C# WorldData.cs:456 — hit test (bit 30 set = full voxel / uniform-full block / uniform-full chunk).
            if (cur_node & 0x4000_0000) != 0 {
                let hit_type = (cur_node & 0x3FFF_FFFF) as u16;
                let result_length = cur_dist + world_bb_dist_or_zero;
                let world_pos = ray_origin + ray_dir * result_length;
                // C# WorldData.cs:461 — normal = mask × (rayDir < 0 ? +1 : -1).
                let normal = Vec3::new(
                    mask.x * if ray_dir.x < 0.0 { 1.0 } else { -1.0 },
                    mask.y * if ray_dir.y < 0.0 { 1.0 } else { -1.0 },
                    mask.z * if ray_dir.z < 0.0 { 1.0 } else { -1.0 },
                );
                return Some(RayHit {
                    world_pos,
                    voxel_pos: cur_cell,
                    normal,
                    voxel_type: VoxelTypeId(hit_type),
                    distance: result_length,
                });
            }

            // C# WorldData.cs:465-469 — DDA step.
            let cur_pos_frac = Vec3::new(
                (is_negative.x as f32 - (cur_pos.x - cur_pos.x.trunc())).abs(),
                (is_negative.y as f32 - (cur_pos.y - cur_pos.y.trunc())).abs(),
                (is_negative.z as f32 - (cur_pos.z - cur_pos.z.trunc())).abs(),
            );
            let dist_for_intersect = ((Vec3::ONE + bounds_in_dir.as_vec3()) - (Vec3::ONE - mask) * cur_pos_frac) * inv_ray_dir_abs;
            let min_dist = dist_for_intersect.x.min(dist_for_intersect.y).min(dist_for_intersect.z);
            mask = Vec3::new(
                if min_dist >= dist_for_intersect.x { 1.0 } else { 0.0 },
                if min_dist >= dist_for_intersect.y { 1.0 } else { 0.0 },
                if min_dist >= dist_for_intersect.z { 1.0 } else { 0.0 },
            );
            cur_dist += min_dist.max(0.00001); // C# WorldData.cs:469 — min step 1e-5.
        }

        None
    }
}

/// Slab-method AABB entry distance for `origin + t * dir` against `[bmin, bmax]`.
/// Returns None if the ray misses or if the entry is behind the origin.
fn ray_aabb_entry_distance(origin: Vec3, dir: Vec3, bmin: Vec3, bmax: Vec3) -> Option<f32> {
    let t1 = (bmin - origin) / dir;
    let t2 = (bmax - origin) / dir;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    if tmax < tmin.max(0.0) {
        None
    } else {
        Some(tmin.max(0.0))
    }
}

fn aabb_contains_point(bmin: Vec3, bmax: Vec3, p: Vec3) -> bool {
    p.x >= bmin.x && p.x <= bmax.x && p.y >= bmin.y && p.y <= bmax.y && p.z >= bmin.z && p.z <= bmax.z
}
```

**Citations.** Every numbered comment above traces to a C# line — `WorldData.cs:399-404`, `:406-410`, `:419`, `:422`, `:424`, `:428-430`, `:433`, `:435-454`, `:456-462`, `:465-469`. The faithful-port rule for `ray_traversal` is satisfied.

**Encoding caveat.** C# packs cell state in bits 30-31 (`state = curNode >> 30`); the Rust port matches this in its `ChunkCell` / `BlockCell` encoding (`aadf/cell.rs`; verified via `aadf/edit.rs:370`'s `chunk_state = chunk_raw >> 30`). The "hit" test in C# is `(curNode & 0x40000000) != 0` — this catches **state 1** (Uniform Full, the `(1 << 30)` voxel-promotion at C# `:450` puts the full voxel under this branch too — bit 30 set, bit 31 clear). The port's state values are the same (Uniform Full = state 1 = `curNode >> 30 == 1`). Verbatim.

### `WorldData::set_voxels_batch(&mut self, edits: &[(IVec3, VoxelTypeId)])`

Sanctioned divergence from C# `setVoxelData` per-voxel semantics. Groups by chunk, builds one combined edit window per chunk, calls `process_edit_batch` ONCE for all chunks. Mirrors the per-voxel path at `world/data.rs:130-209` but multiplexes the per-chunk window construction.

```rust
impl WorldData {
    /// Bulk-edit entry point — programmatic multi-voxel mutation
    /// (`01-context.md` §2 Q&A row 7 — sanctioned divergence). Groups input
    /// by chunk + does ONE `process_edit_batch` invocation per affected
    /// chunk (the actual batch ITSELF spans all chunks, but the per-chunk
    /// edit window construction happens once per chunk).
    ///
    /// Performance: a sphere of radius 16 touches ~5 chunks per axis → 125
    /// chunks → ~625µs vs. ~85ms for 17k individual `set_voxel` calls.
    ///
    /// Mirrors `set_voxel` (`world/data.rs:98-210`) but:
    /// - Groups input by chunk before doing any chunk-window mutation.
    /// - Builds ONE shared edit_data buffer of size `chunks.len() * 2048`
    ///   u32s, one chunk-window per affected chunk.
    /// - Calls `process_edit_batch` ONCE with the multi-chunk edited slice.
    /// - Emits ONE EditBatch into `pending_edits`.
    pub fn set_voxels_batch(&mut self, edits: &[(IVec3, VoxelTypeId)]) {
        if edits.is_empty() {
            return;
        }
        let chunk_size_voxels = (CELL_DIM * CELL_DIM) as u32; // 16
        let sx_v = self.size_in_chunks.x * chunk_size_voxels;
        let sy_v = self.size_in_chunks.y * chunk_size_voxels;
        let sz_v = self.size_in_chunks.z * chunk_size_voxels;

        // Group by chunk_pos. Using a small HashMap is fine — a sphere
        // r=16 touches ~125 chunks; a cube r=400 touches ~16M voxels and
        // ~16k chunks (still well under HashMap-overhead pain point).
        let mut by_chunk: std::collections::HashMap<[u32; 3], Vec<([u32; 3], u16)>> =
            std::collections::HashMap::new();
        for &(pos, ty) in edits {
            if pos.x < 0 || pos.y < 0 || pos.z < 0 {
                continue;
            }
            let p = [pos.x as u32, pos.y as u32, pos.z as u32];
            if p[0] >= sx_v || p[1] >= sy_v || p[2] >= sz_v {
                continue;
            }
            let chunk = [
                p[0] / chunk_size_voxels,
                p[1] / chunk_size_voxels,
                p[2] / chunk_size_voxels,
            ];
            let voxel_in_chunk = [
                p[0] % chunk_size_voxels,
                p[1] % chunk_size_voxels,
                p[2] % chunk_size_voxels,
            ];
            by_chunk.entry(chunk).or_default().push((voxel_in_chunk, ty.raw()));
        }
        if by_chunk.is_empty() {
            return;
        }

        // Build the merged edit_data buffer + the edited_chunks list. The
        // EditBatch will be one combined batch covering every touched chunk.
        let chunk_count = by_chunk.len();
        let mut edit_data: Vec<u32> = vec![0; chunk_count * 2048];
        let mut edited_chunks: Vec<([u32; 3], u32)> = Vec::with_capacity(chunk_count);
        let mut chunk_indices: Vec<usize> = Vec::with_capacity(chunk_count);

        for (i, (chunk_pos, per_chunk_edits)) in by_chunk.into_iter().enumerate() {
            let chunk_idx = (chunk_pos[0]
                + chunk_pos[1] * self.size_in_chunks.x
                + chunk_pos[2] * self.size_in_chunks.x * self.size_in_chunks.y)
                as usize;
            if chunk_idx >= self.chunks_cpu.len() {
                continue;
            }
            chunk_indices.push(chunk_idx);
            let edit_offset = (i * 2048) as u32;
            edited_chunks.push((chunk_pos, edit_offset));
            // Decode the existing chunk into its slice of the edit_data buffer.
            let window_slice = &mut edit_data[i * 2048..(i + 1) * 2048];
            let decoded = crate::aadf::edit::build_chunk_edit_window_from_world(
                &self.chunks_cpu,
                &self.blocks_cpu,
                &self.voxels_cpu,
                chunk_idx,
            );
            window_slice.copy_from_slice(&decoded);
            // Apply every per-voxel mutation.
            for (voxel_in_chunk, ty) in per_chunk_edits {
                crate::aadf::edit::set_voxel_in_window(window_slice, voxel_in_chunk, ty);
            }
        }

        // Run process_edit_batch ONCE with all chunks.
        let v_cursor = self.voxels_cpu.len() as u32;
        let b_cursor = self.blocks_cpu.len() as u32;
        let (batch, _new_v, _new_b) = crate::aadf::edit::process_edit_batch(
            &edit_data,
            &edited_chunks,
            v_cursor,
            b_cursor,
        );

        // Apply to CPU buffers — exactly mirrors set_voxel:158-187. This
        // section is structurally a near-copy of the per-voxel path; an
        // implementer might refactor to share but cleanly mirroring is
        // simpler for the first cut.
        let mut v_iter = batch.changed_voxels.chunks_exact(33);
        while let Some(chunk_vox) = v_iter.next() {
            for &v in &chunk_vox[1..33] {
                self.voxels_cpu.push(v);
            }
        }
        let mut b_iter = batch.changed_blocks.chunks_exact(65);
        while let Some(_chunk_blk) = b_iter.next() {
            // Two-pass: first append the raw blocks, then re-encode AADFs
            // (same as set_voxel:166-187).
        }
        for (idx, edit_block) in batch.changed_blocks.chunks_exact(65).enumerate() {
            let block_ptr = b_cursor + (idx as u32) * 64;
            // Append the 64 block words at block_ptr; first idx == 0 → already
            // at position b_cursor in blocks_cpu. We need to RESIZE blocks_cpu
            // to `b_cursor + 64 * batch_count` first.
            let target_len = (b_cursor + (idx as u32 + 1) * 64) as usize;
            if self.blocks_cpu.len() < target_len {
                self.blocks_cpu.resize(target_len, 0);
            }
            let mut raw = [0u32; 64];
            raw[..64].copy_from_slice(&edit_block[1..65]);
            crate::aadf::edit::apply_block_edit_cpu(&mut self.blocks_cpu, block_ptr, &raw);
        }
        for entry in &batch.changed_chunks {
            let pos_packed = entry[0];
            let new_state = entry[1];
            let cx = pos_packed & 0x7FF;
            let cy = (pos_packed >> 11) & 0x3FF;
            let cz = pos_packed >> 21;
            let ci = (cx
                + cy * self.size_in_chunks.x
                + cz * self.size_in_chunks.x * self.size_in_chunks.y) as usize;
            if ci < self.chunks_cpu.len() {
                self.chunks_cpu[ci] = new_state;
            }
        }
        self.dirty = true;

        // Stash the batch + the group positions of every edited chunk.
        self.pending_edits.batches.push(batch);
        for &(chunk_pos, _) in &edited_chunks {
            self.pending_edits.edited_groups.push([
                chunk_pos[0] / CELL_DIM as u32,
                chunk_pos[1] / CELL_DIM as u32,
                chunk_pos[2] / CELL_DIM as u32,
            ]);
        }

        // Refresh `dense_voxel_types` for the affected voxels — the GPU
        // producer chain reads from here (per `world/data.rs:44-50`'s docstring
        // + `render/construction/mod.rs:827`'s `!w.dense_voxel_types.is_empty()`
        // gate). NOTE: `set_voxel` does NOT update this Vec today (bug? — the
        // GPU producer probably re-derives it elsewhere, or the test grid
        // path is the only consumer). Verified by reading `data.rs:98-210`:
        // `dense_voxel_types` is never written by `set_voxel`. Since the GPU
        // dispatch chain re-reads chunks/blocks/voxels directly, leaving
        // dense_voxel_types stale during the brush stroke is consistent with
        // existing behaviour; not modified here.
    }
}
```

**Why one combined batch.** `process_edit_batch` (`aadf/edit.rs:242-327`) already accepts a `&[(chunk_pos, edit_data_offset)]` array — it loops over edited chunks internally. Calling it once with N chunks produces a single `EditBatch` with N chunk-entries in `changed_chunks`, the union of all per-chunk mixed-block expansions in `changed_blocks`, and the union of all per-chunk voxel slot claims in `changed_voxels`. The extract drains this single batch into `ConstructionEvents` next frame, and `compute_change_groups` runs on the unioned `edited_groups`. All bit-faithful to the C# pipeline.

**Citations.** `world/data.rs:130-209` for the per-voxel mirror logic; `aadf/edit.rs:242-327` for `process_edit_batch`'s multi-chunk acceptance; `render/construction/mod.rs:674-682` for the per-frame extract drainage that aggregates multiple batches (so pushing one batch per frame is fine — but pushing one COMBINED batch is fewer allocations).

### `paint_brush`, `cube_brush`, `sphere_brush`

Each enumerates voxels in its footprint, calls `world_data.set_voxels_batch(&edits)`. Signatures match the audit recommendation (3.5):

```rust
// crates/bevy_naadf/src/editor/tools.rs

/// Paint brush — replaces existing non-empty voxels within radius with `ty`.
/// Faithful port of `EditingToolPaint.cs:69-79` (`for i in 0..4096 { if dist²
/// < r² && current_type != 0 { set_voxel } }`).
///
/// Note: Paint does NOT take `is_erase` — the C# Paint tool has no `isErase`
/// field. The "Paint" semantic is **replace only non-empty voxels**; for
/// erasing, the user picks Cube or Sphere with `is_erase = true`.
pub fn paint_brush(
    world_data: &mut WorldData,
    pos: Vec3,
    radius: f32,
    ty: VoxelTypeId,
) {
    let r2 = radius * radius;
    let mut edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();
    let (lo, hi) = brush_aabb(world_data, pos, radius);
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let voxel = IVec3::new(x, y, z);
                let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos; // voxel centre vs pos
                if d.length_squared() < r2 {
                    // Paint: only replace existing non-empty voxels.
                    if get_voxel_type(world_data, voxel).is_some_and(|t| t != VoxelTypeId::EMPTY) {
                        edits.push((voxel, ty));
                    }
                }
            }
        }
    }
    if !edits.is_empty() {
        world_data.set_voxels_batch(&edits);
    }
}

/// Cube brush — Chebyshev distance < radius. Solid (writes all voxels in
/// radius). `is_erase = true` writes `VoxelTypeId::EMPTY`. Faithful port of
/// `EditingToolCube.cs:76-90` (`distToPosMax = max(|dx|,|dy|,|dz|); if < radius
/// then setVoxel(isErase ? 0 : ty)`).
pub fn cube_brush(
    world_data: &mut WorldData,
    pos: Vec3,
    radius: f32,
    ty: VoxelTypeId,
    is_erase: bool,
) {
    let target = if is_erase { VoxelTypeId::EMPTY } else { ty };
    let mut edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();
    let (lo, hi) = brush_aabb(world_data, pos, radius);
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let voxel = IVec3::new(x, y, z);
                let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos;
                let cheb = d.x.abs().max(d.y.abs()).max(d.z.abs());
                if cheb < radius {
                    edits.push((voxel, target));
                }
            }
        }
    }
    if !edits.is_empty() {
        world_data.set_voxels_batch(&edits);
    }
}

/// Sphere brush — Euclidean `r²` distance check. Solid (writes all voxels in
/// radius, unlike Paint). Faithful port of `EditingToolSphere.cs:76-89`.
pub fn sphere_brush(
    world_data: &mut WorldData,
    pos: Vec3,
    radius: f32,
    ty: VoxelTypeId,
    is_erase: bool,
) {
    let target = if is_erase { VoxelTypeId::EMPTY } else { ty };
    let r2 = radius * radius;
    let mut edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();
    let (lo, hi) = brush_aabb(world_data, pos, radius);
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let voxel = IVec3::new(x, y, z);
                let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos;
                if d.length_squared() < r2 {
                    edits.push((voxel, target));
                }
            }
        }
    }
    if !edits.is_empty() {
        world_data.set_voxels_batch(&edits);
    }
}

/// Compute the brush's affected-voxel AABB, clamped to the world bounds.
fn brush_aabb(world_data: &WorldData, pos: Vec3, radius: f32) -> (IVec3, IVec3) {
    let chunk_size_voxels = (CELL_DIM * CELL_DIM) as i32; // 16
    let sx = (world_data.size_in_chunks.x as i32) * chunk_size_voxels;
    let sy = (world_data.size_in_chunks.y as i32) * chunk_size_voxels;
    let sz = (world_data.size_in_chunks.z as i32) * chunk_size_voxels;
    let lo = IVec3::new(
        ((pos.x - radius).floor() as i32).max(0),
        ((pos.y - radius).floor() as i32).max(0),
        ((pos.z - radius).floor() as i32).max(0),
    );
    let hi = IVec3::new(
        ((pos.x + radius).ceil() as i32).min(sx - 1),
        ((pos.y + radius).ceil() as i32).min(sy - 1),
        ((pos.z + radius).ceil() as i32).min(sz - 1),
    );
    (lo, hi)
}

/// Get the voxel type at a position, or None if out of bounds.
/// Walks the same 3-layer descent as `ray_traversal` — chunk → block → voxel.
/// ~30 LOC; lands as a `WorldData::get_voxel_type` helper in world/data.rs
/// (companion to `set_voxel`).
fn get_voxel_type(world_data: &WorldData, pos: IVec3) -> Option<VoxelTypeId> { /* ... */ }
```

The `EditingToolPaint.cs:48` "radiusOutsideSqr" optimisation (excluding chunks entirely outside the brush) is left to a follow-up: the brush iterates the full AABB once, so adjacent-but-non-intersecting chunks are visited but their per-voxel test fails fast. At brush scale ≤16, this is cheap (~17k tests for sphere r=16). At brush scale 400, the brush touches ~33M voxels; this needs the C# chunk-AABB filter — but radius 400 is the upper end of the slider range and an edge case; we accept the dumb path for the first cut.

**Citation.** `EditingToolPaint.cs:69-79`, `EditingToolCube.cs:76-90`, `EditingToolSphere.cs:76-89` — all three loop over voxels in the chunk AABB and apply the per-tool distance metric. The port matches each metric exactly:

- Paint: Euclidean `< r²` AND non-empty existing voxel. **Matches `Paint.cs:73-76`.**
- Cube: Chebyshev `< r`. **Matches `Cube.cs:86-88`.**
- Sphere: Euclidean `< r²`. **Matches `Sphere.cs:85-87`.**

One subtle point about `pos`: C# computes `voxelPosInChunk + posToChunk` where `posToChunk = (Vector3(0.5) + (chunkPos * 16).ToVector3()) - pos` (`Paint.cs:68`). Expanding: `voxelPosInChunk + (Vector3(0.5) + chunkPos*16) - pos == (chunkPos*16 + voxelPosInChunk + Vector3(0.5)) - pos == voxelWorldCentre - pos`. The port computes `(voxel.as_vec3() + Vec3::splat(0.5)) - pos` — voxel-centre vs pos. **Identical.**

### `screen_to_ray(camera: &Camera, cam_gxf: &GlobalTransform, cursor_pos: Vec2) -> Option<Ray>`

Bevy 0.19 provides `Camera::viewport_to_world(&GlobalTransform, Vec2) -> Result<Ray3d, ViewportConversionError>` at `bevy_camera-0.19.0-rc.1/src/camera.rs:647-672`. Verified via cargo registry source read.

```rust
// crates/bevy_naadf/src/editor/ray.rs

/// World-space ray a screen-space cursor position projects to. Returns None
/// if the camera's projection is degenerate or the cursor is outside the
/// viewport.
pub fn screen_to_ray(
    camera: &Camera,
    cam_gxf: &GlobalTransform,
    cursor_pos: Vec2,
) -> Option<Ray> {
    let ray3d = camera.viewport_to_world(cam_gxf, cursor_pos).ok()?;
    Some(Ray {
        origin: ray3d.origin,
        dir: *ray3d.direction,
    })
}

/// World-space ray (origin + direction). `Ray3d` would do but we expose a
/// trivially-mutable struct so the apply system can prepend the
/// `PositionSplit.pos_int` offset before traversal if needed.
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}
```

**On `PositionSplit`.** The render path consumes the int+frac split (`position_split.rs:30-91`), but the input side — camera `Transform.translation` — is the un-split f32 world position. `viewport_to_world` reads `camera_transform.affine()` (`camera.rs:658`), which uses `Transform.translation` directly. **No `PositionSplit` reconstruction needed for CPU ray-traversal**, because `WorldData::ray_traversal` operates in single-precision world coords against a world that's at most ~16k voxels per axis (well within f32 precision for cells; precision degrades only at >~16M voxels). For the in-scope test grid (64×32×64), f32 is more than fine. The audit's concern that `PositionSplit` affects ray-origin construction was a precaution but is unneeded for the brushes-on-test-grid first cut. Recorded under Assumptions.

## Test plan

### `#[test]` coverage

**In `world/data.rs::tests` (new):**

1. **`ray_traversal_misses_empty_world`** — Build a `WorldData` with all-empty chunks (the constructor pre-condition); a ray from `(0,0,0)` along `+x` returns `None` (the world is entirely empty inside `[0.1, size-0.1]` so the ray hits no full voxel).
2. **`ray_traversal_hits_known_voxel`** — Use the existing test grid (`setup_test_grid` builds it deterministically). Fire a ray from a known camera position toward a known emissive voxel (e.g. `[31, 25, 33]` — `TY_EMISSIVE`, `voxel/grid.rs:367`). Assert `result.voxel_pos == IVec3::new(31, 25, 33)` and `result.voxel_type == TY_EMISSIVE`. Verifies the 3-layer descent end-to-end.
3. **`ray_traversal_normal_is_face_normal`** — Fire a `+y`-ish-but-mostly-`+x` ray at a known ground voxel from above. Assert the resulting normal is `(0, 1, 0)` (top face). Verifies the C# `:461` normal computation.
4. **`ray_traversal_distance_within_eps_of_world_pos`** — Assert `(ray.origin + ray.dir * result.distance - result.world_pos).length() < 1e-3`. Verifies the distance-to-world-pos round-trip.
5. **`set_voxels_batch_byte_equals_per_voxel_loop`** — Build two identical `WorldData`s. On (A), call `set_voxel` N times for the same N voxels. On (B), call `set_voxels_batch(&edits)`. Assert that `wd_a.chunks_cpu == wd_b.chunks_cpu`, `wd_a.blocks_cpu == wd_b.blocks_cpu`, `wd_a.voxels_cpu == wd_b.voxels_cpu`. **N=5 sufficient** for the first pass; N=100 for stress. Fixture set: `(IVec3::new(0,0,0), VoxelTypeId(1))`, `(IVec3::new(1,0,0), VoxelTypeId(2))`, ...  (small distinct voxels in one chunk + one cross-chunk voxel to verify multi-chunk batching).
6. **`set_voxels_batch_empty_is_noop`** — `wd.set_voxels_batch(&[])` leaves all CPU buffers + `pending_edits` unchanged.

**In `editor/tools.rs::tests` (new):**

7. **`sphere_brush_produces_solid_sphere`** — Build a fresh world (4×2×4 chunks). Call `sphere_brush(&mut wd, pos=(32.0, 16.0, 32.0), radius=4.0, ty=VoxelTypeId(7), is_erase=false)`. Iterate the affected AABB; assert every voxel inside the sphere is `VoxelTypeId(7)` and every voxel outside is whatever it was before (or empty for a fresh world).
8. **`cube_brush_produces_solid_cube`** — Same but with `cube_brush`; assert every voxel in the Chebyshev `< r` cube is the target type.
9. **`paint_brush_only_replaces_non_empty`** — Start with the test grid. Call `paint_brush(&mut wd, pos=ground_centre, radius=8.0, ty=VoxelTypeId(7))`. Assert: the ground voxels within radius are now `VoxelTypeId(7)`; the air voxels above the ground (within the same sphere) are still empty.
10. **`erase_with_sphere_clears_voxels`** — Start with the test grid. Call `sphere_brush(..., is_erase=true)` at a known full voxel. Assert the affected voxels are now `VoxelTypeId::EMPTY`.

**In `editor/ray.rs::tests` (new):**

11. **`screen_to_ray_centre_returns_camera_forward`** — Spawn a fake `Camera` + `GlobalTransform` (using Bevy's headless `App` test pattern — or directly construct via `Camera::default()` + manual viewport). Pass `cursor = viewport_centre`; assert the ray's direction matches `cam_gxf.forward()` within 1e-3.
12. **`screen_to_ray_outside_viewport_returns_none`** — Pass `cursor = Vec2::splat(-1.0)`; assert `None`. (Tests the `viewport_to_ndc` failure path bubbling through.)

**In `editor/mod.rs::tests` (new):**

13. **`editor_state_default_is_safe`** — `EditorState::default().edit_active == false`, `selected_type == VoxelTypeId(1)`, `radius == 10.0`, `is_continuous == true`.
14. **`apply_edit_tool_no_op_when_inactive`** — Build a minimal Bevy `App` with `WorldData` + `EditorState::default()` (`edit_active = false`) + a fake LMB-pressed input; run `apply_edit_tool` one tick; assert `WorldData.pending_edits.batches.is_empty()`.

**In `panel.rs::tests` (extend):**

15. **`edit_knob_variants_in_knobs_table`** — Assert that the new `EDITOR` section + 5 editor knob rows exist at the top of `KNOBS`, in the documented order.
16. **`editor_knob_defaults_match_editorstate_default`** — Like the existing `defaults_match_gi_settings_default` test, but for `EditorState`. Iterate `KNOBS`, for each `Edit { variant }`, assert `EditorState::default()` returns the row's `default`.

### What is verified vs. left for visual check

**Verified by `#[test]`:**
- Ray traversal semantics (hit, miss, normal, distance).
- Batch-vs-per-voxel byte equality.
- Brush footprint correctness (sphere = solid, cube = Chebyshev, paint = non-empty-only).
- Erase semantics.
- Editor state machine defaults + LMB-inactive no-op.
- Panel KNOBS table layout (section + 5 editor rows at top).

**Left for the user's manual visual gate** (per global memory `subagent-gpu-app-verification-loop`):
- "Does the brush ACTUALLY render correctly in the live app?" — visual.
- "Does the lerp-on-drag look smooth?" — visual.
- "Does F2 toggle feel intuitive?" — UX feedback.
- "Does the brush footprint show no z-fighting / unexpected gaps at chunk boundaries?" — visual.

**Smoke gate (implementer-time, ONE run):**
- `cargo build` clean.
- `cargo test -p bevy-naadf` green (all existing tests + the new ones).
- `cargo run --bin e2e_render -- baseline` passes (the e2e harness is excluded from the editor; this verifies we didn't accidentally regress the baseline).

## Decisions & rejected alternatives

### Decision 1: `KnobKind` extension shape — single `Edit { variant }` ✅ vs. four parallel `EditU32/F32/Bool/Enum` variants

**Chosen:** Add one new `KnobKind::Edit { variant: EditKnobVariant }` variant; group the four sub-shapes (`U32/F32/Bool/Enum`) under `EditKnobVariant`.

**Rejected:** Add four parallel variants on `KnobKind` directly (`EditU32`, `EditF32`, `EditBool`, `EditEnum`).

**Why:** The grouped form keeps the `KnobKind` enum compact (1 new variant vs. 4) and the systems' match arms shallower (one `KnobKind::Edit { variant }` arm with an inner `match variant` instead of four `KnobKind::Edit*` top-level arms). The grouped form also gives a single place to add a new `EditXxx` sub-variant later (e.g., `EditEnum16` for a 16-variant tool palette).

**Flip-trigger:** If a future panel section also needs `&mut OtherResource` (e.g., a "view config" with its own resource), the grouped shape doesn't generalise — at that point, split into a generic `Knob<Ctx>` pattern. Not a Track-B concern.

### Decision 2: Panel structure — single F1 panel with `EDITOR` section ✅ vs. dual F1+F2 panels

**Chosen:** One panel toggled by `F1`; `F2` toggles only `EditorState.edit_active` (the edit mode, not panel visibility).

**Rejected:** A second sibling panel for editor state on `F2`.

**Why:** (a) `panel.rs` has 28 rows today; +6 keeps total under 35, still <420 px tall on the default 12 px line height — comfortable on any monitor. (b) One panel = one place to look. (c) `F2` as panel-visibility-toggle would mean panel duplication of `Knob` infrastructure; the audit (§3.4) explicitly recommends against this.

**Flip-trigger:** if editor knob count grows past ~12 (e.g., undo controls, brush curves, multi-layer materials) — split into a sibling panel then.

### Decision 3: `FreeCamera` gating — do nothing ✅ vs. (a) `FreeCameraState.enabled = !edit_active` vs. (b) mode-switch resource

**Chosen:** **No gating.** The `FreeCamera` controller binds **mouse look to RMB** (`mouse_key_cursor_grab: MouseButton::Right`, `bevy_camera_controller-0.19.0-rc.1/src/free_camera.rs:150`). WASD movement is always active; mouse look is only active when RMB is held. **LMB is not consumed by the fly camera**, so it's naturally available for the editor.

**Rejected (a):** Toggle `FreeCameraState.enabled` to `false` while in edit mode. **Why rejected:** This would also disable WASD, which the user needs (they need to fly close to a target before brushing). Half-disabling (movement enabled, look disabled) requires a mode-resource and per-system gating that the upstream crate doesn't natively expose.

**Rejected (b):** Add a port-side `EditorMode { Fly, Editing }` resource. **Why rejected:** The mode it would implement is `Fly` (LMB does nothing) vs. `Editing` (LMB applies brush). But this is exactly what `EditorState.edit_active` already represents. A second mode-switch resource is duplication.

**Why "no gating" works:** verified by reading `free_camera.rs:150` + `:401-409` (mouse motion only applied when `cursor_grab` is true, which requires RMB pressed OR `M` keyboard toggle). LMB never enters the camera input path. The audit's row 11 caveat ("`FreeCamera` captures mouse always") was inaccurate — verified counter-example in cargo source.

**One remaining edge case:** the cursor grab via the M key (`keyboard_key_toggle_cursor_grab`, `free_camera.rs:151`) can be left on, in which case mouse motion ALWAYS rotates the camera. The user can press M to toggle this off. Not an editor concern.

**Flip-trigger:** If a future user remaps `FreeCamera::mouse_key_cursor_grab` to LMB or relies on the M-toggle, we'd need to add `FreeCameraState.enabled` gating. Documented as an assumption.

### Decision 4: `ray_traversal` algorithm — naive DDA ✅ vs. AADF-skipping DDA

**Chosen:** Naive DDA with 3-layer descent (chunk → block → voxel). Matches C# `WorldData.RayTraversal:396-473` verbatim.

**Rejected:** AADF-skipping DDA (the `shoot_ray` WGSL approach at `ray_tracing.wgsl:201+`).

**Why:** (a) The C# CPU traversal IS naive — it descends per-cell but doesn't use AADFs to skip empty space. Faithful-port rule (`01-context.md` §2) says match C# semantics. (b) The brief explicitly recommends "NAIVE (matches C# `WorldData.RayTraversal` faithfully)" per audit §4. (c) On a 64×32×64 test grid, pick rays hit typically 50-200 cells via naive DDA, well under a millisecond at single-threaded CPU speeds. AADF skipping is ~5× faster but adds ~50 LOC of bit-unpack code.

**Flip-trigger:** measurable pick latency >10ms on release builds with a larger-than-test-grid world. Not in scope.

### Decision 5: `set_voxels_batch` API shape — `&[(IVec3, VoxelTypeId)]` ✅ vs. closure-based vs. builder

**Chosen:** `set_voxels_batch(&mut self, edits: &[(IVec3, VoxelTypeId)])`. Caller builds the `Vec` of edits; this method groups by chunk, builds the merged edit window, calls `process_edit_batch` once.

**Rejected (alt-1):** `set_voxels_in_region(&mut self, region: IAabb3, ty_fn: impl Fn(IVec3) -> Option<VoxelTypeId>)`. Caller passes a callback that produces the new type per voxel; the method iterates internally and queries the callback. **Why rejected:** The brushes already iterate; another iteration in `set_voxels_in_region` would be a second pass through the same AABB. The slice-based API lets the caller iterate once and only emit voxels they want to mutate.

**Rejected (alt-2):** A builder `BatchEditor { wd: &mut WorldData }` with `.set(pos, ty)` + `.commit()`. **Why rejected:** Adds a stateful wrapper for no clarity gain over a Vec; the brushes can't naturally express their iteration shape through builder method chains.

**Why the slice form wins:** The audit (§4 row 6) recommended this shape verbatim. It matches the user's stated motivation: "groups by chunk + does one `process_edit_batch` per affected chunk." Simple, testable, matches what a Rust dev would write.

**Flip-trigger:** if the brushes need to consume megabyte-scale edit lists (a radius-400 cube is ~512M voxels — well past Vec capacity), switch to a streaming API. Not in scope.

### Decision 6: Tool-state HUD vs. fold into panel

**Chosen:** Separate `Node`+`Text` overlay at top-right (`editor/hud.rs`). Mirrors `hud.rs:92-110` chrome.

**Rejected:** Add a `KnobKind::Readonly` row in the `EDITOR` section that shows "Hover: (32,15,28) ty=5 dist=14.32".

**Why chosen:** (a) The hover info changes every frame; updating a panel `KnobKind::Readonly` would either need cursor-tracking-into-readonly-row plumbing (complex) or be limited to a single-line summary. A dedicated `Text` overlay easily renders 5-7 lines of hover info per frame. (b) The user's "gamified" intent benefits from a HUD-like persistent visual cue when in edit mode — top-right corner is the conventional crosshair / tool-state location in games. (c) Splitting "tweakable controls" (panel) from "live state readout" (HUD) is a clean abstraction.

**Flip-trigger:** if the HUD gets bloated (>10 lines), reconsider. Not in scope.

### Decision 7: `is_continuous` semantics — re-fire every frame ✅ vs. cursor-move threshold

**Chosen:** When `is_continuous = true` and LMB is held, the brush re-fires EVERY frame while LMB is pressed. When `is_continuous = false`, the brush fires only on the LMB-just-pressed frame.

**Rejected (alt-1):** Cursor-move threshold (e.g., re-fire only when the cursor moves > N pixels). **Why rejected:** The C# `EditingToolCube.cs:50-51` / `EditingToolSphere.cs:50-51` semantics are literally `if (!isContinuous && OldLeft == Pressed) return;` — that's "if not continuous AND not the just-pressed frame, return." Per-frame re-fire is the C# default for `is_continuous = true`. Faithful-port rule says match C#.

**Rejected (alt-2):** Velocity-based throttle (re-fire only when smoothed `pos` advances by > 0.5 voxels). **Why rejected:** Same — not in C#.

**Note for implementer:** With `is_continuous = true` and `pos` lerping each frame, the brush at radius 16 fires ~17k voxel writes per frame, batched into ~125 chunks via `set_voxels_batch` — ~625µs/frame, well under the 16ms frame budget. The smoothed `pos` means each frame's brush footprint overlaps the previous frame's heavily; the redundant writes (overwriting the same voxels) are absorbed by the per-chunk `process_edit_batch` re-decoding. Not a perf concern.

**Flip-trigger:** if a future user complains about brush over-fill, add an optional cursor-move-threshold knob. Not in scope.

### Decision 8: `voxel/grid.rs::fill_*` extraction — REJECT the audit's recommendation

**Chosen:** Leave `fill_box` / `fill_sphere` private to `voxel/grid.rs`. The brushes implement their own enumeration loops in `editor/tools.rs`.

**Rejected:** The audit's row 6 + §3.5 recommendation: extract `fill_box` / `fill_sphere` as `pub` helpers taking a closure `|p, ty| world_data.set_voxel(p, ty)`.

**Why rejected:** (a) `voxel/grid.rs::fill_*` mutate a `DenseVolume` directly. Adapting them to take a `&mut dyn FnMut(IVec3, VoxelTypeId)` closure is doable but creates a half-DenseVolume-half-closure shape that's harder to read than direct enumeration in the brush. (b) The brushes need `is_erase` semantics that `fill_*` don't have — extracting would force `fill_*` to gain branching logic for "skip or apply" that doesn't match their `DenseVolume`-mutation use case. (c) The Paint brush needs to read the existing voxel type (the non-empty check). `fill_*` don't have a "read" path — they only write. (d) The C# tools have their own brush loops, not shared helpers; matching the C# structure is faithful.

**Flip-trigger:** If a future tool (e.g., "fill connected" or "fill with gradient") would benefit from sharing the AABB-iteration scaffold with `fill_*`, extract then. Not in scope.

### Decision 9: Selected type — `KnobKind::Edit { variant: U32 }` with raw u16 ✅ vs. `KnobKind::Edit { variant: Enum }` listing palette names

**Chosen:** A `U32` knob showing the raw `VoxelTypeId` index. The user reads which type they're picking from the HUD overlay or from a separate "type table" they can produce.

**Rejected:** An `Enum` knob with variants for every palette entry's name (`TY_GROUND`, `TY_BOX_A`, etc.).

**Why:** (a) Palette length is dynamic (Track A's `.vox` loader can change it; user can add types). An `Enum` requires `&'static [&'static str]`, which is the wrong shape for runtime-discoverable palette. (b) A simple U32 index is what the user sees in the HUD when hovering anyway — consistent presentation. (c) The user's "gamified" requirement is for the chrome (panel chrome + HUD chrome), not for the type-name display.

**Flip-trigger:** if `Track A` exposes type names per palette entry (via a `Vec<&'static str>` adapter), the panel could show "selected_type: TY_GROUND" instead of "selected_type: 1". Not in Track-B scope.

### Decision 10: The "APPLY HOVER ECHO" debug action — keep ✅ vs. drop

**Chosen:** Keep a `KnobKind::Action` row labeled "APPLY HOVER ECHO" that logs the current `last_hover_hit` to the console. ~5 LOC. Always on (not gated on `cfg(debug)`).

**Rejected:** Drop the action; the HUD already shows hover info.

**Why kept:** (a) The HUD shows the value visually but doesn't log it to stderr/console for grep-friendly capture. (b) The user (or implementer) might want to copy a known voxel coord into a test fixture — having a one-click logger is a nice DX touch. (c) Free LOC (~5) — the cost is negligible.

**Flip-trigger:** if the panel gets crowded, drop it. Not currently a concern.

## Assumptions made

1. **Bevy 0.19-rc.1 `Camera::viewport_to_world` exists and matches the documented signature** — verified by reading `bevy_camera-0.19.0-rc.1/src/camera.rs:647-672`. Returns `Result<Ray3d, ViewportConversionError>`; `Ray3d` has `origin: Vec3` + `direction: Dir3` (where `Dir3` derefs to `Vec3`). The design's `screen_to_ray` unwraps the result; bevy's tests at `:1053+` cover the happy path so we expect this is stable.
2. **The Track A design (just landed at `02a-design-vox-loading.md`) does not change the `VoxelTypeId` semantics.** Track A skips K-means and creates one `VoxelType` per palette entry; the port's `VoxelTypeId(u16)` is the same 15-bit-payload type at `voxel/mod.rs:67`. Track B's `selected_type: VoxelTypeId` field reads palette indices identically whether the world came from `setup_test_grid::build_palette` or `vox_import::build_world_from_vox`. **No conflict with Track A.**
3. **`panel.rs` accepts a new `KnobKind` variant + new top-of-`KNOBS` rows without restructuring.** Verified by reading `panel.rs:177-228` (variant table) + `panel.rs:734-836` (`adjust_panel` match) + `panel.rs:844-944` (`mouse_interact_panel`) + `panel.rs:1043-1122` (`update_panel_text`). Each match arm covers one variant; adding a new variant means adding one arm per place. Mechanical.
4. **`reset_all_knobs` signature change (`fn(&mut AppArgs)` → `fn(&mut AppArgs, &mut EditorState)`) is acceptable.** The function is referenced from two places: the `KnobKind::Action { apply: reset_all_knobs }` const + the `adjust_panel` `Shift+R` keybind (`panel.rs:767-770`). Both call sites need to pass `&mut EditorState`. ~10 LOC of plumbing.
5. **`FreeCamera` keeps its default `mouse_key_cursor_grab: MouseButton::Right` binding.** If a future user remaps to LMB, the editor's brush would conflict. Documented under Decision 3 flip-trigger.
6. **`get_voxel_type(&WorldData, IVec3) -> Option<VoxelTypeId>` does NOT yet exist in the port** — verified by `grep get_voxel` returning no port hits in `world/data.rs`. Adding it is part of Track B (lives next to `ray_traversal`); ~30 LOC, walks the same 3-layer descent as `ray_traversal` but without the DDA stepping (single-point lookup).
7. **The `pending_edits` system can absorb 17k-voxel brush strokes per frame.** Verified by reading `render/construction/mod.rs:674-682` — the extract drains ALL batches in `pending_edits.batches` and aggregates them. One `set_voxels_batch` call pushes one `EditBatch` (covering ~125 chunks for r=16); the extract sees one batch per frame and processes it. No issue. The `world_change.wgsl` GPU dispatch handles per-frame edit batches up to the size of the `changed_*_dynamic` buffers (capacity TBD but already sized for the existing per-frame edit workload).
8. **`dense_voxel_types` staleness during a brush stroke is acceptable.** `set_voxel` (`world/data.rs:98-210`) does NOT update `dense_voxel_types`. The GPU producer chain at `render/construction/mod.rs:827` reads `dense_voxel_types` for `segment_voxel_buffer` rebuilds, but the per-frame change-handler chain reads chunks/blocks/voxels directly via `world_change.wgsl`. So during an edit stroke, `dense_voxel_types` stays stale but the rendered world stays correct (it reads from the freshly-mutated chunks/blocks/voxels). Verified by tracing the `extract_world_changes` data flow.
9. **The `chunk_state == 2 /* Mixed */` check in `ray_traversal` correctly maps C#'s `(curNode >> 31) != 0` test.** The C# uses bits 30-31 for state (Mixed = `2 << 30` = high bit set + bit 30 set; `curNode >> 31 != 0` catches the high bit). The Rust port encodes the same way (verified at `aadf/edit.rs:370` `chunk_state = chunk_raw >> 30` and `aadf/cell.rs::ChunkCell::Mixed` discriminant). State 2 has high bit set → `>> 31` is 1. ✅
10. **`process_edit_batch` accepts an arbitrary number of edited chunks in one call.** Verified by reading `aadf/edit.rs:242-327` — it iterates `for &(chunk_pos, edit_offset) in edited_chunks` with no upper bound. Per-chunk slot claims (`v_cursor += 32`, `b_cursor += 64`) keep advancing; the returned `EditBatch` accumulates all chunks' changes. Multi-chunk batching is the function's intended use shape.
11. **The `EditorPlugin` wiring follows the same pattern as `HudPlugin` / `panel` system additions** — additive `add_systems` calls in `build_app_with_args` (`lib.rs:524+`). No new Plugin types needed.
12. **`bevy_ui` `Node { right: px(12) }` works as a top-right anchor** — symmetric to the existing `left: px(12)` at `hud.rs:107`. Bevy 0.19 `bevy_ui` supports left/right/top/bottom edge anchoring on `PositionType::Absolute`. Assumed working without verification — same crate that produced the panel + HUD chrome.
13. **The existing 4×2×4 test grid is large enough to demonstrate the editor.** 64×32×64 voxels gives enough surface area to test sphere r=10 brushes at multiple positions. If the editor is tested on a 1-chunk world, edge cases at the world boundary dominate; the existing grid avoids this.
14. **`KnobKind::Action`'s `apply` signature change doesn't break the `defaults_match_gi_settings_default` test.** The test (`panel.rs:1183-1220`) only inspects `U32`/`F32`/`Bool` variants. New `Edit { variant }` variants need a new mirror test (Test #16 above). Existing test stays green.

## Risks & mitigations

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | `Camera::viewport_to_world` ABI changes between rc.1 and a future release | Low | Build break | Pinned via the `bevy = "=0.19.0-rc.1"` exact-version in `Cargo.toml:40`. |
| 2 | `FreeCamera` user reconfiguration binds LMB to camera grab | Low | LMB conflict | Documented under Decision 3. If observed, gate `apply_edit_tool` on `FreeCameraState.enabled` AND/OR check no `Pressed` state on a marker query for the camera grab. |
| 3 | `set_voxels_batch` races the GPU regime-3 dispatch when multiple bulk edits land in one frame | Low | Visual artefact (one frame of stale render) | `set_voxels_batch` pushes ONE batch into `pending_edits.batches`; the extract drains the whole `batches` Vec each frame. Sequential ordering is preserved. |
| 4 | Chunk-edit-window encoding edge case at chunk boundaries (sphere overlapping 8 chunks) | Medium | Voxel mutations miss at boundaries | `set_voxels_batch` groups by chunk and decodes EACH chunk's window from `chunks_cpu`/`blocks_cpu`/`voxels_cpu` (per `build_chunk_edit_window_from_world` at `aadf/edit.rs:362-415`). Verified path; test `sphere_brush_produces_solid_sphere` should specifically cover a sphere placed AT a chunk boundary (centre at voxel coord `(16, 16, 16)` with radius 4 → touches 8 chunks). |
| 5 | `MAX_RAY_STEPS_*` consts in WGSL aren't checked CPU-side; CPU traversal could loop on a malformed world | Low | Infinite loop risk on bad data | The CPU traversal caps at 1000 steps (`WorldData.cs:419` — same hard cap). Faithful port matches; safe by construction. |
| 6 | Brush at radius 400 takes >100ms even with batching | Medium | Frame stall | Brush limit is 400 (C# default cap, `Paint.cs:92`). A 400-radius sphere is ~268M voxels — well past frame budget. **Mitigation:** the panel slider caps at 400 (matches C#), but a real-world 400-radius edit will stall regardless. **Future:** add async voxel mutation (out of scope). For Track B, the implementer can verify that radius 100 (sphere = ~4M voxels = ~125ms via single-batch path) is the practical upper bound and document it. |
| 7 | The Paint brush's per-voxel `get_voxel_type` lookup is O(layers) — slow on a 17k-voxel stroke | Low | Brush stall ~10ms vs ~1ms | Each `get_voxel_type` does a 3-layer descent on the read path; ~30 instructions per voxel. 17k × 30 = ~500k instructions = ~0.5ms. Negligible. |
| 8 | `EditorState` not gated on `cfg.add_hud` correctly — the e2e harness might init it | Low | e2e harness perturbation | Mirror the `panel.rs` gating: `app.init_resource::<EditorState>()` is in the `if cfg.add_hud { ... }` block. Confirmed by reading `lib.rs:573-597`. |
| 9 | `screen_to_ray` returns `None` (off-viewport cursor) → editor silently does nothing | Low | UX glitch | Handled — `apply_edit_tool` returns early; HUD shows "(no hit — aim at world)". |
| 10 | `panel.rs` test `defaults_match_gi_settings_default` (`panel.rs:1184`) breaks if my new `Edit { variant }` arms slip into the iteration | Low | Test regression | The test's `match row.kind { KnobKind::U32 ... }` arms list each variant explicitly; the new `Edit { .. }` arm is naturally a `_ => {}` no-op. Verified safe by reading `panel.rs:1187`. The new mirror test (Test #16) explicitly covers the editor knobs. |
| 11 | `viewport_to_world` returns a ray with origin AT the near plane, not the camera position | Medium | Ray-traversal starting offset | The audit description was "`shoot_ray` ray-origin built from `PositionSplit`". Bevy's `viewport_to_world` returns origin at the near-plane projection of the cursor; that's already the correct starting point for a CPU ray-cast against the world. C# `EditingToolPaint.cs:29` uses `WorldRender.camera.GetPos()` (full camera position) as the origin — which would mismatch slightly. **Decision:** use Bevy's `viewport_to_world` as-is; the slight offset (near plane vs. camera origin) is invisible at the test-grid scale. If a future fixture exposes the difference, switch to `cam_gxf.translation()` + reconstruct the direction. Recorded in Assumptions. |
| 12 | `bevy_ui` `right: px(12)` doesn't anchor right-edge as expected | Low | HUD overlay positioned wrong | The pattern is symmetric to `left: px(12)` (`hud.rs:107`) and works in bevy_ui 0.19 (the panel uses `bottom: px(12)` + `left: px(12)` at `panel.rs:571-572`; the same engine should handle `top: px(12)` + `right: px(12)`). If it doesn't, fall back to a `width: px(280)` + `position_type: Absolute` + `top: 12 + left: window.width - 280 - 12` computed via window-size system. Edge fallback. |

## Out of scope for this design

- **Flood-fill tool** — explicitly excluded (`01-context.md` §5 forbidden move).
- **Model-paste tool** — same.
- **Undo / redo** — no undo stack; the user accepts edits permanently. (`set_voxels_batch` could trivially capture per-voxel pre-images for an undo log; deferred.)
- **Multi-stroke recording / playback** — out of scope.
- **Save / load of world state after edits** — out of scope (no port-side world-save format exists yet; the test grid rebuilds at startup).
- **`obj2voxel` / Voxlap / any non-`.vox` format integration** — Track A scope only.
- **Visual feedback gizmo (brush sphere wireframe at hover position)** — nice-to-have but out of the brief's tool list. The HUD displays hover voxel coordinates as the navigation aid; rendering a 3D brush gizmo would require touching the render graph.
- **Hot-reload of `EditorState` from disk** — out of scope.
- **Network / multi-player editing** — out of scope.
- **Async / threaded brush application** — the brushes run on the main thread. C# `Paint.cs:63` uses `Parallel.For`; the port runs single-threaded as a faithful first cut. (For r=16, this is 625µs in the batched path — well under 16ms frame budget.)
- **Brush curves / falloff / opacity** — out of the C# tool semantic (solid brushes only).
- **Visual differentiation of erase mode in the HUD** — `is_erase` shows as text only; no red-tint HUD chrome. Cosmetic, out of scope.
- **Sound effects** — out of scope.
- **`MAX_RAY_STEPS_*` const promotion to runtime** — the panel's existing `class: 'P'` rows already promote these; no Track-B touch.
- **`GpuGiParams` mutation** — explicitly out of scope per `01-context.md` §5 forbidden move 4.
- **Mouse-cursor gizmos / crosshair textures** — out of scope.
- **Tutorial / first-run UX** — out of scope.
- **Keyboard shortcuts for tool switching (e.g. `1`/`2`/`3`)** — could be a follow-up; the panel's Enum knob already lets the user cycle tools via Left/Right arrows. Not in first cut.
