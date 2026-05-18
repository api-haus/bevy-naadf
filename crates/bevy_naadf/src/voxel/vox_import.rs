//! MagicaVoxel `.vox` ingestion — Track A v2 (sparse), per
//! `docs/orchestrate/feature-completeness/02a-v2-sparse-vox-ingestion.md`.
//!
//! Parses a `.vox` file into a [`ConstructedWorld`] (the renderer-input shape
//! produced by [`crate::aadf::construct::construct`]) + a [`VoxelType`]
//! palette. Walks scene-graph–composed sparse XYZI records directly into
//! NAADF's 3-buffer encoding (`chunks_cpu` / `blocks_cpu` / `voxels_cpu`)
//! without ever materialising a dense intermediate — so very large composed
//! worlds (Oasis_Hard_Cover.vox-class, 93×34×84 chunks ≈ 50M voxels at ~1%
//! density) load in ~tens of MiB rather than the ~140 GiB a dense
//! `Vec<VoxelTypeId>` would need.
//!
//! ## Pipeline shape (mirrors C# `MagicaVoxel.cs` + `ModelData.ImportFromVox`)
//!
//! 1. `dot_vox::load_bytes(&[u8])` → `DotVoxData` (sparse `Vec<Voxel>` per model).
//! 2. Two-pass scene-graph walk:
//!    - Pass 1 — [`accumulate_world_aabb`] composes parent `nTRN` transforms
//!      (translation `_t` + rotation `_r` byte → signed-permutation matrix)
//!      and unions every shape's transformed AABB into a world AABB. Mirrors
//!      C# `MagicaVoxel.GetWorldAABB` at `MagicaVoxel.cs:651-716`.
//!    - Pass 2 — [`collate_voxels_sparse`] re-walks the same scene graph and
//!      pushes every transformed voxel into a per-chunk sparse bucket
//!      ([`ChunkBuckets`]). Mirrors C# `MagicaVoxel.CollateVoxelData`.
//! 3. [`build_constructed_world_sparse`] walks every non-empty chunk, lifts
//!    its 64 sparse blocks into a 16³ transient dense array (8 KiB, discarded
//!    per-chunk), classifies + dedups + encodes via the helpers from
//!    [`crate::aadf::construct`], and emits `chunks_cpu`/`blocks_cpu`/
//!    `voxels_cpu` byte-equivalent to what `construct(&DenseVolume)` would
//!    produce on the same input (Test #15 enforces this).
//! 4. [`vox_palette_to_voxel_types`] promotes the 256-entry `RGBA` palette +
//!    `MATL` chunks into a `Vec<VoxelType>` — UNCHANGED from v1.
//! 5. The voxel-coordinate convention is swapped: MagicaVoxel `(x, y, z)` →
//!    NAADF `(x, z, y)`. Matches C# `ModelData.cs:386` + `:438`.
//!
//! ## What's out of scope
//!
//! - `obj2voxel` integration (deferred entirely per `01-context.md` §5).
//! - `.vl32` import (Track A is `.vox` only).
//! - Bevy `AssetLoader` registration / hot-reload — synchronous `std::fs::read`
//!   at `Startup` only.
//! - Pre-bake to a port-native binary format — orthogonal track.
//!
//! ## Δ-decisions honoured (per v2 design)
//!
//! - Δ-ModelData — emit `ConstructedWorld` directly, no `ModelData` intermediate.
//! - Δ-AADF — CPU AADFs computed per-chunk inline via `compute_aadf_layer`.
//! - Δ-Hash — block dedup uses `HashMap<[VoxelTypeId; 64], VoxelPtr>` to match
//!   [`crate::aadf::construct::construct`]'s output byte-for-byte.
//! - Δ-DenseFallback — `GridPreset::Default` still uses `DenseVolume + construct()`;
//!   only `.vox` ingestion is sparse-only.
//! - Δ-GPUProducer — `WorldData::dense_voxel_types = Vec::new()` for `.vox`
//!   content; the data-driven gate at `render/construction/mod.rs:833-835`
//!   skips the GPU producer (the renderer reads the pre-built CPU buffers).
//! - Δ-CapsConservative — pre-flight against documented wgpu Vulkan-baseline
//!   minimums (1024 chunks/axis, 256 MiB buffer ceilings); much higher than v1.

use std::collections::HashMap;
use std::path::Path;

use bevy::math::Vec3;
use thiserror::Error;

use crate::aadf::bounds::compute_aadf_layer;
use crate::aadf::cell::{BlockPtr, ChunkCell, VoxelPtr};
use crate::aadf::construct::{
    encode_block_voxels, encode_chunk_blocks, BlockClass, ChunkClass, ConstructedWorld,
};
use crate::voxel::{
    MaterialBase, MaterialLayer, VoxelType, VoxelTypeId, AADF_MAX_CHUNK, CELL_CHILDREN, CELL_DIM,
};

/// Side of a chunk in voxels (= `CELL_DIM² = 16`).
const CHUNK_DIM_VOXELS: u32 = (CELL_DIM as u32) * (CELL_DIM as u32);

/// Max chunks-per-axis the loader will accept (per-axis cap on
/// `size_in_chunks`).
///
/// Δ-CapsConservative — matches the wgpu Vulkan-baseline
/// `max_texture_dimension_3d` (the `chunks` 3D texture's hard ceiling). The
/// real desktop cap on NVIDIA/AMD is typically `2048`; we conservatively
/// pre-flight at `1024` because the loader runs at `Startup`, before the
/// render-app exists, so we can't query the actual device limits.
///
/// A 1024³-chunk world is 16384³ voxels = ~4.4 trillion voxels — past any
/// practical `.vox` fixture (Oasis_Hard_Cover.vox composes to ~93³ chunks ≈
/// 0.09% of this cap).
pub const MAX_CHUNKS_PER_AXIS: u32 = 1024;

/// Pre-flight cap on the `voxels_cpu` (unique-blocks) buffer size, in bytes.
/// Matches the wgpu Vulkan-baseline `max_buffer_size = 256 MiB`. Desktop
/// `max_buffer_size` is typically 2 GiB (8× headroom).
///
/// Each unique mixed block occupies 32 `u32`s (128 B); at 256 MiB the cap is
/// 2M unique blocks ≈ 128M unique voxels worth of geometry, which on a 1%
/// non-empty world is ~12G total voxels = ~480³ NAADF voxels.
pub const MAX_VOXELS_BUFFER_BYTES: u64 = 256 * 1024 * 1024;

/// Pre-flight cap on the `blocks_cpu` buffer size, in bytes. Same rationale
/// as [`MAX_VOXELS_BUFFER_BYTES`].
pub const MAX_BLOCKS_BUFFER_BYTES: u64 = 256 * 1024 * 1024;

/// Parsed `.vox` data, ready to install into a NAADF world.
///
/// Produced by [`parse_vox_bytes`] / [`load_vox`]. Consumed by
/// [`build_world_from_vox`] (or any caller that wants to drop the
/// `ConstructedWorld` into a custom installer).
///
/// **v2 shape (`02a-v2-sparse-vox-ingestion.md`):** v1's `volume: DenseVolume`
/// is retired; the sparse walk produces the final `(chunks, blocks, voxels)`
/// buffers directly, so the renderer can install them without going through
/// `aadf::construct::construct()`.
#[derive(Debug)]
pub struct ImportedVox {
    /// The pre-built constructed world — `chunks`/`blocks`/`voxels` `u32`
    /// buffers with AADFs baked, plus `size_in_chunks`. Byte-equivalent to
    /// what `aadf::construct::construct(&DenseVolume)` would produce on the
    /// same input (Test #15 enforces this).
    pub world: ConstructedWorld,
    /// The voxel-type palette derived from the `.vox` `RGBA` + `MATL`
    /// chunks. Index 0 is the reserved empty placeholder; indices 1..=N
    /// mirror the MagicaVoxel palette entries 0..N-1 (the `+1` shift keeps
    /// slot 0 empty per NAADF convention).
    pub palette: Vec<VoxelType>,
}

