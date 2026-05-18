//! bevy-naadf — Bevy 0.19 port of the NAADF voxel renderer (library surface).
//!
//! Port of NAADF (`/mnt/archive4/DEV/NAADF`, a C#/MonoGame engine — "Nested
//! Axis-Aligned Distance Fields", Ulschmid et al., CGF 2026) to Rust/Bevy.
//!
//! This `lib.rs` carries the shared app-wiring path so the production binary
//! (`src/main.rs`) and the e2e render-test binary (`src/bin/e2e_render.rs`)
//! build the *same* app — `main.rs` is a thin shim over [`build_app`], and the
//! e2e binary boots [`build_app`] with [`AppConfig::e2e`] then drives the
//! bounded-frame harness (see [`crate::e2e`] / `docs/orchestrate/naadf-bevy-port/
//! e2e-render-test.md`).

pub mod aadf;
pub mod camera;
pub mod cli;
pub mod e2e;
pub mod editor;
pub mod hud;
pub mod panel;
pub mod render;
pub mod streaming;
pub mod texture_array;
pub mod voxel;
pub mod world;

/// Roboto Regular TTF bytes, embedded at compile time. The font is Apache 2.0;
/// see `src/assets/fonts/Roboto-LICENSE.txt`.
static ROBOTO_REGULAR_BYTES: &[u8] =
    include_bytes!("assets/fonts/Roboto-Regular.ttf");

/// Main-world resource — the `FontSource` for the embedded Roboto Regular
/// font. Both `hud` and `panel` query this resource to set `TextFont.font`.
///
/// To add a second font in future: add another `&[u8]` static + another field
/// here, load it in `load_dev_font`, and store its `FontSource` alongside this one.
#[derive(Resource)]
pub struct DevFont(pub FontSource);

use bevy::{
    asset::AssetPlugin,
    camera_controller::free_camera::FreeCameraPlugin,
    diagnostic::FrameTimeDiagnosticsPlugin,
    prelude::*,
    render::{diagnostic::RenderDiagnosticsPlugin, RenderPlugin},
    window::WindowResolution,
};

#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy::anti_alias::dlss::DlssProjectId;

/// Which hard-coded Phase-A test grid `voxel::grid::setup_test_grid` builds (D2).
///
/// Track A (`docs/orchestrate/feature-completeness/02a-design-vox-loading.md`)
/// added the [`GridPreset::Vox`] variant — a MagicaVoxel `.vox` file path read
/// synchronously at `Startup` via [`voxel::vox_import::load_vox`]. `PathBuf` is
/// not `Copy`, so this enum is now `Clone` only (the
/// [`AppArgs`] / [`build_app_with_args`] surfaces propagate the move).
///
/// **vox-gpu-rewrite Stage 2 consolidation (2026-05-18):** both variants now
/// always route through the C#-faithful fixed-world install path
/// (`install_default_embedded_in_fixed_world` / `install_vox_in_fixed_world`).
/// The old `tiles` field on `Vox` (driving CPU XZ-replication via
/// `install_vox_sized_to_model`) is gone — the W5 GPU producer chain handles
/// `voxelPos % modelSize` tiling on the device. The legacy sized-to-model
/// install function is preserved only as a test-only oracle reachable from
/// the `--vox-gpu-oracle` CPU-phase branch.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub enum GridPreset {
    /// The default scene: ground slab + axis-aligned boxes + a sphere + one
    /// emissive box.
    #[default]
    Default,
    /// Load a MagicaVoxel `.vox` file from disk (path relative to repo root or
    /// absolute). The file is read once at `Startup`; failure logs an error
    /// and falls back to [`GridPreset::Default`] so the e2e harness still has
    /// a renderable world. See `voxel/vox_import.rs`.
    Vox {
        path: std::path::PathBuf,
    },
    /// Streaming-world preset (`docs/orchestrate/streaming-world`). The world
    /// is procedurally generated on-the-fly by the residency manager driving
    /// the WGSL FastNoiseLite noise → `segment_voxel_buffer` GPU dispatch
    /// chain. `noise_preset` selects one of the built-in WGSL presets (Phase 2
    /// ships exactly one: `0 = SimpleTerrain`); `seed` is the FNL hash seed.
    ProceduralStreaming {
        /// Index into the built-in WGSL noise preset table. Phase 2 supports
        /// `0 = SimpleTerrain` only.
        noise_preset: u32,
        /// FNL seed (`FnlState::seed`).
        seed: i32,
    },
    /// Procedural-static viability preset (streaming-world Phase 2.4). Mirrors
    /// `ProceduralStreaming` BUT generates the **entire** fixed 512-segment
    /// world via the WGSL noise → segment_voxel_buffer → chunk_calc chain in
    /// a single one-shot dispatch at startup (W5-shape loop, like `Default`,
    /// but with `noise_terrain.wgsl` as the per-segment producer). No
    /// residency manager. No per-frame admission queue. No sliding window.
    /// Used by `--noise-static-world` to prove the noise→encoded-chunks→
    /// visible-render chain works end-to-end, independent of the residency
    /// machinery. See
    /// `docs/orchestrate/streaming-world/03d-impl-static-noise.md`.
    ProceduralStatic {
        /// Index into the built-in WGSL noise preset table. Phase 2.4 supports
        /// `0 = SimpleTerrain` only.
        noise_preset: u32,
        /// FNL seed (`FnlState::seed`).
        seed: i32,
    },
}

