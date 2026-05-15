// Phase-C W1 — `chunkCalc.fx` ported to WGSL (`15-design-c.md` §4.1).
//
// Faithful port of NAADF's `Content/shaders/world/data/chunkCalc.fx` (265 lines)
// — the GPU side of paper Algorithm 1 (§3.2). Four entry points:
//
//   1. `calc_block_from_raw_data` (`numthreads(4,4,4)`) — Algorithm 1: per-block
//      open-addressing hash + dedup + atomic insert. Writes `chunks`,
//      `blocks`, `voxels`, mutates `hash_map` + `block_voxel_count`.
//   2. `compute_voxel_bounds` (`numthreads(64,1,1)`) — groupshared 4³ voxel-AADF
//      sweep (`ComputeBounds4` 3-iteration loop, 2-bit AADFs).
//   3. `compute_block_bounds` (`numthreads(64,1,1)`) — same loop over a 4³ block
//      layer (2-bit AADFs).
//   4. `chunk_copy_to_cpu` (`numthreads(64,1,1)`) — GPU→CPU sync stub.
//      Shipped by W1, dispatched by W4 (entity-enabled only).
//
// MonoGame → wgpu deviations (documented per `15-design-c.md` §1.5 / §1.6):
//
// - HLSL `InterlockedCompareExchange(target, expect, value, original_out)`
//   becomes WGSL `atomicCompareExchangeWeak(target, expect, value)` returning
//   `{ old_value: u32, exchanged: bool }`. The HLSL writes the *prior* value
//   into the `original_out` arg; the WGSL port reads `.old_value`. The HLSL
//   convention is "original_out == expect ⇒ CAS succeeded"; the WGSL exposes
//   `.exchanged` directly, so we use that.
// - HLSL `InterlockedOr(target, 0, out)` (the pending-pointer busy-wait at
//   `chunkCalc.fx:88-92`) becomes WGSL `atomicLoad(&target)`. The `Or 0` was
//   a HLSL idiom for "read with sequential consistency"; `atomicLoad` is the
//   direct semantic equivalent.
// - HLSL `RWStructuredBuffer<HashValue> hashMap` with per-field atomic access
//   is split: the WGSL `HashValueSlot` declares `voxel_pointer: atomic<u32>` +
//   `use_count: atomic<u32>` + `hash_raw: u32`. Per `15-design-c.md` §5.2 +
//   `01-context.md` "vec3-then-scalar WGSL hazard": only the CAS target +
//   counter are atomic; `hash_raw` is written non-atomically AFTER the slot is
//   claimed (single-writer at claim time → safe).
// - HLSL `chunks[chunkPos] = uint2(state, 0);` under `#ifdef ENTITIES` is
//   omitted — W4 owns the `R32Uint` → `Rg32Uint` widening (`15-design-c.md`
//   §1.7); W1 writes the single-channel `state` directly via
//   `textureStore(chunks, chunkPos, vec4<u32>(state, 0u, 0u, 0u))`.
//
// Shared imports (the `bounds_common.wgsl` helper functions and the
// `cached_cell` workgroup-shared array) are inlined here because Bevy's WGSL
// composition `#import` surface is unpredictable across naga versions; the
// helpers are duplicated identically in `world_change.wgsl` when W2 lands.
// `bounds_common.wgsl` ships as the canonical reference + the W3
// `bounds_calc.wgsl` future reuse site (§4.2). Edits to the algorithm MUST
// land in all copies — the test `bounds_common_inline_matches_ref` (in
// `render::construction::shader_drift_guard`) pins this.

// ─── Bindings ─────────────────────────────────────────────────────────────────
//
// `@group(0)` = `construction_world_layout` per `15-design-c.md` §1.3:
//   0: chunks_rw       — `texture_storage_3d<r32uint, read_write>`
//   1: blocks_rw       — `array<u32>` (rw storage)
//   2: voxels_rw       — `array<u32>` (rw storage)
//   3: block_voxel_count_rw — `array<atomic<u32>>` (2 elements: [0]=voxels
//                              cursor, [1]=blocks cursor)
//   4: segment_voxel_buffer — `array<u32>` (ro storage)
//   5: hash_map_rw     — `array<HashValueSlot>` (rw storage, atomic per-field)
//   6: params          — `ConstructionParams` uniform

