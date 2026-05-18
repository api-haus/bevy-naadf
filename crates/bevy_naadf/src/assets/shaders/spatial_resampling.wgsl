// spatial_resampling.wgsl — compressed-ReSTIR GI Algorithm 2: the spatial
// resampling pass.
//
// Derives from: render/versions/base/renderSpatialResampling.fx
// `calcSpatialResampling` + `sampleNeighbors` + `getSampleData` + `getBRDF` +
// `getTargetFunctionNew` (`09-design-b.md` §5.1, §5.7, §8.3). This is paper
// Algorithm 2: per pixel, a 12-iteration weighted-reservoir-sampling loop over
// neighbouring 8×8 buckets (with an adaptive per-pixel search radius), merges
// the selected GI sample with a SINGLE 3-step mirror-following visibility ray,
// adds a sun sample, then either writes the transposed `denoise_preprocessed`
// scratch (denoise path) or composites directly into `final_color`
// (non-denoise path).
//
// `[numthreads(64,1,1)]` dispatched over `ceil(pixel_count / 64)` workgroups
// (`WorldRenderBase.cs:397`). It traverses the voxel world (visibility + sun
// rays) so it binds `@group(0)` world; `@group(1)` is the spatial-specific
// buffer set.
//
// PORT NOTES (`09-design-b.md` §5.7 / §8.3 + the Batch-3/4 carry-forwards):
// - `getSampleData`'s HLSL out-params → a WGSL struct return (`SampleData`).
// - `getRayDir(invCamMatrix, ...)` → `get_ray_dir(gi_params.inv_view_proj, ...)`
//   — `spatialResampling` binds the NON-inverse... no: it binds `invCamMatrix`
//   (`renderSpatialResampling.fx:17,351`), the inverse view-proj, which the
//   shared `get_ray_dir` already takes. (Contrast `renderSampleRefine`, which
//   binds the per-frame-history INVERSE ring — §3.6 — a different thing.)
// - The visibility ray + sun ray use `shoot_ray` (int+frac origin, D1) with the
//   `MAX_RAY_STEPS_VISIBILITY` / `MAX_RAY_STEPS_SUN` consts. The HLSL declares a
//   `spatialVisibilityCount` uniform but `sampleNeighbors` actually passes the
//   `MAX_RAY_STEPS_VISIBILITY` const to `shootRay` (`renderSpatialResampling.fx:274`)
//   — ported faithfully: the const cap, not the (dead) uniform field.
// - HLSL implicit float→int truncation / scalar→vector broadcast: explicit
//   `i32()` / `u32()` casts + explicit `vec3` constructors throughout.
// - Every HLSL `mul(v, M)` is the column-vector `M * v` (`05-review.md`).
// - `#ifdef ENTITIES` blocks are omitted — Phase B is entity-free
//   (`09-design-b.md` §1): the `getHitDataFromPlanes` entity params are absent
//   (the shared full version in `render_pipeline_common.wgsl` drops them).
// - CROSS-BATCH DEPENDENCY (`09-design-b.md` §11 Batch 4 step 13 / §11 Batch 5):
//   the refine buffers (`bucket_info` / `valid_samples_compressed`) are
//   *correct-but-empty* until Batch 6 wires `taa_dist_min_max` — so the
//   12-iteration neighbour-reservoir loop yields nothing pre-B6 (`bucketValidStored
//   == 0` for every bucket). But the SUN SAMPLE (`renderSpatialResampling.fx:321-339`)
//   is independent of the refine buffers — it shoots a sun ray and adds
//   `sunColor * weight` for any sun-facing diffuse surface. So direct-sun bounce
//   light DOES land in `final_color` at end-of-Batch-5; the full indirect
//   reservoir-resampled bounce arrives once Batch 6 fills `taa_dist_min_max`.
//
// naga-oil import module — pulls `@group(0)` world bindings in via
// `ray_tracing.wgsl`.

#import "shaders/gi_params.wgsl"::{GpuGiParams, GI_FLAG_IS_DENOISE, GI_FLAG_IS_VARYING_RADIUS}
#import "shaders/render_pipeline_common.wgsl"::{
    VoxelType, FirstHitResult,
    decompress_voxel_type, get_ray_dir, get_hit_data_from_planes,
    HIT_NOTHING, SURFACE_PBR,
}
#import "shaders/ray_tracing.wgsl"::{
    RayResult, shoot_ray,
}
#import "shaders/ray_tracing_common.wgsl"::{
    init_rand, next_rand, oct_decode,
    get_uniform_hemisphere_sample, pdf_vndf_isotropic, geometry_term,
}
#import "shaders/color_compression.wgsl"::COLORS
#import "shaders/world_data.wgsl"::{
    voxel_types,
    pbr_diffuse_ao, pbr_normal, pbr_mrh, pbr_emissive, pbr_sampler,
}
#import "shaders/common.wgsl"::PI
#import "shaders/pbr_sampling.wgsl"::{
    triplanar_blend_weights, triplanar_sample, triplanar_sample_normal,
    select_layer_variant,
    eval_pbr, PbrEval, MIRROR_ROUGHNESS_EPSILON, ROUGH_SPECULAR_DIFFUSE_THRESHOLD,
}

