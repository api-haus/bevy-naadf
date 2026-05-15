//! FastNoise2 noise generation with native and WASM support.
//!
//! This crate provides a Rust wrapper around FastNoise2 C++ library via FFI.
//! Both native and WASM (Emscripten) builds use the same core implementation.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     native.rs                               │
//! │  ┌───────────────────────────────────────────────────────┐  │
//! │  │ NoiseNode (Rust API)                                  │  │
//! │  │   - from_encoded()                                    │  │
//! │  │   - from_preset()                                     │  │
//! │  │   - gen_uniform_grid_3d()                             │  │
//! │  │   - gen_uniform_grid_2d()                             │  │
//! │  │   - gen_single_3d()                                   │  │
//! │  └───────────────────────────────────────────────────────┘  │
//! │  ┌───────────────────────────────────────────────────────┐  │
//! │  │ wasm_api (C-ABI exports, wasm32 only)                 │  │
//! │  │   - vx_noise_create()                                 │  │
//! │  │   - vx_noise_create_preset()                          │  │
//! │  │   - vx_noise_gen_3d()                                 │  │
//! │  │   - vx_noise_gen_2d()                                 │  │
//! │  │   - vx_noise_gen_single_3d()                          │  │
//! │  │   - vx_noise_destroy()                                │  │
//! │  └───────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage (Native)
//! ```ignore
//! use voxel_noise::{NoiseNode, NoisePreset};
//!
//! let node = NoiseNode::from_preset(NoisePreset::SimpleTerrain as u32).unwrap();
//! let mut output = vec![0.0f32; 32 * 32 * 32];
//! node.gen_uniform_grid_3d(&mut output, 0.0, 0.0, 0.0, 32, 32, 32, 0.02, 0.02, 0.02, 1337);
//! ```
//!
//! # WASM Emscripten Build
//! ```bash
//! cd crates/voxel_noise && make build
//! ```
//! Produces `dist/voxel_noise.js` + `dist/voxel_noise.wasm`.
//! The JS bridge (`js/voxel_noise_bridge.js`) wraps these exports.

mod native;
pub use native::NoiseNode;

// Composable noise presets using typed generator API
pub mod presets;
// Re-export wasm_api for Emscripten builds
#[cfg(all(target_arch = "wasm32", target_os = "emscripten"))]
pub use native::wasm_api;
pub use presets::{build_planet_sdf, build_preset, NoisePreset, PlanetNoiseParams};

/// Legacy encoded node tree presets (from FastNoise2 NoiseTool).
/// Prefer using `presets::build_preset()` or `NoiseNode::from_preset()`
/// instead.
pub mod encoded_presets {
  /// Simple terrain noise - FBm with domain warp (from NoiseTool built-in
  /// "Simple Terrain")
  #[deprecated(note = "Use NoiseNode::from_preset(NoisePreset::SimpleTerrain as u32) instead")]
  pub const SIMPLE_TERRAIN: &str =
    "E@BBZEE@BD8JFgIECArXIzwECiQIw/UoPwkuAAE@BJDQAE@BC@AIEAJBwQDZmYmPwsAAIA/HAMAAHBCBA==";
}

#[cfg(test)]
mod tests {
  use super::{NoiseNode, NoisePreset};

  #[test]
  fn gen_single_3d_matches_grid() {
    let node = NoiseNode::from_preset(NoisePreset::SimpleTerrain as u32)
      .expect("Failed to create noise node from preset");

    let seed = 1337;

    // Test several points: gen_single_3d should match gen_uniform_grid_3d
    // with a 1x1x1 grid at the same coordinates.
    let test_points: [(f32, f32, f32); 5] = [
      (0.0, 0.0, 0.0),
      (1.5, 2.3, -0.7),
      (10.0, 20.0, 30.0),
      (-5.0, 0.0, 5.0),
      (100.0, -50.0, 25.0),
    ];

    for (x, y, z) in test_points {
      let single = node.gen_single_3d(x, y, z, seed);

      let mut grid = [0.0f32; 1];
      node.gen_uniform_grid_3d(&mut grid, x, y, z, 1, 1, 1, 1.0, 1.0, 1.0, seed);

      assert!(
        (single - grid[0]).abs() < 1e-6,
        "gen_single_3d({},{},{}) = {} but gen_uniform_grid_3d = {} (diff={})",
        x,
        y,
        z,
        single,
        grid[0],
        (single - grid[0]).abs()
      );
    }
  }

  #[test]
  fn test_simple_terrain() {
    let node = NoiseNode::from_preset(NoisePreset::SimpleTerrain as u32)
      .expect("Failed to create noise node from preset");
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
  fn test_2d_grid() {
    let node = NoiseNode::from_preset(NoisePreset::SimpleTerrain as u32)
      .expect("Failed to create noise node from preset");
    let mut output = vec![0.0f32; 32 * 32];
    node.gen_uniform_grid_2d(&mut output, 0.0, 0.0, 32, 32, 0.02, 0.02, 1337);
    assert!(output.iter().any(|&v| v != 0.0), "All values are zero");
  }

  /// Test that adjacent chunks produce identical values at their shared edge.
  /// This is the critical test for chunk boundary coherency.
  #[test]
  fn test_adjacent_chunk_edge_coherency() {
    let node = NoiseNode::from_preset(NoisePreset::SimpleTerrain as u32)
      .expect("Failed to create noise node from preset");

    const SIZE: usize = 32;
    const VOXEL_SIZE: f32 = 1.0;
    let seed = 1337;

    // Chunk A at origin
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

    // Chunk B adjacent in X (starts at x=28, so it overlaps with chunk A's last 4
    // samples) Note: actual chunk boundary is at sample 28 for 32-sample chunks
    // with 28 voxels per cell
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

    // Compare overlapping edge samples
    // Chunk A's samples at x=28..31 should equal chunk B's samples at x=0..3
    // FastNoise2 layout: index = z * SIZE² + y * SIZE + x (X-fastest)
    let mut mismatches = 0;
    let mut max_diff: f32 = 0.0;

    for y in 0..SIZE {
      for z in 0..SIZE {
        for overlap_idx in 0..4 {
          let a_x = 28 + overlap_idx;
          let b_x = overlap_idx;

          // FastNoise2 X-fastest index
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
      "Found {} edge sample mismatches between adjacent chunks (max diff: {})",
      mismatches, max_diff
    );
  }

  /// Test edge coherency at sub-voxel sizes (< 1.0)
  #[test]
  fn test_edge_coherency_small_voxel_size() {
    let node = NoiseNode::from_preset(NoisePreset::SimpleTerrain as u32)
      .expect("Failed to create noise node from preset");

    const SIZE: usize = 32;
    const VOXEL_SIZE: f32 = 0.25; // Small voxel size that was causing issues
    let seed = 1337;

    // Chunk A at origin
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

    // Chunk B adjacent in X
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

    // Compare overlapping edge samples
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
            if mismatches <= 5 {
              eprintln!(
                "Mismatch at overlap_idx={}, y={}, z={}: a={}, b={}, diff={}",
                overlap_idx, y, z, chunk_a[a_idx], chunk_b[b_idx], diff
              );
            }
          }
        }
      }
    }

    assert_eq!(
      mismatches, 0,
      "Found {} edge sample mismatches at voxel_size={} (max diff: {})",
      mismatches, VOXEL_SIZE, max_diff
    );
  }
}
