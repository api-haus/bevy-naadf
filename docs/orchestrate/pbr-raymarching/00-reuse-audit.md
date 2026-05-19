# Reuse audit — PBR raymarching

## Summary

- **The VNDF-GGX BRDF is already fully ported.** `ray_tracing_common.wgsl` contains
  `sample_vndf_isotropic`, `pdf_vndf_isotropic`, and `geometry_term` (Smith); the GI
  pass already calls them for rough-specular bounces. Schlick Fresnel lives in
  `render_pipeline_common.wgsl::get_reflectance_fresnel`. Zero new BRDF code is required
  for glossy/metallic — the shaders already implement it.
- **The secondary-ray loop exists.** `naadf_global_illum.wgsl` already fires a ≤3-bounce
  loop via `shoot_ray`, including a dedicated sun-shadow ray per bounce. `naadf_first_hit.wgsl`
  adds a 4-iteration specular-mirror-bounce loop. Reflection rays are a first-class existing
  pattern, not a new requirement.
- **Roughness is already in the per-voxel type.** `GpuVoxelType` packs `roughness` as an
  `f16` in `data[0]` (high halfword); `VoxelType::roughness: f32` carries it CPU-side.
  The existing `SURFACE_SPECULAR_ROUGH` material class (value `2`) in the palette exercises
  it. Adding a `metallic` channel requires only extending the 128-bit voxel-type entry and
  the CPU `VoxelType` struct — 16 bits are available without widening the buffer.
- **The texture-array builder exists and is wired.** `TextureArrayPlugin` +
  `TextureArrayLoader` bakes `*.texarray.ron` definitions into `TextureViewDimension::D2Array`
  images ready for `texture_2d_array` WGSL binding. No shader currently binds a texarray
  in the raymarching pipeline — that binding and the triplanar sampling logic must be added.
- **Triplanar helpers and any texture-array bind point in the raymarcher are absent.**
  The shaders do not contain any `textureSample`, `texture_2d_array`, UV-from-world or
  triplanar blend logic. This is the largest missing piece for goal 2.

---

## Findings table

| Need | Existing candidate | Path:line | Coverage | Reuse / extend / new |
|---|---|---|---|---|
| BRDF — VNDF-GGX sampling | `sample_vndf_isotropic`, `geometry_term`, `get_reflectance_fresnel` | `ray_tracing_common.wgsl:120–183`, `render_pipeline_common.wgsl:254–257` | **Full** — already called by GI pass for rough specular | **Reuse** |
| Secondary / reflection rays | `shoot_ray` called in 4-bounce loop (`naadf_first_hit`) and ≤3-bounce GI loop (`naadf_global_illum`) | `naadf_first_hit.wgsl:174–257`, `naadf_global_illum.wgsl:283–442` | **Full** — multi-bounce + shadow ray pattern already working | **Reuse** (extend hit-shading branch) |
| Per-voxel roughness | `VoxelType::roughness` + `GpuVoxelType::data[0]` high halfword | `voxel/mod.rs:119`, `render/gpu_types.rs:283–295`, `render_pipeline_common.wgsl:99,115` | **Full** — already uploaded, decoded, sampled in shader | **Reuse** |
| Per-voxel metallic channel | Not present in `VoxelType` or `GpuVoxelType` | `voxel/mod.rs:113–138`, `render/gpu_types.rs:273–295` | **None** — 16 unused bits in `data[0]` (low halfword after `material_base+layer`=4 bits) or repurpose `color_layered` | **Extend** (add `metallic: f32` to `VoxelType`, pack into existing `u32` free bits) |
| Texture-array builder | `TextureArrayLoader` + `bake_texture_array` | `texture_array/loader.rs:134–202`, `texture_array/mod.rs:1–133` | **Full** — produces a `D2Array` `Image` with `Repeat` sampler, ready for shader binding | **Reuse** (pipeline bind point is new) |
| Texture-array shader binding | Absent — no `texture_2d_array` declared in any raymarching shader | all `*.wgsl` | **None** | **New** — add `@group(N) @binding(M) var mat_tex: texture_2d_array<f32>` + sampler |
| Triplanar sampling helpers | Absent — no `textureSample`, UVs from world-pos, or normal-blend weighting in any shader | all `*.wgsl` | **None** | **New** — write triplanar blend in WGSL; feed world-pos from voxel integer cell |

