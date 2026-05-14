//! bevy-naadf — Bevy 0.19 port of the NAADF voxel renderer (production binary).
//!
//! Thin shim over [`bevy_naadf::build_app`]: all the app wiring lives in
//! `src/lib.rs` so this production binary and the e2e render-test binary
//! (`src/bin/e2e_render.rs`) build the *same* app
//! (`docs/orchestrate/naadf-bevy-port/e2e-render-test.md` §9, §11 step 1).

use bevy::prelude::AppExit;
use bevy_naadf::{build_app, AppConfig};

fn main() -> AppExit {
    build_app(AppConfig::windowed()).run()
}
