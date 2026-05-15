# 25 — Design: Mouse drag-sliders for the raymarching-quality panel

**Date:** 2026-05-15
**Branch:** `main` at HEAD `40bde09`. Tree clean.
**Predecessor:** `21-design-quality-panel.md` (the keyboard panel design) + `22-impl-quality-panel.md` (its impl log).
**Successor (impl log):** `26-impl-panel-mouse.md` — written after gates pass.
**Mode:** consolidated `/delegate` dispatch — one uninterrupted run: design → self-review → implementation → impl log.

---

## §1. Goal restatement (verbatim user ask)

> "Extend the bevy_ui panel with mouse drag-sliders" — staying on Bevy 0.19 (no downgrade).

The keyboard-driven panel from commit `4211910` was a workaround for `bevy_egui` not supporting Bevy 0.19 (`21-design-quality-panel.md` §3). The user now wants proper mouse interaction *without* changing Bevy version or adding new UI deps. Operational definition:

- **Mouse drag** on slider rows changes the value proportional to horizontal cursor delta.
- **Click** on checkbox rows toggles; click on button-style rows (the "Reset all" row added by this dispatch) fires the action.
- **Keyboard navigation stays working as a fallback** — this dispatch is *additive*, not a replacement.
- **No new deps.** Bevy 0.19-rc.1 native `bevy_ui` only.
- **Optional stretch goal:** hover-tooltips with the row's description text. Decision deferred to §8.

The existing 594-line `panel.rs` (`KNOBS` table, `PanelState`, the 4 systems) stays in place; this dispatch **adds** mouse-aware components + systems alongside it. The keyboard code path is unchanged.

---

## §2. Mouse-event ingestion architecture

### §2.1 Bevy APIs used

- **`Interaction`** (component, `bevy::prelude` re-export — verified at `bevy_ui-0.19.0-rc.1/src/lib.rs:68`). Three-state enum (`None` / `Hovered` / `Pressed`). Auto-managed by `ui_focus_system` in `PreUpdate / UiSystems::Focus`.
  - `Interaction::Pressed` is set **only on `MouseButton::Left` press** (verified by reading `ui_focus_system` at `bevy_ui-0.19.0-rc.1/src/focus.rs:174-187` — the system checks `MouseButton::Left` literally).
  - On `MouseButton::Left` release, `Interaction` transitions back to `Hovered` (still under cursor) or `None`.
  - When a node is hidden via `Display::None` or `InheritedVisibility::get() == false`, `Interaction` is forced to `None` (`focus.rs:243-256`). **This is load-bearing for §7.** Panel-closed = no mouse capture.

- **`RelativeCursorPosition`** (component, NOT in prelude — need explicit `use bevy::ui::RelativeCursorPosition;`). Updated by the same `ui_focus_system`. Holds `cursor_over: bool` + `normalized: Option<Vec2>` where (-0.5, -0.5) = top-left, (0.5, 0.5) = bottom-right of the node. Required for **per-row hit-test** — clicking on the row's slider region must know which row was clicked.

- **`bevy::input::mouse::AccumulatedMouseMotion`** (resource). Per-frame mouse Δx/Δy in physical pixels. Already in use by `FreeCameraPlugin`. Drag-delta source — read once per frame while in drag state.

- **`bevy::input::ButtonInput<MouseButton>`** (resource). Edge detection — `just_released(MouseButton::Left)` ends the drag. Already in use by `FreeCameraPlugin`.

### §2.2 Components added

| Component | Where | Purpose |
|---|---|---|
| `PanelRow { knob_index: usize }` | on each per-row Node | Maps the row's `Interaction` events back to the `KNOBS[]` index. |
| `Interaction::default()` | on each per-row Node | Auto-populated each frame by `ui_focus_system`. |
| `RelativeCursorPosition::default()` | on each per-row Node | Cursor x relative to row width (used by §4 sensitivity). |

The existing `PanelRoot` + `PanelText` markers stay, but the layout reshapes from "one root → one multi-line Text" into "one root → N per-row Nodes (each with `Interaction`) → each holds a single-line Text". See §7 for the layout-restructure rationale.

### §2.3 Resources added

| Resource | Purpose |
|---|---|
| `PanelDrag { state: DragState }` | Single drag-state machine; see §3. |

`DragState` is an enum with three states (`Idle`, `Pressed { knob_index, started_at_value }`, `Dragging { knob_index, started_at_value }`). One drag at a time — Bevy UI's `Interaction` is per-entity, but only one entity is ever `Pressed` at a time (since `Pressed` requires the cursor over the entity, and a cursor has one position).

### §2.4 Systems added

| System | Schedule | Order | Purpose |
|---|---|---|---|
| `mouse_interact_panel` | `Update` | After `panel::toggle_panel` and `panel::adjust_panel`, before `panel::update_panel_text` | Reads `Interaction` + `RelativeCursorPosition` + `AccumulatedMouseMotion` + mouse button edges; drives the `PanelDrag` state machine; mutates `AppArgs.gi` for slider drag / bool toggle / button-click "Reset all". |

