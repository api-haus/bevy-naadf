use fastnoise2::generator::prelude::*;

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
  println!("SimpleTerrain components:\n");

  // SuperSimplex baseline
  let simple = supersimplex().build().0;
  test_range("SuperSimplex baseline", &simple);

  // SuperSimplex + FBm
  let fbm = supersimplex().fbm(0.65, 0.5, 4, 2.5).build().0;
  test_range("SuperSimplex + FBm(0.65, 0.5, 4, 2.5)", &fbm);

  // FBm + domain_scale
  let fbm_scaled = supersimplex()
    .fbm(0.65, 0.5, 4, 2.5)
    .domain_scale(0.66)
    .build()
    .0;
  test_range("FBm + domain_scale(0.66)", &fbm_scaled);

  // Gradient
  let grad = gradient().with_multipliers([0.0, 3.0, 0.0, 0.0]).build().0;
  test_range("Gradient with_multipliers([0,3,0,0])", &grad);

  // FBm + Gradient (addition)
  let fbm_grad = (supersimplex().fbm(0.65, 0.5, 4, 2.5).domain_scale(0.66)
    + gradient().with_multipliers([0.0, 3.0, 0.0, 0.0]))
  .build()
  .0;
  test_range("(FBm + domain_scale) + Gradient", &fbm_grad);

  // With domain warping
  let full = (supersimplex().fbm(0.65, 0.5, 4, 2.5).domain_scale(0.66)
    + gradient().with_multipliers([0.0, 3.0, 0.0, 0.0]))
  .domain_warp_gradient(0.2, 2.0)
  .domain_warp_progressive(0.7, 0.5, 2, 2.5)
  .build()
  .0;
  test_range("Full SimpleTerrain", &full);
}
