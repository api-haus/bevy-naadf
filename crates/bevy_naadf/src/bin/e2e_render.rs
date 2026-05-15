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
    // Parse the CLI flag — `--validate-gpu-construction`, default off.
    let validate_gpu_construction = std::env::args()
        .skip(1)
        .any(|a| a == "--validate-gpu-construction");

    let app_exit = bevy_naadf::run_e2e_render();

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

    ExitCode::from(e2e_code)
}
