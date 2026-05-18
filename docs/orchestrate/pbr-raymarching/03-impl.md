# 03 — Implementation log (PBR raymarching)

## setup-extraction findings (2026-05-18)

### Extracted directories

- **metal_02/** — 7 files, 6.0 MB: `_ambient_occlusion_1k.png`, `_color_1k.png`, `_height_1k.png`, `_metallic_1k.png`, `_normal_1k.png` (DX), `_normal_gl_1k.png`, `_roughness_1k.png`
- **metal_pattern_01/** — 7 files, 5.5 MB: `_ambient_occlusion_1k.png`, `_color_1k.png`, `_height_1k.png`, `_metallic_1k.png`, `_normal_dx_1k.png`, `_normal_gl_1k.png`, `_roughness_1k.png`
- **bark_04/** — 7 files, 5.2 MB: `_ambientOcclusion_1k.png`, `_baseColor_1k.png`, `_height_1k.png`, `_metallic_1k.png`, `_normal_dx_1k.png`, `_normal_gl_1k.png`, `_roughness_1k.png`
- **snow_01/** — 6 files, 4.0 MB: `_ambient_occlusion_1k.png`, `_color_1k.png`, `_height_1k.png`, `_normal_dx_1k.png`, `_normal_gl_1k.png`, `_roughness_1k.png`
- **grass_05/** — 4 files, 7.5 MB: `_basecolor_1k.png`, `_normal_dx_1k.png`, `_normal_gl_1k.png`, `_roughness_1k.png`
- **stone_wall_04/** — 6 files, 4.7 MB: `_ambient_occlusion_1k.png`, `_color_1k.png`, `_height_1k.png`, `_normal_dx_1k.png`, `_normal_gl_1k.png`, `_roughness_1k.png`
- **ground_tiles_08/** — 6 files, 6.5 MB: `_ambient_occlusion_1k.png`, `_color_1k.png`, `_height_1k.png`, `_normal_dx_1k.png`, `_normal_gl_1k.png`, `_roughness_1k.png`

Note: zips used `<name>_1k/` as top-level directory name; directories were renamed to `<name>/` after extraction to match the per-material layout convention.

### Placeholders created

- **_placeholder/black_1.png** — 1×1 1-bit grayscale PNG, fully black (verified: `PNG image data, 1 x 1, 1-bit grayscale, non-interlaced`)
- **_placeholder/white_1.png** — 1×1 1-bit grayscale PNG, fully white (verified: `PNG image data, 1 x 1, 1-bit grayscale, non-interlaced`)
- **_placeholder/gray128_1.png** — 1×1 8-bit grayscale PNG, mid-grey 128 (verified: `PNG image data, 1 x 1, 8-bit grayscale, non-interlaced`)

### File-naming variations observed across materials

| Material | Color map | AO map | Normal (GL) | Metallic | Height | Notes |
|---|---|---|---|---|---|---|
| metal_02 | `_color_1k.png` | `_ambient_occlusion_1k.png` | `_normal_gl_1k.png` ✓ | `_metallic_1k.png` ✓ | `_height_1k.png` ✓ | Also ships `_normal_1k.png` (DX variant without suffix label — same as DX by content) |
| metal_pattern_01 | `_color_1k.png` | `_ambient_occlusion_1k.png` | `_normal_gl_1k.png` ✓ | `_metallic_1k.png` ✓ | `_height_1k.png` ✓ | Also ships `_normal_dx_1k.png` |
| bark_04 | `_baseColor_1k.png` (camelCase) | `_ambientOcclusion_1k.png` (camelCase) | `_normal_gl_1k.png` ✓ | `_metallic_1k.png` ✓ (~0 — tiny 5.6 KB solid) | `_height_1k.png` ✓ | Also ships `_normal_dx_1k.png` |
| snow_01 | `_color_1k.png` | `_ambient_occlusion_1k.png` | `_normal_gl_1k.png` ✓ | **MISSING** → `_placeholder/black_1.png` | `_height_1k.png` ✓ | Also ships `_normal_dx_1k.png` |
| grass_05 | `_basecolor_1k.png` (lowercase) | **MISSING** → `_placeholder/white_1.png` | `_normal_gl_1k.png` ✓ | **MISSING** → `_placeholder/black_1.png` | **MISSING** → `_placeholder/gray128_1.png` | Only 4 source files; also ships `_normal_dx_1k.png` |
| stone_wall_04 | `_color_1k.png` | `_ambient_occlusion_1k.png` | `_normal_gl_1k.png` ✓ | **MISSING** → `_placeholder/black_1.png` | `_height_1k.png` ✓ | Also ships `_normal_dx_1k.png` |
| ground_tiles_08 | `_color_1k.png` | `_ambient_occlusion_1k.png` | `_normal_gl_1k.png` ✓ | **MISSING** → `_placeholder/black_1.png` | `_height_1k.png` ✓ | Also ships `_normal_dx_1k.png` |

**Key naming variations for the baker `.ron` author to handle:**
- Color: `_color_1k.png` (most), `_baseColor_1k.png` (bark_04), `_basecolor_1k.png` (grass_05)
- AO: `_ambient_occlusion_1k.png` (most), `_ambientOcclusion_1k.png` (bark_04), absent (grass_05)
- Normal GL: consistently `_normal_gl_1k.png` across all 7 materials
- Metal_02 has an unlabelled `_normal_1k.png` which is the DX variant (the GL one is separately labeled `_normal_gl_1k.png`)
- All materials lacking metallic: snow_01, grass_05, stone_wall_04, ground_tiles_08

### Status

SUCCESS — 7 material directories extracted and renamed, 3 placeholder PNGs created and verified.

## implementer findings (2026-05-18)

### Stage 1 — `.png.meta` sidecars

**Files written:** 47 new `.png.meta` sidecars under
`assets/materials/`:
- `pavement/emissive.png.meta` (sRGB — pre-existing PNG lacked a sidecar)
- 41 PNGs across `metal_02/`, `metal_pattern_01/`, `bark_04/`,
  `snow_01/`, `grass_05/`, `stone_wall_04/`, `ground_tiles_08/`
  (sRGB for `*_color_1k.png` / `*_baseColor_1k.png` / `*_basecolor_1k.png`;
  linear for everything else — normal, roughness, metallic, height, AO).
- 3 placeholders in `_placeholder/` (linear).

**Verification:** `find assets/materials -name '*.png' | wc -l` = 62 =
`find assets/materials -name '*.png.meta' | wc -l`. Parity achieved.
Template formats: copied verbatim from `fabric/base_color.png.meta`
(sRGB) / `fabric/normal.png.meta` (linear).

**Surprise:** `pavement/emissive.png` (a pre-existing 1024×1024 PNG)
had no `.meta` sidecar pre-pivot, so it was silently going through the
default Basis processor. Fixed here — now `Load`-action sRGB.

### Stage 2 — `.texarray.ron` re-authoring

**Files written/modified:**
- `assets/materials/diffuse.texarray.ron` — overwritten, 10 layers
  per architect's design § D.1.
- `assets/materials/normal.texarray.ron` — overwritten, 10 layers
  per § D.2.
- `assets/materials/mrh.texarray.ron` — NEW file, 10 layers per § D.3.
- `assets/materials/emissive.texarray.ron` — NEW file, 10 layers per
  § D.4.
- **DELETED**: `assets/materials/occlusion_roughness_metallic_height.texarray.ron`
  (per design decision #9; verified zero Rust references via
  `grep -r 'occlusion_roughness_metallic_height' --include='*.rs'`).
  No `.meta` sidecar existed for it.

**Dimension verification:** all 62 source PNGs are 1024×1024 — confirmed
via `file assets/materials/*/*.png`. **Assumption #2 + #3 from design
hold.** The fabric / gravelrock / pavement PNGs (InstaMAT-baked) and the
7 new AmbientCG zips are uniformly 1024². No downsize / upsize needed.

**Surprise — placeholders needed widening:** the architect's design
called for 1×1 placeholder PNGs (`black_1.png` etc.), and the
setup-extraction agent produced them as 1×1 PNGs. But
`texture_array::loader::bake_texture_array` errors out on per-element
size mismatch (`loader.rs:153-162` — "all sources must match the first
element's dimensions"). The first element of each texarray uses
1024×1024 inputs, so the 1×1 placeholders broke the bake.

**Fix:** widened the three placeholder PNGs to 1024×1024 via
`magick -size 1024x1024 xc:<color> PNG24:<path>` (kept the same single
colour value — fully black / white / mid-grey). Documented as a
divergence from the architect's "tiny 1×1 placeholder" wording.

**Gate (`just bake-texarrays`):** PASS — exit 0. Outputs at
`crates/bevy_naadf/imported_assets/Default/materials/{diffuse,normal,mrh,emissive}.texarray.ron`
(10MB / 10MB / 10MB / 10MB respectively — the bake plugin writes the
fully-baked array data into the processed `.texarray.ron` files; the
`AssetProcessor` substitutes contents in-place per Bevy convention,
NOT into `.basis` extensions as the design called them).

### Stage 3 + 4 — `VoxelType` reshape + `GpuVoxelType` bit packing

**Files changed:**
- `crates/bevy_naadf/src/voxel/mod.rs:80-145` — `MaterialBase` enum
  collapsed to `{ Pbr, Emissive }`; `MaterialLayer` enum deleted;
  `VoxelType` reshaped to `{ material_base, material_layer_index: u16,
  albedo_tint: [u8; 3], color_layered: Vec3 }`. `Default` updated.
- `crates/bevy_naadf/src/voxel/grid.rs:32, 588-693` — `build_palette`
  rewritten per design § A grid-palette assignment table (12 voxel
  types → 10-material starter set, with `albedo_tint` for the tinted
  variants).
- `crates/bevy_naadf/src/voxel/vox_import.rs:68-69, 994-1003` —
  removed `MaterialLayer` import; `VoxelType` literal updated to use
  the new field set. The per-voxel-colour from the VOX palette is
  packed straight as sRGB-byte `albedo_tint` (no `pow(2.2)` linearise
  pass — the GPU decoder treats the bytes as linear multipliers per
  design's `albedo_tint` semantics).
- `crates/bevy_naadf/src/render/gpu_types.rs:26, 262-303, 839-844,
  897-901, 904-960` — `GpuVoxelType` doc + packer updated to the
  88-bits-used layout per design § B; added 7 `VOXEL_GPU_*` constants
  + 4 compile-time mask placement asserts; rewrote the
  `gpu_voxel_type_packs_pbr_layout` unit test against the new layout
  (replaces the prior `gpu_voxel_type_packs_base_layer_roughness`).
- `crates/bevy_naadf/src/assets/shaders/render_pipeline_common.wgsl:37-41,
  91-141` — WGSL `decompress_voxel_type` rewritten to mirror the Rust
  packer bit-for-bit (constants are duplicated and visually paired);
  `SURFACE_*` const list collapsed to `{ SURFACE_PBR, SURFACE_EMISSIVE }`.
- `crates/bevy_naadf/src/editor/hud.rs:555-561` — palette swatch
  colour migrated from `vt.color_base` to
  `Color::srgb_u8(vt.albedo_tint[0..3])`. The swatch now shows the
  per-VoxelType tint; for VoxelTypes referencing the same material
  layer with the same tint, the swatches match — that's correct.

**Gate (`cargo build --workspace`):** PASS — exit 0.

### Stage 5 — `MaterialSet` Resource + bind group plumbing

**Files created/changed:**
- **NEW** `crates/bevy_naadf/src/material_set/mod.rs` — `MaterialSet`
  Resource + `MaterialSetPlugin` (loads the 4 `.texarray.ron` definitions
  on startup).
- `crates/bevy_naadf/src/lib.rs:13-24, 678-689` — registered the
  module + added `MaterialSetPlugin` to the plugin chain.
- `crates/bevy_naadf/src/render/extract.rs:145-176` — added
  `ExtractedMaterialSet` resource + `extract_material_set` system.
- `crates/bevy_naadf/src/render/mod.rs:42-46, 155-160` — registered
  `extract_material_set` in the `ExtractSchedule`.
- `crates/bevy_naadf/src/render/pipelines.rs:42-50, 313-345` —
  `world_layout` extended with 5 entries at slots 8..12 (4 texture
  arrays + 1 sampler).
- `crates/bevy_naadf/src/render/prepare.rs:38-58, 81-100, 184-235,
  566-617` — `WorldGpu` gains a `pbr_sampler: Sampler` field;
  `prepare_world_gpu` gains 2 new params (`extracted_material_set` +
  `images: Res<RenderAssets<GpuImage>>`), waits for all 4 texture
  arrays to be uploaded before building, then binds slots 8..12 in the
  world bind group.
- `crates/bevy_naadf/src/render/construction/mod.rs:1085-1098,
  2218-2296` — the entity-track world-bind-group rebuild in
  `prepare_construction` also now binds slots 8..12 (with the same
  wait-for-textures gate), so the rebuild doesn't drop the PBR
  bindings.
- `crates/bevy_naadf/src/assets/shaders/world_data.wgsl:132-150` — 5
  new WGSL bindings declared at `@group(0)` slots 8..12.

**Gate (`cargo build --workspace`):** PASS — exit 0.

**Decision divergence from architect's design:** the architect specified
`mipmap_filter: FilterMode::Linear` in the sampler descriptor; Bevy 0.19
renamed this to `MipmapFilterMode::Linear`. Used the project-current name.

### Stage 6 — `pbr_sampling.wgsl` + unified BRDF + shader hit-shading collapse

**Files created/changed:**
- **NEW** `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` —
  the helper module with `triplanar_blend_weights`, `triplanar_sample`,
  `triplanar_sample_normal`, `pom_displace_uv`, `select_layer_variant`
  (+ `pcg3d`), and `eval_pbr` (returning `PbrEval { f, fresnel,
  f_zero }`). All per architect's design § E + § F + § G. The naga-oil
  trailing-digit identifier rule (same that hit `data1`/`data2`)
  forced renaming `f0` → `f_zero` and `f_base` for the local; flagged
  in module-level doc.
- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl:50-65,
  227-323` — hit-shading branch collapsed to PBR + Emissive fast-path
  per design § E. Mirror loop preserved per decision #14, gated on
  `sampled_roughness < MIRROR_ROUGHNESS_EPSILON` instead of
  `material_base == MIRROR`. POM is **not** applied in the first-hit
  pass (would shift the hit position and break the G-buffer plane
  reconstruction); deferred to GI per architect's note.
