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

pub mod canonical;

pub use canonical::GiSettings;

use std::fmt::Write;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::ui::{FocusPolicy, RelativeCursorPosition};
use bevy::window::PrimaryWindow;

use crate::editor::ui_theme::{
    text_style, BG_BACKDROP, BG_HEADING, BG_PANEL, BG_RESET, BG_RESET_HOVER, BG_ROW_HOVER,
    BG_ROW_SELECTED, BORDER_PANEL, FG_MUTED, FG_PRIMARY, FG_READONLY, FG_SECTION,
    FG_VALUE_SELECTED,
};
use crate::render::gi::{
    BUCKET_STORAGE_COUNT, INVALID_SAMPLE_STORAGE_COUNT, REFINED_BUCKET_STORAGE_COUNT,
    VALID_SAMPLE_STORAGE_COUNT,
};
use crate::render::taa::CAMERA_HISTORY_DEPTH;
use crate::{AppArgs, DevFont};

/// Drag-detection threshold in physical pixels.
const DRAG_THRESHOLD_PX: f32 = 2.0;

/// Drag sensitivity reference width in logical pixels — one full traversal
/// spans 1/8 of the knob's `[min..max]` range at base sensitivity.
const DRAG_FULL_RANGE_PX: f32 = 2560.0;

/// Shift-modifier multiplier on drag sensitivity (4× fine-grain).
const DRAG_SHIFT_FACTOR: f32 = 0.25;

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

/// Section header (non-interactive row).
macro_rules! knob_section {
    ($label:literal) => {
        Knob { label: $label, kind: KnobKind::Section }
    };
}

/// `u32` knob. `$field` is a single ident; the getter/setter closures
/// reference `GiSettings::$field` directly — a typo here is a compile error,
/// preserving the "compile-time field-exists" property of the hand-written
/// table. `default` reads from [`GiSettings::DEFAULTS`] (D7's canonical const)
/// — eliminates the literal-duplication that the per-row `default:` field
/// previously carried (HIGH-4).
macro_rules! knob_u32 {
    ($label:literal, $field:ident, nudge=$n:expr, big=$b:expr, min=$mn:expr, max=$mx:expr) => {
        Knob {
            label: $label,
            kind: KnobKind::U32 {
                getter: |g| g.$field,
                setter: |g, v| g.$field = v,
                nudge: $n,
                big_step: $b,
                min: $mn,
                max: $mx,
                default: GiSettings::DEFAULTS.$field,
            },
        }
    };
}

/// `f32` knob — see [`knob_u32!`] for semantics.
macro_rules! knob_f32 {
    ($label:literal, $field:ident, nudge=$n:expr, big=$b:expr, min=$mn:expr, max=$mx:expr) => {
        Knob {
            label: $label,
            kind: KnobKind::F32 {
                getter: |g| g.$field,
                setter: |g, v| g.$field = v,
                nudge: $n,
                big_step: $b,
                min: $mn,
                max: $mx,
                default: GiSettings::DEFAULTS.$field,
            },
        }
    };
}

/// `bool` knob — `default` sourced from [`GiSettings::DEFAULTS`].
macro_rules! knob_bool {
    ($label:literal, $field:ident) => {
        Knob {
            label: $label,
            kind: KnobKind::Bool {
                getter: |g| g.$field,
                setter: |g, v| g.$field = v,
                default: GiSettings::DEFAULTS.$field,
            },
        }
    };
}

/// Read-only diagnostic row — the closure formats the displayed value from
/// `&AppArgs`.
macro_rules! knob_readonly {
    ($label:literal, $expr:expr) => {
        Knob { label: $label, kind: KnobKind::Readonly { value: $expr } }
    };
}

/// Action row — the closure is invoked on click / `R` reset.
macro_rules! knob_action {
    ($label:literal, $fn:expr) => {
        Knob { label: $label, kind: KnobKind::Action { apply: $fn } }
    };
}

