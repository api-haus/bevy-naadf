//! Comprehensive raymarching-quality dev panel (`21-design-quality-panel.md` +
//! `25-design-panel-mouse.md`).
//!
//! A `bevy_ui` 0.19 in-app panel that exposes every meaningful runtime knob
//! affecting raymarching / GI / reservoir / TAA quality, so the user can tune
//! live without rebuilds. Toggled by `F1`; closed by default. The dispatch from
//! `25-design-panel-mouse.md` adds mouse drag-sliders on top of the original
//! keyboard-driven navigator — **both input paths are live concurrently** and
//! mutate the same [`AppArgs.gi`] state.
//!
//! ## Why `bevy_ui` (not `bevy_egui`)
//!
//! `bevy_egui` 0.39.1 declares `bevy_app = 0.18.0` — no Bevy 0.19 release
//! exists, no open PR. `bevy-inspector-egui` rides on the same dep. The brief's
//! sanctioned fallback path is "bare egui + manual integration", but that is
//! ~500 lines of wgpu / winit / clipboard glue (= reimplementing `bevy_egui`).
//! The cleanest path that works on Bevy 0.19-rc.1 is **`bevy_ui` 0.19 native**
//! — zero new deps, same render-graph the HUD uses.
//!
//! ## Architecture
//!
//! - [`PanelState`] — main-world resource holding `open: bool` + the selected
//!   row index. Default = closed (`open: false`).
//! - [`PanelDrag`] — main-world resource holding the mouse drag state machine
//!   (`Idle` / `Pressed` / `Dragging`). See [`DragState`].
//! - [`PanelRoot`] — marker component on the panel root `Node` (the container).
//! - [`PanelRow`] — marker + index component on each per-row Node (one per
//!   entry in [`KNOBS`]). Carries `Interaction` and `RelativeCursorPosition`
//!   so the row is hit-testable.
//! - [`PanelRowText`] — marker on each row's Text entity (one Text per row).
//! - [`PanelLegendText`] — marker on the bottom legend Text entity.
//! - [`setup_panel`] — `Startup` system that spawns the root + per-row entities
//!   with `Display::None` (hidden) and the panel chrome layout.
//! - [`toggle_panel`] — `Update` system: F1 flips `PanelState.open` + toggles
//!   the root's `Display` between `None` ↔ `Flex`. Also forces
//!   `PanelDrag.state` back to `Idle` when the panel closes.
//! - [`adjust_panel`] — `Update` system: while open AND not mid-drag, reads
//!   keyboard input + mutates the selected knob on `AppArgs.gi`. Closed or
//!   mid-drag → no-op.
//! - [`mouse_interact_panel`] — `Update` system: while open, reads
//!   `Interaction` + `RelativeCursorPosition` + `AccumulatedMouseMotion` +
//!   `ButtonInput<MouseButton>`; drives the [`PanelDrag`] state machine;
//!   mutates `AppArgs.gi` for slider drag / bool toggle / button-click
//!   "Reset all".
//! - [`update_panel_text`] — `Update` system: rewrites each row's Text content
//!   every frame from current `AppArgs.gi` + the read-only diagnostics, and
//!   colors the selected row.
//!
//! ## Plumbing
//!
//! `AppArgs` is the single source of truth. Mutations land on
//! `AppArgs.gi: GiSettings`; the render-side `extract_gi_config` mirrors the
//! whole struct into `ExtractedGiConfig` every frame, so panel changes
//! propagate to the GPU uniform on the next frame with no extra wiring.
//!
//! ## Test-mode gate
//!
//! The panel is opt-in via `AppConfig.add_hud` (same gate as the HUD —
//! `01-context.md` §2.2). E2E config (`AppConfig::e2e`) has `add_hud = false`,
//! so the panel never spawns in the harness, and e2e luminance gates are
//! unaffected.
//!
//! ## Mouse handling — full design rationale in `25-design-panel-mouse.md`.

use std::fmt::Write;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::ui::{FocusPolicy, RelativeCursorPosition};
use bevy::window::PrimaryWindow;

use crate::{AppArgs, DevFont, GiSettings, DEFAULT_TAA_RING_DEPTH};
use crate::render::gi::{
    BUCKET_STORAGE_COUNT, INVALID_SAMPLE_STORAGE_COUNT, REFINED_BUCKET_STORAGE_COUNT,
    VALID_SAMPLE_STORAGE_COUNT,
};
use crate::render::taa::CAMERA_HISTORY_DEPTH;

/// Drag-detection threshold in **physical pixels** — below this on
/// `Left::just_released`, the gesture is treated as a click (bool flip / button
/// action / no-op for sliders). Above, it's a drag and any in-flight delta is
/// kept.
const DRAG_THRESHOLD_PX: f32 = 2.0;

/// Drag sensitivity reference width in **logical pixels** — one full traversal
/// of this width spans **1/8 of** the knob's `[min..max]` range (8× the
/// original 320 px, so 1 px ≤ 0.004 of the normalised range at base
/// sensitivity). Multiple drag strokes are needed to sweep the full range, but
/// 0.01-level control is comfortable at slow motion.
/// (`25-design-panel-mouse.md` §4.1). Scaled by `Window::scale_factor()` to
/// match physical-pixel motion on hi-DPI displays (§IR.3.B).
const DRAG_FULL_RANGE_PX: f32 = 2560.0;

