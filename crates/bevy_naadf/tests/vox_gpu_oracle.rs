//! BRP-driven e2e gate — `vox_gpu_oracle`, migrated from the legacy in-app
//! `e2e_render --vox-gpu-oracle` compare orchestrator + its two phases
//! `--vox-gpu-oracle-cpu` / `--vox-gpu-oracle-gpu`
//! (`e2e-ipc-rpc-restructure` Phase 3b).
//!
//! ## What this gate proves
//!
//! The CPU-oracle vs GPU-built SSIM compare (`e2e/vox_gpu_oracle.rs` module
//! doc): the W5 GPU producer chain (`generator_model` + `chunk_calc` + bounds)
//! is the production install path for `.vox` loads; the CPU `aadf::construct`
//! oracle (`install_vox_sized_to_model`) is the known-good reference renderer.
//! The gate renders the Oasis fixture through BOTH paths from an identical
//! top-down camera pose and asserts the two captures are structurally similar
//! (SSIM ≥ `ORACLE_SSIM_THRESHOLD`) — a gross GPU-renderer regression
//! (sky-bleed, voxel-type corruption, palette OOB, AADF-leak) craters SSIM far
//! below the threshold.
//!
//! ## Compare-gate collapse (design §7.3 — binding)
//!
//! The legacy gate was a *Layer-1 subprocess orchestrator*: `--vox-gpu-oracle`
//! spawned two `e2e_render` subprocesses (`--vox-gpu-oracle-cpu` →
//! `oracle_cpu.png`, `--vox-gpu-oracle-gpu` → `oracle_gpu.png`), then loaded
//! both PNGs from disk and SSIM-compared. This migrated gate collapses all
//! three into **one test body that drives the SUT twice**: it spawns one SUT
//! in CPU-construction mode (`--e2e-vox-oracle-cpu`), captures, drops it;
//! spawns a second SUT in GPU-construction mode (the production W5 path, no
//! flag), captures; then SSIM-compares the two captures **in-process** via the
//! library's `compare_oracle_frames` (which `bevy_naadf::e2e::ssim` backs).
//! No subprocess-of-subprocess, no PNG round-trip through disk.
//!
//! ## The CPU/GPU selection is a boot-time knob (Forbidden Move #4)
//!
//! `setup_test_grid` reads `E2eGateMode` at `Startup` to pick the install
//! path: `E2eGateMode::VoxGpuOracleCpu` → `install_vox_sized_to_model` (the
//! natural-bound CPU oracle); anything else → `install_vox_in_fixed_world`
//! (the production W5 GPU producer chain). That selection happens before
//! `app.run()`, so it cannot be a BRP verb — it rides the spawn contract: the
//! `--e2e-vox-oracle-cpu` CLI flag on `bin/bevy-naadf` (Phase 3b) sets
//! `BootstrapInputs.gate_mode = E2eGateMode::VoxGpuOracleCpu`. The GPU phase
//! needs no flag — a bare `--vox` load already routes through the production
//! W5 chain.
//!
//! ## Migration fidelity (Phase 3b brief — binding)
//!
//! The camera pose (`ORACLE_CAMERA_POS` / `ORACLE_CAMERA_LOOK`), the warmup
//! budget (`ORACLE_WARMUP_FRAMES`), the SSIM threshold + sanity guards
//! (`ORACLE_SSIM_THRESHOLD`, `ORACLE_MEAN_DIFF_FLOOR`, the bright/dark
//! fractions) are all reused from the library module **verbatim** — the
//! compare itself is the library's `compare_oracle_frames` called unchanged.
//! No threshold is recalibrated.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test vox_gpu_oracle
//! ```

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::e2e::framebuffer::Framebuffer;
use bevy_naadf::e2e::vox_gpu_oracle::{
    compare_oracle_frames, ORACLE_CAMERA_LOOK, ORACLE_CAMERA_POS, ORACLE_SSIM_THRESHOLD,
    ORACLE_WARMUP_FRAMES,
};

/// The Oasis VOX fixture, crate-root-relative (the SUT CWD). The legacy
/// `oasis_vox_fixture_path()` resolves the workspace-relative path or this
/// crate-relative fallback; with the SUT CWD at the crate root the
/// crate-relative form resolves.
const OASIS_VOX_FIXTURE: &str = "assets/test/oasis_hard_cover.vox";

