//! `streaming::chunk_source` — forward-compat seam per
//! `docs/orchestrate/streaming-world/02b-design-plan-b.md` § K.
//!
//! Phase 2 ships exactly ONE impl ([`NoiseChunkSource`]) that holds the noise
//! state for the GPU `noise_terrain.wgsl` dispatcher. Future sources (`.vox`
//! sparse streaming, Minecraft converters) slot in as additional impls of the
//! same trait without changing the residency manager.
//!
//! The trait itself is intentionally minimal — Phase 2 doesn't need a per-chunk
//! dispatch interface yet, because the streaming GPU path always works through
//! the noise dispatcher (a single shader, one set of parameters). The trait
//! carries `segment_kind` as a discriminant so a future "what kind of chunk
//! source is this resource?" query can decide between dispatch paths.

use bevy::prelude::Resource;

use super::noise_fastnoiselite_cpu_oracle::FnlState;

/// Discriminant for the world-generation source. Phase 2 only ships
/// [`SegmentSourceKind::Noise`]; future impls may add `Vox` / `Minecraft`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentSourceKind {
    /// Procedural noise generator driven by the WGSL FastNoiseLite port.
    Noise,
}

/// Phase-2 forward-compat seam — every chunk source identifies its kind so the
/// streaming driver can pick the right dispatch path. Phase 2's lone impl is
/// [`NoiseChunkSource`]; future `.vox` / Minecraft sources will satisfy the
/// trait alongside it.
pub trait ChunkSource: Send + Sync + 'static {
    /// Discriminant for the chunk source kind.
    fn segment_kind(&self) -> SegmentSourceKind;
}

/// The Phase-2 procedural noise chunk source — holds the [`FnlState`] uniform
/// + the height-relative classification parameters consumed by
/// `noise_terrain.wgsl`.
///
/// Lives as a main-world `Resource`; the noise-dispatch render-world system
/// extracts the contents into a render-world mirror each frame.
#[derive(Resource, Clone, Copy, Debug)]
pub struct NoiseChunkSource {
    /// FastNoiseLite configuration uploaded as `params.state` in the WGSL
    /// shader. Byte-identical layout to the WGSL `FnlState` struct.
    pub state: FnlState,
    /// World-Y at which `noise == 0` flips the solid/empty boundary. Below
    /// `sea_level` is biased toward solid; above is biased toward empty.
    pub sea_level: f32,
    /// Height span over which the noise transition spreads. Larger values give
    /// a wider transition band (less crisp hills); smaller values give sharper
    /// terrain.
    pub terrain_amplitude: f32,
    /// VoxelTypeId emitted for solid voxels (low 15 bits packed into the
    /// `(VOXEL_FULL_FLAG | type)` encoding by the shader).
    pub solid_voxel_type_id: u32,
}

impl Default for NoiseChunkSource {
    fn default() -> Self {
        Self {
            state: default_simple_terrain_state(),
            // Half-world-height in voxels — `WORLD_SIZE_IN_VOXELS.y / 2 = 256`.
            sea_level: 256.0,
            // Architect-picked default — produces a transition band ~64 voxels
            // wide (`amplitude / 1.0` at unit noise scale) which gives credible
            // rolling hills inside the 512-voxel tall fixed world. Justified
            // in `03b-impl-residency.md` § CLI defaults justified.
            terrain_amplitude: 64.0,
            // Palette index 1 — the first non-empty entry in the streaming
            // preset's palette (a generic "ground" type).
            solid_voxel_type_id: 1,
        }
    }
}

impl NoiseChunkSource {
    /// Build a [`NoiseChunkSource`] from the canonical Phase-2 defaults with
    /// `seed` substituted in. Equivalent to taking `Default` and assigning
    /// `state.seed = seed`.
    pub fn from_seed(seed: i32) -> Self {
        let mut me = Self::default();
        me.state.seed = seed;
        me
    }
}

impl ChunkSource for NoiseChunkSource {
    fn segment_kind(&self) -> SegmentSourceKind {
        SegmentSourceKind::Noise
    }
}

/// Phase 2 canonical "simple terrain" preset — `OpenSimplex2 + FBm`, octaves =
/// 4, lacunarity = 2.0, gain = 0.5, frequency = 0.02. Produces credible
/// rolling-hills terrain at the default sea-level + amplitude.
pub fn default_simple_terrain_state() -> FnlState {
    use super::noise_fastnoiselite_cpu_oracle::{
        fnl_create_state, fractal_type, noise_type,
    };
    let mut s = fnl_create_state(1337);
    s.noise_type = noise_type::OPEN_SIMPLEX2;
    s.fractal_type = fractal_type::FBM;
    s.octaves = 4;
    s.lacunarity = 2.0;
    s.gain = 0.5;
    s.frequency = 0.02;
    s
}
