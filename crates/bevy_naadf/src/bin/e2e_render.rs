//! `cargo run --bin e2e_render` — the bounded windowed end-to-end render test.
//!
//! The whole binary: boot the real `DefaultPlugins` + `WinitPlugin` windowed
//! app via [`bevy_naadf::run_e2e_render`], run the render graph for a fixed
//! frame budget, read the on-screen framebuffer back, run the per-batch region
//! gates + the `PipelineCache` error scan + the node-dispatch check, and exit
//! 0 on success / non-zero on failure.
//!
//! `fn main() -> AppExit` works because `AppExit: Termination` — the process
//! exit code *is* the test result, no glue. The window appears for well under
//! a second and closes itself; the impl agent runs this once, reads the exit
//! code + stderr, and is done — no loop.
//!
//! ## Phase-C flag — `--validate-gpu-construction` (`15-design-c.md` §1.6, W0)
//!
//! W0 plumbs the flag end-to-end so W1 has a stable CLI surface to point its
//! bit-exact CPU/GPU oracle at. **W0 body — placeholder.** When the flag is
//! set, after the normal e2e completes the binary prints a placeholder log
//! line and exits with the same status the e2e produced. W1 replaces the
//! placeholder with the real GPU-vs-CPU buffer-byte-equality assertion (the
//! oracle from `aadf::construct::construct` on `GridPreset::Default` vs. the
//! GPU `blocks` / `voxels` / chunks-texture readback).
//!
//! See `docs/orchestrate/naadf-bevy-port/e2e-render-test.md` for the full
//! design.

use std::process::ExitCode;

use bevy::prelude::AppExit;

fn main() -> ExitCode {
    // Parse the W0 CLI flag — `--validate-gpu-construction`, default off.
    // No argument-value pairs; the flag is a presence-only switch. Kept
    // hand-rolled (no `clap`/`argh` dep added by W0) so the surface is the
    // smallest possible — W1 / W2 / W4 each add their own flags in their
    // workstream, and the impl logs document each new flag at its merge.
    let validate_gpu_construction = std::env::args()
        .skip(1)
        .any(|a| a == "--validate-gpu-construction");

    let app_exit = bevy_naadf::run_e2e_render();

    // W0 — placeholder validation. The flag's *plumbing* is load-bearing for
    // W1's bit-exact oracle; the validation **body** is W1's responsibility.
    // We run the placeholder unconditionally on the flag (success path or
    // not) so the flag is verified end-to-end in W0's gate, then fold the
    // e2e's `AppExit` into the exit code (validation cannot *succeed* the
    // run when the e2e fails; it can only *fail* a successful e2e — and W0's
    // placeholder doesn't fail).
    if validate_gpu_construction {
        eprintln!(
            "phase-c W0 seam — gpu construction validation placeholder \
             (no-op until W1 lands)"
        );
    }

    match app_exit {
        AppExit::Success => ExitCode::SUCCESS,
        AppExit::Error(code) => ExitCode::from(code.get()),
    }
}
