// taa.wgsl — the Phase-B `base/` TAA reproject + accumulation + new-sample
// compute passes.
//
// Derives from: render/versions/base/renderTaaSampleReverse.fx
// `reprojectOldSamples` + `calcNewTaaSample` (`09-design-b.md` §5.8). A faithful
// WGSL port of the `base/`-path long-term-memory TAA.
//
// `reproject_old_samples` (Phase B Batch 6 — was the A-2 `albedo/` variant):
// for each pixel, precompute a 3×3 neighbourhood (distance min/max + surface
// hashes + the specular-normal validity mask), write `taa_dist_min_max`, then
// walk up to `sample_age` past frames — reproject this pixel's virtual hit
// position into each past frame's screen, fetch the stored 64-bit sample,
// distance/screen/hash-reject it, sum the accepted history colour, and
// OVERWRITE `taa_sample_accum` with that sum. (In the `base/` pipeline the
// first-hit pass does NOT pre-write the current sample into `taa_sample_accum`
// — it writes `final_color` instead — so this pass overwrites rather than
// reads-adds-writes; `calc_new_taa_sample` then folds in the current frame's
// `final_color` light.)
//
// `calc_new_taa_sample` (Phase B Batch 6 — NEW, `base/renderTaaSampleReverse.fx:
// 170-206`): reconstructs the first-hit virtual path, decompresses the voxel
// type for roughness, reads `final_color` as the current frame's GI light,
// compresses it into the 16-deep `taa_samples` ring, and folds the light into
// `taa_sample_accum` with `sample_weight + 1`. This is the SOLE `taa_samples`
// writer in the `base/` pipeline (the `base/` first-hit no longer writes it —
// `09-design-b.md` §6.3) and the path the per-pixel sample-count signal the GI
// `rayQueueCalc` consumes is maintained for.
//
// `[numthreads(64,1,1)]` in the HLSL → `@workgroup_size(64,1,1)` for both.
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
//   * `getHitDataFromPlanes`: A-2 kept a local single-plane reduction
//     (`get_hit_data_from_planes_a2`); Phase B Batch 1 replaced it with an
//     import of the now-shared full `get_hit_data_from_planes` from
//     `render_pipeline_common.wgsl` (`09-design-b.md` §5.2). The `base/`
//     4-plane first-hit (Batch 2) actually populates planes 1-3 for mirror
//     surfaces, so the specular-reflection loop now runs real iterations.
//     The `get_screen_pos_projection` / `get_screen_index_projection` helpers
//     are likewise shared imports.
//   * `getSpecularNormals` / `validNormalsSpec`: A-2 folded the
//     `validNormalsSpec` accumulation to a no-op (the albedo first-hit always
//     left specular-normals 0 — `06-design-a2.md` §3.2). Phase B Batch 6
//     UN-OMITS it — the `base/` 4-plane first-hit makes `get_specular_normals`
//     real (`09-design-b.md` §5.8.1), and `reproject_old_samples` writes the
//     packed validity mask into `taa_dist_min_max[*].y`.
//   * `screenPosDistanceSqr` reject: A-2's `albedo/` source genuinely uses
//     `> 1.0`; the `base/` source genuinely uses `> 16.0`
//     (`10-impl-b.md` Batch-2 item-#2 finding — a real per-variant divergence,
//     not an A-2 bug). Phase B follows the `base/` shader: `> 16.0`.
//   * The rough-specular reweight branch (`renderTaaSampleReverse.fx:143-153`)
//     is left as a structural dead-`if` comment: porting its body would pull
//     in `pdf_vndf_isotropic`, and in practice `extra_data` only becomes
//     non-zero for rough-specular history samples — the reweight is a quality
//     refinement, not load-bearing for the GI bounce; kept as a documented
//     omission consistent with A-2 §7.4.
//   * Edge-pixel reads: WGSL storage reads out of bounds are undefined (DX11
//     SRVs return 0); the 3×3 neighbour reads clamp the pixel coord to the
//     screen edge before indexing.
//
// naga-oil import module entry points: `reproject_old_samples`,
// `calc_new_taa_sample`.

