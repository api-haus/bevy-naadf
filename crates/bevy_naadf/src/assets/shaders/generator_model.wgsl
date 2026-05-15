// Phase-C W5 — `generatorModel.fx` ported to WGSL (`15-design-c.md` §4.5).
//
// Faithful port of NAADF's `Content/shaders/world/generator/generatorModel.fx`
// (80 lines). One entry point `fill_chunk_data_with_model_data_16`,
// `numthreads(4,4,4)`. Per-thread 32 iterations × 2 voxels per iteration → 64
// voxels per thread → 2048 u32s per workgroup. The CPU oracle
// `crate::aadf::generator::generate_segment_cpu` produces byte-identical
// output (§1.6 W5 row).
//
// MonoGame → wgpu deviations: HLSL `RWStructuredBuffer<uint>` maps to WGSL
// `array<u32>` in a read-write storage buffer; HLSL `StructuredBuffer<uint>`
// maps to WGSL read-only storage. Per-thread params come from a single
// uniform `GeneratorModelParams` instead of HLSL's flat per-effect-parameter
// scalars — collapsed for the same reason `15-design-c.md` §1.8's
// `GpuConstructionParams` collapses NAADF's per-handler scalars.

// W5 keeps its own params uniform separate from `GpuConstructionParams` —
// the generator runs at Startup (regime 1, §1.2), not in the per-frame chain;
// fields collapse to a tight 9-u32 = 36-byte (= rounded-up 48 B with std140
// padding) layout matching the C# `Effect.Parameters` set
// (`WorldGeneratorModel.cs:37-55`).
//
// Layout discipline (`15-design-c.md` §1.5): no `vec3`-then-scalar hazard
// because every `vec3<u32>` is followed by explicit padding to a 16-byte row.
// Total: 48 B = 3 × 16-byte rows.

struct GeneratorModelParams {
    // Row 0 (offset 0): sizeInVoxels (vec3) + pad to 16.
    size_in_voxels: vec3<u32>,
    _pad0: u32,
    // Row 1 (offset 16): modelSizeInChunks (vec3) + pad to 16.
    model_size_in_chunks: vec3<u32>,
    _pad1: u32,
    // Row 2 (offset 32): groupOffsetInChunks (vec3) + groupSizeInChunksX
    //                    (which carries the X stride for `groupIndex`; see
    //                    `generatorModel.fx:57`).
    group_offset_in_chunks: vec3<u32>,
    group_size_in_chunks_x: u32,
    // Row 3 (offset 48): groupSizeInChunksY (= the Y stride for `groupIndex`) +
    //                    3 pad u32s. Total struct size: 64 B = 4 × 16-byte rows.
    group_size_in_chunks_y: u32,
    _pad2: u32,
    _pad3: u32,
    _pad4: u32,
}

@group(0) @binding(0) var<storage, read_write> chunk_data_rw: array<u32>;
@group(0) @binding(1) var<storage, read> model_data_chunk_ro: array<u32>;
@group(0) @binding(2) var<storage, read> model_data_block_ro: array<u32>;
@group(0) @binding(3) var<storage, read> model_data_voxel_ro: array<u32>;
@group(0) @binding(4) var<uniform> params: GeneratorModelParams;

