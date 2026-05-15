// taa_common.wgsl — the 64-bit TAA sample format + the long-term-TAA shared
// constants.
//
// Derives from: render/common/taa/commonTaa.fxh (`06-design-a2.md` §3, §10.1).
// A faithful WGSL port of `commonTaa.fxh`'s `compressSample` / `decompressSample`
// (the 64-bit `uint2` sample format), `getHashFromData`, and `neighborOffsets[9]`,
// plus the 16-deep sample-ring depth constant (the `01-context.md` §2c / §6
// VRAM lever: the sample ring is 16-deep, not NAADF's 32).
//
// NOTE — this is the TAA *sample* compression (8-bit/channel exponential colour,
// from `commonTaa.fxh`). The 5-bit/channel ReSTIR-GI sample compression
// (`commonColorCompression.fxh`) is a separate Phase-B file — NOT this one
// (`06-design-a2.md` §13.1).
//
// naga-oil import module.

// The TAA sample ring depth — 16, NOT NAADF's 32 (the `01-context.md` §6 VRAM
// lever). Every `% 32` / `* 32` in the HLSL `taaSamples` indexing becomes
// `% TAA_SAMPLE_RING_DEPTH` / `* TAA_SAMPLE_RING_DEPTH`.
const TAA_SAMPLE_RING_DEPTH: u32 = 16u;

// The 3×3 neighbour offsets, in `commonTaa.fxh:6-18` order: centre first, then
// the 4-neighbourhood, then the 4 diagonals. The reproject pass walks these for
// the per-pixel min/max-distance + hash precompute (`06-design-a2.md` §7.2).
const taa_neighbor_offsets: array<vec2<i32>, 9> = array<vec2<i32>, 9>(
    vec2<i32>(0, 0),
    vec2<i32>(0, -1),
    vec2<i32>(-1, 0),
    vec2<i32>(1, 0),
    vec2<i32>(0, 1),

    vec2<i32>(-1, -1),
    vec2<i32>(1, -1),
    vec2<i32>(-1, 1),
    vec2<i32>(1, 1),
);

// `getHashFromData` — the surface-classification hash (HLSL
// `commonTaa.fxh:20-28`). In Phase A's plane-0-only, entity-free, all-diffuse
// world this collapses to a single constant value for every hit pixel — but it
// is ported faithfully (it is cheap and Phase B needs it varying). Used by the
// reproject pass's hash reject test.
fn taa_hash_from_data(is_diffuse: u32, specular_normals: u32, entity: u32) -> u32 {
    var hash = is_diffuse | (entity << 1u) | (specular_normals << 15u);
    hash = hash ^ (hash >> 17u);
    hash = hash * 0xed5ad4bbu;
    hash = hash ^ (hash >> 11u);
    hash = hash * 0xac4c1b51u;
    return hash;
}

// The decompressed 64-bit TAA sample (HLSL `decompressSample`'s out-params).
struct TaaSample {
    // Primary-ray hit distance (f16-precision; `65520` ≈ f16 max for a miss).
    dist: f32,
    // The decompressed colour. `.a` is ALWAYS `1.0` — the per-sample "this
    // sample counts as 1" weight the accumulation sums (`06-design-a2.md`
    // §3.2 — load-bearing for the 0.25-spp signal, do not drop it).
    color: vec4<f32>,
    // 3-bit normal-LUT index (`NORMAL[]` index).
    normal_comp: u32,
    // 5-bit material roughness; always `0` in the albedo path.
    extra_data: u32,
    // The surface-classification hash (`taa_hash_from_data` of the sample).
    hash: u32,
}

// `compressSample` — pack a TAA sample into the 64-bit `vec2<u32>` format
// (HLSL `commonTaa.fxh:30-43`).
//
// `dist` is the float hit distance — this helper does the `f32tof16` (HLSL's
// caller `renderFirstHit.fx:115` passes the f16 bits, but folding the
// conversion in keeps the call site clean and matches `06-design-a2.md` §6.1).
fn taa_compress_sample(
    dist: f32,
    color_in: vec3<f32>,
    normal_comp: u32,
    is_diffuse: u32,
    specular_normals: u32,
    extra_data: u32,
    entity: u32,
) -> vec2<u32> {
    // `f32tof16(dist)` — WGSL has no f16 scalar builtin; pack into the low half.
    let dist_comp = pack2x16float(vec2<f32>(dist, 0.0)) & 0xFFFFu;

    // Clamp the colour to `[0, 100]` before the exponential compression
    // (`commonTaa.fxh:32-34`).
    var color = color_in;
    let max_color_channel = max(color.r, max(color.g, color.b));
    if (max_color_channel > 100.0) {
        color = color * (100.0 / max_color_channel);
    }
    // Exponential colour compression — 8 bits/channel (`commonTaa.fxh:35`).
    // HLSL casts the float result to `uint3` (truncation); WGSL has no implicit
    // float→uint truncation, so `u32(...)` explicitly and `min(255u, ...)` /
    // `& 0xFFu` per channel (`06-design-a2.md` §3.2 implementer note).
    let color_comp_f = 12.0 * log2(color + pow(vec3<f32>(2.0), vec3<f32>(-255.0 / 12.0)) * 100.0)
        + (255.0 - 12.0 * log2(100.0));
    let color_comp = min(vec3<u32>(255u), vec3<u32>(max(color_comp_f, vec3<f32>(0.0))));

    let hash = taa_hash_from_data(is_diffuse, specular_normals, entity);

    var sample_comp = vec2<u32>(0u, 0u);
    sample_comp.x = dist_comp | ((hash & 0xFFFFu) << 16u);
    sample_comp.y = (color_comp.x & 0xFFu)
        | ((color_comp.y & 0xFFu) << 8u)
        | ((color_comp.z & 0xFFu) << 16u)
        | ((normal_comp & 0x7u) << 24u)
        | ((extra_data & 0x1Fu) << 27u);
    return sample_comp;
}

// `decompressSample` — unpack a 64-bit `vec2<u32>` TAA sample (HLSL
// `commonTaa.fxh:45-53`).
fn taa_decompress_sample(sample_comp: vec2<u32>) -> TaaSample {
    var s: TaaSample;
    // `f16tof32(sampleComp.x & 0x7FFF)` — the `& 0x7FFF` masks the f16 sign bit
    // (distance is always positive).
    s.dist = unpack2x16float(sample_comp.x & 0x7FFFu).x;
    let col_comp = vec3<f32>(
        f32(sample_comp.y & 0xFFu),
        f32((sample_comp.y >> 8u) & 0xFFu),
        f32((sample_comp.y >> 16u) & 0xFFu),
    );
    // `.a := 1` — the per-sample weight (`06-design-a2.md` §3.2).
    s.color = vec4<f32>(100.0 * pow(vec3<f32>(2.0), (col_comp - 255.0) / 12.0), 1.0);
    s.normal_comp = (sample_comp.y >> 24u) & 0x7u;
    s.extra_data = sample_comp.y >> 27u;
    s.hash = sample_comp.x >> 16u;
    return s;
}
