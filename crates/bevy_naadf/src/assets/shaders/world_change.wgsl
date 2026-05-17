// Phase-C W2 — `worldChange.fx` ported to WGSL (`15-design-c.md` §4.3,
// `16-impl-c-W2.md`). Faithful port of NAADF's
// `Content/shaders/world/data/worldChange.fx` (191 lines): the regime-3
// on-edit-event chunk/block/voxel apply passes + the 4³-group AADF reset +
// bound-queue re-enqueue.
//
// Four entry points:
//   1. `apply_group_change` (`numthreads(4,4,4)`) — per-chunk-in-4³-group:
//      reset the chunk's 5-bit AADF along the flood-fill direction; thread-0
//      groupshared atomicMin reduces the lowest bounds per axis; threads 0/1/2
//      re-enqueue the group into the right size of the bound queue. Mirrors
//      `worldChange.fx:37-113`.
//   2. `apply_chunk_change` (`numthreads(64,1,1)`) — apply a CPU-staged
//      chunk-cell edit (`changedChunks` buffer): write
//      `chunks[chunkPos] = vec2<u32>(change.y, chunks[chunkPos].y)`. The `.y`
//      preservation is **load-bearing**: W4 widened chunks to `Rg32Uint` with
//      the entity-pointer in `.y`; a single-channel write would zero out the
//      entity pointer at the edited chunk. Mirrors `worldChange.fx:115-128`
//      ENTITIES branch.
//   3. `apply_block_change` (`numthreads(4,4,4)`) — apply a CPU-staged 64-block
//      edit: write 64 `blocks[insert_block_index + local_index]`, recompute the
//      local 4³ AADF via `compute_bounds_4`. Mirrors `worldChange.fx:130-147`.
//   4. `apply_voxel_change` (`numthreads(4,4,4)`) — apply a CPU-staged 64-voxel
//      edit: write 32 packed-voxel `uint`s into `voxels[]`, recompute the
//      local 4³ AADF. Mirrors `worldChange.fx:149-168`.
//
// MonoGame → wgpu deviations (documented per `15-design-c.md` §1.5, §1.6):
//
// - HLSL `groupshared uint lowestBoundsShared[3] = { 31, 31, 31 };`
//   (`worldChange.fx:35`) → WGSL `var<workgroup> lowest_bounds_shared:
//   array<atomic<u32>, 3>`. WGSL forbids constant initialisers on
//   `var<workgroup>`; thread-0 seeds the 3 slots before the first barrier.
// - HLSL `InterlockedMin(lowestBoundsShared[i], …)` (`worldChange.fx:86-88`) →
//   WGSL `atomicMin(&lowest_bounds_shared[i], …)`. Direct equivalent.
// - HLSL `chunks[chunkPos] = uint2(state, entity_y);` under `#ifdef ENTITIES`
//   → WGSL `textureStore(chunks, pos, vec4<u32>(new_state, existing_y, 0u, 0u))`.
//   **The `.y` channel is preserved on every chunk write here**; this is the
//   load-bearing W2 contract per the W2 brief.
// - HLSL `InterlockedAdd(boundQueueInfo[...].size, 1, original)` →
//   WGSL `atomicAdd(&bound_queue_info[idx].size, 1u)`. The `BoundQueueInfo.size`
//   field is declared `atomic<u32>` (W3 contract — `bounds_calc.wgsl`).
// - HLSL `groupshared uint cachedCell[64];` (`boundsCommon.fxh:13`) is inlined
//   here under the same `cached_cell` name, identical layout to
//   `chunk_calc.wgsl` / `bounds_common.wgsl`. The inline duplication matches W1's
//   pattern (`16-impl-c-W1.md` decision #6 — Bevy 0.19's WGSL composition
//   surface is unpredictable across naga versions).
//
// `boundsCommon.fxh` constants + helpers used here:
//   - `MASK_MX..MASK_PZ` — direction-exclusion masks (the 5 other-axis bits
//     that must dominate to grow). Same bits as `chunk_calc.wgsl` /
//     `bounds_calc.wgsl`.
//   - `compute_bounds_4` — the synchronised-iteration 2-bit neighbour-merge
//     loop over a `cached_cell[64]` groupshared array; identical to
//     `chunk_calc.wgsl`'s copy. Used by `apply_block_change` /
//     `apply_voxel_change` to recompute the edited block/chunk's local 4³
//     AADF after the cell writes.

