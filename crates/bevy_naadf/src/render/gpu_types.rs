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
//!
//! Phase A-2 adds `GpuTaaParams` + `GpuCameraHistorySlot` (the TAA reproject
//! pass's uniform + the 128-deep camera-history ring slot, `06-design-a2.md`
//! §4.2–4.3); their WGSL counterparts live in `assets/shaders/taa.wgsl`.

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
    /// Padding (offsets 24/28). Formerly `exposure` / `tone_mapping_fac` — the
    /// custom final-blit tonemap constants. The TAA-fidelity track switched the
    /// port to Bevy's built-in tonemapping (`Camera { hdr: true }` + a
    /// `Tonemapping` component; `naadf_final.wgsl` outputs raw linear HDR), so
    /// these fields are dead — replaced with padding to keep the 112-byte
    /// uniform layout (and every downstream field offset) unchanged
    /// (`18-taa-fidelity.md` fix #2).
    pub _pad0a: u32,
    /// Padding — see `_pad0a`.
    pub _pad0b: u32,

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
/// `flags` bit: the `base/` first-hit ray-marches the atmosphere along each
/// primary-ray segment travelled (the C# `WorldRenderBase.isAtmosphereInteraction`
/// — `WorldRenderBase.cs:16,224`; defaults to `true`). Phase B / Batch 2.
pub const FLAG_IS_ATMOSPHERE_INTERACTION: u32 = 1 << 3;
/// `flags` bit: the final blit decodes its source as `final_color`'s packing
/// (`.x = f16(r)|f16(g)<<16, .y = f16(b)`, no weight) rather than
/// `taa_sample_accum`'s packing (`.x = f16(weight)|f16(r)<<16, ...`). This is
/// the **Batch-2 deliberate temporary seam** (`09-design-b.md` §11 Batch 2
/// step 8): the `base/` first-hit no longer writes `taa_sample_accum` and the
/// TAA reproject node is out of the chain, so the blit reads `final_color`
/// directly. Batch 6 wires `CalcNewTaaSample`, reverts the blit source to
/// `taa_sample_accum`, and clears this bit.
pub const FLAG_BLIT_FINAL_COLOR: u32 = 1 << 4;

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

/// TAA reproject-pass uniform — the dedicated scalar uniform for the
/// `taa.wgsl` reproject pass (`06-design-a2.md` §4.2).
///
/// Mirrors `renderTaaSampleReverse.fx:10-21`'s scalar uniforms. It overlaps
/// `GpuRenderParams` but is not identical (it adds `camMatrix` / `sampleAge` /
/// the camera-relative position), so it is its own uniform rather than a
/// widening of `GpuRenderParams`.
///
/// Layout: `mat4 (64) + mat4 (64) + (ivec3+pad) (16) + (vec3+pad) (16) +
/// 4×u32 (16) + 4×u32 (16)` = 192 bytes, 16-byte aligned throughout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuTaaParams {
    /// Rotation-only inverse view-proj (C# `invCamMatrix`) — for `get_ray_dir`.
    /// The same matrix Phase A puts in `GpuCamera.inv_view_proj`.
    pub inv_view_proj: Mat4,
    /// Translation-free view-proj of the CURRENT frame (C# `camMatrix`) —
    /// projects a reprojected virtual pos into the current screen for the
    /// 1-pixel reject test.
    pub view_proj: Mat4,
    /// Current camera integer position (C# `camPosInt`) — base for the
    /// camera-relative reprojection space.
    pub cam_pos_int: IVec3,
    /// Padding to a 16-byte boundary.
    pub _pad0: u32,
    /// Current camera fractional position (C# `camPosFrac`).
    pub cam_pos_frac: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad1: u32,
    /// Render-target width in pixels.
    pub screen_width: u32,
    /// Render-target height in pixels.
    pub screen_height: u32,
    /// Monotonic frame counter (C# `frameCount`).
    pub frame_count: u32,
    /// `taaIndex = CAMERA_HISTORY_DEPTH - (frame_count % CAMERA_HISTORY_DEPTH) - 1`.
    pub taa_index: u32,
    /// How many past frames to walk (C# `sampleAge` / `taaSampleMaxAge`).
    /// Clamped to `[1, TAA_SAMPLE_RING_DEPTH]` in A-2 (`06-design-a2.md` §7.1).
    pub sample_age: u32,
    /// Padding to a 16-byte stride.
    pub _pad2: u32,
    /// Padding to a 16-byte stride.
    pub _pad3: u32,
    /// Padding to a 16-byte stride.
    pub _pad4: u32,
}

