//! Editor HUD — the gamified in-game editing strip.
//!
//! Layout:
//! - **Bottom strip** (`EditorHudRoot`) — always visible. Three rows:
//!   1. Tool buttons (Paint / Cube / Sphere) with Unicode glyphs.
//!   2. Brush radius slider + Erase toggle + Continuous toggle.
//!   3. Horizontally-scrollable voxel-type palette — one swatch per non-empty
//!      entry in [`crate::world::data::VoxelTypes::types`], colored by its
//!      `color_base`.
//! - **Top-right hover-info** (`HoverInfoText`) — compact 2-line readout of the
//!   last `RayHit`. Hidden while no hit.
//!
//! All interactive elements use `Interaction` for hit-testing. The brush
//! dispatcher (`apply_edit_tool`) early-returns when any HUD interactive is
//! hovered/pressed, so HUD clicks don't double-fire as world paints.

use std::fmt::Write;

use bevy::ecs::message::MessageReader;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, FocusPolicy, RelativeCursorPosition, ScrollPosition};
use bevy::window::PrimaryWindow;

use crate::editor::{EditTool, EditorState};
use crate::voxel::VoxelTypeId;
use crate::world::data::VoxelTypes;
use crate::DevFont;

/// Marker for the HUD root Node (the bottom strip container). Carries
/// `Interaction` so `apply_edit_tool` can bail when the cursor is over it.
#[derive(Component)]
pub struct EditorHudRoot;

/// Marker for any interactive descendant of [`EditorHudRoot`] — tool buttons,
/// palette swatches, the radius slider, the toggle buttons. The brush
/// dispatcher iterates these to decide whether HUD owns the click.
#[derive(Component)]
pub struct EditorHudInteractive;

/// Per-tool-button marker carrying which [`EditTool`] the button selects.
#[derive(Component, Clone, Copy)]
pub struct ToolButton(pub EditTool);

/// Per-swatch marker carrying which [`VoxelTypeId`] the swatch selects.
#[derive(Component, Clone, Copy)]
pub struct PaletteSwatch(pub VoxelTypeId);

/// Marker for the radius slider's track Node (mouse drag fills the bar).
#[derive(Component)]
pub struct RadiusSlider;

/// Marker for the radius slider's value-fill Node (child of [`RadiusSlider`],
/// width is rewritten each frame as `radius / RADIUS_MAX`).
#[derive(Component)]
pub struct RadiusSliderFill;

/// Marker for the radius slider's value-label Text.
#[derive(Component)]
pub struct RadiusSliderLabel;

/// Marker for the "Erase" toggle button. Click → flips `EditorState.is_erase`.
#[derive(Component)]
pub struct EraseToggle;

/// Marker for the "Continuous" toggle button. Click → flips
/// `EditorState.is_continuous`.
#[derive(Component)]
pub struct ContinuousToggle;

/// Marker for the top-right hover-info Text entity.
#[derive(Component)]
pub struct HoverInfoText;

/// Marker for the PBR-debug overlay Text entity (top-left). Shows
/// "Debug: <mode name>" when `DebugViewState.mode != Off`; hidden
/// otherwise. See [`update_debug_view_hud`].
#[derive(Component)]
pub struct DebugViewHudText;

/// Marker for the palette viewport (Node with `overflow_x = scroll`). Holds
/// the swatch strip as a single child.
#[derive(Component)]
pub struct PaletteViewport;

/// Marker for the palette strip — the inner flex row that holds each
/// `PaletteSwatch`. `refresh_palette_swatches` writes its children.
#[derive(Component)]
pub struct PaletteStrip;

/// Marker for the palette scrollbar track (sits below the palette viewport).
/// Clicking / dragging anywhere on the track scrolls the palette.
#[derive(Component)]
pub struct PaletteScrollbarTrack;

/// Marker for the scrollbar thumb (child of the track). Width = visible/content
/// fraction; horizontal position = scroll_offset / max_scroll fraction.
#[derive(Component)]
pub struct PaletteScrollbarThumb;

/// Brush radius UI range (matches the old panel knob: 1..400 voxels).
const RADIUS_MIN: f32 = 1.0;
const RADIUS_MAX: f32 = 400.0;

