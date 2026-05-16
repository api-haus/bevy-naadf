//! Track-B editor — paint/cube/sphere brushes
//! (`docs/orchestrate/feature-completeness/02b-design-editor.md`).
//!
//! Layout (`02b-design-editor.md` Architecture):
//! - [`EditorState`] — main-world resource carrying the editor's mutable
//!   configuration (selected tool, radius, erase flag, continuous flag, palette
//!   index) AND the per-stroke runtime state (`pos`, `stroke_just_started`,
//!   `last_hover_hit`, `edit_active`).
//! - [`apply_edit_tool`] — `Update` system: F2 toggles `edit_active`; while
//!   active, casts a CPU pick ray on cursor → world; on LMB held, runs the
//!   selected brush.
//! - [`tools`] — paint / cube / sphere implementations.
//! - [`ray`] — `screen_to_ray` viewport-to-world helper.
//! - [`hud`] — the top-right tool-state HUD overlay.
//!
//! Wired from `lib.rs` behind the same `cfg.add_hud` gate as the panel — the
//! e2e harness (`AppConfig::e2e`) sets `add_hud = false`, so the editor is
//! never present in the harness and the e2e luminance/regression gates are
//! unaffected.

pub mod hud;
pub mod ray;
pub mod tools;

use bevy::camera::Camera3d;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::panel::PanelState;
use crate::voxel::VoxelTypeId;
use crate::world::data::{RayHit, WorldData};

/// Selected brush tool (`02b-design-editor.md` § EditorState).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum EditTool {
    /// Replace non-empty voxels within radius with `selected_type`. No erase.
    #[default]
    Paint = 0,
    /// Chebyshev cube. `is_erase` writes EMPTY.
    Cube = 1,
    /// Euclidean sphere. `is_erase` writes EMPTY.
    Sphere = 2,
}

/// Editor configuration + per-frame runtime state. One resource so the panel
/// and `apply_edit_tool` share a single source of truth (mirrors the
/// `AppArgs.gi` shared-state pattern from `panel.rs`).
#[derive(Resource, Debug, Clone)]
pub struct EditorState {
    // ---- user-tweakable via panel ----
    /// Currently selected tool. Cycled via panel's enum knob.
    pub tool: EditTool,
    /// Currently selected paint type — index into the `VoxelTypes::types`
    /// palette. Clamped to `0..=u16::MAX` by the panel; out-of-range indices
    /// silently no-op via the per-voxel set_voxel path's clamp.
    pub selected_type: VoxelTypeId,
    /// Brush radius in voxels — clamped 1..400 by the panel; C# default 10
    /// (`EditingToolPaint.cs:13`).
    pub radius: f32,
    /// Erase mode (Cube/Sphere only). Paint ignores this — Paint replaces
    /// non-empty voxels, never erases. (`EditingToolCube.cs:20`,
    /// `EditingToolSphere.cs:20`.)
    pub is_erase: bool,
    /// Continuous-brush mode (Cube/Sphere only). When `false`, the brush only
    /// fires on the LMB-down edge; when `true`, it re-fires every frame while
    /// LMB is held. Default `true` matches C#
    /// (`EditingToolCube.cs:50-51` / `EditingToolSphere.cs:50-51`).
    pub is_continuous: bool,

    // ---- runtime-only state (not user-editable) ----
    /// Master gate: `false` means LMB is ignored by the editor AND the
    /// tool-state HUD is hidden. Toggled by F2 (separate from F1 panel
    /// toggle — the user can have the panel open without being in edit mode).
    pub edit_active: bool,
    /// Smoothed brush position in world space. Snapped on LMB-just-pressed
    /// (`EditingToolPaint.cs:34-35`); lerped per-frame thereafter
    /// (`EditingToolPaint.cs:36-40`).
    pub pos: Vec3,
    /// `true` on the LMB-just-pressed frame, false after the stroke continues.
    /// Equivalent to C#'s `IO.MOStates.IsLeftButtonToggleOn()`. Cleared on
    /// LMB release.
    pub stroke_just_started: bool,
    /// Last hover RayHit, refreshed every frame while `edit_active` and the
    /// cursor is in the window (regardless of LMB state) — fed to the HUD.
    pub last_hover_hit: Option<RayHit>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            tool: EditTool::default(),
            selected_type: VoxelTypeId(1), // first non-empty palette index
            radius: 10.0,
            is_erase: false,
            is_continuous: true,
            edit_active: false,
            pos: Vec3::ZERO,
            stroke_just_started: false,
            last_hover_hit: None,
        }
    }
}