/// One slot of the 128-deep camera-history ring, GPU side
/// (`06-design-a2.md` §4.3, `09-design-b.md` §3.6).
///
/// The reproject shader indexes `camRotOld[128]`,
/// `taaOldCamPosFromCurCamInt[128]`, `taaJitterOld[128]` — this struct packs
/// all three per-slot. Bound as a read-only storage buffer
/// (`array<GpuCameraHistorySlot, 128>`); created once, rewritten every frame.
///
/// Phase B adds `view_proj_inv` (the C# `taaSampleCamTransformInvers[128]`
/// inverse rotation-only view-proj ring — `WorldRenderBase.cs:147,162`). The
/// `base/` GI passes need BOTH the non-inverse ring (`globalIllum` /
/// `renderTaaSampleReverse` bind it as `camRotOld`) and the inverse ring
/// (`renderSampleRefine` binds the *inverse* into its same `camRotOld`
/// parameter and calls `getRayDir` with it — `09-design-b.md` §3.6). So the
/// slot carries both.
///
/// Layout: `mat4 (64) + mat4 (64) + (vec3+pad) (16) + (vec2+vec2pad) (16)` =
/// 160 bytes/slot, 16-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuCameraHistorySlot {
    /// Past frame's translation-free view-proj (C# `camRotOld[i]` /
    /// `taaSampleCamTransform[i]`).
    pub view_proj: Mat4,
    /// Past frame's *inverse* translation-free view-proj (C#
    /// `taaSampleCamTransformInvers[i]`) — `renderSampleRefine`'s `camRotOld`.
    /// Computed in `update_camera_history` as `view_proj.inverse()`.
    pub view_proj_inv: Mat4,
    /// Past frame's camera pos, relative to the CURRENT camera int position
    /// (C# `taaOldCamPosFromCurCamInt[i] = (oldCamPositions[i] - camPos).toVector3()`).
    /// Recomputed every frame in `prepare_taa`.
    pub cam_pos_from_cur_int: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad0: u32,
    /// Past frame's Halton jitter (C# `taaJitterOld[i]`).
    pub jitter: Vec2,
    /// Padding to a 16-byte stride.
    pub _pad1: Vec2,
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

// ===========================================================================
// Phase B GPU structs (`09-design-b.md` §3) — WGSL counterparts in
// `assets/shaders/atmosphere.wgsl` (`GpuAtmosphereParams`),
// `assets/shaders/render_pipeline_common.wgsl` (`SampleValid`), and the Phase-B
// GI entry shaders (`GpuGiParams`).
// ===========================================================================

