//! Phase-C W4 — entity-track CPU helpers (`15-design-c.md` §2.1 W4 row, §3.6).
//!
//! - [`compress_quaternion`] / [`decompress_quaternion`] — smallest-three
//!   quaternion encoding (port of NAADF's `Common/Helper.cs` /
//!   `commonRayTracing.fxh::compressQuaternion`).
//! - [`EntityData`] — per-entity AADF voxel volume builder (port of
//!   `EntityData.cs:15-107`). Each entity owns a small voxel volume (typically
//!   8³ or 16³) with its own AADFs computed via the same 31-iteration
//!   per-axis sweep `EntityData.cs:64-105` runs in C#. Faithful port of the
//!   C# inline loop — not delegated to [`crate::aadf::bounds::compute_aadf`],
//!   because the C# `EntityData` AADF kernel runs on a packed-32-bit voxel
//!   buffer (the AADFs in the low 30 bits, the `0x80000000` full-cell flag in
//!   the top bit) that does not match the `aadf::bounds` 5-bit-AADF chunk
//!   format. Both kernels implement paper §3.3 synchronised-iteration
//!   neighbour-merge, just over different bit layouts.
//! - [`compress_entity_chunk_instance`] — pack an `EntityInstance` into the
//!   GPU's 20-byte `GpuEntityChunkInstance` (port of `EntityHandler.cs:325-329`).
//! - [`compress_entity_history`] — pack an `EntityInstance` into the GPU's
//!   16-byte `GpuEntityInstanceHistory` (port of `EntityHandler.cs:367-371`).
//!
//! The entity-handler per-frame logic (overlap counting + prefix-sum +
//! dedup-hash) lives in [`crate::render::construction::entity_handler`]; this
//! module is the **algorithm** layer (CPU, GPU-format-agnostic).

use crate::render::gpu_types::{
    EntityInstance, GpuChunkUpdate, GpuEntityChunkInstance, GpuEntityInstanceHistory,
};

/// The full-cell flag bit in `EntityData::voxels`. Mirrors C# constant
/// `0x80000000` (`EntityData.cs:71, :84, :97, :112`).
pub const ENTITY_VOXEL_FULL_FLAG: u32 = 0x8000_0000;

/// 6-direction masks for the §3.3 neighbour-merge `addBounds` predicate
/// (`EntityData.cs:58-63`). Each bit `b` means "the bound in direction `b`
/// matches the neighbour's bound". The mask excludes the direction pointing
/// back toward us. Order: `-x, +x, -y, +y, -z, +z` (same canonical iteration
/// order as [`crate::aadf::cell::DIRS`]).
const ENTITY_AADF_MASKS: [u32; 6] = [
    0x3D, // -X: 0b111101 — drop the -X bit
    0x3E, // +X: 0b111110 — drop the +X bit
    0x37, // -Y: 0b110111 — drop the -Y bit
    0x3B, // +Y: 0b111011 — drop the +Y bit
    0x1F, // -Z: 0b011111 — drop the -Z bit
    0x2F, // +Z: 0b101111 — drop the +Z bit
];

/// Bit-shifts for the 5-bit-per-axis 6-direction AADF inside an
/// `EntityData::voxels` u32. C# `EntityData.cs:71,73,77,79,83,85` `<< 0..25`.
/// Order matches [`ENTITY_AADF_MASKS`] (`-x, +x, -y, +y, -z, +z`).
const ENTITY_AADF_BIT_SHIFTS: [u32; 6] = [0, 5, 10, 15, 20, 25];