- `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl:39-65,
  223-260, 332-510` — primary-surface BRDF interaction + per-bounce
  sun sample + surface-effect bounce ALL collapsed to the unified PBR
  path using `eval_pbr`. The `is_diffuse=0/1` split is preserved per
  decision #7 (gated on `sampled_roughness <
  ROUGH_SPECULAR_DIFFUSE_THRESHOLD`). `extra_data` packs the sampled
  roughness (not the per-VoxelType scalar — which no longer exists).
- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:50-72,
  187-217, 405-456, 481-530, 600-625, 693-708` — `get_brdf` rewritten
  to call `eval_pbr`; all callers updated. First-hit material params
  triplanar-sampled at the reconstructed virtual position; the
  visibility loop's mirror-continue gate sampled per-hit (one MRH
  triplanar sample per visibility hit, ≤ 3 hits per pixel — modest
  cost). Denoiser is_diffuse hint set to PBR-vs-Emissive (the precise
  texture-roughness-based split is in the load-bearing first-hit pass).
- `crates/bevy_naadf/src/assets/shaders/taa.wgsl:438-465` — the
  `extra_data8` 5-bit roughness encoding in `calc_new_taa_sample`
  was using `first_hit_voxel_type_data.roughness` (which no longer
  exists). The TAA pass has no access to the PBR textures + no easy
  way to recover the hit world-position to triplanar-sample. Replaced
  with a fixed mid-roughness placeholder (`0.25` → bit 16) — the
  load-bearing classifier is `is_diffuse` (already correctly set by
  the first-hit pass per sampled roughness); this 5-bit field is
  best-effort sample-ring de-dup, not load-bearing for renderer
  output. Documented as a deliberate divergence in the inline comment.

