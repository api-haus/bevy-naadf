//! W5 — World generator: `ModelData` consumer + bit-exact CPU oracle for
//! `generatorModel.fx::fillChunkDataWithModelData16` (`15-design-c.md` §2.1 W5
//! row, §4.5, §1.6).
//!
//! NAADF's `WorldGeneratorModel` (C# `World/Generator/WorldGeneratorModel.cs`)
//! evaluates a three-layer voxel model (`ModelData` — `World/Model/ModelData.cs`)
//! at every voxel of the world segment and packs two voxel `u16`s into each
//! `u32` of `segmentVoxelBuffer`. The GPU shader is `generatorModel.fx`:
//! `numthreads(4,4,4)` per chunk, 32 iterations × 2 voxels per iteration → 64
//! voxels per thread → 2048 u32s per workgroup. Voxels with `type > 0` get the
//! "full" flag `1 << 15` set.
//!
//! This module mirrors the C# semantics faithfully:
//!
//! - [`ModelData`] — the host-side representation of NAADF's three flat byte
//!   arrays (`dataChunk` / `dataBlock` / `dataVoxel`) + the model size in
//!   chunks. The encoding is the same one `WorldGeneratorModel.CopyToChunkData`
//!   uploads (`generatorModel.fx:1-14`).
//! - [`get_voxel_type_in_model`] — the bit-exact port of HLSL's
//!   `getVoxelDataInModel` (`generatorModel.fx:16-52`).
//! - [`generate_segment_cpu`] — the bit-exact port of the
//!   `fillChunkDataWithModelData16` workgroup body (`generatorModel.fx:54-72`).
//!   Produces the same byte layout the GPU shader writes to
//!   `chunk_data[group_index * 2048 + local_index * 32 + i]`. This is the
//!   §1.6 oracle compared against the GPU's `segment_voxel_buffer` output.
//!
//! ## Encoding (from `generatorModel.fx`)
//!
//! Each `modelDataChunk[i]` is a `u32`:
//! - top 2 bits = node type (`0` = empty, `1` = uniform-full, `2` = mixed),
//! - low 30 bits = block-base pointer (when mixed) or voxel type (when uniform).
//!
//! Each `modelDataBlock[i]` is the same encoding, with the low 30 bits pointing
//! into `modelDataVoxel` when mixed.
//!
//! Each `modelDataVoxel[i]` packs two voxel `u16`s — bit 15 is the "full" flag,
//! low 15 bits are the voxel type id.
//!
//! ## Y-clamp (faithful port — `generatorModel.fx:48-49`)
//!
//! After resolving the type from the model, the HLSL forces `type = 0` whenever
//! the voxel's Y-coordinate is past the model's vertical extent: only the
//! ground-level copy of the model is materialised vertically; everything above
//! is empty. The CPU oracle replicates this exactly.

use bevy::prelude::Resource;

use crate::voxel::CHUNK_DIM_VOXELS as CHUNK_DIM_VOXELS_USIZE;

/// Side length of a chunk in voxels (`CELL_DIM² = 16`). Sourced from
/// [`crate::voxel::CHUNK_DIM_VOXELS`] — the single Rust SSoT (SSoT-3).
const CHUNK_DIM_VOXELS: u32 = CHUNK_DIM_VOXELS_USIZE as u32;

/// Number of u32s emitted by one workgroup of `generatorModel.fx` — 64
/// voxels per thread × 64 threads ÷ 2 voxels/u32 = 2048 u32s.
pub const CHUNK_DATA_U32S: u32 = 2048;

