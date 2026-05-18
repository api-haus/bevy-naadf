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