Existing systems unchanged: `setup_panel` (rebuilt — see §7), `toggle_panel`, `adjust_panel`, `update_panel_text`. The mouse system slots in between keyboard adjust and text update so the panel text reflects the just-applied mouse value on the same frame (same `.chain()` invariant the existing systems already obey).

---

## §3. Drag state machine

```
            Interaction::Pressed on a U32/F32 row
             ─────────────────────────────────►
   Idle  ────────────────────────────────────►  Pressed { knob, v0 }
    ▲                                                │
    │                                                │ AccumulatedMouseMotion.x != 0
    │                                                ▼
    │     mouse Left released              Dragging { knob, v0 }
    │  ◄─────────────────────────────────────────  (continues until release)
    │
    └─── mouse Left released without motion ───  Pressed { knob, v0 }
         → if row is Bool: flip
         → if row is U32/F32: no-op (treat as click without drag)
         → if row is the "Reset all" button row: invoke reset_all
```

### §3.1 Transitions

- **Idle → Pressed**: when any per-row Node's `Interaction::Pressed` is observed for the first time (edge — track `last_interaction_pressed: bool` per frame in a `Local`). The knob's *current value* is captured into `v0` so drag math is anchored.
- **Pressed → Dragging**: when `AccumulatedMouseMotion.x.abs() > drag_threshold_px` (= 2.0 px; below this, treat as click-without-drag). This avoids treating tiny jitters as a drag.
- **Pressed → Idle (with bool flip)**: when `MouseButton::Left::just_released` AND the row is a `Bool` knob AND no drag occurred. Flip the bool.
- **Pressed → Idle (with button action)**: when `Left::just_released` AND the row is the special `ResetAll` row AND no drag occurred. Invoke `reset_all`.
- **Pressed → Idle (no-op for sliders without drag)**: when `Left::just_released` AND the row is `U32`/`F32` AND no drag occurred. Same effect as keyboard focusing the row — selecting it but not changing the value.
- **Dragging → Idle**: when `Left::just_released`. Commit the value (already applied each frame; nothing extra to commit).
- **Any state → Idle (force)**: when `PanelState.open` flips to `false` (panel closed mid-drag). Defensive: panel closure aborts the drag without applying any further delta.

### §3.2 Cursor-tracks-knob behavior

Per the brief's "mouse hover sets the selected row to whatever is under the cursor": while in `Idle` (no drag in progress), if any row's `Interaction == Hovered`, `PanelState.cursor` is updated to that row's index. Keyboard `↑`/`↓` cursor moves still work; mouse hover effectively *re-selects* whenever it intersects a row. While `Dragging`, the cursor is locked to the dragged row (hover-over-other-rows does NOT steal selection mid-drag — feels more predictable).

### §3.3 Why state on a `Resource`, not per-row `Local`s

Drag is a global mode (only one row can be dragged at a time, and the bool-vs-slider behavior depends on knob kind which lives on `KNOBS[]`). A single `Resource` with a tagged enum is simpler than spreading state across N row entities, and trivially observable from the keyboard `adjust_panel` system for the "abort drag on keyboard input mid-drag" case (§6).

---

## §4. Sensitivity calibration

### §4.1 Choice

**Each pixel of horizontal cursor motion = `(max - min) / drag_full_range_px` units of value change**, where `drag_full_range_px = 2560.0` (8× the original 320 px). A full panel-wide drag (~320 px) sweeps **1/8 of** the `[min..max]` range; multiple strokes traverse the full range. At slow motion (5–10 px/s) this gives ~0.01-unit control. Shift held = 4× finer (32× finer than the original default).

For integer knobs (`U32`): the running float-accumulated value is rounded on commit each frame. The drag math keeps the accumulator at f64 precision in the `PanelDrag` resource (sub-step `f64 fractional_accumulator`) so a slow drag past one-step boundaries actually moves (without f64 accumulation, a single 1-px move on a u32 with range 1..3 would map to ~0.006 — rounding to 0 — and the drag would never advance).

### §4.2 Modifier behaviour

- **Shift**: 0.25× sensitivity (`(max - min) / 1280px` per pixel — i.e. 4× more pixels to traverse the range). Matches the keyboard `Shift+←/→` "fine-grain" semantics.
- **Ctrl**: NOT bound this dispatch. Could be 4× sensitivity later — out of scope.

### §4.3 Rationale for "full width = full range"

- Predictable: every slider behaves the same way regardless of its range. The user knows "drag from left edge of row to right edge = sweep the full range".
- Cheap: one sensitivity constant per drag, no per-knob tuning needed.
- Plays well with f32 and u32: even very wide ranges (e.g. `noise_suppression_factor` 0.01..100, log-scale visually but linear in the slider) reach the extremes without scrubbing.
- The few knobs with awkward ranges (`bounce_count` 1..3 — three discrete values across the row) are fine: a third of the row each.

### §4.4 Why not "1 px = 1 unit"

