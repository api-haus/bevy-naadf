// render_pipeline_common.wgsl — render-pipeline constants, the voxel-type
// decompress, the camera / render-params uniforms, ray-direction setup, and
// the Phase-A G-buffer pack.
//
// Derives from: render/common/commonRenderPipeline.fxh +
// render/versions/albedo/renderFirstHit.fx's uniform block + the
// `compressFirstHitData` helper (`03-design.md` §5.5).
//
// **Phase-A subset** — the `HIT_*` / `SURFACE_*` consts, `VoxelType` +
// `decompressVoxelType`, the `NORMAL[8]` LUT, `getRayDir`, and
// `compressFirstHitData` are ported now. The specular-path
// `getHitDataFromPlanes` virtual-path reconstruction is Phase B
// (`02-research.md` §5.1 — this header splits A/B); Phase A's final blit reads
// the `shaded_color` stand-in directly, so it is not needed yet.
//
// naga-oil import module.

#import "shaders/common.wgsl"::PI

// --- hit / surface constants (commonRenderPipeline.fxh) ---------------------

// "no hit" marker for a packed normal-tang plane code (HLSL `HIT_NOTHING`).
const HIT_NOTHING: u32 = 0x1FFFFu;
// "this plane is undefined / unused" (HLSL `HIT_UNDEFINED`).
const HIT_UNDEFINED: u32 = 0u;
// "no entity" sentinel (HLSL `ENTITY_FREE`). Phase A is entity-free, so every
// `first_hit_data.x` entity field is this value.
const ENTITY_FREE: u32 = 0x3FFFu;

// Base material classes (HLSL `SURFACE_*`).
const SURFACE_DIFFUSE: u32 = 0u;
const SURFACE_EMISSIVE: u32 = 1u;
const SURFACE_SPECULAR_ROUGH: u32 = 2u;
const SURFACE_SPECULAR_MIRROR: u32 = 3u;

// Normal lookup table indexed by the 3-bit normal index of a plane code
// (HLSL `static const float3 NORMAL[8]`).
const NORMAL: array<vec3<f32>, 8> = array<vec3<f32>, 8>(
    vec3<f32>(0.0, 0.0, 0.0),
    vec3<f32>(-1.0, 0.0, 0.0),
    vec3<f32>(1.0, 0.0, 0.0),
    vec3<f32>(0.0, -1.0, 0.0),
    vec3<f32>(0.0, 1.0, 0.0),
    vec3<f32>(0.0, 0.0, -1.0),
    vec3<f32>(0.0, 0.0, 1.0),
    vec3<f32>(0.0, 0.0, 0.0),
);

// --- voxel-type material (commonRenderPipeline.fxh) -------------------------

// Decompressed voxel-type material entry (HLSL `struct VoxelType`).
struct VoxelType {
    material_base: u32,
    material_layer: u32,
    color_base: vec3<f32>,
    color_layer: vec3<f32>,
    roughness: f32,
}

// `decompressVoxelType` — unpack a 128-bit material entry (HLSL
// `decompressVoxelType(uint4 comp)`). The 6 colour channels + roughness are
// `f16`s packed two per `u32`; `unpack2x16float` is the WGSL `f16tof32` pair.
fn decompress_voxel_type(comp: vec4<u32>) -> VoxelType {
    var ty: VoxelType;
    ty.material_base = comp.x & 0x3u;
    ty.material_layer = (comp.x >> 2u) & 0x3u;
    let cy = unpack2x16float(comp.y);
    let cz = unpack2x16float(comp.z);
    let cw = unpack2x16float(comp.w);
    ty.color_base = vec3<f32>(cy.x, cy.y, cz.x);
    ty.color_layer = vec3<f32>(cz.y, cw.x, cw.y);
    // roughness is the high half-word of comp.x.
    ty.roughness = unpack2x16float(comp.x).y;
    return ty;
}

// --- camera / render-params uniforms (renderFirstHit.fx uniform block) ------

// Camera uniform (mirrors `gpu_types::GpuCamera`). The int+frac camera-relative
// position (D1) plus the inverse view-projection matrix `get_ray_dir` uses.
//
// No explicit padding members: naga-oil's composable-module round-trip rejects
// the explicit `_pad` members the Rust `#[repr(C)]` struct carries. WGSL's
// std140-ish uniform layout pads `vec3` to a 16-byte slot anyway, so the field
// offsets here match the padded Rust struct exactly — `inv_view_proj` (0..64),
// `cam_pos_int` (64..76, slot 64..80), `cam_pos_frac` (80..92, slot 80..96),
// total 96 bytes.
struct GpuCamera {
    // `world_from_clip` — HLSL `matrix invCamMatrix`.
    inv_view_proj: mat4x4<f32>,
    // HLSL `int camPosIntX,Y,Z`.
    cam_pos_int: vec3<i32>,
    // HLSL `float3 camPosFrac`.
    cam_pos_frac: vec3<f32>,
}

