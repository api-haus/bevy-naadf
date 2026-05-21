//! Global app-mode state — drives whether the player is interacting with the
//! world (`Playing`) or with the settings overlay (`Settings`).
//!
//! Uses Bevy 0.19's `bevy_state`:
//!
//! - Brush input (`editor::apply_edit_tool`) is gated with
//!   `.run_if(in_state(AppMode::Playing))`.
//! - Camera input is gated by removing the `FreeCamera` component on
//!   `OnEnter(Settings)` and re-inserting it on `OnExit(Settings)` — see
//!   [`suspend_camera_input`] / [`restore_camera_input`]. We toggle the
//!   component (rather than using `DisableOnEnter`/`EnableOnExit` which
//!   would add Bevy's `Disabled` marker) because `Disabled` on a
//!   `Camera3d` entity blanks the entire screen (render extraction queries
//!   skip it).
//! - The settings overlay's visibility is toggled via
//!   `OnEnter(AppMode::Settings)` / `OnExit(AppMode::Settings)` schedules.

use bevy::camera::Camera3d;
use bevy::camera_controller::free_camera::FreeCamera;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::state::state::States;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::camera::default_free_camera;

/// Global app mode — exactly two states: the player is either playing the game
/// or has the settings overlay open. Default = `Playing` so a fresh app boots
/// straight into gameplay (no menu-first frame).
#[derive(States, Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub enum AppMode {
    /// Camera + editing input live; settings overlay hidden.
    #[default]
    Playing,
    /// Settings overlay visible + interactive; camera entity carries
    /// `Disabled` so `FreeCameraPlugin`'s queries skip it; brush input is
    /// gated off by `run_if(in_state(Playing))` on `apply_edit_tool`.
    Settings,
}

/// `Update` system: Escape just-pressed flips `AppMode`. No-ops if the key
/// isn't just-pressed (debounce-free — `just_pressed` only true on the
/// transition frame).
pub fn toggle_settings_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    current: Res<State<AppMode>>,
    mut next: ResMut<NextState<AppMode>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let new_mode = match current.get() {
        AppMode::Playing => AppMode::Settings,
        AppMode::Settings => AppMode::Playing,
    };
    info!(target: "app_mode", "Escape pressed — {:?} -> {:?}", current.get(), new_mode);
    next.set(new_mode);
}

/// `OnEnter(AppMode::Settings)` system — strip `FreeCamera` from the camera
/// entity so `FreeCameraPlugin`'s input queries skip it (camera entity stays
/// otherwise intact so rendering continues), AND release the cursor so the
/// player can interact with the settings overlay with the mouse.
///
/// `FreeCameraPlugin`'s cursor-grab is event-driven (RMB just_pressed /
/// just_released), so it only writes `CursorOptions` on a transition. While
/// the plugin's systems are skipped (no `FreeCamera` component), the cursor
/// would otherwise stay grabbed if it was grabbed at the moment of Esc — we
/// force-release it here.
pub fn suspend_camera_input(
    mut commands: Commands,
    camera: Query<Entity, With<Camera3d>>,
    mut window_cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    for entity in &camera {
        commands.entity(entity).remove::<FreeCamera>();
    }
    if let Ok(mut cursor) = window_cursor.single_mut() {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// `OnExit(AppMode::Settings)` system — re-insert `FreeCamera` so the
/// player can fly the camera again. `FreeCameraState` is left in place by
/// the remove (Bevy's required-components don't auto-despawn on parent
/// removal), so the camera resumes from its last pose.
pub fn restore_camera_input(
    mut commands: Commands,
    camera: Query<Entity, With<Camera3d>>,
) {
    for entity in &camera {
        commands.entity(entity).insert(default_free_camera());
    }
}

/// Plugin owning the global `AppMode` state + the Escape toggle. Prepared by
/// D2 (codebase-tightening side-note 11) ahead of D7's `lib.rs` decomposition;
/// D7 wires this via `app.add_plugins(AppModePlugin)` in place of the inline
/// registration block at `lib.rs:900-971`.
pub struct AppModePlugin;

impl Plugin for AppModePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppMode>()
            .add_systems(Update, toggle_settings_on_escape);
    }
}
