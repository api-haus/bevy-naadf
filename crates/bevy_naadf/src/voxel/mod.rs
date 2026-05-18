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
pub mod vox_import;

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

/// Base material class of a voxel type — post-PBR-raymarching pivot.
///
/// Collapses the C# 4-value `MaterialTypeBase` to a 1-bit `{ PBR, Emissive }`
/// flag. Every PBR hit runs the unified BRDF in `eval_pbr()`
/// (`pbr_sampling.wgsl`); every Emissive hit takes the fast-path (no BRDF,
/// no PBR-array samples). Metallic / roughness / height are now texture-
/// driven (sampled from the `MaterialSet` MRH array) — NOT per-voxel-type
/// scalars. See `docs/orchestrate/pbr-raymarching/02-design.md` § A.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum MaterialBase {
    /// All-PBR — the unified GGX-Smith-Schlick BRDF in
    /// `pbr_sampling.wgsl::eval_pbr`, with `metallic` / `roughness` / `height`
    /// read from the MRH texture array at `material_layer_index`.
    #[default]
    Pbr = 0,
    /// Emissive fast-path — skip the BRDF, sample the Emissive texture array,
    /// multiply by `color_layered` HDR intensity.
    Emissive = 1,
}

/// One entry of the voxel-type palette — the per-voxel-type CPU material
/// data, post-PBR-raymarching pivot.
///
/// **All physical-material parameters (albedo RGB, metallic, roughness,
/// height, AO, tangent-space normal) live in the `MaterialSet` texture
/// arrays at `material_layer_index`; this struct only carries the
/// per-VoxelType bits that *select and tint* the texture sample.**
///
/// The 4 C# `MaterialBase` branches collapse to a 1-bit flag here
/// (`PBR` vs `Emissive`) — see [`MaterialBase`].
///
/// **User-approved divergence from C# NAADF**
/// (`docs/orchestrate/pbr-raymarching/01-context.md` D1, D4): the C#
/// `VoxelType` has no texture-array layer index and carries `color_base`
/// as IOR. This port replaces that with a texture-array layer +
/// `albedo_tint`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct VoxelType {
    /// Base material class — `Pbr` or `Emissive`. Replaces the 4-value C#
    /// `MaterialBase` enum (the `MetallicRough` / `MetallicMirror` values
    /// are removed; metallic comes from the texture sample now).
    pub material_base: MaterialBase,
    /// 0-based index into the `MaterialSet` texture arrays (diffuse_ao,
    /// normal, mrh, emissive — all share the layer-index space). 12-bit
    /// on the GPU (4096 distinct materials max).
    pub material_layer_index: u16,
    /// sRGB byte tint applied multiplicatively to the sampled albedo, like
    /// Bevy's `StandardMaterial.base_color × base_color_texture`. 8-bit
    /// per channel (24 bits total) on the GPU. The neutral value is
    /// `[255, 255, 255]` (no tint).
    pub albedo_tint: [u8; 3],
    /// Layered RGB — **emissive HDR colour multiplier** when
    /// `material_base == Emissive`. The Emissive fast-path output is
    /// `sampled_emissive_rgb × color_layered` (see design § H). Carried
    /// as 3× f16 on the GPU. Unused (zeroed) when `material_base == Pbr`.
    pub color_layered: Vec3,
}

impl Default for VoxelType {
    /// The reserved empty placeholder (material-buffer element 0): a PBR
    /// surface pointing at material-layer 0 (fabric), no tint, no emissive.
    fn default() -> Self {
        Self {
            material_base: MaterialBase::Pbr,
            material_layer_index: 0,
            albedo_tint: [255, 255, 255],
            color_layered: Vec3::ZERO,
        }
    }
}
