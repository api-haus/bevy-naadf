// Phase-C W3 — `boundsCalc.fx` ported to WGSL (`15-design-c.md` §4.2,
// `16-impl-c-W3.md`). Faithful port of NAADF's
// `Content/shaders/world/data/boundsCalc.fx` (209 lines): the regime-2
// background per-frame chunk-AADF queue + the regime-1 one-shot seed.
//
// Three entry points:
//   1. `add_initial_groups_to_bound_queue` (`numthreads(64,1,1)`) —
//      regime-1 startup: seed every 4³-chunk group into the size-0 X / Y / Z
//      queues. Dispatched once at startup after W1's `compute_block_bounds`.
//   2. `prepare_group_bounds`  (`numthreads(1,1,1)`)  — regime-2 picker:
//      single-thread scan of `bound_queue_info[0..32*3]` for the first
//      non-empty queue, slice up to `max_group_bound_dispatch` items into
//      `bound_refined_info`, advance the queue start cursor, write the
//      `compute_group_bounds` indirect-dispatch count into
//      `bound_dispatch_indirect[0]`.
//   3. `compute_group_bounds`  (`numthreads(4,4,4)`)  — regime-2 worker: 64
//      chunks per group processed in parallel; each empty chunk expands its
//      5-bit AADF by one cell along the queue's axis (`addBoundsGroup` +
//      `checkMatchingBounds` inlined here — these helpers are NOT in
//      `bounds_common.wgsl`, which ships only the 2-bit `ComputeBounds4`
//      family for W1 / W2). Thread-0 re-enqueues the group into the
//      next-bound-size queue.
//
// MonoGame → wgpu deviations (documented per `15-design-c.md` §1.5, §1.6):
//
// - HLSL `RWStructuredBuffer<uint3> boundGroupMasks` (a per-axis `uint3` per
//   group, with HLSL syntactic support for `boundGroupMasks[g][axis] &= …`)
//   becomes WGSL `array<atomic<u32>>` of length `bound_group_count * 3`,
//   indexed `group_index * 3 + axis`. WGSL forbids `atomic<vec3<u32>>` and a
//   `struct { atomic<u32>, atomic<u32>, atomic<u32> }` array stride is also
//   16 B — but the per-axis flat array is exactly the access pattern the C#
//   has (every call site updates a single axis at a time —
//   `boundsCalc.fx:135,179,183`). The flat layout matches the C# access 1:1.
// - HLSL `RWByteAddressBuffer.Store(0, value)` (`boundsCalc.fx:92`) becomes
//   WGSL `bound_dispatch_indirect[0] = value`. The buffer carries
//   `dispatch_workgroups_indirect` args layout: 5 × u32 = [GroupCountX,
//   GroupCountY, GroupCountZ, _, _]; we write `GroupCountX` and leave the
//   rest at the prepare-pass startup-seed of 1.
// - HLSL `chunks[chunkPos]` reading + writing the chunk texel uses
//   single-channel `R32Uint` here (W4 owns the `R32Uint` → `Rg32Uint`
//   widening — `15-design-c.md` §1.7). **Forward-compat**: every
//   `textureLoad(chunks, ...)` here uses `.x` so the W4 sweep is a no-op for
//   this shader. The C# `#ifdef ENTITIES` branches at `boundsCalc.fx:105-109,
//   140-144, 164-168` are omitted: this is the non-`#ifdef-ENTITIES` path.
// - HLSL `InterlockedAdd(boundQueueInfo[...].size, 1, originalSize)`
//   (`boundsCalc.fx:185`) becomes WGSL
//   `atomicAdd(&bound_queue_info[idx].size_atomic, 1u)`. Per the same
//   pattern W1 uses for `block_voxel_count`'s atomic cursors: declare the
//   `size` field as `atomic<u32>`.
// - HLSL `groupshared bool anyBoundsIncrease = false;` (`boundsCalc.fx:34`)
//   becomes WGSL `var<workgroup> any_bounds_increase: atomic<u32>` — WGSL
//   doesn't allow `bool` in workgroup storage cleanly; we use `0u`/`1u`. The
//   variable is set but the GPU shader never reads it back (it is a
//   diagnostic in the C# code path — same here).
//
// `boundsCalc.fx:13-20` `BoundQueueInfo` struct port:
struct BoundQueueInfo {
    start: u32,
    size: atomic<u32>,
};

