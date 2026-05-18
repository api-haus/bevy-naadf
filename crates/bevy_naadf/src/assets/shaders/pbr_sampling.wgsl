// pbr_sampling.wgsl — the PBR-raymarching helper module.
//
// All non-trivial WGSL the unified PBR raymarcher needs lives here. See
// `docs/orchestrate/pbr-raymarching/02-design.md` § E / § F / § G for the
// design.
//
// Provides:
//   * `triplanar_blend_weights` — sharpened axis-aligned blend weights from
//     a surface normal (§ F.1).
//   * `triplanar_sample` — 3-plane data sample of a `texture_2d_array<f32>`
//     layer (§ F.2).
//   * `triplanar_sample_normal` — RNM-blended tangent-space normal sample
//     (§ F.3).
//   * `pom_displace_uv` — 8-tap linear + 4-tap binary parallax-occlusion
//     displacement on the dominant projection (§ F.4).
//   * `select_layer_variant` — PCG3D-hashed pick from `[base, base+span)`
//     for the procedural-variant feature (§ G; first cut hard-coded
//     `variant_span = 1` on every VoxelType, so this is an identity
//     return).
//   * `eval_pbr` — the energy-conserving GGX-Smith-Schlick BRDF
//     evaluation (§ E), wrapping `sample_vndf_isotropic` /
//     `geometry_term` (`ray_tracing_common.wgsl`) — zero new BRDF
//     primitives.
//
// Also exposes:
//   * `MIRROR_ROUGHNESS_EPSILON` — below this the first-hit pass re-enters
//     the perfect-reflect mirror loop instead of deferring to GI (design
//     decision #6).
//   * `ROUGH_SPECULAR_DIFFUSE_THRESHOLD` — `is_diffuse` flag split (design
//     decision #7).
//
// naga-oil import module.

#import "shaders/common.wgsl"::PI
#import "shaders/ray_tracing_common.wgsl"::geometry_term
#import "shaders/world_data.wgsl"::{
    pbr_diffuse_ao, pbr_normal, pbr_mrh, pbr_emissive, pbr_sampler,
}

// --- tunables --------------------------------------------------------------

// Triplanar blend sharpness. `k=8` is the de-facto axis-aligned tuning —
// for the AADF voxel face normals (axis-aligned ± normal-map epsilon) one
// projection dominates and the other two contribute ≤ 1%.
const TRIPLANAR_BLEND_SHARPNESS: f32 = 8.0;

// World-space UV scale: 1 voxel = `WORLD_UV_SCALE` texture units. 1.0 means
// the texture tiles once per voxel (1×1×1 m voxels and 1m-tiling textures —
// the AmbientCG default).
const WORLD_UV_SCALE: f32 = 1.0;

// POM tunables (`02-design.md` § F.4). `HEIGHT_SCALE = 0.05` = 5% of a
// voxel side; `LINEAR_STEPS = 8`, `BINARY_STEPS = 4` (industry-standard
// linear-search + binary-refine).
const POM_HEIGHT_SCALE: f32 = 0.05;
const POM_LINEAR_STEPS: i32 = 8;
const POM_BINARY_STEPS: i32 = 4;

// Below this perceptual-roughness the first-hit pass re-enters the existing
// 4-iteration perfect-reflect mirror loop instead of deferring to the GI
// pass (`02-design.md` decision #6).
const MIRROR_ROUGHNESS_EPSILON: f32 = 0.05;

// `is_diffuse` flag split: a perceptual-roughness above this defers
// specular-rough to GI's uniform-hemisphere sample (`is_diffuse=1` —
// Lambertian-like); below it uses the VNDF importance-sample
// (`is_diffuse=0`). Matches the prior `SURFACE_SPECULAR_ROUGH` /
// `SURFACE_DIFFUSE` palette split (`02-design.md` decision #7).
const ROUGH_SPECULAR_DIFFUSE_THRESHOLD: f32 = 0.5;

// --- triplanar blend weights -----------------------------------------------

fn triplanar_blend_weights(n: vec3<f32>) -> vec3<f32> {
    let w = pow(abs(n), vec3<f32>(TRIPLANAR_BLEND_SHARPNESS));
    let s = max(w.x + w.y + w.z, 1e-4);
    return w / s;
}

