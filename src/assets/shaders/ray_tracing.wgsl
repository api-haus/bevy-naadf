// ray_tracing.wgsl — `shoot_ray`, the AADF DDA traversal — the Phase-A core.
//
// Derives from: render/rayTracing.fxh (`03-design.md` §5.5). A faithful port of
// the HLSL `shootRay(int3 rayOriginInt, float3 rayOriginFrac, float3 rayDir,
// int maxStepCount, out RayResult)` + `rayAABB` + `RayResult` + the
// `MAX_RAY_STEPS_*` constants. The `#ifdef ENTITIES` sub-traversal branch is
// **omitted** — Phase A is entity-free (`03-design.md` §7.5).
//
// The DDA (Amanatides & Woo) descends chunk → block → voxel, reads each cell's
// AADF empty-cuboid distances, and advances the ray to the cuboid boundary in
// a single step (`02-research.md` §1.1.5). The AADF bit-fields are read exactly
// as the C# `shootRay` does — `02-research.md` divergence #4 flags the
// two-voxels-per-`u32` packing as easy to get wrong; this port matches it.
//
// naga-oil import module — pulls in the `@group(0)` world bindings from
// `world_data.wgsl`.

#import "shaders/world_data.wgsl"::{chunks, blocks, voxels, world_meta}
#import "shaders/common.wgsl"::flatten_index

// Ray-step caps (HLSL `rayTracing.fxh` `MAX_RAY_STEPS_*`).
const MAX_RAY_STEPS_PRIMARY: i32 = 120;
const MAX_RAY_STEPS_SECONDARY: i32 = 100;
const MAX_RAY_STEPS_SUN: i32 = 120;
const MAX_RAY_STEPS_SUN_SECONDARY: i32 = 80;
const MAX_RAY_STEPS_VISIBILITY: i32 = 60;

// The result of a `shoot_ray` traversal (HLSL `struct RayResult`).
struct RayResult {
    // The hit voxel's 15-bit type id (only meaningful on a hit).
    hit_type: u32,
    // Distance along the ray to the hit, in voxels. < 0 ⇒ no hit.
    length: f32,
    // Surface normal at the hit.
    normal: vec3<f32>,
    // Integer voxel position of the hit cell.
    voxel_pos: vec3<i32>,
    // DDA iterations taken (for the ray-step debug view).
    step_count: i32,
    // Packed `(3-bit normal index, distance-along-normal)` plane code
    // (HLSL `normalComp`).
    normal_comp: u32,
}

// `rayAABB` — slab-test a ray against an axis-aligned box. Returns whether the
// ray hits, the near/far distances in `dist_min_max`, and the face-normal mask
// of the entry face in `normal_mask` (HLSL `rayAABB`).
struct AabbHit {
    hit: bool,
    dist_min_max: vec2<f32>,
    normal_mask: vec3<f32>,
}

fn ray_aabb(
    ray_origin: vec3<f32>,
    ray_dir: vec3<f32>,
    rec_min: vec3<f32>,
    rec_max: vec3<f32>,
) -> AabbHit {
    var result: AabbHit;
    let ray_dir_frac = 1.0 / ray_dir;

    let rec_min_dist = (rec_min - ray_origin) * ray_dir_frac;
    let rec_max_dist = (rec_max - ray_origin) * ray_dir_frac;

    let t1 = min(rec_min_dist, rec_max_dist);
    let t2 = max(rec_min_dist, rec_max_dist);
    var t_near = max(max(t1.x, t1.y), t1.z);
    let t_far = min(min(t2.x, t2.y), t2.z);
    // `step(tNear, t1)` — which axis the entry face is on.
    result.normal_mask = step(vec3<f32>(t_near, t_near, t_near), t1);

    t_near = max(0.0, t_near);
    result.dist_min_max = vec2<f32>(t_near, t_far);

    result.hit = !(t_far < 0.0 || t_near > t_far);
    return result;
}

