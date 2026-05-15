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
// No explicit padding members (naga-oil's composable-module round-trip rejects
// them): WGSL slots each `vec3` into a 16-byte aligned slot, reproducing the
// padded Rust `#[repr(C)]` layout — `size_in_chunks` (0..16),
// `bounding_box_min` (16..32), `bounding_box_max` (32..48), total 48 bytes.
struct GpuWorldMeta {
    // World size in chunks.
    size_in_chunks: vec3<u32>,
    // Geometry AABB minimum, in voxels — NAADF's `boundingBoxMin` (the
    // 0.1-voxel-inset world minimum, `WorldData.cs:477`). `float3`, not
    // integer, faithful to `rayTracing.fxh`'s `float3 boundingBoxMin`.
    bounding_box_min: vec3<f32>,
    // Geometry AABB maximum, in voxels — NAADF's `boundingBoxMax`
    // (`sizeInVoxels - 0.1`, `WorldData.cs:478`).
    bounding_box_max: vec3<f32>,
}

// --- @group(0) — world data (read-only in the render passes) ----------------

// The chunk layer: one encoded chunk `u32` per chunk, indexed by chunk position
// (HLSL `Texture3D<uint> chunks`). Phase A is entity-free so this is `u32`, not
// the `Rg64Uint` the C# uses with `ENTITIES` (`03-design.md` §7.5).
@group(0) @binding(0) var chunks: texture_3d<u32>;

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