/// HUD geometry — tweak these to retune the strip without touching layout code.
const STRIP_BOTTOM_PX: f32 = 16.0;
const STRIP_PAD_PX: f32 = 12.0;
const TOOL_BUTTON_SIZE_PX: f32 = 52.0;
const SWATCH_SIZE_PX: f32 = 28.0;
const SWATCH_GAP_PX: f32 = 4.0;
const SLIDER_WIDTH_PX: f32 = 220.0;
const SLIDER_HEIGHT_PX: f32 = 22.0;

const COL_HUD_BG: Color = Color::srgba(0.05, 0.05, 0.08, 0.82);
const COL_BTN_BG: Color = Color::srgba(0.10, 0.10, 0.14, 1.0);
const COL_BTN_BG_HOVER: Color = Color::srgba(0.18, 0.18, 0.24, 1.0);
const COL_BTN_BG_SELECTED: Color = Color::srgba(0.95, 0.75, 0.20, 1.0);
const COL_BTN_BG_DISABLED: Color = Color::srgba(0.08, 0.08, 0.10, 1.0);
const COL_BTN_BORDER: Color = Color::srgba(0.30, 0.30, 0.34, 1.0);
const COL_BTN_BORDER_SELECTED: Color = Color::srgba(1.0, 0.85, 0.30, 1.0);
const COL_TEXT_PRIMARY: Color = Color::WHITE;
const COL_TEXT_MUTED: Color = Color::srgba(0.65, 0.65, 0.70, 1.0);
const COL_TEXT_DISABLED: Color = Color::srgba(0.35, 0.35, 0.38, 1.0);
const COL_SLIDER_TRACK: Color = Color::srgba(0.10, 0.10, 0.14, 1.0);
const COL_SLIDER_FILL: Color = Color::srgba(0.40, 0.65, 0.95, 1.0);
const COL_SWATCH_BORDER_SELECTED: Color = Color::WHITE;
const COL_SWATCH_BORDER: Color = Color::srgba(0.20, 0.20, 0.24, 1.0);
const COL_SCROLLBAR_TRACK: Color = Color::srgba(0.06, 0.06, 0.08, 1.0);
const COL_SCROLLBAR_THUMB: Color = Color::srgba(0.40, 0.40, 0.50, 1.0);

/// Scrollbar dimensions.
const SCROLLBAR_HEIGHT_PX: f32 = 8.0;
const SCROLLBAR_THUMB_MIN_PX: f32 = 24.0;
/// Pixels per wheel "line" notch — empirically reasonable for palette scroll.
const WHEEL_LINE_PX: f32 = 40.0;

