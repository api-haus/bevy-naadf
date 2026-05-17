// Phase-C W1 — `mapCopy.fx` ported to WGSL (`15-design-c.md` §4.4).
//
// Faithful port of NAADF's `Content/shaders/world/data/mapCopy.fx` (70 lines).
// Two entry points:
//
//   1. `copy_map` (`numthreads(64,1,1)`) — linear-probe re-hash every occupied
//      slot of `old_map` into the larger `new_map`. Used by the
//      `BlockHashingHandler.IncreaseSizeToNewCount` regrow path when occupancy
//      exceeds `wanted_empty_ratio * mapSize` (`BlockHashingHandler.cs:81-83`,
//      `:177-201`).
//   2. `test_hash` (`numthreads(1,1,1)`) — CPU-debugging hash sanity probe.
//      Not in the production startup path; ports the C# `testHash` pass for
//      parity with the reference shader.
//
// MonoGame → wgpu deviations:
//
// - HLSL `InterlockedCompareExchange(target, EMPTY_BLOCK, value, original)`
//   becomes WGSL `atomicCompareExchangeWeak(&target, EMPTY_BLOCK, value)`.
//   The C# tests `originalPointer == EMPTY_BLOCK` to detect CAS success; the
//   WGSL exposes `.exchanged` directly, simpler.
// - HLSL `StructuredBuffer<HashValue> oldMap` (read-only) + `RWStructuredBuffer
//   <HashValue> newMap` (read-write) → WGSL `var<storage, read>` +
//   `var<storage, read_write>`. The `HashValueSlot` definition matches
//   `chunk_calc.wgsl`'s exactly — same atomic discipline (only
//   `voxel_pointer` is the CAS target; `use_count` is atomicAdd-only;
//   `hash_raw` is non-atomic written after slot claim).
//
// Layout: a dedicated `map_copy_layout` `@group(0)` separate from
// `construction_world_layout` — the regrow runs once outside the regime-2 /
// regime-3 chain, against a fresh `new_map` buffer the CPU side allocates
// per growth; reusing the construction_world layout would force the world
// passes' bindings to be present even when they're not used by mapCopy.

struct HashValueSlot {
    voxel_pointer: atomic<u32>,
    use_count: atomic<u32>,
    hash_raw: u32,
    _pad: u32,
};

struct MapCopyParams {
    old_size: u32,
    new_size: u32,
    _pad0: u32,
    _pad1: u32,
};

// Web-WebGPU (`docs/orchestrate/web-chunks-storage-buffer/`) — `old_map` was
// originally declared `var<storage, read>`, but the WGSL spec forbids atomic
// types in a storage variable unless the access mode is `read_write`. Dawn
// (Chrome's WebGPU) enforces this strictly; naga (native wgpu) accepts the
// read-mode variant leniently. Promoting the access mode to `read_write` is
// semantically identical for our use (this kernel never writes `old_map`;
// the regrow holds the only handle, so there's no concurrent-writer hazard),
// and it keeps the WGSL spec-compliant on web. The bind-group-layout slot in
// `map_copy.rs::map_copy_layout_descriptor` is flipped to `storage_buffer_sized`
// to match (was `storage_buffer_read_only_sized`).
@group(0) @binding(0)
var<storage, read_write> old_map: array<HashValueSlot>;
@group(0) @binding(1)
var<storage, read_write> new_map: array<HashValueSlot>;
@group(0) @binding(2)
var<uniform> params: MapCopyParams;

// `test_hash` uses these — separate (single-binding-per-resource) bindings so
// neither `copy_map` nor any other construction pipeline needs them. They are
// optional in the bind group; `test_hash` is not used in the production path.
@group(0) @binding(3)
var<storage, read> hash_coefficients: array<u32>;
@group(0) @binding(4)
var<storage, read> voxels_to_hash: array<u32>;
@group(0) @binding(5)
var<storage, read_write> result_hash: array<u32>;

const EMPTY_BLOCK: u32 = 0x0u;
const PROBE_CAP_MAP_COPY: u32 = 50u; // `mapCopy.fx:32`

@compute @workgroup_size(64, 1, 1)
fn copy_map(
    @builtin(global_invocation_id) global_id: vec3<u32>,
) {
    let id = global_id.x;
    if (id >= params.old_size) { return; }

    // `mapCopy.fx:27` — `HashValue hashValue = oldMap[ID];`. `voxel_pointer` is
    // declared atomic for `chunk_calc.wgsl` compatibility, but here we read
    // through `atomicLoad`. `use_count` similarly.
    let vp = atomicLoad(&old_map[id].voxel_pointer);
    let uc = atomicLoad(&old_map[id].use_count);
    let hr = old_map[id].hash_raw;

    if (vp != EMPTY_BLOCK) {
        var new_hash_bound: u32 = hr & (params.new_size - 1u);
        var count: u32 = 0u;
        loop {
            count = count + 1u;
            if (count >= PROBE_CAP_MAP_COPY) { break; }

            let cas = atomicCompareExchangeWeak(
                &new_map[new_hash_bound].voxel_pointer,
                EMPTY_BLOCK,
                vp,
            );
            if (cas.old_value == EMPTY_BLOCK) { break; }

            new_hash_bound = (new_hash_bound + 1u) & (params.new_size - 1u);
        }
        // `mapCopy.fx:40-41` — single-writer (we just CAS'd the slot), so
        // plain non-atomic writes for `hash_raw` + `use_count`.
        new_map[new_hash_bound].hash_raw = hr;
        atomicStore(&new_map[new_hash_bound].use_count, uc);
    }
}

@compute @workgroup_size(1, 1, 1)
fn test_hash() {
    // `mapCopy.fx:46-55` — re-derive the 64-voxel hash on the CPU-staged
    // `voxelsToHash[32]` array. Single-thread, deterministic. Used by the
    // CPU-side hash sanity probe; not in production.
    var hash: u32 = hash_coefficients[0];
    for (var i: u32 = 0u; i < 32u; i = i + 1u) {
        let voxel_comp = voxels_to_hash[i];
        hash = hash + hash_coefficients[i * 2u + 1u] * (voxel_comp & 0x7FFFu);
        hash = hash + hash_coefficients[i * 2u + 2u] * ((voxel_comp >> 16u) & 0x7FFFu);
    }
    result_hash[0] = hash;
}
