//! `--wgsl-noise-oracle` — Phase-1 e2e gate for the streaming-world
//! orchestration (`docs/orchestrate/streaming-world/02b-design-plan-b.md` § C).
//!
//! Pure compute test (no window, no rendering): boots a headless render world
//! via `MinimalPlugins + RenderPlugin`, dispatches the WGSL FastNoiseLite port
//! over a deterministic matrix of (noise family × fractal type × domain warp ×
//! cellular sub-config × sample point) cases, reads the GPU output back, and
//! compares it to the Rust CPU oracle bit-near-equal (`< 1e-5` for
//! non-cellular, `< 1e-4` for cellular). Exits via `ExitCode` so the gate is a
//! first-class member of the verification surface listed in `CLAUDE.md`.
//!
//! Body lives in [`crate::streaming::noise_fastnoiselite::run_wgsl_noise_oracle`].
//! This module is the thin `ExitCode`-shaped façade the binary calls.

use std::process::ExitCode;

use crate::streaming::noise_fastnoiselite::{run_wgsl_noise_oracle, OracleReport};

/// Entry point invoked by `bin/e2e_render.rs` when `--wgsl-noise-oracle` is
/// passed. Returns `ExitCode::from(0)` on success, non-zero on failure.
///
/// The function prints a one-line PASS line on success that captures total
/// cases, distinct (noise × fractal × ...) combinations exercised, and the
/// worst-case absolute error observed — this is the "X / Y combos, max_diff =
/// Z" summary the agent brief asks the impl agent to copy into the
/// `03a-impl-wgsl-noise.md` summary.
pub fn run_wgsl_noise_oracle_gate() -> ExitCode {
    match run_wgsl_noise_oracle() {
        Ok(report) => {
            print_pass(&report);
            ExitCode::from(0)
        }
        Err(msg) => {
            eprintln!("WGSL noise oracle FAILED:\n{msg}");
            ExitCode::from(1)
        }
    }
}

fn print_pass(report: &OracleReport) {
    eprintln!(
        "WGSL noise oracle PASS: {} cases across {} distinct combos. \
         max_abs_diff = {:.4e} on tag=`{}` at pos={:?} (cpu={}, gpu={}).",
        report.total_cases,
        report.combos,
        report.max_abs_diff,
        report.max_abs_diff_tag,
        report.max_abs_diff_pos,
        report.max_abs_diff_cpu,
        report.max_abs_diff_gpu,
    );
}