/// Atmosphere precompute uniform (`09-design-b.md` §3.9).
///
/// Mirrors the `atmosphereRaw.fxh:6-19` sky uniforms (from `UiSkyDebug.cs`'s
/// field defaults + the `SetShaderData` scaling at `UiSkyDebug.cs:63-79`) plus
/// `renderAtmosphere.fx:6-7`'s `camPos` / `atmosphereTexSizeX/Y` / `frameCount`.
/// The sky parameters are compile-time constants in the port (no GUI); only
/// `cam_pos` / `sky_sun_dir` / `frame_count` are per-frame.
///
/// `skyAtmosphereAveragePoint` (in `UiSkyDebug`) is never read by the shaders —
/// omitted. Layout: 5 × `vec3` 16-byte rows + 12 trailing scalars + 1 = 128 B.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuAtmosphereParams {
    /// Camera world position — only `.y` is read (`renderAtmosphere.fx:25`).
    pub cam_pos: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad0: u32,
    /// Direction *towards* the sun (C# `skySunDir`).
    pub sky_sun_dir: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad1: u32,
    /// Rayleigh scatter coefficients (C# `skyRayleighScatter` = `(5.802,
    /// 13.558, 33.1)`).
    pub sky_rayleigh_scatter: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad2: u32,
    /// Ozone absorption coefficients (C# `skyOzoneAbsorb` = `(0.650, 1.881,
    /// 0.085)`).
    pub sky_ozone_absorb: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad3: u32,
    /// Sun colour × intensity (C# `skySunColor * skySunIntensity` = `(1,1,1)*10`).
    pub sky_sun_color: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad4: u32,
    /// Mie scatter coefficient (C# `skyMieScatter` = `2.5`).
    pub sky_mie_scatter: f32,
    /// Planet sphere radius (C# `skySphereRadius` = `50000.0 * 100`).
    pub sky_sphere_radius: f32,
    /// Atmosphere shell thickness (C# `skyAtmosphereThickness` = `50000.0`).
    pub sky_atmosphere_thickness: f32,
    /// Atmosphere density (C# `skyAtmosphereDensity * 0.01` = `14.0 * 0.01`).
    pub sky_atmosphere_density: f32,
    /// Absorption intensity (C# `skyAbsorbIntensity` = `3.0`).
    pub sky_absorb_intensity: f32,
    /// Scatter intensity (C# `skyScatterIntensity * 0.000001` = `1.35e-6`).
    pub sky_scatter_intensity: f32,
    /// Mie phase-function asymmetry (C# `skyMieFactor` = `0.85`).
    pub sky_mie_factor: f32,
    /// Main ray-march step count (C# `skyMainRaySteps` = `24`).
    pub sky_main_ray_steps: u32,
    /// Sub-scatter ray-march step count (C# `skySubScatterSteps` = `6`).
    pub sky_sub_scatter_steps: u32,
    /// Octahedral atmosphere-buffer width (`ATMOSPHERE_TEX_SIZE` = `1024`).
    pub atmosphere_tex_size_x: u32,
    /// Octahedral atmosphere-buffer height (`ATMOSPHERE_TEX_SIZE` = `1024`).
    pub atmosphere_tex_size_y: u32,
    /// Monotonic frame counter — the quarter-per-frame stride
    /// (`renderAtmosphere.fx:12`).
    pub frame_count: u32,
}

/// The compressed lit GI sample — `Uint8` / 32 bytes
/// (`09-design-b.md` §3.2; C# `globalIlumValidSamples` element).
///
/// 8 raw `u32`s — the GI shaders pack/unpack the bitfields directly
/// (`renderGlobalIllum.fx:34-48` `compressSampleValid`); the CPU never reads or
/// writes individual samples (GPU-only working data), so there is no benefit to
/// a fielded struct. The WGSL counterpart is `SampleValid { data1: vec4<u32>,
/// data2: vec4<u32> }` in `render_pipeline_common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuSampleValid {
    /// `data1` (uvec4) + `data2` (uvec4), packed exactly as `compressSampleValid`.
    pub data: [u32; 8],
}

/// One 8×8 screen-space region's bucket info — `Uint2` / 8 bytes
/// (`09-design-b.md` §3; C# `globalIlumBucketInfo` element).
///
/// 2 raw `u32`s — GPU-only working data, packed/unpacked directly by
/// `renderSampleRefine.fx` / `renderSpatialResampling.fx`. Where atomically
/// written the WGSL declares the buffer as `array<atomic<u32>>` /
/// `array<SampleCountSlot>` — that is a WGSL binding-type concern, not a
/// layout-size concern; the byte layout is two `u32`s either way.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuBucketInfo {
    /// The two packed `u32`s.
    pub data: [u32; 2],
}