/// Drive one SUT to a single oracle capture at the shared oracle camera pose.
///
/// `vox_oracle_cpu = true` spawns the CPU-construction SUT (the
/// `--e2e-vox-oracle-cpu` flag → `install_vox_sized_to_model`); `false` spawns
/// the production W5 GPU-construction SUT. Both warm up `ORACLE_WARMUP_FRAMES`
/// at the identical top-down pose, capture, and drop the SUT (`Sut::Drop`
/// kills the subprocess).
fn capture_oracle_phase(label: &str, vox_oracle_cpu: bool) -> Framebuffer {
    // The legacy oracle phases both run at `AppConfig::e2e()`'s 256×256 window
    // (`compare_oracle_frames` doc) — match it so the SSIM compare sees
    // identically-sized frames.
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .vox(OASIS_VOX_FIXTURE)
            .window(256, 256)
            .vox_oracle_cpu(vox_oracle_cpu),
    );

    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "vox_gpu_oracle [{label}]: SUT reports world_loaded=false — the Oasis \
         VOX load failed"
    );

    // Shared oracle camera pose — top-down, `Vec3::X` up (the legacy
    // `pin_vox_gpu_oracle_camera` convention). Both phases MUST use identical
    // values so the two worlds are sampled from the same voxel volume.
    scenario::set_camera(
        sut.client(),
        [ORACLE_CAMERA_POS.x, ORACLE_CAMERA_POS.y, ORACLE_CAMERA_POS.z],
        [ORACLE_CAMERA_LOOK.x, ORACLE_CAMERA_LOOK.y, ORACLE_CAMERA_LOOK.z],
        Some([1.0, 0.0, 0.0]),
    )
    .expect("naadf/set_camera");

    // Warm up so TAA's 32-deep ring fills + GI's 96-frame accumulation
    // completes (the legacy `ORACLE_WARMUP_FRAMES`).
    scenario::advance(sut.client(), ORACLE_WARMUP_FRAMES).expect("warmup advance");

    let fb = scenario::capture(sut.client()).expect("oracle capture");
    println!(
        "vox_gpu_oracle [{label}]: captured {}x{} (vox_oracle_cpu={vox_oracle_cpu})",
        fb.width(),
        fb.height()
    );

    // Pipeline-error scan before dropping the SUT.
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");
    fb
}

#[test]
fn vox_gpu_oracle() {
    println!(
        "vox_gpu_oracle: camera pos={:?} look={:?}; SSIM threshold {:.3}",
        ORACLE_CAMERA_POS, ORACLE_CAMERA_LOOK, ORACLE_SSIM_THRESHOLD,
    );

    // Phase 1 — CPU oracle: route the Oasis load through the test-only
    // natural-bound CPU loader (`install_vox_sized_to_model`).
    let cpu_fb = capture_oracle_phase("cpu-oracle", true);

    // Phase 2 — GPU: the production W5 GPU producer chain
    // (`install_vox_in_fixed_world`) — a bare `--vox` load, no flag.
    let gpu_fb = capture_oracle_phase("gpu-production", false);

    // Save both captures (legacy filenames) so the artifacts match the legacy
    // gate's `oracle_cpu.png` / `oracle_gpu.png`.
    let _ = cpu_fb.save_png("target/e2e-screenshots/oracle_cpu.png");
    let _ = gpu_fb.save_png("target/e2e-screenshots/oracle_gpu.png");

    // The load-bearing compare — `compare_oracle_frames` reused VERBATIM from
    // the library: SSIM ≥ `ORACLE_SSIM_THRESHOLD` (0.85), the bright/dark
    // sanity guards on the CPU oracle frame, and the `ORACLE_MEAN_DIFF_FLOOR`
    // sanity check. The threshold is NOT recalibrated.
    let report = compare_oracle_frames(&cpu_fb, &gpu_fb).unwrap_or_else(|msg| {
        panic!(
            "vox_gpu_oracle gate FAIL — {msg}\n  Inspect \
             crates/bevy_naadf/target/e2e-screenshots/oracle_{{cpu,gpu}}.png."
        )
    });

    println!("vox_gpu_oracle: PASS — {report}");
}