// Render-params uniform (mirrors `gpu_types::GpuRenderParams`).
//
// Again no explicit padding — WGSL's `vec3` 16-byte slotting + `vec2` 8-byte
// alignment reproduce the padded Rust `#[repr(C)]` layout: the four `u32`s sit
// at 0/4/8/12, `taa_index`/`flags`/`exposure` at 16/20/24, `sky_sun_dir` slots
// to 32, `sun_color` to 48, `taa_jitter` to 64, `bounding_box_min` to 80,
// `bounding_box_max` to 96 — total 112 bytes.
struct GpuRenderParams {
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    rand_counter: u32,

    taa_index: u32,
    // packed `showRayStep` / `checkSun` / `isTAA` — see the `FLAG_*` consts.
    flags: u32,
    exposure: f32,

    sky_sun_dir: vec3<f32>,
    sun_color: vec3<f32>,
    taa_jitter: vec2<f32>,
    bounding_box_min: vec3<f32>,
    bounding_box_max: vec3<f32>,
}

// `flags` bits (mirror `gpu_types::FLAG_*`).
const FLAG_SHOW_RAY_STEP: u32 = 1u;
const FLAG_CHECK_SUN: u32 = 2u;
const FLAG_IS_TAA: u32 = 4u;

// --- ray-direction setup (commonRenderPipeline.fxh `getRayDir`) -------------

// `getRayDir` — the primary-ray direction for a pixel.
//
// HLSL:
//   float2 screenPos = (pixelPos + 0.5 + pixelOffset) / float2(w, h);
//   normalize(mul(float4((screenPos*2-1) * float2(1,-1), 1, 1), camTransform).xyz)
//
// `inv_view_proj` is built by `extract_camera` as a glam (column-major) matrix,
// so the unprojection uses the column-vector convention — `M * v`, NOT `v * M`
// (`v * M` would evaluate `Mᵀ @ v`, a transpose). The perspective `w`-divide is
// mandatory: NAADF's HLSL skips it only because its `invCamMatrix` is
// rotation-only, making `w` per-pixel-constant — the port reproduces the
// rotation-only matrix (`extract.rs`), but still does the divide explicitly so
// the direction is correct regardless. `ndc.z = 1.0` is the NEAR plane under
// Bevy's reverse-Z projection; for the translation-free view matrix the
// normalized direction is `ndc.z`-invariant after the divide, so `1.0` is a
// valid, non-degenerate choice (`0.0` would give `w == 0`).
fn get_ray_dir(
    inv_view_proj: mat4x4<f32>,
    pixel_pos: vec2<u32>,
    screen_width: u32,
    screen_height: u32,
    pixel_offset: vec2<f32>,
) -> vec3<f32> {
    let screen_pos = (vec2<f32>(pixel_pos) + vec2<f32>(0.5, 0.5) + pixel_offset)
        / vec2<f32>(f32(screen_width), f32(screen_height));
    let ndc = (screen_pos * 2.0 - vec2<f32>(1.0, 1.0)) * vec2<f32>(1.0, -1.0);
    let unprojected = inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    return normalize(unprojected.xyz / unprojected.w);
}

// --- Phase-A G-buffer pack (renderFirstHit.fx `compressFirstHitData`) -------

// `compressFirstHitData` — pack the first-hit result into the `vec4<u32>`
// G-buffer element (HLSL `compressFirstHitData(dist, normTangs, voxelTypeRaw,
// entity)`).
//
//   .x = entity        | (normTangs.x << 15)
//   .y = 1             | (normTangs.y << 15)   // the `1` = "is hit" flag
//   .z = voxelTypeRaw  | (normTangs.z << 15)
//   .w = f16(dist)&0x7FFF | (normTangs.w << 15)
fn compress_first_hit_data(
    dist: f32,
    norm_tangs: vec4<u32>,
    voxel_type_raw: u32,
    entity: u32,
) -> vec4<u32> {
    var first_hit: vec4<u32>;
    first_hit.x = entity | (norm_tangs.x << 15u);
    first_hit.y = 1u | (norm_tangs.y << 15u);
    first_hit.z = voxel_type_raw | (norm_tangs.z << 15u);
    let dist_bits = pack2x16float(vec2<f32>(dist, 0.0)) & 0x7FFFu;
    first_hit.w = dist_bits | (norm_tangs.w << 15u);
    return first_hit;
}

// Keep `PI` referenced so the import is not dead (a future Phase-A-2 / B
// addition to this module will use it directly).
fn touch_pi() -> f32 {
    return PI;
}