---

## Per-area detail

### 1. Raymarcher shader entry + hit shading

Entry point: `naadf_first_hit.wgsl::calc_first_hit` (`@compute @workgroup_size(64,1,1)`,
file line 98). Internally calls `shoot_ray` (imported from `ray_tracing.wgsl`) which is
the Amanatides-Woo DDA over the AADF hierarchy.

On a **hit**, `calc_first_hit` (lines 227–247):
1. Calls `decompress_voxel_type(voxel_types[ray_result.hit_type])` to get a `VoxelType`
   with `material_base`, `color_base` (albedo/IOR), `color_layer` (emissive intensity),
   `roughness`.
2. Branches on `material_base`:
   - `SURFACE_SPECULAR_MIRROR` (3): Schlick-Fresnel weight `acc.absorption *= get_reflectance_fresnel(ior, cos_theta)`, then `ray_dir = reflect(ray_dir, normal)`, loops back (up to 4 bounces).
   - `SURFACE_SPECULAR_ROUGH` (2): terminates the primary loop, marks `is_diffuse = 0`.
   - `SURFACE_EMISSIVE` (1): `acc.light += acc.absorption * color_layer.r`.
   - `SURFACE_DIFFUSE` (0): `acc.absorption *= color_base`.
3. Writes the G-buffer (`first_hit_data`, `first_hit_absorption`, `final_color`).

The hit shading in the **first-hit** pass is therefore **Lambertian + emissive + mirror**.
Rough-specular (`SURFACE_SPECULAR_ROUGH`) is recognized but **not shaded** in the first-hit
pass — it only sets `is_diffuse = 0` and defers to the GI pass which calls `sample_vndf_isotropic`.

The **GI pass** (`naadf_global_illum.wgsl::calc_global_ilum`, line 174) applies the full
rough-specular BRDF at lines 254–276 (VNDF-GGX sample, geometry term, Fresnel), fires sun-shadow
rays at lines 375–385, and does 3-bounce rough-specular propagation at lines 401–424.

Normal reconstruction: `shoot_ray` sets `(*ray_result).normal = mask * -sign(ray_dir)` (line
565) — a voxel face normal, axis-aligned only. No smooth normal or normal-map support exists.

### 2. Reflection / secondary-ray scaffolding

`shoot_ray` (`ray_tracing.wgsl`) is a **pure, re-entrant function** with signature:
```wgsl
fn shoot_ray(
    ray_origin_int: vec3<i32>,
    ray_origin_frac: vec3<f32>,
    ray_dir: vec3<f32>,
    max_step_count: i32,
    ray_result: ptr<function, RayResult>,
) -> bool
```
It takes no implicit shader state — every call is independent. Both existing callers use it
in a loop:
- `naadf_first_hit.wgsl`: 4-iteration `loop { shoot_ray(...); reflect(ray_dir); }` (lines 174–264).
- `naadf_global_illum.wgsl`: ≤3-bounce loop (lines 283–442), plus a sun-shadow `shoot_ray` call inside (lines 377–383) — this is a shadow ray, a second independent traversal per bounce.

There is **no dedicated reflection pass** — all secondary rays fire within the same compute
invocation as the primary ray. This is architecturally compatible with adding glossy reflections:
the existing `SURFACE_SPECULAR_ROUGH` branch in the GI loop already does VNDF sampling +
`reflect` + re-enters the bounce loop (lines 401–414).

### 3. Per-voxel material representation

Each voxel stores a **15-bit type id** (`hit_type` in `RayResult`). The id is an index into
`voxel_types: array<vec4<u32>>` at `@group(0) @binding(3)` (`world_data.wgsl` line 73).

Each entry is 128 bits (`vec4<u32>`), packed by `GpuVoxelType::from_voxel_type` (`gpu_types.rs:280–295`):

