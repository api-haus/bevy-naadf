//! Chunk / Block / Voxel cell encode & decode — the AADF bit layouts.
//!
//! Re-derived from paper §3.1–3.3, bit-matching the verified C# re-encoding
//! (`02-research.md` §1.1.2, `03-design.md` §2.2–2.3). The cells are *encoded
//! as `u32` / `u16` words in the GPU buffers*; the Rust types here are
//! encode/decode helpers + a typed view, not a struct-of-fields stored layout
//! (Q3 — idiomatic Rust, not a C# transliteration).
//!
//! Bit layout (`02-research.md` §1.1.2):
//!
//! - **Chunk / block `u32`** — bit 31 = has-children (mixed, low 30 bits =
//!   child pointer); bit 30 = uniform-full (low 15 bits = voxel type); both
//!   clear = empty (bits 0–29 = AADF). Chunk AADF: 6 × 5-bit fields at shifts
//!   `0,5,10,15,20,25`. Block AADF: 6 × 2-bit fields at shifts `0,2,4,6,8,10`.
//! - **Voxel `u16`** — bit 15 = full/empty; bits 0–14 = voxel type (full) or
//!   AADF 6 × 2-bit at shifts `0,2,4,6,8,10` (empty). Voxels are packed two per
//!   `u32` in the voxel buffer (`02-research.md` divergence #4 — see
//!   [`pack_voxels`] / [`unpack_voxel`]).
//!
//! Direction order for all 6-field AADFs is `-x, +x, -y, +y, -z, +z`.

use crate::voxel::{
    cell_state, CellRaw, VoxelTypeId, AADF_BITS_CHUNK, AADF_BITS_SMALL, AADF_MAX_CHUNK,
    AADF_MAX_SMALL, VOXEL_FULL_FLAG, VOXEL_PAYLOAD_MASK,
};

/// Index into an [`Aadf6`] for each of the 6 axis-aligned directions.
pub const DIR_NEG_X: usize = 0;
pub const DIR_POS_X: usize = 1;
pub const DIR_NEG_Y: usize = 2;
pub const DIR_POS_Y: usize = 3;
pub const DIR_NEG_Z: usize = 4;
pub const DIR_POS_Z: usize = 5;

/// All 6 cardinal directions in canonical iteration order (-x,+x,-y,+y,-z,+z).
/// Use this for any `for &dir in DIRS.iter() { … aadf.d[dir] … }` pattern;
/// **inside hot loops (`compute_aadf_layer`, `bounds_match`) the raw `usize`
/// indices are preferred to avoid potential dispatch overhead from a wrapper
/// enum**.
pub const DIRS: [usize; 6] = [
    DIR_NEG_X, DIR_POS_X, DIR_NEG_Y, DIR_POS_Y, DIR_NEG_Z, DIR_POS_Z,
];

/// 6 axis-aligned empty-distance values — the AADF of an empty cell.
///
/// Order: `-x, +x, -y, +y, -z, +z`. Each value is a cell-count distance the
/// empty cuboid extends in that direction (paper §3.3).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Aadf6 {
    /// Per-direction distances, indexed by the `DIR_*` constants.
    pub d: [u8; 6],
}

impl Aadf6 {
    /// All-zero AADF — the cuboid is just the cell itself (paper §3.3 step 1).
    pub const ZERO: Aadf6 = Aadf6 { d: [0; 6] };

    /// Pack the 6 distances into a `bits`-wide-per-field bitfield at shifts
    /// `0, bits, 2*bits, …`. Values are clamped to `max` first.
    fn pack(self, bits: u32, max: u8) -> u32 {
        let mask = (1u32 << bits) - 1;
        let mut out = 0u32;
        for (i, &v) in self.d.iter().enumerate() {
            let clamped = (v.min(max) as u32) & mask;
            out |= clamped << (bits * i as u32);
        }
        out
    }

