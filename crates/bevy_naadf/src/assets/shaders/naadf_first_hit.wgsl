// naadf_first_hit.wgsl — the Phase-B 4-plane-bounce first-hit compute pass.
//
// Derives from: render/versions/base/renderFirstHit.fx `calcFirstHit`
// (`09-design-b.md` §6, §5.1). REPLACES the Phase-A/A-2 single-plane first-hit:
// the old single-plane path is now the `i == 0` iteration of the 4-iteration
// specular-bounce loop (`base/renderFirstHit.fx:65-115`).
//
// What this pass does (`base/renderFirstHit.fx`):
//   * per-pixel ray setup (jittered + un-jittered), `rayAABB` volume clip;
//   * a `for (i = 0; i < 4; ++i)` loop: `shootRay`, fill `normTangs[i]`;
//     - on a MISS: `applyAtmosphere(oldPos, rayDirNoJitter|rayDir)` then break;
//     - on a HIT: advance the ray to the surface; if `isAtmosphereInteraction`,
//       `addLightForDirection` along the segment travelled; decompress the
//       voxel type;
//       - non-mirror surface: apply albedo (unless `SURFACE_SPECULAR_ROUGH`),
//         add emissive, set `distanceRay` / `voxelTypeRaw` / `isDiffuse`, break;
//       - mirror surface: `absorption *= getReflectanceFresnel`, `reflect` the
//         ray, continue to the next plane;
//     - if all 4 iterations run without a non-mirror hit: `normTangs[3] =
//       0x1FFFF`, `distanceRay = -1`;
//   * on a volume MISS entirely: `applyAtmosphere(camPosInt + camPosFrac, rayDir)`;
//   * write `firstHitData` + `firstHitAbsorption` + `finalColor`.
//
// Divergences from Phase A/A-2 (`09-design-b.md` §6.3 — the central restructure):
//   * The `base/` first-hit writes `firstHitData` + `firstHitAbsorption` +
//     `finalColor` and does NOT write `taa_sample_accum` or the `taa_samples`
//     ring (verified `base/renderFirstHit.fx:126-128`). The A-2 `taa_samples`
//     ring write + the `taa_sample_accum` write are REMOVED here — they move to
//     `base/renderTaaSampleReverse.fx`'s `ReprojectOld` + `CalcNewTaaSample`
//     passes (Batch 6). `taa_sample_accum` stays bound at `@group(1) @binding(3)`
//     for frame-layout stability (the reproject node + blit still reference the
//     buffer through their own layouts) but this pass no longer writes it.
//   * The `@group(2)` `taa_samples` ring binding is REMOVED from the first-hit
//     pipeline layout — it moves onto the `calc_new_taa_sample` pipeline
//     (Batch 6). The first-hit pipeline layout becomes `[world, frame,
//     atmosphere]` (`09-design-b.md` §6.3) — the precomputed atmosphere takes
//     the freed `@group(2)` slot (NOT `@group(3)` — §6.3 explicitly removes the
//     taa group so the layout vec has exactly 3 entries; §4.4's "@group(3)" is
//     the stale variant where the taa group stays).
//   * New `@group(2)` atmosphere: `atmosphere_params` (uniform) +
//     `atmosphere_comp` (read-only storage) — `applyAtmosphere` (miss) and
//     `addLightForDirection` (the atmosphere-interaction path) need them.
//   * The inline Phase-A sun+ambient term is GONE — the `base/` first-hit gets
//     all sky light from the full multiple-scattering atmosphere model.
//   * `is_diffuse` is now a real per-hit value (not hardcoded `1u`).
//   * Entity branches (`#ifdef ENTITIES`) are omitted — Phase B is entity-free.
//
// `[numthreads(64,1,1)]` in the HLSL → `@workgroup_size(64,1,1)`.

