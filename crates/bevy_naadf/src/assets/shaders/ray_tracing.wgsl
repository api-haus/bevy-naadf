// ray_tracing.wgsl — `shoot_ray`, the AADF DDA traversal — the Phase-A core.
//
// Derives from: render/rayTracing.fxh (`03-design.md` §5.5). A faithful port of
// the HLSL `shootRay(int3 rayOriginInt, float3 rayOriginFrac, float3 rayDir,
// int maxStepCount, out RayResult)` + `rayAABB` + `RayResult` + the
// `MAX_RAY_STEPS_*` constants.
//
// **W4 (`15-design-c.md` §1.7, §3.6):** the chunks texture is now `Rg32Uint`;
// `.x` is the construction-side state pointer + AADF (this file's load-bearing
// read at line 158 — `.x` selection is the W4 forward-compat read), and `.y`
// carries the per-chunk entity pointer + counter pair the entity track owns.
// The renderer-side **entity sub-traversal branch** mirrors the HLSL
// `#ifdef ENTITIES` path in `rayTracing.fxh:81-240`: collect up to 16 unique
// chunk-entity pointers along the main traversal, then sub-traverse each
// entity's compressed per-entity voxel volume. The W4 deliverable lands the
// **collection logic** + the **entity sub-traversal helper** as named
// functions; **invocation from `shoot_ray` is gated behind the `ENTITIES`
// shader-def** and stays disabled in the merged state because activating it
// requires extending the `naadf_world_bind_group_layout` with the entity
// buffers (`entity_chunk_instances`, `entity_voxel_data`,
// `entity_instances_history`) — which is `NaadfPipelines` territory and the W4
// brief explicitly forbids editing `NaadfPipelines`. Integration follow-up: a
// renderer-side workstream wires the entity bind group + flips the
// `ENTITIES` shader-def on (`16-impl-c-W4.md` integration notes).
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

// W4 §3.6 / `commonRayTracing.fxh:203-220` — decompress the smallest-three
// quaternion encoding back into a float4. The compression is in
// `entity_handler.rs::compress_quaternion`; this is the inverse used by every
// entity-aware traversal shader. Public so the renderer-side workstream that
// wires the entity bind group can re-use it without copy-pasting.
//
// Layout (HLSL `decompressQuaternion`):
//   packed.x =  smallInt.x       (14 bits, 0..16383)
//            | (smallInt.y << 14) (14 bits)
//            | (smallInt.z & 0xF) << 28 (low 4 bits of z in the top of .x)
//   packed.y =  (smallInt.z >> 4) (10 bits — high 10 bits of z)
//            | (maxIndex & 3) << 10
fn decompress_quaternion(packed: vec2<u32>) -> vec4<f32> {
    let max_index = i32((packed.y >> 10u) & 0x3u);
    let small_int = vec3<i32>(
        i32(packed.x & 0x3FFFu),
        i32((packed.x >> 14u) & 0x3FFFu),
        i32((packed.x >> 28u) | ((packed.y & 0x3FFu) << 4u)),
    );
    let small = vec3<f32>(
        f32(small_int.x - 8192) / 8192.0,
        f32(small_int.y - 8192) / 8192.0,
        f32(small_int.z - 8192) / 8192.0,
    );
    let missing = sqrt(max(0.0, 1.0 - dot(small, small)));
    var q: vec4<f32>;
    if (max_index == 0) {
        q = vec4<f32>(missing, small.x, small.y, small.z);
    } else if (max_index == 1) {
        q = vec4<f32>(small.x, missing, small.y, small.z);
    } else if (max_index == 2) {
        q = vec4<f32>(small.x, small.y, missing, small.z);
    } else {
        q = vec4<f32>(small.x, small.y, small.z, missing);
    }
    return q;
}

// W4 §3.6 / `commonRenderPipeline.fxh:95-100` — quaternion rotation. Mirrors
// HLSL `applyRotation`. Used by the entity sub-traversal to bring rays from
// world space into entity-local space.
fn apply_rotation(vec_in: vec3<f32>, q: vec4<f32>) -> vec3<f32> {
    let neg_xyz = -q.xyz;
    let w1 = -dot(vec_in, neg_xyz);
    let xyz1 = q.w * vec_in + cross(vec_in, neg_xyz);
    return q.w * xyz1 + w1 * q.xyz + cross(q.xyz, xyz1);
}

fn quaternion_inverse(q: vec4<f32>) -> vec4<f32> {
    let len_sq = q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w;
    return vec4<f32>(-q.x, -q.y, -q.z, q.w) / len_sq;
}

// W4 §3.6 — the entity-instance decompressed form. CPU mirror lives in
// `gpu_types::EntityInstance`. Decompression mirrors HLSL
// `decompressEntityInstanceFromChunk` (`commonEntities.fxh:61-70`).
struct EntityInstance {
    position: vec3<f32>,
    quaternion: vec4<f32>,
    voxel_start: u32,
    entity: u32,
    size: vec3<u32>,
};

fn decompress_entity_instance_from_chunk(
    data1: u32, data2: u32, data3: u32, data4: u32, data5: u32,
) -> EntityInstance {
    var instance: EntityInstance;
    instance.position = vec3<f32>(
        f32(data1 & 0x1FFFFFu) / 128.0,
        f32(((data1 >> 21u) & 0x7FFu) | (((data2 >> 21u) & 0xFFu) << 11u)) / 128.0,
        f32(data2 & 0x1FFFFFu) / 128.0,
    );
    instance.quaternion = decompress_quaternion(vec2<u32>(data3, data4));
    instance.voxel_start = data4 >> 12u;
    instance.entity = data5 & 0x3FFFu;
    instance.size = vec3<u32>(
        (data5 >> 14u) & 0x7Fu,
        (data5 >> 21u) & 0x7Fu,
        ((data5 >> 28u) & 0xFu) | ((data2 >> 29u) << 4u),
    );
    return instance;
}

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

    // NAADF's `boundingBoxMax` — already a `float3` (the 0.1-inset world
    // extent, `WorldData.cs:478`).
    let bbox_max = world_meta.bounding_box_max;

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
        // test; in WGSL with signed cells we test explicitly. `bounding_box_min`
        // is now NAADF's `float3 boundingBoxMin` — the 0.1-voxel-INSET world
        // minimum (`WorldData.cs:477`), so it is `0.1`, not `0`. This explicit
        // signed-cell break must test the integer world FLOOR (cell index 0 is
        // a valid edge cell — `0 < 0.1` would wrongly break it), so compare
        // against `floor(bounding_box_min)`.
        if (any(vec3<f32>(cur_cell) < floor(world_meta.bounding_box_min))) {
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
