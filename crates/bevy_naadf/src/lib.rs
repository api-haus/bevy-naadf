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
pub mod app_args;
pub mod app_config;
pub mod app_mode;
pub mod camera;
pub mod dev_font;
pub mod diagnostics;
pub mod e2e;
pub mod editor;
pub mod hud;
pub mod render;
pub mod settings;
pub mod voxel;
pub mod window_config;
pub mod world;
pub mod world_size;

pub use app_args::AppArgs;
pub use app_config::AppConfig;
pub use dev_font::{load_dev_font, DevFont};
pub use window_config::WindowConfig;
pub use world_size::{
    WORLD_GEN_SEGMENT_SIZE_IN_GROUPS, WORLD_SIZE_IN_CHUNKS,
    WORLD_SIZE_IN_SEGMENTS, WORLD_SIZE_IN_VOXELS,
};

use bevy::{
    asset::AssetPlugin,
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
    /// Load a voxel file from disk (path relative to repo root or absolute).
    /// The file is read once at `Startup` and the actual parser is selected
    /// from the first 4 magic bytes: MagicaVoxel `.vox` (`"VOX "`) or NAADF
    /// `.cvox` (`"PK\x03\x04"`) — see `voxel/voxel_dispatch.rs`. Failure
    /// logs an error and falls back to [`GridPreset::Default`] so the e2e
    /// harness still has a renderable world. The variant name stays as `Vox`
    /// for source-stability — both formats land into the same install path
    /// (`grid::install_vox_in_fixed_world`) which is parser-agnostic. See
    /// `voxel/vox_import.rs` + `voxel/cvox_import.rs`.
    Vox {
        path: std::path::PathBuf,
    },
    /// **Skybox-only world** — install an EMPTY [`WorldData`] at the fixed
    /// world size (no `ModelData`, `dense_voxel_types = Vec::new()`). The
    /// renderer reads empty `WorldGpu` buffers and produces a pure-sky frame.
    ///
    /// Used by the `--vox-web-parity-skybox` sub-mode of the
    /// `--vox-web-parity` gate (web-vox-async-loading 2026-05-18 follow-up,
    /// Step 8 / Q5). The gate captures this frame, then captures the
    /// counterpart `GridPreset::Vox` rendering, then SSIM-asserts the two
    /// are **dissimilar** (the loaded vox actually rendered geometry).
    Empty,
    /// **Web `?skybox=1` URL-param surface** — same install behaviour as
    /// [`GridPreset::Empty`] (empty world, pure-sky render); kept as a
    /// distinct arm so the wasm bootstrap can express the decision via
    /// `AppArgs.grid_preset` mutation instead of a separate marker
    /// resource + ordering constraint on `setup_test_grid`. The
    /// `[palette-install]` smoke-detector log distinguishes the source
    /// (`"skybox-only"` vs `"cli-empty"`).
    WebSkybox,
}

/// The Phase-B GI pipeline settings (`09-design-b.md` §3.8). The C#
/// `WorldRenderBase` ImGui sliders (`SettingDataRenderBase`) become these
/// `AppArgs` constants — there is no GI settings GUI in the port (§1). The
/// values are the C# slider *defaults*.
#[derive(Clone, Copy, Debug, PartialEq)]
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

impl GiSettings {
    /// Canonical defaults — single source of truth for the C# slider defaults
    /// (`WorldRenderBase.cs:14-25`) + the 5 promoted ray-step caps +
    /// `spatial_iter_count`. Consumed by `Default for GiSettings`, D2's
    /// `settings::KNOBS` table `default:` fields, and D4's GPU-params
    /// `From<&AppArgs>` conversion.
    pub const DEFAULTS: GiSettings = GiSettings {
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
        sun_shadow_taps: 1,
        max_ray_steps_primary: 120,
        max_ray_steps_secondary: 100,
        max_ray_steps_sun: 120,
        max_ray_steps_sun_secondary: 80,
        max_ray_steps_visibility: 60,
        spatial_iter_count: 12,
    };
}

