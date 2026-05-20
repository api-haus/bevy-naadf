//! Voxel type / layered-material system + cell-state bit-layout constants.
//!
//! Ports NAADF's `World/VoxelTypeHandler.cs` (`02-research.md` §4.6) — the
//! [`VoxelType`] palette entry and the [`MaterialBase`] / [`MaterialLayer`]
//! enums — plus the shared cell-state bit-layout constants the AADF cells
//! (`crate::aadf::cell`) encode against (`03-design.md` §2.2).
//!
//! The bit layouts are re-derived from paper §3.1 in the *exact* re-encoding
//! the C# traversal shader uses (`02-research.md` §1.1.2, divergence #3) so the
//! eventual WGSL traversal port bit-matches the algorithm.

pub mod async_vox;
pub mod cvox_import;
pub mod grid;
pub mod vox_import;
pub mod voxel_dispatch;

#[cfg(target_arch = "wasm32")]
pub mod web_vox;

use bevy::prelude::Vec3;

// ---------------------------------------------------------------------------
// Cell-state bit layout (paper §3.1; C# re-encoding — `02-research.md` §1.1.2)
// ---------------------------------------------------------------------------

/// Cell-state nibble at bits 30-31 of a `Chunk`/`Block` `u32`.
///
/// Faithful port of the C# `WorldData.cs:223` `chunk >> 30` discriminator.
/// Mirrors the WGSL `BLOCK_STATE_*` constants in `chunk_calc.wgsl:246-248`,
/// `world_change.wgsl:161-163`, `bounds_calc.wgsl:180-182`. State `3` is
/// reserved / unused (C# never emits it).
pub mod cell_state {
    /// State value for **empty** cells — bits 30-31 = `0b00`. Low 30 bits
    /// carry the AADF (6-direction empty distance — 5-bit fields at chunk
    /// layer, 2-bit at block/voxel layer).
    pub const UNIFORM_EMPTY: u32 = 0;
    /// State value for **uniform-full** cells — bits 30-31 = `0b01`. Low 15
    /// bits carry the voxel type id.
    pub const UNIFORM_FULL: u32 = 1;
    /// State value for **mixed** cells — bits 30-31 = `0b10`. Low 30 bits
    /// carry the child-group pointer (block ptr at chunk layer, voxel ptr at
    /// block layer).
    pub const CHILD: u32 = 2;

    /// Bit shift applied to extract `state` from a raw cell word
    /// (`raw >> SHIFT`). C# `chunk >> 30`.
    pub const SHIFT: u32 = 30;
}

/// Mask for the 30-bit payload of a chunk/block `u32` (AADF when empty, child
/// pointer when mixed, or 15-bit voxel type when uniform-full).
pub const CELL_PAYLOAD_MASK: u32 = 0x3FFF_FFFF;

/// Typed view over a chunk/block cell `u32` word — the regime-B state nibble
/// + 30-bit payload. Same idiom as the existing `pack_chunk_pos` /
/// `unpack_chunk_pos` helpers in `aadf::edit`.
///
/// Used at hot decode sites that want to avoid the `Cell::decode` enum
/// branch. Zero-cost — `repr(transparent)` over a `u32`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct CellRaw(pub u32);

impl CellRaw {
    /// Extract the 2-bit state nibble (one of [`cell_state::UNIFORM_EMPTY`],
    /// [`cell_state::UNIFORM_FULL`], [`cell_state::CHILD`]).
    #[inline]
    pub const fn state(self) -> u32 {
        self.0 >> cell_state::SHIFT
    }
    /// Extract the 30-bit payload (AADF / child-ptr / 15-bit type).
    #[inline]
    pub const fn payload(self) -> u32 {
        self.0 & CELL_PAYLOAD_MASK
    }
    /// Construct a raw word from a state value and 30-bit payload.
    #[inline]
    pub const fn new(state: u32, payload: u32) -> Self {
        Self((state << cell_state::SHIFT) | (payload & CELL_PAYLOAD_MASK))
    }
    /// True iff `state() == cell_state::CHILD`.
    #[inline]
    pub const fn is_child(self) -> bool {
        self.state() == cell_state::CHILD
    }
    /// True iff `state() == cell_state::UNIFORM_EMPTY`.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.state() == cell_state::UNIFORM_EMPTY
    }
}

