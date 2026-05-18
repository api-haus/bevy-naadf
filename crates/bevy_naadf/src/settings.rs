//! Escape settings overlay — the raymarching-quality tuner.
//!
//! Centered modal panel with a full-screen darkening backdrop. Toggled by
//! the global [`crate::app_mode::AppMode`] state — pressing Escape flips
//! `Playing ↔ Settings`; this module's visibility systems are bound to
//! `OnEnter(AppMode::Settings)` / `OnExit(AppMode::Settings)`. While open,
//! the camera entity carries `Disabled` (via `DisableOnEnter(Settings)` on
//! the camera spawn) so `FreeCameraPlugin` skips it, and brush input is
//! gated by `.run_if(in_state(AppMode::Playing))` on `apply_edit_tool`.
//!
//! The knobs table mirrors the GI/raymarching parameters from the old F1
//! panel. Editor-specific knobs (tool / radius / erase / continuous /
//! selected_type) have moved into the in-game editor HUD
//! (`crate::editor::hud`) so the settings overlay is purely "engine quality".
//!
//! Both keyboard navigation and mouse drag-sliders are live concurrently,
//! mutating `AppArgs.gi`.

use std::fmt::Write;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::ui::{FocusPolicy, RelativeCursorPosition};
use bevy::window::PrimaryWindow;

use crate::render::gi::{
    BUCKET_STORAGE_COUNT, INVALID_SAMPLE_STORAGE_COUNT, REFINED_BUCKET_STORAGE_COUNT,
    VALID_SAMPLE_STORAGE_COUNT,
};
use crate::render::taa::CAMERA_HISTORY_DEPTH;
use crate::{AppArgs, DevFont, GiSettings, DEFAULT_TAA_RING_DEPTH};

/// Drag-detection threshold in physical pixels.
const DRAG_THRESHOLD_PX: f32 = 2.0;

/// Drag sensitivity reference width in logical pixels — one full traversal
/// spans 1/8 of the knob's `[min..max]` range at base sensitivity.
const DRAG_FULL_RANGE_PX: f32 = 2560.0;

/// Shift-modifier multiplier on drag sensitivity (4× fine-grain).
const DRAG_SHIFT_FACTOR: f32 = 0.25;

// Visual chrome.
const COL_BACKDROP: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);
const COL_PANEL_BG: Color = Color::srgba(0.06, 0.06, 0.09, 0.96);
const COL_PANEL_BORDER: Color = Color::srgba(0.35, 0.35, 0.42, 1.0);
const COL_HEADING_BG: Color = Color::srgba(0.10, 0.12, 0.18, 1.0);
const COL_SECTION: Color = Color::srgba(0.55, 0.85, 0.95, 1.0);
const COL_ROW_HOVER: Color = Color::srgba(1.0, 1.0, 1.0, 0.05);
const COL_ROW_SELECTED: Color = Color::srgba(1.0, 0.85, 0.30, 0.18);
const COL_VALUE: Color = Color::WHITE;
const COL_VALUE_SEL: Color = Color::srgba(1.0, 1.0, 0.6, 1.0);
const COL_READONLY: Color = Color::srgba(0.55, 0.55, 0.55, 1.0);
const COL_RESET_BG: Color = Color::srgba(0.65, 0.20, 0.20, 1.0);
const COL_RESET_BG_HOVER: Color = Color::srgba(0.85, 0.30, 0.30, 1.0);

/// Main-world resource: which knob row the keyboard cursor is on.
#[derive(Resource, Debug, Clone, Copy)]
pub struct SettingsState {
    /// Currently-selected knob index into [`KNOBS`]. Range
    /// `0..KNOBS.len()`. Only interactive rows step the cursor.
    pub cursor: usize,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self { cursor: 0 }
    }
}

/// Drag state machine.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DragState {
    #[default]
    Idle,
    Pressed { knob_index: usize, total_motion: f32 },
    Dragging { knob_index: usize, frac_accum: f32 },
}

/// Main-world resource: the drag state machine.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct SettingsDrag {
    pub state: DragState,
}

/// Marker for the full-screen darkening backdrop (sibling of the panel root).
#[derive(Component)]
pub struct SettingsBackdrop;

/// Marker for the centered panel root.
#[derive(Component)]
pub struct SettingsRoot;

/// Marker + index for each per-row Node.
#[derive(Component, Clone, Copy)]
pub struct SettingsRow {
    pub knob_index: usize,
}