// `shoot_ray` — the AADF DDA traversal. Casts the ray (origin split into
// `ray_origin_int` + `ray_origin_frac`, D1) through the chunk → block → voxel
// hierarchy, using each empty cell's AADF cuboid to skip empty space in one
// step. Returns `true` on a geometry hit (or, mirroring the C#, `true` when
// zero steps were taken — a degenerate origin-inside-geometry case).
//
// Faithful port of the no-entities path of HLSL `shootRay`.
fn shoot_ray(
    ray_origin_int: vec3<i32>,
    ray_origin_frac: vec3<f32>,
    ray_dir: vec3<f32>,
    max_step_count: i32,
    ray_result: ptr<function, RayResult>,
) -> bool {
    let inv_ray_dir_abs = abs(1.0 / (vec3<f32>(0.000000001) + ray_dir));

    // `isNegative` — 1 where the ray points in the negative direction.
    let is_negative = vec3<u32>(step(ray_dir, vec3<f32>(0.0, 0.0, 0.0)));
    // Per-axis AADF bit-shift for voxels/blocks (2-bit fields) and chunks
    // (5-bit fields), selected by ray sign — HLSL `shiftMaskVoxelAndBlocks` /
    // `shiftMaskChunk`.
    let shift_voxel_block = vec3<u32>(
        select(2u, 0u, is_negative.x == 1u),
        select(6u, 4u, is_negative.y == 1u),
        select(10u, 8u, is_negative.z == 1u),
    );
    let shift_chunk = vec3<u32>(
        select(5u, 0u, is_negative.x == 1u),
        select(15u, 10u, is_negative.y == 1u),
        select(25u, 20u, is_negative.z == 1u),
    );

    let start_pos = ray_origin_frac;
    var cur_dist = 0.0;
    var mask = vec3<f32>(0.0, 0.0, 0.0);
    (*ray_result).length = -1.0;
    (*ray_result).normal_comp = 0x1FFFFu;
    (*ray_result).hit_type = 0u;
    (*ray_result).voxel_pos = vec3<i32>(0, 0, 0);

    let bbox_max = vec3<f32>(world_meta.bounding_box_max);

    var step_count: i32 = 0;
    var cur_pos = start_pos;
    loop {
        if (step_count >= max_step_count) {
            break;
        }
        // `curPos = mad(rayDir, curDist, startPos)` — current ray position
        // (in frac space, relative to `ray_origin_int`).
        cur_pos = ray_dir * cur_dist + start_pos;
        // `curCell = (uint3)((int3)floor(mad(mask, sign(rayDir)*0.5, curPos)) + rayOriginInt)`
        // — the integer cell the ray is in, nudged off a face by `mask`.
        let cell_f = floor(mask * (sign(ray_dir) * 0.5) + cur_pos) + vec3<f32>(ray_origin_int);
        let cur_cell = vec3<i32>(cell_f);

        // `if (any((float3)curCell >= boundingBoxMax)) break;`
        if (any(vec3<f32>(cur_cell) >= bbox_max)) {
            break;
        }
        // Negative cells are outside the world too — the C# relies on the
        // `uint3` cast wrapping huge, which then trips the `>= boundingBoxMax`
        // test; in WGSL with signed cells we test explicitly.
        if (any(cur_cell < world_meta.bounding_box_min)) {
            break;
        }

        // --- chunk lookup ---------------------------------------------------
        let chunk_pos = vec3<u32>(cur_cell) / 16u;
        let voxel_pos_in_chunk = vec3<u32>(cur_cell) % 16u;
        var cur_node: u32 = textureLoad(chunks, vec3<i32>(chunk_pos), 0).x;

        // `boundsInDir` — per-axis AADF cell-count the ray may skip.
        var bounds_in_dir = vec3<u32>(1u, 1u, 1u);

        // `if (curNode.x >> 31)` — chunk is mixed (has children): descend.
        if ((cur_node >> 31u) != 0u) {
            let block_pos_in_chunk = voxel_pos_in_chunk / 4u;
            let block_index =
                (cur_node & 0x3FFFFFFFu) + flatten_index(block_pos_in_chunk, 4u, 16u);
            cur_node = blocks[block_index];
            let voxel_pos_in_block = vec3<u32>(cur_cell) % 4u;

            let block_is_parent = (cur_node >> 31u) != 0u;
            if (block_is_parent) {
                // Descend into the packed voxel buffer. The voxel index inside
                // the block is flattened, then `voxelStartIndex` is a
                // *u32-element* offset (two voxels per u32 —
                // `02-research.md` divergence #4).
                let voxel_index_in_block = flatten_index(voxel_pos_in_block, 4u, 16u);
                let voxel_start_index =
                    (cur_node & 0x3FFFFFFFu) + voxel_index_in_block / 2u;
                let cur_voxel_pair = voxels[voxel_start_index];
                cur_node = (cur_voxel_pair >> (16u * (voxel_index_in_block & 0x1u))) & 0xFFFFu;
                // A full voxel re-tags itself as uniform-full so the shared
                // `& 0x40000000` hit test below catches it.
                if ((cur_node >> 15u) != 0u) {
                    cur_node = cur_node | (1u << 30u);
                }
            }
            // AADF: 2-bit fields. `boundsInDir = (curNode >> shift) & 0x3`.
            bounds_in_dir = vec3<u32>(
                (cur_node >> shift_voxel_block.x) & 0x3u,
                (cur_node >> shift_voxel_block.y) & 0x3u,
                (cur_node >> shift_voxel_block.z) & 0x3u,
            );
            if (!block_is_parent) {
                // The cell is an empty *block* (not descended to voxels): its
                // AADF is in block units; expand it into voxel units, offset
                // by the ray's voxel position within the block.
                //   boundsInDir * 4 + (isNegative ? voxelPosInBlock
                //                                  : 3 - voxelPosInBlock)
                let offset = select(
                    3u - voxel_pos_in_block,
                    voxel_pos_in_block,
                    is_negative == vec3<u32>(1u, 1u, 1u),
                );
                bounds_in_dir = bounds_in_dir * 4u + offset;
            }
        } else {
            // Chunk is *not* mixed. If it is uniform-full the shared hit test
            // below catches it; if empty, its 5-bit AADF is in chunk units —
            // expand into voxel units, offset by the ray's voxel position
            // within the chunk:
            //   (isNegative ? voxelPosInChunk : 15 - voxelPosInChunk)
            //     + 16 * ((curNode >> shiftChunk) & 0x1F)
            let offset = select(
                15u - voxel_pos_in_chunk,
                voxel_pos_in_chunk,
                is_negative == vec3<u32>(1u, 1u, 1u),
            );
            bounds_in_dir = offset + 16u * vec3<u32>(
                (cur_node >> shift_chunk.x) & 0x1Fu,
                (cur_node >> shift_chunk.y) & 0x1Fu,
                (cur_node >> shift_chunk.z) & 0x1Fu,
            );
        }

        // `if (curNode.x & 0x40000000)` — uniform-full cell: it is a hit.
        if ((cur_node & 0x40000000u) != 0u) {
            (*ray_result).hit_type = cur_node & 0x7FFFu;
            (*ray_result).length = cur_dist;
            (*ray_result).voxel_pos = cur_cell;
            break;
        }

        // DDA step: advance the ray to the near face of the skip cuboid.
        //   distForIntersect = (1 + boundsInDir
        //       - (1 - mask) * abs(isNegative - frac(curPos))) * invRayDirAbs
        let dist_for_intersect = (vec3<f32>(1.0, 1.0, 1.0) + vec3<f32>(bounds_in_dir)
            - (vec3<f32>(1.0, 1.0, 1.0) - mask)
                * abs(vec3<f32>(is_negative) - fract(cur_pos)))
            * inv_ray_dir_abs;
        let min_dist = min(dist_for_intersect.x, min(dist_for_intersect.y, dist_for_intersect.z));
        // `mask = step(distForIntersect, minDist)` — which axis we crossed.
        mask = step(dist_for_intersect, vec3<f32>(min_dist, min_dist, min_dist));
        cur_dist = cur_dist + max(min_dist, 0.0001);
        step_count = step_count + 1;
    }

    (*ray_result).step_count = step_count;
    if ((*ray_result).length <= 0.0) {
        (*ray_result).length = 99999999.0;
        // The C# returns `stepCount == 0` here — a degenerate
        // origin-already-inside-geometry case counts as a hit.
        return step_count == 0;
    }

    // Reconstruct the hit normal from the last-crossed-axis `mask` and ray sign
    // (HLSL: `rayResult.normal = mask * -sign(rayDir)`), then build the packed
    // `normalComp` plane code.
    (*ray_result).normal = mask * -sign(ray_dir);
    let normal = (*ray_result).normal;
    let normal_dot = dot(normal, vec3<f32>(1.0, 3.0, 5.0));
    // (abs(normalDot) - (normalDot > 0 ? 0 : 1)) + 1
    //   + (abs(dot(voxelPos, normal)) + max(0, dot(normal, (1,1,1)))) * 8
    let voxel_pos_f = vec3<f32>((*ray_result).voxel_pos);
    let normal_index =
        (abs(normal_dot) - select(1.0, 0.0, normal_dot > 0.0)) + 1.0;
    let dist_along_normal =
        abs(dot(voxel_pos_f, normal)) + max(0.0, dot(normal, vec3<f32>(1.0, 1.0, 1.0)));
    (*ray_result).normal_comp = u32(normal_index + dist_along_normal * 8.0);
    return true;
}
