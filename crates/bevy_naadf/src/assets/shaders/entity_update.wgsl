// Phase-C W4 — `entityUpdate.fx` ported to WGSL (`15-design-c.md` §4.6).
//
// Faithful port of NAADF's `Content/shaders/world/data/entityUpdate.fx` (60 lines).
// Three entry points, all `numthreads(64,1,1)`:
//
//   1. `update_chunks` — apply per-chunk entity-pointer updates. Reads the
//      packed `chunkUpdatesDynamic` upload buffer; writes the `.y` channel of
//      the `chunks` storage texture (the per-chunk entity pointer + counter
//      pair), preserving `.x` (the construction-side block-state pointer W1
//      writes).
//   2. `copy_entity_chunk_instances` — bulk copy the per-frame
//      `entityChunkInstancesDynamic` upload buffer into the GPU
//      `entityChunkInstances` buffer the renderer reads (`rayTracing.fxh:41`).
//   3. `copy_entity_history` — write one slot of the entity-history ring at
//      `taa_index * MAX_ENTITY_INSTANCES + entityInstanceID`. The
//      MAX_ENTITY_INSTANCES = 16384 cap is NAADF's `WorldRender.cs:88`; the
//      Rust mirror lives in [`crate::render::construction::config::DEFAULT_MAX_ENTITY_INSTANCES`].
//
// MonoGame → wgpu deviations:
// - HLSL `RWTexture3D<uint2> chunks` becomes WGSL
//   `array<vec2<u32>>` (the W4 chunks-format chunk-pair carries
//   `(state, entity_y)`). Web-WebGPU migration replaces the original
//   `texture_storage_3d<rg32uint, read_write>` with a storage buffer:
//   `Rg32Uint` `read_write` is not in the WebGPU spec allow-list. Reads come
//   back as `vec2<u32>(x, y)`; field-selector discipline is preserved.
// - HLSL `chunks[chunkPos] = uint2(chunks[chunkPos].x, update.y)` (line 23)
//   becomes WGSL `chunks[chunk_idx] = vec2<u32>(old.x, update.y)`, where
//   `chunk_idx = flatten_index(chunk_pos, size_in_chunks.x, sx*sy)`.
//
// Bindings (parallel to W1's `construction_world_layout`, only the chunks
// texture is needed from `@group(0)`; the entity-track buffers live on
// `@group(1)` = `construction_entity_layout`):
//
//   @group(0)
//     0: chunks_rw                       — `texture_storage_3d<rg32uint, read_write>`
//     6: params                          — uniform<EntityUpdateParams>
//
//   @group(1)
//     0: chunk_updates_dynamic           — ro storage<array<vec2<u32>>>
//     1: entity_chunk_instances_dynamic  — ro storage<array<EntityChunkInstance>>
//     2: entity_history_dynamic          — ro storage<array<vec4<u32>>>
//     3: entity_chunk_instances_rw       — rw storage<array<EntityChunkInstance>>
//     4: entity_instances_history_rw     — rw storage<array<vec4<u32>>>

// `EntityChunkInstance` mirror — 20 B, 5 × u32. Matches
// `gpu_types::GpuEntityChunkInstance`. Every field is a `u32`; no
// `vec3`-then-scalar hazard. WGSL `array<EntityChunkInstance>` stride is 20 B.
struct EntityChunkInstance {
    data1: u32,
    data2: u32,
    data3: u32,
    data4: u32,
    data5: u32,
};

// W4 entity-update uniform — the per-pass scalar set
// (`entityUpdate.fx:12`: `entityInstanceCount, entityChunkInstanceCount,
//  taaIndex, updateCount`).
//
// Distinct from `ConstructionParams` (used by W1/W2/W3) because the entity
// path's per-frame scalars do not overlap with the construction-pass uniform
// fields, and unifying them would force every construction pass to bind every
// entity scalar.
struct EntityUpdateParams {
    entity_instance_count: u32,
    entity_chunk_instance_count: u32,
    taa_index: u32,
    update_count: u32,
    // The `WorldRender.cs:88` per-frame entity-instance cap stride (default
    // 16384). The history ring is `taa_index * max_entity_instances + id`.
    max_entity_instances: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    // Web-WebGPU migration — world size in chunks, needed by `update_chunks`
    // to flatten the chunk position into a buffer index (chunks is now
    // `array<vec2<u32>>`, not a 3D texture). 16 B row (vec3 + pad).
    size_in_chunks: vec3<u32>,
    _pad3: u32,
};

@group(0) @binding(0)
var<storage, read_write> chunks: array<vec2<u32>>;
@group(0) @binding(1)
var<uniform> params: EntityUpdateParams;

@group(1) @binding(0)
var<storage, read> chunk_updates_dynamic: array<vec2<u32>>;
@group(1) @binding(1)
var<storage, read> entity_chunk_instances_dynamic: array<EntityChunkInstance>;
@group(1) @binding(2)
var<storage, read> entity_history_dynamic: array<vec4<u32>>;
@group(1) @binding(3)
var<storage, read_write> entity_chunk_instances_rw: array<EntityChunkInstance>;
@group(1) @binding(4)
var<storage, read_write> entity_instances_history_rw: array<vec4<u32>>;


@compute @workgroup_size(64, 1, 1)
fn update_chunks(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if (global_id.x >= params.update_count) {
        return;
    }
    let update = chunk_updates_dynamic[global_id.x];
    // chunkPos = (update.x & 0x7FF, (update.x >> 11) & 0x3FF, update.x >> 21)
    let chunk_pos = vec3<u32>(
        update.x & 0x7FFu,
        (update.x >> 11u) & 0x3FFu,
        update.x >> 21u,
    );
    // Web-WebGPU migration: flatten the chunk position into the
    // `array<vec2<u32>>` buffer. `chunks[idx] = vec2<u32>(old.x, update.y)` —
    // preserve `.x` (W1 construction-side state), overwrite `.y` (the entity
    // pointer + counter packed into one u32). W4 contract.
    let chunk_idx = chunk_pos.x
        + chunk_pos.y * params.size_in_chunks.x
        + chunk_pos.z * params.size_in_chunks.x * params.size_in_chunks.y;
    let old = chunks[chunk_idx];
    chunks[chunk_idx] = vec2<u32>(old.x, update.y);
}

@compute @workgroup_size(64, 1, 1)
fn copy_entity_chunk_instances(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if (global_id.x >= params.entity_chunk_instance_count) {
        return;
    }
    entity_chunk_instances_rw[global_id.x] = entity_chunk_instances_dynamic[global_id.x];
}

@compute @workgroup_size(64, 1, 1)
fn copy_entity_history(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if (global_id.x >= params.entity_instance_count) {
        return;
    }
    let slot = params.taa_index * params.max_entity_instances + global_id.x;
    entity_instances_history_rw[slot] = entity_history_dynamic[global_id.x];
}