/// Marker for the per-row Text entity.
#[derive(Component)]
pub struct SettingsRowText {
    pub knob_index: usize,
}

/// Marker for the bottom legend Text entity.
#[derive(Component)]
pub struct SettingsLegendText;

/// One knob descriptor.
struct Knob {
    label: &'static str,
    class: char,
    kind: KnobKind,
}

#[allow(clippy::type_complexity)]
enum KnobKind {
    Section,
    U32 {
        getter: fn(&GiSettings) -> u32,
        setter: fn(&mut GiSettings, u32),
        nudge: u32,
        big_step: u32,
        min: u32,
        max: u32,
        default: u32,
    },
    F32 {
        getter: fn(&GiSettings) -> f32,
        setter: fn(&mut GiSettings, f32),
        nudge: f32,
        big_step: f32,
        min: f32,
        max: f32,
        default: f32,
    },
    Bool {
        getter: fn(&GiSettings) -> bool,
        setter: fn(&mut GiSettings, bool),
        default: bool,
    },
    Readonly {
        value: fn(&AppArgs) -> String,
    },
    Action {
        apply: fn(&mut AppArgs),
    },
}

impl KnobKind {
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

/// The full knob table. Section headers + readonly diagnostics + interactive
/// knobs + the bottom "Reset all" action.
const KNOBS: &[Knob] = &[
    Knob { label: "RAY STEP CAPS", class: ' ', kind: KnobKind::Section },
    Knob {
        label: "  primary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_primary,
            setter: |g, v| g.max_ray_steps_primary = v,
            nudge: 8, big_step: 32, min: 1, max: 512, default: 120,
        },
    },
    Knob {
        label: "  secondary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_secondary,
            setter: |g, v| g.max_ray_steps_secondary = v,
            nudge: 8, big_step: 32, min: 1, max: 512, default: 100,
        },
    },
    Knob {
        label: "  sun",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_sun,
            setter: |g, v| g.max_ray_steps_sun = v,
            nudge: 8, big_step: 32, min: 1, max: 512, default: 120,
        },
    },
    Knob {
        label: "  sun-secondary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_sun_secondary,
            setter: |g, v| g.max_ray_steps_sun_secondary = v,
            nudge: 8, big_step: 32, min: 1, max: 512, default: 80,
        },
    },
    Knob {
        label: "  visibility",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_visibility,
            setter: |g, v| g.max_ray_steps_visibility = v,
            nudge: 8, big_step: 32, min: 1, max: 512, default: 60,
        },
    },
    Knob { label: "SPATIAL RESAMPLING", class: ' ', kind: KnobKind::Section },
    Knob {
        label: "  iter count",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.spatial_iter_count,
            setter: |g, v| g.spatial_iter_count = v,
            nudge: 1, big_step: 4, min: 1, max: 32, default: 12,
        },
    },
    Knob {
        label: "  sun_shadow_taps",
        class: 'C',
        kind: KnobKind::U32 {
            getter: |g| g.sun_shadow_taps,
            setter: |g, v| g.sun_shadow_taps = v,
            nudge: 1, big_step: 4, min: 1, max: 32, default: 1,
        },
    },
    Knob {
        label: "  resample_size",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.spatial_resample_size,
            setter: |g, v| g.spatial_resample_size = v,
            nudge: 50.0, big_step: 200.0, min: 32.0, max: 2000.0, default: 500.0,
        },
    },
    Knob {
        label: "  radius_lit_factor",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.radius_lit_factor,
            setter: |g, v| g.radius_lit_factor = v,
            nudge: 0.5, big_step: 3.0, min: 0.0, max: 1000.0, default: 3.0,
        },
    },
    Knob {
        label: "  noise_suppress",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.noise_suppression_factor,
            setter: |g, v| g.noise_suppression_factor = v,
            nudge: 0.05, big_step: 0.5, min: 0.01, max: 100.0, default: 0.4,
        },
    },
    Knob { label: "GI", class: ' ', kind: KnobKind::Section },
    Knob {
        label: "  bounce_count",
        class: 'C',
        kind: KnobKind::U32 {
            getter: |g| g.bounce_count,
            setter: |g, v| g.bounce_count = v,
            nudge: 1, big_step: 1, min: 1, max: 3, default: 3,
        },
    },
    Knob {
        label: "  denoise_thresh",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.denoise_thresh,
            setter: |g, v| g.denoise_thresh = v,
            nudge: 50.0, big_step: 200.0, min: 0.0, max: 2000.0, default: 400.0,
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
    Knob { label: "DIAGNOSTICS (read-only)", class: ' ', kind: KnobKind::Section },
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
    Knob {
        label: "> RESET ALL TO DEFAULTS <",
        class: 'B',
        kind: KnobKind::Action { apply: reset_all_knobs },
    },
];