/// Errors emitted by [`parse_vox_bytes`] / [`load_vox`].
#[derive(Debug, Error)]
pub enum VoxImportError {
    /// `dot_vox` rejected the bytes as a malformed `.vox` file.
    #[error("dot_vox parse failed: {0}")]
    Parse(&'static str),
    /// `std::fs::read` failed (file not found, permission denied, …).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The composed world exceeds [`MAX_CHUNKS_PER_AXIS`] on at least one
    /// axis. Mirrors the wgpu Vulkan-baseline `max_texture_dimension_3d` for
    /// the `chunks` 3D texture. Files past this cap need
    /// streaming / pre-baked / segment-iteration follow-up work.
    #[error("VOX size {dim:?} chunks per axis exceeds soft-cap ({limit} per axis); past wgpu Vulkan-baseline max_texture_dimension_3d. Pre-bake or shrink the .vox file")]
    SizeExceedsTextureLimit { dim: [u32; 3], limit: u32 },
    /// The sparse build emitted a `blocks_cpu` / `voxels_cpu` buffer past the
    /// wgpu Vulkan-baseline `max_buffer_size` pre-flight cap
    /// ([`MAX_VOXELS_BUFFER_BYTES`] / [`MAX_BLOCKS_BUFFER_BYTES`]).
    #[error("VOX produced a {dim:?}-u32 buffer exceeding the {bytes}-byte wgpu pre-flight cap")]
    SizeExceedsBudget { dim: [u32; 3], bytes: u64 },
    /// `dot_vox` parsed a file with `models.is_empty()`.
    #[error("VOX contains no models")]
    Empty,
}

/// Parse `.vox` bytes into an [`ImportedVox`].
///
/// Pure CPU, no Bevy resources, no filesystem — the unit-testable entry point.
pub fn parse_vox_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxImportError> {
    let data = dot_vox::load_bytes(bytes).map_err(VoxImportError::Parse)?;
    parse_dot_vox_data(&data)
}

/// Parse `.vox` bytes into an [`ImportedVox`] with `tiles × tiles` XZ tiling
/// applied. `tiles == 1` is equivalent to [`parse_vox_bytes`].
pub fn parse_vox_bytes_tiled(bytes: &[u8], tiles: u32) -> Result<ImportedVox, VoxImportError> {
    let data = dot_vox::load_bytes(bytes).map_err(VoxImportError::Parse)?;
    parse_dot_vox_data_tiled(&data, tiles)
}

/// Convenience: load a `.vox` file from disk via `std::fs::read` + parse.
///
/// Used by `voxel/grid.rs::setup_test_grid` when `args.grid_preset` is
/// [`crate::GridPreset::Vox`]. On error the caller logs + falls back to the
/// hard-coded test grid.
pub fn load_vox(path: impl AsRef<Path>) -> Result<ImportedVox, VoxImportError> {
    let bytes = std::fs::read(path.as_ref())?;
    parse_vox_bytes(&bytes)
}

/// Convenience: load a `.vox` file from disk via `std::fs::read` + parse with
/// `tiles × tiles` XZ tiling applied. `tiles == 1` is equivalent to
/// [`load_vox`].
///
/// **vox-gpu-rewrite Stage 2 (2026-05-18):** retained as the CPU oracle
/// helper for `--vox-gpu-oracle` only. The production install path uses the
/// W5 GPU producer chain which tiles via `voxelPos % modelSize` on device;
/// CPU XZ replication is no longer a runtime option.
pub fn load_vox_tiled(path: impl AsRef<Path>, tiles: u32) -> Result<ImportedVox, VoxImportError> {
    let bytes = std::fs::read(path.as_ref())?;
    parse_vox_bytes_tiled(&bytes, tiles)
}

/// Convert a parsed `DotVoxData` into [`ImportedVox`].
///
/// Pulled out so unit tests can drive it with a hand-built `DotVoxData`
/// without going through the binary parser.
pub fn parse_dot_vox_data(data: &dot_vox::DotVoxData) -> Result<ImportedVox, VoxImportError> {
    parse_dot_vox_data_tiled(data, 1)
}

/// Convert a parsed `DotVoxData` into [`ImportedVox`] with `tiles × tiles`
/// XZ tiling applied. `tiles == 1` is the canonical single-tile path.
///
/// **vox-gpu-rewrite Stage 2 (2026-05-18):** the production runtime path no
/// longer exposes the tile count — `setup_test_grid` always passes 1 (the W5
/// GPU producer chain handles the C#-faithful `voxelPos % modelSize` tiling
/// across the fixed 256-chunk world). The `tiles > 1` branch is retained for
/// the test corpus + the CPU oracle helpers and exercises the block-dedup
/// HashMap (identical content across tiles collapses to the same `VoxelPtr`
/// slot, so `voxels_cpu` grows by ~0× with tile count).
pub fn parse_dot_vox_data_tiled(
    data: &dot_vox::DotVoxData,
    tiles: u32,
) -> Result<ImportedVox, VoxImportError> {
    if data.models.is_empty() {
        return Err(VoxImportError::Empty);
    }
    let tiles = tiles.max(1);
    // Parse once → single-tile sparse buckets + tile dims.
    let (tile_buckets, tile_size_in_chunks) = compose_to_sparse_world(data)?;
    let buckets = if tiles == 1 {
        tile_buckets
    } else {
        replicate_buckets_xz(&tile_buckets, tile_size_in_chunks, tiles)?
    };
    let world = build_constructed_world_sparse(buckets)?;
    let palette = vox_palette_to_voxel_types(&data.palette, &data.materials);
    Ok(ImportedVox { world, palette })
}

/// Replicate `tile_buckets` (a single `.vox` tile's worth of sparse voxel
/// data) `tiles × tiles` times across the XZ plane, producing a new
/// [`ChunkBuckets`] sized at `(tiles × tile_w, tile_h, tiles × tile_d)`.
///
/// **Faithful-port note:** this mirrors C#'s startup-time multi-`.vox` load
/// (where multiple Oasis_Hard_Cover.vox instances are placed in a 4×4 grid).
/// The block-dedup pass downstream (`build_constructed_world_sparse`'s
/// HashMap) collapses identical block content across tiles for free.
fn replicate_buckets_xz(
    tile_buckets: &ChunkBuckets,
    tile_size_in_chunks: [u32; 3],
    tiles: u32,
) -> Result<ChunkBuckets, VoxImportError> {
    let [tw, th, td] = tile_size_in_chunks;
    let new_size = [
        tw.saturating_mul(tiles),
        th,
        td.saturating_mul(tiles),
    ];
    validate_caps(new_size)?;

    let mut out = ChunkBuckets::new(new_size);
    let new_sx = new_size[0] as usize;
    let new_sy = new_size[1] as usize;
    let tile_sx = tw as usize;
    let tile_sy = th as usize;

    for tz in 0..tiles {
        for tx in 0..tiles {
            let off_cx = (tx * tw) as usize;
            let off_cz = (tz * td) as usize;
            for cz in 0..(td as usize) {
                for cy in 0..(th as usize) {
                    for cx in 0..(tw as usize) {
                        let src_idx = cx + cy * tile_sx + cz * tile_sx * tile_sy;
                        let Some(bucket) = tile_buckets.chunks[src_idx].as_ref() else {
                            continue;
                        };
                        let dst_cx = cx + off_cx;
                        let dst_cy = cy;
                        let dst_cz = cz + off_cz;
                        let dst_idx = dst_cx + dst_cy * new_sx + dst_cz * new_sx * new_sy;
                        out.chunks[dst_idx] = Some(bucket.clone());
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Apply an [`ImportedVox`] to fresh [`crate::world::data::WorldData`] +
/// [`crate::world::data::VoxelTypes`] resources, by installing the pre-built
/// `chunks_cpu`/`blocks_cpu`/`voxels_cpu` directly (no `construct()` call).
///
/// **Δ-GPUProducer (v2):** `dense_voxel_types` is set to `Vec::new()` so the
/// GPU producer's data-driven gate at `render/construction/mod.rs:833-835`
/// skips the segmented-dispatch chain — the renderer reads the pre-built
/// CPU mirror buffers via the existing extract/prepare upload path.
pub fn build_world_from_vox(
    imported: ImportedVox,
) -> (crate::world::data::WorldData, crate::world::data::VoxelTypes) {
    use crate::world::data::{IAabb3, VoxelTypes, WorldData};
    use bevy::math::{IVec3, UVec3};

    let world = imported.world;
    let size = [
        world.size_in_chunks[0] * CHUNK_DIM_VOXELS,
        world.size_in_chunks[1] * CHUNK_DIM_VOXELS,
        world.size_in_chunks[2] * CHUNK_DIM_VOXELS,
    ];

    let mut world_data = WorldData {
        chunks_cpu: world.chunks,
        blocks_cpu: world.blocks,
        voxels_cpu: world.voxels,
        size_in_chunks: UVec3::from_array(world.size_in_chunks),
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(size[0] as i32 - 1, size[1] as i32 - 1, size[2] as i32 - 1),
        },
        pending_edits: Default::default(),
        // Δ-GPUProducer — sparse path skips the GPU producer chain (which
        // requires a dense voxel-type mirror that would cost ~140 GiB for an
        // Oasis-class world). The renderer reads the pre-built CPU buffers
        // directly.
        dense_voxel_types: Vec::new(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();

    let voxel_types = VoxelTypes {
        types: imported.palette,
    };

    (world_data, voxel_types)
}

// ============================================================================
// Scene-graph composition (from 03a-followup, UNCHANGED apart from the
// pass-2 emitter target — was `DenseVolume::set`, now `ChunkBuckets::push`).
// ============================================================================

/// A 3×3 signed-permutation rotation matrix (`_r` byte → integer matrix).
///
/// MagicaVoxel `_r` is a byte with 3 axis-selector bits + 3 sign bits per the
/// scene-format spec; the resulting matrix is always a signed permutation
/// (rows/cols are unit vectors with at most one non-zero, ±1). Mirrors
/// C# `MagicaVoxel.TransformFrame.Read` rotation-byte branch at
/// `MagicaVoxel.cs:127-146`.
#[derive(Clone, Copy, Debug)]
struct Rot3 {
    /// `m[col][row]`. Each column has exactly one non-zero ±1 entry.
    m: [[i32; 3]; 3],
}

impl Rot3 {
    const IDENTITY: Self = Self {
        m: [[1, 0, 0], [0, 1, 0], [0, 0, 1]],
    };

    /// Parse the MagicaVoxel `_r` rotation byte into a signed-permutation
    /// matrix. Mirrors C# `MagicaVoxel.cs:127-146` byte-for-byte.
    fn from_byte(b: u8) -> Self {
        let i1 = ((b & 0b00000011) >> 0) as usize;
        let i2 = ((b & 0b00001100) >> 2) as usize;
        let i3: usize = if i1 != 0 && i2 != 0 {
            0
        } else if i1 != 1 && i2 != 1 {
            1
        } else {
            2
        };
        let s1: i32 = if (b & 0b00010000) >> 4 == 0 { 1 } else { -1 };
        let s2: i32 = if (b & 0b00100000) >> 5 == 0 { 1 } else { -1 };
        let s3: i32 = if (b & 0b01000000) >> 6 == 0 { 1 } else { -1 };

        let mut m = [[0i32; 3]; 3];
        m[i1][0] = s1;
        m[i2][1] = s2;
        m[i3][2] = s3;
        Self { m }
    }

    /// Compose two rotations: `self * other`.
    fn compose(&self, other: &Rot3) -> Rot3 {
        let mut out = [[0i32; 3]; 3];
        for col in 0..3 {
            for row in 0..3 {
                let mut s = 0i32;
                for k in 0..3 {
                    s += self.m[k][row] * other.m[col][k];
                }
                out[col][row] = s;
            }
        }
        Rot3 { m: out }
    }

    /// `M * v` — apply this rotation matrix to a vector.
    fn transform_vec(&self, v: [i32; 3]) -> [i32; 3] {
        [
            self.m[0][0] * v[0] + self.m[1][0] * v[1] + self.m[2][0] * v[2],
            self.m[0][1] * v[0] + self.m[1][1] * v[1] + self.m[2][1] * v[2],
            self.m[0][2] * v[0] + self.m[1][2] * v[1] + self.m[2][2] * v[2],
        ]
    }
}

/// A composed scene-graph transform: rotation + translation, in MagicaVoxel
/// (Z-up) coordinates.
#[derive(Clone, Copy, Debug)]
struct Xform {
    rot: Rot3,
    t: [i32; 3],
}

impl Xform {
    const IDENTITY: Self = Self {
        rot: Rot3::IDENTITY,
        t: [0, 0, 0],
    };

    /// Apply this transform to a MagicaVoxel-space point: `q = R * p + t`.
    fn apply(&self, p: [i32; 3]) -> [i32; 3] {
        let r = self.rot.transform_vec(p);
        [r[0] + self.t[0], r[1] + self.t[1], r[2] + self.t[2]]
    }

    /// Compose `parent ∘ self`: `result.apply(p) = parent.apply(self.apply(p))`.
    fn parent_of(&self, parent: &Xform) -> Xform {
        let new_rot = parent.rot.compose(&self.rot);
        let parent_t_of_self_t = parent.rot.transform_vec(self.t);
        let new_t = [
            parent_t_of_self_t[0] + parent.t[0],
            parent_t_of_self_t[1] + parent.t[1],
            parent_t_of_self_t[2] + parent.t[2],
        ];
        Xform {
            rot: new_rot,
            t: new_t,
        }
    }
}

fn frame_to_xform(frame: &dot_vox::Frame) -> Xform {
    let rot = if let Some(raw) = frame.attributes.get("_r") {
        raw.parse::<u8>().map(Rot3::from_byte).unwrap_or(Rot3::IDENTITY)
    } else {
        Rot3::IDENTITY
    };
    let t = frame
        .position()
        .map(|p| [p.x, p.y, p.z])
        .unwrap_or([0, 0, 0]);
    Xform { rot, t }
}

// ============================================================================
// Pass 1 — world AABB accumulation (UNCHANGED from 03a-followup).
// ============================================================================

fn accumulate_world_aabb(
    data: &dot_vox::DotVoxData,
    node_id: u32,
    parent: Xform,
    visited: &mut [bool],
    world_min: &mut [i32; 3],
    world_max: &mut [i32; 3],
) {
    let idx = node_id as usize;
    if idx >= visited.len() || visited[idx] {
        return;
    }
    visited[idx] = true;
    match &data.scenes[idx] {
        dot_vox::SceneNode::Transform { frames, child, .. } => {
            let frame_xform = frames
                .first()
                .map(frame_to_xform)
                .unwrap_or(Xform::IDENTITY);
            let new_xform = frame_xform.parent_of(&parent);
            accumulate_world_aabb(data, *child, new_xform, visited, world_min, world_max);
        }
        dot_vox::SceneNode::Group { children, .. } => {
            for &c in children {
                accumulate_world_aabb(data, c, parent, visited, world_min, world_max);
            }
        }
        dot_vox::SceneNode::Shape { models, .. } => {
            for sm in models {
                let Some(model) = data.models.get(sm.model_id as usize) else {
                    continue;
                };
                let s = [
                    model.size.x as i32,
                    model.size.y as i32,
                    model.size.z as i32,
                ];
                let lmin = [-s[0] / 2, -s[1] / 2, -s[2] / 2];
                let lmax = [lmin[0] + s[0] - 1, lmin[1] + s[1] - 1, lmin[2] + s[2] - 1];
                for cx in 0..2 {
                    for cy in 0..2 {
                        for cz in 0..2 {
                            let p = [
                                if cx == 0 { lmin[0] } else { lmax[0] },
                                if cy == 0 { lmin[1] } else { lmax[1] },
                                if cz == 0 { lmin[2] } else { lmax[2] },
                            ];
                            let q = parent.apply(p);
                            for a in 0..3 {
                                if q[a] < world_min[a] {
                                    world_min[a] = q[a];
                                }
                                if q[a] > world_max[a] {
                                    world_max[a] = q[a];
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// Pass 2 — sparse voxel emission into per-chunk buckets (NEW; replaces v1's
// `collate_voxels` that wrote into `DenseVolume::set`).
// ============================================================================

/// Per-pass-2 sparse accumulator. One `Vec<(local_idx_in_chunk, ty)>` per
/// chunk; `None` until the chunk receives its first voxel.
///
/// **Host RAM peak ≈ Σ non-empty voxels × ~6–8 bytes** — for Oasis at ~1%
/// density of a 5952×2176×5376-voxel world, ~7M non-empty × ~6 B ≈ 50 MiB.
/// Vs. dense: ~140 GiB.
struct ChunkBuckets {
    size_in_chunks: [u32; 3],
    /// One bucket per chunk; `None` until first push.
    /// `local_idx_in_chunk` = `vx + vy*16 + vz*256` (vx,vy,vz ∈ [0..16)).
    chunks: Vec<Option<Vec<(u16, VoxelTypeId)>>>,
}

impl ChunkBuckets {
    fn new(size_in_chunks: [u32; 3]) -> Self {
        let n = (size_in_chunks[0] as usize)
            * (size_in_chunks[1] as usize)
            * (size_in_chunks[2] as usize);
        Self {
            size_in_chunks,
            chunks: (0..n).map(|_| None).collect(),
        }
    }

    /// Push a single voxel at `[nx, ny, nz]` (post-Z↔Y-swap NAADF coords)
    /// with type `ty` into the per-chunk bucket. Allocates the chunk's
    /// bucket lazily.
    fn push(&mut self, naadf_pos: [u32; 3], ty: VoxelTypeId) {
        let cx = naadf_pos[0] / CHUNK_DIM_VOXELS;
        let cy = naadf_pos[1] / CHUNK_DIM_VOXELS;
        let cz = naadf_pos[2] / CHUNK_DIM_VOXELS;
        let sx = self.size_in_chunks[0];
        let sy = self.size_in_chunks[1];
        let ci = (cx + cy * sx + cz * sx * sy) as usize;
        let lx = (naadf_pos[0] % CHUNK_DIM_VOXELS) as u16;
        let ly = (naadf_pos[1] % CHUNK_DIM_VOXELS) as u16;
        let lz = (naadf_pos[2] % CHUNK_DIM_VOXELS) as u16;
        let local = lx + ly * (CHUNK_DIM_VOXELS as u16) + lz * (CHUNK_DIM_VOXELS as u16 * CHUNK_DIM_VOXELS as u16);
        self.chunks[ci].get_or_insert_with(Vec::new).push((local, ty));
    }
}

/// Walk the scene graph and emit voxels into `ChunkBuckets`. Mirrors C#
/// `MagicaVoxel.CollateVoxelData` at `MagicaVoxel.cs:718-755`.
fn collate_voxels_sparse(
    data: &dot_vox::DotVoxData,
    node_id: u32,
    parent: Xform,
    visited: &mut [bool],
    world_min: [i32; 3],
    buckets: &mut ChunkBuckets,
) {
    let idx = node_id as usize;
    if idx >= visited.len() || visited[idx] {
        return;
    }
    visited[idx] = true;
    match &data.scenes[idx] {
        dot_vox::SceneNode::Transform { frames, child, .. } => {
            let frame_xform = frames
                .first()
                .map(frame_to_xform)
                .unwrap_or(Xform::IDENTITY);
            let new_xform = frame_xform.parent_of(&parent);
            collate_voxels_sparse(data, *child, new_xform, visited, world_min, buckets);
        }
        dot_vox::SceneNode::Group { children, .. } => {
            for &c in children {
                collate_voxels_sparse(data, c, parent, visited, world_min, buckets);
            }
        }
        dot_vox::SceneNode::Shape { models, .. } => {
            for sm in models {
                let Some(model) = data.models.get(sm.model_id as usize) else {
                    continue;
                };
                let s = [
                    model.size.x as i32,
                    model.size.y as i32,
                    model.size.z as i32,
                ];
                // Centered local origin (MagicaVoxel.cs:738 + BoundsXYZ.cs:22).
                let origin = [-s[0] / 2, -s[1] / 2, -s[2] / 2];
                for v in &model.voxels {
                    let local = [v.x as i32, v.y as i32, v.z as i32];
                    let centered = [
                        local[0] + origin[0],
                        local[1] + origin[1],
                        local[2] + origin[2],
                    ];
                    let world = parent.apply(centered);
                    let shifted = [
                        world[0] - world_min[0],
                        world[1] - world_min[1],
                        world[2] - world_min[2],
                    ];
                    if shifted[0] < 0 || shifted[1] < 0 || shifted[2] < 0 {
                        continue;
                    }
                    // Z↔Y swap to NAADF coords (`ModelData.cs:438`).
                    let nx = shifted[0] as u32;
                    let ny = shifted[2] as u32;
                    let nz = shifted[1] as u32;
                    let max_x = buckets.size_in_chunks[0] * CHUNK_DIM_VOXELS;
                    let max_y = buckets.size_in_chunks[1] * CHUNK_DIM_VOXELS;
                    let max_z = buckets.size_in_chunks[2] * CHUNK_DIM_VOXELS;
                    if nx >= max_x || ny >= max_y || nz >= max_z {
                        continue;
                    }
                    let ty = VoxelTypeId(v.i as u16 + 1);
                    buckets.push([nx, ny, nz], ty);
                }
            }
        }
    }
}

// ============================================================================
// Pass-2 driver: scene-graph walk → ChunkBuckets.
// ============================================================================

/// Validate `size_in_chunks` against [`MAX_CHUNKS_PER_AXIS`].
fn validate_caps(size_in_chunks: [u32; 3]) -> Result<(), VoxImportError> {
    if size_in_chunks[0] > MAX_CHUNKS_PER_AXIS
        || size_in_chunks[1] > MAX_CHUNKS_PER_AXIS
        || size_in_chunks[2] > MAX_CHUNKS_PER_AXIS
    {
        return Err(VoxImportError::SizeExceedsTextureLimit {
            dim: size_in_chunks,
            limit: MAX_CHUNKS_PER_AXIS,
        });
    }
    Ok(())
}

/// Compose `data` to a sparse [`ChunkBuckets`] + return the
/// chunks-per-axis dimensions.
///
/// Handles both:
/// - **No-scene-graph fallback** (`data.scenes.is_empty()`): single model
///   path; no transforms, just a Z↔Y swap.
/// - **Scene-graph composition** (general case): two-pass walk over the
///   scene graph, exactly as v1 did but with the pass-2 target swapped from
///   `DenseVolume` to `ChunkBuckets`.
fn compose_to_sparse_world(
    data: &dot_vox::DotVoxData,
) -> Result<(ChunkBuckets, [u32; 3]), VoxImportError> {
    // No-scene-graph fallback — collapse to models[0].
    if data.scenes.is_empty() {
        return compose_models0_fallback(data);
    }

    // Pass 1 — world AABB.
    let mut visited = vec![false; data.scenes.len()];
    let mut world_min = [i32::MAX; 3];
    let mut world_max = [i32::MIN; 3];
    accumulate_world_aabb(
        data,
        0,
        Xform::IDENTITY,
        &mut visited,
        &mut world_min,
        &mut world_max,
    );
    if world_min[0] == i32::MAX {
        // Scene graph walked but no visible shapes — same recovery as v1.
        return compose_models0_fallback(data);
    }

    let mv_size = [
        (world_max[0] - world_min[0] + 1) as u32,
        (world_max[1] - world_min[1] + 1) as u32,
        (world_max[2] - world_min[2] + 1) as u32,
    ];
    // Z↔Y swap: MagicaVoxel (x, y, z) → NAADF (x, z, y).
    let naadf_size = [mv_size[0], mv_size[2], mv_size[1]];
    let size_in_chunks = [
        naadf_size[0].div_ceil(CHUNK_DIM_VOXELS).max(1),
        naadf_size[1].div_ceil(CHUNK_DIM_VOXELS).max(1),
        naadf_size[2].div_ceil(CHUNK_DIM_VOXELS).max(1),
    ];
    validate_caps(size_in_chunks)?;

    // Pass 2 — emit voxels into per-chunk sparse buckets.
    let mut buckets = ChunkBuckets::new(size_in_chunks);
    let mut visited = vec![false; data.scenes.len()];
    collate_voxels_sparse(
        data,
        0,
        Xform::IDENTITY,
        &mut visited,
        world_min,
        &mut buckets,
    );

    Ok((buckets, size_in_chunks))
}

/// "No scene graph" — single-model fallback. Uses `models[0]` directly with
/// no transforms (just the Z↔Y swap on the way in).
fn compose_models0_fallback(
    data: &dot_vox::DotVoxData,
) -> Result<(ChunkBuckets, [u32; 3]), VoxImportError> {
    let model = &data.models[0];
    let mv_size = [model.size.x, model.size.y, model.size.z];
    if mv_size == [0, 0, 0] {
        return Err(VoxImportError::Empty);
    }
    let naadf_size = [mv_size[0], mv_size[2], mv_size[1]];
    let size_in_chunks = [
        naadf_size[0].div_ceil(CHUNK_DIM_VOXELS).max(1),
        naadf_size[1].div_ceil(CHUNK_DIM_VOXELS).max(1),
        naadf_size[2].div_ceil(CHUNK_DIM_VOXELS).max(1),
    ];
    validate_caps(size_in_chunks)?;

    let max_x = size_in_chunks[0] * CHUNK_DIM_VOXELS;
    let max_y = size_in_chunks[1] * CHUNK_DIM_VOXELS;
    let max_z = size_in_chunks[2] * CHUNK_DIM_VOXELS;

    let mut buckets = ChunkBuckets::new(size_in_chunks);
    for v in &model.voxels {
        // Z↔Y swap, no translation.
        let nx = v.x as u32;
        let ny = v.z as u32;
        let nz = v.y as u32;
        if nx >= max_x || ny >= max_y || nz >= max_z {
            continue;
        }
        buckets.push([nx, ny, nz], VoxelTypeId(v.i as u16 + 1));
    }
    Ok((buckets, size_in_chunks))
}

// ============================================================================
// `ChunkBuckets` → `ConstructedWorld` (Δ-AADF: per-chunk inline AADF build,
// then one global chunk-layer AADF pass).
// ============================================================================

/// Build a [`ConstructedWorld`] from per-chunk sparse buckets. Byte-equivalent
/// to what `aadf::construct::construct(&DenseVolume)` would produce on the
/// same input (Test #15 — `sparse_walk_matches_dense_construct_on_small_fixture`).
fn build_constructed_world_sparse(
    buckets: ChunkBuckets,
) -> Result<ConstructedWorld, VoxImportError> {
    let cx_u = buckets.size_in_chunks[0] as usize;
    let cy_u = buckets.size_in_chunks[1] as usize;
    let cz_u = buckets.size_in_chunks[2] as usize;
    let n_chunks = cx_u * cy_u * cz_u;

    // Output buffers. `chunks_cpu` is exactly sized; `blocks_cpu` /
    // `voxels_cpu` grow as non-empty chunks/blocks are encoded.
    let mut chunks_cpu: Vec<u32> = vec![0; n_chunks];
    let mut blocks_cpu: Vec<u32> = Vec::new();
    let mut voxels_cpu: Vec<u32> = Vec::new();

    // Δ-Hash — Block dedup keyed on the literal 64-voxel content. Same shape
    // (and same map type) as `aadf::construct::construct` uses at
    // `aadf/construct.rs:142` so the sparse path's output is byte-equal to
    // the dense path's on the same input (Test #15 enforces this).
    let mut block_dedup: HashMap<[VoxelTypeId; CELL_CHILDREN], VoxelPtr> = HashMap::new();

    let mut chunk_class: Vec<ChunkClass> = vec![ChunkClass::Empty; n_chunks];
    // For mixed chunks we remember the 64 `BlockClass`es so we can run
    // `encode_chunk_blocks` once in phase 2 (after all per-block AADFs are
    // baked into `voxels_cpu`). `None` for empty / uniform-full chunks.
    let mut chunk_block_arrays: Vec<Option<[BlockClass; CELL_CHILDREN]>> = vec![None; n_chunks];

    // The C# walk-order is `cz, cy, cx` outermost (`ModelData.cs:418-499`);
    // the existing dense `construct()` walks `bz_i, by_i, bx_i` (blocks)
    // then `cz_i, cy_i, cx_i` (chunks) at `aadf/construct.rs:147-188`. We
    // walk chunks-then-blocks-within-chunks in the SAME order as the dense
    // path's block walk (z-outer, y-mid, x-inner across the whole world)
    // so the dedup-miss-ordering matches construct()'s. See the byte-equal
    // test that enforces this.
    //
    // Specifically: construct() walks all 64 blocks of chunk 0 first
    // (block-z-major, block-y-mid, block-x-inner WITHIN the world's full
    // block layer), in the *world's* block coordinate order — which is the
    // same as walking all blocks-of-chunk-0, then chunk-1, ...if chunks are
    // walked in z-major order. The orders coincide here because both walks
    // bottom up.
    for cz_i in 0..cz_u {
        for cy_i in 0..cy_u {
            for cx_i in 0..cx_u {
                let ci = cx_i + cy_i * cx_u + cz_i * cx_u * cy_u;
                let Some(bucket) = buckets.chunks[ci].as_ref() else {
                    continue;
                };
                // 1. Replay bucket into a transient 16³ dense chunk_voxels
                //    array (8 KiB; discarded at end-of-iteration).
                //    last-write-wins matches C# `dataImport[q] = v`
                //    (CollateVoxelData:747).
                let mut chunk_voxels = [VoxelTypeId::EMPTY; 16 * 16 * 16];
                for &(local, ty) in bucket {
                    chunk_voxels[local as usize] = ty;
                }
                // 2. Classify the chunk.
                if chunk_voxels.iter().all(|t| *t == VoxelTypeId::EMPTY) {
                    // Defensive — should be unreachable, push() filtered
                    // OOB pushes, so a non-None bucket has ≥1 entry. Keep
                    // chunk_class[ci] = Empty.
                    continue;
                }
                let first = chunk_voxels[0];
                let uniform = chunk_voxels.iter().all(|t| *t == first);
                if uniform && first != VoxelTypeId::EMPTY {
                    chunk_class[ci] = ChunkClass::UniformFull(first);
                    continue;
                }
                // 3. Mixed chunk — classify 64 blocks; dedup + append
                //    mixed-block voxels.
                let mut blocks_in_chunk = [BlockClass::Empty; CELL_CHILDREN];
                // Walk blocks in the same (lx_block, ly_block, lz_block) order
                // as construct() — bz outer, by mid, bx inner — to keep the
                // dedup-miss-ordering identical.
                for bz in 0..CELL_DIM {
                    for by in 0..CELL_DIM {
                        for bx in 0..CELL_DIM {
                            let b_local = bx + by * CELL_DIM + bz * CELL_DIM * CELL_DIM;
                            // Gather the 64 voxels of this block from
                            // chunk_voxels.
                            let mut block_voxels = [VoxelTypeId::EMPTY; CELL_CHILDREN];
                            for lz in 0..CELL_DIM {
                                for ly in 0..CELL_DIM {
                                    for lx in 0..CELL_DIM {
                                        // chunk_local index — x-fastest.
                                        let cx_local = bx * CELL_DIM + lx;
                                        let cy_local = by * CELL_DIM + ly;
                                        let cz_local = bz * CELL_DIM + lz;
                                        let chunk_local =
                                            cx_local + cy_local * 16 + cz_local * 256;
                                        let block_local =
                                            lx + ly * CELL_DIM + lz * CELL_DIM * CELL_DIM;
                                        block_voxels[block_local] = chunk_voxels[chunk_local];
                                    }
                                }
                            }
                            // Classify the block.
                            if block_voxels.iter().all(|t| *t == VoxelTypeId::EMPTY) {
                                blocks_in_chunk[b_local] = BlockClass::Empty;
                            } else {
                                let bf = block_voxels[0];
                                if block_voxels.iter().all(|t| *t == bf) {
                                    blocks_in_chunk[b_local] = BlockClass::UniformFull(bf);
                                } else {
                                    // Mixed — dedup, append + encode on miss.
                                    let ptr = if let Some(&existing) =
                                        block_dedup.get(&block_voxels)
                                    {
                                        existing
                                    } else {
                                        let new_ptr = VoxelPtr(voxels_cpu.len() as u32);
                                        // Append 32 placeholder u32s; the
                                        // call below will overwrite with
                                        // the AADF-augmented encoding.
                                        voxels_cpu
                                            .resize(voxels_cpu.len() + CELL_CHILDREN / 2, 0);
                                        block_dedup.insert(block_voxels, new_ptr);
                                        encode_block_voxels(
                                            &block_voxels,
                                            new_ptr,
                                            &mut voxels_cpu,
                                        );
                                        new_ptr
                                    };
                                    blocks_in_chunk[b_local] = BlockClass::Mixed(ptr);
                                }
                            }
                        }
                    }
                }
                // 4. Reserve 64 consecutive `blocks_cpu` slots for this
                //    chunk; populate in phase 2 once all chunks are
                //    classified.
                let block_base = BlockPtr(blocks_cpu.len() as u32);
                blocks_cpu.resize(blocks_cpu.len() + CELL_CHILDREN, 0);
                chunk_class[ci] = ChunkClass::Mixed(block_base);
                chunk_block_arrays[ci] = Some(blocks_in_chunk);
            }
        }
    }

    // Phase 2 — encode each mixed chunk's 64 blocks (with block-layer AADFs)
    // into the reserved `blocks_cpu` slots.
    for ci in 0..n_chunks {
        if let (ChunkClass::Mixed(base), Some(blocks_in_chunk)) =
            (chunk_class[ci], chunk_block_arrays[ci])
        {
            encode_chunk_blocks(&blocks_in_chunk, base, &mut blocks_cpu);
        }
    }

    // Phase 3 — world-chunk-layer AADFs. One global merge-form
    // `compute_aadf_layer` over the chunks_per_axis³ extent (matches
    // construct.rs:228-232).
    let chunk_is_empty_at = |c: [i32; 3]| -> bool {
        let idx = c[0] as usize + c[1] as usize * cx_u + c[2] as usize * cx_u * cy_u;
        matches!(chunk_class[idx], ChunkClass::Empty)
    };
    let chunk_aadfs = compute_aadf_layer([cx_u, cy_u, cz_u], AADF_MAX_CHUNK, chunk_is_empty_at);

    // Phase 4 — emit `chunks_cpu[i]` u32s.
    for ci in 0..n_chunks {
        let cell = match chunk_class[ci] {
            ChunkClass::Empty => ChunkCell::Empty(chunk_aadfs[ci]),
            ChunkClass::UniformFull(ty) => ChunkCell::UniformFull(ty),
            ChunkClass::Mixed(ptr) => ChunkCell::Mixed(ptr),
        };
        chunks_cpu[ci] = cell.encode();
    }

    // Phase 5 — output buffer size pre-flight (catches pathological-density
    // inputs).
    let voxels_bytes = (voxels_cpu.len() * 4) as u64;
    if voxels_bytes > MAX_VOXELS_BUFFER_BYTES {
        return Err(VoxImportError::SizeExceedsBudget {
            dim: [voxels_cpu.len() as u32, 0, 0],
            bytes: MAX_VOXELS_BUFFER_BYTES,
        });
    }
    let blocks_bytes = (blocks_cpu.len() * 4) as u64;
    if blocks_bytes > MAX_BLOCKS_BUFFER_BYTES {
        return Err(VoxImportError::SizeExceedsBudget {
            dim: [blocks_cpu.len() as u32, 0, 0],
            bytes: MAX_BLOCKS_BUFFER_BYTES,
        });
    }

    Ok(ConstructedWorld {
        chunks: chunks_cpu,
        blocks: blocks_cpu,
        voxels: voxels_cpu,
        size_in_chunks: buckets.size_in_chunks,
    })
}

// ============================================================================
// Palette parse (UNCHANGED from v1).
// ============================================================================

/// Promote the 256-entry MagicaVoxel `RGBA` palette + `MATL` chunks into a
/// `Vec<VoxelType>` of length `palette.len() + 1`. Index 0 is the reserved
/// empty placeholder; indices 1..=N mirror the source palette entries.
///
/// Mirrors C# `ModelData.cs:502-522`.
fn vox_palette_to_voxel_types(
    palette: &[dot_vox::Color],
    materials: &[dot_vox::Material],
) -> Vec<VoxelType> {
    let mut out = Vec::with_capacity(palette.len() + 1);
    out.push(VoxelType::default());

    for (i, color) in palette.iter().enumerate() {
        let srgb = Vec3::new(color.r as f32, color.g as f32, color.b as f32) / 255.0;
        let linear = Vec3::new(srgb.x.powf(2.2), srgb.y.powf(2.2), srgb.z.powf(2.2));

        let (emit, flux) = materials
            .iter()
            .find(|m| m.id as usize == i)
            .map(|m| {
                (
                    m.emission().unwrap_or(0.0),
                    m.radiant_flux().unwrap_or(0.0),
                )
            })
            .unwrap_or((0.0, 0.0));

        let emission = emit * (1.0 + flux).powi(2) * 5.0;

        let (material_base, color_layered) = if emission > 0.0 {
            (MaterialBase::Emissive, Vec3::new(emission, 0.0, 0.0))
        } else {
            (MaterialBase::Diffuse, Vec3::ZERO)
        };

        out.push(VoxelType {
            color_base: linear,
            material_base,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_layered,
        });
    }

    out
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aadf::cell::{BlockCell, VoxelCell};
    use crate::aadf::construct::{construct, DenseVolume};
    use bevy::math::IVec3;
    use std::collections::HashMap as StdHashMap;
    use std::io::Cursor;

    // ------------------------------------------------------------------------
    // Test helper — re-densify an `ImportedVox.world` into a flat
    // `Vec<VoxelTypeId>` so the v1 voxel-position assertions still read.
    //
    // Walks `ChunkCell::decode → BlockCell::decode → VoxelCell::decode` to
    // recover a per-voxel type at world position `[x, y, z]`. Pure read-side;
    // no semantic duplication of `construct()`. Used only by tests.
    // ------------------------------------------------------------------------

    /// Decode the voxel at NAADF position `[x, y, z]` from an [`ImportedVox`].
    /// Returns `VoxelTypeId::EMPTY` for empty cells.
    fn decoded_voxel_at(world: &ConstructedWorld, p: [u32; 3]) -> VoxelTypeId {
        let cx = p[0] / 16;
        let cy = p[1] / 16;
        let cz = p[2] / 16;
        let lcx = p[0] % 16; // chunk-local voxel coord
        let lcy = p[1] % 16;
        let lcz = p[2] % 16;
        let bx = lcx / 4; // block index within chunk
        let by = lcy / 4;
        let bz = lcz / 4;
        let lbx = lcx % 4; // voxel index within block
        let lby = lcy % 4;
        let lbz = lcz % 4;
        let s = world.size_in_chunks;
        let ci = (cx + cy * s[0] + cz * s[0] * s[1]) as usize;
        if ci >= world.chunks.len() {
            return VoxelTypeId::EMPTY;
        }
        match ChunkCell::decode(world.chunks[ci]) {
            ChunkCell::Empty(_) => VoxelTypeId::EMPTY,
            ChunkCell::UniformFull(ty) => ty,
            ChunkCell::Mixed(bp) => {
                let block_idx = (bx + by * 4 + bz * 16) as usize;
                let block_word = world.blocks[bp.0 as usize + block_idx];
                match BlockCell::decode(block_word) {
                    BlockCell::Empty(_) => VoxelTypeId::EMPTY,
                    BlockCell::UniformFull(ty) => ty,
                    BlockCell::Mixed(vp) => {
                        let voxel_idx = (lbx + lby * 4 + lbz * 16) as usize;
                        let pair = voxel_idx / 2;
                        let lo = (world.voxels[vp.0 as usize + pair] & 0xFFFF) as u16;
                        let hi = ((world.voxels[vp.0 as usize + pair] >> 16) & 0xFFFF) as u16;
                        let half = if voxel_idx % 2 == 0 { lo } else { hi };
                        match VoxelCell::decode(half) {
                            VoxelCell::Empty(_) => VoxelTypeId::EMPTY,
                            VoxelCell::Full(ty) => ty,
                        }
                    }
                }
            }
        }
    }

    /// Count non-empty voxels by walking every world coord. Cheap enough for
    /// the test fixtures (max ~64³ = 262K voxels).
    fn count_nonempty(world: &ConstructedWorld) -> u32 {
        let s = world.size_in_chunks;
        let sx = s[0] * 16;
        let sy = s[1] * 16;
        let sz = s[2] * 16;
        let mut n = 0u32;
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    if decoded_voxel_at(world, [x, y, z]) != VoxelTypeId::EMPTY {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// Build a tiny single-voxel `DotVoxData` in MagicaVoxel coords.
    fn build_single_voxel() -> dot_vox::DotVoxData {
        dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 1, y: 1, z: 1 },
                voxels: vec![dot_vox::Voxel { x: 0, y: 0, z: 0, i: 0 }],
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        }
    }

    fn build_small_cube() -> dot_vox::DotVoxData {
        let mut voxels = Vec::with_capacity(7 * 7 * 7 + 1);
        for z in 0..7u8 {
            for y in 0..7u8 {
                for x in 0..7u8 {
                    voxels.push(dot_vox::Voxel { x, y, z, i: 10 });
                }
            }
        }
        voxels.retain(|v| !(v.x == 3 && v.y == 3 && v.z == 3));
        voxels.push(dot_vox::Voxel { x: 3, y: 3, z: 3, i: 20 });

        let mut materials = default_materials();
        for m in &mut materials {
            if m.id == 20 {
                m.properties.insert("_type".into(), "_emit".into());
                m.properties.insert("_emit".into(), "1.0".into());
                m.properties.insert("_flux".into(), "0.0".into());
            }
        }

        dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 8, y: 8, z: 8 },
                voxels,
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials,
            scenes: Vec::new(),
            layers: Vec::new(),
        }
    }

    fn default_materials() -> Vec<dot_vox::Material> {
        (0..256)
            .map(|i| dot_vox::Material {
                id: i,
                properties: {
                    let mut d: dot_vox::Dict = StdHashMap::new().into_iter().collect();
                    d.insert("_type".to_owned(), "_diffuse".to_owned());
                    d
                },
            })
            .collect()
    }

    fn round_trip(data: &dot_vox::DotVoxData) -> ImportedVox {
        let mut buf = Vec::new();
        data.write_vox(&mut Cursor::new(&mut buf))
            .expect("write_vox failed");
        parse_vox_bytes(&buf).expect("parse_vox_bytes failed")
    }

    // -- Test 1 --------------------------------------------------------------

    #[test]
    fn parses_single_voxel_fixture() {
        let data = build_single_voxel();
        let imp = round_trip(&data);

        assert_eq!(imp.world.size_in_chunks, [1, 1, 1]);
        assert_eq!(imp.palette.len(), 257);
        assert_eq!(imp.palette[0], VoxelType::default());

        assert_eq!(decoded_voxel_at(&imp.world, [0, 0, 0]), VoxelTypeId(1));
        assert_eq!(decoded_voxel_at(&imp.world, [1, 0, 0]), VoxelTypeId::EMPTY);
        assert_eq!(decoded_voxel_at(&imp.world, [0, 1, 0]), VoxelTypeId::EMPTY);
        assert_eq!(decoded_voxel_at(&imp.world, [0, 0, 1]), VoxelTypeId::EMPTY);
        assert_eq!(decoded_voxel_at(&imp.world, [8, 8, 8]), VoxelTypeId::EMPTY);
    }

    // -- Test 2 --------------------------------------------------------------

    #[test]
    fn parses_small_cube_fixture() {
        let data = build_small_cube();
        let imp = round_trip(&data);

        assert_eq!(imp.world.size_in_chunks, [1, 1, 1]);

        // 7³ non-empty voxels (one replaced with emissive, but still non-empty).
        assert_eq!(count_nonempty(&imp.world), 343);

        assert_eq!(decoded_voxel_at(&imp.world, [3, 3, 3]), VoxelTypeId(21));
        assert_eq!(decoded_voxel_at(&imp.world, [0, 0, 0]), VoxelTypeId(11));
        assert_eq!(decoded_voxel_at(&imp.world, [6, 6, 6]), VoxelTypeId(11));

        assert_eq!(imp.palette[21].material_base, MaterialBase::Emissive);
        assert!(imp.palette[21].color_layered.x > 0.0);
        assert_eq!(imp.palette[11].material_base, MaterialBase::Diffuse);
    }

    // -- Test 3 --------------------------------------------------------------

    #[test]
    fn palette_index_zero_is_empty_placeholder() {
        let data = build_single_voxel();
        let imp = round_trip(&data);
        assert_eq!(imp.palette[0], VoxelType::default());
    }

    // -- Test 4 --------------------------------------------------------------

    #[test]
    fn palette_emissive_from_matl() {
        let mut materials = default_materials();
        for m in &mut materials {
            if m.id == 5 {
                m.properties.insert("_type".into(), "_emit".into());
                m.properties.insert("_emit".into(), "1.0".into());
                m.properties.insert("_flux".into(), "0.0".into());
            }
        }
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 1, y: 1, z: 1 },
                voxels: vec![dot_vox::Voxel { x: 0, y: 0, z: 0, i: 5 }],
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials,
            scenes: Vec::new(),
            layers: Vec::new(),
        };
        let imp = parse_dot_vox_data(&data).unwrap();
        assert_eq!(imp.palette[6].material_base, MaterialBase::Emissive);
        assert!(imp.palette[6].color_layered.x > 0.0);
        assert!((imp.palette[6].color_layered.x - 5.0).abs() < 1e-4);
    }

    // -- Test 5 --------------------------------------------------------------

    #[test]
    fn zy_swap_matches_csharp() {
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 2, y: 3, z: 4 },
                voxels: vec![dot_vox::Voxel { x: 1, y: 2, z: 3, i: 0 }],
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        };
        let imp = parse_dot_vox_data(&data).unwrap();
        assert_eq!(decoded_voxel_at(&imp.world, [1, 3, 2]), VoxelTypeId(1));
        assert_eq!(decoded_voxel_at(&imp.world, [1, 2, 3]), VoxelTypeId::EMPTY);
    }

    // -- Test 6 --------------------------------------------------------------

    #[test]
    fn size_exceeds_texture_limit_errors() {
        // model x size = 16400 → div_ceil(16) = 1025 chunks → exceeds 1024.
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size {
                    x: 16_400,
                    y: 1,
                    z: 1,
                },
                voxels: Vec::new(),
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        };
        let result = parse_dot_vox_data(&data);
        match result {
            Err(VoxImportError::SizeExceedsTextureLimit { dim, limit }) => {
                assert_eq!(limit, MAX_CHUNKS_PER_AXIS);
                assert!(dim[0] > MAX_CHUNKS_PER_AXIS);
            }
            other => panic!("expected SizeExceedsTextureLimit, got {:?}", other),
        }
    }

    // -- Test 7 --------------------------------------------------------------

    #[test]
    fn empty_models_errors() {
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: Vec::new(),
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        };
        let result = parse_dot_vox_data(&data);
        assert!(matches!(result, Err(VoxImportError::Empty)));
    }

    // -- Test 8 (v2) — byte-equality of sparse output with dense construct() --

    #[test]
    fn sparse_walk_matches_dense_construct_on_small_fixture() {
        // Build the small-cube fixture; drive it through (a) the sparse path
        // (parse_dot_vox_data), and (b) the v1-style dense path (manually
        // build a DenseVolume, call construct()). Assert byte-equality on
        // the resulting (chunks, blocks, voxels) buffers — this is the
        // strongest possible migration safety check.
        let data = build_small_cube();
        let imp = round_trip(&data);

        // Build the equivalent DenseVolume by walking the same data the
        // sparse path walks (single model, no scene graph; Z↔Y swap on
        // write). 8×8×8 MV → after Z↔Y swap NAADF 8×8×8 (cube) → 1×1×1
        // chunks (rounded up from 8 to 16 per axis).
        let mut volume = DenseVolume::empty([1, 1, 1]);
        let model = &data.models[0];
        for v in &model.voxels {
            let nx = v.x as u32;
            let ny = v.z as u32;
            let nz = v.y as u32;
            volume.set([nx, ny, nz], VoxelTypeId(v.i as u16 + 1));
        }
        let oracle = construct(&volume);

        assert_eq!(
            imp.world.size_in_chunks, oracle.size_in_chunks,
            "sparse vs. dense: size_in_chunks must match"
        );
        assert_eq!(
            imp.world.chunks, oracle.chunks,
            "sparse vs. dense: chunks_cpu must be byte-equal"
        );
        assert_eq!(
            imp.world.blocks, oracle.blocks,
            "sparse vs. dense: blocks_cpu must be byte-equal"
        );
        assert_eq!(
            imp.world.voxels, oracle.voxels,
            "sparse vs. dense: voxels_cpu must be byte-equal"
        );
    }

    // -- Test 9 (v2) — `build_world_from_vox` sets dense_voxel_types empty ----

    #[test]
    fn build_world_from_vox_skips_dense_voxel_types_on_sparse_path() {
        // v2 semantics — sparse `.vox` path installs `dense_voxel_types =
        // Vec::new()` (Δ-GPUProducer); the data-driven gate at
        // `render/construction/mod.rs:833-835` skips the GPU producer chain
        // and the renderer reads the pre-built CPU buffers.
        let data = build_small_cube();
        let imp = round_trip(&data);
        let (world, types) = build_world_from_vox(imp);
        assert!(
            world.dense_voxel_types.is_empty(),
            "sparse .vox path must set dense_voxel_types empty (Δ-GPUProducer)"
        );
        // `02f` rearch — `dirty` flag deleted; the GPU upload is gated by
        // `WorldGpu`-existence + the build-once `stage_world_gpu_buildonce`
        // extract path instead.
        let _ = types;
        assert_eq!(world.bounding_box.min, IVec3::ZERO);
        assert_eq!(world.bounding_box.max, IVec3::new(15, 15, 15));
        assert!(!world.chunks_cpu.is_empty(), "sparse path must produce chunks");
    }

    // -- Test 10 -------------------------------------------------------------

    #[test]
    fn load_vox_propagates_io_error() {
        let result = load_vox("/this/path/does/not/exist.vox");
        assert!(matches!(result, Err(VoxImportError::Io(_))));
    }

    // -- Scene-graph composition tests (from 03a-followup, migrated) ---------

    fn build_two_models_translated() -> dot_vox::DotVoxData {
        let mut materials = default_materials();
        for m in &mut materials {
            if m.id == 1 {
                m.properties.insert("_type".into(), "_emit".into());
                m.properties.insert("_emit".into(), "1.0".into());
            }
        }
        dot_vox::DotVoxData {
            version: 200,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![
                dot_vox::Model {
                    size: dot_vox::Size { x: 1, y: 1, z: 1 },
                    voxels: vec![dot_vox::Voxel { x: 0, y: 0, z: 0, i: 0 }],
                },
                dot_vox::Model {
                    size: dot_vox::Size { x: 1, y: 1, z: 1 },
                    voxels: vec![dot_vox::Voxel { x: 0, y: 0, z: 0, i: 1 }],
                },
            ],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials,
            scenes: vec![
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_default())],
                    child: 1,
                    layer_id: 0,
                },
                dot_vox::SceneNode::Group {
                    attributes: dict_default(),
                    children: vec![2, 4],
                },
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_with("_t", "10 0 0"))],
                    child: 3,
                    layer_id: 0,
                },
                dot_vox::SceneNode::Shape {
                    attributes: dict_default(),
                    models: vec![dot_vox::ShapeModel {
                        model_id: 0,
                        attributes: dict_default(),
                    }],
                },
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_with("_t", "0 20 0"))],
                    child: 5,
                    layer_id: 0,
                },
                dot_vox::SceneNode::Shape {
                    attributes: dict_default(),
                    models: vec![dot_vox::ShapeModel {
                        model_id: 1,
                        attributes: dict_default(),
                    }],
                },
            ],
            layers: Vec::new(),
        }
    }

