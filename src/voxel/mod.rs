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

pub mod grid;

use bevy::prelude::Vec3;

// ---------------------------------------------------------------------------
// Cell-state bit layout (paper §3.1; C# re-encoding — `02-research.md` §1.1.2)
// ---------------------------------------------------------------------------

/// Bit 31 of a chunk/block `u32`: set ⇒ the cell is **mixed** (has children),
/// low 30 bits are a child pointer. C# `curNode.x >> 31`.
pub const CELL_HAS_CHILDREN: u32 = 1 << 31;

/// Bit 30 of a chunk/block `u32`: set ⇒ the cell is **uniformly full**, low
/// bits hold the 15-bit voxel type. C# `curNode.x & 0x40000000`.
pub const CELL_UNIFORM_FULL: u32 = 1 << 30;

/// Mask for the 30-bit payload of a chunk/block `u32` (AADF when empty, child
/// pointer when mixed).
pub const CELL_PAYLOAD_MASK: u32 = 0x3FFF_FFFF;

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