struct HashValueSlot {
    voxel_pointer: atomic<u32>,
    use_count: atomic<u32>,
    hash_raw: u32,
    // No `vec3`-then-scalar hazard: 3 × u32, total 12 B, no following vec.
    // WGSL `array<HashValueSlot>` strides this to 16 B; the Rust mirror is
    // 16 B too (see `gpu_types::GpuHashValueSlot`).
    _pad: u32,
};

struct ConstructionParams {
    // Row 0 (offset 0): size_in_chunks (vec3) + pad to 16.
    size_in_chunks: vec3<u32>,
    _pad0: u32,
    // Row 1 (offset 16): group_size_in_groups (vec3) + pad to 16.
    group_size_in_groups: vec3<u32>,
    _pad1: u32,
    // Row 2 (offset 32): 4 × u32.
    bound_group_queue_max_size: u32,
    hash_map_size: u32,
    segment_size_in_chunks: u32,
    max_group_bound_dispatch: u32,
    // Row 3 (offset 48): chunk_offset (vec3) + pad to 16.
    chunk_offset: vec3<u32>,
    _pad2: u32,
    // Row 4 (offset 64): 4 × u32.
    frame_index: u32,
    changed_chunk_count: u32,
    changed_block_count: u32,
    changed_voxel_count: u32,
};

// `chunkCalc.fx:13-20` HashValue struct.
// W4 (`15-design-c.md` §1.7) — chunks texture format widened to `rg32uint`.
// The chunk_calc write site stores into `.x` only (`.y` stays 0 here; the
// entity pointer in `.y` is owned by `entity_update.wgsl`).
@group(0) @binding(0)
var chunks: texture_storage_3d<rg32uint, read_write>;
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

// `chunkCalc.fx:40` — the 65-entry hash-coefficient table (`31^(64-i) mod 2^32`).
// Declared as a separate storage buffer because WGSL uniforms can't be 65
// arbitrary u32s (uniform stride is 16 B per element for arrays). A ro storage
// is the idiomatic mirror — same access semantics as the C# `uint hashCoefficients[65]`
// effect parameter (single per-frame write, many reads). The CPU side
// (`hashing::hash_coefficients`) populates it identically to
// `BlockHashingHandler.cs:50-55`.
@group(0) @binding(7)
var<storage, read> hash_coefficients: array<u32>;

// ─── boundsCommon.fxh helpers (inlined per file header note) ──────────────────

const MASK_MX: u32 = 0x3Du;
const MASK_PX: u32 = 0x3Eu;
const MASK_MY: u32 = 0x37u;
const MASK_PY: u32 = 0x3Bu;
const MASK_MZ: u32 = 0x1Fu;
const MASK_PZ: u32 = 0x2Fu;

// `groupshared uint cachedCell[64]` (`boundsCommon.fxh:13`).
var<workgroup> cached_cell: array<u32, 64>;

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

// ─── chunkCalc.fx defines + groupshared state ─────────────────────────────────

const EMPTY_BLOCK: u32 = 0x0u;
const BLOCK_STATE_CHILD: u32 = 2u;
const BLOCK_STATE_UNIFORM_EMPTY: u32 = 0u;
const BLOCK_STATE_UNIFORM_FULL: u32 = 1u;
const PROBE_CAP: u32 = 250u;        // `chunkCalc.fx:62`
const PENDING_BIT: u32 = 0x80000000u; // `chunkCalc.fx:67`
const PENDING_WAIT_CAP: u32 = 2000u;  // `chunkCalc.fx:89`

// `groupshared uint referenceBlock` / `bool isAllBlocksEqual` /
// `uint insertBlockIndex = 0;` (`chunkCalc.fx:49-51`). WGSL `var<workgroup>`
// counterparts; init for `is_all_blocks_equal` happens at thread-0 store.
var<workgroup> reference_block: u32;
// `bool` is non-host-shareable in storage; use a u32 here (0/1) to model the
// HLSL `bool` cleanly under workgroup storage rules.
var<workgroup> is_all_blocks_equal: atomic<u32>;
var<workgroup> insert_block_index: atomic<u32>;

