//! Command-line options, parsed once and stored as a Bevy `Resource`
//! (`03-design.md` §4.1).
//!
//! After Step 8 of the config-as-resource refactor this carries ONLY the
//! flat set of e2e mode/phase booleans that drive the e2e harness dispatch
//! from `bin/e2e_render.rs`. At most one e2e flag is true at a time — the
//! enum collapse is deferred to a future Step 6 (D6+D7 paired) since it
//! crosses 11 mode files. Steps 6/7/9 drain the remaining shell.
//!
//! **Step 2 of the config-as-resource refactor** migrated the user's named
//! smell `taa_ring_depth` out of this struct onto the
//! [`crate::render::taa::TaaRingConfig`] per-domain main-world resource. The
//! pin tests moved with it to `crates/bevy_naadf/src/render/taa.rs::tests`.
//!
//! **Step 3 of the config-as-resource refactor** migrated `taa: bool` and
//! `gi: GiSettings` onto the per-domain
//! [`crate::render::taa::TaaConfig`] / [`crate::GiSettings`] resources. The
//! settings panel, the diagnostics dump, and the render-world extract systems
//! now read those resources directly; nothing in this file references TAA or
//! GI any more.
//!
//! **Step 4 of the config-as-resource refactor** migrated
//! `construction_config: ConstructionConfig` onto the per-domain
//! [`crate::render::construction::ConstructionConfig`] resource. Bootstrap
//! inserts it from `BootstrapInputs.construction_config`; the render sub-app
//! mirror is extract-driven; the wasm32 divergence (previously inside the
//! deleted `From<&AppArgs>` impl) now lives on
//! `ConstructionConfig::for_target_arch()` (Decision §5).
//!
//! **Step 5 of the config-as-resource refactor** migrated
//! `grid_preset: GridPreset` onto a per-domain main-world resource. The
//! native `--vox <path>` flag and the wasm32 `?skybox=1` URL param now
//! resolve into `BootstrapInputs.grid_preset` BEFORE the App is built;
//! `setup_test_grid` reads `Res<GridPreset>` instead of `Res<AppArgs>`.

use bevy::prelude::*;