| `data[0]` bits | Field |
|---|---|
| `[1:0]` | `material_base` (2 bits: Diffuse/Emissive/MetallicRough/MetallicMirror) |
| `[3:2]` | `material_layer` (2 bits) |
| `[15:4]` | unused (12 bits) |
| `[31:16]` | `roughness` as f16 |
| `data[1]` | `color_base.x` (f16 low) \| `color_base.y` (f16 high) |
| `data[2]` | `color_base.z` (f16 low) \| `color_layered.x` (f16 high) |
| `data[3]` | `color_layered.y` (f16 low) \| `color_layered.z` (f16 high) |

**Available capacity for metallic:** bits `[15:4]` of `data[0]` are unused — 12 bits.
An f16 metallic value fits in 16 bits; it can be packed into `data[0]` by using bits `[15:4]`
for 12-bit fixed-point metallic (0.0–1.0 range, 12-bit precision) or expanding `data[0]` to
carry the f16 properly if bits `[15:4]` + a rearrangement is acceptable.

The `voxel_types` buffer is CPU-built in `prepare.rs` (line 380–386) via
`GpuVoxelType::from_voxel_type` and uploaded once per frame if dirty. Extending it requires:
- Adding `metallic: f32` to `crates/bevy_naadf/src/voxel/mod.rs::VoxelType` (line 113).
- Updating `GpuVoxelType::from_voxel_type` to pack it into the free bits.
- Updating `render_pipeline_common.wgsl::decompress_voxel_type` to unpack it.

### 4. Texture array builder

Full builder: `crates/bevy_naadf/src/texture_array/` (three files: `def.rs`, `loader.rs`,
`mod.rs`). The `bake_texture_array` function (`loader.rs:134`) takes a `TextureArrayDef` and
decoded source images, outputs a Bevy `Image` with:
- `TextureDimension::D2`, `depth_or_array_layers = elements.len()`.
- `TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), .. }` (line 192) — correctly typed for `texture_2d_array` WGSL binding.
- `ImageSampler::Descriptor(ImageSamplerDescriptor { address_mode_u/v: Repeat, ..linear() })` (line 197) — repeat + linear for terrain atlasing.
- Format: `Rgba8UnormSrgb` (colour) or `Rgba8Unorm` (data/roughness) selectable per `.texarray.ron`.

The `TextureArrayPlugin` registers the loader (`mod.rs:105–133`). The asset is loadable as:
```rust
asset_server.load::<Image>("textures/terrain.texarray.ron")
```
and bindable in WGSL as `texture_2d_array<f32>`.

**The resulting array is not bound anywhere in the current raymarching pipeline.** The render
pipeline layouts (`render/pipelines.rs`) have no `texture_2d_array` entry and the world
bind-group layout (`@group(0)`) has no sampler or texture binding beyond the storage buffers.

### 5. Triplanar helpers

**Entirely absent.** A search for `textureSample`, `triplanar`, `texture_2d_array`, UV-from-world
across all 25 WGSL files returns zero matches. The hit-shading branches use only the scalar
`VoxelType.color_base` — a flat RGB. World-space UVs, blend weights from normals, and normal
map fetch+blend are all new.

For the implement path: the hit point world position is available as
`voxel_pos: vec3<i32>` in `RayResult` (ray_tracing.wgsl line 146), plus the `cur_dist` offset.
The face normal is an axis-aligned unit vector (`{±1,0,0}`, `{0,±1,0}`, `{0,0,±1}`).
Standard triplanar UVs from the voxel integer position plus fractional offset within the voxel
are computable without additional shader state.

### 6. PBR / BRDF code (if any)

Substantial BRDF code is **already present**:

`ray_tracing_common.wgsl`:
- `sample_vndf_isotropic` (lines 122–156): GGX-Smith VNDF importance sampling, isotropic.
- `pdf_vndf_isotropic` (lines 159–175): VNDF pdf.
- `geometry_term` (lines 178–183): Smith geometry term.
- `get_uniform_hemisphere_sample` (lines 105–118): cosine-weighted hemisphere sample.
- `get_perpendicular_vector` (lines 95–101): branch-free TBN helper.