    fn dict_default() -> dot_vox::Dict {
        StdHashMap::new().into_iter().collect()
    }

    fn dict_with(k: &str, v: &str) -> dot_vox::Dict {
        let mut d: dot_vox::Dict = StdHashMap::new().into_iter().collect();
        d.insert(k.into(), v.into());
        d
    }

    #[test]
    fn scene_graph_translations_separate_models() {
        let data = build_two_models_translated();
        let imp = parse_dot_vox_data(&data).unwrap();

        // World MV bounds: world_min=(0,0,0), world_max=(10,20,0). MV size
        // (11,21,1). After Z↔Y swap NAADF (11,1,21). Chunks (1,1,2).
        assert_eq!(imp.world.size_in_chunks, [1, 1, 2]);

        assert_eq!(count_nonempty(&imp.world), 2);
        assert_eq!(decoded_voxel_at(&imp.world, [10, 0, 0]), VoxelTypeId(1));
        assert_eq!(decoded_voxel_at(&imp.world, [0, 0, 20]), VoxelTypeId(2));
    }

    #[test]
    fn scene_graph_rotation_applies() {
        let r_byte = "17";
        let data = dot_vox::DotVoxData {
            version: 200,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 3, y: 3, z: 1 },
                voxels: vec![dot_vox::Voxel { x: 2, y: 1, z: 0, i: 0 }],
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: vec![
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_default())],
                    child: 1,
                    layer_id: 0,
                },
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_with("_r", r_byte))],
                    child: 2,
                    layer_id: 0,
                },
                dot_vox::SceneNode::Shape {
                    attributes: dict_default(),
                    models: vec![dot_vox::ShapeModel {
                        model_id: 0,
                        attributes: dict_default(),
                    }],
                },
            ],
            layers: Vec::new(),
        };

        let imp = parse_dot_vox_data(&data).unwrap();
        assert_eq!(count_nonempty(&imp.world), 1, "exactly one voxel must survive rotation");
    }

    #[test]
    fn rotation_byte_identity_and_axis_swap() {
        let r = Rot3::from_byte(4);
        assert_eq!(r.transform_vec([1, 0, 0]), [1, 0, 0]);
        assert_eq!(r.transform_vec([0, 1, 0]), [0, 1, 0]);
        assert_eq!(r.transform_vec([0, 0, 1]), [0, 0, 1]);

        let r = Rot3::from_byte(17);
        assert_eq!(r.transform_vec([1, 0, 0]), [0, 1, 0]);
        assert_eq!(r.transform_vec([0, 1, 0]), [-1, 0, 0]);
        assert_eq!(r.transform_vec([0, 0, 1]), [0, 0, 1]);
    }

    #[test]
    fn xform_compose_matches_csharp_order() {
        let parent = Xform { rot: Rot3::IDENTITY, t: [5, 0, 0] };
        let child = Xform { rot: Rot3::IDENTITY, t: [0, 3, 0] };
        let composed = child.parent_of(&parent);
        assert_eq!(composed.apply([0, 0, 0]), [5, 3, 0]);

        let parent = Xform { rot: Rot3::from_byte(17), t: [0, 0, 0] };
        let child = Xform { rot: Rot3::IDENTITY, t: [1, 0, 0] };
        let composed = child.parent_of(&parent);
        assert_eq!(composed.apply([0, 0, 0]), [0, 1, 0]);
    }

    // -- NEW v2 sparse-walk-specific tests -----------------------------------

    /// Test #16 — drive a 64×64×64-voxel scene (4×4×4 chunks) at ~1% density
    /// through the sparse walk. Verifies the path handles mid-sized worlds
    /// cleanly + the sparse code path is exercised (not just the trivial
    /// single-chunk fixtures).
    #[test]
    fn sparse_walk_handles_mid_sized_world() {
        // Single model 64×64×64 MV. Deterministic sparse fill — every 100th
        // voxel.
        let mut voxels = Vec::new();
        for z in 0..64u8 {
            for y in 0..64u8 {
                for x in 0..64u8 {
                    let idx = (x as u32) + (y as u32) * 64 + (z as u32) * 64 * 64;
                    if idx % 100 == 0 {
                        voxels.push(dot_vox::Voxel { x, y, z, i: 7 });
                    }
                }
            }
        }
        let expected_nonempty = voxels.len() as u32;

        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 64, y: 64, z: 64 },
                voxels,
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        };

        let imp = parse_dot_vox_data(&data).unwrap();
        assert_eq!(imp.world.size_in_chunks, [4, 4, 4]);
        assert_eq!(imp.world.chunks.len(), 64);
        // Some non-empty voxels must round-trip exactly. (Mostly emptyish
        // chunks give us non-empty `blocks` / `voxels`.)
        assert!(!imp.world.blocks.is_empty());
        assert!(!imp.world.voxels.is_empty());
        assert_eq!(count_nonempty(&imp.world), expected_nonempty);
    }

    /// Test #18 — two voxels at identical chunk-local positions in two
    /// different chunks (same 4³ block contents) ⇒ exactly one unique block
    /// in `voxels`. Verifies HashMap dedup fires on the sparse path.
    /// Mirrors `aadf::construct::tests::identical_blocks_dedup`.
    #[test]
    fn sparse_walk_dedups_identical_blocks() {
        // Two single-voxel models translated to MV (0,0,0) and (16,0,0),
        // each at local (0,0,0). After Z↔Y swap the two voxels sit at
        // NAADF (0,0,0) and (16,0,0) — distinct chunks (chunk-x = 0 and 1).
        // Their block-0 content (one voxel at block-local (0,0,0), of the
        // same type) is identical, so the dedup must fire.
        let data = dot_vox::DotVoxData {
            version: 200,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![
                dot_vox::Model {
                    size: dot_vox::Size { x: 1, y: 1, z: 1 },
                    voxels: vec![dot_vox::Voxel { x: 0, y: 0, z: 0, i: 7 }],
                },
                dot_vox::Model {
                    size: dot_vox::Size { x: 1, y: 1, z: 1 },
                    voxels: vec![dot_vox::Voxel { x: 0, y: 0, z: 0, i: 7 }],
                },
            ],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: vec![
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_default())],
                    child: 1,
                    layer_id: 0,
                },
                dot_vox::SceneNode::Group {
                    attributes: dict_default(),
                    children: vec![2, 4],
                },
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_default())],
                    child: 3,
                    layer_id: 0,
                },
                dot_vox::SceneNode::Shape {
                    attributes: dict_default(),
                    models: vec![dot_vox::ShapeModel {
                        model_id: 0,
                        attributes: dict_default(),
                    }],
                },
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_with("_t", "16 0 0"))],
                    child: 5,
                    layer_id: 0,
                },
                dot_vox::SceneNode::Shape {
                    attributes: dict_default(),
                    models: vec![dot_vox::ShapeModel {
                        model_id: 1,
                        attributes: dict_default(),
                    }],
                },
            ],
            layers: Vec::new(),
        };

        let imp = parse_dot_vox_data(&data).unwrap();
        // Two voxels in two distinct chunks, but their block-0 content is
        // identical → dedup must produce exactly 32 u32s in voxels_cpu
        // (one unique block, not two).
        assert_eq!(
            imp.world.voxels.len(),
            CELL_CHILDREN / 2,
            "expected exactly one unique 32-u32 block (dedup hit); got {} u32s",
            imp.world.voxels.len(),
        );
    }

    // -- Test (sparse XZ-tile feature) — replicate one tile N×N in XZ.
    //
    // vox-gpu-rewrite Stage 2 (2026-05-18): the runtime path no longer
    // exposes a tile-count knob, but the helper is still reachable via the
    // CPU oracle paths; this test pins block-dedup correctness across tiles.

    #[test]
    fn tiled_load_expands_world_xz_and_dedups_blocks() {
        // Drive the small-cube fixture through the tiled path at N=3. The
        // world size in chunks must scale by 3× in X and Z (Y unchanged) and
        // the block-dedup HashMap must collapse identical content across
        // tiles, so `voxels_cpu` length stays ≈ the single-tile output's
        // length (each unique block content appears exactly once regardless
        // of how many tile copies reference it).
        let data = build_small_cube();
        let single = parse_dot_vox_data_tiled(&data, 1).unwrap();
        let tiled = parse_dot_vox_data_tiled(&data, 3).unwrap();

        let s = single.world.size_in_chunks;
        let t = tiled.world.size_in_chunks;
        assert_eq!(t[0], s[0] * 3, "X axis must scale by tiles");
        assert_eq!(t[1], s[1], "Y axis stays unchanged");
        assert_eq!(t[2], s[2] * 3, "Z axis must scale by tiles");

        // chunks_cpu grows by tiles² (every tile contributes its own chunks).
        assert_eq!(
            tiled.world.chunks.len(),
            single.world.chunks.len() * 9,
            "chunks_cpu must scale by tiles² (3×3 = 9)"
        );

        // voxels_cpu must NOT scale with tile count — dedup collapses
        // identical block content across tiles.
        assert_eq!(
            tiled.world.voxels.len(),
            single.world.voxels.len(),
            "voxels_cpu length must be identical between tiled and untiled \
             (block dedup collapses identical content across tiles)"
        );

        // Sample-check: the same voxel pattern appears at tile-shifted
        // positions. The small-cube's distinguished emissive voxel sits at
        // NAADF coord (3, 3, 3) within the single tile (after Z↔Y swap).
        // In tile (1, 0) at chunk-offset (s[0]*16, 0, 0), it should appear
        // at NAADF (s[0]*16 + 3, 3, 3).
        let tile_w_voxels = s[0] * 16;
        let tile_d_voxels = s[2] * 16;
        assert_eq!(
            decoded_voxel_at(&tiled.world, [3, 3, 3]),
            decoded_voxel_at(&single.world, [3, 3, 3]),
            "tile (0,0) voxel must match single-tile output"
        );
        assert_eq!(
            decoded_voxel_at(&tiled.world, [tile_w_voxels + 3, 3, 3]),
            decoded_voxel_at(&single.world, [3, 3, 3]),
            "tile (1,0) emissive voxel must match single-tile content"
        );
        assert_eq!(
            decoded_voxel_at(&tiled.world, [3, 3, tile_d_voxels + 3]),
            decoded_voxel_at(&single.world, [3, 3, 3]),
            "tile (0,1) emissive voxel must match single-tile content"
        );
    }

}
