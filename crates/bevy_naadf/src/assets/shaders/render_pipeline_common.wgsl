// render_pipeline_common.wgsl — render-pipeline constants, the voxel-type
// decompress, the camera / render-params uniforms, ray-direction setup, the
// G-buffer pack, the specular-path virtual-path reconstruction, and the
// screen-projection helpers.
//
// Derives from: render/common/commonRenderPipeline.fxh +
// render/versions/albedo/renderFirstHit.fx's uniform block + the
// `compressFirstHitData` helper (`03-design.md` §5.5, `09-design-b.md` §2.2 /
// §5.2).
//
// Phase A: the `HIT_*` / `SURFACE_*` consts, `VoxelType` +
// `decompressVoxelType`, the `NORMAL[8]` LUT, `getRayDir`, `compressFirstHitData`.
// Phase B adds (`09-design-b.md` §5.2): the full specular `getHitDataFromPlanes`
// (the 3-iteration specular-reflection loop + `SPECULAR_MIRROR_FAC` LUT, entity
// branch omitted), `getReflectanceFresnel`, `getSpecularNormals`, `getTang`, the
// `getScreenPosProjection` / `getScreenIndexProjection` pair (promoted here from
// `taa.wgsl` so `renderSampleRefine` / `renderSpatialResampling` can share
// them), the `FirstHitResult` + `SampleValid` structs, and the
// `is_diffuse` parameter on `compress_first_hit_data` (the 5-arg `base/`
// variant — `base/renderFirstHit.fx:18`).
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

// Base material classes — post-PBR-raymarching pivot. Collapsed from the C#
// 4-value `SURFACE_*` set to a 1-bit `{ PBR=0, Emissive=1 }`. The
// `SURFACE_SPECULAR_*` and `SURFACE_DIFFUSE` distinctions move to runtime
// texture-driven `metallic` / `roughness` reads in `pbr_sampling.wgsl`.
const SURFACE_PBR: u32 = 0u;
const SURFACE_EMISSIVE: u32 = 1u;

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

// The per-axis mirror-fac LUT indexed by the 3-bit normal index — used by
// `get_hit_data_from_planes` to fold the accumulated reflection sign
// (HLSL `static const float3 SPECULAR_MIRROR_FAC[7]`).
const SPECULAR_MIRROR_FAC: array<vec3<f32>, 7> = array<vec3<f32>, 7>(
    vec3<f32>(1.0, 1.0, 1.0),
    vec3<f32>(-1.0, 1.0, 1.0),
    vec3<f32>(-1.0, 1.0, 1.0),
    vec3<f32>(1.0, -1.0, 1.0),
    vec3<f32>(1.0, -1.0, 1.0),
    vec3<f32>(1.0, 1.0, -1.0),
    vec3<f32>(1.0, 1.0, -1.0),
);

// The first-hit virtual-path reconstruction result (HLSL `struct FirstHitResult`).
struct FirstHitResult {
    pos: vec3<f32>,
    normal: vec3<f32>,
    normal_mirror_fac: vec3<f32>,
    dist: f32,
    normal_tang: u32,
    ray_dir: vec3<f32>,
}

// The compressed lit GI sample (HLSL `struct SampleValid { uint4 data1, data2 }`).
// GPU-only working data — mirrors `gpu_types::GpuSampleValid` (a raw `[u32;8]`);
// the shaders pack/unpack the bitfields directly (`renderGlobalIllum.fx:34-48`).
//
// PORT NOTE: the HLSL field names are `data1` / `data2`, but naga-oil rejects
// trailing-digit identifiers in a composable module's struct ("must not require
// substitution according to naga writeback rules"). The fields are `data_a` /
// `data_b` here — same `data1` / `data2` content, naga-oil-safe names.
struct SampleValid {
    data_a: vec4<u32>,
    data_b: vec4<u32>,
}

// --- voxel-type material (post-PBR-raymarching pivot) -----------------------

// Decompressed voxel-type material entry. The four physical-material
// parameters (albedo RGB, metallic, roughness, height) move to texture-array
// samples; only the 1-bit base flag + the texture-array layer index +
// per-VoxelType tint/emissive bits live here. See
// `docs/orchestrate/pbr-raymarching/02-design.md` § B for the bit layout.
struct VoxelType {
    // 0 = PBR, 1 = Emissive (see `SURFACE_PBR` / `SURFACE_EMISSIVE` above).
    material_base: u32,
    // 0..4095 — index into the `MaterialSet` texture arrays.
    material_layer_index: u32,
    // 1, 2, 4, 8, ..128 — decoded from the `variant_span_log2` 3-bit field.
    variant_span: u32,
    // sRGB-byte → `[0,1]` linear-on-linear-albedo multiplier (Bevy
    // `StandardMaterial` convention).
    albedo_tint: vec3<f32>,
    // Emissive HDR multiplier (PBR voxels: ignored).
    color_layered: vec3<f32>,
}

