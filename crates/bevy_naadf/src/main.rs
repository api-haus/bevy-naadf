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
//! world (= `(256, 32, 256)` chunks) and either embeds the small primitive
//! test scene at the world origin (no `--vox`) or auto-tiles a loaded `.vox`
//! file across the XZ plane (`--vox <path>`). Both paths mirror C#
//! `WorldHandler.Initialize` (`World/WorldHandler.cs:29-35`) +
//! `generatorModel.fx:16-52`'s `voxelPos % modelSize` tiling with `Y > 0` left
//! empty. The world is editable everywhere — empty cells included — exactly
//! the way C# behaves when `Content/oasis.cvox` is missing.
//!
//! ## CLI flags
//!
//! - `--vox <path>` — load a MagicaVoxel `.vox` file at startup. The model is
//!   sparse-walked + auto-tiled into the fixed `(256, 32, 256)`-chunk world
//!   (matches C# `generatorModel.fx`); load failures log + fall back to the
//!   embedded primitive scene. Minimal `std::env::args` parsing — no `clap`.

use bevy::prelude::AppExit;
use bevy_naadf::{build_app_with_args, AppArgs, AppConfig, GridPreset};

fn main() -> AppExit {
    let mut args = AppArgs::default();
    // Production binary is always C#-faithful — fixed-size world container,
    // model auto-tiling, identical to `WorldHandler.cs:29-35`. The e2e harness
    // leaves this `false` because its luminance gates are tuned to the legacy
    // small-world layout.
    args.fixed_world_size = true;

    let argv: Vec<String> = std::env::args().skip(1).collect();

    if let Some(idx) = argv.iter().position(|a| a == "--vox") {
        if let Some(path) = argv.get(idx + 1) {
            args.grid_preset = GridPreset::Vox {
                path: std::path::PathBuf::from(path),
                // `tiles` is ignored when `fixed_world_size = true` — the
                // loader fills the fixed-size world via `voxelPos %
                // modelSize` instead. Set to 1 for the resource shape.
                tiles: 1,
            };
        } else {
            eprintln!("error: --vox flag requires a path argument");
            return AppExit::error();
        }
    }
    build_app_with_args(AppConfig::windowed(), args).run()
}
