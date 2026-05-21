//! BRP-driven e2e gate — `small_edit_visual`, migrated from the legacy in-app
//! `e2e_render --small-edit-visual` driver mode
//! (`e2e::small_edit_visual::run_small_edit_visual`)
//! (`e2e-ipc-rpc-restructure` Phase 3a).
//!
//! ## What this gate proves
//!
//! The single-voxel edit gate (`e2e/small_edit_visual.rs` module doc): boot the
//! default test grid, pin a birdseye camera over the click voxel, warm up,
//! capture frame A, apply a `cube_brush(radius=1)` at a known-empty voxel via
//! the production runtime path, wait, capture frame B, and assert:
//!   - **Mode 2 (phantom voxels):** the edit produces exactly +1 non-empty
//!     voxel (a phantom-emitting encoder would produce +2 / +3).
//!   - **Mode 1 (cross-section / AADF-skip):** the click rect shows a clear
//!     max-Δ signal, adjacent rects stay below the ceiling, and the
//!     whole-frame catastrophic-pixel fraction stays low.
//!
//! ## Migration fidelity (Phase 3a brief — binding)
//!
//! Every constant + assertion is reused from the library `small_edit_visual`
//! module **verbatim** — `SMALL_EDIT_*` budgets/thresholds, `SMALL_EDIT_CLICK_VOXEL`,
//! `SMALL_EDIT_PAINT_TYPE`, `SMALL_EDIT_RADIUS`, the `birdseye_pose()` camera
//! math, `click_voxel_rects`, and `assert_small_edit_landed` (the load-bearing
//! gate). The test never re-implements the assertion.
//!
//! ## The Mode-2 (CPU voxel-count) signal under BRP
//!
//! The legacy gate counts non-empty voxels in the demo embed before/after the
//! brush (`count_non_empty_voxels`) and asserts `after == before + 1`. A first
//! migration draft tried to reuse `naadf/apply_brush`'s `voxels_delta` return
//! as that signal — **wrong**: `voxels_delta` is the change in
//! `WorldData::voxels_cpu` *array length*, and `voxels_cpu` packs one 4×4×4
//! voxel block as a 32-`u32` record, so a single new voxel in a previously
//! empty block reports `voxels_delta = 32`. Phase 3a therefore adds a
//! `naadf/count_demo_voxels` BRP verb wrapping the library
//! `count_non_empty_voxels` (the genuine non-empty-voxel decode-count, scoped
//! to the demo embed). The test reads the count before + after the brush and
//! feeds `assert_small_edit_landed` the real pair — the legacy `after ==
//! before + 1` Mode-2 phantom-voxel catch, verbatim.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test small_edit_visual
//! ```

use naadf_e2e::{scenario, schema, Sut, SutOpts};

use bevy_naadf::e2e::small_edit_visual::{
    assert_small_edit_landed, birdseye_pose, small_edit_click_voxel_world, SMALL_EDIT_CLICK_VOXEL,
    SMALL_EDIT_PAINT_TYPE, SMALL_EDIT_RADIUS, SMALL_EDIT_WARMUP_FRAMES,
    SMALL_EDIT_POST_EDIT_WAIT_FRAMES,
};

