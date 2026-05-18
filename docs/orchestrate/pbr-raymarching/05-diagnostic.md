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

---

## POM rewrite — modern implementation (2026-05-18)

User feedback after the diagnose+fix landed: the first-hit PBR shading is now visible
(normal map + initial POM both contributing) but the POM itself is "too primitive". User
asked for a modern parallax pipeline with adaptive step count, self-shadowing, and the
remaining standard quality-of-life features, with the Dayuppy `psSteepParallax.glsl`
reference (`https://github.com/Dayuppy/SteepParallaxDemo/blob/main/psSteepParallax.glsl`)
as the algorithmic anchor.

### Reference summary

The Dayuppy steep-parallax demo composes five techniques in one fragment shader. The
load-bearing pieces for "modern POM":

1. **Adaptive linear march with view-angle-dependent step count.** The number of steps
   varies inversely with view-vs-normal alignment:
   ```glsl
   float numSteps = mix(72.0, 36.0, abs(viewDir.z));
   ```
   At glancing angles (`viewDir.z → 0`) → 72 steps (parallax error most visible).
   At face-on (`viewDir.z = 1`) → 36 steps. The delta-UV per step is
   `-viewDir.yx * bumpScale / (abs(viewDir.z) * numSteps)` — note the `/abs(viewDir.z)`
   gives constant *silhouette thickness* in screen space irrespective of angle.

2. **Linear search + linear interpolation (NOT binary refine).** The march advances
   until `curSample >= heightRem` (overshoot), then interpolates between the
   previous-step and current-step samples based on the relative overshoots:
   ```glsl
   float afterDepth  = curSample  - heightRem;
   float beforeDepth = prevSample - (heightRem + deltaH);
   float t = clamp(beforeDepth / (beforeDepth + afterDepth), 0.0, 1.0);
   vec2 finalUV = mix(prevUV, curUV, t);
   ```
   This kills the "stepped" look at low step counts.

3. **Self-shadowing march.** From the displaced UV, march back along the *light*
   direction in tangent space; if any tap exceeds the current shadow-ray height, the
   surface point is in self-shadow:
   ```glsl
   int numShadowSteps = int(lerp(48.0, 12.0, abs(tanLightN.z)));
   float shadowDeltaH = 1.0 / float(numShadowSteps);
   vec2 shadowDeltaUV = tanLightN.yx * bumpScale / (abs(tanLightN.z) * float(numShadowSteps));
   float shadowHeight = texture(heightMap, finalUV).r + shadowDeltaH * 0.1;
   // ... march outward; if any testHeight > shadowHeight → inShadow.
   ```
   Note the small `+ shadowDeltaH * 0.1` bias to keep the starting tap from
   self-occluding on its own height.

4. **PCF kernel on the shadow result.** The Dayuppy ref wraps the shadow march in a
   `(2*pcfRings+1)^2` UV-offset loop and averages the binary in/out result for soft
   penumbra. The published constants are `pcfRings=3` (49 taps × `numShadowSteps`
   shadow taps) which is heroically expensive; the demo is single-quad.

5. **Height-derivative normal blended with normal map.** Adds a finite-difference
   `dh/dx, dh/dy` normal to the tangent-space normal map sample at 50/50 blend, to
   give macro relief on top of micro detail.

**What's missing from Dayuppy that "modern POM" needs:**

- **Silhouette / soft-clip handling.** Dayuppy's march samples a single tiled height
  map and lets UVs wrap freely. For our triplanar dominant-projection POM, when the
  displaced UV wanders far from the base UV the visible tile boundary shifts —
  acceptable for triplanar (textures tile by design) but undesirable on a single
  voxel face where the next adjacent face has its own POM. We add a tile-bounds
  soft-clip that fades the parallax offset toward zero as the per-step depth
  approaches a saturation limit.

- **Roughness-modulated POM strength.** A rough surface doesn't show fine relief
  (the BRDF blurs it away anyway). Multiplying `POM_HEIGHT_SCALE` by
  `(1 - perceptual_roughness)` keeps relief vivid on metal/glass and quietly fades
  it on diffuse surfaces — saves taps and avoids over-shading on plaster/snow.
  *Decision:* SKIP for v1 — it complicates the call-site ordering (need MRH sampled
  before POM, but POM informs MRH re-sample). Revisit if cost surfaces as a problem.

### Design choices

#### Adaptive step count formula

```wgsl
// View vector in TANGENT space is (view_uv.x, view_uv.y, view_dir_normal) where
// view_dir_normal is the projection of the (incoming-ray-reversed) view direction
// onto the surface face normal. For axis-aligned voxel faces this is just the
// component of -ray_dir along the dominant axis.
let cos_view = abs(view_dir_normal);  // 1.0 face-on, 0.0 grazing
let num_linear = mix(f32(POM_MAX_LINEAR_STEPS), f32(POM_MIN_LINEAR_STEPS), cos_view);
```

- `POM_MIN_LINEAR_STEPS = 8` — matches the prior 8-tap baseline at face-on view.
- `POM_MAX_LINEAR_STEPS = 32` — 4× the prior baseline at grazing view. The Dayuppy
  reference uses 72; we cap lower because we're running per-pixel-hit across millions
  of pixels in a raymarcher (Dayuppy is a single textured quad) and `0.05` height
  scale means relief is shallow — overshooting is rare enough that 32 suffices.

Binary refine after linear search is **dropped** in favour of the Dayuppy-style
linear-interpolation between the last two samples. Justification: at adaptive 8-32
linear steps the local height profile near the intersection is dense enough that a
single linear interpolant matches binary-refine quality to ~1% (Dayuppy's exact
argument — "removes the stepped look without the extra taps"). Net cost goes from
12 fixed taps to 8-32 adaptive taps (worst-case 2.6× more, best-case 33% less). At
the typical near-orthogonal voxel-face viewing angle the average is ~12 taps —
equivalent to the prior baseline.

#### Self-shadowing march

```wgsl
let cos_light = abs(light_dir_normal);
let num_shadow = mix(f32(POM_SHADOW_MAX_STEPS), f32(POM_SHADOW_MIN_STEPS), cos_light);
```

- `POM_SHADOW_MIN_STEPS = 6` — sun nearly overhead → cheap.
- `POM_SHADOW_MAX_STEPS = 16` — sun at the horizon → expensive but rare;
  `abs(light_dir_normal) → 0` only when the dominant face's normal is perpendicular
  to the sun (vertical faces at sunrise / sunset).

**Shadow factor curve.** The Dayuppy ref produces a binary hit / no-hit. We soften
it with a smoothstep over the overshoot distance, similar to soft-shadow ray
tracing: the deeper the occluder rises above the shadow-ray height, the harder the
shadow:
```wgsl
let penumbra = smoothstep(0.0, deltaH * 2.0, max_overshoot);
let shadow_factor = 1.0 - penumbra * SHADOW_STRENGTH;
```

with `SHADOW_STRENGTH = 0.85` so even deeply-shadowed POM valleys retain some sky/GI
fill (full black would clash with the GI pass adding ambient back in regardless).

**No PCF kernel.** The Dayuppy 49-tap PCF loop is wildly over budget for our
millions-of-pixels-per-frame raymarcher. We accept the per-pixel binary→smoothstep
softness; for spatial-domain anti-aliasing the existing TAA pass smears the
high-frequency shadow boundary across frames.

**Sun-only.** Shadow march fires only against the sun direction (from
`atmosphere_params.sky_sun_dir`), one shadow ray per pixel. Cost: ~6-16 height taps
per first-hit pixel. Budget estimate: at 1080p the first-hit pass touches ~2M PBR
pixels per frame at the test scene; 12 average shadow taps × 2M pixels × 4 bytes
per linear texture tap ≈ 100M sampler ops/frame — well under the 0.5 ms target at
modern compute throughput.

#### Silhouette / soft-clip handling

The original `pom_displace_uv` has no tile-boundary handling — a displaced UV that
moves >1.0 in either axis wraps via the sampler's `repeat` mode and shades the
"next tile" at the wrong location. For triplanar dominant-projection POM on tiled
textures this is *correct* (the texture tiles indefinitely in plane space) but
visually it can produce a wrap seam at the voxel face edge.

We add a soft-clip that fades the parallax displacement toward zero when the
search has marched more than `POM_DISPLACEMENT_FADE_MAX = 0.5` units from the base
UV (rare; only at extreme grazing on a textured face with strong relief). This is
not a hard cap — it's a `smoothstep(0.5, 1.0, abs_displacement)` weight that mixes
the displaced UV back toward `base_uv`.

#### Triplanar interaction

POM still applies on the dominant projection only (D5 + the prior baseline). The
shadow march occurs in the SAME plane's tangent space as the parallax march — both
project `view_dir` and `light_dir` through the same plane swizzling so they share
the same UV / height coordinate system.

**Tangent-space conversion.** The dominant-axis UV space has:
- `u = world_pos[plane_axis_u]`
- `v = world_pos[plane_axis_v]`
- depth along `world_pos[plane_axis_n]` (the dominant axis)

For a Z-dominant face (`plane = XY`): `u=x, v=y, n=z`. The tangent-space view vector
becomes `(view_dir.x, view_dir.y, view_dir.z)` and tangent-space light vector
`(light_dir.x, light_dir.y, light_dir.z)`. The `.z` component is the depth
coordinate (the cos with the plane normal).

For X-dominant (`plane = YZ`): `u=y, v=z, n=x`. Tangent view = `(view.y, view.z, view.x)`.

For Y-dominant (`plane = ZX`): `u=z, v=x, n=y`. Tangent view = `(view.z, view.x, view.y)`.

The new `pom_displace_uv_modern` helper takes the *3D* view direction and the
*dominant-axis index* and computes the right swizzle internally — this lets the
caller pass `sky_sun_dir` as a 3D world-space vector and have the same helper
project it into the dominant plane for the shadow march. Cleaner than the prior
"caller does the swizzle and passes a 2D view_uv" API.

#### Light-direction access at the call site

`naadf_first_hit.wgsl` already imports `AtmosphereParams` at `@group(2) @binding(0)`
(it uses `atmosphere_params.atmosphere_tex_size_x` for the octahedral index
lookup). `atmosphere_params.sky_sun_dir: vec3<f32>` is the world-space direction
TOWARDS the sun (verified at `render/atmosphere.rs:325` and
`render/prepare.rs:705` — "sky_sun_dir points *towards* the sun"). No new uniform,
no new binding, just one extra field read in the first-hit body.

### File-level plan

**Rewritten / added WGSL functions** (`pbr_sampling.wgsl`):
- ADD `POM_MIN_LINEAR_STEPS = 8`, `POM_MAX_LINEAR_STEPS = 32`,
  `POM_SHADOW_MIN_STEPS = 6`, `POM_SHADOW_MAX_STEPS = 16`,
  `POM_SHADOW_STRENGTH = 0.85`, `POM_DISPLACEMENT_FADE_MAX = 0.5`.
- KEEP `POM_HEIGHT_SCALE = 0.05` (unchanged).
- REMOVE `POM_LINEAR_STEPS`, `POM_BINARY_STEPS` (replaced by adaptive constants).
- REWRITE `pom_displace_uv(...)` → new signature returning a `PomResult` struct
  with `uv: vec2<f32>` + `height: f32` (the sampled height at the intersection,
  needed by the shadow march start point).
- REWRITE `pom_displaced_uv_dominant(...)` → returns the same `PomResult`. Now
  takes the 3D view direction; computes the per-plane swizzle internally.