impl Default for GiSettings {
    fn default() -> Self {
        Self::DEFAULTS
    }
}

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
    // `AppConfig` is also inserted so plugins can `.run_if` on its fields
    // (e.g. `DiagnosticsPlugin` self-skips under e2e).
    app.insert_resource(cfg)
        .insert_resource(args.clone())
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
                // web-vox-async-loading 2026-05-18 follow-up Step 8 / Q5 —
                // when the e2e harness is active, install the
                // `CountingLayer` via the `LogPlugin::custom_layer` hook so
                // the `--vox-web-parity-loaded` gate can assert zero
                // `tracing::error!` events fired during the run. The
                // counter is a process-global static; resetting it in the
                // harness boot via
                // `tracing_error_counter::reset_tracing_error_count()` is
                // safe and idempotent.
                .set(if cfg.add_e2e_systems {
                    bevy::log::LogPlugin {
                        custom_layer: e2e::tracing_error_counter::vox_web_parity_log_layer,
                        ..default()
                    }
                } else {
                    bevy::log::LogPlugin::default()
                })
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
                    // fragile. The dedicated `bake` binary (`src/bin/bake.rs`,
                    // `just bake-texarrays`) opts into `AssetMode::Processed`
                    // instead — retained as an InstaMAT pre-bake scaffold.
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
        ));

    // Camera plugin — owns `sync_position_split`, `update_camera_history`'s
    // ordering edge, the production-only `setup_camera` startup, and the
    // free-camera-conditional fly camera + DLSS toggle. Reads
    // `Res<AppConfig>` (inserted above) for the `add_free_camera` /
    // `add_e2e_systems` gates.
    app.add_plugins(camera::CameraPlugin);

    // Press-P diagnostics dump. The plugin self-skips under the e2e harness
    // (`AppConfig.add_e2e_systems` true) via a `.run_if` so registration is
    // unconditional.
    app.add_plugins(diagnostics::DiagnosticsPlugin);

    // Load the embedded Roboto Regular font into Assets<Font> and store the
    // handle as DevFont. Runs first so setup_hud / setup_panel can query it.
    app.add_plugins(dev_font::DevFontPlugin);

    // The test grid + camera spawn — shared. The e2e config spawns a fixed-pose
    // camera instead of the production `setup_camera`; the e2e systems own that
    // (`crate::e2e::add_e2e_systems`).
    //
    // On web, `voxel::web_vox::startup_fetch_default_vox` runs `.before`
    // `setup_test_grid` so it can mutate `AppArgs.grid_preset` to
    // `GridPreset::WebSkybox` (Q6 `?skybox=1` URL-param handling) before
    // `setup_test_grid` reads it. The ordering is enforced by an explicit
    // `.before(setup_test_grid)` on the web-side registration below.
    app.add_systems(Startup, voxel::grid::setup_test_grid);

    // web-vox-async-loading Step 4 (2026-05-18) — async `.vox` parse pump.
    // The polling system drains the `PendingVoxParse` hand-off resource
    // produced by the target-specific async parse spawn (native:
    // `AsyncComputeTaskPool::spawn` from `native_vox_drop_listener`; web:
    // `rayon::spawn` from `web_vox::apply_pending_vox`). Resource +
    // system registered on BOTH targets so the cfg-gated internals share
    // one main-thread driver. The system is wired even when no drop has
    // landed yet — it short-circuits when `pending.inner.is_none()`.
    app.init_resource::<voxel::async_vox::PendingVoxParse>()
        .add_systems(Update, voxel::async_vox::poll_pending_vox_parse);

    // Web-only .vox streaming: kick off the default-model HTTP fetch on
    // `Startup`, and run the consumer system on `Update` so both the fetch
    // and any drag-dropped `.vox` files swap the active scene the moment
    // their bytes are ready. The default scene from `setup_test_grid` stays
    // visible until then.
    //
    // Order: `apply_pending_vox` runs `.after(poll_pending_vox_parse)` so
    // its overlay-hide branch sees `pending.inner.is_none()` the same
    // frame the polling system clears the slot post-install. Otherwise
    // the overlay would linger an extra frame.
    #[cfg(target_arch = "wasm32")]
    app.add_systems(
        Startup,
        voxel::web_vox::startup_fetch_default_vox
            .before(voxel::grid::setup_test_grid),
    )
    .add_systems(
        Update,
        voxel::web_vox::apply_pending_vox
            .after(voxel::async_vox::poll_pending_vox_parse),
    )
    // 2026-05-19 — `?pose=horizon` URL-param camera pin. Runs every frame
    // when the override resource is present; bypasses FreeCamera input so
    // the cross-target SSIM gate's WASM-side capture is deterministic.
    // `.run_if(resource_exists)` keeps the scheduler from invoking the
    // system body when the param is absent (the common case).
    .add_systems(
        Update,
        voxel::web_vox::pin_web_horizon_camera
            .after(voxel::async_vox::poll_pending_vox_parse)
            .run_if(bevy::ecs::schedule::common_conditions::resource_exists::<
                voxel::web_vox::WebHorizonPoseOverride,
            >),
    );

    // Native drag-and-drop: drop a `.vox` file onto the window to replace the
    // active scene. Gated off the e2e harness — winit emits the event in both
    // modes but the e2e harness should never see foreign input.
    #[cfg(not(target_arch = "wasm32"))]
    if !cfg.add_e2e_systems {
        app.add_systems(Startup, voxel::grid::log_native_dnd_registered)
            .add_systems(Update, voxel::grid::native_vox_drop_listener);
    }

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
    }
    // `setup_camera` + `update_camera_history` + `sync_position_split` are
    // all owned by `camera::CameraPlugin` above. `update_camera_history` is
    // registered with `.after(sync_position_split)` inside the plugin.

    if cfg.add_hud {
        // HUD overlay (FPS / timings) — independent of `AppMode`.
        app.add_systems(Startup, hud::setup_hud.after(load_dev_font))
            .add_systems(Update, hud::update_hud);

        // D2-owned plugins: AppMode state + Escape toggle (AppModePlugin),
        // editor HUD + brush dispatch (EditorPlugin), and the Escape
        // settings overlay (SettingsPlugin). Each plugin owns its own
        // `init_resource` / `init_state` / `add_systems` calls.
        app.add_plugins((
            app_mode::AppModePlugin,
            editor::EditorPlugin,
            settings::SettingsPlugin,
        ));

        // 2026-05-19 cross-target SSIM gate support — when the
        // `UiHiddenOverride` resource is inserted (web: by the
        // `?ui=hide` URL param via `web_vox::startup_fetch_default_vox`;
        // native: by anyone who wants UI off), hide all three UI roots
        // every frame. Lives outside the cfg-gated wasm32-only branch
        // because the system is target-agnostic; only the resource
        // inserter (the `?ui=hide` URL-param resolver) is wasm-only.
        #[cfg(target_arch = "wasm32")]
        app.add_systems(
            Update,
            voxel::web_vox::hide_ui.run_if(
                bevy::ecs::schedule::common_conditions::resource_exists::<
                    voxel::web_vox::UiHiddenOverride,
                >,
            ),
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
    // The window config follows the active e2e mode; the mapping lives in
    // [`window_config::window_for_e2e_args`] so adding a new mode is a
    // one-file edit. All non-`--resize-test` / non-`--small-edit-repro` /
    // non-`--vox-horizon-native` runs use the standard 256×256 e2e window.
    let mut cfg = AppConfig::e2e();
    cfg.window = window_config::window_for_e2e_args(&args);
    let app = build_app_with_args(cfg, args);
    e2e::run_with_app(app)
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

// Tests moved with their subjects:
//   - `default_taa_ring_depth_*` → `app_args.rs::tests`
//   - `fixed_world_size_constants_agree` → `world_size.rs::tests::world_size_matches_csharp`
