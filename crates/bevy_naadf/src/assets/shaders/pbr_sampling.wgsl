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

// POM tunables (`02-design.md` § F.4 + `05-diagnostic.md` "POM rewrite —
// modern implementation").
//
// `POM_HEIGHT_SCALE = 0.05` = 5% of a voxel side. Unchanged from the prior
// baseline.
//
// Adaptive linear-march step count: `mix(MAX, MIN, abs(cos_view))`. Face-on
// → `MIN` steps; grazing → `MAX` steps. Replaces the prior fixed
// `LINEAR_STEPS=8 + BINARY_STEPS=4`. Linear interpolation between the last
// two samples (Dayuppy steep-parallax style) replaces the binary refine —
// at adaptive 8-32 steps the local slope is dense enough that a single
// linear interpolant matches binary-refine quality.
const POM_HEIGHT_SCALE: f32 = 0.05;
const POM_MIN_LINEAR_STEPS: i32 = 8;
const POM_MAX_LINEAR_STEPS: i32 = 32;

// Self-shadow march step count: `mix(MAX, MIN, abs(cos_light))`. Sun
// overhead → `MIN`; sun grazing → `MAX`. The shadow march fires from the
// displaced UV toward the sun in the dominant plane's tangent space.
const POM_SHADOW_MIN_STEPS: i32 = 6;
const POM_SHADOW_MAX_STEPS: i32 = 16;

// Maximum shadow attenuation (the shadow factor cannot dip below
// `1 - SHADOW_STRENGTH`). Keeping valleys at 15% of unshadowed brightness
// avoids fighting with the GI ambient fill that adds light back regardless.
const POM_SHADOW_STRENGTH: f32 = 0.85;

// Soft-clip for the parallax displacement magnitude. When the search has
// marched more than this many UV units from the base UV, the displacement
// fades back toward zero via a smoothstep over `[FADE_MAX, FADE_MAX*2]`.
// Prevents extreme grazing on a high-relief textured face from wrapping
// around the next tile.
const POM_DISPLACEMENT_FADE_MAX: f32 = 0.5;

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

// `PomResult` — bundles the POM-displaced UV with the height at the
// intersection. The shadow march re-uses the intersection height as the
// starting depth, so the modern POM returns both.
struct PomResult {
    uv:     vec2<f32>,
    height: f32,
}

// Tangent-space basis for the dominant triplanar plane.
//
//   axis=0 (X-dominant, plane = YZ): u=world.y, v=world.z, n=world.x
//   axis=1 (Y-dominant, plane = ZX): u=world.z, v=world.x, n=world.y
//   axis=2 (Z-dominant, plane = XY): u=world.x, v=world.y, n=world.z
//
// `project_plane_uv(world_pos, axis)` returns `(u, v)`; `project_plane_n`
// returns the component along the plane normal. The view / light vectors
// are projected via the same routines so all marches share one tangent
// space.
fn project_plane_uv(p: vec3<f32>, dominant_axis: u32) -> vec2<f32> {
    if (dominant_axis == 0u) { return p.yz; }
    if (dominant_axis == 1u) { return p.zx; }
    return p.xy;
}

fn project_plane_n(p: vec3<f32>, dominant_axis: u32) -> f32 {
    if (dominant_axis == 0u) { return p.x; }
    if (dominant_axis == 1u) { return p.y; }
    return p.z;
}

