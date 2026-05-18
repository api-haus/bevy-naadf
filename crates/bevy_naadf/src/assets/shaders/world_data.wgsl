// world_data.wgsl — the `@group(0)` world-data bind declarations.
//
// Derives from: the `RWStructuredBuffer` / `RWTexture3D` declarations in
// `render/rayTracing.fxh` + `world/data/chunkCalc.fx` (`03-design.md` §5.5,
// §2.6). Phase A: the chunk layer is a read-only `texture_3d<u32>` (CPU-built,
// upload-only — `03-design.md` §6.1), the block / voxel / voxel-type layers
// are read-only storage buffers.
//
// HLSL `rayTracing.fxh`:
//   StructuredBuffer<uint> voxels;
//   StructuredBuffer<uint> blocks;
//   Texture3D<CHUNKTYPE> chunks;            // CHUNKTYPE = uint (no ENTITIES)
//   StructuredBuffer<uint4> voxelTypeData;
//   int groupSizeX, groupSizeY, groupSizeZ; // world size in chunks
//   float3 boundingBoxMin, boundingBoxMax;
//
// naga-oil import module — entry shaders pull these bindings in via
// `#import "shaders/world_data.wgsl"::{...}`.

// World geometry the traversal shader needs (mirrors `gpu_types::GpuWorldMeta`).
//
// `rayTracing.fxh` carries `groupSizeX/Y/Z` (= the chunk-grid extent) and
// `boundingBoxMin/Max` (the voxel-space geometry AABB `rayAABB` clips to) as
// loose globals; here they are one small uniform.
//
// streaming-world Phase 2.6: `streaming_active` (slot 12 — the previously-
// implicit `_pad0` u32) controls whether the renderer translates `chunks`
// reads through `window_indirection` (binding 8 below). Promoted from `_pad0`,
// 0 = direct flat-coord reads (Default/Vox/ProceduralStatic/EntityUpdate
// presets stay bit-identical); 1 = indirection-translated reads.
//
// No explicit padding members (naga-oil's composable-module round-trip rejects
// them): WGSL slots each `vec3` into a 16-byte aligned slot, reproducing the
// padded Rust `#[repr(C)]` layout — `size_in_chunks` (0..12),
// `streaming_active` (12..16), `bounding_box_min` (16..32),
// `bounding_box_max` (32..48), total 48 bytes.
struct GpuWorldMeta {
    // World size in chunks.
    size_in_chunks: vec3<u32>,
    // streaming-world Phase 2.6 — 1 when the streaming preset is active; 0
    // otherwise. Gates the `streaming_chunk_index` indirection-translation
    // helper below.
    streaming_active: u32,
    // Geometry AABB minimum, in voxels — NAADF's `boundingBoxMin` (the
    // 0.1-voxel-inset world minimum, `WorldData.cs:477`). `float3`, not
    // integer, faithful to `rayTracing.fxh`'s `float3 boundingBoxMin`.
    bounding_box_min: vec3<f32>,
    // Geometry AABB maximum, in voxels — NAADF's `boundingBoxMax`
    // (`sizeInVoxels - 0.1`, `WorldData.cs:478`).
    bounding_box_max: vec3<f32>,
}

// --- @group(0) — world data (read-only in the render passes) ----------------

// The chunk layer: encoded chunk pair per chunk, indexed by chunk position
// flattened linear via `flatten_index(chunk_pos, sx, sx*sy)` from
// `common.wgsl` (x-fastest then y then z; matches the C# / CPU layout in
// `entity_handler.rs::chunk_index_to_pos`).
//
// **W4 (`15-design-c.md` §1.7) — chunk-pair widened to `vec2<u32>`** so each
// chunk carries the per-chunk entity pointer in `.y`. `.x` = block-state
// pointer + AADF (W1/W2/W3, unchanged semantics); `.y` = entity pointer +
// counter (W4, `entity_update.wgsl` writes; `ray_tracing.wgsl` reads).
//
// **Web-WebGPU migration (`docs/orchestrate/web-chunks-storage-buffer/`)** —
// representation changed from `texture_storage_3d<rg32uint, read_write>` /
// `texture_3d<u32>` to `array<vec2<u32>>` because the WebGPU spec only
// permits `StorageTextureAccess::ReadWrite` on the `r32{uint,sint,float}`
// allow-list. Field-selector discipline (`.x` / `.y`) is preserved
// byte-for-byte; only the binding type changed.
@group(0) @binding(0) var<storage, read> chunks: array<vec2<u32>>;

// The block layer: encoded block `u32`s, 64 consecutive per mixed chunk
// (HLSL `StructuredBuffer<uint> blocks`).
@group(0) @binding(1) var<storage, read> blocks: array<u32>;