#import "shaders/render_pipeline_common.wgsl"::{
    GpuCamera, GpuRenderParams, VoxelType, decompress_voxel_type, get_ray_dir,
    compress_first_hit_data,
    HIT_NOTHING, HIT_UNDEFINED, ENTITY_FREE,
    SURFACE_PBR, SURFACE_EMISSIVE,
    FLAG_SHOW_RAY_STEP, FLAG_IS_ATMOSPHERE_INTERACTION,
}
#import "shaders/ray_tracing.wgsl"::{
    RayResult, ray_aabb, shoot_ray,
}
#import "shaders/world_data.wgsl"::{
    voxel_types, world_meta,
    pbr_diffuse_ao, pbr_normal, pbr_mrh, pbr_emissive, pbr_sampler,
}
#import "shaders/atmosphere.wgsl"::{
    AtmosphereParams, AtmoLight, apply_atmosphere, atmosphere_oct_index,
    add_light_for_direction,
}
#import "shaders/pbr_sampling.wgsl"::{
    triplanar_blend_weights, triplanar_sample, triplanar_sample_normal,
    triplanar_sample_pom, triplanar_sample_normal_pom,
    pom_compute, pom_self_shadow,
    select_layer_variant,
    eval_pbr, PbrEval,
    PbrDebugInputs, debug_view_override,
    MIRROR_ROUGHNESS_EPSILON, ROUGH_SPECULAR_DIFFUSE_THRESHOLD,
}
#import "shaders/common.wgsl"::PI

// --- @group(1) — frame data -------------------------------------------------

@group(1) @binding(0) var<uniform> camera: GpuCamera;
@group(1) @binding(1) var<uniform> params: GpuRenderParams;
// The G-buffer — one `vec4<u32>` per pixel (`09-design-b.md` §3.4 / §6.2).
@group(1) @binding(2) var<storage, read_write> first_hit_data: array<vec4<u32>>;
// `taa_sample_accum` — kept bound for frame-layout stability (the reproject
// node + final blit reference this buffer through their own layouts). The
// `base/` first-hit does NOT write it (`09-design-b.md` §6.3) — `ReprojectOld`
// + `CalcNewTaaSample` do (Batch 6). Touched once below so naga keeps the
// binding in the layout.
@group(1) @binding(3) var<storage, read_write> taa_sample_accum: array<vec2<u32>>;
// `firstHitAbsorption` — per-pixel accumulated transmittance along the primary
// ray path (`base/renderFirstHit.fx:7,127`). One `vec2<u32>` per pixel: three
// f16s `(absorption.x, absorption.y, absorption.z)`.
@group(1) @binding(4) var<storage, read_write> first_hit_absorption: array<vec2<u32>>;
// `finalColor` — the GI working-colour buffer (`base/renderFirstHit.fx:8,128`).
// The first-hit writes the primary-ray light here; later GI passes thread their
// result through it. One `vec2<u32>` per pixel: three f16s `(light.x, light.y,
// light.z)`.
@group(1) @binding(5) var<storage, read_write> final_color: array<vec2<u32>>;

// --- @group(2) — the precomputed atmosphere ---------------------------------
// `atmosphere_comp` — the octahedral precomputed sky buffer (written by
// `naadf_atmosphere.wgsl`); `atmosphere_params` — the sky-model uniform. The
// first-hit fetches `atmosphere_comp[atmosphere_oct_index(...)]` itself and
// passes the slot value into `apply_atmosphere` (WGSL forbids passing a
// `ptr<storage,...>` into a function — `09-design-b.md` §12 #3 / atmosphere.wgsl).
@group(2) @binding(0) var<uniform> atmosphere_params: AtmosphereParams;
@group(2) @binding(1) var<storage, read> atmosphere_comp: array<vec4<u32>>;