// --- triplanar data sample -------------------------------------------------

// Triplanar 3-plane sample of one texture-array layer. `world_pos` is the
// (camera-int-relative) hit position; `weights` come from
// `triplanar_blend_weights`; `layer` is the texture-array layer index.
//
// `textureSampleLevel(..., 0.0)` is used (NOT `textureSample`) because the
// shaders run in compute contexts where implicit derivatives are not
// available. Mip 0 is fine for 1K textures.
//
// Plane assignment (kept consistent with `triplanar_sample_normal` below):
//   x-plane: `world_pos.yz` (X-facing voxel face)
//   y-plane: `world_pos.zx` (Y-facing voxel face — the `zx` order keeps
//                            triplanar handedness consistent when the
//                            camera rotates around Y)
//   z-plane: `world_pos.xy` (Z-facing voxel face)
fn triplanar_sample(
    tex:       texture_2d_array<f32>,
    smp:       sampler,
    world_pos: vec3<f32>,
    weights:   vec3<f32>,
    layer:     u32,
) -> vec4<f32> {
    let p = world_pos * WORLD_UV_SCALE;
    let s_x = textureSampleLevel(tex, smp, p.yz, i32(layer), 0.0);
    let s_y = textureSampleLevel(tex, smp, p.zx, i32(layer), 0.0);
    let s_z = textureSampleLevel(tex, smp, p.xy, i32(layer), 0.0);
    return s_x * weights.x + s_y * weights.y + s_z * weights.z;
}

// `dominant_axis_from_weights` — return 0 for X-plane, 1 for Y-plane,
// 2 for Z-plane (the index of the plane that carries the largest blend
// weight under `triplanar_blend_weights`). Used by the POM-aware sampling
// helpers below to pick which plane's UV gets the height displacement.
fn dominant_axis_from_weights(weights: vec3<f32>) -> u32 {
    if (weights.x >= weights.y && weights.x >= weights.z) { return 0u; }
    if (weights.y >= weights.z) { return 1u; }
    return 2u;
}

// POM-aware triplanar sample. Applies the precomputed `displaced_uv` to the
// dominant plane's lookup; the two non-dominant planes use the geometric
// world-pos UV (their weights are ≤ ~5% under TRIPLANAR_BLEND_SHARPNESS=8 on
// axis-aligned face normals, so a POM-displaced UV on those planes would not
// be visible — design § F.4 "dominant projection only").
fn triplanar_sample_pom(
    tex:           texture_2d_array<f32>,
    smp:           sampler,
    world_pos:     vec3<f32>,
    weights:       vec3<f32>,
    layer:         u32,
    dominant_axis: u32,
    displaced_uv:  vec2<f32>,
) -> vec4<f32> {
    let p = world_pos * WORLD_UV_SCALE;
    var uv_x = p.yz;
    var uv_y = p.zx;
    var uv_z = p.xy;
    if (dominant_axis == 0u) { uv_x = displaced_uv; }
    else if (dominant_axis == 1u) { uv_y = displaced_uv; }
    else { uv_z = displaced_uv; }
    let s_x = textureSampleLevel(tex, smp, uv_x, i32(layer), 0.0);
    let s_y = textureSampleLevel(tex, smp, uv_y, i32(layer), 0.0);
    let s_z = textureSampleLevel(tex, smp, uv_z, i32(layer), 0.0);
    return s_x * weights.x + s_y * weights.y + s_z * weights.z;
}

// Compute the POM-displaced UV on the dominant plane. `view_dir` is the
// world-space ray direction (the camera-to-hit ray; NOT reversed). The 2D
// projection into the plane's UV space uses the same plane swizzling as
// `triplanar_sample` so the displacement direction stays consistent with the
// sampled texture coordinates.
//
// Returns the displaced UV (consumed by `triplanar_sample_pom` /
// `triplanar_sample_normal_pom`).
fn pom_displaced_uv_dominant(
    mrh_tex:       texture_2d_array<f32>,
    smp:           sampler,
    world_pos:     vec3<f32>,
    view_dir:      vec3<f32>,
    layer:         u32,
    dominant_axis: u32,
) -> vec2<f32> {
    let p = world_pos * WORLD_UV_SCALE;
    var base_uv: vec2<f32>;
    var view_uv: vec2<f32>;
    if (dominant_axis == 0u) {
        base_uv = p.yz;
        view_uv = view_dir.yz;
    } else if (dominant_axis == 1u) {
        base_uv = p.zx;
        view_uv = view_dir.zx;
    } else {
        base_uv = p.xy;
        view_uv = view_dir.xy;
    }
    return pom_displace_uv(mrh_tex, smp, base_uv, view_uv, layer);
}