// ─── GetVoxelPointer — `chunkCalc.fx:57-115` ──────────────────────────────────
//
// Open-addressing hash insert with the `0x80000000`-tagged pending-pointer
// busy-wait. Returns the voxel pointer assigned to this group of 32 packed
// voxels (= 64 voxels) — either an existing dedup-hit pointer or a freshly
// claimed slot.
//
// The 250-probe cap matches NAADF's `ConstructionConfig.probe_cap` default.
// Probe-cap exhaustion returns `2u` (`chunkCalc.fx:114`) — a sentinel the
// caller never expects in practice (the world fits comfortably under the
// hash-map's `wanted_empty_ratio` even at the test grid scale).
fn get_voxel_pointer(hash: u32, voxel_raw_start: u32) -> u32 {
    var hash_bounds: u32 = hash & (params.hash_map_size - 1u);
    var count: u32 = 0u;
    loop {
        if (count >= PROBE_CAP) { break; }

        // CAS: try to claim the slot by writing PENDING | voxel_raw_start over
        // EMPTY_BLOCK. The C# `InterlockedCompareExchange(...,
        // 0x80000000 | voxelRawStart, originalPointer)` becomes
        // `atomicCompareExchangeWeak(...)` returning `{old_value, exchanged}`.
        let cas_result = atomicCompareExchangeWeak(
            &hash_map[hash_bounds].voxel_pointer,
            EMPTY_BLOCK,
            PENDING_BIT | voxel_raw_start,
        );
        let original_pointer = cas_result.old_value;

        var voxel_pointer: u32 = 0u;

        if (original_pointer == EMPTY_BLOCK) {
            // We claimed an empty slot. Reserve 64 voxels (= 32 u32 pairs) in
            // the global voxel buffer via the cursor in `block_voxel_count[0]`.
            atomicAdd(&hash_map[hash_bounds].use_count, 1u);
            let original_index = atomicAdd(&block_voxel_count[0], 64u);
            // The HLSL stores `originalIndex /= 2;` then writes
            // `voxels[originalIndex + i] = segmentVoxelBuffer[voxelRawStart + i]`
            // for i in 0..32 — converting from voxel-count units to packed-u32
            // units. The packed buffer holds 2 voxels per u32, so dividing the
            // voxel-count cursor by 2 yields the u32 index.
            let voxel_u32_start = original_index >> 1u;
            for (var i: u32 = 0u; i < 32u; i = i + 1u) {
                voxels[voxel_u32_start + i] =
                    segment_voxel_buffer[voxel_raw_start + i];
            }
            // Plain (non-atomic) write to `hash_raw` is safe — we hold the
            // slot via the PENDING tag; no other thread can have written it
            // yet (other contenders are spinning in the else-branch's busy-wait).
            hash_map[hash_bounds].hash_raw = hash;
            // Atomically replace the PENDING tag with the final pointer.
            atomicStore(&hash_map[hash_bounds].voxel_pointer, voxel_u32_start);
            voxel_pointer = voxel_u32_start;
        } else {
            // The slot is claimed by another thread (or fully written). Spin
            // until the PENDING tag clears (the claimer writes the final
            // pointer). Capped at 2000 iterations per `chunkCalc.fx:89`.
            var voxel_pointer_cur: u32 = atomicLoad(&hash_map[hash_bounds].voxel_pointer);
            var c: u32 = 0u;
            loop {
                if ((voxel_pointer_cur & PENDING_BIT) == 0u) { break; }
                c = c + 1u;
                if (c >= PENDING_WAIT_CAP) { break; }
                voxel_pointer_cur = atomicLoad(&hash_map[hash_bounds].voxel_pointer);
            }

            // Now check if this slot's hash matches ours + the contents agree.
            // `hash_raw` is plain `u32` — single-writer at claim, so a non-
            // atomic read is sound.
            if (hash_map[hash_bounds].hash_raw == hash) {
                var is_all_equal: bool = true;
                for (var i: u32 = 0u; i < 32u; i = i + 1u) {
                    if (segment_voxel_buffer[voxel_raw_start + i] !=
                        voxels[voxel_pointer_cur + i]) {
                        is_all_equal = false;
                    }
                }
                if (is_all_equal) {
                    atomicAdd(&hash_map[hash_bounds].use_count, 1u);
                    voxel_pointer = voxel_pointer_cur;
                }
            }
        }

        if (voxel_pointer > 0u) { return voxel_pointer; }
        hash_bounds = (hash_bounds + 1u) & (params.hash_map_size - 1u);
        count = count + 1u;
    }
    // Probe exhaustion sentinel (`chunkCalc.fx:114`).
    return 2u;
}