#import "shaders/render_pipeline_common.wgsl"::{
    get_ray_dir, NORMAL, HIT_UNDEFINED, ENTITY_FREE,
    get_hit_data_from_planes, FirstHitResult, get_specular_normals,
    get_screen_pos_projection, get_screen_index_projection,
    decompress_voxel_type, VoxelType,
}
#import "shaders/taa_common.wgsl"::{
    taa_decompress_sample, taa_compress_sample, taa_hash_from_data,
    taa_neighbor_offsets, TAA_SAMPLE_RING_DEPTH,
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
// `gpu_types::GpuCameraHistorySlot`, 160 bytes/slot):
//   view_proj            (0..64)   — C# camRotOld[i] (rotation-only view-proj)
//   view_proj_inv        (64..128) — C# taaSampleCamTransformInvers[i]
//   cam_pos_from_cur_int slot 128  — C# taaOldCamPosFromCurCamInt[i]
//   jitter               slot 144  — C# taaJitterOld[i]
//
// `view_proj_inv` is the Phase-B addition (`09-design-b.md` §3.6) — the
// reproject pass does not read it, but the slot layout must match the widened
// Rust struct so the `camera_history` storage buffer round-trips. Batch 3+'s
// `renderSampleRefine` is the consumer.
struct GpuCameraHistorySlot {
    view_proj: mat4x4<f32>,
    view_proj_inv: mat4x4<f32>,
    cam_pos_from_cur_int: vec3<f32>,
    jitter: vec2<f32>,
}

// --- the reproject pass's single bind group (`09-design-b.md` §5.8.1) -------
// The reproject pass does not traverse the voxel world (no `shoot_ray`), so it
// binds no world data — its one bind group is `@group(0)`. Phase B Batch 6
// adds the `taa_dist_min_max` read-write binding at slot 5 (the `base/`
// `ReprojectOld` extra output — `base/renderTaaSampleReverse.fx:9,79`).
@group(0) @binding(0) var<uniform> params: GpuTaaParams;
@group(0) @binding(1) var<storage, read> camera_history: array<GpuCameraHistorySlot, 128>;
@group(0) @binding(2) var<storage, read> first_hit_data: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read> taa_samples: array<vec2<u32>>;
@group(0) @binding(4) var<storage, read_write> taa_sample_accum: array<vec2<u32>>;
@group(0) @binding(5) var<storage, read_write> taa_dist_min_max: array<vec2<u32>>;

// --- the `calc_new_taa_sample` pass's bind group (`09-design-b.md` §4.10) ---
// `calc_new_taa_sample` does NOT traverse the voxel world, so it binds only
// `voxel_types` (not the whole `@group(0)` world layout). It is placed on
// `@group(1)` so its bindings do not collide with the reproject pass's
// `@group(0)` set in this shared naga-oil module; the `calc_new_taa_sample`
// pipeline's layout vec is `[empty, calc_new_taa_sample_layout]` (the same
// `@group`-placeholder pattern `naadf_global_illum.wgsl` uses).
@group(1) @binding(0) var<uniform> cnts_params: GpuTaaParams;
@group(1) @binding(1) var<storage, read> cnts_first_hit_data: array<vec4<u32>>;
@group(1) @binding(2) var<storage, read> cnts_final_color: array<vec2<u32>>;
@group(1) @binding(3) var<storage, read> cnts_voxel_types: array<vec4<u32>>;
@group(1) @binding(4) var<storage, read_write> cnts_taa_samples: array<vec2<u32>>;
@group(1) @binding(5) var<storage, read_write> cnts_taa_sample_accum: array<vec2<u32>>;

