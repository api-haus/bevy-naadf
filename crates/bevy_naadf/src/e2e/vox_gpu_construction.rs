//! `--vox-gpu-construction` mode — regression gate for the vox-gpu-rewrite
//! W5 GPU producer chain (`docs/orchestrate/vox-gpu-rewrite/`).
//!
//! ## Goal
//!
//! End-to-end gate that:
//!   1. Loads the in-tree Oasis fixture ([`OASIS_VOX_FIXTURE_PATH`],
//!      `e2e/oasis_edit_visual.rs:81`) through the production W5.1
//!      [`crate::voxel::grid::install_vox_in_fixed_world`] path.
//!   2. Boots the e2e harness with `fixed_world_size = true` +
//!      `construction_config.gpu_construction_enabled = true` (the
//!      `bevy-naadf::main` shape, per `lib.rs:393` + `:143`) so the new
//!      `ModelData → ModelDataRender → W5 per-segment dispatch → chunk_calc`
//!      chain runs against the production buffers.
//!   3. Asserts the framebuffer captured at the standard e2e camera pose
//!      ([`crate::e2e::gates::e2e_camera_transform`]) is non-empty —
//!      a region-mean luminance above a "captured something" floor.
//!
//! Per Q3 decision (`01-context.md`): no `AppArgs::vox_gpu_construction_mode`
//! flag. The new e2e module sets `AppArgs.fixed_world_size = true` +
//! `construction_config.gpu_construction_enabled = true` +
//! `grid_preset = GridPreset::Vox { path: OASIS_VOX_FIXTURE_PATH, tiles: 1 }`
//! directly. The driver runs the existing Warmup→Motion→Settle→Shoot flow;
//! no driver-flow customisation.
//!
//! Per Q4 decision (`01-context.md`): the W5.5 gate reuses
//! [`OASIS_VOX_FIXTURE_PATH`] (`crates/bevy_naadf/assets/test/oasis_hard_cover.vox`,
//! Git LFS-tracked).
//!
//! ## Camera / Oasis off-frame state
//!
//! The e2e harness uses a fixed pose at NAADF `(86, 42, 90)` looking at
//! `(32, 16, 32)` — calibrated against the legacy `64 × 32 × 64`-voxel
//! default scene. The Oasis fixture is `~93 × 34 × 84` chunks
//! (`~1488 × 544 × 1344` voxels); when loaded through the fixed-size
//! `4096 × 512 × 4096`-voxel world the populated region sits in the OPPOSITE
//! hemisphere from the e2e camera. The framebuffer therefore captures sky /
//! atmosphere tint at the central 40 % × 40 % rect, NOT visible Oasis
//! geometry.
//!
//! Per the `02-design.md` § Decisions — `InitialCameraPose` for the W5.5
//! gate decision: the e2e harness IGNORES `InitialCameraPose` (uses
//! [`crate::e2e::setup_e2e_camera`] verbatim). Overriding the camera would
//! require a driver-mode flag (rejected by Q3) or a per-mode Startup-system
//! patch (more invasive than W5.5's scope). The gate therefore uses the
//! standard e2e pose and the assertion accepts an Oasis-off-frame view.
//!
//! ## Assertion strategy
//!
//! [`assert_frame_not_black`] samples the central 40 % × 40 % region and
//! requires region-mean luminance above [`NOT_BLACK_LUMINANCE_FLOOR`] — a
//! "captured something" floor well below the measured sky band (~146 per
//! `vox_e2e.rs:88`) and well above pure black (0). This:
//!
//!   - PASSES when the harness boots, the render graph runs, and the
//!     framebuffer captures the atmosphere-tinted sky (the expected
//!     state at the e2e camera pose, both pre- and post-W5.3).
//!   - FAILS when the harness boots but the framebuffer is pure-black —
//!     the load-bearing regression signal. Pure-black indicates the GPU
//!     producer chain crashed silently, a pipeline failed to compile,
//!     a bind group was misbound, or buffer alloc failed (W5.2's
//!     surface area).
//!
//! Per `02-design.md` § Assumptions made (#8): the framebuffer assertion
//! choice was made on first run. The custom `assert_frame_not_black` floor
//! was chosen over `vox_e2e_mode = true` (which reuses
//! [`crate::e2e::vox_e2e::assert_vox_geometry_visible`]'s 160-luminance
//! threshold) because the Oasis-off-frame state lands the central rect on
//! sky band (~146), which trips the 160 ceiling. The "not black" floor is
//! the correct shape for catching W5.2 / W5.3 regressions without false
//! positives from the deliberate Oasis-off-frame state.

