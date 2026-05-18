//! bevy-naadf — Bevy 0.19 port of the NAADF voxel renderer (production binary).
//!
//! Thin shim over [`bevy_naadf::build_app_with_args`]: all the app wiring lives
//! in `src/lib.rs` so this production binary and the e2e render-test binary
//! (`src/bin/e2e_render.rs`) build the *same* app
//! (`docs/orchestrate/naadf-bevy-port/e2e-render-test.md` §9, §11 step 1).
//!
//! ## C#-faithful world initialisation
//!
//! The production binary always boots into a fixed `(4096, 512, 4096)`-voxel
//! world (= `(256, 32, 256)` chunks). The four grid presets (`--grid-preset
//! default`/`vox`/`procedural-streaming`/`procedural-static`) all install into
//! the same fixed container — the install path is in
//! `crate::voxel::grid::setup_test_grid`.
//!
//! ## CLI parsing
//!
//! The clap parser lives in [`bevy_naadf::cli::Cli`]. The streaming-world
//! rearch (`docs/orchestrate/streaming-world/02d-design-cli-and-e2e-rearch.md`)
//! collapsed the old `std::env::args` manual scan + the e2e binary's parallel
//! dispatch ladder into a single shared parser; the e2e binary flattens this
//! parser into its own [`bevy_naadf::cli::E2eCli`] and adds `--gate <NAME>`.
//! See `--help` for the full flag list.

use bevy::prelude::AppExit;
use bevy_naadf::cli::Cli;
use bevy_naadf::{build_app_with_args, AppConfig};
use clap::Parser;

fn main() -> AppExit {
    let cli = Cli::parse();
    let args = cli.into_app_args();
    build_app_with_args(AppConfig::windowed(), args).run()
}