// ─── Struct ports ─────────────────────────────────────────────────────────────

// `boundsCalc.fx:13-20` `BoundQueueInfo` — shared with W3 (the
// `construction_bounds_layout` `@group(2)` here is the same layout W3 owns).
struct BoundQueueInfo {
    start: u32,
    size: atomic<u32>,
};

// Construction-side params uniform (shared with W1's `chunk_calc.wgsl` and
// W3's `bounds_calc.wgsl`; `15-design-c.md` §1.8, §5.1 — `GpuConstructionParams`).
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
// `@group(0)` = `construction_world_layout` (shared with `chunk_calc.wgsl` via
// the W1 `chunk_calc::construction_world_layout_descriptor()` — 8 bindings).
// W2 declares only what it consumes; the unused slots (segment_voxel_buffer,
// hash_map, hash_coefficients, block_voxel_count) are bound but never read.
//
//   0: chunks_rw            — `array<vec2<u32>>` (W4-widened pair, storage rw;
//                              web-WebGPU migration replaced the previous
//                              `texture_storage_3d<rg32uint, read_write>`)
//   1: blocks_rw            — `array<u32>` (rw storage)
//   2: voxels_rw            — `array<u32>` (rw storage)
//   3: block_voxel_count_rw — `array<atomic<u32>>` (unused by W2 but layout-shared)
//   4: segment_voxel_buffer — `array<u32>` (unused by W2 but layout-shared)
//   5: hash_map_rw          — `array<HashValueSlot>` (unused by W2 but layout-shared)
//   6: params               — `ConstructionParams` uniform
//   7: hash_coefficients    — `array<u32>` (unused by W2 but layout-shared)

struct HashValueSlot {
    voxel_pointer: atomic<u32>,
    use_count: atomic<u32>,
    hash_raw: u32,
    _pad: u32,
};

@group(0) @binding(0)
var<storage, read_write> chunks: array<vec2<u32>>;
@group(0) @binding(1)
var<storage, read_write> blocks: array<u32>;
@group(0) @binding(2)
var<storage, read_write> voxels: array<u32>;
@group(0) @binding(3)
var<storage, read_write> block_voxel_count: array<atomic<u32>>;
@group(0) @binding(4)
var<storage, read> segment_voxel_buffer: array<u32>;
@group(0) @binding(5)
var<storage, read_write> hash_map: array<HashValueSlot>;
@group(0) @binding(6)
var<uniform> params: ConstructionParams;
@group(0) @binding(7)
var<storage, read> hash_coefficients: array<u32>;

// `@group(1)` = `construction_change_layout` (W2-owned, 4 bindings) — the 4
// CPU-staged upload buffers consumed by the 4 apply passes.
@group(1) @binding(0)
var<storage, read> changed_groups_dynamic: array<vec2<u32>>;
@group(1) @binding(1)
var<storage, read> changed_chunks_dynamic: array<vec2<u32>>;
@group(1) @binding(2)
var<storage, read> changed_blocks_dynamic: array<u32>;
@group(1) @binding(3)
var<storage, read> changed_voxels_dynamic: array<u32>;

