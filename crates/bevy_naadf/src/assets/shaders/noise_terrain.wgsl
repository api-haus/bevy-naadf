// noise_terrain.wgsl — sliding-window streaming-world noise → segment_voxel_buffer.
//
// Phase-2 deliverable of `docs/orchestrate/streaming-world/02b-design-plan-b.md` § E.1.
//
// Drop-in replacement for `generator_model.wgsl::fill_chunk_data_with_model_data`
// (W5 stage-1 producer) in the streaming preset. The output byte layout is
// **byte-identical** to what `generator_model.wgsl` produces:
//
//   chunk_data_rw[group_index * 2048 + local_index * 32 + i] = voxel1 | (voxel2 << 16u)
//
// where each voxel is `(1u << 15) | type_id` for solid, `0` for empty.
//
// Composition with Phase 1: the Rust harness inlines
// `noise_fastnoiselite.wgsl` ABOVE this file's `// @begin` marker (the same
// pattern Phase 1 uses for `noise_oracle_dispatch.wgsl`).
//
// Classification (OQ.1 — height-relative, per `02b-design-plan-b.md` § Phase 2
// design refinements):
//
//   let n = fnl_get_noise_3d(state, world_x, world_y, world_z);
//   let height_term = (sea_level - world_y) / terrain_amplitude;
//   let is_solid = (n + height_term) > 0.0;
//
// Produces ground + rolling hills + caves Minecraft-style.

// @begin

// `NoiseTerrainParams` mirrors `crate::streaming::noise_dispatch::NoiseTerrainParams`
// in the Rust port — 80 B `FnlState` + a few scalar params + segment origin,
// padded to a single uniform.
struct NoiseTerrainParams {
    // Row 0 (offset 0): segment origin in voxels (signed i32 — segments live in
    // world coords).
    seg_origin_in_voxels_x: i32,
    seg_origin_in_voxels_y: i32,
    seg_origin_in_voxels_z: i32,
    terrain_voxel_type_id: u32,  // low 15 bits = VoxelTypeId for solid voxels.
    // Row 1 (offset 16): segment-cubic dispatch shape + classification config.
    group_size_in_chunks_x: u32, // X stride for `group_index`.
    group_size_in_chunks_y: u32, // Y stride for `group_index`.
    sea_level: f32,              // world-Y at which `noise == 0` flips solid/empty.
    terrain_amplitude: f32,      // height span over which noise transitions.
    // Row 2 onward: the full FnlState (80 B = 5 × 16-byte rows).
    state: FnlState,
};

@group(0) @binding(0) var<storage, read_write> chunk_data_rw: array<u32>;
@group(0) @binding(1) var<uniform> params: NoiseTerrainParams;

// Solid-voxel encoding (matches `generator_model.wgsl:149-154`).
const VOXEL_FULL_FLAG_LOCAL: u32 = (1u << 15u);
const VOXEL_PAYLOAD_MASK_LOCAL: u32 = 0x7FFFu;

/// Per-voxel classification. World coords are in voxels (signed; segments may
/// have negative origins in the residency window).
///
/// `n + (sea_level - y) / terrain_amplitude > 0 → solid`.
fn classify_voxel(world_x: f32, world_y: f32, world_z: f32) -> u32 {
    let n = fnl_get_noise_3d(params.state, world_x, world_y, world_z);
    let height_term = (params.sea_level - world_y) / params.terrain_amplitude;
    if ((n + height_term) > 0.0) {
        return (params.terrain_voxel_type_id & VOXEL_PAYLOAD_MASK_LOCAL) | VOXEL_FULL_FLAG_LOCAL;
    }
    return 0u;
}

/// `numthreads(4,4,4)` — one workgroup per chunk in the segment; 64 threads
/// per workgroup, each handling 32 u32s × 2 voxels = 64 voxels' worth.
/// Same dispatch shape as `generator_model.wgsl:121-122`.
@compute @workgroup_size(4, 4, 4)
fn fill_chunk_data_with_noise(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // Match `generator_model.wgsl:130-132` flat-index math so the output buffer
    // layout is byte-identical.
    let group_index = group_id.x
        + group_id.y * params.group_size_in_chunks_x
        + group_id.z * params.group_size_in_chunks_x * params.group_size_in_chunks_y;

    for (var i: u32 = 0u; i < 32u; i = i + 1u) {
        let i2 = i * 2u;
        // Voxel-pair position inside this thread's 4×4×4 block (matches
        // `generator_model.wgsl:137`).
        let voxel_pos_in_block = vec3<u32>(i2 % 4u, (i2 / 4u) % 4u, i2 / 16u);

        // Local (within-segment) voxel offset in u32 components.
        let local_offset_x = group_id.x * 16u + local_id.x * 4u + voxel_pos_in_block.x;
        let local_offset_y = group_id.y * 16u + local_id.y * 4u + voxel_pos_in_block.y;
        let local_offset_z = group_id.z * 16u + local_id.z * 4u + voxel_pos_in_block.z;

        // Convert to signed world-voxel coordinates. The segment origin is the
        // **world** position of the segment's `(0, 0, 0)` voxel — adding the
        // u32 local offset (cast through i32) gives world voxel coords.
        let world_x1_i = params.seg_origin_in_voxels_x + i32(local_offset_x);
        let world_y1_i = params.seg_origin_in_voxels_y + i32(local_offset_y);
        let world_z1_i = params.seg_origin_in_voxels_z + i32(local_offset_z);

        // Voxel 2 is `world_x + 1` (the X-pair packing matches
        // `generator_model.wgsl:146`).
        let world_x2_i = world_x1_i + 1;

        let v1 = classify_voxel(f32(world_x1_i), f32(world_y1_i), f32(world_z1_i));
        let v2 = classify_voxel(f32(world_x2_i), f32(world_y1_i), f32(world_z1_i));

        // Pack the two voxels into one u32 of `chunk_data` (matches
        // `generator_model.wgsl:157-158`).
        let dst = group_index * 2048u + local_index * 32u + i;
        chunk_data_rw[dst] = v1 | (v2 << 16u);
    }
}
