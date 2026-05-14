//! CPU-side AADF construction — dense voxels → three-layer chunk/block/voxel
//! buffers with `HashMap`-based block deduplication.
//!
//! A faithful CPU re-derivation of paper Algorithm 1 (`02-research.md` §1.1.3,
//! `03-design.md` §6.1 step 2) — *not* a transliteration of `chunkCalc.fx`. The
//! GPU hashing construction's exact hash function is not needed CPU-side: a
//! Rust `HashMap` keyed on the 64-voxel array is correct and simpler
//! (`03-design.md` §6.1 step 2).
//!
//! Layer geometry (paper §3.1): `chunk = 4³ blocks = 16³ voxels`,
//! `block = 4³ voxels`, `voxel = 1`.
//!
//! Output buffer layout matches what NAADF's GPU construction would produce, so
//! the eventual GPU-construction phase is a drop-in replacement for the
//! *producer* without touching the *consumer* (the traversal shader)
//! — `03-design.md` §6.2.

use std::collections::HashMap;

use crate::aadf::bounds::{compute_aadf, CellBox};
use crate::aadf::cell::{
    pack_voxels, BlockCell, BlockPtr, ChunkCell, VoxelCell, VoxelPtr,
};
use crate::voxel::{
    VoxelTypeId, AADF_MAX_CHUNK, AADF_MAX_SMALL, CELL_CHILDREN, CELL_DIM,
};

/// Side of a chunk in voxels (`CELL_DIM² = 16`).
pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM;

/// A dense voxel volume — the input to construction. Authored by
/// `voxel::grid` (D2). Sized in whole chunks.
pub struct DenseVolume {
    /// World size in chunks, `[x, y, z]`.
    pub size_in_chunks: [u32; 3],
    /// Flat dense voxel array, `size_in_voxels.x * y * z` entries, indexed
    /// `x + y * sx + z * sx * sy` (see [`Self::voxel_index`]).
    pub voxels: Vec<VoxelTypeId>,
}

impl DenseVolume {
    /// World size in voxels, `[x, y, z]`.
    pub fn size_in_voxels(&self) -> [u32; 3] {
        [
            self.size_in_chunks[0] * CHUNK_DIM_VOXELS as u32,
            self.size_in_chunks[1] * CHUNK_DIM_VOXELS as u32,
            self.size_in_chunks[2] * CHUNK_DIM_VOXELS as u32,
        ]
    }

    /// World size in blocks, `[x, y, z]`.
    pub fn size_in_blocks(&self) -> [u32; 3] {
        [
            self.size_in_chunks[0] * CELL_DIM as u32,
            self.size_in_chunks[1] * CELL_DIM as u32,
            self.size_in_chunks[2] * CELL_DIM as u32,
        ]
    }

    /// Flat index of voxel `[x, y, z]` (x-fastest, then y, then z).
    pub fn voxel_index(&self, v: [u32; 3]) -> usize {
        let s = self.size_in_voxels();
        (v[0] + v[1] * s[0] + v[2] * s[0] * s[1]) as usize
    }

    /// The voxel type at `[x, y, z]`.
    pub fn voxel_at(&self, v: [u32; 3]) -> VoxelTypeId {
        self.voxels[self.voxel_index(v)]
    }

    /// An empty volume of `size_in_chunks` filled with [`VoxelTypeId::EMPTY`].
    pub fn empty(size_in_chunks: [u32; 3]) -> DenseVolume {
        let s = [
            size_in_chunks[0] * CHUNK_DIM_VOXELS as u32,
            size_in_chunks[1] * CHUNK_DIM_VOXELS as u32,
            size_in_chunks[2] * CHUNK_DIM_VOXELS as u32,
        ];
        DenseVolume {
            size_in_chunks,
            voxels: vec![VoxelTypeId::EMPTY; (s[0] * s[1] * s[2]) as usize],
        }
    }

    /// Set the voxel type at `[x, y, z]` (panics if out of bounds).
    pub fn set(&mut self, v: [u32; 3], ty: VoxelTypeId) {
        let i = self.voxel_index(v);
        self.voxels[i] = ty;
    }
}

