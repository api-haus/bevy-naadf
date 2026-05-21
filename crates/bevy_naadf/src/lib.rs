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
#[cfg(target_os = "android")]
pub mod android_main;
pub mod app_args;
pub mod app_config;
pub mod app_mode;
pub mod bootstrap;
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
pub use settings::canonical::GiSettings;
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
#[derive(Resource, Clone, Default, PartialEq, Eq, Debug)]
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
    /// distinct arm so the wasm bootstrap can express the decision via the
    /// `GridPreset` resource value instead of a separate marker resource.
    /// Step 5 of the config-as-resource refactor resolves `?skybox=1` into
    /// `BootstrapInputs.grid_preset = GridPreset::WebSkybox` BEFORE the App
    /// is built (see `crate::voxel::web_vox::resolve_skybox_only_param`),
    /// so `setup_test_grid` reads the already-correct `Res<GridPreset>`
    /// arm with no `Startup`-time mutation or ordering constraint. The
    /// `[palette-install]` smoke-detector log distinguishes the source
    /// (`"skybox-only"` vs `"cli-empty"`).
    WebSkybox,
}

// `GiSettings` lives in `settings/canonical.rs` (D7 cleanup follow-up 4); the
// `pub use settings::canonical::GiSettings;` re-export below preserves all
// `crate::GiSettings` import sites.

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