// ─── Entry point 1: calc_block_from_raw_data — `chunkCalc.fx:117-181` ─────────

@compute @workgroup_size(4, 4, 4)
fn calc_block_from_raw_data(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
    @builtin(global_invocation_id) global_id: vec3<u32>,
) {
    let seg = params.segment_size_in_chunks;
    let chunk_index_in_segment =
        group_id.x + group_id.y * seg + group_id.z * seg * seg;
    let voxel_index_in_segment = chunk_index_in_segment * 2048u + local_index * 32u;

    let chunk_pos = group_id + params.chunk_offset;

    // Hash + all-same check (`chunkCalc.fx:126-136`).
    var hash: u32 = hash_coefficients[0];
    let first_voxel_type_comp = segment_voxel_buffer[voxel_index_in_segment];
    let first_voxel_type = first_voxel_type_comp & 0x7FFFu;
    var is_all_same: bool =
        (first_voxel_type_comp & 0xFFFFu) == (first_voxel_type_comp >> 16u);
    for (var i: u32 = 0u; i < 32u; i = i + 1u) {
        let voxel_comp = segment_voxel_buffer[voxel_index_in_segment + i];
        hash = hash + hash_coefficients[i * 2u + 1u] * (voxel_comp & 0x7FFFu);
        hash = hash + hash_coefficients[i * 2u + 2u] * ((voxel_comp >> 16u) & 0x7FFFu);
        if (first_voxel_type_comp != voxel_comp) {
            is_all_same = false;
        }
    }

    // Per-block classification: uniform (full/empty) or mixed (hash-deduped).
    var block: u32;
    if (is_all_same) {
        let state = select(
            BLOCK_STATE_UNIFORM_FULL,
            BLOCK_STATE_UNIFORM_EMPTY,
            first_voxel_type == 0u,
        );
        block = first_voxel_type | (state << 30u);
    } else {
        block = get_voxel_pointer(hash, voxel_index_in_segment) | (BLOCK_STATE_CHILD << 30u);
    }

    workgroupBarrier();

    if (local_index == 0u) {
        reference_block = block;
        atomicStore(&is_all_blocks_equal, 1u); // assume equal
    }

    workgroupBarrier();

    // `chunkCalc.fx:154-155` — divergence sentinel.
    if (block != reference_block || !is_all_same) {
        atomicStore(&is_all_blocks_equal, 0u);
    }

    workgroupBarrier();

    if (local_index == 0u) {
        var state: u32 = 0u;
        if (atomicLoad(&is_all_blocks_equal) != 0u) {
            let s = select(
                BLOCK_STATE_UNIFORM_FULL,
                BLOCK_STATE_UNIFORM_EMPTY,
                first_voxel_type == 0u,
            );
            state = first_voxel_type | (s << 30u);
        } else {
            let new_base = atomicAdd(&block_voxel_count[1], 64u);
            atomicStore(&insert_block_index, new_base);
            state = new_base | (BLOCK_STATE_CHILD << 30u);
        }
        // R32Uint single-channel write — no `#ifdef ENTITIES` (`15-design-c.md`
        // §1.7; W4 owns the format flip).
        textureStore(chunks, vec3<i32>(chunk_pos), vec4<u32>(state, 0u, 0u, 0u));
    }

    workgroupBarrier();

    if (atomicLoad(&is_all_blocks_equal) == 0u) {
        let base = atomicLoad(&insert_block_index);
        blocks[base + local_index] = block;
    }
}