    /// Unpack 6 distances from a `bits`-wide-per-field bitfield (inverse of
    /// [`pack`](Self::pack)).
    fn unpack(raw: u32, bits: u32) -> Aadf6 {
        let mask = (1u32 << bits) - 1;
        let mut d = [0u8; 6];
        for (i, slot) in d.iter_mut().enumerate() {
            *slot = ((raw >> (bits * i as u32)) & mask) as u8;
        }
        Aadf6 { d }
    }
}

/// Offset of a 64-block child group in the block `u32` buffer (chunk → blocks).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BlockPtr(pub u32);

/// Offset of a 64-voxel child group in the voxel buffer (block → voxels).
///
/// This is a **`u32`-element** offset: voxels are packed two per `u32`, so a
/// 64-voxel group occupies 32 consecutive `u32`s (`02-research.md` divergence
/// #4).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct VoxelPtr(pub u32);

/// A chunk-layer cell — the top of the three-layer hierarchy (paper §3.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChunkCell {
    /// No geometry — carries the AADF (5-bit fields, max distance 31).
    Empty(Aadf6),
    /// All child voxels the same type — carries the 15-bit voxel type.
    UniformFull(VoxelTypeId),
    /// Partially filled — carries a pointer to 64 consecutive blocks.
    Mixed(BlockPtr),
}

/// A block-layer cell — the middle of the three-layer hierarchy (paper §3.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockCell {
    /// No geometry — carries the AADF (2-bit fields, max distance 3).
    Empty(Aadf6),
    /// All child voxels the same type — carries the 15-bit voxel type.
    UniformFull(VoxelTypeId),
    /// Partially filled — carries a pointer to a (hash-deduplicated) group of
    /// 64 voxels.
    Mixed(VoxelPtr),
}

/// A voxel-layer cell — a single voxel (paper §3.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VoxelCell {
    /// No geometry — carries the AADF (2-bit fields, max distance 3).
    Empty(Aadf6),
    /// Solid — carries the 15-bit voxel type.
    Full(VoxelTypeId),
}

impl ChunkCell {
    /// Encode this chunk cell into its `u32` buffer word.
    ///
    /// Regime-B encoding (faithful port of C# `WorldData.cs:223` `chunk >> 30`
    /// discriminator). 2-bit state nibble at bits 30-31 +
    /// [`crate::voxel::CELL_PAYLOAD_MASK`] in low 30 bits. Mirrors WGSL
    /// `BLOCK_STATE_*` constants in `chunk_calc.wgsl`, `world_change.wgsl`,
    /// `bounds_calc.wgsl`.
    pub fn encode(self) -> u32 {
        match self {
            ChunkCell::Empty(aadf) => {
                CellRaw::new(cell_state::UNIFORM_EMPTY, aadf.pack(AADF_BITS_CHUNK, AADF_MAX_CHUNK)).0
            }
            ChunkCell::UniformFull(ty) => CellRaw::new(cell_state::UNIFORM_FULL, ty.raw() as u32).0,
            ChunkCell::Mixed(ptr) => CellRaw::new(cell_state::CHILD, ptr.0).0,
        }
    }

    /// Decode a chunk-cell `u32` buffer word — inverse of [`encode`](Self::encode).
    pub fn decode(raw: u32) -> ChunkCell {
        let cr = CellRaw(raw);
        match cr.state() {
            cell_state::CHILD => ChunkCell::Mixed(BlockPtr(cr.payload())),
            cell_state::UNIFORM_FULL => {
                ChunkCell::UniformFull(VoxelTypeId((cr.payload() as u16) & VOXEL_PAYLOAD_MASK))
            }
            _ => ChunkCell::Empty(Aadf6::unpack(raw, AADF_BITS_CHUNK)),
        }
    }
}

impl BlockCell {
    /// Encode this block cell into its `u32` buffer word.
    ///
    /// Regime-B encoding (see [`ChunkCell::encode`]). The AADF uses 2-bit
    /// fields (max distance 3) and the mixed payload is a [`VoxelPtr`].
    pub fn encode(self) -> u32 {
        match self {
            BlockCell::Empty(aadf) => {
                CellRaw::new(cell_state::UNIFORM_EMPTY, aadf.pack(AADF_BITS_SMALL, AADF_MAX_SMALL)).0
            }
            BlockCell::UniformFull(ty) => CellRaw::new(cell_state::UNIFORM_FULL, ty.raw() as u32).0,
            BlockCell::Mixed(ptr) => CellRaw::new(cell_state::CHILD, ptr.0).0,
        }
    }