/// Build the bevy-naadf `App` with **GPU budget preselection** applied.
///
/// Runs the mobile GPU budget probe ([`crate::render::budget::probe_and_select`])
/// BEFORE building the App, writes the chosen rung into a
/// [`crate::bootstrap::BootstrapInputs`] (Step 2 of the config-as-resource
/// refactor — the migrated field is `taa_ring_depth: TaaRingConfig`; legacy
/// fields still on `AppArgs` ride along through `inputs.args`), then inserts
/// the budget-selected [`render::budget::EffectiveWorldSize`] +
/// [`render::budget::InvalidSampleStorageCount`] resources, overriding the
/// defensive canonical seeds inside [`build_app_with_args`]. On desktop with a
/// generous storage-buffer cap (≥ 1.35 GiB) the budget picks canonical
/// defaults — output is byte-identical to [`build_app_with_args`]. On mobile
/// targets (256 MiB cap — Android Mali / iOS Safari WebGPU) the routine picks
/// the deepest world + TAA + invalid-samples rungs that fit `cap × 75%`.
///
/// Production callers:
///   * Desktop + WebGPU/wasm32 — `src/main.rs::fn main()` → this →`.run()`.
///   * Android JNI entry — `src/android_main.rs::android_main()` → this →
///     [`bevy::winit::WinitSettings::mobile`] → `.run()`.
///
/// The e2e_render binary intentionally skips this and uses
/// [`build_app_with_args`] directly — e2e gates need canonical world / TAA
/// for deterministic SSIM comparisons across runs and across machines.
pub fn build_app_with_budget(cfg: AppConfig, args: AppArgs, grid_preset: GridPreset) -> App {
    let caps = crate::render::budget::probe_and_select();
    let inputs = crate::bootstrap::BootstrapInputs {
        args,
        grid_preset,
        taa_ring_depth: crate::render::taa::TaaRingConfig {
            depth: caps.taa_ring_depth,
        },
        ..Default::default()
    };
    let mut app = crate::bootstrap::build_app_with_bootstrap_inputs(cfg, inputs);
    app.insert_resource(crate::render::budget::EffectiveWorldSize::from_segments(
        caps.world_size_in_segments,
    ));
    app.insert_resource(crate::render::budget::InvalidSampleStorageCount(
        caps.invalid_sample_storage_count,
    ));
    app
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
        .init_resource::<render::taa::CameraHistory>();

    // Step 3 of the config-as-resource refactor — defensively seed the
    // per-domain `TaaConfig` and `GiSettings` resources at the canonical
    // defaults so the e2e_render binary's direct
    // `build_app(AppConfig::e2e())` path (`run_e2e_render` /
    // `run_e2e_render_with_args`, which bypass
    // `build_app_with_bootstrap_inputs`) still has the resources
    // `update_camera_history` (`Res<TaaConfig>`) and the settings panel
    // (`ResMut<GiSettings>`) need.  Callers routing through
    // `build_app_with_bootstrap_inputs` overwrite both with their `inputs`
    // values via Bevy's `insert_resource` overwrite-in-place semantic. Same
    // shape as the `EffectiveWorldSize::canonical()` /
    // `InvalidSampleStorageCount::canonical()` defensive seeds below. Step 9
    // deletes these once every caller routes through the bootstrap fan-out.
    if !app.world().contains_resource::<render::taa::TaaConfig>() {
        app.insert_resource(render::taa::TaaConfig::default());
    }
    if !app.world().contains_resource::<crate::GiSettings>() {
        app.insert_resource(crate::GiSettings::default());
    }
    // Same defensive seed for the Step-2 `TaaRingConfig` — the settings
    // panel readonly knob reads it via `Res<TaaRingConfig>` (non-Option) when
    // the panel is open. Today the e2e harness disables HUD/settings so the
    // gap was invisible; defensively seeding keeps the future
    // `cfg.add_hud + e2e` combination from regressing.
    if !app.world().contains_resource::<render::taa::TaaRingConfig>() {
        app.insert_resource(render::taa::TaaRingConfig::default());
    }
    // Step 4 of the config-as-resource refactor — defensive seed for the
    // per-domain `ConstructionConfig`. `run_gpu_construction_startup` and the
    // `extract_construction_config` system both need it; on the
    // `build_app(AppConfig::e2e())` path we don't go through
    // `build_app_with_bootstrap_inputs`, so the seed is the canonical
    // `for_target_arch()` value (the wasm32 arm applies the documented
    // clamp). Callers routing through the bootstrap fan-out overwrite it.
    if !app
        .world()
        .contains_resource::<render::construction::ConstructionConfig>()
    {
        app.insert_resource(render::construction::ConstructionConfig::for_target_arch());
    }
    // Step 5 of the config-as-resource refactor — defensive seed for the
    // per-domain `GridPreset`. `setup_test_grid` reads it as `Res<GridPreset>`
    // (non-Option) at `Startup`; the `build_app(AppConfig::e2e())` path
    // (`run_e2e_render` / `run_e2e_render_with_args`) bypasses
    // `build_app_with_bootstrap_inputs`, so without the seed the system
    // panics on the missing resource. Canonical default = `GridPreset::Default`
    // (the embedded primitive test scene). Callers routing through the
    // bootstrap fan-out overwrite it via `insert_resource` overwrite-in-place.
    // Step 9 deletes this seed once every caller routes through the fan-out.
    if !app.world().contains_resource::<crate::GridPreset>() {
        app.insert_resource(crate::GridPreset::default());
    }

    // Mobile GPU budget — defensively seed [`EffectiveWorldSize`] to the C#
    // canonical value if no caller (Android entry / future probe-mode CLI)
    // inserted one before this point. Every existing caller (production
    // `main.rs`, 17 e2e gates, every `--bin e2e_render -- <mode>` path) leaves
    // this resource absent; the seed makes their behaviour byte-identical to
    // pre-budget code (the canonical 256×32×256 chunk world).
    // The Android probe routine (`android_main.rs`) overrides this with a
    // smaller rung AFTER `build_app_with_args` returns; Bevy's
    // `insert_resource` second-call semantic is overwrite-in-place.
    //
    // See `docs/orchestrate/mobile-budget/02-design.md` §3 "Insertion point".
    if !app.world().contains_resource::<crate::render::budget::EffectiveWorldSize>() {
        app.insert_resource(crate::render::budget::EffectiveWorldSize::canonical());
    }
    // Post-2026-05-21 — the unlit-sample ring is the third per-pixel-scaled
    // mobile budget lever (`docs/orchestrate/mobile-budget/05-consolidated-fix.md`
    // Design §1). Defensive seed of the canonical 8 keeps every existing
    // caller byte-identical; the Android entry overrides post-build with the
    // budget-selected value (typically 4 on mobile).
    if !app
        .world()
        .contains_resource::<crate::render::budget::InvalidSampleStorageCount>()
    {
        app.insert_resource(crate::render::budget::InvalidSampleStorageCount::canonical());
    }

    app
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

    // `VoxelIoPlugin` owns `setup_test_grid` + `PendingVoxParse` init +
    // `poll_pending_vox_parse` + wasm `web_vox::startup_fetch_default_vox` +
    // wasm `apply_pending_vox` + wasm `pin_web_horizon_camera` + native
    // drag-and-drop systems. Reads `Res<AppConfig>` (inserted above) to gate
    // the native dnd registration off the e2e harness.
    app.add_plugins(voxel::VoxelIoPlugin);

    // `spawn_phase_c_test_entity` (the W4 fixture spawner gated on
    // `AppArgs::spawn_test_entity`) lives in
    // `render::construction::test_fixture` and is registered inside
    // `ConstructionPlugin::build` with the same
    // `.after(voxel::grid::setup_test_grid)` ordering. See D7 cleanup
    // follow-up 2.
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

// `spawn_phase_c_test_entity` moved to `render::construction::test_fixture`
// (D7 cleanup follow-up 2).

// Tests moved with their subjects:
//   - `default_taa_ring_depth_*` → `app_args.rs::tests`
//   - `fixed_world_size_constants_agree` → `world_size.rs::tests::world_size_matches_csharp`
