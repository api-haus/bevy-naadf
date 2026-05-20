# D2 — editor-and-settings-ui — architecture

**Author:** refactor-architect (D2).
**Date:** 2026-05-20.
**Scope:** target structure + ordered migration steps for the 4 HIGH + 4 MEDIUM + 2 LOW findings in `02-exploration.md`. Coordinates with D1 (UA-1 `VoxelEdit` type), D4 (SSoT-1 GPU consumer), D7 (SSoT-1 canonical `GiSettings` location + `AppMode`/`EditorPlugin`/`SettingsPlugin` extraction).

Every file:line cited below verified via Read against current `main`. No invented citations.

---

## 1. Findings addressed

The brief instructed the architect to cover all 10 findings in `02-exploration.md`. This design covers:

- **HIGH-1** (DUP-2 brush triple-loops) — Section 2.1 + Migration Step 4.
- **HIGH-2** (style-bundle boilerplate × 12+) — Section 2.2 + Migration Step 2.
- **HIGH-3** (KnobKind hand-rolled reflection / BEV-4 / OA-1) — Section 2.3 + Migration Step 5. **Decl-macro chosen over `Reflect`** — see "KNOBS-table cutover decision" §3.
- **HIGH-4** (SSoT-1 ray-step default literals) — Section 2.4 + Migration Step 5. Coordinated with D7 (canonical `GiSettings::DEFAULT` const) + D4 (uniform consumer).
- **MEDIUM-5** (`EditorState::tool_from_u32` dead code) — Section 2.5 + Migration Step 1.
- **MEDIUM-6** (`let _ = DEFAULT_TAA_RING_DEPTH` phantom-use) — Section 2.6 + Migration Step 1.
- **MEDIUM-7** (3× dispatch of knob math) — collapses naturally as a consequence of Step 5 — Section 2.7.
- **MEDIUM-8** (Erase/Continuous parallel match blocks) — Section 2.8 + Migration Step 3.
- **LOW-9** (color-const duplication across files) — Section 2.9 + Migration Step 2.
- **LOW-10** (5-Query `handle_hud_clicks`) — **deferred to side-notes** — see §6. Rationale: splitting one Update system into five buys borrow-graph parallelism but bumps registration churn in the new `EditorPlugin` for no observable speedup at this scale. Architect call per `02-exploration.md` LOW-10 open question.

Side-note 11 (`EditorPlugin` / `SettingsPlugin` / `AppModePlugin` extraction) — **landed by D2 itself in Step 6**. D7's later refactor only deletes the inline registration block at `lib.rs:900-971`.

---

## 2. Target-state architecture

### 2.1 Finding HIGH-1: Brush trait + skeleton loop

**Current shape (verified):**

Two byte-equivalent-modulo-classifier triple-loops at `crates/bevy_naadf/src/editor/tools.rs:168-224` (`cube_brush`) and `:231-287` (`sphere_brush`). Each has the structure:

```rust
pub fn {cube|sphere}_brush(world_data, pos, radius, ty, is_erase) {
    if radius <= 0.0 { return; }
    let target = if is_erase { EMPTY } else { ty };
    let target_opt = if is_erase { None } else { Some(ty) };
    let (min_chunk, max_chunk) = brush_chunk_aabb(world_data, pos, radius);
    let mut inside_chunks: Vec<([u32; 3], Option<VoxelTypeId>)> = ...;
    let mut mixed_chunk_edits: Vec<(IVec3, VoxelTypeId)> = ...;
    for cz/cy/cx in chunk-AABB {
        match {cube|sphere}_chunk_classify(...) {
            Outside => continue,
            Inside => inside_chunks.push(([cx,cy,cz] as u32, target_opt)),
            Mixed => triple-loop CHUNK_VOXELS{
                if {cheb < radius | d.length_squared() < r²} {
                    mixed_chunk_edits.push((voxel, target));
                }
            }
        }
    }
    if !inside_chunks.is_empty() { world_data.set_chunks_uniform_batch(&inside_chunks); }
    if !mixed_chunk_edits.is_empty() { world_data.set_voxels_batch(&mixed_chunk_edits); }
}
```

`paint_brush` (`tools.rs:134-162`) is structurally different (no inside fast-path, gates on `get_voxel_type` non-empty); does **not** join the trait.

Classifiers `cube_chunk_classify` (`tools.rs:108-121`) + `sphere_chunk_classify` (`tools.rs:88-103`) are the only meaningful per-shape divergence.

**Target shape:**

A private trait + skeleton fn in `editor/tools.rs`, plus the three e2e-pinned wrapper `pub fn`s as `#[inline]` thin adapters. **Names `paint_brush` / `cube_brush` / `sphere_brush` preserved verbatim** — those are the e2e-pinned symbols at `e2e/oasis_edit_visual.rs:268`, `e2e/small_edit_repro.rs:231`, `e2e/small_edit_visual.rs:349`.

```rust
// editor/tools.rs (new internal API).

/// Classification of a chunk relative to a brush volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChunkClass { Inside, Mixed, Outside }

/// Solid-fill brush shape — the inside/mixed/outside chunk-classify + per-voxel
/// test trio that `cube_brush` and `sphere_brush` share. Paint does NOT
/// implement this trait (no inside fast-path; gates on non-empty voxels).
trait SolidBrushShape {
    /// Classify a chunk's relation to the brush volume.
    fn classify_chunk(&self, chunk_pos: IVec3, pos: Vec3, radius: f32) -> ChunkClass;
    /// Per-voxel inclusion test (mixed-chunk inner loop). `voxel_world_pos`
    /// is the integer voxel position; the brush adds `+0.5` to centre it.
    fn voxel_inside(&self, voxel_world_pos: IVec3, pos: Vec3, radius: f32) -> bool;
}

/// Cube brush — Chebyshev distance `< r`.
struct CubeShape;
impl SolidBrushShape for CubeShape {
    fn classify_chunk(&self, c: IVec3, p: Vec3, r: f32) -> ChunkClass { /* cube_chunk_classify */ }
    fn voxel_inside(&self, v: IVec3, p: Vec3, r: f32) -> bool {
        let d = (v.as_vec3() + Vec3::splat(0.5)) - p;
        d.x.abs().max(d.y.abs()).max(d.z.abs()) < r
    }
}

/// Sphere brush — Euclidean distance `< r`.
struct SphereShape;
impl SolidBrushShape for SphereShape {
    fn classify_chunk(&self, c: IVec3, p: Vec3, r: f32) -> ChunkClass { /* sphere_chunk_classify */ }
    fn voxel_inside(&self, v: IVec3, p: Vec3, r: f32) -> bool {
        let d = (v.as_vec3() + Vec3::splat(0.5)) - p;
        d.length_squared() < r * r
    }
}

/// Drive a solid-fill brush: classify chunks, bulk-fill inside-chunks,
/// per-voxel test mixed-chunks. Owns the iteration skeleton.
fn apply_solid_brush<S: SolidBrushShape>(
    shape: &S,
    world_data: &mut WorldData,
    pos: Vec3,
    radius: f32,
    ty: VoxelTypeId,
    is_erase: bool,
) {
    if radius <= 0.0 { return; }
    let target = if is_erase { VoxelTypeId::EMPTY } else { ty };
    let target_opt = if is_erase { None } else { Some(ty) };
    let (min_chunk, max_chunk) = brush_chunk_aabb(world_data, pos, radius);

    let mut inside_chunks: Vec<ChunkEdit> = Vec::new();    // D1's named type — see §5
    let mut mixed_chunk_edits: Vec<VoxelEdit> = Vec::new(); // D1's named type — see §5

    for cz in min_chunk.z..=max_chunk.z {
        for cy in min_chunk.y..=max_chunk.y {
            for cx in min_chunk.x..=max_chunk.x {
                let chunk_pos = IVec3::new(cx, cy, cz);
                match shape.classify_chunk(chunk_pos, pos, radius) {
                    ChunkClass::Outside => continue,
                    ChunkClass::Inside => inside_chunks.push(ChunkEdit {
                        pos: UVec3::new(cx as u32, cy as u32, cz as u32),
                        ty: target_opt,
                    }),
                    ChunkClass::Mixed => {
                        let chunk_origin = chunk_pos * CHUNK_VOXELS;
                        for lz in 0..CHUNK_VOXELS {
                            for ly in 0..CHUNK_VOXELS {
                                for lx in 0..CHUNK_VOXELS {
                                    let voxel = chunk_origin + IVec3::new(lx, ly, lz);
                                    if shape.voxel_inside(voxel, pos, radius) {
                                        mixed_chunk_edits.push(VoxelEdit { pos: voxel, ty: target });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if !inside_chunks.is_empty() { world_data.set_chunks_uniform_batch(&inside_chunks); }
    if !mixed_chunk_edits.is_empty() { world_data.set_voxels_batch(&mixed_chunk_edits); }
}

/// Cube brush — Chebyshev `< r`. Faithful port of `EditingToolCube.cs:76-90`
/// with the chunk inside/mixed split (`02c` Part 3).
#[inline]
pub fn cube_brush(world_data: &mut WorldData, pos: Vec3, radius: f32, ty: VoxelTypeId, is_erase: bool) {
    apply_solid_brush(&CubeShape, world_data, pos, radius, ty, is_erase);
}

/// Sphere brush — Euclidean `< r`. Faithful port of `EditingToolSphere.cs:76-89`
/// with the chunk inside/mixed split (`02c` Part 3).
#[inline]
pub fn sphere_brush(world_data: &mut WorldData, pos: Vec3, radius: f32, ty: VoxelTypeId, is_erase: bool) {
    apply_solid_brush(&SphereShape, world_data, pos, radius, ty, is_erase);
}

/// Paint brush — unchanged from current (`tools.rs:134-162`). Does NOT use
/// the SolidBrushShape trait (no inside fast-path; gates on non-empty).
pub fn paint_brush(world_data: &mut WorldData, pos: Vec3, radius: f32, ty: VoxelTypeId) { /* as today */ }
```