/// The three-layer buffers produced by [`construct`] — bit-identical in layout
/// to what NAADF's GPU construction emits.
pub struct ConstructedWorld {
    /// Chunk buffer: one encoded [`ChunkCell`] `u32` per chunk, indexed
    /// `x + y * cx + z * cx * cy`.
    pub chunks: Vec<u32>,
    /// Block buffer: encoded [`BlockCell`] `u32`s, 64 consecutive per mixed
    /// chunk.
    pub blocks: Vec<u32>,
    /// Voxel buffer: packed voxel `u32`s (two [`VoxelCell`]s each), 32
    /// consecutive per mixed block.
    pub voxels: Vec<u32>,
    /// World size in chunks.
    pub size_in_chunks: [u32; 3],
}

/// Typed classification of a block before encoding (needs the typed form so
/// AADFs can be computed across same-chunk blocks).
#[derive(Clone, Copy)]
enum BlockClass {
    Empty,
    UniformFull(VoxelTypeId),
    /// Mixed — points at a 64-voxel group already appended to the voxel buffer.
    Mixed(VoxelPtr),
}

/// Typed classification of a chunk before encoding.
#[derive(Clone, Copy)]
enum ChunkClass {
    Empty,
    UniformFull(VoxelTypeId),
    /// Mixed — `BlockPtr` is the base of this chunk's 64-block group.
    Mixed(BlockPtr),
}

/// Build the three-layer chunk/block/voxel buffers from a dense voxel volume
/// (`03-design.md` §6.1 steps 2–3).
pub fn construct(volume: &DenseVolume) -> ConstructedWorld {
    let cx = volume.size_in_chunks[0] as usize;
    let cy = volume.size_in_chunks[1] as usize;
    let cz = volume.size_in_chunks[2] as usize;
    let bx = volume.size_in_blocks()[0] as usize;
    let by = volume.size_in_blocks()[1] as usize;
    let bz = volume.size_in_blocks()[2] as usize;

    // --- Phase 1: classify every block, with HashMap dedup of mixed blocks ---

    // Block dedup: key = the 64 voxel types of the block, value = the VoxelPtr
    // its packed voxels were appended at. The CPU stand-in for the GPU
    // BlockHashingHandler (`03-design.md` §6.1 step 2).
    let mut block_dedup: HashMap<[VoxelTypeId; CELL_CHILDREN], VoxelPtr> = HashMap::new();
    let mut voxels_buf: Vec<u32> = Vec::new();
    // block_class indexed [bx_i + by_i * bx + bz_i * bx * by].
    let mut block_class = vec![BlockClass::Empty; bx * by * bz];

    for bz_i in 0..bz {
        for by_i in 0..by {
            for bx_i in 0..bx {
                let group = gather_block_voxels(volume, [bx_i, by_i, bz_i]);
                let class = classify_block(&group, &mut block_dedup, &mut voxels_buf);
                block_class[bx_i + by_i * bx + bz_i * bx * by] = class;
            }
        }
    }

    // --- Phase 2: classify every chunk, reserving block-buffer groups ---

    let mut blocks_buf: Vec<u32> = Vec::new();
    let mut chunk_class = vec![ChunkClass::Empty; cx * cy * cz];
    // Per-chunk: the typed blocks of mixed chunks, kept for the AADF pass.
    // Indexed the same as chunk_class; `None` for non-mixed chunks.
    let mut mixed_chunk_blocks: Vec<Option<[BlockClass; CELL_CHILDREN]>> =
        vec![None; cx * cy * cz];

    for cz_i in 0..cz {
        for cy_i in 0..cy {
            for cx_i in 0..cx {
                let chunk_idx = cx_i + cy_i * cx + cz_i * cx * cy;
                let blocks = gather_chunk_blocks(&block_class, [bx, by, bz], [cx_i, cy_i, cz_i]);

                if blocks.iter().all(|b| matches!(b, BlockClass::Empty)) {
                    chunk_class[chunk_idx] = ChunkClass::Empty;
                    continue;
                }
                if let Some(ty) = uniform_chunk_type(&blocks) {
                    chunk_class[chunk_idx] = ChunkClass::UniformFull(ty);
                    continue;
                }
                // Mixed: reserve 64 consecutive block slots; fill later (AADFs
                // first need the typed blocks).
                let base = blocks_buf.len() as u32;
                blocks_buf.resize(blocks_buf.len() + CELL_CHILDREN, 0);
                chunk_class[chunk_idx] = ChunkClass::Mixed(BlockPtr(base));
                mixed_chunk_blocks[chunk_idx] = Some(blocks);
            }
        }
    }

    // --- Phase 3: AADFs + encode -------------------------------------------

    // Voxel AADFs + encode each mixed block's 64 voxels into voxels_buf.
    // (voxels_buf currently holds raw type words written by classify_block as a
    // placeholder; we overwrite each mixed group with the final encoded form.)
    // Re-walk every block: mixed blocks get their voxels encoded with AADFs.
    for bz_i in 0..bz {
        for by_i in 0..by {
            for bx_i in 0..bx {
                let class = block_class[bx_i + by_i * bx + bz_i * bx * by];
                if let BlockClass::Mixed(ptr) = class {
                    let group = gather_block_voxels(volume, [bx_i, by_i, bz_i]);
                    encode_block_voxels(&group, ptr, &mut voxels_buf);
                }
            }
        }
    }

    // Block AADFs + encode each mixed chunk's 64 blocks into blocks_buf.
    for chunk_idx in 0..(cx * cy * cz) {
        if let (ChunkClass::Mixed(base), Some(blocks)) =
            (chunk_class[chunk_idx], mixed_chunk_blocks[chunk_idx])
        {
            encode_chunk_blocks(&blocks, base, &mut blocks_buf);
        }
    }

    // Chunk AADFs + encode the chunk buffer.
    let mut chunks_buf = vec![0u32; cx * cy * cz];
    let chunk_is_empty = |c: [i32; 3]| -> bool {
        let idx = c[0] as usize + c[1] as usize * cx + c[2] as usize * cx * cy;
        matches!(chunk_class[idx], ChunkClass::Empty)
    };
    let world_bound = CellBox {
        min: [0, 0, 0],
        max: [cx as i32 - 1, cy as i32 - 1, cz as i32 - 1],
    };
    for cz_i in 0..cz {
        for cy_i in 0..cy {
            for cx_i in 0..cx {
                let chunk_idx = cx_i + cy_i * cx + cz_i * cx * cy;
                let cell = match chunk_class[chunk_idx] {
                    ChunkClass::Empty => {
                        let aadf = compute_aadf(
                            [cx_i as i32, cy_i as i32, cz_i as i32],
                            world_bound,
                            AADF_MAX_CHUNK,
                            chunk_is_empty,
                        );
                        ChunkCell::Empty(aadf)
                    }
                    ChunkClass::UniformFull(ty) => ChunkCell::UniformFull(ty),
                    ChunkClass::Mixed(ptr) => ChunkCell::Mixed(ptr),
                };
                chunks_buf[chunk_idx] = cell.encode();
            }
        }
    }

    ConstructedWorld {
        chunks: chunks_buf,
        blocks: blocks_buf,
        voxels: voxels_buf,
        size_in_chunks: volume.size_in_chunks,
    }
}

