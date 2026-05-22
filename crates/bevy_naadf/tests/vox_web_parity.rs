//! BRP-driven e2e gate — `vox_web_parity`, migrated from the legacy in-app
//! `e2e_render --vox-web-parity` compare orchestrator + its two phases
//! `--vox-web-parity-skybox` / `--vox-web-parity-loaded`
//! (`e2e-ipc-rpc-restructure` Phase 3b).
//!
//! ## What this gate proves
//!
//! The `.vox`-install-actually-rendered gate (`e2e/vox_web_parity.rs` module
//! doc): it captures a **skybox-only baseline** (`GridPreset::Empty`, pure
//! sky) and a **vox-loaded** frame (the production W5 GPU producer chain) from
//! an identical top-down pose, and asserts they are structurally
//! **dissimilar** (SSIM **below** `VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX`).
//! A regression that silently turned the `.vox` install into a no-op would
//! leave both frames as pure sky → SSIM ≈ 1.0 → fail. The assertion direction
//! is inverted from `vox_gpu_oracle` (which asserts *similarity*).
//!
//! It also runs the `web-vox-color-divergence` per-channel guard: the loaded
//! frame's central rect must have a per-channel max above
//! `VOX_WEB_PARITY_CHANNEL_MAX_FLOOR` — catching the "geometry correct, colors
//! collapsed to near-black" regression that SSIM (structurally colour-blind)
//! would miss.
//!
//! ## Compare-gate collapse (design §7.3 — binding)
//!
//! The legacy gate was a *Layer-1 subprocess orchestrator*: `--vox-web-parity`
//! spawned two `e2e_render` subprocesses (`--vox-web-parity-skybox` →
//! `vox_web_parity_skybox.png`, `--vox-web-parity-loaded` →
//! `vox_web_parity_loaded.png`), then loaded both PNGs and SSIM-compared. This
//! migrated gate collapses all three into **one test body that drives the SUT
//! twice**: spawn one SUT with the empty-world skybox baseline
//! (`--e2e-empty-world`), capture, drop it; spawn a second SUT with the Oasis
//! `--vox` fixture (the production W5 path), capture; SSIM-compare the two
//! captures **in-process** via `bevy_naadf::e2e::ssim::ssim_compare_framebuffers`.
//!
//! ## The skybox baseline is a boot-time knob (Forbidden Move #4)
//!
//! `setup_test_grid` reads `GridPreset` at `Startup` to pick the install path;
//! `GridPreset::Empty` installs an empty `WorldData` (pure-sky render). That
//! selection happens before `app.run()`, so it cannot be a BRP verb — it
//! rides the spawn contract: the `--e2e-empty-world` CLI flag on
//! `bin/bevy-naadf` (Phase 3b) sets `BootstrapInputs.grid_preset =
//! GridPreset::Empty`. The loaded phase needs no extra flag — a `--vox` load
//! already routes through the production W5 chain.
//!
//! ## Migration fidelity (Phase 3b brief — binding)
//!
//! The camera pose (`PARITY_CAMERA_POS` / `PARITY_CAMERA_LOOK`), the warmup
//! budget (`PARITY_WARMUP_FRAMES`), and BOTH thresholds
//! (`VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX`, `VOX_WEB_PARITY_CHANNEL_MAX_FLOOR`)
//! are reused from the library module **verbatim** — the SSIM compare itself
//! is `bevy_naadf::e2e::ssim::ssim_compare_framebuffers`, the same impl the
//! legacy gate calls. No threshold is recalibrated.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test vox_web_parity
//! ```

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::e2e::framebuffer::{Framebuffer, Rect};
use bevy_naadf::e2e::ssim::ssim_compare_framebuffers;
use bevy_naadf::e2e::vox_web_parity::{
    PARITY_CAMERA_LOOK, PARITY_CAMERA_POS, PARITY_WARMUP_FRAMES,
    VOX_WEB_PARITY_CHANNEL_MAX_FLOOR, VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX,
};

/// The Oasis VOX fixture, crate-root-relative (the SUT CWD).
const OASIS_VOX_FIXTURE: &str = "assets/test/oasis_hard_cover.vox";

