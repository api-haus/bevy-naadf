//! Composable noise presets using fastnoise2's typed generator API.
//!
//! Presets are defined here and work on both native (direct) and WASM
//! (via the extended C-ABI with `vx_noise_create_preset`).

use fastnoise2::generator::prelude::*;
use fastnoise2::SafeNode;

/// Noise preset IDs. Must match across native and WASM.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoisePreset {
  SimpleTerrain = 0,
  PlanetTerrain = 1,
  SurfaceDetail = 2,
}

/// Build a SafeNode from a preset ID.
pub fn build_preset(id: u32) -> Option<SafeNode> {
  match id {
    0 => Some(simple_terrain()),
    1 => Some(build_planet_sdf(&PlanetNoiseParams::default())),
    2 => Some(surface_detail()),
    _ => None,
  }
}

/// Parameters for building the planet displacement noise graph.
///
/// Single FBm → domain warp → power redistribution → amplitude scaling.
/// A power exponent > 1 pushes elevation toward 0 (sea level), creating
/// wide flat lowlands with rare peaks — matching real terrain distributions.
///
/// All spatial values are in **noise-space** (world coordinates * frequency).
/// The sphere SDF is computed externally in f64 for precision at planet scale.
/// This graph outputs only the **clamped displacement** to add to the sphere.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanetNoiseParams {
  /// FBm domain scale — controls continent count/size (default: 0.4).
  /// Higher = more, smaller continents; lower = fewer, larger.
  pub scale: f32,
  /// Domain warp amplitude — coastline irregularity (default: 0.45).
  pub warp_amplitude: f32,
  /// Elevation curve power — 1.0 = uniform, 2.0 = Earth-like (default: 2.0).
  /// Higher values create flatter lowlands with rarer, sharper peaks.
  pub redistribution: f32,
  /// Fraction of redistributed range below water (default: 0.4).
  pub sea_level: f32,
  /// Total height range in noise-space (default: 0.35).
  pub amplitude: f32,

  // -- Fractal controls --
  /// FBm octaves (default: 6). More = finer detail.
  pub octaves: i32,
  /// FBm lacunarity (default: 2.5). Higher = sharper fractal steps.
  pub lacunarity: f32,
  /// FBm gain/persistence (default: 0.5). Lower = smoother.
  pub gain: f32,

  // -- Derived (auto-computed) --
  /// Minimum displacement — ocean floor depth in noise-space.
  /// Derived: `-(sea_level * amplitude * 1.2)`.
  pub ocean_floor: f32,
  /// Maximum displacement — peak height limit in noise-space.
  /// Derived: `(1 - sea_level) * amplitude * 1.3`.
  pub peak_limit: f32,
}

impl PlanetNoiseParams {
  /// Create params with derived ocean_floor and peak_limit computed
  /// automatically. Fractal params use defaults.
  pub fn new(
    scale: f32,
    warp_amplitude: f32,
    redistribution: f32,
    sea_level: f32,
    amplitude: f32,
  ) -> Self {
    let mut p = Self {
      scale,
      warp_amplitude,
      redistribution,
      sea_level,
      amplitude,
      octaves: 6,
      lacunarity: 2.5,
      gain: 0.5,
      ocean_floor: 0.0,
      peak_limit: 0.0,
    };
    p.recompute_derived();
    p
  }

  /// Recompute `ocean_floor` and `peak_limit` from the current sea_level and
  /// amplitude. Call after mutating those fields directly.
  pub fn recompute_derived(&mut self) {
    self.ocean_floor = -(self.sea_level * self.amplitude * 1.2);
    self.peak_limit = (1.0 - self.sea_level) * self.amplitude * 1.3;
  }
}

impl Default for PlanetNoiseParams {
  fn default() -> Self {
    Self::new(0.4, 0.45, 2.0, 0.4, 0.35)
  }
}