/// The full knob table. Section headers + readonly diagnostics + interactive
/// knobs + the bottom "Reset all" action.
const KNOBS: &[Knob] = &[
    knob_section!("RAY STEP CAPS"),
    knob_u32!("  primary",        max_ray_steps_primary,        nudge=8, big=32, min=1, max=512),
    knob_u32!("  secondary",      max_ray_steps_secondary,      nudge=8, big=32, min=1, max=512),
    knob_u32!("  sun",            max_ray_steps_sun,            nudge=8, big=32, min=1, max=512),
    knob_u32!("  sun-secondary",  max_ray_steps_sun_secondary,  nudge=8, big=32, min=1, max=512),
    knob_u32!("  visibility",     max_ray_steps_visibility,     nudge=8, big=32, min=1, max=512),

    knob_section!("SPATIAL RESAMPLING"),
    knob_u32!("  iter count",      spatial_iter_count,       nudge=1,    big=4,     min=1,    max=32),
    knob_u32!("  sun_shadow_taps", sun_shadow_taps,          nudge=1,    big=4,     min=1,    max=32),
    knob_f32!("  resample_size",   spatial_resample_size,    nudge=50.0, big=200.0, min=32.0, max=2000.0),
    knob_f32!("  radius_lit_factor", radius_lit_factor,      nudge=0.5,  big=3.0,   min=0.0,  max=1000.0),
    knob_f32!("  noise_suppress",  noise_suppression_factor, nudge=0.05, big=0.5,   min=0.01, max=100.0),

    knob_section!("GI"),
    knob_u32!("  bounce_count",   bounce_count,   nudge=1,    big=1,     min=1,   max=3),
    knob_f32!("  denoise_thresh", denoise_thresh, nudge=50.0, big=200.0, min=0.0, max=2000.0),
    knob_bool!("  is_denoise",         is_denoise),
    knob_bool!("  is_sample_leveling", is_sample_leveling),
    knob_bool!("  is_varying_radius",  is_varying_resampling_radius),
    knob_bool!("  is_atmosphere_int",  is_atmosphere_interaction),
    knob_bool!("  skip_samples",       skip_samples),

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
            BackgroundColor(BG_BACKDROP),
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
            BackgroundColor(BG_PANEL),
            BorderColor::all(BORDER_PANEL),
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
            BackgroundColor(BG_HEADING),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("QUALITY SETTINGS"),
                text_style(&dev_font, FG_SECTION, 16.0),
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
                text_style(&dev_font, FG_PRIMARY, 13.0),
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
                text_style(&dev_font, FG_MUTED, 11.0),
            ));
        })
        .id();
    commands.entity(root).add_child(legend);
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
                *text_color = TextColor(FG_SECTION);
            }
            KnobKind::U32 { getter, .. } => {
                let _ = write!(s, "{}{:<24} {:>8}", marker, row.label, getter(&args.gi));
                *text_color = TextColor(if i == state.cursor { FG_VALUE_SELECTED } else { FG_PRIMARY });
            }
            KnobKind::F32 { getter, .. } => {
                let _ = write!(s, "{}{:<24} {:>8.2}", marker, row.label, getter(&args.gi));
                *text_color = TextColor(if i == state.cursor { FG_VALUE_SELECTED } else { FG_PRIMARY });
            }
            KnobKind::Bool { getter, .. } => {
                let mark = if getter(&args.gi) { " ON" } else { "OFF" };
                let _ = write!(s, "{}{:<24} {:>8}", marker, row.label, mark);
                *text_color = TextColor(if i == state.cursor { FG_VALUE_SELECTED } else { FG_PRIMARY });
            }
            KnobKind::Readonly { value } => {
                let _ = write!(s, "{}{:<24} {}", marker, row.label, value(&args));
                *text_color = TextColor(FG_READONLY);
            }
            KnobKind::Action { .. } => {
                let _ = write!(s, "{}      {}", marker, row.label);
                *text_color = TextColor(FG_PRIMARY);
            }
        }
    }

    for (row, interaction, mut bg) in &mut row_bgs {
        let Some(k) = KNOBS.get(row.knob_index) else { continue };
        if matches!(k.kind, KnobKind::Action { .. }) {
            // The reset-all row uses a distinct color scheme.
            let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
            *bg = BackgroundColor(if hovered { BG_RESET_HOVER } else { BG_RESET });
            continue;
        }
        let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        let target = if row.knob_index == state.cursor && k.kind.is_interactive() {
            BG_ROW_SELECTED
        } else if hovered {
            BG_ROW_HOVER
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

/// Plugin owning the Escape settings overlay. Prepared by D2 (codebase-
/// tightening side-note 11) ahead of D7's `lib.rs` decomposition; D7 wires
/// this via `app.add_plugins(SettingsPlugin)` in place of the inline
/// registration block at `lib.rs:900-971`. `setup_settings` runs after
/// `setup_editor_hud` to land later in the UI document order (same
/// belt-and-suspenders ordering the inline block established).
pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        use crate::app_mode::{restore_camera_input, suspend_camera_input, AppMode};
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
            .add_systems(
                OnEnter(AppMode::Settings),
                (show_settings, suspend_camera_input),
            )
            .add_systems(
                OnExit(AppMode::Settings),
                (hide_settings, restore_camera_input),
            )
            .add_systems(
                Update,
                (
                    adjust_settings.run_if(in_state(AppMode::Settings)),
                    mouse_interact_settings.run_if(in_state(AppMode::Settings)),
                    update_settings_text.run_if(in_state(AppMode::Settings)),
                )
                    .chain(),
            );
    }
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

    /// `GiSettings::default()` must equal `GiSettings::DEFAULTS` (D7's
    /// canonical const). After the `knob_*!` macros source their `default:`
    /// field from `DEFAULTS.$field`, KNOBS-vs-DEFAULTS agreement is by
    /// construction; this single round-trip pins `default()` to that same
    /// const so the `R` (reset row) panel action stays bit-equivalent to a
    /// fresh `GiSettings::default()`.
    #[test]
    fn defaults_match_gi_settings_default() {
        assert_eq!(GiSettings::default(), GiSettings::DEFAULTS);
    }

    /// Canonical ray-step-cap values from `WorldRenderBase.cs:14-25` + the C#
    /// `MAX_RAY_STEPS_*` consts. Pins `GiSettings::DEFAULTS` against the
    /// reference values — drift here is a port-vs-C# divergence that the
    /// faithful-port rule forbids.
    #[test]
    fn promoted_defaults_match_canonical_consts() {
        assert_eq!(GiSettings::DEFAULTS.max_ray_steps_primary, 120);
        assert_eq!(GiSettings::DEFAULTS.max_ray_steps_secondary, 100);
        assert_eq!(GiSettings::DEFAULTS.max_ray_steps_sun, 120);
        assert_eq!(GiSettings::DEFAULTS.max_ray_steps_sun_secondary, 80);
        assert_eq!(GiSettings::DEFAULTS.max_ray_steps_visibility, 60);
        assert_eq!(GiSettings::DEFAULTS.spatial_iter_count, 12);
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
