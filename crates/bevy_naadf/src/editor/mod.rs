//! Track-B editor — paint/cube/sphere brushes
//! (`docs/orchestrate/feature-completeness/02b-design-editor.md`).
//!
//! Editing is **always on** in `AppMode::Playing` — there is no F2 toggle.
//! The system is wired with `.run_if(in_state(AppMode::Playing))` in
//! `lib.rs`, so when the Escape settings overlay is open the brush input is
//! silently inert.
//!
//! Layout:
//! - [`EditorState`] — main-world resource carrying the editor's mutable
//!   configuration (selected tool, radius, erase flag, continuous flag,
//!   palette index) AND the per-stroke runtime state (`pos`,
//!   `stroke_just_started`, `last_hover_hit`).
//! - [`apply_edit_tool`] — `Update` system: casts a CPU pick ray on cursor →
//!   world; on LMB held, runs the selected brush. Bails when the cursor is
//!   over the editor HUD so HUD clicks don't double-fire as brush clicks.
//! - [`tools`] — paint / cube / sphere implementations.
//! - [`ray`] — `screen_to_ray` viewport-to-world helper.
//! - [`hud`] — the bottom game-HUD strip (tool buttons + palette + brush
//!   controls) and the small top-right hover-info panel.
//!
//! Wired from `lib.rs` behind the same `cfg.add_hud` gate as the settings
//! overlay — the e2e harness (`AppConfig::e2e`) sets `add_hud = false`, so the
//! editor is never present in the harness and the e2e luminance/regression
//! gates are unaffected.

pub mod hud;
pub mod ray;
pub mod tools;
pub mod ui_theme;

use bevy::camera::Camera3d;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::editor::hud::EditorHudRoot;
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

/// Editor configuration + per-frame runtime state. One resource so the HUD
/// and `apply_edit_tool` share a single source of truth.
#[derive(Resource, Debug, Clone)]
pub struct EditorState {
    // ---- user-tweakable via HUD ----
    /// Currently selected tool. Mutated by clicking a tool button in the HUD.
    pub tool: EditTool,
    /// Currently selected paint type — index into the `VoxelTypes::types`
    /// palette. Mutated by clicking a swatch in the HUD palette strip.
    pub selected_type: VoxelTypeId,
    /// Brush radius in voxels — clamped 1..400 by the HUD slider; C# default
    /// 10 (`EditingToolPaint.cs:13`).
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
    /// Smoothed brush position in world space. Snapped on LMB-just-pressed
    /// (`EditingToolPaint.cs:34-35`); lerped per-frame thereafter
    /// (`EditingToolPaint.cs:36-40`).
    pub pos: Vec3,
    /// `true` on the LMB-just-pressed frame, false after the stroke continues.
    /// Equivalent to C#'s `IO.MOStates.IsLeftButtonToggleOn()`. Cleared on
    /// LMB release.
    pub stroke_just_started: bool,
    /// Last hover RayHit, refreshed every frame while the cursor is in the
    /// window — fed to the top-right hover-info panel.
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
            pos: Vec3::ZERO,
            stroke_just_started: false,
            last_hover_hit: None,
        }
    }
}

