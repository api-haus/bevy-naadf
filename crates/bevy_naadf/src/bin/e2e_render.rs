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
//! ## Dispatch shape (D6 step 5 — codebase-tightening refactor)
//!
//! CLI args are parsed in three layers:
//!
//! 1. [`parse_top_level_short_circuit`] — the no-Bevy-boot commands
//!    (`--vox-gpu-oracle`, `--vox-web-parity`, `--ssim-compare`,
//!    `--validate-gpu-construction-scaled`, `--validate-gpu-construction-production`).
//!    These run + return an `ExitCode` directly without booting an app.
//! 2. [`parse_gate_command`] — the boot commands. Each maps to a single
//!    `bevy_naadf::e2e` gate entry point that constructs a
//!    `BootstrapInputs` carrying the gate's
//!    [`bevy_naadf::e2e::gate::E2eGateMode`] (Step 6 of the
//!    config-as-resource refactor collapsed the e2e-mode booleans into
//!    that enum).
//! 3. [`parse_post_app_validations`] — the orthogonal post-app validation
//!    tails (`--validate-gpu-construction`, `--entities`, `--edit-mode`,
//!    `--runtime-edit-mode`). These run *after* the Bevy app exits and
//!    compose with any gate command above.
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
use bevy_naadf::e2e::gate::E2eGateMode;

/// Top-level commands that exit WITHOUT booting a Bevy app. Returned by
/// [`parse_top_level_short_circuit`] when one of their flags is set.
enum TopLevelShortCircuit {
    /// `--vox-gpu-oracle` — spawn CPU + GPU sub-phases as subprocesses, SSIM-compare.
    VoxGpuOracleCompare,
    /// `--vox-web-parity` — spawn skybox + loaded sub-phases as subprocesses, SSIM-compare.
    VoxWebParityCompare,
    /// `--ssim-compare <a.png> <b.png> [--ssim-min … --ssim-max …]` — pure PNG diff.
    SsimCompare,
    /// `--validate-gpu-construction-scaled` — fixture sweep through W5 chunk_calc.
    ValidateGpuConstructionScaled,
    /// `--validate-gpu-construction-production` — production-scale voxels[] readback.
    ValidateGpuConstructionProduction,
}

/// Boot-the-app commands. Each maps to a single `bevy_naadf::e2e` gate
/// entry point. `gate` carries the [`E2eGateMode`] the run dispatches —
/// surfaced in the boot-dispatch log line for diagnostics; each gate's
/// own `run_*` function constructs the matching `BootstrapInputs` with
/// `gate_mode` set (Step 6 of the config-as-resource refactor).
enum BootCommand {
    /// Run a named gate by delegating to its `run_*` entry point on the
    /// `bevy_naadf::e2e` module. The entry point builds a `BootstrapInputs`
    /// carrying `gate`.
    NamedGate { gate: E2eGateMode, run: fn() -> AppExit },
    /// `--resize-test` — wraps the Bevy boot in pre/post Hyprland windowrule
    /// installation. The resize-test is the canonical `E2eGateMode::Resize`
    /// gate.
    ResizeTest,
    /// `--entities` boot — sets `ConstructionConfig.entities_enabled` +
    /// inserts `SpawnTestEntity(true)` on the standard gate.
    EntitiesBoot,
    /// Standard gate (no flags) — `bevy_naadf::run_e2e_render()`.
    Standard,
}

/// Post-app validation tails that compose orthogonally with a [`BootCommand`].
/// Collected pre-boot; run after the Bevy app exits.
#[derive(Default, Clone, Copy)]
struct PostAppValidations {
    validate_gpu_construction: bool,
    entities: bool,
    edit_mode: bool,
    runtime_edit_mode: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Layer 1: no-boot short-circuits — return early without touching Bevy.
    if let Some(cmd) = parse_top_level_short_circuit(&args) {
        return run_top_level_short_circuit(cmd, &args);
    }

    // Layer 3: collect the orthogonal post-app validation flags pre-boot.
    let post_app = parse_post_app_validations(&args);

    // Layer 2: pick the boot command.
    let boot = parse_gate_command(&args);