/// Gather the 64 voxel types of block `[bx, by, bz]` in child-cell order
/// (`x + y * 4 + z * 16`, x-fastest).
fn gather_block_voxels(volume: &DenseVolume, b: [usize; 3]) -> [VoxelTypeId; CELL_CHILDREN] {
    let mut out = [VoxelTypeId::EMPTY; CELL_CHILDREN];
    for lz in 0..CELL_DIM {
        for ly in 0..CELL_DIM {
            for lx in 0..CELL_DIM {
                let v = [
                    (b[0] * CELL_DIM + lx) as u32,
                    (b[1] * CELL_DIM + ly) as u32,
                    (b[2] * CELL_DIM + lz) as u32,
                ];
                out[lx + ly * CELL_DIM + lz * CELL_DIM * CELL_DIM] = volume.voxel_at(v);
            }
        }
    }
    out
}

/// Gather the 64 block classes of chunk `[cx, cy, cz]` in child-cell order.
fn gather_chunk_blocks(
    block_class: &[BlockClass],
    block_dims: [usize; 3],
    c: [usize; 3],
) -> [BlockClass; CELL_CHILDREN] {
    let [bx, by, _bz] = block_dims;
    let mut out = [BlockClass::Empty; CELL_CHILDREN];
    for lz in 0..CELL_DIM {
        for ly in 0..CELL_DIM {
            for lx in 0..CELL_DIM {
                let block = [
                    c[0] * CELL_DIM + lx,
                    c[1] * CELL_DIM + ly,
                    c[2] * CELL_DIM + lz,
                ];
                let idx = block[0] + block[1] * bx + block[2] * bx * by;
                out[lx + ly * CELL_DIM + lz * CELL_DIM * CELL_DIM] = block_class[idx];
            }
        }
    }
    out
}

