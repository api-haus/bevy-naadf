# D2 — editor-and-settings-ui — exploration

**Author**: refactor-explorer (D2).
**Date**: 2026-05-20.
**Scope**: pure Bevy-UI HUD + Escape settings overlay + per-mode app gating + brush implementations. Read-only — no code changes. Findings are pointers for the D2 architect, not finished designs.

**Paths in scope (verified, LOC `wc -l`):**

| path | LOC |
|---|---|
| `crates/bevy_naadf/src/settings.rs` | 940 |
| `crates/bevy_naadf/src/editor/hud.rs` | 903 |
| `crates/bevy_naadf/src/editor/tools.rs` | 597 |
| `crates/bevy_naadf/src/editor/mod.rs` | 275 |
| `crates/bevy_naadf/src/hud.rs` | 245 |
| `crates/bevy_naadf/src/app_mode.rs` | 95 |
| `crates/bevy_naadf/src/editor/ray.rs` | 61 |
| **total** | **3 116** |

Domain matches the audit's §2 D2 row (3 120 LOC ± rounding).

**External call-graph constraints (verified with Grep):**

- `editor::tools::paint_brush` / `cube_brush` / `sphere_brush` are called from `src/e2e/oasis_edit_visual.rs:268`, `src/e2e/small_edit_repro.rs:231`, `src/e2e/small_edit_visual.rs:349`, and referenced in driver/bin docblocks. **Renaming or removing these functions is forbidden by `01-context.md` constraint #3 (zero-callers verification required).** Their signatures `(world_data, pos, radius, ty[, is_erase])` are e2e-pinned.
- `editor::EditorState`, `editor::EditTool`, `editor::hud::*` setup/handler systems are referenced from `lib.rs:915-959`. No e2e references to the HUD spawners themselves.
- `settings::setup_settings`, `settings::show_settings`, `settings::hide_settings`, `settings::adjust_settings`, `settings::mouse_interact_settings`, `settings::update_settings_text` are all referenced from `lib.rs:925-957`. No e2e references — the e2e harness sets `add_hud = false` so the entire D2 surface is dark in gates (`lib.rs:900`).
- `app_mode::AppMode`, `app_mode::toggle_settings_on_escape`, `app_mode::suspend_camera_input`, `app_mode::restore_camera_input` are all wired from `lib.rs:915-936`.

---

## Findings

### HIGH-1 — `editor/tools.rs` has two near-identical brush body triple-loops (cube vs sphere) — DUP-2 from audit §3.2

**Location:** `crates/bevy_naadf/src/editor/tools.rs:168-287`.

**What's there now:**
`cube_brush` (168-224) and `sphere_brush` (231-287) are 57 lines and 57 lines respectively, structurally **identical** modulo:

1. The classifier called: `cube_chunk_classify` vs `sphere_chunk_classify`.
2. The per-voxel test inside the `Mixed` arm: `cheb < radius` vs `d.length_squared() < r2`.

Everything else (the `if radius <= 0.0` guard, the `target`/`target_opt` setup, the `(min_chunk, max_chunk) = brush_chunk_aabb(...)` call, the inside-chunks + mixed-edits vec allocation, the triple `cz/cy/cx` chunk loop, the `match … { Outside => continue, Inside => push uniform, Mixed => triple-loop over `CHUNK_VOXELS` per-voxel test }` skeleton, the two final batch flushes) is byte-for-byte the same. The two classifier functions themselves (`cube_chunk_classify:108-121`, `sphere_chunk_classify:88-103`) also share the structure `(chunk_center compute → distance compute → r_inside/r_outside compute → 3-way branch)`.

**Why it's a problem:**
A new brush shape (cone? capsule? "magic wand"?) means a third 60-line copy of the same loop. The C# also has this duplication (`EditingToolCube.cs` vs `EditingToolSphere.cs` per the module docblock) so this isn't unique to the port, but the port is the place to fix it — the audit calls this out at §3.2 DUP-2.

**Suggested direction (NOT a design):**
A `BrushShape` trait with three methods — `chunk_classify(&self, chunk_pos, pos, radius) -> ChunkClass`, `voxel_in(&self, voxel_world_pos: Vec3, pos: Vec3, radius: f32) -> bool`, and a free fn `iterate_brush<S: BrushShape>(world, pos, radius, target, target_opt, shape)` that owns the chunk/voxel iteration skeleton. `paint_brush` (which has different semantics — only replaces non-empty, no inside fast-path) stays standalone. Names `cube_brush`/`sphere_brush` MUST be preserved as thin wrappers because e2e gates call them.

