//! Editor UI theme — semantic colour palette + dev-font text bundle constructor.
//!
//! Consolidates the `COL_*` consts previously file-local to `settings.rs` and
//! `editor/hud.rs`, plus the inline `Text + TextColor + TextFont` literal
//! repeated nine times across `settings.rs`, `editor/hud.rs`, and `hud.rs`.

use bevy::prelude::*;

use crate::DevFont;

// === Semantic palette ===
// Names describe role, not file-of-origin. File-specific consts are removed.

/// HUD strip background.
pub const BG_HUD: Color = Color::srgba(0.05, 0.05, 0.08, 0.82);
/// Settings panel background.
pub const BG_PANEL: Color = Color::srgba(0.06, 0.06, 0.09, 0.96);
/// Settings backdrop dimming layer.
pub const BG_BACKDROP: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);
/// Settings heading row background.
pub const BG_HEADING: Color = Color::srgba(0.10, 0.12, 0.18, 1.0);
/// Default button background.
pub const BG_BUTTON: Color = Color::srgba(0.10, 0.10, 0.14, 1.0);
/// Button background while hovered.
pub const BG_BUTTON_HOVER: Color = Color::srgba(0.18, 0.18, 0.24, 1.0);
/// Button background while selected.
pub const BG_BUTTON_SELECTED: Color = Color::srgba(0.95, 0.75, 0.20, 1.0);
/// Button background when disabled (no-op tool).
pub const BG_BUTTON_DISABLED: Color = Color::srgba(0.08, 0.08, 0.10, 1.0);
/// Reset-all button background (settings panel).
pub const BG_RESET: Color = Color::srgba(0.65, 0.20, 0.20, 1.0);
/// Reset-all button background while hovered.
pub const BG_RESET_HOVER: Color = Color::srgba(0.85, 0.30, 0.30, 1.0);
/// Settings row background while hovered.
pub const BG_ROW_HOVER: Color = Color::srgba(1.0, 1.0, 1.0, 0.05);
/// Settings row background while the keyboard cursor is on it.
pub const BG_ROW_SELECTED: Color = Color::srgba(1.0, 0.85, 0.30, 0.18);

/// Settings-panel outer border.
pub const BORDER_PANEL: Color = Color::srgba(0.35, 0.35, 0.42, 1.0);
/// Default button border.
pub const BORDER_BUTTON: Color = Color::srgba(0.30, 0.30, 0.34, 1.0);
/// Button border while selected.
pub const BORDER_BUTTON_SELECTED: Color = Color::srgba(1.0, 0.85, 0.30, 1.0);

/// Primary text colour.
pub const FG_PRIMARY: Color = Color::WHITE;
/// Muted secondary-label text.
pub const FG_MUTED: Color = Color::srgba(0.65, 0.65, 0.70, 1.0);
/// Text colour for disabled controls.
pub const FG_DISABLED: Color = Color::srgba(0.35, 0.35, 0.38, 1.0);
/// Section-header text colour.
pub const FG_SECTION: Color = Color::srgba(0.55, 0.85, 0.95, 1.0);
/// Settings value text while the row is keyboard-selected.
pub const FG_VALUE_SELECTED: Color = Color::srgba(1.0, 1.0, 0.6, 1.0);
/// Read-only diagnostic-row text.
pub const FG_READONLY: Color = Color::srgba(0.55, 0.55, 0.55, 1.0);

/// Default palette-swatch border.
pub const SWATCH_BORDER: Color = Color::srgba(0.20, 0.20, 0.24, 1.0);
/// Selected palette-swatch border.
pub const SWATCH_BORDER_SELECTED: Color = Color::WHITE;
/// Slider track background.
pub const SLIDER_TRACK: Color = Color::srgba(0.10, 0.10, 0.14, 1.0);
/// Slider value-fill colour.
pub const SLIDER_FILL: Color = Color::srgba(0.40, 0.65, 0.95, 1.0);

/// Palette scrollbar track.
pub const SCROLLBAR_TRACK: Color = Color::srgba(0.06, 0.06, 0.08, 1.0);
/// Palette scrollbar thumb.
pub const SCROLLBAR_THUMB: Color = Color::srgba(0.40, 0.40, 0.50, 1.0);

// === Text bundle constructor ===

/// Common dev-font font sizes used by the UI surface. Bare `f32` lets call
/// sites read `text_style(font, FG_PRIMARY, 13.0)` while keeping the type
/// system honest about what unit is in play.
pub type FontSizePx = f32;

/// `(TextColor, TextFont)` bundle for a `Text::new(...)` spawn. Centralizes the
/// 5-line `TextFont { font: dev_font.0.clone(), font_size: FontSize::Px(N), ..default() }`
/// boilerplate previously inlined nine times across `settings.rs`,
/// `editor/hud.rs`, `hud.rs`.
pub fn text_style(
    dev_font: &DevFont,
    color: Color,
    size_px: FontSizePx,
) -> (TextColor, TextFont) {
    (
        TextColor(color),
        TextFont {
            font: dev_font.0.clone(),
            font_size: FontSize::Px(size_px),
            ..default()
        },
    )
}