/// `Update` system — the editor's per-frame entry point.
///
/// Flow:
/// 1. If any descendant of the editor HUD root is hovered/pressed, bail (HUD
///    owns the click — its own click-handler system mutates `EditorState`).
/// 2. Cast cursor → world via `screen_to_ray` + `WorldData::ray_traversal`,
///    cache the hit on `state.last_hover_hit`.
/// 3. If LMB not pressed, clear `stroke_just_started`, return.
/// 4. Snap/lerp `state.pos` toward the hit.
/// 5. Apply the `is_continuous` early-out for Cube/Sphere.
/// 6. Apply the `is_erase`+`EMPTY type` early-out for Cube/Sphere.
/// 7. Dispatch to the selected brush.
/// 8. Clear `stroke_just_started`.
///
/// This system is gated globally via `.run_if(in_state(AppMode::Playing))` in
/// `lib.rs`, so it's inert while the Escape settings overlay is open.
#[allow(clippy::too_many_arguments)]
pub fn apply_edit_tool(
    mouse: Res<ButtonInput<MouseButton>>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut world_data: ResMut<WorldData>,
    mut state: ResMut<EditorState>,
    time: Res<Time>,
    hud_interactions: Query<&Interaction, With<EditorHudRoot>>,
    hud_child_interactions: Query<
        &Interaction,
        (With<crate::editor::hud::EditorHudInteractive>, Without<EditorHudRoot>),
    >,
) {
    // Bail if any HUD interactive (tool button / swatch / slider) is engaged —
    // its own click-handler system mutates `EditorState`; the brush must not
    // also fire on the same LMB-down.
    let hud_engaged = hud_interactions
        .iter()
        .chain(hud_child_interactions.iter())
        .any(|i| matches!(*i, Interaction::Pressed | Interaction::Hovered));
    if hud_engaged {
        state.stroke_just_started = false;
        return;
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
    //
    // C# `gameTime` arrives in **milliseconds**; `delta_secs() * 1000.0`
    // restores parity (`03b-followup-editor-bugs-234.md`).
    let just_pressed = mouse.just_pressed(MouseButton::Left);
    if just_pressed {
        state.stroke_just_started = true;
        state.pos = hit.world_pos;
    } else {
        let dt_ms = time.delta_secs() * 1000.0;
        let radius = state.radius.max(f32::EPSILON);
        let lerp_value = (1.0 - 1.0 / (1.0 + dt_ms * 0.15 / radius)).min(1.0);
        state.pos = hit.world_pos * lerp_value + state.pos * (1.0 - lerp_value);
    }

    // is_continuous gate (Cube + Sphere only; Paint is always continuous).
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

    state.stroke_just_started = false;
}

/// Plugin owning the in-game editor HUD + brush-input wiring. Prepared by D2
/// (codebase-tightening side-note 11) ahead of D7's `lib.rs` decomposition;
/// D7 wires this via `app.add_plugins(EditorPlugin)` in place of the inline
/// registration block at `lib.rs:900-971`. The `.after(toggle_settings_on_escape)`
/// edge on `apply_edit_tool` preserves the same-frame state-transition
/// observation the original 9-system chain depended on.
pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        use crate::app_mode::{toggle_settings_on_escape, AppMode};
        use bevy::state::condition::in_state;

        app.init_resource::<EditorState>()
            .add_systems(Startup, hud::setup_editor_hud.after(crate::load_dev_font))
            .add_systems(
                Update,
                (
                    hud::refresh_palette_swatches,
                    hud::handle_hud_clicks,
                    hud::scroll_palette_with_wheel,
                    hud::drag_palette_scrollbar,
                    hud::update_palette_scrollbar,
                    hud::update_editor_hud,
                    apply_edit_tool
                        .run_if(in_state(AppMode::Playing))
                        .after(toggle_settings_on_escape),
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_default_is_safe() {
        let s = EditorState::default();
        assert_eq!(s.selected_type, VoxelTypeId(1));
        assert!((s.radius - 10.0).abs() < f32::EPSILON);
        assert!(s.is_continuous);
        assert!(!s.is_erase);
        assert_eq!(s.tool, EditTool::Paint);
    }

    /// C# `EditingToolPaint.cs:38` lerp formula uses `gameTime` in
    /// MILLISECONDS; port multiplies `delta_secs() * 1000` to match. Validate
    /// the lerp coefficient at a typical 60-FPS frame (dt = 16.67 ms, r = 10).
    #[test]
    fn brush_lerp_uses_milliseconds_to_match_csharp() {
        let dt_ms = 16.667_f32;
        let radius = 10.0_f32;
        let lerp_value = (1.0 - 1.0 / (1.0 + dt_ms * 0.15 / radius)).min(1.0);
        assert!(
            lerp_value > 0.1,
            "lerp_value = {lerp_value} too small — brush would not track cursor"
        );
        assert!(
            lerp_value <= 1.0,
            "lerp_value = {lerp_value} should be capped at 1.0"
        );
        let expected = 1.0 - 1.0 / (1.0 + 0.25);
        assert!(
            (lerp_value - expected).abs() < 1e-5,
            "lerp_value = {lerp_value} but expected ~{expected}"
        );
    }
}
