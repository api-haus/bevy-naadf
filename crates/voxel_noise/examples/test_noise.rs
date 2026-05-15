use voxel_noise::{NoiseNode, NoisePreset, PlanetNoiseParams};

fn main() {
  // Test with SimpleTerrain preset
  println!("Testing SimpleTerrain preset...");
  let node = NoiseNode::from_preset(NoisePreset::SimpleTerrain as u32).unwrap();
  let mut output = vec![0.0f32; 8];
  node.gen_uniform_grid_3d(&mut output, 0.0, 0.0, 0.0, 2, 2, 2, 1.0, 1.0, 1.0, 1337);
  println!("SimpleTerrain output: {:?}", output);

  // Test PlanetTerrain preset (unified SDF graph with DistanceToPoint)
  println!("\nTesting PlanetTerrain preset (unified SDF)...");
  let node2 = NoiseNode::from_preset(NoisePreset::PlanetTerrain as u32).unwrap();
  let mut planet_output = vec![0.0f32; 32 * 32 * 32];
  for offset in [0.0f32, 3.0, 6.0, -3.0, -6.0] {
    node2.gen_uniform_grid_3d(
      &mut planet_output,
      offset,
      offset,
      offset,
      32,
      32,
      32,
      0.1,
      0.1,
      0.1,
      1337,
    );
    let min = planet_output.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = planet_output
      .iter()
      .cloned()
      .fold(f32::NEG_INFINITY, f32::max);
    let mean: f32 = planet_output.iter().sum::<f32>() / planet_output.len() as f32;
    println!(
      "  offset={:>5.0}: min={:>8.4}, max={:>8.4}, mean={:>8.4}, range={:>8.4}",
      offset,
      min,
      max,
      mean,
      max - min
    );
  }

  // Test custom planet params (graph outputs displacement, not SDF)
  println!("\nTesting custom PlanetNoiseParams (displacement output)...");
  let params = PlanetNoiseParams::new(0.8, 0.3, 3.0, 0.5, 0.40);
  let custom_node = NoiseNode::from_safe_node(voxel_noise::build_planet_sdf(&params));
  let disp_origin = custom_node.gen_single_3d(0.0, 0.0, 0.0, 1337);
  let disp_off = custom_node.gen_single_3d(5.0, 3.0, -2.0, 1337);
  println!(
    "  displacement at origin: {:.4}, at (5,3,-2): {:.4}",
    disp_origin, disp_off
  );

  // Try 2D mode
  println!("\nTesting 2D grid...");
  let mut output_2d = vec![0.0f32; 4];
  node.gen_uniform_grid_2d(&mut output_2d, 0.0, 0.0, 2, 2, 0.1, 0.1, 1337);
  println!("2D output: {:?}", output_2d);
}