/// Apply every knob's `default` to `AppArgs.gi`.
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

fn first_interactive() -> usize {
    KNOBS.iter().position(|k| k.kind.is_interactive()).unwrap_or(0)
}

/// `Startup` system — spawn the full-screen backdrop with the centered panel
/// nested inside. Starts hidden; the `OnEnter(Settings)` system reveals it.
///
/// Layout: the backdrop is itself a flex container (`align/justify: center`)
/// that holds the panel as its only child. This gives us reliable centering
/// (no margin tricks) AND guaranteed z-order — the panel renders on top of
/// its parent's background fill.
pub fn setup_settings(mut commands: Commands, dev_font: Res<DevFont>) {
    // The backdrop is a full-screen flex container that centers its single
    // child (the panel root). Spawned last in `Startup` (see `lib.rs`), so
    // in document order it sits above the editor HUD — no GlobalZIndex
    // needed. GlobalZIndex was previously tried here but Bevy treats a child
    // of a `GlobalZIndex` node as its own stack-root for render purposes,
    // which broke the nested layout. Plain document-order stacking works.
    let backdrop = commands
        .spawn((
            SettingsBackdrop,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(COL_BACKDROP),
            FocusPolicy::Block,
            Interaction::default(),
        ))
        .id();

    let root = commands
        .spawn((
            SettingsRoot,
            Node {
                width: px(680.0),
                min_height: px(120.0),
                padding: px(16.0).all(),
                flex_direction: FlexDirection::Column,
                row_gap: px(2.0),
                border: px(2.0).all(),
                ..default()
            },
            BackgroundColor(COL_PANEL_BG),
            BorderColor::all(COL_PANEL_BORDER),
            FocusPolicy::Block,
        ))
        .id();
    commands.entity(backdrop).add_child(root);
    info!(target: "settings", "setup_settings spawned backdrop={:?} root={:?}", backdrop, root);

    // Heading.
    let heading = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                padding: UiRect::axes(px(8.0), px(6.0)),
                margin: UiRect {
                    bottom: px(8.0),
                    ..default()
                },
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(COL_HEADING_BG),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("QUALITY SETTINGS"),
                TextColor(COL_SECTION),
                TextFont {
                    font: dev_font.0.clone(),
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
            ));
        })
        .id();
    commands.entity(root).add_child(heading);

    // One Node per KNOBS row.
    for (i, knob) in KNOBS.iter().enumerate() {
        let mut row_cmd = commands.spawn((
            SettingsRow { knob_index: i },
            Node {
                width: Val::Percent(100.0),
                padding: UiRect::axes(px(6.0), px(2.0)),
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
                SettingsRowText { knob_index: i },
                Text::default(),
                TextColor(COL_VALUE),
                TextFont {
                    font: dev_font.0.clone(),
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
            ))
            .id();
        commands.entity(row).add_child(text);
        commands.entity(root).add_child(row);
    }

    // Bottom legend.
    let legend = commands
        .spawn((
            Node {
                margin: UiRect {
                    top: px(10.0),
                    ..default()
                },
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                SettingsLegendText,
                Text::default(),
                TextColor(Color::srgba(0.65, 0.65, 0.70, 1.0)),
                TextFont {
                    font: dev_font.0.clone(),
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
            ));
        })
        .id();
    commands.entity(root).add_child(legend);

    // Keep the const re-export-still-used compiler check warm.
    let _ = DEFAULT_TAA_RING_DEPTH;
}

/// `OnEnter(AppMode::Settings)` system — show the backdrop (the root is a
/// child and inherits visibility).
pub fn show_settings(
    mut backdrop: Query<&mut Node, With<SettingsBackdrop>>,
    mut state: ResMut<SettingsState>,
) {
    let mut shown = false;
    if let Ok(mut node) = backdrop.single_mut() {
        node.display = Display::Flex;
        shown = true;
    }
    if !KNOBS.get(state.cursor).map(|k| k.kind.is_interactive()).unwrap_or(false) {
        state.cursor = first_interactive();
    }
    info!(target: "settings", "show_settings fired (backdrop found = {shown})");
}

/// `OnExit(AppMode::Settings)` system — hide the backdrop + abort any drag.
pub fn hide_settings(
    mut backdrop: Query<&mut Node, With<SettingsBackdrop>>,
    mut drag: ResMut<SettingsDrag>,
) {
    let mut hidden = false;
    if let Ok(mut node) = backdrop.single_mut() {
        node.display = Display::None;
        hidden = true;
    }
    drag.state = DragState::Idle;
    info!(target: "settings", "hide_settings fired (backdrop found = {hidden})");
}

/// `Update` system — keyboard navigation while the overlay is open + no drag.
/// Gated by `.run_if(in_state(AppMode::Settings))` in `lib.rs`.
pub fn adjust_settings(
    keys: Res<ButtonInput<KeyCode>>,
    drag: Res<SettingsDrag>,
    mut state: ResMut<SettingsState>,
    mut args: ResMut<AppArgs>,
) {
    if !matches!(drag.state, DragState::Idle) {
        return;
    }

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    if keys.just_pressed(KeyCode::ArrowUp) {
        state.cursor = step_cursor(state.cursor, -1);
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        state.cursor = step_cursor(state.cursor, 1);
    }

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
                if left { v = v.saturating_sub(n_step); }
                if right { v = v.saturating_add(n_step); }
                if big_left { v = v.saturating_sub(big_step); }
                if big_right { v = v.saturating_add(big_step); }
                if reset_one { v = default; }
                setter(&mut args.gi, v.clamp(min, max));
            }
            KnobKind::F32 { getter, setter, nudge, big_step, min, max, default } => {
                let mut v = getter(&args.gi);
                let n_step = if shift { nudge / 4.0 } else { nudge };
                if left { v -= n_step; }
                if right { v += n_step; }
                if big_left { v -= big_step; }
                if big_right { v += big_step; }
                if reset_one { v = default; }
                setter(&mut args.gi, v.clamp(min, max));
            }
            KnobKind::Bool { getter, setter, default } => {
                let mut v = getter(&args.gi);
                if left || right { v = !v; }
                if reset_one { v = default; }
                setter(&mut args.gi, v);
            }
            KnobKind::Action { apply } => {
                if reset_one {
                    apply(&mut args);
                }
            }
            _ => {}
        }
    }
}