/// Per-frame GI uniform — the union of every GI pass's scalar uniforms
/// (`09-design-b.md` §3.8; verified against `rayQueueCalc.fx:9-10`,
/// `renderGlobalIllum.fx:16-28`, `renderSampleRefine.fx:21-32`,
/// `renderSpatialResampling.fx:15-27`, `renderDenoiseSplit.fx:11-14`).
///
/// One shared uniform bound by every GI pass. The `WorldRenderBase` ImGui
/// sliders become the constant fields here (no GUI in the port — §1 / §3.8).
/// Created + uploaded by `prepare_gi` (Batch 3); declared in Batch 1 so the
/// layout + size assert exist for the GI passes that arrive in Batches 3-5.
///
/// Layout: `mat4 (64) + mat4 (64)` then 4 × 16-byte `vec3` rows
/// (`cam_pos_int`/`cam_pos_frac`/`sky_sun_dir`/`sun_color`, 64 B) then a 24-slot
/// scalar tail (`screen_width` … the last 16-byte row `flags`/`_pad4`/
/// `taa_jitter`, 96 B) — total 288 bytes. The TAA-fidelity fix replaced the
/// trailing `_pad5`/`_pad6` pair with `taa_jitter: Vec2` (offset 280, 8-byte
/// aligned) — the GI rays' per-frame Halton sub-pixel jitter.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuGiParams {
    /// C# `invCamMatrix` — `getRayDir` in `globalIllum` / `spatialResampling`.
    pub inv_view_proj: Mat4,
    /// C# `camMatrix` — `sampleRefine`'s reproject (rotation-only view-proj).
    pub view_proj: Mat4,
    /// Current camera integer position (C# `camPosInt`).
    pub cam_pos_int: IVec3,
    /// Padding to a 16-byte boundary.
    pub _pad0: u32,
    /// Current camera fractional position (C# `camPosFrac`).
    pub cam_pos_frac: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad1: u32,
    /// Direction *towards* the sun (C# `skySunDir`) — shared with the atmosphere.
    pub sky_sun_dir: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad2: u32,
    /// Sun radiance colour — C# `sunColor = Atmosphere.GetLightForPoint`
    /// (`09-design-b.md` §9.2).
    pub sun_color: Vec3,
    /// Padding to a 16-byte boundary.
    pub _pad3: u32,
    /// Render-target width in pixels.
    pub screen_width: u32,
    /// Render-target height in pixels.
    pub screen_height: u32,
    /// Monotonic frame counter (C# `frameCount` / `frameIndex`).
    pub frame_count: u32,
    /// TAA history slot index (C# `taaIndex`).
    pub taa_index: u32,
    /// `globalIllumMaxAccum - (frameCount % globalIllumMaxAccum) - 1`
    /// (C# `globalIlumAccumIndex`, `WorldRenderBase.cs:181`).
    pub accum_index: u32,
    /// Per-frame RNG salt (C# `randCounter`).
    pub rand_counter: u32,
    /// Second per-frame RNG salt (C# `randCounter2`).
    pub rand_counter2: u32,
    /// Max secondary-ray bounce count (C# `GiSettings.bounce_count` = `3`).
    pub max_bounce_count: u32,
    /// 8×8 bucket-grid cell width in pixels (`(w + 7) / 8`).
    pub bucket_size_x: u32,
    /// 8×8 bucket-grid cell height in pixels (`(h + 7) / 8`).
    pub bucket_size_y: u32,
    /// Total bucket count (`bucket_size_x * bucket_size_y`).
    pub bucket_count: u32,
    /// GI accumulation-ring depth (C# `globalIllumMaxAccum` = `128`).
    pub sample_max_accum: u32,
    /// Lit-sample ring depth multiplier (C# `globalIllumValidSampleStorageCount`
    /// = `2`).
    pub valid_sample_storage_count: u32,
    /// Unlit-sample ring depth multiplier
    /// (C# `globalIllumInvalidSampleStorageCount` = `8`).
    pub invalid_sample_storage_count: u32,
    /// Per-bucket refined-sample capacity (C# `globalIllumBucketStorageCount`
    /// = `32`).
    pub bucket_storage_count: u32,
    /// Per-bucket compressed-sample capacity
    /// (C# `globalIllumRefinedBucketStorageCount` = `8`).
    pub refined_bucket_storage_count: u32,
    /// Spatial-resampling neighbour-search size (C# `spatialResampleSize`
    /// = `500.0`).
    pub spatial_resample_size: f32,
    /// Lit-radius factor (C# `radiusLitFactor` = `3.0`).
    pub radius_lit_factor: f32,
    /// Noise-suppression factor (C# `noiseSuppressionFactor` = `0.4`).
    pub noise_suppression_factor: f32,
    /// Denoiser threshold (C# `denoiseThresh` = `400.0`).
    pub denoise_thresh: f32,
    /// Packed GI flags — see the `GI_FLAG_*` constants.
    pub flags: u32,
    /// Padding to a 16-byte stride. Sits at struct offset 276 — keeps
    /// `taa_jitter` on the 8-byte-aligned offset 280 (a `Vec2` needs 8-byte
    /// alignment to mirror a WGSL `vec2<f32>` — see the layout note below).
    pub _pad4: u32,
    /// Sub-pixel TAA jitter (the C# `taaJitter`) — the per-frame Halton 2-D
    /// offset the GI sample-generation + spatial-resampling rays are fired
    /// through (`renderGlobalIllum.fx:69` / `renderSpatialResampling.fx:351`).
    /// Zero when `AppArgs.taa` is off. **Layout (the `vec3`-then-scalar /
    /// `vec4` WGSL hazard that bit this port 3×):** this field occupies the
    /// former `_pad5`/`_pad6` slot — struct offset **280**, which is 8-byte
    /// aligned (`280 % 8 == 0`), so a WGSL `vec2<f32>` lands here byte-for-byte.
    /// The struct stays 288 bytes; the WGSL counterpart declares `flags: u32,
    /// pad_a: u32, taa_jitter: vec2<f32>` so the offsets match exactly.
    pub taa_jitter: Vec2,
}