/// Three-layer voxel model (`ModelData` — NAADF
/// `World/Model/ModelData.cs:23-31`).
///
/// The byte arrays are the same encoding `WorldGeneratorModel` uploads as
/// `modelDataChunk` / `modelDataBlock` / `modelDataVoxel` to the
/// `generatorModel.fx` shader. The host-side flat layout is preserved verbatim:
/// no Bevy-flavoured wrapping, no `Vec<VoxelTypeId>` translation. This is
/// load-bearing for byte-exact CPU/GPU parity.
///
/// Layout (`generatorModel.fx:16-52`):
/// - `data_chunk[c]` — `chunk` node at the model's chunk grid `c`; top 2 bits
///   = node type, low 30 bits = payload.
/// - `data_block[c & 0x3FFF_FFFF + b]` — `block` node at the chunk's block
///   index `b` (only valid when `data_chunk[c] >> 30 == 2`).
/// - `data_voxel[(b & 0x3FFF_FFFF) + v / 2]` — two voxels per `u32`, low half
///   = even-index, high half = odd-index (only valid when
///   `data_block[base + b] >> 30 == 2`).
#[derive(Resource, Clone, Debug)]
pub struct ModelData {
    /// Flat `dataChunk` array — `size_in_chunks.x * y * z` entries.
    pub data_chunk: Vec<u32>,
    /// Flat `dataBlock` array — variable length.
    pub data_block: Vec<u32>,
    /// Flat `dataVoxel` array — variable length, two voxels per element.
    pub data_voxel: Vec<u32>,
    /// Model size in chunks, `[x, y, z]` (C# `ModelData.sizeInChunks` —
    /// `ModelData.cs:40`).
    pub size_in_chunks: [u32; 3],
}

impl ModelData {
    /// An empty model — a zero-volume model that resolves to type 0 everywhere.
    /// Used to exercise the Y-clamp branch in tests.
    pub fn empty(size_in_chunks: [u32; 3]) -> Self {
        let chunk_count = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
        Self {
            data_chunk: vec![0; chunk_count],
            data_block: vec![0],
            data_voxel: vec![0],
            size_in_chunks,
        }
    }

    /// A "uniform-full of type `ty`" model — every chunk encodes `(1 << 30) | ty`.
    /// Useful for tests: the generator should pack the same type into every
    /// voxel of the world (modulo the Y-clamp).
    pub fn uniform_full(size_in_chunks: [u32; 3], ty: u32) -> Self {
        let chunk_count = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
        let payload = (1u32 << 30) | (ty & 0x3FFF_FFFF);
        Self {
            data_chunk: vec![payload; chunk_count],
            data_block: vec![0],
            data_voxel: vec![0],
            size_in_chunks,
        }
    }
}

