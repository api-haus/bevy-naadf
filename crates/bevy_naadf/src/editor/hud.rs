//! Track-B editor — top-right tool-state HUD overlay
//! (`docs/orchestrate/feature-completeness/02b-design-editor.md` § Tool-state
//! HUD overlay).
//!
//! A sibling Node+Text overlay (mirrors `hud.rs:92-110`) anchored top-RIGHT
//! (HUD is top-LEFT, panel is bottom-LEFT, editor-HUD is top-RIGHT — three
//! corners, no overlap). Shows current tool + brush settings + hover voxel
//! info while `EditorState.edit_active = true`; hidden otherwise.

use std::fmt::Write;

use bevy::prelude::*;

use crate::editor::{EditTool, EditorState};
use crate::DevFont;

/// Marker for the editor's HUD Text entity.
#[derive(Component)]
pub struct EditorHudText;

/// `Startup` system — spawn the top-right editor HUD Text. Starts hidden;
/// `update_editor_hud` reveals it when `EditorState.edit_active = true`.
pub fn setup_editor_hud(mut commands: Commands, dev_font: Res<DevFont>) {
    commands.spawn((
        EditorHudText,
        Text::default(),
        TextColor(Color::WHITE),
        TextFont {
            font: dev_font.0.clone(),
            font_size: FontSize::Px(14.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        Node {
            position_type: PositionType::Absolute,
            top: px(12.0),
            right: px(12.0),
            padding: px(8.0).all(),
            // Hidden until edit_active toggles on.
            display: Display::None,
            ..default()
        },
    ));
}

/// `Update` system — refresh editor HUD content while `edit_active`; hide
/// otherwise.
pub fn update_editor_hud(
    state: Res<EditorState>,
    mut hud: Query<(&mut Text, &mut Node), With<EditorHudText>>,
) {
    let Ok((mut text, mut node)) = hud.single_mut() else {
        return;
    };

    if !state.edit_active {
        node.display = Display::None;
        text.0.clear();
        return;
    }
    node.display = Display::Flex;

    let s = &mut text.0;
    s.clear();
    let _ = writeln!(s, "EDITOR MODE");
    let _ = writeln!(s, "  Tool:       {}", tool_label(state.tool));
    let _ = writeln!(s, "  Radius:     {:.1}", state.radius);
    let _ = writeln!(s, "  Erase:      {}", state.is_erase);
    let _ = writeln!(s, "  Continuous: {}", state.is_continuous);
    let _ = writeln!(s, "  Type:       {}", state.selected_type.0);
    let _ = writeln!(s);
    let _ = writeln!(s, "Hover:");
    match &state.last_hover_hit {
        Some(hit) => {
            let _ = writeln!(
                s,
                "  Voxel:    ({}, {}, {})",
                hit.voxel_pos.x, hit.voxel_pos.y, hit.voxel_pos.z
            );
            let _ = writeln!(s, "  Type:     {}", hit.voxel_type.0);
            let _ = writeln!(
                s,
                "  Normal:   ({:.0}, {:.0}, {:.0})",
                hit.normal.x, hit.normal.y, hit.normal.z
            );
            let _ = writeln!(s, "  Distance: {:.2}", hit.distance);
        }
        None => {
            let _ = writeln!(s, "  (no hit — aim at world)");
        }
    }
    let _ = write!(s, "\n[F2] exit edit mode");
}

fn tool_label(tool: EditTool) -> &'static str {
    match tool {
        EditTool::Paint => "Paint",
        EditTool::Cube => "Cube",
        EditTool::Sphere => "Sphere",
    }
}
