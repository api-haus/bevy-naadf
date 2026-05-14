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
//! See `docs/orchestrate/naadf-bevy-port/e2e-render-test.md` for the full
//! design.

use bevy::prelude::AppExit;

fn main() -> AppExit {
    bevy_naadf::run_e2e_render()
}