// `@group(2)` = `construction_bounds_layout` — re-use of W3's
// `bound_queue_info` / `bound_group_queues` / `bound_group_masks` /
// `bound_refined_info`. Only `apply_group_change` consumes group (2); the other
// 3 entry points still bind it (the pipeline-vs-layout match is per-pipeline).
@group(2) @binding(0)
var<storage, read_write> bound_queue_info: array<BoundQueueInfo>;
@group(2) @binding(1)
var<storage, read_write> bound_group_queues: array<u32>;
@group(2) @binding(2)
var<storage, read_write> bound_group_masks: array<atomic<u32>>;
@group(2) @binding(3)
var<storage, read_write> bound_refined_info: array<u32>;

// ─── Constants ────────────────────────────────────────────────────────────────

const BLOCK_STATE_CHILD: u32 = 2u;
const BLOCK_STATE_UNIFORM_EMPTY: u32 = 0u;
const BLOCK_STATE_UNIFORM_FULL: u32 = 1u;

// Same masks as `chunk_calc.wgsl` / `bounds_calc.wgsl` — direction-exclusion
// masks for the 2-bit `check_matching_bounds` helper.
const MASK_MX: u32 = 0x3Du;
const MASK_PX: u32 = 0x3Eu;
const MASK_MY: u32 = 0x37u;
const MASK_PY: u32 = 0x3Bu;
const MASK_MZ: u32 = 0x1Fu;
const MASK_PZ: u32 = 0x2Fu;

// ─── boundsCommon.fxh helpers — inlined (same as `chunk_calc.wgsl`) ───────────

// `groupshared uint cachedCell[64]` — used by `apply_block_change` /
// `apply_voxel_change` for the `compute_bounds_4` neighbour-merge.
var<workgroup> cached_cell: array<u32, 64>;

// `groupshared uint lowestBoundsShared[3] = { 31, 31, 31 };` —
// `worldChange.fx:35`. WGSL doesn't allow constant initialisers on
// var<workgroup>; thread-0 of `apply_group_change` seeds the 3 slots before
// the first barrier (the seed value 31 == `0x1F` is the 5-bit AADF cap, "we
// haven't found a bound yet" sentinel).
var<workgroup> lowest_bounds_shared: array<atomic<u32>, 3>;

fn check_matching_bounds(
    neighbour: u32,
    cur_voxel: u32,
    shift_offset: u32,
    shift_count: u32,
    shift_mask: u32,
) -> u32 {
    var mask: u32 = 0u;
    let n0 = (neighbour >> (shift_offset + shift_count * 0u)) & shift_mask;
    let c0 = (cur_voxel  >> (shift_offset + shift_count * 0u)) & shift_mask;
    if (n0 >= c0) { mask = mask | (1u << 0u); }
    let n1 = (neighbour >> (shift_offset + shift_count * 1u)) & shift_mask;
    let c1 = (cur_voxel  >> (shift_offset + shift_count * 1u)) & shift_mask;
    if (n1 >= c1) { mask = mask | (1u << 1u); }
    let n2 = (neighbour >> (shift_offset + shift_count * 2u)) & shift_mask;
    let c2 = (cur_voxel  >> (shift_offset + shift_count * 2u)) & shift_mask;
    if (n2 >= c2) { mask = mask | (1u << 2u); }
    let n3 = (neighbour >> (shift_offset + shift_count * 3u)) & shift_mask;
    let c3 = (cur_voxel  >> (shift_offset + shift_count * 3u)) & shift_mask;
    if (n3 >= c3) { mask = mask | (1u << 3u); }
    let n4 = (neighbour >> (shift_offset + shift_count * 4u)) & shift_mask;
    let c4 = (cur_voxel  >> (shift_offset + shift_count * 4u)) & shift_mask;
    if (n4 >= c4) { mask = mask | (1u << 4u); }
    let n5 = (neighbour >> (shift_offset + shift_count * 5u)) & shift_mask;
    let c5 = (cur_voxel  >> (shift_offset + shift_count * 5u)) & shift_mask;
    if (n5 >= c5) { mask = mask | (1u << 5u); }
    return mask;
}