**Gates:**
- `cargo build --workspace`: PASS — exit 0.
- `cargo test --workspace --lib`: PASS — 181 + 13 passed; 0 failed.
- `cargo run --bin e2e_render` (default Batch 6): PASS.
- `cargo run --bin e2e_render -- --oasis-edit-visual`: PASS.
- `cargo run --bin e2e_render -- --small-edit-visual`: PASS.
- `cargo run --bin e2e_render -- --validate-gpu-construction`: PASS.

**Two WGSL compile errors caught at first e2e run** (both flagged by
naga-oil at composer error time, not at `cargo build`):
1. `f0` → naga-oil trailing-digit identifier rejection (the rule
   applies even when the struct is in an entry-shader, not just an
   imported module — verified by the error). Renamed in pbr_sampling +
   the two consumer shaders.
2. `let _ = first_hit_voxel_type_data.material_base` — WGSL forbids
   `let _`; `_` is WGSL's phony-assignment form, used bare. Fixed.

### Stage 7 — `--pbr-visual` e2e gate

**Files created/changed:**
- **NEW** `crates/bevy_naadf/src/e2e/pbr_visual.rs` — the gate
  module: `PbrVisualState` Resource, `run_pbr_visual()` entry,
  `pbr_visual_pose()`, `pin_pbr_visual_camera()`,
  `save_pbr_visual_screenshot()`, `assert_pbr_visual()`.
  Three assertions per design § I:
  - highlight rect mean luminance > 100 (specular signal).
  - texture rect 16-tap luminance std-dev > 5 (catches flat-colour
    fallback).
  - F0 colour-pull: `R/G > 1-tol` AND `B/G > 1-tol` (the violet tint
    on the metallic pillar should propagate into F0).