// PBR-raymarching mask/shift constants — mirror Rust `gpu_types.rs`
// `VOXEL_GPU_*` (kept in lock-step; the runtime test
// `gpu_voxel_type_packs_pbr_layout` exercises them).
const VOXEL_GPU_BASE_MASK: u32     = 0x1u;
const VOXEL_GPU_LAYER_SHIFT: u32   = 1u;
const VOXEL_GPU_LAYER_MASK: u32    = 0x1FFEu;       // 0xFFF << 1
const VOXEL_GPU_VARIANT_SHIFT: u32 = 13u;
const VOXEL_GPU_VARIANT_MASK: u32  = 0xE000u;       // 0x7 << 13
const VOXEL_GPU_TINT_R_SHIFT: u32  = 16u;
const VOXEL_GPU_TINT_R_MASK: u32   = 0x00FF0000u;

// `decompress_voxel_type` — unpack a 128-bit material entry per the PBR
// bit-layout (`02-design.md` § B). The 3 emissive `color_layered`
// components are f16 (packed in `comp.z` / `comp.w`).
fn decompress_voxel_type(comp: vec4<u32>) -> VoxelType {
    var ty: VoxelType;
    ty.material_base = comp.x & VOXEL_GPU_BASE_MASK;
    ty.material_layer_index =
        (comp.x & VOXEL_GPU_LAYER_MASK) >> VOXEL_GPU_LAYER_SHIFT;
    let variant_log2 =
        (comp.x & VOXEL_GPU_VARIANT_MASK) >> VOXEL_GPU_VARIANT_SHIFT;
    ty.variant_span = 1u << variant_log2;

    // Albedo tint: 3 × sRGB bytes packed into bits 16..24 of `data[0]` +
    // bits 0..8 / 8..16 of `data[1]`.
    let tint_r = (comp.x & VOXEL_GPU_TINT_R_MASK) >> VOXEL_GPU_TINT_R_SHIFT;
    let tint_g =  comp.y         & 0xFFu;
    let tint_b = (comp.y >> 8u)  & 0xFFu;
    // Treat the bytes as linear multipliers in `[0,1]` (the sampled albedo
    // is already linear after `Rgba8UnormSrgb` decode — multiplicative tint
    // on linear-space colour is the Bevy `StandardMaterial` convention).
    ty.albedo_tint =
        vec3<f32>(f32(tint_r), f32(tint_g), f32(tint_b)) / 255.0;

    // Color-layered (emissive HDR multiplier): f16 packed in `comp.z` and
    // `comp.w` (low half-word).
    let cl_xy = unpack2x16float(comp.z);
    let cl_zw = unpack2x16float(comp.w);
    ty.color_layered = vec3<f32>(cl_xy.x, cl_xy.y, cl_zw.x);
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
// at 0/4/8/12, `taa_index`/`flags`/`max_ray_steps_primary`/`pad0b` at
// 16/20/24/28, `sky_sun_dir` slots to 32, `sun_color` to 48, `taa_jitter` to
// 64, `bounding_box_min` to 80, `bounding_box_max` to 96 — total 112 bytes.
// `max_ray_steps_primary` (offset 24) was `pad0a`, formerly `exposure` /
// `tone_mapping_fac` — the custom final-blit tonemap constants. The
// TAA-fidelity track switched the port to Bevy's built-in tonemapping
// (`naadf_final.wgsl` outputs raw linear HDR), so the pad slot was free; this
// dispatch reclaims it for the runtime-tunable primary-ray DDA cap (the
// quality panel — `21-design-quality-panel.md` §4.1). Layout-preserving
// rename only; the 112-byte struct size is unchanged.
struct GpuRenderParams {
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    rand_counter: u32,

    taa_index: u32,
    // packed `showRayStep` / `checkSun` / `isTAA` — see the `FLAG_*` consts.
    flags: u32,
    // Max DDA step count for the primary G-buffer ray
    // (`naadf_first_hit.wgsl::shoot_ray`). Runtime knob promoted from the WGSL
    // `MAX_RAY_STEPS_PRIMARY` const (`ray_tracing.wgsl:122`) — quality panel.
    // Default 120 = pre-dispatch const bit-equivalent. Consumer clamps
    // `max(_, 1u)` defensively.
    max_ray_steps_primary: u32,
    pad0b: u32,

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
// The `base/` first-hit ray-marches the atmosphere along each primary-ray
// segment (`WorldRenderBase.isAtmosphereInteraction`). Phase B / Batch 2.
const FLAG_IS_ATMOSPHERE_INTERACTION: u32 = 8u;
// The final blit decodes its source as `final_color`'s packing (no weight
// field) — the Batch-2 deliberate temporary seam (`09-design-b.md` §11 Batch 2
// step 8). Batch 6 reverts the blit source to `taa_sample_accum` + clears this.
const FLAG_BLIT_FINAL_COLOR: u32 = 16u;

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
// G-buffer element. The `base/` variant (HLSL `base/renderFirstHit.fx:18` —
// `compressFirstHitData(dist, normTangs, voxelTypeRaw, isDiffuse, entity)`) has
// 5 args: `.y = isDiffuse | ...` instead of the `albedo/` variant's `.y = 1 | ...`.
// (The Phase-A/A-2 call sites pass `1u` for `is_diffuse` until Batch 2 ports
// the `base/` first-hit — `09-design-b.md` §2.3.)
//
//   .x = entity        | (normTangs.x << 15)
//   .y = isDiffuse     | (normTangs.y << 15)
//   .z = voxelTypeRaw  | (normTangs.z << 15)
//   .w = f16(dist)&0x7FFF | (normTangs.w << 15)
fn compress_first_hit_data(
    dist: f32,
    norm_tangs: vec4<u32>,
    voxel_type_raw: u32,
    is_diffuse: u32,
    entity: u32,
) -> vec4<u32> {
    var first_hit: vec4<u32>;
    first_hit.x = entity | (norm_tangs.x << 15u);
    first_hit.y = is_diffuse | (norm_tangs.y << 15u);
    first_hit.z = voxel_type_raw | (norm_tangs.z << 15u);
    let dist_bits = pack2x16float(vec2<f32>(dist, 0.0)) & 0x7FFFu;
    first_hit.w = dist_bits | (norm_tangs.w << 15u);
    return first_hit;
}

// --- Phase-B: specular helpers (commonRenderPipeline.fxh) -------------------

// `getReflectanceFresnel` — Schlick Fresnel reflectance from an index-of-
// refraction triple (HLSL `getReflectanceFresnel`).
fn get_reflectance_fresnel(ior: vec3<f32>, cos_theta: f32) -> vec3<f32> {
    let r0 = pow((vec3<f32>(1.0) - ior) / (vec3<f32>(1.0) + ior), vec3<f32>(2.0));
    return r0 + (vec3<f32>(1.0) - r0) * pow(1.0 - cos_theta, 5.0);
}

// `getSpecularNormals` — pack the 3-bit normal index of each mirror-bounce
// plane (0..2) whose *next* plane is populated (HLSL `getSpecularNormals`).
fn get_specular_normals(hit: vec4<u32>) -> u32 {
    var normals = 0u;
    for (var i = 0u; i < 3u; i = i + 1u) {
        let next_normal_tang = hit[i + 1u] >> 15u;
        if (next_normal_tang != 0u) {
            normals |= ((hit[i] >> 15u) & 0x7u) << (i * 3u);
        }
    }
    return normals;
}

// `getTang` — the deepest populated plane's normal-tang code (HLSL `getTang`).
fn get_tang(first_hit: vec4<u32>) -> u32 {
    var normal_tang = 0u;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let new_normal_tang = first_hit[i] >> 15u;
        if (new_normal_tang != 0u) {
            normal_tang = new_normal_tang;
        }
    }
    return normal_tang;
}

// --- Phase-B: screen-projection helpers ------------------------------------
// Port of `getScreenPosProjection` + `getScreenIndexProjection`
// (`commonRenderPipeline.fxh:133-152`). Promoted here from `taa.wgsl` (where
// A-2 kept them local) so `taa.wgsl` / `sample_refine.wgsl` /
// `spatial_resampling.wgsl` can share them (`09-design-b.md` §5.2). WGSL has no
// `out` params and no default args, so these return small structs and take
// `pixel_offset` explicitly.
//
// MATRIX CONVENTION: the HLSL `mul(float4(pos,1), transformation)` is the
// column-vector `transformation * vec4(pos, 1.0)` against a glam matrix — the
// `05-review.md` perspective-fix convention. Do NOT swap to `v * M`.

struct ScreenPosProj {
    valid: bool,
    screen_pos: vec2<f32>,
}

fn get_screen_pos_projection(
    screen_width: u32,
    screen_height: u32,
    pos: vec3<f32>,
    transformation: mat4x4<f32>,
) -> ScreenPosProj {
    var r: ScreenPosProj;
    let screen_projection = transformation * vec4<f32>(pos, 1.0);
    let ndc = screen_projection.xyz / screen_projection.w;
    if (ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0
        || ndc.z < 0.0 || ndc.z > 1.0) {
        r.valid = false;
        r.screen_pos = vec2<f32>(0.0, 0.0);
        return r;
    }
    var ndc_y = ndc;
    ndc_y.y = ndc_y.y * -1.0;
    let ndc01 = (ndc_y.xy + vec2<f32>(1.0, 1.0)) * 0.5;
    r.valid = true;
    r.screen_pos = ndc01 * vec2<f32>(f32(screen_width), f32(screen_height));
    return r;
}

struct ScreenIndexProj {
    valid: bool,
    screen_index: u32,
}

fn get_screen_index_projection(
    screen_width: u32,
    screen_height: u32,
    pos: vec3<f32>,
    transformation: mat4x4<f32>,
    pixel_offset: vec2<f32>,
) -> ScreenIndexProj {
    let proj = get_screen_pos_projection(screen_width, screen_height, pos, transformation);
    // HLSL clamps `screenPos + pixelOffset` to `[0, (w-1, h-1)]` even when
    // `valid` is false — the index is still computed (and benignly clamped);
    // the caller gates on `valid`.
    let clamped = clamp(
        proj.screen_pos + pixel_offset,
        vec2<f32>(0.0, 0.0),
        vec2<f32>(f32(screen_width - 1u), f32(screen_height - 1u)),
    );
    let screen_pos_int = vec2<u32>(clamped);
    var r: ScreenIndexProj;
    r.valid = proj.valid;
    r.screen_index = screen_pos_int.x + screen_pos_int.y * screen_width;
    return r;
}

// --- Phase-B: the full specular `getHitDataFromPlanes` ---------------------
// Port of `commonRenderPipeline.fxh:154-213` — the 3-iteration specular-
// reflection loop + the tail. The `#ifdef ENTITIES` block (`:183-203`) is
// OMITTED (Phase B is entity-free — `09-design-b.md` §1 / §5.2), so the
// `entityInstancesHistory` + `taaIndex` parameters are dropped.
//
// When planes 1-3 are `HIT_UNDEFINED` the loop runs zero iterations and this
// reduces *exactly* to A-2's single-plane `get_hit_data_from_planes_a2` — so
// `taa.wgsl` calling this in place of its old local helper is not a behaviour
// change for the albedo path (`09-design-b.md` §5.2).
//
// `pos` is built from `cam_pos_frac` only — never adding `cam_pos_int` (the D1
// camera-relative trick), so the virtual hit position stays current-camera-int-
// relative.
fn get_hit_data_from_planes(
    first_hit: vec4<u32>,
    cam_pos_int: vec3<i32>,
    cam_pos_frac: vec3<f32>,
    ray_dir: vec3<f32>,
) -> FirstHitResult {
    var r: FirstHitResult;
    r.normal = vec3<f32>(0.0, 0.0, 0.0);
    r.normal_tang = first_hit.x >> 15u;
    r.pos = cam_pos_frac;
    r.dist = 0.0;
    r.normal_mirror_fac = vec3<f32>(1.0, 1.0, 1.0);
    r.ray_dir = ray_dir;

    for (var i = 0u; i < 3u; i = i + 1u) {
        let next_normal_tang = first_hit[i + 1u] >> 15u;
        if (next_normal_tang == HIT_UNDEFINED) {
            break;
        }
        // Apply reflection.
        r.normal_mirror_fac *= SPECULAR_MIRROR_FAC[r.normal_tang & 0x7u];
        r.normal = NORMAL[r.normal_tang & 0x7u];
        let ray_dir_comp_for_normal = abs(dot(r.ray_dir, r.normal));
        let dist_to_tang = abs(
            dot(r.pos, abs(r.normal))
            - (f32(r.normal_tang >> 3u) - dot(vec3<f32>(cam_pos_int), abs(r.normal)))
        );
        let dist_fac = dist_to_tang / ray_dir_comp_for_normal;
        r.dist += dist_fac;
        r.pos += r.ray_dir * dist_fac + r.normal * 0.01;
        r.ray_dir = reflect(r.ray_dir, r.normal);
        r.normal_tang = next_normal_tang;
    }

    // The `#ifdef ENTITIES` block is omitted — Phase B is entity-free.

    // Tail (`commonRenderPipeline.fxh:205-211`).
    r.normal = NORMAL[r.normal_tang & 0x7u];
    let ray_dir_comp_for_normal = abs(dot(r.ray_dir, r.normal));
    let dist_to_tang = abs(
        dot(r.pos, abs(r.normal))
        - (f32(r.normal_tang >> 3u) - dot(vec3<f32>(cam_pos_int), abs(r.normal)))
    );
    let dist_fac = dist_to_tang / ray_dir_comp_for_normal;
    r.dist += dist_fac;
    r.pos += r.ray_dir * dist_fac;
    return r;
}