/// Shift-modifier multiplier on drag sensitivity — matches the keyboard
/// `Shift+←/→` fine-grain semantics (4× more pixels for the same delta).
const DRAG_SHIFT_FACTOR: f32 = 0.25;

/// Main-world resource: is the panel open + what knob is selected.
///
/// `open` is flipped by [`toggle_panel`] on F1; the panel UI's `Display`
/// follows. `cursor` is a 0-based index into the [`KNOBS`] table (only
/// interactive rows step the cursor — the navigator skips readonly rows).
#[derive(Resource, Debug, Clone, Copy)]
pub struct PanelState {
    /// Panel visibility — toggled by F1.
    pub open: bool,
    /// Currently-selected knob index into [`KNOBS`]. Range 0..[`KNOBS`].len().
    pub cursor: usize,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            open: false,
            // Start on the first interactive knob (`max_ray_steps_primary`).
            cursor: 0,
        }
    }
}

/// Drag state machine — see `25-design-panel-mouse.md` §3.
///
/// Only one row can be dragged at a time (Bevy's `Interaction::Pressed` is
/// per-entity but the cursor has one position; only one entity reaches
/// `Pressed` at a time).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DragState {
    /// No interaction in progress.
    #[default]
    Idle,
    /// Mouse Left pressed on row `knob_index`, but motion below threshold —
    /// gesture is still ambiguous click-vs-drag. `total_motion` accumulates
    /// horizontal motion since press.
    Pressed { knob_index: usize, total_motion: f32 },
    /// Threshold exceeded — the gesture is a drag and value is integrating
    /// each frame. `frac_accum` carries fractional steps for `U32` knobs.
    Dragging { knob_index: usize, frac_accum: f32 },
}

/// Main-world resource: the drag state machine. `Default` = `Idle`.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct PanelDrag {
    /// Current state.
    pub state: DragState,
}

/// Marker for the panel root `Node` (the container).
#[derive(Component)]
pub struct PanelRoot;

/// Marker + index for each per-row Node. The index maps to [`KNOBS`].
#[derive(Component, Clone, Copy)]
pub struct PanelRow {
    /// Index into [`KNOBS`] this row represents.
    pub knob_index: usize,
}

/// Marker for the per-row Text entity (one Text child per row Node).
#[derive(Component)]
pub struct PanelRowText {
    /// Index into [`KNOBS`] this Text describes (mirror of the parent
    /// `PanelRow.knob_index`).
    pub knob_index: usize,
}

/// Marker for the panel's bottom legend Text entity (the keybind hint line).
#[derive(Component)]
pub struct PanelLegendText;

/// One row in the panel — a knob descriptor.
///
/// `kind` determines mutation behaviour (`nudge` / `big_step` / a `bool` flip
/// / readonly / button action); `getter` / `setter` operate on `AppArgs.gi`
/// for `GiKnob` rows.
struct Knob {
    /// Display label (left-aligned in the panel row).
    label: &'static str,
    /// Knob class indicator: 'P' = promoted runtime knob, 'C' = already
    /// config, 'D' = read-only diagnostic, 'B' = button action, ' ' = section.
    class: char,
    /// Mutation kind.
    kind: KnobKind,
}

#[allow(clippy::type_complexity)] // function pointers carry their own arity
enum KnobKind {
    /// Section-header row — no value, no interaction. The cursor skips it.
    Section,
    /// A `u32` knob on `AppArgs.gi`. `getter` reads, `setter` writes (clamped).
    U32 {
        getter: fn(&GiSettings) -> u32,
        setter: fn(&mut GiSettings, u32),
        nudge: u32,
        big_step: u32,
        min: u32,
        max: u32,
        default: u32,
    },
    /// An `f32` knob on `AppArgs.gi`.
    F32 {
        getter: fn(&GiSettings) -> f32,
        setter: fn(&mut GiSettings, f32),
        nudge: f32,
        big_step: f32,
        min: f32,
        max: f32,
        default: f32,
    },
    /// A `bool` knob on `AppArgs.gi` (keyboard Left/Right flip; mouse click
    /// flip).
    Bool {
        getter: fn(&GiSettings) -> bool,
        setter: fn(&mut GiSettings, bool),
        default: bool,
    },
    /// A read-only diagnostic — display only, cursor skips, mouse passes.
    Readonly {
        value: fn(&AppArgs) -> String,
    },
    /// A button-action row — no value, just a click target. Action runs the
    /// `apply` closure-pointer when clicked (or selected via keyboard `R`).
    /// `25-design-panel-mouse.md` §5.2 — used for the "Reset all" row.
    Action {
        /// Run the action; receives the panel's full AppArgs for mutation.
        apply: fn(&mut AppArgs),
    },
}