// Compute the POM-displaced UV on the dominant plane.
//
// `view_dir` is the world-space ray direction (the camera-to-hit ray; NOT
// reversed). The function projects it into the dominant plane's tangent
// space and runs the modern adaptive-step parallax march, returning both
// the displaced UV AND the sampled height at the intersection.
//
// The returned UV is consumed by `triplanar_sample_pom` /
// `triplanar_sample_normal_pom`; the returned height is consumed by
// `pom_self_shadow` (the shadow march starts at that height to avoid
// self-occlusion on the first tap).
fn pom_displaced_uv_dominant(
    mrh_tex:       texture_2d_array<f32>,
    smp:           sampler,
    world_pos:     vec3<f32>,
    view_dir:      vec3<f32>,
    layer:         u32,
    dominant_axis: u32,
) -> PomResult {
    let p = world_pos * WORLD_UV_SCALE;
    let base_uv = project_plane_uv(p, dominant_axis);
    let view_uv = project_plane_uv(view_dir, dominant_axis);
    let view_n  = project_plane_n(view_dir, dominant_axis);
    return pom_displace_uv(mrh_tex, smp, base_uv, view_uv, view_n, layer);
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

// --- POM displacement (dominant-plane only, modern adaptive) --------------

// Modern steep-parallax displacement. After the linear search overshoots
// the heightfield, the intersection is approximated by linear interpolation
// between the last-before and first-after samples (Dayuppy steep-parallax
// reference, `05-diagnostic.md` "POM rewrite" § Design).
//
// The step count adapts to the view angle: face-on uses
// `POM_MIN_LINEAR_STEPS`, grazing uses `POM_MAX_LINEAR_STEPS`. The march
// proceeds opposite the view direction in UV space, divided by
// `abs(view_n)` so silhouette thickness stays constant in screen space.
//
// `base_uv` is the original plane UV (e.g. `world_pos.xy` for Z-dominant);
// `view_uv` is the plane projection of the world-space view direction
// (e.g. `view_dir.xy`); `view_n` is the view direction's component along
// the dominant axis (e.g. `view_dir.z`).
//
// Returns the displaced UV and the sampled height at the intersection
// point. The caller (typically `pom_self_shadow`) re-uses the height as
// the shadow march's starting depth to avoid the first-tap self-occlusion
// failure mode.
fn pom_displace_uv(
    mrh_tex:    texture_2d_array<f32>,
    smp:        sampler,
    base_uv:    vec2<f32>,
    view_uv:    vec2<f32>,
    view_n:     f32,
    layer:      u32,
) -> PomResult {
    // Adaptive step count: face-on → MIN, grazing → MAX. `abs(view_n)`
    // approximates `cos(theta)` between view and surface normal.
    let cos_view = clamp(abs(view_n), 0.01, 1.0);
    let num_steps_f = mix(
        f32(POM_MAX_LINEAR_STEPS),
        f32(POM_MIN_LINEAR_STEPS),
        cos_view,
    );
    let num_steps = i32(num_steps_f);
    let inv_steps = 1.0 / num_steps_f;

    // Per-step UV delta. The minus sign marches AGAINST view_dir (we step
    // toward the camera in plane coords). `/cos_view` keeps silhouette
    // thickness constant across view angles (Dayuppy ref convention).
    let step = -view_uv * POM_HEIGHT_SCALE * inv_steps / cos_view;
    let delta_h = inv_steps;

    var uv = base_uv;
    var prev_uv = uv;
    var depth: f32 = 0.0;
    var prev_depth: f32 = 0.0;
    var sampled: f32 = 0.0;
    var prev_sampled: f32 = 0.0;

    // Linear search. We march until the sampled height rises above the
    // current ray-remaining depth (the surface "catches up" to the ray).
    // `depth` here is "depth into the heightfield from the top", matching
    // the prior convention `depth >= 1.0 - sampled`.
    for (var i: i32 = 0; i < num_steps; i = i + 1) {
        prev_uv = uv;
        prev_depth = depth;
        prev_sampled = sampled;
        uv = uv + step;
        depth = depth + delta_h;
        sampled = textureSampleLevel(mrh_tex, smp, uv, i32(layer), 0.0).b;
        if (depth >= 1.0 - sampled) { break; }
    }

    // Linear interpolation between the last-before and first-after taps.
    // Replaces the prior binary-refine pass with a single weighted blend.
    //
    //   after_overshoot  = sampled      - (1 - depth)
    //   before_overshoot = prev_sampled - (1 - prev_depth)  [≤ 0 normally]
    //
    // The intersection is at `t = before / (before + after)` along the
    // step (the "lerp toward the larger overshoot" Dayuppy trick).
    let after  = sampled      - (1.0 - depth);
    let before = (1.0 - prev_depth) - prev_sampled;
    let denom = before + after;
    let t = select(0.5, clamp(before / denom, 0.0, 1.0), denom > 1e-5);
    let raw_uv = mix(prev_uv, uv, t);
    let intersection_height = mix(prev_sampled, sampled, t);

    // Soft-clip the parallax displacement magnitude. When the search has
    // marched far from `base_uv`, fade the displacement back toward zero
    // to avoid wrap artefacts at the tile boundary. The fade kicks in
    // beyond `FADE_MAX` and saturates at `FADE_MAX * 2`.
    let raw_offset = raw_uv - base_uv;
    let off_mag = length(raw_offset);
    let fade = 1.0 - smoothstep(
        POM_DISPLACEMENT_FADE_MAX,
        POM_DISPLACEMENT_FADE_MAX * 2.0,
        off_mag,
    );
    let final_uv = base_uv + raw_offset * fade;

    var r: PomResult;
    r.uv = final_uv;
    r.height = intersection_height;
    return r;
}

// `pom_self_shadow` — secondary march from the POM-displaced surface point
// toward the light, in the dominant plane's tangent space.
//
// Starting at the intersection height (NOT the surface), march outward
// along the light direction in UV space, sampling height at each step. If
// any tap exceeds the current shadow-ray height, the surface is in
// self-shadow (the light is occluded by a higher local microgeometry
// feature).
//
// Returns a shadow factor in `[1 - POM_SHADOW_STRENGTH, 1]`. A smoothstep
// over the maximum overshoot softens the binary hard-shadow into a
// pseudo-penumbra without the cost of a PCF kernel.
//
// The march is skipped when:
//   * `cos_light_n <= 0` — light below the plane horizon (back-face);
//   * `intersection_height >= 1.0` — surface tap is already at the
//     heightmap maximum (no occluder above).
//
// `light_dir` is the world-space direction TOWARDS the light (sky_sun_dir
// convention). `base_height` is the `PomResult.height` from
// `pom_displace_uv` — the height at the intersection point that anchors
// the shadow ray's starting depth.
fn pom_self_shadow(
    mrh_tex:       texture_2d_array<f32>,
    smp:           sampler,
    displaced_uv:  vec2<f32>,
    base_height:   f32,
    light_dir:     vec3<f32>,
    layer:         u32,
    dominant_axis: u32,
) -> f32 {
    let light_uv = project_plane_uv(light_dir, dominant_axis);
    let light_n  = project_plane_n(light_dir, dominant_axis);

    // Light below the plane → fully shadowed (back-face); the caller is
    // expected to gate the multiplication on `dot(n, l) > 0` already, but
    // a defensive early-out keeps the function safe in isolation.
    let cos_light = clamp(abs(light_n), 0.01, 1.0);
    if (light_n <= 0.0) {
        return 1.0 - POM_SHADOW_STRENGTH;
    }
    if (base_height >= 0.999) {
        return 1.0;
    }

    // Adaptive shadow-march step count: overhead → MIN, grazing → MAX.
    let num_steps_f = mix(
        f32(POM_SHADOW_MAX_STEPS),
        f32(POM_SHADOW_MIN_STEPS),
        cos_light,
    );
    let num_steps = i32(num_steps_f);
    let inv_steps = 1.0 / num_steps_f;

    // Per-step UV delta along the light direction. The plus sign marches
    // AWAY from the surface toward the light. `/cos_light` keeps silhouette
    // thickness constant.
    let step = light_uv * POM_HEIGHT_SCALE * inv_steps / cos_light;
    // Per-step shadow-ray height delta. The ray starts at `base_height +
    // small_bias` and climbs toward 1.0; an occluder at any UV whose
    // height exceeds the ray height blocks the sun.
    let delta_h = inv_steps;
    let bias = delta_h * 0.1;

    var uv = displaced_uv + step;
    var ray_h = base_height + bias;
    var max_overshoot: f32 = 0.0;

    for (var i: i32 = 0; i < num_steps; i = i + 1) {
        if (ray_h >= 1.0) { break; }
        let h = textureSampleLevel(mrh_tex, smp, uv, i32(layer), 0.0).b;
        let overshoot = h - ray_h;
        if (overshoot > max_overshoot) {
            max_overshoot = overshoot;
        }
        uv = uv + step;
        ray_h = ray_h + delta_h;
    }

    // Smoothstep over the maximum overshoot → soft pseudo-penumbra.
    // `0` overshoot → no shadow; `2 * delta_h` overshoot → full shadow.
    let penumbra = smoothstep(0.0, delta_h * 2.0, max_overshoot);
    return 1.0 - penumbra * POM_SHADOW_STRENGTH;
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