/// The Phase-B GI pipeline settings (`09-design-b.md` §3.8). The C#
/// `WorldRenderBase` ImGui sliders (`SettingDataRenderBase`) become these
/// `AppArgs` constants — there is no GI settings GUI in the port (§1). The
/// values are the C# slider *defaults*.
#[derive(Clone, Copy, Debug)]
pub struct GiSettings {
    /// Max secondary-ray bounce count (C# `bounceCount`).
    pub bounce_count: u32,
    /// GI accumulation-ring depth (C# `globalIllumMaxAccum`).
    pub global_illum_max_accum: u32,
    /// Spatial-resampling neighbour-search size (C# `spatialResampleSize`).
    pub spatial_resample_size: f32,
    /// Spatial-resampling visibility ray-step count (C# `spatialVisibilityCount`).
    pub spatial_visibility_count: u32,
    /// Denoiser threshold (C# `denoiseThresh`).
    pub denoise_thresh: f32,
    /// Lit-radius factor (C# `radiusLitFactor`).
    pub radius_lit_factor: f32,
    /// Noise-suppression factor (C# `noiseSuppressionFactor`).
    pub noise_suppression_factor: f32,
    /// The 1↔0.25-spp toggle (C# `skipSamples`) — drives `rayQueueCalc`.
    pub skip_samples: bool,
    /// Run the sparse bilateral denoiser (C# `isDenoise`).
    pub is_denoise: bool,
    /// Brightness-level the bucket samples (C# `isSampleLeveling`).
    pub is_sample_leveling: bool,
    /// Vary the spatial-resampling radius per pixel (C# `isVaryingResmaplingRadius`).
    pub is_varying_resampling_radius: bool,
    /// Apply the in-volume atmosphere interaction (C# `isAtmosphereInteraction`).
    pub is_atmosphere_interaction: bool,
    /// Per-pixel sun-shadow tap count for the spatial-resampling sun sample
    /// (`crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:529-560`).
    /// Multi-tap extension addressing the paper §5.2 limitation: *"soft shadows
    /// from the sun are not handled during resampling, resulting in slightly
    /// increased noise."* Default **4** — N=1 reproduces the C# single-tap path
    /// bit-equivalently (modulo loop-induced rand-stream advancement). The
    /// shader clamps to `max(_, 1)`, so writing 0 here is harmless (resolves
    /// to a single tap, matching the C# baseline). No CLI flag — config-struct
    /// knob only (Dispatch A scope; see
    /// `docs/orchestrate/naadf-bevy-port/19-gi-reservoir-scope.md` §3.1).
    pub sun_shadow_taps: u32,
    // === Quality-panel runtime knobs (`21-design-quality-panel.md` §2.1) ====
    // The 5 ray-step caps + spatial iter count promoted from WGSL `const`s
    // to runtime uniform fields, so the in-app quality panel can dial them
    // live without rebuilds. All defaults match the C#/paper canonical values
    // bit-for-bit — panel-disabled (or default-loaded) behaviour is identical
    // to pre-dispatch. The WGSL consumers clamp `max(_, 1u)` defensively;
    // zero is safe.
    /// Max DDA step count for the primary G-buffer ray
    /// (`naadf_first_hit.wgsl::shoot_ray` arg, was const
    /// `MAX_RAY_STEPS_PRIMARY = 120`). Uploaded into
    /// `GpuRenderParams.max_ray_steps_primary` (offset 24, repurposed `_pad0a`
    /// slot — layout-preserving).
    pub max_ray_steps_primary: u32,
    /// Max DDA step count for GI secondary bounce rays
    /// (`naadf_global_illum.wgsl::shoot_ray`, was const
    /// `MAX_RAY_STEPS_SECONDARY = 100`). Uploaded into
    /// `GpuGiParams.max_ray_steps_secondary`.
    pub max_ray_steps_secondary: u32,
    /// Max DDA step count for the spatial-resampling sun-visibility ray
    /// (`spatial_resampling.wgsl::shoot_ray`, was const
    /// `MAX_RAY_STEPS_SUN = 120`). Uploaded into `GpuGiParams.max_ray_steps_sun`.
    pub max_ray_steps_sun: u32,
    /// Max DDA step count for the per-bounce sun-shadow ray inside
    /// `globalIllum` (`naadf_global_illum.wgsl::shoot_ray` sun-secondary call,
    /// was const `MAX_RAY_STEPS_SUN_SECONDARY = 80`). Uploaded into
    /// `GpuGiParams.max_ray_steps_sun_secondary`.
    pub max_ray_steps_sun_secondary: u32,
    /// Max DDA step count for the spatial-resampling reservoir-visibility ray
    /// (`spatial_resampling.wgsl::shoot_ray` visibility-loop, was const
    /// `MAX_RAY_STEPS_VISIBILITY = 60`). Note the 3-iteration outer mirror
    /// loop multiplies this cost up to 3×. Uploaded into
    /// `GpuGiParams.max_ray_steps_visibility`.
    pub max_ray_steps_visibility: u32,
    /// Algorithm-2 spatial-resampling iteration count
    /// (`spatial_resampling.wgsl::sample_neighbors` `sample_count` arg, was
    /// hardcoded `12u`). Paper §4.2 + C# `renderSpatialResampling.fx:359`
    /// default = 12. Variance ∝ 1/√N — bump to 16/24 trades cost for less
    /// indirect-bounce noise (`19-gi-reservoir-scope.md` §3.3). Uploaded into
    /// `GpuGiParams.spatial_iter_count`.
    pub spatial_iter_count: u32,
}

impl Default for GiSettings {
    fn default() -> Self {
        // The `SettingDataRenderBase` defaults (`WorldRenderBase.cs:14-25`).
        Self {
            bounce_count: 3,
            global_illum_max_accum: 128,
            spatial_resample_size: 500.0,
            spatial_visibility_count: 80,
            denoise_thresh: 400.0,
            radius_lit_factor: 3.0,
            noise_suppression_factor: 0.4,
            skip_samples: true,
            is_denoise: true,
            is_sample_leveling: true,
            is_varying_resampling_radius: true,
            is_atmosphere_interaction: true,
            // Sun-shadow tap count — C# default 1 (no loop;
            // `renderSpatialResampling.fx:322-339` is a single
            // `getUniformHemisphereSample` + single `shootRay`). The
            // Phase-D-shadow Dispatch A (`1c35c7f`, 2026-05-15) shipped
            // N=4 as the paper-§5.2 soft-shadow mitigation; per
            // `docs/orchestrate/feature-completeness/02d-render-perf-
            // investigation.md` §1 + user directive 2026-05-15, the
            // default is reverted to the C# canonical 1 — the multi-tap
            // path stays available via the quality panel's
            // `sun_shadow_taps` knob (range 1..32) for users who want
            // softer penumbras at the perf cost. The shader's
            // `max(_, 1u)` clamp at `spatial_resampling.wgsl:547`
            // handles N=1 safely (bit-equivalent to pre-Dispatch-A path
            // per `20-impl-phase-d-shadow-A.md` §4).
            sun_shadow_taps: 1,
            // Quality-panel runtime knobs — defaults bit-equivalent to the
            // pre-dispatch WGSL `const`s these promotions replaced (the
            // `MAX_RAY_STEPS_*` consts at `ray_tracing.wgsl:122-126` and the
            // `12u` literal at `spatial_resampling.wgsl:622`). Verified by the
            // §6 defaults table of `21-design-quality-panel.md`.
            max_ray_steps_primary: 120,
            max_ray_steps_secondary: 100,
            max_ray_steps_sun: 120,
            max_ray_steps_sun_secondary: 80,
            max_ray_steps_visibility: 60,
            spatial_iter_count: 12,
        }
    }
}