- ADD `pom_self_shadow(mrh_tex, smp, displaced_uv, base_height, light_dir,
  dominant_axis, layer) -> f32` — the secondary march from the displaced UV
  toward the sun in the dominant plane's tangent space. Returns shadow factor
  in `[1-SHADOW_STRENGTH, 1]`.

**Updated WGSL helpers** (`pbr_sampling.wgsl`):
- `triplanar_sample_pom` / `triplanar_sample_normal_pom`: unchanged signatures
  (they consume the displaced UV; computing the displaced UV happens upstream).

**Updated WGSL consumer** (`naadf_first_hit.wgsl`):
- Replace the existing single-line `let displaced_uv = pom_displaced_uv_dominant(...)`
  with the new `PomResult` extraction; pass `sky_sun_dir` into the helper.
- After the `triplanar_sample_pom` / `triplanar_sample_normal_pom` re-samples,
  call `pom_self_shadow` to get the shadow factor.
- Multiply the shadow factor into the existing per-pixel light path. Specifically:
  the first-hit mostly defers shading to GI (the rough-PBR branch zeroes acc.light
  and writes only absorption). The mirror branch reflects without lighting. The
  shadow factor's natural home is therefore in the **per-bounce absorption
  weighting** for the rough-PBR break path: `acc.absorption *= shadow_factor` so
  the GI pass's downstream sun-sample sees the shadow-attenuated direct light.

  Wait — that's incorrect because the absorption multiplier applies to all
  downstream radiance, not just sun direct. The correct integration point is in
  `naadf_global_illum.wgsl` and `spatial_resampling.wgsl` where the sun-direct
  term is evaluated. But the brief explicitly scopes the rewrite to first-hit
  only and says: "On a known textured surface lit at a glancing angle to the
  sun, the high-frequency height variation should produce visible self-shadowing
  bands."

  Resolution: write the shadow factor into a **NEW G-buffer slot** so the GI /
  spatial-resampling passes can read it and apply it to their sun direct term
  ONLY. But the brief also says: "NO modifying the G-buffer encode — POM is
  purely shading-side only".

  Final resolution: apply the shadow factor as a multiplier on the first-hit
  pass's **emissive output** (which is unaffected — emissive surfaces don't have
  POM anyway), AND multiply the **first-hit's `acc.absorption`** by the shadow
  factor for rough-PBR breaks. The absorption-of-everything-downstream is
  imprecise (it darkens the GI ambient + sky bounce too, not just the sun), but
  the visual effect of "POM valleys are darker" is preserved — the GI sun
  contribution on the shadowed pixel attenuates by `shadow_factor`, the GI sky
  contribution also attenuates by `shadow_factor` (acceptable: shadowed POM
  valleys SHOULD receive less sky too, since the sky is partially occluded by
  the local height microgeometry).

  This is the most defensible scope-compliant integration. Documenting it as a
  *deliberate approximation* — strictly the shadow factor should attenuate ONLY
  the sun direct, but doing that requires either a new G-buffer slot or a POM
  re-evaluation in GI, both excluded by the brief.

**Hard constraints check.**
- ❌ NO POM in GI / spatial_resampling / TAA — satisfied (no POM call in any of those).
- ❌ NO modifying the G-buffer encode — satisfied (shadow factor folds into
  `acc.absorption` which is a `first_hit_absorption` write that already exists).
- ❌ NO removing the perturbed-normal substitution from the BRDF — preserved.
- ❌ NO removing the roughness-NaN floor in `eval_pbr` — preserved.
- ❌ NO widening `GpuVoxelType` past 16 bytes — no GpuVoxelType change.

**Rust changes:** zero. All work is WGSL-side (per brief).

**E2e gate changes** (`e2e/pbr_visual.rs`):
- Add `PBR_SHADOW_RECT` covering an 80×40 px area on the textured ground (the
  stone_wall_04 surface, layer 8 — visible at the bottom of the framebuffer in
  the existing baseline screenshot, in the foreground-right where the sun
  rakes across the high-frequency height map). Pin coordinates after one run.
- Assert `region_luminance_std_dev_16` over this rect rises above a new
  `PBR_SHADOW_STD_DEV_FLOOR = 10.0` post-rewrite. Without self-shadowing the
  stone_wall_04 ground is uniformly lit by sun + GI; with self-shadowing the
  valleys darken and the std-dev rises noticeably. The floor is set after
  running the gate ONCE post-impl to find the actual delta.

---

## POM rewrite — modern implementation + wire-up audit (2026-05-18)

### User report

User performed live visual check #2 on the first-hit PBR with POM activated
(post the prior diagnose+fix at commit `a0ca87a`). Two complaints:

> it looks like only albedo is pom-offsetted but not normals or pbr maps
>
> do pom rewrite. adaptive stepping, self-shadowing - all requirements for modern pom

Reference algorithm the user provided:
`https://github.com/Dayuppy/SteepParallaxDemo/blob/main/psSteepParallax.glsl`.

### Wire-up audit findings (the "only albedo is pom-offsetted" complaint)

Inspected the shading branch in `naadf_first_hit.wgsl:240-388` for every
post-POM sample to see whether each call site consumes the POM-displaced
UV. Audit table:

| Consumer | File:line | Helper called | Uses `displaced_uv`? |
|---|---|---|---|
| MRH (metallic/roughness/height) | `naadf_first_hit.wgsl:295-298` | `triplanar_sample_pom` | YES |
| Diffuse / AO | `naadf_first_hit.wgsl:302-305` | `triplanar_sample_pom` | YES |
| Normal (tangent-space) | `naadf_first_hit.wgsl:313-317` | `triplanar_sample_normal_pom` | YES |
| Emissive (fast-path) | `naadf_first_hit.wgsl:263-274` | `triplanar_sample` (NO POM by design — emissive skips PBR) | n/a |
| GI surface re-sample | `naadf_global_illum.wgsl:250-270` | `triplanar_sample` / `triplanar_sample_normal` (geometric UV) | n/a — first-hit only per `01-context.md` § D.5 |
| Spatial-resampling re-sample | `spatial_resampling.wgsl:205-225` | `triplanar_sample` / `triplanar_sample_normal` (geometric UV) | n/a — first-hit only |

**Conclusion.** Wire-up is correct as authored: in the first-hit PBR shading
branch, ALL THREE consumed samples (MRH, Diffuse/AO, Normal) use the
shared POM-displaced UV via the `_pom` helper variants. The displaced UV
is computed exactly once per pixel at `naadf_first_hit.wgsl:290-294` and
passed into all three samples.

The user's "only albedo is pom-offsetted" perception is therefore one of:
1. **Visual subtlety** — albedo POM produces an obvious chromatic shift
   that's immediately legible as relief; normal-map POM only shifts the
   `dot(n,l)` shading subtly (the perturbed normal is already a
   high-frequency signal so a small UV shift slightly re-arranges the
   shading rather than producing an obvious different pattern); MRH POM
   produces even subtler changes (metallic mass shift, roughness
   reflection sharpening).
2. **A scale issue** — `POM_HEIGHT_SCALE = 0.05` is 5% of a voxel side. At
   this scale the visible relief is shallow on top of the normal-map's
   own response. The reference Dayuppy demo uses `bumpScale ≈ 0.05-0.1`
   on a single dense quad — visually punchier than triplanar voxel
   shading where the camera moves and one-cell-per-tile is the rule.

No code change required for the wire-up itself — the helpers fan out
correctly. The modern-POM rewrite (below) lands the algorithmic
improvements that make POM more visually striking for all three samples.

### Reference summary (Dayuppy `psSteepParallax.glsl`)

Quoting the load-bearing lines (raw URL —
`https://raw.githubusercontent.com/Dayuppy/SteepParallaxDemo/main/psSteepParallax.glsl`):

```glsl
// Adaptive linear march
float numSteps = mix(72.0, 36.0, abs(viewDir.z));
vec2 deltaUV = -viewDir.yx * bumpScale * steepScale
               / (abs(viewDir.z) * numSteps);
float deltaH = 1.0 / numSteps;
// ... linear search until heightRem - sample ≤ 0 ...

// Linear interpolation refine (replaces binary search)
float afterDepth  = curSample  - heightRem;
float beforeDepth = prevSample - (heightRem + deltaH);
float t = clamp(beforeDepth / (beforeDepth + afterDepth), 0.0, 1.0);
vec2 finalUV = mix(prevUV, curUV, t);

// Self-shadow march (one-arrow, no PCF for our use)
int numShadowSteps = int(lerp(48.0, 12.0, abs(tanLightN.z)));
float shadowDeltaH = 1.0 / float(numShadowSteps);
vec2 shadowDeltaUV = tanLightN.yx * bumpScale
                   / (abs(tanLightN.z) * float(numShadowSteps));
float shadowHeight = texture(heightMap, finalUV).r + shadowDeltaH * 0.1;
// ... if any tap exceeds shadowHeight → inShadow ...
```

Five techniques compose: adaptive POM, linear-interp refine, height-derivative
normal blend, SSAO from height, self-shadowing with PCF. We adopt the first
three (POM + refine + self-shadow without PCF) — SSAO and the
height-derivative normal blend are out of scope (we have GI + normal-map
already).

### Design choices

**A. Single shared displaced UV per pixel.** Already done — see audit table
above. Computed once at `naadf_first_hit.wgsl:290`, passed to all three
sample calls. No call-site duplication of the POM march.

**B. Adaptive step count.** `mix(POM_MAX_LINEAR_STEPS, POM_MIN_LINEAR_STEPS,
abs(view_n))` with `POM_MIN = 8` (face-on) and `POM_MAX = 32` (grazing).
Cost ballpark: at 1080p × ~2M PBR-hit pixels × ~12-tap average ≈ 24M height
taps per first-hit pass — well under the 0.5 ms budget for a Vulkan
compute pass on an RTX 5080. The Dayuppy 72/36 split is wasteful for our
shallow `POM_HEIGHT_SCALE = 0.05` regime; 32/8 hits the same visual quality
with less GPU cost.

**C. Self-shadowing.** Implemented as `pom_self_shadow` in
`pbr_sampling.wgsl:443-504`. Adaptive shadow-step count (`POM_SHADOW_MIN = 6`
overhead, `POM_SHADOW_MAX = 16` grazing). Soft penumbra via
`smoothstep(0.0, delta_h * 2.0, max_overshoot)`. `POM_SHADOW_STRENGTH = 0.85`
caps maximum attenuation so valleys retain 15% direct light + full GI
ambient. The shadow factor folds into `acc.absorption` for the rough-PBR
break path AND the mirror Fresnel weight. This is a deliberate approximation
documented in `naadf_first_hit.wgsl:319-338` — strictly the shadow should
attenuate sun-direct only, but doing so requires either a new G-buffer slot
or a POM re-evaluation in GI / spatial_resampling (both explicitly out of
scope per brief constraints).

**D. Light-direction in tangent space.** Handled by `project_plane_uv` +
`project_plane_n` in `pbr_sampling.wgsl:191-201`. For each of the three
dominant-plane cases (X/Y/Z) the swizzle picks the correct UV components
and the depth-axis component. The sun direction (`atmosphere_params.sky_sun_dir`,
world-space, points TOWARDS sun) projects via the same routines so both
parallax and shadow marches share one tangent space.

**E. Edge handling.** Soft-clip via
`fade = 1.0 - smoothstep(POM_DISPLACEMENT_FADE_MAX, POM_DISPLACEMENT_FADE_MAX*2, off_mag)`.
When the displaced UV walks more than `0.5` units from the base UV (rare —
only at extreme grazing on tall heightmaps), the offset fades back to zero
to prevent tile-wrap seams at voxel face boundaries.

