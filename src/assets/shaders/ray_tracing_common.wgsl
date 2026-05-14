// ray_tracing_common.wgsl — RNG + octahedral normal encode/decode + the
// Phase-B VNDF-GGX importance-sampling block.
//
// Derives from: render/common/commonRayTracing.fxh (`03-design.md` §5.5,
// `09-design-b.md` §2.2 / §5.1).
// The PCG / xoroshiro RNG and octahedral encode/decode are Phase A; the
// VNDF-GGX importance sampling, the uniform-hemisphere sample, the
// perpendicular-vector helper, and the geometry term are Phase B
// (`02-research.md` §5.5 — this header splits A/B; `commonRayTracing.fxh:65-137`).
// The quaternion (de)compress stays un-ported — entity-only.
//
// naga-oil import module.

#import "shaders/common.wgsl"::PI

// --- PCG / xoroshiro64* RNG (commonRayTracing.fxh) --------------------------

// https://jcgt.org/published/0009/03/02/ — `pcg_hash`.
fn pcg_hash(input: u32) -> u32 {
    let state = input * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// `initRand` — seed a xoroshiro64* state from a 3-component key (HLSL
// `initRand(uint3 data)`).
fn init_rand(data: vec3<u32>) -> vec2<u32> {
    let seed_x = pcg_hash(data.x + pcg_hash(data.y + data.z));
    let seed_y = pcg_hash(seed_x + data.z);
    var rng: vec2<u32>;
    rng.x = seed_x;
    rng.y = select(seed_y, 0xa7e2bf31u, seed_y == 0u);
    return rng;
}

// `rotl` — rotate-left (HLSL `rotl`).
fn rotl(x: u32, k: u32) -> u32 {
    return (x << k) | (x >> (32u - k));
}

// xoroshiro64* 1.0 — 32-bit generator. `state` is updated in place; returns the
// next raw `u32` (HLSL `xoroshiro64star`, `inout uint2 state`).
fn xoroshiro64star(state: ptr<function, vec2<u32>>) -> u32 {
    let s0 = (*state).x;
    var s1 = (*state).y;
    let result = s0 * 0x9E3779BBu;
    s1 = s1 ^ s0;
    (*state).x = rotl(s0, 26u) ^ s1 ^ (s1 << 9u);
    (*state).y = rotl(s1, 13u);
    return result;
}

// `nextRand` — next uniform `f32` in `[0,1)` (HLSL `nextRand`). The magic
// constant is `1.0 / 2^32`.
fn next_rand(state: ptr<function, vec2<u32>>) -> f32 {
    return f32(xoroshiro64star(state)) * 2.3283064365387e-10;
}

// `nextRand2` — two uniform `f32`s in `[0,1)` (HLSL `nextRand2`).
fn next_rand2(state: ptr<function, vec2<u32>>) -> vec2<f32> {
    return vec2<f32>(next_rand(state), next_rand(state));
}

// --- octahedral normal encode / decode (commonRayTracing.fxh) ---------------

// https://knarkowicz.wordpress.com/2014/04/16/octahedron-normal-vector-encoding/
fn oct_wrap(v: vec2<f32>) -> vec2<f32> {
    let s = select(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), v >= vec2<f32>(0.0, 0.0));
    return (vec2<f32>(1.0, 1.0) - abs(v.yx)) * s;
}

// `octEncode` — encode a unit normal into a `[0,1]²` octahedral coordinate.
fn oct_encode(n_in: vec3<f32>) -> vec2<f32> {
    var n = n_in / (abs(n_in.x) + abs(n_in.y) + abs(n_in.z));
    var nxy = select(oct_wrap(n.xy), n.xy, n.z >= 0.0);
    nxy = nxy * 0.5 + vec2<f32>(0.5, 0.5);
    return nxy;
}

// `octDecode` — decode a `[0,1]²` octahedral coordinate back to a unit normal.
fn oct_decode(f_in: vec2<f32>) -> vec3<f32> {
    let f = f_in * 2.0 - vec2<f32>(1.0, 1.0);
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    let t = clamp(-n.z, 0.0, 1.0);
    let adjust = select(vec2<f32>(t, t), vec2<f32>(-t, -t), n.xy >= vec2<f32>(0.0, 0.0));
    n = vec3<f32>(n.xy + adjust, n.z);
    return normalize(n);
}