`render_pipeline_common.wgsl`:
- `get_reflectance_fresnel` (lines 254–257): Schlick Fresnel from IOR triple.

`naadf_global_illum.wgsl` (lines 356–368): explicit GGX-NDF `D` term + `geometry_term` sun-sample.

`spatial_resampling.wgsl` (line 129): `get_brdf` function wrapping `geometry_term` for ReSTIR
weight normalization.

**Missing from a full metallic-PBR model:**
- No `metallic` channel in the BRDF — `color_base` is used as IOR, not split into
  diffuse-albedo vs. F0 by metallic factor.
- No energy conservation between diffuse and specular lobes (no `1 - F` diffuse weight).
- No anisotropic GGX (not needed if only isotropic roughness is targeted).
- No normal map decode (not applicable without triplanar).

### 7. Material RON asset format

Two separate material formats exist:

**A. `material.ron` / `MaterialRon`** (`baked_material.rs:29–56`): a
`StandardMaterial`-targeting format with fields:
`name`, `base_color`, `normal`, `metallic_roughness`, `occlusion`, `emissive`, `height`,
`perceptual_roughness: f32`, `metallic: f32`, `emissive_is_textured: bool`.
This is for Bevy's standard mesh renderer, not for the NAADF raymarcher — it yields a
`StandardMaterial` asset, not a `VoxelType`.

**B. `VoxelType` (inline Rust struct)** (`voxel/mod.rs:113–138`): the raymarcher's per-type
material palette entry — `material_base`, `material_layer`, `roughness`, `color_base`,
`color_layered`. No file format (it is built programmatically in `voxel/grid.rs` and
`vox_import.rs`). The only GPU upload format is the 128-bit `GpuVoxelType`.

For PBR, `VoxelType` needs:
- `metallic: f32` — to split diffuse vs. specular (F0 = `mix(0.04, albedo, metallic)`).
- Optionally a per-type texture layer index (`u16`) if triplanar texturing is per-voxel-type
  (the texture array layer for that material).

The `MaterialRon` format already has `perceptual_roughness` and `metallic` scalars plus full
PBR texture slots, but it does not connect to the NAADF voxel pipeline. It could serve as an
authoring format for injecting values into a `VoxelType` palette entry if the loader is extended.

### 8. Relevant e2e gates

Framebuffer-capturing gates that exercise the shading path:

| Gate flag | File | What it captures | Relevant to PBR |
|---|---|---|---|
| (default, no flag) | `e2e/gates.rs` Batch 6 | 256×256 framebuffer at fixed pose; asserts emissive luminance > 120, GI-lit diffuse > 150, sky in [10,230] | Yes — the `solid_block_rect` region is the diffuse GI-lit check; a PBR change altering diffuse/specular balance would move it |
| `--oasis-edit-visual` | `e2e/oasis_edit_visual.rs` | Full-frame A/B diff across a brush stroke on Oasis VOX scene | Indirect — catches regressions in the voxel shading path |
| `--small-edit-visual` | `e2e/small_edit_visual.rs` | Click-rect before/after a cube_brush application | Indirect |
| `--small-edit-repro` | `e2e/small_edit_repro.rs` | Oasis pose post-edit — no pitch-black assertion | Indirect |
| `--vox-gpu-oracle` | `e2e/vox_gpu_oracle.rs` | CPU-vs-GPU per-pixel comparison on Oasis | Indirect |
| `--entities` | `e2e/gates.rs` entity gate | entity_pixel_rect luminance > 80 | Indirect — entity emissive contribution check |

No gate currently isolates a **specular highlight**, a **reflection**, or a **textured-surface
luminance**. A new gate targeting a known rough-specular or metallic voxel at a specific pose
would be the appropriate test deliverable for goals 1 and 2.

---

## Borderline calls

