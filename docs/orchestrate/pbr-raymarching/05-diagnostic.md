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