fn add_bounds_voxels_or_blocks(
    local_index: u32,
    mask: u32,
    direction_offset: i32,
    bounds_location: u32,
    state_location: u32,
    state_mask: u32,
    cur_cell: u32,
) -> u32 {
    let neighbour_idx = u32(i32(local_index) + direction_offset);
    let neighbour = cached_cell[neighbour_idx];
    var out = cur_cell;
    if (((neighbour >> state_location) & state_mask) == 0u) {
        if ((check_matching_bounds(neighbour, cur_cell, 0u, 2u, 0x3u) & mask) == mask) {
            out = cur_cell + (1u << bounds_location);
        }
    }
    return out;
}

fn compute_bounds_4(
    local_index: u32,
    pos_in_volume: vec3<i32>,
    state_location: u32,
    state_mask: u32,
    cur_cell_in: u32,
) -> u32 {
    var cur_cell = cur_cell_in;
    workgroupBarrier();
    for (var i: i32 = 0; i < 3; i = i + 1) {
        if (pos_in_volume.x > 0) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_MX, -1, 0u, state_location, state_mask, cur_cell);
        }
        if (pos_in_volume.x + 1 < 4) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_PX, 1, 2u, state_location, state_mask, cur_cell);
        }
        cached_cell[local_index] = cur_cell;
        workgroupBarrier();

        if (pos_in_volume.y > 0) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_MY, -4, 4u, state_location, state_mask, cur_cell);
        }
        if (pos_in_volume.y + 1 < 4) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_PY, 4, 6u, state_location, state_mask, cur_cell);
        }
        cached_cell[local_index] = cur_cell;
        workgroupBarrier();

        if (pos_in_volume.z > 0) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_MZ, -16, 8u, state_location, state_mask, cur_cell);
        }
        if (pos_in_volume.z + 1 < 4) {
            cur_cell = add_bounds_voxels_or_blocks(
                local_index, MASK_PZ, 16, 10u, state_location, state_mask, cur_cell);
        }
        cached_cell[local_index] = cur_cell;
        workgroupBarrier();
    }
    return cur_cell;
}

// ─── Entry point 1: apply_group_change — fx:37-113 ────────────────────────────
//
// Per-chunk-in-4³-group: reset the chunk's 5-bit AADF to the flood-fill
// distance, re-enqueue the group into the right size of `bound_queue_info` /
// `bound_group_queues`.
//
// The C# code uses `groupshared uint lowestBoundsShared[3] = { 31, 31, 31 };`
// — WGSL doesn't permit a constant initialiser on var<workgroup>; thread-0
// seeds the 3 slots before the first barrier.