/// C# `WorldHandler.worldSizeToUseInWorldGenSegments` (`WorldHandler.cs:19`).
///
/// NAADF's fixed startup world size, expressed in **WorldGenSegment** units.
/// One segment is `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 * 16` voxels per axis
/// (4 chunks per group × 16 voxels per chunk = 64 voxels per group × 4 groups
/// per segment = 256 voxels per segment). C# uses `(16, 2, 16)`, which gives
/// the canonical `(4096, 512, 4096)`-voxel world the original engine boots
/// into regardless of whether a model file is present.
pub const WORLD_SIZE_IN_SEGMENTS: UVec3 = UVec3::new(16, 2, 16);

/// C# `WorldHandler.worldGenSegmentSizeInGroups` (`WorldHandler.cs:18`). One
/// group is `4^3` chunks (= `64^3` voxels); this many groups per segment per
/// axis. Combined with [`WORLD_SIZE_IN_SEGMENTS`] this pins the fixed world
/// dimensions.
pub const WORLD_GEN_SEGMENT_SIZE_IN_GROUPS: u32 = 4;

/// Derived: world size in chunks (`16 * 4 * 4 = 256`, `2 * 4 * 4 = 32`).
///
/// The same number `WorldData.cs:64-65` arrives at:
/// `sizeInVoxels = sizeInWorldGenSegments * worldGenSegmentSizeInVoxels`, then
/// `sizeInChunks = sizeInVoxels / 16`.
///
/// Hardcoded rather than computed because `glam`'s `UVec3` ops are not `const`;
/// the relationship is enforced by [`tests::fixed_world_size_constants_agree`].
pub const WORLD_SIZE_IN_CHUNKS: UVec3 = UVec3::new(256, 32, 256);

/// Derived: world size in voxels (`256 * 16 = 4096`, `32 * 16 = 512`).
pub const WORLD_SIZE_IN_VOXELS: UVec3 = UVec3::new(4096, 512, 4096);

/// The default TAA sample-ring depth — **32**, NAADF's / the paper's depth
/// (`WorldRenderBase.cs:17`, paper §4.1 / Fig 6).
///
/// `18-taa-fidelity.md` fix #3 made the ring depth a configurable
/// `AppArgs.taa_ring_depth`, superseding the `01-context.md` §2c / §6 binding
/// 16-deep VRAM lever (the 16-deep ring was a secondary cause of the port's
/// "barely resolves" noise — it halves the temporal-averaging window). 16 / 24
/// stay available via the config knob; **32 is the default**. This single
/// const is the source of truth for both the WGSL `#{TAA_SAMPLE_RING_DEPTH}`
/// shader-def (`render/pipelines.rs`) and the Rust buffer sizing
/// (`render/taa.rs`) — the two MUST agree exactly (a mismatch is silent ring
/// corruption), so they both read it from here, via `AppArgs.taa_ring_depth`.
pub const DEFAULT_TAA_RING_DEPTH: u32 = 32;