impl KnobKind {
    /// `true` if the cursor should land on this row AND mouse hit-test the row
    /// — `Section` and `Readonly` rows are visually present but inert.
    fn is_interactive(&self) -> bool {
        matches!(
            self,
            KnobKind::U32 { .. }
                | KnobKind::F32 { .. }
                | KnobKind::Bool { .. }
                | KnobKind::Action { .. }
        )
    }
}

/// The full knob table — one row per panel line, in display order. Sections
/// land as `Section` kinds (cursor skips); readonly rows as `Readonly`. The
/// final `Action` row is the mouse-clickable "Reset all" button.
///
/// `21-design-quality-panel.md` §5 + `25-design-panel-mouse.md` §5.2 panel
/// layout maps directly to this array.
const KNOBS: &[Knob] = &[
    Knob {
        label: "RAY STEP CAPS",
        class: ' ',
        kind: KnobKind::Section,
    },
    Knob {
        label: "  primary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_primary,
            setter: |g, v| g.max_ray_steps_primary = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 120,
        },
    },
    Knob {
        label: "  secondary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_secondary,
            setter: |g, v| g.max_ray_steps_secondary = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 100,
        },
    },
    Knob {
        label: "  sun",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_sun,
            setter: |g, v| g.max_ray_steps_sun = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 120,
        },
    },
    Knob {
        label: "  sun-secondary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_sun_secondary,
            setter: |g, v| g.max_ray_steps_sun_secondary = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 80,
        },
    },
    Knob {
        label: "  visibility",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_visibility,
            setter: |g, v| g.max_ray_steps_visibility = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 60,
        },
    },
    Knob {
        label: "SPATIAL RESAMPLING",
        class: ' ',
        kind: KnobKind::Section,
    },
    Knob {
        label: "  iter count",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.spatial_iter_count,
            setter: |g, v| g.spatial_iter_count = v,
            nudge: 1,
            big_step: 4,
            min: 1,
            max: 32,
            default: 12,
        },
    },
    Knob {
        label: "  sun_shadow_taps",
        class: 'C',
        kind: KnobKind::U32 {
            getter: |g| g.sun_shadow_taps,
            setter: |g, v| g.sun_shadow_taps = v,
            nudge: 1,
            big_step: 4,
            min: 1,
            max: 32,
            default: 4,
        },
    },
    Knob {
        label: "  resample_size",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.spatial_resample_size,
            setter: |g, v| g.spatial_resample_size = v,
            nudge: 50.0,
            big_step: 200.0,
            min: 32.0,
            max: 2000.0,
            default: 500.0,
        },
    },
    Knob {
        label: "  radius_lit_factor",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.radius_lit_factor,
            setter: |g, v| g.radius_lit_factor = v,
            nudge: 0.5,
            big_step: 3.0,
            min: 0.0,
            max: 1000.0,
            default: 3.0,
        },
    },
    Knob {
        label: "  noise_suppress",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.noise_suppression_factor,
            setter: |g, v| g.noise_suppression_factor = v,
            nudge: 0.05,
            big_step: 0.5,
            min: 0.01,
            max: 100.0,
            default: 0.4,
        },
    },
    Knob {
        label: "GI",
        class: ' ',
        kind: KnobKind::Section,
    },
    Knob {
        label: "  bounce_count",
        class: 'C',
        kind: KnobKind::U32 {
            getter: |g| g.bounce_count,
            setter: |g, v| g.bounce_count = v,
            nudge: 1,
            big_step: 1,
            min: 1,
            max: 3,
            default: 3,
        },
    },
    Knob {
        label: "  denoise_thresh",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.denoise_thresh,
            setter: |g, v| g.denoise_thresh = v,
            nudge: 50.0,
            big_step: 200.0,
            min: 0.0,
            max: 2000.0,
            default: 400.0,
        },
    },
    Knob {
        label: "  is_denoise",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.is_denoise,
            setter: |g, v| g.is_denoise = v,
            default: true,
        },
    },
    Knob {
        label: "  is_sample_leveling",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.is_sample_leveling,
            setter: |g, v| g.is_sample_leveling = v,
            default: true,
        },
    },
    Knob {
        label: "  is_varying_radius",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.is_varying_resampling_radius,
            setter: |g, v| g.is_varying_resampling_radius = v,
            default: true,
        },
    },
    Knob {
        label: "  is_atmosphere_int",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.is_atmosphere_interaction,
            setter: |g, v| g.is_atmosphere_interaction = v,
            default: true,
        },
    },
    Knob {
        label: "  skip_samples",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.skip_samples,
            setter: |g, v| g.skip_samples = v,
            default: true,
        },
    },
    Knob {
        label: "DIAGNOSTICS (read-only)",
        class: ' ',
        kind: KnobKind::Section,
    },
    Knob {
        label: "  taa_ring_depth",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |a| format!("{} [restart-required]", a.taa_ring_depth),
        },
    },
    Knob {
        label: "  camera_history_depth",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [const]", CAMERA_HISTORY_DEPTH),
        },
    },
    Knob {
        label: "  valid_sample_storage",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [storage-tied]", VALID_SAMPLE_STORAGE_COUNT),
        },
    },
    Knob {
        label: "  invalid_sample_storage",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [storage-tied]", INVALID_SAMPLE_STORAGE_COUNT),
        },
    },
    Knob {
        label: "  bucket_storage",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [storage-tied]", BUCKET_STORAGE_COUNT),
        },
    },
    Knob {
        label: "  refined_bucket",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [storage-tied]", REFINED_BUCKET_STORAGE_COUNT),
        },
    },
    Knob {
        label: "  global_illum_max_accum",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |a| format!("{} [const]", a.gi.global_illum_max_accum),
        },
    },
    // The "Reset all" button row — `25-design-panel-mouse.md` §5.2. Click (or
    // keyboard `R` while selected) restores every knob to its declared
    // default. Mirrors the existing `Shift+R` keybind for mouse users.
    Knob {
        label: "> RESET ALL TO DEFAULTS <",
        class: 'B',
        kind: KnobKind::Action {
            apply: reset_all_knobs,
        },
    },
];