/// Compress a quaternion via the smallest-three encoding
/// (port of `EntityHandler.cs:499-546` /
/// `commonRayTracing.fxh:163-200`).
///
/// Picks the axis with the largest absolute component, encodes the remaining
/// three components as 14-bit signed fixed-point in `[-1, 1]`, and packs the
/// dropped-component index into the top 2 bits of the second u32. The
/// "smallest-three" coding produces ~33 bits of precision per quaternion vs.
/// the naive 128-bit float storage.
///
/// Returns `(data1, data2)` — packed:
///   data1 = small.x (14 bits) | (small.y << 14) (14 bits)
///         | (small.z & 0xF) << 28 (low 4 bits of z in the top of data1)
///   data2 = (small.z >> 4) (10 bits)
///         | (max_index & 3) << 10
///
/// The C# code flips signs of the remaining components when the dropped
/// component is negative — this preserves the canonical `q ~ -q` ambiguity
/// (the dropped component is always non-negative in the reconstructed form).
pub fn compress_quaternion(q: [f32; 4]) -> (u32, u32) {
    // Find max-abs component.
    let abs = [q[0].abs(), q[1].abs(), q[2].abs(), q[3].abs()];
    let mut max_index = 0usize;
    let mut max_abs = abs[0];
    for i in 1..4 {
        if abs[i] > max_abs {
            max_abs = abs[i];
            max_index = i;
        }
    }
    let is_neg = q[max_index] < 0.0;

    // Pick the other three components in order.
    let mut small = [0.0f32; 3];
    let mut s_idx = 0;
    for i in 0..4 {
        if i == max_index {
            continue;
        }
        small[s_idx] = q[i];
        s_idx += 1;
    }
    if is_neg {
        small[0] = -small[0];
        small[1] = -small[1];
        small[2] = -small[2];
    }

    // Map [-1, 1] → [0, 16383]. The C# uses `(small + 1) * 8192 + 0.5`, then
    // clamps to `[0, 16383]` (14-bit range). Faithful port.
    let s0 = ((small[0] + 1.0) * 8192.0 + 0.5).clamp(0.0, 16383.0) as u32;
    let s1 = ((small[1] + 1.0) * 8192.0 + 0.5).clamp(0.0, 16383.0) as u32;
    let s2 = ((small[2] + 1.0) * 8192.0 + 0.5).clamp(0.0, 16383.0) as u32;

    let data1 = s0 | (s1 << 14) | ((s2 & 0xF) << 28);
    let data2 = (s2 >> 4) | (((max_index as u32) & 0x3) << 10);
    (data1, data2)
}

/// Decompress the smallest-three encoding produced by [`compress_quaternion`].
///
/// Mirrors `commonRayTracing.fxh::decompressQuaternion` (the shader-side
/// inverse). Used by tests + the renderer-side helpers.
pub fn decompress_quaternion(packed: (u32, u32)) -> [f32; 4] {
    let (d1, d2) = packed;
    let max_index = ((d2 >> 10) & 0x3) as usize;
    let s0 = (d1 & 0x3FFF) as i32;
    let s1 = ((d1 >> 14) & 0x3FFF) as i32;
    let s2 = ((d1 >> 28) | ((d2 & 0x3FF) << 4)) as i32;
    let small = [
        (s0 - 8192) as f32 / 8192.0,
        (s1 - 8192) as f32 / 8192.0,
        (s2 - 8192) as f32 / 8192.0,
    ];
    let missing = (1.0 - (small[0] * small[0] + small[1] * small[1] + small[2] * small[2]))
        .max(0.0)
        .sqrt();
    match max_index {
        0 => [missing, small[0], small[1], small[2]],
        1 => [small[0], missing, small[1], small[2]],
        2 => [small[0], small[1], missing, small[2]],
        _ => [small[0], small[1], small[2], missing],
    }
}

/// W4 §2.1 — per-entity voxel volume + AADFs (port of `EntityData.cs`).
///
/// Each entity has a small voxel volume sized `size.x × size.y × size.z`
/// (typically 8³ or 16³). The `voxels` buffer stores one u32 per voxel:
///   - bit 31 = full-cell flag ([`ENTITY_VOXEL_FULL_FLAG`])
///   - bits 0..30 = AADFs (5 bits per axis × 6 axes = 30 bits) for empty cells
///     OR voxel type for full cells.
///
/// `compute` builds both:
/// 1. Walk `size.x*y*z` voxels, marking full cells from the type buffer.
/// 2. Run the 31-iteration synchronised-iteration neighbour-merge per-axis to
///    populate AADFs.
#[derive(Debug, Clone)]
pub struct EntityData {
    pub size: [u32; 3],
    pub voxels: Vec<u32>,
}

