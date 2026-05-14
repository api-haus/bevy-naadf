//! `#[repr(C)]` bytemuck structs mirroring every WGSL struct / uniform
//! (`03-design.md` §5.2–5.3).
//!
//! These are the CPU side of the uniform/storage buffers the Phase-A render
//! passes bind. Each one is `#[repr(C)]` + `bytemuck::Pod` so it can be
//! `bytemuck::bytes_of`'d straight into a wgpu buffer, and is laid out to
//! match a WGSL `struct` with std140-ish (uniform) padding.
//!
//! Provenance: `GpuCamera` / `GpuRenderParams` mirror the uniform set of
//! `Content/shaders/render/versions/albedo/renderFirstHit.fx`
//! (`03-design.md` §5.2); `GpuVoxelType` mirrors `VoxelType.compressForRender()`
//! / `decompressVoxelType` (`03-design.md` §2.4, `02-research.md` §4.6);
//! `GpuWorldMeta` carries the world geometry the traversal shader needs.
//!
//! WGSL counterpart declarations live in `assets/shaders/world_data.wgsl`
//! (`GpuWorldMeta`, `GpuVoxelType`) and `assets/shaders/render_pipeline_common.wgsl`
//! (`GpuCamera`, `GpuRenderParams`) — keep the field order / padding in sync.

use bevy::math::{IVec3, Mat4, UVec3, Vec2, Vec3};
use bytemuck::{Pod, Zeroable};

use crate::voxel::{MaterialBase, MaterialLayer, VoxelType};

/// Camera uniform — the int+frac camera-relative position (D1) plus the
/// inverse view-projection matrix `getRayDir` needs.
///
/// Mirrors the albedo `renderFirstHit.fx` uniforms `matrix invCamMatrix;
/// int camPosIntX,Y,Z; float3 camPosFrac;`. The `shootRay` DDA takes
/// `cam_pos_int` / `cam_pos_frac` separately as `rayOriginInt` / `rayOriginFrac`
/// — no f32 world position is ever formed in the shader (`03-design.md` §5.2).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuCamera {
    /// `world_from_clip` — the inverse view-projection. `getRayDir` transforms
    /// an NDC ray by this and normalises; translation drops out.
    pub inv_view_proj: Mat4,
    /// Integer voxel position of the camera (C# `camPosIntX/Y/Z`).
    pub cam_pos_int: IVec3,
    /// std140 padding so `cam_pos_frac` starts on a 16-byte boundary.
    pub _pad0: u32,
    /// Fractional offset within the voxel, `[0,1)³` (C# `camPosFrac`).
    pub cam_pos_frac: Vec3,
    /// std140 padding to a 16-byte stride.
    pub _pad1: u32,
}

/// Render-params uniform — screen size, frame counters, sun term, jitter,
/// flags, and the world bounding box `rayAABB` tests against.
///
/// Mirrors the rest of the albedo `renderFirstHit.fx` uniform set
/// (`screenWidth/Height`, `frameCount`, `randCounter`, `taaIndex`,
/// `skySunDir`, `sunColor`, `taaJitter`, the `showRayStep`/`checkSun`/`isTAA`
/// bools) plus `renderFinal.fx`'s `exposure`. The bools are packed into
/// `flags` (see the `FLAG_*` constants). Phase A sets `is_taa = 0` (D4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuRenderParams {
    /// Render-target width in pixels.
    pub screen_width: u32,
    /// Render-target height in pixels.
    pub screen_height: u32,
    /// Frames rendered so far (the C# `frameCount`).
    pub frame_count: u32,
    /// Per-frame RNG salt (the C# `randCounter`).
    pub rand_counter: u32,

    /// TAA history slot index (the C# `taaIndex`). Unused in Phase A (`is_taa`
    /// is always 0) but kept so the uniform layout is Phase-A-2-ready.
    pub taa_index: u32,
    /// Packed boolean flags — see `FLAG_SHOW_RAY_STEP` / `FLAG_CHECK_SUN` /
    /// `FLAG_IS_TAA`.
    pub flags: u32,
    /// Final-blit tonemap exposure (the C# `renderFinal.fx` `exposure`).
    pub exposure: f32,
    /// Padding to a 16-byte boundary before the first `vec3`.
    pub _pad0: u32,

    /// Direction *towards* the sun (the C# `skySunDir`).
    pub sky_sun_dir: Vec3,
    /// Padding so `sun_color` starts on a 16-byte boundary.
    pub _pad1: u32,

    /// Sun radiance colour (the C# `sunColor`).
    pub sun_color: Vec3,
    /// Padding to a 16-byte stride.
    pub _pad2: u32,

    /// Sub-pixel TAA jitter (the C# `taaJitter`). Zero in Phase A (D4).
    pub taa_jitter: Vec2,
    /// Padding to a 16-byte boundary before the next `vec3`.
    pub _pad3: Vec2,

    /// World geometry AABB minimum, in voxels (the C# `boundingBoxMin`).
    pub bounding_box_min: Vec3,
    /// Padding so `bounding_box_max` starts on a 16-byte boundary.
    pub _pad4: u32,

    /// World geometry AABB maximum, in voxels (the C# `boundingBoxMax`).
    pub bounding_box_max: Vec3,
    /// Padding to a 16-byte stride.
    pub _pad5: u32,
}

