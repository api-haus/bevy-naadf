//! PBR rendering debugger — runtime-switchable per-pixel BRDF-channel
//! visualisation.
//!
//! See `docs/orchestrate/pbr-raymarching/05-diagnostic.md` § "PBR rendering
//! debugger (2026-05-18, post-`bf3281f`)" for the design rationale.
//!
//! ## Pipeline
//!
//! 1. [`DebugViewState`] — main-world resource holding the current
//!    [`DebugViewMode`]. Default is [`DebugViewMode::Off`] (production
//!    path, zero perf cost).
//! 2. [`cycle_debug_view_mode`] — `Update` system; reads
//!    `ButtonInput<KeyCode>`:
//!    - `F1`: toggle Off ↔ last-non-zero mode (defaults to `Albedo`).
//!    - `BracketLeft`: step mode -1.
//!    - `BracketRight`: step mode +1.
//! 3. [`crate::render::extract::extract_debug_view`] — ferries the mode
//!    into the render world as `ExtractedDebugView`.
//! 4. [`crate::render::prepare::prepare_frame_gpu`] — writes the mode into
//!    `GpuRenderParams.debug_view_mode` on the per-frame uniform upload.
//! 5. `naadf_first_hit.wgsl` checks `params.debug_view_mode != 0u`; if so,
//!    collects [`pbr_sampling.wgsl::PbrDebugInputs`] from the surface
//!    samples + BRDF results, calls `debug_view_override`, overwrites
//!    `acc.light` with the debug colour, clears `acc.absorption` so
//!    downstream passes contribute zero, and stamps the TAA accumulator so
//!    the blit reads the debug colour crisply.

use bevy::input::ButtonInput;
use bevy::prelude::*;

/// Per-channel debug visualisation modes. Mode 0 / [`DebugViewMode::Off`]
/// is the production path — the first-hit shader short-circuits before
/// touching any debug code (one uniform load + one compare per pixel).
///
/// The `u32` discriminant is the value uploaded to
/// `GpuRenderParams.debug_view_mode`; it must match the `switch` in
/// `pbr_sampling.wgsl::debug_view_override`.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DebugViewMode {
    /// Production path. Zero overhead.
    #[default]
    Off                  = 0,
    /// Triplanar-sampled diffuse RGB × `albedo_tint`, before lighting.
    Albedo               = 1,
    /// Perturbed (normal-mapped) world-space normal, RGB-encoded `(n+1)/2`.
    NormalPerturbed      = 2,
    /// Voxel face normal before normal-map perturbation, RGB-encoded.
    NormalGeometric      = 3,
    /// MRH.R scalar as greyscale.
    Metallic             = 4,
    /// MRH.G scalar as greyscale.
    Roughness            = 5,
    /// Diffuse.A scalar (AO) as greyscale.
    Ao                   = 6,
    /// MRH.B (height at POM-displaced UV) as greyscale.
    Height               = 7,
    /// `mix(0.04, albedo, metallic)` — Schlick base reflectance, RGB.
    F0                   = 8,
    /// Schlick F at the sun half-vector (Fresnel weight), greyscale.
    KS                   = 9,
    /// `(1 - F) * (1 - metallic)` (diffuse weight), greyscale.
    KD                   = 10,
    /// Direct-only sun contribution proxy (no occlusion ray).
    DirectOnly           = 11,
    /// GI-only proxy (atmosphere fold + diffuse transport).
    GiOnly               = 12,
    /// `pom_self_shadow` factor as greyscale.
    PomSelfShadow        = 13,
    /// POM displaced UV `(u, v, 0)` as RGB, fract-folded to `[0,1)`.
    PomDisplacedUv       = 14,
    /// Material layer index, hashed into a saturated false-colour RGB.
    MaterialLayerIndex   = 15,
    /// Triplanar weights as RGB.
    TriplanarWeights     = 16,
    /// Emissive contribution only (zero for non-emissive voxels).
    Emissive             = 17,
}

impl DebugViewMode {
    /// Total count of non-Off variants. Used by the e2e gate's iteration
    /// loop.
    pub const NUM_DEBUG_MODES: u32 = 17;

    /// Convert a `u32` to a debug mode, clamping out-of-range values to
    /// `Off`. Used by the e2e gate and the keyboard cycler.
    pub fn from_u32(v: u32) -> Self {
        match v {
            1  => Self::Albedo,
            2  => Self::NormalPerturbed,
            3  => Self::NormalGeometric,
            4  => Self::Metallic,
            5  => Self::Roughness,
            6  => Self::Ao,
            7  => Self::Height,
            8  => Self::F0,
            9  => Self::KS,
            10 => Self::KD,
            11 => Self::DirectOnly,
            12 => Self::GiOnly,
            13 => Self::PomSelfShadow,
            14 => Self::PomDisplacedUv,
            15 => Self::MaterialLayerIndex,
            16 => Self::TriplanarWeights,
            17 => Self::Emissive,
            _  => Self::Off,
        }
    }