/// `flags` bit: skip samples — the 1↔0.25-spp toggle (C# `skipSamples`).
pub const GI_FLAG_SKIP_SAMPLES: u32 = 1 << 0;
/// `flags` bit: run the sparse bilateral denoiser (C# `isDenoise`).
pub const GI_FLAG_IS_DENOISE: u32 = 1 << 1;
/// `flags` bit: brightness-level the bucket samples (C# `isSampleLeveling`).
pub const GI_FLAG_IS_SAMPLE_LEVELING: u32 = 1 << 2;
/// `flags` bit: vary the spatial-resampling radius per pixel
/// (C# `isVaryingResmaplingRadius`).
pub const GI_FLAG_IS_VARYING_RADIUS: u32 = 1 << 3;
/// `flags` bit: apply the in-volume atmosphere interaction
/// (C# `isAtmosphereInteraction`).
pub const GI_FLAG_IS_ATMOSPHERE_INTERACTION: u32 = 1 << 4;

// === Phase C — `GpuConstructionParams` (`15-design-c.md` §1.8, §5.1) ========

/// Phase-C construction-pass uniform — the per-frame scalar parameters every
/// construction pass needs (`15-design-c.md` §1.8 / §5.1).
///
/// Collapses NAADF's per-handler `Effect.Parameters` scalars
/// (`WorldBoundHandler.cs:97-111`, `ChangeHandler.cs:188-200`,
/// `BlockHashingHandler` size, the segment offsets) into a single uniform
/// the construction WGSL shaders bind once per pass.
///
/// **Layout discipline (`18-taa-fidelity.md` fix #1, `15-design-c.md` §1.5).**
/// Every 3-tuple is **explicitly padded to 16 bytes at the Rust level** so the
/// WGSL counterpart can declare `vec3<u32>` + scalar without triggering the
/// `vec3`-then-scalar / `vec4` layout hazard. Total: 80 B = 5 × 16-byte rows.
/// `offset_of!` guards below pin the row boundaries; a runtime mirror lives in
/// `tests::construction_params_layout`.
///
/// WGSL counterpart lives in W1's `chunk_calc.wgsl` shader-prelude
/// (`construction_common.wgsl`); W0 lands the Rust struct + guards so W1's
/// WGSL has a stable Rust mirror to point at.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuConstructionParams {
    // Row 0 (offset 0): sizeInChunks (vec3) + pad to 16.
    /// `chunkSizeX/Y/Z` — world size in chunks. C# `WorldData.sizeInChunks`.
    pub size_in_chunks: [u32; 3],
    /// std140 padding so the next row starts on a 16-byte boundary.
    pub _pad0: u32,
    // Row 1 (offset 16): groupSizeInGroups (vec3) + pad to 16.
    /// `groupSizeInGroups.x/y/z` — world size in 4³-chunk groups
    /// (`sizeInChunks / 4`). C# `WorldBoundHandler.groupCountX/Y/Z`.
    pub group_size_in_groups: [u32; 3],
    /// std140 padding to the next 16-byte row.
    pub _pad1: u32,
    // Row 2 (offset 32): 4 × u32.
    /// `boundGroupQueueMaxSize` == `boundGroupCount = chunkCount / 64`. C#
    /// `WorldBoundHandler.cs:44` queue-size derivation.
    pub bound_group_queue_max_size: u32,
    /// Current power-of-two hash-map capacity (grows via `mapCopy.fx`). C#
    /// `BlockHashingHandler.hashMapSize`.
    pub hash_map_size: u32,
    /// `segmentSizeInChunks` — NAADF default 4 (`WorldData.cs:73`).
    pub segment_size_in_chunks: u32,
    /// `maxGroupBoundDispatch` — the regime-2 throttle. NAADF default `512 * 64`
    /// (`WorldBoundHandler.cs:25`). Mirrored from `ConstructionConfig`.
    pub max_group_bound_dispatch: u32,
    // Row 3 (offset 48): chunkOffset (vec3) + pad to 16.
    /// `chunkOffsetX/Y/Z` — per-segment chunk offset for the regime-1 dispatch
    /// loop. C# `WorldData.cs:138-151`.
    pub chunk_offset: [u32; 3],
    /// std140 padding to the next 16-byte row.
    pub _pad2: u32,
    // Row 4 (offset 64): 4 × u32.
    /// Monotonic frame counter — shared with `GpuRenderParams.frame_count` /
    /// `GpuTaaParams.frame_count`; populated identically.
    pub frame_index: u32,
    /// Per-frame edit-event counts (regime-3 — `worldChange.fx`). Zero on
    /// frames with no pending edits (the `naadf_world_change_node` is gated
    /// off when all three are zero — `15-design-c.md` §1.2 regime 3).
    pub changed_chunk_count: u32,
    pub changed_block_count: u32,
    pub changed_voxel_count: u32,
}