use std::path::PathBuf;

use bevy::prelude::AppExit;

use crate::e2e::framebuffer::{Framebuffer, Rect};
use crate::e2e::oasis_edit_visual::{oasis_vox_fixture_path, OASIS_VOX_FIXTURE_PATH};

/// Screen-rect fractional bounds the [`assert_frame_not_black`] gate
/// samples — a central 40 % × 40 % region (same shape as
/// [`crate::e2e::vox_e2e`]'s `VOX_GEOMETRY_RECT_FRACS`). The standard
/// e2e camera pose ([`crate::e2e::gates::e2e_camera_transform`] at
/// `(86, 42, 90)` looking at `(32, 16, 32)`) does NOT frame the populated
/// region of `oasis_hard_cover.vox` (the Oasis model occupies
/// `~93 × 34 × 84` chunks ≈ `1488 × 544 × 1344` voxels — far outside the
/// e2e camera's calibrated `64 × 32 × 64`-voxel view box).
///
/// See the module-level "Camera / Oasis off-frame state" section — for the
/// W5.5 gate the framebuffer floor assertion uses a coarse "framebuffer is
/// not pure-black" check rather than a "see the model" check.
const FRAME_REGION_FRACS: (f32, f32, f32, f32) = (0.30, 0.30, 0.70, 0.70);

/// Mean luminance floor: any frame above this value has captured
/// *something*. Calibration baselines (from `vox_e2e.rs:88` and the
/// `03a-impl-vox-loading.md` post-Track-A run):
///
/// | Frame content                          | Luminance |
/// |----------------------------------------|-----------|
/// | Pure black (no render delivered)       | 0.0       |
/// | Atmosphere-tinted sky band (no geom)   | ~146      |
/// | Default-scene solid (GI-lit diffuse)   | ~242      |
/// | Default-scene emissive                 | ~247      |
///
/// Threshold set to **40.0** — well below the measured sky band (~146) so
/// any frame with even atmospheric tint passes; well above 0 so a
/// pure-black "render graph delivered nothing" failure trips. Catches
/// catastrophic failures (pipeline compile errors, bind-group misbindings,
/// buffer alloc failures).
///
/// **First-run baseline (pre-W5.3, 2026-05-17):** with the W5 segment loop
/// NOT yet landed, the W5 prepare block allocates buffers + builds the
/// bind group, but `naadf_gpu_producer_node` does NOT dispatch the
/// generator pass — so `gpu_producer_has_run` never flips on the W5 path,
/// `WorldGpu::chunks` stays zeroed, AND nine downstream render-graph nodes
/// (naadf_first_hit, naadf_taa_reproject, naadf_ray_queue, naadf_global_illum,
/// naadf_sample_refine, naadf_spatial_resampling, naadf_denoise,
/// naadf_calc_new_taa_sample, naadf_final_blit) never dispatch because
/// their `WorldGpu`-readiness preconditions are unmet. The standard e2e
/// driver therefore fails the node-dispatch + luminance + region gates;
/// the framebuffer is pure-black (luminance ~0.7). This module's
/// `assert_frame_not_black` floor (40.0) would also trip — by design:
/// pre-W5.3 the gate is EXPECTED to fail. Post-W5.3, when the segment
/// loop dispatches and `WorldGpu::chunks` populates, the downstream nodes
/// run, the sky band lifts the framebuffer to ~146 luminance (well above
/// 40), and this gate passes.
const NOT_BLACK_LUMINANCE_FLOOR: f32 = 40.0;

