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
//!
//! - `--e2e-brp <port>` — boot as the e2e *system-under-test*: select
//!   `AppConfig::e2e_sut` (the e2e determinism profile) and, when built
//!   `--features e2e-brp`, install the Bevy Remote Protocol HTTP server on
//!   `127.0.0.1:<port>` so the external `naadf_e2e` runner can drive the app.
//!   Native-only. Skips the production GPU budget probe — the SUT forces the
//!   canonical memory budget for deterministic SSIM (see the boot path below).
//! - `--e2e-window <w>x<h>` — override the SUT window size (default 256×256).
//!   Only meaningful alongside `--e2e-brp`.
//! - `--e2e-vox-oracle-cpu` — boot-time knob for the BRP-driven
//!   `vox_gpu_oracle` compare gate's **CPU-oracle phase**: routes a `--vox`
//!   load through the test-only `install_vox_sized_to_model` natural-bound CPU
//!   loader (`E2eGateMode::VoxGpuOracleCpu`) instead of the production W5 GPU
//!   producer chain. Boot-time because `setup_test_grid` reads it at
//!   `Startup`; rides the spawn contract per Forbidden Move #4. Only
//!   meaningful alongside `--e2e-brp` + `--vox`.
//! - `--e2e-entities` — boot-time knob for the BRP-driven `entities` gate:
//!   spawns the Phase-C 4×4×4 emissive-voxel test fixture
//!   (`SpawnTestEntity(true)`) and enables the W4 entity track
//!   (`ConstructionConfig.entities_enabled = true`). Boot-time config — both
//!   are consumed before `app.run()` — so it rides the spawn contract. Only
//!   meaningful alongside `--e2e-brp`.
//! - `--e2e-empty-world` — boot-time knob for the BRP-driven `vox_web_parity`
//!   gate's **skybox-baseline phase**: installs `GridPreset::Empty` (an empty
//!   `WorldData`, no `ModelData`, pure-sky render) instead of the default
//!   embedded test scene. `setup_test_grid` reads `GridPreset` at `Startup`,
//!   so it rides the spawn contract. Mutually exclusive with `--vox` (a
//!   `--vox` path wins). Only meaningful alongside `--e2e-brp`.
//! - `--e2e-resizable` — boot-time knob for the BRP-driven `resize_test` gate:
//!   makes the SUT window **user-resizable** (`Window.resizable = true`) and
//!   pins its Wayland `app_id` / X11 `WM_CLASS` to `bevy_naadf_e2e`
//!   (`Window.name`). Both are window-creation attributes, so they ride the
//!   spawn contract. `resizable: true` is required for winit to advertise a
//!   resizable surface (mirrors the legacy `WindowConfig::e2e_resize_test`);
//!   the deterministic `app_id` lets the `resize_test` gate target the window
//!   with a Hyprland `float on` windowrule so a tiling compositor does not
//!   refuse the `naadf/resize_window` verb's resize (see the gate's module
//!   doc for the full D10 finding). Only meaningful alongside `--e2e-brp`.

use bevy::prelude::AppExit;
use bevy_naadf::{AppConfig, GridPreset};