/// Bit-exact port of `generatorModel.fx::getVoxelDataInModel` (`:16-52`).
///
/// Returns the 15-bit voxel type at `voxel_pos`, or `0` when the position is
/// out of bounds or when the Y-clamp at `:48-49` fires.
///
/// The function reads `data_chunk` / `data_block` / `data_voxel` as the GPU
/// shader does — same indexing math, same masks, same `>> 30` discriminators.
fn get_voxel_type_in_model(
    model: &ModelData,
    voxel_pos: [i64; 3],
    size_in_voxels: [u32; 3],
) -> u32 {
    // HLSL: `if (any(voxelPos >= sizeInVoxels)) return 0;` (`:18-19`). Note that
    // `voxelPos` is HLSL `uint3` so the >= 0 branch is implicit; the CPU port
    // models the variable as `i64` so we can express "out of range" cleanly.
    // The Rust port uses i64 to safely model the C# `uint3 >= uint3` semantics
    // (negative inputs would never pass `>= sizeInVoxels` anyway).
    if voxel_pos[0] < 0
        || voxel_pos[1] < 0
        || voxel_pos[2] < 0
        || voxel_pos[0] >= size_in_voxels[0] as i64
        || voxel_pos[1] >= size_in_voxels[1] as i64
        || voxel_pos[2] >= size_in_voxels[2] as i64
    {
        return 0;
    }

    let vx = voxel_pos[0] as u32;
    let vy = voxel_pos[1] as u32;
    let vz = voxel_pos[2] as u32;

    let msc = model.size_in_chunks;

    // HLSL: `voxelPosInModel = voxelPos % (modelSizeInChunks * 16);` (`:20`).
    let model_extent_v = [msc[0] * 16, msc[1] * 16, msc[2] * 16];
    let vpim = [vx % model_extent_v[0], vy % model_extent_v[1], vz % model_extent_v[2]];

    // HLSL: `modelIndexY = voxelPos.y / (modelSizeInChunksY * 16);` (`:21`).
    let model_index_y = vy / (msc[1] * 16);

    // HLSL: `chunkPosInModel = voxelPosInModel / 16;`
    //       `chunkIndexInModel = chunkPosInModel.x + ... ;` (`:24-25`).
    let cpim = [vpim[0] / 16, vpim[1] / 16, vpim[2] / 16];
    let chunk_index_in_model =
        (cpim[0] + cpim[1] * msc[0] + cpim[2] * msc[0] * msc[1]) as usize;
    let chunk = model.data_chunk[chunk_index_in_model];

    let mut ty: u32 = 0;

    let chunk_disc = chunk >> 30;
    if chunk_disc == 2 {
        // HLSL: `modelBlockPosInChunk = (voxelPosInModel % 16) / 4;` (`:32`).
        let mbpic = [(vpim[0] % 16) / 4, (vpim[1] % 16) / 4, (vpim[2] % 16) / 4];
        // `modelBlockIndex = ... ;` (`:33`).
        let model_block_index = mbpic[0] + mbpic[1] * 4 + mbpic[2] * 16;
        // `block = modelDataBlock[(chunk & 0x3FFFFFFF) + modelBlockIndex];` (`:34`).
        let block_addr = ((chunk & 0x3FFF_FFFF) + model_block_index) as usize;
        let block = model.data_block[block_addr];

        let block_disc = block >> 30;
        if block_disc == 2 {
            // HLSL: `modelVoxelPosInChunk = voxelPosInModel % 4;` (`:37`).
            let mvpic = [vpim[0] % 4, vpim[1] % 4, vpim[2] % 4];
            // `modelVoxelIndex = ...;` (`:38`).
            let model_voxel_index = mvpic[0] + mvpic[1] * 4 + mvpic[2] * 16;
            // `modelVoxelComp = modelDataVoxel[(block & 0x3FFFFFFF) + modelVoxelIndex / 2];` (`:39`).
            let voxel_addr =
                ((block & 0x3FFF_FFFF) + model_voxel_index / 2) as usize;
            let voxel_comp = model.data_voxel[voxel_addr];
            // `:40` — even index reads low half, odd index reads high half;
            // mask to 15 bits.
            ty = if model_voxel_index % 2 == 0 {
                voxel_comp & 0x7FFF
            } else {
                (voxel_comp >> 16) & 0x7FFF
            };
        } else if block_disc == 1 {
            // HLSL: `type = block & 0x3FFFFFFF;` (`:43`).
            ty = block & 0x3FFF_FFFF;
        }
    } else if chunk_disc == 1 {
        // HLSL: `type = chunk & 0x3FFFFFFF;` (`:46`).
        ty = chunk & 0x3FFF_FFFF;
    }

    // HLSL: `if (modelIndexY > 0) type = 0;` (`:48-49`). Only the ground-level
    // copy of the model is materialised vertically.
    if model_index_y > 0 {
        ty = 0;
    }

    ty
}

