# 01 — Context bundle (PBR raymarching)

> This file is the canonical context every non-review agent reads first.
> It is self-contained: every fact a downstream agent needs is inlined or
> pinned to an exact file path + line range. Do not assume agents can see
> conversation, attached images, or prior tool outputs.

## Restated goal (verbatim from user)

> implement texture-array based PBR material raymarching with
> parallax-occlusion-mapping, glossy/metallic surfaces with reflections into
> naadf voxel raymarching

And the two-bullet expansion:
> 1. extend voxel naadf raymarching to support glossy and metallic surfaces — how viable is it?
> 2. extend with triplanar pbr texture-array based shading (we already have a texture array builder)

## Pivot decisions locked in via Q&A

These supersede any earlier framing. The orchestrator dropped the original
"add a `metallic` scalar to `VoxelType`" plan after the user clarified the
architecture.

### D1. ALL surfaces are unified-PBR (no separate Diffuse/MetallicRough/MetallicMirror branches)

User verbatim:
> will we replace metallicRough/metallicMirror ? ALL of them are supposed to
> be PBR and metallic will be determined by the actual texture array sample

**Decision.** The `material_base` enum collapses to a 1-bit flag:
`{ PBR=0, Emissive=1 }`. Every PBR hit runs the same unified BRDF; metallic,
roughness, and height are all sampled from the texture array (NOT from
per-VoxelType scalars).

### D2. Texture-array layout — glTF-standard, 4 linked arrays per material

Q: "Texture-array layout for the linked PBR set?"
A (chosen): "glTF-standard: Diffuse(RGB)+AO(A), Normal(RGB), MRH(M/R/H),
Emissive(RGB)".

| Array | Slot | Channels | Format |
|---|---|---|---|
| 0 | Diffuse + AO | RGB=albedo, A=AO | `Rgba8UnormSrgb` |
| 1 | Normal | RGB=tangent-space normal (OpenGL convention), A unused | `Rgba8Unorm` |
| 2 | MRH | R=metallic, G=roughness, B=height, A unused | `Rgba8Unorm` |
| 3 | Emissive | RGB=emissive color, A unused | `Rgba8UnormSrgb` |

**All 4 arrays share the same layer-index space** — material N occupies layer
N in all four arrays. The same `material_layer_index: u16` selects across the
set. The audit confirmed `TextureArrayDef`'s `ChannelSource` already supports
arbitrary `{ input: PathBuf, channel: ChannelSelector }` per-channel routing,
including reading multiple source `.png` files per array slot — see
`00-reuse-audit.md` § "Follow-up audit § 3" and § 5 for the verbatim baker
behaviour and per-array gap analysis.

### D3. Emissive — two branches (PBR + Emissive fast-path)

Q: "How do we treat emissive after the PBR unification?"
A (chosen): "Two branches — PBR + Emissive fast-path".

**Decision.** Keep `material_base` as a 1-bit flag {PBR, Emissive}. Emissive
voxels skip the BRDF and just emit; the emissive output is the Emissive array
sample × per-VoxelType emissive multiplier (TBD by architect — see Q3 below).
PBR voxels do NOT sample the Emissive array — they pay zero cost for it.

### D4. Per-VoxelType palette: `material_layer_index` + `albedo_tint: Vec3`

Q: "What stays on the per-VoxelType (CPU palette) entry after the pivot?"
A (chosen): "`material_layer_index` + per-type `albedo_tint: Vec3`".

