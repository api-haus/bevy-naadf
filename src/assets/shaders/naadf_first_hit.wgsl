// naadf_first_hit.wgsl — the Phase-A first-hit compute pass.
//
// Derives from: render/versions/albedo/renderFirstHit.fx `calcFirstHit`
// (`03-design.md` §5.5). A faithful port of the no-TAA path: per-pixel ray
// setup, `rayAABB` volume test, `shootRay`, a simple sun + ambient term, and
// the G-buffer / shaded-colour writes.
//
// Divergences from the HLSL (per `03-design.md` §5.3 + `06-design-a2.md`):
//   * The `taaSamples` ring write (HLSL `if (isTAA)`) is ported in Phase A-2
//     Batch 2 step 6, gated on `FLAG_IS_TAA` (`06-design-a2.md` §6.1). The HLSL
//     calls `getSpecularNormals(firstHit)` — in A-2 that is always 0 (plane-0
//     only, entity-free), so the port hardcodes `specular_normals = 0u` rather
//     than porting that Phase-B helper.
//   * The HLSL only writes `firstHitData` inside `if (isTAA)`; the port writes
//     it unconditionally so the G-buffer plane 0 is always populated.
//   * The HLSL's `taaSampleAccum` write is this pass's `taa_sample_accum`
//     write — Phase A-2 renamed Phase A's `shaded_color` stand-in (the
//     stand-in was deliberately built to the `taaSampleAccum` `vec2<u32>`
//     element format, so the rename is logic-free — `06-design-a2.md` §5.1).
//
// `[numthreads(64,1,1)]` in the HLSL → `@workgroup_size(64,1,1)`.

#import "shaders/render_pipeline_common.wgsl"::{
    GpuCamera, GpuRenderParams, VoxelType, decompress_voxel_type, get_ray_dir,
    compress_first_hit_data, HIT_NOTHING, HIT_UNDEFINED, ENTITY_FREE, SURFACE_EMISSIVE,
    FLAG_SHOW_RAY_STEP, FLAG_CHECK_SUN, FLAG_IS_TAA,
}
#import "shaders/ray_tracing.wgsl"::{
    RayResult, ray_aabb, shoot_ray, MAX_RAY_STEPS_PRIMARY, MAX_RAY_STEPS_SUN,
}
#import "shaders/world_data.wgsl"::{voxel_types, world_meta}
#import "shaders/taa_common.wgsl"::{taa_compress_sample, TAA_SAMPLE_RING_DEPTH}

// --- @group(1) — frame data -------------------------------------------------

@group(1) @binding(0) var<uniform> camera: GpuCamera;
@group(1) @binding(1) var<uniform> params: GpuRenderParams;
// The Phase-A G-buffer — one `vec4<u32>` per pixel (`03-design.md` §5.3).
@group(1) @binding(2) var<storage, read_write> first_hit_data: array<vec4<u32>>;
// The real `taaSampleAccum` — one `vec2<u32>` per pixel (`06-design-a2.md`
// §2.2, §5.1). Phase A-2 renamed Phase A's `shaded_color` stand-in to
// `taa_sample_accum` and re-homed the buffer into `TaaGpu`; the element format
// and the write site below are unchanged (the stand-in was deliberately built
// to the `taaSampleAccum` format), so this is a pure rename.
@group(1) @binding(3) var<storage, read_write> taa_sample_accum: array<vec2<u32>>;

