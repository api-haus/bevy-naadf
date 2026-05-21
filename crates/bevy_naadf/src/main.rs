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
//! - `--vox <path>` — load a voxel file at startup. The file format is
//!   auto-detected from the first 4 magic bytes (see
//!   `voxel/voxel_dispatch.rs`): MagicaVoxel `.vox` (`"VOX "`) or NAADF
//!   `.cvox` (`"PK\x03\x04"` ZIP local file header). The flag name + path
//!   shape stay unchanged for source-stability; the parser routing happens
//!   on file content, not the path extension. The model is auto-tiled into
//!   the fixed `(256, 32, 256)`-chunk world (matches C# `generatorModel.fx`);
//!   load failures log + fall back to the embedded primitive scene. Minimal
//!   `std::env::args` parsing — no `clap`.

use bevy::prelude::AppExit;
use bevy_naadf::{AppArgs, AppConfig, GridPreset};

fn main() -> AppExit {
    // vox-gpu-rewrite Stage 2 consolidation (2026-05-18): the production
    // binary and every e2e gate route through the SAME C#-faithful fixed-
    // size world install path. `AppArgs::fixed_world_size` is gone; there's
    // no per-binary divergence to configure.
    let mut args = AppArgs::default();

    let argv: Vec<String> = std::env::args().skip(1).collect();

    if let Some(idx) = argv.iter().position(|a| a == "--vox") {
        if let Some(path) = argv.get(idx + 1) {
            args.grid_preset = GridPreset::Vox {
                path: std::path::PathBuf::from(path),
            };
        } else {
            eprintln!("error: --vox flag requires a path argument");
            return AppExit::error();
        }
    }

    // Native: sync probe path (`probe_and_select` → spin up a throwaway Bevy
    // render App, read `device.limits()`, drop it). Picks canonical defaults
    // on desktop with a ≥ 1.35 GiB storage-buffer-binding cap; picks mobile
    // rungs on Android Mali (256 MiB).
    #[cfg(not(target_arch = "wasm32"))]
    {
        bevy_naadf::build_app_with_budget(AppConfig::windowed(), args).run()
    }

    // wasm32 (web / iOS Safari / Android Chrome): async probe path. The
    // Bevy plugin-pyramid sync probe deadlocks the browser main thread on
    // `Atomics.wait` (RenderPlugin device creation is async). Instead, call
    // `wgpu::Instance::request_adapter` directly via `wasm_bindgen_futures::
    // spawn_local`, read the REAL `adapter.limits()`, then build + run the
    // App. Desktop Chrome on a workstation reports 2-4 GiB cap → canonical
    // defaults selected; iOS Safari + Android Chrome report 256 MiB → mobile
    // rungs. `main` returns AppExit::Success immediately; the spawned future
    // does the actual App boot via the wasm event loop.
    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_futures::spawn_local(async move {
            let caps = bevy_naadf::render::budget::probe_and_select_async().await;
            // Step 2 of the config-as-resource refactor — the TAA sample-ring
            // depth lives on `BootstrapInputs.taa_ring_depth: TaaRingConfig`
            // now, fanned out into a main-world resource by
            // `build_app_with_bootstrap_inputs`. Legacy `AppArgs` fields ride
            // along via `inputs.args` until subsequent steps migrate them.
            let inputs = bevy_naadf::bootstrap::BootstrapInputs {
                args,
                taa_ring_depth: bevy_naadf::render::taa::TaaRingConfig {
                    depth: caps.taa_ring_depth,
                },
            };
            let mut app = bevy_naadf::bootstrap::build_app_with_bootstrap_inputs(
                AppConfig::windowed(),
                inputs,
            );
            app.insert_resource(
                bevy_naadf::render::budget::EffectiveWorldSize::from_segments(
                    caps.world_size_in_segments,
                ),
            );
            app.insert_resource(bevy_naadf::render::budget::InvalidSampleStorageCount(
                caps.invalid_sample_storage_count,
            ));
            app.run();
        });
        AppExit::Success
    }
}