    /// Decode a block-cell `u32` buffer word — inverse of [`encode`](Self::encode).
    pub fn decode(raw: u32) -> BlockCell {
        let cr = CellRaw(raw);
        match cr.state() {
            cell_state::CHILD => BlockCell::Mixed(VoxelPtr(cr.payload())),
            cell_state::UNIFORM_FULL => {
                BlockCell::UniformFull(VoxelTypeId((cr.payload() as u16) & VOXEL_PAYLOAD_MASK))
            }
            _ => BlockCell::Empty(Aadf6::unpack(raw, AADF_BITS_SMALL)),
        }
    }
}

impl VoxelCell {
    /// Encode this voxel cell into its `u16` buffer half-word.
    ///
    /// Bit 15 = full/empty; low 15 bits = voxel type (full) or 2-bit AADF
    /// (empty).
    pub fn encode(self) -> u16 {
        match self {
            VoxelCell::Empty(aadf) => {
                (aadf.pack(AADF_BITS_SMALL, AADF_MAX_SMALL) as u16) & VOXEL_PAYLOAD_MASK
            }
            VoxelCell::Full(ty) => VOXEL_FULL_FLAG | (ty.raw() & VOXEL_PAYLOAD_MASK),
        }
    }

    /// Decode a voxel-cell `u16` half-word — inverse of [`encode`](Self::encode).
    pub fn decode(raw: u16) -> VoxelCell {
        if raw & VOXEL_FULL_FLAG != 0 {
            VoxelCell::Full(VoxelTypeId(raw & VOXEL_PAYLOAD_MASK))
        } else {
            VoxelCell::Empty(Aadf6::unpack((raw & VOXEL_PAYLOAD_MASK) as u32, AADF_BITS_SMALL))
        }
    }
}

/// Pack two voxel half-words into one buffer `u32`: `voxel0 | voxel1 << 16`
/// (`02-research.md` §1.1.2, divergence #4).
pub fn pack_voxels(voxel0: u16, voxel1: u16) -> u32 {
    (voxel0 as u32) | ((voxel1 as u32) << 16)
}