// Construction-side params uniform (shared with W1's `chunk_calc.wgsl`;
// see `15-design-c.md` §1.8, §5.1 — `GpuConstructionParams`). The
// `bound_group_queue_max_size` field IS `boundGroupCount` (`chunkCount/64`).
struct ConstructionParams {
    size_in_chunks: vec3<u32>,
    _pad0: u32,
    group_size_in_groups: vec3<u32>,
    _pad1: u32,
    bound_group_queue_max_size: u32,
    hash_map_size: u32,
    segment_size_in_chunks: u32,
    max_group_bound_dispatch: u32,
    chunk_offset: vec3<u32>,
    _pad2: u32,
    frame_index: u32,
    changed_chunk_count: u32,
    changed_block_count: u32,
    changed_voxel_count: u32,
};

// ─── Bindings ─────────────────────────────────────────────────────────────────
//
// `@group(0)` = `construction_bounds_world_layout` — chunks_rw + params
// (a minimal version of the §1.3 `construction_world_layout`; `boundsCalc`
// only needs chunks + the params uniform, NOT the blocks/voxels/hash buffers
// the W1 layout carries). Keeping `boundsCalc`'s `@group(0)` separate from
// `chunk_calc`'s 8-binding layout means: (a) the W3 prepare system does not
// need W1's hash buffers to exist, and (b) the bind-group construction is
// 2-binding instead of 8-binding (cheaper, lower-conflict at prepare time).
// W4 widened the chunk-pair from `R32Uint` to `(x,y)`. The W3 reads still
// take `.x`, and the AADF-expansion write at line 394 preserves `.y` (the
// entity-pointer channel) like the W2 shaders do.
// Web-WebGPU migration: chunks is now `array<vec2<u32>>` (was
// `texture_storage_3d<rg32uint, read_write>`).
@group(0) @binding(0)
var<storage, read_write> chunks: array<vec2<u32>>;
@group(0) @binding(1)
var<uniform> params: ConstructionParams;

// `@group(1)` = `construction_bounds_layout` — the W3 bound-queue family.
@group(1) @binding(0)
var<storage, read_write> bound_queue_info: array<BoundQueueInfo>;
@group(1) @binding(1)
var<storage, read_write> bound_group_queues: array<u32>;
@group(1) @binding(2)
var<storage, read_write> bound_group_masks: array<atomic<u32>>;
@group(1) @binding(3)
var<storage, read_write> bound_refined_info: array<u32>;

// `@group(2)` = `bound_dispatch_indirect_layout` — single-binding rw storage
// for the indirect-dispatch counter, mirrors the Phase-B Batch-4
// `sample_refine_dispatch_layout` split (`15-design-c.md` §1.3 + the
// wgpu STORAGE_READ_WRITE × INDIRECT exclusivity split).
@group(2) @binding(0)
var<storage, read_write> bound_dispatch_indirect: array<u32>;

// ─── Constants — `boundsCalc.fx:4-12` ─────────────────────────────────────────

const BLOCK_STATE_CHILD: u32 = 2u;
const BLOCK_STATE_UNIFORM_EMPTY: u32 = 0u;
const BLOCK_STATE_UNIFORM_FULL: u32 = 1u;
const BOUND_INFO_GROUPS: u32 = 0u;