/// Classify a block from its 64 voxels: all-empty → `Empty`; all the same
/// non-empty type → `UniformFull`; else dedup + append packed voxels → `Mixed`.
///
/// On a dedup *miss* the 64 voxels are appended to `voxels_buf` as raw type
/// words (a placeholder — Phase 3's [`encode_block_voxels`] overwrites them
/// with the AADF-augmented encoding).
fn classify_block(
    group: &[VoxelTypeId; CELL_CHILDREN],
    dedup: &mut HashMap<[VoxelTypeId; CELL_CHILDREN], VoxelPtr>,
    voxels_buf: &mut Vec<u32>,
) -> BlockClass {
    if group.iter().all(|v| *v == VoxelTypeId::EMPTY) {
        return BlockClass::Empty;
    }
    let first = group[0];
    if group.iter().all(|v| *v == first) {
        // first is non-empty here (the all-empty case returned above).
        return BlockClass::UniformFull(first);
    }
    // Mixed — dedup against equal voxel groups.
    if let Some(&ptr) = dedup.get(group) {
        return BlockClass::Mixed(ptr);
    }
    // Miss: append 32 placeholder u32s (64 packed voxels). VoxelPtr is a
    // u32-element offset (`02-research.md` divergence #4).
    let ptr = VoxelPtr(voxels_buf.len() as u32);
    voxels_buf.resize(voxels_buf.len() + CELL_CHILDREN / 2, 0);
    dedup.insert(*group, ptr);
    BlockClass::Mixed(ptr)
}

/// Whether all 64 blocks of a chunk are the same uniform-full type → the
/// chunk's uniform type, else `None`.
fn uniform_chunk_type(blocks: &[BlockClass; CELL_CHILDREN]) -> Option<VoxelTypeId> {
    let mut ty: Option<VoxelTypeId> = None;
    for b in blocks {
        match b {
            BlockClass::UniformFull(t) => match ty {
                None => ty = Some(*t),
                Some(prev) if prev == *t => {}
                Some(_) => return None,
            },
            _ => return None,
        }
    }
    ty
}

/// Encode the 64 voxels of one mixed block into `voxels_buf` at `ptr`, with
/// per-empty-voxel AADFs (bounded by the block's 4³ extent, max distance 3).
fn encode_block_voxels(
    group: &[VoxelTypeId; CELL_CHILDREN],
    ptr: VoxelPtr,
    voxels_buf: &mut [u32],
) {
    let local_empty = |c: [i32; 3]| -> bool {
        let idx = c[0] as usize + c[1] as usize * CELL_DIM + c[2] as usize * CELL_DIM * CELL_DIM;
        group[idx] == VoxelTypeId::EMPTY
    };
    let bound = CellBox::cube(CELL_DIM as i32);

    let mut encoded = [0u16; CELL_CHILDREN];
    for lz in 0..CELL_DIM {
        for ly in 0..CELL_DIM {
            for lx in 0..CELL_DIM {
                let i = lx + ly * CELL_DIM + lz * CELL_DIM * CELL_DIM;
                let cell = if group[i] == VoxelTypeId::EMPTY {
                    let aadf = compute_aadf(
                        [lx as i32, ly as i32, lz as i32],
                        bound,
                        AADF_MAX_SMALL,
                        local_empty,
                    );
                    VoxelCell::Empty(aadf)
                } else {
                    VoxelCell::Full(group[i])
                };
                encoded[i] = cell.encode();
            }
        }
    }
    // Pack two voxels per u32 into the buffer at ptr (u32-element offset).
    let base = ptr.0 as usize;
    for pair in 0..(CELL_CHILDREN / 2) {
        voxels_buf[base + pair] = pack_voxels(encoded[pair * 2], encoded[pair * 2 + 1]);
    }
}