// --- @group(1) — the spatial-resampling-specific bindings -------------------

@group(1) @binding(0) var<uniform> gi_params: GpuGiParams;
// `firstHitData` — the G-buffer, read-only.
@group(1) @binding(1) var<storage, read> first_hit_data: array<vec4<u32>>;
// `firstHitAbsorption` — the per-pixel primary-ray transmittance, read-only.
@group(1) @binding(2) var<storage, read> first_hit_absorption: array<vec2<u32>>;
// `globalIlumBucketInfo` — the 8×8 screen-space region data, read-only here
// (`renderSampleRefine` wrote it). CROSS-BATCH: empty until Batch 6.
@group(1) @binding(3) var<storage, read> bucket_info: array<vec2<u32>>;
// `globalIlumValidSamplesCompressed` — `bucket_count * 8` × `vec4<u32>`, the
// brightness-levelled per-bucket lit samples, read-only here. CROSS-BATCH:
// empty until Batch 6.
@group(1) @binding(4) var<storage, read> valid_samples_compressed: array<vec4<u32>>;
// `taaSampleAccum` — the 16-frame TAA accumulator, read-only (the denoise-path
// TAA-colour read at `renderSpatialResampling.fx:371`). CROSS-BATCH: written by
// `ReprojectOld` / `CalcNewTaaSample` in Batch 6; zero until then.
@group(1) @binding(5) var<storage, read> taa_sample_accum: array<vec2<u32>>;
// `finalColor` — the GI working-colour buffer. The first-hit wrote the
// primary-ray light here; the non-denoise path adds the resampled GI into it.
@group(1) @binding(6) var<storage, read_write> final_color: array<vec2<u32>>;
// `denoisePreprocessed` — the denoiser scratch (`Uint3` stored padded to a
// 16-byte `vec4<u32>` stride — `09-design-b.md` §3.3). The denoise path writes
// it (column-major / transposed index — the denoiser reads it that way).
@group(1) @binding(7) var<storage, read_write> denoise_preprocessed: array<vec4<u32>>;

// `getSampleData`'s decoded result (`renderSpatialResampling.fx:29-38` — the
// HLSL out-params become a struct return, the A-2 `decompressSample` pattern).
struct SampleData {
    vis_pos_int: vec3<i32>,
    vis_pos_frac: vec3<f32>,
    sample_dir: vec3<f32>,
    sample_dist: f32,
    sample_normal: vec3<f32>,
    is_diffuse: bool,
}

// `getSampleData` (`renderSpatialResampling.fx:29-38`) — decode a compressed
// lit sample (`globalIlumValidSamplesCompressed[...]`) into its visible-surface
// position / sample direction / distance / normal.
fn get_sample_data(res: vec4<u32>) -> SampleData {
    var d: SampleData;
    let vis_pos_comp = vec3<u32>(
        (res.z >> 11u) & 0x7FFFFu,
        res.x >> 15u,
        (res.w >> 11u) & 0x7FFFFu,
    );
    // HLSL `int3 visPosInt = visPosComp / 32` — `uint3 / 32` then signed.
    d.vis_pos_int = vec3<i32>(vis_pos_comp / 32u);
    d.vis_pos_frac = vec3<f32>(vis_pos_comp % 32u) / 32.0;
    d.sample_dist = unpack2x16float(res.y >> 16u).x;
    d.is_diffuse = ((res.w >> 30u) & 0x1u) != 0u;
    d.sample_dir = oct_decode(
        vec2<f32>(vec2<u32>(res.z & 0x7FFu, res.w & 0x7FFu)) / 2047.5,
    );
    d.sample_normal = oct_decode(
        vec2<f32>(vec2<u32>(res.y & 0xFFu, (res.y >> 8u) & 0xFFu)) / 255.0,
    );
    return d;
}

// `get_brdf` — post-PBR-raymarching: call `eval_pbr` and return the full
// energy-conserving BRDF (diffuse + specular). Preserves the call shape;
// swaps the BRDF model from IOR-Fresnel-Cook-Torrance to the unified
// `eval_pbr` (`02-design.md` § E). `albedo` + `metallic` replace the prior
// `ior` parameter.
fn get_brdf(
    roughness: f32,
    albedo:    vec3<f32>,
    metallic:  f32,
    normal:    vec3<f32>,
    light_dir: vec3<f32>,
    ray_dir:   vec3<f32>,
) -> vec3<f32> {
    let pbr = eval_pbr(light_dir, ray_dir, normal, albedo, metallic, roughness);
    return pbr.f;
}

// `getTargetFunctionNew` (`renderSpatialResampling.fx:49-54`) — the reservoir's
// target function: the luminance of `radiance * clamp(brdf, 0, 100)`.
fn get_target_function_new(
    sample_dir: vec3<f32>,
    vis_normal: vec3<f32>,
    radiance: vec3<f32>,
    brdf: vec3<f32>,
) -> f32 {
    let brdf_cos = clamp(brdf, vec3<f32>(0.0), vec3<f32>(100.0));
    return length(radiance * brdf_cos);
}