/// Command-line options, parsed once and stored as a resource (`03-design.md` §4.1).
///
/// Track A (`docs/orchestrate/feature-completeness/02a-design-vox-loading.md`
/// — Assumption #5) dropped `Copy` because [`GridPreset::Vox`] carries a
/// `PathBuf`. Every internal use is by-ref (`Res<AppArgs>` / `&AppArgs`); the
/// only by-value site is [`build_app_with_args`], where a single move
/// suffices.
#[derive(Resource, Clone)]
pub struct AppArgs {
    /// Which hard-coded test grid to build (D2).
    pub grid_preset: GridPreset,
    /// Long-term TAA. Wired but always `false` in Phase A (D4) — Phase A-2
    /// turns it on.
    pub taa: bool,
    /// The TAA sample-ring depth — the long-term-memory TAA history depth
    /// (`18-taa-fidelity.md` fix #3). The single config source of truth: it
    /// feeds BOTH the Rust buffer sizing (`render/taa.rs` — `taa_samples` is
    /// `pixel_count * taa_ring_depth`) AND the WGSL `#{TAA_SAMPLE_RING_DEPTH}`
    /// shader-def injected at pipeline specialisation (`render/pipelines.rs`),
    /// so the loop bounds / `% N` indexing in `taa.wgsl` agree byte-for-byte
    /// with the buffer size. Default [`DEFAULT_TAA_RING_DEPTH`] (32); 16 / 24
    /// are the VRAM-lever alternatives. Read on the render side via the
    /// `TaaRingConfig` render-world resource (`render::taa`).
    pub taa_ring_depth: u32,
    /// The Phase-B GI pipeline settings (`09-design-b.md` §3.8).
    pub gi: GiSettings,
    /// The Phase-C GPU-construction configuration (`15-design-c.md` §1.8,
    /// §2.1 W0 row). Same plumbing pattern as `taa_ring_depth`: this main-
    /// world field is the source of truth; `render::construction::
    /// ConstructionPlugin::build` mirrors it into the render sub-app as the
    /// `ConstructionConfig` `Resource` (via `From<&AppArgs>`).
    ///
    /// W0 default: GPU construction off / CPU fallback on. W1 flips
    /// `gpu_construction_enabled` after the bit-exact CPU/GPU oracle is
    /// green; W4 may flip `entities_enabled`. The CLI flags that mutate
    /// individual fields land per-workstream — W0 only exposes the struct.
    pub construction_config: render::construction::ConstructionConfig,
    /// Phase-C wave-3 — when `true`, [`build_app`] adds a `Startup` system
    /// that spawns one fixture entity into [`render::construction::MainWorldEntities`]
    /// (a 4×4×4 emissive-voxel block at the world centre). Combined with
    /// `construction_config.entities_enabled = true`, this is the load-bearing
    /// `--entities` mode of `e2e_render`: the entity is uploaded each frame
    /// + rendered via `ray_tracing.wgsl::shoot_ray`'s entity sub-traversal
    /// branch, surfacing in the framebuffer as an extra hit on top of the
    /// world geometry.
    pub spawn_test_entity: bool,
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
    /// streaming-world Phase 2 — runs the `--streaming-window` e2e gate. When
    /// `true`, the e2e harness boots a `GridPreset::ProceduralStreaming` world
    /// at the configured `vram_budget_mib` + `max_segments_per_frame` budget,
    /// walks the camera across ≥2 segment boundaries in X, captures
    /// framebuffers before/after, and asserts the residency window followed
    /// the camera + terrain re-populated. See
    /// `docs/orchestrate/streaming-world/02b-design-plan-b.md` § J +
    /// `crate::e2e::streaming_window`.
    pub streaming_window_mode: bool,
    /// streaming-world Phase 2.11
    /// (`docs/orchestrate/streaming-world/03n-diagnosis-aadf-building.md`
    /// punch-list item 4) — runs the `--gate streaming-aadf-parity` e2e gate.
    /// Wraps the standard streaming-window flow + adds a post-walk
    /// `chunks_buffer` GPU-readback that asserts the W3 chunk-level 5-bit
    /// AADFs are self-consistent: for every empty chunk c, the AADF skip
    /// distance in each of 6 directions must not exceed the actual distance
    /// to the nearest non-empty chunk in that direction. Catches the Phase
    /// 2.11 bug class (W3 chain baking long stale skips through yet-to-be-
    /// admitted zero-chunks).
    pub streaming_aadf_parity_mode: bool,
    /// streaming-world Phase 2.12
    /// (`docs/orchestrate/streaming-world/02e-design-phase-2-12.md` § A,
    /// MUST-3) — `true` when the e2e harness is running the **static
    /// subprocess** of the `--gate streaming-framebuffer-diff` compare. The
    /// driver routes through a single-shot Warmup → Shoot → Drain flow, the
    /// camera-pin system pins to the shared framebuffer-diff pose, and the
    /// drain phase saves `framebuffer_static.png`. Mutually exclusive with
    /// `streaming_framebuffer_streaming_phase`.
    pub streaming_framebuffer_static_phase: bool,
    /// streaming-world Phase 2.12 — `true` when the e2e harness is running
    /// the **streaming subprocess** of `--gate streaming-framebuffer-diff`.
    /// Same shape as `streaming_framebuffer_static_phase` but installs
    /// `ProceduralStreaming` and runs an extended cold-start drain (~256
    /// ticks at 4 admissions/frame so all 512 segments admit before the
    /// screenshot). Saves `framebuffer_streaming.png`.
    pub streaming_framebuffer_streaming_phase: bool,
    /// streaming-world Phase 2.4 — runs the `--noise-static-world` e2e gate.
    /// When `true`, the e2e harness boots a `GridPreset::ProceduralStatic`
    /// world (the full 512-segment fixed-world container, generated once at
    /// startup via the noise → segment_voxel_buffer → chunk_calc → bounds
    /// chain), captures a framebuffer post-warmup, and asserts strict
    /// luminance variance + non-sky-pixel-ratio floors that fail on
    /// sky-only output. Proves the noise→encoded-chunks→render chain works
    /// independent of the residency / sliding-window machinery. See
    /// `docs/orchestrate/streaming-world/03d-impl-static-noise.md` +
    /// `crate::e2e::noise_static_world`.
    pub noise_static_mode: bool,
    /// streaming-world Phase 2 — VRAM budget (in MiB) for the residency slab.
    /// Default `1024`. Asserted at startup install time against the slab's
    /// computed total per `02-design.md` § A.4; panic on under-budget.
    pub vram_budget_mib: u32,
    /// streaming-world Phase 2 — per-frame admission cap for the residency
    /// driver. Default `4` per `02b-design-plan-b.md` § D.B6.
    pub max_segments_per_frame: u32,
    /// streaming-world Phase 2 — `world_y` value at which `noise == 0` flips
    /// solid/empty. Default = half world height (`WORLD_SIZE_IN_VOXELS.y / 2 =
    /// 256`).
    pub sea_level: f32,
    /// streaming-world Phase 2 — height span over which the noise transition
    /// spreads. Default `64.0` (architect-picked; justified in
    /// `03b-impl-residency.md` § CLI defaults justified).
    pub terrain_amplitude: f32,
    /// streaming-world Phase 2 — FNL seed for the streaming preset (default
    /// `1337`).
    pub noise_seed: i32,
    /// streaming-world Phase 2 — WGSL noise preset index (flat CLI feed for
    /// the `GridPreset::Procedural*` variant payloads). Default `0 =
    /// SimpleTerrain`. [`crate::cli::Cli::into_app_args`] copies this into
    /// the matching `GridPreset` variant's `noise_preset` field at parse
    /// time so the install path
    /// (`crate::voxel::grid::install_procedural_streaming_world`) reads it
    /// off the variant; the flat field exists so a single `--noise-preset
    /// N` CLI flag drives both presets uniformly.
    pub noise_preset: u32,
}

impl Default for AppArgs {
    fn default() -> Self {
        Self {
            grid_preset: GridPreset::default(),
            taa: true,
            taa_ring_depth: DEFAULT_TAA_RING_DEPTH,
            gi: GiSettings::default(),
            construction_config: render::construction::ConstructionConfig::default(),
            spawn_test_entity: false,
            resize_test: false,
            vox_e2e_mode: false,
            oasis_edit_visual_mode: false,
            small_edit_visual_mode: false,
            small_edit_repro_mode: false,
            vox_gpu_construction_mode: false,
            vox_gpu_oracle_cpu_phase: false,
            vox_gpu_oracle_gpu_phase: false,
            streaming_window_mode: false,
            streaming_aadf_parity_mode: false,
            streaming_framebuffer_static_phase: false,
            streaming_framebuffer_streaming_phase: false,
            noise_static_mode: false,
            vram_budget_mib: 1024,
            max_segments_per_frame: 4,
            sea_level: (WORLD_SIZE_IN_VOXELS.y as f32) * 0.5,
            terrain_amplitude: 64.0,
            noise_seed: 1337,
            noise_preset: 0,
        }
    }
}