/// `flags` bit: show per-pixel ray step count instead of shaded colour
/// (the C# `showRayStep`).
pub const FLAG_SHOW_RAY_STEP: u32 = 1 << 0;
/// `flags` bit: trace a shadow ray towards the sun (the C# `checkSun`).
pub const FLAG_CHECK_SUN: u32 = 1 << 1;
/// `flags` bit: write a TAA sample. Always clear in Phase A (D4).
pub const FLAG_IS_TAA: u32 = 1 << 2;

/// World-meta uniform — the world geometry the traversal shader needs that is
/// not per-frame: the chunk-grid extent and the voxel-space bounding box.
///
/// Mirrors `rayTracing.fxh`'s `groupSizeX/Y/Z` + `boundingBoxMin/Max` globals
/// (`03-design.md` §2.6 — the small `WorldMeta` uniform in `@group(0)`).
///
/// `bounding_box_min/max` are `float3` (not integers) — faithful to NAADF's
/// `rayTracing.fxh` (`float3 boundingBoxMin, boundingBoxMax;`). NAADF's
/// `WorldData.setEffect` (`WorldData.cs:477-478`) writes them as the world
/// extent **inset by 0.1 voxel** on every side — `boundingBoxMin = (0.1,0.1,0.1)`,
/// `boundingBoxMax = sizeInVoxels - (0.1,0.1,0.1)`. That inset keeps the
/// ray-AABB entry point off the integer voxel planes so `floor()` of the
/// entry point is unambiguous; an integer-inclusive box (`min..=size-1`)
/// instead puts the entry point exactly on a voxel boundary and `floor()`
/// flips with f32 noise — the out-of-volume concentric-lines artifact.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuWorldMeta {
    /// World size in chunks.
    pub size_in_chunks: UVec3,
    /// Padding to a 16-byte boundary.
    pub _pad0: u32,
    /// Geometry AABB minimum, in voxels — NAADF's `boundingBoxMin` (the
    /// 0.1-voxel-inset world minimum, `WorldData.cs:477`).
    pub bounding_box_min: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad1: u32,
    /// Geometry AABB maximum, in voxels — NAADF's `boundingBoxMax`
    /// (`sizeInVoxels - 0.1`, `WorldData.cs:478`).
    pub bounding_box_max: Vec3,
    /// Padding to a 16-byte stride.
    pub _pad2: u32,
}

/// GPU material entry — the 128-bit (`UVec4`) form of a [`VoxelType`], mirroring
/// the C# `VoxelType.compressForRender()` (`03-design.md` §2.4):
///
/// - `data[0]` = `base | layer << 2 | f16(roughness) << 16`
/// - `data[1]` = `f16(color_base.r) | f16(color_base.g) << 16`
/// - `data[2]` = `f16(color_base.b) | f16(color_layered.r) << 16`
/// - `data[3]` = `f16(color_layered.g) | f16(color_layered.b) << 16`
///
/// Decoded GPU-side by `decompressVoxelType` in `render_pipeline_common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuVoxelType {
    /// The four packed `u32`s (the C# `Uint4`).
    pub data: [u32; 4],
}