// --- triplanar normal-map sample (RNM blend) -------------------------------

// Decode a tangent-space normal byte triplet (R,G,B in [0,1]) to a
// world-space unit normal under the triplanar projection. Plane assignment
// follows `triplanar_sample`; the sign of each lifted normal is taken from
// the face_normal (so a face oriented `-X` produces a correctly-flipped
// normal).
//
// Reference: Ben Golus, "Normal Mapping for a Triplanar Shader" (2017).
fn triplanar_sample_normal(
    tex:         texture_2d_array<f32>,
    smp:         sampler,
    world_pos:   vec3<f32>,
    weights:     vec3<f32>,
    face_normal: vec3<f32>,
    layer:       u32,
) -> vec3<f32> {
    let p = world_pos * WORLD_UV_SCALE;
    let n_x_local = textureSampleLevel(tex, smp, p.yz, i32(layer), 0.0).xyz * 2.0 - 1.0;
    let n_y_local = textureSampleLevel(tex, smp, p.zx, i32(layer), 0.0).xyz * 2.0 - 1.0;
    let n_z_local = textureSampleLevel(tex, smp, p.xy, i32(layer), 0.0).xyz * 2.0 - 1.0;

    let sign_x = sign(face_normal.x);
    let sign_y = sign(face_normal.y);
    let sign_z = sign(face_normal.z);

    // Lift each tangent-space normal into world space. The `local.z` is the
    // tangent-space normal Z (perturbation along the surface normal); we
    // route it into the world-space axis matching the projection face.
    let n_x_world = vec3<f32>(n_x_local.z * sign_x, n_x_local.y, n_x_local.x);
    let n_y_world = vec3<f32>(n_y_local.x, n_y_local.z * sign_y, n_y_local.y);
    let n_z_world = vec3<f32>(n_z_local.x, n_z_local.y, n_z_local.z * sign_z);

    let blended = n_x_world * weights.x
                + n_y_world * weights.y
                + n_z_world * weights.z;
    // Guard against the rare zero-length blended vector (axis exactly on
    // a knife-edge between two planes); fall back to the face normal.
    let len2 = dot(blended, blended);
    if (len2 < 1e-6) {
        return face_normal;
    }
    return blended / sqrt(len2);
}

// POM-aware variant of `triplanar_sample_normal`. Same blend math as
// `triplanar_sample_normal`, but the dominant plane samples from
// `displaced_uv` (the POM-displaced UV produced by
// `pom_displaced_uv_dominant`); the two non-dominant planes use the
// geometric world-pos UV. Mirrors `triplanar_sample_pom` for the diffuse /
// MRH samples so all three texture taps (diffuse, normal, MRH re-sample)
// stay consistent at the displaced surface point.
fn triplanar_sample_normal_pom(
    tex:           texture_2d_array<f32>,
    smp:           sampler,
    world_pos:     vec3<f32>,
    weights:       vec3<f32>,
    face_normal:   vec3<f32>,
    layer:         u32,
    dominant_axis: u32,
    displaced_uv:  vec2<f32>,
) -> vec3<f32> {
    let p = world_pos * WORLD_UV_SCALE;
    var uv_x = p.yz;
    var uv_y = p.zx;
    var uv_z = p.xy;
    if (dominant_axis == 0u) { uv_x = displaced_uv; }
    else if (dominant_axis == 1u) { uv_y = displaced_uv; }
    else { uv_z = displaced_uv; }
    let n_x_local = textureSampleLevel(tex, smp, uv_x, i32(layer), 0.0).xyz * 2.0 - 1.0;
    let n_y_local = textureSampleLevel(tex, smp, uv_y, i32(layer), 0.0).xyz * 2.0 - 1.0;
    let n_z_local = textureSampleLevel(tex, smp, uv_z, i32(layer), 0.0).xyz * 2.0 - 1.0;

    let sign_x = sign(face_normal.x);
    let sign_y = sign(face_normal.y);
    let sign_z = sign(face_normal.z);

    let n_x_world = vec3<f32>(n_x_local.z * sign_x, n_x_local.y, n_x_local.x);
    let n_y_world = vec3<f32>(n_y_local.x, n_y_local.z * sign_y, n_y_local.y);
    let n_z_world = vec3<f32>(n_z_local.x, n_z_local.y, n_z_local.z * sign_z);

    let blended = n_x_world * weights.x
                + n_y_world * weights.y
                + n_z_world * weights.z;
    let len2 = dot(blended, blended);
    if (len2 < 1e-6) {
        return face_normal;
    }
    return blended / sqrt(len2);
}