/// Window sizing/title knobs that `build_app` threads into the `WindowPlugin`
/// (`e2e-render-test.md` §9). The production config takes the platform
/// default; the e2e config pins a small fixed non-resizable window so the
/// framebuffer readback is fast and every `pixel_count`-sized buffer is
/// identical run-to-run (§4.2 determinism row).
#[derive(Clone, Copy, Debug)]
pub struct WindowConfig {
    /// Logical resolution. `None` → the Bevy default (`Window::default`).
    pub resolution: Option<(f32, f32)>,
    /// Whether the window is user-resizable.
    pub resizable: bool,
    /// Window title.
    pub title: &'static str,
    /// Wayland `app_id` / X11 `WM_CLASS` (Bevy `Window.name`). `None` lets
    /// winit pick a default (usually the binary name). The resize-test config
    /// sets this explicitly so the hyprctl `class:...` selector is
    /// deterministic.
    pub name: Option<&'static str>,
}

impl WindowConfig {
    /// The production window — platform default size, resizable.
    fn windowed() -> Self {
        Self {
            resolution: None,
            resizable: true,
            title: "bevy-naadf",
            name: None,
        }
    }

    /// The e2e window — a small fixed 256×256 non-resizable window
    /// (`e2e-render-test.md` §4.2 / §9). 256² is large enough for stable
    /// region gates, small enough for a fast readback + cheap GI dispatch.
    fn e2e() -> Self {
        Self {
            resolution: Some((
                crate::e2e::E2E_WIDTH as f32,
                crate::e2e::E2E_HEIGHT as f32,
            )),
            // Production e2e config — non-resizable for determinism (every
            // `pixel_count`-sized buffer identical run-to-run). The
            // resize-blackness reproduction test forks into
            // [`WindowConfig::e2e_resize_test`] (resizable: true) instead.
            resizable: false,
            title: "bevy-naadf e2e_render",
            name: None,
        }
    }

    /// The e2e window for the resize-blackness reproduction test
    /// (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
    /// `## GI-bounce-on-resize fix (2026-05-16)`).
    ///
    /// Same 256×256 starting size as [`WindowConfig::e2e`] but with
    /// `resizable: true` — must be true for hyprctl-driven resize to
    /// propagate through winit; resize-test mode only.
    ///
    /// Without this flag the Hyprland compositor refuses pixel-precise resize
    /// requests on the surface (winit advertises the surface as fixed-size to
    /// the compositor when `resizable: false`). The standard e2e harness
    /// continues to use [`WindowConfig::e2e`] — only the `--resize-test`
    /// branch picks this up.
    fn e2e_resize_test() -> Self {
        Self {
            // User spec for the three-step resize test (boot → 1920×1080 →
            // 2000×1000): the *initial* screenshot is taken at 800×600, so
            // the window boots at exactly that size. Larger than the
            // standard e2e 256×256 because the user wants visual coverage of
            // shadow regions across resolution changes.
            resolution: Some((
                crate::e2e::E2E_RESIZE_BOOT_WIDTH as f32,
                crate::e2e::E2E_RESIZE_BOOT_HEIGHT as f32,
            )),
            // test-only: must be true for hyprctl-driven resize to propagate
            // through winit; resize-test mode only.
            resizable: true,
            title: "bevy-naadf e2e_render",
            // test-only: pin Wayland app_id to "e2e_render" so the hyprctl
            // `class:e2e_render` selector matches deterministically. Without
            // this, winit picks a default app_id that varies by build and
            // the hyprctl dispatcher prints "resizeWindow: no window".
            name: Some("e2e_render"),
        }
    }
}

/// The four deliberate, minimal ways the e2e app differs from the production
/// app (`e2e-render-test.md` §2.2 / §9). Everything else — `DefaultPlugins`,
/// `WinitPlugin`, the real window, the asset path, `WorldPlugin`,
/// `NaadfRenderPlugin`, the diagnostics plugins — is *identical*, so the e2e
/// run exercises the real boot path, not a near-copy of it.
#[derive(Clone, Copy, Debug)]
pub struct AppConfig {
    /// Add the diagnostics HUD overlay (`setup_hud` / `update_hud`).
    pub add_hud: bool,
    /// Add `FreeCameraPlugin` + the runtime DLSS toggle (the fly camera).
    pub add_free_camera: bool,
    /// `RenderPlugin { synchronous_pipeline_compilation, .. }` — the e2e config
    /// flips this on so `PipelineCache` resolves every queued pipeline to
    /// `Ok`/`Err` within the same `app.update()`, making the bounded-frame run
    /// deterministic (`e2e-render-test.md` §2.2 point 1).
    pub synchronous_pipeline_compilation: bool,
    /// Window sizing/title.
    pub window: WindowConfig,
    /// Add the e2e bounded-frame driver + readback + assertion systems + the
    /// `WinitSettings::game()`-style `Continuous` update mode + the fixed-pose
    /// camera (`e2e-render-test.md` §4 / §6 / §2.2 point 2).
    pub add_e2e_systems: bool,
}

impl AppConfig {
    /// The production config: HUD on, free camera on, async pipeline
    /// compilation (no startup hitch), platform-default window, no e2e systems.
    pub fn windowed() -> Self {
        Self {
            add_hud: true,
            add_free_camera: true,
            synchronous_pipeline_compilation: false,
            window: WindowConfig::windowed(),
            add_e2e_systems: false,
        }
    }

    /// The e2e config: HUD off, free camera off, *synchronous* pipeline
    /// compilation, a 256×256 non-resizable window, e2e systems on
    /// (`e2e-render-test.md` §2.2 / §9).
    pub fn e2e() -> Self {
        Self {
            add_hud: false,
            add_free_camera: false,
            synchronous_pipeline_compilation: true,
            window: WindowConfig::e2e(),
            add_e2e_systems: true,
        }
    }
}

/// Build the bevy-naadf `App` from an [`AppConfig`].
///
/// This is the single shared app-wiring path — `main.rs` calls it with
/// [`AppConfig::windowed`], the e2e binary with [`AppConfig::e2e`]. The plugin
/// set is the real `DefaultPlugins` (incl. `WinitPlugin` — a real on-screen
/// window) in *both* configs; `AppConfig` only flips the four deliberate e2e
/// deltas (`e2e-render-test.md` §2.2). Caller runs `.run()` on the result.
pub fn build_app(cfg: AppConfig) -> App {
    build_app_with_args(cfg, AppArgs::default())
}