/// Bit 15 of a voxel `u16`: set ⇒ the voxel is **full** (low 15 bits = type),
/// clear ⇒ **empty** (low 15 bits = AADF). C# voxel `>> 15`.
pub const VOXEL_FULL_FLAG: u16 = 1 << 15;

/// Mask for the 15-bit payload of a voxel `u16` (voxel type when full, AADF
/// when empty).
pub const VOXEL_PAYLOAD_MASK: u16 = 0x7FFF;

/// Valid range of a 15-bit voxel-type id (`0..=0x7FFF`).
pub const VOXEL_TYPE_MAX: u16 = 0x7FFF;

/// Per-direction AADF field width for **chunk** cells: 5 bits, max distance 31.
pub const AADF_BITS_CHUNK: u32 = 5;
/// Maximum AADF distance a **chunk** cell can store (`2^5 - 1`).
pub const AADF_MAX_CHUNK: u8 = 31;

/// Per-direction AADF field width for **block** and **voxel** cells: 2 bits,
/// max distance 3.
pub const AADF_BITS_SMALL: u32 = 2;
/// Maximum AADF distance a **block** or **voxel** cell can store (`2^2 - 1`).
pub const AADF_MAX_SMALL: u8 = 3;

/// Side length of a cell in cells of the layer below — every NAADF layer is a
/// 4×4×4 grid of the layer beneath it (paper §3.1).
pub const CELL_DIM: usize = 4;
/// Child cells per cell (`CELL_DIM³ = 64`).
pub const CELL_CHILDREN: usize = CELL_DIM * CELL_DIM * CELL_DIM;
/// Side length of a **chunk** in voxels (`CELL_DIM² = 16`). Single Rust SSoT
/// — every D1 file derives this; WGSL receives it via shader-defs (SSoT-3).
pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM;
/// Voxels per chunk (`CHUNK_DIM_VOXELS³ = 4096` — same as 64 blocks × 64
/// voxels/block).
pub const CHUNK_VOLUME_VOXELS: usize =
    CHUNK_DIM_VOXELS * CHUNK_DIM_VOXELS * CHUNK_DIM_VOXELS;

// ---------------------------------------------------------------------------
// Voxel-type / material system (`World/VoxelTypeHandler.cs`, `02-research.md` §4.6)
// ---------------------------------------------------------------------------

/// 15-bit voxel-type id — an index into the material buffer (`VoxelTypes`).
/// Element `0` is the reserved empty placeholder (C# convention).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct VoxelTypeId(pub u16);

impl VoxelTypeId {
    /// The reserved empty-placeholder type (material-buffer element 0).
    pub const EMPTY: VoxelTypeId = VoxelTypeId(0);

    /// The raw 15-bit id, masked into valid range.
    pub fn raw(self) -> u16 {
        self.0 & VOXEL_PAYLOAD_MASK
    }
}

/// Base material class of a voxel type (C# `MaterialTypeBase`).
///
/// Phase A only needs to distinguish *emissive* from the rest for albedo; the
/// metal/mirror BRDF and emissive contribution are Phase B (`02-research.md`
/// §4.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum MaterialBase {
    #[default]
    Diffuse = 0,
    Emissive = 1,
    MetallicRough = 2,
    MetallicMirror = 3,
}

/// Optional second material layer of a voxel type (C# `MaterialTypeLayer`).
/// Note `1` is intentionally absent — the C# enum skips it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum MaterialLayer {
    #[default]
    None = 0,
    MetallicRough = 2,
    MetallicMirror = 3,
}

/// One entry of the voxel-type palette (C# `VoxelType`, `02-research.md` §4.6).
///
/// Follows the C# 128-bit `Uint4` entry, not the paper's 16-bit summary
/// (`03-design.md` §2.4). Phase A uses only `color_base` (albedo) and
/// `material_base` (emissive-vs-diffuse); the full layout is built so Phase B
/// needs no data-format change.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct VoxelType {
    /// Base material class.
    pub material_base: MaterialBase,
    /// Optional second material layer.
    pub material_layer: MaterialLayer,
    /// Surface roughness (`f16` on the GPU).
    pub roughness: f32,
    /// Base RGB albedo.
    pub color_base: Vec3,
    /// Layered RGB — emissive intensity for `Emissive`, tint for layered metals.
    pub color_layered: Vec3,
}

impl Default for VoxelType {
    /// The reserved empty placeholder (material-buffer element 0): a black
    /// diffuse surface.
    fn default() -> Self {
        Self {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::ZERO,
            color_layered: Vec3::ZERO,
        }
    }
}