/// Bit-exact CPU oracle for `generatorModel.fx::fillChunkDataWithModelData16`
/// (`15-design-c.md` §1.6, §4.5).
///
/// Generates the same packed-voxel `u32` array the GPU shader writes to
/// `segment_voxel_buffer`, byte-for-byte. The output is sized
/// `group_size_in_chunks.x * y * z * 2048` u32s — same as the GPU buffer's
/// regime-1 allocation (§1.4).
///
/// The shader's dispatch shape is one workgroup per chunk in the segment
/// (§4.5); per workgroup, `numthreads(4,4,4)` = 64 threads each emit 32 u32s
/// = 64 voxels. The CPU oracle iterates `(group_pos × local_pos × i)` in the
/// exact same order, calling [`get_voxel_type_in_model`] at the exact same
/// world-space coordinate the HLSL would.
///
/// # Parameters
/// - `model` — the three-layer voxel model.
/// - `group_offset_in_chunks` — `groupOffsetInChunksX/Y/Z` C# parameter
///   (`WorldGeneratorModel.cs:45-47`). The chunk-space position of the
///   segment's origin.
/// - `group_size_in_chunks` — segment size in chunks; the dispatch shape
///   (`WorldGeneratorModel.cs:48-49`, `:59`). Note only X/Y are passed as
///   shader parameters because the per-thread `groupIndex` only needs the X/Y
///   stride (`generatorModel.fx:57`).
/// - `size_in_voxels` — world `sizeInVoxels.x/y/z` (the out-of-range gate at
///   `generatorModel.fx:18-19`).
///
/// # Returns
/// Flat `Vec<u32>` of length `group_size_in_chunks.x * y * z * 2048`,
/// addressable as `out[group_index * 2048 + local_index * 32 + i]` per
/// `generatorModel.fx:70`.
pub fn generate_segment_cpu(
    model: &ModelData,
    group_offset_in_chunks: [u32; 3],
    group_size_in_chunks: [u32; 3],
    size_in_voxels: [u32; 3],
) -> Vec<u32> {
    let gscx = group_size_in_chunks[0];
    let gscy = group_size_in_chunks[1];
    let gscz = group_size_in_chunks[2];
    let total_chunks = (gscx * gscy * gscz) as usize;
    let mut out = vec![0u32; total_chunks * CHUNK_DATA_U32S as usize];

    // One workgroup per chunk in the segment (`generatorModel.fx` dispatch
    // shape, §4.5).
    for gz in 0..gscz {
        for gy in 0..gscy {
            for gx in 0..gscx {
                let group_id = [gx, gy, gz];
                // `groupIndex = groupID.x + groupID.y * groupSizeInChunksX +
                //               groupID.z * groupSizeInChunksX * groupSizeInChunksY;`
                // (`generatorModel.fx:57`).
                let group_index = gx + gy * gscx + gz * gscx * gscy;
                // Per-workgroup: `numthreads(4,4,4)` = 64 threads.
                for lz in 0..4u32 {
                    for ly in 0..4u32 {
                        for lx in 0..4u32 {
                            let local_id = [lx, ly, lz];
                            // `SV_GroupIndex` = `lx + ly*4 + lz*16` (HLSL
                            // group-index ordering — verified against
                            // `numthreads(4,4,4)` group-index doc).
                            let local_index = lx + ly * 4 + lz * 16;
                            // Per-thread 32 iterations × 2 voxels per iter.
                            for i in 0..32u32 {
                                let i2 = i * 2;
                                // `voxelPosInBlock = uint3(i2 % 4, (i2/4)%4, i2/16);`
                                // (`generatorModel.fx:61`).
                                let vpib =
                                    [i2 % 4, (i2 / 4) % 4, i2 / 16];
                                // `voxelPos = (groupOffsetInChunks + groupID) * 16 +
                                //              localID * 4 + voxelPosInBlock;`
                                // (`generatorModel.fx:62`).
                                let voxel_pos = [
                                    ((group_offset_in_chunks[0] + group_id[0])
                                        * CHUNK_DIM_VOXELS
                                        + local_id[0] * 4
                                        + vpib[0])
                                        as i64,
                                    ((group_offset_in_chunks[1] + group_id[1])
                                        * CHUNK_DIM_VOXELS
                                        + local_id[1] * 4
                                        + vpib[1])
                                        as i64,
                                    ((group_offset_in_chunks[2] + group_id[2])
                                        * CHUNK_DIM_VOXELS
                                        + local_id[2] * 4
                                        + vpib[2])
                                        as i64,
                                ];

                                // `voxel1 = getVoxelDataInModel(voxelPos);`
                                // `voxel2 = getVoxelDataInModel(voxelPos + uint3(1,0,0));`
                                // (`generatorModel.fx:64-65`).
                                let mut voxel1 = get_voxel_type_in_model(
                                    model,
                                    voxel_pos,
                                    size_in_voxels,
                                );
                                let mut voxel2 = get_voxel_type_in_model(
                                    model,
                                    [voxel_pos[0] + 1, voxel_pos[1], voxel_pos[2]],
                                    size_in_voxels,
                                );

                                // HLSL "full flag" application (`:67-68`):
                                //   `voxel1 |= voxel1 > 0 ? (1 << 15) : 0;`
                                if voxel1 > 0 {
                                    voxel1 |= 1 << 15;
                                }
                                if voxel2 > 0 {
                                    voxel2 |= 1 << 15;
                                }

                                // `chunk_data[group_index * 2048 + local_index * 32 + i] =
                                //   voxel1 | (voxel2 << 16);` (`generatorModel.fx:70`).
                                let dst = (group_index * CHUNK_DATA_U32S
                                    + local_index * 32
                                    + i) as usize;
                                out[dst] = voxel1 | (voxel2 << 16);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: an empty model produces all-zero output.
    #[test]
    fn empty_model_produces_zeros() {
        let model = ModelData::empty([1, 1, 1]);
        let out = generate_segment_cpu(&model, [0, 0, 0], [1, 1, 1], [16, 16, 16]);
        assert_eq!(out.len(), 2048);
        assert!(out.iter().all(|&u| u == 0));
    }

    /// `generator_model_cpu_deterministic`: the same `ModelData` + same params
    /// produces byte-identical output across two calls — no hidden state, no
    /// non-determinism (`15-design-c.md` §1.6 requires the CPU oracle to be a
    /// pure function).
    #[test]
    fn generator_model_cpu_deterministic() {
        // 2 × 1 × 2 chunk model, uniform-full of type 0x42, queried over a
        // 2×1×2 chunk segment matching the model size.
        let model = ModelData::uniform_full([2, 1, 2], 0x42);
        let segment = [2, 1, 2];
        let size_in_voxels = [32, 16, 32];

        let a = generate_segment_cpu(&model, [0, 0, 0], segment, size_in_voxels);
        let b = generate_segment_cpu(&model, [0, 0, 0], segment, size_in_voxels);

        assert_eq!(a, b, "CPU oracle must be deterministic");
        assert_eq!(a.len(), 2 * 1 * 2 * 2048);

        // Spot-check: every emitted u32 packs two type-0x42 voxels with the
        // "full" flag set (`type | (1<<15)` in both halves).
        let expected_voxel = (0x42 | (1u32 << 15)) as u32;
        let expected_packed = expected_voxel | (expected_voxel << 16);
        // Sample a few positions — every voxel is in-range and inside the
        // model's vertical extent (the Y-clamp doesn't fire).
        for &sample in &[0, 100, 1023, 2047, 4095, 8191] {
            assert_eq!(
                a[sample], expected_packed,
                "u32 @ {sample} should pack two type-0x42 voxels"
            );
        }
    }

    /// Y-clamp test: model is uniform-full of a non-zero type, but the segment
    /// spans 2 vertical chunks of model height (so the second chunk is above
    /// the model's vertical extent and must be clamped to type 0).
    #[test]
    fn generator_model_y_clamp_above_model() {
        let model = ModelData::uniform_full([1, 1, 1], 0x11);
        // Segment is 1×2×1 chunks — twice the model's Y height. The world's
        // sizeInVoxels covers the full segment so the out-of-bounds gate at
        // `:18-19` does not fire; the Y-clamp at `:48-49` must.
        let out = generate_segment_cpu(&model, [0, 0, 0], [1, 2, 1], [16, 32, 16]);
        assert_eq!(out.len(), 2 * 2048);

        // First chunk (y range 0..16): type 0x11 with the full flag.
        let expected_voxel = (0x11 | (1u32 << 15)) as u32;
        let expected_packed = expected_voxel | (expected_voxel << 16);
        for &sample in &[0, 500, 1024, 2047] {
            assert_eq!(out[sample], expected_packed);
        }
        // Second chunk (y range 16..32): Y-clamp fires → type 0 → packed
        // u32 is 0.
        for &sample in &[2048, 2500, 3072, 4095] {
            assert_eq!(out[sample], 0, "second chunk above model — Y-clamp");
        }
    }

    /// Out-of-bounds test: voxelPos >= sizeInVoxels short-circuits to type 0
    /// (`generatorModel.fx:18-19`). Verify by setting `sizeInVoxels` smaller
    /// than the segment's extent — the trailing region must be zero.
    #[test]
    fn generator_model_oob_voxels_clamp_to_zero() {
        let model = ModelData::uniform_full([1, 1, 1], 0x33);
        // 1×1×1 segment, but world `sizeInVoxels` of only 8×8×8 — half the
        // chunk in every axis is out of range.
        let out = generate_segment_cpu(&model, [0, 0, 0], [1, 1, 1], [8, 8, 8]);

        // A non-zero result must come from a voxel whose pos is fully inside
        // [0..8]^3. Re-derive which (local_index, i) tuples produce in-range
        // voxels and confirm the rest are zero.
        let mut in_range_count = 0;
        let mut out_of_range_count = 0;
        for lz in 0..4u32 {
            for ly in 0..4u32 {
                for lx in 0..4u32 {
                    let local_index = lx + ly * 4 + lz * 16;
                    for i in 0..32u32 {
                        let i2 = i * 2;
                        let vpib = [i2 % 4, (i2 / 4) % 4, i2 / 16];
                        // Each pair: voxel1 at vpos, voxel2 at vpos+(1,0,0).
                        let vx = lx * 4 + vpib[0];
                        let vy = ly * 4 + vpib[1];
                        let vz = lz * 4 + vpib[2];
                        let dst = (local_index * 32 + i) as usize;
                        let v1_in = vx < 8 && vy < 8 && vz < 8;
                        let v2_in = (vx + 1) < 8 && vy < 8 && vz < 8;
                        let got = out[dst];
                        // Model is uniform-full of type 0x33 — every in-range
                        // voxel resolves to 0x33 + full flag; every OOB voxel
                        // is clamped to type 0 (no full flag set since
                        // `type == 0`).
                        let expected_v1 = if v1_in { 0x33 | (1u32 << 15) } else { 0 };
                        let expected_v2 = if v2_in { 0x33 | (1u32 << 15) } else { 0 };
                        let expected = expected_v1 | (expected_v2 << 16);
                        assert_eq!(got, expected, "@({vx},{vy},{vz})");
                        if v1_in {
                            in_range_count += 1;
                        } else {
                            out_of_range_count += 1;
                        }
                    }
                }
            }
        }
        // Sanity: both branches were exercised.
        assert!(in_range_count > 0);
        assert!(out_of_range_count > 0);
    }

    /// Voxel-level model: a 1-chunk model whose `data_chunk` says "mixed",
    /// whose blocks/voxels store a single non-zero voxel at (0,0,0). The
    /// generator must put that voxel in the right output u32, and zero
    /// everywhere else.
    #[test]
    fn generator_model_mixed_single_voxel() {
        // Model layout:
        //   data_chunk[0]   = (2 << 30) | 0      — mixed, block base = 0
        //   data_block[0..64] — 64 blocks for the single chunk, all empty except
        //                       data_block[0]   = (2 << 30) | 0      — mixed, voxel base = 0
        //   data_voxel[0..32] — 32 packed u32s for the single mixed block, all
        //                       zero except data_voxel[0] = 0x0011 (type=0x11 at
        //                       voxel-pair-0, even slot)
        let mut data_block = vec![0u32; 64];
        data_block[0] = (2u32 << 30) | 0;
        let mut data_voxel = vec![0u32; 32];
        // Place voxel type 0x11 in the even slot of pair index 0 → voxel pair
        // covers (vx=0, vy=0, vz=0) and (vx=1, vy=0, vz=0). Even slot = voxel
        // at (0,0,0); the value is the raw type 0x11 (no full flag yet — the
        // full flag is set by the generator after the type is resolved).
        // NOTE: `data_voxel` is *post-render-encoding* (NAADF's
        // `CreateDataForRender` sets the "full" flag on the model side), so
        // the value the shader reads is `(1<<15) | 0x11`.
        data_voxel[0] = (1u32 << 15) | 0x11;

        let model = ModelData {
            data_chunk: vec![(2u32 << 30) | 0],
            data_block,
            data_voxel,
            size_in_chunks: [1, 1, 1],
        };

        let out = generate_segment_cpu(&model, [0, 0, 0], [1, 1, 1], [16, 16, 16]);

        // Voxel at (0,0,0): local_id = (0,0,0), i = 0, half = low half of
        // out[0]. Generator pulls type 0x11, marks full → low u16 =
        // (1<<15) | 0x11 = 0x8011. Voxel at (1,0,0) is the second voxel of
        // pair index 0; the model's data_voxel[0] high half is 0, so:
        //   voxel2 type = (data_voxel[0] >> 16) & 0x7FFF = 0
        //   full flag NOT applied (since type == 0).
        // → out[0] = 0x8011 | (0 << 16) = 0x8011.
        //
        // Verify by manually reading the model: the voxel @ (0,0,0) is the
        // even slot, so the GPU shader reads `modelVoxelComp & 0x7FFF` =
        // 0x8011 & 0x7FFF = 0x11. After the full-flag application, it becomes
        // (1<<15) | 0x11 = 0x8011.
        assert_eq!(out[0], 0x8011, "out[0] = packed voxel @ (0,0,0)+(1,0,0)");
    }
}