// --- POM displacement (dominant-plane only) --------------------------------

// Displace a 2D UV by sampling the MRH.B height channel along the view
// direction projected into the plane's UV space. 8-tap linear search +
// 4-tap binary refine. Returns the displaced UV (suitable for a final
// albedo/normal/MRH re-sample on that plane). The caller passes one of
// `world_pos.yz` / `.zx` / `.xy` and the matching `view_dir.yz/.zx/.xy`.
fn pom_displace_uv(
    mrh_tex:     texture_2d_array<f32>,
    smp:         sampler,
    base_uv:     vec2<f32>,
    view_dir_2d: vec2<f32>,
    layer:       u32,
) -> vec2<f32> {
    let dir = view_dir_2d * POM_HEIGHT_SCALE;
    let step = dir / f32(POM_LINEAR_STEPS);
    var uv = base_uv;
    var prev_uv = uv;
    var prev_layer_depth: f32 = 0.0;
    var depth: f32 = 0.0;
    var sampled: f32 = 1.0;

    for (var i: i32 = 0; i < POM_LINEAR_STEPS; i = i + 1) {
        prev_uv = uv;
        prev_layer_depth = depth;
        uv = uv + step;
        depth = depth + 1.0 / f32(POM_LINEAR_STEPS);
        sampled = textureSampleLevel(mrh_tex, smp, uv, i32(layer), 0.0).b;
        if (depth >= 1.0 - sampled) { break; }
    }

    // Binary refine between (prev_uv, uv).
    var lo = prev_uv;
    var hi = uv;
    var lo_depth = prev_layer_depth;
    var hi_depth = depth;
    for (var i: i32 = 0; i < POM_BINARY_STEPS; i = i + 1) {
        let mid = 0.5 * (lo + hi);
        let mid_depth = 0.5 * (lo_depth + hi_depth);
        let mid_sample = textureSampleLevel(mrh_tex, smp, mid, i32(layer), 0.0).b;
        if (mid_depth >= 1.0 - mid_sample) {
            hi = mid;
            hi_depth = mid_depth;
        } else {
            lo = mid;
            lo_depth = mid_depth;
        }
    }
    return 0.5 * (lo + hi);
}

// --- variant select (PCG3D hash) -------------------------------------------

// PCG3D — Jarzynski & Olano (2020), "Hash Functions for GPU Rendering".
fn pcg3d(seed: vec3<u32>) -> vec3<u32> {
    var v = seed * 1664525u + 1013904223u;
    v.x = v.x + v.y * v.z;
    v.y = v.y + v.z * v.x;
    v.z = v.z + v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x = v.x + v.y * v.z;
    v.y = v.y + v.z * v.x;
    v.z = v.z + v.x * v.y;
    return v;
}

// Pick one of `variant_span` adjacent layers (base, base+1, ..., base+span-1)
// for the integer voxel position. `variant_span` must be a power of two
// (encoded as `variant_span_log2` in `GpuVoxelType`); first cut hard-codes
// 1 ⇒ identity return.
fn select_layer_variant(
    base_layer:   u32,
    variant_span: u32,
    voxel_pos:    vec3<i32>,
) -> u32 {
    if (variant_span <= 1u) {
        return base_layer;
    }
    let h = pcg3d(vec3<u32>(voxel_pos)).x;
    return base_layer + (h & (variant_span - 1u));
}