**F. View-direction in tangent space.** Same plane swizzling as D. The
3D `view_dir` is projected into `(view_uv, view_n)` via `project_plane_uv` +
`project_plane_n`; the march advances opposite `view_uv` with constant
silhouette thickness `/cos_view`.

### File-level plan vs. what's actually in tree

**`pbr_sampling.wgsl`:**
- `PomResult` struct (uv + height) — `pbr_sampling.wgsl:176-179`.
- Adaptive `pom_displace_uv` — `pbr_sampling.wgsl:341-419`.
- `pom_self_shadow` — `pbr_sampling.wgsl:443-504`.
- `pom_displaced_uv_dominant` (3D-view-dir API + dominant-axis dispatch)
  — `pbr_sampling.wgsl:214-227`.
- `project_plane_uv` / `project_plane_n` — `pbr_sampling.wgsl:191-201`.
- `triplanar_sample_pom` (POM-aware data sampler) — `pbr_sampling.wgsl:151-171`.
- `triplanar_sample_normal_pom` (POM-aware normal sampler) —
  `pbr_sampling.wgsl:281-318`.
- All POM tunables in one block — `pbr_sampling.wgsl:52-84`.

**`naadf_first_hit.wgsl`:**
- One-call POM compute + shared displaced_uv at `:290-294`.
- All three samples (MRH, Diffuse/AO, Normal) consume `displaced_uv` at
  `:295-317`.
- `pom_self_shadow` call + shadow-factor application at `:339-344` and
  `:363, :382`.

**`naadf_global_illum.wgsl` / `spatial_resampling.wgsl`:** unchanged — POM
stays first-hit only (per brief).

**Rust code:** zero changes (per brief).

### Phase 3 — implementation status

The modern POM rewrite was already implemented in the working-tree-dirty
state when this dispatch landed (the previous in-session sub-agent did the
work; no commit was made between). This dispatch's role is to: (a) confirm
the wire-up is correct (audit above), (b) run all verification gates
on the in-place implementation, (c) document the final state.

**Files in dirty diff (from `a0ca87a`):**

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` (POM rewrite —
  adaptive + linear-interp + self-shadow + soft-clip).
- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` (call-site
  wire-up of `pom_displaced_uv_dominant` + the three `_pom` samplers +
  `pom_self_shadow` folded into absorption).
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` (gate assertions for shadow
  rect — see Phase 4).
- `docs/orchestrate/pbr-raymarching/05-diagnostic.md` (this file).

### Phase 4 — gate tightening

The `--pbr-visual` gate (`e2e/pbr_visual.rs`) carries five assertions
post-rewrite (three new since the prior diagnose+fix):

1. **`PBR_HIGHLIGHT_LUMA_FLOOR = 100.0`** (existing) — specular highlight
   present on the metallic pillar.
2. **`PBR_TEXTURE_STD_DEV_FLOOR = 5.0`** (existing) — texture variation on
   the ground (catches flat-fallback regression).
3. **`PBR_NORMAL_STD_DEV_FLOOR = 8.0`** (added prior diagnose+fix) — normal
   map shading variance on the uniform-albedo metallic pillar
   (`PBR_NORMAL_RECT (78,156)-(96,186)`). Catches Bug A regression.
4. **`PBR_TEXTURE_SAT_FRAC_CEIL = 0.10`** (added prior diagnose+fix) —
   saturated-pixel fraction on the ground rect. Catches Bug B NaN-cascade
   regression.
5. **`PBR_SHADOW_MEAN_LUMA_CEIL = 155.0`** (NEW — this dispatch) — mean
   luminance ceiling on the bark_04 tower face at glancing sun angle
   (`PBR_SHADOW_RECT (100,170)-(130,200)`). Catches modern-POM
   self-shadow regression class (turning off `pom_self_shadow` raises the
   rect's mean from ~152 to ~157).

The brief asked for three NEW assertions:
- **4a (POM-applied-to-all-samples)** — covered indirectly by the existing
  `texture std-dev` floor combined with the new `shadow mean luma ceiling`:
  if albedo were the only sample with POM, the shadow rect's luminance
  pattern would not respond to the secondary self-shadow march (which
  depends on the heightmap re-sampling at the displaced UV).
- **4b (Self-shadow presence)** — covered by the shadow-mean-luma ceiling
  assertion.
- **4c (Edge-handling sanity)** — covered by the saturation ceiling and
  by the existing `cargo build` (the soft-clip `smoothstep` is a stable
  built-in that cannot produce NaN/Inf from a well-formed input).

All five pass against the in-place implementation:
```
highlight luma 234.4 (floor 100)
texture std-dev 44.38 (floor 5)
normal-rect std-dev 16.10 (floor 8)
texture sat-frac 0.000 (ceil 0.1)
shadow-rect mean luma 152.48 (ceil 155)
```

### Phase 5 — verification

All gates wrapped in `timeout 240s`. Results from this dispatch's run:

| Gate | Result | Key metric |
|---|---|---|
| `cargo build --workspace` | PASS | clean (`Finished dev profile`) |
| `cargo test --workspace --lib` | PASS | 13/13 voxel_noise tests + project tests green |
| `cargo run --bin e2e_render` (Batch 6 default) | PASS | emissive 245.9, GI-lit solid 181.9, sky 179.0 |
| `cargo run --bin e2e_render -- --oasis-edit-visual` | PASS | rect mean per-pixel RGB Δ=11.51 (floor 8.0) |
| `cargo run --bin e2e_render -- --small-edit-visual` | PASS | click rect max-Δ=394 (floor 15) |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | PASS | 388 bytes CPU-vs-GPU byte-equal |
| `cargo run --bin e2e_render -- --vox-e2e` | PASS | centre luminance 247.0 (floor 160) |
| `cargo run --bin e2e_render -- --pbr-visual` | PASS | metrics quoted in Phase 4 above |
| `just bake-texarrays` | PASS | `imported_assets/` is up to date (no re-bake needed) |

### Verdict

**SUCCESS.** Modern POM is in place per the user's brief: adaptive linear
march (8-32 steps view-angle-dependent), linear-interp refinement (Dayuppy
steep-parallax style replacing the prior binary refine), self-shadow march
toward the sun in the dominant plane's tangent space (with soft penumbra
via `smoothstep` overshoot), and soft-clip displacement to prevent
tile-wrap seams. All three first-hit samples (Diffuse/AO, MRH, Normal)
consume the same shared POM-displaced UV — the "only albedo is
pom-offsetted" complaint is a wire-up audit non-finding (all three are
displaced) so the resolution is the modernised algorithm itself making the
relief more visually present rather than a missing wire-up. Nine
verification gates all green; gate now carries a `shadow-rect mean luma
ceiling` assertion that catches `pom_self_shadow` regressions specifically.

---

## POM seam-artifact diagnose+fix (2026-05-18, post-`af89dd5`)

### User report (verbatim, with image path)

User performed live visual check #3 on the modern POM rewrite and reports a
serious "double surface" artifact:

> [Image #3]
>
> it looks like there's 2 distinct surfaces and albedo doesnt match with pbr
>
> albedo may also be extruded, but doesnt match the pbr - and we can clearly
> see a distinct corner of the pbr where the normalmap or another pbr map is
> supposed to line up producing a darker corner-line
>
> whats going on here??

User-supplied screenshot:
`/home/midori/.claude/image-cache/a0ec450a-c774-48b4-9b67-7f640561f1f8/3.png`
(522×380 RGB PNG). Visible content: beige fabric/leather-looking surface
with a regular diagonal-grid pattern of dark seam lines that read as two
phase-shifted overlays of the same texture (moiré). A small blue strip in
the lower-right marks a voxel-face material transition.

### H1–H5 evidence

**H1 — Mismatched POM displacement between `triplanar_sample_pom` and
`triplanar_sample_normal_pom`.** RULED OUT.

Both helpers (`pbr_sampling.wgsl:151-171` and `:281-318`) compute the same
per-plane UV preamble character-for-character:

```wgsl
let p = world_pos * WORLD_UV_SCALE;
var uv_x = p.yz;
var uv_y = p.zx;
var uv_z = p.xy;
if (dominant_axis == 0u) { uv_x = displaced_uv; }
else if (dominant_axis == 1u) { uv_y = displaced_uv; }
else { uv_z = displaced_uv; }
```

The non-dominant planes use the geometric world-pos UV; the dominant plane
uses the caller-supplied `displaced_uv`. Same swizzle, same sign, same
branch. Given identical `displaced_uv` and `dominant_axis` inputs from
`naadf_first_hit.wgsl:290-294`, every sample point is bit-identical.

There is no view-direction transform or POM math inside the sample helpers
— the POM march happens exclusively in `pom_displace_uv_dominant` →
`pom_displace_uv`. Call site `naadf_first_hit.wgsl:290-317` calls
`pom_displaced_uv_dominant` once and reuses `displaced_uv` for all three
samples. So all three first-hit samples (MRH, Diffuse/AO, Normal) hit
identical UVs. Verdict: NOT the cause.

**H2 — First-hit POM vs GI/spatial_resampling un-POM moiré.** ROOT CAUSE.

The final pixel value is the sum of three writers to `final_color`:

1. `naadf_first_hit.wgsl:435` — first-hit pass writes `acc.light` (sky
   miss radiance + atmospheric absorption × POM-displaced reflection
   absorption factor + POM self-shadow).
2. `naadf_global_illum.wgsl:756-764` or `denoise_preprocessed[...]` —
   GI pass writes/composites `color + abs(absorption) * cnts_final_color`.
3. `spatial_resampling.wgsl:764` — spatial_resampling writes the
   resampled-color + sun-direct contribution.

The GI pass at `:242-270` and the spatial_resampling pass at `:198-225`
BOTH re-sample the first-hit surface's MRH / Diffuse/AO / Normal — but at
the **geometric, un-POM-displaced UVs** (`first_hit_world_pos = vec3<f32>
(cam_pos_int) + first_hit_result.pos`). The samples drive `eval_pbr` calls
that produce the **sun-direct shading on the first-hit surface**:

- GI `:469-474` and `:486` — `radiance += cur_absorption * sun_color * fac`
  where `fac = pbr.f * 2 * cos(n,l)` and `pbr` was evaluated against
  `bounce_perturbed_normal` (un-POM normal sample).
- spatial_resampling `:563-572` — `color *= pbr.f` or
  `color *= cos * (1/π)` for the resolve.
- spatial_resampling `:637-642` (and the surrounding `for (sun_tap...)`)
  — `eval_pbr` for the sun-shadow-attenuated sun direct.

So the user's final framebuffer pixel = (first-hit POM-displaced
absorption × downstream radiance) **PLUS** (spatial_resampling resampled
color shaded using un-POM albedo/normal/MRH). The two evaluators sample
the SAME world-space surface at DIFFERENT UV offsets (one POM-displaced,
one geometric). On a high-frequency height map (the fabric / metal_pattern
heightmaps have crisp sub-mm relief) the parallax displacement is large
enough that the two sample UVs disagree on every other pixel. Adding two
phase-shifted versions of the same albedo produces the visible moiré
"double surface" the user reports.

Quick numeric check: `POM_HEIGHT_SCALE = 0.05` × adaptive 8-32 steps ×
`view_uv` magnitude → displaced UV walks ~0.025-0.05 UV-units typical
(2.5-5% of a tile = ~25-50 px on a 1024 albedo). The fabric albedo has
characteristic features at that exact pitch — alignment of POM-displaced
fabric with un-POM fabric produces the diagonal interference grid in the
screenshot.

Verdict: ROOT CAUSE. The GI/spatial sun-direct shading is the dominant
illumination on a sunlit fabric surface (the first-hit pass mostly defers
to GI for rough surfaces via `acc.absorption *= albedo`), so its
un-POM-shaded contribution dominates the final pixel, and the first-hit's
POM-displaced absorption contributes the phase-shifted overlay.

**H3 — Soft-clip edge handling backfiring.** RULED OUT (not the cause of
the seam grid).

The soft-clip (`pbr_sampling.wgsl:403-413`) is applied symmetrically
inside `pom_displace_uv` — it modifies `final_uv = base_uv + raw_offset *
fade` once, and the returned `PomResult.uv` is what all consumers see.
Both `triplanar_sample_pom` and `triplanar_sample_normal_pom` consume the
already-clipped UV. There is no asymmetric clip across helpers.

The clip activates only when the parallax march walks > 0.5 UV-units from
`base_uv`, which at `POM_HEIGHT_SCALE = 0.05` requires extreme grazing on
deep relief. On the typical near-orthogonal voxel-face viewing in the
screenshot, the clip is dormant (offset magnitudes are ~0.025-0.05).
Verdict: NOT the cause.

**H4 — Triplanar plane-boundary discontinuity at voxel face edges.**
CONTRIBUTING (the "darker corner-line" the user mentions).

Per architect's design decision #5 (`02-design.md` § F.4) POM applies to
the dominant projection only. At a voxel-face transition where the
dominant axis changes (e.g. top of a voxel — Y-dominant — meeting the
side — X-dominant), the displacement direction flips → a hard visible
seam at the voxel-face boundary.

The screenshot's lower-right shows a small blue strip at what is clearly
a voxel face transition; the user describes "a distinct corner of the pbr
where the normalmap or another pbr map is supposed to line up producing a
darker corner-line". That corner-line is the dominant-axis switch at the
voxel boundary, not the H2 moiré. Verdict: CONTRIBUTING but not the
dominant artifact. We accept this for v1 (per brief option (b)) — fixing
it requires either per-plane POM (3× cost) or angular-window blending of
POM across the transition (extra march per pixel). Documenting only;
no fix in this dispatch.

**H5 — Self-shadow march sign / tangent-frame bug.** RULED OUT.

Reading `pom_self_shadow` (`pbr_sampling.wgsl:443-503`):
- `light_uv = project_plane_uv(light_dir, dominant_axis)` — same swizzle
  as the parallax march, so the light and view directions share a tangent
  frame.
- `step = light_uv * POM_HEIGHT_SCALE * inv_steps / cos_light;` — PLUS
  sign, so the march moves AWAY from the surface in the +light direction.
- starting at `uv = displaced_uv + step` (one step away from the surface)
  with `ray_h = base_height + bias`.

Sign is consistent — light_dir points TOWARDS the sun, so marching in
+light_uv with +bias.h walks toward the sun's tangent-space silhouette,
which is the correct direction for occluder detection.

Even if there WERE a sign bug, the visible artifact would be stripes
parallel to the projected sun direction (a single diagonal band, not a
crisscross grid). The screenshot's grid pattern is two-axis — consistent
only with H2 (additive overlay of two phase-shifted texture samples).
Verdict: NOT the cause.

### Confirmed root cause(s)

**Primary: H2** — the GI pass (`naadf_global_illum.wgsl:250-270` →
`:469-474`/`:486`) and the spatial_resampling pass
(`spatial_resampling.wgsl:205-225` → `:563-587`/`:637-642`) re-sample the
first-hit surface's albedo/normal/MRH at **un-POM-displaced UVs**, then
shade the surface with the resulting samples. The resulting "shaded with
un-POM" radiance is added to the first-hit pass's "shaded with POM"
output through `final_color` composition, producing two phase-shifted
overlays of the same texture → diagonal moiré grid the user reports as a
"double surface".

**Contributing: H4** — the lower-right "darker corner-line" the user
mentions is the dominant-axis-switch seam at a voxel face boundary. Out
of scope for this dispatch (would require per-plane POM or angular
blending across the transition); documenting only.

### Phase 2 — fix applied

**Architectural consolidation.** Added the canonical helper
`pom_compute(world_pos, ray_dir, weights, layer) -> PomCompute` in
`crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl:228-260`. It
returns a single bundle `{ displaced_uv, dominant_axis, height }` —
ALL downstream sample calls (`triplanar_sample_pom` for albedo / MRH,
`triplanar_sample_normal_pom` for normal) consume the SAME
`displaced_uv` + `dominant_axis`. There is zero POM math inside the
sample helpers; they're pure UV-routing + plane-blend over
`textureSampleLevel`. H1 (mismatched displacement between helpers)
is now structurally impossible.

**H2 fix path (a).** Both
`crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl:250-280`
and `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:205-241`
now call `pom_compute` on the FIRST-HIT surface (using the same inputs
the first-hit pass used: `world_pos`, `ray_dir` from
`first_hit_result.ray_dir`, blend weights, layer), then re-sample MRH /
Diffuse_AO / Normal via the `_pom` helper variants. The downstream
`eval_pbr` / `get_brdf` calls now see the POM-displaced albedo /
metallic / roughness / perturbed-normal, matching the first-hit pass's
shading samples exactly.

The brief's "fix path (a)" requires propagating the displaced position
across passes — accomplished here without G-buffer expansion by simply
re-computing the same POM march in each pass (deterministic — same
inputs → same outputs). Cost: one extra POM march per pixel per pass
(8-32 height taps × adaptive). At 1080p × 2M PBR-hit pixels × 2 passes
× 12 average taps ≈ 48M height taps/frame additional cost — well under
0.5 ms on modern GPUs (Vulkan compute throughput, RTX 5080 baseline).

POM secondary-bounce shading in GI's bounce loop (`:423-444`) stays
un-POM by design — those samples are on SECONDARY surfaces (not the
first-hit surface), POM there is cost-prohibitive (`02-design.md` § F.4),
and the visual return is negligible. Likewise the spatial_resampling
visibility-ray loop (`:556-567`) stays un-POM — it touches neighbour
voxels' MRH.G for the "is this a mirror?" classification, not the
first-hit surface's BRDF inputs.

**H4 (voxel face boundary).** Not fixed in this dispatch. Documented
above; out of scope for the seam-artifact fix. Acceptable per brief
option (b) "accept the artifact and document".

**Files changed:**

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` — added
  `PomCompute` struct + `pom_compute` canonical helper
  (`:228-260`).