// Bit-exact port of `generatorModel.fx::getVoxelDataInModel` (`:16-52`).
//
// Returns the 15-bit voxel type at `voxel_pos`, or `0` when the position is
// out of bounds or when the Y-clamp at `:48-49` fires. The Rust CPU oracle in
// `crate::aadf::generator::get_voxel_type_in_model` mirrors this byte-for-byte.
fn get_voxel_data_in_model(voxel_pos: vec3<u32>) -> u32 {
    // `:18-19` — out-of-volume short circuit.
    if (any(voxel_pos >= params.size_in_voxels)) {
        return 0u;
    }

    let msc = params.model_size_in_chunks;
    let model_extent_v = msc * 16u;
    let vpim = voxel_pos % model_extent_v;
    // `:21` — vertical "stamp index" for the Y-clamp.
    let model_index_y = voxel_pos.y / (msc.y * 16u);

    // `:24-25` — chunk index in the model.
    let cpim = vpim / 16u;
    let chunk_index_in_model = cpim.x + cpim.y * msc.x + cpim.z * msc.x * msc.y;
    let chunk = model_data_chunk_ro[chunk_index_in_model];

    var ty: u32 = 0u;

    let chunk_disc = chunk >> 30u;
    if (chunk_disc == 2u) {
        // `:32-33` — block index in the chunk.
        let mbpic = (vpim % 16u) / 4u;
        let model_block_index = mbpic.x + mbpic.y * 4u + mbpic.z * 16u;
        // `:34` — fetch the block node from the model.
        let block_addr = (chunk & 0x3FFFFFFFu) + model_block_index;
        let block = model_data_block_ro[block_addr];

        let block_disc = block >> 30u;
        if (block_disc == 2u) {
            // `:37-38` — voxel index in the block.
            let mvpic = vpim % 4u;
            let model_voxel_index = mvpic.x + mvpic.y * 4u + mvpic.z * 16u;
            // `:39` — fetch the voxel pair; even index in low half, odd in high.
            let voxel_addr = (block & 0x3FFFFFFFu) + model_voxel_index / 2u;
            let model_voxel_comp = model_data_voxel_ro[voxel_addr];
            // `:40` — mask to 15 bits.
            if ((model_voxel_index % 2u) == 0u) {
                ty = model_voxel_comp & 0x7FFFu;
            } else {
                ty = (model_voxel_comp >> 16u) & 0x7FFFu;
            }
        } else if (block_disc == 1u) {
            // `:43` — uniform-full block; type is the low 30 bits.
            ty = block & 0x3FFFFFFFu;
        }
    } else if (chunk_disc == 1u) {
        // `:46` — uniform-full chunk; type is the low 30 bits.
        ty = chunk & 0x3FFFFFFFu;
    }

    // `:48-49` — Y-clamp: only the ground-level copy materialises vertically.
    if (model_index_y > 0u) {
        ty = 0u;
    }

    return ty;
}

@compute @workgroup_size(4, 4, 4)
fn fill_chunk_data_with_model_data_16(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // `:57` — group_index uses X/Y strides only (Z is the outer loop in the
    // dispatch shape; the per-segment buffer is logically a 3D array of chunks
    // but addressed as a flat u32 buffer of `chunks * 2048` elements).
    let group_index = group_id.x
        + group_id.y * params.group_size_in_chunks_x
        + group_id.z * params.group_size_in_chunks_x * params.group_size_in_chunks_y;

    for (var i: u32 = 0u; i < 32u; i = i + 1u) {
        let i2 = i * 2u;
        // `:61` — voxel pair position inside this thread's 4×4×4 block.
        let voxel_pos_in_block = vec3<u32>(i2 % 4u, (i2 / 4u) % 4u, i2 / 16u);
        // `:62` — full world-space voxel position for this pair's voxel1.
        let voxel_pos =
            (params.group_offset_in_chunks + group_id) * 16u
            + local_id * 4u
            + voxel_pos_in_block;

        // `:64-65` — fetch the two adjacent voxels' types.
        var voxel1 = get_voxel_data_in_model(voxel_pos);
        var voxel2 = get_voxel_data_in_model(voxel_pos + vec3<u32>(1u, 0u, 0u));

        // `:67-68` — apply the "full" flag whenever the type is non-zero.
        if (voxel1 > 0u) {
            voxel1 = voxel1 | (1u << 15u);
        }
        if (voxel2 > 0u) {
            voxel2 = voxel2 | (1u << 15u);
        }

        // `:70` — pack the two voxels into one u32 of `chunk_data`.
        let dst = group_index * 2048u + local_index * 32u + i;
        chunk_data_rw[dst] = voxel1 | (voxel2 << 16u);
    }
}
