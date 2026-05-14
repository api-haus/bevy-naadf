// taa.wgsl — the Phase-A-2 TAA reproject + accumulation compute pass.
//
// Derives from: render/versions/albedo/renderTaaSampleReverse.fx
// `reprojectOldSamples` (`06-design-a2.md` §7). A faithful WGSL port of the
// albedo-path long-term-memory TAA: for each pixel, precompute a 3×3
// neighbourhood (distance min/max + surface hashes), then walk up to
// `sample_age` past frames — reproject this pixel's virtual hit position into
// each past frame's screen, fetch the stored 64-bit sample, distance/screen/
// hash-reject it, and accumulate the accepted history colour into
// `taa_sample_accum` on top of the current frame's sample.
//
// `[numthreads(64,1,1)]` in the HLSL → `@workgroup_size(64,1,1)`.
//
// --- Faithful-port deviations (per `06-design-a2.md` §7) --------------------
//   * Matrix convention: the HLSL `mul(v, M)` against NAADF's row-major
//     matrices is the column-vector `M * v` against a glam (column-major)
//     matrix — the `05-review.md` perspective-fix convention, exactly as
//     `get_ray_dir` was corrected. Every matrix multiply below uses `M * v`
//     with the perspective `w`-divide. Do NOT "fix" this back to `v * M`.
//   * The §6 16-deep sample ring: `(taaIndex + i) % 32` in the HLSL becomes
//     `% TAA_SAMPLE_RING_DEPTH` (= 16). The camera-history `% 128` stays 128.
//   * Entity blocks (`renderTaaSampleReverse.fx:76-84, 96-104`,
//     `entityInstancesHistory`) are OMITTED — A-2 is entity-free, exactly as
//     Phase A omitted the `ENTITIES` traversal branch. `entityPosChange` is
//     `(0,0,0)` without entities, so wherever the HLSL adds it the port simply
//     does not have the term.
//   * `getHitDataFromPlanes` reduces to a single-plane reconstruction
//     (`get_hit_data_from_planes_a2`, §7.3) — planes 1-3 are `HIT_UNDEFINED` in
//     the albedo path, so the HLSL specular-reflection loop runs zero
//     iterations and the function is just its tail.
//   * The rough-specular reweight branch (`renderTaaSampleReverse.fx:138-148`)
//     is left as a structural dead-`if` comment: `extra_data` is provably 0 in
//     the albedo path (`06-design-a2.md` §3.2), so the branch never executes;
//     porting its body would pull in `pdf_vndf_isotropic`, a Phase-B function.
//   * Edge-pixel reads: WGSL storage reads out of bounds are undefined (DX11
//     SRVs return 0); the 3×3 neighbour reads clamp the pixel coord to the
//     screen edge before indexing.
//
// naga-oil import module entry point: `reproject_old_samples`.

#import "shaders/render_pipeline_common.wgsl"::{get_ray_dir, NORMAL, HIT_UNDEFINED, ENTITY_FREE}
#import "shaders/taa_common.wgsl"::{
    taa_decompress_sample, taa_hash_from_data, taa_neighbor_offsets, TAA_SAMPLE_RING_DEPTH,
}

// --- struct decls (mirror `gpu_types::GpuTaaParams` / `GpuCameraHistorySlot`)
//
// No explicit padding members — naga-oil's composable-module round-trip
// rejects them, and WGSL's `vec3`→16-byte / `vec2`→8-byte slotting reproduces
// the padded Rust `#[repr(C)]` layout (`06-design-a2.md` §4.4, the same
// convention as `render_pipeline_common.wgsl`'s `GpuCamera`).

