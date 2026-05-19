# 02 — Design (PBR raymarching)

## Overview

The pivot collapses the four C# `MaterialBase` branches into one PBR pipeline +
one Emissive fast-path. Per-voxel-type CPU palette (`VoxelType`) drops
`roughness` and `color_base`, keeps `color_layered` as the emissive HDR colour,
and gains a 12-bit `material_layer_index: u16` (one of 4096 materials in the
texture-array set) plus a 24-bit `albedo_tint: [u8; 3]` (sRGB-byte tint
multiplier on the sampled albedo). The 128-bit `GpuVoxelType` re-packs with no
buffer widening. A new `MaterialSet` `Resource` programmatically bundles four
`Handle<Image>` (loaded from re-authored `.texarray.ron` definitions:
`diffuse.texarray.ron`, `normal.texarray.ron`, `mrh.texarray.ron`,
`emissive.texarray.ron`) plus one shared `Handle<Image>` for the sampler-only
binding, all bound into the existing `world_layout` `@group(0)` at fresh slots
8…12 + a shared linear-repeat `sampler`. WGSL adds a `pbr_sampling.wgsl`
naga-oil module exposing `triplanar_blend_weights`, `triplanar_sample`,
`pom_displace_uv`, `decode_triplanar_tangent_normal`, `select_layer_variant`,
and `eval_pbr` (energy-conserving GGX/Smith/Schlick wrapping the existing
`sample_vndf_isotropic` / `geometry_term` / `get_reflectance_fresnel`). The
three hit-shading branches in `naadf_first_hit.wgsl` / `naadf_global_illum.wgsl`
/ `spatial_resampling.wgsl` collapse to two cases (`PBR=0` vs `Emissive=1`);
the `is_diffuse` first-hit/GI split is preserved (PBR voxels with
`sampled_roughness > 0.5` set `is_diffuse=0` and defer to GI; emissive voxels
set `is_diffuse=1` and stop). A new `--pbr-visual` e2e gate captures a known
metallic-roughness voxel at a fixed pose, asserts specular-highlight luminance
above a threshold, sampled-albedo variation across 16 sample points, and a
metallic-F0 colour-pull check.

---

## A. VoxelType (CPU) reshape

Edit `crates/bevy_naadf/src/voxel/mod.rs:113–138`.

### Before (current)

```rust
pub struct VoxelType {
    pub material_base: MaterialBase,
    pub material_layer: MaterialLayer,
    pub roughness: f32,
    pub color_base: Vec3,
    pub color_layered: Vec3,
}
```

### After (post-pivot)

```rust
/// One entry of the voxel-type palette — the per-voxel-type CPU material
/// data. **All physical-material parameters (albedo RGB, metallic, roughness,
/// height, AO, tangent-space normal) live in the `MaterialSet` texture
/// arrays at `material_layer_index`; this struct only carries the
/// per-VoxelType bits that *select and tint* the texture sample.**
///
/// The 4 C# `MaterialBase` branches collapse to a 1-bit flag here
/// (`PBR` vs `Emissive`) — every PBR hit runs the unified BRDF in
/// `eval_pbr()` (`pbr_sampling.wgsl`); every Emissive hit takes the
/// fast-path (no BRDF, no PBR-array samples).
///
/// **User-approved divergence from C# NAADF** (`01-context.md` D1, D4):
/// the C# `VoxelType` has no texture-array layer index and carries
/// `color_base` as IOR. This port replaces that with a texture-array layer
/// + `albedo_tint`. Documented as a deliberate divergence in
/// `docs/orchestrate/naadf-bevy-port/12-alignment-gap.md` (added by impl
/// agent).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct VoxelType {
    /// Base material class — `PBR` (0) or `Emissive` (1). Replaces the
    /// 4-value `MaterialBase` enum (the `MetallicRough` / `MetallicMirror`
    /// values are removed; metallic comes from the texture sample now).
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
    /// `sampled_emissive_rgb × color_layered` (see § H). Carried as
    /// 3× f16 on the GPU. Unused (zeroed) when `material_base == PBR`.
    pub color_layered: Vec3,
}
```

`MaterialBase` becomes a 1-bit enum:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum MaterialBase {
    #[default]
    Pbr = 0,
    Emissive = 1,
}
```

Delete the `MaterialLayer` enum entirely (no callers after the pivot).

`Default for VoxelType` becomes:

```rust
impl Default for VoxelType {
    fn default() -> Self {
        Self {
            material_base: MaterialBase::Pbr,
            material_layer_index: 0,
            albedo_tint: [255, 255, 255],
            color_layered: Vec3::ZERO,
        }
    }
}
```

### Call sites — migration

All `VoxelType { .. }` struct literals are migrated. Verified call-site list
(grep across `crates/bevy_naadf/src` + `bins`):

| File:line | Migration |
|---|---|
| `crates/bevy_naadf/src/voxel/mod.rs:130-137` (`Default`) | as above |
| `crates/bevy_naadf/src/voxel/grid.rs:593-689` (12 literals in `build_palette`) | each maps to `material_layer_index` selected from the 10-material starter set (see assignment table below); `material_base: Diffuse` → `Pbr`, `Emissive` → `Emissive`. `color_base` → `albedo_tint = [255,255,255]` (sampled albedo is already coloured by the texture); for emissive entries `color_layered` stays. `roughness` field is dropped (texture-driven). |
| `crates/bevy_naadf/src/voxel/vox_import.rs:994-1000` | dropped `color_base: linear`; instead set `albedo_tint = quantize_srgb(linear)` (use existing `Vec3` → `[u8; 3]` quantiser — implementer writes a 3-line `linear_to_srgb_bytes` helper). `material_layer_index: 0` (a single neutral grey-ish material in slot 0 of the array works for VOX, where each model has its own per-voxel colour). |
| `crates/bevy_naadf/src/render/gpu_types.rs:918-924` (test) | drop `roughness`, `color_base`; add `material_layer_index: 42`, `albedo_tint: [255, 128, 64]`. The test then asserts the new bit layout (see § B). |
| `crates/bevy_naadf/src/editor/hud.rs:558-561` | replace `vt.color_base.{x,y,z}` with `srgb_byte_to_f32(vt.albedo_tint[c])`. The HUD swatch shows the tint, which is the correct user-visible "what colour did I pick" indicator (the texture details aren't selectable per-VoxelType yet). |

### Grid-palette assignment (12 VoxelTypes → 10 materials)

`build_palette` in `voxel/grid.rs:588–690` defines 12 types (0 = empty + 11
real). Map them onto the 10-material starter set thus:

| VoxelType | Was | New `material_base` | New `material_layer_index` | `albedo_tint` | `color_layered` |
|---|---|---|---|---|---|
| 0 empty | placeholder | `Pbr` | 0 | `[255,255,255]` | `ZERO` |
| 1 ground | grey diffuse | `Pbr` | 5 (stone_wall_04) | `[255,255,255]` | `ZERO` |
| 2 boxA | warm | `Pbr` | 6 (ground_tiles_08) | `[204, 76, 56]` | `ZERO` |
| 3 boxB | cool | `Pbr` | 6 (ground_tiles_08) | `[64, 115, 204]` | `ZERO` |
| 4 sphere | green | `Pbr` | 7 (grass_05) | `[76, 178, 81]` | `ZERO` |
| 5 emissive A | warm-white | `Emissive` | 2 (pavement, has emissive) | `[255,255,255]` | `(8.0, 7.4, 6.2)` |
| 6 tower | grey | `Pbr` | 8 (bark_04) | `[255,255,255]` | `ZERO` |
| 7 wall | sand | `Pbr` | 9 (snow_01) | `[183, 158, 107]` | `ZERO` |
| 8 pillar | violet | `Pbr` | 3 (metal_02) | `[115, 82, 158]` | `ZERO` |
| 9 emissive B | cool-white | `Emissive` | 2 (pavement) | `[255,255,255]` | `(6.4, 6.9, 8.0)` |
| 10 emissive C | amber | `Emissive` | 2 | `[255,255,255]` | `(8.0, 5.3, 2.2)` |
| 11 emissive D | magenta | `Emissive` | 2 | `[255,255,255]` | `(8.0, 3.4, 6.9)` |

(The layer-index slots reference the array order in § D.)

The `vox_import.rs` per-voxel-type creation always uses
`material_layer_index: 0` (fabric — see § D layer 0). This keeps `.vox` model
rendering visually similar to today: every voxel reads the same neutral
fabric albedo modulated by the per-VoxelType tint.

---

## B. GpuVoxelType bit layout

Edit `crates/bevy_naadf/src/render/gpu_types.rs:260–295`.

128 bits total (`[u32; 4]`). The layout balances the post-pivot field set
against the hard `size_of::<GpuVoxelType>() == 16` assert at line 839.

### Field budget

| Field | Bits | Encoding | Rationale |
|---|---|---|---|
| `material_base` | 1 | `{0=PBR, 1=Emissive}` | the only branch in the hit-shader |
| `material_layer_index` | 12 | u12, range `[0, 4095]` | 4096 distinct materials, far exceeds the 10-material starter set |
| `variant_span_log2` | 3 | u3, range `[0, 7]` ⇒ variants ∈ `{1, 2, 4, 8, ...128}` | D6 procedural blend (§ G). `0` ⇒ 1 variant (no blend); first cut starter palette uses `0` for every type |
| (reserved low) | 0 | — | bits 1+12+3 = 16; fills `data[0]` low halfword |
| `albedo_tint.r` | 8 | sRGB byte | high halfword of `data[0]` |
| `albedo_tint.g` | 8 | sRGB byte | low halfword of `data[1]` |
| `albedo_tint.b` | 8 | sRGB byte | bits 16..24 of `data[1]` |
| (reserved high `data[1]`) | 8 | zeroed | future use; explicitly zeroed by packer |
| `color_layered.r` | 16 | f16 | low halfword of `data[2]` |
| `color_layered.g` | 16 | f16 | high halfword of `data[2]` |
| `color_layered.b` | 16 | f16 | low halfword of `data[3]` |
| (reserved high `data[3]`) | 16 | zeroed | spare |

**Total used:** 1 + 12 + 3 + 24 + 48 = 88 bits. **Reserved/spare:** 40 bits.
Fits in 128 bits with comfortable headroom.

### Rust packer (`GpuVoxelType::from_voxel_type`)

```rust
// Mask/shift constants (top of module).
pub const VOXEL_GPU_BASE_MASK: u32 = 0x1;                     // bit 0
pub const VOXEL_GPU_LAYER_SHIFT: u32 = 1;
pub const VOXEL_GPU_LAYER_MASK: u32 = 0xFFF << VOXEL_GPU_LAYER_SHIFT;       // bits 1..13
pub const VOXEL_GPU_VARIANT_SHIFT: u32 = 13;
pub const VOXEL_GPU_VARIANT_MASK: u32 = 0x7 << VOXEL_GPU_VARIANT_SHIFT;     // bits 13..16
pub const VOXEL_GPU_TINT_R_SHIFT: u32 = 16;
pub const VOXEL_GPU_TINT_R_MASK: u32 = 0xFF << VOXEL_GPU_TINT_R_SHIFT;      // bits 16..24
// (no extra bytes in data[0] above bit 24)