**Reuse choices:**
- **D1's `VoxelEdit { pos: IVec3, ty: VoxelTypeId }` named type** consumed at `mixed_chunk_edits.push(VoxelEdit { ... })`. D1's architect owns the type definition per UA-1 + Finding 5 of D1's exploration; D2 consumes verbatim. **Hard cross-domain dependency:** D1's impl phase MUST land first (it's already scheduled to per the interleave order). If D1's architect named it differently (e.g. `Edit` or `BulkVoxelEdit`), D2's impl phase reads D1's `03-architecture.md` before Step 4 lands and substitutes the chosen name.
- **D1's `ChunkEdit { pos: UVec3, ty: Option<VoxelTypeId> }`** consumed at `inside_chunks.push(ChunkEdit { ... })`. Same dependency arrow as above. D1's Finding 5 explicitly proposes this struct.
- Trait + private struct units `CubeShape` / `SphereShape` are zero-sized — no allocation cost; `apply_solid_brush::<S>` monomorphizes to two specialized fns, codegen byte-equivalent to the current pair (only the `voxel_inside` body differs). No virtual dispatch.
- `brush_chunk_aabb` / `CHUNK_VOXELS` / `cube_chunk_classify` / `sphere_chunk_classify` already exist (`tools.rs:43,67-103,108-121`) — bodies preserved, callers shift from free-fn to trait method.

**Behavioural delta:**
- **None.** Byte-for-byte output equality — same chunk iteration order, same voxel iteration order, same classifier predicate, same batch shape. Pre-existing tests `cube_brush_radius_one_emits_exactly_one_voxel` (`tools.rs:340-385`), `sphere_brush_produces_solid_sphere` (`:316-333`), `cube_brush_produces_solid_cube` (`:390-406`), `sphere_brush_chunk_inside_path_uses_set_chunks_uniform` (`:472-508`), `sphere_brush_chunk_outside_path_skipped` (`:514-526`) cover both the trait skeleton and the per-shape predicates; they continue to pass without modification.

---

### 2.2 Finding HIGH-2 + LOW-9: Shared UI helpers + semantic palette

**Current shape (verified):**

Repeated bundle-literal blocks across `settings.rs`, `editor/hud.rs`, `hud.rs`:

```rust
TextColor(SOME_COLOR),
TextFont { font: dev_font.0.clone(), font_size: FontSize::Px(N.0), ..default() },
```

at `settings.rs:468-475, 504-510, 530-536`, `editor/hud.rs:280-284, 348-352, 437-443, 451-457, 521-527`, `hud.rs:117-123`. Verified 9 occurrences (the explorer's count is correct).

Color-palette consts spread across two files: `editor/hud.rs:109-124` (16 consts) + `settings.rs:44-56` (12 consts). Several conceptually overlapping (`COL_BTN_BG_HOVER` vs `COL_RESET_BG_HOVER`; `COL_TEXT_PRIMARY` vs `COL_VALUE`; `COL_BTN_BG_SELECTED` vs `COL_ROW_SELECTED`).

**Target shape:**

New module `crates/bevy_naadf/src/editor/ui_theme.rs` (top-level under `editor/` since editor is the heavier consumer; `hud.rs` and `settings.rs` import via `crate::editor::ui_theme::*`). Holds:

```rust
//! Editor UI theme — semantic colour palette + dev-font text bundle constructor.
//!
//! Consolidates the COL_* consts previously file-local to `settings.rs` and
//! `editor/hud.rs`, plus the inline `Text + TextColor + TextFont` literal
//! repeated 9× across `settings.rs`, `editor/hud.rs`, and `hud.rs`.

use bevy::prelude::*;
use crate::DevFont;

// === Semantic palette ===
// Names describe role, not file-of-origin. File-specific consts are removed.

/// Background fills.
pub const BG_HUD: Color           = Color::srgba(0.05, 0.05, 0.08, 0.82);
pub const BG_PANEL: Color         = Color::srgba(0.06, 0.06, 0.09, 0.96);
pub const BG_BACKDROP: Color      = Color::srgba(0.0, 0.0, 0.0, 0.55);
pub const BG_HEADING: Color       = Color::srgba(0.10, 0.12, 0.18, 1.0);
pub const BG_BUTTON: Color        = Color::srgba(0.10, 0.10, 0.14, 1.0);
pub const BG_BUTTON_HOVER: Color  = Color::srgba(0.18, 0.18, 0.24, 1.0);
pub const BG_BUTTON_SELECTED: Color = Color::srgba(0.95, 0.75, 0.20, 1.0);
pub const BG_BUTTON_DISABLED: Color = Color::srgba(0.08, 0.08, 0.10, 1.0);
pub const BG_RESET: Color         = Color::srgba(0.65, 0.20, 0.20, 1.0);
pub const BG_RESET_HOVER: Color   = Color::srgba(0.85, 0.30, 0.30, 1.0);
pub const BG_ROW_HOVER: Color     = Color::srgba(1.0, 1.0, 1.0, 0.05);
pub const BG_ROW_SELECTED: Color  = Color::srgba(1.0, 0.85, 0.30, 0.18);

/// Borders.
pub const BORDER_PANEL: Color     = Color::srgba(0.35, 0.35, 0.42, 1.0);
pub const BORDER_BUTTON: Color    = Color::srgba(0.30, 0.30, 0.34, 1.0);
pub const BORDER_BUTTON_SELECTED: Color = Color::srgba(1.0, 0.85, 0.30, 1.0);

/// Foreground / text.
pub const FG_PRIMARY: Color       = Color::WHITE;
pub const FG_MUTED: Color         = Color::srgba(0.65, 0.65, 0.70, 1.0);
pub const FG_DISABLED: Color      = Color::srgba(0.35, 0.35, 0.38, 1.0);
pub const FG_SECTION: Color       = Color::srgba(0.55, 0.85, 0.95, 1.0);
pub const FG_VALUE_SELECTED: Color = Color::srgba(1.0, 1.0, 0.6, 1.0);
pub const FG_READONLY: Color      = Color::srgba(0.55, 0.55, 0.55, 1.0);

/// Swatches + sliders.
pub const SWATCH_BORDER: Color    = Color::srgba(0.20, 0.20, 0.24, 1.0);
pub const SWATCH_BORDER_SELECTED: Color = Color::WHITE;
pub const SLIDER_TRACK: Color     = Color::srgba(0.10, 0.10, 0.14, 1.0);
pub const SLIDER_FILL: Color      = Color::srgba(0.40, 0.65, 0.95, 1.0);

/// Scrollbar.
pub const SCROLLBAR_TRACK: Color  = Color::srgba(0.06, 0.06, 0.08, 1.0);
pub const SCROLLBAR_THUMB: Color  = Color::srgba(0.40, 0.40, 0.50, 1.0);

// === Text bundle constructor ===

/// Common dev-font font sizes used by the UI surface. Bare `f32` lets call
/// sites read `text_label(font, FG_PRIMARY, 13.0)` while keeping the type
/// system honest about what unit is in play.
pub type FontSizePx = f32;

/// `(TextFont, TextColor)` bundle for a `Text::new(...)` spawn. Centralizes the
/// 5-line `TextFont { font: dev_font.0.clone(), font_size: FontSize::Px(N), ..default() }`
/// boilerplate currently inlined 9× across `settings.rs`, `editor/hud.rs`,
/// `hud.rs`.
pub fn text_style(dev_font: &DevFont, color: Color, size_px: FontSizePx) -> (TextColor, TextFont) {
    (
        TextColor(color),
        TextFont {
            font: dev_font.0.clone(),
            font_size: FontSize::Px(size_px),
            ..default()
        },
    )
}
```

Call sites change from:

```rust
// before
Text::new("QUALITY SETTINGS"),
TextColor(COL_SECTION),
TextFont {
    font: dev_font.0.clone(),
    font_size: FontSize::Px(16.0),
    ..default()
},
```

to:

```rust
// after
(
    Text::new("QUALITY SETTINGS"),
    text_style(&dev_font, FG_SECTION, 16.0),
)
```

(Spawning a tuple-bundle composed of `(Text, (TextColor, TextFont))` flattens correctly in Bevy 0.19.)

**Reuse choices:**
- New file `editor/ui_theme.rs` is the canonical home. **Rejected** alternative `crates/bevy_naadf/src/ui_theme.rs` (top-level) — `hud.rs` (the FPS overlay) is the only D2 consumer at lib-root, and a single `use crate::editor::ui_theme::*;` is one line. Top-level `ui_theme.rs` would imply other crate consumers; there are none.
- `DevFont` definition stays in `lib.rs:39` (D7 owns; out of scope to move).
- `FontSize` is Bevy 0.19's `bevy::text::FontSize` — same type both files use today.

**Behavioural delta:**
- **None.** Pure structural — same colour values, same font sizes, same spawn-tree shape. The renamed semantic consts (`COL_BTN_BG_HOVER` → `BG_BUTTON_HOVER`) are pure refactor; no value drift.

---

### 2.3 Finding HIGH-3: Decl-macro `knob!{}` over `bevy_reflect` for KNOBS

See **KNOBS-table cutover decision** §3 for the choice + rationale.

**Current shape (verified):**

`KnobKind` enum (`settings.rs:118-150`) is a 5-variant tagged union; non-trivial variants (`U32`, `F32`, `Bool`) carry a `getter: fn(&GiSettings) -> T` + `setter: fn(&mut GiSettings, T)` + `nudge` / `big_step` / `min` / `max` / `default`. `Action` carries `apply: fn(&mut AppArgs)`, `Readonly` carries `value: fn(&AppArgs) -> String`. The `KNOBS: &[Knob]` table at `settings.rs:166-378` is **213 lines** of 30 row literals. Eight match-on-`KnobKind` sites: `reset_all_knobs` (`:382-389`), `first_interactive` (`:392-394`), `step_cursor` (`:846-849`), `setup_settings` (`:491`), `show_settings` (`:557`), `mouse_interact_settings` (`:664`), `apply_drag_delta` (`:726-749`), `handle_click_release` (`:754-761`), `update_settings_text` (`:779-805,810`).

The `class: char` field (`'P'`/`'C'`/`'B'`/`'D'`/`' '`) is a magic-character tag used only for the `[P]`/`[C]` suffix in `update_settings_text:785,789,794,798`.

**Target shape:**

Decl-macro `knob_u32!` / `knob_f32!` / `knob_bool!` / `knob_readonly!` / `knob_action!` / `knob_section!` that expand to the current row literals, with the `getter` / `setter` closures **derived from a single field-ident** so a typo is caught at compile time (the compile-time-field-exists property explorer HIGH-3.q is concerned about). The `default` field is sourced from D7's new `GiSettings::DEFAULT` const (see §4 SSoT-1 coordination) — eliminates the literal-duplication of HIGH-4.

The `class: char` field is **deleted**. The `[P]`/`[C]`/`[D]` suffix in the panel readout was unloaded ornament; section headers already group rows by category.

```rust
// editor/settings.rs (top of file, near KnobKind).

/// Knob descriptor — one row of the settings table.
struct Knob {
    label: &'static str,
    kind: KnobKind,
}

#[allow(clippy::type_complexity)]
enum KnobKind {
    Section,
    U32 {
        getter: fn(&GiSettings) -> u32,
        setter: fn(&mut GiSettings, u32),
        nudge: u32, big_step: u32, min: u32, max: u32, default: u32,
    },
    F32 {
        getter: fn(&GiSettings) -> f32,
        setter: fn(&mut GiSettings, f32),
        nudge: f32, big_step: f32, min: f32, max: f32, default: f32,
    },
    Bool {
        getter: fn(&GiSettings) -> bool,
        setter: fn(&mut GiSettings, bool),
        default: bool,
    },
    Readonly { value: fn(&AppArgs) -> String },
    Action { apply: fn(&mut AppArgs) },
}

/// Section header (non-interactive row).
macro_rules! knob_section {
    ($label:literal) => {
        Knob { label: $label, kind: KnobKind::Section }
    };
}

/// U32 knob. `$field` is a single ident; the getter/setter closures reference
/// `GiSettings::$field` directly — a typo here is a compile error, preserving
/// the "compile-time field-exists" property of the hand-written table.
/// `default` is read from `GiSettings::DEFAULT.$field` — eliminates HIGH-4's
/// literal-duplication (D7 supplies `DEFAULT`).
macro_rules! knob_u32 {
    ($label:literal, $field:ident, nudge=$n:expr, big=$b:expr, min=$mn:expr, max=$mx:expr) => {
        Knob {
            label: $label,
            kind: KnobKind::U32 {
                getter: |g| g.$field,
                setter: |g, v| g.$field = v,
                nudge: $n, big_step: $b, min: $mn, max: $mx,
                default: GiSettings::DEFAULT.$field,
            },
        }
    };
}

macro_rules! knob_f32 {
    ($label:literal, $field:ident, nudge=$n:expr, big=$b:expr, min=$mn:expr, max=$mx:expr) => {
        Knob {
            label: $label,
            kind: KnobKind::F32 {
                getter: |g| g.$field,
                setter: |g, v| g.$field = v,
                nudge: $n, big_step: $b, min: $mn, max: $mx,
                default: GiSettings::DEFAULT.$field,
            },
        }
    };
}

macro_rules! knob_bool {
    ($label:literal, $field:ident) => {
        Knob {
            label: $label,
            kind: KnobKind::Bool {
                getter: |g| g.$field,
                setter: |g, v| g.$field = v,
                default: GiSettings::DEFAULT.$field,
            },
        }
    };
}

macro_rules! knob_readonly {
    ($label:literal, $expr:expr) => {
        Knob { label: $label, kind: KnobKind::Readonly { value: $expr } }
    };
}

macro_rules! knob_action {
    ($label:literal, $fn:expr) => {
        Knob { label: $label, kind: KnobKind::Action { apply: $fn } }
    };
}

const KNOBS: &[Knob] = &[
    knob_section!("RAY STEP CAPS"),
    knob_u32!("  primary",        max_ray_steps_primary,        nudge=8, big=32, min=1, max=512),
    knob_u32!("  secondary",      max_ray_steps_secondary,      nudge=8, big=32, min=1, max=512),
    knob_u32!("  sun",            max_ray_steps_sun,            nudge=8, big=32, min=1, max=512),
    knob_u32!("  sun-secondary",  max_ray_steps_sun_secondary,  nudge=8, big=32, min=1, max=512),
    knob_u32!("  visibility",     max_ray_steps_visibility,     nudge=8, big=32, min=1, max=512),

    knob_section!("SPATIAL RESAMPLING"),
    knob_u32!("  iter count",       spatial_iter_count,            nudge=1,    big=4,     min=1,    max=32),
    knob_u32!("  sun_shadow_taps",  sun_shadow_taps,               nudge=1,    big=4,     min=1,    max=32),
    knob_f32!("  resample_size",    spatial_resample_size,         nudge=50.0, big=200.0, min=32.0, max=2000.0),
    knob_f32!("  radius_lit_factor",radius_lit_factor,             nudge=0.5,  big=3.0,   min=0.0,  max=1000.0),
    knob_f32!("  noise_suppress",   noise_suppression_factor,      nudge=0.05, big=0.5,   min=0.01, max=100.0),

    knob_section!("GI"),
    knob_u32!("  bounce_count",    bounce_count,    nudge=1,    big=1,     min=1,   max=3),
    knob_f32!("  denoise_thresh",  denoise_thresh,  nudge=50.0, big=200.0, min=0.0, max=2000.0),
    knob_bool!("  is_denoise",          is_denoise),
    knob_bool!("  is_sample_leveling",  is_sample_leveling),
    knob_bool!("  is_varying_radius",   is_varying_resampling_radius),
    knob_bool!("  is_atmosphere_int",   is_atmosphere_interaction),
    knob_bool!("  skip_samples",        skip_samples),

    knob_section!("DIAGNOSTICS (read-only)"),
    knob_readonly!("  taa_ring_depth",         |a| format!("{} [restart-required]", a.taa_ring_depth)),
    knob_readonly!("  camera_history_depth",   |_| format!("{} [const]", CAMERA_HISTORY_DEPTH)),
    knob_readonly!("  valid_sample_storage",   |_| format!("{} [storage-tied]", VALID_SAMPLE_STORAGE_COUNT)),
    knob_readonly!("  invalid_sample_storage", |_| format!("{} [storage-tied]", INVALID_SAMPLE_STORAGE_COUNT)),
    knob_readonly!("  bucket_storage",         |_| format!("{} [storage-tied]", BUCKET_STORAGE_COUNT)),
    knob_readonly!("  refined_bucket",         |_| format!("{} [storage-tied]", REFINED_BUCKET_STORAGE_COUNT)),
    knob_readonly!("  global_illum_max_accum", |a| format!("{} [const]", a.gi.global_illum_max_accum)),

    knob_action!("> RESET ALL TO DEFAULTS <", reset_all_knobs),
];
```

213 lines collapse to ~32 lines of table — the table reads as the spec.

**Reuse choices:**
- **`bevy_reflect` rejected.** See §3.
- Decl-macros are file-local (no `pub` / `#[macro_export]`) — they exist solely to format the KNOBS table.
- `GiSettings::DEFAULT` const (D7's deliverable) replaces every `default: 120` literal. Compile-time-evaluated. D7's architect doc §SSoT-1 coordination already commits to producing this — see §4 below.
- All 8 match-on-`KnobKind` consumer sites stay structurally unchanged — they keep matching on the same enum variants. They just no longer touch `class: char`.

**Behavioural delta:**
- **Observable in the UI:** the `[P]` / `[C]` / `[D]` / `[B]` suffix in each row's readout text disappears. The text format goes from `"> primary                 120 [P]"` to `"> primary                 120"`. **This is a faithful-port-rule edge case:** bevy_ui-vs-ImGui divergence is pre-approved per `01-context.md`, but the suffix is a port-specific addition (no C# counterpart) — its deletion is a port-internal cleanup, not a divergence. Section headers (`"RAY STEP CAPS"`, `"GI"`, etc.) already group rows by category; the per-row tag is redundant.
- Existing tests `defaults_match_gi_settings_default` (`settings.rs:875-892`) and `promoted_defaults_match_canonical_consts` (`settings.rs:897-905`) continue to pass — the macros source `default` from `GiSettings::DEFAULT` which by construction equals `GiSettings::default()` (the `Default` impl returns `Self::DEFAULT` after D7's change — see §4).
- `update_settings_text` (`settings.rs:766-836`) loses 4 lines (the `class` formatting) but no other system body changes structurally.

---

### 2.4 Finding HIGH-4: SSoT-1 ray-step defaults via `GiSettings::DEFAULT`

**Current shape (verified):**

5 ray-step caps + spatial_iter_count appear as integer literals in:

- `lib.rs:223-228` — `GiSettings::default()` body.
- `settings.rs:174,184,194,202,210,220` — KNOBS row `default:` fields (verified: 120 / 100 / 120 / 80 / 60 / 12).
- `assets/shaders/ray_tracing.wgsl:122-126` — `MAX_RAY_STEPS_*` WGSL consts (documentation-only per D4 §SSoT-1, but read by the human as authoritative).

Two unit tests pin the Rust-side agreement: `defaults_match_gi_settings_default` (`settings.rs:875-892`) and `promoted_defaults_match_canonical_consts` (`settings.rs:897-905`).

**Target shape:**

D7 introduces `impl GiSettings { pub const DEFAULT: GiSettings = GiSettings { ... }; }` and `impl Default for GiSettings { fn default() -> Self { Self::DEFAULT } }`. This is **D7 territory** — `GiSettings` lives at `lib.rs:108-185`. D2's design depends on this; D2's architect formally requests it (the request is mirrored in D7's own exploration F2 `## SSoT coordination notes`, so D7 has already designed for it).

Within D2: every `default: 120` literal in the KNOBS row is removed by the macro expansion — `default: GiSettings::DEFAULT.$field` is the only site. D2 owns no copy of the literal after Step 5.

D4's WGSL consumer side is per D4 exploration §SSoT-1: `ray_tracing.wgsl:122-126` are documentation-only; the live SSoT is `GpuRenderParams.max_ray_steps_*`. D4's architect may delete those WGSL consts entirely; that is D4's call, not D2's.

**Reuse choices:**
- `GiSettings::DEFAULT` (D7-supplied) — the new canonical site for the 5+1 numbers.
- D2's KNOBS macro inlines the const-load at expansion time; no runtime cost.
- The unit test `promoted_defaults_match_canonical_consts` (`settings.rs:897-905`) becomes redundant after Step 5 (the KNOBS table reads from `DEFAULT` by construction) but is kept as a sanity assertion against the C# canonical values (120, 100, 120, 80, 60, 12) — that's the only check still needed once the duplication is gone.

**Behavioural delta:** none.

---

### 2.5 Finding MEDIUM-5: Delete dead `tool_from_u32`

**Current shape (verified):**

`editor/mod.rs:107-114` defines `pub fn EditorState::tool_from_u32(v: u32) -> EditTool`. Test at `editor/mod.rs:245-251` is the sole caller (verified by grepping the workspace — no production / e2e / bin reference).

**Target shape:** function deleted, test deleted.

**Reuse choices:** none — `EditTool` already has `#[repr(u32)]` (`mod.rs:42`); future use of `tool as u32` works directly without `tool_from_u32`.

**Behavioural delta:** none (function had no callers).

---

### 2.6 Finding MEDIUM-6: Delete phantom `DEFAULT_TAA_RING_DEPTH` use

**Current shape (verified):**

`settings.rs:32` imports `DEFAULT_TAA_RING_DEPTH`; `settings.rs:543` has `let _ = DEFAULT_TAA_RING_DEPTH;` to suppress an unused-import warning. The diagnostics row at `settings.rs:328` reads `a.taa_ring_depth` (the live `AppArgs` field), not the const.

**Target shape:**

Remove `DEFAULT_TAA_RING_DEPTH` from the `use crate::{...}` at `:32`. Remove the `let _ = DEFAULT_TAA_RING_DEPTH;` line at `:543`.

**Reuse choices:** none — the const stays defined in `lib.rs:274` (consumed by `render/taa.rs:36`, `render/mod.rs:111`, `lib.rs:446`).

**Behavioural delta:** none.

---

### 2.7 Finding MEDIUM-7: Knob dispatch math (collapsed via Step 5)

**Current shape (verified):**

Three sites match on `KnobKind` and apply value writes: `adjust_settings` (`settings.rs:610-645`), `apply_drag_delta` (`:721-749`), `handle_click_release` (`:752-761`). Each carries its own clamp / saturation idiom.

**Target shape:**

After Step 5, the macros' `getter`/`setter`/`min`/`max` extraction is centralized — each call site still matches on `KnobKind` but reads from a uniform field shape. The three clamp idioms can converge as a follow-up:

```rust
impl KnobKind {
    fn apply_delta_u32(&self, gi: &mut GiSettings, delta: i64) {
        if let KnobKind::U32 { getter, setter, min, max, .. } = *self {
            let cur = getter(gi) as i64;
            let new = (cur + delta).clamp(min as i64, max as i64) as u32;
            setter(gi, new);
        }
    }
    fn apply_delta_f32(&self, gi: &mut GiSettings, delta: f32) { ... }
    fn toggle_bool(&self, gi: &mut GiSettings) { ... }
    fn reset_to_default(&self, args: &mut AppArgs) { ... }
}
```

The architect designed but **deferred** as a third-tier polish. The three call sites are not byte-equivalent today (the keyboard uses `saturating_add/sub`, drag uses i64-widened arithmetic, click uses simple toggle) so collapsing them introduces a subtle behavioural change in saturation semantics. **Out of scope for D2's current refactor — flagged for a follow-up** unless implementor judges otherwise after Step 5 lands. Not on the migration step list.

**Reuse choices:** none — design notes for the follow-up.

**Behavioural delta:** none (deferred).

---

### 2.8 Finding MEDIUM-8: Toggle-style helper

**Current shape (verified):**

Two near-identical 18-line blocks in `update_editor_hud`: erase loop at `editor/hud.rs:839-857`, continuous loop at `:859-877`. Each computes a `(target_bg, target_border, text_color)` triple from `(erase_affects, state.is_{erase|continuous}, hovered)`.

**Target shape:**

Free fn `toggle_button_style` in `editor/hud.rs` (private — only consumed by the two blocks):

```rust
/// Compute the (bg, border, text) colour triple for a toggle button given its
/// affordance state. Used by both the Erase and Continuous toggle update
/// loops in `update_editor_hud` (previously open-coded × 2 with drift risk).
fn toggle_button_style(is_on: bool, hovered: bool, disabled: bool) -> (Color, Color, Color) {
    use crate::editor::ui_theme::*;
    if disabled {
        (BG_BUTTON_DISABLED, BORDER_BUTTON, FG_DISABLED)
    } else if is_on {
        (BG_BUTTON_SELECTED, BORDER_BUTTON_SELECTED, FG_PRIMARY)
    } else if hovered {
        (BG_BUTTON_HOVER, BORDER_BUTTON, FG_PRIMARY)
    } else {
        (BG_BUTTON, BORDER_BUTTON, FG_PRIMARY)
    }
}
```

Each loop body collapses from 18 lines to ~9:

```rust
for (interaction, mut bg, mut border, children) in &mut erase_buttons {
    let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
    let (bg_c, border_c, text_c) = toggle_button_style(state.is_erase, hovered, !erase_affects);
    *bg = BackgroundColor(bg_c);
    *border = BorderColor::all(border_c);
    for &child in children {
        if let Ok(mut tc) = text_colors.get_mut(child) { *tc = TextColor(text_c); }
    }
}
```

**Reuse choices:**
- Uses §2.2's semantic palette (`BG_BUTTON_DISABLED` etc.) — depends on Step 2 landing first.
- Private to `editor/hud.rs` — no `pub`, no external consumer.

**Behavioural delta:** none.

---

### 2.9 Finding LOW-9: Color-palette unification

Covered by §2.2's `editor/ui_theme.rs` consolidation — both `settings.rs::COL_*` and `editor/hud.rs::COL_*` consts move into one semantic palette. **No name re-mappings carry semantic drift**:

| old (`editor/hud.rs`) | new (`ui_theme`) | old (`settings.rs`) | new (`ui_theme`) |
|---|---|---|---|
| `COL_HUD_BG` | `BG_HUD` | `COL_BACKDROP` | `BG_BACKDROP` |
| `COL_BTN_BG` | `BG_BUTTON` | `COL_PANEL_BG` | `BG_PANEL` |
| `COL_BTN_BG_HOVER` | `BG_BUTTON_HOVER` | `COL_PANEL_BORDER` | `BORDER_PANEL` |
| `COL_BTN_BG_SELECTED` | `BG_BUTTON_SELECTED` | `COL_HEADING_BG` | `BG_HEADING` |
| `COL_BTN_BG_DISABLED` | `BG_BUTTON_DISABLED` | `COL_SECTION` | `FG_SECTION` |
| `COL_BTN_BORDER` | `BORDER_BUTTON` | `COL_ROW_HOVER` | `BG_ROW_HOVER` |
| `COL_BTN_BORDER_SELECTED` | `BORDER_BUTTON_SELECTED` | `COL_ROW_SELECTED` | `BG_ROW_SELECTED` |
| `COL_TEXT_PRIMARY` | `FG_PRIMARY` | `COL_VALUE` | `FG_PRIMARY` (same value `Color::WHITE`) |
| `COL_TEXT_MUTED` | `FG_MUTED` | `COL_VALUE_SEL` | `FG_VALUE_SELECTED` |
| `COL_TEXT_DISABLED` | `FG_DISABLED` | `COL_READONLY` | `FG_READONLY` |
| `COL_SLIDER_TRACK` | `SLIDER_TRACK` | `COL_RESET_BG` | `BG_RESET` |
| `COL_SLIDER_FILL` | `SLIDER_FILL` | `COL_RESET_BG_HOVER` | `BG_RESET_HOVER` |
| `COL_SWATCH_BORDER` | `SWATCH_BORDER` | | |
| `COL_SWATCH_BORDER_SELECTED` | `SWATCH_BORDER_SELECTED` | | |
| `COL_SCROLLBAR_TRACK` | `SCROLLBAR_TRACK` | | |
| `COL_SCROLLBAR_THUMB` | `SCROLLBAR_THUMB` | | |

The collapsed pair (`COL_TEXT_PRIMARY` + `COL_VALUE` → `FG_PRIMARY`) is verified byte-equal (both = `Color::WHITE`). All other renames carry distinct values → distinct semantic names.

---

### 2.10 Side-note 11: `EditorPlugin` / `SettingsPlugin` / `AppModePlugin`

**Current shape (verified):**

`lib.rs:900-971` registers 3 resources, 1 state, 15 systems for the editor + settings + app_mode subsystems in one inline `if cfg.add_hud { ... }` block. D7's exploration F1 calls this out as monolithic; D2's side-note 11 proposes D2 land the plugins so D7's later refactor only deletes inline registration.

**Target shape:**

Three plugins, one per concern:

```rust
// editor/mod.rs (new at bottom of file).
pub struct EditorPlugin;
impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        use bevy::state::condition::in_state;
        app.init_resource::<EditorState>()
           .add_systems(Startup, hud::setup_editor_hud.after(crate::load_dev_font))
           .add_systems(Update, (
                hud::refresh_palette_swatches,
                hud::handle_hud_clicks,
                hud::scroll_palette_with_wheel,
                hud::drag_palette_scrollbar,
                hud::update_palette_scrollbar,
                hud::update_editor_hud,
                apply_edit_tool.run_if(in_state(crate::app_mode::AppMode::Playing)),
           ).chain());
    }
}
```

```rust
// settings.rs (new at bottom).
pub struct SettingsPlugin;
impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        use crate::app_mode::AppMode;
        use bevy::state::condition::in_state;
        use bevy::state::state::{OnEnter, OnExit};
        app.init_resource::<SettingsState>()
           .init_resource::<SettingsDrag>()
           .add_systems(
                Startup,
                setup_settings
                    .after(crate::load_dev_font)
                    .after(crate::editor::hud::setup_editor_hud),
            )
           .add_systems(OnEnter(AppMode::Settings),
                (show_settings, crate::app_mode::suspend_camera_input))
           .add_systems(OnExit(AppMode::Settings),
                (hide_settings, crate::app_mode::restore_camera_input))
           .add_systems(Update, (
                adjust_settings.run_if(in_state(AppMode::Settings)),
                mouse_interact_settings.run_if(in_state(AppMode::Settings)),
                update_settings_text.run_if(in_state(AppMode::Settings)),
           ).chain());
    }
}
```

```rust
// app_mode.rs (new at bottom).
pub struct AppModePlugin;
impl Plugin for AppModePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppMode>()
           .add_systems(Update, toggle_settings_on_escape);
    }
}
```

The FPS HUD (`hud.rs`) stays a thin pair `setup_hud` + `update_hud`. D2's brief assigns `hud.rs` to D2 but D2 does **not** wrap it in a plugin in this phase — D7 owns the `HudPlugin` decision per side-note 11 in D7's own exploration. D2 leaves `hud::setup_hud` + `hud::update_hud` callable as before.

**Reuse choices:**
- The 3 plugins above own their resource init + system registration + state init. Each is added independently in `lib.rs` after Step 6.
- The chain ordering inside each plugin preserves the original `.chain()` semantics for systems that need them. The 9-system chain at `lib.rs:937-960` splits across `EditorPlugin` (7 systems chained) + `SettingsPlugin` (3 systems chained) — the chains never crossed plugins (Esc toggle ran first, but it ran on its own; the rest of the chain was within-subsystem).

**Behavioural delta:**
- `app_mode::toggle_settings_on_escape` previously ran with `.chain()` first in the 9-system chain. After split it runs in `AppModePlugin`'s Update without an explicit ordering edge. **This matters:** if the Esc-toggle fires after `editor::apply_edit_tool` in the same frame, the brush sees the old state but the panel shows immediately on the next frame. Verified: the previous chain placed Esc-toggle BEFORE `apply_edit_tool` so the brush-input gate saw the new state on the same frame.
- **Mitigation:** within `EditorPlugin`, add `.after(crate::app_mode::toggle_settings_on_escape)` to `apply_edit_tool`'s `.run_if(in_state(Playing))` registration. This preserves the same-frame transition observation. The implementor must add this edge — it's pinned in Step 6 below.

---

## 3. KNOBS-table cutover decision + rationale

**Chosen:** **Option (c) — decl-macro `knob_*!`**. Reject `Reflect` (option a) and `#[knob]` proc-macro (option b); reject "keep KNOBS explicit" (option d).

**Why decl-macro over `Reflect`:**

1. **`bevy_reflect` is not used anywhere else in the codebase.** Grepped `derive(Reflect` / `#[reflect` — zero hits across all 73 Rust files. Introducing it for one struct creates an oddity. Decl-macros are vanilla Rust.
2. **Compile-time field-existence preserved.** `knob_u32!("foo", max_ray_steps_primary, ...)` expands to `getter: |g| g.max_ray_steps_primary` — a typo (`max_ray_steps_primry`) is a compile error. `Reflect`-driven lookup by name (`Reflect::get_field("max_ray_steps_primary")`) fails at runtime, caught only by the `defaults_match_gi_settings_default` test. Compile-time check is strictly stronger — and it's the explicit user concern in HIGH-3.q.
3. **Default-literal duplication killed.** The macro sources `default: GiSettings::DEFAULT.$field` (a const) — the 6 ray-step / iter-count literals at `settings.rs:174,184,194,202,210,220` (HIGH-4) disappear without an extra step. `Reflect`-with-`#[reflect(default=...)]`-attrs would carry the literals on the struct definition instead of on the KNOBS rows — no improvement.
4. **No proc-macro infra.** A `#[derive(Knob)]` proc-macro (option b) would need its own crate, build-script setup, and a parser for the metadata attributes (`#[knob(nudge=8, big=32)]`). Decl-macros need none of that — Rust's `macro_rules!` is already in the compiler.
5. **Reads as a spec.** The post-macro `KNOBS` table at `settings.rs:166-378` collapses from 213 lines to ~32 lines and reads as the design document (one row = one knob). The current per-row 7-field block is what HIGH-3 is fundamentally complaining about; decl-macro fixes it without machinery.
6. **The `class: char` field deletion is independent of choice (a)/(c).** Chose to delete it (no semantic loss — section headers carry the category).

**Rejected (a) `Reflect`:**
- Adds a feature/crate that nothing else in the codebase uses.
- Trades compile-time field check for runtime.
- Bevy 0.19's `Reflect::get_field` returns `Option<&dyn Reflect>` — every consumer site needs downcast logic (`.downcast_ref::<u32>()`). That's more code, not less.
- Per-knob metadata (nudge / big_step / min / max) would have to ride on `#[reflect(...)]` attrs — Bevy's reflect-attribute parser doesn't natively support custom metadata; you'd add a parallel attribute mechanism (e.g. a `KnobMetadata` table keyed by field-name string). That parallel table is exactly the data-driven mechanism the user wanted to escape.

**Rejected (b) `#[knob]` proc-macro:**
- Same metadata-storage problem as (a) — you'd be hand-rolling reflection anyway.
- Adds a build-time crate (`bevy_naadf_knob_macros` or similar) and the proc-macro2 / syn / quote dep stack.
- No win over decl-macro for a 30-row table.

**Rejected (d) "keep KNOBS explicit":**
- The 213-line table IS the smell. The compile-time field-existence property the user prizes is preserved by (c).
- Even the existing in-doc-test (`defaults_match_gi_settings_default` at `settings.rs:875-892`) becomes superfluous after (c)'s default-from-const lands — the agreement is by construction, the test is belt-and-braces.

---

## 4. SSoT-1 coordination with D4 and D7

**Chain:** D7 (canonical `GiSettings::DEFAULT`) → D2 (KNOBS consumes via macro expansion) → D4 (uniform-side mirror).

### D7's deliverable (D2 depends on)

D7's `03-architecture.md` produces:

```rust
// crates/bevy_naadf/src/lib.rs (or new home if D7 moves GiSettings, see D7 exploration F2).
impl GiSettings {
    /// Canonical default settings — the single source of truth for the
    /// `WorldRenderBase.cs:14-25` C# slider defaults + the panel reset target.
    /// Read by `editor/settings.rs::KNOBS` (via the `knob_*!` macros) and by
    /// `render/extract.rs::extract_gi_config` (when the resource is absent).
    pub const DEFAULT: GiSettings = GiSettings {
        bounce_count: 3,
        global_illum_max_accum: 128,
        spatial_resample_size: 500.0,
        spatial_visibility_count: 80,
        denoise_thresh: 400.0,
        radius_lit_factor: 3.0,
        noise_suppression_factor: 0.4,
        skip_samples: true,
        is_denoise: true,
        is_sample_leveling: true,
        is_varying_resampling_radius: true,
        is_atmosphere_interaction: true,
        sun_shadow_taps: 1,
        max_ray_steps_primary: 120,
        max_ray_steps_secondary: 100,
        max_ray_steps_sun: 120,
        max_ray_steps_sun_secondary: 80,
        max_ray_steps_visibility: 60,
        spatial_iter_count: 12,
    };
}

impl Default for GiSettings {
    fn default() -> Self { Self::DEFAULT }
}
```

D7's exploration F2 + §SSoT coordination notes commits to this shape (their text: "Move `GiSettings` ... retain `pub use` re-export from `lib.rs` if needed for source-stability"). The const is `pub` — D2 reads `GiSettings::DEFAULT.$field` at macro expansion time.

If D7's architect ends up moving `GiSettings` into `settings.rs` (D7 F2's primary suggestion), the import side in `settings.rs` changes from `use crate::GiSettings;` to `use super::GiSettings;` and the macro continues to work. Either home is fine for D2.

### D4's mirror (independent — D2 does nothing for D4)

Per D4's exploration §SSoT-1: the WGSL `MAX_RAY_STEPS_*` consts at `ray_tracing.wgsl:122-126` are documentation-only; the live SSoT is `GpuRenderParams.max_ray_steps_*` (`gpu_types.rs:87`) and `GpuGiParams.{max_ray_steps_*}` (`gpu_types.rs:539`). D4's architect may delete the WGSL consts or leave them as documentation — D2's design assumes either outcome (D2's macros only read from `GiSettings::DEFAULT`, never from WGSL).

### D2's responsibility

Step 5 of D2's migration (below) is the consumer landing — KNOBS macros source `default` from `GiSettings::DEFAULT.$field`. D2 holds the dependency arrow on D7; D2's impl phase MUST wait for D7's impl to land the const, OR D2's impl can land its half conditionally on a `#[cfg(feature = "gi-default-const")]` flag and switch over after D7. **Recommended:** D2 impl waits for D7's `GiSettings::DEFAULT` const to be on `main`. The user's chosen sequencing (D7 last) **conflicts** with this dependency arrow.

**Resolution:** D7's `GiSettings::DEFAULT` const is a tiny pre-extraction landing — adding the `pub const DEFAULT` impl block + flipping `Default` to read it is **3 lines of edit** and doesn't require any other D7 work to land. **Proposal:** the orchestrator allows D7's impl phase to pre-land just the `DEFAULT` const before D2's impl runs Step 5. Everything else in D7's domain (lib.rs decomposition, AppArgs split, device_snapshot delete) stays as-scheduled (D7 last). This is a 3-line pre-land, not a re-ordering of D7's scope.

---

## 5. Brush trait extraction — D1 coordination notes

D1 owns the `VoxelEdit` + `ChunkEdit` named types per UA-1 / D1 Finding 5. D2's design consumes them at:

- `apply_solid_brush` body: `Vec<VoxelEdit>` for mixed-chunk edits, `Vec<ChunkEdit>` for inside-chunks.
- `paint_brush`'s `Vec<(IVec3, VoxelTypeId)>` at `tools.rs:139` also migrates to `Vec<VoxelEdit>`. (Not part of HIGH-1 specifically, but part of UA-1.)

**The `WorldData::set_voxels_batch` / `set_chunks_uniform_batch` signature changes are D1's deliverable.** D1's Finding 5 explicitly says: "Introduce `pub struct VoxelEdit { pub pos: IVec3, pub ty: VoxelTypeId }` in `world::data` (or `aadf::edit`)". D1's architect names the types and writes the signatures; D2 consumes.

**D2's Step 4 has a hard dependency on D1 having landed the new signatures.** If D1's impl phase has not landed by D2's Step 4, the implementor either:

1. Waits for D1.
2. Lands Step 4 with the existing anonymous-tuple shape, then files a follow-up to migrate after D1 lands.

**Recommended:** option (1). D1 is scheduled to interleave between D5 and D7 in the user's sequencing; D2 is also in that interleave window. D1 lands first naturally if the orchestrator picks D1 → D2 order, which is consistent with "named types land before consumers".

Names `paint_brush` / `cube_brush` / `sphere_brush` are **inviolate** in D2 — e2e gates at `e2e/oasis_edit_visual.rs:268`, `e2e/small_edit_repro.rs:231`, `e2e/small_edit_visual.rs:349` reference them by exact name. Signatures `(world_data: &mut WorldData, pos: Vec3, radius: f32, ty: VoxelTypeId[, is_erase: bool])` also preserved verbatim — those are the e2e call shapes.

---

## 6. Migration steps

Six steps. Each leaves the codebase buildable + tests passing.

### Step 1 — Delete dead code (MEDIUM-5 + MEDIUM-6)

**Edits:**
- `crates/bevy_naadf/src/editor/mod.rs:106-115` — delete `impl EditorState { pub fn tool_from_u32(...) }`.
- `crates/bevy_naadf/src/editor/mod.rs:244-251` — delete `#[test] fn edit_tool_from_u32_total`.
- `crates/bevy_naadf/src/settings.rs:32` — remove `DEFAULT_TAA_RING_DEPTH` from the `use crate::{...}` list.
- `crates/bevy_naadf/src/settings.rs:543` — delete `let _ = DEFAULT_TAA_RING_DEPTH;`.

**Rationale:** trivial pre-clean removes ~10 LOC and unblocks the `use` reshape in Step 2.

**Post-step state:** dead `tool_from_u32` gone. `settings.rs` import list has one less entry. No phantom binds.

**Verification:** `cargo build --workspace`, `cargo test --workspace --lib` (especially `editor::tests::*` + `settings::tests::*`).

---

### Step 2 — Introduce `editor/ui_theme.rs` + migrate consumers (HIGH-2 + LOW-9)

**Edits:**
- `crates/bevy_naadf/src/editor/mod.rs:27-29` — add `pub mod ui_theme;` to the module list.
- New file `crates/bevy_naadf/src/editor/ui_theme.rs` — semantic palette + `text_style()` per §2.2.
- `crates/bevy_naadf/src/editor/hud.rs:109-124` — delete the 16 file-local `COL_*` consts; add `use crate::editor::ui_theme::*;`.
- `crates/bevy_naadf/src/editor/hud.rs` — replace every old `COL_*` reference with the new semantic name per the §2.9 mapping table. Use `Edit`'s `replace_all` per const.
- `crates/bevy_naadf/src/editor/hud.rs:280-284, 348-352, 437-443, 451-457, 521-527` — replace inline 5-line `TextColor + TextFont` blocks with `text_style(&dev_font, FG_*, N.0)`. Five sites.
- `crates/bevy_naadf/src/settings.rs:32` — add `use crate::editor::ui_theme::*;` (Step 1 already trimmed the unused import).
- `crates/bevy_naadf/src/settings.rs:44-56` — delete the 12 file-local `COL_*` consts.
- `crates/bevy_naadf/src/settings.rs:468-475, 504-510, 530-536` — three inline-bundle sites → `text_style(...)`. Plus replace every old `COL_*` reference.
- `crates/bevy_naadf/src/hud.rs:117-123` — replace inline-bundle with `text_style(&dev_font, ui_theme::FG_PRIMARY, 14.0)`. Add `use crate::editor::ui_theme;`.

**Rationale:** centralizes 28 colour consts + 9 bundle sites into one module. Drops ~50 LOC.

**Post-step state:** `editor/ui_theme.rs` is the sole palette + text-bundle source. Three files (`hud.rs`, `editor/hud.rs`, `settings.rs`) consume; no file-local `COL_*`.

**Verification:** `cargo build --workspace`, `cargo test --workspace --lib`. **User visual check recommended:** since the rename touches every UI colour reference, an `e2e_render -- baseline` framebuffer is unaffected (HUD is `add_hud=false`), but the user's live-run will surface a visual regression if a name mismapping introduced a colour drift. Step 2 is mechanical but high-volume — implementor verifies the rename by diffing the colour value of each old/new pair before committing (the §2.9 table makes this a 28-row check).

---

### Step 3 — `toggle_button_style` helper (MEDIUM-8)

**Edits:**
- `crates/bevy_naadf/src/editor/hud.rs` (after `fn update_palette_scrollbar`, around line 730) — add `fn toggle_button_style(is_on, hovered, disabled) -> (Color, Color, Color)` per §2.8.
- `crates/bevy_naadf/src/editor/hud.rs:839-857` — replace the 18-line erase-loop body with the 9-line shape calling `toggle_button_style(state.is_erase, hovered, !erase_affects)`.
- `crates/bevy_naadf/src/editor/hud.rs:859-877` — replace the 18-line continuous-loop body with `toggle_button_style(state.is_continuous, hovered, !erase_affects)`.

**Rationale:** kills the parallel-match drift between Erase/Continuous loops.

**Post-step state:** one `(Color, Color, Color)` mapping fn; two consumer loops.

**Verification:** `cargo build --workspace`, `cargo test --workspace --lib`. The toggle visual behaviour is not in any e2e gate; user live-check recommended.

---

### Step 4 — Brush trait extraction (HIGH-1, depends on D1 landing `VoxelEdit` + `ChunkEdit`)

**Pre-condition:** D1's impl phase has landed `VoxelEdit` + `ChunkEdit` named types and updated `WorldData::set_voxels_batch` / `set_chunks_uniform_batch` signatures.

**Edits:**
- `crates/bevy_naadf/src/editor/tools.rs:30-43` — `ChunkClass` enum + `CHUNK_VOXELS` const stay; restructure docblock at top of file to introduce the trait abstraction.
- `crates/bevy_naadf/src/editor/tools.rs:88-103` — body of `sphere_chunk_classify` moves into `impl SolidBrushShape for SphereShape::classify_chunk`. Free fn deleted.
- `crates/bevy_naadf/src/editor/tools.rs:108-121` — body of `cube_chunk_classify` moves into `impl SolidBrushShape for CubeShape::classify_chunk`. Free fn deleted.
- `crates/bevy_naadf/src/editor/tools.rs` — add `trait SolidBrushShape`, `struct CubeShape`, `struct SphereShape`, `fn apply_solid_brush<S: SolidBrushShape>(...)` per §2.1. Place between the helper section (`brush_chunk_aabb` etc.) and the public brush wrappers.
- `crates/bevy_naadf/src/editor/tools.rs:168-224` — `cube_brush` body replaced with `apply_solid_brush(&CubeShape, world_data, pos, radius, ty, is_erase)`. Signature unchanged.
- `crates/bevy_naadf/src/editor/tools.rs:231-287` — `sphere_brush` body replaced with `apply_solid_brush(&SphereShape, ...)`. Signature unchanged.
- `crates/bevy_naadf/src/editor/tools.rs:139,153,208,222,256,270` — anonymous-tuple `(IVec3, VoxelTypeId)` and `([u32;3], Option<VoxelTypeId>)` literals become `VoxelEdit { pos, ty }` and `ChunkEdit { pos: UVec3::new(...), ty: ... }`. This depends on D1's named types being on `main`.
- `crates/bevy_naadf/src/editor/tools.rs:159,160,218-222,281-285` — `world_data.set_voxels_batch(&edits)` / `set_chunks_uniform_batch(&inside_chunks)` call sites take the new typed slices. D1's signature change is `&[VoxelEdit]` / `&[ChunkEdit]` per D1 Finding 5.

**Rationale:** kills DUP-2 (~120 LOC of duplicate triple-loop); unifies tuple shape across the brush API surface; preserves e2e-pinned names.

**Post-step state:** `paint_brush` / `cube_brush` / `sphere_brush` public surface unchanged. `cube_brush` and `sphere_brush` are 3-line wrappers. `apply_solid_brush` owns the skeleton. `editor/tools.rs` drops ~80 LOC net.

**Verification:**
- `cargo build --workspace` — picks up `VoxelEdit` / `ChunkEdit` type changes.
- `cargo test --workspace --lib` — all 8 tests in `editor::tools::tests` (lines 290-597) must pass without modification.
- `cargo run --bin e2e_render -- --oasis-edit-visual` — calls `sphere_brush` directly; verifies the wrapper preserves shape.
- `cargo run --bin e2e_render -- --small-edit-repro` — calls `cube_brush(radius=1)` directly; verifies wrapper and the radius-1-emits-1-voxel invariant.
- `cargo run --bin e2e_render -- --small-edit-visual` — calls `cube_brush(radius=1.0)` (verified at `e2e/small_edit_visual.rs:349`).
- ≥2× runs per non-deterministic gate per `feedback-multiple-runs-rule-out-false-positives`.

---

### Step 5 — KNOBS decl-macro cutover (HIGH-3 + HIGH-4, depends on D7 landing `GiSettings::DEFAULT`)

**Pre-condition:** D7 has landed `pub const DEFAULT: GiSettings = GiSettings { ... };` + flipped `Default for GiSettings` to return `Self::DEFAULT`. Per §4, this can be a 3-line pre-land before the rest of D7's scope.

**Edits:**
- `crates/bevy_naadf/src/settings.rs:111-116` — delete `class: char` field from `struct Knob`. New shape `struct Knob { label: &'static str, kind: KnobKind }`.
- `crates/bevy_naadf/src/settings.rs:118-150` — `KnobKind` enum body unchanged.
- `crates/bevy_naadf/src/settings.rs:152-162` — `is_interactive` method unchanged.
- `crates/bevy_naadf/src/settings.rs:163-378` — add the 5 decl-macros (`knob_section!`, `knob_u32!`, `knob_f32!`, `knob_bool!`, `knob_readonly!`, `knob_action!`) per §2.3. Replace the 213-line `KNOBS: &[Knob]` table with the ~32-line macro-expansion table.
- `crates/bevy_naadf/src/settings.rs:779-805` — `update_settings_text` body: remove the `[{}]` suffix and the `row.class` reference (4 sites, lines 785/789/794/798).
- `crates/bevy_naadf/src/settings.rs:782` — `KnobKind::Section` branch unchanged; no class to print.
- `crates/bevy_naadf/src/settings.rs:897-905` — `promoted_defaults_match_canonical_consts` test now redundant by construction; replace its 7 `assert_eq!`s with a single `assert_eq!(GiSettings::DEFAULT.max_ray_steps_primary, 120);` etc. as **sanity assertion against the C# canonical values** (the only check still needed — DEFAULT-vs-KNOBS agreement is by construction).
- `crates/bevy_naadf/src/settings.rs:875-892` — `defaults_match_gi_settings_default` test: now redundant by construction (DEFAULT = Default by construction; KNOBS reads DEFAULT.field). Reduce to a single round-trip assertion: `assert_eq!(GiSettings::default(), GiSettings::DEFAULT)`. Requires `#[derive(PartialEq)]` on `GiSettings` (additive — D7's struct currently derives `Clone, Copy, Debug`).

**Rationale:** kills BEV-4 + OA-1 (213 → 32 LOC table). Kills HIGH-4 SSoT-1 partial (no defaults duplicated in `settings.rs`).

**Post-step state:** KNOBS reads as a spec table. No `default:` literals in `settings.rs`. `[P]`/`[C]`/`[D]` suffix gone from panel UI.

**Verification:**
- `cargo build --workspace`.
- `cargo test --workspace --lib` — `settings::tests::*` (lines 852-940) all pass. The renamed `promoted_defaults_match_canonical_consts` test now pins `GiSettings::DEFAULT` against the canonical literals.
- `cargo run --bin e2e_render -- baseline` — the e2e harness sets `add_hud = false`, so the settings panel doesn't render in any e2e gate. Step 5's correctness is purely unit-test + user-visual.
- **User live-check required:** Press Esc → see the panel; verify rows render (label + value), arrow keys navigate, drag-sliders scrub, R/Shift+R reset row/all, Esc closes. The `[P]`/`[C]` suffix being gone is the only intentional visual delta.

---

### Step 6 — `EditorPlugin` + `SettingsPlugin` + `AppModePlugin` (side-note 11)

**Edits:**
- `crates/bevy_naadf/src/editor/mod.rs` (bottom) — add `pub struct EditorPlugin; impl Plugin for EditorPlugin { ... }` per §2.10. Wraps the 7-system editor chain + `EditorState` init + `setup_editor_hud` startup.
- `crates/bevy_naadf/src/settings.rs` (bottom) — add `pub struct SettingsPlugin; impl Plugin for SettingsPlugin { ... }` per §2.10. Wraps `SettingsState` / `SettingsDrag` init + `setup_settings` startup + OnEnter/OnExit transitions + the 3-system Update chain.
- `crates/bevy_naadf/src/app_mode.rs` (bottom) — add `pub struct AppModePlugin; impl Plugin for AppModePlugin { ... }` per §2.10. Wraps `init_state::<AppMode>()` + `toggle_settings_on_escape` Update registration.
- `crates/bevy_naadf/src/editor/mod.rs` (inside `EditorPlugin::build`) — `apply_edit_tool.run_if(in_state(AppMode::Playing)).after(crate::app_mode::toggle_settings_on_escape)` — preserves the same-frame state-transition observation that the original 9-system `.chain()` had.

**Do NOT touch `lib.rs`.** D7's impl phase deletes the inline registration block at `lib.rs:900-971` and replaces it with `app.add_plugins((AppModePlugin, EditorPlugin, SettingsPlugin));` inside the existing `if cfg.add_hud { ... }` branch. D7 owns `lib.rs`; D2 only delivers the plugins ready to wire.

**Rationale:** D2 prepares the plugins so D7 only has to wire them (per the user's chosen D7-last sequencing). Net D2-scope delta: ~+90 LOC of plugin definitions across 3 files; D7 deletes ~70 LOC of inline registration at `lib.rs:900-971`. Net codebase: roughly neutral on LOC, but the editor + settings + app_mode subsystems are now each self-contained — adding a new editor system means editing `editor/mod.rs::EditorPlugin::build`, not `lib.rs`.

**Post-step state:** three new `Plugin` types are exported from `crate::editor::EditorPlugin`, `crate::settings::SettingsPlugin`, `crate::app_mode::AppModePlugin`. They are NOT yet registered anywhere — wiring is D7's job.

**Verification:**
- `cargo build --workspace` — plugins are unwired but the `Plugin` impls compile.
- `cargo test --workspace --lib` — nothing changes in unit tests.
- **D7's impl phase later runs all e2e gates** when wiring lands. D2's Step 6 alone produces no behavioural change; the wire is the test.

**Coordination note:** if D7's impl phase is delayed (D7 is scheduled last), D2 Step 6 lands without wiring. The `EditorPlugin` / `SettingsPlugin` / `AppModePlugin` types are dead code until D7 wires them. **This is by design** — the plugin types compile, are reachable, but unwired. The inline registration at `lib.rs:900-971` continues to drive the app. When D7 lands, the inline block is deleted and `app.add_plugins((AppModePlugin, EditorPlugin, SettingsPlugin))` replaces it — a single-commit swap.

---

## 7. What stays / what changes / what's removed

### Stays unchanged

- `crates/bevy_naadf/src/editor/ray.rs` (61 LOC) — `Ray` + `screen_to_ray`. Pure helper, no D2 finding touches it.
- `crates/bevy_naadf/src/editor/tools.rs:42` — `CHUNK_VOXELS` const.
- `crates/bevy_naadf/src/editor/tools.rs:47-82` — `brush_aabb` + `brush_chunk_aabb` helpers.
- `crates/bevy_naadf/src/editor/tools.rs:134-162` — `paint_brush` body (does not join the trait; faithful port unchanged).
- `crates/bevy_naadf/src/editor/mod.rs:42-103` — `EditTool` enum + `EditorState` struct + `Default` impl. Field set unchanged.
- `crates/bevy_naadf/src/editor/mod.rs:117-228` — `apply_edit_tool` system. Body unchanged.
- `crates/bevy_naadf/src/editor/hud.rs::setup_editor_hud` + `spawn_*` private helpers (`:135-533`) — internal structure unchanged (other than colour-const renames in Step 2). The 160-LOC `setup_editor_hud` is large but readable; side-note 6 of explorer flagged it but it's out of scope for D2's current refactor.
- `crates/bevy_naadf/src/editor/hud.rs::scroll_palette_with_wheel`, `drag_palette_scrollbar`, `update_palette_scrollbar`, `refresh_palette_swatches`, `handle_hud_clicks`, `update_editor_hud` system bodies (`:610-792, :796-903`) — only colour-const renames + the toggle-style fn extraction (Step 3) touch them.
- `crates/bevy_naadf/src/hud.rs::setup_hud` + `update_hud` + the GPU-timing path constants. Step 2 only swaps the inline text-bundle.
- `crates/bevy_naadf/src/app_mode.rs::AppMode` enum + `toggle_settings_on_escape` + `suspend_camera_input` + `restore_camera_input`. Step 6 only adds the plugin wrapper.
- `crates/bevy_naadf/src/settings.rs::setup_settings` body — colour-const + text-bundle renames only.
- `crates/bevy_naadf/src/settings.rs::adjust_settings`, `mouse_interact_settings`, `apply_drag_delta`, `handle_click_release`, `update_settings_text`, `step_cursor` — bodies unchanged except (a) the `class` field reference deleted from `update_settings_text` (Step 5), (b) Step 7's MEDIUM-7 deferred follow-up.
- `crates/bevy_naadf/src/settings.rs::SettingsState`, `SettingsDrag`, `DragState`, `SettingsBackdrop`, `SettingsRoot`, `SettingsRow`, `SettingsRowText`, `SettingsLegendText` markers / resources / enums. Unchanged.

### Changes

- `crates/bevy_naadf/src/editor/mod.rs` — adds `pub mod ui_theme`; removes `tool_from_u32` + its test (Step 1); adds `EditorPlugin` (Step 6).
- `crates/bevy_naadf/src/editor/tools.rs` — adds `trait SolidBrushShape` + `struct CubeShape` + `struct SphereShape` + `fn apply_solid_brush`; `cube_brush` / `sphere_brush` bodies become 1-line wrappers (Step 4); call sites switch to `VoxelEdit` / `ChunkEdit` (Step 4).
- `crates/bevy_naadf/src/editor/hud.rs` — 16 `COL_*` consts deleted, replaced by `use ui_theme::*;` (Step 2); 5 inline text-bundles → `text_style(...)` (Step 2); adds `fn toggle_button_style` + 2 callers shrink from 18 to 9 lines each (Step 3).
- `crates/bevy_naadf/src/settings.rs` — drops `DEFAULT_TAA_RING_DEPTH` import + the `let _` (Step 1); 12 `COL_*` consts deleted, replaced by `use ui_theme::*;` (Step 2); 3 inline text-bundles → `text_style(...)` (Step 2); 213-line KNOBS table → 32-line macro-driven table + 5 macro defs (Step 5); `class: char` field deleted; `update_settings_text` loses class suffix (Step 5); 2 unit tests collapse to sanity asserts (Step 5); adds `SettingsPlugin` (Step 6).
- `crates/bevy_naadf/src/hud.rs` — inline text-bundle → `text_style(...)` (Step 2).
- `crates/bevy_naadf/src/app_mode.rs` — adds `AppModePlugin` (Step 6).

### Removed

- `EditorState::tool_from_u32` (`editor/mod.rs:107-114`) + its test (`:246-251`) — Step 1, MEDIUM-5.
- `let _ = DEFAULT_TAA_RING_DEPTH;` (`settings.rs:543`) — Step 1, MEDIUM-6.
- All 16 file-local `COL_*` consts in `editor/hud.rs:109-124` — Step 2, LOW-9. **Replacement landing place:** `editor/ui_theme.rs` semantic palette.
- All 12 file-local `COL_*` consts in `settings.rs:44-56` — Step 2, LOW-9. **Replacement landing place:** `editor/ui_theme.rs`.
- 213-line `KNOBS: &[Knob]` literal table at `settings.rs:166-378` — Step 5, HIGH-3. **Replacement landing place:** decl-macro-driven ~32-line table in the same place.
- 6 `default:` literals at `settings.rs:174,184,194,202,210,220` — Step 5, HIGH-4. **Replacement landing place:** `GiSettings::DEFAULT.<field>` via macro expansion (D7 owns the const).
- `class: char` field on `struct Knob` (`settings.rs:114`) — Step 5. **No replacement** — section headers carry category info.
- The `[P]` / `[C]` / `[D]` / `[B]` suffix from panel readout — Step 5. **No replacement** — visual delta accepted per §2.3.
- 18-line erase loop body at `editor/hud.rs:839-857` — Step 3. **Replacement landing place:** 9-line shape calling `toggle_button_style` (also in `editor/hud.rs`).
- 18-line continuous loop body at `editor/hud.rs:859-877` — Step 3. Same replacement.
- 213-line single-method `cube_brush` + 60-line `sphere_brush` bodies (`tools.rs:168-224, 231-287`) — Step 4, HIGH-1. **Replacement landing place:** 3-line wrappers calling `apply_solid_brush(&CubeShape, ...)` / `apply_solid_brush(&SphereShape, ...)` in same file; trait skeleton fn owns the iteration.
- Free fns `cube_chunk_classify`, `sphere_chunk_classify` (`tools.rs:88-103, 108-121`) — Step 4. **Replacement landing place:** trait methods on `CubeShape::classify_chunk` / `SphereShape::classify_chunk` (bodies preserved verbatim).

---

## 8. Decisions & rejected alternatives

### Decision A — Decl-macro over Reflect for KNOBS

Rationale in §3.

### Decision B — `editor/ui_theme.rs` over top-level `ui_theme.rs`

Three D2 files consume; one top-level (`hud.rs`) + one nested (`editor/hud.rs`) + one nested (`settings.rs`). All three are D2; no other crate-wide consumer. Top-level naming would imply non-D2 reuse that doesn't exist. **Rejected** top-level placement.

### Decision C — Delete `class: char` field outright

Section headers carry category. The `[P]`/`[C]`/`[D]`/`[B]` suffix is port-specific ornament with no C# counterpart. **Rejected** "keep the suffix" alternative — it's a magic-character smell that adds nothing the section header doesn't already give.

### Decision D — `cube_brush` / `sphere_brush` as inline wrappers, not direct trait methods

The e2e gates call `crate::editor::tools::cube_brush(...)` and `crate::editor::tools::sphere_brush(...)` — by name. Making them `impl SolidBrushShape for CubeShape { fn dispatch(world_data, ...) }` would require call sites to construct a `CubeShape` value, which is a name + signature break. **Rejected** trait-method-only shape; kept the wrapper-fn shape.

### Decision E — `paint_brush` does not join `SolidBrushShape`

`paint_brush` has no inside-chunk fast-path and gates on `get_voxel_type` non-empty. Forcing it into the trait would need a 3rd predicate (`accept_voxel: fn(WorldData, IVec3) -> bool`) and a 4th classifier mode (`PaintMode → no Inside, only Mixed`), bloating the trait for one caller. **Rejected** "unify all 3 brushes" — keep paint standalone per the explorer's HIGH-1 suggestion.

### Decision F — Defer MEDIUM-7 (keyboard / drag / click dispatch math collapse)

The three sites diverge in saturation semantics (`saturating_add` vs `.clamp(min, max)` vs i64-widened) — collapsing introduces subtle behavioural change. The 213-line KNOBS table is the bigger fish in D2; MEDIUM-7 is the third-tier polish. **Deferred** explicitly (§2.7); flagged as a follow-up for a future architect or implementor judgement call after Step 5 lands.

### Decision G — Defer LOW-10 (split `handle_hud_clicks` into 5 systems)

The system has 5 Query args. Splitting buys borrow-graph parallelism, but the queries don't actually conflict (each is a `Changed<Interaction>` over a different marker component) and the system is short. Bevy's parallel scheduler will run `handle_hud_clicks` concurrently with any independent system already; splitting it into 5 doesn't change that. **Deferred** — flagged in side-notes.

### Decision H — D2 lands plugins; D7 wires them

The user's chosen sequencing has D7 last. D2's plugin extraction prepares the wrap so D7's later refactor is a single-commit swap. This is explicitly the recommendation in D7's exploration F1 ("D7 just deletes the inline lines").

---

## 9. Assumptions made

1. **D1's `VoxelEdit { pos: IVec3, ty: VoxelTypeId }` and `ChunkEdit { pos: UVec3, ty: Option<VoxelTypeId> }` named types land before D2 Step 4 runs.** Their D1 Finding 5 commits to producing them; if D1's architect picks different names, D2's impl phase reads D1's `03-architecture.md` first and substitutes. The names above are the proposal; the design adapts to whatever D1 names them.
2. **D7's `pub const GiSettings::DEFAULT: GiSettings = GiSettings { ... }` + `Default for GiSettings` returning `Self::DEFAULT` lands before D2 Step 5 runs.** D7's exploration F2 + §SSoT coordination commits to this; the const is a 3-line addition that can pre-land independently of D7's other scope. **Coordination request:** the orchestrator allows this pre-land.
3. **D7 keeps `GiSettings` either in `lib.rs:108-185` or in `settings.rs` after D7's F2 refactor.** D2's design works for either home — the macros use `GiSettings::DEFAULT` regardless of whether `GiSettings` lives in `crate` or `crate::settings`.
4. **D4 owns the WGSL-side SSoT-1 decision** (`ray_tracing.wgsl:122-126` doc-only consts: keep, delete, or shader-def-inject). D2 does nothing on the WGSL side.
5. **The `[P]` / `[C]` / `[D]` / `[B]` suffix in the settings panel readout is acceptable to delete.** Per `01-context.md` addendum: bevy_ui-vs-ImGui divergence is pre-approved within bevy_ui; this is port-internal ornament. If the user wants the suffix back, it can return as a `KnobKind`-driven prefix on the label string at table-build time — but no consumer relies on it today.
6. **Bevy 0.19-rc.1's `text_style()` returning `(TextColor, TextFont)` will spread correctly when used inside a tuple-bundle spawn.** Verified pattern: `commands.spawn((Text::new("..."), text_style(&dev_font, FG_PRIMARY, 13.0), Pickable::IGNORE))` flattens to `(Text, TextColor, TextFont, Pickable)` per Bevy's tuple-bundle implementation. If this assumption fails (a Bevy 0.19 trait-impl bound issue), the helper alternative is `text_label(dev_font, color, size) -> (Text, TextColor, TextFont)` constructing the `Text` too — slightly less flexible at call sites but always works.
7. **`#[derive(PartialEq)]` on `GiSettings` is additive.** `GiSettings` (lib.rs:108) currently derives `Clone, Copy, Debug`. Adding `PartialEq` is safe (all fields are `Copy` POD or `bool`); no consumer of `GiSettings` should break. Required for the simplified `defaults_match_gi_settings_default` unit test. **D7 territory** — D2 architect flags D7 to add it when landing `DEFAULT`.

---

## 10. D1 / D4 / D7 coordination notes

### D1 (UA-1 — `VoxelEdit` + `ChunkEdit` named types)

- **D2 depends on D1 for Step 4.** D1 ships `pub struct VoxelEdit { pub pos: IVec3, pub ty: VoxelTypeId }` + `pub struct ChunkEdit { pub pos: UVec3, pub ty: Option<VoxelTypeId> }` and updates `WorldData::set_voxels_batch` / `set_chunks_uniform_batch` signatures. D2 consumes.
- **D2 has no input on the named-type definitions** — D1's architect owns them per UA-1. D2 reads D1's `03-architecture.md` before Step 4 lands and substitutes the chosen field names if they differ from the proposal.
- D2 does NOT define these types. D2 is consumer only.

### D4 (SSoT-1 — GPU uniform consumer)

- D4 reads `GpuRenderParams.max_ray_steps_primary` + `GpuGiParams.{max_ray_steps_*, spatial_iter_count}` from the live `AppArgs.gi` (a.k.a. `GiSettings`). After D7's `DEFAULT` lands, the values still flow the same way (the uniform-side path is unchanged — D7's struct is the source, D4's uniform mirrors it).
- D4 may delete the documentation-only `MAX_RAY_STEPS_*` WGSL consts at `ray_tracing.wgsl:122-126`. D2 has no opinion — D2's KNOBS macros never reference the WGSL side.
- **No D4 ↔ D2 file overlap.** D4 owns `gpu_types.rs` + `extract.rs` + the WGSL. D2 owns `settings.rs`. Their only shared concept is `GiSettings`, which D7 owns the home of.

### D7 (canonical `GiSettings::DEFAULT` + `EditorPlugin`/`SettingsPlugin` wiring)

**Pre-land requested:**
- `impl GiSettings { pub const DEFAULT: GiSettings = GiSettings { ... }; }` per §4.
- `impl Default for GiSettings { fn default() -> Self { Self::DEFAULT } }` (replaces current body at `lib.rs:187-231`).
- `#[derive(PartialEq)]` added to `GiSettings` (lib.rs:108).
- This pre-land is **~30 lines of edit** in `lib.rs:108-231` and unblocks D2 Step 5.

**D2's deliverables for D7's later refactor:**
- `pub struct EditorPlugin` at `crate::editor::EditorPlugin` (Step 6).
- `pub struct SettingsPlugin` at `crate::settings::SettingsPlugin` (Step 6).
- `pub struct AppModePlugin` at `crate::app_mode::AppModePlugin` (Step 6).

**D7's later wire-up (D7's impl phase):**
- Delete the inline `if cfg.add_hud { ... }` block at `lib.rs:900-971`.
- Replace with: `if cfg.add_hud { app.add_plugins((AppModePlugin, EditorPlugin, SettingsPlugin)); app.add_systems(Startup, hud::setup_hud.after(load_dev_font)); app.add_systems(Update, hud::update_hud); }`.
- Or — if D7 decides to plugin-ize `hud.rs` too — `app.add_plugins((HudPlugin, AppModePlugin, EditorPlugin, SettingsPlugin))`. D7's call.
- D7 also handles the wasm-only `voxel::web_vox::hide_ui` registration at `lib.rs:970` (out of D2's scope).

If D7's `EditorPlugin` / `SettingsPlugin` ownership decision changes (e.g. D7 wants `HudPlugin` to live inside `editor/`), D2 architect has no objection — the plugin types D2 produces are immediately consumable and D7 may re-home them as needed.

---

## 11. Open conflicts

None. The brief explicitly pre-approves bevy_ui-vs-ImGui divergence; the `[P]`/`[C]` suffix deletion is within bevy_ui (not a behavioural change to the C# port). No forbidden moves (no API breaks, no file moves outside D2's path list, no dependency changes — `bevy_reflect` was a candidate but rejected for unrelated reasons).

The SSoT-1 dependency arrow on D7 is a **sequencing coordination**, not a conflict — the orchestrator can either let D7 pre-land the 3-line `DEFAULT` const, or D2 holds Step 5 until D7's full impl phase. Recommended path: pre-land.

---

## 12. Side notes / observations / complaints

1. **The brief says "your design is read by the implementor directly; the orchestrator does NOT" — this is correct discipline. Architect kept the design concrete and signature-level (not "design the trait" but "here is the trait, here is the wrapper signature, here are the macro arms, here is the migration order").**

2. **The KNOBS table cutover is the highest-leverage single change in D2 by a wide margin.** 213 LOC of repeated row literal → 32 LOC + 5 small macro definitions. This is the answer to the user's Q1 ("tight idiomatic Rust, idiomatic Bevy"): the macro approach IS idiomatic Rust (`macro_rules!` is the textbook tool for repetitive table literals) and idiomatic Bevy is *not* `bevy_reflect` for a config table — `bevy_reflect` is for runtime introspection (Bevy editor, scene serialization). The KNOBS table is build-time data; decl-macro fits.

3. **The brush trait extraction is genuinely smaller-value than the explorer suggested.** ~120 LOC saved, but the original duplication wasn't bug-prone (it tracks two C# classes that are also near-duplicated — `EditingToolCube.cs` vs `EditingToolSphere.cs`). The win is more "future cone capsule brush won't add a 3rd 60-line copy" than "current rot fixed". Still worth doing for IoC / idiom-fit reasons, but not the headline.

4. **D7's `lib.rs:900-971` block is 71 LOC and is the user-visible UI-wiring concentration. D2's Step 6 hauls it into 3 plugins.** Cleaner LOC-wise: ~+90 (3 plugin definitions) vs. -71 (the inline block) = net +19 LOC in D2's domain, **but** the inline block in `lib.rs` (D7's domain) goes away — net codebase delta is ~+19. The win is structural, not LOC.

5. **`editor/hud.rs::setup_editor_hud` is 160 lines (side-note 6 of explorer).** Not addressed in this design — out of D2 scope's bandwidth and the inlining is readable. Flag for a future refactor.

6. **`editor::EditorState` mixes config + runtime state (side-note 7 of explorer).** Not addressed — the docblock at `editor/mod.rs:53-89` already makes the mixing legible. Out of scope.

7. **The `[P]`/`[C]`/`[D]`/`[B]` suffix deletion is a deliberate, minor user-visible delta.** Most users won't notice. If the user pushes back, the architect's fallback is: keep the `class: char` field on `Knob` and the macros gain a `class=` parameter. Trivial revert; not designed for in this doc but flagged.

8. **`bevy_reflect` is genuinely not in this codebase.** That's a meaningful signal — the project deliberately stays away from runtime introspection. My rejection of `Reflect` for KNOBS aligns with the project's existing posture (verified via `grep` returning zero hits across 73 Rust files).

9. **Cross-domain rot observed but not edited:** `lib.rs:900-971` is D7's territory. D2's plugin extraction in Step 6 produces the plugins; D7's impl phase deletes the inline lines. This is a clean handoff, not D2 cross-editing into `lib.rs`. (The brief forbids cross-domain edits; D2 stays inside its own files.)

10. **Subjective:** the bevy-naadf D2 surface is genuinely tight under the duplication. The brush triple-loops and KNOBS table are the two real smells; everything else (the colour-const sprawl, the toggle-style drift) is minor cleanup that falls out naturally. D2 is one of the easier domains to land.

11. **Verification discipline:** every `path:line` reference in this doc was Read or grep-verified against current `main`. No fabricated citations. Per the brief's vigilance preamble.

12. **Compile-time field-existence — the explorer's binding open question HIGH-3.q (`02-exploration.md:118-119`) — is the central question this architect doc had to answer.** Choice (c) decl-macro preserves it strictly stronger than the existing function-pointer approach (typo in field name → compile error, no test required to catch). The user's stated concern ("the project may deliberately keep KNOBS explicit for compile-time safety") is honoured. Choice (a) `Reflect` would have weakened it.