impl EditorState {
    /// Map a u32 cycle value (0/1/2) to an EditTool — wraps via modulo.
    /// Used by the panel's `Edit { variant: Enum }` setter to cycle tool.
    pub fn tool_from_u32(v: u32) -> EditTool {
        match v % 3 {
            0 => EditTool::Paint,
            1 => EditTool::Cube,
            2 => EditTool::Sphere,
            _ => EditTool::Paint,
        }
    }
}

/// `Update` system — the editor's per-frame entry point.
///
/// Flow (`02b-design-editor.md` § apply_edit_tool):
/// 1. F2 toggles `edit_active`.
/// 2. If panel cursor is hovering / pressing a PanelRow, bail (the panel
///    owns this click).
/// 3. Cast cursor → world via `screen_to_ray` + `WorldData::ray_traversal`,
///    cache the hit on `state.last_hover_hit`.
/// 4. If LMB not pressed, clear `stroke_just_started`, return.
/// 5. Snap/lerp `state.pos` toward the hit.
/// 6. Apply the `is_continuous` early-out for Cube/Sphere.
/// 7. Apply the `is_erase`+`EMPTY type` early-out for Cube/Sphere.
/// 8. Dispatch to the selected brush.
/// 9. Clear `stroke_just_started` (so a non-continuous brush fires once per
///    stroke).
#[allow(clippy::too_many_arguments)]
pub fn apply_edit_tool(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut world_data: ResMut<WorldData>,
    mut state: ResMut<EditorState>,
    time: Res<Time>,
    panel_state: Res<PanelState>,
    panel_rows: Query<&Interaction, With<crate::panel::PanelRow>>,
) {
    // F2 toggles edit mode.
    if keys.just_pressed(KeyCode::F2) {
        state.edit_active = !state.edit_active;
        info!("editor edit_active = {}", state.edit_active);
    }

    if !state.edit_active {
        state.last_hover_hit = None;
        state.stroke_just_started = false;
        return;
    }

    // Bail if the panel owns the current cursor interaction (open + any
    // interactive row is pressed or hovered while LMB is down).
    if panel_state.open {
        let any_panel_engaged = panel_rows
            .iter()
            .any(|i| matches!(*i, Interaction::Pressed | Interaction::Hovered));
        if any_panel_engaged {
            return;
        }
    }

    let Ok(window) = window.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        state.last_hover_hit = None;
        return;
    };
    let Ok((camera, cam_gxf)) = camera.single() else {
        return;
    };
    let Some(ray) = ray::screen_to_ray(camera, cam_gxf, cursor_pos) else {
        return;
    };
    state.last_hover_hit = world_data.ray_traversal(ray.origin, ray.dir);

    // LMB handling.
    if !mouse.pressed(MouseButton::Left) {
        state.stroke_just_started = false;
        return;
    }
    let Some(hit) = state.last_hover_hit.clone() else {
        return;
    };

    // Snap-on-first-press; lerp on continued press
    // (`EditingToolPaint.cs:34-40` / `Cube.cs:42-48` / `Sphere.cs:42-48`).
    let just_pressed = mouse.just_pressed(MouseButton::Left);
    if just_pressed {
        state.stroke_just_started = true;
        state.pos = hit.world_pos;
    } else {
        let dt = time.delta_secs();
        let radius = state.radius.max(f32::EPSILON);
        let lerp_value = (1.0 - 1.0 / (1.0 + dt * 0.15 / radius)).min(1.0);
        state.pos = hit.world_pos * lerp_value + state.pos * (1.0 - lerp_value);
    }

    // is_continuous gate (Cube + Sphere only; Paint is always continuous).
    // C# EditingToolCube.cs:50-51 / EditingToolSphere.cs:50-51:
    //   if (!isContinuous && OldLeft == Pressed) return;
    if matches!(state.tool, EditTool::Cube | EditTool::Sphere)
        && !state.is_continuous
        && !state.stroke_just_started
    {
        return;
    }

    // is_erase + selected_type sanity (`Cube.cs:30-31` / `Sphere.cs:30-31`).
    if !state.is_erase
        && matches!(state.tool, EditTool::Cube | EditTool::Sphere)
        && state.selected_type == VoxelTypeId::EMPTY
    {
        return;
    }

    // Dispatch.
    let pos = state.pos;
    let radius = state.radius;
    let ty = state.selected_type;
    let is_erase = state.is_erase;
    match state.tool {
        EditTool::Paint => tools::paint_brush(&mut world_data, pos, radius, ty),
        EditTool::Cube => tools::cube_brush(&mut world_data, pos, radius, ty, is_erase),
        EditTool::Sphere => tools::sphere_brush(&mut world_data, pos, radius, ty, is_erase),
    }

    // Clear "first frame" AFTER the brush ran — guarantees a non-continuous
    // brush fires exactly once per LMB-down.
    state.stroke_just_started = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test #13 — defaults are safe / match design.
    #[test]
    fn editor_state_default_is_safe() {
        let s = EditorState::default();
        assert!(!s.edit_active);
        assert_eq!(s.selected_type, VoxelTypeId(1));
        assert!((s.radius - 10.0).abs() < f32::EPSILON);
        assert!(s.is_continuous);
        assert!(!s.is_erase);
        assert_eq!(s.tool, EditTool::Paint);
    }

    /// Test #14 — `apply_edit_tool` is a no-op when `edit_active = false`.
    /// We construct a minimal headless App, init the resources, run one
    /// `Update`, and verify nothing landed in `pending_edits`.
    #[test]
    fn apply_edit_tool_no_op_when_inactive() {
        use crate::world::data::{IAabb3, PendingEdits, WorldData};
        use bevy::math::UVec3;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // Resources the system reads.
        let wd = WorldData {
            chunks_cpu: vec![0u32; 8],
            blocks_cpu: Vec::new(),
            voxels_cpu: Vec::new(),
            size_in_chunks: UVec3::new(2, 2, 2),
            bounding_box: IAabb3 {
                min: IVec3::ZERO,
                max: IVec3::new(31, 31, 31),
            },
            dirty: false,
            pending_edits: PendingEdits::default(),
            dense_voxel_types: Vec::new(),
        };
        app.insert_resource(wd);
        app.init_resource::<EditorState>(); // edit_active = false
        app.init_resource::<crate::panel::PanelState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        // No camera / window entity — system bails before reaching brushes.
        app.add_systems(Update, apply_edit_tool);
        app.update();

        let wd_after = app.world().resource::<WorldData>();
        assert!(
            wd_after.pending_edits.batches.is_empty(),
            "edit_active=false should leave pending_edits untouched"
        );
    }

    /// Sanity — the EditTool u32 round-trip is total.
    #[test]
    fn edit_tool_from_u32_total() {
        assert_eq!(EditorState::tool_from_u32(0), EditTool::Paint);
        assert_eq!(EditorState::tool_from_u32(1), EditTool::Cube);
        assert_eq!(EditorState::tool_from_u32(2), EditTool::Sphere);
        // Wraps for any large value.
        assert_eq!(EditorState::tool_from_u32(123), EditTool::Paint);
    }
}