// --- the unified BRDF ------------------------------------------------------

// `eval_pbr` return — the BRDF value `f`, plus the Schlick Fresnel `F` at
// the half-vector (consumers may need it to weight the next bounce's
// throughput) and the base specular reflectance (for the e2e gate's
// "metallic F0 ≈ albedo" check).
//
// PORT NOTE: naga-oil's composable-module rule rejects trailing-digit
// identifiers (the same rule that hit `data1` / `data2` in `SampleValid`).
// `f0` is renamed `f_zero`.
struct PbrEval {
    f:       vec3<f32>,
    fresnel: vec3<f32>,
    f_zero:  vec3<f32>,
}

// `eval_pbr` — the unified, energy-conserving GGX-Smith-Schlick BRDF.
//
// Reuses `geometry_term` (Smith G) from `ray_tracing_common.wgsl`. The
// metal/dielectric split is the standard `F0 = mix(0.04, albedo, metallic)`
// + `kS = F; kD = (1 - F) * (1 - metallic)` formulation
// (`02-design.md` § E energy-conserving composition).
//
// `light_dir` is the direction towards the light (sampled), `view_dir` is
// the direction towards the camera (incoming ray reversed), `normal` is
// the perturbed surface normal — all unit world-space vectors.
//
// Returns the BRDF value `f`, ready to multiply into the bounce
// accumulator: `radiance += throughput * f * cosTheta_l * incoming_radiance`
// (the `cos_theta_l` and the light-side sample stay in the caller).
fn eval_pbr(
    light_dir: vec3<f32>,
    view_dir:  vec3<f32>,
    normal:    vec3<f32>,
    albedo:    vec3<f32>,
    metallic:  f32,
    perceptual_roughness: f32,
) -> PbrEval {
    // Clamp `alpha` (the GGX α = perceptual_roughness²) away from zero so
    // the `D` denominator `n·h² * (α²-1) + 1` cannot collapse to zero at
    // perfect half-vector alignment (`d = 0/0 = NaN`). 1e-3 is the standard
    // Frostbite / Filament `MIN_PERCEPTUAL_ROUGHNESS² = (0.045)² ≈ 0.002`
    // industry tuning. Without the clamp, GI / spatial-resampling
    // `eval_pbr` calls on metals with authored roughness ≈ 0 generate
    // occasional NaN sparkles that tonemap as bright clusters.
    let alpha = max(perceptual_roughness * perceptual_roughness, 1e-3);
    let half_dir = normalize(light_dir + view_dir);
    let n_dot_l = clamp(dot(normal, light_dir), 0.0, 1.0);
    let n_dot_v = clamp(dot(normal, view_dir),  0.0, 1.0);
    let v_dot_h = clamp(dot(view_dir, half_dir), 0.0, 1.0);
    let n_dot_h = clamp(dot(normal, half_dir),   0.0, 1.0);

    // F0 — base specular reflectance. Dielectric ≈ 0.04 (plastic, n≈1.5);
    // pure metal = albedo (energy-conserving via `kD = 0` for metals).
    let f_base = mix(vec3<f32>(0.04), albedo, metallic);

    // Schlick Fresnel at the half-vector incident angle.
    let one_minus_voh = 1.0 - v_dot_h;
    let f = f_base + (vec3<f32>(1.0) - f_base)
        * pow(one_minus_voh, 5.0);

    // GGX-Smith specular: D · G_in · G_out · F / (4 · n·l · n·v).
    let alpha2 = alpha * alpha;
    let denom_term = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
    let d = alpha2 / (PI * denom_term * denom_term);
    let g_in  = geometry_term(perceptual_roughness, n_dot_l);
    let g_out = geometry_term(perceptual_roughness, n_dot_v);
    let denom_brdf = max(4.0 * n_dot_l * n_dot_v, 1e-4);
    let specular = (d * g_in * g_out * f) / denom_brdf;

    // Diffuse: Lambertian with energy-conserving suppression.
    //   kS = F; kD = (1 - F) * (1 - metallic); diffuse = albedo / PI.
    let k_d = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    let diffuse = k_d * albedo / PI;

    var out: PbrEval;
    out.f = diffuse + specular;
    out.fresnel = f;
    out.f_zero = f_base;
    return out;
}
