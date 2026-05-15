// naadf_global_illum.wgsl — the compressed-ReSTIR GI secondary-ray tracer.
//
// Derives from: render/versions/base/renderGlobalIllum.fx `calcGlobalIlum`
// (`09-design-b.md` §5.1, §5.5, §8.1). The first GI dispatch: it takes the
// adaptive ray queue `rayQueueCalc` built, traces a ≤3-bounce secondary ray
// per queued pixel, and classifies the result as a *lit* sample (radiance > 0,
// `compressSampleValid` → `valid_samples`) or an *unlit* sample (every 8th one
// stored, `compressSampleInvalid` → `invalid_samples`). The per-frame lit/unlit
// totals go into the 128-frame `sample_counts` accumulation ring.
//
// `[numthreads(64,1,1)]` dispatched INDIRECT off `ray_queue_indirect` — one
// thread per *queued* pixel, so GI cost scales with the ~0.25-spp adaptive rate
// (`WorldRenderBase.cs:323` `DispatchComputeIndirect`).
//
// GROUP-SHARED ATOMICS (`renderGlobalIllum.fx:30-32,254-268`): HLSL
// `groupshared uint sharedResCount / globalResCountValid / globalResCountInvalid`
// + `InterlockedAdd` + `GroupMemoryBarrierWithGroupSync` → WGSL
// `var<workgroup> ...: atomic<u32>` + `atomicAdd` + `workgroupBarrier()`. The
// per-thread `InterlockedAdd(globalIlumSampleCounts[3+accumIndex].x|.y, ...)` is
// a STORAGE-buffer atomic — `sample_counts` is declared `array<SampleCountSlot>`
// where `SampleCountSlot { valid: atomic<u32>, invalid: atomic<u32> }`
// (`09-design-b.md` §5.5 / §12 #5). HLSL `groupshared` is zero-initialised at
// module scope; WGSL `var<workgroup>` is NOT — lane 0 zeroes the three
// workgroup atomics before the first barrier.
//
// ENTITY BRANCHES (`#ifdef ENTITIES`) are OMITTED — Phase B is entity-free
// (`09-design-b.md` §1): `entitySample` is always `ENTITY_FREE`,
// `entityInstancesHistory` is never bound, the `getHitDataFromPlanes` entity
// params are absent (the shared full version in `render_pipeline_common.wgsl`
// already drops them).
//
// Per the `09-design-b.md` Batch-3 carry-forward: explicit `u32()` / `i32()`
// casts wherever the HLSL relies on implicit float→int truncation.
//
// naga-oil import module — pulls `@group(0)` world bindings in via
// `ray_tracing.wgsl`.

#import "shaders/gi_params.wgsl"::GpuGiParams
#import "shaders/render_pipeline_common.wgsl"::{
    VoxelType, SampleValid, FirstHitResult,
    decompress_voxel_type, get_ray_dir, get_hit_data_from_planes,
    get_reflectance_fresnel,
    HIT_NOTHING, HIT_UNDEFINED, ENTITY_FREE,
    SURFACE_EMISSIVE, SURFACE_SPECULAR_ROUGH, SURFACE_SPECULAR_MIRROR,
}
#import "shaders/ray_tracing.wgsl"::{
    RayResult, shoot_ray,
}
#import "shaders/ray_tracing_common.wgsl"::{
    init_rand, next_rand, next_rand2,
    oct_encode, get_uniform_hemisphere_sample, sample_vndf_isotropic, geometry_term,
}
#import "shaders/color_compression.wgsl"::compress_color
#import "shaders/atmosphere.wgsl"::{
    AtmosphereParams, AtmoLight, apply_atmosphere, atmosphere_oct_index,
}
#import "shaders/world_data.wgsl"::voxel_types
#import "shaders/common.wgsl"::PI

// --- @group(1) — the GI-specific bindings -----------------------------------