/// Build the bevy-naadf `App` with a caller-supplied [`AppArgs`].
///
/// Phase-C wave-3 — added to let the e2e binary toggle `--entities`-driven
/// state (`entities_enabled = true` + `spawn_test_entity = true`) without
/// having to mutate the global `AppArgs::default()`. Callers that don't need
/// to override args use [`build_app`] (which forwards to this with the default).
pub fn build_app_with_args(cfg: AppConfig, args: AppArgs) -> App {

    // streaming-world Phase 2.10
    // (`docs/orchestrate/streaming-world/03l-diagnosis-hitch-and-view-distance.md`
    // punch-list item 2 + item 6): the canonical NAADF primary ray-step cap is
    // 120 (tuned for full AADF acceleration). On the streaming preset there is
    // a narrow window during admission settling where freshly-admitted segments
    // may carry partially-stale AADF; the per-segment bounds dispatch (item 1)
    // covers voxel + block AADF current-frame, but the W3 regime-2 background
    // queue refines chunk-level 5-bit AADFs incrementally over many frames.
    // Bump the cap to 240 on streaming only, as a safety belt during the
    // multi-frame W3 settling. Other presets stay at 120 — the diagnostic
    // explicitly warned against masking the real fix by raising the global
    // cap.
    let args = {
        let mut a = args;
        if matches!(a.grid_preset, GridPreset::ProceduralStreaming { .. })
            && a.gi.max_ray_steps_primary < 240
        {
            a.gi.max_ray_steps_primary = 240;
        }
        a
    };

    let mut app = App::new();

    // `DlssProjectId` must be inserted before `DefaultPlugins` so the render
    // sub-app sees it during DLSS initialisation. DLSS plumbing stays available
    // (Phase-B-relevant) but is dormant. The e2e config does *not* insert it
    // either — `DlssProjectId` is only consulted if inserted, so the e2e run
    // simply leaves DLSS dormant the same way (`e2e-render-test.md` §2.2).
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
    app.insert_resource(DlssProjectId(bevy::asset::uuid::uuid!(
        "8f6b1d2e-3c4a-4f5b-9a7c-1e2d3f4a5b6c"
    )));

    // The primary window — fixed small + non-resizable for e2e, platform
    // default for production.
    let mut primary_window = Window {
        title: cfg.window.title.to_string(),
        resizable: cfg.window.resizable,
        name: cfg.window.name.map(|s| s.to_string()),
        ..default()
    };
    if let Some((w, h)) = cfg.window.resolution {
        primary_window.resolution = WindowResolution::new(w as u32, h as u32);
    }

    // Web (wasm32 / WebGPU) build: bind the Bevy window to the
    // `<canvas id="bevy">` declared in `index.html` and track its parent's
    // size, instead of letting winit create a detached canvas the page never
    // shows. `prevent_default_event_handling` keeps browser hotkeys (F5, tab,
    // …) from firing while the app has focus. No effect on native targets.
    #[cfg(target_arch = "wasm32")]
    {
        primary_window.canvas = Some("#bevy".to_string());
        primary_window.fit_canvas_to_parent = true;
        primary_window.prevent_default_event_handling = true;
    }

    // `AppArgs` lost `Copy` in Track A (carries `PathBuf` in
    // `GridPreset::Vox`). The resource gets a clone — `args` is consumed
    // afterwards for the `spawn_test_entity` / `resize_test` reads below.
    app.insert_resource(args.clone())
        // The 128-deep camera-history ring + the monotonic frame counter
        // (`06-design-a2.md` §2.3). Main-world resource, `Default`-seeded,
        // updated each frame by `update_camera_history`.
        .init_resource::<render::taa::CameraHistory>()
        .add_plugins(
            // The NAADF WGSL render shaders live in `src/assets/shaders/`
            // (`03-design.md` §1 module layout) — point the asset server
            // there. `RenderPlugin` carries the
            // `synchronous_pipeline_compilation` flag (the e2e delta —
            // `e2e-render-test.md` §2.2 point 1); the `WindowPlugin` carries
            // the fixed-size e2e window.
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: "src/assets".to_string(),
                    // Web: Trunk's dev server has no `.meta` sidecars and
                    // answers unknown paths with a 200 HTML fallback, so Bevy's
                    // default meta probe parses that HTML as RON and fails the
                    // load of every shader. The project ships no `.meta` files
                    // anyway — skip the probe. Gated to wasm32 so the native
                    // boot path stays byte-identical.
                    #[cfg(target_arch = "wasm32")]
                    meta_check: bevy::asset::AssetMetaCheck::Never,
                    // Stays `AssetMode::Unprocessed` for the production app and
                    // the e2e harness: a Bevy `AssetProcessor` is app-global and
                    // racing it against the render pipeline's shader loads is
                    // fragile. The texture-array Basis pipeline runs out-of-band
                    // in the dedicated `bake` binary instead (`src/bin/bake.rs`,
                    // `just bake`) — see `crate::texture_array`.
                    ..default()
                })
                .set(RenderPlugin {
                    synchronous_pipeline_compilation: cfg
                        .synchronous_pipeline_compilation,
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(primary_window),
                    ..default()
                }),
        )
        .add_plugins((
            FrameTimeDiagnosticsPlugin::default(),
            RenderDiagnosticsPlugin,
            world::WorldPlugin,
            render::NaadfRenderPlugin,
            // Phase-C construction seam (`15-design-c.md` §3, §1.1). W0 lands
            // the empty `ConstructionPlugin` (empty pipeline registry, empty
            // `ConstructionGpu` / `ConstructionBindGroups` resources, the
            // empty `prepare_construction` + `run_gpu_construction_startup`
            // placeholders). W1..W5 each merge in their workstream's
            // pipelines / buffers / systems behind this plugin — without
            // re-editing `build_app`. Inserted **after** `NaadfRenderPlugin`
            // so the render sub-app exists and our `init_gpu_resource` call
            // succeeds (same ordering as `NaadfRenderPlugin`'s
            // `init_gpu_resource::<NaadfPipelines>()`).
            render::construction::ConstructionPlugin,
            // streaming-world Phase 2 — registers the residency driver +
            // the render-world extract. Unconditional registration: when no
            // `Residency` / `NoiseChunkSource` resources exist (the
            // non-streaming presets) the systems early-return.
            streaming::StreamingPlugin,
            // `material.ron` loader — registers `MaterialRonLoader` so
            // `materials/<name>/material.ron` resolves to a `StandardMaterial`.
            // Infrastructure only: nothing in the scene consumes a baked
            // material yet (wiring baked PBR into the custom voxel render path
            // is a separate future effort).
            bevy_instamat::BakedMaterialPlugin,
            // Registers the `*.texarray.ron` asset loader. The plugin also wires
            // the native Basis `AssetProcessor`, but that only activates when an
            // `AssetProcessor` resource exists — i.e. in the `bake` binary's
            // `AssetMode::Processed` app, not here. See `crate::texture_array`.
            texture_array::TextureArrayPlugin,
        ));

    // The fly camera + runtime DLSS toggle — production only. The e2e config
    // omits `FreeCameraPlugin` so even though the window is real and can
    // receive focus/input, no system moves the camera — the fixed `Transform`
    // never changes (`e2e-render-test.md` §2.2 point 4 / §4.2).
    if cfg.add_free_camera {
        app.add_plugins(FreeCameraPlugin).add_systems(
            Update,
            (camera::toggle_dlss, camera::sync_position_split),
        );
    } else {
        // No `FreeCameraPlugin`, so `sync_position_split` still needs to run
        // once (it is a pure function of the `Transform` → deterministic).
        app.add_systems(Update, camera::sync_position_split);
    }

    // Load the embedded Roboto Regular font into Assets<Font> and store the
    // handle as DevFont. Runs first so setup_hud / setup_panel can query it.
    app.add_systems(Startup, load_dev_font);

    // The test grid + camera spawn — shared. The e2e config spawns a fixed-pose
    // camera instead of the production `setup_camera`; the e2e systems own that
    // (`crate::e2e::add_e2e_systems`).
    app.add_systems(Startup, voxel::grid::setup_test_grid);

    // Phase-C wave-3 — spawn the W4 fixture entity (gated on
    // `args.spawn_test_entity`). Runs after `setup_test_grid` so the world
    // dimensions are known; populates `MainWorldEntities` with one entity at
    // the test grid centre. Per-frame `extract_world_changes` then runs the
    // `EntityHandler` + uploads the result into `ConstructionEvents`; the
    // wave-3 dispatch chain (`naadf_entity_update_node` + the
    // `ray_tracing.wgsl::shoot_ray` entity sub-traversal) folds it into the
    // framebuffer.
    if args.spawn_test_entity {
        app.add_systems(Startup, spawn_phase_c_test_entity.after(voxel::grid::setup_test_grid));
    }
    if cfg.add_e2e_systems {
        e2e::add_e2e_systems(&mut app);
    } else {
        // `.after(setup_test_grid)` so the `GridPreset::Vox` arm has had a
        // chance to insert `InitialCameraPose` (the world-sized C#-faithful
        // camera pose, `crate::camera::InitialCameraPose`); `setup_camera`
        // then frames the loaded world instead of falling back to the
        // hard-coded test-grid pose. The e2e harness uses its own
        // `setup_e2e_camera` and ignores the resource entirely.
        app.add_systems(
            Startup,
            camera::setup_camera.after(voxel::grid::setup_test_grid),
        );
    }

    // The camera-history ring update must run *after* `sync_position_split` so
    // the ring stores this frame's current camera state (`06-design-a2.md`
    // §9.3).
    app.add_systems(
        Update,
        render::taa::update_camera_history.after(camera::sync_position_split),
    );

    if cfg.add_hud {
        app.add_systems(Startup, hud::setup_hud.after(load_dev_font))
            .add_systems(Update, hud::update_hud);
        // Quality panel (`21-design-quality-panel.md` + mouse extension
        // `25-design-panel-mouse.md`) — gated on the same `add_hud` flag as
        // the HUD itself. The e2e harness (`AppConfig::e2e`) sets
        // `add_hud = false`, so the panel never spawns in the bounded harness
        // — luminance gates are unaffected. The mouse system slots in
        // between `adjust_panel` and `update_panel_text` so per-frame mouse
        // mutations are reflected in the same frame's text refresh
        // (`25-design-panel-mouse.md` §6.1).
        app.init_resource::<panel::PanelState>()
            .init_resource::<panel::PanelDrag>()
            // Track-B editor — `02b-design-editor.md`. Shares the same
            // `add_hud` gate as panel + HUD so the e2e harness never sees
            // the editor either (luminance gates unaffected).
            .init_resource::<editor::EditorState>()
            .add_systems(Startup, panel::setup_panel.after(load_dev_font))
            .add_systems(Startup, editor::hud::setup_editor_hud.after(load_dev_font))
            .add_systems(
                Update,
                (
                    panel::toggle_panel,
                    panel::adjust_panel,
                    panel::mouse_interact_panel,
                    panel::update_panel_text,
                    // apply_edit_tool runs AFTER mouse_interact_panel so the
                    // panel-press bail-out reads up-to-date Interaction state
                    // (a panel row LMB-click owns the click).
                    editor::apply_edit_tool,
                    editor::hud::update_editor_hud,
                )
                    .chain(),
            );
    }

    app
}