    // Run the boot command — installs the resize-test windowrule if needed,
    // boots the app, returns its `AppExit`.
    let app_exit = run_boot_command(boot);
    let e2e_code = app_exit_to_code(app_exit);

    // Post-app validation tails — `--validate-gpu-construction`, `--entities`,
    // `--edit-mode`, `--runtime-edit-mode`. Each runs after the app exits and
    // can flip the exit code to 1 independently.
    run_post_app_validations(post_app, e2e_code)
}

// ---------------------------------------------------------------------------
// Layer 1 — top-level short-circuit dispatch (no Bevy boot)
// ---------------------------------------------------------------------------

/// Return the no-Bevy-boot command implied by `args`, or `None` if no
/// short-circuit flag is set. The flag-priority order matches the original
/// if-ladder shape — `--vox-gpu-oracle` first, then `--vox-web-parity`,
/// `--ssim-compare`, then the two `--validate-gpu-construction-*`
/// diagnostics.
fn parse_top_level_short_circuit(args: &[String]) -> Option<TopLevelShortCircuit> {
    if args.iter().any(|a| a == "--vox-gpu-oracle") {
        return Some(TopLevelShortCircuit::VoxGpuOracleCompare);
    }
    if args.iter().any(|a| a == "--vox-web-parity") {
        return Some(TopLevelShortCircuit::VoxWebParityCompare);
    }
    if args.iter().any(|a| a == "--ssim-compare") {
        return Some(TopLevelShortCircuit::SsimCompare);
    }
    if args.iter().any(|a| a == "--validate-gpu-construction-scaled") {
        return Some(TopLevelShortCircuit::ValidateGpuConstructionScaled);
    }
    if args.iter().any(|a| a == "--validate-gpu-construction-production") {
        return Some(TopLevelShortCircuit::ValidateGpuConstructionProduction);
    }
    None
}