// === Phase C W1 — `GpuHashValueSlot` (`15-design-c.md` §5.2, W1) =============

/// Rust mirror of `chunk_calc.wgsl::HashValueSlot` (the per-slot hash-table
/// record) (`15-design-c.md` §5.2 + W1's atomicity discipline).
///
/// Three semantically-distinct fields packed into 16 bytes:
///   - `voxel_pointer` (u32) — the open-addressing CAS target. WGSL declares
///     it `atomic<u32>`. Plain `u32` on the Rust mirror (Rust never accesses
///     it atomically — only uploads `0` once at allocation time + reads it
///     back via buffer mapping).
///   - `use_count` (u32) — the `atomicAdd` slot-occupancy counter.
///   - `hash_raw` (u32) — the slot's stored hash. Plain non-atomic (written
///     after the slot is CAS-claimed, single-writer at write time).
///   - `_pad` (u32) — explicit padding so `array<HashValueSlot>` stride is 16
///     bytes (matches WGSL's storage-buffer array element alignment for a
///     12-byte struct of 3 × u32). **This is the documented `vec3<u32>`
///     storage-buffer alignment deviation** (`12-alignment-gap.md` §3
///     D-A class) — the C# struct is 12 B; the WGSL stride is 16 B; the
///     Rust mirror declares the pad explicitly to match.
///
/// W1 ports this single struct; W2 / W3 do not extend it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuHashValueSlot {
    pub voxel_pointer: u32,
    pub use_count: u32,
    pub hash_raw: u32,
    pub _pad: u32,
}

const _: () = assert!(std::mem::size_of::<GpuHashValueSlot>() == 16);
const _: () = assert!(std::mem::offset_of!(GpuHashValueSlot, voxel_pointer) == 0);
const _: () = assert!(std::mem::offset_of!(GpuHashValueSlot, use_count) == 4);
const _: () = assert!(std::mem::offset_of!(GpuHashValueSlot, hash_raw) == 8);
const _: () = assert!(std::mem::offset_of!(GpuHashValueSlot, _pad) == 12);