@compute @workgroup_size(4, 4, 4)
fn apply_group_change(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // Seed groupshared `lowest_bounds_shared` to 31 (the 5-bit AADF cap).
    // Thread 0 owns the seed; the barrier below makes it visible to every
    // other thread.
    if (local_index == 0u) {
        atomicStore(&lowest_bounds_shared[0], 31u);
        atomicStore(&lowest_bounds_shared[1], 31u);
        atomicStore(&lowest_bounds_shared[2], 31u);
    }
    workgroupBarrier();

    let change = changed_groups_dynamic[group_id.x];
    let group_position = vec3<u32>(
        change.x & 0x7FFu,
        (change.x >> 11u) & 0x3FFu,
        change.x >> 21u,
    );
    let group_index =
        group_position.x
        + group_position.y * params.group_size_in_groups.x
        + group_position.z * params.group_size_in_groups.x * params.group_size_in_groups.y;
    let chunk_pos = vec3<u32>(
        group_position.x * 4u + local_id.x,
        group_position.y * 4u + local_id.y,
        group_position.z * 4u + local_id.z,
    );
    // `worldChange.fx:44` — `CHUNKTYPE curChunk = chunks[chunkPos];`. With W4's
    // chunks-pair, `.x` carries the construction state + AADF, `.y` carries
    // the entity pointer / counter. Web-WebGPU migration: chunks is
    // `array<vec2<u32>>` indexed by `flatten_index(chunk_pos, sx, sx*sy)`.
    let chunk_idx = chunk_pos.x
        + chunk_pos.y * params.size_in_chunks.x
        + chunk_pos.z * params.size_in_chunks.x * params.size_in_chunks.y;
    let cur_chunk_load = chunks[chunk_idx];
    let cur_chunk_x = cur_chunk_load.x;
    let cur_chunk_y = cur_chunk_load.y;

    let chunk_state = cur_chunk_x >> 30u;
    // `worldChange.fx:46` — `bool isResetCompletely = change.y >> 30;`. The
    // top 2 bits of `change.y` carry the reset-completely flag (set when this
    // group has been edited directly, not just touched by the flood-fill).
    let is_reset_completely = (change.y >> 30u) != 0u;

    var lowest_x: u32 = select(31u, 0u, is_reset_completely);
    var lowest_y: u32 = select(31u, 0u, is_reset_completely);
    var lowest_z: u32 = select(31u, 0u, is_reset_completely);

    if (chunk_state == BLOCK_STATE_UNIFORM_EMPTY) {
        let new_chunk_state = chunk_state; // bits 30-31 stay 0 for empty

        // `worldChange.fx:56-61` — the 6 per-direction change bounds, indexed
        // by the local-id within the 4³ group. The flood-fill distance in
        // `change.y` is added to the chunk's offset within the group along
        // each axis side (M/P), then the min over all 6 sides is taken.
        let change_bound_xm = (change.y & 0x1Fu)         + local_id.x;
        let change_bound_xp = ((change.y >> 5u) & 0x1Fu) + (3u - local_id.x);
        let change_bound_ym = ((change.y >> 10u) & 0x1Fu) + local_id.y;
        let change_bound_yp = ((change.y >> 15u) & 0x1Fu) + (3u - local_id.y);
        let change_bound_zm = ((change.y >> 20u) & 0x1Fu) + local_id.z;
        let change_bound_zp = ((change.y >> 25u) & 0x1Fu) + (3u - local_id.z);
        let change_all = min(
            min(min(change_bound_xm, change_bound_xp),
                min(change_bound_ym, change_bound_yp)),
            min(change_bound_zm, change_bound_zp),
        );

        // Take min of the current chunk's AADF on each axis with `change_all`.
        let new_bound_xm = min(cur_chunk_x         & 0x1Fu, change_all);
        let new_bound_xp = min((cur_chunk_x >> 5u) & 0x1Fu, change_all);
        let new_bound_ym = min((cur_chunk_x >> 10u) & 0x1Fu, change_all);
        let new_bound_yp = min((cur_chunk_x >> 15u) & 0x1Fu, change_all);
        let new_bound_zm = min((cur_chunk_x >> 20u) & 0x1Fu, change_all);
        let new_bound_zp = min((cur_chunk_x >> 25u) & 0x1Fu, change_all);

        var new_chunk_x = new_chunk_state;
        if (!is_reset_completely) {
            new_chunk_x = new_chunk_x
                | new_bound_xm
                | (new_bound_xp << 5u)
                | (new_bound_ym << 10u)
                | (new_bound_yp << 15u)
                | (new_bound_zm << 20u)
                | (new_bound_zp << 25u);
        }

        // Track the lowest bound across all 6 sides per axis for the queue
        // re-enqueue at the bottom. `worldChange.fx:73-75`.
        lowest_x = min(lowest_x, min(new_bound_xm, new_bound_xp));
        lowest_y = min(lowest_y, min(new_bound_ym, new_bound_yp));
        lowest_z = min(lowest_z, min(new_bound_zm, new_bound_zp));

        // **Preserve the `.y` entity-pointer channel** — W2 contract.
        chunks[chunk_idx] = vec2<u32>(new_chunk_x, cur_chunk_y);
    }

    workgroupBarrier();

    // `worldChange.fx:86-88` — atomicMin across all 64 threads of the group
    // for each axis. Every thread contributes its `lowest_*` value; the lowest
    // wins.
    atomicMin(&lowest_bounds_shared[0], lowest_x);
    atomicMin(&lowest_bounds_shared[1], lowest_y);
    atomicMin(&lowest_bounds_shared[2], lowest_z);

    workgroupBarrier();

    // `worldChange.fx:92-112` — threads 0, 1, 2 re-enqueue the group into the
    // next size of the bound queue along their respective axis.
    if (local_index < 3u) {
        let xyz = local_index;
        var next_bound_size = atomicLoad(&lowest_bounds_shared[xyz]);
        if (is_reset_completely) {
            next_bound_size = 0u;
        }
        // Check if the group is already in the target queue (mask bit set).
        // `worldChange.fx:99` — `boundGroupMasks[groupIndex][xyz] >> next ... & 0x1`.
        let mask_idx = group_index * 3u + xyz;
        let prev_mask = atomicLoad(&bound_group_masks[mask_idx]);
        let is_already_in_queue = ((prev_mask >> next_bound_size) & 0x1u) != 0u;
        if (!is_already_in_queue && next_bound_size < 31u) {
            // Set the bit (`atomicOr` so concurrent writers on the same group
            // for different axes don't race).
            atomicOr(&bound_group_masks[mask_idx], 1u << next_bound_size);
            // Atomic-add the queue size; we get the prior `originalQueueSize`.
            let qi = next_bound_size * 3u + xyz;
            let original_queue_size = atomicAdd(&bound_queue_info[qi].size, 1u);
            // Read the start cursor (unchanged this dispatch; only
            // `prepare_group_bounds` advances it).
            let queue_start_index = bound_queue_info[qi].start;
            let max_size = params.bound_group_queue_max_size;
            let queue_index =
                (next_bound_size * 3u + xyz) * max_size
                + ((queue_start_index + original_queue_size) % max_size);
            bound_group_queues[queue_index] = change.x;
        }
    }
}