@compute @workgroup_size(64, 1, 1)
fn calc_first_hit(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let pixel_index = global_id.x;
    if (pixel_index >= params.screen_width * params.screen_height) {
        return;
    }

    let cam_pos_int = camera.cam_pos_int;
    let cam_pos_frac = camera.cam_pos_frac;

    let pixel_pos = vec2<u32>(
        pixel_index % params.screen_width,
        pixel_index / params.screen_width,
    );
    // `getRayDir(invCamMatrix, pixelPos, w, h, taaJitter)` — the jittered ray;
    // `rayDirNoJitter` is the un-jittered ray (`base/renderFirstHit.fx:37-38`).
    var ray_dir = get_ray_dir(
        camera.inv_view_proj,
        pixel_pos,
        params.screen_width,
        params.screen_height,
        params.taa_jitter,
    );
    let ray_dir_no_jitter = get_ray_dir(
        camera.inv_view_proj,
        pixel_pos,
        params.screen_width,
        params.screen_height,
        vec2<f32>(0.0, 0.0),
    );

    // `rayAABB(camPosInt + camPosFrac, rayDir, boundingBoxMin, boundingBoxMax, ...)`
    // — clip the ray to the world volume. Bounds come from `world_meta`
    // (`@group(0)`), the 0.1-voxel-inset world extent (`WorldData.cs:477-478`).
    let bbox_min = world_meta.bounding_box_min;
    let bbox_max = world_meta.bounding_box_max;
    let cam_pos_world = vec3<f32>(cam_pos_int) + cam_pos_frac;
    let volume = ray_aabb(cam_pos_world, ray_dir, bbox_min, bbox_max);

    // The atmosphere accumulator — HLSL's `inout float3 absorption, light`
    // become this `AtmoLight` in/out value (`atmosphere.wgsl`).
    var acc: AtmoLight;
    acc.absorption = vec3<f32>(1.0, 1.0, 1.0);
    acc.light = vec3<f32>(0.0, 0.0, 0.0);

    // A `function`-space copy of the atmosphere uniform — `add_light_for_direction`
    // takes a `ptr<function, AtmosphereParams>` (WGSL forbids pointing at a
    // uniform var). Copied once here, not per loop iteration.
    var atmo_params = atmosphere_params;

    var norm_tangs = array<u32, 4>(HIT_NOTHING, HIT_UNDEFINED, HIT_UNDEFINED, HIT_UNDEFINED);
    var voxel_type_raw: u32 = 0u;
    var is_diffuse: u32 = 1u;
    var distance_ray: f32 = -1.0;
    var ray_result: RayResult;
    ray_result.step_count = 0;
    var cur_pos_int = cam_pos_int;
    var cur_pos_frac = cam_pos_frac;
    let entity = ENTITY_FREE;

    // PBR rendering debugger — when `params.debug_view_mode != 0u`, the
    // first-hit branches that terminate on a PBR surface (rough-PBR break +
    // emissive fast-path) populate `debug_color` with the per-pixel debug
    // RGB. After the loop, the value is stomped into `final_color` and
    // `taa_sample_accum`, and `acc.absorption` is cleared so downstream
    // GI / spatial-resampling / sun-direct multiplications all contribute
    // zero light. Mode 0 leaves `debug_active = false` and the production
    // path is untouched (zero perf cost — one uniform load + one compare
    // dead-code-eliminated by the WGSL compiler). See
    // `docs/orchestrate/pbr-raymarching/05-diagnostic.md` § "PBR rendering
    // debugger".
    var debug_active: bool = false;
    var debug_color: vec3<f32> = vec3<f32>(0.0);

    if (volume.hit) {
        // `oldPos = curPosInt + curPosFrac` BEFORE advancing to the volume
        // entry point (`base/renderFirstHit.fx:58`) — the camera-int-relative
        // world position the atmosphere functions march from.
        var old_pos = vec3<f32>(cur_pos_int) + cur_pos_frac;

        // Advance the ray origin to the volume entry point, re-splitting into
        // int + frac (all ray math stays in int+frac — D1).
        cur_pos_frac = cam_pos_frac + ray_dir * volume.dist_min_max.x;
        cur_pos_int = cur_pos_int + vec3<i32>(floor(cur_pos_frac));
        cur_pos_frac = cur_pos_frac - floor(cur_pos_frac);

        var dist: f32 = 0.0;
        // The 4-iteration specular-bounce loop (`base/renderFirstHit.fx:65-115`).
        // WGSL has no `[unroll]`; naga unrolls this small constant loop. `i` is
        // declared outside so the post-loop `i == 4u` test (`:117`) can read it.
        var i: u32 = 0u;
        loop {
            if (i >= 4u) {
                break;
            }

            let is_hit = shoot_ray(
                cur_pos_int, cur_pos_frac, ray_dir,
                i32(max(params.max_ray_steps_primary, 1u)),
                &ray_result,
            );
            norm_tangs[i] = ray_result.normal_comp;

            if (!is_hit) {
                // MISS — fold the precomputed atmosphere in along `rayDir`
                // (`rayDirNoJitter` for `i == 0`, `rayDir` otherwise — `:73`).
                // `applyAtmosphere`'s HLSL body ignores its `pos` arg (it only
                // uses `rayDir`), so the port fetches the octahedral slot for
                // the appropriate direction and folds it into `acc`.
                let miss_dir = select(ray_dir, ray_dir_no_jitter, i == 0u);
                let oct_index = atmosphere_oct_index(
                    miss_dir,
                    atmosphere_params.atmosphere_tex_size_x,
                    atmosphere_params.atmosphere_tex_size_y,
                );
                acc = apply_atmosphere(atmosphere_comp[oct_index], acc, 1.0);
                break;
            }

            // Advance the ray to the new surface (`:78-81`).
            dist += ray_result.length;
            cur_pos_frac = cur_pos_frac + ray_dir * ray_result.length
                + ray_result.normal * 0.01;
            cur_pos_int = cur_pos_int + vec3<i32>(floor(cur_pos_frac));
            cur_pos_frac = cur_pos_frac - floor(cur_pos_frac);

            // `addLightForDirection` along the segment travelled, when the
            // atmosphere-interaction flag is set (`:85-86`). The HLSL passes
            // `false, 3, 3` for `includeMie / mainIterationCount /
            // secondIterationCount`.
            if ((params.flags & FLAG_IS_ATMOSPHERE_INTERACTION) != 0u) {
                let new_pos = vec3<f32>(cur_pos_int) + cur_pos_frac;
                acc = add_light_for_direction(
                    &atmo_params,
                    old_pos,
                    ray_dir,
                    distance(new_pos, old_pos),
                    acc,
                    false,
                    3u,
                    3u,
                );
            }

            let voxel_type: VoxelType =
                decompress_voxel_type(voxel_types[ray_result.hit_type]);

            // PBR-raymarching pivot: every hit goes through the unified PBR
            // path or the Emissive fast-path. `material_base` is the only
            // surviving branch (`02-design.md` § E call-site map).
            //
            // World-space hit position for triplanar UV — `cur_pos_int +
            // cur_pos_frac` is the camera-int-relative world position
            // (`02-design.md` assumption #5). The triplanar functions only
            // need a position whose mod-1 footprint tiles the texture; the
            // camera-int offset is constant per frame so the tiling shifts
            // uniformly — the visible texture is identical to the
            // world-absolute case.
            let hit_world_pos = vec3<f32>(cur_pos_int) + cur_pos_frac;
            let face_normal = ray_result.normal;
            let blend_weights = triplanar_blend_weights(face_normal);
            let layer = select_layer_variant(
                voxel_type.material_layer_index,
                voxel_type.variant_span,
                ray_result.voxel_pos,
            );

            // Emissive fast-path — skip the BRDF, sample the Emissive
            // texture, multiply by per-VoxelType `color_layered` (HDR), add
            // and terminate (`02-design.md` § H).
            if (voxel_type.material_base == SURFACE_EMISSIVE) {
                let emissive_sample = triplanar_sample(
                    pbr_emissive, pbr_sampler,
                    hit_world_pos, blend_weights, layer,
                ).rgb;
                let emissive_full = emissive_sample * voxel_type.color_layered;
                acc.light = acc.light + acc.absorption * emissive_full;
                distance_ray = dist + volume.dist_min_max.x;
                voxel_type_raw = ray_result.hit_type;
                is_diffuse = 1u;
                // Debug view — emissive fast-path inputs. Most BRDF channels
                // are degenerate for emissive (no metallic / roughness /
                // POM), so fill plausible defaults. Mode 17 (Emissive) shows
                // the actual emissive contribution; modes 1/2/3/16 still
                // produce signal.
                if (params.debug_view_mode != 0u) {
                    var dbg_in: PbrDebugInputs;
                    dbg_in.albedo               = vec3<f32>(0.0);
                    dbg_in.normal_perturbed     = face_normal;
                    dbg_in.normal_geometric     = face_normal;
                    dbg_in.metallic             = 0.0;
                    dbg_in.roughness            = 1.0;
                    dbg_in.ao                   = 1.0;
                    dbg_in.height               = 0.5;
                    dbg_in.f_base               = vec3<f32>(0.04);
                    dbg_in.f_fresnel            = vec3<f32>(0.04);
                    dbg_in.k_d                  = vec3<f32>(1.0);
                    dbg_in.direct_contribution  = vec3<f32>(0.0);
                    dbg_in.gi_proxy             = emissive_full;
                    dbg_in.self_shadow          = 1.0;
                    dbg_in.displaced_uv         = vec2<f32>(0.0);
                    dbg_in.material_layer_index = layer;
                    dbg_in.triplanar_weights    = blend_weights;
                    dbg_in.emissive             = emissive_full;
                    debug_color  = debug_view_override(params.debug_view_mode, dbg_in);
                    debug_active = true;
                }
                break;
            }

            // PBR hit — sample MRH (geometric uv), then POM-displace the
            // dominant plane's UV from the MRH.B height channel, then
            // re-sample MRH / diffuse / normal with POM applied on the
            // dominant plane. POM changes the SHADING-INPUT UVs only — the
            // geometric hit position written into the G-buffer
            // (`compress_first_hit_data` below uses `distance_ray` +
            // `norm_tangs`, both untouched by POM) is unaffected.
            //
            // Modern POM (`05-diagnostic.md` "POM rewrite — modern
            // implementation"): adaptive step count (8-32 view-angle-
            // dependent), linear-interpolation refine, soft-clip
            // displacement, plus a secondary `pom_self_shadow` march toward
            // the sun in the dominant plane's tangent space.
            //
            // ALL POM math lives in `pom_compute` — the helper returns a
            // single canonical `PomCompute { displaced_uv, dominant_axis,
            // height }`. Every downstream sample call MUST consume the
            // SAME `displaced_uv` + `dominant_axis` to keep the
            // first-hit-pass texture sampling consistent. GI /
            // spatial_resampling MUST call `pom_compute` themselves with
            // the same inputs when re-shading the first-hit surface
            // (see `05-diagnostic.md` § "POM seam-artifact diagnose+fix"
            // for the seam-moiré root cause this consolidation closes).
            let pom = pom_compute(
                pbr_mrh, pbr_sampler,
                hit_world_pos, ray_dir, blend_weights, layer,
            );
            let displaced_uv  = pom.displaced_uv;
            let dominant_axis = pom.dominant_axis;
            let mrh = triplanar_sample_pom(
                pbr_mrh, pbr_sampler,
                hit_world_pos, blend_weights, layer, dominant_axis, displaced_uv,
            );
            let sampled_metallic = mrh.r;
            let sampled_roughness = mrh.g;

            let diffuse_ao = triplanar_sample_pom(
                pbr_diffuse_ao, pbr_sampler,
                hit_world_pos, blend_weights, layer, dominant_axis, displaced_uv,
            );
            // sRGB-decoded albedo × per-VoxelType tint, × per-voxel-face AO.
            let sampled_albedo = diffuse_ao.rgb * voxel_type.albedo_tint * diffuse_ao.a;

            // Perturbed (normal-mapped) surface normal — RNM-blended tangent
            // normals lifted into world space. Replaces the geometric
            // axis-aligned face normal for every BRDF call below (mirror
            // Schlick + reflection axis), so the normal map is visible.
            let perturbed_normal = triplanar_sample_normal_pom(
                pbr_normal, pbr_sampler,
                hit_world_pos, blend_weights, face_normal,
                layer, dominant_axis, displaced_uv,
            );

            // POM self-shadow — secondary march from the displaced surface
            // point toward the sun. The shadow factor folds into
            // `acc.absorption` so downstream GI / sun-direct shading on
            // this pixel sees the attenuated direct light. The valleys of
            // the heightfield receive `(1 - SHADOW_STRENGTH) × full` light;
            // the peaks see full light. The G-buffer encode is unchanged
            // (the shadow factor passes through `first_hit_absorption`,
            // a buffer that already exists and is already written every
            // pixel).
            //
            // The shadow attenuation multiplies ALL downstream radiance
            // through the absorption chain (sun direct + sky bounce +
            // GI). Strictly the shadow factor should attenuate sun direct
            // only — doing so requires either a new G-buffer slot or a
            // POM re-evaluation in GI / spatial_resampling, both excluded
            // by the rewrite brief. The current approximation is
            // visually defensible: shadowed POM valleys are also less
            // sky-exposed (the local microgeometry partially occludes
            // the sky), so attenuating the sky bounce alongside the sun
            // is geometrically reasonable.
            let pom_shadow = pom_self_shadow(
                pbr_mrh, pbr_sampler,
                displaced_uv, pom.height,
                atmosphere_params.sky_sun_dir,
                layer, dominant_axis,
            );

            // Polished metal / glass — re-enter the existing 4-iteration
            // perfect-reflect mirror loop. Schlick Fresnel weights the
            // absorption; the mirror reflection direction stays exactly the
            // same as the prior C# mirror branch (`02-design.md` decision
            // #14). Reuses the `mix(0.04, albedo, metallic)` F0 to make
            // metallic mirrors retain their metallic tint.
            //
            // Mirror Fresnel + reflect axis use the PERTURBED normal so the
            // normal map shows up even on near-mirror metals.
            if (sampled_roughness < MIRROR_ROUGHNESS_EPSILON) {
                let cos_theta = clamp(dot(perturbed_normal, -ray_dir), 0.0, 1.0);
                let f_base = mix(vec3<f32>(0.04), sampled_albedo, sampled_metallic);
                let one_minus_ct = 1.0 - cos_theta;
                let r = f_base + (vec3<f32>(1.0) - f_base) * pow(one_minus_ct, 5.0);
                // POM self-shadow attenuates the Fresnel weight: the
                // reflected mirror ray carries the same shadow factor as
                // the rough-PBR break path.
                acc.absorption = acc.absorption * r * pom_shadow;
                ray_dir = reflect(ray_dir, perturbed_normal);
                old_pos = vec3<f32>(cur_pos_int) + cur_pos_frac;
                i = i + 1u;
                continue;
            }

            // Rough PBR — terminate the primary-ray bounce, defer the
            // shading to the GI pass. Apply `(1-metallic)*albedo`
            // absorption (diffuse colour transport — the specular lobe
            // contribution is added back by the GI pass per `eval_pbr`).
            // `is_diffuse=0` for fine-roughness surfaces (VNDF sampling
            // wins); `is_diffuse=1` above the threshold (uniform-hemisphere
            // wins). `02-design.md` decision #7.
            //
            // POM self-shadow folds into the absorption: shadowed valleys
            // attenuate downstream radiance by `(1 - SHADOW_STRENGTH)`.
            let albedo_attenuation = (vec3<f32>(1.0) - vec3<f32>(sampled_metallic))
                * sampled_albedo;
            acc.absorption = acc.absorption * albedo_attenuation * pom_shadow;
            distance_ray = dist + volume.dist_min_max.x;
            voxel_type_raw = ray_result.hit_type;
            is_diffuse = select(
                1u, 0u, sampled_roughness < ROUGH_SPECULAR_DIFFUSE_THRESHOLD,
            );

            // PBR rendering debugger — populate `PbrDebugInputs` with the
            // values we just computed for the production path, plus a couple
            // of derived BRDF quantities for the kS / kD / direct-only /
            // GI-only modes. Skips entirely when `debug_view_mode == 0u`.
            // See `docs/orchestrate/pbr-raymarching/05-diagnostic.md` §
            // "PBR rendering debugger".
            if (params.debug_view_mode != 0u) {
                let sun_dir = atmosphere_params.sky_sun_dir;
                let view_dir = -ray_dir;
                let n_dot_l = clamp(dot(perturbed_normal, sun_dir), 0.0, 1.0);
                let pbr_eval = eval_pbr(
                    sun_dir, view_dir, perturbed_normal,
                    sampled_albedo, sampled_metallic, sampled_roughness,
                );
                let direct = atmosphere_params.sky_sun_color
                    * pbr_eval.f * n_dot_l * pom_shadow;
                // GI proxy: atmosphere fold (in `acc.light`) plus the
                // diffuse colour transport this branch just put into
                // `acc.absorption`. Coarse but visually informative.
                let gi_proxy = acc.light + albedo_attenuation;
                var dbg_in: PbrDebugInputs;
                dbg_in.albedo               = sampled_albedo;
                dbg_in.normal_perturbed     = perturbed_normal;
                dbg_in.normal_geometric     = face_normal;
                dbg_in.metallic             = sampled_metallic;
                dbg_in.roughness            = sampled_roughness;
                dbg_in.ao                   = diffuse_ao.a;
                dbg_in.height               = mrh.b;
                dbg_in.f_base               = pbr_eval.f_zero;
                dbg_in.f_fresnel            = pbr_eval.fresnel;
                dbg_in.k_d                  = (vec3<f32>(1.0) - pbr_eval.fresnel)
                                              * (1.0 - sampled_metallic);
                dbg_in.direct_contribution  = direct;
                dbg_in.gi_proxy             = gi_proxy;
                dbg_in.self_shadow          = pom_shadow;
                dbg_in.displaced_uv         = displaced_uv;
                dbg_in.material_layer_index = layer;
                dbg_in.triplanar_weights    = blend_weights;
                dbg_in.emissive             = vec3<f32>(0.0);
                debug_color  = debug_view_override(params.debug_view_mode, dbg_in);
                debug_active = true;
            }
            break;
        }

        // All 4 iterations ran without a non-mirror hit (`:117-121`).
        if (i == 4u) {
            norm_tangs[3] = 0x1FFFFu;
            distance_ray = -1.0;
        }
    } else {
        // Volume miss entirely — `applyAtmosphere(camPosInt + camPosFrac,
        // rayDir, ...)` (`base/renderFirstHit.fx:124`). `applyAtmosphere`
        // ignores `pos`, so only `rayDir` matters here.
        let oct_index = atmosphere_oct_index(
            ray_dir,
            atmosphere_params.atmosphere_tex_size_x,
            atmosphere_params.atmosphere_tex_size_y,
        );
        acc = apply_atmosphere(atmosphere_comp[oct_index], acc, 1.0);
    }

    // --- PBR debug-view stomp ----------------------------------------------
    // When the debugger is active for this pixel, overwrite `acc.light`
    // with the debug colour and clear `acc.absorption` so downstream
    // GI / spatial-resampling / sun-direct multiplications produce no
    // additional light. The `taa_sample_accum` write below seeds the TAA
    // history with the debug colour at weight=1 so the blit reads it
    // crisply this frame (without it the blit's
    // `cur_color = rgb / max(1, weight)` would mix in stale history).
    if (debug_active) {
        acc.light = debug_color;
        acc.absorption = vec3<f32>(0.0);
    }

    // --- G-buffer + absorption + colour writes -----------------------------
    // `base/renderFirstHit.fx:126-128`:
    //   firstHitData[id]       = compressFirstHitData(distanceRay, normTangs,
    //                              showRayStep ? stepCount : voxelTypeRaw,
    //                              isDiffuse, entity)
    //   firstHitAbsorption[id] = uint2(f16(abs.x)|f16(abs.y)<<16, f16(abs.z))
    //   finalColor[id]         = uint2(f16(light.x)|f16(light.y)<<16, f16(light.z))
    let norm_tangs_vec = vec4<u32>(
        norm_tangs[0], norm_tangs[1], norm_tangs[2], norm_tangs[3],
    );
    // The `showRayStep` debug stuffs the raw step count into `voxelTypeRaw`.
    let voxel_type_or_step = select(
        voxel_type_raw,
        u32(ray_result.step_count),
        (params.flags & FLAG_SHOW_RAY_STEP) != 0u,
    );
    first_hit_data[pixel_index] = compress_first_hit_data(
        distance_ray, norm_tangs_vec, voxel_type_or_step, is_diffuse, entity,
    );

    let absorption = acc.absorption;
    first_hit_absorption[pixel_index] = vec2<u32>(
        pack2x16float(vec2<f32>(absorption.x, absorption.y)),
        pack2x16float(vec2<f32>(absorption.z, 0.0)),
    );

    let light = acc.light;
    final_color[pixel_index] = vec2<u32>(
        pack2x16float(vec2<f32>(light.x, light.y)),
        pack2x16float(vec2<f32>(light.z, 0.0)),
    );

    // Keep `taa_sample_accum` referenced so naga retains the binding in the
    // frame layout — the `base/` first-hit no longer writes it (`ReprojectOld`
    // + `CalcNewTaaSample` do, Batch 6). PBR-debugger: when active, stomp
    // the debug colour into the TAA accumulator with weight=1.0 so the
    // final blit's `cur_color = rgb / max(1, weight)` returns it bitwise.
    // Mirrors the encoding in `naadf_final.wgsl::fragment`:
    //   .x = pack16(weight, R), .y = pack16(G, B)
    if (debug_active) {
        let r = clamp(debug_color.x, 0.0, 65000.0);
        let g = clamp(debug_color.y, 0.0, 65000.0);
        let b = clamp(debug_color.z, 0.0, 65000.0);
        taa_sample_accum[pixel_index] = vec2<u32>(
            pack2x16float(vec2<f32>(1.0, r)),
            pack2x16float(vec2<f32>(g, b)),
        );
    } else if (pixel_index == 0xFFFFFFFFu) {
        taa_sample_accum[pixel_index] = vec2<u32>(0u, 0u);
    }
}