// `MASK_MX..MASK_PZ` from `boundsCommon.fxh:6-11`. Same masks as
// `bounds_common.wgsl` — the back-pointer exclusion masks. Duplicated here
// because the WGSL composition surface in Bevy 0.19 is unpredictable;
// `bounds_common.wgsl` ships only the *2-bit* helpers (the AADF-encode
// family). The chunk-level `boundsCalc` family is **5-bit AADFs** and uses
// the same mask bits but the shift count is 5, not 2.
const MASK_MX: u32 = 0x3Du;
const MASK_PX: u32 = 0x3Eu;
const MASK_MY: u32 = 0x37u;
const MASK_PY: u32 = 0x3Bu;
const MASK_MZ: u32 = 0x1Fu;
const MASK_PZ: u32 = 0x2Fu;

// `groupshared bool anyBoundsIncrease = false;` — `boundsCalc.fx:34`.
// Diagnostic-only; the C# never reads it back outside the kernel.
var<workgroup> any_bounds_increase: atomic<u32>;

// `checkMatchingBounds` for 5-bit chunk AADFs. The `boundsCommon.fxh:15-26`
// function is generic over the field width via the `shiftCount` / `shiftMask`
// parameters; the C# `boundsCalc.fx:113` call passes `0, 5, 0x1F` (5-bit
// fields starting at bit 0). Inlined here with those constants baked in.
fn check_matching_bounds_5bit(neighbour: u32, cur_chunk: u32) -> u32 {
    var mask: u32 = 0u;
    let n0 = (neighbour >> 0u)  & 0x1Fu;
    let c0 = (cur_chunk >> 0u)  & 0x1Fu;
    if (n0 >= c0) { mask = mask | (1u << 0u); }
    let n1 = (neighbour >> 5u)  & 0x1Fu;
    let c1 = (cur_chunk >> 5u)  & 0x1Fu;
    if (n1 >= c1) { mask = mask | (1u << 1u); }
    let n2 = (neighbour >> 10u) & 0x1Fu;
    let c2 = (cur_chunk >> 10u) & 0x1Fu;
    if (n2 >= c2) { mask = mask | (1u << 2u); }
    let n3 = (neighbour >> 15u) & 0x1Fu;
    let c3 = (cur_chunk >> 15u) & 0x1Fu;
    if (n3 >= c3) { mask = mask | (1u << 3u); }
    let n4 = (neighbour >> 20u) & 0x1Fu;
    let c4 = (cur_chunk >> 20u) & 0x1Fu;
    if (n4 >= c4) { mask = mask | (1u << 4u); }
    let n5 = (neighbour >> 25u) & 0x1Fu;
    let c5 = (cur_chunk >> 25u) & 0x1Fu;
    if (n5 >= c5) { mask = mask | (1u << 5u); }
    return mask;
}