// TAA reproject-pass uniform (mirrors `gpu_types::GpuTaaParams`, 192 bytes):
//   inv_view_proj  (0..64)   — C# invCamMatrix, rotation-only inverse view-proj
//   view_proj      (64..128) — C# camMatrix, rotation-only view-proj (current)
//   cam_pos_int    slot 128  — C# camPosInt
//   cam_pos_frac   slot 144  — C# camPosFrac
//   screen_width   160, screen_height 164, frame_count 168, taa_index 172
//   sample_age     176
struct GpuTaaParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    cam_pos_int: vec3<i32>,
    cam_pos_frac: vec3<f32>,
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    taa_index: u32,
    sample_age: u32,
}

// One slot of the 128-deep camera-history ring (mirrors
// `gpu_types::GpuCameraHistorySlot`, 96 bytes/slot):
//   view_proj            (0..64)  — C# camRotOld[i] (rotation-only view-proj)
//   cam_pos_from_cur_int slot 64  — C# taaOldCamPosFromCurCamInt[i]
//   jitter               slot 80  — C# taaJitterOld[i]
struct GpuCameraHistorySlot {
    view_proj: mat4x4<f32>,
    cam_pos_from_cur_int: vec3<f32>,
    jitter: vec2<f32>,
}

// --- the reproject pass's single bind group (`06-design-a2.md` §5.3) --------
// The reproject pass does not traverse the voxel world (no `shoot_ray`), so it
// binds no `@group(0)` world data — its one bind group is `@group(0)`.
@group(0) @binding(0) var<uniform> params: GpuTaaParams;
@group(0) @binding(1) var<storage, read> camera_history: array<GpuCameraHistorySlot, 128>;
@group(0) @binding(2) var<storage, read> first_hit_data: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read> taa_samples: array<vec2<u32>>;
@group(0) @binding(4) var<storage, read_write> taa_sample_accum: array<vec2<u32>>;

// --- get_hit_data_from_planes_a2 -------------------------------------------
// Single-plane reduction of `getHitDataFromPlanes`
// (`commonRenderPipeline.fxh:205-211`). In A-2 only plane 0 is filled (Phase
// A's first-hit only sets `normTangs[0]`; planes 1-3 are `HIT_UNDEFINED`), so
// the HLSL's 3-iteration specular-reflection loop runs zero iterations and the
// function reduces to its tail. The full specular `getHitDataFromPlanes` (the
// loop, `SPECULAR_MIRROR_FAC`, the `ENTITIES` block) is Phase B.
struct FirstHitResultA2 {
    // Virtual hit position, in CURRENT-camera-int-relative space (built from
    // `cam_pos_frac` only — never adding `cam_pos_int`; the D1 trick).
    pos: vec3<f32>,
    normal: vec3<f32>,
    // (1,1,1) in A-2 — no specular bounces, so the mirror-fac is identity.
    normal_mirror_fac: vec3<f32>,
    dist: f32,
    normal_tang: u32,
    ray_dir: vec3<f32>,
}

fn get_hit_data_from_planes_a2(
    first_hit: vec4<u32>,
    cam_pos_int: vec3<i32>,
    cam_pos_frac: vec3<f32>,
    ray_dir: vec3<f32>,
) -> FirstHitResultA2 {
    var r: FirstHitResultA2;
    // plane-0 normal-tang code (HLSL `firstHitResult.normalTang = firstHit.x >> 15`).
    let normal_tang = first_hit.x >> 15u;
    r.normal_tang = normal_tang;
    r.normal = NORMAL[normal_tang & 0x7u];
    let ray_dir_comp_for_normal = abs(dot(ray_dir, r.normal));
    // HLSL: distToTang = abs(dot(pos, abs(normal))
    //                        - (float)((normalTang >> 3) - dot(camPosInt, abs(normal))))
    // — `pos` here is `camPosFrac` (the function's initial `firstHitResult.pos`).
    let dist_to_tang = abs(
        dot(cam_pos_frac, abs(r.normal))
        - (f32(normal_tang >> 3u) - dot(vec3<f32>(cam_pos_int), abs(r.normal)))
    );
    let dist_fac = dist_to_tang / ray_dir_comp_for_normal;
    // HLSL: firstHitResult.pos += firstHitResult.rayDir * distFac;  (pos was camPosFrac)
    r.pos = cam_pos_frac + ray_dir * dist_fac;
    r.dist = dist_fac;
    r.normal_mirror_fac = vec3<f32>(1.0, 1.0, 1.0);
    r.ray_dir = ray_dir;
    return r;
}