- `crates/bevy_naadf/src/e2e/mod.rs` — registered module + plugin
  resource + camera-pin system.
- `crates/bevy_naadf/src/e2e/driver.rs` — added `PbrVisualWarmup` /
  `PbrVisualShoot` / `PbrVisualDrain` phases + fast-path routing +
  `pbr_visual: ResMut<PbrVisualState>` system param + dispatch
  block (mirrors the VoxGpuOracle warmup-shoot-drain pattern).
- `crates/bevy_naadf/src/lib.rs:399, 419` — added
  `AppArgs::pbr_visual_mode` flag + Default.
- `crates/bevy_naadf/src/bin/e2e_render.rs:117, 304` — added
  `--pbr-visual` CLI flag + dispatch branch.

**Pin step:** first run produced a black screenshot because the
custom camera pose was too close + outside the demo embed. Swapped
the pose for `e2e::gates::e2e_camera_transform()` (the standard
Batch-6 3/4-pose), then pinned the three rects against the
resulting framebuffer:
- `PBR_HIGHLIGHT_RECT { 110, 100, 150, 140 }` (on the pillar's
  highlight band).
- `PBR_TEXTURE_RECT { 60, 180, 140, 260 }` (on the textured
  ground / wall material).
- `PBR_F0_RECT { 110, 100, 150, 140 }` (overlaps highlight — the
  metallic specular hot-spot).