// `addBoundsGroup(chunkPos, directionOffset, mask, boundsLocation, curBound,
//                 inout curChunk)` — `boundsCalc.fx:95-116`.
//
// Forward-compat: `textureLoad(chunks, neighbour_chunk_pos).x` keeps W4's
// `R32Uint` → `Rg32Uint` flip a no-op for this shader (`15-design-c.md`
// §1.7).
//
// Returns the updated `cur_chunk` (WGSL forbids `inout` cleanly over
// arbitrary types; mirrors the W1 pattern in `bounds_common.wgsl`).
fn add_bounds_group(
    chunk_pos: vec3<i32>,
    direction_offset: vec3<i32>,
    mask: u32,
    bounds_location: u32,
    cur_bound: u32,
    cur_chunk_in: u32,
) -> u32 {
    var cur_chunk = cur_chunk_in;
    let neighbour_chunk_pos = chunk_pos + direction_offset;
    let cx = i32(params.size_in_chunks.x);
    let cy = i32(params.size_in_chunks.y);
    let cz = i32(params.size_in_chunks.z);
    let out_of_bounds =
        neighbour_chunk_pos.x < 0 ||
        neighbour_chunk_pos.y < 0 ||
        neighbour_chunk_pos.z < 0 ||
        neighbour_chunk_pos.x >= cx ||
        neighbour_chunk_pos.y >= cy ||
        neighbour_chunk_pos.z >= cz;
    if (out_of_bounds) {
        // `boundsCalc.fx:98-103` — out-of-bounds treats the world edge as
        // permissive: bump the bound if this is the queue's current bound
        // size on that side. (NAADF's chunk-world-edge inversion vs the
        // small-AADF wall-bound, documented in `16-impl-c-W6.md` assumption 2.)
        if (((cur_chunk >> bounds_location) & 0x1Fu) == cur_bound) {
            cur_chunk = cur_chunk + (1u << bounds_location);
        }
        return cur_chunk;
    }
    // Web-WebGPU migration: chunks is `array<vec2<u32>>`; we use `.x`.
    // After the out-of-bounds gate above, `neighbour_chunk_pos` is in
    // `[0, size_in_chunks)` per axis, safe to cast to `vec3<u32>`.
    let neighbour_pos_u = vec3<u32>(neighbour_chunk_pos);
    let neighbour_idx = neighbour_pos_u.x
        + neighbour_pos_u.y * params.size_in_chunks.x
        + neighbour_pos_u.z * params.size_in_chunks.x * params.size_in_chunks.y;
    let neighbour_x = chunks[neighbour_idx].x;
    // `(neighbour.x >> 30) == BLOCK_STATE_UNIFORM_EMPTY && neighbour.y == 0`
    // — the non-`#ifdef ENTITIES` path collapses `.y` to 0 (W4 owns the
    // widening; until then we have no entity counts and the test is just
    // "is the chunk uniform-empty?"). The current-bound match keeps us in
    // step with the queue's bound size for this axis side.
    let state = neighbour_x >> 30u;
    if (state != BLOCK_STATE_UNIFORM_EMPTY) { return cur_chunk; }
    if (((cur_chunk >> bounds_location) & 0x1Fu) != cur_bound) { return cur_chunk; }
    // Neighbour empty + we are at this bound size → check the 5 other
    // directions' bounds dominate ours, and if so grow by 1.
    if ((check_matching_bounds_5bit(neighbour_x, cur_chunk) & mask) == mask) {
        cur_chunk = cur_chunk + (1u << bounds_location);
    }
    return cur_chunk;
}

// ─── Entry point 1: add_initial_groups_to_bound_queue — fx:39-48 ──────────────

@compute @workgroup_size(64, 1, 1)
fn add_initial_groups_to_bound_queue(
    @builtin(global_invocation_id) global_id: vec3<u32>,
) {
    let group_index = global_id.x;
    if (group_index >= params.bound_group_queue_max_size) {
        return;
    }
    let gsx = params.group_size_in_groups.x;
    let gsy = params.group_size_in_groups.y;
    let gpx = group_index % gsx;
    let gpy = (group_index / gsx) % gsy;
    let gpz = group_index / (gsx * gsy);

    // Seed the per-axis mask with bit-0 set (the size-0 queue holds every
    // group). 3 atomic stores — one per axis.
    atomicStore(&bound_group_masks[group_index * 3u + 0u], 1u);
    atomicStore(&bound_group_masks[group_index * 3u + 1u], 1u);
    atomicStore(&bound_group_masks[group_index * 3u + 2u], 1u);

    // Pack `(gpx | gpy<<11 | gpz<<21)` and write it into each of the three
    // size-0 X / Y / Z queues at offset `group_index`.
    let packed = gpx | (gpy << 11u) | (gpz << 21u);
    let max_size = params.bound_group_queue_max_size;
    bound_group_queues[max_size * 0u + group_index] = packed; // X
    bound_group_queues[max_size * 1u + group_index] = packed; // Y
    bound_group_queues[max_size * 2u + group_index] = packed; // Z
}

// ─── Entry point 2: prepare_group_bounds — fx:51-93 ───────────────────────────