/// Boot the bounded windowed e2e render test and return its `AppExit`.
///
/// `cargo run --bin e2e_render` calls this. It builds the real app with
/// [`AppConfig::e2e`], runs it (the winit runner drives the loop; the
/// bounded-frame driver self-terminates after a fixed frame budget — see
/// [`crate::e2e::driver`]), then runs the post-run `PipelineCache` error scan +
/// node-dispatch check + degenerate-frame floor and folds any failure into the
/// returned `AppExit` (`e2e-render-test.md` §3 / §7 / §8 / §11 step 7).
pub fn run_e2e_render() -> AppExit {
    e2e::run_e2e_render()
}

/// Phase-C wave-3 — boot the windowed e2e with caller-supplied [`AppArgs`].
///
/// Mirrors [`run_e2e_render`] but lets the `--entities` flag in `e2e_render`'s
/// `main` toggle `entities_enabled = true` + `spawn_test_entity = true` for
/// the fixture-entity render path.
pub fn run_e2e_render_with_args(args: AppArgs) -> AppExit {
    // resize-blackness reproduction: swap in the resize-test window
    // config so the surface is advertised as resizable to the compositor —
    // a hard prerequisite for hyprctl-driven resize to propagate through
    // winit. All non-`--resize-test` runs keep `AppConfig::e2e()` unchanged.
    let mut cfg = AppConfig::e2e();
    if args.resize_test {
        cfg.window = WindowConfig::e2e_resize_test();
    }
    // `--small-edit-repro` runs at the user's screen size (1920×1080) so the
    // bug-or-fix signal matches what the user observes in the live binary.
    // The pitch-black-pixel assertion is resolution-independent in principle,
    // but the user's report specifies this size; reproduce verbatim.
    if args.small_edit_repro_mode {
        cfg.window = WindowConfig {
            resolution: Some((
                crate::e2e::small_edit_repro::SMALL_EDIT_REPRO_WIDTH as f32,
                crate::e2e::small_edit_repro::SMALL_EDIT_REPRO_HEIGHT as f32,
            )),
            resizable: false,
            title: "bevy-naadf e2e_render small-edit-repro",
            name: None,
        };
    }
    let app = build_app_with_args(cfg, args);
    e2e::run_with_app(app)
}

