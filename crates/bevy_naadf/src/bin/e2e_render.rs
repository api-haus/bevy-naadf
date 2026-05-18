//! `cargo run --bin e2e_render` — the bounded windowed end-to-end render test.
//!
//! The whole binary: boot the real `DefaultPlugins` + `WinitPlugin` windowed
//! app via [`bevy_naadf::run_e2e_render`], run the render graph for a fixed
//! frame budget, read the on-screen framebuffer back, run the per-batch region
//! gates + the `PipelineCache` error scan + the node-dispatch check, and exit
//! 0 on success / non-zero on failure.
//!
//! `fn main() -> ExitCode` folds the e2e's `AppExit` + the optional Phase-C
//! validation result into a single explicit numeric exit code (W0 switched
//! away from `AppExit: Termination` so this binary has one mapping site).
//!
//! ## Phase-C flag — `--validate-gpu-construction` (`15-design-c.md` §1.6, W1)
//!
//! W0 plumbed the flag end-to-end with a placeholder body; **W1 fills the
//! body** with the real bit-exact CPU/GPU oracle gate.
//!
//! ## Phase-C W2 flag — `--edit-mode` (`15-design-c.md` §2.1 W2 row)
//!
//! Runs the CPU-side editing chain end-to-end against a small fixed scene:
//! builds a 4×2×4-chunk world, applies a single `set_voxel` call at a known
//! position with a known new type, then asserts:
//!   - `WorldData::pending_edits.batches` is non-empty (the edit produced
//!     a batch).
//!   - `WorldData::chunks_cpu` was mutated (the edit reached the CPU mirror).
//!   - The flood-fill CPU oracle produces the expected `changed_groups`
//!     entries.
//!
//! Until wave-3 wires the full render-graph dispatch path so the edit is
//! *visible* in the screenshot, this CPU validation is the integration-level
//! W2 e2e gate. The GPU bit-exact validation lives in the `world_change::tests`
//! unit-test module (which boots a headless render world + runs the actual
//! `world_change.wgsl` shader passes against the CPU oracles).
//!
//! ## Phase-C W4 flag — `--entities` (`15-design-c.md` §2.1 W4 row)
//!
//! Runs the CPU-side `EntityHandler::update` against a small fixed-pose
//! moving-entity scene and asserts the per-frame uploads are non-empty +
//! self-consistent (deterministic). Until wave-3 wires the render-side
//! dispatch, this flag exercises the W4 CPU port (overlap counting +
//! prefix-sum + dedup-hash + the smallest-three quaternion compression);
//! the GPU pipelines themselves are exercised by the unit test
//! `entity_update_gpu_smoke` (compiles them; no full render run).
//!
//! When the flag is set, after the normal e2e exits, the binary runs
//! `bevy_naadf::render::construction::validate_gpu_construction` which boots a
//! headless render world, runs `chunk_calc.wgsl`'s 3 production entry points
//! (Algorithm 1, voxel-bound, block-bound) against a deterministic 1×1×1
//! chunk world with a single mixed block, then maps the GPU `blocks` /
//! `voxels` / chunks-texture buffers back to CPU and asserts byte-equality
//! with the CPU oracle `aadf::construct::construct`. On success the binary
//! prints `GPU construction byte-equal to CPU oracle: N bytes compared`; on
//! failure it prints the mismatch + exits non-zero.
//!
//! The validation scene is intentionally small (the 1×1×1 single-voxel case
//! exercises every shader code-path with deterministic `VoxelPtr(0)` /
//! `BlockPtr(0)` assignment) — `15-design-c.md` §1.6 assumption #7 flags that
//! on larger scenes CPU `HashMap` iteration order diverges from GPU
//! open-addressing-by-hash, breaking byte-equality even though semantic
//! equality holds. W1's gate proves the algorithm is correct on the
//! deterministic case; semantic-equality validation on `GridPreset::Default`
//! is a W2/W3 follow-up.
//!
//! See `docs/orchestrate/naadf-bevy-port/e2e-render-test.md` for the full
//! e2e design + `16-impl-c-W1.md` for W1's validation specifics.

use std::process::ExitCode;