// `get_hit_data_from_planes`, `FirstHitResult`, `get_screen_pos_projection`,
// `get_screen_index_projection`, `get_specular_normals`, `decompress_voxel_type`
// are imported from `render_pipeline_common.wgsl` (Phase B Batch 1 promoted the
// shared helpers out of this file — `09-design-b.md` §5.2).

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
    // (`base/renderTaaSampleReverse.fx:35-79`). `valid_hashes_comp` packs the 8
    // neighbour hashes 2-per-u32; `valid_hash_center` is the centre hash;
    // `valid_normals_spec` accumulates the packed specular-normal validity
    // mask (un-omitted in Phase B Batch 6 — the `base/` 4-plane first-hit
    // makes `get_specular_normals` real).
    var valid_hashes_comp = array<u32, 4>(0u, 0u, 0u, 0u);
    var valid_hash_center: u32 = 0u;
    var valid_normals_spec: u32 = 0u;
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
        let cur_first_hit_result = get_hit_data_from_planes(
            cur_first_hit, cam_pos_int, cam_pos_frac, ray_dir,
        );

        // Phase B Batch 6: `get_specular_normals` is real — the `base/`
        // 4-plane first-hit populates planes 1-3 for mirror surfaces
        // (`base/renderTaaSampleReverse.fx:51`).
        let cur_first_hit_specular_normals = get_specular_normals(cur_first_hit);
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
        // `validNormalsSpec` accumulation (`base/renderTaaSampleReverse.fx:
        // 68-70`) — un-omitted in Phase B Batch 6. Three 7-bit fields, one per
        // mirror-bounce plane (0..2), each a `1 << normalIndex` bit.
        valid_normals_spec |= 1u << (cur_first_hit_specular_normals & 0x7u);
        valid_normals_spec |= (1u << ((cur_first_hit_specular_normals >> 3u) & 0x7u)) << 7u;
        valid_normals_spec |= (1u << ((cur_first_hit_specular_normals >> 6u) & 0x7u)) << 14u;

        let cur_hash = taa_hash_from_data(
            cur_first_hit_is_diffuse, cur_first_hit_specular_normals, cur_first_hit_entity,
        ) & 0xFFFFu;
        if (i == 0u) {
            valid_hash_center = cur_hash;
        } else {
            valid_hashes_comp[(i - 1u) / 2u] |= cur_hash << (16u * ((i - 1u) % 2u));
        }
    }
    // Write the `base/` `ReprojectOld` extra output `taa_dist_min_max`
    // (`base/renderTaaSampleReverse.fx:79`). `.x` = `f16(distMin) |
    // f16(distMax)<<16`, `.y` = the packed specular-normal validity mask. This
    // is the write Batch 4's `renderSampleRefine` reprojection validity test
    // consumes — un-blocking the `sampleRefine → valid_samples_compressed →
    // spatialResampling` chain (the visible GI bounce — `10-impl-b.md` Batch-5
    // note for B6).
    let dist_min_packed = pack2x16float(vec2<f32>(dist_min_max.x, 0.0)) & 0xFFFFu;
    let dist_max_packed = pack2x16float(vec2<f32>(dist_min_max.y, 0.0)) & 0xFFFFu;
    taa_dist_min_max[pixel_index] =
        vec2<u32>(dist_min_packed | (dist_max_packed << 16u), valid_normals_spec);

    // ENTITIES block (`base/renderTaaSampleReverse.fx:81-89`) — omitted (Phase
    // B is entity-free). `first_hit_entity` / `first_hit_pos` /
    // `first_hit_mirror_fac` are therefore consumed only structurally below.

    // --- Phase 2: the reprojection loop -----------------------------------
    // (`base/renderTaaSampleReverse.fx:91-166`).
    let pos_virtual = ray_dir * first_hit_dist;
    var color_sum = vec4<f32>(0.0, 0.0, 0.0, 0.0); // .rgb accumulated, .a = accepted count

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

        // Screen-position reject — project the old virtual pos into the
        // CURRENT screen with `params.view_proj` (C# camMatrix). `M * v` — the
        // `05-review.md` perspective-fix convention; do NOT swap. The `base/`
        // source uses a `> 16.0` reject (`base/renderTaaSampleReverse.fx:139`)
        // — a looser screen-position-similarity gate than the A-2 `albedo/`
        // path's `> 1.0` (a real per-variant divergence — `10-impl-b.md`
        // Batch-2 item-#2 finding; the `base/` source is authoritative here).
        let screen_projection_new = params.view_proj * vec4<f32>(old_virtual_pos, 1.0);
        var ndc_new = screen_projection_new.xyz / screen_projection_new.w;
        ndc_new.y = ndc_new.y * -1.0;
        let ndc01_new = ndc_new.xy * 0.5 + vec2<f32>(0.5, 0.5);
        let screen_pos_new = ndc01_new * vec2<f32>(f32(screen_width), f32(screen_height));
        let screen_pos_dif = screen_pos_new - vec2<f32>(pixel_pos);
        if (dot(screen_pos_dif, screen_pos_dif) > 16.0) {
            continue;
        }

        // Rough-specular reweight (`base/renderTaaSampleReverse.fx:143-153`) —
        // left as a structural comment: `if (extra_data != 0)` only fires for
        // rough-specular history samples, and porting its body would pull in
        // `pdf_vndf_isotropic`. It is a quality refinement, not load-bearing
        // for the GI bounce — kept as a documented omission per the
        // file-header deviations note.

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

    // --- Phase 3: OVERWRITE taa_sample_accum with the reprojected history --
    // (`base/renderTaaSampleReverse.fx:167`). The `base/` pipeline differs
    // from the A-2 `albedo/` one here: the `base/` first-hit does NOT pre-write
    // the current frame's sample into `taa_sample_accum` (it writes
    // `final_color`), so `ReprojectOld` *overwrites* `taa_sample_accum` with
    // just the reprojected-history sum — `uint2(f16(colorSum.w) |
    // f16(colorSum.r)<<16, f16(colorSum.g) | f16(colorSum.b)<<16)`.
    // `calc_new_taa_sample` (the second `base/` pass) then folds in the
    // current frame's `final_color` light with `sample_weight + 1`.
    // `color_sum.a` is the count of accepted reprojected history samples — the
    // per-pixel accumulated sample count `rayQueueCalc` reads, stored as f16
    // in `taa_sample_accum[px].x & 0xFFFF`. Each thread only touches its own
    // pixel — no cross-thread hazard.
    var new_color_comp = vec2<u32>(0u, 0u);
    new_color_comp.x = pack2x16float(vec2<f32>(color_sum.a, color_sum.r));
    new_color_comp.y = pack2x16float(vec2<f32>(color_sum.g, color_sum.b));
    taa_sample_accum[pixel_index] = new_color_comp;
}