**Decision.** `VoxelType` becomes:
- `material_layer_index: u16` (the texture-array layer for this material set).
- `albedo_tint: Vec3` (sRGB multiplier on the sampled albedo — mirrors
  Bevy's `StandardMaterial.base_color × base_color_texture`).
- `material_base: 1 bit` ({PBR=0, Emissive=1}) — preserved as fast-path flag.
- `color_layered: Vec3` — **preserved** because the Emissive fast-path
  multiplies it with the sampled emissive (or uses it directly if no
  per-voxel-type emissive texture is desired). Architect decides exact semantics.

**Removed from `VoxelType`:**
- `roughness: f32` — now texture-driven.
- `color_base: Vec3` — replaced by `albedo_tint`.
- The 2-bit `material_layer` enum sub-field — collapse into the 1-bit emissive
  flag (architect may keep additional flag bits if useful).

### D5. POM (parallax-occlusion-mapping) — included in this PR

Q: "Parallax-occlusion-mapping was in your original sentence but dropped from
the /delegate bullets. How should we treat it?"
A (chosen): "Include POM in this PR".

**Decision.** POM is sampled from MRH.B (height) at the voxel face under the
triplanar projection. POM iterations displace the UVs before the
albedo/normal/MR samples. POM applies only in the PBR branch (Emissive does
not need it). Cost ballpark per PBR hit: 3 triplanar samples × 3 maps (Diffuse,
Normal, MRH) = 9 texSamples + POM iterations on height = ~6–10 height-only
samples = ~15–20 texSamples total per PBR hit.

### D6. Layer selection per voxel — per-VoxelType base + procedural blend

Q: "Given emissive is per-VoxelType (palette-level), how should texture-array
layers be selected per voxel?"
A (chosen): "Per-VoxelType base + procedural blend (world-pos hash) to break
repetition".

**Decision.** Each `VoxelType` stores a base `material_layer_index` plus a
small variant span (e.g. 2 variants ⇒ base+0 or base+1). A world-position
hash selects which variant a given voxel-face uses, breaking visible tiling
on large flat surfaces. Architect picks the variant count + the hash function
(e.g. PCG3D, xxhash-like int hash) and decides whether variant-span is per-
VoxelType or global. Cost: +1 texSample per channel per pixel for the second
variant + a hash() call.

Note: if variants are blended (cross-fade between variants based on hash
distance) the per-pixel cost doubles. Picking hard-select vs. blended is the
architect's call — prefer hard-select for the first cut, blended as a follow-
up if the seams are objectionable.

### D7. Execution mode — distributed

Q: "Distributed or consolidated execution mode?"
A (chosen): "Distributed (Recommended)".

**Decision.** Architect → user-approved design → implementer → fresh-eyes
reviewer with hard gates between phases. Shader/BRDF subtlety justifies the
fresh-eyes review pass.

## 7-material starter palette (CC0, AmbientCG/Poly-Haven 1K PNG)

The user supplied seven zip files under `/mnt/archive4/Downloads/`. The setup
sub-agent extracts each into `<worktree>/assets/materials/<material_name>/`
(following the existing fabric/gravelrock/pavement convention found by audit
§ 4).

| Slot | Material | Source zip | Color | Normal | Rough | Metal | AO | Height |
|---|---|---|:-:|:-:|:-:|:-:|:-:|:-:|
| 0 | metal_02 | `metal_02_1k.zip` | ✓ | ✓ gl | ✓ | ✓ | ✓ | ✓ |
| 1 | metal_pattern_01 | `metal_pattern_01_1k.zip` | ✓ | ✓ gl | ✓ | ✓ | ✓ | ✓ |
| 2 | bark_04 | `bark_04_1k.zip` | ✓ | ✓ gl | ✓ | ~0 | ✓ | ✓ |
| 3 | snow_01 | `snow_01_1k.zip` | ✓ | ✓ gl | ✓ | placeholder=0 | ✓ | ✓ |
| 4 | grass_05 | `grass_05_1k.zip` | ✓ | ✓ gl | ✓ | placeholder=0 | placeholder=1 | placeholder=0.5 |
| 5 | stone_wall_04 | `stone_wall_04_1k.zip` | ✓ | ✓ gl | ✓ | placeholder=0 | ✓ | ✓ |
| 6 | ground_tiles_08 | `ground_tiles_08_1k.zip` | ✓ | ✓ gl | ✓ | placeholder=0 | ✓ | ✓ |

**Naming conventions used in the zips** (varies — the setup agent inspects):

- color: `_color_1k.png`, `_basecolor_1k.png`, or `_baseColor_1k.png`
- normal: prefer `_normal_gl_1k.png` (OpenGL Y-up — matches Bevy default)
- roughness: `_roughness_1k.png`
- metallic: `_metallic_1k.png` (when present)
- AO: `_ambient_occlusion_1k.png` or `_ambientOcclusion_1k.png`
- height: `_height_1k.png`

**Placeholder strategy for missing channels** (architect formalises in
`02-design.md`):

- Missing metallic (snow, grass, stone, ground_tiles): reference a tiny
  `placeholder_black_1.png` (1×1 black PNG) in the MRH.R channel slot. The
  audit confirmed the baker supports same-file-multiple-layers via
  `ChannelSource.input`.
- Missing AO (grass): reference `placeholder_white_1.png` in Diffuse.A
  channel slot.
- Missing height (grass): reference `placeholder_gray128_1.png` (mid-grey,
  flat surface) in MRH.B channel slot.

These three placeholder PNGs go in `assets/materials/_placeholder/`.

## Existing baker pipeline — what we reuse vs. what's new

### Reuse without modification

| Component | Where | Why reuse |
|---|---|---|
| Baker binary | `crates/bevy_naadf/src/bin/bake.rs:25` | Headless AssetProcessor; already produces Basis-compressed `D2Array` images |
| `TextureArrayLoader` | `crates/bevy_naadf/src/texture_array/loader.rs:134` | Per-channel routing already generic |
| `TextureArrayDef` `.ron` schema | `crates/bevy_naadf/src/texture_array/def.rs` | Supports `ChannelSource { input, channel }` per channel per layer |
| `bake_texture_array` function | `loader.rs:134–202` | Produces correctly-typed `D2Array` `Image` |
| `TextureArrayBasisSaver` | (same crate, audit § 1) | Basis-compresses output |
| `just bake-texarrays` | `justfile:34–37` | Bake invocation |
| VNDF-GGX BRDF | `ray_tracing_common.wgsl:120–183` | `sample_vndf_isotropic`, `pdf_vndf_isotropic`, `geometry_term` — full GGX BRDF |
| Schlick Fresnel | `render_pipeline_common.wgsl:254–257` | `get_reflectance_fresnel` |
| `shoot_ray` | `ray_tracing.wgsl` | Re-entrant Amanatides–Woo DDA — caller fires reflection rays in a loop |
| 4-bounce mirror loop | `naadf_first_hit.wgsl:174–264` | Already exists; specular branch reuses it |
| ≤3-bounce GI + sun-shadow loop | `naadf_global_illum.wgsl:283–442` | Already exists; rough-specular branch reuses it |

### Re-author (no code change)

| Asset | Change |
|---|---|
| `assets/materials/diffuse.texarray.ron` | A ← `occlusion.png.R` (was: pass-through alpha). Add 7 new layers. |
| `assets/materials/normal.texarray.ron` | Switch source to `*_normal_gl_1k.png`. Add 7 new layers. |
| `assets/materials/occlusion_roughness_metallic_height.texarray.ron` | **Rename to `mrh.texarray.ron`.** Channels: R←metallic, G←roughness, B←height. AO removed (now in diffuse.A). Add 7 new layers. |
| **NEW** `assets/materials/emissive.texarray.ron` | New file. RGB=emissive. All 10 layers (3 existing + 7 new) source a black placeholder PNG unless the material has its own emissive (pavement already does). |

### New Rust code

1. **`MaterialSet` asset or resource** — bundles 4 `Handle<Image>` (diffuse_ao,
   normal, mrh, emissive) into one named unit keyed by `material_layer_index`.
   Architect decides: asset type with `.matset.ron` loader, OR resource built
   programmatically. (No existing analogue — audit § 6.)
2. **`VoxelType` reshape** — `crates/bevy_naadf/src/voxel/mod.rs:113–138`:
   replace `roughness`, `color_base`, `material_layer` fields; add
   `material_layer_index: u16`, `albedo_tint: Vec3`. Keep `material_base` as
   1-bit flag {PBR, Emissive} and `color_layered`.
3. **`GpuVoxelType` bit packing** — `crates/bevy_naadf/src/render/gpu_types.rs:273–295`:
   re-pack the 128-bit (`vec4<u32>`) layout to carry
   `material_layer_index` (~12 bits = 4096 materials), `material_base` (1 bit),
   variant span (architect's call), `albedo_tint` (3× f16), `color_layered`
   (3× f16). Free bits from removing `roughness` (f16) + `color_base` (3× f16)
   = 64 bits freed; need ~16+1 + albedo_tint = ~64 bits new → fits.
4. **WGSL: bind group + new functions** — `render/pipelines.rs` adds
   `texture_2d_array<f32>` × 4 + samplers. WGSL: new `triplanar_sample()`,
   `pom_displace_uv()`, unified `eval_pbr()` calls. Hit-shading branches in
   `naadf_first_hit.wgsl`, `naadf_global_illum.wgsl`, `spatial_resampling.wgsl`
   collapse to one PBR branch + one Emissive branch.
5. **e2e gate** — new gate flag in `bins/e2e_render.rs` (architect specifies),
   captures a known PBR voxel at a known pose, asserts specular highlight
   luminance + albedo texture variation (not flat color).

## Required reading (cited paths + why)

Every non-review agent MUST read these in order:

1. `docs/orchestrate/pbr-raymarching/01-context.md` — this file.
2. `docs/orchestrate/pbr-raymarching/00-reuse-audit.md` — full audit, both
   the initial section and the appended "Follow-up audit: declarative baker
   pipeline" section.
3. `docs/orchestrate/pbr-raymarching/02-design.md` — architect's design.
   (Implementer also reads the `## Decisions & rejected alternatives` and
   `## Assumptions made` sub-sections — those are the load-bearing trace.)
4. `crates/bevy_naadf/src/bin/bake.rs:1–120` — baker entry point.
5. `crates/bevy_naadf/src/texture_array/def.rs` (full) and
   `crates/bevy_naadf/src/texture_array/loader.rs:1–250` — `.texarray.ron`
   schema + bake function.
6. `crates/bevy_naadf/src/texture_array/mod.rs:105–133` — plugin registration.
7. `crates/bevy_naadf/src/baked_material.rs:29–56` — `MaterialRon` schema
   (informational — may inspire `MaterialSet.matset.ron` schema, though they
   serve different pipelines).
8. `crates/bevy_naadf/src/voxel/mod.rs:113–138` — `VoxelType`.
9. `crates/bevy_naadf/src/render/gpu_types.rs:273–295` — `GpuVoxelType` and
   `from_voxel_type`.
10. `crates/bevy_naadf/src/render/pipelines.rs` (full) — bind group layouts.
11. `crates/bevy_naadf/src/render/prepare.rs` (relevant sections — buffer
    upload paths, `voxel_types` buffer upload around line 380).
12. WGSL shaders:
    - `ray_tracing.wgsl` (full) — `shoot_ray`, ray result.
    - `ray_tracing_common.wgsl:95–185` — VNDF-GGX + helpers.
    - `render_pipeline_common.wgsl:254–257` — Schlick Fresnel; and lines
      90–130 around `decompress_voxel_type`.
    - `naadf_first_hit.wgsl:174–264` — primary loop + hit-shading.
    - `naadf_global_illum.wgsl:283–442` — GI loop + sun-shadow.
    - `spatial_resampling.wgsl:120–140` — `get_brdf`.
    - `world_data.wgsl:60–80` — bind group 0 declarations.
13. Existing material data:
    - `assets/materials/diffuse.texarray.ron`
    - `assets/materials/normal.texarray.ron`
    - `assets/materials/occlusion_roughness_metallic_height.texarray.ron`
    - `assets/materials/fabric/material.ron`
    - `assets/materials/gravelrock/material.ron`
    - `assets/materials/pavement/material.ron`
14. `justfile:34–37` — `bake-texarrays` target.
15. `bins/e2e_render.rs` (relevant sections — gate dispatch table + an
    existing framebuffer-capture gate as a template, e.g. `--oasis-edit-visual`).
16. `CLAUDE.md` (project root) — **verification discipline**: never run
    `cargo run --bin bevy-naadf` as a verification step; add a new e2e gate
    instead.

## Forbidden moves

The orchestrator and downstream agents are barred from these — they have
either been explicitly ruled out by the user or were demonstrated failure
modes in prior sessions.

- ❌ **Don't add a `metallic: f32` scalar to `VoxelType`.** Metallic comes
  from the texture sample (D1). Earlier audit draft proposed this; user
  pivot supersedes it.
- ❌ **Don't keep `MetallicRough` / `MetallicMirror` as separate material
  branches.** Collapse to unified PBR (D1).
- ❌ **Don't write a parallel baker.** The existing `bake.rs` + `TextureArrayLoader`
  + `TextureArrayBasisSaver` chain already does everything. Extend, never
  replace.
- ❌ **Don't extend `MaterialRon` / `MaterialRonLoader` to feed the
  raymarcher.** `MaterialRon` feeds Bevy's `StandardMaterial` mesh pipeline;
  that's a separate consumer. The NAADF raymarcher pipeline uses
  `.texarray.ron` definitions + a new `MaterialSet` type. Do not couple them.
- ❌ **Don't write new BRDF code from scratch.** The existing VNDF-GGX +
  Schlick Fresnel is correct and complete; the work is to compose them with
  metallic split (`F0 = mix(vec3(0.04), albedo, metallic)`,
  `diffuse = (1 - metallic) * albedo`), NOT to re-derive a BRDF.
- ❌ **Don't run `cargo run --bin bevy-naadf` for verification.** Per project
  `CLAUDE.md`: it boots a windowed app and proves nothing the deterministic
  gates haven't. Add or extend an `e2e_render` gate instead. Live visual
  check on the binary is the **user's** job, not the agent's.
- ❌ **Don't widen the `GpuVoxelType` size past 128 bits (`vec4<u32>`).**
  The buffer is used per-frame; bit packing must fit. The freed bits from
  removing `roughness` and `color_base` are enough; architect verifies the
  exact layout.
- ❌ **Don't break the `is_diffuse=0` / `is_diffuse=1` split between the
  first-hit pass and the GI pass.** The audit § 1 documents how the GI pass
  handles rough specular separately from primary — preserve this division.
  Specifically: the first-hit pass's mirror loop stays; rough specular
  contribution continues to come from the GI pass.
- ❌ **Don't add Bevy-only behaviour not in the C# NAADF reference.**
  Project rule per `bevy-naadf-faithful-port-rule` memory: any divergence
  from C# NAADF requires explicit user approval. This PR IS an explicit
  user-approved divergence (the C# NAADF has no triplanar PBR texture-array
  pipeline) — document this divergence in the implementation log and in any
  alignment-gap doc the architect updates.
- ❌ **Don't downgrade the rendered specular under reflection by skipping
  energy conservation.** The unified BRDF must conserve energy:
  `kS = F`, `kD = (1 - F) * (1 - metallic)`. The architect specifies the
  exact composition.
- ❌ **Don't checkout / restore / stash a file in any checkpoint commit
  sub-agent.** Commit-only, `git add -A .`, submodules-first then root.

## Success criteria (to be reflected in `04-review.md`)

1. **Compiles workspace-wide.** `cargo build --workspace` clean.
2. **Unit/integration tests pass.** `cargo test --workspace --lib` clean.
3. **All existing e2e gates still pass** with the new shading enabled, including
   the default Batch 6 framebuffer gate, `--oasis-edit-visual`, and
   `--small-edit-visual`.
4. **New PBR e2e gate passes** — a known PBR voxel at a known pose shows
   (a) a specular highlight luminance above a threshold, (b) sampled-albedo
   texture variation across at least 16 sample points in the captured region
   (not flat color), (c) for a metallic voxel, F0 ≈ albedo (not 0.04).
5. **Baker produces all 4 linked `.basis` arrays** with 10 layers each (3
   existing + 7 new). `just bake-texarrays` returns cleanly.
6. **No new monomorphizations or duplicate BRDF code** — the unified BRDF
   reuses `sample_vndf_isotropic` / `geometry_term` / `get_reflectance_fresnel`.
7. **`GpuVoxelType` is still 128 bits** (`vec4<u32>`). No buffer widening.
8. **Energy conservation verified by inspection** in the unified BRDF
   composition: `kS = F; kD = (1 - F) * (1 - metallic)`.

## Open questions left to the architect

- Variant blending (hard-select vs cross-fade) for the per-voxel
  procedural-blend in D6.
- Exact bit layout for the new `GpuVoxelType` packing (architect picks one;
  must fit in 128 bits, must include `material_layer_index`, `material_base`
  flag, `albedo_tint`, `color_layered`, any variant-span field).
- `MaterialSet` as asset (with `.matset.ron`) vs as resource built
  programmatically — architect picks one with a one-paragraph rationale.
- POM iteration count + early-out — start with linear-search + binary-search
  refine (industry standard) at e.g. 8/4 iterations; architect specifies.
- Whether to recompute the bouncing GI pass to use texture-sampled metallic
  too, or to use a per-VoxelType average (cheaper, less correct).
- Where to place the new `MaterialSet` Rust file in the crate tree.
- Whether the Emissive fast-path samples the Emissive `.texarray.ron` array
  AT ALL, or just emits `color_layered` (skipping the sample). The decision
  changes whether non-emissive voxels need any emissive sample at all.

## Worktree

All paths in this orchestration are relative to:
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/pbr-raymarching`

Branch: `feat/pbr-raymarching` (branched from local `main`).