// `sampleNeighbors` (`renderSpatialResampling.fx:56-342`) — the 12-iteration
// reservoir loop + the adaptive-radius 12-tap pre-pass + the single visibility
// ray + the sun sample. Returns the resampled GI colour for `pixelPos`.
fn sample_neighbors(
    pixel_pos: vec2<u32>,
    sample_count: u32,
    first_hit: FirstHitResult,
    type_index: u32,
) -> vec3<f32> {
    var rand = init_rand(vec3<u32>(pixel_pos, gi_params.rand_counter));
    let cam_pos_int = gi_params.cam_pos_int.xyz;

    var sum_weight: f32 = 0.0;

    var selected_color = vec3<f32>(0.0, 0.0, 0.0);
    var selected_ray_dir = vec3<f32>(0.0, 0.0, 0.0);
    var selected_length_to_sample_squared_now: f32 = 0.0;
    var selected_is_sky = false;
    var selected_bounce_state: u32 = 0u;

    // `firstHitPos = firstHit.pos + firstHit.normal * 0.02` (re-split into
    // int + frac — D1).
    let first_hit_pos = first_hit.pos + first_hit.normal * 0.02;
    let first_hit_pos_frac = fract(first_hit_pos);
    let first_hit_pos_int = cam_pos_int + vec3<i32>(floor(first_hit_pos));

    let first_hit_type: VoxelType = decompress_voxel_type(voxel_types[type_index]);

    // Post-PBR-raymarching: sample the first-hit material's PBR parameters
    // (albedo / metallic / roughness) from the texture arrays at the
    // reconstructed virtual hit world position. The spatial-resampling
    // pass's BRDF + visibility-ray logic needs these in place of the prior
    // per-VoxelType scalars (`02-design.md` § E).
    let fh_world_pos = vec3<f32>(cam_pos_int) + first_hit.pos;
    let fh_blend = triplanar_blend_weights(first_hit.normal);
    let fh_layer = select_layer_variant(
        first_hit_type.material_layer_index,
        first_hit_type.variant_span,
        vec3<i32>(floor(fh_world_pos)),
    );
    let fh_mrh = triplanar_sample(
        pbr_mrh, pbr_sampler, fh_world_pos, fh_blend, fh_layer,
    );
    let first_hit_metallic = fh_mrh.r;
    let first_hit_roughness = fh_mrh.g;
    let fh_diffuse_ao = triplanar_sample(
        pbr_diffuse_ao, pbr_sampler, fh_world_pos, fh_blend, fh_layer,
    );
    let first_hit_albedo = fh_diffuse_ao.rgb
        * first_hit_type.albedo_tint * fh_diffuse_ao.a;
    // Perturbed (normal-mapped) surface normal at the reconstructed first
    // hit. Used in all BRDF calls below (`get_brdf`, the resolved-color
    // `eval_pbr`, the sun-sample `eval_pbr`) so the spatial-resampling pass
    // shares the same per-pixel normal-map response as the first-hit pass.
    // Geometric tests (visibility ray origin, sun-shadow ray origin,
    // bucket-classification) keep using `first_hit.normal` — they are
    // geometric face-orientation lookups, not BRDF inputs.
    let first_hit_perturbed_normal = triplanar_sample_normal(
        pbr_normal, pbr_sampler,
        fh_world_pos, fh_blend, first_hit.normal, fh_layer,
    );

    // `is_diffuse` split mirrors the first-hit pass's gate
    // (`ROUGH_SPECULAR_DIFFUSE_THRESHOLD`, `02-design.md` decision #7). A
    // fine-roughness PBR surface defers to specular sampling; coarse PBR
    // (or emissive — shouldn't reach here) is treated Lambertian.
    let first_hit_is_diffuse =
        first_hit_type.material_base != SURFACE_PBR
        || first_hit_roughness >= ROUGH_SPECULAR_DIFFUSE_THRESHOLD;

    let radius = gi_params.spatial_resample_size;
    var radius_fac: f32 = 1.0;

    let is_varying_radius = (gi_params.flags & GI_FLAG_IS_VARYING_RADIUS) != 0u;

    // --- the adaptive-radius 12-tap pre-pass (renderSpatialResampling.fx:81-148)
    if (is_varying_radius) {
        var valid_bucket_count_small: i32 = 0;
        var valid_bucket_count_big: i32 = 0;
        var max_color_small: f32 = 0.0;
        var max_color_sum_small: f32 = 1.0;
        var worst_lit_small: f32 = 0.0001;
        var worst_lit_big: f32 = 0.0001;

        for (var i: i32 = 0; i < 12; i = i + 1) {
            let scale = select(1.0, 0.1, i < 6);
            let xy = (vec2<f32>(-radius * 0.5)
                + radius * vec2<f32>(next_rand(&rand), next_rand(&rand))) * scale;

            // The HLSL mirrors an out-of-range neighbour index back in-bounds.
            var neighbor_index = vec2<i32>(vec2<f32>(pixel_pos) + xy);
            neighbor_index.x = select(
                select(neighbor_index.x, 2 * i32(gi_params.screen_width) - neighbor_index.x,
                       neighbor_index.x > i32(gi_params.screen_width)),
                -neighbor_index.x,
                neighbor_index.x < 0,
            );
            neighbor_index.y = select(
                select(neighbor_index.y, 2 * i32(gi_params.screen_height) - neighbor_index.y,
                       neighbor_index.y > i32(gi_params.screen_height)),
                -neighbor_index.y,
                neighbor_index.y < 0,
            );

            let bucket_pos = vec2<u32>(neighbor_index) / 8u;
            let bucket_index = bucket_pos.x + bucket_pos.y * gi_params.bucket_size_x;
            let bucket = bucket_info[bucket_index];

            // HLSL `(firstHit.normalTang & 0x7) - 1` — unsigned subtract; a 0
            // normal index would wrap, but a real first-hit always has a
            // populated plane (`normalTang != HIT_NOTHING` gates the caller).
            let first_hit_normal = (first_hit.normal_tang & 0x7u) - 1u;
            let normal_mask = bucket.x & 0x3Fu;
            if ((normal_mask & (1u << first_hit_normal)) == 0u) {
                continue;
            }

            let bucket_min_dist = unpack2x16float(bucket.y & 0xFFFFu).x;
            let bucket_max_dist = unpack2x16float(bucket.y >> 16u).x;
            let dist_fac = 0.95 * max(
                0.2,
                pow(max(dot(first_hit.normal, -first_hit.ray_dir), 0.0), 0.25),
            );
            if (first_hit.dist < bucket_min_dist * dist_fac
                || first_hit.dist > min(bucket_min_dist * 2.0, bucket_max_dist) / dist_fac) {
                continue;
            }

            let bucket_valid_stored = (bucket.x >> 6u) & 0x7u;
            let bucket_lit_ratio = unpack2x16float((bucket.x >> 9u) & 0x7FFFu).x;
            let samples_comp_color_max = (bucket.x >> 24u) & 0x1Fu;
            let samples_color_max = COLORS[samples_comp_color_max];
            let total_sample_count_comp = f32(bucket.x >> 29u) / 7.0;

            if (bucket_valid_stored > 0u) {
                if (i < 6) {
                    valid_bucket_count_small = valid_bucket_count_small + 1;
                    max_color_small = max(max_color_small, samples_color_max);
                    worst_lit_small += (bucket_lit_ratio * bucket_lit_ratio
                        * f32(bucket_valid_stored))
                        * (total_sample_count_comp * total_sample_count_comp);
                    max_color_sum_small += samples_color_max;
                } else {
                    valid_bucket_count_big = valid_bucket_count_big + 1;
                    worst_lit_big += bucket_lit_ratio * bucket_lit_ratio
                        * f32(bucket_valid_stored);
                }
            }
        }

        valid_bucket_count_small = max(1, valid_bucket_count_small);
        valid_bucket_count_big = max(1, valid_bucket_count_big);
        // (The HLSL `if (validBucketCountSmall == 0) maxColorSmall = 100` is
        // dead after the `max(1, ...)` above — ported verbatim, never taken.)

        max_color_sum_small /= f32(valid_bucket_count_small);
        worst_lit_small /= f32(valid_bucket_count_small);
        worst_lit_big /= f32(valid_bucket_count_big);

        let radius_fac_raw = (max(1.0, max_color_small / max_color_sum_small)
            / worst_lit_small)
            * sqrt(worst_lit_big) * gi_params.radius_lit_factor * 0.01;
        radius_fac = 0.07 + clamp(1.0 - (1.0 / (1.0 + radius_fac_raw)), 0.0, 1.0);
    }

    // --- the 12-iteration neighbour-reservoir loop (renderSpatialResampling.fx:153-263)
    var sum_samples: f32 = 0.0;
    for (var i: u32 = 0u; i < sample_count; i = i + 1u) {
        var xy = vec2<f32>(-radius * 0.5)
            + radius * vec2<f32>(next_rand(&rand), next_rand(&rand));
        if (is_varying_radius) {
            xy = xy * radius_fac;
        }

        var neighbor_index = vec2<i32>(vec2<f32>(pixel_pos) + xy);
        neighbor_index.x = select(
            select(neighbor_index.x, 2 * i32(gi_params.screen_width) - neighbor_index.x,
                   neighbor_index.x > i32(gi_params.screen_width)),
            -neighbor_index.x,
            neighbor_index.x < 0,
        );
        neighbor_index.y = select(
            select(neighbor_index.y, 2 * i32(gi_params.screen_height) - neighbor_index.y,
                   neighbor_index.y > i32(gi_params.screen_height)),
            -neighbor_index.y,
            neighbor_index.y < 0,
        );

        let bucket_pos = vec2<u32>(neighbor_index) / 8u;
        let bucket_index = bucket_pos.x + bucket_pos.y * gi_params.bucket_size_x;
        let bucket = bucket_info[bucket_index];

        let first_hit_normal = (first_hit.normal_tang & 0x7u) - 1u;
        let normal_mask = bucket.x & 0x3Fu;
        if ((normal_mask & (1u << first_hit_normal)) == 0u) {
            continue;
        }

        let bucket_min_dist = unpack2x16float(bucket.y & 0xFFFFu).x;
        let bucket_max_dist = unpack2x16float(bucket.y >> 16u).x;
        let dist_fac = 0.95 * max(
            0.2,
            pow(max(dot(first_hit.normal, -first_hit.ray_dir), 0.0), 0.25),
        );
        if (first_hit.dist < bucket_min_dist * dist_fac
            || first_hit.dist > min(bucket_min_dist * 2.0, bucket_max_dist) / dist_fac) {
            continue;
        }

        let bucket_valid_stored = (bucket.x >> 6u) & 0x7u;
        let bucket_lit_ratio = unpack2x16float((bucket.x >> 9u) & 0x7FFFu).x;

        if (bucket_valid_stored == 0u) {
            sum_samples += 1.0;
            continue;
        }

        // HLSL `uint randSampleIndex = bucketValidStored * nextRand(rand)` —
        // implicit float→uint truncation.
        let rand_sample_index = u32(f32(bucket_valid_stored) * next_rand(&rand));
        let neighbor_res = valid_samples_compressed[
            bucket_index * gi_params.refined_bucket_storage_count + rand_sample_index
        ];

        let neighbor: SampleData = get_sample_data(neighbor_res);

        if (first_hit_is_diffuse != neighbor.is_diffuse) {
            continue;
        }

        let is_sky = neighbor.sample_dist == 0.0;

        let path_to_sample_neighbor = neighbor.sample_dir * neighbor.sample_dist;
        let path_to_sample_now_frac =
            (neighbor.vis_pos_frac + path_to_sample_neighbor) - first_hit.pos;
        let path_to_sample_now =
            vec3<f32>(neighbor.vis_pos_int - cam_pos_int) + path_to_sample_now_frac;

        let length_to_sample_squared_now = dot(path_to_sample_now, path_to_sample_now);
        let length_to_sample_squared_neighbor =
            dot(path_to_sample_neighbor, path_to_sample_neighbor);

        let dir_to_sample_now =
            path_to_sample_now * inverseSqrt(length_to_sample_squared_now);
        let dir_to_sample_now_or_sun =
            select(dir_to_sample_now, neighbor.sample_dir, is_sky);
        let cos_theta = dot(first_hit.normal, dir_to_sample_now_or_sun);

        if (cos_theta < 0.0001) {
            continue;
        }

        let pdf_now = select(
            pdf_vndf_isotropic(
                dir_to_sample_now_or_sun, -first_hit.ray_dir,
                first_hit_roughness, first_hit.normal,
            ),
            1.0 / (2.0 * PI),
            first_hit_is_diffuse,
        );
        let pdf_then = select(
            pdf_vndf_isotropic(
                neighbor.sample_dir,
                normalize(
                    vec3<f32>(cam_pos_int - neighbor.vis_pos_int)
                    + (gi_params.cam_pos_frac.xyz - neighbor.vis_pos_frac)
                ),
                first_hit_roughness, first_hit.normal,
            ),
            1.0 / (2.0 * PI),
            first_hit_is_diffuse,
        );
        let pdf_ratio = pdf_now / pdf_then;

        if (pdf_ratio < 0.25 || pdf_ratio > 2.0 || pdf_then < 0.01) {
            continue;
        }

        // The Jacobian compensating for the spatial difference
        // (`renderSpatialResampling.fx:227-237`).
        let jacobian_now =
            dot(neighbor.sample_normal, dir_to_sample_now) * length_to_sample_squared_neighbor;
        let jacobian_neighbor =
            dot(neighbor.sample_normal, neighbor.sample_dir) * length_to_sample_squared_now;
        let jacobian_raw = jacobian_now / (0.00000001 + jacobian_neighbor);
        var jacobian = clamp(jacobian_raw, 0.0, 4.0);
        if (is_sky) {
            jacobian = 1.0;
        }
        if (jacobian > 2.5 || jacobian < 0.3) {
            continue;
        }

        // The sample's base colour (5-bit/channel exponential decode).
        let comp_color = neighbor_res.x & 0x7FFFu;
        var neighbor_color = vec3<f32>(
            COLORS[comp_color & 0x1Fu],
            COLORS[(comp_color >> 5u) & 0x1Fu],
            COLORS[comp_color >> 10u],
        );
        neighbor_color = neighbor_color * bucket_lit_ratio;

        let brdf_neighbor = select(
            get_brdf(
                first_hit_roughness, first_hit_albedo, first_hit_metallic,
                first_hit_perturbed_normal, dir_to_sample_now_or_sun, -first_hit.ray_dir,
            ),
            vec3<f32>(1.0),
            first_hit_is_diffuse,
        );
        let target_function_neighbor = get_target_function_new(
            dir_to_sample_now_or_sun, first_hit.normal, neighbor_color, brdf_neighbor,
        );
        let weight = max(0.0, (1.0 / pdf_then) * target_function_neighbor * jacobian);

        sum_weight += weight;
        sum_samples += 1.0;

        let is_update = next_rand(&rand) * sum_weight < weight;
        if (is_update) {
            selected_color = neighbor_color;
            selected_ray_dir = dir_to_sample_now_or_sun;
            selected_length_to_sample_squared_now = length_to_sample_squared_now;
            selected_is_sky = is_sky;
            selected_bounce_state = neighbor_res.z >> 30u;
        }
    }

    // --- the single visibility check (renderSpatialResampling.fx:266-302) ----
    var total_hit_length: f32 = 0.0;
    var is_hit = false;
    var cur_test_pos_int = first_hit_pos_int;
    var cur_test_pos_frac = first_hit_pos_frac;
    var cur_test_ray_dir = selected_ray_dir;
    for (var i: u32 = 0u; i < 3u; i = i + 1u) {
        var ray_result: RayResult;
        is_hit = shoot_ray(
            cur_test_pos_int, cur_test_pos_frac, cur_test_ray_dir,
            i32(max(gi_params.max_ray_steps_visibility, 1u)),
            &ray_result,
        );
        if (!is_hit || selected_is_sky) {
            break;
        }

        total_hit_length += ray_result.length;
        let cur_voxel_type: VoxelType = decompress_voxel_type(voxel_types[ray_result.hit_type]);
        cur_test_pos_frac += cur_test_ray_dir * ray_result.length
            + ray_result.normal * 0.01;
        cur_test_pos_int += vec3<i32>(floor(cur_test_pos_frac));
        cur_test_pos_frac = cur_test_pos_frac - floor(cur_test_pos_frac);

        let cur_bounce_state = (selected_bounce_state >> i) & 0x1u;
        if (cur_bounce_state == 0u) {
            break;
        }

        // Post-PBR-raymarching: "specular mirror" = a PBR voxel whose
        // sampled-roughness is in the mirror band. Sample MRH.G only on
        // this hit (one triplanar sample, three texture fetches — the
        // visibility loop runs ≤ 3 times so worst-case 9 extra fetches per
        // pixel). Avoid sampling diffuse/normal/emissive (`02-design.md`
        // decision #14).
        var has_specular = false;
        if (cur_voxel_type.material_base == SURFACE_PBR) {
            let cur_world_pos = vec3<f32>(cur_test_pos_int) + cur_test_pos_frac;
            let cur_blend = triplanar_blend_weights(ray_result.normal);
            let cur_layer = select_layer_variant(
                cur_voxel_type.material_layer_index,
                cur_voxel_type.variant_span,
                ray_result.voxel_pos,
            );
            let cur_roughness = triplanar_sample(
                pbr_mrh, pbr_sampler, cur_world_pos, cur_blend, cur_layer,
            ).g;
            has_specular = cur_roughness < MIRROR_ROUGHNESS_EPSILON;
        }
        if (!has_specular) {
            break;
        }

        cur_test_ray_dir = reflect(cur_test_ray_dir, ray_result.normal);
    }
    total_hit_length += 0.15;
    total_hit_length *= 1.04;
    var is_visible = total_hit_length * total_hit_length
        - selected_length_to_sample_squared_now >= 0.0;
    if (selected_is_sky) {
        is_visible = !is_hit;
    }
    if (!is_visible) {
        sum_weight = 0.0;
    }

    // --- the resampled-colour resolve (post-PBR-raymarching) ---------------
    // Both the target-function BRDF and the post-resolve weighting collapse
    // to `eval_pbr` (`02-design.md` § E call-site map).
    let brdf = select(
        get_brdf(
            first_hit_roughness, first_hit_albedo, first_hit_metallic,
            first_hit_perturbed_normal, selected_ray_dir, -first_hit.ray_dir,
        ),
        vec3<f32>(1.0),
        first_hit_is_diffuse,
    );
    let target_function_new = get_target_function_new(
        selected_ray_dir, first_hit_perturbed_normal, selected_color, brdf,
    );
    let average_weight_new = sum_weight
        / max(0.0000000000001, sum_samples * target_function_new);
    var color = average_weight_new * selected_color;
    if (!first_hit_is_diffuse) {
        // Specular weighting: full `eval_pbr` BRDF * incident cos. Uses the
        // perturbed normal so normal-map detail modulates the specular lobe.
        let pbr = eval_pbr(
            selected_ray_dir, -first_hit.ray_dir, first_hit_perturbed_normal,
            first_hit_albedo, first_hit_metallic, first_hit_roughness,
        );
        color *= pbr.f;
    } else {
        // Diffuse weighting: Lambertian against the perturbed normal so the
        // normal-map shading varies in the resampled diffuse output too.
        color *= clamp(dot(first_hit_perturbed_normal, selected_ray_dir), 0.0, 1.0) * (1.0 / PI);
    }

    // --- the sun sample (renderSpatialResampling.fx:321-339) -----------------
    // INDEPENDENT of the refine buffers — this is why direct-sun bounce light
    // lands at end-of-Batch-5 even though the reservoir loop yields nothing
    // until Batch 6 fills `taa_dist_min_max` (see the module header).
    //
    // MULTI-TAP EXTENSION (paper §5.2 limitation — "soft shadows from the sun
    // are not handled during resampling, resulting in slightly increased
    // noise"). The C# reference fires ONE sun-disk cone sample (the
    // `0.9999`-deviation `getUniformHemisphereSample` — a ~0.81° half-angle
    // cone around `skySunDir`) and accumulates the binary visibility result.
    // Generalised here to `gi_params.sun_shadow_taps` independent taps, each
    // with a fresh rand-stream pair; the cone width (`0.9999`) is unchanged.
    // Visibility is accumulated over the N taps and the per-tap weighted
    // contribution is divided by N to keep the expected value identical to
    // the single-tap path (so `sun_shadow_taps == 1` matches C# bit-equivalently
    // modulo the loop-induced rand-stream advancement, which is the same two
    // `next_rand` draws per tap as the original single-tap code did once).
    let n_sun_taps = max(gi_params.sun_shadow_taps, 1u);
    var sun_accum = vec3<f32>(0.0);
    for (var sun_tap: u32 = 0u; sun_tap < n_sun_taps; sun_tap = sun_tap + 1u) {
        let sun_dir_rand = get_uniform_hemisphere_sample(
            vec2<f32>(next_rand(&rand), next_rand(&rand)),
            gi_params.sky_sun_dir.xyz, 0.9999,
        );
        var sun_temp: RayResult;
        let is_sun_blocked = shoot_ray(
            first_hit_pos_int, first_hit_pos_frac, sun_dir_rand,
            i32(max(gi_params.max_ray_steps_sun, 1u)),
            &sun_temp,
        );
        // `sun_dir_cos_theta` uses the perturbed normal so the sun
        // shading varies with the normal map; geometric self-shadowing was
        // already accounted for by `shoot_ray`.
        let sun_dir_cos_theta = clamp(dot(sun_dir_rand, first_hit_perturbed_normal), 0.0, 1.0);
        if (!is_sun_blocked && first_hit.normal_tang != HIT_NOTHING
            && sun_dir_cos_theta > 0.001) {
            // Post-PBR-raymarching: the sun-sample weighting collapses to
            // one branch — the fine-roughness PBR surface gets the full
            // `eval_pbr` BRDF (the C# `0.5 * D*G*F / (4 * cos * cos)`
            // factor reduces to `pbr.f` minus the diffuse term; we keep
            // the full `pbr.f` for energy-conserving sun directional
            // lighting on both specular and rough surfaces). Coarse
            // PBR/emissive falls back to the Lambertian `2 * cos_theta`.
            let is_specular = first_hit_type.material_base == SURFACE_PBR
                && first_hit_roughness < ROUGH_SPECULAR_DIFFUSE_THRESHOLD;
            var weight = vec3<f32>(2.0 * sun_dir_cos_theta);
            if (is_specular) {
                let pbr = eval_pbr(
                    sun_dir_rand, -first_hit.ray_dir, first_hit_perturbed_normal,
                    first_hit_albedo, first_hit_metallic, first_hit_roughness,
                );
                weight = pbr.f * (2.0 * sun_dir_cos_theta);
            }
            sun_accum += gi_params.sun_color.xyz * weight;
        }
    }
    color += sun_accum / f32(n_sun_taps);

    return color;
}