/// Encode the 64 blocks of one mixed chunk into `blocks_buf` at `base`, with
/// per-empty-block AADFs (bounded by the chunk's 4³ extent, max distance 3).
fn encode_chunk_blocks(
    blocks: &[BlockClass; CELL_CHILDREN],
    base: BlockPtr,
    blocks_buf: &mut [u32],
) {
    let local_empty = |c: [i32; 3]| -> bool {
        let idx = c[0] as usize + c[1] as usize * CELL_DIM + c[2] as usize * CELL_DIM * CELL_DIM;
        matches!(blocks[idx], BlockClass::Empty)
    };
    let bound = CellBox::cube(CELL_DIM as i32);

    for lz in 0..CELL_DIM {
        for ly in 0..CELL_DIM {
            for lx in 0..CELL_DIM {
                let i = lx + ly * CELL_DIM + lz * CELL_DIM * CELL_DIM;
                let cell = match blocks[i] {
                    BlockClass::Empty => {
                        let aadf = compute_aadf(
                            [lx as i32, ly as i32, lz as i32],
                            bound,
                            AADF_MAX_SMALL,
                            local_empty,
                        );
                        BlockCell::Empty(aadf)
                    }
                    BlockClass::UniformFull(ty) => BlockCell::UniformFull(ty),
                    BlockClass::Mixed(ptr) => BlockCell::Mixed(ptr),
                };
                blocks_buf[base.0 as usize + i] = cell.encode();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aadf::cell::{unpack_voxel, DIR_POS_X, DIR_POS_Y, DIR_POS_Z};

    /// A single 1×1×1-chunk volume left entirely empty: one Empty chunk, no
    /// blocks, no voxels. The chunk's AADF is 0 in every direction (it is the
    /// whole — and only — chunk; the world bound is itself).
    #[test]
    fn all_empty_volume() {
        let volume = DenseVolume::empty([1, 1, 1]);
        let w = construct(&volume);
        assert_eq!(w.chunks.len(), 1);
        assert!(w.blocks.is_empty());
        assert!(w.voxels.is_empty());
        match ChunkCell::decode(w.chunks[0]) {
            ChunkCell::Empty(aadf) => assert_eq!(aadf.d, [0; 6]),
            other => panic!("expected Empty chunk, got {other:?}"),
        }
    }

    /// A 1×1×1-chunk volume filled solid with one type: one UniformFull chunk.
    #[test]
    fn uniform_full_volume() {
        let mut volume = DenseVolume::empty([1, 1, 1]);
        let ty = VoxelTypeId(7);
        for v in volume.voxels.iter_mut() {
            *v = ty;
        }
        let w = construct(&volume);
        assert_eq!(w.chunks.len(), 1);
        assert!(w.blocks.is_empty());
        assert!(w.voxels.is_empty());
        assert_eq!(ChunkCell::decode(w.chunks[0]), ChunkCell::UniformFull(ty));
    }

    /// A hand-checked tiny mixed volume: a single solid voxel at the origin of
    /// a 1×1×1-chunk world. Walks chunk → block → voxel and confirms the one
    /// full voxel plus a sampled empty voxel's AADF.
    #[test]
    fn single_voxel_mixed() {
        let mut volume = DenseVolume::empty([1, 1, 1]);
        let ty = VoxelTypeId(3);
        volume.set([0, 0, 0], ty);
        let w = construct(&volume);

        // Chunk 0 is mixed → points at a 64-block group.
        let chunk = ChunkCell::decode(w.chunks[0]);
        let block_base = match chunk {
            ChunkCell::Mixed(BlockPtr(base)) => base as usize,
            other => panic!("expected Mixed chunk, got {other:?}"),
        };
        assert_eq!(w.blocks.len(), CELL_CHILDREN);

        // Block 0 (contains the voxel) is mixed; the other 63 are empty.
        let block0 = BlockCell::decode(w.blocks[block_base]);
        let voxel_base = match block0 {
            BlockCell::Mixed(VoxelPtr(base)) => base as usize,
            other => panic!("expected Mixed block 0, got {other:?}"),
        };
        for i in 1..CELL_CHILDREN {
            assert!(
                matches!(BlockCell::decode(w.blocks[block_base + i]), BlockCell::Empty(_)),
                "block {i} should be empty"
            );
        }

        // Voxel 0 is full of `ty`; voxel 1 is empty.
        assert_eq!(w.voxels.len(), CELL_CHILDREN / 2);
        // voxel_base is a u32-element offset; voxel index i → voxels[base + i/2].
        let v0 = unpack_voxel(&w.voxels, voxel_base + 0);
        let v1 = unpack_voxel(&w.voxels, voxel_base + 1);
        assert_eq!(VoxelCell::decode(v0), VoxelCell::Full(ty));
        // Voxel at local (1,0,0): empty, can expand +x to 3, +y/+z to 3,
        // -x blocked (would be the full voxel at (0,0,0)).
        match VoxelCell::decode(v1) {
            VoxelCell::Empty(aadf) => {
                assert_eq!(aadf.d[DIR_POS_X], 2); // x: 1 → 3 is 2 cells
                assert_eq!(aadf.d[DIR_POS_Y], 3);
                assert_eq!(aadf.d[DIR_POS_Z], 3);
            }
            other => panic!("expected Empty voxel 1, got {other:?}"),
        }
    }

    /// Block deduplication: two identical mixed blocks share one voxel group.
    #[test]
    fn identical_blocks_dedup() {
        // 2×1×1 chunks. Put the same mixed pattern in the first block of each
        // chunk: a solid voxel at the chunk-local origin.
        let mut volume = DenseVolume::empty([2, 1, 1]);
        let ty = VoxelTypeId(5);
        volume.set([0, 0, 0], ty);
        volume.set([CHUNK_DIM_VOXELS as u32, 0, 0], ty);
        let w = construct(&volume);

        // Both chunks are mixed. Each chunk's block 0 is mixed; both block-0s
        // should resolve to the *same* VoxelPtr (dedup hit).
        let c0 = ChunkCell::decode(w.chunks[0]);
        let c1 = ChunkCell::decode(w.chunks[1]);
        let base0 = match c0 {
            ChunkCell::Mixed(BlockPtr(b)) => b as usize,
            o => panic!("chunk 0 not mixed: {o:?}"),
        };
        let base1 = match c1 {
            ChunkCell::Mixed(BlockPtr(b)) => b as usize,
            o => panic!("chunk 1 not mixed: {o:?}"),
        };
        let vp0 = match BlockCell::decode(w.blocks[base0]) {
            BlockCell::Mixed(vp) => vp,
            o => panic!("chunk 0 block 0 not mixed: {o:?}"),
        };
        let vp1 = match BlockCell::decode(w.blocks[base1]) {
            BlockCell::Mixed(vp) => vp,
            o => panic!("chunk 1 block 0 not mixed: {o:?}"),
        };
        assert_eq!(vp0, vp1, "identical blocks should dedup to one VoxelPtr");
        // Only one voxel group was appended (32 u32s).
        assert_eq!(w.voxels.len(), CELL_CHILDREN / 2);
    }

    /// Chunk-level AADF: in a 3×1×1-chunk world with only the middle chunk
    /// solid, the two empty end chunks each see distance-0 toward the solid
    /// neighbour and distance-0 toward the world edge.
    #[test]
    fn chunk_aadf_bounded_by_neighbour_and_world() {
        let mut volume = DenseVolume::empty([3, 1, 1]);
        // Fill the entire middle chunk (chunk x=1) solid.
        let ty = VoxelTypeId(9);
        for vz in 0..CHUNK_DIM_VOXELS as u32 {
            for vy in 0..CHUNK_DIM_VOXELS as u32 {
                for vx in 0..CHUNK_DIM_VOXELS as u32 {
                    volume.set([CHUNK_DIM_VOXELS as u32 + vx, vy, vz], ty);
                }
            }
        }
        let w = construct(&volume);
        assert_eq!(w.chunks.len(), 3);
        assert_eq!(ChunkCell::decode(w.chunks[1]), ChunkCell::UniformFull(ty));

        // Chunk 0 (x=0): empty. +x is blocked by the solid chunk 1 → 0.
        // -x is the world edge → 0. y/z are the world edge (size 1) → 0.
        match ChunkCell::decode(w.chunks[0]) {
            ChunkCell::Empty(aadf) => {
                assert_eq!(aadf.d[DIR_POS_X], 0, "+x blocked by solid chunk 1");
                assert_eq!(aadf.d, [0; 6]);
            }
            o => panic!("chunk 0 not empty: {o:?}"),
        }
    }
}
