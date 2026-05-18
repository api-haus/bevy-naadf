//! `cargo run --bin e2e_render` — the bounded windowed end-to-end render test.
//!
//! The whole binary: boot the **same App** `bin/bevy-naadf` boots (small
//! fixed window, synchronous pipeline compile, bounded-frame driver),
//! attach the gate selected by `--gate <NAME>`, run the render graph for a
//! fixed frame budget, read the on-screen framebuffer back, run the
//! per-batch region gates + the `PipelineCache` error scan + the
//! node-dispatch check, and exit 0 on success / non-zero on failure.
//!
//! `fn main() -> ExitCode` folds the e2e's `AppExit` + the optional
//! post-App validation result into a single explicit numeric exit code.
//!
//! ## CLI shape
//!
//! Every interactive flag the production binary `bevy-naadf` accepts (see
//! `crate::cli::Cli`) is also accepted here; the e2e binary additionally
//! accepts `--gate <NAME>` (see `crate::cli::Gate`).
//!
//! - `--gate baseline` — plain bounded-frame run, no observer.
//! - `--gate streaming-window` — installs `ProceduralStreaming` preset (if
//!   the user didn't override) + the streaming-window observer.
//! - `--gate noise-static-world` — installs `ProceduralStatic` (if no
//!   override) + the static-noise observer.
//! - … (every existing gate is reachable via `--gate <kebab-name>`; see
//!   `crate::cli::Gate` for the full list).
//!
//! ## Headless / multi-process gates
//!
//! Four gates short-circuit BEFORE App boot (they're pure-compute / spawn
//! subprocesses):
//!
//! - `--gate vox-gpu-oracle` — spawns CPU + GPU subprocesses, compares PNGs.
//! - `--gate wgsl-noise-oracle` — headless WGSL ↔ CPU oracle byte equality.
//! - `--gate validate-gpu-construction-scaled` — fixture sweep diagnostic.
//! - `--gate validate-gpu-construction-production` — production-scale
//!   readback diagnostic.
//!
//! These are explicitly NOT App-based and intentionally bypass `build_app`
//! — they exist as compute-only validators outside the renderer's frame
//! loop. Every other gate goes through the standard
//! `run_e2e_render_with_args(args)` path.
//!
//! ## The streaming-world / "e2e drives actual main" rearch
//!
//! Pre-rearch (`docs/orchestrate/streaming-world/02d-design-cli-and-e2e-rearch.md`),
//! this binary had a 180-line dispatch ladder where each gate constructed
//! `AppArgs::default()` internally and ignored every CLI flag the user
//! supplied. The rearch moved all CLI parsing into `crate::cli::E2eCli`
//! (which flattens `crate::cli::Cli`); each gate gets an
//! `apply_<gate>_defaults(&mut AppArgs)` overlay function that the
//! `cli::apply_gate_defaults` dispatcher calls. The body of this binary is
//! now just (parse → match short-circuit gates → run_e2e_render_with_args
//! → match post-App validators).

use std::process::ExitCode;

use bevy::prelude::AppExit;
use bevy_naadf::cli::{E2eCli, Gate};
use clap::Parser;