// `calcSpatialResampling` (`renderSpatialResampling.fx:344-399`) —
// `[numthreads(64,1,1)]`.
@compute @workgroup_size(64, 1, 1)
fn calc_spatial_resampling(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let cam_pos_int = gi_params.cam_pos_int.xyz;
    let screen_width = gi_params.screen_width;
    let screen_height = gi_params.screen_height;

    // The indirect-free `[numthreads(64,1,1)]` dispatch covers
    // `ceil(pixel_count / 64)` workgroups — the tail group has lanes past
    // `pixel_count`; guard them.
    if (global_id.x >= screen_width * screen_height) {
        return;
    }

    let pixel_pos = vec2<u32>(global_id.x % screen_width, global_id.x / screen_width);

    // `getRayDir(invCamMatrix, pixelPos, screenWidth, screenHeight, taaJitter)`
    // — the JITTERED ray (`renderSpatialResampling.fx:351`). The spatial pass
    // reconstructs the surface for reservoir merging from a ray that MUST match
    // the jittered ray the G-buffer was encoded with; firing it through the
    // pixel centre every frame leaves it per-frame-constant and inconsistent
    // with the jittered first-hit encoding (`18-taa-fidelity.md` cause #1).
    let ray_dir = get_ray_dir(
        gi_params.inv_view_proj, pixel_pos, screen_width, screen_height,
        gi_params.taa_jitter,
    );

    let first_hit = first_hit_data[pixel_pos.x + pixel_pos.y * screen_width];

    var color = vec3<f32>(0.0, 0.0, 0.0);
    let first_hit_result: FirstHitResult = get_hit_data_from_planes(
        first_hit, cam_pos_int, gi_params.cam_pos_frac.xyz, ray_dir,
    );
    let first_hit_type_index = first_hit.z & 0x7FFFu;
    if (first_hit_result.normal_tang != HIT_NOTHING) {
        // The spatial-resampling Algorithm-2 iteration count (was hardcoded
        // `12u` const, the C# `renderSpatialResampling.fx:359` value). Now a
        // runtime knob via `gi_params.spatial_iter_count`
        // (`21-design-quality-panel.md` §2.1 row 6). Default 12 = paper /
        // C# bit-equivalent. Variance ∝ 1/√N. The `max(_, 1u)` clamp keeps a
        // zero-init `GiSettings` from emitting a 0-iter empty-loop black GI.
        color = sample_neighbors(
            pixel_pos,
            max(gi_params.spatial_iter_count, 1u),
            first_hit_result, first_hit_type_index,
        );
    }

    let absorption_comp = first_hit_absorption[global_id.x];
    let absorption = vec3<f32>(
        unpack2x16float(absorption_comp.x & 0xFFFFu).x,
        unpack2x16float(absorption_comp.x >> 16u).x,
        unpack2x16float(absorption_comp.y).x,
    );
    color = min(color, vec3<f32>(COLORS[26]));

    let is_denoise = (gi_params.flags & GI_FLAG_IS_DENOISE) != 0u;
    if (is_denoise) {
        let first_hit_type: VoxelType =
            decompress_voxel_type(voxel_types[first_hit_type_index]);
        // Post-PBR-raymarching denoise flag: PBR surfaces are conservative
        // "diffuse" here (the per-pixel specular path uses the sampled
        // roughness via `extra_data` further upstream; the denoise flag is
        // only consumed to bias the bilateral filter, so a coarse
        // material-class hint is enough). Emissive surfaces don't need GI
        // denoising — they generate their own light — so flag as
        // not-diffuse to bypass the denoiser pass.
        let first_hit_is_diffuse =
            select(0u, 1u, first_hit_type.material_base == SURFACE_PBR);

        let cur_taa_sample = taa_sample_accum[global_id.x];
        var cur_taa_color = vec3<f32>(0.0, 0.0, 0.0);
        let accum = unpack2x16float(cur_taa_sample.x & 0xFFFFu).x;
        if (accum <= 1.0) {
            cur_taa_color = color;
        } else {
            cur_taa_color = vec3<f32>(
                unpack2x16float(cur_taa_sample.x >> 16u).x,
                unpack2x16float(cur_taa_sample.y & 0xFFFFu).x,
                unpack2x16float(cur_taa_sample.y >> 16u).x,
            );
            cur_taa_color /= accum * dot(absorption, vec3<f32>(1.0, 1.0, 1.0)) + 0.01;
        }

        let cur_color_comp = vec2<u32>(
            pack2x16float(vec2<f32>(color.x, color.y)),
            pack2x16float(vec2<f32>(color.z, 0.0)) & 0xFFFFu,
        );

        var final_val = vec3<u32>(0u, 0u, 0u);
        final_val.x = cur_color_comp.x;
        final_val.y = (cur_color_comp.y & 0xFFFFu)
            | ((pack2x16float(vec2<f32>(dot(cur_taa_color, vec3<f32>(1.0, 1.0, 1.0)), 0.0))
                & 0xFFFFu) << 16u);
        // `type = firstHitIsDiffuse ? 0 : (firstHitTypeIndex & 0xFFF) + 1`.
        let type_field = select((first_hit_type_index & 0xFFFu) + 1u, 0u, first_hit_is_diffuse != 0u);
        final_val.z = first_hit_result.normal_tang | (type_field << 23u);
        // The TRANSPOSED index: `pixelPos.y + pixelPos.x * screenHeight` — the
        // denoiser reads `denoisePreprocessed` column-major.
        // `denoise_preprocessed` is the `vec4<u32>`-padded `Uint3` buffer
        // (§3.3) — write `.w = 0` padding.
        denoise_preprocessed[pixel_pos.y + pixel_pos.x * screen_height] =
            vec4<u32>(final_val, 0u);
    } else {
        let final_col_comp = final_color[global_id.x];
        var final_col = vec3<f32>(
            unpack2x16float(final_col_comp.x & 0xFFFFu).x,
            unpack2x16float(final_col_comp.x >> 16u).x,
            unpack2x16float(final_col_comp.y).x,
        );
        final_col += absorption * color;
        final_col = min(final_col, vec3<f32>(COLORS[26]));
        final_color[global_id.x] = vec2<u32>(
            pack2x16float(vec2<f32>(final_col.x, final_col.y)),
            pack2x16float(vec2<f32>(final_col.z, 0.0)) & 0xFFFFu,
        );
    }
}