impl EntityData {
    /// Build an `EntityData` from a dense type-id source. `types[i]` is the
    /// type for voxel `i` in flat (`x + y*sx + z*sx*sy`) order; `0` is empty.
    ///
    /// AADFs are computed using the same 6-bit-mask synchronised iteration
    /// `EntityData.cs:64-106` runs: 31 outer iterations, each iteration
    /// sweeps all voxels per-axis (X, Y, Z), expanding empty cells'
    /// neighbour-merged distance fields by one step.
    pub fn from_types(size: [u32; 3], types: &[u32]) -> Self {
        let voxel_count = (size[0] * size[1] * size[2]) as usize;
        assert_eq!(types.len(), voxel_count, "types buffer size mismatch");
        let mut voxels: Vec<u32> = Vec::with_capacity(voxel_count);
        for &t in types {
            if t != 0 {
                voxels.push(ENTITY_VOXEL_FULL_FLAG | (t & 0x7FFF_FFFF));
            } else {
                voxels.push(0);
            }
        }

        let sx = size[0] as i32;
        let sy = size[1] as i32;
        let sz = size[2] as i32;

        for _iter in 0..31 {
            // X pass.
            for v in 0..voxel_count {
                let x = (v as i32) % sx;
                let cur = voxels[v];
                if (cur & ENTITY_VOXEL_FULL_FLAG) != 0 {
                    continue;
                }
                let mut updated = cur;
                if x > 0 {
                    add_bounds(
                        &voxels,
                        v,
                        ENTITY_AADF_MASKS[0],
                        -1,
                        ENTITY_AADF_BIT_SHIFTS[0],
                        &mut updated,
                    );
                }
                if x + 1 < sx {
                    add_bounds(
                        &voxels,
                        v,
                        ENTITY_AADF_MASKS[1],
                        1,
                        ENTITY_AADF_BIT_SHIFTS[1],
                        &mut updated,
                    );
                }
                voxels[v] = updated;
            }
            // Y pass.
            for v in 0..voxel_count {
                let y = ((v as i32) / sx) % sy;
                let cur = voxels[v];
                if (cur & ENTITY_VOXEL_FULL_FLAG) != 0 {
                    continue;
                }
                let mut updated = cur;
                if y > 0 {
                    add_bounds(
                        &voxels,
                        v,
                        ENTITY_AADF_MASKS[2],
                        -sx,
                        ENTITY_AADF_BIT_SHIFTS[2],
                        &mut updated,
                    );
                }
                if y + 1 < sy {
                    add_bounds(
                        &voxels,
                        v,
                        ENTITY_AADF_MASKS[3],
                        sx,
                        ENTITY_AADF_BIT_SHIFTS[3],
                        &mut updated,
                    );
                }
                voxels[v] = updated;
            }
            // Z pass.
            for v in 0..voxel_count {
                let z = ((v as i32) / (sx * sy)) % sz;
                let cur = voxels[v];
                if (cur & ENTITY_VOXEL_FULL_FLAG) != 0 {
                    continue;
                }
                let mut updated = cur;
                if z > 0 {
                    add_bounds(
                        &voxels,
                        v,
                        ENTITY_AADF_MASKS[4],
                        -(sx * sy),
                        ENTITY_AADF_BIT_SHIFTS[4],
                        &mut updated,
                    );
                }
                if z + 1 < sz {
                    add_bounds(
                        &voxels,
                        v,
                        ENTITY_AADF_MASKS[5],
                        sx * sy,
                        ENTITY_AADF_BIT_SHIFTS[5],
                        &mut updated,
                    );
                }
                voxels[v] = updated;
            }
        }

        Self { size, voxels }
    }