// --- the `calc_new_taa_sample` pass (`base/renderTaaSampleReverse.fx:170-206`)
// Folds the current frame's denoised GI result (`final_color`) into the
// 16-deep `taa_samples` ring + the `taa_sample_accum` history. This is the
// SOLE `taa_samples` writer in the `base/` pipeline.
@compute @workgroup_size(64, 1, 1)
fn calc_new_taa_sample(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let pixel_index = global_id.x;
    if (pixel_index >= cnts_params.screen_width * cnts_params.screen_height) {
        return;
    }

    let cam_pos_int = cnts_params.cam_pos_int;
    let cam_pos_frac = cnts_params.cam_pos_frac;
    let screen_width = cnts_params.screen_width;
    let screen_height = cnts_params.screen_height;

    // HLSL: pixelPos = uint2(globalID.x % w, globalID.x / w).
    let pixel_pos = vec2<u32>(pixel_index % screen_width, pixel_index / screen_width);
    // `getRayDir(invCamMatrix, pixelPos, w, h)` — no jitter, exactly as the
    // `base/` HLSL (`base/renderTaaSampleReverse.fx:178`).
    let ray_dir = get_ray_dir(
        cnts_params.inv_view_proj, pixel_pos, screen_width, screen_height,
        vec2<f32>(0.0, 0.0),
    );

    let first_hit = cnts_first_hit_data[pixel_index];
    let first_hit_result = get_hit_data_from_planes(
        first_hit, cam_pos_int, cam_pos_frac, ray_dir,
    );

    // Decompress the voxel type for the roughness (`base/...:183-184`).
    let voxel_type = first_hit.z & 0x7FFFu;
    let first_hit_voxel_type_data = decompress_voxel_type(cnts_voxel_types[voxel_type]);
    let specular_normals = get_specular_normals(first_hit);

    // Read the current frame's GI light from `final_color`
    // (`base/...:187-188` — raw RGB f16 triple, no weight field).
    let light_comp = cnts_final_color[pixel_index];
    let light_lo = unpack2x16float(light_comp.x);
    let light_hi = unpack2x16float(light_comp.y);
    let light = vec3<f32>(light_lo.x, light_lo.y, light_hi.x);

    // `extra_data8` — the 5-bit roughness for a non-diffuse surface
    // (`base/...:189-192`). `isDiffuse` is `firstHit.y & 0x1`.
    let is_diffuse = first_hit.y & 0x1u;
    var extra_data8: u32 = 0u;
    if (is_diffuse == 0u) {
        extra_data8 = 1u + u32(pow(first_hit_voxel_type_data.roughness, 0.5) * 30.5);
    }

    // Compress the new sample into the 16-deep ring. The HLSL passes the f16
    // *bits* (`voxelType == 0 ? f32tof16(65520) : (firstHit.w & 0x7FFF)`);
    // `taa_compress_sample` (the A-2 helper) takes a float `dist` and does the
    // `f32tof16` itself, so the float distance is passed here — `65520.0` for
    // a miss (`voxel_type == 0`), else the decoded `firstHit.w & 0x7FFF` f16.
    var dist: f32;
    if (voxel_type == 0u) {
        dist = 65520.0;
    } else {
        dist = unpack2x16float(first_hit.w & 0x7FFFu).x;
    }
    let sample_comp = taa_compress_sample(
        dist, light, first_hit_result.normal_tang & 0x7u, is_diffuse,
        specular_normals, extra_data8, first_hit.x & 0x3FFFu,
    );
    // The 16-deep ring — HLSL `% 32` → `% TAA_SAMPLE_RING_DEPTH` (the §6
    // VRAM lever, `taa_common.wgsl`).
    cnts_taa_samples[
        (cnts_params.taa_index % TAA_SAMPLE_RING_DEPTH) * screen_width * screen_height
        + pixel_index
    ] = sample_comp;

    // Fold the current frame's light into `taa_sample_accum`
    // (`base/...:197-205`): `sample_weight` is the reprojected-history count
    // `ReprojectOld` just wrote; `+ 1` adds this frame's sample.
    let taa_color_comp = cnts_taa_sample_accum[pixel_index];
    let weight_rg = unpack2x16float(taa_color_comp.x); // .x = f16(weight), .y = f16(R)
    let gb = unpack2x16float(taa_color_comp.y);        // .x = f16(G), .y = f16(B)
    let sample_weight = weight_rg.x;
    let taa_color = vec3<f32>(weight_rg.y, gb.x, gb.y) + light;

    var new_color_comp = vec2<u32>(0u, 0u);
    new_color_comp.x = pack2x16float(vec2<f32>(sample_weight + 1.0, taa_color.r));
    new_color_comp.y = pack2x16float(vec2<f32>(taa_color.g, taa_color.b));
    cnts_taa_sample_accum[pixel_index] = new_color_comp;
}