@group(1) @binding(0) var<uniform> gi_params: GpuGiParams;
// `firstHitData` — the G-buffer, read-only.
@group(1) @binding(1) var<storage, read> first_hit_data: array<vec4<u32>>;
// `firstHitAbsorption` — declared `RWStructuredBuffer` in the HLSL
// (`renderGlobalIllum.fx:7`) but `calcGlobalIlum` never writes it; bound
// read_write for layout stability with the other GI passes that do touch it.
@group(1) @binding(2) var<storage, read_write> first_hit_absorption: array<vec2<u32>>;
// `globalIlumValidSamples` — the lit-sample ring (`pixel_count * 2` ×
// `SampleValid`). Written here (wrapping ring write).
@group(1) @binding(3) var<storage, read_write> valid_samples: array<SampleValid>;
// `globalIlumInvalidSamples` — the unlit-sample ring (`pixel_count * 8` ×
// `vec4<u32>`). Written here (wrapping ring write).
@group(1) @binding(4) var<storage, read_write> invalid_samples: array<vec4<u32>>;
// `globalIlumSampleCounts` — the 128-frame accumulation ring (`128 + 3` slots).
// Slot `[3 + accumIndex]` is this frame's `(validCount, invalidCount)`,
// atomically incremented; slot `[0]` is the ring's current write cursors. Each
// slot is a `SampleCountSlot` so the per-thread `InterlockedAdd` is legal.
@group(1) @binding(5) var<storage, read_write> sample_counts: array<SampleCountSlot>;
// `finalColor` — declared in the HLSL (`renderGlobalIllum.fx:12`) but
// `calcGlobalIlum` does not write it; bound read_write for layout stability.
@group(1) @binding(6) var<storage, read_write> final_color: array<vec2<u32>>;
// `pixelsToRender` — the ray queue (read-only here; `rayQueueCalc` filled it).
@group(1) @binding(7) var<storage, read> ray_queue: array<u32>;
// `camRotOld` / `taaOldCamPosFromCurCamInt` / `taaJitterOld` — the 128-deep
// camera-history ring. `globalIllum` binds the NON-inverse rotation-only
// view-proj (`view_proj`) as `camRotOld` (`WorldRenderBase.cs:291-293`). Bound
// for layout/struct fidelity; `calcGlobalIlum` does not actually index it (the
// reprojection-via-history is `renderSampleRefine`'s job — `09-design-b.md`
// §8.1 lists it bound, the HLSL declares the arrays but the function body uses
// only the *current*-frame camera).
@group(1) @binding(8) var<storage, read> camera_history: array<GpuCameraHistorySlot>;

// --- @group(3) — the precomputed atmosphere ---------------------------------
// `applyAtmosphere` on a secondary-ray miss (`renderGlobalIllum.fx:132`). As in
// the first-hit pass, the entry shader fetches the octahedral slot itself (WGSL
// forbids passing a `ptr<storage,...>` into a function).
@group(3) @binding(0) var<uniform> atmosphere_params: AtmosphereParams;
@group(3) @binding(1) var<storage, read> atmosphere_comp: array<vec4<u32>>;

// One slot of the 128-deep camera-history ring (mirrors
// `gpu_types::GpuCameraHistorySlot` / the `taa.wgsl` decl — 160 bytes).
struct GpuCameraHistorySlot {
    view_proj: mat4x4<f32>,
    view_proj_inv: mat4x4<f32>,
    cam_pos_from_cur_int: vec3<f32>,
    jitter: vec2<f32>,
}

// One slot of the GI accumulation ring — `Uint2` per slot, declared with
// `atomic<u32>` components so the per-thread `InterlockedAdd` is legal
// (`09-design-b.md` §5.5). `.valid` = the C# `.x`, `.invalid` = the C# `.y`.
struct SampleCountSlot {
    valid: atomic<u32>,
    invalid: atomic<u32>,
}

// --- group-shared sample counters (renderGlobalIllum.fx:30-32) --------------
// HLSL `groupshared uint sharedResCount = 0, globalResCountValid = 0,
// globalResCountInvalid = 0`. WGSL `var<workgroup>` is not auto-zeroed — lane 0
// zeroes them before the first barrier.
var<workgroup> shared_res_count: atomic<u32>;
var<workgroup> global_res_count_valid: u32;
var<workgroup> global_res_count_invalid: u32;