// === Phase C W3 — `GpuBoundQueueInfo` (`15-design-c.md` §5.3, W3) ============

/// Rust mirror of `bounds_calc.wgsl::BoundQueueInfo` (the per-queue head/size
/// record) (`15-design-c.md` §5.3, `boundsCalc.fx:13-17`).
///
/// Two `u32` fields, total 8 B. The WGSL declares `size` as `atomic<u32>` so
/// `prepare_group_bounds` + `compute_group_bounds` can atomically advance it;
/// the Rust mirror uploads the initial seed (`0`, `bound_group_count` for the
/// size-0 queues; `0`, `0` for the rest) once at allocation time as plain
/// `u32`s. WGSL `array<BoundQueueInfo>` stride is 8 B (no padding required
/// for a 2-u32 struct).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuBoundQueueInfo {
    pub start: u32,
    pub size: u32,
}

const _: () = assert!(std::mem::size_of::<GpuBoundQueueInfo>() == 8);
const _: () = assert!(std::mem::offset_of!(GpuBoundQueueInfo, start) == 0);
const _: () = assert!(std::mem::offset_of!(GpuBoundQueueInfo, size) == 4);

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
// rows: (u32×4) (taa_index/flags/_pad0a/_pad0b) (sky_sun_dir/_pad1)
// (sun_color/_pad2) (taa_jitter/_pad3) (bbox_min/_pad4) (bbox_max/_pad5).
//
// `GpuGiParams.taa_jitter` placement guard (`18-taa-fidelity.md` fix #1): the
// new `taa_jitter` field must land at struct offset 280 — the last row's
// 8-byte-aligned slot — so the WGSL `vec2<f32>` matches byte-for-byte. The
// `vec3`-then-scalar / `vec4` layout hazard bit this port 3× already; pin the
// offset at compile time. (`280 % 8 == 0`, satisfying WGSL's 8-byte `vec2<f32>`
// alignment; `288 - 280 == 8` bytes for the `Vec2`.)
const _: () = assert!(std::mem::size_of::<GpuCamera>() == 64 + 32);
const _: () = assert!(std::mem::size_of::<GpuRenderParams>() == 16 * 7);
const _: () = assert!(std::mem::size_of::<GpuWorldMeta>() == 48);
const _: () = assert!(std::mem::size_of::<GpuVoxelType>() == 16);
// Phase A-2 TAA structs (`06-design-a2.md` §4.2, §4.3); `GpuCameraHistorySlot`
// is widened to 160 bytes by Phase B's `view_proj_inv` ring (`09-design-b.md`
// §3.6).
const _: () = assert!(std::mem::size_of::<GpuTaaParams>() == 192);
const _: () = assert!(std::mem::size_of::<GpuCameraHistorySlot>() == 64 + 64 + 16 + 16);
// Phase B GPU structs (`09-design-b.md` §3.2, §3.8, §3.9).
const _: () = assert!(std::mem::size_of::<GpuAtmosphereParams>() == 128);
const _: () = assert!(std::mem::size_of::<GpuSampleValid>() == 32);
const _: () = assert!(std::mem::size_of::<GpuBucketInfo>() == 8);
const _: () = assert!(std::mem::size_of::<GpuGiParams>() == 288);
// `taa_jitter` placement guard — must land at offset 280, 8-byte aligned, so
// the WGSL `vec2<f32>` matches byte-for-byte (`18-taa-fidelity.md` fix #1).
const _: () = assert!(std::mem::offset_of!(GpuGiParams, taa_jitter) == 280);
const _: () = assert!(std::mem::offset_of!(GpuGiParams, taa_jitter) % 8 == 0);
// Phase C — `GpuConstructionParams` layout pins (`15-design-c.md` §5.1).
// 80 bytes = 5 × 16-byte rows; every `vec3` 3-tuple explicitly padded to 16
// so the WGSL `vec3<u32>`-then-scalar hazard cannot recur on the construction
// uniform. The runtime mirror lives in `tests::construction_params_layout`.
const _: () = assert!(std::mem::size_of::<GpuConstructionParams>() == 80);
const _: () = assert!(std::mem::offset_of!(GpuConstructionParams, size_in_chunks) == 0);
const _: () =
    assert!(std::mem::offset_of!(GpuConstructionParams, group_size_in_groups) == 16);