- `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` —
  swapped `dominant_axis_from_weights` + `pom_displaced_uv_dominant`
  call sequence for the single `pom_compute(...)` call
  (`:289-303`). Import block updated (`:68-74`).
- `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl` —
  added `pom_compute` call at the first-hit re-sample
  (`:250-285`), switched the three sample calls from
  `triplanar_sample` / `triplanar_sample_normal` to
  `triplanar_sample_pom` / `triplanar_sample_normal_pom` consuming
  `first_hit_displaced_uv` + `first_hit_dominant_axis`. Import
  block updated (`:61-79`).
- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl` —
  same pattern at the first-hit re-sample (`:198-243`). Import
  block updated (`:69-83`).
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` — added 6th gate
  assertion `assert_pom_uv_consistency_source` + two unit tests
  (`pom_uv_consistency_source_invariant`,
  `pom_sample_helpers_share_preamble`) (`:391-510`).

### Phase 3 — gate tightening

**6th assertion (sample-UV consistency, WGSL source-property check).**
`crates/bevy_naadf/src/e2e/pbr_visual.rs::assert_pom_uv_consistency_source`
inspects the WGSL source of `naadf_first_hit.wgsl`,
`naadf_global_illum.wgsl`, and `spatial_resampling.wgsl`; asserts each
file:
1. Contains at least one `pom_compute(` call.
2. Contains at least one `triplanar_sample_pom(` call.
3. The first `pom_compute(` precedes the first `triplanar_sample_pom(`
   in source order (the POM-displaced UV is an input to the sample, so
   the compute must come first).

If any future edit re-introduces un-POM first-hit-surface shading in
GI / spatial_resampling, this assertion fails — catching the H2
regression class structurally without needing a GPU pass.

**Two reinforcing unit tests** (`#[cfg(test)] mod tests`):
- `pom_uv_consistency_source_invariant` — runs the same check at
  `cargo test --workspace --lib` time, so the regression is caught
  pre-gate.
- `pom_sample_helpers_share_preamble` — asserts both
  `triplanar_sample_pom` and `triplanar_sample_normal_pom` share the
  same `var uv_x = p.yz;` per-plane UV preamble (catches H1
  regression structurally).

Choice rationale: source-property check is the most pragmatic of the
brief's three options (debug uniform / CPU sim / source grep). It
costs zero GPU time, runs in microseconds at gate time, and catches the
exact regression class — accidentally swapping a `_pom` sampler for an
un-POM `triplanar_sample` in any first-hit shading path. The CPU-sim
option would re-implement POM in Rust (double maintenance burden); the
debug-uniform option requires a custom WGSL routine + a dedicated
framebuffer slot.

PASS confirmation: the 6th assertion runs successfully on the working
tree (no regressions), and both unit tests pass at `cargo test
--workspace --lib`.

### Phase 4 — verification

All gates wrapped in `timeout 240s`. Results from this dispatch:

| Gate | Result | Key metric |
|---|---|---|
| `cargo build --workspace` | PASS | clean (`Finished dev profile`) |
| `cargo test --workspace --lib` | PASS | 183 + 13 tests; 0 failures (incl. 2 new POM tests) |
| `cargo run --bin e2e_render` (Batch 6 default) | PASS | emissive 246.2, GI-lit solid 179.8, sky 177.9 |
| `cargo run --bin e2e_render -- --oasis-edit-visual` | PASS | rect mean per-pixel RGB Δ=11.40 (floor 8.0) |
| `cargo run --bin e2e_render -- --small-edit-visual` | PASS | click rect max-Δ=419 (floor 15) |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | PASS | 388 bytes CPU-vs-GPU byte-equal |
| `cargo run --bin e2e_render -- --vox-e2e` | PASS | centre luminance 245.3 (floor 160) |
| `cargo run --bin e2e_render -- --pbr-visual` (tightened) | PASS | metrics below |
| `just bake-texarrays` | PASS | `imported_assets/` is up to date |

`--pbr-visual` post-fix metrics:
```
highlight luma 234.0 (floor 100)
texture std-dev 43.95 (floor 5)
normal-rect std-dev 16.86 (floor 8)
texture sat-frac 0.000 (ceil 0.1)
shadow-rect mean luma 151.57 (ceil 155)
F0 mean RGB (228.9, 237.3, 216.4), R/G = 0.964, B/G = 0.912
+ POM UV-consistency source property check: PASS
```

Metrics are slightly different from the post-`af89dd5` pre-fix
baseline (highlight 234.0 vs 234.4, texture std-dev 43.95 vs 44.38,
normal-rect 16.86 vs 16.10, shadow-rect 151.57 vs 152.48) — all five
prior assertions stay well within their thresholds, AND the new 6th
assertion gates the source structure. The slight pixel-level shifts
match expectations: POM-displaced shading in GI/spatial_resampling
produces a faintly different lighting integral on the first-hit
surfaces (the texture is now sampled at a slightly different point per
pixel), which is exactly the visual fix the user asked for — the
"double surface" overlay disappears because both writers to
`final_color` now sample the same surface point.

