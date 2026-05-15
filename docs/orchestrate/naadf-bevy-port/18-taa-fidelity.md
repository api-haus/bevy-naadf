# 18 — TAA Fidelity Diagnosis: why the port's TAA is noisier than the C# reference

## taa-fidelity diagnosis (2026-05-15)

**Author:** delegated read-only diagnosis agent (`/delegate`).
**Scope:** the full TAA + denoiser + GI-accumulation convergence chain of the
Rust/Bevy port (`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/`) measured
against the NAADF C#/MonoGame `WorldRenderBase` reference
(`/mnt/archive4/DEV/NAADF/NAADF/`) and the canonical paper
(`/mnt/archive4/PAPERS/Prepared/ulschmid-2026-naadf-voxel-gi.md`).
**Method:** static line-by-line code comparison + one grounding `cargo run --bin
e2e_render` (passed; screenshot read — see below). No build→run loop.

### Summary — the most likely overall explanation

The port's GI pipeline is structurally faithful — every pass exists, the
render-graph order matches `WorldRenderBase.cs` line-for-line, and the
accumulation/reprojection math in `taa.wgsl` / `sample_refine.wgsl` /
`spatial_resampling.wgsl` / `denoise_split.wgsl` is a faithful port. There is
**no single broken pass**. Instead, "noisier than C# / barely resolves" is the
compound result of **three independent faithfulness/config defects that each
attack convergence**, plus the deliberate 16-deep ring:

1. **The GI sample-generation and spatial-resampling rays are NOT jittered.**
   `naadf_global_illum.wgsl` and `spatial_resampling.wgsl` call `get_ray_dir`
   with a hard `vec2(0.0, 0.0)` offset, where the C# passes the per-frame Halton
   `taaJitter`. `GpuGiParams` has **no jitter field at all**. This is the
   single biggest cause: without per-frame sub-pixel jitter on the GI rays,
   every frame samples the *exact same* sub-pixel point, so the long-term TAA
   has no sub-pixel variation to average — it can only smooth temporal Monte-Carlo
   noise, never resolve sub-pixel detail. NAADF's whole "jitter + TAA consider
   albedo; resampling/denoise work on indirect" design (paper §4) is half-disabled.
2. **The final-blit `exposure` and `toneMappingFac` constants are swapped.** The
   C# defaults are `exposure = 1.0`, `toneMappingFac = 1.5`; the port hard-codes
   `exposure: 1.5`, `tone_mapping_fac: 1.0`. This does not *create* noise but it
   flattens contrast and over-brightens — the e2e screenshot is visibly milky /
   washed-out pastel, which reads as "barely resolves" because low-contrast noise
   is more visible and detail is crushed toward white.