fn main() -> AppExit {
    // vox-gpu-rewrite Stage 2 consolidation (2026-05-18): the production
    // binary and every e2e gate route through the SAME C#-faithful fixed-
    // size world install path — no per-binary divergence to configure.
    //
    // Step 5 of the config-as-resource refactor: `grid_preset` is a
    // per-domain resource. `--vox <path>` writes `BootstrapInputs.grid_preset`
    // (native) or the main-thread bootstrap reads `?skybox=1` to write the
    // same field (wasm32) BEFORE the App is built. Step 9 removed the last
    // `AppArgs` reference — every config value is now a per-domain resource.
    let mut grid_preset = GridPreset::default();

    let argv: Vec<String> = std::env::args().skip(1).collect();

    if let Some(idx) = argv.iter().position(|a| a == "--vox") {
        if let Some(path) = argv.get(idx + 1) {
            grid_preset = GridPreset::Vox {
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
        // --- e2e SUT spawn contract (e2e-ipc-rpc-restructure, Phase 1) -------
        // `--e2e-brp <port>` boots the production binary as the system-under-
        // test for the external BRP-driven e2e runner: it selects
        // `AppConfig::e2e_sut` instead of `AppConfig::windowed()` (the e2e
        // determinism profile + the BRP server on `port`). `--e2e-window
        // <w>x<h>` optionally overrides the SUT window size (default 256×256
        // from the e2e profile). Both are hand-parsed alongside `--vox`,
        // matching `main.rs`'s "no `clap`" doctrine (design §5). Native-only:
        // the spawn contract / BRP transport are native-only (design §3
        // forbidden moves) — the wasm arm never reads these flags.
        //
        // The BRP server itself is behind the `e2e-brp` cargo feature; with
        // that feature off `AppConfig::e2e_sut`'s `brp_port` is read by no
        // compiled code. The flags are still parsed (so a typo'd flag fails
        // cleanly with a clear error rather than silently changing the
        // production boot) and `e2e_sut` still selects the determinism profile
        // — but a default-feature build launched with `--e2e-brp` simply runs
        // the determinism profile with no remote-control socket. The runner
        // always builds the SUT `--features e2e-brp`, so this is a
        // developer-ergonomics edge, not the real path.
        let mut e2e_brp_port: Option<u16> = None;
        if let Some(idx) = argv.iter().position(|a| a == "--e2e-brp") {
            match argv.get(idx + 1).map(|p| p.parse::<u16>()) {
                Some(Ok(port)) => e2e_brp_port = Some(port),
                _ => {
                    eprintln!("error: --e2e-brp flag requires a numeric port argument");
                    return AppExit::error();
                }
            }
        }
        let mut e2e_window: Option<(u32, u32)> = None;
        if let Some(idx) = argv.iter().position(|a| a == "--e2e-window") {
            match argv.get(idx + 1).and_then(|spec| parse_window_spec(spec)) {
                Some(dims) => e2e_window = Some(dims),
                None => {
                    eprintln!(
                        "error: --e2e-window flag requires a `<width>x<height>` \
                         argument (e.g. 1280x720)"
                    );
                    return AppExit::error();
                }
            }
        }

        if let Some(port) = e2e_brp_port {
            // e2e SUT boot. Route through the bootstrap fan-out directly —
            // NOT `build_app_with_budget` — so the SUT FORCES the canonical
            // memory budget and skips the production `probe_and_select`,
            // exactly as the legacy `e2e_render` path does (`lib.rs`
            // `run_e2e_render` → `build_app`, which bypasses the probe).
            // Rationale: e2e gates need canonical world / TAA rungs for
            // deterministic SSIM across runs and machines (the design's
            // hard-gate resolution; `lib.rs` `build_app_with_budget` doc).
            let mut cfg = AppConfig::e2e_sut(port);
            if let Some((w, h)) = e2e_window {
                cfg.window.resolution = Some((w as f32, h as f32));
            }
            // `--e2e-resizable` — make the SUT window user-resizable AND pin
            // its app_id. The `resize_test` gate's `naadf/resize_window` verb
            // drives a winit `request_inner_size`; winit advertises a
            // fixed-size surface (and a Wayland compositor refuses the resize)
            // unless `resizable` is `true`, and a *tiling* compositor refuses
            // it for any non-floating window — so the gate also installs a
            // `float on` windowrule, which needs a deterministic app_id to
            // target. Both `resizable` and `name` are window-creation
            // attributes → boot-time config → spawn contract (this mirrors
            // the legacy `WindowConfig::e2e_resize_test`, which set the same
            // two fields).
            if argv.iter().any(|a| a == "--e2e-resizable") {
                cfg.window.resizable = true;
                cfg.window.name = Some("bevy_naadf_e2e");
            }

            // --- boot-time-knob spawn flags (e2e-ipc-rpc-restructure Phase 3b)
            // Two `vox_gpu_oracle` / `entities` gate knobs are consumed before
            // `app.run()` (`setup_test_grid` reads `E2eGateMode` at `Startup`;
            // `spawn_phase_c_test_entity` reads `SpawnTestEntity` at `Startup`)
            // — so per Forbidden Move #4 they ride the spawn contract, not a
            // BRP verb. Bare presence flags, hand-parsed like the others.

            // `--e2e-vox-oracle-cpu` selects the test-only CPU-oracle install
            // branch: `setup_test_grid` routes a `Vox` load through
            // `install_vox_sized_to_model` (natural-bound CPU loader) when
            // `E2eGateMode::VoxGpuOracleCpu` is set. The BRP-driven
            // `vox_gpu_oracle` compare gate spawns one SUT with this flag (the
            // CPU phase) and one without (the production W5 GPU phase).
            let e2e_vox_oracle_cpu = argv.iter().any(|a| a == "--e2e-vox-oracle-cpu");

            // `--e2e-entities` spawns the Phase-C 4×4×4 emissive-voxel test
            // fixture + enables the W4 entity track — the boot-time config the
            // legacy `e2e_render --entities` (`EntitiesBoot` arm) sets on its
            // `BootstrapInputs`.
            let e2e_entities = argv.iter().any(|a| a == "--e2e-entities");

            // `--e2e-empty-world` installs `GridPreset::Empty` (pure-sky
            // baseline) — the skybox phase of the BRP-driven `vox_web_parity`
            // compare gate. A `--vox` path wins (the loaded phase passes
            // `--vox` and never `--e2e-empty-world`).
            if argv.iter().any(|a| a == "--e2e-empty-world")
                && !matches!(grid_preset, GridPreset::Vox { .. })
            {
                grid_preset = GridPreset::Empty;
            }

            let gate_mode = if e2e_vox_oracle_cpu {
                bevy_naadf::e2e::gate::E2eGateMode::VoxGpuOracleCpu
            } else {
                bevy_naadf::e2e::gate::E2eGateMode::default()
            };

            let mut construction_config =
                bevy_naadf::render::construction::ConstructionConfig::for_target_arch();
            if e2e_entities {
                construction_config.entities_enabled = true;
            }

            let inputs = bevy_naadf::bootstrap::BootstrapInputs {
                grid_preset,
                gate_mode,
                construction_config,
                spawn_test_entity:
                    bevy_naadf::render::construction::SpawnTestEntity(e2e_entities),
                ..Default::default()
            };
            return bevy_naadf::bootstrap::build_app_with_bootstrap_inputs(cfg, inputs)
                .run();
        }
        bevy_naadf::build_app_with_budget(
            AppConfig::windowed(),
            grid_preset,
        )
        .run()
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
        // Step 5 of the config-as-resource refactor — relocate the
        // `?skybox=1` URL-param resolution OUT of
        // `voxel::web_vox::startup_fetch_default_vox` (which used to mutate
        // `args.grid_preset` at `Startup` time) INTO the wasm32 bootstrap.
        // Read the URL param on the main thread before the App is built;
        // write `GridPreset::WebSkybox` directly into `BootstrapInputs`.
        // The `?pose=horizon` / `?ui=hide` resolvers stay where they are
        // — they insert separate marker resources at `Startup` time.
        let mut grid_preset = grid_preset;
        if bevy_naadf::voxel::web_vox::resolve_skybox_only_param() {
            grid_preset = GridPreset::WebSkybox;
        }
        wasm_bindgen_futures::spawn_local(async move {
            let caps = bevy_naadf::render::budget::probe_and_select_async().await;
            // Step 2/5 of the config-as-resource refactor — the TAA
            // sample-ring depth and the grid preset both live on
            // `BootstrapInputs`, fanned out into per-domain main-world
            // resources by `build_app_with_bootstrap_inputs`. Step 9 drained
            // the last `AppArgs` field, so every config value is a typed
            // per-domain field on `BootstrapInputs` now.
            let inputs = bevy_naadf::bootstrap::BootstrapInputs {
                grid_preset,
                taa_ring_depth: bevy_naadf::render::taa::TaaRingConfig {
                    depth: caps.taa_ring_depth,
                },
                ..Default::default()
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

/// Parse a `--e2e-window` argument of the form `<width>x<height>`
/// (e.g. `1280x720`) into `(width, height)`. Returns `None` for any
/// malformed spec — missing `x`, a non-numeric or zero component, or extra
/// segments. Native-only: the `--e2e-window` flag is part of the native-only
/// e2e SUT spawn contract (design §5).
#[cfg(not(target_arch = "wasm32"))]
fn parse_window_spec(spec: &str) -> Option<(u32, u32)> {
    let (w, h) = spec.split_once('x')?;
    let w: u32 = w.parse().ok()?;
    let h: u32 = h.parse().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}