@compute @workgroup_size(1, 1, 1)
fn prepare_group_bounds() {
    // Find the first non-empty queue (lowest bound size with any non-empty
    // axis). `boundsCalc.fx:54-72` — break out as soon as we find one.
    var found: bool = false;
    var found_bound_size: u32 = 0u;
    var found_xyz: u32 = 0u;
    var found_start: u32 = 0u;
    var found_size: u32 = 0u;
    for (var i: u32 = 0u; i < 32u && !found; i = i + 1u) {
        for (var xyz: u32 = 0u; xyz < 3u; xyz = xyz + 1u) {
            let qi = BOUND_INFO_GROUPS + i * 3u + xyz;
            let start = bound_queue_info[qi].start;
            let size = atomicLoad(&bound_queue_info[qi].size);
            if (size > 0u) {
                found = true;
                found_bound_size = i;
                found_xyz = xyz;
                found_start = start;
                found_size = size;
                break;
            }
        }
    }

    var group_amount: u32 = 0u;
    if (found) {
        group_amount = min(params.max_group_bound_dispatch, found_size);
        bound_refined_info[0] = found_start % params.bound_group_queue_max_size;
        bound_refined_info[1] = group_amount;
        bound_refined_info[2] = found_bound_size | (found_xyz << 16u);

        // Update queue head for next frame: advance `start`, decrement `size`
        // by the slice we just claimed. `boundsCalc.fx:84-87`.
        let qi = BOUND_INFO_GROUPS + found_bound_size * 3u + found_xyz;
        bound_queue_info[qi].start =
            (found_start + group_amount) % params.bound_group_queue_max_size;
        // `atomicStore` on `size` to keep the field's declared atomic
        // discipline; we are the single writer this frame.
        atomicStore(&bound_queue_info[qi].size, found_size - group_amount);
    } else {
        bound_refined_info[1] = 0u;
    }

    // `boundsCalc.fx:92` — `boundGroupQueueDispatchCount.Store(0, max(1, n))`
    // The `max(1, …)` ensures the indirect dispatch always launches at least
    // one workgroup so the no-op work-item case still issues a (trivial)
    // dispatch — `compute_group_bounds` sees `count = 0` and bails internally.
    bound_dispatch_indirect[0] = max(1u, group_amount);
}

// ─── Entry point 3: compute_group_bounds — fx:118-193 ─────────────────────────