**`material.ron` / `MaterialRon` as a VoxelType authoring format (area 7).**
Currently it produces a `StandardMaterial`, entirely separate from the NAADF pipeline.
If the goal is to let artists author per-voxel-type roughness/metallic/texture-layer via a
file format, `MaterialRon` could be extended to also produce a `VoxelType` palette entry
(the fields are a superset of what `VoxelType` needs). The current verdict is "not applicable"
because the pipeline connection doesn't exist, but it flips to "extend" the moment someone
decides to wire a file-driven voxel-palette authoring tool. The flip condition: the architect
chooses to make `asset_server.load::<VoxelType>("...")` the palette registration path rather
than building the palette programmatically.

**Per-voxel `metallic` packing into existing `GpuVoxelType` bits (area 3).**
The verdict is "extend" but the exact encoding is borderline: 12 free bits in `data[0]`
(`[15:4]`) allow 12-bit fixed-point metallic, which is adequate precision for a physically-based
parameter (4096 steps). However, if the team prefers f16, the 128-bit budget needs rearrangement
(e.g. pack `material_base` + `material_layer` into the low nibble of `data[1]`, freeing a full
halfword in `data[0]`). Either is an "extend" of the existing struct — this is borderline
only on encoding choice, not on whether reuse or new is appropriate.

---

## Viability snapshot

**Goal 1 (glossy + metallic surfaces):** Highly viable with minimal new code. The GI pass
already applies VNDF-GGX sampling and the Smith geometry term for `SURFACE_SPECULAR_ROUGH`
surfaces; the Schlick Fresnel is already vectorized over `color_base` as an IOR triple. Adding
metallic requires: (a) one new `f32 metallic` field in `VoxelType`, packed into the 12 free
bits of `GpuVoxelType::data[0]`; (b) decoding it in `decompress_voxel_type`; (c) splitting
the BRDF evaluation to use `mix(vec3(0.04), color_base, metallic)` as F0 and
`(1.0 - metallic) * color_base` as diffuse albedo — a ~5-line WGSL edit in the three hit-shading
branches (`naadf_first_hit.wgsl`, `naadf_global_illum.wgsl`, `spatial_resampling.wgsl`). There
is no structural hostility to this: the integrator loop is a clean, callable `shoot_ray` + a
separately structured shading branch, not a monolithic inlined mess.

**Goal 2 (triplanar PBR texture-array shading):** Viable but structurally new. The texture
array builder already produces a correctly-typed `D2Array` image. What is new: a bind group
layout entry, a sampler binding, the WGSL `texture_2d_array`/`sampler` declarations, and the
triplanar UV + blend-weight logic (~30–50 new WGSL lines). The hit-shading branches must also be
extended to replace the flat `color_base` lookup with a triplanar sample, which requires knowing
the hit point fractional world position (already in `voxel_pos` + `cur_dist`) and the face
normal (already in `ray_result.normal`). The biggest coordination cost is plumbing the new bind
group through `render/pipelines.rs` and `render/prepare.rs` — non-trivial but a clear,
contained addition to the existing pattern. A per-voxel-type texture-array layer index also needs
a slot in `GpuVoxelType` (the free bits in `data[0]` are sufficient for a 12-bit layer index,
covering 4096 distinct material layers).

---

## Follow-up audit: declarative baker pipeline (2026-05-18)

### 1. Offline baker entry point

**Baker binary:** `crates/bevy_naadf/src/bin/bake.rs`, `fn main()` at line 25.

Invoked via `just bake-texarrays` (justfile line 37):
```
cargo run -p bevy-naadf --bin bake --no-default-features --release
```

The justfile comment at line 34–35 explicitly describes the binary:
> "Bake `*.texarray.ron` definitions → Basis `.basis` arrays under `imported_assets/` (headless AssetProcessor; no GPU/DLSS needed)."