// ─── Entry point 2: apply_chunk_change — fx:115-128 ───────────────────────────
//
// Apply a CPU-staged chunk-cell edit. `changedChunksDynamic` is a `vec2<u32>[]`
// of `(packed_chunk_pos, new_chunk_state)`. The chunk's `.x` channel (state +
// AADF) gets the new value; the `.y` channel (entity pointer / counter from W4)
// is **preserved**.

@compute @workgroup_size(64, 1, 1)
fn apply_chunk_change(
    @builtin(global_invocation_id) global_id: vec3<u32>,
) {
    if (global_id.x >= params.changed_chunk_count) {
        return;
    }
    let change = changed_chunks_dynamic[global_id.x];
    let chunk_pos = vec3<u32>(
        change.x & 0x7FFu,
        (change.x >> 11u) & 0x3FFu,
        change.x >> 21u,
    );
    // Web-WebGPU migration: chunks is `array<vec2<u32>>`. Load to preserve
    // `.y` (entity pointer channel) — W2 contract — then write the pair.
    let chunk_idx = chunk_pos.x
        + chunk_pos.y * params.size_in_chunks.x
        + chunk_pos.z * params.size_in_chunks.x * params.size_in_chunks.y;
    let cur = chunks[chunk_idx];
    chunks[chunk_idx] = vec2<u32>(change.y, cur.y);
}

// ─── Entry point 3: apply_block_change — fx:130-147 ───────────────────────────
//
// Apply a CPU-staged 64-block edit. Per dispatch group: 64 threads each load
// one block from the CPU-staged buffer, compute the 4³ AADF via the shared
// `compute_bounds_4` helper, write the 64 blocks back into `blocks[]` at the
// CPU-supplied `change_pointer`.
//
// `changedBlocksDynamic` layout — one edit batch = 65 u32s: `[pointer, 64 ×
// block_word]`. The pointer is the base offset into `blocks[]` where the 64
// new blocks go.

