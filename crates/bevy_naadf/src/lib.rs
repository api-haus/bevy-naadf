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
pub mod e2e;
pub mod hud;
pub mod panel;
pub mod render;
pub mod texture_array;
pub mod voxel;
pub mod world;

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
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum GridPreset {
    /// The default scene: ground slab + axis-aligned boxes + a sphere + one
    /// emissive box.
    #[default]
    Default,
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
            // Multi-tap sun shadow — paper §5.2 soft-shadow noise mitigation
            // (Dispatch A — `19-gi-reservoir-scope.md` §3.1). Default 4.
            sun_shadow_taps: 4,
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
#[derive(Resource, Clone, Copy)]
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
}

impl WindowConfig {
    /// The production window — platform default size, resizable.
    fn windowed() -> Self {
        Self {
            resolution: None,
            resizable: true,
            title: "bevy-naadf",
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
            resizable: false,
            title: "bevy-naadf e2e_render",
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

    app.insert_resource(args)
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
            // InstaMAT baked-material loader — registers `MaterialRonLoader` so
            // `materials/<name>/material.ron` resolves to a `StandardMaterial`.
            // Infrastructure only: nothing in the scene consumes a baked
            // material yet (wiring baked PBR into the custom voxel render path
            // is a separate future effort). The `bevy-instamat` dependency is
            // the `instamat` feature OFF — zero FFI / libloading / image-baker
            // code enters this build.
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
        app.add_systems(Startup, camera::setup_camera);
    }

    // The camera-history ring update must run *after* `sync_position_split` so
    // the ring stores this frame's current camera state (`06-design-a2.md`
    // §9.3).
    app.add_systems(
        Update,
        render::taa::update_camera_history.after(camera::sync_position_split),
    );

    if cfg.add_hud {
        app.add_systems(Startup, hud::setup_hud)
            .add_systems(Update, hud::update_hud);
        // Quality panel (`21-design-quality-panel.md`) — gated on the same
        // `add_hud` flag as the HUD itself. The e2e harness (`AppConfig::e2e`)
        // sets `add_hud = false`, so the panel never spawns in the bounded
        // harness — luminance gates are unaffected.
        app.init_resource::<panel::PanelState>()
            .add_systems(Startup, panel::setup_panel)
            .add_systems(
                Update,
                (
                    panel::toggle_panel,
                    panel::adjust_panel,
                    panel::update_panel_text,
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
    let app = build_app_with_args(AppConfig::e2e(), args);
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

    // Place at (30, 24, 30) — chunk (1,1,1) corner area, well above the
    // ground slab + existing emissive blocks. Identity quaternion.
    entities.instances = vec![EntityInstance {
        position: bevy::math::Vec3::new(30.0, 24.0, 30.0),
        quaternion: [0.0, 0.0, 0.0, 1.0],
        voxel_start: 0,
        entity: 0,
        size,
    }];

    info!(
        "phase-c wave-3 — spawned fixture entity: 4×4×4 green-emissive @ (30, 24, 30); \
         voxel_data {} u32s",
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
}