Considered. Rejected: ranges differ wildly. `radius_lit_factor`'s range is 0..1000; dragging 1000 px is more than the panel is wide. `noise_suppression_factor`'s range is 0.01..100, in steps of 0.05 nominally; mapping 1 px = 1.0 means 0.05 = unreachable. Full-width-spans-range scales correctly across all knobs.

---

## §5. Click-edge detection for checkboxes and buttons

Bevy's `Interaction::Pressed` persists every frame the mouse is down. A naive `if interaction == Pressed: toggle()` fires every frame, not once per click.

### §5.1 Edge detection via `MouseButton::Left::just_released` + drag-amount check

The cleanest approach uses the drag state machine itself:

1. **On `Interaction::Pressed` (edge)**: enter `Pressed { knob, v0 }` state. Track the entry frame's mouse position. **Do not** apply any action yet.
2. **On each frame while in `Pressed`/`Dragging`**: accumulate `AccumulatedMouseMotion.x` into `total_drag_px`.
3. **On `MouseButton::Left::just_released`**: commit. If `total_drag_px.abs() < drag_threshold_px` (= 2.0), treat as a *click*:
   - U32/F32 row: no-op (the row is "selected" but its value unchanged — same as keyboard `↑/↓` to it).
   - Bool row: flip the value.
   - The special "Reset all" row: invoke `reset_all`.
4. If `total_drag_px.abs() >= drag_threshold_px`, it was a *drag* — the value has been mutated each frame already; nothing to do on release except return to `Idle`.