// ─── Entry point 2: compute_voxel_bounds — `chunkCalc.fx:193-217` ─────────────

@compute @workgroup_size(64, 1, 1)
fn compute_voxel_bounds(
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let block_index = group_id.x;
    let voxel_index = block_index * 64u + local_index;

    let cur_voxel_pair = voxels[voxel_index / 2u];
    let cur_voxel: u32 = select(
        (cur_voxel_pair >> 16u),
        (cur_voxel_pair & 0xFFFFu),
        voxel_index % 2u == 0u,
    );
    let orig_voxel = cur_voxel;
    let state = cur_voxel >> 15u;

    let voxel_pos_in_block = vec3<i32>(
        i32(local_index % 4u),
        i32((local_index / 4u) % 4u),
        i32((local_index / 16u) % 4u),
    );

    cached_cell[local_index] = cur_voxel;
    let updated = compute_bounds_4(local_index, voxel_pos_in_block, 15u, 0x1u, cur_voxel);
    cached_cell[local_index] = updated;

    // `chunkCalc.fx:210-211` — preserve full-voxel encoding (the state bit set
    // means "full"; the AADF write doesn't apply). The C# does this *after* the
    // `ComputeBounds4` loop, restoring `cachedCell[localIndex] = origVoxel`.
    if (state == 1u) {
        cached_cell[local_index] = orig_voxel;
    }

    workgroupBarrier();

    // Pack two voxels per u32 (`chunkCalc.fx:215-216`). Only even threads write.
    if (local_index % 2u == 0u) {
        let lo = cached_cell[local_index];
        let hi = cached_cell[local_index + 1u];
        voxels[voxel_index / 2u] = lo | (hi << 16u);
    }
}

// ─── Entry point 3: compute_block_bounds — `chunkCalc.fx:219-241` ─────────────

@compute @workgroup_size(64, 1, 1)
fn compute_block_bounds(
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let chunk_index = group_id.x;
    let block_index = chunk_index * 64u + local_index;

    let cur_block = blocks[block_index];
    let orig_block = cur_block;
    let state = cur_block >> 30u;

    let block_pos_in_chunk = vec3<i32>(
        i32(local_index % 4u),
        i32((local_index / 4u) % 4u),
        i32((local_index / 16u) % 4u),
    );

    cached_cell[local_index] = cur_block;
    let updated = compute_bounds_4(local_index, block_pos_in_chunk, 30u, 0x3u, cur_block);
    cached_cell[local_index] = updated;

    if (state != 0u) {
        cached_cell[local_index] = orig_block;
    }

    workgroupBarrier();

    blocks[block_index] = cached_cell[local_index];
}

// ─── Entry point 4: chunk_copy_to_cpu — DEFERRED to W4 ────────────────────────
//
// Per `15-design-c.md` §4.1 the design specifies W1 "ship the WGSL but DO NOT
// dispatch (W4 will)". The C# `chunkCopyToCpu` entry point
// (`chunkCalc.fx:183-191`) needs two additional bindings the production
// `construction_world_layout` does NOT carry (a `gpu_cpu_sync_buffer` rw
// storage + a small uniform with the `copyOffset`/`copyMaxCount`/`sizeInChunks
// X/Y` scalars). Adding those bindings to `construction_world_layout` would
// force every Phase-C compute pass (W1, W2, W3) to declare them even though
// only `chunk_copy_to_cpu` uses them.
//
// The clean seam: W4 ships `chunk_copy_to_cpu` either as a separate WGSL file
// or as an extended layout in its own merge — the same way W4 ships the
// `Rg32Uint` chunk-format widening. W1 documents the deferral here.
//
// The CPU equivalent of the GPU→CPU sync is already available without this
// shader: `WorldData.chunks_cpu` is the CPU mirror, populated by the CPU
// `aadf::construct::construct` path; W4 only needs `chunkCopyToCpu` when the
// production producer flips to GPU (regime-1 startup is the GPU-only path).