/// Build a planet displacement noise graph from parameters.
///
/// Single FBm → domain warp → power redistribution → amplitude scaling:
///
/// 1. **FBm**: `supersimplex().fbm(gain, 0, octaves, lacunarity)`
/// 2. **Domain warp**: `domain_warp_super_simplex(warp_amplitude, scale * 2)`
/// 3. **Redistribution**: remap [−1,1]→[0,1], abs, powf
/// 4. **Elevation**: `(redistributed − sea_level) * amplitude`
/// 5. **Clamp**: `max(ocean_floor).min(peak_limit)`
///
/// Outputs **clamped displacement** in noise-space, NOT a full SDF.
/// The caller combines this with a sphere SDF computed in f64:
/// ```text
/// world_sdf = sphere_sdf_f64 - (displacement / frequency)
/// ```
///
/// Input coordinates should be center-relative and frequency-scaled:
/// `(world_pos - center) * frequency`.
pub fn build_planet_sdf(params: &PlanetNoiseParams) -> SafeNode {
  let s = params.scale;

  // Single FBm + domain warp for organic coastlines
  let terrain = supersimplex()
    .fbm(params.gain, 0.0, params.octaves, params.lacunarity)
    .domain_scale(s)
    .domain_warp_super_simplex(params.warp_amplitude, s * 2.0)
    .remap(-1.0, 1.0, 0.0, 1.0)
    .abs()
    .powf(params.redistribution);

  // Elevation: (redistributed - sea_level) * amplitude → 0 = sea level
  let elevation = (terrain - params.sea_level) * params.amplitude;

  // Clamp to ocean floor / peak limit
  elevation
    .max(params.ocean_floor)
    .min(params.peak_limit)
    .build()
    .0
}

/// FBm + domain warp terrain (equivalent to old SIMPLE_TERRAIN encoded string).
fn simple_terrain() -> SafeNode {
  (supersimplex().fbm(0.65, 0.5, 4, 2.5).domain_scale(0.66)
    + gradient().with_multipliers([0.0, 3.0, 0.0, 0.0]))
  .domain_warp_gradient(0.2, 2.0)
  .domain_warp_progressive(0.7, 0.5, 2, 2.5)
  .build()
  .0
}