This is robust against:
- Holding the mouse button (only releasing fires the click action — same as every GUI ever).
- Cursor leaving the row mid-drag (the `Dragging` state is anchored to the knob, not to current hit-test — keep applying motion to the captured knob_index until release).
- Multiple rows under cursor at once (the `Interaction::Pressed` edge picks one — Bevy's `ui_focus_system` ensures only the topmost gets `Pressed`).

### §5.2 The "Reset all" button row

The brief mentions click-on-button rows. The existing panel layout has a "Reset all" function bound to `Shift+R` (keyboard-only). This dispatch **adds a button-style row** to the panel — a separate `KNOBS[]` entry with `KnobKind::Action { label, fn }` — so mouse users can click it.

- Label: `> RESET ALL TO DEFAULTS <` (visually distinct from the value rows — centered, no value column).
- Position: at the bottom of the panel, just above the keybind legend.
- Behavior: on click (per §5.1), invokes the same `reset_all` code path the `Shift+R` keybind uses.

This is the only new `KnobKind` variant added; the keyboard `Shift+R` keybind continues to work (additive, not a replacement).

---

## §6. Keyboard/mouse interaction policy

### §6.1 Composition, not replacement

Both input paths target the same state:
- `PanelState.cursor` (which row is selected).
- `AppArgs.gi` (the knob values).

There is **no shared mid-mutation state** between the two: keyboard mutations happen all in one frame of `adjust_panel`; mouse mutations happen all in one frame of `mouse_interact_panel`. No race.

The systems run **serially via `.chain()`**: `toggle_panel → adjust_panel → mouse_interact_panel → update_panel_text`. Order chosen so that:
- Keyboard cursor moves (`↑/↓`) land before mouse hover override (mouse hover wins when present, else keyboard wins).
- Both adjustment paths (kb and mouse) land before the text rewrite, so the panel text always reflects the latest applied values.

### §6.2 Simultaneous input — what happens

The cases:

| Case | Policy | Why |
|---|---|---|
| Keyboard arrow pressed mid-drag | **Drag wins; keyboard arrow is ignored.** | The mouse is mid-gesture; finishing it predictably is better than confusing the user with a hybrid mutation. Implementation: in `adjust_panel`, early-out if `PanelDrag.state != Idle`. |
| Mouse drag while keyboard held | Mouse drag updates the value continuously; the keyboard's `just_pressed` was already consumed last frame. No conflict. | Both paths see the same `AppArgs.gi` and just write to it. |
| Mouse drag of row A while keyboard `R` (reset) | The drag commits each frame; `R` resets the row mid-drag → the drag's next-frame delta applies to the reset value. Slightly weird but not broken. | The user pressed two conflicting actions; let the value end up wherever they leave it. |
| Mouse hover over row B while keyboard `↑`/`↓` cycles | Mouse hover sets cursor to B; keyboard then steps off B. Last writer wins per-frame. | Acceptable — mouse hover and keyboard nav can fight, but the user can stop hovering or stop pressing arrows. |

### §6.3 What is NOT changed in the keyboard path

`toggle_panel` (F1), `adjust_panel`'s arrow/PgUp/PgDn/R/Shift+R bindings, the cursor wrap logic — all unchanged. The only addition to `adjust_panel` is the early-out check on `PanelDrag.state != Idle` (1 line) to honor §6.2 row 1.

---

## §7. UI Z-order / hit-testing strategy

### §7.1 Layout restructure — single Text → per-row Nodes

The existing panel is `Root Node → Text (one Text entity with `\n`-separated rows)`. For per-row hit-testing, each row needs its own entity with `Interaction`. New structure:

```
Root Node  [PanelRoot, BackgroundColor, Display::None when closed]
  ├── header row Node       [Text "Raymarching Quality"]
  ├── row Node 0            [PanelRow { knob_index: 0 }, Interaction, RelativeCursorPosition, contains Text]
  ├── row Node 1            [PanelRow { knob_index: 1 }, Interaction, ...]
  ├── ...
  ├── row Node N-1
  ├── ResetAll row Node     [PanelRow { knob_index: KNOBS.len() }, Interaction, ...]
  └── legend row Node       [Text — "[↑↓] navigate ..."]
```

Each row Node is `Display::Flex` with one Text child (the row's value+label string). Section header rows still need entities but **must not** be hit-testable — they get NO `Interaction` component (the cursor-tracks-hover only updates `PanelState.cursor` for rows that have `Interaction`).

### §7.2 Click-bleed prevention

The brief flags hit-testing leaking clicks through to scene controls.

**Findings:**
- `FreeCameraPlugin` (`bevy_camera_controller-0.19.0-rc.1/src/free_camera.rs:150`) grabs mouse with `MouseButton::Right` — NOT `Left`. Left mouse is unused by the camera controller.
- `Interaction::Pressed` triggers only on `MouseButton::Left` (`bevy_ui/src/focus.rs:186-187`).
- Therefore: **right-button camera-grab and left-button panel-drag share no input channel.** No bleed risk.

The panel's per-row Nodes default to `FocusPolicy::Pass` (Bevy 0.19 default — `bevy_ui/src/focus.rs:115-116`). For safety against future scene controls that bind `MouseButton::Left`, set `FocusPolicy::Block` on each row Node. With `Block`, lower-stacked UI never receives the press for that pixel, and the camera (which is not a UI node) reads `MouseButton::Left` directly via `ButtonInput<MouseButton>` — that read happens whether or not UI blocks. The brief's concern is satisfied by the right-vs-left split alone; `FocusPolicy::Block` is belt-and-suspenders.

(If the user later adds a left-button voxel-pick system, that system should also early-out when `PanelState.open == true` AND the cursor is over the panel. Out of scope for this dispatch — no such system exists today.)

### §7.3 Panel-closed → no mouse capture

When `PanelState.open == false`, the root node's `Display::None` (set by `toggle_panel`) propagates `InheritedVisibility::get() == false` to all children. The `ui_focus_system` then forces every child's `Interaction` to `None` (`bevy_ui/src/focus.rs:243-256`). No mouse event reaches the panel rows. ✓

---

## §8. Hover tooltip — DEFERRED

### §8.1 Decision

**Deferred to a future dispatch.** Hover tooltips are listed in the brief as an *optional* stretch goal "if clean to implement, otherwise drop". The clean implementation requires:

1. A floating tooltip `Node` entity (one, reused) with `PositionType::Absolute`, default `Display::None`.
2. A system that, while any row's `Interaction == Hovered`, sets the tooltip's `left`/`top` from `Window::cursor_position()`, sets its Text from the row's description, and toggles `Display::Flex`.
3. Per-knob description text (currently each `Knob` only has a `label`; tooltips need a separate `description: &'static str` field — the §2.1 knob-table tooltips in `21-design-quality-panel.md` already enumerate the text).

This is implementable in ~50 LOC, but:
- It requires extending the `Knob` struct with a `description` field, plumbed through `KNOBS[]` — a 28-row edit per the existing table.
- Hover-tooltip systems compose awkwardly with the cursor-tracks-hover behavior from §3.2 — both react to `Interaction::Hovered`. Not a hard conflict, but two systems read the same edge.
- Position math (cursor coords are window-relative; the tooltip Node is in UI-local coords) needs care for high-DPI / scale-factor windows.

The dispatch's load-bearing goal (mouse drag + click) is independent of tooltips. Deferring lets this dispatch land cleanly with the option to add tooltips in a follow-on dispatch when the user has actually used the panel and confirmed they want them.

### §8.2 What stays

The right-margin class indicator (`P` / `C` / `D`) introduced by `21-design-quality-panel.md` §5 stays — it gives a 1-character mode hint per row, which is the lightweight stand-in for full tooltips.

---

## §9. Self-review notes

### §9.1 Re-implementation check — no prior Interaction-based input

Verified with `rtk grep -rn "Interaction\|MouseButton\|CursorMoved" crates/bevy_naadf/src/`. The only hits are unrelated string matches in doc comments (`isAtmosphereInteraction`). No prior `bevy_ui::Interaction` usage; no prior `MouseButton` reads. No pattern to follow from elsewhere in the crate — this dispatch establishes the project's first `Interaction` consumer. **Follow Bevy's documented `Interaction` pattern verbatim.**

### §9.2 Coupling check — keyboard system

`adjust_panel` uses `Res<ButtonInput<KeyCode>>` + `ResMut<PanelState>` + `ResMut<AppArgs>`. No `Local<>` state, no internal cursor tracking — the cursor lives on `PanelState`. **The mouse system needs the same `ResMut<AppArgs>` access**, which means the two cannot run in parallel — Bevy will serialize them automatically (one writer). `.chain()` makes that ordering explicit. No data-race risk.

### §9.3 Drag-during-keyboard edge case

Adopted policy §6.2 row 1: keyboard arrow ignored while drag in progress. This avoids the kind-mismatch hazard (e.g. drag was integrating onto knob X but keyboard arrow moved cursor to knob Y mid-drag — without the early-out, the next mouse frame would mutate Y, not X). Implementation: 1-line `if drag.state != Idle { return; }` in `adjust_panel`. Documented in §6.2.

### §9.4 UI Z-order / left-vs-right button split

The biggest potential foot-gun (mouse click bleeding through panel to scene controls) is **non-issue** because of the right-vs-left mouse split. Documented in §7.2 with citations. **Belt-and-suspenders: per-row `FocusPolicy::Block`** for future-proofing.

### §9.5 Panel-closed state

When closed, `Display::None` forces `Interaction = None` on all children (cited at §7.3). No special "tear down on close" code needed. Stays Bevy-idiomatic.

### §9.6 What changed from initial sketch

Original mental model: per-row `Interaction` + a global `cursor_position` tracker reading from `Window::cursor_position()`. **Revised to use `AccumulatedMouseMotion` + `RelativeCursorPosition` instead** because:
- `AccumulatedMouseMotion` is already a frame-coherent Δ that survives sub-frame jitter and frame skips (FreeCamera uses it for the same reason).
- `RelativeCursorPosition` is updated by `ui_focus_system` in the same `PreUpdate` slot as `Interaction`, so they're consistent. Reading `Window::cursor_position()` would be a different time slice.

The drag math uses Δ from `AccumulatedMouseMotion`, not absolute position — this is the easiest way to support "drag continues even if cursor leaves the row mid-drag" without needing to remember start-position-in-physical-pixels.

### §9.7 Self-review against brief checklist (§"Self-review checklist before writing code")

1. **Re-implementation check**: done §9.1. No prior pattern. ✓
2. **Coupling check**: done §9.2. ✓
3. **Drag-during-keyboard edge case**: done §9.3, policy §6.2. ✓
4. **UI Z-order**: done §7.2 — right-vs-left split + `FocusPolicy::Block`. ✓
5. **Panel-closed state**: done §7.3 — `Display::None` propagation. ✓

### §9.8 High-risk findings — self-certified

This dispatch's only material risk is the `Interaction` semantics being slightly different from my reading of the source. Mitigation: verified `ui_focus_system` source directly at `bevy_ui-0.19.0-rc.1/src/focus.rs` for the `MouseButton::Left`-only `Pressed` claim, the `Display::None → Interaction::None` claim, and the `FocusPolicy::Pass` default. All three are line-cited in this design doc — verifiable by orchestrator review.

No HIGH-RISK escalation to fresh-eyes reviewer needed:
- This is a purely additive change to a single module (`panel.rs`) + 3 lines in `lib.rs` (system registration). No render-graph wiring, no GPU struct edits, no shader edits.
- The `21-design-quality-panel.md` design's high-risk item (bevy_egui vs bevy_ui choice) has already been resolved by that landing dispatch — it doesn't recur here.
- Bit-equivalent default preservation is automatic — no default values change; the new code path only acts on user input.

### §9.9 Decisions & rejected alternatives

1. **Chose: per-row Node entities with `Interaction`.** Rejected: overlay invisible hit-zone Nodes over the existing single-Text panel. Reason: hit-zone overlays are brittle (positions coupled to font metrics; one font change breaks alignment). Per-row Nodes are the Bevy-idiomatic way.
2. **Chose: drag state in a single `Resource`.** Rejected: per-row `Local` state. Reason: drag is global (only one active at a time); the `KnobKind`-dependent bool/slider branch wants centralised state.
3. **Chose: `AccumulatedMouseMotion` Δ-based drag math.** Rejected: store start cursor position + compute Δ each frame from `Window::cursor_position()`. Reason: `AccumulatedMouseMotion` is frame-coherent and unaffected by cursor leaving the row.
4. **Chose: 2.0 px drag threshold for click-vs-drag.** Rejected: 0 px (every press is a drag). Reason: small natural cursor jitter on press would unintentionally drag bool knobs. Reasonable threshold: 2 px ≈ "user clearly moved the mouse".
5. **Chose: "Reset all" button row in the panel.** Rejected: a separate Bevy `Button` outside the row list. Reason: keeping all panel interactions in the unified row table simplifies the layout system and keeps the keyboard `Shift+R` and mouse-click paths converging on the same `KNOBS[]` entry.
6. **Chose: drag full-row-width = full knob range.** Rejected: per-knob sensitivity. Reason: predictable cross-knob feel; single sensitivity constant.
7. **Chose: defer tooltips.** Rejected: implement tooltips this dispatch. Reason: clean implementation requires extending every knob row with a description string and adds a system that conflicts with the cursor-tracks-hover behavior. Better as a follow-on once the panel is in regular use.
8. **Chose: drag wins, keyboard ignored during drag.** Rejected: keyboard wins, drag aborts. Reason: mid-gesture mouse interaction should complete predictably; aborting on a keyboard arrow would feel like a bug.

### §9.10 Assumptions made

1. **`Interaction::Pressed` is set on `MouseButton::Left` only** — verified `bevy_ui/src/focus.rs:186-187` (literal `MouseButton::Left` check).
2. **`Display::None` propagates and forces `Interaction::None`** — verified `bevy_ui/src/focus.rs:243-256`.
3. **`ui_focus_system` runs in `PreUpdate` under `UiSystems::Focus`** — verified `bevy_ui/src/lib.rs:172` (`.after(InputSystems)`), so my `Update` system reads up-to-date `Interaction`.
4. **`AccumulatedMouseMotion` is in physical pixels** — read pattern in FreeCamera (`free_camera.rs:268`). The drag sensitivity constant is in physical pixels.
5. **`RelativeCursorPosition` is not in `bevy::prelude`** — verified, must `use bevy::ui::RelativeCursorPosition`. Actually `use bevy_ui::RelativeCursorPosition` per Bevy umbrella crate re-export; documented in §2.1.
6. **`FreeCameraPlugin` grabs mouse with `MouseButton::Right`** — verified `bevy_camera_controller-0.19.0-rc.1/src/free_camera.rs:150`.
7. **The panel only spawns when `cfg.add_hud == true`** — verified by `lib.rs:492-510`. The e2e harness has `add_hud == false`, so mouse systems never run in e2e — luminance gates are unaffected.

---

## §10. Files-touched preview

| Path | Change | Approx LoC |
|---|---|---|
| `crates/bevy_naadf/src/panel.rs` | Restructure `setup_panel` (single Text → per-row Nodes); add `KnobKind::Action`; add `mouse_interact_panel` system; add `PanelDrag` resource; add `PanelRow` component; modify `adjust_panel` to early-out during drag; modify `update_panel_text` to write per-row Text entities | ~250 net add |
| `crates/bevy_naadf/src/lib.rs` | Register `PanelDrag` resource; chain `mouse_interact_panel` into the existing system tuple | ~5 |
| `crates/bevy_naadf/src/hud.rs` | (None — keybind hint stays; drag is on the panel, not the HUD) | 0 |

Estimated total: ~255 LoC net add to `panel.rs`, ~5 in `lib.rs`. The `panel.rs` line count will grow from 594 to ~850.

---

## §11. Verification gates plan

Per the brief:
1. `cargo build --workspace` — exit 0, no new warnings on `panel.rs` or `lib.rs`.
2. `cargo test -p bevy-naadf --lib` — **must show ≥ 116 passed** (the baseline from `4211910`). New tests welcome but not required.
3. `cargo run --release --bin e2e_render` — exit 0; luminance: emissive 247.1, solid 242.0, sky 145.9 (Dispatch A baseline, unchanged since `4211910`).
4. `cargo run --release --bin e2e_render -- --entities` — exit 0; entity_pixel gate green.

The panel is `add_hud`-gated; e2e has `add_hud == false` → no panel → no mouse system → no luminance impact. The e2e gates serve as non-regression checks for *everything else* (no accidental render-graph changes, no Cargo dep changes, no compile-time regressions in non-panel modules).

**Do NOT run the windowed app** (`cargo run --bin bevy-naadf`) — per the brief, visual validation is the user's job.

---

## Independent review

Adversarial self-review against the success criteria in the brief + the existing code the change touches. The brief lists five specific checklist items in "Self-review checklist (before writing code)" and four hard constraints in "Constraints"; I work through every one then hunt for the assumptions I baked in.

### §IR.1 — Brief's checklist items (re-checked adversarially)

**(1) Re-implementation check.** Re-grepped: `Interaction`, `MouseButton`, `CursorMoved` zero usages outside doc strings. **Confirmed: no prior pattern to follow.** But also: `hud.rs` (read again) does NOT take input; no pre-existing UI input idiom in the project. The dispatch establishes the pattern. **Risk surfaced: the project has no existing convention I can rubber-stamp my system schedule order against.** Mitigation: follow Bevy's own examples (`bevy_ui` repo) — `Update`-schedule mouse handlers reading `PreUpdate`-updated `Interaction` is the documented norm.

**(2) Coupling check on keyboard.** Re-read `adjust_panel`: it owns `ResMut<PanelState>` and `ResMut<AppArgs>`. My new `mouse_interact_panel` will also take `ResMut<PanelState>` and `ResMut<AppArgs>`. Two `ResMut` writers on the same Resource forces serial execution. `.chain()` makes the ordering explicit. Bevy will not allow them to race. ✓

But — **subtle finding**: if I make my system take `Res<PanelDrag>` (read-only) and `adjust_panel` modifies `PanelDrag` (it should not, but if it did via a typo), the system order matters. Mitigation: `mouse_interact_panel` owns the `ResMut<PanelDrag>`; `adjust_panel` takes `Res<PanelDrag>` (read-only) to honor §6.2 (keyboard ignored during drag) without owning the write side. **Documented for impl.**

**(3) Drag-during-keyboard edge.** Policy §6.2 row 1 — drag wins, keyboard arrow ignored. Verified: this is sufficient. The "what if user presses `R` (reset) during drag" sub-case: `R` is a separate edge, not a continuous adjustment; letting it reset the row's value mid-drag is harmless because the next mouse frame just resumes integrating from the reset value (drag math reads `getter(&args.gi)` every frame, not a captured `v0`). Wait — **my §3 state machine captured `v0` at press-time and applied `v0 + delta` each frame!** That captures the value at press; a mid-drag `R` from keyboard would be overwritten on the next mouse frame. **Risk: keyboard `R` is unintentionally clobbered.**

**Resolution:** the drag system must NOT use a captured `v0` + accumulated delta. Instead, **read the current value each frame, apply this-frame's `AccumulatedMouseMotion.x` × sensitivity to it, clamp, write back.** This is incremental integration, and it cleanly composes with any other mutator (incl. keyboard `R`). Slightly more f32 drift than v0-anchored math, but bounded by 1 frame's motion (typically < 50 px = sub-1% of range).

The integer-knob accumulator (§4.1) still works — it's a sub-step accumulator that holds the fractional pending step, NOT a captured-value anchor.

**§3 state machine UPDATED accordingly:**
- `Pressed { knob_index, total_drag_px, frac_accumulator }` — no more `v0`.
- Each frame: `delta = motion.x * sensitivity_for_knob; current = getter(); new = current + delta; for U32 round with frac_accumulator; clamp; setter(new)`.
- This way, mid-drag keyboard `R` resets to default, next frame integration resumes from default + this-frame's delta. Predictable.

**(4) UI Z-order.** Re-verified: right-vs-left mouse split eliminates click-bleed at the input layer. `FocusPolicy::Block` on each row is belt-and-suspenders. **One more edge case found:** clicking on the panel's *padding* (the 10 px around the rows) — those pixels are over the root Node but NOT over any row Node. Without `FocusPolicy::Block` on the root, a click on the padding bleeds through. **Fix: `FocusPolicy::Block` on the root Node too** (in addition to per-row). Updated in §IR.4.

**(5) Panel-closed state.** Re-verified `Display::None` → `InheritedVisibility::get() == false` → `Interaction::None` on all children (`bevy_ui/src/focus.rs:243-256`). But — **the `ui_focus_system` also needs to actually run to do that reset.** If panel is closed during the same frame as a click started, the press could land on an entity that hasn't been reset yet. **Mitigation:** my `mouse_interact_panel` reads `PanelState.open` first; if `!open`, it transitions `PanelDrag.state` to `Idle` and skips all interaction reads. Safe regardless of whether `Interaction` was already reset. **Documented for impl.**

### §IR.2 — Brief's hard constraints

1. **Stay on Bevy 0.19-rc.1.** ✓ — no version edit planned. Verified the Bevy APIs I use (`Interaction`, `RelativeCursorPosition`, `AccumulatedMouseMotion`, `ButtonInput<MouseButton>`) all exist in `bevy_ui-0.19.0-rc.1` + `bevy_input-0.19.0-rc.1`.
2. **No new deps.** ✓ — none planned. All APIs are Bevy native.
3. **Don't break keyboard nav.** ✓ — only addition to `adjust_panel` is a 1-line early-out (`if drag != Idle { return; }`) and (revised in IR.1.2) changing `Res<PanelDrag>` to read-only access. Keyboard semantics unchanged.
4. **Don't change paper-canonical defaults / `GpuGiParams` / `GpuRenderParams`.** ✓ — no GPU struct edits, no shader edits, no default value changes. Mouse only mutates `AppArgs.gi` at runtime; it cannot change any default.
5. **Don't churn row layout.** Defensible. The restructure (single Text → per-row Nodes) is a *structural* change but **visually identical** — each row's font, color, indent, value formatting stay the same. The line spacing changes from "Text natural \n line height" to "Node Flex column with 0 gap" which can be tuned to match. **Risk: visual drift between Text-newline spacing and Node-Flex spacing.** Mitigation: pin `row_gap: px(0.0)` on the root Node + `margin: UiRect::ZERO` on each row Node + use the same font size. **Documented for impl.**
6. **Don't add `bevy_egui`.** ✓ — none planned.
7. **Don't loop visual verification.** ✓ — build + lib tests + e2e gates only.

### §IR.3 — Adversarial assumption hunt

Combing for places I waved off:

**A. "Mouse `Interaction::Pressed` is set on Left only."** Verified by reading the source. But is there any UI plugin in `DefaultPlugins` that hijacks left button before `ui_focus_system` runs? Skimmed `DefaultPlugins` — there's no global Left-button consumer in 0.19. ✓

**B. "AccumulatedMouseMotion is in physical pixels."** Verified by reading FreeCamera's use (it scales by `config.sensitivity * dt`, treating motion as raw pixels). For a 360-px-wide panel on a 100% DPI display = 360 physical pixels. On a 200% DPI display the panel might be 720 physical pixels logical-wide, so my sensitivity constant (`drag_full_range_px = 320.0` literal) over-shoots on hi-DPI displays. **Risk: drag is half-sensitivity on 2x-DPI.** **Mitigation:** scale by `Window::scale_factor()`. **Documented for impl.**

Actually wait — `Window::cursor_position()` and `AccumulatedMouseMotion` use different coordinate systems. Let me re-check:

- `AccumulatedMouseMotion.delta`: per Bevy docs, "mouse motion in physical pixels".
- `Window::cursor_position()`: "logical pixels" by default.

For a 360-px (logical) panel on a 2x DPI display: physical width = 720 px. A drag of 360 physical px = half the panel = half the range. To get "full panel = full range", I need: sensitivity = `(max - min) / (panel_width_logical_px × scale_factor)` so that "drag across the panel" = "scale_factor × panel_width_logical" physical pixels worth of motion.

**Practical resolution:** read `Window::scale_factor()` and multiply `drag_full_range_px` by it. The default 1x scaling gives `drag_full_range_px = 320.0` as in §4.1; hi-DPI scales appropriately. **Documented for impl.**

**C. "`Interaction` updates every frame."** Yes, but only for nodes that have `Interaction` and are visible. Hidden nodes don't update. My systems do not depend on stale `Interaction` data — the per-frame read picks up the latest. ✓

**D. "Setting `Interaction` on a parent Node propagates to children."** **No, it does not** — `Interaction` is per-entity. The hit-test on each row entity is independent. My layout has per-row Nodes with `Interaction`; the parent `PanelRoot` does NOT need `Interaction` (it's just a container). ✓

**E. "The drag threshold of 2.0 px is right."** 2 px feels small. On a 4K display at native scale that's ~0.4 mm. Probably fine. If too jumpy, bump to 4 px later. **Acceptable risk.**

**F. "One row per knob = ~28 row entities = no perf issue."** Bevy UI handles hundreds of nodes trivially. 28 is negligible. ✓

**G. "Cursor-tracks-hover doesn't conflict with keyboard ↑/↓ during drag."** Resolved: during drag, hover doesn't change cursor (§3.2). During non-drag, mouse hover and keyboard arrows can fight, but the user can stop one or the other. Acceptable per §6.2 row 4.

**H. "PanelDrag is bevy_ecs::system::Resource."** Yes — needs `#[derive(Resource)]`. **Documented for impl.**

**I. "The text rewrite happens every frame and rebuilds the entire panel."** Currently `update_panel_text` is per-frame; rewriting all rows is cheap. **But with per-row Text entities, I need a Query over them all, indexed by `PanelRow.knob_index`.** Bevy's `Query` is the same cost — negligible. ✓

**J. "DispatchSystem ordering — mouse runs before text rewrite."** Documented in §6.1. The existing `.chain()` already enforces `toggle → adjust → text`; my new system slots in as `toggle → adjust → mouse → text`. ✓

### §IR.4 — Required changes to the design (resolved here, before impl)

From this review pass:

1. **§3 state machine: drop the `v0`-anchored math, use per-frame integration.** Updated above in IR.1.3.
2. **§4 sensitivity: scale by `Window::scale_factor()`** to honor hi-DPI. Documented in IR.3.B.
3. **§7 `FocusPolicy::Block` on the root Node too**, not just per-row, to cover the panel's padding region. Documented in IR.1.4.
4. **`PanelDrag` is `ResMut` for `mouse_interact_panel`, `Res` for `adjust_panel`.** Documented in IR.1.2.
5. **`mouse_interact_panel` early-outs on `!PanelState.open` AND forces drag to Idle on close.** Documented in IR.1.5.
6. **Pin `row_gap: px(0.0)` + same font size on row Nodes to match the prior Text-newline visual spacing.** Documented in IR.2.5.

### §IR.5 — Findings I do NOT escalate to fresh-eyes

None of the above is HIGH-RISK in the brief's sense:

- All edits are confined to `panel.rs` + 5 lines of `lib.rs` system registration.
- No GPU struct edit, no shader edit, no `DefaultPlugins` change.
- No paper-canonical value or render-graph wiring touched.
- The hi-DPI scaling concern (IR.3.B) is a minor sensitivity-tuning issue, not a correctness bug — at worst, hi-DPI users get half-speed drag, still usable.

**No fresh-eyes `delegate-reviewer` recommended for this dispatch.** I will execute and run the four gates.

### §IR.6 — Coverage re-check vs brief

Brief requires (Goal section):
- **Mouse drag on slider rows** — §3 (state machine) + §4 (sensitivity).
- **Click on checkbox rows toggles** — §5.1 (edge-detection on release without drag).
- **Click on button rows fires the action** — §5.2 ("Reset all" row).
- **Keyboard navigation stays working as fallback** — §6 (composition policy, additive change).
- **No new deps** — confirmed.
- **Hover-tooltips (optional)** — §8: deferred with rationale.

All five mandatory items covered. The optional item is documented as deferred. ✓