    /// World-space size of a single voxel — useful for entity-AABB bookkeeping.
    pub fn voxel_count(&self) -> usize {
        (self.size[0] * self.size[1] * self.size[2]) as usize
    }
}

/// `EntityData.cs:109-117` — single-direction AADF expansion.
fn add_bounds(
    voxels: &[u32],
    cur_index: usize,
    mask: u32,
    direction_offset: i32,
    bounds_location: u32,
    cur_voxel: &mut u32,
) {
    let neighbour_idx = (cur_index as i32 + direction_offset) as usize;
    let neighbour = voxels[neighbour_idx];
    if (neighbour & ENTITY_VOXEL_FULL_FLAG) == 0 {
        if (check_matching_bound_cell(neighbour, *cur_voxel) & mask) == mask {
            *cur_voxel = cur_voxel.wrapping_add(1u32 << bounds_location);
        }
    }
}

/// `EntityData.cs:119-130` — 6-bit "neighbour's bound ≥ ours" mask.
fn check_matching_bound_cell(neighbour: u32, cur_voxel: u32) -> u32 {
    let mut mask = 0u32;
    let n0 = (neighbour >> 0) & 0x1F;
    let c0 = (cur_voxel >> 0) & 0x1F;
    mask |= ((n0 >= c0) as u32) << 0;
    let n1 = (neighbour >> 5) & 0x1F;
    let c1 = (cur_voxel >> 5) & 0x1F;
    mask |= ((n1 >= c1) as u32) << 1;
    let n2 = (neighbour >> 10) & 0x1F;
    let c2 = (cur_voxel >> 10) & 0x1F;
    mask |= ((n2 >= c2) as u32) << 2;
    let n3 = (neighbour >> 15) & 0x1F;
    let c3 = (cur_voxel >> 15) & 0x1F;
    mask |= ((n3 >= c3) as u32) << 3;
    let n4 = (neighbour >> 20) & 0x1F;
    let c4 = (cur_voxel >> 20) & 0x1F;
    mask |= ((n4 >= c4) as u32) << 4;
    let n5 = (neighbour >> 25) & 0x1F;
    let c5 = (cur_voxel >> 25) & 0x1F;
    mask |= ((n5 >= c5) as u32) << 5;
    mask
}