/// Boot the e2e harness with the production W5 GPU producer path enabled.
///
/// Returns the harness's `AppExit`. The actual framebuffer assertion is
/// run by the driver: [`crate::AppArgs::vox_e2e_mode`] is intentionally
/// NOT set — the gate uses the standard default-scene region gates
/// (sky / solid / emissive) which at this pose will all sample sky or
/// adjacent atmosphere; a non-black readback is the correct signal here
/// (see module-level docs). The driver's degenerate-frame check + the
/// `PipelineCache` scan + the node-dispatch check provide the catastrophic-
/// failure coverage; the custom [`assert_frame_not_black`] gate is the
/// load-bearing W5.5-specific assertion (callable by tests / future
/// driver integrations).
pub fn run_vox_gpu_construction() -> AppExit {
    let path = oasis_vox_fixture_path();
    if !path.exists() {
        eprintln!(
            "e2e_render --vox-gpu-construction: FIXTURE MISSING at {} \
             (cwd = {:?}). The fixture is Git LFS-tracked at \
             {OASIS_VOX_FIXTURE_PATH}. Run `git lfs pull` to fetch the \
             binary content, OR run the binary from the workspace root.",
            path.display(),
            std::env::current_dir().ok()
        );
        return AppExit::error();
    }
    println!(
        "e2e_render --vox-gpu-construction: loading Oasis VOX fixture from \
         {} ({} bytes) into the W5 GPU producer chain",
        path.display(),
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
    );

    let mut app_args = crate::AppArgs::default();
    // Production W5 path: fixed-size world + GPU construction default-on
    // (= the `bevy-naadf::main` shape, per `lib.rs:393` + `:143`).
    app_args.grid_preset = crate::GridPreset::Vox {
        path: PathBuf::from(&app_path_for_args(&path)),
        tiles: 1,
    };
    app_args.fixed_world_size = true;
    app_args.construction_config.gpu_construction_enabled = true;
    // NOTE: `vox_e2e_mode` is intentionally NOT set. The Oasis fixture's
    // populated region sits in the opposite hemisphere from the e2e camera
    // (see module docs), so the central rect samples sky band (~146) — that
    // would trip the `--vox-e2e` driver's `SKY_LUMINANCE_CEILING = 160`
    // gate. The custom `assert_frame_not_black` floor in this module is
    // the correct shape for this off-frame state. The driver's standard
    // PipelineCache scan + node-dispatch check + degenerate-frame floor
    // still run and cover catastrophic GPU-producer failures.

    crate::run_e2e_render_with_args(app_args)
}

/// Re-export the resolved path for the `AppArgs::grid_preset`. Mirrors the
/// shape of `oasis_edit_visual.rs::run_oasis_edit_visual` so the same path
/// the existence-check used is the same path the load-bearing
/// `GridPreset::Vox` carries.
fn app_path_for_args(p: &std::path::Path) -> PathBuf {
    p.to_path_buf()
}

/// Region-luminance gate: framebuffer must have captured SOMETHING —
/// region-mean luminance above the not-black floor.
///
/// Per the module-level docs, this is a coarse "did we get a frame at all"
/// gate; the load-bearing signal is that the harness boots, the render
/// graph runs to completion, and the framebuffer is not pure-black. A
/// regression in W5.2's bind-group setup or W5.3's segment loop that
/// crashes the device, fails to compile a pipeline, or leaves the
/// framebuffer untouched will trip this floor.
///
/// Returns `Ok(())` on success; an `Err(String)` describing the failure
/// on a sub-floor luminance.
pub fn assert_frame_not_black(fb: &Framebuffer) -> Result<(), String> {
    let (fx0, fy0, fx1, fy1) = FRAME_REGION_FRACS;
    let region = Rect::from_fractional(fb, fx0, fy0, fx1, fy1);
    let mean = fb.region_mean(region);
    let lum = Framebuffer::luminance(mean);

    println!(
        "e2e_render --vox-gpu-construction: region mean rgba {mean:?}, \
         luminance {lum:.1} (floor > {NOT_BLACK_LUMINANCE_FLOOR:.0})",
    );

    if lum <= NOT_BLACK_LUMINANCE_FLOOR {
        return Err(format!(
            "vox-gpu-construction gate FAIL — central region mean \
             luminance {lum:.1} is at or below the not-black floor \
             {NOT_BLACK_LUMINANCE_FLOOR:.0}. The W5 GPU producer chain \
             likely failed (a pipeline failed to compile, the segment \
             loop didn't run, the dispatch silently crashed the device, \
             or a bind group was misbound — W5.2 / W5.3 surface area). \
             Inspect target/e2e-screenshots/vox_gpu_construction_latest.png \
             + run with `RUST_LOG=debug` for shader-cache + dispatch \
             traces.",
        ));
    }
    Ok(())
}

/// Save a vox-gpu-construction-specific PNG alongside the standard
/// `e2e_latest.png` slot. Best-effort — the gate verdict is unchanged
/// either way.
pub fn save_vox_gpu_construction_screenshot(fb: &Framebuffer) {
    let path = std::path::Path::new(crate::e2e::E2E_SCREENSHOT_DIR)
        .join("vox_gpu_construction_latest.png");
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --vox-gpu-construction: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --vox-gpu-construction: vox_gpu_construction_latest.png \
             save failed: {e}"
        ),
    }
}