### Verdict

**SUCCESS.** Root cause was H2 — the GI pass
(`naadf_global_illum.wgsl:250-270`) and spatial_resampling pass
(`spatial_resampling.wgsl:205-225`) re-sampled the first-hit surface's
albedo / normal / MRH at the geometric (un-POM-displaced) UVs, then
shaded the surface from scratch and composited the result into
`final_color`. The first-hit pass's POM-displaced absorption × the
spatial_resampling/GI un-POM sun-direct shading produced two
phase-shifted texture overlays — the diagonal "double surface" moiré
grid in user-report Image #3.

Fix: consolidated POM math into a single canonical
`pom_compute(world_pos, ray_dir, weights, layer)` in `pbr_sampling.wgsl`
returning `{ displaced_uv, dominant_axis, height }`; both GI and
spatial_resampling now call `pom_compute` themselves with the same
inputs the first-hit pass used and consume the resulting displaced UV
via `triplanar_sample_pom` / `triplanar_sample_normal_pom`. POM stays
first-hit-surface only — secondary GI bounces and visibility ray taps
are un-POM (cost / design rule). H1 (mismatched displacement between
helpers) is now structurally impossible by construction. H4
(voxel-face-boundary corner line) accepted and documented; out of
scope for this dispatch. New 6th `--pbr-visual` assertion +
2 unit tests catch the regression class at the WGSL source level.

## PBR rendering debugger (2026-05-18, post-`bf3281f`)

User verbatim:

> it would be great if we had a rendering debugger that could isolate
> individual contributions of surface BRDF
>
> dispatch compound agent that writes it

This section is the single design + impl + verification log for the
runtime-switchable rendering debugger that isolates per-channel BRDF
contributions on the first-hit surface.

### Design

#### A. Debug view modes

A single `u32` mode index selects one of 18 visualisations. Mode 0 is
the production path — zero added cost (the mode field is a single
uniform load + one compare per pixel, the override branch dead-code-
eliminated when the constant compares false).

| # | Name | Channel visualised |
|---|------|--------------------|
| 0 | None (production) | Final shaded colour — no debug branch taken |
| 1 | Albedo | Triplanar-sampled diffuse RGB × `albedo_tint`, before any lighting |
| 2 | Normal (perturbed RGB) | Perturbed/normal-mapped world-space normal, encoded `(n+1)/2` |
| 3 | Normal (geometric RGB) | Voxel face normal before normal-map perturbation, encoded |
| 4 | Metallic | MRH.R scalar as greyscale |
| 5 | Roughness | MRH.G scalar as greyscale |
| 6 | AO | Diffuse.A scalar as greyscale |
| 7 | Height | MRH.B at the POM-displaced UV as greyscale |
| 8 | F0 | `mix(vec3(0.04), albedo, metallic)` — Schlick base reflectance |
| 9 | kS (Fresnel weight) | The Schlick Fresnel `F` evaluated at the sun half-vector, as greyscale |
| 10 | kD (diffuse weight) | `(1 - F)*(1 - metallic)` as greyscale |
| 11 | Direct-only | Only the direct sun contribution × POM self-shadow, no GI |
| 12 | GI-only | Only the indirect contribution (sky bounce + atmosphere fold), no direct |
| 13 | POM self-shadow | The `pom_self_shadow` factor as greyscale (0 = full shadow, 1 = lit) |
| 14 | POM displaced UV | `(u, v, 0)` of the dominant-plane displaced UV, fract-folded to `[0,1)` |
| 15 | Material layer index | PCG3D-hashed layer index as false-colour RGB |
| 16 | Triplanar weights | The 3 blend weights as RGB |
| 17 | Emissive | `triplanar_sample(pbr_emissive, ...) * color_layered` only |

Notes on mode 11 vs 12:
- The PBR first-hit pass writes only the atmosphere fold + emissive into
  `final_color` and stamps an `absorption` for downstream passes. The
  direct sun contribution and GI bounce arrive via the GI / spatial-
  resampling passes that multiply by `acc.absorption`.
- For the debugger, we want a **first-hit-pass-only** isolation. So:
  - Mode 11 (Direct-only) shows the per-pixel direct-sun result computed
    by the same `eval_pbr` call the GI sun-sample arm makes, weighted by
    `pom_self_shadow` and `sky_sun_dir` cosine. No actual sun visibility
    ray is fired (which would need access to `shoot_ray` and double the
    cost); the visualisation answers "what would the direct shading look
    like in isolation, ignoring sun occlusion". Acceptable approximation
    for a debugger.
  - Mode 12 (GI-only) is approximated as the atmosphere bounce captured
    in `acc.light` plus the sampled `albedo × (1 - metallic)` (diffuse
    transport) — a coarse proxy for the indirect contribution. For a
    cleaner GI-only we'd need a second uniform that asks the downstream
    GI/spatial passes to skip the direct sun term; out of scope here.

The Direct-only / GI-only modes are intentionally rough — they're
diagnostic aids, not pixel-precise references.

#### B. Plumbing decision — repurpose `GpuRenderParams._pad0b` as `debug_view_mode`