@compute @workgroup_size(4, 4, 4)
fn compute_group_bounds(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // Read queue info refined by `prepare_group_bounds`.
    let start = bound_refined_info[0];
    let count = bound_refined_info[1];
    let bound_info = bound_refined_info[2];
    let bound_size = bound_info & 0xFFFFu;
    let bound_xyz = bound_info >> 16u;

    let is_group_active = group_id.x < count;

    // Decode the packed group position from the queue.
    let max_size = params.bound_group_queue_max_size;
    let queue_base = (bound_size * 3u + bound_xyz) * max_size;
    let queue_slot = (start + group_id.x) % max_size;
    let group_position_comp = bound_group_queues[queue_base + queue_slot];
    let gp = vec3<u32>(
        group_position_comp & 0x7FFu,
        (group_position_comp >> 11u) & 0x3FFu,
        group_position_comp >> 21u,
    );
    let group_index =
        gp.x + gp.y * params.group_size_in_groups.x
             + gp.z * params.group_size_in_groups.x * params.group_size_in_groups.y;

    // `boundsCalc.fx:134-135` — clear the mask bit for the *current* bound
    // size on the queue's axis (we are processing it now; the re-enqueue at
    // the bottom of this kernel sets the next bound size's bit).
    if (is_group_active && local_index == 0u) {
        let mask_idx = group_index * 3u + bound_xyz;
        atomicAnd(&bound_group_masks[mask_idx], ~(1u << bound_size));
    }

    // Per-chunk position inside the 4³ group.
    let chunk_pos = vec3<i32>(
        i32(gp.x * 4u + local_id.x),
        i32(gp.y * 4u + local_id.y),
        i32(gp.z * 4u + local_id.z),
    );
    // Web-WebGPU migration: chunks is `array<vec2<u32>>`. `chunk_pos` is
    // `vec3<i32>` constructed from `gp * 4 + local_id` (both non-negative);
    // safe to cast for flatten.
    let chunk_pos_u = vec3<u32>(chunk_pos);
    let chunk_idx = chunk_pos_u.x
        + chunk_pos_u.y * params.size_in_chunks.x
        + chunk_pos_u.z * params.size_in_chunks.x * params.size_in_chunks.y;
    let cur_chunk_full = chunks[chunk_idx];
    let cur_chunk_load = cur_chunk_full.x;
    // W4 — preserve `.y` (entity pointer channel) on the write below.
    let entity_y = cur_chunk_full.y;
    var cur_chunk = cur_chunk_load;
    let cur_chunk_copy = cur_chunk_load;
    let chunk_state = cur_chunk >> 30u;

    // `boundsCalc.fx:150-158` — only empty chunks expand. The `.y == 0`
    // entity-empty check collapses to "always true" on the non-`#ifdef
    // ENTITIES` path (W4 owns the `Rg32Uint` widening).
    if (chunk_state == BLOCK_STATE_UNIFORM_EMPTY) {
        var mask_minus: u32 = MASK_MX;
        var mask_plus: u32  = MASK_PX;
        if (bound_xyz == 1u) { mask_minus = MASK_MY; mask_plus = MASK_PY; }
        if (bound_xyz == 2u) { mask_minus = MASK_MZ; mask_plus = MASK_PZ; }
        var dir_abs: vec3<i32> = vec3<i32>(1, 0, 0);
        if (bound_xyz == 1u) { dir_abs = vec3<i32>(0, 1, 0); }
        if (bound_xyz == 2u) { dir_abs = vec3<i32>(0, 0, 1); }
        // -direction grow: location = boundXYZ * 10 + 0.
        cur_chunk = add_bounds_group(
            chunk_pos, -dir_abs, mask_minus,
            bound_xyz * 10u + 0u, bound_size, cur_chunk);
        // +direction grow: location = boundXYZ * 10 + 5.
        cur_chunk = add_bounds_group(
            chunk_pos, dir_abs, mask_plus,
            bound_xyz * 10u + 5u, bound_size, cur_chunk);
    }

    workgroupBarrier();

    // `boundsCalc.fx:162-170` — write back if changed, set the diagnostic
    // `any_bounds_increase` flag (atomic-store path, not actually consumed).
    // **Preserve `.y` (entity-pointer channel)** — W4 contract; without this
    // the W3 background queue would silently zero the entity pointer on
    // every AADF expansion.
    if (is_group_active && cur_chunk_copy != cur_chunk) {
        // Web-WebGPU migration: write to the flat chunks buffer. `chunk_idx`
        // was computed alongside the load above (same chunk-pos).
        chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y);
        atomicStore(&any_bounds_increase, 1u);
    }

    workgroupBarrier();

    // `boundsCalc.fx:174-192` — re-enqueue this group into the next bound
    // size's queue (only thread 0, and only if there is a next size below
    // the 5-bit AADF cap of 31).
    if (local_index == 0u && bound_size < 30u && is_group_active) {
        let next_bound_size = bound_size + 1u;
        let mask_idx = group_index * 3u + bound_xyz;
        // Set the next-bound mask bit; if it was already set, skip the
        // re-enqueue (we are already in that queue).
        let prev_mask = atomicOr(&bound_group_masks[mask_idx], 1u << next_bound_size);
        let already_in_queue = ((prev_mask >> next_bound_size) & 1u) != 0u;
        if (!already_in_queue) {
            let qi = BOUND_INFO_GROUPS + next_bound_size * 3u + bound_xyz;
            // Atomic-add the queue size; we get the prior `originalQueueSize`.
            let original_size = atomicAdd(&bound_queue_info[qi].size, 1u);
            // Read the start cursor (unchanged this frame; only `prepare`
            // updates it).
            let queue_start_index = bound_queue_info[qi].start;
            let next_max = params.bound_group_queue_max_size;
            let next_base = (next_bound_size * 3u + bound_xyz) * next_max;
            let next_slot = (queue_start_index + original_size) % next_max;
            bound_group_queues[next_base + next_slot] = group_position_comp;
        }
    }
}