/// `Update` system — mouse drag-sliders + click flips. Gated by
/// `.run_if(in_state(AppMode::Settings))`.
pub fn mouse_interact_settings(
    mut state: ResMut<SettingsState>,
    mut drag: ResMut<SettingsDrag>,
    mut args: ResMut<AppArgs>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rows: Query<(&SettingsRow, &Interaction)>,
) {
    // Hover-follows-cursor while idle.
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

    let pressed_row: Option<usize> = rows
        .iter()
        .find(|(_, i)| **i == Interaction::Pressed)
        .map(|(r, _)| r.knob_index);

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let raw_dx = motion.delta.x;
    let shift_factor = if shift { DRAG_SHIFT_FACTOR } else { 1.0 };

    match drag.state {
        DragState::Idle => {
            if let Some(knob_index) = pressed_row {
                drag.state = DragState::Pressed { knob_index, total_motion: 0.0 };
            }
        }
        DragState::Pressed { knob_index, mut total_motion } => {
            total_motion += raw_dx.abs();
            if total_motion >= DRAG_THRESHOLD_PX {
                drag.state = DragState::Dragging { knob_index, frac_accum: 0.0 };
                apply_drag_delta(&mut args, &mut drag, knob_index, raw_dx, shift_factor, window_scale(&window));
            } else if mouse_buttons.just_released(MouseButton::Left) {
                handle_click_release(&mut args, knob_index);
                drag.state = DragState::Idle;
            } else {
                drag.state = DragState::Pressed { knob_index, total_motion };
            }
        }
        DragState::Dragging { knob_index, .. } => {
            if mouse_buttons.just_released(MouseButton::Left) {
                drag.state = DragState::Idle;
            } else {
                apply_drag_delta(&mut args, &mut drag, knob_index, raw_dx, shift_factor, window_scale(&window));
            }
        }
    }
}