The binary builds a headless Bevy app with `AssetMode::Processed`, `TextureArrayPlugin`, and an
`exit_when_finished` system that polls `ProcessorState::Finished` then exits clean. It is deliberately
minimal: task pools, the asset pipeline, `ImagePlugin`, and `TextureArrayPlugin` — no renderer, no
window, no DLSS. The asset source root is `"src/assets"` (hardwired at `bake.rs:77`), output is
`imported_assets/Default/` (Bevy's standard processed path).

No other baker binaries exist. `find . -name '*bake*' -o -name '*baker*'` in `bins/` and `crates/*/src/bin/`
returns only `bake.rs` (plus `e2e_render.rs`, which is the e2e harness, not a baker).

### 2. Legacy bevy-instamat trace + current state of baked_material.rs

**Git ancestry (relevant commits, `--all --follow`):**

```
43dbd9f  refactor(naadf): inline `material.ron` loader into bevy_naadf, drop bevy-instamat crate
44b7412  chore(repo): strip InstaMAT FFI + baker + .imp sources; keep material.ron + PNG loader
480afe6  feat: bake height as Luma16 + wire StandardMaterial::depth_map
c73c461  feat: land bevy-instamat crate + gravelRock baked material
a572bb7  feat: split into crates/ workspace, add batch baker, update orchestration
8099b8c  checkpoint: instamat Bevy integration — bake pipeline refactor
6406aa0  checkpoint: scaffold src/instamat/ module tree
```

`44b7412` stripped the InstaMAT FFI bindings (`crates/bevy-instamat/src/instamat/`), the FFI-backed
`instamat_bake` binary (`crates/bevy-instamat/src/bin/instamat_bake.rs`), and the `.imp` source
packages — keeping only the `material.ron` + sibling-PNG loader (`BakedMaterialPlugin` /
`MaterialRonLoader`) and the already-baked PBR PNGs under `assets/materials/<name>/`.

`43dbd9f` then inlined that 236-line loader from `crates/bevy-instamat/src/baked_material.rs` into
`crates/bevy_naadf/src/baked_material.rs` verbatim (a `git mv` + crate-ref update).

**Current state of `baked_material.rs`** (`crates/bevy_naadf/src/baked_material.rs:1–226`):

- `MaterialRon` struct (lines 29–56): RON-deserializable schema with fields
  `name`, `base_color`, `normal`, `metallic_roughness`, `occlusion`, `emissive`, `height`,
  `perceptual_roughness: f32`, `metallic: f32`, `emissive_is_textured: bool`.
- `MaterialRonLoader` (lines 101–206): an `AssetLoader<Asset = StandardMaterial>` that
  reads a `material.ron`, loads each referenced sibling PNG (with explicit sRGB/linear
  `ImageLoaderSettings`), and assembles a `StandardMaterial` with `base_color_texture`,
  `normal_map_texture`, `metallic_roughness_texture`, `occlusion_texture`, `emissive_texture`,
  `depth_map`, scalar `perceptual_roughness`, `metallic`.
- `BakedMaterialPlugin` (lines 219–225): registers the loader. No scene spawning.

**What it does NOT do:** it does not produce an `Image`, a `.texarray.ron`, or any packed texture
output. It is a pure `material.ron` → `StandardMaterial` runtime asset loader, consuming Bevy's
standard mesh renderer pipeline, not the NAADF raymarcher.

No sibling files named `baked_*.rs`, `material_*.rs`, or `instamat_*.rs` exist in
`crates/bevy_naadf/src/` beyond `baked_material.rs` itself.

### 3. MaterialRon → on-disk output flow

The **runtime** path (`MaterialRonLoader`):

1. `material.ron` is RON-parsed into `MaterialRon`.
2. Each named PNG sibling is loaded via Bevy `AssetLoader` dependency (`load_context.load_builder()`)
   with explicit `is_srgb` — sRGB for `base_color` / `emissive`, linear for all others.
3. Returns a `StandardMaterial` (in-memory Bevy asset). **No file is written.**

The **baker** (`bake.rs` binary + `TextureArrayPlugin`):

The baker does **not** touch `material.ron` or `MaterialRon` at all. Its `TextureArrayPlugin`
registers `TextureArrayLoader` for the `*.texarray.ron` extension only. The `AssetProcessor`
scans `src/assets/`, finds `.texarray.ron` files, runs `TextureArrayLoader` (reads the RON def +
resolves source PNGs from `ChannelSource.input` paths), then `TextureArrayBasisSaver`
Basis-compresses the resulting `Image` into `imported_assets/Default/*.texarray.ron.basis`.

**Output locations produced by the baker:**

- `imported_assets/Default/materials/diffuse.texarray.ron.basis` — sRGB Basis array, 3 layers.
- `imported_assets/Default/materials/normal.texarray.ron.basis` — linear Basis array, 3 layers.
- `imported_assets/Default/materials/occlusion_roughness_metallic_height.texarray.ron.basis` — linear Basis array, 3 layers.

**Channel packing performed by the baker:**

The `diffuse.texarray.ron` passes `base_color.png` RGBA through unchanged (R←R, G←G, B←B, A←A).
It does **not** pack AO into the diffuse alpha — the diffuse alpha is the source PNG's own alpha.

The `occlusion_roughness_metallic_height.texarray.ron` packs:
- R ← `occlusion.png`.R
- G ← `metallic_roughness.png`.G (roughness — glTF convention)
- B ← `metallic_roughness.png`.B (metallic — glTF convention)
- A ← `height.png`.R

This is ORMH (Occlusion/Roughness/Metallic/Height) packed into one RGBA linear layer. The AO
channel is already packed here (as R), not into the diffuse alpha.

The baker resolves the referenced PNG files from the `.texarray.ron` definitions directly — it does
**not** read `material.ron` at all. The connection `material.ron` → `.texarray.ron` definitions is
**manual** (the artist writes both).

### 4. Existing .material.ron / .texarray.ron definitions

**`.material.ron` files** (3 examples, one per material):

- `assets/materials/fabric/material.ron` — `base_color`, `normal`, `metallic_roughness`,
  `occlusion`, `emissive: None`, `height`; `perceptual_roughness: 0.5`, `metallic: 0.0`.
- `assets/materials/gravelrock/material.ron` — same fields, `emissive: None`.
- `assets/materials/pavement/material.ron` — `emissive: Some("emissive.png")`,
  `emissive_is_textured: true`.

**`.texarray.ron` files** (3 arrays, all in `assets/materials/`):

| File | Format | Layers | Channel packing |
|---|---|---|---|
| `diffuse.texarray.ron` | `Rgba8UnormSrgb` | 3 (fabric/gravelrock/pavement) | Pass-through RGBA from `base_color.png` |
| `normal.texarray.ron` | `Rgba8Unorm` | 3 | Pass-through RGBA from `normal.png` |
| `occlusion_roughness_metallic_height.texarray.ron` | `Rgba8Unorm` | 3 | R=AO, G=roughness, B=metallic, A=height |

Layer 0 = fabric, layer 1 = gravelrock, layer 2 = pavement across all three — layer index is
consistent (the comment in `diffuse.texarray.ron:7` explicitly guarantees this).

No `emissive.texarray.ron` file exists — emissive is not yet packed into a fourth array. One
material (pavement) has an `emissive.png` on disk but it appears only in its `material.ron`.

**Sample `.texarray.ron` definition** (from `crates/bevy_naadf/src/assets/textures/sample.texarray.ron`):
this is a developer example/test fixture, not a production material.

### 5. Gap analysis vs target 4-linked-array set

The target layout is four linked arrays per material:

| Target array | Content | Format |
|---|---|---|
| Array 1: Diffuse+AO | RGB=albedo, A=AO | sRGB (Diffuse), linear (AO) — mixed |
| Array 2: Normal | RGB=tangent normal | linear |
| Array 3: MRH | R=metallic, G=roughness, B=height | linear |
| Array 4: Emissive | RGB=emissive | sRGB |

**Gap for each array:**

**Array 1 (Diffuse+AO):** The existing `diffuse.texarray.ron` passes `base_color.png` RGBA
through unchanged. AO is **not** packed into the alpha channel — it is packed separately in the
ORMH array's R channel. To match the target (AO in diffuse alpha), the `diffuse.texarray.ron`
must be reauthored to source its A channel from `occlusion.png.R`. This is a `.ron` file edit
only — no code change. The baker already supports arbitrary channel routing.

**Array 2 (Normal):** The existing `normal.texarray.ron` passes `normal.png` RGBA through.
The target is RGB normal, which matches. **Covered — reuse with no change** (or optionally
ignore the alpha channel in the shader; it carries nothing meaningful from the current source).

**Array 3 (MRH):** The existing `occlusion_roughness_metallic_height.texarray.ron` packs
R=AO, G=roughness, B=metallic, A=height. The target MRH is R=metallic, G=roughness, B=height.
Two gaps:
1. Channel order differs (M and H need swapping vs. current B and A).
2. AO is in R of the existing ORMH array but the target puts it in diffuse alpha — so it
   should be removed from this array.
The fix is a `.ron` re-author: R←`metallic_roughness.G` (roughness), G←`metallic_roughness.B`
(metallic), B←`height.R`. Again, no code change — the baker supports any channel routing.

**Array 4 (Emissive):** **Does not exist.** No `emissive.texarray.ron` is authored. Only
`pavement` has an `emissive.png`; fabric and gravelrock have none. A new
`emissive.texarray.ron` must be created. For non-emissive materials, a black placeholder
layer (or a 1×1 black texture reused per layer) is needed. The baker supports this (the same
source texture path can be shared across multiple layer definitions). This is a `.ron`
authoring task, no code change.

**Summary of what needs code changes vs. what needs only `.ron` re-authoring:**

| Gap | Code change? | `.ron` re-author? |
|---|---|---|
| Diffuse+AO packing | No | Yes — repoint diffuse.A to occlusion.R |
| Normal array | No | No — already usable |
| MRH channel order | No | Yes — reorder channels in ORMH def |
| Emissive array | No | Yes — new file + black placeholder layers |
| 4-array "linked" concept in code | Yes — new struct | N/A |

### 6. Existing 'linked array / material set' concept (or absence thereof)

**No such concept exists in the codebase.** A search for `MaterialSet`, `MaterialBundle`,
`linked_array`, `material_set`, `texarray_set`, `PbrArraySet`, `TextureSet`, or `array_set`
across all `*.rs` files returns zero matches.

The three existing `.texarray.ron` files maintain consistent layer ordering by convention only
(the comment in `diffuse.texarray.ron:7–14` documents the layer↔material correspondence and
notes it must match across all three files). There is no Rust type that bundles the three
`Handle<Image>` assets into a single named unit, no asset format that declares "these four
arrays belong to one material set", and no runtime check that enforces layer-count parity
across the group.

Adding a "material set" type — a struct holding `Handle<Image> × 4` (diffuse_ao, normal,
mrh, emissive) keyed by a `material_layer_index: u16` — is entirely new code. It would be a
small addition (a resource or asset holding four `Handle<Image>` values) and could be
structured as a Bevy `Resource` or a new asset type with a matching `.matset.ron`.

### Verdict

An offline declarative baker exists and is fully operational: `src/bin/bake.rs` runs a headless
Bevy `AssetProcessor` that turns `*.texarray.ron` channel-routing definitions into
Basis-compressed `D2Array` images via `TextureArrayLoader` + `TextureArrayBasisSaver`. Three of
the four target arrays are already authored as `.texarray.ron` definitions with real material
content (fabric / gravelrock / pavement), and the channel-combine infrastructure in
`TextureArrayDef` / `bake_texture_array` already supports arbitrary per-channel routing with
inversion. The highest-leverage existing piece is the **ORMH `.texarray.ron` re-author** — the
existing `occlusion_roughness_metallic_height.texarray.ron` already packs the right source
textures (metallic, roughness, height) into one array; it only needs channel-order adjustment
and AO removal, both expressible as `.ron` edits with zero code changes. What the baker does
**not** provide is: (a) the fourth emissive array (needs new `.ron` authoring + placeholder
layers for non-emissive materials), (b) the AO-in-diffuse-alpha packing (one `.ron` line
change), (c) a "linked material set" Rust type bundling the four `Handle<Image>` assets into a
single named unit, and (d) any connection from `material.ron` / `MaterialRon` to the baker —
that bridge is entirely absent; the baker reads `.texarray.ron` only.