/// Extract voxel half-word `i` from the packed voxel buffer.
///
/// A voxel index `i` addresses `voxels[i / 2]`, masked `>> (16 * (i & 1))`
/// (`02-research.md` divergence #4 — flagged as easy to get wrong).
pub fn unpack_voxel(voxels: &[u32], i: usize) -> u16 {
    (voxels[i / 2] >> (16 * (i & 1))) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_aadf_small() -> Aadf6 {
        // All values within the 2-bit range [0, 3].
        Aadf6 {
            d: [0, 1, 2, 3, 1, 2],
        }
    }

    fn sample_aadf_chunk() -> Aadf6 {
        // All values within the 5-bit range [0, 31].
        Aadf6 {
            d: [0, 31, 7, 16, 1, 30],
        }
    }

    #[test]
    fn chunk_empty_round_trip() {
        let cell = ChunkCell::Empty(sample_aadf_chunk());
        assert_eq!(ChunkCell::decode(cell.encode()), cell);
    }

    #[test]
    fn chunk_uniform_round_trip() {
        let cell = ChunkCell::UniformFull(VoxelTypeId(0x7FFF));
        assert_eq!(ChunkCell::decode(cell.encode()), cell);
        let cell2 = ChunkCell::UniformFull(VoxelTypeId(1));
        assert_eq!(ChunkCell::decode(cell2.encode()), cell2);
    }

    #[test]
    fn chunk_mixed_round_trip() {
        // Pointer must fit in 30 bits.
        let cell = ChunkCell::Mixed(BlockPtr(0x3FFF_FFFF));
        assert_eq!(ChunkCell::decode(cell.encode()), cell);
        let cell2 = ChunkCell::Mixed(BlockPtr(64));
        assert_eq!(ChunkCell::decode(cell2.encode()), cell2);
    }

    #[test]
    fn block_empty_round_trip() {
        let cell = BlockCell::Empty(sample_aadf_small());
        assert_eq!(BlockCell::decode(cell.encode()), cell);
    }

    #[test]
    fn block_uniform_round_trip() {
        let cell = BlockCell::UniformFull(VoxelTypeId(0x1234));
        assert_eq!(BlockCell::decode(cell.encode()), cell);
    }

    #[test]
    fn block_mixed_round_trip() {
        let cell = BlockCell::Mixed(VoxelPtr(0x3FFF_FFFF));
        assert_eq!(BlockCell::decode(cell.encode()), cell);
        let cell2 = BlockCell::Mixed(VoxelPtr(32));
        assert_eq!(BlockCell::decode(cell2.encode()), cell2);
    }

    #[test]
    fn voxel_empty_round_trip() {
        let cell = VoxelCell::Empty(sample_aadf_small());
        assert_eq!(VoxelCell::decode(cell.encode()), cell);
    }

    #[test]
    fn voxel_full_round_trip() {
        let cell = VoxelCell::Full(VoxelTypeId(0x7FFF));
        assert_eq!(VoxelCell::decode(cell.encode()), cell);
        let cell2 = VoxelCell::Full(VoxelTypeId(0));
        assert_eq!(VoxelCell::decode(cell2.encode()), cell2);
    }

    #[test]
    fn aadf_field_shifts_are_distinct() {
        // Each direction lands in its own bit field — set one direction at a
        // time and confirm decode isolates it.
        for dir in 0..6 {
            let mut a = Aadf6::ZERO;
            a.d[dir] = 3;
            let cell = BlockCell::Empty(a);
            match BlockCell::decode(cell.encode()) {
                BlockCell::Empty(out) => {
                    assert_eq!(out.d[dir], 3, "dir {dir} lost");
                    for (other, &v) in out.d.iter().enumerate() {
                        if other != dir {
                            assert_eq!(v, 0, "dir {dir} bled into dir {other}");
                        }
                    }
                }
                other => panic!("expected Empty, got {other:?}"),
            }
        }
    }

    #[test]
    fn chunk_aadf_uses_5_bit_fields() {
        // 31 is representable in a chunk AADF but would overflow a 2-bit field.
        let mut a = Aadf6::ZERO;
        a.d[DIR_POS_X] = 31;
        let cell = ChunkCell::Empty(a);
        match ChunkCell::decode(cell.encode()) {
            ChunkCell::Empty(out) => assert_eq!(out.d[DIR_POS_X], 31),
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn aadf_values_clamp_to_field_max() {
        // A block AADF field is 2 bits — a value of 7 must clamp to 3, not wrap.
        let mut a = Aadf6::ZERO;
        a.d[DIR_NEG_X] = 7;
        let cell = BlockCell::Empty(a);
        match BlockCell::decode(cell.encode()) {
            BlockCell::Empty(out) => assert_eq!(out.d[DIR_NEG_X], 3),
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn voxel_packing_round_trips() {
        let packed = pack_voxels(0xABCD, 0x1234);
        assert_eq!(packed, 0x1234_ABCD);
        let buf = [packed];
        assert_eq!(unpack_voxel(&buf, 0), 0xABCD);
        assert_eq!(unpack_voxel(&buf, 1), 0x1234);
    }

    #[test]
    fn voxel_packing_addresses_correct_word() {
        // Voxel index i addresses voxels[i / 2], half (i & 1).
        let buf = [
            pack_voxels(10, 11),
            pack_voxels(20, 21),
            pack_voxels(30, 31),
        ];
        let got: Vec<u16> = (0..6).map(|i| unpack_voxel(&buf, i)).collect();
        assert_eq!(got, vec![10, 11, 20, 21, 30, 31]);
    }
}