**Out-of-scope ripple:**
- `editor/tools.rs:182,193,247,257` build `([u32; 3], Option<VoxelTypeId>)` and `(IVec3, VoxelTypeId)` anonymous tuples to pass to `WorldData::set_chunks_uniform_batch` / `set_voxels_batch`. The audit's UA-1 tags this as D1 + D2 crosscut; the named type (`pub struct VoxelEdit { pos: IVec3, ty: VoxelTypeId }`) would have to land in D1's `world::data` first before D2 can consume it. **D2 should NOT define this type itself** — that's D1's surface.

---

### HIGH-2 — Style boilerplate (font + size + color) repeats 12+ times across `settings.rs`, `editor/hud.rs`, `hud.rs`

**Location:**
- `settings.rs:468-475, 504-510, 530-536`
- `editor/hud.rs:280-284, 348-352, 437-443, 451-457, 521-527`
- `hud.rs:117-123`

**What's there now:**
Every Text spawn carries an inline 5-6-line block of the shape:

```rust
TextColor(SOME_COLOR),
TextFont {
    font: dev_font.0.clone(),
    font_size: FontSize::Px(N.0),
    ..default()
},
```

There are **at least 9 occurrences** (Grep'd above: settings.rs has 3, editor/hud.rs has 5, hud.rs has 1; the global FPS/timing HUD adds 1). Font sizes used: 10, 11, 12, 13, 14, 16 (with `13` being the row-text size, `16` the heading, `11` the legend, `12` the hover-info / labels). The colors used are file-local `COL_*` consts.

**Why it's a problem:**
Each new label needs the same 5-line struct-literal block; the only varying parts are *font size* and *text color*. The pattern is identical enough that a `text_label(dev_font, color, size_px) -> impl Bundle` helper would let every call site read `text_label(&dev_font, COL_VALUE, 13.0)` plus the `Text::new("…")`. The audit's `00-reuse-audit.md §2 D2.suspicion-2` flags this exact ratio (903 vs 940 LOC, both authoring near-identical bevy_ui).

**Suggested direction (NOT a design):**
A shared `ui_helpers` (or `editor::ui_helpers`) submodule with two or three thin constructors:

- A bundle constructor for "the dev-font text block at size N px in colour C".
- A bundle constructor for "the standard panel button" (currently re-spawned in `spawn_tool_button`, `spawn_toggle_button`, and (de-facto) every settings row).
- Possibly a "centered modal panel root" helper if the settings panel's backdrop+root nesting recurs anywhere else (it does not today, so this third one is speculative).

Architect to decide: live inside `editor/` (since the editor is the heavier consumer) or as a top-level `ui_helpers` module that `hud.rs`, `editor/hud.rs`, and `settings.rs` import. The current arrangement has `hud.rs` and `editor/hud.rs` as siblings — they share zero code today.

**Out-of-scope ripple:**
- `hud.rs` (the FPS/timing overlay) is *also* in D2 per the brief — it would consume this helper. Its content (pass-timing labels) is unrelated to the editor HUD though, so the shared surface is style-only.

---

### HIGH-3 — `settings.rs` re-implements `bevy_reflect` from scratch — BEV-4 + OA-1 from audit §3.3 / §3.4

**Location:** `crates/bevy_naadf/src/settings.rs:112-378` (the `Knob` + `KnobKind` types, the `KNOBS: &[Knob]` table, the `is_interactive` helper, the `reset_all_knobs` traversal).

**What's there now:**
- `Knob` (112-116) is `{ label, class, kind: KnobKind }`.
- `KnobKind` (118-150) is a 5-variant enum where each non-`Section` / non-`Readonly` variant carries 2-7 function pointers: `getter: fn(&GiSettings) -> u32`, `setter: fn(&mut GiSettings, u32)`, `nudge`, `big_step`, `min`, `max`, `default` (and an `apply: fn(&mut AppArgs)` for `Action`, `value: fn(&AppArgs) -> String` for `Readonly`).
- `KNOBS: &[Knob]` (166-378) is a **213-line** table of 30 rows. Every interactive row repeats 7-8 fields by hand. Every closure is `|g| g.field` / `|g, v| g.field = v` — i.e. a literal field-accessor pair, exactly what `bevy_reflect`'s `ReflectMut` / `from_reflect` machinery is for.
- `KNOBS` consumers: `reset_all_knobs` (380-389, matches on `KnobKind`), `first_interactive` (392-394), `step_cursor` (838-850), `setup_settings` (480-514 — spawns one row per entry), `show_settings` (557 — bounds check), `adjust_settings` (610-645 — matches on kind and calls getter/setter), `mouse_interact_settings` (660 + 664 + 672 — bounds check + classify), `apply_drag_delta` (721-749 — matches on kind), `handle_click_release` (753-760), `update_settings_text` (775-806, 809-815). **Eight separate match-on-`KnobKind` sites.**

**Why it's a problem:**

1. Adding a new GI knob requires touching three places: (a) add a field to `GiSettings` in `lib.rs`, (b) add a `setter` somewhere that maps to a `GpuRenderParams`/`GpuGiParams` field (D4 territory), (c) add a 7-field row to `KNOBS` in `settings.rs`. The current table-of-fn-pointers is a manual reflection implementation. Bevy 0.19 ships `bevy_reflect` for exactly this — it can iterate a `Reflect`-derived struct's fields generically, read/write by name, and combined with a `#[reflect(...)]` attribute can carry the nudge/big_step/min/max/default metadata.
2. The `class: char` field — `'P'`, `'C'`, `'B'`, `'D'`, `' '` — is a single-byte tag used only by `update_settings_text` (formatting suffix `[P]` / `[C]` / etc) and is unexplained. No grep'd doc for what `P/C/B/D` means; the reader has to infer from context (`P` ≈ "performance", `C` ≈ "config", `B` ≈ "button", `D` ≈ "diagnostic"). This is a weak-type / magic-character smell on top of the larger smell.

**Suggested direction (NOT a design):**

Two distinct moves, architect picks which (or both):

- **(a) `Reflect`-driven:** derive `Reflect` on `GiSettings`, attach a custom attribute-derived metadata table (e.g. `#[knob(nudge = 8, big_step = 32, min = 1, max = 512)]` per field; section headers via `#[knob(section = "RAY STEP CAPS")]`). The panel iterates fields generically. Drops `KNOBS` to a one-line `GiSettings::reflect_fields()` style call. Eight `match KnobKind` sites collapse to a single dynamic dispatch.
- **(b) Decl-macro `knob!{...}`:** if `Reflect` feels too heavyweight (the audit's `01-context.md` Low-confidence note says "architects re-examine" this exact call), a `knob!{ label="primary", field=max_ray_steps_primary, nudge=8, big_step=32, min=1, max=512 }` macro expands to the current 7-field row. Same table-of-rows shape, but each row drops from 7 lines to 1 line + the macro removes the literal-default duplication (`default: 120` in the row literally matches `GiSettings::default().max_ray_steps_primary = 120` in `lib.rs:223`).

**The `class: char` field**: regardless of (a) or (b), kill the magic char. Either an `enum KnobClass { Perf, Config, Button, Diagnostic }` or — better — derive the styling from the variant itself (`Section` already self-renders; `Action` already self-renders; `Readonly` already self-renders; `U32`/`F32`/`Bool` could all just use the same colour). Audit whether the `[P]`/`[C]`/`[D]` suffix is even *used* — `update_settings_text` writes it, but a user would just read the row's category from the section header above it.

**Open question for the architect (HIGH-3.q):**
The `01-context.md` low-confidence note says "the project may deliberately keep KNOBS explicit for compile-time safety — architects judge". The current explicit table catches a missing field at compile-time via the `getter: fn(&GiSettings)` pointer; a `Reflect`-driven version moves that check to a per-knob runtime "does this field exist on `GiSettings`?" — caught by the `defaults_match_gi_settings_default` test at `settings.rs:875-892` only at test time, not at compile time. **Trade compile-time exhaustiveness for editing ergonomics? Architect call.**

**Out-of-scope ripple:**
- `GiSettings` lives in `lib.rs:109-184` (D7's territory). Deriving `Reflect` on it touches a D7 file. The audit's `01-context.md §"Crosscutting"` notes "GiSettings lives in lib.rs, but the knobs table that drives it lives in settings.rs" — moving `GiSettings` into D2 would be the cleaner home long-term but is a D7 decision. **The architect should propose the surface change but flag D7 must approve.**
- The `setter`s reach into `args.gi` (`AppArgs.gi` field). `AppArgs` is in `lib.rs:296+` — same D7-owned file.

---

### HIGH-4 — `settings.rs` SSoT-1 partial — KNOBS table hardcodes the 5 ray-step caps **with literal default values** that duplicate `GiSettings::default()` literals

**Location:**
- `crates/bevy_naadf/src/settings.rs:174` (`default: 120` for primary)
- `crates/bevy_naadf/src/settings.rs:184` (`default: 100` for secondary)
- `crates/bevy_naadf/src/settings.rs:194` (`default: 120` for sun)
- `crates/bevy_naadf/src/settings.rs:202` (`default: 80` for sun-secondary)
- `crates/bevy_naadf/src/settings.rs:210` (`default: 60` for visibility)
- `crates/bevy_naadf/src/settings.rs:220` (`default: 12` for spatial_iter_count)

vs.

- `crates/bevy_naadf/src/lib.rs:223-228` (the matching `GiSettings::default()` literals).
- `crates/bevy_naadf/src/render/gpu_types.rs:87` (the uniform field `max_ray_steps_primary`).
- `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:122-126` (the WGSL `MAX_RAY_STEPS_*` consts, kept deliberately per audit `01-context.md`).

**What's there now:**
Each ray-step cap appears as a number literal in `lib.rs` (the `Default` impl), the `KNOBS` row (`default:` field), and `ray_tracing.wgsl`. The unit test `settings.rs:897-905` (`promoted_defaults_match_canonical_consts`) and `settings.rs:875-892` (`defaults_match_gi_settings_default`) catch drift between the first two. The WGSL side is not auto-checked.

**Why it's a problem:**
A user who edits `lib.rs:223` to bump `max_ray_steps_primary` from `120` to `140` will (a) get the test failure from `defaults_match_gi_settings_default` *if* they remember `KNOBS` exists, (b) silently keep the WGSL const at `120` even though the WGSL const is documented (`ray_tracing.wgsl:128`) as *"MUST equal"* the `GiSettings::default()` value. The audit's `01-context.md §3.1 SSoT-1` says **"3 sources of truth for the same 5 numbers."** In the D2 file specifically, the `default: 120` literal is the simplest one to remove — it could reference `GiSettings::default().max_ray_steps_primary` directly (the `Default` impl is `const`-eligible per Rust 2026, but `GiSettings::default()` is not a `const fn` today; an `impl GiSettings { pub const DEFAULT: GiSettings = GiSettings {...} }` constant would work).

**Suggested direction (NOT a design):**
- The cleanest path is bundled with HIGH-3: if `Reflect`-driven or decl-macro is adopted, the `default:` field reads from `GiSettings::default()` (or a `const DEFAULT: GiSettings`) at panel-construction time and the literal disappears from `KNOBS`.
- The minimal path: define `const GI_DEFAULTS: GiSettings = GiSettings { ... }` in `lib.rs` and reference it from `settings.rs` so the `default:` fields read `GI_DEFAULTS.max_ray_steps_primary` instead of `120`.

**Out-of-scope ripple:**
- This finding's full fix crosses into D4 (`render/gpu_types.rs:87`) and D4's WGSL `ray_tracing.wgsl:122-126`. D2 owns only the `KNOBS` table copy. **Architect: propose how to consume the SSoT, then flag D4 + D7 to land their halves.** D2's part is the easy half.

---

### MEDIUM-5 — `editor/mod.rs::EditorState::tool_from_u32` is dead code

**Location:** `crates/bevy_naadf/src/editor/mod.rs:107-114`.

**What's there now:**
A `pub fn tool_from_u32(v: u32) -> EditTool { match v % 3 { 0 => Paint, 1 => Cube, 2 => Sphere, _ => Paint } }` with a matching unit test at `editor/mod.rs:246-251`.

**Why it's a problem:**
Grep'd the entire workspace — only the unit test and its own definition reference `tool_from_u32`. No keyboard cycling system uses it; no HUD click-cycle uses it; no CLI flag uses it. It looks like a leftover from a planned "press Tab to cycle tools" feature that didn't ship. The cube/sphere/paint selection is wired through `editor::hud::handle_hud_clicks` reading `ToolButton(EditTool)` markers (`editor/hud.rs:747-751`) — no integer conversion needed.

**Suggested direction (NOT a design):**
Delete the function + its test. Trivial. Bevy's `#[repr(u32)]` on `EditTool` already covers the "convert to/from u32" use case if it ever becomes needed (`as u32` works directly; `from_repr` would need `num_derive` but isn't called anywhere).

**Out-of-scope ripple:**
None. Pure D2.

---

### MEDIUM-6 — `settings.rs::setup_settings` does its `_ = DEFAULT_TAA_RING_DEPTH` dance instead of importing-on-use

**Location:** `crates/bevy_naadf/src/settings.rs:543`.

**What's there now:**

```rust
// Keep the const re-export-still-used compiler check warm.
let _ = DEFAULT_TAA_RING_DEPTH;
```

Followed by the use being entirely fictitious — `DEFAULT_TAA_RING_DEPTH` is imported (line 32) but never used in `settings.rs` except for this `let _ =`. The actual current value displayed in the diagnostics row is `a.taa_ring_depth` (`settings.rs:328`), read from `AppArgs` directly.

**Why it's a problem:**
This is a deliberate "keep the import warm so clippy doesn't drop it" hack. The const is re-exported by `lib.rs:274` (`pub const DEFAULT_TAA_RING_DEPTH: u32 = 32`) and consumed elsewhere (`render/taa.rs:36 doc-link only`, `render/mod.rs:111` actual use, `lib.rs:446` actual use). The "unused warning" being suppressed is a phantom — the const is not actually used in `settings.rs`. Drop the import + drop the `let _`.

**Suggested direction (NOT a design):**
Remove the import + the `let _` line. If the diagnostics row at line 328 wanted to fall back to the const when `a.taa_ring_depth` is missing, that fallback was lost — but `AppArgs.taa_ring_depth` is `u32` not `Option<u32>` so the const is genuinely unused.

**Out-of-scope ripple:**
None — both the import and the `let _` are pure D2.

---

### MEDIUM-7 — `settings::adjust_settings` keyboard handler + `mouse_interact_settings` drag handler each duplicate the "kind-dispatch + min/max clamp" math

**Location:**
- `crates/bevy_naadf/src/settings.rs:610-645` (`adjust_settings` — keyboard arrow + PgUp/PgDn + R/Shift+R nudge)
- `crates/bevy_naadf/src/settings.rs:721-749` (`apply_drag_delta` — mouse drag scrubbing)
- `crates/bevy_naadf/src/settings.rs:752-761` (`handle_click_release` — bool/action toggle on click)

**What's there now:**
All three functions match on `KnobKind` and write the value back via `setter(&mut args.gi, ...)`. Each has its own clamp / saturation logic but the structure is the same. `adjust_settings:613-620` (U32 keyboard) reads the same `getter/setter/nudge/big_step/min/max/default` set that `apply_drag_delta:728-740` reads. The "compute a u32 delta and apply" logic is split.

**Why it's a problem:**
Three different code paths nudge the same knob and they each have to keep the clamp/saturation invariant correct independently. The clamp lives in three places (saturating_sub/add for keyboard, `.clamp(min, max)` for everything else, `(cur + whole as i64).clamp(min as i64, max as i64) as u32` for drag — note the i64 widening in only one of the three). The "interactive knob" surface should expose a single `nudge(getter, setter, delta_units)` method or similar.

**Suggested direction (NOT a design):**
If HIGH-3's Reflect/macro work lands, this collapses too — each knob exposes `apply_delta(&mut GiSettings, n_units: f32)` once and the keyboard / drag / click sites all call the same primitive. Even without HIGH-3, a `Knob::set_clamped(&self, args: &mut AppArgs, new_value_or_delta)` method on `KnobKind` would centralize the clamp.

**Out-of-scope ripple:**
None — pure D2.

---

### MEDIUM-8 — `editor/hud.rs::update_editor_hud` has parallel match blocks for Erase + Continuous toggle buttons that diverge from `tool_buttons` block

**Location:** `crates/bevy_naadf/src/editor/hud.rs:839-877`.

**What's there now:**
Two near-identical 18-line blocks (`erase_buttons` loop at 839-857, `cont_buttons` loop at 859-877). Each:

1. Computes `hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed)`.
2. Picks `(target_bg, target_border, text_color)` from a 4-way ternary based on `(erase_affects, state.is_erase|is_continuous, hovered)`.
3. Writes `*bg`, `*border`, and the child's TextColor.

These two blocks differ in:
- The marker queried (`EraseToggle` vs `ContinuousToggle`).
- The state field read (`state.is_erase` vs `state.is_continuous`).

**Why it's a problem:**
Same shape as DUP-2 (cube/sphere) but at the HUD layer. Two near-identical 18-line loops drifting independently is a maintenance hazard — change the disabled-color scheme and you've got two places to update. The `tool_buttons` block at 821-832 has a *third* slightly-different version of "select highlight + hover".

**Suggested direction (NOT a design):**
A small helper `update_toggle_style(interaction, hovered, is_on, disabled, &mut bg, &mut border, &mut text_color)` that both blocks call. Or a `ToggleVisualState` enum (`Off`, `OffHover`, `On`, `OnHover`, `Disabled`) computed once per row, then a single match for the colour mapping.

**Out-of-scope ripple:**
None — pure D2.

---

### LOW-9 — `editor/hud.rs` has 5 colour constants whose only difference is alpha — and the colour palette spreads across both UI files

**Location:**
- `crates/bevy_naadf/src/editor/hud.rs:109-124` (16 colour consts)
- `crates/bevy_naadf/src/settings.rs:44-56` (12 colour consts)

**What's there now:**
The editor HUD declares `COL_BTN_BG`, `COL_BTN_BG_HOVER`, `COL_BTN_BG_SELECTED`, `COL_BTN_BG_DISABLED`, `COL_BTN_BORDER`, `COL_BTN_BORDER_SELECTED`, `COL_TEXT_PRIMARY`, `COL_TEXT_MUTED`, `COL_TEXT_DISABLED`, plus `COL_HUD_BG`, `COL_SLIDER_TRACK`, `COL_SLIDER_FILL`, `COL_SWATCH_BORDER`, `COL_SWATCH_BORDER_SELECTED`, `COL_SCROLLBAR_TRACK`, `COL_SCROLLBAR_THUMB`. Settings declares its own `COL_BACKDROP`, `COL_PANEL_BG`, `COL_PANEL_BORDER`, `COL_HEADING_BG`, `COL_SECTION`, `COL_ROW_HOVER`, `COL_ROW_SELECTED`, `COL_VALUE`, `COL_VALUE_SEL`, `COL_READONLY`, `COL_RESET_BG`, `COL_RESET_BG_HOVER`.

**Why it's a problem:**
Two parallel "design palettes" with overlapping concepts (`COL_BTN_BG_HOVER` in editor vs `COL_RESET_BG_HOVER` in settings; `COL_TEXT_PRIMARY` vs `COL_VALUE`; `COL_BTN_BG_SELECTED` vs `COL_ROW_SELECTED`). No semantic palette layer — if a user wants to retheme the app's button-hover colour, they touch 2 files. Low blast-radius today (UI doesn't change often) but the duplication will rot.

**Suggested direction (NOT a design):**
A `theme.rs` or `editor::theme` submodule with semantic-named colours (`HOVER_TINT`, `SELECTED_TINT`, `MUTED_FG`, `BG_PANEL`, `BG_BUTTON`, ...) consumed by both files. Architect call on naming — the current file-local consts have decent self-documentation (`COL_RESET_BG` = "the red 'Reset all' button background") so the consolidation only pays off if you also collapse equivalent concepts.

**Out-of-scope ripple:**
None — both files are D2.

---

### LOW-10 — `editor/hud.rs::handle_hud_clicks` is a 60-line system with 5 Query args and `#[allow(clippy::too_many_arguments)]`

**Location:** `crates/bevy_naadf/src/editor/hud.rs:734-792` (and `update_editor_hud` at 796-903 has 7 Query args, also annotated).

**What's there now:**
`handle_hud_clicks` takes one `Res`, one `Local`, one `Window` Query, and 4 entity Queries — one per HUD element type (tool buttons, swatches, erase, continuous, slider). Each loop is short (3-4 lines) but the function is wide. `update_editor_hud` is the same shape but bigger.

**Why it's a problem:**
The `too_many_arguments` clippy lint exists for a reason — these systems mix five unrelated event sources. Each could be its own system (`handle_tool_buttons`, `handle_swatch_clicks`, `handle_erase_toggle`, `handle_continuous_toggle`, `drag_radius_slider`). Bevy's parallel scheduler benefits from finer-grained systems (each only locks the components it queries).

**Suggested direction (NOT a design):**
Split each block into its own `Update` system. Slightly more registration in `lib.rs:946-957` but each system is testable in isolation and Bevy's scheduler can run them concurrently where the borrow graph allows.

**Out-of-scope ripple:**
- `lib.rs:946-957` would gain a few more system names in the `.chain()` (D7's territory). The chain is structurally `editor::hud::*` only so it's a contained edit.

---

## Confirmed / refuted audit suspicions

### Confirmed

- **OA-1 + BEV-4 (`KnobKind` function-pointer table)**: confirmed at `settings.rs:112-150` and the 30-row `KNOBS` table at `settings.rs:166-378`. See HIGH-3. The architect must decide between Reflect, decl-macro, or "stay explicit" per the audit's low-confidence flag.
- **DUP-2 (3 brush AABB / classify fns)**: confirmed at `editor/tools.rs:47-122`. Two AABB helpers (voxel AABB + chunk AABB) and two chunk-classify helpers (cube cheb + sphere euclid). See HIGH-1.
- **SSoT-1 partial (max_ray_steps_* literal defaults)**: confirmed at `settings.rs:174,184,194,202,210,220` — each ray-step cap default + spatial_iter_count default repeats the literal in `lib.rs:223-228`. See HIGH-4.
- **UA-1 partial (anonymous `(IVec3, VoxelTypeId)` and `([u32;3], Option<VoxelTypeId>)` tuples)**: confirmed at `editor/tools.rs:139, 153, 182, 193, 207, 246, 256, 270` — four call sites build the anonymous tuples to pass to `WorldData::set_voxels_batch` / `set_chunks_uniform_batch`. **D2 is a consumer only — the named type must land in D1 first.**
- **The audit's `00-reuse-audit.md §2 D2.suspicion-2` (HUD vs settings UI builder duplication)**: confirmed — both files author bevy_ui Node trees with ~9 sites of `TextFont { font: dev_font.0.clone(), font_size: FontSize::Px(N) }` boilerplate. See HIGH-2.

### Refuted / narrowed

- **The audit's `00-reuse-audit.md §2 D2.suspicion-2` claims a `spawn_h_row`/`spawn_v_row` helper exists in `editor/hud.rs`**: only `spawn_h_row` exists (`editor/hud.rs:297-309`); there is no `spawn_v_row`. The settings panel uses a plain inline spawn with `flex_direction: FlexDirection::Column` (`settings.rs:430-446`) and does not reuse this helper. A genuine helper would consolidate both — see HIGH-2.
- **The audit suggests "shared `node_*`/`row_*` helpers would deduplicate"**: the bigger win is the `TextFont`/`TextColor` boilerplate (HIGH-2), not the Node spawns themselves. The Node spawns differ enough between files (the settings panel has 1 unique structure, the editor HUD has 6 unique row shapes) that a single `row_*` helper would be hard to fit. The font + color helper is more tractable.
- **`feature-completeness/01-context.md ¶71` flags bevy_ui as a sanctioned divergence**: confirmed. No need to revisit the C# ImGui parity question.

---

## Side notes / observations / complaints

1. **The `lib.rs` `add_systems(Update, (...).chain())` for editor + settings is brittle.** `lib.rs:937-960` registers **9 systems in a single `.chain()`** for the editor + settings + app_mode pile. The audit's BEV-1 flags the same pattern in `render/mod.rs` for the render graph. Same smell, smaller scale. A `EditorPlugin` + `SettingsPlugin` + `AppModePlugin` extraction (D7's territory per audit §2 D7-suspicion-1) would let each subsystem own its system registration and the `.chain()` would only carry the cross-plugin order constraints. **Flag for D7.**

2. **`settings.rs` doc-comment at the top says the panel is "purely engine quality"** (line 14) **but the file is 940 LOC** of UI scaffolding + drag state machine + reflection-from-scratch + diagnostic readback rows. The docblock undersells the file's scope. The body is doing real work (the drag-state machine at `settings.rs:73-79` is non-trivial — `Idle → Pressed → Dragging` with a threshold to distinguish click from drag). This isn't rot, just under-documented complexity.

3. **`editor/hud.rs::scroll_palette_with_wheel`** (`editor/hud.rs:610-653`) **handles the case where Bevy's picking gives `Interaction::Hovered` only to the topmost hit-tested entity** — the comment at lines 605-609 explains the workaround. This is a Bevy idiom-fit problem at the framework level (Bevy 0.19 has no "any descendant hovered?" query), not a D2 problem. **Not actionable here**, but if Bevy 0.20+ ships a `HoveredHierarchy` filter the workaround could simplify.

4. **`editor/mod.rs::apply_edit_tool`'s "bail if HUD engaged" check at lines 142-156** iterates **two queries** (`hud_interactions` for the root, `hud_child_interactions` for descendants) — same workaround as #3. **Same root cause.** Could be combined with the same future "any descendant hovered" idiom.

5. **`editor/hud.rs` has two `Local` resources defined as standalone pub structs**: `RadiusSliderDrag` (lines 587-590) and `PaletteScrollbarDrag` (lines 595-598). Both have a single `pub active: bool` field. Both are `Local<...>` in systems. **Why are they `pub` and not module-private?** They have no consumers outside `editor/hud.rs` (grep'd). Drop `pub`.

6. **`editor/hud.rs::setup_editor_hud` is 160 lines (135-294)** and has no internal structure — flat run of `commands.spawn(...).id()` blocks for ~12 entities. The `spawn_h_row` / `spawn_tool_button` / `spawn_radius_slider` / `spawn_toggle_button` helpers extract some of the repetition but the palette viewport / scrollbar / strip / hover-info still inline 80+ LOC. A few more spawn helpers (`spawn_palette_viewport`, `spawn_scrollbar`, `spawn_hover_info_panel`) would bring the function back to ~30 LOC.

7. **The `EditorState` field `last_hover_hit: Option<RayHit>`** (`editor/mod.rs:87`) **is mutated every frame in `apply_edit_tool` and read every frame in `update_editor_hud`** (via `state.last_hover_hit` at `editor/hud.rs:889`). This is fine but it does mean `EditorState` carries both *configuration* (tool, radius, selected_type, is_erase, is_continuous) and *runtime cache* (`pos`, `stroke_just_started`, `last_hover_hit`). The docblock at `editor/mod.rs:56-89` calls this out explicitly. Possible split: `EditorConfig` (user settings) + `EditorRuntime` (per-frame state). Low priority — the docblock makes the mixing legible.

8. **Subjective reaction**: `settings.rs` is the most "Rust-y by-construction" file in D2 — the `KnobKind` enum + the `&[Knob]` table is a textbook "data-driven UI" design. It's not wrong, just out of step with what Bevy gives you for free via `Reflect`. If the user's stated goal is "tight idiomatic Bevy" (Q1 chosen direction), this file is the single highest-leverage change in D2.

9. **Subjective reaction**: `editor/tools.rs` reads cleanly. The C# docblock annotations at every function (cube/sphere/paint cite their `EditingTool*.cs:LL-LL` line ranges) make it easy to verify the port. The DUP-2 trait extraction is more about avoiding *future* drift than fixing *current* friction. Architect: weigh whether the trait abstraction adds more conceptual weight than the duplication does.

10. **The brief instructed**: *"This is a significant task in computer graphics — be vigilant; verify every file:line ref with Read/Grep before citing"*. I verified each cited `path:line` via `Read` (for files I read whole) or `Grep` (for spot-checks). No fabricated line numbers in this doc.

11. **Cross-domain rot I noticed while reading**: the `lib.rs:900-971` `add_hud` block conflates **four** orthogonal concerns: (a) FPS overlay setup, (b) editor HUD + settings overlay state init, (c) AppMode state init + Escape toggle, (d) wasm-only UI hide override. The audit calls this out under D7 — but D2's refactor will land into this same block (every system registered there is D2-owned except `hud::setup_hud`/`update_hud` and the `voxel::web_vox::hide_ui`). **Recommendation for the architect:** propose the surface change as "extract `EditorPlugin` + `SettingsPlugin` + `AppModePlugin`" and flag D7 to land them. The D2 architect should NOT do this themselves — but should *prepare for it* by structuring the refactor so the migration is one move.

---

## Open questions for the architect

- **HIGH-3.q (binding)**: trade compile-time exhaustiveness for editing ergonomics on the `KNOBS` table? `Reflect`-driven loses compile-time field-existence checking; decl-macro keeps it but adds a macro to read. User's stated goal is *"tight idiomatic Bevy"* — both fit; `Reflect` is more idiomatic Bevy specifically.
- **HIGH-4.q**: who owns the SSoT? D2's `KNOBS` defaults are downstream of `GiSettings::default()` (D7's `lib.rs`); D4's `gpu_types.rs` is also downstream of the same source. Propose: a single `const GI_DEFAULTS: GiSettings = GiSettings { ... }` in `lib.rs`, consumed by both `KNOBS` defaults and the WGSL shader-def upload. **D2 architect proposes; D7 architect lands; D4 architect coordinates the WGSL side.**
- **HIGH-2.q**: shared UI helpers live where? `editor/ui_helpers.rs` (editor-only, settings imports across), or a top-level `crates/bevy_naadf/src/ui_helpers.rs`? `hud.rs` is also a consumer. Cleanest is top-level since 3 files consume it. Architect call.
- **Side-note 11 (binding)**: how should the D2 architect's refactor stage to land cleanly after D7's `EditorPlugin` / `SettingsPlugin` extraction? Either D2 lands first (functions stay in current modules, D7 wraps them in plugins later) or D2 + D7 land together (D2 emits `EditorPlugin` as part of its refactor). Sequencing should be in `03-architecture.md`. The audit's `01-context.md §Q3` says D7 is *last*; D2's architect should propose plugins so D7 only has to *wire* them, not *create* them.
- **MEDIUM-5.q (trivial)**: confirm `EditorState::tool_from_u32` truly has zero callers — re-run grep at architecture time in case a future PR added one. If still dead, delete.