/// Drive one SUT to a single parity capture at the shared parity camera pose.
///
/// `opts` decides the phase: `.empty_world(true)` → the skybox baseline
/// (`GridPreset::Empty`, pure sky); `.vox(OASIS_VOX_FIXTURE)` → the loaded
/// phase (the production W5 GPU producer chain). Both warm up
/// `PARITY_WARMUP_FRAMES` at the identical pose, capture, and drop the SUT.
fn capture_parity_phase(label: &str, opts: SutOpts) -> Framebuffer {
    // Both legacy parity phases run at `AppConfig::e2e()`'s 256×256 window —
    // match it so the two captures SSIM-compare without resize.
    let mut sut = Sut::spawn(opts.window(256, 256));

    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    // NOTE: `world_loaded` is `true` for the empty-world skybox baseline too —
    // `install_empty_world` still inserts a (empty) `WorldData` resource.
    assert!(
        state.world_loaded,
        "vox_web_parity [{label}]: SUT reports world_loaded=false — the world \
         install failed"
    );

    // Shared parity camera pose — top-down, `Vec3::X` up (the legacy
    // `pin_vox_web_parity_camera` convention). Identical for both phases so
    // the dissimilarity is a real `.vox`-geometry signal.
    scenario::set_camera(
        sut.client(),
        [PARITY_CAMERA_POS.x, PARITY_CAMERA_POS.y, PARITY_CAMERA_POS.z],
        [PARITY_CAMERA_LOOK.x, PARITY_CAMERA_LOOK.y, PARITY_CAMERA_LOOK.z],
        Some([1.0, 0.0, 0.0]),
    )
    .expect("naadf/set_camera");

    // Warm up so the TAA 32-deep ring + GI 96-frame accumulator converge.
    scenario::advance(sut.client(), PARITY_WARMUP_FRAMES).expect("warmup advance");

    let fb = scenario::capture(sut.client()).expect("parity capture");
    println!(
        "vox_web_parity [{label}]: captured {}x{}",
        fb.width(),
        fb.height()
    );

    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");
    fb
}

#[test]
fn vox_web_parity() {
    println!(
        "vox_web_parity: camera pos={:?} look={:?}; SSIM dissimilarity max {:.3}, \
         channel-max floor {:.0}",
        PARITY_CAMERA_POS,
        PARITY_CAMERA_LOOK,
        VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX,
        VOX_WEB_PARITY_CHANNEL_MAX_FLOOR,
    );

    // Phase 1 — skybox baseline: `GridPreset::Empty` via `--e2e-empty-world`.
    let skybox_fb = capture_parity_phase(
        "skybox",
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .empty_world(true),
    );

    // Phase 2 — loaded: the Oasis fixture through the production W5 GPU
    // producer chain (a bare `--vox` load, no flag).
    let loaded_fb = capture_parity_phase(
        "loaded",
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .vox(OASIS_VOX_FIXTURE),
    );

    // Save both captures (legacy filenames).
    let _ = skybox_fb.save_png("target/e2e-screenshots/vox_web_parity_skybox.png");
    let _ = loaded_fb.save_png("target/e2e-screenshots/vox_web_parity_loaded.png");

    // Assertion (1) — `web-vox-color-divergence` per-channel guard. The
    // SSIM compare below is structurally colour-blind; a near-black-but-
    // structurally-correct render still scores SSIM ≈ 0 vs the skybox
    // baseline. The per-channel floor catches "geometry correct, colors
    // collapsed" directly. Ported VERBATIM (central rect 0.30..0.70 ×
    // 0.30..0.70, floor `VOX_WEB_PARITY_CHANNEL_MAX_FLOOR`).
    let central = Rect::from_fractional(&loaded_fb, 0.30, 0.30, 0.70, 0.70);
    let loaded_channel_max = loaded_fb.region_channel_max(central);
    println!(
        "vox_web_parity: loaded frame central rect channel max = \
         {loaded_channel_max:.1} (floor > {VOX_WEB_PARITY_CHANNEL_MAX_FLOOR:.0})"
    );
    assert!(
        loaded_channel_max > VOX_WEB_PARITY_CHANNEL_MAX_FLOOR,
        "vox_web_parity gate FAIL — loaded frame channel max {loaded_channel_max:.1} \
         <= floor {VOX_WEB_PARITY_CHANNEL_MAX_FLOOR:.0}. The .vox install path \
         rendered structurally correct geometry but colorless / near-black voxels \
         (web-vox-color-divergence class). Inspect \
         crates/bevy_naadf/target/e2e-screenshots/vox_web_parity_loaded.png."
    );

    // Assertion (2) — SSIM dissimilarity. `ssim_compare_framebuffers` reused
    // VERBATIM from the library (the same impl the legacy gate calls).
    let ssim_score = ssim_compare_framebuffers(&skybox_fb, &loaded_fb)
        .unwrap_or_else(|e| panic!("vox_web_parity gate FAIL — SSIM compare failed: {e}"));
    println!(
        "vox_web_parity: SSIM = {ssim_score:.4} (threshold < \
         {VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX:.3} for dissimilarity); \
         skybox {}x{}, loaded {}x{}",
        skybox_fb.width(),
        skybox_fb.height(),
        loaded_fb.width(),
        loaded_fb.height(),
    );
    assert!(
        ssim_score < VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX,
        "vox_web_parity gate FAIL — SSIM {ssim_score:.4} >= dissimilarity max \
         {VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX:.3}. The loaded frame is \
         structurally too similar to the skybox baseline; the .vox install path \
         likely failed to populate the renderer. Inspect \
         crates/bevy_naadf/target/e2e-screenshots/vox_web_parity_{{skybox,loaded}}.png."
    );

    println!(
        "vox_web_parity: PASS — channel max {loaded_channel_max:.1} > floor + \
         SSIM {ssim_score:.4} dissimilar enough"
    );
}
