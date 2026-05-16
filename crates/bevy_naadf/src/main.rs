//! bevy-naadf — Bevy 0.19 port of the NAADF voxel renderer (production binary).
//!
//! Thin shim over [`bevy_naadf::build_app_with_args`]: all the app wiring lives
//! in `src/lib.rs` so this production binary and the e2e render-test binary
//! (`src/bin/e2e_render.rs`) build the *same* app
//! (`docs/orchestrate/naadf-bevy-port/e2e-render-test.md` §9, §11 step 1).
//!
//! ## CLI flags
//!
//! - `--vox <path>` — Track A
//!   (`docs/orchestrate/feature-completeness/02a-design-vox-loading.md`):
//!   load a MagicaVoxel `.vox` file at startup instead of the hard-coded test
//!   grid. The path is read synchronously by
//!   `voxel/vox_import::load_vox`; failure logs + falls back to
//!   [`bevy_naadf::GridPreset::Default`] so the harness always boots into a
//!   renderable world. Minimal `std::env::args` parsing — no `clap`.
//! - `--vox-grid <N>` — tile the loaded `.vox` N×N times in the XZ plane.
//!   Default `1` (single tile, identical to pre-existing `--vox`). At `N=4`
//!   the world is 16 copies of the loaded `.vox`, matching C#'s startup
//!   behaviour that loads 4×4 Oasis_Hard_Cover.vox at boot
//!   (`docs/orchestrate/feature-completeness/03e-impl-dirty-fix-and-vox-grid.md`).

use bevy::prelude::AppExit;
use bevy_naadf::{build_app_with_args, AppArgs, AppConfig, GridPreset};

fn main() -> AppExit {
    let mut args = AppArgs::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Parse --vox-grid <N> first (independent of --vox order; we apply tiles
    // to whatever Vox preset gets set below).
    let mut tiles: u32 = 1;
    if let Some(idx) = argv.iter().position(|a| a == "--vox-grid") {
        if let Some(n_str) = argv.get(idx + 1) {
            match n_str.parse::<u32>() {
                Ok(n) if n >= 1 => tiles = n,
                Ok(_) => {
                    eprintln!("error: --vox-grid N requires N >= 1");
                    return AppExit::error();
                }
                Err(_) => {
                    eprintln!("error: --vox-grid expects a positive integer");
                    return AppExit::error();
                }
            }
        } else {
            eprintln!("error: --vox-grid flag requires an integer argument");
            return AppExit::error();
        }
    }

    if let Some(idx) = argv.iter().position(|a| a == "--vox") {
        if let Some(path) = argv.get(idx + 1) {
            args.grid_preset = GridPreset::Vox {
                path: std::path::PathBuf::from(path),
                tiles,
            };
        } else {
            eprintln!("error: --vox flag requires a path argument");
            return AppExit::error();
        }
    }
    build_app_with_args(AppConfig::windowed(), args).run()
}