/// W4 — pack an [`EntityInstance`] into the 20-byte
/// [`GpuEntityChunkInstance`] layout (port of `EntityHandler.cs:325-329`).
///
/// The quaternion field is the **inverse** of the instance's rotation
/// (`EntityHandler.cs:322` — `Quaternion.Inverse(...)`); the renderer uses
/// the inverse to bring world-space rays into entity-local space.
pub fn compress_entity_chunk_instance(instance: &EntityInstance) -> GpuEntityChunkInstance {
    // Position is in voxels; the GPU stores it as 21-bit fixed-point at 1/128
    // resolution per axis. `Point3.FromVector3(position * 128)` in C# is
    // `f32 → i32` (cast-truncate, matches Rust's `as i32`).
    let pos_comp_x = (instance.position.x * 128.0) as u32;
    let pos_comp_y = (instance.position.y * 128.0) as u32;
    let pos_comp_z = (instance.position.z * 128.0) as u32;
    // Invert the quaternion for storage (renderer applies the rotation to
    // bring rays into entity-local space).
    let q = instance.quaternion;
    let len_sq = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    let inv = if len_sq > 0.0 {
        [-q[0] / len_sq, -q[1] / len_sq, -q[2] / len_sq, q[3] / len_sq]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    let (qc1, qc2) = compress_quaternion(inv);
    let size = instance.size;
    let data1 = pos_comp_x | ((pos_comp_y & 0x7FF) << 21);
    let data2 = pos_comp_z | ((pos_comp_y >> 11) << 21) | ((size[2] >> 4) << 29);
    let data3 = qc1;
    let data4 = qc2 | (instance.voxel_start << 12);
    let data5 = instance.entity
        | (size[0] << 14)
        | (size[1] << 21)
        | ((size[2] & 0xF) << 28);
    GpuEntityChunkInstance {
        data1,
        data2,
        data3,
        data4,
        data5,
    }
}

/// W4 — pack an [`EntityInstance`] into the 16-byte
/// [`GpuEntityInstanceHistory`] layout (port of `EntityHandler.cs:367-371`).
///
/// Unlike [`compress_entity_chunk_instance`], the history form stores the
/// **non-inverted** quaternion (the renderer's TAA reprojection wants the
/// forward rotation to project old positions into world space).
pub fn compress_entity_history(instance: &EntityInstance) -> GpuEntityInstanceHistory {
    let pos_comp_x = (instance.position.x * 128.0) as u32;
    let pos_comp_y = (instance.position.y * 128.0) as u32;
    let pos_comp_z = (instance.position.z * 128.0) as u32;
    let (qc1, qc2) = compress_quaternion(instance.quaternion);
    GpuEntityInstanceHistory {
        data1: pos_comp_x | ((pos_comp_y & 0x7FF) << 21),
        data2: pos_comp_z | ((pos_comp_y >> 11) << 21),
        data3: qc1,
        data4: qc2,
    }
}

/// W4 — pack a chunk-position + entity-pointer pair into the 8-byte
/// [`GpuChunkUpdate`] layout (port of `EntityHandler.cs:338-341`).
pub fn pack_chunk_update(chunk_pos: [u32; 3], entity_pointer_and_size: u32) -> GpuChunkUpdate {
    GpuChunkUpdate {
        data1: chunk_pos[0] | (chunk_pos[1] << 11) | (chunk_pos[2] << 21),
        data2: entity_pointer_and_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quaternion smallest-three encode/decode round-trips within tight error
    /// tolerance. The encoding loses ~14 bits of precision per component
    /// (after the absorbed-sign + dropped-component); within `[−1, 1]` the
    /// quantisation step is `1/8192 ≈ 1.22e-4`, so a tolerance of 5e-4 covers
    /// it comfortably.
    #[test]
    fn compress_quaternion_roundtrip() {
        // Identity.
        let id = [0.0, 0.0, 0.0, 1.0];
        let packed = compress_quaternion(id);
        let out = decompress_quaternion(packed);
        assert!((out[0] - id[0]).abs() < 5e-4, "identity x");
        assert!((out[1] - id[1]).abs() < 5e-4, "identity y");
        assert!((out[2] - id[2]).abs() < 5e-4, "identity z");
        assert!((out[3] - id[3]).abs() < 5e-4, "identity w");

        // 90° around Y: (0, sin(45°), 0, cos(45°)) = (0, sqrt(0.5), 0, sqrt(0.5)).
        let s = (0.5f32).sqrt();
        let q = [0.0, s, 0.0, s];
        let packed = compress_quaternion(q);
        let out = decompress_quaternion(packed);
        // The decompressed form may be either `(0, s, 0, s)` or its negation;
        // check the absolute components agree.
        for i in 0..4 {
            let d = (out[i].abs() - q[i].abs()).abs();
            assert!(d < 5e-4, "90Y component {i}: got {} expected {}", out[i], q[i]);
        }

        // A random-ish unit quaternion.
        let q: [f32; 4] = [0.4, -0.3, 0.5, 0.7071];
        // Normalize.
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        let q = [q[0] / n, q[1] / n, q[2] / n, q[3] / n];
        let packed = compress_quaternion(q);
        let out = decompress_quaternion(packed);
        // The encoding may negate the entire quaternion (q ~ -q): test both
        // possibilities.
        let same = (out[0] - q[0]).abs() < 5e-4
            && (out[1] - q[1]).abs() < 5e-4
            && (out[2] - q[2]).abs() < 5e-4
            && (out[3] - q[3]).abs() < 5e-4;
        let negated = (out[0] + q[0]).abs() < 5e-4
            && (out[1] + q[1]).abs() < 5e-4
            && (out[2] + q[2]).abs() < 5e-4
            && (out[3] + q[3]).abs() < 5e-4;
        assert!(same || negated, "random q roundtrip: got {:?} expected {:?}", out, q);
    }

    /// The smallest-three encoding uses 32 bits in `data1` + 12 bits in
    /// `data2` (the low 10 of z and the 2-bit max-index). Any extra bits in
    /// `data2` mean the packing is wrong.
    #[test]
    fn compress_quaternion_bit_layout() {
        // A small non-axis quaternion so max_index = 3 (w is biggest).
        let q: [f32; 4] = [0.1, 0.2, 0.3, 0.9];
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        let q = [q[0] / n, q[1] / n, q[2] / n, q[3] / n];
        let (_d1, d2) = compress_quaternion(q);
        // d2's top bits beyond bit 11 should be zero (10 bits of z + 2 bits
        // of max_index).
        assert_eq!(d2 & !0xFFF, 0, "data2 top bits non-zero: {:#x}", d2);
        // max_index lives in bits 10-11 of d2.
        let max_index = (d2 >> 10) & 0x3;
        assert_eq!(max_index, 3, "max_index should be 3 (w largest)");
    }

    /// Per-entity AADF voxel volume — a 2×2×2 volume with one full voxel at
    /// the origin should produce 7 empty voxels with AADFs filled in.
    #[test]
    fn entity_data_cpu_aadf_correctness() {
        let types = vec![1u32, 0, 0, 0, 0, 0, 0, 0]; // (0,0,0) full only
        let entity = EntityData::from_types([2, 2, 2], &types);
        assert_eq!(entity.voxels.len(), 8);
        // (0,0,0): full
        assert_ne!(entity.voxels[0] & ENTITY_VOXEL_FULL_FLAG, 0);
        // (1,0,0): empty, has a -X neighbour (full at (0,0,0)).
        assert_eq!(entity.voxels[1] & ENTITY_VOXEL_FULL_FLAG, 0);
        // The remaining 6 empty cells should have AADFs reflecting neighbour
        // distances. The exact values depend on the iteration count; we just
        // assert "not all zero" (the AADFs converged).
        let any_aadf_set = (1..8).any(|i| entity.voxels[i] != 0);
        assert!(any_aadf_set, "expected non-zero AADFs in at least one empty voxel");
    }

    /// `compress_entity_chunk_instance` packs all five u32s with the correct
    /// bit fields.
    #[test]
    fn compress_entity_chunk_instance_packs_fields() {
        let instance = EntityInstance {
            position: bevy::math::Vec3::new(1.0, 2.0, 3.0),
            quaternion: [0.0, 0.0, 0.0, 1.0], // identity
            voxel_start: 5,
            entity: 7,
            size: [4, 8, 16],
        };
        let packed = compress_entity_chunk_instance(&instance);
        // posComp = position * 128 = (128, 256, 384).
        // data1 = posX (128) | (posY & 0x7FF) << 21 = 128 | (256 << 21).
        assert_eq!(packed.data1, 128 | (256u32 << 21));
        // data2 = posZ (384) | (posY >> 11) << 21 | (sizeZ >> 4) << 29
        //       = 384 | (0 << 21) | (1 << 29).
        assert_eq!(packed.data2, 384 | (1u32 << 29));
        // data5: entity (7) | sizeX (4) << 14 | sizeY (8) << 21 | (sizeZ & 0xF) (0) << 28
        //      = 7 | (4 << 14) | (8 << 21) | (0 << 28).
        assert_eq!(packed.data5, 7 | (4u32 << 14) | (8u32 << 21));
    }
}