    /// Short human-readable name for the HUD overlay.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off                  => "Off",
            Self::Albedo               => "Albedo",
            Self::NormalPerturbed      => "Normal (perturbed)",
            Self::NormalGeometric      => "Normal (geometric)",
            Self::Metallic             => "Metallic",
            Self::Roughness            => "Roughness",
            Self::Ao                   => "AO",
            Self::Height               => "Height",
            Self::F0                   => "F0",
            Self::KS                   => "kS (Fresnel weight)",
            Self::KD                   => "kD (diffuse weight)",
            Self::DirectOnly           => "Direct-only",
            Self::GiOnly               => "GI-only",
            Self::PomSelfShadow        => "POM self-shadow",
            Self::PomDisplacedUv       => "POM displaced UV",
            Self::MaterialLayerIndex   => "Material layer",
            Self::TriplanarWeights     => "Triplanar weights",
            Self::Emissive             => "Emissive",
        }
    }
}

/// Main-world resource holding the current [`DebugViewMode`]. Reset to
/// `Off` at startup. Mutated by [`cycle_debug_view_mode`] and read by
/// [`crate::render::extract::extract_debug_view`].
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct DebugViewState {
    pub mode: DebugViewMode,
    /// Last non-Off mode so `F1` toggles back to it instead of always
    /// landing on `Albedo`.
    pub last_active: Option<DebugViewMode>,
}

/// `Update` system: cycle the debug view mode on key press.
///
/// Key bindings:
/// - `F1` — toggle Off ↔ `last_active` (default `Albedo` if never set).
/// - `BracketLeft` (`[`) — step mode -1 (wrap from 1 → Off).
/// - `BracketRight` (`]`) — step mode +1 (wrap from N → 1).
pub fn cycle_debug_view_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<DebugViewState>,
) {
    let mut changed = false;
    if keys.just_pressed(KeyCode::F1) {
        state.mode = match state.mode {
            DebugViewMode::Off => state.last_active.unwrap_or(DebugViewMode::Albedo),
            other => {
                state.last_active = Some(other);
                DebugViewMode::Off
            }
        };
        changed = true;
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        let next = (state.mode as u32) + 1;
        let wrapped = if next > DebugViewMode::NUM_DEBUG_MODES { 1 } else { next };
        state.mode = DebugViewMode::from_u32(wrapped);
        if state.mode != DebugViewMode::Off {
            state.last_active = Some(state.mode);
        }
        changed = true;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        let cur = state.mode as u32;
        let prev = if cur <= 1 {
            DebugViewMode::NUM_DEBUG_MODES
        } else {
            cur - 1
        };
        state.mode = DebugViewMode::from_u32(prev);
        if state.mode != DebugViewMode::Off {
            state.last_active = Some(state.mode);
        }
        changed = true;
    }
    if changed {
        info!(target: "debug_view", "DebugViewMode = {:?} ({})", state.mode, state.mode.label());
    }
}

/// Plugin — install the resource + the keyboard system.
pub struct DebugViewPlugin;

impl Plugin for DebugViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugViewState>()
            .add_systems(Update, cycle_debug_view_mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every debug mode in the `from_u32` table must round-trip through
    /// its `u32` discriminant. Catches a misaligned discriminant the
    /// shader's `switch` would silently miss.
    #[test]
    fn debug_view_mode_from_u32_round_trips() {
        for m in 0..=DebugViewMode::NUM_DEBUG_MODES {
            let mode = DebugViewMode::from_u32(m);
            assert_eq!(mode as u32, m, "DebugViewMode discriminant mismatch at {m}");
        }
    }

    /// Out-of-range values map to `Off` (defensive).
    #[test]
    fn debug_view_mode_out_of_range_clamps_off() {
        assert_eq!(DebugViewMode::from_u32(99), DebugViewMode::Off);
        assert_eq!(
            DebugViewMode::from_u32(DebugViewMode::NUM_DEBUG_MODES + 1),
            DebugViewMode::Off,
        );
    }

    /// `label` is non-empty for every mode (HUD readability).
    #[test]
    fn debug_view_mode_labels_non_empty() {
        for m in 0..=DebugViewMode::NUM_DEBUG_MODES {
            let label = DebugViewMode::from_u32(m).label();
            assert!(!label.is_empty(), "empty label for mode {m}");
        }
    }
}