/// Command-line options, parsed once and stored as a resource
/// (`03-design.md` §4.1).
///
/// After Steps 2-5 + 8 of the config-as-resource refactor only the
/// 10 e2e mode booleans + `vox_e2e_mode` (Bucket B, migrate in Step 6 /
/// Step 7) remain here. Steps 6/7/9 drain the shell.
#[derive(Resource, Clone)]
pub struct AppArgs {
    /// When `true`, the e2e driver runs the **resize-blackness reproduction
    /// test** instead of the standard WARMUP→MOTION→SETTLE→SHOOT flow.
    ///
    /// Permanent regression coverage for the GI-bounce-on-resize fix
    /// (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
    /// `## GI-bounce-on-resize fix (2026-05-16)`). Boots at 800×600,
    /// settles, screenshots, hyprctl-resizes to 1920×1080, settles,
    /// screenshots, hyprctl-resizes to 2000×1000, settles, screenshots,
    /// then compares full-frame luma ratios against
    /// `E2E_RESIZE_MIN_LUMA_RATIO = 0.7`. Without the
    /// `MAX_INDIRECT_GROUPS` cap in `sample_refine.wgsl`, wgpu's
    /// indirect-validation pass zeros the `count_invalid_data` dispatch at
    /// the larger viewports → GI bounce disappears → ratio collapses to
    /// ~0.5. See [`crate::e2e::driver`].
    pub resize_test: bool,
    /// When `true`, the e2e driver swaps the default `assert_batch_6`
    /// region gates for the `--vox-e2e` "non-skybox" assertion. The
    /// default-scene gate rectangles (`solid_block_rect`, `emissive_rect`,
    /// etc.) are tuned for the hard-coded test grid's content layout, so
    /// they don't apply when [`GridPreset::Vox`] loaded a different scene.
    ///
    /// Permanent regression coverage for the `.vox` ingestion path landed
    /// in Track A (`docs/orchestrate/feature-completeness/03a-impl-vox-loading.md`)
    /// — the brief explicitly required an automated assert that the
    /// framebuffer captures something other than skybox after loading a
    /// `.vox` file through the production `--vox` path. See
    /// [`crate::e2e::vox_e2e`].
    pub vox_e2e_mode: bool,
    /// `02f-followup` — when `true`, the e2e driver runs the
    /// **oasis-edit-visual gate**: birdseye over a loaded Oasis VOX scene,
    /// screenshot A, programmatically erase a sphere at world centre via the
    /// runtime brush path, wait 5 s for the W2 GPU dispatch + GI / TAA to
    /// converge, screenshot B, assert framebuffer pixels around the erase
    /// projection meaningfully changed. Catches the regression class
    /// `--runtime-edit-mode`'s record-counter gate misses: edits land in the
    /// W2 batch but never reach the framebuffer (the `81171f9` regression).
    /// See [`crate::e2e::oasis_edit_visual`].
    pub oasis_edit_visual_mode: bool,
    /// `03g` — when `true`, the e2e driver runs the **small-edit-visual
    /// gate**: birdseye over the default test grid, screenshot A,
    /// programmatically place a single 1×1×1 voxel via the runtime
    /// `cube_brush` path with radius=1, count non-empty voxels before vs
    /// after (must differ by exactly +1 — catches Mode 2 phantom-voxel
    /// bugs), wait for W2 / W3 / TAA convergence, screenshot B, assert
    /// framebuffer changed in the click bbox AND did NOT change in
    /// adjacent bboxes (catches Mode 1 AADF-skip cross-section bugs). See
    /// [`crate::e2e::small_edit_visual`].
    pub small_edit_visual_mode: bool,
    /// `2026-05-17` — when `true`, the e2e driver runs the
    /// **small-edit-repro gate**: load the Oasis VOX fixture, pin the camera
    /// to a user-captured pose, programmatically place a single 1×1×1 voxel
    /// at the user-captured brush position, then assert the post-edit
    /// framebuffer contains NO pitch-black (RGB == 0,0,0) pixels. Catches
    /// the user-reported "small edits render as inverted black shapes"
    /// regression that `--small-edit-visual` does not catch. See
    /// [`crate::e2e::small_edit_repro`].
    pub small_edit_repro_mode: bool,
    /// `vox-gpu-rewrite W5.3-fix Stage 1` — when `true`, the e2e driver runs
    /// the **vox-gpu-construction production-path gate**: load the Oasis VOX
    /// fixture through `install_vox_in_fixed_world`'s W5 GPU producer chain
    /// (Stage 2 consolidation 2026-05-18: the production install path is now
    /// the ONLY install path; `gpu_construction_enabled = true` is the only
    /// remaining knob), pin the camera to C#'s literal `(500, 200, 40)` voxel spawn
    /// (`WorldRender.cs:48-49`), capture frame A, dispatch a sphere brush
    /// directly in front of the camera, wait ~5 s for W2 / GI / TAA to
    /// converge, capture frame B, assert the per-pixel RGB Δ over a central
    /// rect exceeds the floor (the `--oasis-edit-visual` assertion shape).
    ///
    /// The gate routes through the same `OasisWarmup → OasisShootBefore →
    /// OasisApplyEdit → OasisWaitPostEdit → OasisShootAfter → OasisAssert`
    /// driver phases as `--oasis-edit-visual` (the brush + capture +
    /// assertion mechanics are identical), but the camera and brush
    /// position are mode-specific. The flag IS load-bearing: it deviates
    /// from Q3 (`docs/orchestrate/vox-gpu-rewrite/01-context.md`) which
    /// rejected this flag, but the user's "why the fuck does this 'e2e'
    /// test avoid the same production path?" frustration justifies the
    /// deviation — the production-path-faithful gate is more valuable than
    /// preserving Q3.
    pub vox_gpu_construction_mode: bool,
    /// `vox-gpu-rewrite W5.3-fix Stage 4` — when `true`, the e2e driver runs a
    /// **CPU oracle render phase** for the `--vox-gpu-oracle` gate: load Oasis
    /// via the legacy `install_vox_sized_to_model` path (the known-good CPU
    /// renderer used by `--oasis-edit-visual`), pin a shared in-world camera
    /// pose, warm up, capture a single screenshot to `oracle_cpu.png`, then
    /// exit. The oracle gate's compare phase reads this PNG and the matching
    /// `oracle_gpu.png` from disk and asserts per-pixel diff < small floor.
    /// See [`crate::e2e::vox_gpu_oracle`].
    pub vox_gpu_oracle_cpu_phase: bool,
    /// `vox-gpu-rewrite W5.3-fix Stage 4` — when `true`, the e2e driver runs a
    /// **GPU producer render phase** for the `--vox-gpu-oracle` gate: load
    /// Oasis via `install_vox_in_fixed_world` (the W5 GPU producer chain), pin
    /// the SAME camera pose as the CPU oracle phase (in world voxel coords;
    /// the camera position must hit the first XZ tile of Oasis so the GPU
    /// tiling collapses to the same voxel data the CPU oracle holds), warm up,
    /// capture a single screenshot to `oracle_gpu.png`, then exit. See
    /// [`crate::e2e::vox_gpu_oracle`].
    pub vox_gpu_oracle_gpu_phase: bool,
    /// web-vox-async-loading 2026-05-18 follow-up Step 8 / Q5 — when `true`,
    /// boots the e2e harness with `GridPreset::Empty` (skybox baseline) and
    /// captures a single screenshot to `vox_web_parity_skybox.png`. The
    /// `--vox-web-parity` top-level mode spawns this as a subprocess.
    pub vox_web_parity_skybox_phase: bool,
    /// web-vox-async-loading 2026-05-18 follow-up Step 8 / Q5 — when `true`,
    /// boots the e2e harness with `GridPreset::Vox { path: oasis }` (the
    /// production W5 GPU producer chain) and captures a single screenshot to
    /// `vox_web_parity_loaded.png`. The `--vox-web-parity` top-level mode
    /// spawns this as a subprocess; the compare phase SSIM-asserts the two
    /// captured PNGs are dissimilar.
    pub vox_web_parity_loaded_phase: bool,
    /// 2026-05-19 — when `true`, boots the e2e harness with
    /// `GridPreset::Vox { path: oasis.cvox }` through the production W5 GPU
    /// producer chain, pins the camera at the C#-faithful horizon pose
    /// (`InitialCameraPose::from_world_voxels(WORLD_SIZE_IN_VOXELS)`), and
    /// captures `vox_horizon_native.png` at a 1280×720 window — the
    /// resolution the Playwright cross-target SSIM gate compares against.
    /// See [`crate::e2e::vox_horizon_parity`].
    pub vox_horizon_native_phase: bool,
}

impl Default for AppArgs {
    fn default() -> Self {
        Self {
            resize_test: false,
            vox_e2e_mode: false,
            oasis_edit_visual_mode: false,
            small_edit_visual_mode: false,
            small_edit_repro_mode: false,
            vox_gpu_construction_mode: false,
            vox_gpu_oracle_cpu_phase: false,
            vox_gpu_oracle_gpu_phase: false,
            vox_web_parity_skybox_phase: false,
            vox_web_parity_loaded_phase: false,
            vox_horizon_native_phase: false,
        }
    }
}

// Step 2 of the config-as-resource refactor (`02-design.md` §4 Step 2): the
// `taa_ring_depth` pin tests moved to `render/taa.rs::tests` along with the
// migrated field. No remaining `AppArgs`-rooted tests live in this file;
// subsequent migration steps may add per-bucket tests on their target
// resources, not here.