/// Execute a top-level short-circuit and return the process exit code.
fn run_top_level_short_circuit(cmd: TopLevelShortCircuit, args: &[String]) -> ExitCode {
    match cmd {
        TopLevelShortCircuit::VoxGpuOracleCompare => {
            // vox-gpu-rewrite W5.3-fix Stage 4 — top-level oracle gate. Returns
            // its own exit code WITHOUT booting a bevy app (the compare phase
            // spawns the two render phases as subprocesses of THIS binary).
            ExitCode::from(bevy_naadf::e2e::vox_gpu_oracle::run_vox_gpu_oracle_compare())
        }
        TopLevelShortCircuit::VoxWebParityCompare => {
            // web-vox-async-loading 2026-05-18 follow-up Step 8 / Q5 — top-level
            // parity gate. Returns its own exit code WITHOUT booting a bevy app
            // (the compare phase spawns the two sub-phases as subprocesses of
            // THIS binary).
            ExitCode::from(bevy_naadf::e2e::vox_web_parity::run_vox_web_parity_compare())
        }
        TopLevelShortCircuit::SsimCompare => {
            // web-vox-async-loading 2026-05-18 follow-up Step 9 / Q6 — pure PNG
            // diff. Loads two PNGs from disk, computes SSIM, exits per the
            // `[min, max)` band assertion. Used by `--vox-web-parity` and by the
            // Playwright spec. Does NOT boot a bevy app.
            let parsed = match bevy_naadf::e2e::ssim::parse_ssim_compare_args(args) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "e2e_render --ssim-compare: argument parse error: {e}\n\
                         Usage: e2e_render --ssim-compare <a.png> <b.png> \
                         [--ssim-max <f64>] [--ssim-min <f64>]"
                    );
                    return ExitCode::from(2);
                }
            };
            ExitCode::from(bevy_naadf::e2e::ssim::ssim_compare_command(&parsed))
        }
        TopLevelShortCircuit::ValidateGpuConstructionScaled => {
            // vox-gpu-rewrite Stage 6 — concrete byte-diff diagnostic.
            // Short-circuits before booting the e2e binary; runs a fixture
            // sweep through the W5 chunk_calc chain and prints
            // first-divergent-index per buffer (raw + semantic).
            match bevy_naadf::render::construction::validate_gpu_construction_scaled() {
                Ok(_report) => ExitCode::from(0),
                Err(msg) => {
                    eprintln!("scaled byte-diff diagnostic FAILED: {msg}");
                    ExitCode::from(1)
                }
            }
        }
        TopLevelShortCircuit::ValidateGpuConstructionProduction => {
            // vox-gpu-rewrite Stage 9 — production-scale voxels[] readback
            // diagnostic. Short-circuits before booting the e2e binary.
            match bevy_naadf::render::construction::validate_gpu_construction_production_scale() {
                Ok(_report) => ExitCode::from(0),
                Err(msg) => {
                    eprintln!("production-scale readback diagnostic FAILED: {msg}");
                    ExitCode::from(1)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Layer 2 — boot-command dispatch (Bevy app boot)
// ---------------------------------------------------------------------------

/// Pick the boot command from `args`. The boot-command table is ordered to
/// match the original if-ladder priority — `--resize-test` first, then the
/// per-gate flags in declaration order, then `--entities` (no special gate),
/// then the standard fallback.
fn parse_gate_command(args: &[String]) -> BootCommand {
    if args.iter().any(|a| a == "--resize-test") {
        return BootCommand::ResizeTest;
    }
    if args.iter().any(|a| a == "--oasis-edit-visual") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::OasisEdit,
            run: bevy_naadf::e2e::oasis_edit_visual::run_oasis_edit_visual,
        };
    }
    if args.iter().any(|a| a == "--small-edit-visual") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::SmallEditVisual,
            run: bevy_naadf::e2e::small_edit_visual::run_small_edit_visual,
        };
    }
    if args.iter().any(|a| a == "--small-edit-repro") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::SmallEditRepro,
            run: bevy_naadf::e2e::small_edit_repro::run_small_edit_repro,
        };
    }
    if args.iter().any(|a| a == "--vox-gpu-oracle-cpu") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::VoxGpuOracleCpu,
            run: bevy_naadf::e2e::vox_gpu_oracle::run_vox_gpu_oracle_cpu_phase,
        };
    }
    if args.iter().any(|a| a == "--vox-gpu-oracle-gpu") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::VoxGpuOracleGpu,
            run: bevy_naadf::e2e::vox_gpu_oracle::run_vox_gpu_oracle_gpu_phase,
        };
    }
    if args.iter().any(|a| a == "--vox-web-parity-skybox") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::VoxWebParitySkybox,
            run: bevy_naadf::e2e::vox_web_parity::run_vox_web_parity_skybox_phase,
        };
    }
    if args.iter().any(|a| a == "--vox-web-parity-loaded") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::VoxWebParityLoaded,
            run: bevy_naadf::e2e::vox_web_parity::run_vox_web_parity_loaded_phase,
        };
    }
    if args.iter().any(|a| a == "--vox-horizon-native") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::VoxHorizonNative,
            run: bevy_naadf::e2e::vox_horizon_parity::run_vox_horizon_native_phase,
        };
    }
    if args.iter().any(|a| a == "--vox-gpu-construction") {
        return BootCommand::NamedGate {
            gate: E2eGateMode::VoxGpuConstruction,
            run: bevy_naadf::e2e::vox_gpu_construction::run_vox_gpu_construction,
        };
    }
    if args.iter().any(|a| a == "--vox-e2e") {
        // `--vox-e2e` runs the STANDARD driver flow (Decision §3 — the
        // vox-e2e ASSERT tag is Bucket A, not a flow selector). `run_vox_e2e`
        // sets the `VoxE2eAssertion` resource on its `BootstrapInputs`.
        return BootCommand::NamedGate {
            gate: E2eGateMode::Standard,
            run: bevy_naadf::e2e::vox_e2e::run_vox_e2e,
        };
    }
    if args.iter().any(|a| a == "--entities") {
        return BootCommand::EntitiesBoot;
    }
    BootCommand::Standard
}