pub fn from_voxel_type(ty: &VoxelType) -> GpuVoxelType {
    let base   = (ty.material_base as u32) & VOXEL_GPU_BASE_MASK;
    let layer  = ((ty.material_layer_index as u32) << VOXEL_GPU_LAYER_SHIFT)
                 & VOXEL_GPU_LAYER_MASK;
    // Variant span: first cut hard-codes 0 (1 variant) for every type;
    // a later patch may surface this on `VoxelType` (see §G).
    let variant_log2: u32 = 0;
    let variant = (variant_log2 << VOXEL_GPU_VARIANT_SHIFT) & VOXEL_GPU_VARIANT_MASK;
    let tint_r  = (u32::from(ty.albedo_tint[0]) << VOXEL_GPU_TINT_R_SHIFT)
                  & VOXEL_GPU_TINT_R_MASK;
    let data0 = base | layer | variant | tint_r;

    let tint_g = u32::from(ty.albedo_tint[1]);
    let tint_b = u32::from(ty.albedo_tint[2]) << 8;
    // bits 16..32 of data[1] are reserved (zeroed).
    let data1 = tint_g | tint_b;

    let cl_r = f16_bits(ty.color_layered.x) as u32;
    let cl_g = (f16_bits(ty.color_layered.y) as u32) << 16;
    let data2 = cl_r | cl_g;

    let cl_b = f16_bits(ty.color_layered.z) as u32;
    // bits 16..32 of data[3] reserved (zeroed).
    let data3 = cl_b;

    GpuVoxelType { data: [data0, data1, data2, data3] }
}
```

Doc comment on the `GpuVoxelType` struct must be rewritten to spell out the
new bit layout (replaces lines 260–270).

Add the field-placement compile-time guards next to the existing
`assert!(std::mem::size_of::<GpuVoxelType>() == 16)` at line 839:

```rust
// PBR-raymarching bit-layout pins — see `02-design.md` §B.
const _: () = assert!(VOXEL_GPU_BASE_MASK == 0x1);
const _: () = assert!(VOXEL_GPU_LAYER_MASK == 0x1FFE);          // 12 bits at offset 1
const _: () = assert!(VOXEL_GPU_VARIANT_MASK == 0xE000);        // 3 bits at offset 13
const _: () = assert!(VOXEL_GPU_TINT_R_MASK == 0x00FF_0000);    // 8 bits at offset 16
```

### WGSL decoder

Edit `crates/bevy_naadf/src/assets/shaders/render_pipeline_common.wgsl:91–117`.

```wgsl
// Decompressed voxel-type material entry (replaces the legacy 4-base struct).
struct VoxelType {
    material_base: u32,           // 0 = PBR, 1 = Emissive
    material_layer_index: u32,    // 0..4095
    variant_span: u32,            // 1, 2, 4, 8, ...128 (decoded = 1 << variant_span_log2)
    albedo_tint: vec3<f32>,       // sRGB-byte → f32 / 255.0 (linear-space multiplier — see note)
    color_layered: vec3<f32>,     // emissive HDR multiplier (PBR voxels: ignored)
}

// PBR mask/shift constants (mirror the Rust constants in gpu_types.rs).
const VOXEL_GPU_BASE_MASK: u32           = 0x1u;
const VOXEL_GPU_LAYER_SHIFT: u32         = 1u;
const VOXEL_GPU_LAYER_MASK: u32          = 0x1FFEu;       // 0xFFF << 1
const VOXEL_GPU_VARIANT_SHIFT: u32       = 13u;
const VOXEL_GPU_VARIANT_MASK: u32        = 0xE000u;       // 0x7 << 13
const VOXEL_GPU_TINT_R_SHIFT: u32        = 16u;
const VOXEL_GPU_TINT_R_MASK: u32         = 0x00FF0000u;

fn decompress_voxel_type(comp: vec4<u32>) -> VoxelType {
    var ty: VoxelType;
    ty.material_base = comp.x & VOXEL_GPU_BASE_MASK;
    ty.material_layer_index =
        (comp.x & VOXEL_GPU_LAYER_MASK) >> VOXEL_GPU_LAYER_SHIFT;
    let variant_log2 =
        (comp.x & VOXEL_GPU_VARIANT_MASK) >> VOXEL_GPU_VARIANT_SHIFT;
    ty.variant_span = 1u << variant_log2;

    let tint_r = (comp.x & VOXEL_GPU_TINT_R_MASK) >> VOXEL_GPU_TINT_R_SHIFT;
    let tint_g =  comp.y        & 0xFFu;
    let tint_b = (comp.y >> 8u) & 0xFFu;
    // Treat the bytes as *linear* multipliers in [0,1]. The user picks
    // perceptual sRGB bytes via the HUD, but the multiplier semantics here
    // are linear-on-linear-albedo (the sampled albedo is decoded by the
    // Rgba8UnormSrgb format already). This is the Bevy `StandardMaterial`
    // convention.
    ty.albedo_tint = vec3<f32>(f32(tint_r), f32(tint_g), f32(tint_b)) / 255.0;

    let cl_xy = unpack2x16float(comp.z);
    let cl_zw = unpack2x16float(comp.w);
    ty.color_layered = vec3<f32>(cl_xy.x, cl_xy.y, cl_zw.x);
    return ty;
}
```

**Constants surfaced in BOTH places** so the impl can `assert_eq!` them in a
runtime-mirror test (`shader_drift_guard` style):

| Symbol | Rust | WGSL |
|---|---|---|
| `BASE_MASK` | `VOXEL_GPU_BASE_MASK = 0x1` | `0x1u` |
| `LAYER_SHIFT/MASK` | `1` / `0x1FFE` | `1u` / `0x1FFEu` |
| `VARIANT_SHIFT/MASK` | `13` / `0xE000` | `13u` / `0xE000u` |
| `TINT_R_SHIFT/MASK` | `16` / `0x00FF_0000` | `16u` / `0x00FF0000u` |

### Sanity

Bits used: 88. Bits remaining: 40. `data[3]` high-halfword is the largest
contiguous reserved slab — natural home for a future per-VoxelType POM
strength multiplier or a flag word.

---

## C. MaterialSet — linked-arrays bundle

### Decision: `Resource` (built programmatically), NOT an asset format

The `MaterialSet` is a `Resource` populated at startup from four hard-coded
`asset_server.load(...)` calls — one per `.texarray.ron`. **No `.matset.ron`
loader is added.**

**Rationale.** The four `.texarray.ron` files are the source-of-truth — they
already declare each array's layer ordering. A `.matset.ron` would only
re-name the four paths; it would add a parsing layer, a `MaterialSetLoader`,
and a 4-array layer-count parity check that the current pipeline does not
need (one set, hard-coded). Build a programmatic `Resource` in
`MaterialSetPlugin::build` for now; if the project ever ships multiple
material sets selectable per-world, promote to an asset format then. The
audit (`00-reuse-audit.md` § 6) explicitly notes there is no analogue today
and the upgrade path is local.

### Rust type

New file `crates/bevy_naadf/src/material_set/mod.rs`:

```rust
//! `MaterialSet` — the four linked texture-arrays (Diffuse+AO, Normal,
//! MRH, Emissive) that the PBR raymarcher samples per voxel-face.
//!
//! Each `Handle<Image>` points at the baked output of one `.texarray.ron`
//! (`assets/materials/*.texarray.ron`). All four arrays SHARE the
//! layer-index space — material N occupies layer N in every array — so the
//! 12-bit `VoxelType.material_layer_index` (`crate::voxel::VoxelType`)
//! selects across the whole set.
//!
//! Built programmatically in [`MaterialSetPlugin::build`] for now; if the
//! project ever ships multiple material sets selectable per-world this
//! resource is the seam to lift into an asset format (`.matset.ron`).

use bevy::asset::AssetServer;
use bevy::ecs::system::Resource;
use bevy::image::Image;
use bevy::prelude::*;

/// The four linked texture arrays + the shared sampler binding placeholder.
#[derive(Resource, Clone)]
pub struct MaterialSet {
    /// Layer N: RGB = sampled albedo (sRGB-decoded by `Rgba8UnormSrgb`),
    /// A = AO factor in [0,1]. Loaded from `materials/diffuse.texarray.ron`.
    pub diffuse_ao: Handle<Image>,
    /// Layer N: RGB = tangent-space normal (GL convention, Y-up), A unused.
    /// Loaded from `materials/normal.texarray.ron`.
    pub normal: Handle<Image>,
    /// Layer N: R = metallic, G = roughness (perceptual), B = height in
    /// [0,1] (POM source), A unused. Loaded from `materials/mrh.texarray.ron`.
    pub mrh: Handle<Image>,
    /// Layer N: RGB = emissive HDR colour (sRGB-decoded). For PBR voxels
    /// every layer is the `_placeholder/black_1.png` constant; only the
    /// Emissive fast-path samples it. Loaded from
    /// `materials/emissive.texarray.ron`.
    pub emissive: Handle<Image>,
}

/// Plugin: registers `MaterialSet` as a resource by loading the four
/// `.texarray.ron` definitions from the asset server.
pub struct MaterialSetPlugin;

impl Plugin for MaterialSetPlugin {
    fn build(&self, app: &mut App) {
        let asset_server = app.world().resource::<AssetServer>();
        let set = MaterialSet {
            diffuse_ao: asset_server.load("materials/diffuse.texarray.ron"),
            normal:     asset_server.load("materials/normal.texarray.ron"),
            mrh:        asset_server.load("materials/mrh.texarray.ron"),
            emissive:   asset_server.load("materials/emissive.texarray.ron"),
        };
        app.insert_resource(set);
    }
}
```

Register the plugin in the main crate `lib.rs` next to `TextureArrayPlugin`.
Re-export `MaterialSet` from `crate::material_set` to mirror the
`crate::baked_material` pattern.

### Render-world extraction

The `MaterialSet` lives in the main world — handles must be extracted into
the render sub-app for `prepare_*` to read them. Add an
`ExtractedMaterialSet` resource + an extraction system in
`crates/bevy_naadf/src/render/extract.rs` (alongside `ExtractedCameraData` /
`ExtractedGiConfig`):

```rust
#[derive(Resource, Clone)]
pub struct ExtractedMaterialSet {
    pub diffuse_ao: Handle<Image>,
    pub normal: Handle<Image>,
    pub mrh: Handle<Image>,
    pub emissive: Handle<Image>,
}

