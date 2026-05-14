// naadf_first_hit.wgsl — the Phase-A first-hit compute pass.
//
// Derives from: render/versions/albedo/renderFirstHit.fx `calcFirstHit`
// (`03-design.md` §5.5). A faithful port of the no-TAA path: per-pixel ray
// setup, `rayAABB` volume test, `shootRay`, a simple sun + ambient term, and
// the G-buffer / shaded-colour writes.
//
// Phase-A divergences from the HLSL (all per `03-design.md` §5.3 + D4):
//   * `isTAA` is always 0 — the `taaSamples` ring write is omitted entirely
//     (that buffer does not exist in Phase A).
//   * The HLSL only writes `firstHitData` inside `if (isTAA)`; Phase A writes
//     it unconditionally so the G-buffer plane 0 is always populated.
//   * The HLSL's `taaSampleAccum` write is Phase A's `shaded_color` write —
//     identical `vec2<u32>` element format (`03-design.md` §5.3), so the final
//     blit stays a near-verbatim port of `renderFinal.fx`.
//
// `[numthreads(64,1,1)]` in the HLSL → `@workgroup_size(64,1,1)`.

#import "shaders/render_pipeline_common.wgsl"::{
    GpuCamera, GpuRenderParams, VoxelType, decompress_voxel_type, get_ray_dir,
    compress_first_hit_data, HIT_NOTHING, HIT_UNDEFINED, ENTITY_FREE, SURFACE_EMISSIVE,
    FLAG_SHOW_RAY_STEP, FLAG_CHECK_SUN,
}
#import "shaders/ray_tracing.wgsl"::{
    RayResult, ray_aabb, shoot_ray, MAX_RAY_STEPS_PRIMARY, MAX_RAY_STEPS_SUN,
}
#import "shaders/world_data.wgsl"::{voxel_types, world_meta}

// --- @group(1) — frame data -------------------------------------------------

@group(1) @binding(0) var<uniform> camera: GpuCamera;
@group(1) @binding(1) var<uniform> params: GpuRenderParams;
// The Phase-A G-buffer — one `vec4<u32>` per pixel (`03-design.md` §5.3).
@group(1) @binding(2) var<storage, read_write> first_hit_data: array<vec4<u32>>;
// The blit-source stand-in — one `vec2<u32>` per pixel, `taaSampleAccum` format.
@group(1) @binding(3) var<storage, read_write> shaded_color: array<vec2<u32>>;

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
    let bbox_min = vec3<f32>(world_meta.bounding_box_min);
    let bbox_max = vec3<f32>(world_meta.bounding_box_max);
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
    // The `taaSamples` ring write is omitted (Phase A is TAA-off — D4).
    first_hit_data[pixel_index] =
        compress_first_hit_data(distance_ray, norm_tangs, voxel_type_raw, entity);

    // --- shaded-colour write (HLSL `taaSampleAccum` write) -----------------
    // `newColorComp.x = f16(1.0) | (f16(light.r) << 16)`
    // `newColorComp.y = f16(light.g) | (f16(light.b) << 16)`
    // i.e. a `1.0` weight in the low half of `.x`, RGB as three f16s.
    var new_color = vec2<u32>(0u, 0u);
    new_color.x = pack2x16float(vec2<f32>(1.0, light.r));
    new_color.y = pack2x16float(vec2<f32>(light.g, light.b));
    // The ray-step debug view stuffs the raw step count into `.x`.
    if ((params.flags & FLAG_SHOW_RAY_STEP) != 0u) {
        new_color.x = u32(ray_result.step_count);
    }
    shaded_color[pixel_index] = new_color;
}