// `compressSampleValid` (`renderGlobalIllum.fx:34-48`) — pack a lit GI sample
// into the 32-byte `SampleValid` (`data_a` / `data_b` = the HLSL `data1` /
// `data2`). `octEncode(sampleDir) * pow(2, 22)` — explicit `u32()` cast on the
// `f32` octahedral coordinates (the HLSL `uint2(...)` truncates implicitly).
fn compress_sample_valid(
    pixel_pos: vec2<u32>,
    first_hit: vec4<u32>,
    sample_dir: vec3<f32>,
    comp_color: u32,
    sample_specular_normals: vec3<u32>,
    entity_sample: u32,
    roughness: u32,
    is_first: u32,
) -> SampleValid {
    let oct = oct_encode(sample_dir);
    let sample_dir_oct = vec2<u32>(
        u32(oct.x * 4194304.0),  // pow(2, 22)
        u32(oct.y * 4194304.0),
    );

    var sv: SampleValid;
    sv.data_a.x = first_hit.x;
    sv.data_a.y = pixel_pos.x | (first_hit.y & 0xFFFF8000u);
    sv.data_a.z = pixel_pos.y | (first_hit.z & 0xFFFF8000u);
    sv.data_a.w = gi_params.taa_index | (roughness << 7u) | (first_hit.w & 0xFFFF8000u);
    sv.data_b.x = entity_sample | (is_first << 14u) | (sample_specular_normals.x << 15u);
    sv.data_b.y = comp_color | (sample_specular_normals.y << 15u);
    sv.data_b.z = (sample_dir_oct.y >> 10u) | (sample_specular_normals.z << 15u);
    sv.data_b.w = (sample_dir_oct.y & 0x3FFu) | (sample_dir_oct.x << 10u);
    return sv;
}

// `compressSampleInvalid` (`renderGlobalIllum.fx:50-58`) — pack an unlit GI
// sample into a `vec4<u32>`.
fn compress_sample_invalid(
    pixel_pos: vec2<u32>,
    first_hit: vec4<u32>,
    roughness: u32,
) -> vec4<u32> {
    var si: vec4<u32>;
    si.x = first_hit.x;
    si.y = pixel_pos.x | (first_hit.y & 0xFFFF8000u);
    si.z = pixel_pos.y | (first_hit.z & 0xFFFF8000u);
    si.w = gi_params.taa_index | (roughness << 7u) | (first_hit.w & 0xFFFF8000u);
    return si;
}