/// Fractal surface detail noise for close-up terrain roughness.
///
/// Designed to be sampled at a separate (higher) frequency from the
/// continental `planet_terrain()` preset and additively displaced onto the
/// SDF. 5-octave FBm with light domain warp produces features spanning a
/// 16x frequency range with organic, non-grid-aligned patterns.
///
/// Output range: approximately [-1, 1].
fn surface_detail() -> SafeNode {
  supersimplex()
    .seed_offset(99)
    .fbm(0.5, 0.0, 5, 2.0)
    .domain_warp_gradient(0.4, 1.5)
    .build()
    .0
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_build_simple_terrain() {
    let node = build_preset(NoisePreset::SimpleTerrain as u32);
    assert!(node.is_some(), "Failed to build SimpleTerrain preset");

    let node = node.unwrap();
    let mut output = vec![0.0f32; 32 * 32 * 32];
    node.gen_uniform_grid_3d(
      &mut output,
      0.0,
      0.0,
      0.0,
      32,
      32,
      32,
      0.02,
      0.02,
      0.02,
      1337,
    );
    assert!(output.iter().any(|&v| v != 0.0), "All values are zero");
  }

  #[test]
  fn test_build_planet_terrain() {
    let node = build_preset(NoisePreset::PlanetTerrain as u32);
    assert!(node.is_some(), "Failed to build PlanetTerrain preset");

    let node = node.unwrap();
    let mut output = vec![0.0f32; 32 * 32 * 32];
    node.gen_uniform_grid_3d(
      &mut output,
      0.0,
      0.0,
      0.0,
      32,
      32,
      32,
      0.02,
      0.02,
      0.02,
      1337,
    );
    assert!(output.iter().any(|&v| v != 0.0), "All values are zero");
  }

  #[test]
  fn test_build_planet_displacement_custom() {
    let params = PlanetNoiseParams::new(
      0.8,  // scale
      0.3,  // warp_amplitude
      3.0,  // redistribution
      0.5,  // sea_level
      0.40, // amplitude
    );
    let node = build_planet_sdf(&params);
    let mut output = vec![0.0f32; 8 * 8 * 8];
    node.gen_uniform_grid_3d(&mut output, 0.0, 0.0, 0.0, 8, 8, 8, 1.0, 1.0, 1.0, 1337);
    assert!(output.iter().any(|&v| v != 0.0), "All values are zero");
  }

  #[test]
  fn test_build_surface_detail() {
    let node = build_preset(NoisePreset::SurfaceDetail as u32);
    assert!(node.is_some(), "Failed to build SurfaceDetail preset");

    let node = node.unwrap();
    let mut output = vec![0.0f32; 32 * 32 * 32];
    node.gen_uniform_grid_3d(
      &mut output,
      0.0,
      0.0,
      0.0,
      32,
      32,
      32,
      0.02,
      0.02,
      0.02,
      1337,
    );
    assert!(output.iter().any(|&v| v != 0.0), "All values are zero");
  }

  #[test]
  fn test_surface_detail_edge_coherency() {
    let node = build_preset(NoisePreset::SurfaceDetail as u32).unwrap();

    const SIZE: usize = 32;
    const VOXEL_SIZE: f32 = 1.0;
    let seed = 1337;

    let mut chunk_a = vec![0.0f32; SIZE * SIZE * SIZE];
    node.gen_uniform_grid_3d(
      &mut chunk_a,
      0.0,
      0.0,
      0.0,
      SIZE as i32,
      SIZE as i32,
      SIZE as i32,
      VOXEL_SIZE,
      VOXEL_SIZE,
      VOXEL_SIZE,
      seed,
    );

    let chunk_b_offset_x = 28.0 * VOXEL_SIZE;
    let mut chunk_b = vec![0.0f32; SIZE * SIZE * SIZE];
    node.gen_uniform_grid_3d(
      &mut chunk_b,
      chunk_b_offset_x,
      0.0,
      0.0,
      SIZE as i32,
      SIZE as i32,
      SIZE as i32,
      VOXEL_SIZE,
      VOXEL_SIZE,
      VOXEL_SIZE,
      seed,
    );

    let mut mismatches = 0;
    let mut max_diff: f32 = 0.0;
    for y in 0..SIZE {
      for z in 0..SIZE {
        for overlap_idx in 0..4 {
          let a_x = 28 + overlap_idx;
          let b_x = overlap_idx;
          let a_idx = z * SIZE * SIZE + y * SIZE + a_x;
          let b_idx = z * SIZE * SIZE + y * SIZE + b_x;
          let diff = (chunk_a[a_idx] - chunk_b[b_idx]).abs();
          if diff > 1e-6 {
            mismatches += 1;
            max_diff = max_diff.max(diff);
          }
        }
      }
    }

    assert_eq!(
      mismatches, 0,
      "Found {} edge sample mismatches (max diff: {})",
      mismatches, max_diff
    );
  }

  #[test]
  fn test_invalid_preset_returns_none() {
    assert!(build_preset(999).is_none());
  }

  #[test]
  fn test_preset_edge_coherency() {
    let node = build_preset(NoisePreset::PlanetTerrain as u32).unwrap();

    const SIZE: usize = 32;
    const VOXEL_SIZE: f32 = 1.0;
    let seed = 1337;

    let mut chunk_a = vec![0.0f32; SIZE * SIZE * SIZE];
    node.gen_uniform_grid_3d(
      &mut chunk_a,
      0.0,
      0.0,
      0.0,
      SIZE as i32,
      SIZE as i32,
      SIZE as i32,
      VOXEL_SIZE,
      VOXEL_SIZE,
      VOXEL_SIZE,
      seed,
    );

    let chunk_b_offset_x = 28.0 * VOXEL_SIZE;
    let mut chunk_b = vec![0.0f32; SIZE * SIZE * SIZE];
    node.gen_uniform_grid_3d(
      &mut chunk_b,
      chunk_b_offset_x,
      0.0,
      0.0,
      SIZE as i32,
      SIZE as i32,
      SIZE as i32,
      VOXEL_SIZE,
      VOXEL_SIZE,
      VOXEL_SIZE,
      seed,
    );

    let mut mismatches = 0;
    let mut max_diff: f32 = 0.0;
    for y in 0..SIZE {
      for z in 0..SIZE {
        for overlap_idx in 0..4 {
          let a_x = 28 + overlap_idx;
          let b_x = overlap_idx;
          let a_idx = z * SIZE * SIZE + y * SIZE + a_x;
          let b_idx = z * SIZE * SIZE + y * SIZE + b_x;
          let diff = (chunk_a[a_idx] - chunk_b[b_idx]).abs();
          if diff > 1e-6 {
            mismatches += 1;
            max_diff = max_diff.max(diff);
          }
        }
      }
    }

    assert_eq!(
      mismatches, 0,
      "Found {} edge sample mismatches (max diff: {})",
      mismatches, max_diff
    );
  }

  /// Zero displacement when ocean_floor=0 and peak_limit=0.
  /// Graph outputs clamped displacement, which should be exactly 0.
  #[test]
  fn test_zero_displacement_with_zero_clamps() {
    let params = PlanetNoiseParams {
      ocean_floor: 0.0,
      peak_limit: 0.0,
      ..Default::default()
    };
    let node = build_planet_sdf(&params);

    // With max(displacement, 0).min(0), output should be exactly 0
    // regardless of the noise values underneath.
    let val = node.gen_single_3d(0.0, 0.0, 0.0, 1337);
    assert!(
      val.abs() < 1e-6,
      "Displacement should be 0 with zero clamps, got {}",
      val
    );

    let val2 = node.gen_single_3d(5.0, 3.0, -2.0, 1337);
    assert!(
      val2.abs() < 1e-6,
      "Displacement should be 0 at any point with zero clamps, got {}",
      val2
    );
  }
}