use bevy::prelude::AppExit;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Parse the CLI flags — `--validate-gpu-construction` (W1) +
    // `--entities` (W4) + `--edit-mode` (W2) + `--resize-test`
    // (resize-blackness reproduction — see
    // `docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
    // `## GI-bounce-on-resize fix (2026-05-16)`) + `--vox-e2e`
    // (synthesised-.vox regression gate — see
    // `docs/orchestrate/feature-completeness/03a-impl-vox-loading.md` —
    // `## E2E gate addendum`), default off.
    let validate_gpu_construction = args.iter().any(|a| a == "--validate-gpu-construction");
    let validate_gpu_construction_scaled =
        args.iter().any(|a| a == "--validate-gpu-construction-scaled");
    // vox-gpu-rewrite Stage 9 — production-scale voxels[] readback diagnostic.
    // Loads the real Oasis VOX fixture, runs the FULL W5 producer chain at
    // production scale (256×32×256 chunk fixed world, 512 segments, full
    // bounds chain), reads back voxels[] at TWO checkpoints (post-producer,
    // post-bounds), and diffs against the CPU oracle at ~25 sampled
    // Oasis-populated voxel positions. Discriminating test for whether the
    // visible "voxel types in thousands" bug is in the producer/bounds path
    // (corrupted voxels[]) or in the renderer's decode path
    // (voxels[] byte-correct but read wrongly). See
    // `docs/orchestrate/vox-gpu-rewrite/15-diagnostic-production-scale-readback.md`.
    let validate_gpu_construction_production =
        args.iter().any(|a| a == "--validate-gpu-construction-production");
    let entities_mode = args.iter().any(|a| a == "--entities");
    let edit_mode = args.iter().any(|a| a == "--edit-mode");
    let runtime_edit_mode = args.iter().any(|a| a == "--runtime-edit-mode");
    let resize_test = args.iter().any(|a| a == "--resize-test");
    let vox_e2e_mode = args.iter().any(|a| a == "--vox-e2e");
    let oasis_edit_visual_mode = args.iter().any(|a| a == "--oasis-edit-visual");
    let small_edit_visual_mode = args.iter().any(|a| a == "--small-edit-visual");
    let small_edit_repro_mode = args.iter().any(|a| a == "--small-edit-repro");
    let vox_gpu_construction_mode =
        args.iter().any(|a| a == "--vox-gpu-construction");
    // vox-gpu-rewrite W5.3-fix Stage 4 — three-flag oracle gate:
    //   --vox-gpu-oracle           = the top-level mode: spawn the CPU + GPU
    //                                phases as subprocesses, then compare the
    //                                two PNGs per-pixel.
    //   --vox-gpu-oracle-cpu       = single-phase CPU oracle render (called
    //                                by the top-level mode via subprocess).
    //   --vox-gpu-oracle-gpu       = single-phase GPU producer render.
    // See `bevy_naadf::e2e::vox_gpu_oracle` for the gate design + camera pose.
    let vox_gpu_oracle_mode = args.iter().any(|a| a == "--vox-gpu-oracle");
    let vox_gpu_oracle_cpu_mode = args.iter().any(|a| a == "--vox-gpu-oracle-cpu");
    let vox_gpu_oracle_gpu_mode = args.iter().any(|a| a == "--vox-gpu-oracle-gpu");
    // PBR-raymarching visual gate (`02-design.md` § I) — side-on metallic-
    // pillar view of the default test grid, single screenshot, three PBR
    // signal assertions (specular highlight luma, textured-albedo variation,
    // metallic F0 colour-pull).
    let pbr_visual_mode = args.iter().any(|a| a == "--pbr-visual");
    // PBR rendering-debugger gate
    // (`docs/orchestrate/pbr-raymarching/05-diagnostic.md` § "PBR rendering
    // debugger"). Iterates every non-zero `DebugViewMode`, captures a
    // per-mode framebuffer, asserts each is non-degenerate.
    let pbr_debug_modes_mode = args.iter().any(|a| a == "--pbr-debug-modes");

    // Phase-C wave-3 — when `--entities` is set, override `AppArgs` to enable
    // the W4 entity track (`entities_enabled = true`) AND spawn the fixture
    // entity (`spawn_test_entity = true`). The Startup
    // `spawn_phase_c_test_entity` system populates `MainWorldEntities`; the
    // render pipeline then dispatches `entity_update.wgsl` per-frame and
    // `ray_tracing.wgsl::shoot_ray`'s entity sub-traversal renders the
    // fixture into the framebuffer.
    //
    // `--resize-test` — sets `AppArgs.resize_test = true`. The e2e driver
    // then runs the resize-blackness reproduction phases instead of the
    // standard Warmup→Motion→Settle→Shoot flow: boot at 800×600, settle,
    // screenshot, hyprctl-resize to 1920×1080, settle, screenshot,
    // hyprctl-resize to 2000×1000, settle, screenshot, then compare
    // full-frame luma ratios against an `E2E_RESIZE_MIN_LUMA_RATIO = 0.7`
    // threshold. Reproduces the GI-bounce-on-resize bug (wgpu indirect
    // dispatch limit overflow at viewport sizes ≥ 1920×1080); fixed by
    // capping `padded_*_group_count` at 32 768 in `sample_refine.wgsl`.
    // See `docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
    // `## GI-bounce-on-resize fix (2026-05-16)`.
    // vox-gpu-rewrite W5.3-fix Stage 4 — top-level oracle gate. Returns its
    // own exit code WITHOUT booting a bevy app (the compare phase spawns the
    // two render phases as subprocesses of THIS binary). Handled at the very
    // top of the dispatch so the standard `e2e_render` flow doesn't fight
    // over `app_exit`.
    if vox_gpu_oracle_mode {
        let code = bevy_naadf::e2e::vox_gpu_oracle::run_vox_gpu_oracle_compare();
        return ExitCode::from(code);
    }

    // vox-gpu-rewrite Stage 6 — concrete byte-diff diagnostic. Short-circuits
    // before booting the e2e binary; runs a fixture sweep through the W5
    // chunk_calc chain and prints first-divergent-index per buffer (raw +
    // semantic). See `docs/orchestrate/vox-gpu-rewrite/12-diagnostic-byte-diff-concrete.md`.
    if validate_gpu_construction_scaled {
        match bevy_naadf::render::construction::validate_gpu_construction_scaled() {
            Ok(_report) => {
                return ExitCode::from(0);
            }
            Err(msg) => {
                eprintln!("scaled byte-diff diagnostic FAILED: {msg}");
                return ExitCode::from(1);
            }
        }
    }

    // vox-gpu-rewrite Stage 9 — production-scale voxels[] readback diagnostic.
    // Short-circuits before booting the e2e binary. See module-level docs at
    // `crate::render::construction::validate_gpu_construction_production_scale`
    // and `docs/orchestrate/vox-gpu-rewrite/15-diagnostic-production-scale-readback.md`.
    if validate_gpu_construction_production {
        match bevy_naadf::render::construction::validate_gpu_construction_production_scale() {
            Ok(_report) => {
                return ExitCode::from(0);
            }
            Err(msg) => {
                eprintln!("production-scale readback diagnostic FAILED: {msg}");
                return ExitCode::from(1);
            }
        }
    }

    let app_exit = if resize_test {
        // resize-blackness: pre-launch — install a Hyprland windowrule so
        // the e2e_render window starts ALREADY-FLOATING (no togglefloating
        // dance after the fact). Pixel-precise resize via
        // `hyprctl dispatch resizewindowpixel` only takes effect on floating
        // windows; the prior togglefloating-after-launch approach was unreliable
        // because Hyprland's default behaviour or user windowrules could leave
        // the window tiled (or re-tile it after toggling). A pre-launch
        // windowrule sidesteps the race entirely.
        //
        // Hyprland 0.54+ syntax: `match:class <regex>, float on` (the older
        // `windowrulev2 float,class:^(...)$` is deprecated). Verified against
        // the live `hyprctl --help` + `hyprctl keyword windowrule "..."` on
        // 2026-05-15.
        //
        // Cleanup uses `hyprctl reload` (after the run) to re-read the config
        // from disk, which discards every runtime keyword set since boot. If
        // the test panics the rule leaks until the next manual `hyprctl reload`
        // / Hyprland restart — explicitly acceptable per the dispatch brief.
        //
        // Both invocations are gated behind `--resize-test` so the standard
        // e2e path never shells out to hyprctl.
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            let status = std::process::Command::new("hyprctl")
                .args([
                    "keyword",
                    "windowrule",
                    "match:class ^(e2e_render)$, float on",
                ])
                .status();
            match status {
                Ok(s) => eprintln!(
                    "e2e_render: pre-launch hyprctl keyword windowrule \
                     'match:class ^(e2e_render)$, float on' -> {s:?}"
                ),
                Err(e) => eprintln!(
                    "e2e_render: pre-launch hyprctl keyword windowrule \
                     FAILED to spawn: {e} — test will likely fall back to \
                     tiled behaviour and assert via luma comparison"
                ),
            }
        } else {
            eprintln!(
                "e2e_render: pre-launch — HYPRLAND_INSTANCE_SIGNATURE not set; \
                 skipping windowrule install (driver will abort the run)"
            );
        }

        let mut app_args = bevy_naadf::AppArgs::default();
        app_args.resize_test = true;
        let exit = bevy_naadf::run_e2e_render_with_args(app_args);

        // Cleanup: discard the runtime windowrule by reloading the config
        // from disk. Best-effort — failure here doesn't change the test's
        // pass/fail verdict, it just leaves a runtime rule until the user
        // reloads Hyprland.
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            let status = std::process::Command::new("hyprctl")
                .args(["reload"])
                .status();
            match status {
                Ok(s) => eprintln!("e2e_render: post-run hyprctl reload -> {s:?}"),
                Err(e) => eprintln!(
                    "e2e_render: post-run hyprctl reload FAILED to spawn: {e} \
                     — runtime windowrule may persist until next reload"
                ),
            }
        }

        exit
    } else if oasis_edit_visual_mode {
        // `02f-followup` — visual-diff edit-pipeline gate. Loads the
        // Oasis VOX fixture from
        // `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` (Git LFS
        // tracked), pins a birdseye camera over the world centre,
        // captures frame A, programmatically erases a sphere via the
        // production `sphere_brush` (runtime path), waits ~5 s for the
        // W2 GPU dispatch + GI / TAA to converge, captures frame B, and
        // asserts the bounding-box mean per-pixel delta exceeds
        // [`OASIS_EDIT_DIFF_FLOOR`]. See [`bevy_naadf::e2e::oasis_edit_visual`].
        bevy_naadf::e2e::oasis_edit_visual::run_oasis_edit_visual()
    } else if small_edit_visual_mode {
        // `03g` — single-voxel-edit gate. Boots the default test grid,
        // pins a birdseye camera, captures frame A, snapshots non-empty
        // voxel count, applies a `cube_brush(radius=1.0)` at a known
        // empty voxel via the runtime path, asserts the voxel count rose
        // by exactly 1 (CPU pre-condition / Mode 2 catch), waits ~5 s,
        // captures frame B, asserts the click rect changed and adjacent
        // rects did not (framebuffer post-condition / Mode 1 catch).
        // See [`bevy_naadf::e2e::small_edit_visual`].
        bevy_naadf::e2e::small_edit_visual::run_small_edit_visual()
    } else if small_edit_repro_mode {
        // `2026-05-17` — user-captured single-voxel-edit reproduction.
        // Loads the Oasis VOX fixture, pins the camera to the user's
        // EDIT_REPRO-logged pose, runs the exact `cube_brush(radius=1)`
        // call the user made, then asserts no pitch-black pixels in the
        // 1920×1080 post-edit framebuffer. Catches the regression the
        // standard `--small-edit-visual` gate misses. See
        // [`bevy_naadf::e2e::small_edit_repro`].
        bevy_naadf::e2e::small_edit_repro::run_small_edit_repro()
    } else if vox_gpu_oracle_cpu_mode {
        // vox-gpu-rewrite W5.3-fix Stage 4 — CPU oracle phase. Loads Oasis
        // via the legacy `install_vox_sized_to_model` path, pins the shared
        // oracle camera pose, captures `target/e2e-screenshots/oracle_cpu.png`,
        // and exits. The top-level `--vox-gpu-oracle` mode spawns this as a
        // subprocess and pairs the output with the GPU phase.
        bevy_naadf::e2e::vox_gpu_oracle::run_vox_gpu_oracle_cpu_phase()
    } else if vox_gpu_oracle_gpu_mode {
        // vox-gpu-rewrite W5.3-fix Stage 4 — GPU phase. Loads Oasis via
        // `install_vox_in_fixed_world` (W5 GPU producer chain), pins the
        // shared oracle camera pose, captures `oracle_gpu.png`, exits.
        bevy_naadf::e2e::vox_gpu_oracle::run_vox_gpu_oracle_gpu_phase()
    } else if vox_gpu_construction_mode {
        // `--vox-gpu-construction` — load the Oasis fixture through the
        // production W5 GPU producer chain (vox-gpu-rewrite W5.5). Loads
        // `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` as
        // `ModelData`, runs `16 × 2 × 16 = 512` per-segment generator +
        // chunk_calc dispatches against the production WorldGpu buffers,
        // and asserts the framebuffer is not pure-black. See
        // `bevy_naadf::e2e::vox_gpu_construction` (+ the orchestration
        // bundle at `docs/orchestrate/vox-gpu-rewrite/`).
        bevy_naadf::e2e::vox_gpu_construction::run_vox_gpu_construction()
    } else if pbr_visual_mode {
        bevy_naadf::e2e::pbr_visual::run_pbr_visual()
    } else if pbr_debug_modes_mode {
        bevy_naadf::e2e::pbr_debug_modes::run_pbr_debug_modes()
    } else if vox_e2e_mode {
        // `--vox-e2e` — synthesise a 2-model `.vox` fixture in memory,
        // write it to `target/e2e-screenshots/vox_e2e_fixture.vox`, then
        // boot the e2e harness with `GridPreset::Vox { path: ... }` so
        // the production `--vox <path>` load path drives the test. The
        // driver swaps the default-scene region gate for the
        // `assert_vox_geometry_visible` non-skybox gate (the synthesised
        // fixture replaces the default voxel grid; the default-scene
        // gate rects don't apply). See
        // `docs/orchestrate/feature-completeness/03a-impl-vox-loading.md`
        // `## E2E gate addendum`.
        bevy_naadf::e2e::vox_e2e::run_vox_e2e()
    } else if entities_mode {
        let mut app_args = bevy_naadf::AppArgs::default();
        app_args.construction_config.entities_enabled = true;
        app_args.spawn_test_entity = true;
        bevy_naadf::run_e2e_render_with_args(app_args)
    } else {
        bevy_naadf::run_e2e_render()
    };

    let e2e_code = match app_exit {
        AppExit::Success => 0u8,
        AppExit::Error(code) => code.get(),
    };

    if validate_gpu_construction {
        match bevy_naadf::render::construction::validate_gpu_construction() {
            Ok(bytes_compared) => {
                eprintln!(
                    "GPU construction byte-equal to CPU oracle: {bytes_compared} bytes compared"
                );
                if e2e_code != 0 {
                    eprintln!(
                        "(e2e itself returned non-zero exit {e2e_code}; validation succeeded \
                         but the e2e failure is the load-bearing failure)"
                    );
                }
            }
            Err(msg) => {
                eprintln!("GPU construction validation FAILED: {msg}");
                return ExitCode::from(1);
            }
        }
    }

    if entities_mode {
        match bevy_naadf::render::construction::validate_entity_handler() {
            Ok(report) => {
                eprintln!("entity handler validation PASS: {report}");
            }
            Err(msg) => {
                eprintln!("entity handler validation FAILED: {msg}");
                return ExitCode::from(1);
            }
        }
    }

    if edit_mode {
        match bevy_naadf::render::construction::validate_edit_mode() {
            Ok(report) => {
                eprintln!("edit-mode validation PASS: {report}");
            }
            Err(msg) => {
                eprintln!("edit-mode validation FAILED: {msg}");
                return ExitCode::from(1);
            }
        }
    }

    // `02f` rearch — runtime-edit gate. Complements `--edit-mode` by
    // exercising the production brush path (`set_voxels_batch`); closes the
    // regression hole the pre-`02f` CPU-oracle-only `--edit-mode` left open
    // (edit-doesn't-reach-W2-batch). See `validate_runtime_edit_mode`'s
    // module-level doc for what is + isn't asserted by this gate.
    if runtime_edit_mode {
        match bevy_naadf::render::construction::validate_runtime_edit_mode() {
            Ok(report) => {
                eprintln!("runtime-edit gate PASS: {report}");
            }
            Err(msg) => {
                eprintln!("runtime-edit gate FAILED: {msg}");
                return ExitCode::from(1);
            }
        }
    }

    ExitCode::from(e2e_code)
}