// --- Phase-B VNDF-GGX importance sampling (commonRayTracing.fxh:65-137) ------

// `getPerpendicularVector` — a branch-free perpendicular vector of `u`. From
// "Efficient Construction of Perpendicular Vectors Without Branching"
// (HLSL `getPerpendicularVector`).
fn get_perpendicular_vector(u: vec3<f32>) -> vec3<f32> {
    let a = abs(u);
    let xm = select(0u, 1u, (a.x - a.y) < 0.0 && (a.x - a.z) < 0.0);
    let ym = select(0u, 1u ^ xm, (a.y - a.z) < 0.0);
    let zm = 1u ^ (xm | ym);
    return cross(u, vec3<f32>(f32(xm), f32(ym), f32(zm)));
}

// `getUniformHemisphereSample` — uniform hemisphere sample around `hit_norm`,
// optionally narrowed by `deviation` (HLSL `getUniformHemisphereSample`).
fn get_uniform_hemisphere_sample(
    rand: vec2<f32>,
    hit_norm: vec3<f32>,
    deviation: f32,
) -> vec3<f32> {
    let bitangent = get_perpendicular_vector(hit_norm);
    let tangent = cross(bitangent, hit_norm);
    let z = deviation + (1.0 - deviation) * rand.x;
    let r = sqrt(1.0 - z * z);
    let phi = 2.0 * PI * rand.y;
    return normalize(
        tangent * (r * cos(phi)) + bitangent * (r * sin(phi)) + hit_norm * z
    );
}

// `sample_vndf_isotropic` — importance-sample a VNDF (GGX-Smith) isotropic
// distribution (HLSL `sample_vndf_isotropic`). `saturate` → `clamp(_,0,1)`.
fn sample_vndf_isotropic(
    u: vec2<f32>,
    wi: vec3<f32>,
    alpha: f32,
    n: vec3<f32>,
) -> vec3<f32> {
    // decompose the vector in parallel and perpendicular components
    let wi_z = -n * dot(wi, n);
    let wi_xy = wi + wi_z;

    // warp to the hemisphere configuration
    let wi_std = -normalize(alpha * wi_xy + wi_z);

    // sample a spherical cap in (-wiStd.z, 1]
    let wi_std_z = dot(wi_std, n);
    let z = 1.0 - u.y * (1.0 + wi_std_z);
    let sin_theta = sqrt(clamp(1.0 - z * z, 0.0, 1.0));
    let phi = 2.0 * PI * u.x - PI;
    let x = sin_theta * cos(phi);
    let y = sin_theta * sin(phi);
    let c_std = vec3<f32>(x, y, z);

    // reflect sample to align with normal
    let up = vec3<f32>(0.0, 0.0, 1.000001); // for the singularity
    let wr = n + up;
    let c = dot(wr, c_std) * wr / wr.z - c_std;

    // compute halfway direction as standard normal
    let wm_std = c + wi_std;
    let wm_std_z = n * dot(n, wm_std);
    let wm_std_xy = wm_std_z - wm_std;

    // return final normal
    return normalize(alpha * wm_std_xy + wm_std_z);
}

// `pdf_vndf_isotropic` — the VNDF isotropic-distribution pdf (HLSL
// `pdf_vndf_isotropic`). `rsqrt` → `inverseSqrt`.
fn pdf_vndf_isotropic(
    wo: vec3<f32>,
    wi: vec3<f32>,
    alpha: f32,
    n: vec3<f32>,
) -> f32 {
    let alpha_square = alpha * alpha;
    let wm = normalize(wo + wi);
    let zm = dot(wm, n);
    let zi = dot(wi, n);
    let nrm = inverseSqrt((zi * zi) * (1.0 - alpha_square) + alpha_square);
    let sigma_std = (zi * nrm) * 0.5 + 0.5;
    let sigma_i = sigma_std / nrm;
    let nrm_n = (zm * zm) * (alpha_square - 1.0) + 1.0;
    return alpha_square / (PI * 4.0 * nrm_n * nrm_n * sigma_i);
}

// `geometryTerm` — the Smith geometry term (HLSL `geometryTerm`).
fn geometry_term(roughness: f32, cos_theta: f32) -> f32 {
    return (2.0 * cos_theta)
        / (cos_theta
            + sqrt(roughness * roughness
                + (1.0 - roughness * roughness) * cos_theta * cos_theta));
}