/// `Startup` system — spawn the bottom HUD strip + the top-right hover-info
/// panel. Palette swatches are populated each frame by
/// [`refresh_palette_swatches`] from the live `VoxelTypes` resource.
pub fn setup_editor_hud(mut commands: Commands, dev_font: Res<DevFont>) {
    let strip = commands
        .spawn((
            EditorHudRoot,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(STRIP_BOTTOM_PX),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                padding: px(STRIP_PAD_PX).all(),
                flex_direction: FlexDirection::Column,
                row_gap: px(8.0),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
            FocusPolicy::Pass,
            Interaction::default(),
        ))
        .id();

    let panel = commands
        .spawn((
            Node {
                padding: px(10.0).all(),
                flex_direction: FlexDirection::Column,
                row_gap: px(6.0),
                align_items: AlignItems::Center,
                border: px(1.0).all(),
                ..default()
            },
            BackgroundColor(COL_HUD_BG),
            BorderColor::all(Color::srgba(0.0, 0.0, 0.0, 0.0)),
            FocusPolicy::Block,
        ))
        .id();
    commands.entity(strip).add_child(panel);

    // Row 1: tool buttons.
    let tools_row = spawn_h_row(&mut commands, 8.0);
    commands.entity(panel).add_child(tools_row);
    for tool in [EditTool::Paint, EditTool::Cube, EditTool::Sphere] {
        let button = spawn_tool_button(&mut commands, &dev_font, tool);
        commands.entity(tools_row).add_child(button);
    }

    // Row 2: radius slider + erase + continuous toggles.
    let controls_row = spawn_h_row(&mut commands, 14.0);
    commands.entity(panel).add_child(controls_row);
    let slider = spawn_radius_slider(&mut commands, &dev_font);
    commands.entity(controls_row).add_child(slider);
    let erase = spawn_toggle_button(&mut commands, &dev_font, "Erase", EraseToggle);
    commands.entity(controls_row).add_child(erase);
    let cont = spawn_toggle_button(&mut commands, &dev_font, "Continuous", ContinuousToggle);
    commands.entity(controls_row).add_child(cont);

    // Row 3: palette strip + a draggable scrollbar beneath it. Both wrapped
    // in a Column container so they share the same Percent(80.0) width.
    let palette_column = commands
        .spawn((
            Node {
                width: Val::Percent(80.0),
                max_width: px(900.0),
                flex_direction: FlexDirection::Column,
                row_gap: px(3.0),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    let palette_viewport = commands
        .spawn((
            PaletteViewport,
            EditorHudInteractive,
            Node {
                width: Val::Percent(100.0),
                height: px(SWATCH_SIZE_PX + 8.0),
                overflow: Overflow::scroll_x(),
                padding: px(2.0).all(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.04, 1.0)),
            FocusPolicy::Block,
            // Bevy needs Interaction for hover-detection. The scroll-wheel
            // system reads `Interaction == Hovered` to route wheel events
            // to this viewport.
            Interaction::default(),
            // Bevy 0.19 scroll container — `Overflow::scroll_x` activates
            // scrolling, `ScrollPosition.x` is the offset in logical pixels.
            ScrollPosition::default(),
        ))
        .id();
    let palette_strip = commands
        .spawn((
            PaletteStrip,
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(SWATCH_GAP_PX),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    commands.entity(palette_viewport).add_child(palette_strip);
    commands.entity(palette_column).add_child(palette_viewport);

    let scrollbar_track = commands
        .spawn((
            PaletteScrollbarTrack,
            EditorHudInteractive,
            Node {
                width: Val::Percent(100.0),
                height: px(SCROLLBAR_HEIGHT_PX),
                ..default()
            },
            BackgroundColor(COL_SCROLLBAR_TRACK),
            Interaction::default(),
            RelativeCursorPosition::default(),
            FocusPolicy::Block,
        ))
        .id();
    let scrollbar_thumb = commands
        .spawn((
            PaletteScrollbarThumb,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Relative,
                left: Val::Px(0.0),
                ..default()
            },
            BackgroundColor(COL_SCROLLBAR_THUMB),
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(scrollbar_track).add_child(scrollbar_thumb);
    commands.entity(palette_column).add_child(scrollbar_track);
    commands.entity(panel).add_child(palette_column);

    // Top-right hover-info text.
    commands.spawn((
        HoverInfoText,
        Text::default(),
        TextColor(COL_TEXT_PRIMARY),
        TextFont {
            font: dev_font.0.clone(),
            font_size: FontSize::Px(12.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            top: px(12.0),
            right: px(12.0),
            padding: px(8.0).all(),
            display: Display::None,
            ..default()
        },
    ));

    // Top-left PBR-debug overlay. Hidden until `DebugViewState.mode != Off`
    // (`update_debug_view_hud` toggles visibility).
    commands.spawn((
        DebugViewHudText,
        Text::default(),
        TextColor(Color::srgba(1.0, 0.85, 0.30, 1.0)),
        TextFont {
            font: dev_font.0.clone(),
            font_size: FontSize::Px(13.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.60)),
        Node {
            position_type: PositionType::Absolute,
            top: px(12.0),
            left: px(12.0),
            padding: px(8.0).all(),
            display: Display::None,
            ..default()
        },
    ));
}

/// `Update` system — show the top-left "Debug: <mode>" overlay when the
/// PBR rendering debugger is engaged. See
/// `docs/orchestrate/pbr-raymarching/05-diagnostic.md` § "PBR rendering
/// debugger".
pub fn update_debug_view_hud(
    state: Option<Res<crate::debug_view::DebugViewState>>,
    mut overlay: Query<(&mut Text, &mut Node), With<DebugViewHudText>>,
) {
    let Some(state) = state else { return };
    let Ok((mut text, mut node)) = overlay.single_mut() else { return };
    if state.mode == crate::debug_view::DebugViewMode::Off {
        node.display = Display::None;
        text.0.clear();
    } else {
        node.display = Display::Flex;
        text.0.clear();
        let _ = write!(
            text.0,
            "Debug: {}   [F1: toggle | [ / ]: cycle]",
            state.mode.label(),
        );
    }
}

fn spawn_h_row(commands: &mut Commands, gap_px: f32) -> Entity {
    commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(gap_px),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id()
}

fn spawn_tool_button(
    commands: &mut Commands,
    dev_font: &DevFont,
    tool: EditTool,
) -> Entity {
    let label = match tool {
        EditTool::Paint => "Paint",
        EditTool::Cube => "Cube",
        EditTool::Sphere => "Sphere",
    };
    let button = commands
        .spawn((
            ToolButton(tool),
            EditorHudInteractive,
            Node {
                width: px(TOOL_BUTTON_SIZE_PX),
                height: px(TOOL_BUTTON_SIZE_PX),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: px(3.0),
                border: px(2.0).all(),
                ..default()
            },
            BackgroundColor(COL_BTN_BG),
            BorderColor::all(COL_BTN_BORDER),
            Interaction::default(),
            RelativeCursorPosition::default(),
            FocusPolicy::Block,
        ))
        .id();
    // Shape-drawn icon — no font glyphs, works identically on web.
    let icon = spawn_tool_icon(commands, tool);
    let label_text = commands
        .spawn((
            Text::new(label),
            TextColor(COL_TEXT_PRIMARY),
            TextFont {
                font: dev_font.0.clone(),
                font_size: FontSize::Px(10.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(button).add_child(icon);
    commands.entity(button).add_child(label_text);
    button
}

/// Spawn a bevy_ui-rendered icon shape representing the tool. No font glyphs.
///
/// - Paint: small filled circle (a brush tip).
/// - Cube: filled square with white border.
/// - Sphere: filled rounded square (BorderRadius::MAX) with white border.
fn spawn_tool_icon(commands: &mut Commands, tool: EditTool) -> Entity {
    match tool {
        EditTool::Paint => commands
            .spawn((
                Node {
                    width: px(18.0),
                    height: px(18.0),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BackgroundColor(Color::WHITE),
                Pickable::IGNORE,
            ))
            .id(),
        EditTool::Cube => commands
            .spawn((
                Node {
                    width: px(22.0),
                    height: px(22.0),
                    border: px(2.0).all(),
                    ..default()
                },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.55)),
                BorderColor::all(Color::WHITE),
                Pickable::IGNORE,
            ))
            .id(),
        EditTool::Sphere => commands
            .spawn((
                Node {
                    width: px(22.0),
                    height: px(22.0),
                    border: px(2.0).all(),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.55)),
                BorderColor::all(Color::WHITE),
                Pickable::IGNORE,
            ))
            .id(),
    }
}

fn spawn_radius_slider(commands: &mut Commands, dev_font: &DevFont) -> Entity {
    let container = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Start,
                row_gap: px(2.0),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();

    let label_row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(6.0),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    let static_label = commands
        .spawn((
            Text::new("Brush"),
            TextColor(COL_TEXT_MUTED),
            TextFont {
                font: dev_font.0.clone(),
                font_size: FontSize::Px(11.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .id();
    let value_label = commands
        .spawn((
            RadiusSliderLabel,
            Text::default(),
            TextColor(COL_TEXT_PRIMARY),
            TextFont {
                font: dev_font.0.clone(),
                font_size: FontSize::Px(11.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(label_row).add_child(static_label);
    commands.entity(label_row).add_child(value_label);
    commands.entity(container).add_child(label_row);

    let track = commands
        .spawn((
            RadiusSlider,
            EditorHudInteractive,
            Node {
                width: px(SLIDER_WIDTH_PX),
                height: px(SLIDER_HEIGHT_PX),
                ..default()
            },
            BackgroundColor(COL_SLIDER_TRACK),
            Interaction::default(),
            RelativeCursorPosition::default(),
            FocusPolicy::Block,
        ))
        .id();
    let fill = commands
        .spawn((
            RadiusSliderFill,
            Node {
                width: Val::Percent(0.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(COL_SLIDER_FILL),
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(track).add_child(fill);
    commands.entity(container).add_child(track);
    container
}

fn spawn_toggle_button<M: Component>(
    commands: &mut Commands,
    dev_font: &DevFont,
    label: &str,
    marker: M,
) -> Entity {
    let button = commands
        .spawn((
            marker,
            EditorHudInteractive,
            Node {
                padding: UiRect::axes(px(10.0), px(6.0)),
                border: px(1.0).all(),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(COL_BTN_BG),
            BorderColor::all(COL_BTN_BORDER),
            Interaction::default(),
            RelativeCursorPosition::default(),
            FocusPolicy::Block,
        ))
        .id();
    let text = commands
        .spawn((
            Text::new(label),
            TextColor(COL_TEXT_PRIMARY),
            TextFont {
                font: dev_font.0.clone(),
                font_size: FontSize::Px(12.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(button).add_child(text);
    button
}

/// `Update` system — refresh the palette strip's children whenever
/// `VoxelTypes` changes (or on first availability after `setup_editor_hud`).
/// Builds one swatch per non-empty entry, colored by its `color_base`.
pub fn refresh_palette_swatches(
    mut commands: Commands,
    voxel_types: Res<VoxelTypes>,
    strip: Query<Entity, With<PaletteStrip>>,
    existing_swatches: Query<Entity, With<PaletteSwatch>>,
) {
    if !voxel_types.is_changed() && !existing_swatches.is_empty() {
        return;
    }
    let Ok(strip_entity) = strip.single() else {
        return;
    };

    for swatch in &existing_swatches {
        commands.entity(swatch).despawn();
    }

    for (idx, vt) in voxel_types.types.iter().enumerate().skip(1) {
        let id = VoxelTypeId(idx as u16);
        // Post-PBR-raymarching pivot: the per-VoxelType "user-picked colour"
        // is `albedo_tint` (the sampled albedo multiplier). The swatch shows
        // the tint — sRGB byte → linear `[0,1]` via Bevy's `Color::srgb_u8`,
        // which converts to linear internally.
        let color = Color::srgb_u8(vt.albedo_tint[0], vt.albedo_tint[1], vt.albedo_tint[2]);
        let swatch = commands
            .spawn((
                PaletteSwatch(id),
                EditorHudInteractive,
                Node {
                    width: px(SWATCH_SIZE_PX),
                    height: px(SWATCH_SIZE_PX),
                    border: px(2.0).all(),
                    flex_shrink: 0.0,
                    ..default()
                },
                BackgroundColor(color),
                BorderColor::all(COL_SWATCH_BORDER),
                Interaction::default(),
                RelativeCursorPosition::default(),
                FocusPolicy::Block,
            ))
            .id();
        commands.entity(strip_entity).add_child(swatch);
    }
}

/// Local state for the radius-slider drag (so it survives across frames
/// without leaving the LMB-down handler stranded if the cursor exits the
/// track).
#[derive(Default)]
pub struct RadiusSliderDrag {
    pub active: bool,
}

/// Local state for the palette scrollbar drag — set when LMB-pressed on the
/// scrollbar track, cleared on LMB release. While active, each frame's
/// `RelativeCursorPosition` maps to a new scroll offset.
#[derive(Default)]
pub struct PaletteScrollbarDrag {
    pub active: bool,
}

/// `Update` system — route mouse wheel events to the palette viewport's
/// `ScrollPosition` while the cursor is over *any* part of the palette
/// region. Handles both horizontal wheel input (trackpad) and vertical wheel
/// (standard mouse).
///
/// The gate intentionally checks the swatches + the scrollbar + the viewport,
/// because Bevy's picking gives `Interaction::Hovered` to the topmost
/// hit-tested entity only — when the cursor is over a swatch, the swatch
/// reports `Hovered` and the viewport reports `None`, so a viewport-only
/// gate would dead-zone the swatch area itself.
pub fn scroll_palette_with_wheel(
    mut wheel: MessageReader<MouseWheel>,
    mut viewport: Query<(&mut ScrollPosition, &ComputedNode), With<PaletteViewport>>,
    palette_zone_viewport: Query<&Interaction, With<PaletteViewport>>,
    palette_zone_swatches: Query<&Interaction, With<PaletteSwatch>>,
    palette_zone_scrollbar: Query<&Interaction, With<PaletteScrollbarTrack>>,
) {
    let active = |i: &Interaction| matches!(i, Interaction::Hovered | Interaction::Pressed);
    let any_hovered = palette_zone_viewport.iter().any(active)
        || palette_zone_swatches.iter().any(active)
        || palette_zone_scrollbar.iter().any(active);
    if !any_hovered {
        wheel.clear();
        return;
    }
    let Ok((mut scroll, computed)) = viewport.single_mut() else {
        wheel.clear();
        return;
    };

    let mut delta_logical = 0.0_f32;
    for ev in wheel.read() {
        // Prefer horizontal-axis input; for a plain wheel (only Y) treat
        // Y-up as scroll-left so the palette behaves like a horizontal page.
        let raw = if ev.x.abs() > 0.0 { ev.x } else { ev.y };
        let unit_px = match ev.unit {
            MouseScrollUnit::Pixel => 1.0,
            MouseScrollUnit::Line => WHEEL_LINE_PX,
        };
        delta_logical -= raw * unit_px;
    }
    if delta_logical == 0.0 {
        return;
    }
    let scale = if computed.inverse_scale_factor > 0.0 {
        computed.inverse_scale_factor
    } else {
        1.0
    };
    let viewport_w_logical = computed.size.x * scale;
    let content_w_logical = computed.content_size.x * scale;
    let max_scroll = (content_w_logical - viewport_w_logical).max(0.0);
    scroll.x = (scroll.x + delta_logical).clamp(0.0, max_scroll);
}

/// `Update` system — write the scrollbar thumb's width + horizontal offset to
/// reflect the palette's current scroll position. Thumb width = visible
/// fraction of content; thumb left = scroll fraction of slack.
pub fn update_palette_scrollbar(
    viewport: Query<(&ScrollPosition, &ComputedNode), With<PaletteViewport>>,
    track: Query<&ComputedNode, With<PaletteScrollbarTrack>>,
    mut thumb: Query<&mut Node, With<PaletteScrollbarThumb>>,
) {
    let Ok((scroll, vp)) = viewport.single() else { return };
    let Ok(track_node) = track.single() else { return };
    let Ok(mut thumb_node) = thumb.single_mut() else { return };

    let scale = if vp.inverse_scale_factor > 0.0 { vp.inverse_scale_factor } else { 1.0 };
    let viewport_w = vp.size.x * scale;
    let content_w = vp.content_size.x * scale;
    let track_w = track_node.size.x * scale;

    if content_w <= viewport_w + 0.5 || track_w <= 0.0 {
        thumb_node.width = Val::Percent(100.0);
        thumb_node.left = Val::Px(0.0);
        return;
    }

    let visible_frac = (viewport_w / content_w).clamp(0.0, 1.0);
    let thumb_w = (track_w * visible_frac).max(SCROLLBAR_THUMB_MIN_PX);
    let max_thumb_left = (track_w - thumb_w).max(0.0);
    let max_scroll = (content_w - viewport_w).max(1.0);
    let scroll_frac = (scroll.x / max_scroll).clamp(0.0, 1.0);

    thumb_node.width = Val::Px(thumb_w);
    thumb_node.left = Val::Px(scroll_frac * max_thumb_left);
}

/// `Update` system — drag the scrollbar to scroll the palette. Click anywhere
/// on the track snaps the cursor to that scroll position; continued LMB-hold
/// scrubs the scroll position with cursor X.
pub fn drag_palette_scrollbar(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut drag: Local<PaletteScrollbarDrag>,
    track: Query<(&Interaction, &RelativeCursorPosition), With<PaletteScrollbarTrack>>,
    mut viewport: Query<(&mut ScrollPosition, &ComputedNode), With<PaletteViewport>>,
) {
    let Ok((interaction, rel_pos)) = track.single() else { return };
    let Ok((mut scroll, vp)) = viewport.single_mut() else { return };

    let lmb_down = mouse_buttons.pressed(MouseButton::Left);
    let lmb_just_pressed = mouse_buttons.just_pressed(MouseButton::Left);
    let lmb_just_released = mouse_buttons.just_released(MouseButton::Left);

    if lmb_just_pressed && *interaction == Interaction::Pressed {
        drag.active = true;
    }
    if lmb_just_released {
        drag.active = false;
    }
    if !drag.active || !lmb_down {
        return;
    }
    let Some(rel) = rel_pos.normalized else { return };

    let scale = if vp.inverse_scale_factor > 0.0 { vp.inverse_scale_factor } else { 1.0 };
    let viewport_w = vp.size.x * scale;
    let content_w = vp.content_size.x * scale;
    if content_w <= viewport_w {
        return;
    }
    let max_scroll = (content_w - viewport_w).max(0.0);

    // Center the thumb under the cursor: subtract half the visible-fraction
    // so cursor sits in the middle of the thumb instead of its left edge.
    let visible_frac = (viewport_w / content_w).clamp(0.0, 1.0);
    let half = (visible_frac * 0.5).clamp(0.0, 0.499);
    let usable = (1.0 - 2.0 * half).max(0.001);
    let frac = ((rel.x - half) / usable).clamp(0.0, 1.0);
    scroll.x = frac * max_scroll;
}

/// `Update` system — read every interactive element's `Interaction` state and
/// mutate `EditorState` on click.
#[allow(clippy::too_many_arguments)]
pub fn handle_hud_clicks(
    mut state: ResMut<EditorState>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut drag: Local<RadiusSliderDrag>,
    tool_buttons: Query<(&ToolButton, &Interaction), Changed<Interaction>>,
    swatches: Query<(&PaletteSwatch, &Interaction), Changed<Interaction>>,
    erase: Query<&Interaction, (With<EraseToggle>, Changed<Interaction>)>,
    cont: Query<&Interaction, (With<ContinuousToggle>, Changed<Interaction>)>,
    slider: Query<(&Interaction, &RelativeCursorPosition), With<RadiusSlider>>,
) {
    for (button, interaction) in &tool_buttons {
        if *interaction == Interaction::Pressed {
            state.tool = button.0;
        }
    }
    for (swatch, interaction) in &swatches {
        if *interaction == Interaction::Pressed {
            state.selected_type = swatch.0;
        }
    }
    for interaction in &erase {
        if *interaction == Interaction::Pressed {
            state.is_erase = !state.is_erase;
        }
    }
    for interaction in &cont {
        if *interaction == Interaction::Pressed {
            state.is_continuous = !state.is_continuous;
        }
    }

    if let Ok((interaction, rel_pos)) = slider.single() {
        let lmb_down = mouse_buttons.pressed(MouseButton::Left);
        let lmb_just_pressed = mouse_buttons.just_pressed(MouseButton::Left);
        let lmb_just_released = mouse_buttons.just_released(MouseButton::Left);

        if lmb_just_pressed && *interaction == Interaction::Pressed {
            drag.active = true;
            if let Some(rel) = rel_pos.normalized {
                state.radius = (RADIUS_MIN + rel.x.clamp(0.0, 1.0) * (RADIUS_MAX - RADIUS_MIN))
                    .clamp(RADIUS_MIN, RADIUS_MAX);
            }
        } else if drag.active && lmb_down {
            let scale = window.iter().next().map(|w| w.scale_factor()).unwrap_or(1.0);
            let dx_pixels = motion.delta.x;
            if dx_pixels != 0.0 {
                let range = RADIUS_MAX - RADIUS_MIN;
                let delta = dx_pixels * (range / (SLIDER_WIDTH_PX * scale.max(0.001)));
                state.radius = (state.radius + delta).clamp(RADIUS_MIN, RADIUS_MAX);
            }
        }
        if lmb_just_released {
            drag.active = false;
        }
    }
}

/// `Update` system — repaint every HUD element from `EditorState`.
#[allow(clippy::too_many_arguments)]
pub fn update_editor_hud(
    state: Res<EditorState>,
    mut tool_buttons: Query<
        (&ToolButton, &Interaction, &mut BackgroundColor, &mut BorderColor),
        (Without<PaletteSwatch>, Without<EraseToggle>, Without<ContinuousToggle>),
    >,
    mut swatches: Query<
        (&PaletteSwatch, &mut BorderColor),
        (Without<ToolButton>, Without<EraseToggle>, Without<ContinuousToggle>),
    >,
    mut erase_buttons: Query<
        (&Interaction, &mut BackgroundColor, &mut BorderColor, &Children),
        (With<EraseToggle>, Without<ToolButton>, Without<PaletteSwatch>, Without<ContinuousToggle>),
    >,
    mut cont_buttons: Query<
        (&Interaction, &mut BackgroundColor, &mut BorderColor, &Children),
        (With<ContinuousToggle>, Without<ToolButton>, Without<PaletteSwatch>, Without<EraseToggle>),
    >,
    mut text_colors: Query<&mut TextColor>,
    mut fill: Query<&mut Node, With<RadiusSliderFill>>,
    mut label: Query<&mut Text, (With<RadiusSliderLabel>, Without<HoverInfoText>)>,
    mut hover_info: Query<(&mut Text, &mut Node), (With<HoverInfoText>, Without<RadiusSliderLabel>, Without<RadiusSliderFill>)>,
) {
    let erase_affects = matches!(state.tool, EditTool::Cube | EditTool::Sphere);

    for (button, interaction, mut bg, mut border) in &mut tool_buttons {
        let selected = button.0 == state.tool;
        let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        *bg = BackgroundColor(if selected {
            COL_BTN_BG_SELECTED
        } else if hovered {
            COL_BTN_BG_HOVER
        } else {
            COL_BTN_BG
        });
        *border = BorderColor::all(if selected { COL_BTN_BORDER_SELECTED } else { COL_BTN_BORDER });
    }

    for (swatch, mut border) in &mut swatches {
        let selected = swatch.0 == state.selected_type;
        *border = BorderColor::all(if selected { COL_SWATCH_BORDER_SELECTED } else { COL_SWATCH_BORDER });
    }

    for (interaction, mut bg, mut border, children) in &mut erase_buttons {
        let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        let (target_bg, target_border, text_color) = if !erase_affects {
            (COL_BTN_BG_DISABLED, COL_BTN_BORDER, COL_TEXT_DISABLED)
        } else if state.is_erase {
            (COL_BTN_BG_SELECTED, COL_BTN_BORDER_SELECTED, COL_TEXT_PRIMARY)
        } else if hovered {
            (COL_BTN_BG_HOVER, COL_BTN_BORDER, COL_TEXT_PRIMARY)
        } else {
            (COL_BTN_BG, COL_BTN_BORDER, COL_TEXT_PRIMARY)
        };
        *bg = BackgroundColor(target_bg);
        *border = BorderColor::all(target_border);
        for &child in children {
            if let Ok(mut tc) = text_colors.get_mut(child) {
                *tc = TextColor(text_color);
            }
        }
    }

    for (interaction, mut bg, mut border, children) in &mut cont_buttons {
        let hovered = matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        let (target_bg, target_border, text_color) = if !erase_affects {
            (COL_BTN_BG_DISABLED, COL_BTN_BORDER, COL_TEXT_DISABLED)
        } else if state.is_continuous {
            (COL_BTN_BG_SELECTED, COL_BTN_BORDER_SELECTED, COL_TEXT_PRIMARY)
        } else if hovered {
            (COL_BTN_BG_HOVER, COL_BTN_BORDER, COL_TEXT_PRIMARY)
        } else {
            (COL_BTN_BG, COL_BTN_BORDER, COL_TEXT_PRIMARY)
        };
        *bg = BackgroundColor(target_bg);
        *border = BorderColor::all(target_border);
        for &child in children {
            if let Ok(mut tc) = text_colors.get_mut(child) {
                *tc = TextColor(text_color);
            }
        }
    }

    let pct = ((state.radius - RADIUS_MIN) / (RADIUS_MAX - RADIUS_MIN)).clamp(0.0, 1.0) * 100.0;
    for mut node in &mut fill {
        node.width = Val::Percent(pct);
    }
    for mut text in &mut label {
        text.0.clear();
        let _ = write!(text.0, "{:.0} vx", state.radius);
    }

    if let Ok((mut text, mut node)) = hover_info.single_mut() {
        match &state.last_hover_hit {
            Some(hit) => {
                node.display = Display::Flex;
                let s = &mut text.0;
                s.clear();
                let _ = writeln!(s, "voxel ({}, {}, {})", hit.voxel_pos.x, hit.voxel_pos.y, hit.voxel_pos.z);
                let _ = write!(s, "type #{}", hit.voxel_type.0);
            }
            None => {
                node.display = Display::None;
                text.0.clear();
            }
        }
    }
}