// --- get_screen_pos_projection / get_screen_index_projection ---------------
// Port of `getScreenPosProjection` + `getScreenIndexProjection`
// (`commonRenderPipeline.fxh:133-152`). WGSL has no `out` params and no
// default args, so these return small structs and take `pixel_offset`
// explicitly.
//
// MATRIX CONVENTION: the HLSL `mul(float4(pos,1), transformation)` is the
// column-vector `transformation * vec4(pos, 1.0)` against a glam matrix — the
// `05-review.md` perspective-fix convention. Do NOT swap to `v * M`.

struct ScreenPosProj {
    valid: bool,
    screen_pos: vec2<f32>,
}

fn get_screen_pos_projection(
    screen_width: u32,
    screen_height: u32,
    pos: vec3<f32>,
    transformation: mat4x4<f32>,
) -> ScreenPosProj {
    var r: ScreenPosProj;
    let screen_projection = transformation * vec4<f32>(pos, 1.0);
    let ndc = screen_projection.xyz / screen_projection.w;
    if (ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0
        || ndc.z < 0.0 || ndc.z > 1.0) {
        r.valid = false;
        r.screen_pos = vec2<f32>(0.0, 0.0);
        return r;
    }
    var ndc_y = ndc;
    ndc_y.y = ndc_y.y * -1.0;
    let ndc01 = (ndc_y.xy + vec2<f32>(1.0, 1.0)) * 0.5;
    r.valid = true;
    r.screen_pos = ndc01 * vec2<f32>(f32(screen_width), f32(screen_height));
    return r;
}

struct ScreenIndexProj {
    valid: bool,
    screen_index: u32,
}

fn get_screen_index_projection(
    screen_width: u32,
    screen_height: u32,
    pos: vec3<f32>,
    transformation: mat4x4<f32>,
    pixel_offset: vec2<f32>,
) -> ScreenIndexProj {
    let proj = get_screen_pos_projection(screen_width, screen_height, pos, transformation);
    // HLSL clamps `screenPos + pixelOffset` to `[0, (w-1, h-1)]` even when
    // `valid` is false — the index is still computed (and benignly clamped);
    // the caller gates on `valid`.
    let clamped = clamp(
        proj.screen_pos + pixel_offset,
        vec2<f32>(0.0, 0.0),
        vec2<f32>(f32(screen_width - 1u), f32(screen_height - 1u)),
    );
    let screen_pos_int = vec2<u32>(clamped);
    var r: ScreenIndexProj;
    r.valid = proj.valid;
    r.screen_index = screen_pos_int.x + screen_pos_int.y * screen_width;
    return r;
}