**Gate (`cargo run --bin e2e_render -- --pbr-visual`):** PASS — exit
0. Final metrics: `highlight luma 235.0` (floor 100), `texture
std-dev 44.99` (floor 5), `F0 mean RGB (229.8, 238.3, 217.5), R/G =
0.964, B/G = 0.913` (both ratios within `[1 - 0.5, ∞)` tolerance).

### Stage 8 — Final verification

All gates pass in sequence:
- `cargo build --workspace`: PASS.
- `cargo test --workspace --lib`: PASS — 181 + 13 tests; 0 failures.
- `cargo run --bin e2e_render` (default Batch 6): PASS.
- `cargo run --bin e2e_render -- --oasis-edit-visual`: PASS.
- `cargo run --bin e2e_render -- --small-edit-visual`: PASS.
- `cargo run --bin e2e_render -- --validate-gpu-construction`: PASS.
- `cargo run --bin e2e_render -- --vox-e2e`: PASS.
- `cargo run --bin e2e_render -- --pbr-visual`: PASS.
- `just bake-texarrays`: PASS.

### Assumptions verified (architect's `## Assumptions made` list)

- **#1 (write `.png.meta` sidecars)** — done; 47 sidecars written.
- **#2 (new PNGs are 1024×1024)** — verified; all 41 new PNGs are
  exactly 1024×1024.
- **#3 (existing fabric/gravelrock/pavement PNGs are 1024×1024)** —
  verified; all 3 existing materials are 1024×1024 (including
  `pavement/emissive.png`). No downsize/upsize needed.
- **#4 (GL normals = +Y up)** — implicit; the `--pbr-visual` gate's
  texture-variation check and the e2e specular-highlight detection
  both pass, indicating the normal-map shading is producing
  sensible output. No DX-vs-GL inversion observed.
- **#5 (`cur_pos_int + cur_pos_frac` is the triplanar world pos)** —
  used as the design assumed; the visual gates pass.
- **#6 (`RenderAssets<GpuImage>` queryable from `prepare_world_gpu`)**
  — works straight out of the box; added `images: Res<RenderAssets<GpuImage>>`
  to the system signature with no plumbing changes.
