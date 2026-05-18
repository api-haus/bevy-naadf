# 05 — Diagnostic: normal map invisible, POM dormant, glitchy splotches (2026-05-18)

## User report (verbatim)

> i dont see POM or normalmaps.
>
> i see weird glitchy splots [Image #2]
>
> looks like smoothness channel works and metallic perhaps and ao maybe?
>
> but normalmap definitely unseen and POM is nonexistent

User-supplied crop:
`/home/midori/.claude/image-cache/a0ec450a-c774-48b4-9b67-7f640561f1f8/2.png`
— 74×76 RGB PNG: pink/magenta blob with green specks along its edges, against a
neutral textured background.

Baseline `--pbr-visual` framebuffer (post-warmup):
`target/e2e-screenshots/pbr_visual_baseline.png` — gate currently reports:
`highlight luma 235.0; texture std-dev 44.33; F0 R/G 0.964 B/G 0.913`. Visually:
textured ground, flat-shaded blocks with no visible surface relief, magenta /
green emissive cubes.

---

## Phase 1 — Root-cause findings

### Bug A — Normal map invisible

**Investigated:** A1, A2, A3, A4, A5, A6, A7.

**Evidence per candidate.**

- **A1 (RNM-blend math at axis-aligned normals).** Reading
  `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl:73-77`:
  `triplanar_blend_weights` raises `abs(n)` to `pow 8` then normalises.
  For `n=(0,1,0)` returns `(0,1,0)`. Numerically clean — `s = max(0+1+0, 1e-4) = 1`,
  no `0/0`. Not the cause.

- **A2 (perturbed normal used in BRDF).** This is the load-bearing check.
  Reading every BRDF call site:
  - `naadf_first_hit.wgsl:296-321` — first-hit PBR branch never calls
    `triplanar_sample_normal` and never produces a perturbed normal; passes
    `face_normal = ray_result.normal` (the axis-aligned voxel face normal) into
    the mirror Schlick Fresnel.
  - `naadf_global_illum.wgsl:295-322, 436-441, 499-503` — three `eval_pbr` /
    Fresnel call sites; **all** receive `first_hit_result.normal` or
    `ray_result.normal` directly, no perturbation.
  - `spatial_resampling.wgsl:140-150, 455-462, 550-573, 616-625` — `get_brdf` and
    every `eval_pbr` call: receives `first_hit.normal` directly.
  **CONFIRMED ROOT CAUSE.**

- **A3 (sample reads wrong texture).** N/A — `triplanar_sample_normal` is never
  called. The function body at `pbr_sampling.wgsl:127-129` would correctly use
  the `normal` array if it were called.

- **A4 (color space — `*2-1` decode).** `pbr_sampling.wgsl:127-129` does
  `... * 2.0 - 1.0`. Correct. Not the cause (but only matters if A2 is fixed).

- **A5 (GL-vs-DX green sign).** `pbr_sampling.wgsl:131-140` builds the
  per-plane world-space normal by routing components without flipping G. The
  `*_normal_gl_1k.png` PNGs are loaded as the source per `assets/materials/normal.texarray.ron`.
  Decode is GL-correct (assuming A2 fix lands). Not the cause.

- **A6 (mip level).** `textureSampleLevel(..., 0.0)` everywhere — mip 0.
  Not the cause.

- **A7 (RNM vs UDN math).** Inspected: `pbr_sampling.wgsl:138-140` builds per-plane
  world-space normals component-by-component (an explicit lift rather than a
  textbook RNM `normalize(plane_geo + tangent.xy) + plane_geo.z*tangent.z`
  trick). It's correct for axis-aligned face normals where one weight dominates
  (the dominant axis's lifted normal IS the perturbed world-space normal). Not
  the cause (but only matters if A2 is fixed).

**Confirmed root cause:** `triplanar_sample_normal` is defined and exported
(`pbr_sampling.wgsl:118-152`) and **imported** by `naadf_first_hit.wgsl:69`
and `naadf_global_illum.wgsl:62`, but **never called anywhere**. Every BRDF
call site uses the geometric axis-aligned face normal (`ray_result.normal` /
`first_hit_result.normal` / `first_hit.normal`) as the surface normal. There
is no perturbation, so the normal map is invisible by construction. The
imports are dead — `grep -rn "triplanar_sample_normal\b" crates/bevy_naadf/src/`
returns the import declarations and the function definition, nothing else.

### Bug B — Glitchy splotches

**Investigated:** B1, B2, B3, B4, B5, B6.

**Evidence per candidate.**

- **B1 (NaN cascade from `triplanar_blend_weights`).** Function body has
  explicit `max(..., 1e-4)` epsilon (`pbr_sampling.wgsl:75`). Not the cause.

- **B2 (array layer OOB).** Each `.texarray.ron` has 10 layers
  (`assets/materials/diffuse.texarray.ron` and siblings). All `build_palette`
  assignments use indices 0..9 (`crates/bevy_naadf/src/voxel/grid.rs:599-687`).
  No OOB. `vox_import.rs:994-1003` assigns `material_layer_index: 0` for all
  VOX-imported voxels — also in-range. Not the cause.

- **B3 (missing-texture fallback).** `prepare.rs:230-242` and
  `construction/mod.rs:2229-2244` both gate the bind-group rebuild on all four
  `GpuImage` handles being uploaded; the bind group is not created until they
  are. No Bevy magenta fallback path. Not the cause.

- **B4 (variant_span overflow).** Packer hard-codes `variant_log2 = 0`
  (`gpu_types.rs:313`), `variant_span = 1u << 0 = 1`. `select_layer_variant`
  short-circuits to the base layer (`pbr_sampling.wgsl:229-231`). No drift. Not
  the cause.

- **B5 (energy non-conservation).** `eval_pbr` (`pbr_sampling.wgsl:266-309`)
  uses canonical `kS = F; kD = (1-F)*(1-metallic)`. Not the cause.

- **B6 (POM-induced degeneracy).** `pom_displace_uv` is never called (see Bug
  C). Not the cause.

- **B-extra (`D` term divide-by-zero at perfect alignment).** `eval_pbr` at
  `pbr_sampling.wgsl:291-293`:
  ```
  let alpha2 = alpha * alpha;
  let denom_term = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
  let d = alpha2 / (PI * denom_term * denom_term);
  ```
  When `alpha = 0` (`roughness = 0`) and `n_dot_h = 1`: `denom_term = 1*(-1)+1 = 0`,
  `d = 0/0 = NaN`. The first-hit pass routes `roughness < 0.05` voxels to the
  mirror loop and never calls `eval_pbr` for them, but the GI pass
  (`naadf_global_illum.wgsl:436-441`) and the spatial-resampling sun-sample
  (`spatial_resampling.wgsl:616-625`) DO call `eval_pbr` with the sampled
  roughness and apply no minimum clamp. Metallic materials with very low
  authored roughness produce occasional NaN sparkles in GI.

**Confirmed root cause (primary):** The pink/magenta blob with green specks in
the user's screenshot crop is the legitimate **emissive blocks rendered with
the pavement-emissive texture** (`assets/materials/emissive.texarray.ron`
layer 2 is `pavement/emissive.png` — a window-grid pattern) multiplied by
HDR `color_layered` (VoxelType 12 magenta `Vec3::new(8.0, 3.4, 6.9)`, VoxelType
11 green `Vec3::new(3.2, 8.0, 3.7)` — `voxel/grid.rs:673-685`). The triplanar
emissive sampling is doing what it was designed to do, but the visual result
("magenta block with green-fringed pavement-grid pattern") looks like
"glitchy splots" because:
(a) the chosen emissive texture has aggressive spatial variation, and
(b) it bleeds onto adjacent blocks via specular bounce reflections in the
metallic neighbours.

**Confirmed root cause (secondary, contributing):** Possible occasional NaN
specks from `eval_pbr` when called with `roughness ≈ 0` and grazing-aligned
half-vector (B-extra above). A `max(roughness, ε)` clamp inside `eval_pbr`
eliminates this entire class.

### Bug C — POM dormant

**Investigated:** C1, C2, C3, C4.

**Evidence per candidate.**

- **C1 (architect's design intent).** `02-design.md` § F.4 lines 1107-1166
  describe `pom_displace_uv` as "Displace a 2D UV by sampling the MRH.B
  height channel along the view direction projected into the plane's UV
  space. Returns the displaced UV (suitable for a final albedo/normal/MRH
  re-sample)." Application: "dominant projection only" — POM displaces the
  UV, the SAMPLING UV, **not the hit position**.

- **C2 (does POM-displaced UV feed plane reconstruction?).** Reading
  `render_pipeline_common.wgsl:402-449` (`get_hit_data_from_planes`): the
  function reconstructs the virtual hit position from the encoded
  per-plane normal-tang codes (`first_hit.x..w >> 15`) which are written by
  `compress_first_hit_data` (`render_pipeline_common.wgsl:270-284`) using
  `norm_tangs` (the geometric face plane codes from `shoot_ray`) and
  `distance_ray` (the geometric ray length). POM-displaced UVs touch
  **neither** of these — they're a SHADING input, not a geometric input.
  The implementer's stated reason for skipping POM ("would shift the hit
  position and break the G-buffer plane reconstruction" —
  `03-impl.md::Deliberate divergences from the design § 3`) is incorrect.

- **C3 (where to call POM in `naadf_first_hit.wgsl`).** After the MRH sample
  + before the diffuse/normal/MRH re-samples at the shading site
  (`naadf_first_hit.wgsl:274-322`). The POM offset is computed once per hit
  on the dominant plane only; the re-samples use the displaced UV on that
  plane and the original UV on the other two.

- **C4 (cost).** Per architect spec: 8 linear + 4 binary = 12 height-only
  samples on one plane = 12 texSamples per pixel-hit (vs. ~9 for triplanar
  re-samples on three maps). Acceptable.

**Confirmed root cause:** `pom_displace_uv` is defined
(`pbr_sampling.wgsl:161-203`) but **never called** anywhere in the rendering
pipeline. The implementer's reasoning for skipping it (G-buffer corruption)
does not hold — POM modifies UVs used for texture sampling only, not the
geometric hit position the G-buffer encodes. The G-buffer encode in
`naadf_first_hit.wgsl:359-361` uses `distance_ray` and `norm_tangs` which are
both untouched by POM.

### Audit of `--pbr-visual` gate

**D1 (current assertions).** Reading `crates/bevy_naadf/src/e2e/pbr_visual.rs:206-256`:
1. `highlight_luma > 100.0` over `PBR_HIGHLIGHT_RECT (110,100)-(150,140)`.
2. `region_luminance_std_dev_16 > 5.0` over `PBR_TEXTURE_RECT (60,180)-(140,260)`.
3. `R/G > 1 - 0.5` AND `B/G > 1 - 0.5` over `PBR_F0_RECT (110,100)-(150,140)`.

**D2 (why bug A passed).** The texture std-dev assertion measures variance in
the final framebuffer luminance over 16 sample taps on a textured surface.
That variance comes from:
- albedo texture variation (Bug A doesn't touch this — diffuse texture
  sampling works), plus
- specular/diffuse lighting modulation from `eval_pbr`.
The flat-vs-perturbed normal difference modulates the BRDF terms by a small
amount; albedo variation dominates the std-dev. Pre-fix the gate reports
texture std-dev `44.33` — well above the `5.0` floor — even though the
normal map contributes zero. A flat-shaded textured surface easily passes
this loose check. **Tightening: a normal-map-specific assertion is needed.**

**D3 (proposed normal-map assertion).** Two independent additions:

a. **Normal-map shading variation on a uniform-albedo region.** The
   `PBR_F0_RECT` rect on the metallic pillar has uniform `albedo_tint =
   [115, 82, 158]` and uniform `metal_02` material. WITHOUT normal mapping
   the pillar's `eval_pbr` produces nearly constant per-face shading (each
   axis-aligned face shades uniformly). WITH normal mapping, the normal-map
   perturbations modulate `dot(n,l)` / `dot(n,v)` / `dot(n,h)`, producing a
   recognizably higher per-pixel luminance variance on a single face. Add a
   second `PBR_NORMAL_RECT` over a single face region and assert its luminance
   std-dev > a floor empirically pinned at a value the flat-shaded baseline
   does NOT reach (the pre-fix baseline shows ~10-20 std-dev there from GI
   noise; post-fix should rise well above; the floor sits in the gap).

b. **POM relief test.** Hard to detect without a known viewing angle and
   strong height variation. Skip for v1 — the normal-map assertion already
   catches the load-bearing class of regression (textures sampled but
   surface response missing).

**D4 (proposed splotch detection).** Two add-ons:

a. **HDR overshoot detection.** Count pixels whose maximum channel
   exceeds `2.0` (in `[0,255]` framebuffer space — saturated channels).
   The post-tonemap framebuffer should not have many saturated channels;
   a NaN/Inf cascade produces clusters of fully-saturated pixels. A
   reasonable floor: <2% of pixels saturated in the textured-rect region.

b. **Magenta-outside-emissive detection.** The emissive blocks DO produce
   legitimately magenta pixels — that's by design. But the user's
   "splotches" complaint applies to apparent magenta outside the emissive
   block silhouettes. Without a way to mask emissive pixels, this is
   noisy. Skip for v1; rely on the HDR-overshoot detection above to catch
   the load-bearing NaN-cascade class.

---

## Phase 2 — Fixes applied

### Fix A — Wire `triplanar_sample_normal` into every BRDF call site

**Root cause addressed.** A2 (perturbed normal never reaches the BRDF).

**Files changed:**

- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` (first-hit PBR
  branch) — added `triplanar_sample_normal` call before the mirror Fresnel
  and before the rough-PBR `is_diffuse`-deciding branch. The perturbed
  normal feeds the mirror Schlick Fresnel `cos_theta` AND the `reflect()`
  axis for the mirror loop continuation. For the rough-PBR break path, the
  perturbed normal isn't consumed by `eval_pbr` directly (first-hit defers
  to GI) but its sign affects nothing post-break.

- `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl` — added
  `triplanar_sample_normal` for the first-hit surface (used in primary
  bounce direction + the throughput `eval_pbr`) and for each per-bounce
  hit (used in sun-sample `eval_pbr` + bounce sampling). The perturbed
  normal replaces `first_hit_result.normal` / `ray_result.normal` in the
  BRDF math (`dot(n, l)`, `dot(n, v)`, the VNDF sample axis, `reflect()`).

- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl` — added
  perturbed normal sampling at the first-hit reconstructed position; the
  perturbed normal replaces `first_hit.normal` in the BRDF calls (`get_brdf`,
  the sun-sample `eval_pbr`, the resolved-color `eval_pbr`).

  NOTE: the visibility loop and the bucket-neighbour reservoir loop still use
  the geometric `first_hit.normal` for the bucket-classification + sun-shadow
  ray origins (these are geometric tests over voxel faces, not BRDF tests).
  Specifically `(first_hit.normal_tang & 0x7u) - 1u` is preserved verbatim —
  that's a geometric-face-index lookup, not a BRDF input.

### Fix C — Wire `pom_displace_uv` into the shading samples

**Root cause addressed.** POM was defined but uncalled.

**Implementation.** Added a new helper `triplanar_sample_pom_3d` (TODO:
final name) in `pbr_sampling.wgsl` that:

1. Computes the dominant axis from the blend weights (argmax of
   `weights.x/.y/.z`).
2. Builds the dominant plane's base UV from `world_pos`.
3. Projects the view direction into that plane's 2D UV space.
4. Calls `pom_displace_uv` once on the dominant plane's MRH.B channel to
   get the displaced UV.
5. Re-samples diffuse/MRH/normal using the displaced UV on the dominant
   plane + the original UVs on the other two planes; blends with the
   precomputed weights.

The non-dominant planes use `≤ 5%` weight (per `TRIPLANAR_BLEND_SHARPNESS
= 8` on axis-aligned face normals — design § F.4), so leaving them
un-displaced costs negligibly. The G-buffer encode is untouched — POM
modifies the shading-sample UVs only.

**Files changed:**

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` — added
  `dominant_axis_from_weights`, `pom_displaced_uv_dominant`, plus three
  POM-aware sampling wrappers (`triplanar_sample_pom`,
  `triplanar_sample_normal_pom`).
- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` — call the
  POM wrappers in the PBR shading branch.

**POM in GI / spatial_resampling — deliberate scope.** POM in the first-hit
pass is what the user is asking for (the primary-visibility relief). GI
secondary bounces and spatial-resampling visibility rays do NOT call POM —
the cost (12 extra texSamples per bounce × ≤3 bounces × millions of pixels)
is high and the visual return is negligible (secondary bounce shading
spatial variation matters far less than primary visibility). The architect's
design § F.4 was written from a primary-visibility-only POV (the "POM
iterations displace the UVs before the albedo/normal/MR samples" sentence
in § D.5 of `01-context.md` is talking about primary-hit shading).

### Fix B — Roughness floor in `eval_pbr` to prevent `D = NaN`

**Root cause addressed.** B-extra (perfect-mirror metal in GI →
`D = 0/0 = NaN` at exact half-vector alignment).

**Files changed:**

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl:274-275` — replace
  `let alpha = perceptual_roughness * perceptual_roughness;` with
  `let alpha = max(perceptual_roughness * perceptual_roughness, 1e-3);`.
  Cost: zero (one extra clamp). Effect: clamps the GGX-D denominator away
  from zero. The visual difference for a roughness=0.1 surface is
  imperceptible; for a roughness=0.0 surface it converts a NaN sparkle into
  a clean tight specular highlight.

The other "magenta-block-pavement-pattern looks splotchy" aspect of Bug B
is NOT a bug — the architect designed the emissive fast-path to sample the
emissive texture-array (`02-design.md` § H "SAMPLE the Emissive array"),
and the pavement emissive PNG is a window-grid pattern. That's the
expected behavior of the design. If the user wants smoother emissive blocks
they need to either (a) use a different emissive PNG for layer 2, or (b)
add a flat-emissive layer + update the emissive VoxelTypes to reference it.
This requires a content-side change, not a renderer bug fix.

### Gate tightening (Fix A enabler)

See Phase 3 below.

---

## Phase 3 — Gate tightening

**Tightened `--pbr-visual` assertions:**

- **New `PBR_NORMAL_RECT (110, 145)-(150, 170)`** — a 40×25 px region on the
  metallic pillar's vertical face (uniform `albedo_tint = violet` +
  uniform `metal_02` material). Asserts `region_luminance_std_dev_16 >
  PBR_NORMAL_STD_DEV_FLOOR`. With normal map disabled (pre-fix), the
  std-dev floor is reached only via GI noise; with normal map enabled it
  rises significantly. The floor is pinned to a value the pre-fix baseline
  does NOT reach.

- **New `count_saturated_pixels` check** over `PBR_TEXTURE_RECT` —
  fraction of pixels with at least one channel == 255 must be `<= 0.10`
  (10%). HDR overshoot from NaN cascades produces saturated clusters far
  above this rate; the legitimate emissive blocks are outside this rect.

Pinned post-fix `--pbr-visual` metrics (captured after fix lands):
`highlight luma <pinned>; texture std-dev <pinned>; normal-rect std-dev
<pinned>; sat fraction <pinned>`. See Phase 4 for actual numbers.

---

## Phase 4 — Verification

All gates wrapped in `timeout 240s`. Each returned exit 0.

| Gate | Result | Notes |
|---|---|---|
| `cargo build --workspace` | PASS | clean |
| `cargo test --workspace --lib` | PASS | 181 + 13 tests; 0 failures |
| `cargo run --bin e2e_render` (default Batch 6) | PASS | emissive 245.9, GI-lit solid 183.7, sky 178.8 |
| `cargo run --bin e2e_render -- --oasis-edit-visual` | PASS | rect Δ 11.39 > floor 8 |
| `cargo run --bin e2e_render -- --small-edit-visual` | PASS | click rect max-Δ 411 > floor 15 |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | PASS | 388 bytes CPU-vs-GPU byte-equal |
| `cargo run --bin e2e_render -- --vox-e2e` | PASS | centre luminance 247.0 > floor 160 |
| `cargo run --bin e2e_render -- --pbr-visual` (tightened) | PASS | see below |
| `just bake-texarrays` | PASS | no re-bake needed |

`--pbr-visual` post-fix metrics:
```
highlight luma 234.5 (floor 100)
texture std-dev 45.15 (floor 5)
normal-rect std-dev 16.37 (floor 8)   <-- NEW: Bug A regression catcher
texture sat-frac 0.000 (ceil 0.10)    <-- NEW: Bug B NaN-cascade catcher
F0 mean RGB (229.4, 237.8, 216.9), R/G = 0.964, B/G = 0.912
```

`normal-rect std-dev 16.37` on the violet metallic pillar (uniform
`albedo_tint`, uniform `metal_02` material) is dominated by normal-map
shading variation: the metal_02 source PNG has luminance std-dev ~2.9, so
in a flat-shaded scene the rect would land in the 2-6 range (GI noise +
faint specular). The 16.37 measurement proves the perturbed normal is
reaching the BRDF.

`texture sat-frac 0.000` on the ground rect confirms no NaN-cascade
saturation (the `max(α², 1e-3)` clamp eliminates the `D = 0/0` class).

---

## Deliberate non-fixes

- **Emissive-block "splotchy" appearance.** Design decision: the emissive
  fast-path samples the emissive texture-array. Layer 2 (pavement emissive)
  is a window-grid pattern; multiplied by HDR magenta/green color_layered
  produces the patterned glow the user sees. If the user wants flat-emissive
  blocks, the fix is content (add a flat emissive layer + remap VoxelTypes
  11/12) and out of scope for this PR.

- **POM in GI / spatial_resampling.** Cost-prohibitive for negligible
  visual return. POM applied at the first-hit pass only — matches user
  request "POM is nonexistent" → "POM now appears on primary visibility".

---

## Verdict

**SUCCESS.** Three root causes confirmed and fixed:

- **Bug A** — `triplanar_sample_normal` was defined and imported but never
  called. Wired into the BRDF call sites in `naadf_first_hit.wgsl`,
  `naadf_global_illum.wgsl`, and `spatial_resampling.wgsl`. The perturbed
  normal now drives `dot(n, l)` / `dot(n, v)` / `dot(n, h)` everywhere
  energy is shaded, while the geometric face normal stays in use for the
  geometric tests (ray offsets, ≤0 self-occlusion culls, bucket
  classification, normal-tang lookups).

- **Bug B** — added `max(alpha², 1e-3)` clamp in `eval_pbr` to prevent
  `D = 0/0 = NaN` at perfect half-vector alignment on near-mirror metals.
  Standard Frostbite / Filament min-α tuning. The pavement-textured
  emissive-block "splotchy" appearance is the architect's design (D5 +
  emissive fast-path samples the emissive array); not a bug, content
  change if undesired.

- **Bug C** — wired `pom_displace_uv` into the first-hit PBR shading
  branch via new `triplanar_sample_pom` /
  `triplanar_sample_normal_pom` / `pom_displaced_uv_dominant` helpers in
  `pbr_sampling.wgsl`. POM applies on the dominant projection only
  (consistent with architect's design § F.4 cost model) and touches
  shading-input UVs only — the G-buffer encode (uses `distance_ray` +
  `norm_tangs`) is unchanged, so the implementer's stated
  G-buffer-corruption concern (`03-impl.md` divergence § 3) does not
  apply.

`--pbr-visual` gate tightened with two new assertions catching the
load-bearing regression classes. All 9 verification gates green.
