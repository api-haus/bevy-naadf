//! Embedded developer-font (`Roboto-Regular.ttf`) registration. The font is
//! Apache 2.0 — see `src/assets/fonts/Roboto-LICENSE.txt`.
//!
//! `hud`, `editor::hud`, and `settings` all `.after(load_dev_font)` so they
//! see the [`DevFont`] resource by the time they spawn `TextFont` nodes.

use bevy::prelude::*;

/// Roboto Regular TTF bytes, embedded at compile time. Apache 2.0 — see
/// `src/assets/fonts/Roboto-LICENSE.txt`.
static ROBOTO_REGULAR_BYTES: &[u8] =
    include_bytes!("assets/fonts/Roboto-Regular.ttf");

/// Main-world resource — the `FontSource` for the embedded Roboto Regular
/// font. `hud`, `editor::hud`, and `settings` all query this resource to set
/// `TextFont.font`.
///
/// To add a second font in future: add another `&[u8]` static + another field
/// here, load it in [`load_dev_font`], and store its `FontSource` alongside
/// this one.
#[derive(Resource)]
pub struct DevFont(pub FontSource);

/// `Startup` system: load the embedded Roboto Regular bytes into
/// `Assets<Font>` and insert the resulting `Handle<Font>` as the [`DevFont`]
/// resource.
///
/// Must run before `setup_hud` and `setup_panel` so those systems can resolve
/// the resource. Runs unconditionally in both windowed and e2e configs.
pub fn load_dev_font(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let font = Font::from_bytes(ROBOTO_REGULAR_BYTES.to_vec(), "Roboto");
    let handle = fonts.add(font);
    commands.insert_resource(DevFont(FontSource::Handle(handle)));
}

/// Wires [`load_dev_font`] into `Startup`. Lives in this small dedicated
/// plugin so `lib.rs::build_app_with_args` stays a thin spine.
pub struct DevFontPlugin;

impl Plugin for DevFontPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_dev_font);
    }
}