fn main() -> ExitCode {
    let cli = E2eCli::parse();
    let (args, gate) = cli.into_app_args_and_gate();

    // --- (1) Headless / multi-process gates — short-circuit BEFORE App boot.
    //         These intentionally don't build the renderer; they're pure-
    //         compute validators or subprocess orchestrators.
    match gate {
        Some(Gate::VoxGpuOracle) => {
            let code = bevy_naadf::e2e::vox_gpu_oracle::run_vox_gpu_oracle_compare();
            return ExitCode::from(code);
        }
        Some(Gate::WgslNoiseOracle) => {
            return bevy_naadf::e2e::wgsl_noise_oracle::run_wgsl_noise_oracle_gate();
        }
        Some(Gate::ValidateGpuConstructionScaled) => {
            return match bevy_naadf::render::construction::validate_gpu_construction_scaled() {
                Ok(_report) => ExitCode::from(0),
                Err(msg) => {
                    eprintln!("scaled byte-diff diagnostic FAILED: {msg}");
                    ExitCode::from(1)
                }
            };
        }
        Some(Gate::ValidateGpuConstructionProduction) => {
            return match bevy_naadf::render::construction::validate_gpu_construction_production_scale() {
                Ok(_report) => ExitCode::from(0),
                Err(msg) => {
                    eprintln!("production-scale readback diagnostic FAILED: {msg}");
                    ExitCode::from(1)
                }
            };
        }
        _ => {}
    }

    // --- (2) ResizeTest gate — install pre-launch Hyprland windowrule.
    //         (Best-effort; failure logs to stderr but proceeds.)
    let resize_test = matches!(gate, Some(Gate::ResizeTest));
    if resize_test {
        install_hyprland_windowrule_for_resize();
    }

    // --- (3) Drive the actual App. This is the ONE path that boots the
    //         renderer for every App-rendering gate — exactly the same
    //         `build_app(AppConfig::e2e())` + `app.run()` `bin/bevy-naadf`
    //         uses for its own App, modulo the e2e window config (small,
    //         non-resizable, synchronous pipeline compile).
    let app_exit = bevy_naadf::run_e2e_render_with_args(args);

    let e2e_code = match app_exit {
        AppExit::Success => 0u8,
        AppExit::Error(code) => code.get(),
    };

    // --- (4) Post-App validation passes — each gates a SEPARATE headless
    //         render world for byte-equality / behavioural checks.
    match gate {
        Some(Gate::ValidateGpuConstruction) => {
            match bevy_naadf::render::construction::validate_gpu_construction() {
                Ok(bytes_compared) => {
                    eprintln!(
                        "GPU construction byte-equal to CPU oracle: \
                         {bytes_compared} bytes compared"
                    );
                    if e2e_code != 0 {
                        eprintln!(
                            "(e2e itself returned non-zero exit {e2e_code}; \
                             validation succeeded but the e2e failure is the \
                             load-bearing failure)"
                        );
                    }
                }
                Err(msg) => {
                    eprintln!("GPU construction validation FAILED: {msg}");
                    return ExitCode::from(1);
                }
            }
        }
        Some(Gate::Entities) => {
            match bevy_naadf::render::construction::validate_entity_handler() {
                Ok(report) => eprintln!("entity handler validation PASS: {report}"),
                Err(msg) => {
                    eprintln!("entity handler validation FAILED: {msg}");
                    return ExitCode::from(1);
                }
            }
        }
        Some(Gate::EditMode) => {
            match bevy_naadf::render::construction::validate_edit_mode() {
                Ok(report) => eprintln!("edit-mode validation PASS: {report}"),
                Err(msg) => {
                    eprintln!("edit-mode validation FAILED: {msg}");
                    return ExitCode::from(1);
                }
            }
        }
        Some(Gate::RuntimeEditMode) => {
            match bevy_naadf::render::construction::validate_runtime_edit_mode() {
                Ok(report) => eprintln!("runtime-edit gate PASS: {report}"),
                Err(msg) => {
                    eprintln!("runtime-edit gate FAILED: {msg}");
                    return ExitCode::from(1);
                }
            }
        }
        _ => {}
    }

    if resize_test {
        cleanup_hyprland_windowrule_for_resize();
    }

    ExitCode::from(e2e_code)
}

/// Pre-launch Hyprland windowrule install for `--gate resize-test`. Best-
/// effort; emits diagnostic to stderr on failure but proceeds (the resize-
/// test then fails on its own luma assertion with a clear "not floating"
/// signal).
///
/// Hyprland 0.54+ syntax: `match:class <regex>, float on`. Cleanup uses
/// `hyprctl reload` (see [`cleanup_hyprland_windowrule_for_resize`]). See
/// `docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
/// `## GI-bounce-on-resize fix (2026-05-16)`.
fn install_hyprland_windowrule_for_resize() {
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

/// Post-run cleanup — discards the runtime windowrule. Best-effort; failure
/// leaves the rule until the user reloads Hyprland.
fn cleanup_hyprland_windowrule_for_resize() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let status = std::process::Command::new("hyprctl").args(["reload"]).status();
        match status {
            Ok(s) => eprintln!("e2e_render: post-run hyprctl reload -> {s:?}"),
            Err(e) => eprintln!(
                "e2e_render: post-run hyprctl reload FAILED to spawn: {e} \
                 — runtime windowrule may persist until next reload"
            ),
        }
    }
}
