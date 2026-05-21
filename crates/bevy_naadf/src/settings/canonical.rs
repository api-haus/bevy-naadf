//! Canonical `GiSettings` struct + `DEFAULTS` const — the SSoT-1 home.
//!
//! Per D7 architect §2 F2, D2's KNOBS table reads `GiSettings::DEFAULTS.*`; D4's
//! GPU uniform mirror (`GpuRenderParams`/`GpuGiParams`) also reads
//! `GiSettings::DEFAULTS.*` via `From<&AppArgs>`. This file holds the canonical
//! definition; `lib.rs` re-exports `GiSettings` for source-stability on existing
//! `crate::GiSettings` imports.
//!
//! **Step 3 of the config-as-resource refactor** promoted `GiSettings` to a
//! Bevy `Resource` (via `#[derive(Resource)]`). The settings panel mutates it
//! through `ResMut<GiSettings>` and the render-world extract reads it through
//! `Extract<Option<Res<GiSettings>>>` — `AppArgs.gi` is gone.

use bevy::prelude::Resource;

/// The Phase-B GI pipeline settings (`09-design-b.md` §3.8). The C#
/// `WorldRenderBase` ImGui sliders (`SettingDataRenderBase`) become these
/// `GiSettings` constants — there is no GI settings GUI in the port (§1). The
/// values are the C# slider *defaults*.
///
/// Step 3 of the config-as-resource refactor promoted this struct to a
/// `Resource`: bootstrap inserts it via [`crate::bootstrap::BootstrapInputs`];
/// the settings panel mutates it via `ResMut<GiSettings>`; the render-world
/// `extract_gi_config` reads it via `Extract<Option<Res<GiSettings>>>`.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
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
