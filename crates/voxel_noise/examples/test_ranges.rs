use fastnoise2::generator::cellular::CellularDistanceReturnType;
use fastnoise2::generator::prelude::*;
use fastnoise2::generator::DistanceFunction;

fn test_range(name: &str, node: &fastnoise2::SafeNode) {
  let mut output = vec![0.0f32; 1024];
  node.gen_uniform_grid_3d(&mut output, 0.0, 0.0, 0.0, 16, 8, 8, 0.1, 0.1, 0.1, 1337);
  let min = output.iter().fold(f32::INFINITY, |a, &b| a.min(b));
  let max = output.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
  let avg = output.iter().sum::<f32>() / output.len() as f32;
  println!(
    "{}: min={:.4}, max={:.4}, avg={:.4}, range={:.4}",
    name,
    min,
    max,
    avg,
    max - min
  );
}

fn main() {
  println!("Testing output ranges at larger scale:\n");

  // Test baseline SuperSimplex
  let simple = supersimplex().build().0;
  test_range("SuperSimplex baseline", &simple);

  // FBm with 0 weighted_strength (should be pure FBm)
  let fbm = supersimplex().fbm(0.5, 0.0, 5, 2.0).build().0;
  test_range("FBm(0.5, 0.0, 5, 2.0)", &fbm);

  // FBm with domain scale
  let fbm_scaled = supersimplex()
    .fbm(0.5, 0.0, 5, 2.0)
    .domain_scale(0.66)
    .build()
    .0;
  test_range("FBm + domain_scale(0.66)", &fbm_scaled);

  // Ridged
  let ridged = supersimplex().ridged(0.5, 0.0, 4, 2.0).build().0;
  test_range("Ridged(0.5, 0.0, 4, 2.0)", &ridged);

  // Ridged with domain scale
  let ridged_scaled = supersimplex()
    .ridged(0.5, 0.0, 4, 2.0)
    .domain_scale(0.5)
    .build()
    .0;
  test_range("Ridged + domain_scale(0.5)", &ridged_scaled);

  // Full planet_terrain preset
  println!("\nPlanet terrain preset (wide sampling):\n");

  let continents = supersimplex()
    .fbm(0.5, 0.0, 6, 2.0)
    .domain_warp_gradient(0.3, 2.0);
  let archipelago = cellular_distance(
    1.0,
    DistanceFunction::EuclideanSquared,
    0,
    1,
    CellularDistanceReturnType::Index0Sub1,
  )
  .domain_scale(0.3)
  .remap(-1.0, 1.0, -0.6, 0.4);
  let ridges2 = supersimplex()
    .seed_offset(42)
    .ridged(0.5, 0.0, 4, 2.0)
    .domain_scale(3.0);
  let landmass = continents
    .max_smooth(archipelago, 0.15)
    .domain_warp_gradient(0.15, 3.0);
  let surface = supersimplex().fbm(0.5, 0.0, 4, 2.0).domain_scale(2000.0);
  let node = (landmass + ridges2 * 0.20 + surface * 0.05).build().0;

  // Sample across noise-coordinate space [-7, 7] with multiple seeds
  let size = 64;
  let total = size * size * size;
  let mut output = vec![0.0f32; total];
  let mut global_min = f32::INFINITY;
  let mut global_max = f32::NEG_INFINITY;
  let mut sum = 0.0f64;
  let mut count = 0u64;

  let bases: Vec<f32> = (-7..=7).map(|i| i as f32).collect();
  for seed in [1337, 42, 999, 0, 7777] {
    for &bx in &bases {
      for &by in &bases {
        for &bz in &bases {
          let min_max = node.gen_uniform_grid_3d(
            &mut output,
            bx,
            by,
            bz,
            size as i32,
            size as i32,
            size as i32,
            0.05,
            0.05,
            0.05,
            seed,
          );
          global_min = global_min.min(min_max.min);
          global_max = global_max.max(min_max.max);
          sum += output.iter().map(|&v| v as f64).sum::<f64>();
          count += total as u64;
        }
      }
    }
  }

  let mean = sum / count as f64;
  println!(
    "planet_terrain: min={:.4}, max={:.4}, mean={:.4}, range={:.4} ({} samples)",
    global_min,
    global_max,
    mean,
    global_max - global_min,
    count
  );
}
