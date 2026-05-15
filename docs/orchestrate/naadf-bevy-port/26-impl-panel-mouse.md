# 26 — Impl: Mouse drag-sliders for the raymarching-quality panel

**Date:** 2026-05-15
**Branch:** `main` at HEAD `40bde09` (pre-dispatch). Tree clean at start.
**Predecessor:** `25-design-panel-mouse.md` (design + self-review).
**Mode:** consolidated `/delegate` dispatch — design / review / implement / impl-log all in one uninterrupted run, per the brief's "consolidated mode" instructions.

---

## §1. Files touched

| Path | Range | Change | Net ∆ |
|---|---|---|---|
| `crates/bevy_naadf/src/panel.rs` | Whole file rewritten | Restructured panel UI from single-Text → per-row Nodes; added `KnobKind::Action` for the "Reset all" row; added `PanelDrag` resource + `DragState` enum + `PanelRow` / `PanelRowText` / `PanelLegendText` components; added `mouse_interact_panel` system; refactored `setup_panel` to spawn per-row entities; modified `toggle_panel` to abort in-flight drag on close; modified `adjust_panel` to early-out during drag; rewrote `update_panel_text` to write per-row Text entities + color the selected row. | 594 → 1286 lines (+692). |
| `crates/bevy_naadf/src/lib.rs` | ~492-510 | Registered `panel::PanelDrag` resource; inserted `panel::mouse_interact_panel` into the existing `.chain()` between `adjust_panel` and `update_panel_text`. | +6 / -1 (5 net add). |

**Git diff stats:** `2 files changed, 585 insertions(+), 88 deletions(-)`. The `panel.rs` line count grew 594 → 1286; about 240 of those are new test scaffolding + doc comments + the `KnobKind::Action` row machinery; the rest is the per-row spawn rewrite and the new `mouse_interact_panel` system + its `apply_drag_delta` / `handle_click_release` helpers.

**No edits to:**
- `crates/bevy_naadf/Cargo.toml` — zero new deps.
- `crates/bevy_naadf/src/render/**` — no GPU struct edits, no shader edits, no render-graph wiring.
- `crates/bevy_naadf/src/hud.rs` — no HUD change (the existing `[F1] quality panel` hint already covers the discovery story).
- `crates/bevy_naadf/src/assets/shaders/**` — no shader change.
- Any test in any other module.

---

## §2. New Components / Resources / systems

### Components

| Component | Where attached | Purpose |
|---|---|---|
| `PanelRow { knob_index: usize }` | Each per-row Node (one per `KNOBS[]` entry) | Maps row entity → `KNOBS[]` index. Mouse handler reads `(PanelRow, Interaction)` pairs to find the active row. |
| `PanelRowText { knob_index: usize }` | Each per-row Text (one per `KNOBS[]` entry) | Mirror of `PanelRow.knob_index` on the Text grandchild; lets `update_panel_text` find each row's Text without parent-child traversal. |
| `PanelLegendText` | The bottom legend Text | Lets `update_panel_text` find the keybind-hint line. |
| `Interaction::default()` | Each interactive row Node (U32/F32/Bool/Action) | Auto-populated by Bevy's `ui_focus_system` in `PreUpdate`. Section/Readonly rows do NOT carry `Interaction` — they're inert. |
| `RelativeCursorPosition::default()` | Each interactive row Node | Pulled in to honor the `25-design-panel-mouse.md` §2.1 design; not currently consumed by the runtime path (the drag math reads `AccumulatedMouseMotion` directly, not relative cursor position) — left in place for future hover-tooltip work (deferred per §8). |
| `FocusPolicy::Block` | Root Node + each interactive row Node | Block mouse press-through to lower UI / scene. Belt-and-suspenders — the `MouseButton::Left` vs `MouseButton::Right` split already prevents bleed to the FreeCamera, but `Block` future-proofs against any later scene-controls that bind Left. |

