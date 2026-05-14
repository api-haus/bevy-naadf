// common.wgsl — shared constants + helpers.
//
// Derives from: render/common/common.fxh + commonConstants.fxh + settings.fxh +
// commonOther.fxh (`03-design.md` §5.5, `09-design-b.md` §2.2).
//
// HLSL `common.fxh` is just an umbrella include + the `FLATTEN_INDEX` macro;
// `commonConstants.fxh` is `PI`; `settings.fxh` is the build flags + the
// `CHUNKTYPE` choice. Phase A is entity-free, so `CHUNKTYPE` is `u32`
// (`03-design.md` §7.5) — the chunk texture is `texture_3d<u32>` and that
// choice lives in `world_data.wgsl`, not here.
//
// Phase B adds the `commonOther.fxh` pure-math helpers (`gaussian_f`, `gcd`,
// `find_coprime`, `next_pow2`) — the group-shared counter helpers
// (`addToCounter*`) from that header are NOT shared functions (they need
// `var<workgroup>` at entry-point scope) and are ported inline per-shader.
//
// WGSL has no `#include`; this is a naga-oil import module — other shaders
// pull symbols in via `#import "shaders/common.wgsl"::{...}`.

// Pi (HLSL `commonConstants.fxh` `PI`).
const PI: f32 = 3.141592653589793;

// Flatten a 3D position into a 1D index, x-fastest then y then z.
//
// HLSL `common.fxh`:
//   #define FLATTEN_INDEX(pos, sy, sz) mad(pos.z, sz, mad(pos.y, sy, pos.x))
//
// NAADF calls this with `(blockPosInChunk, 4, 16)` and
// `(voxelPosInBlock, 4, 16)` — note the *second* stride argument is the
// y-stride (4) and the *third* is the z-stride (16), i.e. for a 4×4×4 cell
// `flatten_index(p, 4u, 16u)`.
fn flatten_index(pos: vec3<u32>, stride_y: u32, stride_z: u32) -> u32 {
    return pos.z * stride_z + pos.y * stride_y + pos.x;
}

// --- commonOther.fxh pure-math helpers (Phase B) ---------------------------

// `gaussianF` — un-normalised-ish Gaussian weight (HLSL `gaussianF`). Used by
// the sparse bilateral denoiser (`renderDenoiseSplit.fx`).
fn gaussian_f(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.0 * PI * sigma * sigma);
}

// `gcd` — greatest common divisor (HLSL `gcd`).
fn gcd(a_in: u32, b_in: u32) -> u32 {
    var a = a_in;
    var b = b_in;
    while (b != 0u) {
        let t = b;
        b = a % b;
        a = t;
    }
    return a;
}

// `findCoprime` — the smallest odd number `>= (seed | 1)` coprime with `n`
// (HLSL `findCoprime`). Used by the `ShuffleGroup` coprime stride.
fn find_coprime(n: u32, seed: u32) -> u32 {
    var a = seed | 1u;
    while (gcd(a, n) != 1u) {
        a += 2u;
    }
    return a;
}

// `nextPow2` — the smallest power of two `>= v` (HLSL `nextPow2`).
fn next_pow2(v_in: u32) -> u32 {
    if (v_in <= 1u) {
        return 1u;
    }
    var v = v_in - 1u;
    v |= v >> 1u;
    v |= v >> 2u;
    v |= v >> 4u;
    v |= v >> 8u;
    v |= v >> 16u;
    return v + 1u;
}