#[test]
fn small_edit_visual() {
    println!(
        "small_edit_visual: target click voxel {:?} (world {:?}), brush radius {SMALL_EDIT_RADIUS}, \
         paint type {:?}",
        SMALL_EDIT_CLICK_VOXEL,
        small_edit_click_voxel_world(),
        SMALL_EDIT_PAINT_TYPE
    );

    // 1. Spawn the SUT — no `--vox` ⇒ the `GridPreset::Default` embedded test
    //    grid (the gate edits the hard-coded scene); the legacy 256×256 window.
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR")).window(256, 256),
    );

    // 2. World presence + size.
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "small_edit_visual: SUT reports world_loaded=false — the default test \
         grid failed to install"
    );
    let world_size = state
        .world_size_voxels
        .expect("world_loaded but world_size_voxels is None");

    // 3. Pin the birdseye camera (`small_edit_visual::birdseye_pose` — a `pub`
    //    library fn returning the exact `Transform` the legacy `pin_small_edit_camera`
    //    writes). `naadf/set_camera` rebuilds it from translation + look point.
    let pose = birdseye_pose();
    let translation = [pose.translation.x, pose.translation.y, pose.translation.z];
    let fwd = pose.forward();
    let look_at = [
        pose.translation.x + fwd.x,
        pose.translation.y + fwd.y,
        pose.translation.z + fwd.z,
    ];
    // The legacy `birdseye_pose` uses `looking_at(.., Vec3::X)` — pass +X up.
    scenario::set_camera(sut.client(), translation, look_at, Some([1.0, 0.0, 0.0]))
        .expect("naadf/set_camera");

    // 4. Warm up — `SMALL_EDIT_WARMUP_FRAMES` (TAA + GI convergence).
    scenario::advance(sut.client(), SMALL_EDIT_WARMUP_FRAMES).expect("warmup advance");

    // 5. Capture frame A + the pre-edit non-empty voxel count (the Mode-2
    //    baseline — `naadf/count_demo_voxels`, see the module doc).
    let before = scenario::capture(sut.client()).expect("capture A");
    let voxel_count_before =
        scenario::count_demo_voxels(sut.client()).expect("naadf/count_demo_voxels (before)");

    // 6. Apply the single-voxel cube edit through the production brush path.
    //    The legacy `apply_small_cube_edit` calls `cube_brush(world, pos,
    //    SMALL_EDIT_RADIUS, SMALL_EDIT_PAINT_TYPE, is_erase=false)` with `pos`
    //    the voxel-centre world coord (`click.as_vec3() + 0.5`). `naadf/apply_brush`
    //    `kind:"cube"` calls the identical `cube_brush`.
    let click = small_edit_click_voxel_world();
    let pos = [
        click.x as f32 + 0.5,
        click.y as f32 + 0.5,
        click.z as f32 + 0.5,
    ];
    let brush: schema::ApplyBrushResult = sut
        .client()
        .call_typed(
            "naadf/apply_brush",
            serde_json::json!({
                "kind": "cube",
                "pos": pos,
                "radius": SMALL_EDIT_RADIUS,
                "voxel_type": SMALL_EDIT_PAINT_TYPE.raw() as u32,
                "erase": false,
            }),
        )
        .expect("naadf/apply_brush");
    println!(
        "small_edit_visual: cube_brush at {pos:?} — voxels_delta {} blocks_delta {} batches {}",
        brush.voxels_delta, brush.blocks_delta, brush.batches
    );

    // 7. Read the post-edit non-empty voxel count. Done immediately after the
    //    brush (it is a CPU-mirror count — independent of render convergence),
    //    mirroring the legacy `apply_small_cube_edit`'s before/after snapshot.
    let voxel_count_after =
        scenario::count_demo_voxels(sut.client()).expect("naadf/count_demo_voxels (after)");
    println!(
        "small_edit_visual: non-empty demo voxels {voxel_count_before} → {voxel_count_after} \
         (Δ {}; expected +1)",
        voxel_count_after as i64 - voxel_count_before as i64
    );

    // 8. Wait for W2 GPU dispatch + W3 background AADF + TAA/GI re-convergence.
    scenario::advance(sut.client(), SMALL_EDIT_POST_EDIT_WAIT_FRAMES).expect("post-edit advance");

    // 9. Capture frame B.
    let after = scenario::capture(sut.client()).expect("capture B");

    // 10. Save both framebuffers (legacy filenames `small_edit_before.png` /
    //     `small_edit_after.png`).
    let _ = before.save_png("target/e2e-screenshots/small_edit_before.png");
    let _ = after.save_png("target/e2e-screenshots/small_edit_after.png");

    // 11. The load-bearing gate — `assert_small_edit_landed` reused verbatim.
    //     Fed the genuine pre/post non-empty voxel counts (`naadf/count_demo_voxels`),
    //     so the library's Mode-2 `voxel_count_after == voxel_count_before + 1`
    //     phantom-voxel check runs exactly as the legacy gate's.
    let report = assert_small_edit_landed(
        &before,
        &after,
        voxel_count_before,
        voxel_count_after,
        world_size,
    )
    .unwrap_or_else(|msg| panic!("small_edit_visual gate FAIL — {msg}"));

    // 12. Pipeline-error scan.
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");

    println!("small_edit_visual: {report}");
}