pub fn extract_material_set(
    mut commands: Commands,
    set: Extract<Option<Res<crate::material_set::MaterialSet>>>,
) {
    if let Some(set) = set.as_deref() {
        commands.insert_resource(ExtractedMaterialSet {
            diffuse_ao: set.diffuse_ao.clone(),
            normal: set.normal.clone(),
            mrh: set.mrh.clone(),
            emissive: set.emissive.clone(),
        });
    }
}
```

Wire the system into `RenderPlugin::build` via
`app.sub_app_mut(RenderApp).add_systems(ExtractSchedule, extract_material_set);`
(same shape as the existing extract systems).

### Bind-group integration — extending `@group(0)` `world_layout`

`world_layout` currently has 8 entries (slots 0..7). Append 5 PBR entries
(slots 8..12):

| New slot | Binding | WGSL type |
|---|---|---|
| 8 | `pbr_diffuse_ao` | `texture_2d_array<f32>` |
| 9 | `pbr_normal` | `texture_2d_array<f32>` |
| 10 | `pbr_mrh` | `texture_2d_array<f32>` |
| 11 | `pbr_emissive` | `texture_2d_array<f32>` |
| 12 | `pbr_sampler` | `sampler` |

Edits in `crates/bevy_naadf/src/render/pipelines.rs`:

- At line 313–339 (`world_layout`), extend the `sequential` tuple with five
  more entries using `binding_types::texture_2d_array(TextureSampleType::Float { filterable: true })`
  for the four texture-arrays and `binding_types::sampler(SamplerBindingType::Filtering)`
  for the sampler. Add `import std::num::NonZeroU64;` is not needed (already
  imported); the new imports needed are
  `binding_types::{texture_2d_array, sampler}` and
  `TextureSampleType, SamplerBindingType` from `bevy::render::render_resource`.
- Edit the layout doc comment at lines 296–312 to describe the 5 new bindings.
- Bind-group construction in `prepare.rs:552–564` extends
  `BindGroupEntries::sequential` with `BindingResource::TextureView` for each
  of the four arrays + `BindingResource::Sampler` for one shared sampler.
- `prepare_world_gpu` must wait until all four `Handle<Image>`s have a
  `GpuImage` available in `RenderAssets<GpuImage>`. Pattern: read
  `RenderAssets<GpuImage>` via parameter; for each handle call
  `images.get(&handle)`; if any returns `None`, **return early** (don't
  build the bind group this frame). This mirrors the existing
  `prepare_atmosphere_gpu` / `prepare_taa` "wait for upstream resource"
  pattern.
- Create one shared sampler with `address_mode_*: Repeat` + linear filter.
  Bevy's standard pattern: `render_device.create_sampler(&SamplerDescriptor { ... })`.
  Cache it on `WorldGpu` so it isn't recreated per frame.

### Why one sampler not four

All four texture arrays are sampled the same way (repeat + linear) — the
`bake_texture_array` function already sets `ImageSampler::Descriptor` to that
configuration (`loader.rs:197`). A single shared `sampler` binding suffices.
If a future change introduces e.g. point-sampled normals, split then.

---

## D. The 4 `.texarray.ron` definitions

All four files live in `assets/materials/` (symlinked into
`crates/bevy_naadf/src/assets/materials/`). Layer index ↔ material:

| Layer | Material | Notes |
|---|---|---|
| 0 | fabric | existing — 1024×1024 (resize required, see assumptions) |
| 1 | gravelrock | existing |
| 2 | pavement | existing (has emissive) |
| 3 | metal_02 | new |
| 4 | metal_pattern_01 | new |
| 5 | bark_04 | new (camelCase filenames) |
| 6 | snow_01 | new (placeholder metallic) |
| 7 | grass_05 | new (placeholder AO + metallic + height) |
| 8 | stone_wall_04 | new (placeholder metallic) |
| 9 | ground_tiles_08 | new (placeholder metallic) |

> **Layer order assignment note.** The grid-palette table in § A uses these
> indices. Implementer must keep `build_palette`'s `material_layer_index`
> values in lock-step with the `elements:` ordering of all four
> `.texarray.ron` files (the comment at the top of each file documents the
> mapping — already a project convention,
> `assets/materials/diffuse.texarray.ron:7-14`).

### D.1 `assets/materials/diffuse.texarray.ron` — re-author in place

```ron
// Diffuse + AO 2D-array — RGB albedo + A=AO per material. sRGB pixel format
// (RGB is colour; A is "data" but `Rgba8UnormSrgb` decodes A as linear so
// it round-trips correctly — Bevy convention).
//
// Layer index ↔ material is fixed across diffuse / normal / mrh / emissive:
//   0 fabric          1 gravelrock      2 pavement       3 metal_02
//   4 metal_pattern_01 5 bark_04        6 snow_01        7 grass_05
//   8 stone_wall_04   9 ground_tiles_08
(
    format: Rgba8UnormSrgb,
    elements: [
        // layer 0 — fabric: A ← occlusion.R (was: pass-through alpha).
        ( r: (input: "materials/fabric/base_color.png",     channel: R),
          g: (input: "materials/fabric/base_color.png",     channel: G),
          b: (input: "materials/fabric/base_color.png",     channel: B),
          a: (input: "materials/fabric/occlusion.png",      channel: R) ),
        // layer 1 — gravelrock.
        ( r: (input: "materials/gravelrock/base_color.png", channel: R),
          g: (input: "materials/gravelrock/base_color.png", channel: G),
          b: (input: "materials/gravelrock/base_color.png", channel: B),
          a: (input: "materials/gravelrock/occlusion.png",  channel: R) ),
        // layer 2 — pavement.
        ( r: (input: "materials/pavement/base_color.png",   channel: R),
          g: (input: "materials/pavement/base_color.png",   channel: G),
          b: (input: "materials/pavement/base_color.png",   channel: B),
          a: (input: "materials/pavement/occlusion.png",    channel: R) ),
        // layer 3 — metal_02.
        ( r: (input: "materials/metal_02/metal_02_color_1k.png",                    channel: R),
          g: (input: "materials/metal_02/metal_02_color_1k.png",                    channel: G),
          b: (input: "materials/metal_02/metal_02_color_1k.png",                    channel: B),
          a: (input: "materials/metal_02/metal_02_ambient_occlusion_1k.png",        channel: R) ),
        // layer 4 — metal_pattern_01.
        ( r: (input: "materials/metal_pattern_01/metal_pattern_01_color_1k.png",                    channel: R),
          g: (input: "materials/metal_pattern_01/metal_pattern_01_color_1k.png",                    channel: G),
          b: (input: "materials/metal_pattern_01/metal_pattern_01_color_1k.png",                    channel: B),
          a: (input: "materials/metal_pattern_01/metal_pattern_01_ambient_occlusion_1k.png",        channel: R) ),
        // layer 5 — bark_04 (camelCase filenames).
        ( r: (input: "materials/bark_04/bark_04_baseColor_1k.png",         channel: R),
          g: (input: "materials/bark_04/bark_04_baseColor_1k.png",         channel: G),
          b: (input: "materials/bark_04/bark_04_baseColor_1k.png",         channel: B),
          a: (input: "materials/bark_04/bark_04_ambientOcclusion_1k.png",  channel: R) ),
        // layer 6 — snow_01.
        ( r: (input: "materials/snow_01/snow_01_color_1k.png",                    channel: R),
          g: (input: "materials/snow_01/snow_01_color_1k.png",                    channel: G),
          b: (input: "materials/snow_01/snow_01_color_1k.png",                    channel: B),
          a: (input: "materials/snow_01/snow_01_ambient_occlusion_1k.png",        channel: R) ),
        // layer 7 — grass_05 (AO missing → white placeholder = 1.0).
        ( r: (input: "materials/grass_05/grass_05_basecolor_1k.png", channel: R),
          g: (input: "materials/grass_05/grass_05_basecolor_1k.png", channel: G),
          b: (input: "materials/grass_05/grass_05_basecolor_1k.png", channel: B),
          a: (input: "materials/_placeholder/white_1.png",           channel: R) ),
        // layer 8 — stone_wall_04.
        ( r: (input: "materials/stone_wall_04/stone_wall_04_color_1k.png",                    channel: R),
          g: (input: "materials/stone_wall_04/stone_wall_04_color_1k.png",                    channel: G),
          b: (input: "materials/stone_wall_04/stone_wall_04_color_1k.png",                    channel: B),
          a: (input: "materials/stone_wall_04/stone_wall_04_ambient_occlusion_1k.png",        channel: R) ),
        // layer 9 — ground_tiles_08.
        ( r: (input: "materials/ground_tiles_08/ground_tiles_08_color_1k.png",                    channel: R),
          g: (input: "materials/ground_tiles_08/ground_tiles_08_color_1k.png",                    channel: G),
          b: (input: "materials/ground_tiles_08/ground_tiles_08_color_1k.png",                    channel: B),
          a: (input: "materials/ground_tiles_08/ground_tiles_08_ambient_occlusion_1k.png",        channel: R) ),
    ],
)
```

### D.2 `assets/materials/normal.texarray.ron` — re-author in place

```ron
// Normal-map 2D-array — RGB = tangent-space (GL convention, Y-up). Linear.
// Layer order matches diffuse / mrh / emissive (see diffuse.texarray.ron).
(
    format: Rgba8Unorm,
    elements: [
        // layer 0 — fabric (existing files keep their plain `normal.png` name —
        // they were authored as GL convention).
        ( r: (input: "materials/fabric/normal.png",     channel: R),
          g: (input: "materials/fabric/normal.png",     channel: G),
          b: (input: "materials/fabric/normal.png",     channel: B),
          a: (input: "materials/fabric/normal.png",     channel: A) ),
        // layer 1 — gravelrock.
        ( r: (input: "materials/gravelrock/normal.png", channel: R),
          g: (input: "materials/gravelrock/normal.png", channel: G),
          b: (input: "materials/gravelrock/normal.png", channel: B),
          a: (input: "materials/gravelrock/normal.png", channel: A) ),
        // layer 2 — pavement.
        ( r: (input: "materials/pavement/normal.png",   channel: R),
          g: (input: "materials/pavement/normal.png",   channel: G),
          b: (input: "materials/pavement/normal.png",   channel: B),
          a: (input: "materials/pavement/normal.png",   channel: A) ),
        // layer 3 — metal_02.
        ( r: (input: "materials/metal_02/metal_02_normal_gl_1k.png",                channel: R),
          g: (input: "materials/metal_02/metal_02_normal_gl_1k.png",                channel: G),
          b: (input: "materials/metal_02/metal_02_normal_gl_1k.png",                channel: B),
          a: (input: "materials/metal_02/metal_02_normal_gl_1k.png",                channel: A) ),
        // layer 4 — metal_pattern_01.
        ( r: (input: "materials/metal_pattern_01/metal_pattern_01_normal_gl_1k.png", channel: R),
          g: (input: "materials/metal_pattern_01/metal_pattern_01_normal_gl_1k.png", channel: G),
          b: (input: "materials/metal_pattern_01/metal_pattern_01_normal_gl_1k.png", channel: B),
          a: (input: "materials/metal_pattern_01/metal_pattern_01_normal_gl_1k.png", channel: A) ),
        // layer 5 — bark_04.
        ( r: (input: "materials/bark_04/bark_04_normal_gl_1k.png",                  channel: R),
          g: (input: "materials/bark_04/bark_04_normal_gl_1k.png",                  channel: G),
          b: (input: "materials/bark_04/bark_04_normal_gl_1k.png",                  channel: B),
          a: (input: "materials/bark_04/bark_04_normal_gl_1k.png",                  channel: A) ),
        // layer 6 — snow_01.
        ( r: (input: "materials/snow_01/snow_01_normal_gl_1k.png",                  channel: R),
          g: (input: "materials/snow_01/snow_01_normal_gl_1k.png",                  channel: G),
          b: (input: "materials/snow_01/snow_01_normal_gl_1k.png",                  channel: B),
          a: (input: "materials/snow_01/snow_01_normal_gl_1k.png",                  channel: A) ),
        // layer 7 — grass_05.
        ( r: (input: "materials/grass_05/grass_05_normal_gl_1k.png",                channel: R),
          g: (input: "materials/grass_05/grass_05_normal_gl_1k.png",                channel: G),
          b: (input: "materials/grass_05/grass_05_normal_gl_1k.png",                channel: B),
          a: (input: "materials/grass_05/grass_05_normal_gl_1k.png",                channel: A) ),
        // layer 8 — stone_wall_04.
        ( r: (input: "materials/stone_wall_04/stone_wall_04_normal_gl_1k.png",      channel: R),
          g: (input: "materials/stone_wall_04/stone_wall_04_normal_gl_1k.png",      channel: G),
          b: (input: "materials/stone_wall_04/stone_wall_04_normal_gl_1k.png",      channel: B),
          a: (input: "materials/stone_wall_04/stone_wall_04_normal_gl_1k.png",      channel: A) ),
        // layer 9 — ground_tiles_08.
        ( r: (input: "materials/ground_tiles_08/ground_tiles_08_normal_gl_1k.png",  channel: R),
          g: (input: "materials/ground_tiles_08/ground_tiles_08_normal_gl_1k.png",  channel: G),
          b: (input: "materials/ground_tiles_08/ground_tiles_08_normal_gl_1k.png",  channel: B),
          a: (input: "materials/ground_tiles_08/ground_tiles_08_normal_gl_1k.png",  channel: A) ),
    ],
)
```

### D.3 `assets/materials/mrh.texarray.ron` — new file (rename + reorder)

Delete the old `assets/materials/occlusion_roughness_metallic_height.texarray.ron`.
Write `assets/materials/mrh.texarray.ron`:

```ron
// Metallic / Roughness / Height 2D-array (target glTF MRH layout):
//   R ← metallic
//   G ← roughness (perceptual; perceptual^2 inside the BRDF as α — see § E)
//   B ← height in [0,1] (POM source — 0=deepest, 1=surface)
//   A ← unused (zeroed via black placeholder)
//
// Layer order matches diffuse / normal / emissive (see diffuse.texarray.ron).
(
    format: Rgba8Unorm,
    elements: [
        // layer 0 — fabric: legacy `metallic_roughness.png` is glTF-packed
        // (R unused, G=roughness, B=metallic). R metallic ← MR.B; G roughness
        // ← MR.G; B height ← height.R.
        ( r: (input: "materials/fabric/metallic_roughness.png",     channel: B),
          g: (input: "materials/fabric/metallic_roughness.png",     channel: G),
          b: (input: "materials/fabric/height.png",                 channel: R),
          a: (input: "materials/_placeholder/black_1.png",          channel: R) ),
        // layer 1 — gravelrock.
        ( r: (input: "materials/gravelrock/metallic_roughness.png", channel: B),
          g: (input: "materials/gravelrock/metallic_roughness.png", channel: G),
          b: (input: "materials/gravelrock/height.png",             channel: R),
          a: (input: "materials/_placeholder/black_1.png",          channel: R) ),
        // layer 2 — pavement.
        ( r: (input: "materials/pavement/metallic_roughness.png",   channel: B),
          g: (input: "materials/pavement/metallic_roughness.png",   channel: G),
          b: (input: "materials/pavement/height.png",               channel: R),
          a: (input: "materials/_placeholder/black_1.png",          channel: R) ),
        // layer 3 — metal_02 (separate metallic + roughness + height PNGs).
        ( r: (input: "materials/metal_02/metal_02_metallic_1k.png",  channel: R),
          g: (input: "materials/metal_02/metal_02_roughness_1k.png", channel: R),
          b: (input: "materials/metal_02/metal_02_height_1k.png",    channel: R),
          a: (input: "materials/_placeholder/black_1.png",           channel: R) ),
        // layer 4 — metal_pattern_01.
        ( r: (input: "materials/metal_pattern_01/metal_pattern_01_metallic_1k.png",  channel: R),
          g: (input: "materials/metal_pattern_01/metal_pattern_01_roughness_1k.png", channel: R),
          b: (input: "materials/metal_pattern_01/metal_pattern_01_height_1k.png",    channel: R),
          a: (input: "materials/_placeholder/black_1.png",                           channel: R) ),
        // layer 5 — bark_04.
        ( r: (input: "materials/bark_04/bark_04_metallic_1k.png",  channel: R),
          g: (input: "materials/bark_04/bark_04_roughness_1k.png", channel: R),
          b: (input: "materials/bark_04/bark_04_height_1k.png",    channel: R),
          a: (input: "materials/_placeholder/black_1.png",         channel: R) ),
        // layer 6 — snow_01 (no metallic PNG → black placeholder = 0).
        ( r: (input: "materials/_placeholder/black_1.png",           channel: R),
          g: (input: "materials/snow_01/snow_01_roughness_1k.png",   channel: R),
          b: (input: "materials/snow_01/snow_01_height_1k.png",      channel: R),
          a: (input: "materials/_placeholder/black_1.png",           channel: R) ),
        // layer 7 — grass_05 (no metallic, no height → black / gray128).
        ( r: (input: "materials/_placeholder/black_1.png",           channel: R),
          g: (input: "materials/grass_05/grass_05_roughness_1k.png", channel: R),
          b: (input: "materials/_placeholder/gray128_1.png",         channel: R),
          a: (input: "materials/_placeholder/black_1.png",           channel: R) ),
        // layer 8 — stone_wall_04 (no metallic).
        ( r: (input: "materials/_placeholder/black_1.png",                  channel: R),
          g: (input: "materials/stone_wall_04/stone_wall_04_roughness_1k.png", channel: R),
          b: (input: "materials/stone_wall_04/stone_wall_04_height_1k.png",    channel: R),
          a: (input: "materials/_placeholder/black_1.png",                  channel: R) ),
        // layer 9 — ground_tiles_08 (no metallic).
        ( r: (input: "materials/_placeholder/black_1.png",                          channel: R),
          g: (input: "materials/ground_tiles_08/ground_tiles_08_roughness_1k.png",  channel: R),
          b: (input: "materials/ground_tiles_08/ground_tiles_08_height_1k.png",     channel: R),
          a: (input: "materials/_placeholder/black_1.png",                          channel: R) ),
    ],
)
```

**Decision on the old file.** Delete
`assets/materials/occlusion_roughness_metallic_height.texarray.ron` in the
same patch. No consumer in the codebase references it (`grep -rn
"occlusion_roughness_metallic_height" crates/ bins/`) — it was only a target
of `just bake-texarrays`. Keeping a deprecated file around invites the
implementer or the next contributor to accidentally re-wire it; deleting it
is the cleaner move and the git history preserves the prior content.

### D.4 `assets/materials/emissive.texarray.ron` — new file

```ron
// Emissive 2D-array — RGB = emissive HDR colour, A unused. sRGB. Only the
// Emissive-base-class voxels sample this; for PBR layers every slot uses
// the black 1×1 placeholder so the texture exists but the sample is zero.
// Layer order matches diffuse / normal / mrh.
(
    format: Rgba8UnormSrgb,
    elements: [
        // layer 0 — fabric (no emissive).
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        // layer 1 — gravelrock (no emissive).
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        // layer 2 — pavement (has emissive.png).
        ( r: (input: "materials/pavement/emissive.png",    channel: R),
          g: (input: "materials/pavement/emissive.png",    channel: G),
          b: (input: "materials/pavement/emissive.png",    channel: B),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        // layer 3..9 — none of the new materials ship an emissive PNG, so
        // every layer is the black placeholder.
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
        ( r: (input: "materials/_placeholder/black_1.png", channel: R),
          g: (input: "materials/_placeholder/black_1.png", channel: R),
          b: (input: "materials/_placeholder/black_1.png", channel: R),
          a: (input: "materials/_placeholder/black_1.png", channel: R) ),
    ],
)
```

### `.png.meta` sidecars for the new source files

`bake.rs` requires every source PNG to ship a `Load`-action `.meta` sidecar
(`bake.rs:90-92` + `texture_array/mod.rs:50-69`). The existing
`assets/materials/{fabric,gravelrock,pavement}/*.png.meta` are the template:

- For each new sourced PNG (per § D.1–D.4, the entries with paths
  like `materials/metal_02/metal_02_color_1k.png`), the implementer must
  write a `.png.meta` sidecar with `is_srgb: true` for `*_color_*` /
  `*_basecolor_*` / `*_baseColor_*` / `emissive*`, and `is_srgb: false` for
  every other channel (normal, roughness, metallic, height, AO).
- The three placeholders (`_placeholder/black_1.png`, `white_1.png`,
  `gray128_1.png`) need `.meta` sidecars with `is_srgb: false`.

This is mechanical scaffolding — copy the existing template, adjust the
`is_srgb` flag.

---

## E. Unified BRDF — WGSL composition

Create `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` (new
naga-oil import module).

The unified BRDF entry point computes, for one bounce direction, the BRDF
value `f(wi, wo)` weighting the bounce throughput, using `albedo`,
`metallic`, `roughness` already sampled from the texture arrays.

### Energy-conserving composition (D8 success criterion)

```wgsl
// `eval_pbr` — the unified energy-conserving GGX-Smith-Schlick BRDF, used by
// every PBR-class hit. Reuses `sample_vndf_isotropic` /
// `pdf_vndf_isotropic` / `geometry_term` (ray_tracing_common.wgsl) and
// `get_reflectance_fresnel` (render_pipeline_common.wgsl). Zero new
// trigonometric primitives.
//
// `light_dir` is the direction towards the light (sampled), `view_dir` is the
// direction towards the camera (the incoming ray reversed), both in world
// space, both unit. `normal` is the perturbed (normal-mapped) surface normal,
// unit. `albedo` is the texture-sampled, tint-multiplied linear-space RGB.
// `metallic` ∈ [0,1] and `perceptual_roughness` ∈ [0,1] are sampled from MRH.
//
// Returns the BRDF value `f`, ready to multiply into the bounce accumulator:
//   radiance += throughput * f * cosTheta_l * incoming_radiance
// (The cosTheta_l and the light-side hemisphere sample stay where the
// caller already does them — see § E call-site map.)
struct PbrEval {
    f: vec3<f32>,           // the BRDF value (diffuse + specular)
    fresnel: vec3<f32>,     // F at the half-vector — for energy-conserving
                            // mix into the next bounce's throughput
    f0: vec3<f32>,          // for the e2e gate's "metallic F0 ≈ albedo" check
}

fn eval_pbr(
    light_dir: vec3<f32>,
    view_dir:  vec3<f32>,
    normal:    vec3<f32>,
    albedo:    vec3<f32>,
    metallic:  f32,
    perceptual_roughness: f32,
) -> PbrEval {
    let alpha = perceptual_roughness * perceptual_roughness;        // α = r²
    let half_dir = normalize(light_dir + view_dir);
    let n_dot_l = clamp(dot(normal, light_dir), 0.0, 1.0);
    let n_dot_v = clamp(dot(normal, view_dir),  0.0, 1.0);
    let v_dot_h = clamp(dot(view_dir, half_dir), 0.0, 1.0);
    let n_dot_h = clamp(dot(normal, half_dir),  0.0, 1.0);

    // F₀ — base specular reflectance. Dielectric ≈ 0.04 (n≈1.5 plastic);
    // pure metal = albedo (energy-conserving).
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);

    // Schlick Fresnel at the half-vector (specular-side incident angle).
    // Re-use `get_reflectance_fresnel`'s shape, but it takes an IOR triple;
    // for the F0-based form we inline Schlick directly (3-instruction).
    let one_minus_voh = 1.0 - v_dot_h;
    let f = f0 + (vec3<f32>(1.0) - f0) * pow(one_minus_voh, 5.0);

    // GGX-Smith specular numerator: D * G_in * G_out * F.
    let alpha2 = alpha * alpha;
    let denom_term = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
    let d = alpha2 / (PI * denom_term * denom_term);
    let g_in  = geometry_term(perceptual_roughness, n_dot_l);
    let g_out = geometry_term(perceptual_roughness, n_dot_v);
    // 4 * n·l * n·v is the BRDF denominator — guard ε for grazing angles.
    let denom_brdf = max(4.0 * n_dot_l * n_dot_v, 1e-4);
    let specular = (d * g_in * g_out * f) / denom_brdf;

    // Diffuse: Lambertian with energy taken away by the specular lobe AND
    // suppressed when metallic (metals have no diffuse).
    //   kS = F; kD = (1 - F) * (1 - metallic); diffuse = albedo / PI.
    let k_d = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    let diffuse = k_d * albedo / PI;

    var out: PbrEval;
    out.f = diffuse + specular;
    out.fresnel = f;
    out.f0 = f0;
    return out;
}
```

### Where it gets called

**Sun-sample arm — `naadf_global_illum.wgsl:340-385`** (the per-bounce sun
shadow). Today there are two arms (`SURFACE_SPECULAR_ROUGH` evaluates the
GGX numerator explicitly at lines 356-368; the diffuse arm just keeps the
`fac` Lambert weight at line 354). Collapse both to one `eval_pbr` call:

```wgsl
// Before (lines 351-368, the rough-specular arm). After:
let pbr = eval_pbr(
    sun_dir_rand,                       // light_dir
    -cur_dir,                           // view_dir
    surface_normal,                     // perturbed normal (from triplanar)
    sampled_albedo,                     // tinted texture sample
    sampled_metallic,
    sampled_roughness,
);
// fac becomes a pure vec3 (no scalar broadcast):
var fac = pbr.f * (2.0 * clamp(dot(surface_normal, sun_dir_rand), 0.0, 1.0));
```

The C# `2 * cos(θ_l)` weight is preserved (matches NAADF). The
`SURFACE_SPECULAR_ROUGH` branch deletes; the diffuse branch deletes.

**Bounce-direction selection — `naadf_global_illum.wgsl:394-428`** (the
"surface-effect bounce"). The current three-way fork (mirror / rough-spec /
diffuse) collapses into one PBR path that ALWAYS samples `sample_vndf_isotropic`
for the next direction (with `alpha = perceptual_roughness²`), then weights
`cur_absorption *= pbr.fresnel * pbr_geom_factor + (1-pbr.fresnel)*(1-metallic)*albedo*cosTheta*2`.
Implementer derivation: extract from the existing rough-specular arm
(`naadf_global_illum.wgsl:401-424`) — it already does VNDF sampling +
geometry + Fresnel; the post-pivot path is identical mathematically modulo
the metallic split (`(1-F)*(1-metallic)*albedo` diffuse re-injection).

**Direct-mirror branch in the first-hit pass — `naadf_first_hit.wgsl:174-258`**.
This 4-iteration mirror loop is the C# "perfectly smooth metallic"
reflection chain — it terminates when it hits a non-mirror surface. After
the pivot **there is no `Mirror` material class** — but the loop is still
useful for any PBR voxel whose sampled `roughness ≈ 0` (a mirror-finish
material). Decision: gate the mirror-bounce-loop continue condition on
`sampled_roughness < MIRROR_ROUGHNESS_EPSILON` (default `0.05`) instead of
`material_base == SURFACE_SPECULAR_MIRROR`. The loop still calls
`get_reflectance_fresnel` and `reflect(ray_dir, normal)`; only the
condition changes.

```wgsl
// Replaces the `if material_base != SURFACE_SPECULAR_MIRROR { ... } else { ... }`
// branch at naadf_first_hit.wgsl:232-258. Pseudocode:
let mrh = triplanar_sample(pbr_mrh, ...);                    // see § F
let sampled_roughness = mrh.g;
let sampled_metallic  = mrh.r;
let sampled_albedo    = triplanar_sample(pbr_diffuse_ao, ...).rgb * ty.albedo_tint;

if (ty.material_base == 1u) {  // Emissive fast-path — see § H.
    let emissive = triplanar_sample(pbr_emissive, ...).rgb * ty.color_layered;
    acc.light = acc.light + acc.absorption * emissive;
    distance_ray = dist + volume.dist_min_max.x;
    voxel_type_raw = ray_result.hit_type;
    is_diffuse = 1u;
    break;
}

// PBR fast-path — multiply absorption by `(1-metallic)*albedo` (diffuse colour
// transport), record is_diffuse based on roughness, then either continue
// the mirror loop (sampled_roughness < ε) or terminate and defer to GI.
if (sampled_roughness < MIRROR_ROUGHNESS_EPSILON) {
    // The C# mirror branch (unchanged shape):
    let cos_theta = clamp(dot(ray_result.normal, -ray_dir), 0.0, 1.0);
    let f0 = mix(vec3<f32>(0.04), sampled_albedo, sampled_metallic);
    let one_minus_ct = 1.0 - cos_theta;
    let r = f0 + (vec3<f32>(1.0) - f0) * pow(one_minus_ct, 5.0);
    acc.absorption = acc.absorption * r;
    ray_dir = reflect(ray_dir, ray_result.normal);
    old_pos = vec3<f32>(cur_pos_int) + cur_pos_frac;
    i = i + 1u;
    continue;
}

// Non-mirror PBR: terminate (the GI pass takes over).
let albedo_attenuation = (vec3<f32>(1.0) - vec3<f32>(sampled_metallic)) * sampled_albedo;
acc.absorption = acc.absorption * albedo_attenuation;
distance_ray = dist + volume.dist_min_max.x;
voxel_type_raw = ray_result.hit_type;
is_diffuse = select(1u, 0u, sampled_roughness < ROUGH_SPECULAR_DIFFUSE_THRESHOLD);
break;
```

`MIRROR_ROUGHNESS_EPSILON = 0.05` and `ROUGH_SPECULAR_DIFFUSE_THRESHOLD = 0.5`
are WGSL consts at the top of `naadf_first_hit.wgsl` (with rationale comments).
The `is_diffuse` semantics — `0` defers specular-rough to GI, `1` treats as
Lambertian — is **preserved**.

**Spatial resampling `get_brdf` — `spatial_resampling.wgsl:129-146`**. The
existing function evaluates GGX-Smith specular with IOR Fresnel. After the
pivot, replace its body to call `eval_pbr` and return `.f` (sum of
diffuse + specular) — preserves the call shape, swaps the BRDF model.

### Where `eval_pbr` lives in `pbr_sampling.wgsl` import surface

Re-exported via naga-oil module path:

```wgsl
#import "shaders/pbr_sampling.wgsl"::{
    PbrEval, eval_pbr,
    triplanar_blend_weights, triplanar_sample, triplanar_sample_normal,
    pom_displace_uv, select_layer_variant,
    MIRROR_ROUGHNESS_EPSILON, ROUGH_SPECULAR_DIFFUSE_THRESHOLD,
}
```

---

## F. Triplanar + POM helpers — WGSL

All three live in the new `pbr_sampling.wgsl` module.

### F.1 Triplanar blend weights

```wgsl
// Sharpen the weights so axis-aligned faces sample one plane only. k=8 is
// the de-facto "sharp axis-aligned voxel face" tuning — for the AADF voxel
// normals (which ARE axis-aligned ± epsilon from the normal map) one
// projection dominates and the other two contribute ≤1% — effectively a
// "pick the dominant axis" decision under linear filtering.
const TRIPLANAR_BLEND_SHARPNESS: f32 = 8.0;

fn triplanar_blend_weights(n: vec3<f32>) -> vec3<f32> {
    let w = pow(abs(n), vec3<f32>(TRIPLANAR_BLEND_SHARPNESS));
    let s = max(w.x + w.y + w.z, 1e-4);
    return w / s;
}
```

### F.2 Triplanar sample (data channels — albedo / MRH / emissive)

```wgsl
// World-space UV scale: 1 voxel = WORLD_UV_SCALE texture units. 1.0 means
// the texture tiles once per voxel — for 1×1×1 m voxels and 1m-tiling
// textures (the AmbientCG default). The HUD doesn't expose this yet;
// implementer ships 1.0 with a comment.
const WORLD_UV_SCALE: f32 = 1.0;

// Triplanar 3-plane sample of a single texture-array layer. `world_pos` is
// the camera-int-relative hit position (which the existing shaders already
// compute as `cur_pos_frac + camera.cam_pos_int`-equivalent), `weights`
// are the pre-computed blend weights, `layer` is the array layer.
fn triplanar_sample(
    tex:       texture_2d_array<f32>,
    smp:       sampler,
    world_pos: vec3<f32>,
    weights:   vec3<f32>,
    layer:     u32,
) -> vec4<f32> {
    let p = world_pos * WORLD_UV_SCALE;
    // YZ plane (X-facing), XZ plane (Y-facing), XY plane (Z-facing).
    let s_x = textureSampleLevel(tex, smp, p.yz, i32(layer), 0.0);
    let s_y = textureSampleLevel(tex, smp, p.zx, i32(layer), 0.0);
    let s_z = textureSampleLevel(tex, smp, p.xy, i32(layer), 0.0);
    return s_x * weights.x + s_y * weights.y + s_z * weights.z;
}
```

The Y plane uses `p.zx` (not `p.xz`) so the textures look the same when the
camera rotates around the Y axis — a standard triplanar convention to keep
plane orientation handedness consistent.

`textureSampleLevel(..., 0.0)` is used instead of `textureSample(...)`
because the shaders run in compute contexts (no implicit derivatives are
available). Mip level 0 is fine for 1K textures; if mipmapping becomes
necessary the implementer can switch to a manual derivative.

### F.3 Triplanar normal-map sample — RNM blend

For tangent-space normals (the GL-convention normal maps) the WGSL
"reoriented-normal-mapping" (RNM) blend gives the correct world-space
perturbed normal for an axis-aligned face triplanar projection (Christopher
Oat / Stephen Hill 2013 — the de-facto choice for triplanar PBR). Cite:
[Ben Golus, "Normal Mapping for a Triplanar Shader" (2017)](https://bgolus.medium.com/normal-mapping-for-a-triplanar-shader-10bf39dca05a).

```wgsl
// Decode a tangent-space normal byte triplet (R,G,B in [0,1]) to a
// world-space unit normal vector under the triplanar projection.
// Each plane's UV/world mapping is fixed by `triplanar_sample` above; the
// normal-blend follows the same plane assignment.
//
// Plane assignment:
//   x-plane (YZ): tangent-x = world-z, tangent-y = world-y, normal = world-x
//   y-plane (XZ): tangent-x = world-x, tangent-y = world-z, normal = world-y
//   z-plane (XY): tangent-x = world-x, tangent-y = world-y, normal = world-z
fn triplanar_sample_normal(
    tex:       texture_2d_array<f32>,
    smp:       sampler,
    world_pos: vec3<f32>,
    weights:   vec3<f32>,
    face_normal: vec3<f32>,  // the unperturbed voxel-face normal (axis-aligned)
    layer:     u32,
) -> vec3<f32> {
    let p = world_pos * WORLD_UV_SCALE;
    let n_x_local = textureSampleLevel(tex, smp, p.yz, i32(layer), 0.0).xyz * 2.0 - 1.0;
    let n_y_local = textureSampleLevel(tex, smp, p.zx, i32(layer), 0.0).xyz * 2.0 - 1.0;
    let n_z_local = textureSampleLevel(tex, smp, p.xy, i32(layer), 0.0).xyz * 2.0 - 1.0;

    // Lift each tangent-space normal into world space. Choose axis sign from
    // face_normal so a flipped face produces a correctly-flipped normal.
    let sign_x = sign(face_normal.x);
    let sign_y = sign(face_normal.y);
    let sign_z = sign(face_normal.z);

    let n_x_world = vec3<f32>(n_x_local.z * sign_x, n_x_local.y, n_x_local.x);
    let n_y_world = vec3<f32>(n_y_local.x, n_y_local.z * sign_y, n_y_local.y);
    let n_z_world = vec3<f32>(n_z_local.x, n_z_local.y, n_z_local.z * sign_z);

    return normalize(n_x_world * weights.x
                   + n_y_world * weights.y
                   + n_z_world * weights.z);
}
```

(Implementer: double-check the swizzles by running the e2e `--pbr-visual`
gate; a mistake here shows up as a normal pointing "the wrong way" and
breaks the highlight position check.)

### F.4 POM — linear 8-tap + 4-tap binary refine, dominant-plane only

```wgsl
const POM_HEIGHT_SCALE: f32 = 0.05;   // world units the height map "pushes"
                                       // into the voxel face — fraction of
                                       // a voxel side; 0.05 = 5% so POM
                                       // doesn't ooze past the next voxel.
const POM_LINEAR_STEPS: i32 = 8;
const POM_BINARY_STEPS: i32 = 4;

// Displace a 2D UV by sampling the MRH.B height channel along the
// view direction projected into the plane's UV space.
// Returns the displaced UV (suitable for a final albedo/normal/MRH
// re-sample) — caller passes one of `world_pos.yz`, `.zx`, `.xy` and the
// matching `view_dir.yz/.zx/.xy`.
fn pom_displace_uv(
    mrh_tex:    texture_2d_array<f32>,
    smp:        sampler,
    base_uv:    vec2<f32>,
    view_dir_2d:vec2<f32>,   // view_dir projected into the plane (un-normalised)
    layer:      u32,
) -> vec2<f32> {
    let dir = view_dir_2d * POM_HEIGHT_SCALE;
    let step = dir / f32(POM_LINEAR_STEPS);
    var uv = base_uv;
    var prev_uv = uv;
    var prev_layer_depth = 0.0;
    var depth = 0.0;
    var sampled = 1.0;

    for (var i: i32 = 0; i < POM_LINEAR_STEPS; i = i + 1) {
        prev_uv = uv;
        prev_layer_depth = depth;
        uv = uv + step;
        depth = depth + 1.0 / f32(POM_LINEAR_STEPS);
        sampled = textureSampleLevel(mrh_tex, smp, uv, i32(layer), 0.0).b;
        if (depth >= 1.0 - sampled) { break; }
    }

    // Binary refine between (prev_uv, uv).
    var lo = prev_uv;
    var hi = uv;
    var lo_depth = prev_layer_depth;
    var hi_depth = depth;
    for (var i: i32 = 0; i < POM_BINARY_STEPS; i = i + 1) {
        let mid = 0.5 * (lo + hi);
        let mid_depth = 0.5 * (lo_depth + hi_depth);
        let mid_sample = textureSampleLevel(mrh_tex, smp, mid, i32(layer), 0.0).b;
        if (mid_depth >= 1.0 - mid_sample) {
            hi = mid; hi_depth = mid_depth;
        } else {
            lo = mid; lo_depth = mid_depth;
        }
    }
    return 0.5 * (lo + hi);
}
```

**Application: dominant projection only.** Apply POM only to the projection
whose weight is the maximum of the three triplanar weights, then re-sample
albedo/normal/MRH at the displaced UV on that one plane; the other two
planes use the un-displaced UV. The non-dominant weights are ≤ 0.05 with
`TRIPLANAR_BLEND_SHARPNESS = 8` on axis-aligned voxel normals, so their POM
contribution is invisible — paying ~16 height-samples × 3 planes (~48 total)
for an invisible improvement is wasteful. **Cost: one POM displacement = 12
height samples; per PBR hit = 1 POM (dominant plane only) + 3 triplanar
samples × 3 maps (Diffuse, Normal, MRH) = 9, total ≈ 21 texSamples.** Worst
case at a face oriented 45° between two planes the dominant weight is ~0.7;
the second plane contributes ~0.3 unperturbed parallax — visible as a slight
mismatch only on those rare oblique faces. Acceptable for v1.

---

## G. D6 variant-selection design

### Decision: hard-select, 1 variant per VoxelType for the first cut

`VoxelType` carries `variant_span` (in `GpuVoxelType`'s `data[0]`
`VOXEL_GPU_VARIANT_MASK`). Decoded value is `1 << variant_span_log2`. For
the first cut every type in `build_palette` packs `variant_span_log2 = 0`
⇒ 1 variant (no procedural blend, no extra texSample, no hash). The bit
field is reserved so a later patch can flip on variants without touching
the buffer layout.

### Hash function (when variants > 1)

```wgsl
// PCG3D — Jarzynski & Olano (2020), "Hash Functions for GPU Rendering".
// Pure, branch-free, three-component → three-component. Used to pick a
// variant from the integer voxel position.
fn pcg3d(seed: vec3<u32>) -> vec3<u32> {
    var v = seed * 1664525u + 1013904223u;
    v.x = v.x + v.y * v.z;
    v.y = v.y + v.z * v.x;
    v.z = v.z + v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x = v.x + v.y * v.z;
    v.y = v.y + v.z * v.x;
    v.z = v.z + v.x * v.y;
    return v;
}

// `select_layer_variant` — pick one of `variant_span` adjacent layers
// (base, base+1, ..., base+variant_span-1) for the integer voxel position.
// `variant_span` must be 1, 2, 4, 8, ..., 128 (a power of two — see § B
// bit-field encoding). Returns `base_layer` when `variant_span == 1`.
fn select_layer_variant(base_layer: u32, variant_span: u32, voxel_pos: vec3<i32>) -> u32 {
    if (variant_span <= 1u) { return base_layer; }
    let h = pcg3d(vec3<u32>(voxel_pos)).x;
    return base_layer + (h & (variant_span - 1u));
}
```

`voxel_pos` is already available in `RayResult.voxel_pos` (`ray_tracing.wgsl:147`),
read once per hit. The cost is 9 muls + 6 xors per hit (negligible).

### Hard-select vs cross-fade

Hard-select chosen for the first cut. **Rationale:** halves the texSample
cost vs cross-fade (which doubles every sample to interpolate between two
variants by the fractional hash). The seams of hard-select on a flat
ground will be visible as voxel-cell-size colour discontinuities — but the
voxels are big (1 m), the variants are similar materials (e.g.
`metal_02` + `metal_pattern_01` would tile coherently), and the variant
mechanic is OFF for every VoxelType in this PR. A cross-fade implementation
is a 1-day follow-up that only touches `pbr_sampling.wgsl`.

---

## H. Emissive fast-path — exact semantics

### Decision: SAMPLE the Emissive array

The Emissive array IS bound (§ C makes it part of `MaterialSet`); the cost
is only paid by Emissive-class voxels. A PBR voxel takes the `material_base
== 0` branch first and never touches the Emissive texture. **The Emissive
fast-path skips the BRDF entirely AND skips POM** — it does:

1. one triplanar sample of `pbr_emissive` (3 texSamples, no POM)
2. multiply by per-VoxelType `color_layered`
3. add to `acc.light * acc.absorption`
4. terminate the bounce loop (or in GI: terminate the bounce with
   `radiance += cur_absorption * emissive`)

### Composition

```
emissive_rgb = sampled_emissive_rgb * color_layered
```

`sampled_emissive_rgb` is the sRGB-decoded RGB of the Emissive array sample
(already linear after `Rgba8UnormSrgb` decode). `color_layered` is the
per-VoxelType HDR multiplier (e.g. `(8.0, 7.4, 6.2)` for the warm emissive
in § A). For the 10-material set only `pavement` (layer 2) has a real
emissive texture; the others sample black, so an Emissive-base voxel
referencing one of those layers emits zero — by design, the
`color_layered` multiplier alone is the colour for those (multiplied with
the black RGB ⇒ zero). **Therefore the implementer must ensure Emissive
voxels reference `material_layer_index = 2` (pavement) OR a follow-up adds
a "constant white" 1×1 sample in the Emissive array.**

> For the existing palette (§ A grid-palette table), every emissive entry
> uses `material_layer_index = 2` — pavement's emissive PNG provides the
> RGB pattern, `color_layered` provides the colour + intensity. If the
> implementer encounters a visual oddity (the pavement emissive texture's
> pattern leaking into other emissive voxels), the easy fix is to make the
> Emissive fast-path use a constant `vec3(1.0)` instead of sampling — the
> branch becomes:
>
> ```wgsl
> let emissive = ty.color_layered;
> ```
>
> and the emissive array sample is unused. Skipping the sample entirely
> for emissive saves 3 texSamples per Emissive hit; the cost is per-Emissive
> texture variation. Recommendation: SHIP WITH THE SAMPLE; if the visual
> oddity manifests, the swap is one line.

---

## I. New e2e gate spec

### Gate flag: `--pbr-visual`

Add to the dispatch in `crates/bevy_naadf/src/bin/e2e_render.rs:96-105`
(alongside `oasis_edit_visual_mode` / `small_edit_visual_mode` / etc.):

```rust
let pbr_visual_mode = args.iter().any(|a| a == "--pbr-visual");
// ...
} else if pbr_visual_mode {
    bevy_naadf::e2e::pbr_visual::run_pbr_visual()
}
```

### Scene + pose

A new file `crates/bevy_naadf/src/e2e/pbr_visual.rs`. Use the
**default `build_palette` test grid** (no need for VOX loading), and
pin the camera at a side-on pose looking at one of the metallic columns —
the pillar (VoxelType 8, mapped to `metal_02`, fully metallic with
metallic≈1.0 sampled from the texture). The pose template comes from the
existing `e2e/small_edit_visual.rs` (uses the default grid + a fixed
camera). Implementer:

- Set `AppArgs::default()` (grid_preset stays `Default`, the standard
  build_palette test scene).
- Pin camera at `Transform::from_xyz(GRID_X*0.5 + 20.0, GRID_Y*0.7,
  GRID_Z*0.5).looking_at(Vec3::new(GRID_X*0.5, GRID_Y*0.3, GRID_Z*0.5),
  Vec3::Y)` (a slight-overhead side view of the pillar). Exact constants
  come from `e2e::gates::e2e_motion_start_transform` (which produces a
  similar pose for the standard gate).
- Warm-up frames: 150 (TAA + GI converge — same as oasis_edit_visual:
  `OASIS_WARMUP_FRAMES = 120`).
- Capture one framebuffer A.

### Assertion targets

The captured framebuffer is asserted via three checks:

| Check | Region | Metric | Threshold |
|---|---|---|---|
| 1. Specular highlight present | Manually-pinned `Rect { x0: 250, y0: 90, x1: 290, y1: 130 }` (a 40×40 px region containing the sun-side highlight on the pillar) | `Framebuffer::region_luminance(rect)` | `> 130.0` (a "the highlight is bright" floor; ambient lit pillar without specular sits at ~60-90 luminance from prior gate data) |
| 2. Albedo texture variation | Manually-pinned `Rect { x0: 200, y0: 200, x1: 280, y1: 280 }` (a 80×80 px region on a textured surface — the wall, layer 9 = snow_01) | Sample 16 pixels at fixed offsets `(rect.x0 + 10*i, rect.y0 + 10*i)` for `i in 0..16`; compute std-dev of the 16 R+G+B luminance values | `> 5.0` (a "this is not a flat colour" floor; sampled values from textured surfaces empirically vary by 10-40 luminance units; a flat colour-only fallback regression yields std-dev ~0-2) |
| 3. Metallic F0 ≈ albedo (colour pull) | Manually-pinned `Rect { x0: 320, y0: 150, x1: 360, y1: 190 }` (40×40 on the metallic pillar specular hot-spot) | `Framebuffer::region_mean(rect)` returns `[r, g, b, a]`; verify `r/g` ratio and `r/b` ratio are within 30% of the expected metal_02 albedo tint ratio (which is roughly neutral — implementer pins from a screenshot of the converged frame) | `(r/g) ∈ [0.7*expected, 1.3*expected]` AND `(r/b) ∈ [0.7*expected, 1.3*expected]` |

Pixel coordinates are placeholders; the implementer pins them after running
the gate once and inspecting the saved PNG. Document the pinning step in
the module-level doc comment.

The gate saves a screenshot to
`target/e2e-screenshots/pbr_visual_baseline.png` on PASS for review.

### Dispatch-table line in `bins/e2e_render.rs`

The new branch is inserted at line ~313 (just before the trailing
`entities_mode` / default `else { bevy_naadf::run_e2e_render() }` block).
Mirrors `--vox-e2e`'s shape: one-line module call returning an
`AppExit`.

### Wall-clock budget

The orchestrator's memory entry `feedback-e2e-gates-must-fail-fast` is
binding: this gate runs in ≤ 120s. With 150 warmup + 16 drain ≈ 170 frames
at ~30fps in e2e ≈ 6 s; the budget is well under the cap. No timeout
needed unless integration shows otherwise.

---

## J. Migration / cleanup plan

### `SURFACE_*` const removal

`render_pipeline_common.wgsl:37-40` define `SURFACE_DIFFUSE: u32 = 0u`,
`SURFACE_EMISSIVE: u32 = 1u`, `SURFACE_SPECULAR_ROUGH: u32 = 2u`,
`SURFACE_SPECULAR_MIRROR: u32 = 3u`. Replace with two consts:

```wgsl
const SURFACE_PBR: u32 = 0u;
const SURFACE_EMISSIVE: u32 = 1u;
```

Every shader that imports the old four consts updates its
`#import "shaders/render_pipeline_common.wgsl"::{...}` list to the new two.
Verified import sites (search `SURFACE_DIFFUSE\|SURFACE_SPECULAR_ROUGH\|SURFACE_SPECULAR_MIRROR`):

| File | Branches collapsed |
|---|---|
| `naadf_first_hit.wgsl:54, :232-258` | The 4-way mirror/non-mirror branch collapses to `if material_base == SURFACE_EMISSIVE { /* fast-path */ } else { /* PBR; see § E call-site */ }`. Mirror-continue is now gated on `sampled_roughness < MIRROR_ROUGHNESS_EPSILON`, NOT on `material_base`. |
| `naadf_global_illum.wgsl:44, :252-276, :336-441` | Three-way bounce / sun-sample collapses to one PBR path + Emissive fast-path; `eval_pbr` consumes `sampled_albedo/metallic/roughness`. |
| `spatial_resampling.wgsl:130-146` (`get_brdf`) | Body becomes `let pbr = eval_pbr(...); return pbr.f;` (preserves signature). |

### CPU consumers of `MaterialBase` enum values

- `voxel/grid.rs:594, 602, 610, ...` (12 sites): every `material_base:
  MaterialBase::Diffuse` becomes `material_base: MaterialBase::Pbr`; every
  `material_base: MaterialBase::Emissive` stays. No `MetallicRough` or
  `MetallicMirror` usage anywhere — `grep -n 'MetallicRough\|MetallicMirror'`
  in `crates/` finds only `voxel/mod.rs` (the enum decl) and
  `gpu_types.rs` (a comment + the test). Both delete-clean.
- `vox_import.rs:988-992`: the `if emission > 0.0` branch returns either
  `MaterialBase::Emissive` or `MaterialBase::Diffuse`. The latter becomes
  `MaterialBase::Pbr`.
- The unit test in `gpu_types.rs:918-933` rewrites against the new bit
  layout (see § B).
- `editor/hud.rs:558-561` uses `vt.color_base.{x,y,z}` for the palette
  swatch colour — migrate to `srgb_byte_to_f32(vt.albedo_tint[c])` (or
  approximate: `vt.albedo_tint[c] as f32 / 255.0`).

### `material_layer` field removal

`grep -rn 'material_layer\b'` returns only the enum decl + the 12
`build_palette` literals (`material_layer: MaterialLayer::None`) + the GPU
packer/unpacker reference. All sites either drop the field (CPU literals)
or compute the new variant_span / layer_index encoding (GPU packer/unpacker).

### Old `MaterialRon` / `MaterialRonLoader` — leave alone

`crates/bevy_naadf/src/baked_material.rs` is **unchanged**. It feeds Bevy's
`StandardMaterial` mesh pipeline, not the raymarcher. Forbidden move per
`01-context.md`.

### `occlusion_roughness_metallic_height.texarray.ron` cleanup

**Delete** the file in the same patch. No code references it; the renamed
`mrh.texarray.ron` supersedes it. Keeping it confuses future readers.

### `MaterialLayer` enum cleanup

Delete the entire enum from `voxel/mod.rs:95-104`. The `gpu_types.rs:26`
import statement updates to drop the `MaterialLayer` symbol. The trailing
"keep referenced so future format-change can't drift" const at
`gpu_types.rs:897-901` removes the `let _ = MaterialLayer::None;` line.

---

## Decisions & rejected alternatives

1. **`Resource` vs `Asset` for `MaterialSet`.** Chose **Resource** (programmatic
   construction in `MaterialSetPlugin`). Rejected `.matset.ron` `Asset` because
   the four `.texarray.ron` files already are the source-of-truth for the
   layer ordering; a `.matset.ron` would only restate the four paths and add a
   parser/loader/parity-check for no current consumer benefit. The Resource is
   trivial to promote later if a multi-material-set authoring tool ever lands.

2. **`albedo_tint` as `[u8; 3]` (24 bits) vs `Vec3` f16 (48 bits).** Chose
   **`[u8; 3]`** (8-bit sRGB byte). Rationale: the tint is a user-pickable
   colour in the HUD; 8-bit per channel is what every standard colour picker
   produces, and 256 distinct values per channel is past human discrimination
   for tint multipliers (the texture provides the high-frequency variation,
   the tint is a low-frequency multiplier). Saves 24 bits in `GpuVoxelType`.
   The neutral value `[255, 255, 255]` quantises to exactly 1.0 in the
   shader; no banding artefacts at the high end. Rejected f16 because the
   extra precision would never be perceptible.

3. **`variant_span_log2` as 3-bit (1..128 variants).** Chose 3 bits because
   8 binary choices (1, 2, 4, 8, 16, 32, 64, 128) cover every realistic
   variant-count without dedicating more of the precious `data[0]` budget.
   Rejected a raw u4 `variant_count` because the hash-select arithmetic
   (`h & (variant_span - 1u)`) requires a power-of-two; encoding `log2`
   directly avoids a runtime power-of-two check.

4. **`material_layer_index` as 12 bits (4096 materials).** Chose 12 bits as
   a balance between "way bigger than the 10-material starter set" and
   "fits comfortably". Rejected 16 bits (would consume the entire
   `data[0]` low half — leaving no room for the variant span). Rejected
   8 bits because 256 materials is in the realistic upper bound for a
   single set; 12 bits leaves a 16× safety margin.

5. **POM applies to dominant projection only.** Chose dominant-only because
   `TRIPLANAR_BLEND_SHARPNESS = 8` on axis-aligned voxel normals already
   reduces the non-dominant weights to ≤ 0.05. The non-dominant plane's
   POM contribution would be invisible (5% of zero parallax). Rejected
   per-plane POM (3× the cost for invisible improvement). Rejected "no
   POM" — D5 explicitly includes POM, the user expects it.

6. **`MIRROR_ROUGHNESS_EPSILON = 0.05`** — the threshold above which the
   first-hit pass stops the mirror-bounce loop and treats the surface as
   rough-PBR (defers to GI). Below 0.05 ≈ "polished metal / glass" — the
   mirror loop's 4 perfect-reflect bounces are visually correct. Rejected
   0.0 (pure mirrors only — most fine-roughness materials would defer to
   GI's stochastic sampling, which produces visibly noisier mirror
   highlights at low roughness for the same noise budget). Rejected 0.1
   (the rough-specular GI BRDF starts producing visible highlights again
   at this roughness, but the visual quality of 4-bounce-reflect at
   `r=0.1` is fuzzy enough to suggest GI). Picked 0.05 as the
   inflection — implementer may tune live with the user.

7. **`ROUGH_SPECULAR_DIFFUSE_THRESHOLD = 0.5`** — splits the `is_diffuse`
   flag the same way C# `SURFACE_SPECULAR_ROUGH` (=2) vs
   `SURFACE_DIFFUSE` (=0) split it. Rough surfaces above 0.5 are nearly
   Lambertian (GGX D peaks broad), so `is_diffuse=1` keeps the GI's
   uniform-hemisphere sample efficient. Below 0.5 they have a specular
   lobe, and the VNDF importance-sample wins for sample efficiency
   (`is_diffuse=0`). Verified the threshold matches the C# behaviour —
   `naadf_first_hit.wgsl:243-245` previously assigned `is_diffuse=0`
   only for `SURFACE_SPECULAR_ROUGH` and the C# palette used that for
   anything with `roughness < 0.5` in practice. Pivot preserves that
   semantic.

8. **Sample the Emissive array in the fast-path (vs emit `color_layered`
   only).** Chose **sample**, because the pavement material's emissive
   pattern is part of the visual identity of that material — emitting a
   flat colour wastes the authored emissive texture. The cost (3
   texSamples on Emissive voxels only — never paid by PBR voxels) is
   negligible. Rejected emit-only because the simpler code loses the
   pavement emissive pattern; documented the fallback in § H in case the
   shared-pattern visual oddity surfaces.

9. **Delete the old ORMH `.texarray.ron` outright (vs deprecate).** Chose
   **delete**. The file has zero code references (grep verified); keeping
   it as "deprecated" invites future contributors to accidentally re-wire
   it. Git history preserves the prior content for archaeology.

10. **Extend the existing `world_layout` `@group(0)` (vs create a new
    `@group(4)` for PBR bindings).** Chose **extend `world_layout`**.
    Rationale: every render pass that runs `decompress_voxel_type` and
    needs to shade a hit needs ALL of the PBR bindings; bundling them
    into `@group(0)` (the world-data group, bound everywhere) is the
    natural fit. The bindings cost (5 extra entries) is well within
    wgpu's `maxBindingsPerBindGroup` default (1000). Rejected a fresh
    `@group(4)` because the placeholder-empty-group pattern (see
    `pipelines.rs:478` and the existing `empty_layout`) would propagate
    across every existing render pass — needless plumbing.

11. **WGSL POM in the new `pbr_sampling.wgsl` module (vs inlined per
    entry shader).** Chose **module**. The triplanar+POM helpers are
    called from `naadf_first_hit.wgsl`, `naadf_global_illum.wgsl`,
    `spatial_resampling.wgsl` — three call sites; inlining triples the
    drift surface. naga-oil's import mechanism is the established pattern
    (`ray_tracing_common.wgsl`, `render_pipeline_common.wgsl`).

12. **No `ImageSampler::Linear` on the Normal array (vs nearest).** Chose
    **linear** because the bake function already sets linear addressing
    + repeat on every baked array (`loader.rs:197`), and the alternative
    requires a second sampler binding (point-filter, repeat). The
    visual cost (normal-map interpolation across texel boundaries
    smooths a 1K normal map's 1-pixel features) is acceptable for a v1.

13. **Hard-select variants only for v1 (vs ship cross-fade).** Chose
    hard-select because cross-fade doubles every triplanar texSample
    (3 × 4 = 12 extra) for a marginal seam reduction. The hash seams
    on voxel-cell boundaries are not load-bearing for the user's "see
    the metals" demonstration. Cross-fade lands as a follow-up if the
    seams prove objectionable.

14. **PBR voxels with `roughness < 0.05` re-enter the existing 4-iteration
    mirror loop (vs always defer to GI's rough-specular).** Chose to
    repurpose the existing mirror loop. The shader infrastructure already
    works perfectly for perfectly-smooth metallics; re-routing every PBR
    hit to GI would double the work for the most common "shiny metal"
    case. The condition that controls the loop changes from "type ==
    MIRROR" to "sampled_roughness < ε" — a one-token edit.

---

## Assumptions made

1. **Implementer can write `.png.meta` sidecar files.** The bake pipeline
   requires every source PNG to have a sibling `Load`-action `.meta` file.
   The setup-extraction agent (per `03-impl.md`) wrote the 1×1 placeholders
   but I am NOT confident it wrote `.meta` sidecars for the new
   per-material PNGs (the `find` output in `03-impl.md` didn't include
   any `*.meta` files for the new materials). The implementer must write
   ~70 `.meta` files (mechanical: copy `assets/materials/fabric/base_color.png.meta`
   for sRGB sources, copy a linear variant for the rest). I assume this is
   in scope for the implementer; flag it explicitly in the impl log so it
   isn't missed before the first `just bake-texarrays` run.

2. **All source PNGs in the seven new material directories are 1024×1024.**
   The `bake_texture_array` function (`loader.rs:138-162`) errors out on
   size mismatch. The setup-extraction agent reported "1k" zip variants,
   which usually means 1024 px square. If any of the 7 new materials
   shipped a non-1024 PNG (e.g. some packs ship grayscale single-channel
   images as 8-bit smaller PNGs), the bake fails fast with a clear error.
   The implementer's first `just bake-texarrays` run will surface this;
   not load-bearing for the design.

3. **The existing 3 `material.ron` PNGs are 1024×1024.** The
   `assets/materials/{fabric,gravelrock,pavement}/*.png` were authored
   by InstaMAT (per `00-reuse-audit.md` § 2 + the existing
   `material.ron` metadata). I haven't verified their dimensions match
   the AmbientCG 1K downloads. If they DON'T (e.g. fabric is 2048×2048),
   either (a) downsize the InstaMAT PNGs to 1024 in this PR, or (b)
   upsize the AmbientCG PNGs to 2048 (not recommended — bigger basis
   files). The implementer must verify with `file
   assets/materials/{fabric,gravelrock,pavement}/*.png` before running
   the bake; if mismatch, downsizing is the lighter fix.

4. **The "GL convention" normals in the AmbientCG `*_normal_gl_1k.png`
   files have green channel = +Y (up).** The setup-extraction agent
   reported the GL variants are uniformly labelled `_normal_gl_1k.png`
   across all 7 materials, which is the universal AmbientCG convention
   for OpenGL/Bevy-compatible normal maps. The existing
   `assets/materials/*/normal.png` (fabric/gravelrock/pavement) were
   InstaMAT-baked — `00-reuse-audit.md` § 7 implies they're already GL
   convention but doesn't prove it. If they're DX convention (green
   inverted), the normal-blend would point the wrong way and the
   highlight check in the e2e gate would fail. Implementer can verify
   by viewing one in any image viewer — DX normals have a "concave"
   appearance when read as RGB.

5. **The `cur_pos_int + cur_pos_frac` world-position is the input
   `world_pos` for triplanar sampling.** The existing shaders compute it
   piecewise: in `naadf_first_hit.wgsl:204-207` after a hit, `cur_pos_int`
   + `cur_pos_frac` IS the world position of the hit (camera-int-
   relative — but the triplanar functions only need relative position,
   since the sample is mod-1 over the texture tile). The triplanar
   functions take `vec3<f32>(cur_pos_int) + cur_pos_frac` directly. This
   is the convention the rest of the shaders use.

6. **`RenderAssets<GpuImage>` is queryable from `prepare_world_gpu`.**
   The standard Bevy pattern; if it isn't, the implementer plumbs it
   through and the pre-pivot `prepare_world_gpu` becomes a "wait for
   textures, then build" two-phase system. The system already runs in
   `RenderSystems::PrepareResources` after the GPU images are uploaded
   (Bevy's `prepare_assets::<GpuImage>` runs in the same set).

7. **`bytemuck::Pod` derives still work after the `VoxelType` reshape.**
   The new `VoxelType` carries `material_base: MaterialBase` (1-bit
   `#[repr(u8)]`) + `material_layer_index: u16` + `albedo_tint: [u8; 3]`
   + `color_layered: Vec3`. None of these are `Pod`-derivable as a
   struct (the enum, the 24-bit `[u8; 3]` will require padding). **But
   `VoxelType` does not need `Pod`** — it's CPU-only; `GpuVoxelType`
   (which IS `Pod`) is the GPU upload form, packed by
   `from_voxel_type`. Verified: current `VoxelType` is not `Pod` either
   (carries `MaterialBase`, `MaterialLayer` enums).