// --- @group(2) — the TAA sample ring ----------------------------------------
// The 16-deep `taaSamples` ring, slot-major (`06-design-a2.md` §2.1, §5.2).
// Written here (one slot per pixel) when `FLAG_IS_TAA` is set; read by the TAA
// reproject pass (`taa.wgsl`). Always bound — the `if` below guards the write,
// so when TAA is off the buffer is simply never touched.
@group(2) @binding(0) var<storage, read_write> taa_samples: array<vec2<u32>>;

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
    // `getRayDir(invCamMatrix, pixelPos, w, h, taaJitter)`.
    let ray_dir = get_ray_dir(
        camera.inv_view_proj,
        pixel_pos,
        params.screen_width,
        params.screen_height,
        params.taa_jitter,
    );

    // `rayAABB(camPosInt + camPosFrac, rayDir, boundingBoxMin, boundingBoxMax, ...)`
    // — clip the ray to the world volume. The bounding box comes from
    // `world_meta` (`@group(0)`), not `params` — `03-design.md` prepare note.
    // `bounding_box_min/max` are NAADF's `float3 boundingBoxMin/Max` — the
    // 0.1-voxel-inset world extent (`WorldData.cs:477-478`).
    let bbox_min = world_meta.bounding_box_min;
    let bbox_max = world_meta.bounding_box_max;
    let cam_pos_world = vec3<f32>(cam_pos_int) + cam_pos_frac;
    let volume = ray_aabb(cam_pos_world, ray_dir, bbox_min, bbox_max);

    var light = vec3<f32>(0.0, 0.0, 0.0);
    var absorption = vec3<f32>(1.0, 1.0, 1.0);
    var norm_tangs = vec4<u32>(HIT_NOTHING, HIT_UNDEFINED, HIT_UNDEFINED, HIT_UNDEFINED);
    var voxel_type_raw: u32 = 0u;
    var first_hit_normal = vec3<f32>(0.0, 0.0, 0.0);
    var distance_ray: f32 = -1.0;
    var ray_result: RayResult;
    ray_result.step_count = 0;
    var cur_pos_int = cam_pos_int;
    var cur_pos_frac = cam_pos_frac;
    let entity = ENTITY_FREE;

    if (volume.hit) {
        // Advance the ray origin to the volume entry point, re-splitting into
        // int + frac (`03-design.md` §5.2 — all ray math stays in int+frac).
        cur_pos_frac = cam_pos_frac + ray_dir * volume.dist_min_max.x;
        cur_pos_int = cur_pos_int + vec3<i32>(floor(cur_pos_frac));
        cur_pos_frac = cur_pos_frac - floor(cur_pos_frac);

        let is_hit = shoot_ray(
            cur_pos_int, cur_pos_frac, ray_dir, MAX_RAY_STEPS_PRIMARY, &ray_result,
        );
        norm_tangs.x = ray_result.normal_comp;

        if (is_hit) {
            // Step the ray origin onto the hit surface (+ a small normal nudge).
            cur_pos_frac = cur_pos_frac + ray_dir * ray_result.length
                + ray_result.normal * 0.01;
            cur_pos_int = cur_pos_int + vec3<i32>(floor(cur_pos_frac));
            cur_pos_frac = cur_pos_frac - floor(cur_pos_frac);

            let voxel_type: VoxelType =
                decompress_voxel_type(voxel_types[ray_result.hit_type]);
            distance_ray = ray_result.length + volume.dist_min_max.x;
            first_hit_normal = ray_result.normal;
            voxel_type_raw = ray_result.hit_type;

            absorption = absorption * voxel_type.color_base;
            // Emissive surfaces add `colorLayer.r` * albedo (HLSL).
            if (voxel_type.material_base == SURFACE_EMISSIVE) {
                light = light + absorption * voxel_type.color_layer.r;
            }
        }
    }

    // Sample the sun (HLSL `if (distanceRay > 0)`).
    if (distance_ray > 0.0) {
        if ((params.flags & FLAG_CHECK_SUN) != 0u) {
            var sun_block: RayResult;
            let is_sun_blocked = shoot_ray(
                cur_pos_int,
                cur_pos_frac + first_hit_normal * 0.01,
                params.sky_sun_dir,
                MAX_RAY_STEPS_SUN,
                &sun_block,
            );
            let sun_cos_theta = clamp(dot(params.sky_sun_dir, first_hit_normal), 0.0, 1.0);
            if (!is_sun_blocked && sun_cos_theta > 0.001) {
                let weight = 2.0 * sun_cos_theta;
                light = light + params.sun_color * weight * absorption;
            }
        }
        // A cheap ambient term (HLSL: ambient from a sun-biased direction).
        let dir_for_ambient = normalize(first_hit_normal + params.sky_sun_dir * 1.01);
        light = light
            + absorption * params.sun_color * 0.2 * dot(params.sky_sun_dir, dir_for_ambient);
    }

    // --- G-buffer write ----------------------------------------------------
    // The HLSL only writes `firstHitData` inside `if (isTAA)`; Phase A writes
    // it unconditionally so plane 0 is always populated (`03-design.md` §5.3).
    first_hit_data[pixel_index] =
        compress_first_hit_data(distance_ray, norm_tangs, voxel_type_raw, entity);

    // --- taa_samples ring write (HLSL `if (isTAA)` block) ------------------
    // `renderFirstHit.fx:109-117` — when TAA is on, compress the shaded sample
    // into the 64-bit format and write it into the ring slot `taaIndex % 16`
    // (the §6 16-deep ring, NOT NAADF's 32). `getSpecularNormals(firstHit)` is
    // always 0 in A-2 (plane-0-only, entity-free — `06-design-a2.md` §3.1,
    // §6.1), so `specular_normals` is hardcoded rather than porting that
    // Phase-B helper. `light` is the same shaded colour written to
    // `taa_sample_accum` below; `taa_compress_sample` does the exponential
    // colour compression internally.
    if ((params.flags & FLAG_IS_TAA) != 0u) {
        let specular_normals = 0u;
        // dist for the sample: hit → distance_ray; miss (voxel_type_raw == 0)
        // → 65520 (≈ f16 max), matching `renderFirstHit.fx:115`.
        let sample_dist = select(distance_ray, 65520.0, voxel_type_raw == 0u);
        let sample = taa_compress_sample(
            sample_dist,
            light,
            norm_tangs.x & 0x7u, // plane-0 normal-tang code, low 3 bits
            1u,                  // isDiffuse — A-2 is all-diffuse
            specular_normals,
            0u,                  // extraData — 0 in the albedo path
            entity,
        );
        let slot = params.taa_index % TAA_SAMPLE_RING_DEPTH;
        taa_samples[slot * (params.screen_width * params.screen_height) + pixel_index] =
            sample;
    }

    // --- taa_sample_accum write (HLSL `taaSampleAccum` write) --------------
    // `newColorComp.x = f16(1.0) | (f16(light.r) << 16)`
    // `newColorComp.y = f16(light.g) | (f16(light.b) << 16)`
    // i.e. a `1.0` weight in the low half of `.x`, RGB as three f16s. The
    // `1.0` weight is the current frame's per-pixel sample count — the
    // load-bearing 0.25-spp signal (`06-design-a2.md` §2.2, §3.2).
    var new_color = vec2<u32>(0u, 0u);
    new_color.x = pack2x16float(vec2<f32>(1.0, light.r));
    new_color.y = pack2x16float(vec2<f32>(light.g, light.b));
    // The ray-step debug view stuffs the raw step count into `.x`.
    if ((params.flags & FLAG_SHOW_RAY_STEP) != 0u) {
        new_color.x = u32(ray_result.step_count);
    }
    taa_sample_accum[pixel_index] = new_color;
}