/// `Startup` system: load the embedded Roboto Regular bytes into `Assets<Font>`
/// and insert the resulting `Handle<Font>` as the [`DevFont`] resource.
///
/// Must run before `setup_hud` and `setup_panel` so those systems can resolve
/// the resource. Runs unconditionally in both windowed and e2e configs.
fn load_dev_font(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let font = Font::from_bytes(ROBOTO_REGULAR_BYTES.to_vec(), "Roboto");
    let handle = fonts.add(font);
    commands.insert_resource(DevFont(FontSource::Handle(handle)));
}

/// Phase-C wave-3 — startup system that spawns one W4 fixture entity into
/// the main-world [`render::construction::MainWorldEntities`] resource.
///
/// Gated on `AppArgs::spawn_test_entity = true` at `build_app_with_args` time
/// — `e2e_render --entities` sets the flag.
///
/// Fixture: a 4×4×4-voxel green-emissive block at the (sky-visible) world
/// position that the e2e camera frames in front of the look target — the
/// camera at `(86, 42, 90)` looking at `(32, 16, 32)` sees this entity high
/// + central in the framebuffer. All voxels are voxel-type 11 (green
/// emissive, `voxel/grid.rs:192-199`). The entity is at identity rotation;
/// one entity instance, `entity = 0`, `voxel_start = 0` (the first 64 u32s
/// of `entity_voxel_data`). The entity sits ~3 voxels above the existing
/// scene's tallest emissive block so the screen position is distinct.
fn spawn_phase_c_test_entity(
    mut entities: ResMut<render::construction::MainWorldEntities>,
) {
    use crate::aadf::entity::EntityData;
    use crate::render::gpu_types::EntityInstance;

    // 4×4×4 green-emissive entity, every voxel type = 11.
    let size = [4u32, 4, 4];
    let voxel_count = (size[0] * size[1] * size[2]) as usize;
    let types: Vec<u32> = vec![11u32; voxel_count];
    let data = EntityData::from_types(size, &types);

    // Pad to 64 u32s (NAADF `EntityHandler.cs:325-329` indexes
    // `voxelStart * 64 + voxelIndex`, and a 4×4×4 entity uses 64 voxels).
    let mut voxel_data = data.voxels.clone();
    while voxel_data.len() < 64 {
        voxel_data.push(0);
    }
    entities.voxel_data = voxel_data;
    entities.voxel_data_generation = entities.voxel_data_generation.wrapping_add(1);

    // Place at (30, 24, 30) RELATIVE TO THE SMALL DEFAULT-SCENE DEMO
    // ORIGIN. vox-gpu-rewrite Stage 2 (2026-05-18): the demo now lives
    // centered in the fixed `(4096, 512, 4096)`-voxel world, so the entity
    // position must translate through `demo_origin_v` to land in the same
    // relative spot the e2e camera frames.
    let demo_off = crate::e2e::gates::demo_origin_v();
    let entity_pos = demo_off + bevy::math::Vec3::new(30.0, 24.0, 30.0);
    entities.instances = vec![EntityInstance {
        position: entity_pos,
        quaternion: [0.0, 0.0, 0.0, 1.0],
        voxel_start: 0,
        entity: 0,
        size,
    }];

    info!(
        "phase-c wave-3 — spawned fixture entity: 4×4×4 green-emissive @ {:?} \
         (demo-relative (30, 24, 30) + demo origin {:?}); voxel_data {} u32s",
        entity_pos,
        demo_off,
        entities.voxel_data.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AppArgs::default().taa_ring_depth` MUST be the documented default
    /// (`18-taa-fidelity.md` fix #3): a mismatch between the const + the
    /// default would mean the WGSL shader-def and the Rust buffer sizing
    /// disagree by default, which is silent TAA ring corruption.
    #[test]
    fn default_taa_ring_depth_is_32() {
        assert_eq!(DEFAULT_TAA_RING_DEPTH, 32);
        assert_eq!(AppArgs::default().taa_ring_depth, DEFAULT_TAA_RING_DEPTH);
    }

    /// The ring depth must stay in the supported VRAM-lever range — 16 / 24 /
    /// 32 are the three values the design records (`01-context.md` §2c /
    /// `design-exploration-qa.md` §6 + the `18-taa-fidelity.md` fix #3
    /// supersession). Pin the default at 32 so future edits do not silently
    /// roll back to the old 16-deep value.
    #[test]
    fn default_taa_ring_depth_is_a_supported_lever_value() {
        let depth = AppArgs::default().taa_ring_depth;
        assert!(
            matches!(depth, 16 | 24 | 32),
            "taa_ring_depth = {depth} is not one of the supported 16/24/32 lever values"
        );
    }

    /// The derived [`WORLD_SIZE_IN_CHUNKS`] and [`WORLD_SIZE_IN_VOXELS`] must
    /// match `WORLD_SIZE_IN_SEGMENTS * WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4`
    /// (chunks) and `× 16` again (voxels). Pinned because the chunks/voxels
    /// constants are hardcoded for `const`-eval and would silently drift if
    /// the segment factors changed without updating them.
    #[test]
    fn fixed_world_size_constants_agree() {
        let chunks = WORLD_SIZE_IN_SEGMENTS * WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
        assert_eq!(
            chunks, WORLD_SIZE_IN_CHUNKS,
            "WORLD_SIZE_IN_CHUNKS drifted from segments × groups × 4",
        );
        assert_eq!(
            chunks * 16,
            WORLD_SIZE_IN_VOXELS,
            "WORLD_SIZE_IN_VOXELS drifted from chunks × 16",
        );
        // The C# canonical values — same numbers `WorldHandler.cs:18-19` pins.
        assert_eq!(WORLD_SIZE_IN_CHUNKS, UVec3::new(256, 32, 256));
        assert_eq!(WORLD_SIZE_IN_VOXELS, UVec3::new(4096, 512, 4096));
    }
}
