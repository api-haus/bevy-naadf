//! `e2e_render` — the pure-CPU SSIM PNG-diff utility.
//!
//! ## What this binary is, post-restructure
//!
//! The booted-window e2e harness moved out of this binary entirely
//! (`docs/orchestrate/e2e-ipc-rpc-restructure/`). The 13 booted-window e2e
//! gates are now BRP-driven `#[test]` bodies in `crates/bevy_naadf/tests/`,
//! driving the production `bin/bevy-naadf` binary as the system-under-test
//! over the Bevy Remote Protocol. The in-app driver-mode machinery
//! (`e2e/driver.rs`, `E2eGateMode`, the 3-layer argv parser, the per-gate
//! `run_*` boot fns) was deleted in Phase 5 of that restructure.
//!
//! What survives is a single leaf utility: **`--ssim-compare`**. It is a
//! pure-CPU PNG diff — no Bevy, no `App`, no GPU, no window — that the
//! cross-target Playwright gate (`e2e/tests/vox-horizon-parity.spec.ts`)
//! shells out to in order to SSIM-compare a native reference PNG against a
//! wasm-canvas capture. Porting the SSIM into Node would fork the tuned
//! (TAA/GI-shimmer-tolerant) algorithm, so the algorithm stays in Rust and
//! this thin binary exposes it (`02-design.md` §8 / D9).
//!
//! ## Contract — preserved byte-for-byte for the Playwright spec
//!
//! ```text
//! e2e_render --ssim-compare <a.png> <b.png> [--ssim-max <f64>] [--ssim-min <f64>]
//! ```
//!
//! Exit codes (`ssim.rs::ssim_compare_command`):
//! - `0` — gate passed (SSIM within the asserted `[min, max)` band).
//! - `1` — gate failed (SSIM out of the asserted range).
//! - `2` — internal error (file not found, decode error, dimension
//!   mismatch, argument parse error).
//!
//! `ssim_compare_command` also prints a `SSIM=<f64>` line on stdout, which
//! the Playwright spec's `extractSsimScore()` parses.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // The sole command: `--ssim-compare <a> <b> [--ssim-max] [--ssim-min]`.
    // A pure no-App PNG diff (`02-design.md` §8 / D9). Loads two PNGs,
    // computes SSIM, exits per the `[min, max)` band assertion.
    let parsed = match bevy_naadf::e2e::ssim::parse_ssim_compare_args(&args) {
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