// The voxel layer: packed voxel `u32`s (two 16-bit voxels each), 32 consecutive
// per mixed block (HLSL `StructuredBuffer<uint> voxels`).
@group(0) @binding(2) var<storage, read> voxels: array<u32>;

// The material buffer: one 128-bit (`vec4<u32>`) material entry per voxel type;
// voxel 15-bit type ids index into it (HLSL `StructuredBuffer<uint4>
// voxelTypeData`).
@group(0) @binding(3) var<storage, read> voxel_types: array<vec4<u32>>;

// World geometry (HLSL `groupSize*` + `boundingBox*`).
@group(0) @binding(4) var<uniform> world_meta: GpuWorldMeta;

// === W4 entity track — render-side read-only bindings (Phase-C wave-3) =====
//
// The entity track widens `@group(0)` with 3 read-only buffers consumed by the
// `ray_tracing.wgsl::shoot_ray` entity sub-traversal branch (the HLSL
// `#ifdef ENTITIES` path in `rayTracing.fxh:81-240` /
// `commonEntities.fxh`). Layout-wise these bindings are **always present** —
// wave-3 extended `NaadfPipelines::world_layout` with them so a single
// `naadf_world_bind_group_layout` covers both the disabled-entities and
// enabled-entities cases. When `ConstructionConfig.entities_enabled = false`,
// `prepare_construction` binds 1-element placeholder buffers so the layout is
// satisfied; the `shoot_ray` entity branch never fires because the gate is
// checked first (`ENTITIES_ENABLED` shader-def + the runtime `chunks[pos].y`
// pointer check).

// `EntityChunkInstance` — 20 B, 5 × u32, mirrors `gpu_types::GpuEntityChunkInstance`.
// Field names avoid trailing `<digit>` (naga-oil composable-module identifier
// rule: identifiers must not resemble `#{...}` substitution targets, which
// match e.g. `data1`).
struct EntityChunkInstance {
    pack_a: u32,
    pack_b: u32,
    pack_c: u32,
    pack_d: u32,
    pack_e: u32,
};

// Per-(chunk × entity) packed instance — indexed by
// `(chunks[pos].y >> 8) + chunk_entity_index`. HLSL
// `StructuredBuffer<EntityChunkInstance> entityChunkInstances` (`rayTracing.fxh:41`).
@group(0) @binding(5) var<storage, read> entity_chunk_instances: array<EntityChunkInstance>;

// Per-entity AADF voxel volume — 64 u32s per entity. Indexed by
// `entity_instance.voxel_start * 64 + voxel_idx`. HLSL
// `StructuredBuffer<uint> entityVoxelData` (`rayTracing.fxh:42`).
@group(0) @binding(6) var<storage, read> entity_voxel_data: array<u32>;

// TAA-history ring of `Uint4` per entity-instance per TAA frame. Indexed by
// `taa_index * MAX_ENTITY_INSTANCES + entity_instance_id`. HLSL
// `StructuredBuffer<uint4> entityInstancesHistory` (`rayTracing.fxh:48`).
// Currently unused by the renderer-side `shoot_ray` traversal (the C# uses it
// for TAA reprojection of moving entities — Phase-C wave-3 lands the layout
// binding; the consumer is a Phase-D / paper-gap follow-up).
//
// Phase-C followup #4 — the allocation backing this binding is gated by
// `ConstructionConfig.entity_history_enabled` (default `false`). When `false`,
// `prepare_construction` allocates a 16 B (1-vec4) placeholder so the
// bind-group layout is satisfied without paying the
// `max_entity_instances * taa_ring_depth * 16 B` price; the
// `copy_entity_history` GPU dispatch is skipped. The shader treats this
// binding as read-only and never indexes into it (the entity sub-traversal
// branch does not consume the history) — the placeholder is bind-only,
// never read, never written.
@group(0) @binding(7) var<storage, read> entity_instances_history: array<vec4<u32>>;

// streaming-world Phase 2.6 — window indirection table
// (`02c-design-windowed-slot-map.md` § D). Maps
// `pack(local_xyz) = lx + ly*WINDOW_SIZE_X + lz*WINDOW_SIZE_X*WINDOW_SIZE_Y →
// SlotIndex`, or `0xFFFFFFFFu` (EMPTY_SLOT) for "no segment bound at this
// local position".
//
// Size: fixed 512 u32 (= WORLD_SIZE_IN_SEGMENTS.x * y * z = 16 * 2 * 16).
// On non-streaming presets, prepare_world_gpu binds a 1-u32 placeholder
// here — the `streaming_active == 0` branch in the helpers below short-
// circuits the read so `array_length(&window_indirection) > 1u` never fires
// on those presets.
@group(0) @binding(8) var<storage, read> window_indirection: array<u32>;