const _: () =
    assert!(std::mem::offset_of!(GpuConstructionParams, bound_group_queue_max_size) == 32);
const _: () = assert!(std::mem::offset_of!(GpuConstructionParams, chunk_offset) == 48);
const _: () = assert!(std::mem::offset_of!(GpuConstructionParams, frame_index) == 64);
const _: () = assert!(std::mem::offset_of!(GpuConstructionParams, size_in_chunks) % 16 == 0);
const _: () =
    assert!(std::mem::offset_of!(GpuConstructionParams, group_size_in_groups) % 16 == 0);
const _: () = assert!(std::mem::offset_of!(GpuConstructionParams, chunk_offset) % 16 == 0);

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

    /// Phase-C — runtime mirror of the compile-time `GpuConstructionParams`
    /// layout guards (`15-design-c.md` §1.5, §5.1; `18-taa-fidelity.md` fix #1
    /// pattern). The struct is 80 bytes = 5 × 16-byte rows; every `vec3` is
    /// explicitly padded to 16 so the WGSL `vec3<u32>`-then-scalar hazard
    /// cannot recur. The `const _: () = assert!(...)` guards above already
    /// catch this at compile time; this `#[test]` exists so a test-only
    /// failure mode (a refactor that adds `#[cfg(feature = ...)]` around a
    /// guard, future tooling that strips `const _ = assert!(…)`, an editor
    /// auto-fix that "fixes" the casts) still has a runtime line to fire on.
    #[test]
    fn construction_params_layout() {
        assert_eq!(std::mem::size_of::<GpuConstructionParams>(), 80);
        assert_eq!(std::mem::offset_of!(GpuConstructionParams, size_in_chunks), 0);
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, group_size_in_groups),
            16
        );
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, bound_group_queue_max_size),
            32
        );
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, hash_map_size),
            36
        );
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, segment_size_in_chunks),
            40
        );
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, max_group_bound_dispatch),
            44
        );
        assert_eq!(std::mem::offset_of!(GpuConstructionParams, chunk_offset), 48);
        assert_eq!(std::mem::offset_of!(GpuConstructionParams, frame_index), 64);
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, changed_chunk_count),
            68
        );
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, changed_block_count),
            72
        );
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, changed_voxel_count),
            76
        );
        // Every `vec3` 3-tuple lands on a 16-byte boundary — the WGSL
        // `vec3<u32>`-then-scalar hazard guard.
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, size_in_chunks) % 16,
            0
        );
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, group_size_in_groups) % 16,
            0
        );
        assert_eq!(
            std::mem::offset_of!(GpuConstructionParams, chunk_offset) % 16,
            0
        );
    }

    /// Phase-C W1 — runtime mirror of the `GpuHashValueSlot` layout guards
    /// (`15-design-c.md` §5.2). 16 B = 4 × u32, the `_pad` field documents the
    /// WGSL `array<HashValueSlot>` 16-byte stride explicitly so the Rust
    /// upload matches the WGSL read byte-for-byte.
    #[test]
    fn hash_value_slot_layout() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<GpuHashValueSlot>(), 16);
        assert_eq!(offset_of!(GpuHashValueSlot, voxel_pointer), 0);
        assert_eq!(offset_of!(GpuHashValueSlot, use_count), 4);
        assert_eq!(offset_of!(GpuHashValueSlot, hash_raw), 8);
        assert_eq!(offset_of!(GpuHashValueSlot, _pad), 12);
    }

    /// Phase-C W3 — runtime mirror of the `GpuBoundQueueInfo` layout guards
    /// (`15-design-c.md` §5.3). 8 B = 2 × u32; the WGSL declares `size` as
    /// `atomic<u32>` but the Rust upload writes plain `u32` at allocation.
    #[test]
    fn bound_queue_info_layout() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<GpuBoundQueueInfo>(), 8);
        assert_eq!(offset_of!(GpuBoundQueueInfo, start), 0);
        assert_eq!(offset_of!(GpuBoundQueueInfo, size), 4);
    }
}