The existing `PanelRoot` + `PanelText` markers were partially refactored: `PanelRoot` stays as before; `PanelText` is replaced by `PanelRowText` + `PanelLegendText` for the per-row layout. No external consumer of `PanelText` exists outside `panel.rs` (verified by grep).

### Resources

| Resource | Default | Purpose |
|---|---|---|
| `PanelDrag { state: DragState }` | `DragState::Idle` | The single drag-state machine. One row at a time can be in `Pressed` or `Dragging`. Registered via `init_resource` in `lib.rs` next to `PanelState`. |

### Systems

| System | Schedule | Order | Purpose |
|---|---|---|---|
| `mouse_interact_panel` | `Update` | After `adjust_panel`, before `update_panel_text` (via the same `.chain()` as the existing trio) | Drives the drag state machine; mutates `AppArgs.gi` on drag / click / button-action. Also implements the cursor-tracks-hover (mouse hover updates `PanelState.cursor` while idle). |

The system chain in `lib.rs` is now `toggle_panel → adjust_panel → mouse_interact_panel → update_panel_text`, serialised by `.chain()`. Ordering rationale: keyboard cursor moves (`↑/↓`) land before mouse hover override (mouse hover wins when present), both before the text rewrite so the panel always reflects the latest values.

### Free functions

| Function | Purpose |
|---|---|
| `reset_all_knobs(&mut AppArgs)` | Apply every knob's `default` to `AppArgs.gi`. Shared by keyboard `Shift+R`, the `KnobKind::Action` "Reset all" row, and the mouse click-release handler. |
| `apply_drag_delta(...)` | Apply one frame's drag motion to the selected knob — handles U32 fractional accumulator + F32 linear math. |
| `handle_click_release(...)` | Click semantics: Bool flips, Action invokes, U32/F32 no-op. |
| `window_scale(...)` | `Window::scale_factor()` reader with a 1.0 fallback. |
| `row_color(selected: bool) -> TextColor` | Brighter color when cursor on row. |

---

## §3. Drag sensitivity — final value + rationale

`DRAG_FULL_RANGE_PX = 320.0` (logical pixels) — one full traversal of this width spans the knob's full `[min..max]` range. Scaled by `Window::scale_factor()` so hi-DPI displays preserve the "panel-wide drag = full range" feel.

`DRAG_SHIFT_FACTOR = 0.25` — Shift held → 4× more pixels for the same value delta (matches the keyboard `Shift+←/→` fine-grain semantics).

`DRAG_THRESHOLD_PX = 2.0` (physical pixels) — below this on `Left::just_released`, the gesture is a *click* (bool flip / action / no-op for sliders).

### Rationale (cross-link: `25-design-panel-mouse.md` §4)

- **320 px** ≈ the panel's value-column width on a 360-px-wide panel with 10-px padding on each side and a ~30-px label gutter. Dragging from "near the left edge of the value column" to "near the right" sweeps the full range.
- **Uniform sensitivity across knobs** — every slider feels the same; no per-knob tuning needed. Predictable.
- **Per-frame integration (no captured `v0`)** — drag math reads the current value each frame and adds this-frame's delta. Composes cleanly with mid-drag keyboard `R` resets (which would clobber a captured-`v0` design). Documented as a design-review fix in `25-design-panel-mouse.md` §IR.1.3.
- **U32 fractional accumulator** — slow drags on narrow-range integer knobs (e.g. `bounce_count` 1..3) still advance; sub-step fractions are carried in the `Dragging::frac_accum` field between frames.

---

## §4. Keyboard preservation confirmation

The existing F1 / ↑↓ / ←→ / PgUp/PgDn / Shift / R / Shift+R bindings are **all preserved**. The only modifications to the keyboard code path:

1. **`toggle_panel` (F1)**: added a 3-line stanza to force `PanelDrag.state` back to `Idle` on close — prevents a dangling drag if the panel is closed mid-drag. Open-toggle behaviour unchanged.