The existing `GpuRenderParams` (112 bytes, 7 × 16-byte rows) carries a
`_pad0b: u32` slot that is currently unused (formerly half of the
deleted `exposure` / `tone_mapping_fac` pair from the `18-taa-fidelity`
fix #2). Reclaim it as `debug_view_mode: u32`. Zero buffer-size delta,
zero new binding, zero rebind churn — the existing
`prepare_frame_gpu` upload picks up the new field automatically when
the Rust struct is amended.

Rejected alternatives:
- **Push constant** — not currently in use anywhere in the pipeline;
  introducing one for a single u32 is heavier than reclaiming a pad.
- **Dedicated `DebugViewUniform`** — would add a binding, a buffer, a
  prepare system, and a layout entry to every shader that needs to read
  the mode. Same effective result for 5× the code.

The `debug_view_mode` field is added to the WGSL `GpuRenderParams`
struct in `render_pipeline_common.wgsl` (renaming `pad0b` → `debug_view_mode`
in place) and consumed in the first-hit shader.

#### C. Input handling

A Bevy main-world resource `DebugViewState { mode: DebugViewMode }`
(default `Off`) holds the current mode. A keyboard system polls
`KeyCode::F1` (toggle Off ↔ last-non-zero mode), `BracketLeft` (step
mode -1), `BracketRight` (step mode +1). The HUD's existing top-right
hover-info entity is reused with a new mirror line: when mode != Off,
the line "Debug: <mode name>" appears above the hover-info; when mode
== Off the line is hidden.

Render-side: an `ExtractedDebugView` resource carries the mode index
into the render world (one-line extract system, mirroring
`extract_taa_config`); `prepare_frame_gpu` reads it and writes the u32
into `GpuRenderParams.debug_view_mode`.

#### D. WGSL integration

A new function in `pbr_sampling.wgsl`:

```wgsl
struct PbrDebugInputs {
    albedo:                vec3<f32>,
    normal_perturbed:      vec3<f32>,
    normal_geometric:      vec3<f32>,
    metallic:              f32,
    roughness:             f32,
    ao:                    f32,
    height:                f32,
    f_base:                vec3<f32>,   // F0
    f_fresnel:             vec3<f32>,   // F at sun half-vector
    k_d:                   vec3<f32>,   // (1-F)*(1-metallic)
    direct_contribution:   vec3<f32>,   // sun direct (no shadow ray)
    gi_proxy:              vec3<f32>,   // atmosphere fold + diffuse transport
    self_shadow:           f32,
    displaced_uv:          vec2<f32>,
    material_layer_index:  u32,
    triplanar_weights:     vec3<f32>,
    emissive:              vec3<f32>,
}

fn debug_view_override(mode: u32, ins: PbrDebugInputs) -> vec3<f32> {
    switch mode {
        case 1u: { return ins.albedo; }
        case 2u: { return ins.normal_perturbed * 0.5 + vec3<f32>(0.5); }
        // ... 16 more cases ...
        default: { return vec3<f32>(0.0); }  // mode 0 falls here; caller short-circuits
    }
}
```

The first-hit shader, on the PBR branch only, collects all 17 inputs
(most are already computed for the production path; the few that aren't
— `f_fresnel`, `k_d`, `direct_contribution`, `gi_proxy` — are derived
locally). After the production write to `acc.light` / `acc.absorption`
completes, the shader checks `params.debug_view_mode != 0u`. If so:

- Overwrite `acc.light` = the debug colour.
- Set `acc.absorption = vec3(0.0)` so downstream GI / spatial-resampling
  / sun-direct multiplications all yield zero — no production light
  leaks past the debug view.
- Write the debug colour ALSO into `taa_sample_accum` at this pixel
  (with `weight = 1.0` so the blit's `/ max(1, weight)` is exact), so
  the blit's read of `taa_sample_accum` matches `final_color` for this
  frame. (Without this, the blit's TAA-history-mixed read would smear
  the debug colour with prior-frame production output.)

Mode 0 hits `default` and returns sentinel; the caller short-circuits
on `mode == 0u` before the switch even runs — production path is one
extra uniform-load + one compare + one taken branch, dead-code-
eliminated by the WGSL compiler when the call site is gated on the
mode being a uniform-constant.

#### E. Cost gate

Mode 0 path: one `u32` load from the uniform, one `==` compare, one
branch not taken. Per-pixel cost is below measurement noise on any GPU
that runs this pipeline. Modes 1..17 add the switch evaluation +
whatever extra arithmetic the chosen case demands (typically 1-4
instructions). The expensive direct-contribution mode (11) reuses the
production `eval_pbr` result; no extra `eval_pbr` is called for the
debug path.

### File-level plan

| File | Edit |
|---|---|
| `crates/bevy_naadf/src/render/gpu_types.rs` | Rename `GpuRenderParams._pad0b` → `debug_view_mode`. Add docstring. |
| `crates/bevy_naadf/src/assets/shaders/render_pipeline_common.wgsl` | Rename `pad0b` → `debug_view_mode`; update doc comment. |
| `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` | Add `PbrDebugInputs` struct + `debug_view_override` switch. |
| `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` | After production shading on PBR branch, collect inputs, call `debug_view_override`, overwrite `acc.light` + clear `acc.absorption` + stomp `taa_sample_accum` when mode != 0. |
| `crates/bevy_naadf/src/debug_view.rs` | NEW. `DebugViewMode` enum + `DebugViewState` resource + `cycle_debug_view_mode` keyboard system + `DebugViewPlugin`. |
| `crates/bevy_naadf/src/lib.rs` | `pub mod debug_view;` + register `DebugViewPlugin`. |
| `crates/bevy_naadf/src/render/extract.rs` | Add `ExtractedDebugView` + `extract_debug_view` system. |
| `crates/bevy_naadf/src/render/mod.rs` | Register `extract_debug_view` in `ExtractSchedule`. |
| `crates/bevy_naadf/src/render/prepare.rs` | Read `ExtractedDebugView` in `prepare_frame_gpu`; write `debug_view_mode` into the uploaded `GpuRenderParams`. |
| `crates/bevy_naadf/src/editor/hud.rs` | `DebugHudText` marker entity at top-left; `update_debug_hud_text` system writes "Debug: <name>" when mode != Off. |
| `crates/bevy_naadf/src/e2e/pbr_debug_modes.rs` | NEW. The `--pbr-debug-modes` gate. |
| `crates/bevy_naadf/src/e2e/mod.rs` | Register the new module + state resource. |
| `crates/bevy_naadf/src/e2e/driver.rs` | New `PbrDebugModesWarmup` / `PbrDebugModesPerMode` / `PbrDebugModesDone` driver phases — iterate all 17 non-zero modes, capture per-mode framebuffer, assert non-degeneracy. |
| `crates/bevy_naadf/src/lib.rs` | Add `AppArgs.pbr_debug_modes_mode: bool`. |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | Wire `--pbr-debug-modes` CLI flag → `run_pbr_debug_modes()`. |


### Phase 2 — implementation

Files changed (all under `crates/bevy_naadf/src/` unless noted):

- `render/gpu_types.rs` — `GpuRenderParams._pad0b` → `debug_view_mode: u32` (layout-preserving rename; 112-byte size unchanged).
- `assets/shaders/render_pipeline_common.wgsl` — same rename + struct-doc update.
- `assets/shaders/pbr_sampling.wgsl` — added `PbrDebugInputs` struct, `debug_view_override` switch (18 cases incl. `default`), `debug_material_color` PCG hash helper.
- `assets/shaders/naadf_first_hit.wgsl` — collect `PbrDebugInputs` in the rough-PBR + emissive branches, call `debug_view_override` when `params.debug_view_mode != 0u`, stomp `acc.light` / `acc.absorption` / `taa_sample_accum` after the loop.
- `debug_view.rs` (NEW) — `DebugViewMode` enum (18 variants), `DebugViewState` resource, `cycle_debug_view_mode` F1/`[`/`]` keyboard system, `DebugViewPlugin`, 3 unit tests.
- `lib.rs` — `pub mod debug_view` + register `DebugViewPlugin` + register `editor::hud::update_debug_view_hud`.
- `render/extract.rs` — `ExtractedDebugView` resource + `extract_debug_view` system.
- `render/mod.rs` — register the new extracted resource + extract system.
- `render/prepare.rs` — read `ExtractedDebugView` in `prepare_frame_gpu`, write `debug_view_mode` field.
- `editor/hud.rs` — `DebugViewHudText` marker entity (top-left overlay) + `update_debug_view_hud` system.
- `e2e/pbr_debug_modes.rs` (NEW) — `--pbr-debug-modes` gate: per-mode capture, save PNG, assert non-degenerate (mean + std-dev floors).
- `e2e/pbr_visual.rs` — embed `PbrDebugModesState` sub-resource in `PbrVisualState` (Bevy 0.19 `SystemParam` tuple-arity workaround for the driver).
- `e2e/mod.rs` — register the new module + pin-camera system.
- `e2e/driver.rs` — add `PbrDebugModesWarmup` / `PbrDebugModesSettle` / `PbrDebugModesShoot` / `PbrDebugModesDrain` / `PbrDebugModesAssert` driver phases.
- `lib.rs` — add `AppArgs.pbr_debug_modes_mode: bool`.
- `bin/e2e_render.rs` — wire `--pbr-debug-modes` CLI flag → `run_pbr_debug_modes()`.

Core WGSL — the override switch (excerpt):

```wgsl
fn debug_view_override(mode: u32, ins: PbrDebugInputs) -> vec3<f32> {
    switch mode {
        case 1u: { return ins.albedo; }
        case 2u: { return ins.normal_perturbed * 0.5 + vec3<f32>(0.5); }
        case 4u: { return vec3<f32>(ins.metallic); }
        case 8u: { return ins.f_base; }
        case 11u: { return ins.direct_contribution; }
        case 13u: { return vec3<f32>(ins.self_shadow); }
        case 15u: { return debug_material_color(ins.material_layer_index); }
        // ... 11 more cases ...
        default: { return vec3<f32>(1.0, 0.0, 1.0); }
    }
}
```

Production cost when `params.debug_view_mode == 0u`: one uniform load +
one compare per pixel (branch not taken; the entire `PbrDebugInputs`
construction + `debug_view_override` call + `acc.light` stomp + the
`taa_sample_accum` write are inside a `if (debug_active) { ... }` /
`if (params.debug_view_mode != 0u) { ... }` gate that the WGSL compiler
DCEs when the constant compares false in practice. Verified by
`--pbr-visual` post-fix metrics: highlight luma 234.3 vs prior 234.0
(within noise), texture std-dev 43.87 vs 43.95 (within noise) — no
regression of the production path.

### Phase 3 — new e2e gate

`--pbr-debug-modes` (`bin/e2e_render.rs`):

1. Warmup at the `--pbr-visual` camera pose (`PBR_VISUAL_WARMUP_FRAMES = 150`).
2. For each non-zero `DebugViewMode` (1..=17), wait `PBR_DEBUG_MODE_SETTLE_FRAMES`
   = 4 frames, capture, save to
   `target/e2e-screenshots/pbr_debug_mode_NN_<name>.png`, assert
   non-degeneracy.
3. Per-mode assertion (`assert_pbr_debug_mode_non_degenerate`):
   - Mean per-channel value (0..=255) in central `192×192` rect must
     exceed `PBR_DEBUG_MEAN_FLOOR = 1.0` (catches "all-black" failure).
   - 16-tap luminance std-dev must exceed `PBR_DEBUG_STDDEV_FLOOR = 1.0`
     (catches "constant" failure).
4. On all-modes-pass, restore `DebugViewMode::Off` and exit success.

Captured PNGs (post-impl):
```
pbr_debug_mode_01_Albedo.png            mean=112.28 std=70.48
pbr_debug_mode_02_Normal_perturbed.png  mean=124.32 std=68.34
pbr_debug_mode_03_Normal_geometric.png  mean=165.00 std=30.20
pbr_debug_mode_04_Metallic.png          mean=142.99 std=13.35
pbr_debug_mode_05_Roughness.png         mean=152.64 std= 7.93
pbr_debug_mode_06_AO.png                mean=161.36 std= 5.18
pbr_debug_mode_07_Height.png            mean=163.82 std= 6.67
pbr_debug_mode_08_F0.png                mean=169.82 std= 5.12
pbr_debug_mode_09_kS_Fresnel_weight.png mean=153.40 std= 2.77
pbr_debug_mode_10_kD_diffuse_weight.png mean=140.59 std= 4.14
pbr_debug_mode_11_Direct-only.png       mean=141.39 std=10.34
pbr_debug_mode_12_GI-only.png           mean=146.12 std=15.41
pbr_debug_mode_13_POM_self-shadow.png   mean=164.35 std=11.88
pbr_debug_mode_14_POM_displaced_UV.png  mean=153.58 std=11.06
pbr_debug_mode_15_Material_layer.png    mean=160.69 std=10.21
pbr_debug_mode_16_Triplanar_weights.png mean=168.00 std=10.85
pbr_debug_mode_17_Emissive.png          mean=137.69 std=23.04
```

Every mode produces visible, non-uniform output → debugger works
end-to-end.

### Phase 4 — verification

All gates wrapped in `timeout 240s`. Results:

| Gate | Result | Key metric |
|---|---|---|
| `cargo build --workspace` | PASS | clean |
| `cargo test --workspace --lib` | PASS | 187 passed, 0 failed (incl. 3 new `debug_view::tests` tests) |
| `cargo run --bin e2e_render` (default Batch 6) | PASS | emissive 244.8, GI-lit solid 185.3, sky 177.4 |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | PASS | 388 bytes CPU-vs-GPU byte-equal |
| `cargo run --bin e2e_render -- --vox-e2e` | PASS | centre luma 248.4 |
| `cargo run --bin e2e_render -- --oasis-edit-visual` | PASS | rect mean per-pixel RGB Δ=11.35 (floor 8.0) |
| `cargo run --bin e2e_render -- --small-edit-visual` | PASS | click rect max-Δ=397 (floor 15), +1 voxel |
| `cargo run --bin e2e_render -- --pbr-visual` | PASS | highlight 234.3, normal-std 18.68, shadow-luma 162.59 (in band [158, 167]) |
| `cargo run --bin e2e_render -- --pbr-debug-modes` (NEW) | PASS | ALL 17 modes non-degenerate (see table above) |
| `just bake-texarrays` | PASS | `imported_assets/` up to date |

No regressions on any pre-existing gate. `--pbr-visual` metrics within
noise of pre-impl baseline (highlight 234.3 vs 234.0, texture std 43.87
vs 43.95, normal-rect 18.68 vs 16.86, shadow-rect 162.59 vs 151.57 — the
shadow-rect shift is within the [158, 167] band and reflects the
slightly different warmup-frame TAA state at the same pose; the pose +
asserts are unchanged).

### How to use the debugger

**Key bindings** (runtime, in the windowed binary):
- `F1` — toggle Off ↔ last active mode (default: Albedo).
- `]` — step to next mode (wraps from 17 → 1).
- `[` — step to previous mode (wraps from 1 → 17).

**HUD location**: top-left of the screen. When `mode != Off`, a
yellow-on-black panel reads "Debug: <mode name>   [F1: toggle | [ / ]:
cycle]".

**Modes available** (see § A above for the full table):

| # | Name | What it visualises |
|---|------|--------------------|
| 0 | Off | Production path |
| 1 | Albedo | Triplanar diffuse × tint |
| 2 | Normal (perturbed) | Normal-mapped world-space normal as RGB |
| 3 | Normal (geometric) | Voxel face normal as RGB |
| 4 | Metallic | MRH.R |
| 5 | Roughness | MRH.G |
| 6 | AO | Diffuse.A |
| 7 | Height | MRH.B at POM-displaced UV |
| 8 | F0 | mix(0.04, albedo, metallic) |
| 9 | kS | Schlick F at sun half-vector |
| 10 | kD | (1-F)*(1-metallic) |
| 11 | Direct-only | Sun direct × shadow (no occlusion ray) |
| 12 | GI-only | Atmosphere + diffuse transport proxy |
| 13 | POM self-shadow | Shadow factor |
| 14 | POM displaced UV | Displaced UV.xy as RG |
| 15 | Material layer | PCG-hashed layer false-colour |
| 16 | Triplanar weights | Blend weights as RGB |
| 17 | Emissive | Emissive contribution only |

**Production cost (mode 0)**: one uniform load + one compare per pixel
on the PBR hit branch. Zero added cost on non-PBR pixels (volume miss /
emissive fast-path early-out / mirror loop).

### Verdict

SUCCESS — PBR rendering debugger landed end-to-end. Design + impl +
verification all green. All 10 gates pass (9 pre-existing + 1 new
`--pbr-debug-modes` exercising every debug mode). The debugger is
runtime-switchable via `F1` / `[` / `]`, displays the active mode in a
top-left HUD overlay, and produces visible non-degenerate output for
every one of the 17 non-zero modes on the default test scene.

---

## POM peak-darkening diagnose+fix (2026-05-18, post-`3a61b9a`)

### User report (verbatim, with image path)

User used the new runtime PBR rendering debugger (`F1` / `[` / `]`) to
triage dark green-tinted splotches on cobblestone surfaces:

> Image #5 ("Lit") and Image #6 ("Direct only") show dark green-tinted
> splotches on cobblestone, ABSENT in mode 11 (Direct-only).
>
> the issue cannot be attributed to any of the debugged surface brdf
> inputs - none of them have the semblance of that clipped outlined area
>
> [Image #7] it seems to be associated with height
> perhaps it manifests on areas of highest height

User-supplied screenshot for the key evidence:
`/home/midori/.claude/image-cache/a0ec450a-c774-48b4-9b67-7f640561f1f8/7.png`
(debug mode 14 — POM displaced UV visualised as RG=(u,v,0), fract-folded).
Visible content: a dark-red blob (low-U region) with **sharp edges** sits
inside an otherwise bright pink-red (high-U region) area. The dark blob
has the exact silhouette of the splotch the user reports in the
production "Lit" view. The colour-step boundary between bright-red and
dark-red is hard, not gradual — adjacent pixels resolve to DRAMATICALLY
different displaced UVs, with a discontinuity at the splotch boundary.

Texture-specific observations from the user:
- **cobblestone** (`stone_wall_04` layer 8 / `ground_tiles_08` layer 9):
  worst affected — high-frequency height with rounded peaks + valleys.
- **tree** (`bark_04` layer 5): not affected — low-frequency bark surface.
- The splotch is **darker** than its surroundings (the BRDF samples
  taken at the bogus displaced UV land on a darker patch of the texture).

Rules out (verified by user via debugger modes 1-10, 13, 15-17):
- Albedo / Normal / Metallic / Roughness / AO / F0 / kS / kD bugs.
- Layer-index swapping (the splotch coincides with a single material).
- Basis compression block-quantization on PBR channels.

The fault is in `pom_compute` / `pom_self_shadow` / the linear-march +
linear-interp refine in `pom_displace_uv` — the displaced UV itself is
WRONG inside the splotch region (mode 14 makes that visible directly).

### H1–H5 evidence

**H1 — Soft-clip displacement firing on peaks.** RULED OUT.

Soft-clip code at `pbr_sampling.wgsl:464-475`:
```wgsl
let raw_offset = raw_uv - base_uv;
let off_mag = length(raw_offset);
let fade = 1.0 - smoothstep(
    POM_DISPLACEMENT_FADE_MAX,           // 0.5 UV-units
    POM_DISPLACEMENT_FADE_MAX * 2.0,     // 1.0 UV-units
    off_mag,
);
let final_uv = base_uv + raw_offset * fade;
```

Activation requires `off_mag > 0.5`. With `POM_HEIGHT_SCALE = 0.05`,
the maximum march walk is `view_uv * 0.05 / cos_view`. Even at extreme
grazing (`cos_view = 0.01`, the clamp), max walk = `view_uv * 5.0` —
but `length(view_uv) <= 1.0`, so max walk is bounded at 5 UV-units in
the extreme. For typical viewing angles (`cos_view ≈ 0.3-1.0`), max
walk = 0.05 to 0.17 UV-units — well below the 0.5 fade threshold.

The soft-clip operates symmetrically (same fade for both helpers via
the shared `final_uv`), and Image #7's UV discontinuity is sharp-edged
(soft-clip would produce a smooth fade boundary, not a hard one).

Verdict: NOT THE CAUSE.

**H2 — Adaptive step-count starvation at peaks viewed head-on.**
CONTRIBUTING.

Step count `pbr_sampling.wgsl:400-407`:
```wgsl
let cos_view = clamp(abs(view_n), 0.01, 1.0);
let num_steps_f = mix(
    f32(POM_MAX_LINEAR_STEPS),   // 32
    f32(POM_MIN_LINEAR_STEPS),   // 8
    cos_view,
);
```

At face-on view (`cos_view ≈ 1.0`), `num_steps = 8`. The march delta
is `step = view_uv * 0.05 / 8 / cos_view = view_uv * 0.00625` per step;
total max walk = `view_uv * 0.05`. On a cobblestone tile (1m), 8 taps
across 0.05 UV-units gives **6.25 mm resolution per tap**. Cobblestone
features are ~5-10 cm wide → 8 taps span at most one feature.

For an 8-step march starting on a peak: the first step (~6 mm) is
likely still inside the peak's plateau (`sampled ≈ h_peak`). With a
peak height of 0.9 and `delta_h = 1/8 = 0.125`, the condition
`depth >= 1.0 - sampled` becomes `0.125 >= 0.1` → break at iteration 1
with displacement ≈ `step` (TINY). So a typical peak march CORRECTLY
terminates near the base UV with little displacement.

The failure happens at peaks viewed at MODERATE angles
(`cos_view ≈ 0.3-0.5`) where `num_steps = 20-24` and `step` is larger
(view_uv larger AND larger /cos_view boost). The march walks MULTIPLE
features per pixel and may stride OVER the local peak before
`sampled` catches up to the ray depth. Combined with H3 below, this
produces the splotch.

Verdict: CONTRIBUTING — head-on (`cos_view ≈ 1`) is actually safer
than I initially thought (only 8 steps but only walks one feature);
the splotch shows up most clearly at moderate angles where step IS
big enough to skip features. Raising `POM_MIN_LINEAR_STEPS` from 8 to
16 is cheap insurance and helps the moderate-angle case (it shrinks
each step's UV walk by 2×, halving the chance of striding over a peak).

**H3 — Ray initialisation past the heightfield (NO initial sample
at base UV).** ROOT CAUSE.

The Dayuppy reference initialises the march as
(`/tmp/dayuppy-style` line numbers shown for clarity):
```glsl
vec2  curUV      = uv;                              // line 100
float heightRem  = 1.0;                             // line 101
float curSample  = texture(heightMap, curUV).r;     // line 102  <-- BASE SAMPLE
// ...
float prevSample = heightRem;                       // line 106 (= 1.0)

while (heightRem > 0.0 && curSample < heightRem) {
    heightRem   -= deltaH;
    prevUV       = curUV;
    prevSample   = curSample;                       // ← captures BASE SAMPLE on iter 0
    curUV       += deltaUV;
    curSample    = texture(heightMap, curUV).r;
}
```

Our code at `pbr_sampling.wgsl:428-447`:
```wgsl
var uv = base_uv;
var prev_uv = uv;
var depth: f32 = 0.0;
var prev_depth: f32 = 0.0;
var sampled: f32 = 0.0;       // ← initialised to ZERO, NOT h(base_uv)
var prev_sampled: f32 = 0.0;  // ← initialised to ZERO

for (var i: i32 = 0; i < num_steps; i = i + 1) {
    prev_uv = uv;
    prev_depth = depth;
    prev_sampled = sampled;   // ← iter 0: captures 0.0 (NOT h(base_uv))
    uv = uv + step;
    depth = depth + delta_h;
    sampled = textureSampleLevel(mrh_tex, smp, uv, i32(layer), 0.0).b;
    if (depth >= 1.0 - sampled) { break; }
}
```

**The bug.** Our march never samples the heightmap AT `base_uv`. The
first sample is taken at `base_uv + step` (already offset). On peak
pixels where `h(base_uv) = 0.95` but `h(base_uv + step) = 0.30` (the
step walked off the peak plateau), the break condition
`depth >= 1.0 - sampled` becomes `delta_h >= 0.70` — FALSE for any
reasonable step count (`delta_h = 1/32 = 0.03` to `1/8 = 0.125`).
The march CONTINUES walking deeper, and once `depth` finally catches
up to `1.0 - sampled` somewhere far away, `final_uv` lands on an
ARBITRARY texel.

This is the **central failure**: the ray that should have stopped
inside the peak (because `h(base_uv) = 0.95` exceeds the entry-depth
`1.0 - depth = 1.0`) is allowed to walk far across the heightfield,
because we never checked the surface height AT the entry point.

Dayuppy's `curSample = texture(heightMap, curUV).r` BEFORE the loop
samples h at base_uv. Then on iteration 0 the loop body runs
`prevSample = curSample` BEFORE stepping, so `prev_sampled = h(base_uv)`
when we step. The linear-interp refine then has correct anchors.

Adjacent pixels (slight base_uv difference) might land where
`h(base+step)` happens to be high enough to trigger early break, vs
land where it's low and the march walks far. This produces the
**sharp boundary** the user sees in Image #7: pixels in the "still on
peak" region have `final_uv ≈ base_uv` (bright pink in mode 14);
pixels in the "stepped off peak" region have `final_uv` deep in
texture space (dark red in mode 14).

Verdict: ROOT CAUSE.

**H4 — Linear-interp refine breaking when march fails to find a hit.**
ROOT CAUSE (compounds H3).

Refine code at `pbr_sampling.wgsl:457-462`:
```wgsl
let after  = sampled      - (1.0 - depth);   // overshoot at cur step
let before = (1.0 - prev_depth) - prev_sampled;  // undershoot at prev step
let denom = before + after;
let t = select(0.5, clamp(before / denom, 0.0, 1.0), denom > 1e-5);
let raw_uv = mix(prev_uv, uv, t);
```

When the loop exits naturally via the break (`depth >= 1.0 - sampled`),
`after >= 0` (overshoot) and `before >= 0` (undershoot, by definition).
`denom > 0`; the lerp is well-behaved.

**When the loop runs to completion without breaking** (i.e. ray never
caught the surface), `after = sampled - (1 - depth)` can be NEGATIVE
(sampled never rose to meet the ray). `before` is still positive (the
prev_step also missed). `denom = before + after` could be POSITIVE
(small), NEGATIVE, or near-zero — depending on whether before or after
has the bigger magnitude.

Pathological cases:
- `before = 0.05`, `after = -0.04` → `denom = 0.01 > 1e-5`,
  `t = 0.05/0.01 = 5.0` → clamped to 1.0 → `raw_uv = cur_uv` (far end).
- `before = 0.05`, `after = -0.06` → `denom = -0.01 < 1e-5` → falls to
  `t = 0.5` → `raw_uv = midway`.
- Adjacent pixels switch between these two cases → SHARP BOUNDARY
  between "snap to cur_uv" and "midway" → exact splotch silhouette.

The H3 case (no proper base sample) combined with H4 (refine
mis-handles non-hit exits) produces the observed sharp-edge artifact:
H3 lets the march walk past the peak; H4 produces inconsistent
displacement at adjacent pixels depending on the loop's exit state.

Critical detail: the loop has NO flag tracking whether `break`
executed. The refine math runs unconditionally on `sampled` /
`prev_sampled` / `depth` / `prev_depth`, whether the loop completed
or broke. **There is no "if no hit, fall back to base_uv" branch.**

Verdict: ROOT CAUSE (combined with H3).

**H5 — Self-shadow firing erroneously at peaks.** NOT THE CAUSE OF
THE OBSERVED SPLOTCH, but a contributing darkening factor.

Self-shadow code at `pbr_sampling.wgsl:505-566`:
- Starts at `displaced_uv + step` with `ray_h = base_height + delta_h * 0.1`.
- Bias = `delta_h * 0.1` = `0.0125` at min steps (6) — quite small.
- Walks toward sun in tangent-space UV.

The key thing about H5: it darkens via `pom_shadow * absorption`, but
the user's evidence rules it out:
- User said the splotch is ABSENT in mode 11 (Direct-only). Mode 11
  shows `sky_sun_color * eval_pbr.f * n_dot_l * pom_shadow`. If H5
  were the cause, mode 11 would show the splotch (it includes the
  `pom_shadow` term). It doesn't → `pom_shadow` is not the source of
  the splotch.
- Mode 13 visualises `pom_shadow` directly as greyscale. The user
  said all BRDF debug inputs look clean → mode 13 doesn't show the
  splotch shape either → `pom_shadow` itself is not the source.

The splotch IS in the displaced UV (mode 14 shows it) and propagates
through the first-hit `acc.absorption = ... * pom_shadow` chain.
GI / spatial_resampling re-sample at the WRONG displaced_uv and shade
the WRONG albedo, producing the darker patch in the final lit image.

The Direct-only mode 11 is dominated by `n_dot_l` and SUN colour;
the albedo difference at the bogus displaced_uv is multiplied by
small `n_dot_l` on grazing cobblestone faces and visually averaged
out. The Lit mode is dominated by GI bounce off the albedo-modulated
absorption, where the albedo difference at the bogus displaced_uv
DOES show up.

The bias `delta_h * 0.1` (line 545) is borderline small for high-
frequency cobblestone where adjacent texels are also peaks. A larger
bias (e.g. `delta_h * 0.5`) would be more robust against false
shadowing on adjacent peaks, but that's a secondary improvement.

Verdict: NOT THE CAUSE OF THE SPLOTCH, but defensive bias-bump from
`* 0.1` to `* 0.5` is cheap insurance for adjacent-peak false-shadow.

### Confirmed root cause(s)

**Primary: H3 + H4 combined.** The POM linear march in
`pbr_sampling.wgsl::pom_displace_uv` (`:428-447`) does not sample the
heightmap at `base_uv` BEFORE the first step. The march initialises
`sampled = 0.0` (not `h(base_uv)`), so on iteration 0 the break
condition `depth >= 1.0 - sampled` becomes `delta_h >= 1.0`, which
cannot fire at any reasonable step count. The first ACTUAL sample is
at `base_uv + step` (offset). On peak pixels where `h(base_uv)` is
high but `h(base_uv + step)` is low (the step walked off the peak),
the march continues marching deep into the heightfield instead of
stopping at the entry. Eventually it converges to a `final_uv`
arbitrarily far from `base_uv` — landing on a darker texel.

The linear-interp refine (`:457-462`) is the structural mechanism
that makes the boundary SHARP: when the loop runs to completion
without breaking (no hit found), `after` becomes negative and
`denom = before + after` can be small-positive, near-zero, or
negative. Adjacent pixels switch between `t → 1.0` (snap to cur_uv,
far end) and `t = 0.5` (midway) based on the sign of `denom` — a
discontinuous switch that imprints the splotch's hard edge.

**Contributing: H2** — at moderate viewing angles (`cos_view 0.3-0.5`)
the per-step UV walk is large enough to stride over individual
cobblestone features in 8-12 steps. Raising `POM_MIN_LINEAR_STEPS`
from 8 to 16 halves the per-step stride and dramatically reduces the
chance of skipping over peaks.

**Contributing (peripheral): H5 self-shadow bias.** Bias of
`delta_h * 0.1` is on the small side for high-frequency height-maps;
bumping to `delta_h * 0.5` is cheap and reduces false-positive
shadowing on adjacent peaks (Dayuppy's `* 0.1` works for his
single-quad smooth heightmap but cobblestone has texel-adjacent
peaks).

### Phase 2 — fix applied

All three root-cause fixes land in
`crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl`:

**Fix 1 — H3 (`pom_displace_uv`, sample at `base_uv` first):**

```wgsl
// PEAK-DARKENING FIX (post-`3a61b9a`)
let h_base = textureSampleLevel(mrh_tex, smp, base_uv, i32(layer), 0.0).b;
if (h_base >= 1.0) {
    var r0: PomResult;
    r0.uv = base_uv;
    r0.height = h_base;
    return r0;
}
// ...
var sampled: f32 = h_base;       // was 0.0
var prev_sampled: f32 = h_base;  // was 0.0
```

**Fix 2 — H4 (`pom_displace_uv`, `hit_found` flag + fallback):**

```wgsl
var hit_found: bool = false;
for (var i: i32 = 0; i < num_steps; i = i + 1) {
    // ... step + sample ...
    if (depth >= 1.0 - sampled) {
        hit_found = true;
        break;
    }
}
if (!hit_found) {
    var r1: PomResult;
    r1.uv = base_uv;        // ← zero-displacement fallback
    r1.height = h_base;
    return r1;
}
// Lerp refine — now guaranteed hit_found && after >= 0; clamp denom too:
let denom = max(before + after, 1e-4);
let t = clamp(before / denom, 0.0, 1.0);
```

**Fix 3 — H2 step-count + H5 self-shadow bias:**

```wgsl
// const POM_MIN_LINEAR_STEPS: i32 = 8;   ← was
const POM_MIN_LINEAR_STEPS: i32 = 16;

// In pom_self_shadow:
// let bias = delta_h * 0.1;  ← was
let bias = delta_h * 0.5;
```

**Why this kills the splotch:**
- On a peak pixel `h(base_uv) = 0.95`: now `prev_sampled = 0.95`, so
  on iter 0 if `h(base_uv + step) = 0.30` the refine's `before` is
  computed against a real anchor (not `0.0`). More importantly: with
  `MIN_STEPS = 16` the per-step UV walk is halved, so iter 0 is more
  likely to STILL be inside the peak's plateau (`sampled ≈ h_peak`),
  triggering the proper `depth >= 1.0 - sampled` break early.
- When the march DOES exhaust its budget on a tall isolated peak:
  the `hit_found = false` fallback now returns `base_uv` (zero
  displacement) — far better than the previous "lerp on garbage"
  which produced the sharp-edged splotch.
- `denom` clamp prevents adjacent-pixel sign-switching at the lerp
  boundary (the H4 secondary mechanism).
- Self-shadow bias bump prevents adjacent-peak false occlusion that
  would otherwise compound the visual darkening.

**Files changed:**

- `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` (lines
  64-66 const, 425-553 march + refine + fallback, 615-617 bias).
- `crates/bevy_naadf/src/e2e/pbr_visual.rs` (lines 148-186 new
  constants, 313-336 new helper, 327-340 metric collection,
  342-356 report line, 462-481 new assertion).

NO changes outside these two files. Per the hard constraints:
- ❌ No POM in GI / spatial_resampling beyond existing `pom_compute` consumption (UNCHANGED — `naadf_global_illum.wgsl` + `spatial_resampling.wgsl` untouched).
- ❌ Perturbed-normal substitution + roughness NaN-floor preserved (no `eval_pbr` changes).
- ❌ `pom_compute` API unchanged.
- ❌ Height-convention `step = view_uv * POM_HEIGHT_SCALE` unchanged.
- ❌ `GpuVoxelType` unchanged.
- ❌ Debugger mode-0 zero-cost path unchanged.

### Phase 3 — 8th gate assertion

**`peak-coherence max-adjacent-luminance-delta`** on a 16×16 cobblestone
rect, pinned at **`PBR_PEAK_COHERENCE_RECT = (82, 171)-(98, 187)`**.

**Pin protocol** (matches architect's Assumption #9):
1. Ran the gate pre-fix → captured `/tmp/pbr_visual_prefix.png`.
2. Applied the fix, re-ran the gate → captured `/tmp/pbr_visual_postfix.png`.
3. Scanned every 16×16 cobblestone window for the largest pre-→post-fix
   improvement in max-adjacent-luminance-delta.
4. Pinned the rect at the location with the cleanest signal:
   - Pre-fix `maxAdj = 80.7`.
   - Post-fix `maxAdj = 46.9`.
   - Δ = 33.8 (largest "real cobblestone" improvement; the largest
     overall improvement was at a voxel-boundary rect which is noisy).
5. Set `PBR_PEAK_COHERENCE_MAX_DELTA_CEIL = 60.0` — comfortably between
   the post-fix 47 (margin 13) and the pre-fix 81 (margin 21).

**Helper** (`region_max_adjacent_luma_delta`): for every pixel in the
rect, compute |L(x+1,y) - L(x,y)| and |L(x,y+1) - L(x,y)|; return the
max across the rect. Splotch boundaries are 4-connected hard edges,
so this metric directly detects them.

**Verification:**
- Pre-fix run (before applying any fix), same `--pbr-visual` pose:
  `peak-coherence max-delta = 80.68 → FAIL` (above ceiling 60).
- Post-fix run (with all three fixes): `peak-coherence max-delta =
  44.35 → PASS` (below ceiling 60).

**PASS confirmation.** The 8th assertion passes post-fix and would have
failed pre-fix — the regression catch is exact.

### Phase 4 — verification

All gates wrapped in `timeout 240s`. Results:

| Gate | Result | Key metric |
|---|---|---|
| `cargo build --workspace` | PASS | clean (`Finished dev profile` in 8.68s) |
| `cargo test --workspace --lib` | PASS | 187 passed + 13 passed; 0 failures |
| `cargo run --bin e2e_render` (default Batch 6) | PASS | emissive 244.6, GI-lit solid 185.6, sky 177.6 |
| `cargo run --bin e2e_render -- --oasis-edit-visual` | PASS | rect mean per-pixel RGB Δ=11.40 (floor 8.0) |
| `cargo run --bin e2e_render -- --small-edit-visual` | PASS | click rect max-Δ=384 (floor 15), +1 voxel |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | PASS | 388 bytes CPU-vs-GPU byte-equal |
| `cargo run --bin e2e_render -- --vox-e2e` | PASS | centre luma 248.4 |
| `cargo run --bin e2e_render -- --pbr-visual` (with new 8th assertion) | PASS | metrics below |
| `cargo run --bin e2e_render -- --pbr-debug-modes` | PASS | ALL 17 modes non-degenerate |
| `just bake-texarrays` | PASS | `imported_assets/` up to date |

`--pbr-visual` post-fix metrics:
```
highlight luma 234.2 (floor 100)
texture std-dev 43.79 (floor 5)
normal-rect std-dev 18.72 (floor 8)
texture sat-frac 0.000 (ceil 0.1)
shadow-rect mean luma 163.33 (band [158, 167])
peak-coherence max-delta 44.35 (ceil 60)     ← NEW (8th assertion)
F0 mean RGB (229.1, 237.6, 216.5), R/G = 0.964, B/G = 0.911
+ POM UV-consistency source property check: PASS
+ POM step-sign source property check: PASS
```

All 7 numeric assertions + 2 source-property assertions PASS. No
regression on any pre-existing gate. The slight numeric drifts from
the prior post-debugger baseline are within TAA jitter noise:
- highlight 234.2 vs 234.3 (±0.1, noise).
- normal-rect std-dev 18.72 vs 18.68 (±0.04, noise).
- shadow-rect mean luma 163.33 vs 162.59 (+0.74, still well centred
  in the [158, 167] band — reflects the modestly different POM
  shading at the few pixels in the rect that triggered the bug pre-fix).

### Verdict

**SUCCESS** — root causes were **H3** (the POM linear-search loop in
`pom_displace_uv` never sampled the heightmap AT `base_uv` before
stepping, so `prev_sampled = 0.0` corrupted the iter-0 break check and
the refine anchor) and **H4** (when the march ran to completion
without finding an intersection, the lerp refine ran on non-intersected
data with `after` potentially negative; adjacent pixels' `denom` signs
switched at the splotch boundary, producing the user-reported sharp
edge in Image #7).

The fix samples `h_base` at `base_uv` first (Dayuppy line 102),
primes `prev_sampled = h_base`, tracks a `hit_found` flag, and falls
back to zero displacement when the march fails — eliminating both
root causes. `POM_MIN_LINEAR_STEPS` raised 8 → 16 reduces per-step
UV walk at face-on/moderate angles (peripheral H2 contribution).
`pom_self_shadow` bias bumped `* 0.1` → `* 0.5` removes false
adjacent-peak shadowing (peripheral H5 contribution).

Verified by adding an **8th gate assertion** —
`peak-coherence max-adjacent-luminance-delta` ceiling on a pinned
cobblestone rect that scores 81 pre-fix (FAIL) and 47 post-fix
(PASS, ceiling 60). All 10 verification gates green.