/// Apply every knob's `default` to `AppArgs.gi` (preserves field identity by
/// calling each row's `setter`). Shared by the `Shift+R` keybind, the
/// `KnobKind::Action` "Reset all" row, and `mouse_interact_panel`'s
/// button-action edge.
fn reset_all_knobs(args: &mut AppArgs) {
    for row in KNOBS {
        match row.kind {
            KnobKind::U32 { setter, default, .. } => setter(&mut args.gi, default),
            KnobKind::F32 { setter, default, .. } => setter(&mut args.gi, default),
            KnobKind::Bool { setter, default, .. } => setter(&mut args.gi, default),
            _ => {}
        }
    }
}

/// First interactive-row index (the cursor lands here on startup) — the first
/// `KnobKind::U32` / `F32` / `Bool` / `Action` past any leading `Section`s.
fn first_interactive() -> usize {
    KNOBS
        .iter()
        .position(|k| k.kind.is_interactive())
        .unwrap_or(0)
}

/// `Startup` system: spawn the panel root + per-row Node entities. The root
/// starts hidden (`Display::None`) — F1 reveals it.
///
/// Layout: each row of [`KNOBS`] becomes one child Node with a Text
/// grandchild. Interactive rows (U32/F32/Bool/Action) also get `Interaction`
/// + `RelativeCursorPosition` for mouse hit-testing; `Section`/`Readonly`
/// rows do not. The root + all rows carry `FocusPolicy::Block` so mouse
/// presses inside the panel never bleed to a future scene-control system.
pub fn setup_panel(mut commands: Commands, dev_font: Res<DevFont>) {
    let root = commands
        .spawn((
            PanelRoot,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(12.0),
                left: px(12.0),
                padding: px(10.0).all(),
                width: px(360.0),
                // Vertical stack of rows.
                flex_direction: FlexDirection::Column,
                row_gap: px(0.0),
                // Hidden until F1 toggle.
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            // Belt-and-suspenders: block mouse press-through on the panel
            // padding too, not just the rows (`25-design-panel-mouse.md`
            // §IR.1.4).
            FocusPolicy::Block,
        ))
        .id();

    // Title row (non-interactive header).
    let title = commands
        .spawn((
            Node::default(),
            BackgroundColor(Color::NONE),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("[F1] Raymarching Quality"),
                TextColor(Color::srgb(0.85, 0.85, 0.85)),
                TextFont {
                    font: dev_font.0.clone(),
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
            ));
        })
        .id();
    commands.entity(root).add_child(title);

    // Divider row.
    let divider = commands
        .spawn((Node::default(),))
        .with_children(|p| {
            p.spawn((
                Text::new("─────────────────────────────"),
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
                TextFont {
                    font: dev_font.0.clone(),
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
            ));
        })
        .id();
    commands.entity(root).add_child(divider);

    // One Node per KNOBS row.
    for (i, knob) in KNOBS.iter().enumerate() {
        // Build the row Node — interactive rows additionally carry
        // `Interaction` + `RelativeCursorPosition` + `FocusPolicy::Block`.
        let mut row_cmd = commands.spawn((
            PanelRow { knob_index: i },
            Node {
                width: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ));
        if knob.kind.is_interactive() {
            row_cmd.insert((
                Interaction::default(),
                RelativeCursorPosition::default(),
                FocusPolicy::Block,
            ));
        }
        let row = row_cmd.id();

        let text = commands
            .spawn((
                PanelRowText { knob_index: i },
                Text::default(),
                TextColor(Color::WHITE),
                TextFont {
                    font: dev_font.0.clone(),
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
            ))
            .id();
        commands.entity(row).add_child(text);
        commands.entity(root).add_child(row);
    }

    // Bottom legend Text.
    let legend = commands
        .spawn((Node::default(),))
        .with_children(|p| {
            p.spawn((
                PanelLegendText,
                Text::default(),
                TextColor(Color::srgb(0.65, 0.65, 0.65)),
                TextFont {
                    font: dev_font.0.clone(),
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
            ));
        })
        .id();
    commands.entity(root).add_child(legend);

    // Silence the const re-export-still-used check (mirrored from the prior
    // panel.rs structure — `DEFAULT_TAA_RING_DEPTH` is documented elsewhere
    // and the import keeps a doc anchor live).
    let _ = DEFAULT_TAA_RING_DEPTH;
}

/// `Update` system: F1 toggles `PanelState.open` and the root `Node`'s
/// `Display` between `None` (hidden) and `Flex` (visible). Just-pressed only —
/// holding F1 does not retoggle. Also forces `PanelDrag.state` back to `Idle`
/// when the panel closes mid-drag (`25-design-panel-mouse.md` §3.1 last
/// transition).
pub fn toggle_panel(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<PanelState>,
    mut drag: ResMut<PanelDrag>,
    mut root: Query<&mut Node, With<PanelRoot>>,
) {
    if !keys.just_pressed(KeyCode::F1) {
        return;
    }
    state.open = !state.open;
    if let Ok(mut node) = root.single_mut() {
        node.display = if state.open {
            Display::Flex
        } else {
            Display::None
        };
    }
    // Closing aborts any in-flight drag.
    if !state.open {
        drag.state = DragState::Idle;
    }
    // On first-open, anchor the cursor on the first interactive row in case
    // the default was somehow skipped.
    if state.open && !KNOBS.get(state.cursor).map(|k| k.kind.is_interactive()).unwrap_or(false) {
        state.cursor = first_interactive();
    }
}

/// `Update` system: while the panel is open AND no drag is in progress, read
/// keyboard input + mutate the selected knob on `AppArgs.gi`. Closed → no-op
/// (camera input flows through normally). Mid-drag → no-op
/// (`25-design-panel-mouse.md` §6.2 row 1 — drag wins).
///
/// Bindings:
/// - Up / Down — move cursor (skips Section / Readonly rows).
/// - Left / Right — adjust selected knob by `nudge` (bool flips; Action
///   row is a no-op).
/// - PageUp / PageDown — adjust by `big_step`.
/// - Shift + Left/Right — fine adjust (`nudge / 4`, rounded for u32).
/// - R — reset selected knob to its default (or fire the Action row).
/// - Shift + R — reset every knob to defaults.
pub fn adjust_panel(
    keys: Res<ButtonInput<KeyCode>>,
    drag: Res<PanelDrag>,
    mut state: ResMut<PanelState>,
    mut args: ResMut<AppArgs>,
) {
    if !state.open {
        return;
    }
    // Drag wins — keyboard adjustments are ignored mid-drag
    // (`25-design-panel-mouse.md` §6.2 row 1).
    if !matches!(drag.state, DragState::Idle) {
        return;
    }

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    // Cursor navigation — skip non-interactive rows in the appropriate dir.
    if keys.just_pressed(KeyCode::ArrowUp) {
        state.cursor = step_cursor(state.cursor, -1);
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        state.cursor = step_cursor(state.cursor, 1);
    }

    // Adjust the selected knob.
    let left = keys.just_pressed(KeyCode::ArrowLeft);
    let right = keys.just_pressed(KeyCode::ArrowRight);
    let big_left = keys.just_pressed(KeyCode::PageUp);
    let big_right = keys.just_pressed(KeyCode::PageDown);
    let reset_one = keys.just_pressed(KeyCode::KeyR) && !shift;
    let reset_all = keys.just_pressed(KeyCode::KeyR) && shift;

    if reset_all {
        reset_all_knobs(&mut args);
        return;
    }

    if let Some(row) = KNOBS.get(state.cursor) {
        match row.kind {
            KnobKind::U32 { getter, setter, nudge, big_step, min, max, default } => {
                let mut v = getter(&args.gi);
                let n_step = if shift { (nudge / 4).max(1) } else { nudge };
                let b_step = big_step;
                if left {
                    v = v.saturating_sub(n_step);
                }
                if right {
                    v = v.saturating_add(n_step);
                }
                if big_left {
                    v = v.saturating_sub(b_step);
                }
                if big_right {
                    v = v.saturating_add(b_step);
                }
                if reset_one {
                    v = default;
                }
                setter(&mut args.gi, v.clamp(min, max));
            }
            KnobKind::F32 { getter, setter, nudge, big_step, min, max, default } => {
                let mut v = getter(&args.gi);
                let n_step = if shift { nudge / 4.0 } else { nudge };
                let b_step = big_step;
                if left {
                    v -= n_step;
                }
                if right {
                    v += n_step;
                }
                if big_left {
                    v -= b_step;
                }
                if big_right {
                    v += b_step;
                }
                if reset_one {
                    v = default;
                }
                setter(&mut args.gi, v.clamp(min, max));
            }
            KnobKind::Bool { getter, setter, default } => {
                let mut v = getter(&args.gi);
                if left || right {
                    v = !v;
                }
                if reset_one {
                    v = default;
                }
                setter(&mut args.gi, v);
            }
            KnobKind::Action { apply } => {
                // R while cursored on the action row fires it (the keyboard
                // mirror of clicking the row).
                if reset_one {
                    apply(&mut args);
                }
            }
            _ => {}
        }
    }
}

/// `Update` system: while the panel is open, drive the mouse-drag state
/// machine; mutate `AppArgs.gi` for slider drags / bool clicks / button
/// clicks. Closed → forces drag back to `Idle` and returns.
///
/// See `25-design-panel-mouse.md` §3 for the state machine, §4 for
/// sensitivity, §5 for click-edge detection, §6 for keyboard/mouse policy.
pub fn mouse_interact_panel(
    mut state: ResMut<PanelState>,
    mut drag: ResMut<PanelDrag>,
    mut args: ResMut<AppArgs>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rows: Query<(&PanelRow, &Interaction)>,
) {
    // Closed → drag aborted, no work.
    if !state.open {
        drag.state = DragState::Idle;
        return;
    }

    // Hover → cursor-tracks-knob (only while Idle). Find the first
    // interactive row whose `Interaction == Hovered`. Bevy guarantees at most
    // one row reports `Hovered` at a time because `ui_focus_system`
    // partitions by topology — but iterating is fine.
    if matches!(drag.state, DragState::Idle) {
        for (row, interaction) in &rows {
            if *interaction == Interaction::Hovered {
                if KNOBS.get(row.knob_index).map(|k| k.kind.is_interactive()).unwrap_or(false) {
                    state.cursor = row.knob_index;
                }
                break;
            }
        }
    }

    // Edge detection — find a row in `Pressed` state that is NOT the row
    // currently being dragged. This fires on the frame Bevy flips
    // `Interaction` from `Hovered → Pressed`.
    let pressed_row: Option<usize> = rows
        .iter()
        .find(|(_, i)| **i == Interaction::Pressed)
        .map(|(r, _)| r.knob_index);

    // Compute physical Δx for this frame, scaled by Shift modifier.
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let raw_dx = motion.delta.x;
    let shift_factor = if shift { DRAG_SHIFT_FACTOR } else { 1.0 };

    // Drive the state machine.
    match drag.state {
        DragState::Idle => {
            if let Some(knob_index) = pressed_row {
                // Transition Idle → Pressed.
                drag.state = DragState::Pressed {
                    knob_index,
                    total_motion: 0.0,
                };
            }
        }
        DragState::Pressed { knob_index, mut total_motion } => {
            total_motion += raw_dx.abs();
            if total_motion >= DRAG_THRESHOLD_PX {
                // Promote to Dragging — apply this frame's delta as the first
                // drag step (so the value moves immediately once threshold
                // crossed, not on the next frame).
                drag.state = DragState::Dragging {
                    knob_index,
                    frac_accum: 0.0,
                };
                apply_drag_delta(
                    &mut args,
                    &mut drag,
                    knob_index,
                    raw_dx,
                    shift_factor,
                    window_scale(&window),
                );
            } else if mouse_buttons.just_released(MouseButton::Left) {
                // Release-without-drag → click semantics.
                handle_click_release(&mut args, knob_index);
                drag.state = DragState::Idle;
            } else {
                drag.state = DragState::Pressed {
                    knob_index,
                    total_motion,
                };
            }
        }
        DragState::Dragging { knob_index, .. } => {
            if mouse_buttons.just_released(MouseButton::Left) {
                drag.state = DragState::Idle;
            } else {
                // Apply this frame's motion delta.
                apply_drag_delta(
                    &mut args,
                    &mut drag,
                    knob_index,
                    raw_dx,
                    shift_factor,
                    window_scale(&window),
                );
            }
        }
    }
}

/// Window scale factor for hi-DPI sensitivity scaling
/// (`25-design-panel-mouse.md` §IR.3.B). Defaults to 1.0 if no primary window
/// is available.
fn window_scale(window: &Query<&Window, With<PrimaryWindow>>) -> f32 {
    window.iter().next().map(|w| w.scale_factor()).unwrap_or(1.0)
}

/// Apply one frame's drag motion to the selected knob.
///
/// `raw_dx` is the physical-pixel horizontal cursor delta this frame.
/// `shift_factor` is 1.0 by default, 0.25 with Shift held.
/// `scale_factor` is `Window::scale_factor()` — multiplies `DRAG_FULL_RANGE_PX`
/// so a full panel-wide drag spans the full range regardless of DPI.
///
/// For `U32` knobs the integer accumulator (`frac_accum`) carries the
/// fractional pending step between frames, so very slow drags eventually
/// advance.
fn apply_drag_delta(
    args: &mut AppArgs,
    drag: &mut PanelDrag,
    knob_index: usize,
    raw_dx: f32,
    shift_factor: f32,
    scale_factor: f32,
) {
    let row = match KNOBS.get(knob_index) {
        Some(r) => r,
        None => return,
    };
    let full_range_px = DRAG_FULL_RANGE_PX * scale_factor;
    if full_range_px <= 0.0 {
        return;
    }
    let dx = raw_dx * shift_factor;

    match row.kind {
        KnobKind::U32 { getter, setter, min, max, .. } => {
            let range = (max as f32 - min as f32).max(1.0);
            let delta_f = dx * (range / full_range_px);
            // Pull current frac_accum out (mut bind below).
            let mut frac_accum = match drag.state {
                DragState::Dragging { frac_accum, .. } => frac_accum,
                _ => 0.0,
            };
            frac_accum += delta_f;
            let whole = frac_accum.trunc();
            frac_accum -= whole;
            let cur = getter(&args.gi) as i64;
            let new = (cur + whole as i64).clamp(min as i64, max as i64) as u32;
            setter(&mut args.gi, new);
            // Write back the residual fractional step.
            drag.state = DragState::Dragging {
                knob_index,
                frac_accum,
            };
        }
        KnobKind::F32 { getter, setter, min, max, .. } => {
            let range = (max - min).max(f32::EPSILON);
            let delta = dx * (range / full_range_px);
            let cur = getter(&args.gi);
            let new = (cur + delta).clamp(min, max);
            setter(&mut args.gi, new);
        }
        // Bool / Readonly / Section / Action: drag does nothing (release-
        // without-drag handles bool flip + action click; readonly + section
        // are inert).
        _ => {}
    }
}

/// Handle a release-without-drag (click) on the given row.
///
/// - `Bool` row: flip the value.
/// - `Action` row: invoke the action.
/// - `U32` / `F32` row: no-op (cursor already followed hover; the click
///   "selects" but doesn't change the value — match the keyboard
///   semantics of `↑/↓` to a row).
fn handle_click_release(args: &mut AppArgs, knob_index: usize) {
    let row = match KNOBS.get(knob_index) {
        Some(r) => r,
        None => return,
    };
    match row.kind {
        KnobKind::Bool { getter, setter, .. } => {
            let v = getter(&args.gi);
            setter(&mut args.gi, !v);
        }
        KnobKind::Action { apply } => {
            apply(args);
        }
        _ => {}
    }
}

/// `Update` system: rewrite each row's Text content from `AppArgs.gi` + the
/// read-only diagnostics, and color the selected row brighter. Runs every
/// frame *only when the panel is open* (cheap `state.open` guard).
pub fn update_panel_text(
    state: Res<PanelState>,
    args: Res<AppArgs>,
    mut row_texts: Query<(&PanelRowText, &mut Text, &mut TextColor)>,
    mut legend: Query<&mut Text, (With<PanelLegendText>, Without<PanelRowText>)>,
) {
    if !state.open {
        return;
    }

    for (row_text, mut text, mut text_color) in &mut row_texts {
        let i = row_text.knob_index;
        let Some(row) = KNOBS.get(i) else { continue };
        let s = &mut text.0;
        s.clear();
        let marker = if i == state.cursor && row.kind.is_interactive() {
            "> "
        } else {
            "  "
        };
        match &row.kind {
            KnobKind::Section => {
                let _ = write!(s, "  {}", row.label);
                *text_color = TextColor(Color::srgb(0.65, 0.85, 0.85));
            }
            KnobKind::U32 { getter, .. } => {
                let _ = write!(
                    s,
                    "{}{:<22} {:>6} [{}]",
                    marker,
                    row.label,
                    getter(&args.gi),
                    row.class,
                );
                *text_color = row_color(i == state.cursor);
            }
            KnobKind::F32 { getter, .. } => {
                let _ = write!(
                    s,
                    "{}{:<22} {:>6.2} [{}]",
                    marker,
                    row.label,
                    getter(&args.gi),
                    row.class,
                );
                *text_color = row_color(i == state.cursor);
            }
            KnobKind::Bool { getter, .. } => {
                let _ = write!(
                    s,
                    "{}{:<22} {:>6} [{}]",
                    marker,
                    row.label,
                    if getter(&args.gi) { "true" } else { "false" },
                    row.class,
                );
                *text_color = row_color(i == state.cursor);
            }
            KnobKind::Readonly { value } => {
                let _ = write!(s, "{}{:<22} {} [{}]", marker, row.label, value(&args), row.class);
                *text_color = TextColor(Color::srgb(0.55, 0.55, 0.55));
            }
            KnobKind::Action { .. } => {
                // Center-ish — the label already carries its own framing.
                let _ = write!(s, "{}      {}", marker, row.label);
                *text_color = row_color(i == state.cursor);
            }
        }
    }

    if let Ok(mut text) = legend.single_mut() {
        let s = &mut text.0;
        s.clear();
        let _ = write!(
            s,
            "[↑↓] navigate  [←→] adjust  [PgUp/PgDn] big  [Shift+←→] fine\n\
             [R] reset row  [Shift+R] reset all   |   Mouse: drag sliders, click rows",
        );
    }
}

/// Row text color: brighter when the cursor is on this row, normal otherwise.
fn row_color(selected: bool) -> TextColor {
    if selected {
        TextColor(Color::srgb(1.0, 1.0, 0.6))
    } else {
        TextColor(Color::WHITE)
    }
}

/// Move the cursor by `delta` (±1), skipping non-interactive rows. Wraps at
/// both ends. Returns the new cursor index.
fn step_cursor(cur: usize, delta: i32) -> usize {
    let n = KNOBS.len();
    if n == 0 {
        return 0;
    }
    let mut i = cur as i32;
    for _ in 0..n {
        i += delta;
        if i < 0 {
            i = (n as i32) - 1;
        }
        if i >= n as i32 {
            i = 0;
        }
        let ui = i as usize;
        if KNOBS[ui].kind.is_interactive() {
            return ui;
        }
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cursor must land on an interactive row after a single `step` from
    /// any starting position. Verifies the section / readonly skip logic.
    #[test]
    fn cursor_skips_non_interactive_rows() {
        for start in 0..KNOBS.len() {
            let next = step_cursor(start, 1);
            assert!(
                KNOBS[next].kind.is_interactive(),
                "cursor landed on a non-interactive row from start={start}, next={next}",
            );
            let prev = step_cursor(start, -1);
            assert!(
                KNOBS[prev].kind.is_interactive(),
                "cursor landed on a non-interactive row from start={start}, prev={prev}",
            );
        }
    }

    /// All defaults in the knob table MUST match `GiSettings::default()` —
    /// `21-design-quality-panel.md` §6 bit-equivalence promise. A drift here
    /// is a panel-default-vs-`GiSettings::default()` mismatch that would
    /// silently change behaviour on `R` (reset row).
    #[test]
    fn defaults_match_gi_settings_default() {
        let g = GiSettings::default();
        for row in KNOBS {
            match row.kind {
                KnobKind::U32 { getter, default, .. } => {
                    assert_eq!(
                        getter(&g),
                        default,
                        "u32 knob {:?} default ({}) != GiSettings::default ({})",
                        row.label,
                        default,
                        getter(&g),
                    );
                }
                KnobKind::F32 { getter, default, .. } => {
                    assert!(
                        (getter(&g) - default).abs() < f32::EPSILON,
                        "f32 knob {:?} default ({}) != GiSettings::default ({})",
                        row.label,
                        default,
                        getter(&g),
                    );
                }
                KnobKind::Bool { getter, default, .. } => {
                    assert_eq!(
                        getter(&g),
                        default,
                        "bool knob {:?} default ({}) != GiSettings::default ({})",
                        row.label,
                        default,
                        getter(&g),
                    );
                }
                _ => {}
            }
        }
    }

    /// Class-P ray-step-cap defaults must equal the WGSL `MAX_RAY_STEPS_*`
    /// consts the promotions replaced — the bit-equivalence promise of
    /// `21-design-quality-panel.md` §6.
    #[test]
    fn promoted_defaults_match_canonical_consts() {
        let g = GiSettings::default();
        // Mirror of `ray_tracing.wgsl:122-126`.
        assert_eq!(g.max_ray_steps_primary, 120);
        assert_eq!(g.max_ray_steps_secondary, 100);
        assert_eq!(g.max_ray_steps_sun, 120);
        assert_eq!(g.max_ray_steps_sun_secondary, 80);
        assert_eq!(g.max_ray_steps_visibility, 60);
        // Mirror of `spatial_resampling.wgsl::sample_neighbors` argument.
        assert_eq!(g.spatial_iter_count, 12);
    }

    /// At least one interactive knob exists (otherwise `first_interactive`
    /// returns 0 which would land on a section header — broken UX).
    #[test]
    fn at_least_one_interactive_knob() {
        let count = KNOBS.iter().filter(|k| k.kind.is_interactive()).count();
        assert!(count > 0, "no interactive knobs in the panel table");
        assert!(KNOBS[first_interactive()].kind.is_interactive());
    }

    /// The KNOBS table must end with the "Reset all" action row
    /// (`25-design-panel-mouse.md` §5.2) — guards against accidental row
    /// re-ordering that would silently drop the mouse-clickable reset.
    #[test]
    fn knobs_ends_with_reset_all_action() {
        let last = KNOBS.last().expect("KNOBS must not be empty");
        assert!(
            matches!(last.kind, KnobKind::Action { .. }),
            "expected the last KNOBS row to be a KnobKind::Action (the \"Reset all\" button); \
             got label {:?}",
            last.label,
        );
        assert!(
            last.label.to_lowercase().contains("reset"),
            "expected the Action row's label to mention \"reset\"; got {:?}",
            last.label,
        );
    }

    /// `reset_all_knobs` returns every knob to its declared default. Mutate
    /// each `Bool` to its inverse and each U32 to a sentinel, then reset and
    /// verify.
    #[test]
    fn reset_all_knobs_restores_defaults() {
        let mut args = AppArgs::default();
        // Mutate a few knobs.
        args.gi.max_ray_steps_primary = 7;
        args.gi.spatial_iter_count = 1;
        args.gi.is_denoise = false;
        args.gi.spatial_resample_size = 1.0;
        // Apply reset.
        reset_all_knobs(&mut args);
        // Verify the mutated knobs are restored.
        assert_eq!(args.gi.max_ray_steps_primary, 120);
        assert_eq!(args.gi.spatial_iter_count, 12);
        assert!(args.gi.is_denoise);
        assert!((args.gi.spatial_resample_size - 500.0).abs() < f32::EPSILON);
    }

    /// `DragState::default()` is `Idle` — the resource starts in a safe
    /// state so the first frame after `setup_panel` cannot accidentally
    /// process drag motion.
    #[test]
    fn drag_state_default_is_idle() {
        assert_eq!(PanelDrag::default().state, DragState::Idle);
    }
}
