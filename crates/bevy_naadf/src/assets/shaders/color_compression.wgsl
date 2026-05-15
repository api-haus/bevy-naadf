// color_compression.wgsl — the 5-bit/channel exponential GI-sample colour
// compression.
//
// Derives from: render/common/commonColorCompression.fxh (`09-design-b.md`
// §5.3). **Phase-B-only** — `02-research.md` §5.1 tags
// `commonColorCompression.fxh` Phase B. Not yet imported by any entry shader in
// Batch 1; Batch 3+ (`renderGlobalIllum`, `renderSampleRefine`) consume it.
//
// THE `COLORS` / `COLOR_DIF_PROB` TABLES ARE HARD-CODED LITERALS. The HLSL
// builds them from `pow()` const-expressions (`COLOR_EXP = pow(2, 0.6)`); WGSL
// `const` arrays cannot hold `pow()` results (`pow` is not const-evaluable in
// WGSL). So the 32 + 31 values are computed on the CPU and pasted here as f32
// literals. `src/render/color_compression.rs` recomputes them from the source
// formula and a Rust `#[test]` (`color_tables_match_wgsl`) asserts the literals
// below match — the same discipline as `gpu_types.rs`'s size asserts. If you
// regenerate these, update that test's expectations too (or vice-versa — they
// must agree).
//
//   COLOR_EXP   = 2^0.6              = 1.515716566510398
//   COLOR_START = 1 / 64             = 0.015625
//   COLORS[0]   = 0
//   COLORS[i]   = COLOR_START * COLOR_EXP^(i-1)   for i in 1..=31
//   COLOR_DIF_PROB[i] = 1 - COLOR_EXP^(-i)        for i in 0..=30
//
// naga-oil import module.

#import "shaders/ray_tracing_common.wgsl"::next_rand

// `MAX_COLOR_LEVELING` (HLSL `#define MAX_COLOR_LEVELING 20`).
const MAX_COLOR_LEVELING: u32 = 20u;

// The 5-bit-index → f32-colour LUT (HLSL `static const float COLORS[32]`).
const COLORS: array<f32, 32> = array<f32, 32>(
    0.0,
    0.015625,
    0.02368307113647461,
    0.035896822810173035,
    0.05440941080451012,
    0.08246924728155136,
    0.125,
    0.18946456909179688,
    0.2871745824813843,
    0.43527528643608093,
    0.6597539782524109,
    1.0,
    1.515716552734375,
    2.297396659851074,
    3.4822022914886475,
    5.278031826019287,
    8.0,
    12.125732421875,
    18.379173278808594,
    27.85761833190918,
    42.2242546081543,
    64.0,
    97.005859375,
    147.03338623046875,
    222.86094665527344,
    337.7940368652344,
    512.0,
    776.046875,
    1176.26708984375,
    1782.8875732421875,
    2702.352294921875,
    4096.0,
);

// The exponential-difference removal-probability LUT
// (HLSL `static const float COLOR_DIF_PROB[31]`).
const COLOR_DIF_PROB: array<f32, 31> = array<f32, 31>(
    0.0,
    0.3402460515499115,
    0.5647247433662415,
    0.7128254175186157,
    0.8105354309082031,
    0.875,
    0.9175307750701904,
    0.945590615272522,
    0.9641031622886658,
    0.9763169288635254,
    0.984375,
    0.9896913170814514,
    0.993198812007904,
    0.9955129027366638,
    0.9970396161079407,
    0.998046875,
    0.9987114071846008,
    0.9991498589515686,
    0.9994391202926636,
    0.9996299743652344,
    0.999755859375,
    0.9998389482498169,
    0.9998937249183655,
    0.9999299049377441,
    0.9999537467956543,
    0.999969482421875,
    0.9999798536300659,
    0.9999867081642151,
    0.999991238117218,
    0.9999942183494568,
    0.9999961853027344,
);

// `refineCompColor` — stochastically round a compressed colour index down one
// level so it converges to `actual_color` in expectation (HLSL
// `refineCompColor`). WGSL has no `inout` params; this takes `comp_color` +
// `rand` by pointer.
fn refine_comp_color(
    comp_color: ptr<function, u32>,
    rand: ptr<function, vec2<u32>>,
    actual_color: f32,
) {
    let cur_col = COLORS[*comp_color];
    // HLSL `COLORS[compColor - 1]` — `comp_color` is always >= 1 at the call
    // sites (`compress_color` starts the index at `1 + firstLeadingBit(...)`).
    let prev_col = COLORS[*comp_color - 1u];
    if ((actual_color - prev_col) / (cur_col - prev_col) < next_rand(rand)) {
        *comp_color = *comp_color - 1u;
    }
}

// `compressColor` — compress an RGB colour into the 5-bit/channel exponential
// format (HLSL `compressColor`). The HLSL `firstbithigh` is WGSL
// `firstLeadingBit` (same semantics for the `max(1, ...)`-guarded non-zero
// inputs here).
fn compress_color(color_in: vec3<f32>, rand: ptr<function, vec2<u32>>) -> u32 {
    let color = min(color_in, vec3<f32>(COLORS[31]));
    let color_sq = pow(abs(color * 64.0), vec3<f32>(1.6666666));
    var comp_color_r = 1u + firstLeadingBit(max(1u, u32(color_sq.r) * 2u));
    var comp_color_g = 1u + firstLeadingBit(max(1u, u32(color_sq.g) * 2u));
    var comp_color_b = 1u + firstLeadingBit(max(1u, u32(color_sq.b) * 2u));

    refine_comp_color(&comp_color_r, rand, color.r);
    refine_comp_color(&comp_color_g, rand, color.g);
    refine_comp_color(&comp_color_b, rand, color.b);

    return comp_color_r | (comp_color_g << 5u) | (comp_color_b << 10u);
}