// `calcGlobalIlum` (`renderGlobalIllum.fx:60-291`) — `[numthreads(64,1,1)]`.
@compute @workgroup_size(64, 1, 1)
fn calc_global_ilum(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // HLSL `groupshared` is zero-initialised at module scope; WGSL is not.
    if (local_index == 0u) {
        atomicStore(&shared_res_count, 0u);
        global_res_count_valid = 0u;
        global_res_count_invalid = 0u;
    }
    workgroupBarrier();

    let cam_pos_int = gi_params.cam_pos_int.xyz;
    let cam_pos_frac = gi_params.cam_pos_frac.xyz;
    let screen_width = gi_params.screen_width;
    let screen_height = gi_params.screen_height;

    // `pixelsToRender[globalID.x]` — the queued, packed pixel position. The
    // indirect dispatch launches exactly `ceil(queued_count / 64)` workgroups,
    // so the tail group has lanes past `queued_count` — those still must reach
    // every `workgroupBarrier()`, but contribute neither a sample nor a count.
    let pixel_pos_comp = ray_queue[global_id.x];
    let pixel_pos = vec2<u32>(pixel_pos_comp & 0xFFFFu, (pixel_pos_comp >> 16u) & 0xFFFFu);

    // A lane is "active" if its queued slot is real: the ray queue holds at most
    // `pixel_count` entries, and `rayQueueCalc` only ever writes a slot for a
    // hit pixel — so a slot index `< pixel_count` AND a non-zero pixelTypeRaw
    // marks a real entry. `ray_queue` is zero-cleared on creation, so a tail
    // lane reads `pixel_pos_comp == 0` ⇒ `pixel_pos == (0,0)`; guard with the
    // explicit dispatch-tail check below.
    var rand = init_rand(vec3<u32>(global_id.x, gi_params.rand_counter, gi_params.rand_counter_b));
    // `getRayDir(invCamMatrix, pixelPos, screenWidth, screenHeight, taaJitter)`
    // — the JITTERED primary ray (`renderGlobalIllum.fx:69`). The GI ray must
    // be fired through the per-frame Halton sub-pixel offset, not the pixel
    // centre — without it the long-term TAA has no sub-pixel variation to
    // average and can never resolve sub-pixel detail (`18-taa-fidelity.md`
    // cause #1). `gi_params.taa_jitter` is the same value the first-hit pass
    // jitters with — the G-buffer was encoded for a jittered ray.
    let ray_dir = get_ray_dir(
        gi_params.inv_view_proj, pixel_pos, screen_width, screen_height,
        gi_params.taa_jitter,
    );

    let first_hit = first_hit_data[pixel_pos.x + pixel_pos.y * screen_width];
    let first_hit_result: FirstHitResult =
        get_hit_data_from_planes(first_hit, cam_pos_int, cam_pos_frac, ray_dir);

    let first_hit_type_index = first_hit.z & 0x7FFFu;
    let first_hit_type: VoxelType = decompress_voxel_type(voxel_types[first_hit_type_index]);
    let ior = first_hit_type.color_base;

    // A primary-ray miss (voxelTypeRaw == 0) is never queued by `rayQueueCalc`,
    // but the indirect dispatch tail lanes read a zero-cleared `ray_queue` slot
    // ⇒ `pixel_pos == (0,0)` ⇒ `first_hit_type_index` may be 0. Treat a zero
    // type index as an inactive lane: it still hits every barrier, but writes
    // no sample and adds no count (`shared_res_count` add is gated below).
    let lane_active = first_hit_type_index != 0u;

    // `curPosFrac = firstHitResult.pos + firstHitResult.normal * 0.01`
    // (`renderGlobalIllum.fx:80-82`) — re-split into int + frac (D1).
    var cur_pos_frac = first_hit_result.pos + first_hit_result.normal * 0.01;
    var cur_pos_int = cam_pos_int + vec3<i32>(floor(cur_pos_frac));
    cur_pos_frac = cur_pos_frac - floor(cur_pos_frac);

    var cur_dir = ray_dir;
    var material_state = first_hit_type.material_base;
    var radiance = vec3<f32>(0.0, 0.0, 0.0);
    var cur_absorption = vec3<f32>(1.0, 1.0, 1.0);
    var extra_absorption = vec3<f32>(1.0, 1.0, 1.0);
    // `normTangs = uint3(HIT_NOTHING, HIT_UNDEFINED, HIT_UNDEFINED)`.
    var norm_tangs = vec3<u32>(HIT_NOTHING, HIT_UNDEFINED, HIT_UNDEFINED);
    var is_first_diffuse_hit = false;
    var sample_normal_comp: u32 = 0u;
    var hit_emitter_directly = false;
    let entity_sample = ENTITY_FREE;

    // --- primary-surface BRDF interaction (renderGlobalIllum.fx:97-116) -----
    if (material_state == SURFACE_SPECULAR_MIRROR) {
        cur_dir = reflect(cur_dir, first_hit_result.normal);
    } else if (material_state == SURFACE_SPECULAR_ROUGH) {
        // `do { ... } while (dot(curDir, n) <= 0 && count++ < 2)`.
        var rough_normal = vec3<f32>(0.0, 0.0, 0.0);
        var count: i32 = 0;
        loop {
            rough_normal = sample_vndf_isotropic(
                next_rand2(&rand), -cur_dir, first_hit_type.roughness, first_hit_result.normal,
            );
            cur_dir = reflect(cur_dir, rough_normal);
            if (!(dot(cur_dir, first_hit_result.normal) <= 0.0 && count < 2)) {
                break;
            }
            count = count + 1;
        }
        let gi = geometry_term(
            first_hit_type.roughness,
            clamp(dot(cur_dir, first_hit_result.normal), 0.0, 1.0),
        );
        let f = get_reflectance_fresnel(ior, dot(cur_dir, rough_normal));
        extra_absorption = gi * f;
    } else {
        cur_dir = get_uniform_hemisphere_sample(next_rand2(&rand), first_hit_result.normal, 0.0);
    }
    let sample_dir = cur_dir;
    var sample_dist: f32 = 0.0;

    // --- the ≤3-bounce GI loop (renderGlobalIllum.fx:121-235) ---------------
    let bounce_max = min(gi_params.max_bounce_count, 3u);
    var bounce: u32 = 0u;
    loop {
        if (bounce >= bounce_max) {
            break;
        }

        var ray_result: RayResult;
        let is_hit = shoot_ray(
            cur_pos_int, cur_pos_frac, cur_dir,
            i32(max(gi_params.max_ray_steps_secondary, 1u)),
            &ray_result,
        );
        // `if (bounce < 3 && !isFirstDiffuseHit) normTangs[bounce] = ...`.
        if (bounce < 3u && !is_first_diffuse_hit) {
            norm_tangs[bounce] = ray_result.normal_comp;
        }

        if (!is_hit) {
            // Russian roulette: with probability 1/16, fold the atmosphere in
            // weighted ×16 (`renderGlobalIllum.fx:131-132`).
            if (next_rand(&rand) <= 1.0 / 16.0) {
                let oct_index = atmosphere_oct_index(
                    cur_dir,
                    atmosphere_params.atmosphere_tex_size_x,
                    atmosphere_params.atmosphere_tex_size_y,
                );
                var acc: AtmoLight;
                acc.absorption = cur_absorption;
                acc.light = radiance;
                acc = apply_atmosphere(atmosphere_comp[oct_index], acc, 16.0);
                cur_absorption = acc.absorption;
                radiance = acc.light;
            }
            if (!is_first_diffuse_hit) {
                sample_dist = 1024.0;
            }
            break;
        }

        if (!is_first_diffuse_hit) {
            sample_dist += ray_result.length;
        }

        // `newPosFrac = curPosFrac + curDir * length + normal * 0.01` (re-split).
        var new_pos_frac = cur_pos_frac + cur_dir * ray_result.length
            + ray_result.normal * 0.01;
        let new_pos_int = cur_pos_int + vec3<i32>(floor(new_pos_frac));
        new_pos_frac = new_pos_frac - floor(new_pos_frac);
        var new_dir = cur_dir;

        let voxel_type: VoxelType = decompress_voxel_type(voxel_types[ray_result.hit_type]);
        material_state = voxel_type.material_base;

        // Apply albedo (diffuse / emissive only — `materialState <= 1`).
        if (material_state <= SURFACE_EMISSIVE) {
            cur_absorption = cur_absorption * voxel_type.color_base;
        }

        // --- sun sample (renderGlobalIllum.fx:156-187) ----------------------
        if (material_state <= SURFACE_SPECULAR_ROUGH) {
            if (!is_first_diffuse_hit) {
                sample_normal_comp = ray_result.normal_comp & 0x7u;
                // (`#ifdef ENTITIES entitySample = rayResult.entity` — omitted.)
                is_first_diffuse_hit = true;
            }

            let sun_dir_rand = get_uniform_hemisphere_sample(
                vec2<f32>(next_rand(&rand), next_rand(&rand)), gi_params.sky_sun_dir.xyz, 0.9999,
            );
            // HLSL `float3 fac = saturate(...) * 2` — the scalar is broadcast to
            // a `float3` (the rough-specular branch multiplies it by the `vec3`
            // Fresnel `F`, and `radiance += ... * fac` is all `vec3`).
            var fac = vec3<f32>(clamp(dot(ray_result.normal, sun_dir_rand), 0.0, 1.0) * 2.0);

            if (material_state == SURFACE_SPECULAR_ROUGH) {
                let gi = geometry_term(voxel_type.roughness, dot(sun_dir_rand, ray_result.normal));
                let go = geometry_term(voxel_type.roughness, dot(-cur_dir, ray_result.normal));
                let half_dir = normalize(sun_dir_rand + -cur_dir);
                let nh = dot(ray_result.normal, half_dir);
                let r2 = voxel_type.roughness * voxel_type.roughness;
                let denom = nh * nh * (r2 - 1.0) + 1.0;
                let d = r2 / (PI * denom * denom);
                let f = get_reflectance_fresnel(voxel_type.color_base, dot(sun_dir_rand, ray_result.normal));
                let norm_max_d = voxel_type.roughness * 500.0 + 1.0;
                let norm_d = norm_max_d - norm_max_d / ((1.0 / norm_max_d) * d + 1.0);
                fac = fac * (0.5 * norm_d * gi * go * f)
                    / (4.0 * 1.0 * dot(-cur_dir, ray_result.normal));
            }

            // The single sun-shadow ray (was `MAX_RAY_STEPS_SUN_SECONDARY`
            // const; now `gi_params.max_ray_steps_sun_secondary` runtime knob —
            // `21-design-quality-panel.md`). The defensive `max(_, 1u)` clamp
            // mirrors Dispatch A's `sun_shadow_taps` clamp.
            if (dot(sun_dir_rand, ray_result.normal) > 0.0) {
                var temp: RayResult;
                let sun_blocked = shoot_ray(
                    new_pos_int, new_pos_frac, sun_dir_rand,
                    i32(max(gi_params.max_ray_steps_sun_secondary, 1u)),
                    &temp,
                );
                if (!sun_blocked) {
                    radiance += cur_absorption * gi_params.sun_color.xyz * fac * 1.0;
                }
            }
        }

        // Emissive surfaces add `colorLayer.r` * absorption.
        if (material_state == SURFACE_EMISSIVE) {
            hit_emitter_directly = bounce == 0u;
            radiance += cur_absorption * voxel_type.color_layer.r;
        }

        // --- surface-effect bounce (renderGlobalIllum.fx:197-225) -----------
        var rough_break = false;
        if (material_state == SURFACE_SPECULAR_MIRROR) {
            new_dir = reflect(cur_dir, ray_result.normal);
            cur_absorption *= get_reflectance_fresnel(
                voxel_type.color_base, dot(new_dir, ray_result.normal),
            );
        } else if (material_state == SURFACE_SPECULAR_ROUGH) {
            var rough_normal = vec3<f32>(0.0, 0.0, 0.0);
            var count: i32 = 0;
            new_dir = cur_dir;
            loop {
                rough_normal = sample_vndf_isotropic(
                    next_rand2(&rand), -new_dir, voxel_type.roughness, ray_result.normal,
                );
                new_dir = reflect(new_dir, rough_normal);
                if (!(dot(new_dir, ray_result.normal) <= 0.0 && count < 2)) {
                    break;
                }
                count = count + 1;
            }
            if (dot(new_dir, ray_result.normal) <= 0.0) {
                rough_break = true;
            } else {
                let gi = geometry_term(
                    voxel_type.roughness,
                    clamp(dot(new_dir, ray_result.normal), 0.0, 1.0),
                );
                let f = get_reflectance_fresnel(voxel_type.color_base, dot(new_dir, rough_normal));
                cur_absorption *= gi * f;
            }
        } else {
            new_dir = get_uniform_hemisphere_sample(next_rand2(&rand), ray_result.normal, 0.0);
            cur_absorption *= clamp(dot(ray_result.normal, new_dir), 0.0, 1.0) * 2.0;
        }
        if (rough_break) {
            break;
        }

        // `if (bounce == 2 && !isFirstDiffuseHit) normTangs[2] = 0x1FFFF`.
        if (bounce == 2u && !is_first_diffuse_hit) {
            norm_tangs[2] = 0x1FFFFu;
        }

        cur_pos_int = new_pos_int;
        cur_pos_frac = new_pos_frac;
        cur_dir = new_dir;
        bounce = bounce + 1u;
    }

    // --- compress + classify (renderGlobalIllum.fx:237-289) -----------------
    // `radianceCompWithAbsorption` is computed in the HLSL (`:237`) but never
    // read — keep the call only for RNG-state fidelity (`compress_color`
    // advances `rand`). A named throwaway (not `let _ =`: naga-oil's import
    // writeback rejects a `_` binding on a namespaced-rewritten call); the
    // `_unused` prefix keeps naga's own dead-code lint quiet.
    let _unused_radiance_comp_with_absorption =
        compress_color(radiance * extra_absorption, &rand);

    let radiance_single = dot(radiance, vec3<f32>(1.0, 1.0, 1.0));
    let radiance_reduction_val = 2.0;
    if (radiance_single < radiance_reduction_val) {
        let test = max(radiance_single, 0.05);
        radiance = radiance * (radiance_reduction_val / test);
        if (next_rand(&rand) > test / radiance_reduction_val) {
            radiance = vec3<f32>(0.0, 0.0, 0.0);
        }
    }
    let radiance_comp = compress_color(radiance, &rand);

    let is_valid = radiance_comp > 0u;
    // `isSkip = !isValid && nextRand > 1/8` — the "every 8th unlit sample stored".
    let is_skip = !is_valid && (next_rand(&rand) > 1.0 / 8.0);

    workgroupBarrier();

    // Per-thread reservation in the group-shared counter. A lit sample adds 1
    // to the low 16 bits; an unlit (kept) sample adds 1 to the high 16 bits.
    // Inactive tail lanes (`!lane_active`) and skipped unlit samples reserve
    // nothing. The HLSL gates on `!isSkip`; the port also gates on `lane_active`
    // so an indirect-dispatch tail lane never inflates the count.
    var prev_sample_count: u32 = 0u;
    if (lane_active && !is_skip) {
        let add = select(1u << 16u, 1u, is_valid);
        prev_sample_count = atomicAdd(&shared_res_count, add);
    }

    workgroupBarrier();

    if (local_index == 0u) {
        let total = atomicLoad(&shared_res_count);
        global_res_count_valid = atomicAdd(
            &sample_counts[3u + gi_params.accum_index].valid, total & 0xFFFFu,
        );
        global_res_count_invalid = atomicAdd(
            &sample_counts[3u + gi_params.accum_index].invalid, total >> 16u,
        );
    }

    workgroupBarrier();

    // `samplesStartIndex = globalIlumSampleCounts[0]` — the ring write cursors.
    let samples_start_index = vec2<u32>(
        atomicLoad(&sample_counts[0].valid),
        atomicLoad(&sample_counts[0].invalid),
    );
    let shared_total = atomicLoad(&shared_res_count);

    // `extraData` — 8-bit roughness for a rough-specular first hit, else 0.
    var extra_data: u32 = 0u;
    if (first_hit_type.material_base == SURFACE_SPECULAR_ROUGH) {
        extra_data = 1u + u32(first_hit_type.roughness * 254.5);
    }

    if (lane_active && is_valid) {
        let max_sample_count =
            gi_params.valid_sample_storage_count * gi_params.screen_width * gi_params.screen_height;
        let index = prev_sample_count & 0xFFFFu;
        // `compressSampleValid(..., normTangs, ...)` (`renderGlobalIllum.fx:280`)
        // — the HLSL passes the `normTangs` `uint3` of secondary-bounce plane
        // codes DIRECTLY as the `sampleSpecularNormals` parameter (NOT
        // `getSpecularNormals(...)` — that helper is `renderSampleRefine`'s, on
        // the *first-hit* planes). `compress_sample_valid` packs `.x`/`.y`/`.z`
        // each `<< 15` into `data_b`.
        let sv = compress_sample_valid(
            pixel_pos, first_hit, sample_dir, radiance_comp, norm_tangs,
            entity_sample, extra_data, select(0u, 1u, hit_emitter_directly),
        );
        // The wrapping ring write (`renderGlobalIllum.fx:281`).
        let write_index = (samples_start_index.x + max_sample_count + index
            - (global_res_count_valid + (shared_total & 0xFFFFu))) % max_sample_count;
        valid_samples[write_index] = sv;
    } else if (lane_active && !is_skip) {
        let max_sample_count =
            gi_params.invalid_sample_storage_count * gi_params.screen_width * gi_params.screen_height;
        let index = prev_sample_count >> 16u;
        let si = compress_sample_invalid(pixel_pos, first_hit, extra_data);
        let write_index = (samples_start_index.y + max_sample_count + index
            - (global_res_count_invalid + (shared_total >> 16u))) % max_sample_count;
        invalid_samples[write_index] = si;
    }

    // Keep `first_hit_absorption` / `final_color` / `camera_history` referenced
    // so naga retains the bindings in the layout — `calcGlobalIlum` does not
    // write `firstHitAbsorption` / `finalColor` (the HLSL declares them RW but
    // the function body never writes them — `09-design-b.md` §8.1) and uses
    // only the *current*-frame camera (not the history ring). Zero-additive
    // no-ops on an unreachable index; `_ = expr` is WGSL's phony-assignment
    // form (`let _ = ...` is not valid WGSL).
    if (global_id.x == 0xFFFFFFFFu) {
        first_hit_absorption[0] = vec2<u32>(0u, 0u);
        final_color[0] = vec2<u32>(0u, 0u);
        _ = camera_history[0].view_proj[0][0];
    }
}