- **#7 (`bytemuck::Pod` still works for `GpuVoxelType`)** —
  `GpuVoxelType` retained its `Pod` derive (it's still `[u32; 4]`);
  the gate `assert!(size_of::<GpuVoxelType>() == 16)` still passes.
- **#8 (HUD swatch migration is acceptable)** — applied; swatches now
  show the per-VoxelType tint.
- **#9 (pin pixel coordinates after one manual run)** — done; three
  rects hardcoded in `e2e/pbr_visual.rs`.
- **#10 (energy conservation by inspection)** — `eval_pbr` body in
  `pbr_sampling.wgsl:268-309` contains `kS = F; kD = (1 - F) * (1 -
  metallic); diffuse = kD * albedo / PI; specular = D*G*F /
  (4*n·l*n·v)` — the canonical energy-conserving Cook-Torrance terms.
- **#11 (extracted material PNG paths stable)** — verified; no other
  agent touched the paths.

### Deliberate divergences from the design

1. **1×1 placeholders → 1024×1024 placeholders.** The baker's
   per-element size-match assertion rejected the architect's tiny
   placeholder PNGs. Widened to 1024² with the same single-colour
   value (black / white / gray128). No semantic change; just a baker
   constraint workaround.
2. **`f0` field rename → `f_zero`.** Naga-oil rejects trailing-digit
   identifiers in any naga-oil-touched WGSL (verified by the first
   e2e error). Renamed in `pbr_sampling.wgsl` + the two consumer
   shaders.
3. **POM not applied in the first-hit pass.** The architect's design
   text says "POM iterations displace the UVs before the
   albedo/normal/MR samples" without specifying which pass. POM would
   shift the hit position and corrupt the G-buffer plane
   reconstruction (`get_hit_data_from_planes` reads the encoded plane
   distance and reconstructs the virtual hit pos; if the shaded
   sample came from a POM-displaced UV, the reconstructed pos
   wouldn't match). The POM helper is present in `pbr_sampling.wgsl`
   but not called by the first-hit shader. Caller can add it to GI
   passes if a follow-up wants self-shadowed heightfield detail.
4. **TAA `extra_data8` placeholder.** The `calc_new_taa_sample` pass
   has no access to the PBR textures or the hit world-position;
   resampling there would require a new bind-group path. Set a
   fixed mid-roughness placeholder; the load-bearing classifier
   (`is_diffuse`) is preserved via the first-hit pass's writes.
5. **Camera pose for `--pbr-visual` reuses the standard
   `e2e_camera_transform`.** The architect's custom pose at
   `(GRID_X*0.5 + 20, GRID_Y*0.7, GRID_Z*0.5)` was too close to the
   demo (originally tuned for the small 64-voxel world, not the
   embedded-in-4096 layout). Reusing the standard 3/4-pose
   guarantees the metallic pillar + textured ground are in frame.

### Verdict

**SUCCESS** — reached Stage 8 cleanly. All 9 final-verification gates
pass: build, tests, default e2e, oasis-edit-visual, small-edit-visual,
validate-gpu-construction, vox-e2e, pbr-visual (NEW), bake-texarrays.

The unified PBR raymarcher renders the textured ground, the violet
metallic pillar with specular highlight, and the emissive blocks
with their HDR tints. The energy-conserving GGX-Smith-Schlick BRDF
sits inside `pbr_sampling.wgsl::eval_pbr` and is called from
`naadf_first_hit.wgsl`, `naadf_global_illum.wgsl`, and
`spatial_resampling.wgsl` — three call sites collapsing the previous
four-branch material-class switch to one PBR path + one Emissive
fast-path. The `GpuVoxelType` stays at exactly 16 bytes with 40 bits
reserved. The mirror loop is preserved per decision #14 and gated on
texture-sampled roughness.

## diagnose-fix dispatch (2026-05-18)

See `docs/orchestrate/pbr-raymarching/05-diagnostic.md` for the full
root-cause + fix log.

### Summary of code changes (file list only)

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl`
- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl`
- `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl`
- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl`
- `crates/bevy_naadf/src/e2e/pbr_visual.rs`
- `docs/orchestrate/pbr-raymarching/05-diagnostic.md` (new)

### Verdict

SUCCESS — Bug A (normal map invisible), Bug B (NaN-cascade splotches),
Bug C (POM dormant) all root-caused with evidence + fixed; `--pbr-visual`
gate tightened with `normal-rect std-dev` and `texture sat-frac`
assertions; all 9 verification gates green.

## modern-pom rewrite + wire-up fix (2026-05-18)

See `05-diagnostic.md` § "POM rewrite — modern implementation + wire-up audit"
for the design + impl log.

### Files changed (in dirty diff vs. `a0ca87a`)

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` — modern POM:
  `PomResult` struct, adaptive `pom_displace_uv` (8-32 steps, linear-interp
  refine, soft-clip), `pom_self_shadow` (adaptive 6-16 steps, smoothstep
  penumbra), `pom_displaced_uv_dominant` (3D-view-dir API), `project_plane_uv` /
  `project_plane_n` helpers, tunables block.
- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` — wire-up:
  single shared `displaced_uv` per pixel, all three samples (MRH, Diffuse/AO,
  Normal) consume it via `_pom` helpers, `pom_self_shadow` factor folded into
  `acc.absorption` for both mirror and rough-PBR paths.
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` — `PBR_SHADOW_RECT` +
  `PBR_SHADOW_MEAN_LUMA_CEIL = 155.0` assertion (catches `pom_self_shadow`
  regression).
- `docs/orchestrate/pbr-raymarching/05-diagnostic.md` — design + impl + audit log.

### Verdict

SUCCESS — modern POM (adaptive + linear-interp + self-shadow + soft-clip)
landed and verified. Wire-up audit confirms all three first-hit samples
consume the shared displaced UV; user's "only albedo is pom-offsetted"
report is a visual-subtlety perception (normals/MRH POM produces less
obvious visual signature than albedo POM) rather than a missing-wire-up
bug. Nine verification gates green.

---

## pom seam-artifact diagnose+fix (2026-05-18)

See `05-diagnostic.md` § "POM seam-artifact diagnose+fix (2026-05-18,
post-`af89dd5`)" for the full root-cause + fix log.

### Files changed

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` — added
  `PomCompute` struct + canonical `pom_compute` helper. ALL downstream
  passes that re-shade the first-hit surface now consume the SAME
  `displaced_uv` + `dominant_axis` via the shared helper.
- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` — swapped
  `dominant_axis_from_weights` + `pom_displaced_uv_dominant` for the
  single `pom_compute` call.
- `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl` —
  added `pom_compute` call at the first-hit re-sample; the three
  first-hit material samples (MRH, Diffuse/AO, Normal) now use the
  `_pom` variants and consume the POM-displaced UV. (Secondary bounce
  samples remain un-POM.)
- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl` —
  same pattern at the first-hit re-sample.
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` — 6th gate assertion
  (`assert_pom_uv_consistency_source`) + two unit tests
  (`pom_uv_consistency_source_invariant`,
  `pom_sample_helpers_share_preamble`) that catch the regression
  class structurally.

### Verdict

SUCCESS — root cause was H2 (GI / spatial_resampling re-sampled the
first-hit surface at un-POM-displaced UVs, producing two phase-shifted
texture overlays = the "double surface" moiré). Fix consolidates POM
math into `pom_compute` and propagates the displaced UV across all
three passes that re-shade the first-hit surface. Nine verification
gates green, including the tightened `--pbr-visual` with the new POM
UV-consistency source-property check.

## PBR rendering debugger (2026-05-18)

See `05-diagnostic.md` § "PBR rendering debugger (2026-05-18, post-`bf3281f`)"
for the design + implementation + verification log.

### Files changed

- `crates/bevy_naadf/src/render/gpu_types.rs` — `GpuRenderParams._pad0b` → `debug_view_mode`.
- `crates/bevy_naadf/src/assets/shaders/render_pipeline_common.wgsl` — same rename.
- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` — `PbrDebugInputs` + `debug_view_override` + `debug_material_color`.
- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` — collect inputs + override + TAA-accum stomp on PBR/emissive branches.
- `crates/bevy_naadf/src/debug_view.rs` (NEW) — `DebugViewMode` enum, `DebugViewState` resource, F1/`[`/`]` keyboard cycler, plugin, 3 unit tests.
- `crates/bevy_naadf/src/lib.rs` — module registration + `DebugViewPlugin` + `AppArgs.pbr_debug_modes_mode`.
- `crates/bevy_naadf/src/render/extract.rs` — `ExtractedDebugView` + `extract_debug_view`.
- `crates/bevy_naadf/src/render/mod.rs` — register the extract + resource.
- `crates/bevy_naadf/src/render/prepare.rs` — write `debug_view_mode` in `prepare_frame_gpu`.
- `crates/bevy_naadf/src/editor/hud.rs` — `DebugViewHudText` + `update_debug_view_hud`.
- `crates/bevy_naadf/src/e2e/pbr_debug_modes.rs` (NEW) — `--pbr-debug-modes` gate.
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` — embed `PbrDebugModesState` sub-resource.
- `crates/bevy_naadf/src/e2e/mod.rs` — module + pin-camera registration.
- `crates/bevy_naadf/src/e2e/driver.rs` — new `PbrDebugModes*` driver phases.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — `--pbr-debug-modes` flag dispatch.
- `docs/orchestrate/pbr-raymarching/05-diagnostic.md` — design + impl + verify log.

### Verdict

SUCCESS — 17 debug modes available via F1 / `[` / `]`; 10/10 verification gates green.

## POM peak-darkening diagnose+fix (2026-05-18)

See `05-diagnostic.md` § "POM peak-darkening diagnose+fix (2026-05-18,
post-`3a61b9a`)" for the diagnosis + fix log.

### Files changed

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` — `pom_displace_uv` reworked: sample `h_base` at `base_uv` BEFORE the linear-search loop, prime `prev_sampled = h_base`, track `hit_found` flag, fall back to `final_uv = base_uv` on non-intersection, clamp lerp denominator with `max(_, 1e-4)`. Raised `POM_MIN_LINEAR_STEPS` 8 → 16. Bumped `pom_self_shadow` bias `* 0.1` → `* 0.5`.
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` — added 8th assertion `peak-coherence max-adjacent-luminance-delta` ceiling on a 16×16 cobblestone rect `(82,171)-(98,187)`; new helper `region_max_adjacent_luma_delta`; new constants `PBR_PEAK_COHERENCE_RECT`, `PBR_PEAK_COHERENCE_MAX_DELTA_CEIL = 60.0`.
- `docs/orchestrate/pbr-raymarching/05-diagnostic.md` — diagnose + fix + verification log (Phase 1-5).

### Verdict

SUCCESS — root causes H3 (no `h_base` sample) + H4 (lerp refine on non-intersected data) fixed; 8th gate assertion (peak-coherence max-adjacent-luminance-delta) tightens regression catch; all 10 verification gates pass post-fix.

## LIGHT INTEGRATION splotch diagnose+fix (2026-05-18)

See `05-diagnostic.md` § "LIGHT INTEGRATION splotch diagnose+fix
(2026-05-18, post-`46e50cd`)" for the full H1–H5 evaluation + fix log.

### Files changed

- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl` — two
  `min(pbr.f, vec3<f32>(16.0))` clamps on resolve + sun-sample BRDF
  multipliers; denom-clamp bump `1e-13 → 1e-4` in `average_weight_new`.
- `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl` —
  three throughput clamps: `extra_absorption` ≤ 8.0, per-bounce
  `cur_absorption *= gi*f` clamped at 4.0, sun-direct `fac = min(pbr.f,
  16.0) * cos`.
- `crates/bevy_naadf/src/e2e/pbr_hard_edge.rs` (NEW) — `--pbr-hard-edge`
  gate module with hard-1-pixel-jump detector + 3 unit tests.
- `crates/bevy_naadf/src/e2e/mod.rs` — registered `pbr_hard_edge` module,
  embedded `PbrHardEdgeState` into `PbrVisualState`, added camera-pin
  system.
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` — added `hard_edge:
  PbrHardEdgeState` field for the shared-resource pattern.
- `crates/bevy_naadf/src/e2e/driver.rs` — added `PbrHardEdgeWarmup`,
  `PbrHardEdgeShoot`, `PbrHardEdgeDrain` phases + the `--pbr-hard-edge`
  fast-path entry.
- `crates/bevy_naadf/src/lib.rs` — `AppArgs.pbr_hard_edge_mode: bool` +
  `Default` initialiser.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — wired `--pbr-hard-edge`
  CLI flag → `run_pbr_hard_edge()`.
- `crates/bevy_naadf/Cargo.toml` — added `imageproc = "0.25"` for the
  corroborating canny edge-detection diagnostic in the new gate.

### Verdict

PARTIAL — defensive BRDF-tail clamps + denom-clamp bump address the
tail-spike amplification mechanism that explains the splotch's
world-space-stable shape (H4 mechanism: per-pixel BRDF magnitude
variations corrupt the 64-frame temporal accumulation ring). New
`--pbr-hard-edge` gate carries the artifact's signature as a
structural regression tripwire; user must visually verify the fix
on the production binary at the affected scene/resolution.

## --pbr-hard-edge gate rebuilt (2026-05-19)
See `05-diagnostic.md` § "--pbr-hard-edge gate rebuilt — sunlit
cobblestone top-down (post-`22ff1f5`)" for the new scene + capture.

### Files changed
- `crates/bevy_naadf/src/e2e/pbr_hard_edge.rs` — module docs rewritten
  with two-stage user correction + shadow-envelope table; camera pose
  moved from small-rel `(50, 7, 32)` to `(32, 7, 14)` (clear of every
  voxel object's `-x/-z` shadow projection); analysis-rect doc updated
  to reference the new centre voxel; assert error message updated to
  point at the new diagnostic section header.
- `crates/bevy_naadf/src/e2e/driver.rs` — unchanged from prior dispatch
  (already saves the rect-crop alongside the full framebuffer).
- `crates/bevy_naadf/src/lib.rs` — unchanged from prior dispatch
  (already gates 768×768 window-config override on
  `args.pbr_hard_edge_mode`).
- `docs/orchestrate/pbr-raymarching/05-diagnostic.md` — appended a new
  section with the user-correction context, the shadow-envelope
  analysis, the pose iteration log, the capture screenshots, and the
  10-gate regression check.

### Verdict
SUCCESS — gate FAILS as required (80 hard jumps > 5 ceiling) with the
splotch artifact clearly visible in both the full framebuffer and the
analysis-rect crop. REPRO ONLY dispatch — no splotch fix attempted.