// --- the reproject + accumulation pass -------------------------------------
@compute @workgroup_size(64, 1, 1)
fn reproject_old_samples(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let pixel_index = global_id.x;
    if (pixel_index >= params.screen_width * params.screen_height) {
        return;
    }

    let cam_pos_int = params.cam_pos_int;
    let cam_pos_frac = params.cam_pos_frac;
    let screen_width = params.screen_width;
    let screen_height = params.screen_height;

    // HLSL: pixelPos = uint2(globalID.x % w, globalID.x / w).
    let pixel_pos = vec2<u32>(pixel_index % screen_width, pixel_index / screen_width);
    // `getRayDir(invCamMatrix, pixelPos, w, h)` — NO jitter for this pass
    // (`renderTaaSampleReverse.fx:30` calls `getRayDir` with the default
    // `(0,0)` offset). Reuse the perspective-fixed `get_ray_dir`.
    let ray_dir = get_ray_dir(
        params.inv_view_proj, pixel_pos, screen_width, screen_height, vec2<f32>(0.0, 0.0),
    );

    // --- Phase 1: the 3×3 neighbourhood precompute ------------------------
    // (`renderTaaSampleReverse.fx:32-75`). `valid_hashes_comp` packs the 8
    // neighbour hashes 2-per-u32; `valid_hash_center` is the centre hash.
    var valid_hashes_comp = array<u32, 4>(0u, 0u, 0u, 0u);
    var valid_hash_center: u32 = 0u;
    var dist_min_max = vec2<f32>(999999.9, 0.0);

    var first_hit_dist: f32 = 99999999.0;
    // `first_hit_entity` / `first_hit_pos` / `first_hit_mirror_fac` are tracked
    // exactly as the HLSL does; in A-2 `first_hit_mirror_fac` is always (1,1,1)
    // and `first_hit_entity` is only consumed by the omitted `ENTITIES` block,
    // but they are kept for structural fidelity with the HLSL.
    var first_hit_entity: u32 = ENTITY_FREE;
    var first_hit_pos = vec3<f32>(0.0, 0.0, 0.0);
    var first_hit_mirror_fac = vec3<f32>(1.0, 1.0, 1.0);

    for (var i = 0u; i < 9u; i = i + 1u) {
        // Edge-pixel clamp: WGSL storage out-of-bounds reads are undefined
        // (DX11 SRVs return 0). Clamp the neighbour coord to the screen edge —
        // benign for the 3×3 min/max/hash precompute.
        let off = taa_neighbor_offsets[i];
        let cur_pixel_pos = vec2<u32>(clamp(
            vec2<i32>(pixel_pos) + off,
            vec2<i32>(0, 0),
            vec2<i32>(i32(screen_width) - 1, i32(screen_height) - 1),
        ));
        let cur_first_hit =
            first_hit_data[cur_pixel_pos.x + cur_pixel_pos.y * screen_width];
        let cur_first_hit_result = get_hit_data_from_planes_a2(
            cur_first_hit, cam_pos_int, cam_pos_frac, ray_dir,
        );

        // A-2: `getSpecularNormals(curFirstHit)` is always 0 (plane-0-only).
        let cur_first_hit_specular_normals = 0u;
        let cur_first_hit_entity = cur_first_hit.x & 0x3FFFu;
        let cur_first_hit_is_diffuse = cur_first_hit.y & 0x1u;
        // HLSL: curDist = f16tof32(curFirstHit.w & 0x7FFF);
        //       if ((curFirstHit.z & 0x7FFF) == 0) curDist = 65520;
        var cur_dist = unpack2x16float(cur_first_hit.w & 0x7FFFu).x;
        if ((cur_first_hit.z & 0x7FFFu) == 0u) {
            cur_dist = 65520.0;
        }

        // Track the closest neighbour (the HLSL `if (curDist < firstHitDist)`).
        if (cur_dist < first_hit_dist) {
            first_hit_dist = cur_dist;
            first_hit_entity = cur_first_hit_entity;
            first_hit_pos = cur_first_hit_result.pos;
            first_hit_mirror_fac = cur_first_hit_result.normal_mirror_fac;
        }

        dist_min_max.x = min(dist_min_max.x, cur_dist);
        dist_min_max.y = max(dist_min_max.y, cur_dist);
        // The HLSL's `validNormalsSpec` accumulation folds to a no-op in A-2
        // (`cur_first_hit_specular_normals` is always 0) — omitted.

        let cur_hash = taa_hash_from_data(
            cur_first_hit_is_diffuse, cur_first_hit_specular_normals, cur_first_hit_entity,
        ) & 0xFFFFu;
        if (i == 0u) {
            valid_hash_center = cur_hash;
        } else {
            valid_hashes_comp[(i - 1u) / 2u] |= cur_hash << (16u * ((i - 1u) % 2u));
        }
    }
    // ENTITIES block (`renderTaaSampleReverse.fx:76-84`) — omitted (A-2 is
    // entity-free). `first_hit_entity` / `first_hit_pos` / `first_hit_mirror_fac`
    // are therefore consumed only structurally below.

    // --- Phase 2: the reprojection loop -----------------------------------
    // (`renderTaaSampleReverse.fx:86-161`).
    let pos_virtual = ray_dir * first_hit_dist;
    var color_sum = vec4<f32>(0.0, 0.0, 0.0, 0.0); // .rgb accumulated, .a = accepted count

    // TEMP STEP-8 DEBUG counters
    var dbg_valid = 0.0;
    var dbg_dist_pass = 0.0;
    var dbg_screen_pass = 0.0;

    for (var i = 1u; i < params.sample_age; i = i + 1u) {
        let cur_history_index = (params.taa_index + i) % 128u;
        // The §6 16-deep sample ring — the SECOND `% 32` in the HLSL (`:91`).
        let cur_taa_index = (params.taa_index + i) % TAA_SAMPLE_RING_DEPTH;

        // `curPosVirtual = posVirtual` (+ entityPosChange — omitted, A-2 is
        // entity-free; `entityPosChange` is (0,0,0) without entities).
        let cur_pos_virtual = pos_virtual;
        let slot = camera_history[cur_history_index];
        let cur_taa_jitter = slot.jitter;

        // Reproject into the past frame's screen. The position is expressed
        // current-camera-int-relative, so subtracting the past camera's
        // (also current-int-relative) position gives the past-camera-relative
        // position that `slot.view_proj` (rotation-only) projects correctly.
        let reproject_pos = cur_pos_virtual - slot.cam_pos_from_cur_int;
        let proj = get_screen_index_projection(
            screen_width, screen_height, reproject_pos, slot.view_proj, -cur_taa_jitter,
        );
        if (!proj.valid) {
            continue;
        }
        dbg_valid = dbg_valid + 1.0;

        // Fetch + decompress the past sample (slot-major ring index — the
        // SECOND `% 32` already applied to `cur_taa_index`).
        let cur_samp = taa_samples[
            proj.screen_index + cur_taa_index * screen_width * screen_height
        ];
        let s = taa_decompress_sample(cur_samp);
        // s.dist = sample distance, s.color (.a == 1), s.normal_comp,
        // s.extra_data, s.hash.

        // Distance reject — the noise-insensitive long-term-TAA core.
        let ray_dir_old = normalize(cur_pos_virtual - slot.cam_pos_from_cur_int);
        // `oldVirtualPos = taaOldCamPosFromCurCamInt + rayDirOld * sampleDist
        //                  - entityPosChange` (entityPosChange omitted).
        let old_virtual_pos = slot.cam_pos_from_cur_int + ray_dir_old * s.dist;
        let dist_cur = distance(old_virtual_pos, vec3<f32>(0.0, 0.0, 0.0));
        if (dist_cur < dist_min_max.x * (1022.0 / 1024.0)
            || dist_cur > dist_min_max.y * (1026.0 / 1024.0)
            || s.dist > dist_min_max.y * 2.0) {
            continue;
        }
        dbg_dist_pass = dbg_dist_pass + 1.0;

        // 1-pixel screen-position reject — project the old virtual pos into
        // the CURRENT screen with `params.view_proj` (C# camMatrix). `M * v`
        // — the `05-review.md` perspective-fix convention; do NOT swap.
        let screen_projection_new = params.view_proj * vec4<f32>(old_virtual_pos, 1.0);
        var ndc_new = screen_projection_new.xyz / screen_projection_new.w;
        ndc_new.y = ndc_new.y * -1.0;
        let ndc01_new = ndc_new.xy * 0.5 + vec2<f32>(0.5, 0.5);
        let screen_pos_new = ndc01_new * vec2<f32>(f32(screen_width), f32(screen_height));
        let screen_pos_dif = screen_pos_new - vec2<f32>(pixel_pos);
        if (dot(screen_pos_dif, screen_pos_dif) > 1.0) {
            continue;
        }
        dbg_screen_pass = dbg_screen_pass + 1.0;

        // Rough-specular reweight (`renderTaaSampleReverse.fx:138-148`) — DEAD
        // in A-2: `s.extra_data` is always 0 in the albedo path
        // (`06-design-a2.md` §3.2), so `if (extra_data != 0)` is never taken.
        // Porting the body would pull in `pdf_vndf_isotropic`, a Phase-B
        // function — left as this structural comment per `06-design-a2.md` §7.4.

        // Hash reject — the past sample's hash must match the centre hash or
        // one of the 8 neighbour hashes.
        if (s.hash != valid_hash_center) {
            var is_hash_valid = false;
            for (var h = 0u; h < 8u; h = h + 1u) {
                is_hash_valid = is_hash_valid
                    || s.hash == ((valid_hashes_comp[h / 2u] >> (16u * (h % 2u))) & 0xFFFFu);
            }
            if (!is_hash_valid) {
                continue;
            }
        }

        // `color.a == 1` ⇒ `color_sum.a` counts the accepted history samples.
        color_sum = color_sum + s.color;
    }

    // --- Phase 3: accumulation into taa_sample_accum ----------------------
    // (`renderTaaSampleReverse.fx:163-171`). This is the load-bearing
    // 0.25-spp signal: `sample_weight` is the current frame's count (1.0,
    // written by the first-hit pass); `color_sum.a` is the count of accepted
    // reprojected history samples; `sample_weight + color_sum.a` is the
    // per-pixel accumulated sample count, stored back as f16 in
    // `taa_sample_accum[px].x & 0xFFFF`. Each thread only touches its own
    // pixel — no cross-thread hazard; the first-hit → reproject ordering is a
    // render-graph edge, so wgpu's buffer barriers serialise them.
    let taa_color_comp = taa_sample_accum[pixel_index];
    let weight_rg = unpack2x16float(taa_color_comp.x); // .x = f16(weight), .y = f16(R)
    let gb = unpack2x16float(taa_color_comp.y);        // .x = f16(G), .y = f16(B)
    let sample_weight = weight_rg.x;
    var taa_color = vec3<f32>(weight_rg.y, gb.x, gb.y);
    taa_color = taa_color + color_sum.rgb;

    var new_color_comp = vec2<u32>(0u, 0u);
    new_color_comp.x = pack2x16float(vec2<f32>(sample_weight + color_sum.a, taa_color.r));
    new_color_comp.y = pack2x16float(vec2<f32>(taa_color.g, taa_color.b));
    taa_sample_accum[pixel_index] = new_color_comp;

    // TEMP STEP-8 DEBUG: for the one debug pixel, overwrite with RAW INTEGER
    // counters (no f16 packing) so the readback decode is unambiguous.
    // .x = valid | (dist_pass << 8) | (screen_pass << 16) | (accepted << 24)
    // .y = u32(color_sum.a) | (u32(first_hit_dist) << 16)
    if (pixel_index == params.screen_width * params.screen_height / 2u + 7u) {
        var dbg = vec2<u32>(0u, 0u);
        dbg.x = (u32(dbg_valid) & 0xFFu)
            | ((u32(dbg_dist_pass) & 0xFFu) << 8u)
            | ((u32(dbg_screen_pass) & 0xFFu) << 16u)
            | ((u32(color_sum.a) & 0xFFu) << 24u);
        dbg.y = (u32(color_sum.a) & 0xFFFFu) | ((u32(first_hit_dist) & 0xFFFFu) << 16u);
        taa_sample_accum[pixel_index] = dbg;
    }
}