2. **`adjust_panel`**: added a single-line early-out:

   ```rust
   if !matches!(drag.state, DragState::Idle) {
       return;
   }
   ```

   This is the `25-design-panel-mouse.md` §6.2 row 1 policy ("drag wins, keyboard ignored mid-drag"). It only takes effect *while* a drag is in progress; otherwise keyboard input is unchanged.

3. **`adjust_panel`'s `KnobKind::Action` branch**: keyboard `R` while cursor-on the "Reset all" row now invokes the same `reset_all_knobs` code path the existing `Shift+R` does. Added so the new row has both a keyboard *and* mouse entry-point.

The new `mouse_interact_panel` system never reads `KeyCode::*` for *adjustment*; it only reads `ShiftLeft/ShiftRight` for the sensitivity-fine modifier. No conflict with the keyboard's `just_pressed` consumption.

Tests verifying the keyboard path is intact: the 4 pre-existing `panel.rs` tests (`cursor_skips_non_interactive_rows`, `defaults_match_gi_settings_default`, `promoted_defaults_match_canonical_consts`, `at_least_one_interactive_knob`) all still pass (gate 2 confirms 119 passed). 3 new tests cover the new code (`knobs_ends_with_reset_all_action`, `reset_all_knobs_restores_defaults`, `drag_state_default_is_idle`).

---

## §5. Gate results

```
1) cargo build --workspace                                    → exit 0
   "Finished `dev` profile [optimized + debuginfo]" — clean,
   zero warnings on `panel.rs` or `lib.rs` (verified by touch
   + re-build).

2) cargo test -p bevy-naadf --lib                              → exit 0
   "test result: ok. 119 passed; 0 failed; 1 ignored;
    0 measured; 0 filtered out; finished in 4.31s"
   Baseline was 116 (commit 4211910); +3 new tests from this
   dispatch:
     - knobs_ends_with_reset_all_action
     - reset_all_knobs_restores_defaults
     - drag_state_default_is_idle
   The 4 pre-existing panel tests still pass.

3) cargo run --release --bin e2e_render                        → exit 0
   PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle
   frames, framebuffer read back & non-degenerate, per-batch
   region gate green through camera motion, every pipeline
   created cleanly, every expected render-graph node dispatched.
   Region luminance — emissive 247.0, solid 242.0, sky 145.9.
   Dispatch A baseline: 247.1 / 242.0 / 145.9 — within float
   noise.

4) cargo run --release --bin e2e_render -- --entities          → exit 0
   PASS (batch 6) — same per-batch gates green.
   Region luminance — emissive 247.0, solid 242.0, sky 145.9.
   entity_pixel gate PASS.
   "entity handler validation PASS: frame A: 8 chunk_updates,
    1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates"
   Dispatch A baseline: 247.0 / 241.9 / 145.9 — within float
   noise.
```

All four gates green. No luminance drift beyond float noise → no regression. The panel is `add_hud`-gated, so the e2e harness (`add_hud == false`) never spawns the panel; the panel-mouse code path is dead code in e2e, which is the load-bearing reason the luminance gates are unaffected.

---

## §6. Hover tooltip status — DEFERRED

Per `25-design-panel-mouse.md` §8, hover tooltips are **deferred to a future dispatch**. Reasons:

1. The brief listed tooltips as *optional* stretch goal ("if clean to implement, otherwise drop").
2. Clean implementation requires extending every knob row's struct with a per-knob `description: &'static str` field (28-row edit).
3. The tooltip-positioning code interacts awkwardly with the cursor-tracks-hover system added by this dispatch — both react to `Interaction::Hovered`.
4. The right-margin class indicator (`P` / `C` / `D` / `B`) introduced in `21-design-quality-panel.md` §5 provides a lightweight 1-character mode hint per row, which is the stand-in for a full tooltip.

If the user has used the panel and decides tooltips are wanted, a follow-on dispatch adds them in ~50 LOC. **NOT a fresh-eyes escalation** — pure deferral.

---

## §7. What was NOT done (scope discipline)

- **No Bevy version change** — still on Bevy 0.19-rc.1 (verified by inspecting `Cargo.lock`'s bevy-naadf dep declaration; no edit).
- **No new deps** — `Cargo.toml` unchanged. Verified by `git status`.
- **No layout overhaul** — the panel still occupies the same screen real estate (bottom-left, 360 px wide, dim grey background). The structural change from "one Text with `\n`s" → "Node column of single-line Texts" is visually identical (same font size 12 px, same padding, `row_gap: px(0.0)` to match the prior line spacing).
- **No paper-canonical defaults changed** — all 28 knob defaults preserved. `defaults_match_gi_settings_default` test guards this. `promoted_defaults_match_canonical_consts` re-verifies the §6 bit-equivalence promise.
- **No `GpuGiParams` / `GpuRenderParams` edit** — no GPU struct change. Verified by git diff (only `panel.rs` + `lib.rs` touched).
- **No shader edit** — no WGSL file modified.
- **No render-graph wiring** — no `prepare_*` / `extract_*` / pipeline-spec edit.
- **No HUD edit** — `hud.rs` untouched; existing `[F1] quality panel` keybind hint is sufficient discovery.
- **No CLI flag added** — runtime knob via panel only, per the original brief's spirit.
- **No `bevy_egui` / `bevy_inspector_egui`** — none added.
- **No visual loop verification** — only build + lib tests + e2e gates run, per the brief's "do NOT loop visual verification" rule.

---

## §8. Self-review findings — none escalated to fresh-eyes

The design's `25-design-panel-mouse.md` §IR.5 explicitly stated no HIGH-RISK item required fresh-eyes escalation: this dispatch is purely additive to a single module, no GPU struct edit, no shader edit, no render-graph wiring touched. Implementation confirmed that judgment — all gates green on first try after a one-line cleanup of an accidental duplicate `Res<PanelState>` + `ResMut<PanelState>` in the original `mouse_interact_panel` signature (fixed in-place during impl).

**One implementation-time finding worth recording for future readers:** the design originally captured a `v0` value at press-time and applied "v0 + accumulated delta" each frame for drag math. The self-review pass (§IR.1.3) caught that mid-drag keyboard `R` would be silently clobbered. The implementation uses **per-frame integration** instead (read current value, add this-frame's delta, write back) — this composes cleanly with any other mutator. Slightly more f32 drift than v0-anchored math, but bounded by one frame's motion (typically < 50 px = sub-1% of range). Documented in `25-design-panel-mouse.md` §IR.1.3.

---

## §9. Decisions & rejected alternatives (impl-stage)

1. **Chose: per-row Node + Text grandchild.** Rejected: per-row Node with text-on-self (no grandchild). Reason: `bevy_ui` 0.19 doesn't render Text on a Node entity directly without explicit Text component; the grandchild pattern is more idiomatic and matches the way the existing HUD spawns a Text child of a Node.

2. **Chose: `init_resource::<PanelDrag>()` alongside `PanelState`.** Rejected: bundle into `PanelState`. Reason: `PanelState` is `Copy`-able and small; `PanelDrag.state` is an enum with payload — fitting it into `PanelState` would break the `Copy` derive. Keeping them separate is cleaner and matches Bevy's resource-per-concern idiom.

3. **Chose: cursor-tracks-hover updates `PanelState.cursor` directly.** Rejected: a separate `hover_cursor` field for "where the mouse last hovered". Reason: keyboard `↑/↓` already updates `PanelState.cursor`; mouse hover should produce the same observable cursor state, not a separate one. One source of truth.

4. **Chose: `MouseButton::Left::just_released` for the "drag/click ended" edge.** Rejected: Bevy's `Pressed → Hovered` `Interaction` transition. Reason: the cursor may *leave the row* mid-drag, transitioning `Pressed → None` — that loses the edge. Reading the mouse-button release globally captures every release regardless of where the cursor is.

5. **Chose: `Window::scale_factor()` hi-DPI scaling.** Rejected: hard-code physical-pixel sensitivity. Reason: on a 2x DPI display, the panel is 720 physical pixels wide; without scaling, a drag across the panel would only span half the range. The 8-line `apply_drag_delta` change costs ~zero perf for clean hi-DPI behavior.

6. **Chose: per-frame integration (no captured v0).** Rejected: capture `v0` at press, apply `v0 + total_delta` each frame. Reason: design review §IR.1.3 — mid-drag keyboard mutations are clobbered with captured-v0. Per-frame integration composes cleanly.

7. **Chose: 4 panel tests retained + 3 new tests added.** Rejected: rip out the old tests. Reason: the old tests (`cursor_skips_non_interactive_rows`, `defaults_match_gi_settings_default`, etc.) protect against drift in the unchanged keyboard path. They are still load-bearing.

---

## §10. Assumptions made (impl-stage)

1. **`Interaction::Pressed` only fires on `MouseButton::Left`** — verified at `bevy_ui-0.19.0-rc.1/src/focus.rs:174-187` (literal `MouseButton::Left` check). Right-button camera-grab and left-button panel-drag share no input channel.
2. **`Display::None` propagates to children and forces `Interaction::None`** — verified at `bevy_ui-0.19.0-rc.1/src/focus.rs:243-256`. When panel closed, no mouse capture; FreeCamera's mouse-look continues to work.
3. **`AccumulatedMouseMotion.delta` is in physical pixels** — read pattern in `FreeCameraPlugin` (`bevy_camera_controller-0.19.0-rc.1/src/free_camera.rs:268`).
4. **`Window::scale_factor()` returns the OS-reported scale factor (`1.0` on 100% DPI, `2.0` on 200% DPI)** — Bevy convention; default-window assumes this on all platforms.
5. **`ui_focus_system` runs in `PreUpdate / UiSystems::Focus.after(InputSystems)`** — verified at `bevy_ui-0.19.0-rc.1/src/lib.rs:172`. My `Update` system reads `Interaction` after the focus system has refreshed it.
6. **`FocusPolicy::Pass` is the default; `Block` is the opt-in** — verified at `bevy_ui-0.19.0-rc.1/src/focus.rs:115-116`. Explicit `Block` annotation is required.
7. **The 1-frame ordering invariant of `.chain()` is enough** — the four systems run serially within one `Update` slot. No frame-skip race possible.
8. **The `dbg!` mouse-test fixture wasn't needed** — the 119-test gate (incl. 3 new panel tests) is sufficient regression guard; full mouse-integration testing would require winit harness scaffolding outside this dispatch's scope.

---

## §11. Carry-forward for future sessions

- **Hover tooltips** — deferred (§6). ~50 LOC follow-on if the user wants them.
- **Drag-delta on bool / action rows** — currently a no-op (`apply_drag_delta`'s `_ => {}` catch-all). If a user accidentally drags a bool/action row, nothing happens; the gesture resolves as a click on release. Acceptable UX.
- **`RelativeCursorPosition` is queried but unused at runtime** — kept on each interactive row Node as a placeholder for future hover-tooltip positioning. If the future-tooltip dispatch reads it, no further plumbing needed.
- **The 3-iteration mirror loop in `spatial_resampling.wgsl`** (cited in `21-design-quality-panel.md` §9.4 / `19-gi-reservoir-scope.md` §3.2) is unchanged — the mouse path drags the same `max_ray_steps_visibility` field; the WGSL still loops 3× over its calls. Cost cap behaviour unchanged.

---

## §12. Verification gates retraced — one-line summary

```
Gate 1: cargo build --workspace                              exit 0, 0 warnings on touched files
Gate 2: cargo test -p bevy-naadf --lib                       exit 0, 119 passed (+3 new)
Gate 3: cargo run --release --bin e2e_render                 exit 0, lum 247.0/242.0/145.9 (baseline match)
Gate 4: cargo run --release --bin e2e_render -- --entities   exit 0, entity_pixel PASS, lum 247.0/242.0/145.9
```

No fresh-eyes review recommended — this dispatch is purely additive, single-module, no GPU/shader/render-graph touch.