/// Execute a boot command and return its `AppExit`. Handles the resize-test
/// windowrule install/cleanup wrap.
fn run_boot_command(boot: BootCommand) -> AppExit {
    match boot {
        BootCommand::NamedGate { gate, run } => {
            // `gate` is the `E2eGateMode` the run dispatches; surfaced here
            // for diagnostics. The gate's own `run_*` function constructs a
            // `BootstrapInputs` with `gate_mode` set to this same value
            // (Step 6 of the config-as-resource refactor).
            eprintln!("e2e_render: boot dispatch — gate mode {gate:?}");
            run()
        }
        BootCommand::ResizeTest => run_resize_test(),
        BootCommand::EntitiesBoot => {
            // Steps 4 + 8 of the config-as-resource refactor — the
            // `construction_config.entities_enabled = true` override (Step 4)
            // and the `spawn_test_entity` flag (Step 8) both moved off
            // `AppArgs` onto typed `BootstrapInputs` fields. Route through
            // `run_e2e_render_with_bootstrap_inputs` so the bootstrap fan-out
            // inserts both as per-domain resources.
            let mut construction_config =
                bevy_naadf::render::construction::ConstructionConfig::for_target_arch();
            construction_config.entities_enabled = true;
            let inputs = bevy_naadf::bootstrap::BootstrapInputs {
                construction_config,
                spawn_test_entity:
                    bevy_naadf::render::construction::SpawnTestEntity(true),
                ..bevy_naadf::bootstrap::BootstrapInputs::default()
            };
            bevy_naadf::bootstrap::run_e2e_render_with_bootstrap_inputs(inputs)
        }
        BootCommand::Standard => bevy_naadf::run_e2e_render(),
    }
}

/// Run the `--resize-test` boot wrapped in the Hyprland windowrule
/// install/cleanup. Behaviour identical to the inline block the if-ladder
/// previously held.
fn run_resize_test() -> AppExit {
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
    install_resize_test_windowrule();

    // Step 6 of the config-as-resource refactor — the `resize_test` boolean
    // collapsed into `E2eGateMode::Resize`. Route through the
    // `BootstrapInputs` fan-out; `window_for_gate_mode` reads the gate mode
    // to pick the 800×600 resize-boot window.
    let inputs = bevy_naadf::bootstrap::BootstrapInputs {
        gate_mode: bevy_naadf::e2e::gate::E2eGateMode::Resize,
        ..bevy_naadf::bootstrap::BootstrapInputs::default()
    };
    let exit = bevy_naadf::bootstrap::run_e2e_render_with_bootstrap_inputs(inputs);

    cleanup_resize_test_windowrule();

    exit
}

/// Pre-launch — install the Hyprland windowrule that pre-floats the
/// e2e_render window so `hyprctl dispatch resizewindowpixel` takes effect.
fn install_resize_test_windowrule() {
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
}

/// Post-run — discard the runtime windowrule by reloading the config from
/// disk. Best-effort; failure here doesn't change the test verdict.
fn cleanup_resize_test_windowrule() {
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
}

/// Translate Bevy's `AppExit` into a process exit-code byte.
fn app_exit_to_code(app_exit: AppExit) -> u8 {
    match app_exit {
        AppExit::Success => 0u8,
        AppExit::Error(code) => code.get(),
    }
}

// ---------------------------------------------------------------------------
// Layer 3 — post-app validation tails
// ---------------------------------------------------------------------------

/// Collect the orthogonal post-app validation flags. Run after the Bevy app
/// exits; each can flip the final exit code to 1 independently.
fn parse_post_app_validations(args: &[String]) -> PostAppValidations {
    PostAppValidations {
        validate_gpu_construction: args.iter().any(|a| a == "--validate-gpu-construction"),
        entities: args.iter().any(|a| a == "--entities"),
        edit_mode: args.iter().any(|a| a == "--edit-mode"),
        runtime_edit_mode: args.iter().any(|a| a == "--runtime-edit-mode"),
    }
}

/// Run the post-app validation tails, folding any failure into the final
/// exit code. `e2e_code` is the boot command's exit code; on validation
/// failure the function returns `ExitCode::from(1)` even if `e2e_code` was
/// 0. On all-pass the function returns `ExitCode::from(e2e_code)`.
fn run_post_app_validations(post_app: PostAppValidations, e2e_code: u8) -> ExitCode {
    if post_app.validate_gpu_construction {
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

    if post_app.entities {
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

    if post_app.edit_mode {
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
    if post_app.runtime_edit_mode {
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
