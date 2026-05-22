//! BRP-driven e2e gate — `vox_gpu_construction`, migrated from the legacy
//! in-app `e2e_render --vox-gpu-construction` driver mode
//! (`e2e::vox_gpu_construction::run_vox_gpu_construction`)
//! (`e2e-ipc-rpc-restructure` Phase 3a).
//!
//! ## What this gate proves
//!
//! The production-path W5 GPU-producer-chain gate (`e2e/vox_gpu_construction.rs`
//! module doc): load the Oasis VOX through the production `install_vox_in_fixed_world`
//! W5 chain, pin a top-down camera A, warm up, capture frame A, **promote the
//! camera to pose B** (a lateral +256-voxel sweep — NO brush; the W5 install
//! path leaves the CPU mirror empty so a brush would no-op), wait, capture
//! frame B, and assert:
//!   - rect mean per-pixel RGB Δ ≥ `VOX_GPU_CONSTRUCTION_DIFF_FLOOR` (catches
//!     the empty-scene regression — both cameras would see only sky);
//!   - frame-A near-black pixel count ≤ `VOX_GPU_CONSTRUCTION_NEAR_BLACK_FRACTION_CEILING`
//!     of the frame (catches the inversion regression — scattered hole pixels).
//!
//! ## Migration fidelity (Phase 3a brief — binding)
//!
//! Both camera poses, both Δ/near-black thresholds, and the assertion
//! `assert_vox_gpu_construction_landed` are reused from the library module
//! **verbatim**. The "promotion" — a no-brush camera move — becomes a plain
//! second `naadf/set_camera` call between the two captures.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test vox_gpu_construction
//! ```

use bevy::math::Vec3;

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::e2e::vox_gpu_construction::{
    assert_vox_gpu_construction_landed, VOX_GPU_CONSTRUCTION_CAMERA_LOOK_A,
    VOX_GPU_CONSTRUCTION_CAMERA_LOOK_B, VOX_GPU_CONSTRUCTION_CAMERA_POS_A,
    VOX_GPU_CONSTRUCTION_CAMERA_POS_B,
};

/// The Oasis VOX fixture, crate-root-relative (the SUT CWD).
const OASIS_VOX_FIXTURE: &str = "assets/test/oasis_hard_cover.vox";

// ---------------------------------------------------------------------------
// Frame budget — the legacy gate shares the Oasis driver flow (its driver
// fast-path triggers for `E2eGateMode::VoxGpuConstruction` exactly as it does
// for `E2eGateMode::OasisEdit`), so it counts the same `OASIS_WARMUP_FRAMES` +
// `OASIS_POST_EDIT_WAIT_FRAMES` budget the oasis gate uses
// (`e2e/oasis_edit_visual.rs`). Ported verbatim.
// ---------------------------------------------------------------------------

/// `OASIS_WARMUP_FRAMES` — warmup before capture A.
const OASIS_WARMUP_FRAMES: u32 = 120;
/// `OASIS_POST_EDIT_WAIT_FRAMES` — wait between the camera promotion and capture B.
const OASIS_POST_EDIT_WAIT_FRAMES: u32 = 300;

/// Camera pin: feed `naadf/set_camera` a top-down pose. The legacy
/// `pin_vox_gpu_construction_camera` writes `Transform::from_translation(pos)
/// .looking_at(look_at, Vec3::X)` — pass +X up.
fn pin_camera(sut: &mut Sut, pos: Vec3, look_at: Vec3) {
    scenario::set_camera(
        sut.client(),
        [pos.x, pos.y, pos.z],
        [look_at.x, look_at.y, look_at.z],
        Some([1.0, 0.0, 0.0]),
    )
    .expect("naadf/set_camera");
}

#[test]
fn vox_gpu_construction() {
    println!(
        "vox_gpu_construction: camera A at {:?} look {:?} → camera B at {:?} look {:?}",
        VOX_GPU_CONSTRUCTION_CAMERA_POS_A,
        VOX_GPU_CONSTRUCTION_CAMERA_LOOK_A,
        VOX_GPU_CONSTRUCTION_CAMERA_POS_B,
        VOX_GPU_CONSTRUCTION_CAMERA_LOOK_B,
    );

    // 1. Spawn the SUT — Oasis VOX through the production W5 GPU producer chain;
    //    the legacy 256×256 e2e window (the near-black-fraction ceiling is
    //    calibrated against a 256×256 frame).
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .vox(OASIS_VOX_FIXTURE)
            .window(256, 256),
    );

    // 2. World presence check.
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "vox_gpu_construction: SUT reports world_loaded=false — the Oasis VOX \
         load through the W5 GPU producer chain failed"
    );

    // 3. Pin camera A (top-down birdseye at the world centre).
    pin_camera(
        &mut sut,
        VOX_GPU_CONSTRUCTION_CAMERA_POS_A,
        VOX_GPU_CONSTRUCTION_CAMERA_LOOK_A,
    );

    // 4. Warm up — `OASIS_WARMUP_FRAMES`.
    scenario::advance(sut.client(), OASIS_WARMUP_FRAMES).expect("warmup advance");

    // 5. Capture frame A.
    let before = scenario::capture(sut.client()).expect("capture A");

    // 6. Promote the camera to pose B — the legacy gate hijacks the
    //    `OasisApplyEdit` phase as a NO-brush camera move (the W5 install path's
    //    empty CPU mirror would no-op a brush). Under BRP this is just a second
    //    `naadf/set_camera`.
    println!("vox_gpu_construction: promoting camera A→B (lateral +X sweep, no brush)");
    pin_camera(
        &mut sut,
        VOX_GPU_CONSTRUCTION_CAMERA_POS_B,
        VOX_GPU_CONSTRUCTION_CAMERA_LOOK_B,
    );

    // 7. Wait for TAA + GI convergence at the new pose — `OASIS_POST_EDIT_WAIT_FRAMES`.
    scenario::advance(sut.client(), OASIS_POST_EDIT_WAIT_FRAMES).expect("post-promotion advance");

    // 8. Capture frame B.
    let after = scenario::capture(sut.client()).expect("capture B");

    // 9. Save both framebuffers (legacy filenames).
    let _ = before.save_png("target/e2e-screenshots/vox_gpu_construction_before.png");
    let _ = after.save_png("target/e2e-screenshots/vox_gpu_construction_after.png");

    // 10. The load-bearing gate — `assert_vox_gpu_construction_landed` reused
    //     verbatim (the Δ floor + near-black ceiling live inside it).
    let report = assert_vox_gpu_construction_landed(&before, &after)
        .unwrap_or_else(|msg| panic!("vox_gpu_construction gate FAIL — {msg}"));

    // 11. Pipeline-error scan.
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");

    println!("vox_gpu_construction: {report}");
}
