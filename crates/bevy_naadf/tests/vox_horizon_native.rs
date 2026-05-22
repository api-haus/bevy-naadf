//! BRP-driven e2e gate — `vox_horizon_native`, migrated from the legacy in-app
//! `e2e_render --vox-horizon-native` driver mode
//! (`e2e::vox_horizon_parity::run_vox_horizon_native_phase`)
//! (`e2e-ipc-rpc-restructure` Phase 3a).
//!
//! ## What this gate proves
//!
//! The native side of the cross-target (native ↔ WASM) horizon-view parity
//! gate (`e2e/vox_horizon_parity.rs` module doc): load the Oasis `.cvox`
//! through the production W5 GPU producer chain, pin the camera to the
//! C#-faithful default horizon pose at 1280×720, warm up so TAA + GI converge,
//! capture, and **write the native PNG to disk**. The legacy gate's only pass
//! criterion is "the screenshot was captured + saved" — the SSIM comparison
//! against the WASM-side capture is the separate `--vox-horizon-parity` /
//! Playwright step.
//!
//! ## Phase 4 contract — the native PNG path (design §8 item 1)
//!
//! Phase 4 repoints `e2e/tests/vox-horizon-parity.spec.ts` to drive this test
//! instead of `cargo run --bin e2e_render -- --vox-horizon-native`. The
//! Playwright spec reads the native PNG from `target/e2e-screenshots/vox_horizon_native.png`.
//! This test writes exactly that file as a **side effect** — see step 6. The
//! test process CWD is the `bevy_naadf` crate root (`CARGO_MANIFEST_DIR`), so
//! the relative path resolves to `crates/bevy_naadf/target/e2e-screenshots/`,
//! the same place the legacy `save_horizon_screenshot` wrote it.
//!
//! ## Migration fidelity (Phase 3a brief — binding)
//!
//! The camera pose (`HORIZON_CAMERA_POS` / `HORIZON_CAMERA_ROT`), the
//! 1280×720 window (`HORIZON_WIDTH` / `HORIZON_HEIGHT`), the warmup budget
//! (`HORIZON_WARMUP_FRAMES`), and the output filename (`HORIZON_NATIVE_PNG`)
//! are reused from the library module **verbatim**. The raw-quaternion pose is
//! reconstructed via forward + up exactly as in the `small_edit_repro` gate.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test vox_horizon_native
//! ```

use bevy::math::Vec3;

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::camera::poses::{HORIZON_CAMERA_POS, HORIZON_CAMERA_ROT};
use bevy_naadf::e2e::vox_horizon_parity::{
    HORIZON_HEIGHT, HORIZON_NATIVE_PNG, HORIZON_WARMUP_FRAMES, HORIZON_WIDTH,
};

/// The Oasis `.cvox` fixture, crate-root-relative (the SUT CWD). The legacy
/// `oasis_cvox_fixture_path()` resolves the workspace-relative path or this
/// crate-relative fallback; with the SUT CWD at the crate root the
/// crate-relative form resolves.
const OASIS_CVOX_FIXTURE: &str = "assets/test/oasis.cvox";

#[test]
fn vox_horizon_native() {
    println!(
        "vox_horizon_native: camera translation={:?} rotation={:?}; window {}x{}; \
         output {HORIZON_NATIVE_PNG}",
        HORIZON_CAMERA_POS, HORIZON_CAMERA_ROT, HORIZON_WIDTH, HORIZON_HEIGHT,
    );

    // 1. Spawn the SUT — Oasis `.cvox` through the production W5 GPU producer
    //    chain; the 1280×720 horizon window (matches the Playwright viewport so
    //    the cross-target PNGs SSIM-compare without resize).
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .vox(OASIS_CVOX_FIXTURE)
            .window(HORIZON_WIDTH, HORIZON_HEIGHT),
    );

    // 2. World presence check.
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "vox_horizon_native: SUT reports world_loaded=false — the Oasis .cvox \
         load through the W5 GPU producer chain failed"
    );

    // 3. Pin the C#-faithful horizon camera pose. The legacy
    //    `pin_vox_horizon_camera` writes a raw quaternion — reconstruct it via
    //    forward + up (exact for a unit rotation; see the `small_edit_repro`
    //    gate's module doc).
    let pos = HORIZON_CAMERA_POS;
    let quat = HORIZON_CAMERA_ROT;
    let forward = quat * Vec3::NEG_Z;
    let up = quat * Vec3::Y;
    let look_at = pos + forward;
    scenario::set_camera(
        sut.client(),
        [pos.x, pos.y, pos.z],
        [look_at.x, look_at.y, look_at.z],
        Some([up.x, up.y, up.z]),
    )
    .expect("naadf/set_camera");

    // 4. Warm up — `HORIZON_WARMUP_FRAMES` (TAA + GI convergence; reuses the
    //    parity warmup budget).
    scenario::advance(sut.client(), HORIZON_WARMUP_FRAMES).expect("warmup advance");

    // 5. Capture the horizon frame. `scenario::capture` errors if no capture
    //    delivers — that delivery is the legacy gate's sole pass criterion.
    let fb = scenario::capture(sut.client()).expect("capture");
    assert_eq!(
        (fb.width(), fb.height()),
        (HORIZON_WIDTH, HORIZON_HEIGHT),
        "vox_horizon_native: captured framebuffer {}x{} does not match the \
         requested {HORIZON_WIDTH}x{HORIZON_HEIGHT} horizon window",
        fb.width(),
        fb.height()
    );

    // 6. PHASE 4 CONTRACT — write the native PNG to the path the Playwright
    //    cross-target spec reads (`target/e2e-screenshots/vox_horizon_native.png`).
    //    `HORIZON_NATIVE_PNG` is the legacy filename; the directory is relative
    //    to the crate-root CWD, exactly as the legacy `save_horizon_screenshot`.
    let native_png = format!("target/e2e-screenshots/{HORIZON_NATIVE_PNG}");
    fb.save_png(&native_png).unwrap_or_else(|msg| {
        panic!("vox_horizon_native: failed to write the native PNG to {native_png}: {msg}")
    });
    println!(
        "vox_horizon_native: PASS — native horizon capture {}x{} written to \
         crates/bevy_naadf/{native_png}",
        fb.width(),
        fb.height()
    );
}