fn window_scale(window: &Query<&Window, With<PrimaryWindow>>) -> f32 {
    window.iter().next().map(|w| w.scale_factor()).unwrap_or(1.0)
}

fn apply_drag_delta(
    args: &mut AppArgs,
    drag: &mut SettingsDrag,
    knob_index: usize,
    raw_dx: f32,
    shift_factor: f32,
    scale_factor: f32,
) {
    let Some(row) = KNOBS.get(knob_index) else { return };
    let full_range_px = DRAG_FULL_RANGE_PX * scale_factor;
    if full_range_px <= 0.0 { return; }
    let dx = raw_dx * shift_factor;

    match row.kind {
        KnobKind::U32 { getter, setter, min, max, .. } => {
            let range = (max as f32 - min as f32).max(1.0);
            let delta_f = dx * (range / full_range_px);
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
            drag.state = DragState::Dragging { knob_index, frac_accum };
        }
        KnobKind::F32 { getter, setter, min, max, .. } => {
            let range = (max - min).max(f32::EPSILON);
            let delta = dx * (range / full_range_px);
            let cur = getter(&args.gi);
            setter(&mut args.gi, (cur + delta).clamp(min, max));
        }
        _ => {}
    }
}

fn handle_click_release(args: &mut AppArgs, knob_index: usize) {
    let Some(row) = KNOBS.get(knob_index) else { return };
    match row.kind {
        KnobKind::Bool { getter, setter, .. } => {
            let v = getter(&args.gi);
            setter(&mut args.gi, !v);
        }
        KnobKind::Action { apply } => apply(args),
        _ => {}
    }
}

/// `Update` system — rewrite each row's Text content + row background color.
/// Gated by `.run_if(in_state(AppMode::Settings))`.
pub fn update_settings_text(
    state: Res<SettingsState>,
    args: Res<AppArgs>,
    mut row_texts: Query<(&SettingsRowText, &mut Text, &mut TextColor)>,
    mut row_bgs: Query<(&SettingsRow, &Interaction, &mut BackgroundColor)>,
    mut legend: Query<&mut Text, (With<SettingsLegendText>, Without<SettingsRowText>)>,
) {
    for (row_text, mut text, mut text_color) in &mut row_texts {
        let i = row_text.knob_index;
        let Some(row) = KNOBS.get(i) else { continue };
        let s = &mut text.0;
        s.clear();
        let marker = if i == state.cursor && row.kind.is_interactive() { "> " } else { "  " };
        match &row.kind {
            KnobKind::Section => {
                let _ = write!(s, "  {}", row.label);
                *text_color = TextColor(COL_SECTION);
            }
            KnobKind::U32 { getter, .. } => {
                let _ = write!(s, "{}{:<24} {:>8} [{}]", marker, row.label, getter(&args.gi), row.class);
                *text_color = TextColor(if i == state.cursor { COL_VALUE_SEL } else { COL_VALUE });
            }
            KnobKind::F32 { getter, .. } => {
                let _ = write!(s, "{}{:<24} {:>8.2} [{}]", marker, row.label, getter(&args.gi), row.class);
                *text_color = TextColor(if i == state.cursor { COL_VALUE_SEL } else { COL_VALUE });
            }
            KnobKind::Bool { getter, .. } => {
                let mark = if getter(&args.gi) { " ON" } else { "OFF" };
                let _ = write!(s, "{}{:<24} {:>8} [{}]", marker, row.label, mark, row.class);
                *text_color = TextColor(if i == state.cursor { COL_VALUE_SEL } else { COL_VALUE });
            }
            KnobKind::Readonly { value } => {
                let _ = write!(s, "{}{:<24} {} [{}]", marker, row.label, value(&args), row.class);
                *text_color = TextColor(COL_READONLY);
            }
            KnobKind::Action { .. } => {
                let _ = write!(s, "{}      {}", marker, row.label);
                *text_color = TextColor(COL_VALUE);
            }
        }
    }

    for (row, interaction, mut bg) in &mut row_bgs {
        let Some(k) = KNOBS.get(row.knob_index) else { continue };
        if matches!(k.kind, KnobKind::Action { .. }) {
            // The reset-all row uses a distinct color scheme.
            let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
            *bg = BackgroundColor(if hovered { COL_RESET_BG_HOVER } else { COL_RESET_BG });
            continue;
        }
        let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        let target = if row.knob_index == state.cursor && k.kind.is_interactive() {
            COL_ROW_SELECTED
        } else if hovered {
            COL_ROW_HOVER
        } else {
            Color::NONE
        };
        *bg = BackgroundColor(target);
    }

    if let Ok(mut text) = legend.single_mut() {
        let s = &mut text.0;
        s.clear();
        let _ = write!(
            s,
            "[Up/Down] navigate  [Left/Right] adjust  [PgUp/PgDn] big  [Shift+L/R] fine\n\
             [R] reset row  [Shift+R] reset all  [Esc] resume    Mouse: drag rows, click toggles",
        );
    }
}