3. **The 16-deep `taaSamples` ring (vs the paper/C# 32) halves the temporal
   averaging window** — a deliberate, paper-sanctioned VRAM lever
   (`design-exploration-qa.md` §6), but it is genuinely "Ours (16 samples)" =
   "slightly noisier" per the paper's own Table 4. It is *a* cause but not *the*
   cause — and crucially it does **not** affect the adaptive spp rate (see the
   ranked analysis).

Fix #1 and #2 are small, high-confidence, and together should bring the port
most of the way to the C# bar. #3 is the explicit VRAM trade and only matters
once #1/#2 are fixed.

Separately, the **black-on-resize** bug has a clear root cause: the screen-space
buffers (`taa_samples`/`taa_sample_accum`/`first_hit_data`/`final_color`/the GI
buffers) are sized from `ExtractedCameraData.viewport_size`, which is derived
from `camera.physical_viewport_size()` with an `.unwrap_or(UVec2::new(1,1))`
fallback — during a window resize that call transiently returns `None` (or a
stale size), so for one or more frames the buffers collapse to 1×1 (or the old
size) while the blit's fullscreen triangle covers the full new-size view target
→ every fragment past the buffer length does an out-of-bounds storage read
(0 in WGSL) → black frame; the freshly zero-cleared `taa_samples` ring then
needs ~16 frames to refill, extending the artifact.

---

## Ranked suspected causes

### 1. GI rays are unjittered — `GpuGiParams` has no jitter field — HIGH confidence, MEDIUM scope

- **Subsystem:** GI sample generation (`globalIllum`) + spatial resampling
  (`spatialResampling`) primary-ray reconstruction.
- **Port behavior:**
  - `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl:206-208` —
    `get_ray_dir(gi_params.inv_view_proj, pixel_pos, screen_width, screen_height,
    vec2<f32>(0.0, 0.0))`.
  - `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:582-584` —
    `get_ray_dir(... , vec2<f32>(0.0, 0.0))`.
  - `crates/bevy_naadf/src/render/gpu_types.rs:404-481` — `GpuGiParams` has
    **no `taa_jitter` field** (it has `cam_pos_*`, `sky_sun_dir`, `sun_color`,
    counters, storage counts, the float knobs, `flags` — no jitter).
  - `crates/bevy_naadf/src/render/gi.rs:323-358` — `prepare_gi` never writes a
    jitter into `gi_params`.
- **C# expected behavior:**
  - `renderGlobalIllum.fx:69` — `getRayDir(invCamMatrix, pixelPos, screenWidth,
    screenHeight, taaJitter)` — **jittered**.
  - `renderSpatialResampling.fx:351` — `getRayDir(invCamMatrix, pixelPos,
    screenWidth, screenHeight, taaJitter)` — **jittered**.
  - `WorldRenderBase.cs:311` / `:389` upload `taaJitter` (the current frame's
    `getJitter(frameCount)` Halton 2-D, `WorldRender.cs:137-140`) into both
    effects' `taaJitter` parameter.
  - Paper §4 (`02-research.md` §1.2.1): "sample positions are jittered (Halton)
    … jitter + TAA consider albedo, resampling + denoising work with indirect
    illumination only."
- **Concrete difference:** the port jitters *only* the first-hit pass
  (`naadf_first_hit.wgsl:113-118` does pass `params.taa_jitter`) but **not** the
  two GI passes. So the GI ray that generates the indirect sample, and the
  spatial-resampling ray that reconstructs the surface for reservoir merging, are
  fired through the *pixel centre* every single frame. `first_hit_data` was
  encoded for a *jittered* ray; `globalIllum` / `spatialResampling` then
  reconstruct `getHitDataFromPlanes` from it with an *un-jittered* ray — a
  per-frame-constant reconstruction, and subtly inconsistent with the G-buffer's
  jittered encoding.
- **Hypothesized impact on noise/convergence:** large. The long-term TAA's job
  is to integrate many *sub-pixel-jittered* samples into a resolved pixel; with
  the GI rays unjittered, all 16 ring entries for a pixel are sampled at the same
  sub-pixel point — the TAA averages noise but cannot anti-alias or resolve
  sub-pixel structure, so edges and fine GI detail stay permanently soft/noisy
  ("barely resolves"). It also weakens the spatial resampling's neighbour
  decorrelation. This is the prime mechanism behind "noisier than C#."
- **Proposed fix direction:** add a `taa_jitter: Vec2` (+ `_pad`) field to
  `GpuGiParams` (mind the `vec3`-then-scalar / `vec4` WGSL layout hazard — put it
  on a clean 16-byte row), have `prepare_gi` write `extracted_history.current_jitter`
  into it, declare it in `gi_params.wgsl`, and pass it as the `get_ray_dir`
  offset in both `naadf_global_illum.wgsl` and `spatial_resampling.wgsl` (exactly
  where the C# passes `taaJitter`). `ray_queue_calc.wgsl` does not need it
  (`rayQueueCalc.fx` does not jitter).
- **Scope:** medium — one new uniform field + 2 shader call-site edits + the
  `prepare_gi` write; the `vec3`-then-scalar layout audit is the only risk.

### 2. `exposure` / `tone_mapping_fac` constants are swapped — HIGH confidence, SMALL scope

- **Subsystem:** final-blit tonemap (`renderFinal`).
- **Port behavior:** `crates/bevy_naadf/src/render/prepare.rs:392,396` —
  `exposure: 1.5` and `tone_mapping_fac: 1.0`.
- **C# expected behavior:** `Settings.cs:36-37` — `public float exposure = 1.0f;`
  and `public float toneMappingFac = 1.5f;`. `WorldRenderBase.cs:435,437` feed
  these into `renderFinal`'s `exposure` / `toneMappingFac`.
- **Concrete difference:** the two values are transposed. In the tonemap
  (`naadf_final.wgsl:69-75` = `renderFinal.fx:53-56`):
  `tv = curColor / (toneMappingFac + curColor)` and
  `colorNormalized = lerp(curColor/(exposure + luminance), tv, tv)`. With
  `toneMappingFac = 1.0` (should be 1.5) the Reinhard knee is lower → highlights
  roll off sooner and the image brightens/flattens; with `exposure = 1.5`
  (should be 1.0) the linear term is dimmer — net result is the washed-out,
  low-contrast pastel look visible in `target/e2e-screenshots/e2e_latest.png`.
- **Hypothesized impact:** does not generate noise, but flattens contrast and
  crushes detail toward white — directly produces the "barely resolves /
  milky" appearance and makes residual noise more visible. A real, easy quality
  win toward the C# bar.
- **Proposed fix direction:** swap them — `exposure: 1.0`, `tone_mapping_fac:
  1.5` in `prepare.rs` (`prepare_frame_gpu`'s `GpuRenderParams` build).
- **Scope:** small — two literal values.

### 3. 16-deep `taaSamples` ring instead of 32 — MEDIUM confidence (it IS a cause, just not the dominant one), MEDIUM scope

- **Subsystem:** the long-term-memory TAA history depth (paper §4.1).
- **Port behavior:**
  `crates/bevy_naadf/src/assets/shaders/taa_common.wgsl:20` —
  `TAA_SAMPLE_RING_DEPTH: u32 = 16u`;
  `crates/bevy_naadf/src/render/taa.rs:35` — `TAA_SAMPLE_RING_DEPTH: u32 = 16`;
  `taa.rs:256` — `TAA_SAMPLE_AGE = TAA_SAMPLE_RING_DEPTH` (16);
  `taa.wgsl:289` — the reproject loop is `for i in 1..sample_age` ⇒ walks 15
  past frames.
- **C#/paper expected behavior:** `WorldRenderBase.cs:17` —
  `taaSampleMaxAge = 32`; `:146` — `taaSamples` sized `… * 32`;
  `renderTaaSampleReverse.fx:93` — `for (i = 1; i < sampleAge; ++i)` ⇒ walks 31
  past frames; `:96` — `curTaaIndex = (taaIndex + i) % 32`. Paper §4.1 / Fig 6:
  "store the last 32 frames." (`14-paper-gap.md` flags the 16-vs-32 deviation;
  `design-exploration-qa.md` §6 records it as the binding VRAM lever — ~501 MB
  @16 vs ~973 MB @32 @1440p.)
- **Concrete difference:** the temporal averaging window is halved — 15 history
  frames + 1 current vs 31 + 1. The paper's own Table 4 lists "Ours (16 samples)"
  as a sanctioned, *slightly noisier* configuration.
- **IMPORTANT — what 16-vs-32 does NOT change:** it does **not** weaken the
  adaptive ~0.25-spp rate. `rayQueueCalc.fx:17-19` (`ray_queue_calc.wgsl:111-117`):
  `fac = accum / 2`, `modSize = round(clamp(fac*2, 0, 3) + 1)` = `round(clamp(
  accum, 0, 3) + 1)`. `modSize` saturates at 4 as soon as `accum >= 3`, and
  `accum` (the reprojected-history count `ReprojectOld` writes into
  `taa_sample_accum.x`) reaches 3 within ~3 frames in *both* the 16- and 32-deep
  configurations. So the spp adaptivity is unchanged — the 16-deep ring's cost
  is purely shallower temporal smoothing, nothing more.
- **Hypothesized impact:** real but secondary — roughly "half the temporal
  noise reduction." On its own it makes the port "slightly noisier" (the paper's
  word); it does NOT explain "barely resolves" (that is #1). Order it *after*
  #1/#2 — once those are fixed, decide whether to spend the VRAM.
- **Proposed fix direction:** if VRAM allows, raise `TAA_SAMPLE_RING_DEPTH` to
  32 in `taa_common.wgsl` + `taa.rs` (single source of truth on each side; the
  buffer sizing at `taa.rs:459` and `% 32` sites all key off the constant) and
  `TAA_SAMPLE_AGE` follows. **Middle ground:** 24-deep is a real option — ~750 MB
  @1440p, 75 % of the temporal window for 75 % of the cost; the ring code is
  fully parametric so any value works. Recommendation: try 32 first (the e2e
  test scene is small; the dev box is a 16 GB RTX 5080 — VRAM is not tight here),
  fall back to 24 only if a real 1440p+ content scene needs the headroom.
- **Scope:** medium — one constant on each side, but it changes a large buffer
  allocation; re-verify the e2e VRAM headroom.

### 4. `denoiseThresh` magnitude makes the bilateral filter near-pass-through — MEDIUM confidence, SMALL scope (verify, do not blindly change)

- **Subsystem:** sparse bilateral denoiser weight (`renderDenoiseSplit`).
- **Port behavior:** `crates/bevy_naadf/src/lib.rs:82` — `denoise_thresh: 400.0`
  (matches C#). The weight is
  `bilateral_fac = 1 / (1 + abs(curTaaWeight - taaWeight) * denoise_thresh)`
  (`denoise_split.wgsl:121-122` = `renderDenoiseSplit.fx:53`).
- **C# expected behavior:** `WorldRenderBase.cs` `SettingDataRenderBase.denoiseThresh
  = 400` — **same value**. So the *constant* is faithful.
- **Concrete difference / why it still matters:** the `taaWeight` fed into this
  bilateral term is `dot(curTaaColor, (1,1,1))` — the *luminance of the
  TAA-accumulated colour* — written by `spatial_resampling.wgsl:632-635`
  (= `renderSpatialResampling.fx:386`). `curTaaColor` is derived from
  `taa_sample_accum` divided by `accum * dot(absorption,1) + 0.01`
  (`spatial_resampling.wgsl:614-623` = `renderSpatialResampling.fx:373-380`).
  If `taa_sample_accum` is itself noisier than C#'s (because of cause #1 — no GI
  jitter) **or** scaled differently (because of the 16-deep ring's lower `accum`
  ceiling), then the bilateral guide signal is degraded and the denoiser either
  over-blurs across edges or under-filters noise. The denoiser shader itself is
  a **faithful port** (verified line-by-line vs `renderDenoiseSplit.fx` — kernel
  21, σ=10, separable, sparse per-row/-column random offset, `lerp(colorOrig,
  color, 0.92)`, `*= absorption`, `+= final_color`).
- **Hypothesized impact:** secondary — the denoiser cannot fix what an upstream
  defect feeds it. It is listed so a fix dispatch does NOT chase `denoise_thresh`
  as a primary suspect: it is faithful; fix #1 first, then re-evaluate.
- **Proposed fix direction:** none directly — verify it behaves once #1 lands.
  Only if it still over/under-filters after #1/#2 should `denoise_thresh` be
  re-tuned, and then only as a documented deviation.
- **Scope:** small (if anything is needed at all).

### 5. `taa_index` increment-order vs NAADF — LOW confidence, audit-only

- **Subsystem:** the camera-history ring slot labelling / frame counter.
- **Port behavior:** `crates/bevy_naadf/src/render/taa.rs:173-209`
  (`update_camera_history`): derive `taa_index` from the *current*
  `frame_count`, write ring slot `taa_index`, store `taa_index`, **then**
  `frame_count += 1`.
- **C# behavior:** `WorldRender.cs:86-88` — `frameCount++` **then**
  `taaIndex = 128 - (frameCount % 128) - 1`; `WorldRenderBase.cs:187-194` writes
  ring slot `taaIndex` for the current frame.
- **Concrete difference:** NAADF increments `frameCount` *before* deriving
  `taaIndex`; the port increments *after*. `10-impl-b.md`'s camera-motion audit
  argues both write slot `taa_index` and pass the same `taa_index` to the
  shader, and `taa_index` decrements by 1 per frame either way, so `(taa_index +
  i)` relative indexing is identical. This is **probably fine** and was already
  reasoned through — but the *absolute* `frame_count` value the port feeds to
  `getJitter`/`rand_counter`/`accum_index` is offset by 1 vs NAADF for the same
  logical frame. That offset is harmless for jitter (it is just a sequence
  position) but it is the kind of thing worth a 10-minute re-check while fixing
  #1, since #1 will route `current_jitter` into the GI passes and any
  frame-counter skew would show up there.
- **Hypothesized impact:** low / probably none — listed for completeness so the
  fix dispatch verifies it rather than assuming.
- **Proposed fix direction:** no change unless the re-check finds a real skew;
  if so, align to NAADF's increment-before-derive order.
- **Scope:** trivial (audit), small (if a fix is needed).

---

## Pipeline completeness check

Port render-graph order (`crates/bevy_naadf/src/render/mod.rs:207-228`,
`.chain()` in `Core3d` `PostProcess`) vs C# `WorldRenderBase.RenderInternal`
(`WorldRenderBase.cs:205-443`):

| # | C# `WorldRenderBase` dispatch | Port node | Status |
|---|---|---|---|
| 1 | `renderSky` `Atmosphere` (`:205-206`) | `naadf_atmosphere_node` | present, faithful |
| 2 | `firstHitEffect` `FirstHit` (`:228-229`) | `naadf_first_hit_node` | present, faithful (4-plane, jittered) |
| 3 | `renderTaaSample` `ReprojectOld` (`:252-253`) | `naadf_taa_reproject_node` | present, faithful |
| 4 | `sampleRefineEffect` `ClearBucketsAndCalcMask` (`:272-273`) | `naadf_sample_refine_clear_node` | present, faithful |
| 5 | `rayQueueEffect` `RayQueue` + `RayQueueStore` (`:285-288`) | `naadf_ray_queue_node` (both entry points) | present, faithful |
| 6 | `globalIllumEffect` `GlobalIlum` (indirect) (`:322-323`) | `naadf_global_illum_node` | present — **unjittered ray (cause #1)** |
| 7 | `sampleRefineEffect` `ValidHistory` (`:352-353`) | `naadf_sample_refine_valid_history_node` | present, faithful |
| 8 | `sampleRefineEffect` `CountValidAndRefine` (indirect) (`:355-356`) | `naadf_sample_refine_count_valid_node` | present, faithful |
| 9 | `sampleRefineEffect` `CountInvalid` (indirect) (`:358-359`) | `naadf_sample_refine_count_invalid_node` | present, faithful |
| 10 | `sampleRefineEffect` `RefineBuckets` (`:361-362`) | `naadf_sample_refine_buckets_node` | present, faithful |
| 11 | `spatialResamplingEffect` `SpatialResampling` (`:396-397`) | `naadf_spatial_resampling_node` | present — **unjittered ray (cause #1)** |
| 12 | `denoiseEffect` `CalcDenoiseHorizontal` (`:412-413`), gated `isDenoise` | `naadf_denoise_node` (H pass), gated `is_denoise` | present, faithful |
| 13 | `denoiseEffect` `CalcDenoiseVertical` (`:415-416`), gated `isDenoise` | `naadf_denoise_node` (V pass) | present, faithful |
| 14 | `renderTaaSample` `CalcNewTaaSample` (`:421-422`) | `naadf_calc_new_taa_sample_node` | present, faithful |
| 15 | `renderFinal` fullscreen (`:432-443`) | `naadf_final_blit_node` | present — **exposure/toneMappingFac swapped (cause #2)** |

**No pass is missing, stubbed, or no-op'd.** The graph order matches NAADF
exactly. Caveats / debris (not convergence-affecting, but listed for the fix
dispatch):

- **Dead two-frame temporal-stability scaffold** (`12-alignment-gap.md` B-3):
  `GateState.fb_next`, `batch_needs_second_frame`, `Framebuffer::mean_pixel_delta`
  in `src/e2e/` are still unwired. The moving-camera e2e mode was since added
  (the e2e run reports "96 warmup + 48 camera-motion + 1 settle frames") so this
  scaffold is now partially superseded — but the three named symbols are still
  dead. Cosmetic; not a convergence bug.
- **Dead plumbing** (`12-alignment-gap.md` B-5): `FLAG_BLIT_FINAL_COLOR`, the
  dormant `taa_layout` descriptor + `TaaGpu.taa_first_hit_bind_group` field, the
  `taa_sample_accum` no-op touch in `naadf_first_hit.wgsl`. The
  `taa_first_hit_bind_group` is *still built every frame* in `prepare_taa`
  (`taa.rs:427-434`) and bound by nothing — pure waste, not a bug.
- `rayQueueCalc`'s two entry points (`RayQueue` + `RayQueueStore`) are correctly
  one node with two dispatches; `denoise` is correctly one node with two
  dispatches — both faithful to the C# pass grouping.

---

## Configuration diff table

| Constant | Port value | C# value | Paper value | Note |
|---|---|---|---|---|
| `taaSampleMaxAge` / sample-ring depth | **16** (`taa_common.wgsl:20`, `taa.rs:35`, `TAA_SAMPLE_AGE` `taa.rs:256`) | **32** (`WorldRenderBase.cs:17,146`) | 32 (§4.1, Fig 6) | **Cause #3** — deliberate VRAM lever (`design-exploration-qa.md` §6); paper Table 4 sanctions "Ours (16)". |
| camera-history ring depth | 128 (`taa.rs:30`) | 128 (`WorldRenderBase.cs:150-154`) | 128 (`02-research.md` div #5) | faithful — correctly NOT the §6 lever. |
| `exposure` (final blit) | **1.5** (`prepare.rs:392`) | **1.0** (`Settings.cs:36`) | n/a (post-process) | **Cause #2** — swapped with `toneMappingFac`. |
| `toneMappingFac` (final blit) | **1.0** (`prepare.rs:396`) | **1.5** (`Settings.cs:37`) | n/a | **Cause #2** — swapped with `exposure`. |
| GI ray jitter (`globalIllum`) | **`vec2(0,0)`** (`naadf_global_illum.wgsl:207`) | **`taaJitter`** (`renderGlobalIllum.fx:69`) | jittered (§4) | **Cause #1** — `GpuGiParams` has no jitter field. |
| GI ray jitter (`spatialResampling`) | **`vec2(0,0)`** (`spatial_resampling.wgsl:583`) | **`taaJitter`** (`renderSpatialResampling.fx:351`) | jittered (§4) | **Cause #1** — same. |
| first-hit ray jitter | `params.taa_jitter` (`naadf_first_hit.wgsl:118`) | `taaJitter` (`renderFirstHit.fx:37`) | jittered | faithful — first-hit IS jittered. |
| Halton jitter bases | (3, 7) fixed (`taa.rs:124`) | (3, 7) `coprimes` (`WorldRender.cs:113`) | Halton (§4) | faithful in practice (`14-paper-gap.md` #7). |
| `getJitter` index | `(frame % 32) + 1` (`taa.rs:123`) | `(frame % 32) + 1` (`WorldRender.cs:139`) | — | faithful. |
| denoiser kernel size | 21 (`y/x ∈ [-10,10]`, `denoise_split.wgsl:102,199`) | 21 (`renderDenoiseSplit.fx:39,99`) | 21 (§4.3) | faithful. |
| denoiser σ | 10 (`gaussian_f(_, 10.0)`, `denoise_split.wgsl:130,225`) | 10 (`gaussianF(_, 10)`, `renderDenoiseSplit.fx:55,115`) | 10 (§4.3) | faithful. |
| denoiser sparsity | random per-row/-col offset (`denoise_split.wgsl:105,201`) | same (`renderDenoiseSplit.fx:41,101`) | ~½ pixels (§4.3) | faithful. |
| denoiser final lerp | `mix(color_orig, color, 0.92)` (`denoise_split.wgsl:232`) | `lerp(colorOrig, color, 0.92)` (`renderDenoiseSplit.fx:124`) | — | faithful. |
| `denoiseThresh` | 400.0 (`lib.rs:82`) | 400 (`SettingDataRenderBase`) | — | faithful constant; guide signal is upstream-degraded (cause #4). |
| `globalIllumMaxAccum` (sample-counts ring) | 128 (`lib.rs:79`) | 128 (`SettingDataRenderBase`) | — | faithful. |
| `sample_counts` ring length | 128 + 3 (`gi.rs:47`) | 128 + 3 (`WorldRenderBase.cs:165`) | — | faithful. |
| `validSampleStorageCount` (lit) | 2 (`gi.rs:51`) | 2 (`WorldRenderBase.cs:57`) | "2 frames'" (§4.2) | faithful. |
| `invalidSampleStorageCount` (unlit) | 8 (`gi.rs:54`) | 8 (`WorldRenderBase.cs:58`) | "4 frames'" (§4.2) | faithful (C# uses 8; paper prose says "4 frames' worth"). |
| `bucketStorageCount` | 32 (`gi.rs:57`) | 32 (`WorldRenderBase.cs:59`) | ≤32 lit/region (§4.2) | faithful. |
| `refinedBucketStorageCount` | 8 (`gi.rs:60`) | 8 (`WorldRenderBase.cs:60`) | ≤8 refined (§4.2) | faithful. |
| 8×8 region size | `ceil(viewport/8)` (`gi.rs:93-96`) | `(w+7)/8 × (h+7)/8` (`WorldRenderBase.cs:157-159`) | 8×8 (§4.2) | faithful. |
| spatial-resampling iterations | 12 (`spatial_resampling.wgsl:594` — `sample_neighbors(_, 12u, _)`) | 12 (`renderSpatialResampling.fx:359` — `sampleNeighbors(_, 12, _)`) | 12 (§4.2, Alg 2) | faithful. |
| `maxBounceCount` | 3 (`lib.rs:78`) | 3 (`SettingDataRenderBase`) | ≤3 (§4.2) | faithful. |
| unlit 8:1 compression | `!is_valid && next_rand > 1/8` (`naadf_global_illum.wgsl:452`) | `!isValid && nextRand > 1/8` (`renderGlobalIllum.fx:252`) | every 8th, ×8 (§4.2) | faithful. |
| atmosphere RR (miss) | `next_rand <= 1/16` ×16 (`naadf_global_illum.wgsl:292`) | `nextRand <= 1/16` ×16 (`renderGlobalIllum.fx:130-131`) | — | faithful. |
| `skipSamples` (1↔0.25 spp) | `flags & GI_FLAG_SKIP_SAMPLES`, default on (`lib.rs:85`) | `skipSamples` default true (`SettingDataRenderBase`) | adaptive 0.25–1 spp (§4.2) | faithful — and the `modSize` math is depth-independent (see cause #3). |
| `noiseSuppressionFactor` | 0.4 (`lib.rs:84`) | 0.4 (`SettingDataRenderBase`) | — | faithful. |
| `radiusLitFactor` | 3.0 (`lib.rs:83`) | 3.0 (`SettingDataRenderBase`) | — | faithful. |
| `spatialResampleSize` | 500.0 (`lib.rs:80`) | 500.0 (`SettingDataRenderBase`) | — | faithful. |
| `spatialResampleVisibilityTestMaxDepth` | dropped — `MAX_RAY_STEPS_VISIBILITY` const used (`spatial_resampling.wgsl:457`) | 80 (`SettingDataRenderBase`), but the uniform is **dead** in the HLSL — `sampleNeighbors` passes the `MAX_RAY_STEPS_VISIBILITY` const directly (`renderSpatialResampling.fx:274`) | single visibility check (§4.2) | faithful — divergence D-C, the C# uniform is dead; port correctly uses the const. |
| `screenPosDistanceSqr` reject (TAA reproject) | `> 16.0` (`taa.wgsl:346`) | `> 16.0` (`renderTaaSampleReverse.fx:139`) | 1-px check (§4.1) | faithful — `base/` variant value (divergence D-B). |
| TAA distance reject bounds | `1022/1024 … 1026/1024 … *2` (`taa.wgsl:327-329`) | same (`renderTaaSampleReverse.fx:128`) | depth-range (§4.1) | faithful. |
| `refineBuckets` `< 12` gate | `new_valid + new_invalid < 12` → `compressed_index = 0` (`sample_refine.wgsl:706-708`) | same (`renderSampleRefine.fx:411-412`) | — | faithful — needs ≥12 accumulated samples per bucket; this is why `E2E_RENDER_FRAMES` was raised to 96. |
| TAA color compression | `12·log2(x/100 + 2^(-255/12)) + …` (`taa_common.wgsl:97-98`) | algebraically equal (`commonTaa.fxh:35`) | paper formula (§4.1) | faithful (recompute `#[test]` exists). |

---

## Black-on-resize root cause

**The specific defect:** the screen-space GPU buffers are sized from
`ExtractedCameraData.viewport_size`, which is derived in `extract_camera`
(`crates/bevy_naadf/src/render/extract.rs:131-134`) as:

```rust
let viewport_size = camera
    .physical_viewport_size()
    .unwrap_or(UVec2::new(1, 1))
    .max(UVec2::ONE);
```

During a window resize, `Camera::physical_viewport_size()` transiently returns
`None` (the camera's viewport rect is recomputed by Bevy's
`camera_system`/`update_frusta` *after* the window's new size is known, and the
render-world `extract` schedule can run on a frame where it has not been
recomputed yet) — or returns the *previous* frame's size. When it returns
`None`, the fallback collapses `viewport_size` to **1×1**, so `pixel_count == 1`.

That one value then drives, in the same render frame:

- `prepare_taa` (`taa.rs:289-371`): sees `taa.pixel_count != 1` → re-creates
  `taa_samples` / `taa_sample_accum` / `taa_dist_min_max` at **1 pixel** and
  `clear_buffer`s them to zero.
- `prepare_gi` (`gi.rs:236-265`): sees `gpu.pixel_count != 1` → re-creates every
  `pixel_count`-sized GI buffer at **1 pixel** (and the `bucket_count`-sized ones
  at 1 bucket), zero-clears them.
- `prepare_frame_gpu` (`prepare.rs:331-457`): sees `frame.pixel_count != 1` →
  `needs_new_storage = true` → re-creates `first_hit_data` / `final_color` at
  **1 pixel**, rebuilds every bind group (frame, blit, taa-reproject,
  calc-new-taa-sample, and all of `GiBindGroups`) against the 1-pixel buffers.
- The render-graph nodes (`graph.rs` / `graph_b.rs`) dispatch
  `ceil(pixel_count / 64) = 1` workgroup each — they run, but only over 1 pixel.
- `naadf_final_blit_node` (`graph.rs:258-309`) draws a **fullscreen triangle**
  over the view target's `main_texture_view()` — which is the **new, full
  window size** (the swapchain *did* resize). For every fragment except
  pixel (0,0), `pixel_index = x + y*screen_width` indexes far past the
  1-element `taa_sample_accum` → a WGSL out-of-bounds storage read, which
  returns 0 → `cur_color = 0` → tonemaps to ~0 → **black**.

So the frame goes (near-)black. The next frame, `physical_viewport_size()`
returns the real new size, all buffers rebuild at full size — **but**
`taa_samples` was just zero-cleared, so for the following ~16 frames
`reproject_old_samples` finds all reprojected history rejected (the ring is
zero), `taa_sample_accum` is overwritten with `color_sum = 0`, and only the
fresh `calc_new_taa_sample` fold (`final_color`, weight 1) lights the image —
i.e. a multi-frame dark/flickering recovery on top of the one fully-black frame.

**The same mechanism fires (less severely) even when `physical_viewport_size()`
returns a non-`None` but *stale* size:** the buffers are then sized for the
*old* resolution while the blit covers the *new* one — if the window grew, the
new-area fragments read OOB → a black border; if it shrank, the buffers are
oversized (harmless that frame) but still get re-created next frame, again
zero-clearing `taa_samples`.

**Resources that go stale/zero/wrong-sized on resize:**
- `TaaGpu.taa_samples` / `.taa_sample_accum` / `.taa_dist_min_max` — re-created
  and **zero-cleared** (`taa.rs:363-371`), so even a *correct* resize loses the
  16-frame history ring.
- `FrameGpu.first_hit_data` / `.final_color` — re-created + zero-cleared
  (`prepare.rs:487-495`).
- All `GiGpu` `pixel_count`/`bucket_count`-sized buffers — re-created +
  zero-cleared (`gi.rs:496-512`).
- `GiGpu.sample_counts` — note: re-created **and re-zeroed** on a viewport
  change (`gi.rs:248-265` always takes the `create_gi_buffers` path when
  `pixel_count` differs), so the 128-frame GI accumulation ring is also lost.
- The camera-history ring (`TaaGpu.camera_history`) is fixed-size and survives —
  but its stored per-frame `view_proj` / jitter were captured at the *old*
  resolution's projection, so for ~128 frames after a resize the reprojection
  indexes into slots whose matrices are subtly wrong (minor; not the black
  cause).

**Root cause in one line:** `extract_camera`'s `.unwrap_or(UVec2::new(1,1))`
fallback (and the broader "size everything from a single per-frame
`physical_viewport_size()` read") lets a transient resize-frame `None`/stale
viewport collapse every screen-space buffer to 1×1 (or the old size) while the
blit covers the new-size target → out-of-bounds reads → black.

**Fix direction (for the follow-up dispatch — not done here):** in
`extract_camera`, when `physical_viewport_size()` is `None` (or obviously
degenerate), **keep the last-known-good `viewport_size`** instead of falling
back to 1×1 (e.g. leave `ExtractedCameraData.viewport_size` unchanged, or carry
a `last_valid` in the resource); and/or have the `prepare_*` resize path skip
the rebuild when the new size is degenerate. A `.max()` against a sane floor is
not enough — the real fix is "never shrink to a bogus size; keep the previous
valid one until a real new size arrives."

---

## Recommended fix plan

In priority order, for a follow-up code-mutating dispatch. The bar is the C#
version, not a perfect renderer.

1. **Fix #1 — jitter the GI rays (highest impact on "barely resolves").**
   - Add `taa_jitter: Vec2` + padding to `GpuGiParams` (`gpu_types.rs`) on a
     clean 16-byte row — explicitly audit the `vec3`-then-scalar / `vec4` WGSL
     layout hazard that bit the port 3× (`RESUME.md` hazard class); the safest
     placement is its own `vec4`-aligned row or alongside an existing pad pair.
   - In `prepare_gi` (`gi.rs`), write `extracted_history.current_jitter` into it
     (the same value `prepare_frame_gpu` already routes to `GpuRenderParams
     .taa_jitter` for first-hit).
   - Declare the field in `gi_params.wgsl`.
   - In `naadf_global_illum.wgsl:207` and `spatial_resampling.wgsl:583`, replace
     the `vec2<f32>(0.0, 0.0)` `get_ray_dir` offset with the new
     `gi_params.taa_jitter` — matching `renderGlobalIllum.fx:69` /
     `renderSpatialResampling.fx:351`.
   - Verify `cargo test` + one `cargo run --bin e2e_render` (the existing e2e
     gates should still pass; the GI-lit fraction may shift slightly).

2. **Fix #2 — swap `exposure` / `tone_mapping_fac` (cheapest contrast win).**
   - In `prepare.rs` `prepare_frame_gpu`'s `GpuRenderParams` build: set
     `exposure: 1.0`, `tone_mapping_fac: 1.5` (the `Settings.cs:36-37` defaults).

3. **Re-evaluate #3 — restore the 32-deep `taaSamples` ring (or 24 as a middle
   ground).** After #1/#2 land and the image is close to the C# bar, decide the
   VRAM trade: bump `TAA_SAMPLE_RING_DEPTH` to 32 (`taa_common.wgsl` +
   `taa.rs`); the ring code is fully parametric. If a real 1440p+ content scene
   is VRAM-tight, 24 is a clean middle ground (~75 % window for ~75 % cost).
   This is a *quality dial*, not a correctness fix — sequence it last and treat
   it as a deliberate, documented choice.

4. **Fix the black-on-resize bug (separate, can be parallel).** In
   `extract_camera` (`extract.rs:131-134`), stop collapsing to `UVec2::new(1,1)`
   on a `None`/degenerate `physical_viewport_size()` — retain the
   last-known-good viewport size instead, so the resize-frame buffers never
   shrink to a bogus size. Optionally also guard the `prepare_*` resize paths
   against degenerate sizes. Accept that `taa_samples` / `sample_counts` are
   still legitimately lost on a *real* resize (NAADF does the same — the next
   ~16/128 frames refill them); the goal is to eliminate the *bogus*-size
   collapse and the resulting fully-black frame.

5. **Audit #5 (cheap, while doing #1).** Re-check the `frame_count`
   increment-before-vs-after-`taa_index`-derive order against `WorldRender.cs:86-88`
   now that `current_jitter` is being routed into the GI passes — confirm no
   1-frame skew in the jitter sequence. No change expected; verify and move on.

6. **(Optional polish, non-convergence)** Drop the dead
   `TaaGpu.taa_first_hit_bind_group` build in `prepare_taa` and the other dead
   plumbing (`12-alignment-gap.md` B-5); finish or delete the dead
   two-frame-stability scaffold (`12-alignment-gap.md` B-3). These do not affect
   noise but they are wasted work / misleading code on the TAA surface.

**Expected outcome:** #1 + #2 alone should close most of the gap to the C#
bar — the GI rays will anti-alias sub-pixel detail across the temporal window
and the tonemap will restore contrast. #3 then buys the remaining "slightly
noisier" margin if the VRAM is spent. The pipeline is structurally faithful;
this is a small, well-scoped set of fixes, not a rewrite.