8. **The Phase-A wave's `editor/hud.rs:558-561` swatch colour migration
   is acceptable.** The HUD swatch currently shows the per-VoxelType
   albedo (`color_base`). After the pivot the "user-picked colour" is
   `albedo_tint`; the swatch shows that. The implication is that two
   VoxelTypes referencing the same texture layer with the same tint
   would have identical swatches — which is correct (they ARE visually
   identical). If the user wants the swatch to show a texture preview,
   that's a follow-up; the simple tint-as-swatch is the right v1.

9. **The `--pbr-visual` gate pixel coordinates are pinned after one
   manual run.** I cannot pre-compute the exact pixel coordinates of the
   highlight/textured surface/F0 region without running the gate and
   inspecting the PNG. The implementer runs the gate once, opens the
   resulting screenshot, picks three 40×40 rects matching the
   description, hardcodes them as consts in `e2e/pbr_visual.rs`.
   Documented this pinning step in § I.

10. **Energy conservation can be verified by inspection (Success
    criterion 8).** The `eval_pbr` body shows `kS = F; kD = (1 - F) *
    (1 - metallic); diffuse = albedo / PI; specular = D*G*F /
    (4*n·l*n·v)` — these are the energy-conserving Cook-Torrance terms.
    The implementer's `04-review.md` documents this by quoting the
    `eval_pbr` body and labelling the terms; no quantitative whitefurnace
    test required.

11. **The setup-extraction agent's existing material PNG paths in
    `assets/materials/<name>/` are stable** (i.e. nobody renamed the
    extracted dirs between when the brief was written and when the
    implementer reads `02-design.md`). Verified at design time via
    `find assets/materials -maxdepth 2 -type f` (output captured in
    the design's bash session); but stability across the
    implementer's later session depends on no other agent touching
    those paths.

---

## Open questions for the user

None. All 7 questions from `01-context.md` are closed in this design.

The orchestrator can proceed directly to dispatching the implementation
agent against this file + `01-context.md`.