impl GpuVoxelType {
    /// Compress a CPU [`VoxelType`] into its 128-bit GPU form.
    pub fn from_voxel_type(ty: &VoxelType) -> GpuVoxelType {
        let base = ty.material_base as u32 & 0x3;
        let layer = ty.material_layer as u32 & 0x3;
        let rough = f16_bits(ty.roughness);
        let data0 = base | (layer << 2) | ((rough as u32) << 16);
        let data1 = (f16_bits(ty.color_base.x) as u32)
            | ((f16_bits(ty.color_base.y) as u32) << 16);
        let data2 = (f16_bits(ty.color_base.z) as u32)
            | ((f16_bits(ty.color_layered.x) as u32) << 16);
        let data3 = (f16_bits(ty.color_layered.y) as u32)
            | ((f16_bits(ty.color_layered.z) as u32) << 16);
        GpuVoxelType {
            data: [data0, data1, data2, data3],
        }
    }
}

/// IEEE-754 half-float bit pattern of `x` (the C# `f32tof16`).
///
/// A straightforward round-to-nearest-even f32 → f16 conversion, sufficient
/// for the small positive colour / roughness values Phase A stores.
fn f16_bits(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mantissa = bits & 0x007F_FFFF;

    if exp == 0xFF {
        // Inf / NaN.
        return sign | 0x7C00 | if mantissa != 0 { 0x0200 } else { 0 };
    }
    // Re-bias the exponent from 127 (f32) to 15 (f16).
    let new_exp = exp - 127 + 15;
    if new_exp >= 0x1F {
        // Overflow → Inf.
        return sign | 0x7C00;
    }
    if new_exp <= 0 {
        // Subnormal or underflow to zero.
        if new_exp < -10 {
            return sign;
        }
        let mant = (mantissa | 0x0080_0000) >> (1 - new_exp);
        // Round to nearest even.
        let rounded = (mant + 0x0000_1000) >> 13;
        return sign | rounded as u16;
    }
    // Normal half-float; round the 23-bit mantissa down to 10 bits.
    let half = sign | ((new_exp as u16) << 10) | ((mantissa >> 13) as u16);
    // Round to nearest even using the dropped low 13 bits.
    if mantissa & 0x0000_1000 != 0 {
        half + 1
    } else {
        half
    }
}

// Compile-time sanity: the GPU structs must be exactly the size their WGSL
// counterparts declare (no surprise padding). `GpuRenderParams` is 7 × 16-byte
// rows: (u32×4) (taa_index/flags/exposure/_pad0) (sky_sun_dir/_pad1)
// (sun_color/_pad2) (taa_jitter/_pad3) (bbox_min/_pad4) (bbox_max/_pad5).
const _: () = assert!(std::mem::size_of::<GpuCamera>() == 64 + 32);
const _: () = assert!(std::mem::size_of::<GpuRenderParams>() == 16 * 7);
const _: () = assert!(std::mem::size_of::<GpuWorldMeta>() == 48);
const _: () = assert!(std::mem::size_of::<GpuVoxelType>() == 16);

// Keep the material enums referenced so a future material-format change can't
// silently drift this file out of step (also documents the intent).
const _: () = {
    let _ = MaterialBase::Diffuse;
    let _ = MaterialLayer::None;
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_round_trips_simple_values() {
        // 0.0, 1.0, 0.5 have exact half-float representations.
        assert_eq!(f16_bits(0.0), 0x0000);
        assert_eq!(f16_bits(1.0), 0x3C00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(2.0), 0x4000);
    }

    #[test]
    fn gpu_voxel_type_packs_base_layer_roughness() {
        let ty = VoxelType {
            material_base: MaterialBase::Emissive, // 1
            material_layer: MaterialLayer::MetallicMirror, // 3
            roughness: 1.0,
            color_base: Vec3::new(1.0, 0.0, 0.0),
            color_layered: Vec3::new(0.0, 1.0, 0.0),
        };
        let g = GpuVoxelType::from_voxel_type(&ty);
        // data[0] low bits: base=1, layer=3 → 1 | (3<<2) = 0b1101 = 13.
        assert_eq!(g.data[0] & 0xFFFF, 13);
        // roughness 1.0 → f16 0x3C00 in the high half-word.
        assert_eq!(g.data[0] >> 16, 0x3C00);
        // color_base.r = 1.0 → 0x3C00 low half of data[1]; .g = 0.0 → 0 high.
        assert_eq!(g.data[1] & 0xFFFF, 0x3C00);
        assert_eq!(g.data[1] >> 16, 0x0000);
    }
}