// Phase 2.6 — translate a window-local (or absolute, on the streaming preset
// the camera is pre-translated to window-local frame) chunk-coord through
// the indirection table to its slot-indexed position in `chunks_buffer`.
// Returns `0xFFFFFFFFu` ("no chunk") when the local coord points at an empty
// slot — callers treat this as "sky" (early-exit / empty texel).
//
// Non-streaming presets: pass-through to the flat-coord layout, byte-
// identical to the pre-2.6 read path. The `streaming_active == 0` branch is
// uniform-flow across the frame (the uniform field doesn't vary by wave) so
// no real branch divergence.
fn streaming_chunk_index(chunk_pos: vec3<u32>) -> u32 {
    if (world_meta.streaming_active == 0u) {
        return chunk_pos.x
             + chunk_pos.y * world_meta.size_in_chunks.x
             + chunk_pos.z * world_meta.size_in_chunks.x * world_meta.size_in_chunks.y;
    }
    // Streaming path:
    //   1. Translate world-local chunk coord to (segment-local, chunk-in-seg).
    //   2. pack(segment-local) → indirection table index.
    //   3. indirection[idx] = slot → slot * 4096 + chunk_in_seg_idx.
    let chunks_per_seg_x: u32 = 16u;
    let chunks_per_seg_y: u32 = 16u;
    let chunks_per_seg_z: u32 = 16u;
    let seg_local = vec3<u32>(
        chunk_pos.x / chunks_per_seg_x,
        chunk_pos.y / chunks_per_seg_y,
        chunk_pos.z / chunks_per_seg_z,
    );
    let chunk_in_seg = vec3<u32>(
        chunk_pos.x % chunks_per_seg_x,
        chunk_pos.y % chunks_per_seg_y,
        chunk_pos.z % chunks_per_seg_z,
    );
    // pack(local_xyz) for WINDOW_SIZE = (16, 2, 16).
    let local_pack = seg_local.x + seg_local.y * 16u + seg_local.z * (16u * 2u);
    let slot = window_indirection[local_pack];
    if (slot == 0xFFFFFFFFu) {
        return 0xFFFFFFFFu; // empty — caller treats as sky.
    }
    let chunks_per_seg_total: u32 = 4096u; // 16^3.
    let chunk_in_seg_idx = chunk_in_seg.x
        + chunk_in_seg.y * chunks_per_seg_x
        + chunk_in_seg.z * chunks_per_seg_x * chunks_per_seg_y;
    return slot * chunks_per_seg_total + chunk_in_seg_idx;
}

// Convenience wrapper — load the chunks vec2<u32> at the streaming-translated
// position, returning `vec2(0u, 0u)` (the "uniform empty" chunk state) on
// EMPTY_SLOT.
//
// streaming-world Phase 2.10
// (`docs/orchestrate/streaming-world/03l-diagnosis-hitch-and-view-distance.md`
// § 2 — "EMPTY_SLOT returning sky vs treating as empty chunk"): the design
// note `02c-design-windowed-slot-map.md` § E proposed
// `if slot == EMPTY_SLOT { return SKY; }` as an early-out. What is actually
// shipped is "treat as uniform-empty chunk with zero AADF":
//   - cur_node = (0u, 0u) → `(cur_node >> 31u) == 0u` → ray DDA falls into
//     the "not mixed" branch and skips to the far face of the empty chunk.
//   - 5-bit chunk-level AADFs at bits [0..30) are zero → the skip is only
//     `offset + 16 * 0 = offset` voxels (≤ 16), so the ray steps once per
//     chunk through the empty region.
//   - The ray either hits a resident chunk past the empty slot OR exits the
//     world bbox at `bounding_box_max` and misses to atmosphere.
//
// We KEEP the "uniform empty" semantic in Phase 2.10 — it's well-defined and
// rays exit the window cleanly. Picking up a SKY early-out instead would
// short-circuit a tiny number of cycles on the per-chunk descent but
// complicate the ray loop's atmosphere-shading path (which currently runs
// only at `cur_cell >= bbox_max` exit). Phase 2.10's per-segment bounds
// dispatch (`03l` punch-list item 1) + W3 regime-1 seed restoration (item 3)
// make the AADF-stale-on-fresh-admission scenario disappear, so the
// per-chunk-step penalty through EMPTY_SLOT regions is no longer a
// user-visible artefact — there are no large EMPTY_SLOT regions inside the
// resident window at steady state.
fn streaming_chunk_load(chunk_pos: vec3<u32>) -> vec2<u32> {
    let idx = streaming_chunk_index(chunk_pos);
    if (idx == 0xFFFFFFFFu) {
        return vec2<u32>(0u, 0u);
    }
    return chunks[idx];
}