@compute @workgroup_size(4, 4, 4)
fn apply_block_change(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let edit_base = group_id.x * (64u + 1u);
    let change_pointer = changed_blocks_dynamic[edit_base];
    // Load the block this thread owns into both `cur_block` and `cached_cell`.
    var cur_block = changed_blocks_dynamic[edit_base + 1u + local_index];
    cached_cell[local_index] = cur_block;

    let block_pos_in_chunk = vec3<i32>(
        i32(local_index & 3u),
        i32((local_index >> 2u) & 3u),
        i32((local_index >> 4u) & 3u),
    );

    // 2-bit AADFs at shift offset 0, mask 0x3, state at shift 30 — the
    // BlockCell layout (`worldChange.fx:139`).
    cur_block = compute_bounds_4(local_index, block_pos_in_chunk, 30u, 0x3u, cur_block);

    // Non-empty blocks pass through unchanged (`worldChange.fx:141-142`).
    if ((cur_block >> 30u) != 0u) {
        cur_block = changed_blocks_dynamic[edit_base + 1u + local_index];
    }
    cached_cell[local_index] = cur_block;
    workgroupBarrier();

    blocks[change_pointer + local_index] = cached_cell[local_index];
}

// ─── Entry point 4: apply_voxel_change — fx:149-168 ───────────────────────────
//
// Apply a CPU-staged 64-voxel edit. Per dispatch group: 64 threads each
// compute one voxel's AADF (the voxel half-word is unpacked from the
// CPU-staged buffer's 32-u32 packed-voxel-pair layout); threads 0-31 then
// re-pack two voxels per u32 and write 32 `uint`s into `voxels[]`.
//
// `changedVoxelsDynamic` layout — one edit batch = 33 u32s: `[pointer, 32 ×
// packed_voxel_pair]`. The pointer is the base offset into `voxels[]` where
// the 32 new packed-voxel-pair u32s go.

@compute @workgroup_size(4, 4, 4)
fn apply_voxel_change(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let edit_base = group_id.x * (32u + 1u);
    let change_pointer = changed_voxels_dynamic[edit_base];

    // Unpack the voxel half-word this thread owns from the packed pair.
    // `worldChange.fx:154-155` — even `local_index` reads low 16 bits, odd
    // reads high 16 bits.
    let pair_u32 = changed_voxels_dynamic[edit_base + 1u + (local_index / 2u)];
    let is_high = (local_index & 1u) == 1u;
    var cur_voxel = select(pair_u32 & 0xFFFFu, pair_u32 >> 16u, is_high);
    cached_cell[local_index] = cur_voxel;

    let voxel_pos_in_block = vec3<i32>(
        i32(local_index & 3u),
        i32((local_index >> 2u) & 3u),
        i32((local_index >> 4u) & 3u),
    );

    // 2-bit AADFs at shift offset 0, mask 0x1 (the 1-bit state at bit 15;
    // here we extract via state_location=15, state_mask=1 — voxel layout
    // differs from block layout). `worldChange.fx:159`.
    cur_voxel = compute_bounds_4(local_index, voxel_pos_in_block, 15u, 0x1u, cur_voxel);

    // Non-empty voxels (`>> 15 != 0`) pass through unchanged.
    if ((cur_voxel >> 15u) != 0u) {
        let original_pair = changed_voxels_dynamic[edit_base + 1u + (local_index / 2u)];
        cur_voxel = select(original_pair & 0xFFFFu, original_pair >> 16u, is_high);
    }
    cached_cell[local_index] = cur_voxel;
    workgroupBarrier();

    // Threads 0-31 each re-pack a voxel pair and write one u32.
    // `worldChange.fx:166-167`.
    if (local_index < 32u) {
        let lo = cached_cell[local_index * 2u];
        let hi = cached_cell[local_index * 2u + 1u];
        voxels[change_pointer + local_index] = (lo & 0xFFFFu) | ((hi & 0xFFFFu) << 16u);
    }
}
