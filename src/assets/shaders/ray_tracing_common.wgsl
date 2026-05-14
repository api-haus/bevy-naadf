// ray_tracing_common.wgsl — RNG + octahedral normal encode/decode.
//
// Derives from: render/common/commonRayTracing.fxh (`03-design.md` §5.5).
// **Phase-A subset** — the PCG / xoroshiro RNG and octahedral encode/decode are
// ported now; VNDF-GGX importance sampling, hemisphere sampling, and the
// geometry term are Phase B (`02-research.md` §5.5 — this header splits A/B).
// The quaternion (de)compress is also Phase B (entity / GI use).
//
// naga-oil import module.

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