fn step_cursor(cur: usize, delta: i32) -> usize {
    let n = KNOBS.len();
    if n == 0 { return 0; }
    let mut i = cur as i32;
    for _ in 0..n {
        i += delta;
        if i < 0 { i = (n as i32) - 1; }
        if i >= n as i32 { i = 0; }
        let ui = i as usize;
        if KNOBS[ui].kind.is_interactive() { return ui; }
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_skips_non_interactive_rows() {
        for start in 0..KNOBS.len() {
            let next = step_cursor(start, 1);
            assert!(
                KNOBS[next].kind.is_interactive(),
                "cursor landed on non-interactive row from start={start}, next={next}",
            );
            let prev = step_cursor(start, -1);
            assert!(
                KNOBS[prev].kind.is_interactive(),
                "cursor landed on non-interactive row from start={start}, prev={prev}",
            );
        }
    }

    /// All defaults in the knob table MUST match `GiSettings::default()` —
    /// bit-equivalence promise; a drift here is a panel-vs-default mismatch
    /// that would silently change behaviour on `R` (reset row).
    #[test]
    fn defaults_match_gi_settings_default() {
        let g = GiSettings::default();
        for row in KNOBS {
            match row.kind {
                KnobKind::U32 { getter, default, .. } => {
                    assert_eq!(getter(&g), default, "u32 knob {:?} default mismatch", row.label);
                }
                KnobKind::F32 { getter, default, .. } => {
                    assert!((getter(&g) - default).abs() < f32::EPSILON, "f32 knob {:?} default mismatch", row.label);
                }
                KnobKind::Bool { getter, default, .. } => {
                    assert_eq!(getter(&g), default, "bool knob {:?} default mismatch", row.label);
                }
                _ => {}
            }
        }
    }

    /// Class-P ray-step-cap defaults must equal the WGSL `MAX_RAY_STEPS_*`
    /// consts the promotions replaced.
    #[test]
    fn promoted_defaults_match_canonical_consts() {
        let g = GiSettings::default();
        assert_eq!(g.max_ray_steps_primary, 120);
        assert_eq!(g.max_ray_steps_secondary, 100);
        assert_eq!(g.max_ray_steps_sun, 120);
        assert_eq!(g.max_ray_steps_sun_secondary, 80);
        assert_eq!(g.max_ray_steps_visibility, 60);
        assert_eq!(g.spatial_iter_count, 12);
    }

    #[test]
    fn at_least_one_interactive_knob() {
        let count = KNOBS.iter().filter(|k| k.kind.is_interactive()).count();
        assert!(count > 0);
        assert!(KNOBS[first_interactive()].kind.is_interactive());
    }

    /// The KNOBS table must end with the "Reset all" action row.
    #[test]
    fn knobs_ends_with_reset_all_action() {
        let last = KNOBS.last().expect("KNOBS must not be empty");
        assert!(matches!(last.kind, KnobKind::Action { .. }));
        assert!(last.label.to_lowercase().contains("reset"));
    }

    #[test]
    fn reset_all_knobs_restores_defaults() {
        let mut args = AppArgs::default();
        args.gi.max_ray_steps_primary = 7;
        args.gi.spatial_iter_count = 1;
        args.gi.is_denoise = false;
        args.gi.spatial_resample_size = 1.0;
        reset_all_knobs(&mut args);
        assert_eq!(args.gi.max_ray_steps_primary, 120);
        assert_eq!(args.gi.spatial_iter_count, 12);
        assert!(args.gi.is_denoise);
        assert!((args.gi.spatial_resample_size - 500.0).abs() < f32::EPSILON);
    }

    #[test]
    fn drag_state_default_is_idle() {
        assert_eq!(SettingsDrag::default().state, DragState::Idle);
    }
}
