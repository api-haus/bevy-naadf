# 19 — GI Reservoir / Shadow-Filtering Improvements — Scope

**Date:** 2026-05-15
**Author:** delegate-architect (read-only scoping pass — no code mutated)
**Predecessors:** `01-context.md`, `09-design-b.md`, `10-impl-b.md`, `11-review-b.md`,
`12-alignment-gap.md`, `14-paper-gap.md`, `18-taa-fidelity.md`. Paper:
`/mnt/archive4/PAPERS/Prepared/ulschmid-2026-naadf-voxel-gi.md`. C# reference:
`/mnt/archive4/DEV/NAADF/`.

---

## §1. Goal restated

Verbatim user ask:

> "lets hit on the GI reservoir"

Contextualised: post-TAA-fidelity-track the user noted "ways to improve shadow
filtering in the future would help significantly". The paper's §4.2
compressed-ReSTIR machinery is the place where shadow-noise quality is decided.
**Job: identify and rank the concrete improvements available within the §4.2
reservoir / spatial-resampling pipeline (plus the directly-coupled sun-shadow
ray) that would reduce shadow noise / improve GI convergence quality.**

**In scope:** anything inside the GI-reservoir node sequence — sample generation
(`naadf_global_illum`), the 5-pass refine, spatial resampling (Algorithm 2), the
single sun-visibility ray inside spatial-resampling, the per-secondary-bounce
sun cone inside `naadf_global_illum`, and the storage-budget knobs that gate
how much temporal/spatial information any of those passes can use. Reservoir
changes that *interact* with the denoiser (the bilateral filter's guide signal
in `denoise_split.wgsl:121-122,218` is `taa_sample_accum` divided by
`accum * dot(absorption,1)` — a reservoir-output-driven term) are *also* in
scope as second-order analysis.

**Out of scope:** SVGF or other denoiser replacements; sparse-bilateral kernel
changes; the TAA pipeline itself (the `base/` `ReprojectOld` /
`CalcNewTaaSample` math); atmosphere changes; any reservoir-scheme overhaul
(ReSTIR PT, ReSTIR DI rewiring); §6.

This is a **quality** track within an already-faithful subsystem — §4.2 is
classed FAITHFUL in both `12-alignment-gap.md` and `14-paper-gap.md`. The work
is not fixing a missing methodology; it is choosing knobs / extending the
single-tap sun-shadow into a multi-tap one, etc., within the paper's frame.

---

## §2. Current GI reservoir state-of-port (audit)

What the port does today, walked node-by-node with code citations
(post-TAA-fidelity, post-Phase-C; `main` at the head of the audit
`12-alignment-gap.md` runs against).

### 2.1 Sample generation — `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl`

- **`[numthreads(64,1,1)]`, indirect-dispatched** off `ray_queue_indirect[0]`
  produced by `rayQueueCalc` (`crates/bevy_naadf/src/render/gi.rs:115`,
  `graph_b.rs:223`). Adaptive ~0.25 spp via the
  `taa_sample_accum.x → modSize ∈ {1..4}` test in `ray_queue_calc.wgsl:111-117`
  (paper §4.2 "selectively generate samples where necessary"; reviewer criterion
  2 — met).
- **Jittered primary GI ray** (TAA-fidelity fix #1):
  `naadf_global_illum.wgsl:213-216` calls `get_ray_dir(..., gi_params.taa_jitter)` —
  Halton (3,7), `GpuGiParams.taa_jitter` at byte offset 280 with
  `offset_of!` guard (`gpu_types.rs` ~404-481; rationale `18-taa-fidelity.md`
  fix #1).
- **≤3-bounce secondary loop** at `naadf_global_illum.wgsl:281-440`. Cap is
  `min(gi_params.max_bounce_count, 3u)` and `gi.bounce_count = 3`
  (`lib.rs:79`). Diffuse / specular-rough / specular-mirror branches at
  `:252-276` (mirror reflect) / VNDF GGX (`:254-273`) /
  `get_uniform_hemisphere_sample(... , 0.0)` for diffuse (`:275`).
- **Russian-roulette atmosphere fold on miss**:
  `naadf_global_illum.wgsl:300` — `if (next_rand <= 1.0/16.0) { ... × 16 }`.
- **Per-bounce sun sample** at `naadf_global_illum.wgsl:346-380`. SINGLE TAP
  per bounce, drawn from `get_uniform_hemisphere_sample(rand, sky_sun_dir,
  0.9999)` — the `0.9999` deviation turns the "uniform hemisphere" into a
  **narrow cone around the sun direction** with cap-half-angle ≈ 0.81°
  (`z ∈ [0.9999, 1.0]`, `r ∈ [0, sqrt(1-0.9999²)] ≈ [0, 0.01414]`). This is
  NAADF's *sun-disk approximation* — verified against `commonRayTracing.fxh:75-84`
  and `renderGlobalIllum.fx:168` (same `0.9999`). One sun-shadow ray fired
  from `new_pos_int + new_pos_frac` via `shoot_ray(..., MAX_RAY_STEPS_SUN_SECONDARY,
  ...)` (`naadf_global_illum.wgsl:370-378`); `MAX_RAY_STEPS_SUN_SECONDARY = 80`
  (`ray_tracing.wgsl:125`).
- **Unlit 8:1 compression** at `naadf_global_illum.wgsl:458-460`:
  `is_valid = radiance_comp > 0`; `is_skip = !is_valid && next_rand > 1/8` —
  every 8th unlit sample stored, weighted ×8 (paper §4.2 — verified faithful).
- **5-bit / channel colour compression** via `compress_sample_valid` /
  `compress_sample_invalid`; `COLORS[32]` LUT at `color_compression.wgsl:33-66`
  (`COLOR_EXP = 2^0.6`, `COLOR_START = 1/64`) with a Rust recompute test
  (`render/color_compression.rs`, `color_tables_match_wgsl`).
- **Region projection:** every accepted lit sample is written to a per-region
  bucket via the 5-pass refine downstream; unlit samples bump a region
  counter only (paper §4.2).

### 2.2 Sample-refine — `sample_refine.wgsl` (5 passes)

Each pass is `[numthreads(64,1,1)]`. Ordered as `WorldRenderBase.cs:272-362`.

1. `clear_buckets_and_calc_mask` (`sample_refine.wgsl` first kernel). Zero the
   bucket counters; thread-0 also resets `ray_queue_indirect[0] = 0u`
   (renderSampleRefine.fx:39 — the rayqueue reset is folded into this pass,
   not into prepare-time; verified `gi.rs:266-280` notes).
2. `valid_history` — reproject the per-pixel lit sample into the bucket grid
   (writes `bucket_info` valid-mask).
3. `count_valid_and_refine` (indirect) — count + refine the lit samples into
   `valid_samples_refined`.
4. `count_invalid` (indirect) — bump the per-bucket unlit count atomically:
   `atomicAdd(&bucket_info[reproj.bucket_index].x, 1u << 18u);`
   (`sample_refine.wgsl:586`).
5. `refine_buckets` (`sample_refine.wgsl:599-722`) — **the brightness-leveling
   pass.** Per bucket: find `samples_comp_color_max` (the max compressed
   colour level across the ≤32 refined lit samples), then for each refined
   sample compute `max_color_dif = samples_comp_color_max - comp_color_max`
   (`:654-658`) and `remove_prob = COLOR_DIF_PROB[max_color_dif]`
   (`:659`). The `COLOR_DIF_PROB[31]` table is at `color_compression.wgsl:70-102`
   (`COLOR_DIF_PROB[i] = 1 - COLOR_EXP^(-i)`). Survivors are compensated via
   the `darkening_offset` distance-variance term (`:668-680`); ≤8 survivors
   land in `valid_samples_compressed`; the bucket's `original_lit_ratio` is
   packed into `bucket_info`.
- **`<12` accumulation gate** at `sample_refine.wgsl:706-708`: a bucket with
  `new_valid + new_invalid < 12` gets `compressed_index = 0`, i.e. the
  bucket is treated as empty until ≥12 samples accumulated. This is why
  `E2E_RENDER_FRAMES = 96` (`10-impl-b.md`).
- **Indirect-buffer split** — wgpu forbids
  `STORAGE_READ_WRITE × INDIRECT` in one bind group, so the `valid_dispatch` /
  `invalid_dispatch` buffers occupy a dedicated `@group(1)` while the rest of
  the sample-refine bindings stay on `@group(0)` (divergence D-D /
  `12-alignment-gap.md` §3).

### 2.3 Spatial resampling — `spatial_resampling.wgsl` (Algorithm 2)

`calc_spatial_resampling` `[numthreads(64,1,1)]` at `spatial_resampling.wgsl:567-666`
(C# `calcSpatialResampling` `renderSpatialResampling.fx:344-399`).

- **`get_ray_dir` jittered** (TAA-fidelity fix #1):
  `spatial_resampling.wgsl:588-591` passes `gi_params.taa_jitter` — same source
  of truth as `naadf_global_illum.wgsl`'s GI ray.
- **`sample_neighbors(pixel_pos, 12u, first_hit_result, first_hit_type_index)`**
  at `spatial_resampling.wgsl:601` — the **12-iteration** neighbour-reservoir
  loop (`renderSpatialResampling.fx:359` — paper Algorithm 2 maxIterations =
  12, hardcoded).
- **Adaptive-radius 12-tap pre-pass** (`spatial_resampling.wgsl` lines
  around the `is_varying_resampling_radius` gate inside `sample_neighbors` —
  ports `renderSpatialResampling.fx:81-148`). Driven by `radius_lit_factor`
  (= 3.0, `lib.rs:84`) and `spatial_resample_size` (= 500.0, `lib.rs:81`).
- **Per-iteration neighbour evaluation:** normal-mask + bucket distance
  band reject; Jacobian compute `dot(neighborSampleNormal, dirToSampleNow)
  * lengthToSampleSquaredNeighbor / (... )` clamped `[0, 4]`; reject outside
  `[0.3, 2.5]`; pdf-ratio reject outside `[0.25, 2.0]`; cosθ reject if
  < 0.0001 (the port mirrors `renderSpatialResampling.fx:200-237` exactly).
- **Single 3-step visibility ray** at `spatial_resampling.wgsl:447-489` (the
  HLSL loop `:266-302`). Three iterations because a specular-mirror bounce
  can chain up to 3 reflections; each iteration `shoot_ray(..., MAX_RAY_STEPS_VISIBILITY,
  ...)` with `MAX_RAY_STEPS_VISIBILITY = 60` (`ray_tracing.wgsl:126`).
  `is_visible = total_hit_length² - selected_length_to_sample_squared_now ≥ 0`.
- **Single sun sample** at `spatial_resampling.wgsl:529-560` (HLSL `:321-339`).
  ONE TAP from `get_uniform_hemisphere_sample(rand, sky_sun_dir, 0.9999)` — the
  same `0.9999` narrow-cone sun-disk approximation as the per-bounce sample.
  Visibility via `shoot_ray(first_hit_pos, sun_dir_rand, MAX_RAY_STEPS_SUN, ...)`
  with `MAX_RAY_STEPS_SUN = 120` (`ray_tracing.wgsl:124`). **This is the
  primary "shadow noise" lever the user is asking about** — every shadowed
  pixel sees a fresh single sun-disk sample every frame; temporal accumulation
  is what eventually averages the shadow edge, but the *per-frame* shadow at
  a moving camera is one-sample-noisy.
- **Write-out branches:** `is_denoise` (default `true`, `lib.rs:88`) →
  transposed-index `denoise_preprocessed`; non-denoise → direct `final_color`.

### 2.4 Sparse bilateral denoise — `denoise_split.wgsl`

Kernel 21, σ=10, separable H+V, sparse per-row/-column random offset, ~½
pixels processed (paper §4.3). `bilateral_fac = 1 / (1 + abs(curTaaWeight -
taaWeight) * denoise_thresh)` at `denoise_split.wgsl:121,218`, `denoise_thresh
= 400.0` (`lib.rs:83`). **Faithful** per `14-paper-gap.md` §4.3 row;
unchanged across the TAA-fidelity track. The bilateral *guide* signal
(`taa_weight = dot(curTaaColor, vec3(1))` from
`spatial_resampling.wgsl:632-635`) depends on the reservoir's output, so any
reservoir improvement propagates into the denoise weight automatically.

### 2.5 Storage budgets — `gi.rs`

```
SAMPLE_COUNTS_LEN              = 128 + 3      // gi.rs:47 (128-frame accum + 3 hdr)
VALID_SAMPLE_STORAGE_COUNT     = 2            // gi.rs:51 — "2 frames of lit"
INVALID_SAMPLE_STORAGE_COUNT   = 8            // gi.rs:54 — "8 frames of unlit"
BUCKET_STORAGE_COUNT           = 32           // gi.rs:57 — ≤32 lit/region
REFINED_BUCKET_STORAGE_COUNT   = 8            // gi.rs:60 — ≤8 refined
```

The paper §4.2 prose says **"two frames' worth" of lit storage** (covers ≤64
past frames; cap 64 lit/pixel temporal range) and **"four frames' worth" of
unlit storage** (covers ≥32 frames). **C# uses 8 for unlit** (`WorldRenderBase.cs:58`)
— deeper than the paper prose's "four". The port follows C# (paper-gap row
12 — FAITHFUL to C#). `global_illum_max_accum` is 128 (`lib.rs:80`) — the
`sample_counts` ring depth (per-pixel hit count over the past 128 frames,
drives `accum_index` for the adaptive sampler).

### 2.6 Region geometry — 8×8

`bucket_grid_of(viewport)` at `gi.rs:93-97`: `bucket_size = ceil(viewport / 8)`,
`bucket_count = bucket_size.x * bucket_size.y` — matches `WorldRenderBase.cs:157-159`.
8×8 disjoint screen-space regions, per paper §4.2 "8×8 disjoint screen-space
pixel regions". The 8 is wired into `naadf_global_illum.wgsl` /
`sample_refine.wgsl` (`pixel_pos / 8`) — not a single named constant; changing
it would require shader edits + bucket-buffer resizing.

### 2.7 Current deviations from a straight C# port (already in place — the new baseline)

- TAA-fidelity track (2026-05-15): GI rays now jittered with
  `GpuGiParams.taa_jitter` offset 280; Bevy `TonyMcMapface` tonemapping
  replaces C# Reinhard (port emits raw linear HDR); TAA ring depth
  configurable default 32; black-on-resize fix.
- `spatialVisibilityCount` uniform dropped (dead in C#; const used
  instead — divergence D-C, `12-alignment-gap.md` §3).
- Prefix-counter atomic in `rayQueueCalc` (`9-design-b.md` §7).
- `valid_dispatch` / `invalid_dispatch` on `@group(1)` (divergence D-D —
  wgpu `STORAGE_READ_WRITE × INDIRECT` exclusivity).
- `denoise_preprocessed` is a `vec4<u32>` (`Uint3` padded), not the
  C# `Uint3` — `09-design-b.md` §3.3 vec3-trailing-scalar layout adapter.

---

## §3. The improvement surface (ranked)

Each candidate is followed by: paper/C# anchor, expected impact on shadow
noise / convergence, affected files, effort estimate (trivial / small /
medium / large), risk, validation strategy. The ranking is **impact ×
(low effort) × low risk**.

### 3.1 — Multi-tap sun-shadow in spatial resampling (#1 highest impact)

**Anchor.** Paper §5.2 limitation: *"soft shadows from the sun are not handled
during resampling, resulting in slightly increased noise."* C#:
`renderSpatialResampling.fx:321-339`. Port: `spatial_resampling.wgsl:529-560`.

**What it does.** Replace the single sun-disk sample with **N taps per pixel
per frame**, averaging the (occluded? ×0 : ×weight) result. Each tap is a
fresh `get_uniform_hemisphere_sample(rand, sky_sun_dir, deviation)` with a
fresh `shoot_ray(..., MAX_RAY_STEPS_SUN, ...)`. N=4 or N=8 produces visibly
softer penumbras within a single frame and decorrelates the shadow noise
across temporal accumulation.

**Expected impact.** **High on shadow-band noise.** Currently every shadowed
pixel sees one binary sun-ray result per frame; the temporal window (32
frames) averages those, but rapid camera motion (the e2e moving-camera mode
or any user pan) reveals the 1-tap stochastic edge. With N=4, the shadow
penumbra resolves ~4× faster per frame and the camera-motion shadow flicker
visible at edges drops substantially. Paper §5.2 explicitly identifies this
as the residual noise source.

**Affected files.**
- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:529-560` —
  wrap the existing single tap in a `for i in 0..N` loop, accumulate
  `weight * (sun_blocked ? 0 : 1)` into a running `f32`, divide by N at end.
- Optionally `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl:346-380` —
  the per-secondary-bounce sun sample is the *same* one-tap structure; the
  same N-tap extension applies (and decorrelates secondary-bounce shadow
  noise). Likely lower priority than the spatial-resampling tap because the
  spatial pass writes the final colour, but worth surfacing as a sub-option.
- `crates/bevy_naadf/src/lib.rs` `GiSettings` + `crates/bevy_naadf/src/render/gpu_types.rs`
  `GpuGiParams` — add `sun_shadow_taps: u32` (or hard-code 4 if the user
  doesn't want a configurable knob). If made configurable, beware the
  `vec3`-then-scalar trap: a new `u32` after the existing tail must fit
  inside the 16-byte row holding `taa_jitter`'s pad, or a new vec4 row must
  be added with an `offset_of!` guard.

**Effort.** **Small** if hard-coded N=4 (≈10 WGSL lines + 10 WGSL lines if
also done in `naadf_global_illum.wgsl`); **medium** if exposed as a runtime
knob (uniform field + layout guard + Rust plumbing through `extract_gi`
already established).

**Risk.**
- **Cost scaling.** N=4 multiplies the spatial-resampling sun-ray work by 4.
  `MAX_RAY_STEPS_SUN = 120` is the highest ray budget in the whole pipeline.
  On dense-occluder scenes this could be a meaningful frame-time hit. Mitigation:
  share the random base across taps (offset by golden-ratio rotations) so
  the cost is N × 120 ray steps but with quasi-Monte-Carlo coverage — better
  variance than N × independent uniform samples.
- **`0.9999` deviation tightness.** With N=4 you may want to *widen* the
  cone to a true sun-disk radius (real sun half-angle ≈ 0.26°, i.e.
  deviation ≈ 0.99999). The C# `0.9999` is wider than the real sun; widening
  *further* would soften the penumbra to physically-correct, but tighter is
  what users typically want for crisp content. **Keep `0.9999` as the cone
  half-width unless the user explicitly wants soft physical penumbras.**

**Validation strategy.** A new `--moving-camera --sun-shadow-bench` e2e mode
or a new gate in `e2e/gates.rs` that measures the variance of luminance in a
designated *shadowed* region rect across the WARMUP→MOTION→SETTLE phases.
Expected: 4-tap variance ≈ 1-tap variance / 4 (Monte-Carlo √N). Existing
`MIN_GI_BOUNCE_AFTER_MOTION = 150.0` gate at `gates.rs:643` should keep
passing.

---

### 3.2 — Audit the C# `MAX_RAY_STEPS_SUN_SECONDARY = 80` vs port (#2 sanity check)

**Anchor.** C# `rayTracing.fxh:10` — `#define MAX_RAY_STEPS_SUN_SECONDARY 80`.
Port `ray_tracing.wgsl:125` — `MAX_RAY_STEPS_SUN_SECONDARY: i32 = 80`. C#
`rayTracing.fxh:11` — `#define MAX_RAY_STEPS_VISIBILITY 60`. Port
`ray_tracing.wgsl:126` — `MAX_RAY_STEPS_VISIBILITY: i32 = 60`.

**What it does.** Both port and C# already match — confirm this is unchanged.
But the *paper §5.2 limitation* on sun shadows is qualified by what the
visibility / sun ray actually probes — at `MAX_RAY_STEPS_VISIBILITY = 60` ray
steps, distant occluders are missed and the spatial-resampling pass's single
visibility check (`spatial_resampling.wgsl:447-489`) returns "visible" by
default for samples whose true blocker is beyond 60 voxel-traverse iterations.
**This produces *light leaks*, not shadow noise** — the opposite of the user
ask — but it's worth surfacing as a *known limitation* because future shadow
work may want to lift the cap.

**Expected impact.** None on shadow *noise* directly. Could reduce light
leaks if `MAX_RAY_STEPS_VISIBILITY` were raised. C#'s slider
(`SettingDataRenderBase.spatialResampleVisibilityTestMaxDepth = 80` — paper
prose, slider 0..80, ImGui — `WorldRenderBase.cs:19,36`) clamps the constant
to 80 in the live config — **but the uniform is dead** (`12-alignment-gap.md`
divergence D-C). The port correctly drops the uniform and uses
`MAX_RAY_STEPS_VISIBILITY = 60` directly. **The C# constant is 60, the
slider 0..80 only widened the *dead* uniform path.**

**Affected files.** None for an audit. To act: `ray_tracing.wgsl:126` const
change + a regression eyeball.

**Effort.** **Trivial** (audit-only) / **trivial** (one const) if a change
is wanted.

**Risk.** Raising `MAX_RAY_STEPS_VISIBILITY` linearly increases the
spatial-resampling cost per visibility check (one final visibility ray per
pixel, on the longest-tail path). **Not a quality win for shadow noise**;
worth noting only.

**Validation strategy.** Light-leak comparison eyeball — not a numeric gate.
**Recommendation: drop from the dispatch list unless the user specifically
asks for light-leak fixes.**

---

### 3.3 — Increase spatial-resampling iteration count (12 → 16/24) (#3)

**Anchor.** Paper §4.2 Algorithm 2 with `maxIterations = 12`. C#
`renderSpatialResampling.fx:359` — `sampleNeighbors(_, 12, _)` (hard-coded).
Port `spatial_resampling.wgsl:601` — `sample_neighbors(_, 12u, _)`
(hard-coded). Paper §4.2 prose: *"ReSTIR GI runs 3 iterations [...] We
optimize this by running 12 iterations instead"*; NAADF chose 12 as the
quality/cost knee (more iterations → less variance, more cost).

**What it does.** Bump the loop to 16 or 24, increasing the number of
neighbour reservoirs sampled per pixel. Reduces *indirect-lighting* variance
in shadowed/dim regions (more reservoirs evaluated → better chance of
finding a lit one near the shadow boundary).

**Expected impact.** **Medium on indirect-lighting noise.** Reduces variance
roughly proportionally to √(N_iter); 12 → 16 is ~15 % less variance, 12 → 24
is ~30 %. **Not specific to sun shadows** — it improves *all* indirect-bounce
convergence, the user's secondary concern.

**Affected files.**
- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:601` — change
  the `12u` literal.
- Optionally extract into a `GpuGiParams` field, but the C# hard-codes it;
  faithfulness-wise a const change tracks the C# call site.

**Effort.** **Trivial** (one literal) if hard-coded; **small** if exposed
through `GpuGiParams` (layout audit).

**Risk.** Linear cost in iterations. Each iteration is the
`sample_neighbors` loop body (`spatial_resampling.wgsl:140-445`): a bucket
fetch, several rejects, a Jacobian, a weight, an `is_update` branch. The
**single visibility ray** at the end is *outside* the loop (paper §4.2's
key optimisation) so the cost doesn't multiply by N_iter for visibility.
Should be a modest hit, not a doubling.

**Validation strategy.** Same as 3.1 — variance of a luminance region across
WARMUP/MOTION/SETTLE; expect √N variance reduction.

---

### 3.4 — Tune `radius_lit_factor` (adaptive radius bias) (#4)

**Anchor.** Paper §4.2: *"adaptive radius for each pixel"*. C# `SettingDataRenderBase.radiusLitFactor = 3.0f`. Port `lib.rs:84` — `radius_lit_factor: 3.0`. The
factor drives `renderSpatialResampling.fx:146` —
`radiusFacRaw = (max(1, pow(maxColorSmall/maxColorSumSmall, 1)) /
(1 * pow(worstLitSmall, 1))) * sqrt(worstLitBig) * radiusLitFactor * 0.01f`.

**What it does.** The adaptive-radius pre-pass (the inner 12-tap probe at
`renderSpatialResampling.fx:81-148`) decides whether a pixel pulls reservoirs
from a tight or wide neighbourhood. `radiusLitFactor` scales the "stretch
the search radius" multiplier when many nearby buckets are dim. Larger
factor → wider search → more lit reservoirs found in shadowed regions.

**Expected impact.** **Low-to-medium on shadowed-region noise** specifically.
The C# slider is 0..1000 (logarithmic), default 3.0. The C# ImGui helper
text reads *"Mitigates some of the darkening from resampling"* — i.e. the
exact paper §5.2 "darkening in B/C" limitation. So increasing it would
explicitly trade off "more search wash" for "less shadow darkening / noise".

**Affected files.** `lib.rs:84` — change the `3.0` default. **No shader
change** (the value is uniform-fed).

**Effort.** **Trivial.**

**Risk.** Too high a value erases the adaptive bias — every pixel pulls
from a huge neighbourhood, the GI loses local feature. The C# 0..1000 range
is huge; the paper doesn't quote a number, so 3.0 was hand-tuned. **Don't
raise blindly** — needs a measured sweep in the e2e harness.

**Validation strategy.** A small sweep of values (e.g. 3.0 / 6.0 / 12.0) in
`--sun-shadow-bench` mode; record the WARMUP→SETTLE *bounce-luminance* in
the designated shadow rect; pick the lowest variance. Visual regression
risk: the gate already checks `MIN_GI_BOUNCE_AFTER_MOTION = 150.0` — a
wider radius could push above (still pass) or below (would catch regression).

---

### 3.5 — Tune `noise_suppression_factor` (#5)

**Anchor.** C# `SettingDataRenderBase.noiseSupressionFactor = 0.4f`
(`WorldRenderBase.cs:24`, slider 0.01..100). Port `lib.rs:85` — 0.4. Used in
`refine_buckets` `sample_refine.wgsl` via the `darkening_offset` term:
`(distFac² * originalLitRatio * noiseSupressionFactor − 1)` clamped ≥ 0
(`renderSampleRefine.fx:387`). Larger value → more aggressive
distance-variance penalty when refining bucket survivors.

**What it does.** Suppresses the per-bucket variance contribution of
fireflies (samples whose distance-from-mean is large). 0.4 is moderate; the
C# slider exposes 0.01..100 (logarithmic), so the design space is large.

**Expected impact.** **Low on shadow-band noise** (this is fireflies-not-shadows),
but a separately useful knob for "GI sparkle reduction". Listed for
completeness because the user said "shadow filtering improvements" — but
narrowly the user wants shadow penumbra noise, which is #1's lever.

**Affected files.** `lib.rs:85` only.

**Effort.** **Trivial.**

**Risk.** Over-suppression → wash-out of legitimate bright samples →
darkening (paper §5.2 "darkening in B/C" limitation worsens). 0.4 is the
hand-tuned C# default; deviating from it deviates from faithfulness.

**Validation strategy.** Eyeball + the bright-region luminance gate (`solid`
rect of `--entities` mode; threshold 80, measured 187.93 — a wider headroom
than tight).

---

### 3.6 — Lift `BUCKET_STORAGE_COUNT` 32 → 48 or 64 (#6)

**Anchor.** Paper §4.2: *"up to 32 lit samples are stored for each region"*
(the cap). C# `globalIllumBucketStorageCount = 32` (`WorldRenderBase.cs:59`).
Port `BUCKET_STORAGE_COUNT = 32` (`gi.rs:57`). The bucket holds the **refined**
samples before brightness-leveling drops them to ≤8
(`REFINED_BUCKET_STORAGE_COUNT = 8`).

**What it does.** Increasing the cap lets the brightness-leveling pass see
more samples per region, so the `samples_comp_color_max` estimator (the
"max brightness in the region", `sample_refine.wgsl:619-633`) is better and
the leveling probability `COLOR_DIF_PROB[max_color_dif]` (`:659`) is fairer.
Reduces the per-region "missed-a-bright-sample" sparkle.

**Expected impact.** **Low-medium on overall GI variance**, not specifically
on shadow penumbras. The cap is rarely hit in practice (paper §4.2 says
"as is typically the case, few lit samples are generated due to
material-based sampling") — so the 32-cap probably isn't biting in the test
scene at all. Worth confirming with an instrumentation pass before tuning.

**Affected files.**
- `crates/bevy_naadf/src/render/gi.rs:57` — `BUCKET_STORAGE_COUNT`.
- The `comp_color_max_storage: array<u32, 32>` at
  `sample_refine.wgsl:622` is a function-scope array sized to the cap; must
  rise in lockstep with the constant. Also `:625` `for i < effective_valid_count`
  is bounded by the cap.
- Buffer-size impact: `valid_samples_refined` is sized `bucket_count *
  BUCKET_STORAGE_COUNT * 16 B` (`gi.rs:465`). At 1920×1080 (240×135 buckets =
  32400) — 32 → 64 doubles from ~16.6 MB to ~33.2 MB. Not VRAM-critical;
  paper §6 Table 4 reservoir total is 509 MB.

**Effort.** **Small** (const + WGSL array literal + buffer-size validation).

**Risk.**
- **Stack pressure.** `comp_color_max_storage: array<u32, 64>` per-thread
  may pressure shader register usage. Likely tolerable but verify on the
  e2e GPU.
- **Cost.** The per-bucket O(N) refinement loop runs longer; per-frame cost
  bumps slightly.

**Validation strategy.** Instrument first: add a debug counter for "buckets
where `effective_valid_count == BUCKET_STORAGE_COUNT`" — if it's near 0 on
the test scene, the change is dead weight. Only worth doing if the
instrumentation says the cap is actually being hit.

---

### 3.7 — Lift `VALID_SAMPLE_STORAGE_COUNT` 2 → 3/4 (#7)

**Anchor.** Paper §4.2: *"we chose two frames' worth of storage [for lit
samples], while still storing information from a large number of past frames
(maximum 64 frames)."* C# `globalIllumValidSampleStorageCount = 2`
(`WorldRenderBase.cs:57`). Port `VALID_SAMPLE_STORAGE_COUNT = 2` (`gi.rs:51`).
`valid_samples` is sized `pixel_count * 2`.

**What it does.** Increase the per-pixel lit-sample reservoir depth so more
frames of "this pixel saw a lit ray" survive into the temporal pass before
overwrite. Per the paper, "two frames" was chosen for the SAN MIGUEL test
scene; for sun-shadow band convergence specifically, more lit samples
retained → less chance the sun-tap's lit result is overwritten by a
neighbour's miss.

**Expected impact.** **Low-medium on dim/shadow-band stability.** This is
mostly relevant when GI is sparse (e.g. far from emitters), which describes
shadow bands. Going 2 → 4 doubles the lit-sample VRAM
(`pixel_count * 2 * 16 B = 64 MB → 128 MB @ 1920×1080`).

**Affected files.** `gi.rs:51` only. The shader reads through
`gi_params.valid_sample_storage_count` already (`gi_params.wgsl:93`), so no
WGSL edit. Buffer-size auto-updates in `create_gi_buffers`.

**Effort.** **Trivial** (one const). Buffer rebuilds on resource init.

**Risk.** VRAM impact is real (+64 MB on 2K). The C# value is hand-tuned
for SAN MIGUEL; deviating is a real deviation-from-C# (Q3 cross-check).

**Validation strategy.** Variance of shadow-band luminance pre/post; VRAM
check; visual eyeball.

---

### 3.8 — Lift `INVALID_SAMPLE_STORAGE_COUNT` 8 → 16 (#8)

**Anchor.** Paper §4.2: *"four frames worth of storage [...] we can
accumulate at least 32 frames for unlit samples."* C#
`globalIllumInvalidSampleStorageCount = 8` (`WorldRenderBase.cs:58`) — note
the C# is **2× the paper's prose** (already faithful-with-deviation in
`14-paper-gap.md` row 12). Port `INVALID_SAMPLE_STORAGE_COUNT = 8`
(`gi.rs:54`).

**What it does.** Doubles the unlit-sample retention; the unlit/lit ratio
(`bucket_info` `bucketLitRatio`) is sharper, leveling decisions better-informed.

**Expected impact.** **Low.** Unlit samples don't carry colour — only count
into the per-region "fraction lit" estimator. Doubling that mostly improves
the *rate of convergence*, not the steady-state shadow noise. Limited shadow
specifically.

**Affected files.** `gi.rs:54`. VRAM: `pixel_count * INVALID_SAMPLE_STORAGE_COUNT *
16 B` — at 1920×1080, 8 → 16 doubles 256 → 512 MB. **Significant.**

**Effort.** **Trivial.** VRAM impact is real.

**Risk.** VRAM (largest of the lever changes). Marginal quality return for
the cost.

**Validation strategy.** Same instrumentation as 3.6/3.7.

---

### 3.9 — `COLOR_DIF_PROB` curve tuning (#9)

**Anchor.** C# `commonColorCompression.fxh`-derived `COLOR_DIF_PROB[31]`,
`COLOR_DIF_PROB[i] = 1 - COLOR_EXP^(-i)` with `COLOR_EXP = 2^0.6 = 1.515716...`.
Port `color_compression.wgsl:70-102` (32 baked literals with a Rust
recompute `#[test]`). Used in `refine_buckets` (`sample_refine.wgsl:659`) —
the leveling-removal probability.

**What it does.** Re-tune the exponential curve. A *flatter* curve (smaller
`COLOR_EXP`) preserves more dim samples (better shadow detail, more
fireflies); a *steeper* curve culls dim samples more aggressively (cleaner
brights, but worse dim-region detail).

**Expected impact.** **Low — and risky.** The curve is hand-tuned to match
the `COLORS[]` LUT's exponential spacing; a mismatch desyncs the two tables.
The `Rust #[test] color_tables_match_wgsl` (`color_compression.rs`) keys
both off the same formula. Worth flagging only because the paper doesn't
prescribe the curve, but in practice this is far from low-effort —
re-tuning would need a fresh hand-design pass, not a literal tweak.

**Affected files.** `color_compression.rs` source formula + the 31 literals
in `color_compression.wgsl` (kept in sync by the test).

**Effort.** **Medium** (curve design + bake script + test update).

**Risk.** Faithfulness deviation. **Recommendation: leave it.** Only
revisit if 3.1–3.4 don't move the needle.

---

### 3.10 — Halton (3,7) coprime variation (#10 — flagged only)

**Anchor.** C# `findCoprime` resolves to bases (3,7) in practice
(`14-paper-gap.md` §4.1 Halton row, FAITHFUL-with-deviation S);
port `taa.rs:124` hard-codes (3,7).

**What it does.** Coprime selection affects the Halton sequence's
sub-pixel coverage uniformity. Different bases (e.g. (2,3), (3,5), (5,7))
might decorrelate temporal samples differently.

**Expected impact.** **Negligible.** The C# already picks (3,7); the port
matches in practice. No meaningful quality lever.

**Affected files.** `taa.rs:124`.

**Effort.** **Trivial** but **uninteresting.** Drop from the dispatch list
unless someone explicitly asks about Halton bases.

---

### 3.11 — Audit summary (port vs. C# config snapshot)

Cross-checked against `WorldRenderBase.cs:14-25` (`SettingDataRenderBase`
defaults) and `:57-60` (the four `globalIllumXxxStorageCount` consts):

| Setting | C# default | Port default | Faithful? |
|---|---|---|---|
| `bounceCount` | 3 | 3 (`lib.rs:79`) | yes |
| `globalIllumMaxAccum` | 128 | 128 (`lib.rs:80`) | yes |
| `taaSampleMaxAge` | 32 | 32 (`lib.rs:107`) | yes (TAA-fidelity) |
| `spatialResampleSize` | 500.0 | 500.0 (`lib.rs:81`) | yes |
| `spatialResampleVisibilityTestMaxDepth` | 80 (slider, dead uniform) | const 60 (`ray_tracing.wgsl:126`) | YES — port uses the live const, C# does too |
| `denoiseThresh` | 400 | 400.0 (`lib.rs:83`) | yes |
| `radiusLitFactor` | 3.0 | 3.0 (`lib.rs:84`) | yes |
| `noiseSupressionFactor` | 0.4 | 0.4 (`lib.rs:85`) | yes |
| `skipSamples` | true | true (`lib.rs:86`) | yes |
| `isDenoise` | true | true (`lib.rs:87`) | yes |
| `isSampleLeveling` | true | true (`lib.rs:88`) | yes |
| `isVaryingResmaplingRadius` | true | true (`lib.rs:89`) | yes |
| `isAtmosphereInteraction` | true | true (`lib.rs:90`) | yes |
| `globalIllumValidSampleStorageCount` | 2 | 2 (`gi.rs:51`) | yes |
| `globalIllumInvalidSampleStorageCount` | 8 | 8 (`gi.rs:54`) | yes |
| `globalIllumBucketStorageCount` | 32 | 32 (`gi.rs:57`) | yes |
| `globalIllumRefinedBucketStorageCount` | 8 | 8 (`gi.rs:60`) | yes |
| `0.9999` sun cone deviation | hard-coded | hard-coded | yes |
| `MAX_RAY_STEPS_SUN` | 120 | 120 (`ray_tracing.wgsl:124`) | yes |
| `MAX_RAY_STEPS_SUN_SECONDARY` | 80 | 80 (`ray_tracing.wgsl:125`) | yes |
| `MAX_RAY_STEPS_VISIBILITY` | 60 (live, dead uniform 80) | 60 (`ray_tracing.wgsl:126`) | yes |
| spatial-resampling iterations | 12 (call-site) | 12 (`spatial_resampling.wgsl:601`) | yes |
| 8×8 region size | hard-coded | hard-coded | yes |
| GI ray jitter | jittered | jittered (`taa_jitter`) | yes (TAA-fidelity) |

**Conclusion: every C# config knob and every numeric constant is faithfully
ported.** This is purely a **paper §5.2 limitation** improvement track, not
an alignment-gap fix.

---

## §4. Top-3 recommendation

Synthesised from §3. The dispatch order should be:

### Dispatch A — Multi-tap sun-shadow in spatial resampling (and optionally per-secondary-bounce sun)

The **single highest-leverage change for shadow noise.** Paper §5.2 calls
it out by name. Small / well-bounded; one shader location + (optionally) a
mirror in `naadf_global_illum.wgsl`. Variance reduction ∝ √N per tap.
**Hard-code N=4** to start (no uniform field needed → no `vec3`-then-scalar
hazard); if the result is good, leave; if not great, lift to a runtime knob.
Effort: small. Risk: cost scaling (4×120-step ray traces — measure the
frame-time bump and back off to N=2 if hot).

### Dispatch B — Bump spatial-resampling iterations 12 → 16

A single-literal change with √(N) variance reduction on indirect-bounce
convergence overall. Independent of A; can land in the same dispatch or a
follow-on. The C# 12 is a quality knee; nudging to 16 trades a small cost
for measurable noise reduction. Effort: trivial. Risk: linear cost.

### Dispatch C — `radius_lit_factor` sweep (3.0 → ?)

The C# ImGui slider hint literally says this is the "mitigate darkening
from resampling" knob — the paper §5.2 limitation #2 ("darkening in B/C").
Worth a 3-value e2e sweep (3.0 / 6.0 / 12.0) to find the local optimum on
*our* test scene (the GI-lit `solid` rect's luminance variance under
camera motion). Effort: trivial. Risk: deviation-from-C# only, no
correctness risk.

### Forced-pick: the single highest-leverage change

**Dispatch A.** Sun-shadow multi-tap. It is the only candidate that
specifically targets the **paper-acknowledged limitation the user is asking
about**; everything else is a general convergence dial.

### Diminishing-returns rest

3.6/3.7/3.8/3.9/3.10 are sub-percent quality improvements with real
correctness/VRAM/maintenance costs. **Do not dispatch them in this track.**
3.2 is an audit-only confirmation that the port already matches C#.

---

## §5. Phase-D-shadow proposed dispatch shape

Mirroring the `15-design-c.md` §11 seam pattern: small surface area, single
worktree, one or two workstreams. **The dispatches in §4 do NOT need
parallel worktrees** — they all touch the same 1-2 files.

### 5.1 Single workstream (recommended)

```
/.claude/worktrees/gi-reservoir-shadow/
  branch: feat/gi-reservoir-shadow
  scope: §4 Dispatch A (+B as a follow-on commit if A lands cleanly)
```

**Seam.** The reservoir-improvement work touches only:
- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:529-560`
  (sun loop unroll)
- `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:601`
  (12 → 16 iterations — Dispatch B as a 1-line commit)
- *(optional)* `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl:346-380`
  (per-secondary-bounce sun-tap — only if Dispatch A measures well in the
  spatial pass)

No new buffers, no new bindings, no `GpuGiParams` layout edit (if N is
hard-coded). The Rust render side does NOT need touching. The seam is
**zero**.

**Sub-agent ordering.** Three sub-agents, sequential:

1. `delegate-implementer` — apply Dispatch A in the worktree. Hard-code
   `N=4`. Run `cargo build` + `cargo test` + `cargo run --bin e2e_render`
   green. Inspect screenshot + the post-MOTION luminance value — confirm
   shadow regions are visibly softer.
2. `delegate-implementer` (or same agent, second commit) — apply Dispatch B
   (one-line iter count bump). Re-run e2e. Confirm no regression.
3. `delegate-reviewer` — fresh-eyes review of the diff. Specifically
   audit: cost of the 4-tap loop on the e2e box (if frame-time bump is
   meaningful, recommend N=2 instead); verify `MAX_RAY_STEPS_SUN` cap not
   blown; confirm the `0.9999` cone width unchanged (a different cone is a
   different deviation from C#).

### 5.2 If a runtime knob is wanted

Replace step 1 with: add `GpuGiParams.sun_shadow_taps: u32` (mind the
trailing-pad / `offset_of!` guard pattern from `18-taa-fidelity.md` fix #1
— place after the existing `taa_jitter` row's vec4, on a fresh
16-byte-aligned slot; do NOT bury inside the `flags`/`pad_a` row). This
adds Rust plumbing through `extract_gi` and `prepare_gi`, plus a CLI flag
+ `AppArgs.gi.sun_shadow_taps`. Bumps Dispatch A from small to medium.

### 5.3 If Dispatch C (radius_lit_factor sweep) is included

Pure CLI flag + a new e2e mode that runs the harness N times with N values
of `radius_lit_factor`, dumping a per-run luminance variance number. Belongs
in a separate sub-workstream (probably a dedicated bench harness), not in
the shader-mutation workstream. Effort: small but logically separate.

---

## §6. Out-of-scope (please confirm)

These are **not** addressed in this track. The user can flip any of them
back in by adding them to the dispatch brief:

- **SVGF or any denoiser replacement.** `14-paper-gap.md` row §4.3 — un-portable
  from the NAADF source, and the paper itself favours the bilateral.
- **Bilateral kernel changes** (kernel size, σ, sparsity ratio). `denoise_split.wgsl`
  itself is faithful per `14-paper-gap.md`; out of this track's scope.
- **TAA pipeline changes.** Reprojection rejection, hash validation,
  3×3 min/max depth, the 32-deep ring depth, the 128-deep camera ring — all
  TAA-internal and untouched here. The TAA-fidelity track already closed the
  TAA fidelity gap.
- **Atmosphere model changes.** The sun colour comes from the atmosphere
  precompute; if the user wants brighter / dimmer sun, that's an atmosphere
  edit, not a reservoir one.
- **ReSTIR scheme overhaul** — ReSTIR PT, separating direct lighting into
  a dedicated ReSTIR DI pass, any structural rewiring of the §4.2 pipeline.
  The user's ask is "hit the reservoir" — interpreted as *tune the existing
  reservoir machinery*, not *replace it*.
- **First-hit pass / G-buffer changes.** 4-plane encoding, `compress_first_hit_data`,
  the `i==4` mirror-tail — unchanged.
- **`rayQueueCalc` adaptive-sampling logic.** The `modSize` formula is
  faithful; not a target.
- **The `vec3`-then-scalar layout hazard sweep** (`11-review-b.md` finding
  6). Already addressed for new structs in Phase C; any *new* uniform field
  this track adds gets an `offset_of!` guard, but a retroactive sweep of
  pre-Phase-C structs is not on this track.
- **Phase-D `B-7` storage-texture barrier** (`12-alignment-gap.md` §4).
  Independent of reservoir work.
- **`entity_instances_history` TAA reprojection of moving entities** —
  separate Phase-D track per `12-alignment-gap.md` §6.

---

## §7. Open questions for the user

Three questions whose answers would meaningfully change the dispatch plan.
Each is a fork in the road; the dispatch can't pick without an answer.

1. **Hard-coded N=4 sun taps, or runtime-configurable?** Hard-coded keeps
   the dispatch tiny (Dispatch A → small, ≈10 WGSL lines + an iter-loop
   bump in Dispatch B). Configurable adds a `GpuGiParams.sun_shadow_taps:
   u32` field, an `AppArgs.gi.sun_shadow_taps`, a CLI flag — bumps the
   dispatch to medium with the layout-trap risk class. The faithfulness
   argument leans hard-coded (the C# also hard-codes N=1); the
   experimentation-friendliness argument leans configurable.

2. **Should the per-secondary-bounce sun sample in `naadf_global_illum.wgsl:346-380`
   also be multi-tapped, or only the spatial-resampling sun sample at
   `spatial_resampling.wgsl:529-560`?** They are the *same* one-tap
   structure with the same `0.9999` cone. Multi-tapping both decorrelates
   shadow noise across both passes; multi-tapping only the spatial pass is
   cheaper and the spatial pass is what writes the final colour. The cost
   scaling for the per-bounce version is N × 80 ray steps × ≤3 bounces per
   GI ray, which is more aggressive than the spatial pass's N × 120 × 1
   per pixel.

3. **Is `radius_lit_factor` worth a sweep (Dispatch C), or is that a
   separate hand-tuning thread the user wants to drive interactively?**
   The paper §5.2 says this knob "mitigates darkening from resampling" —
   the closest knob to the residual shadow/dim-region quality complaint
   that *isn't* a code change. If the user prefers to interactively tune
   it later (after the configurable knob exists), keep it out of this
   track. If the user wants a number picked now, Dispatch C bakes the
   e2e-best value as the new `lib.rs` default.

---

## Design

(Already inline above in §§2-5. The implementation plan is the per-candidate
"Affected files" + the §5 worktree + dispatch shape. No code changes are
made in this track — the design lands as the per-dispatch brief.)

## Decisions & rejected alternatives

1. **Chose: list ALL 10 surfaced candidates with rankings.** Rejected:
   only listing the top 3. Reason: the user asked for "the GI reservoir"
   broadly; surfacing the diminishing-returns levers explicitly is what
   keeps the orchestrator from later dispatching them under-justified.
   Flip-trigger: the user says "be terser, only show me top 3" — drop §§3.6
   onward to a one-line "candidates considered, rejected" mention.

2. **Chose: position the sun-shadow multi-tap (3.1) as the dominant
   recommendation.** Rejected: positioning the spatial-iteration bump (3.3)
   as #1. Reason: the user's framing was "shadow filtering" — the spatial
   iter bump improves indirect-lighting noise in *general*, but the
   single-sun-sample-tap-per-frame is what the paper §5.2 names as the
   specific noise source the user is asking about. Flip-trigger: if the
   user says "I'm not worried about sun shadows specifically — I want
   overall less GI noise," reorder 3.3 to #1.

3. **Chose: keep N=4 as the default starting tap count.** Rejected: N=8 or
   N=2. Reason: N=4 is the lowest interesting tap count for visible
   variance reduction (√4 = 2× variance drop) without doubling the
   shadow-ray cost in a way that risks frame-time. N=2 is marginal; N=8
   would be 8× the visibility-ray cost in the spatial pass. Flip-trigger:
   if Dispatch A's frame-time hit is excessive on the e2e box, drop to
   N=2; if the visual gain is too subtle, raise to N=8.

4. **Chose: keep the `0.9999` cone deviation untouched.** Rejected: tighter
   (0.99999, physical-correct sun) or widening (0.999, fuzzier penumbra).
   Reason: the C# baked-in value is the faithful-port anchor; changing it
   is a deviation. The "soft shadow" the user wants comes from *N taps in
   the same cone*, not from changing the cone width. Flip-trigger: if the
   user explicitly wants physically-correct sun penumbras (rare in game
   content), narrow to 0.99999.

5. **Chose: avoid configurable sun-shadow-tap-count (Open Question 1
   default).** Rejected: the configurable variant. Reason: faithful-port
   bias + the layout hazard makes hard-coded the safer first pass.
   Flip-trigger: the user's answer to OQ1.

6. **Chose: scope the sun multi-tap ONLY in `spatial_resampling.wgsl`
   first** (Open Question 2 default). Rejected: simultaneously multi-tap
   `naadf_global_illum.wgsl`'s per-bounce sun. Reason: the spatial pass is
   the final colour-write site; if the spatial multi-tap measures well, the
   per-bounce version is a follow-on at known-low risk. Flip-trigger: OQ2's
   answer.

7. **Chose: NO retroactive `vec3`-then-scalar audit, NO mechanical
   offset-assert harness** (the `11-review-b.md` finding-6 follow-up).
   Reason: this track is about reservoir quality, not the layout-trap meta;
   the Phase-C `offset_of!` guard pattern (77 sites) is the de-facto
   solution for new structs. Flip-trigger: a future struct edit (not in
   this track) re-introduces the bug — then revisit.

8. **Chose: leave `MAX_RAY_STEPS_VISIBILITY = 60` alone** (the C# slider
   range is 0..80 but the live const is 60). Reason: raising it is a
   light-leak fix, not a shadow-noise fix; the user asked about the
   latter. Flip-trigger: if the user explicitly asks about light leaks
   ("things look hollow / wrong indoor lighting"), revisit.

9. **Chose: single worktree, one workstream** (§5.1). Rejected: parallel
   worktrees Phase-C-style. Reason: the changes touch 1-2 files in a
   single shader subsystem — no seam to split. Phase-C parallelism was
   load-bearing because of *file-isolation between independent subsystems*;
   here there is no isolation to gain. Flip-trigger: if the user requests
   sweep-Dispatch-C alongside Dispatch-A, split off the bench-harness work
   into its own worktree.

10. **Chose: NOT include adaptive-radius bias toward shadow regions
    specifically (the brief's #9 surface)** as its own dispatch. Reason: that
    *is* `radius_lit_factor` tuning (Dispatch C / §3.4) — they're the same
    knob. The brief described it as a separate item but on inspection the
    "adaptive radius" mechanic at `renderSpatialResampling.fx:81-148` is
    *already shadow-aware* (the `worstLitSmall` term penalises dim
    neighbourhoods so the radius widens there). The lever is just the scale
    factor. Flip-trigger: if the user is asking for an entirely new
    shadow-detector in the adaptive-radius pre-pass, that becomes a
    medium-effort shader change — out of this track's "quality knobs only"
    framing.

11. **Chose: not surface the `globalIllumMaxAccum = 128` knob (the
    sample-counts ring depth)** as a candidate. Reason: this is the
    *per-pixel hit count* ring that drives the adaptive 0.25-spp signal
    via `accum_index`. Tampering with it would change the adaptive-sampling
    behaviour, not shadow filtering. Flip-trigger: the user redirects the
    track to general indirect convergence (not shadows).

## Assumptions made

1. **The user's framing "GI reservoir" means the §4.2 compressed-ReSTIR
   pipeline.** Not a brand-new reservoir scheme (ReSTIR DI/PT) — *tune the
   existing one.* Surfacing this in OQ implicitly: the dispatch plan
   assumes the user wants knobs-and-multi-tap within the paper's frame.

2. **The user's "shadow filtering" specifically means sun-shadow penumbra
   noise**, not light-leak fixes (visibility-ray cap) or general indirect
   convergence. The TAA-fidelity track post-note ("ways to improve shadow
   filtering in the future would help significantly") was made
   post-multi-bounce-GI-restoration, so "shadow" reads as the
   sun-disk-shadow-edge artifact, not "lit-from-emitter" indirect bounce
   shadow.

3. **The dev box (16 GB RTX 5080, `18-taa-fidelity.md` fix #3 context)
   tolerates a 4× cost bump on the spatial-resampling sun ray.** N=4 ×
   `MAX_RAY_STEPS_SUN = 120` per pixel adds roughly 4 × the current
   sun-ray cost to the spatial pass. Mitigation in §3.1 risk section.

4. **The C# `0.9999` cone deviation is the intended sun-disk width, not a
   bug.** Reading `commonRayTracing.fxh:75-84` shows it's a deliberate
   parameterisation; reading the 3 use sites confirms a single value is
   used consistently.

5. **No new e2e harness mode is needed for Dispatch A alone.** The
   `--moving-camera` mode already exercises shadow regions under motion;
   a new `--sun-shadow-bench` harness is a future Dispatch-C enabler,
   not a prerequisite. If the user disagrees, the dispatch grows.

6. **The brief's `spatial_resampling.wgsl:529-538` line range for the sun
   ray** is approximate; the actual range in the audited code is
   `:529-560` (verified). Implementation briefs should cite the precise
   range.

7. **`14-paper-gap.md` row §4.2 "Soft sun shadows during resampling" being
   marked FAITHFUL** means the port faithfully reproduces NAADF's single-tap
   sun sample — *including the paper's stated limitation.* This track does
   NOT close a gap; it deviates from C# *to improve* on the paper-stated
   limitation. That is a deliberate deviation, and Dispatch A's PR-body
   /design-doc should record it explicitly as a deviation-from-faithfulness
   in the same family as the TAA-fidelity track's Bevy-tonemapping switch.

8. **The `sun_color` term comes from the same atmosphere source for both
   the per-bounce and spatial passes** — verified `gi_params.sun_color`
   is shared, populated once per frame by `prepare_gi:303` and read in both
   shader sites. Multi-tapping in either site is internally consistent.

9. **The configurability of `taa_ring_depth` (the supporting
   `TaaRingConfig` resource pattern from `18-taa-fidelity.md` fix #3)
   provides a template** for any `sun_shadow_taps` runtime knob — same
   render-world resource + shader-def-or-uniform pattern. The dispatch
   brief can point at it as the established idiom.

10. **`radius_lit_factor` and `noise_suppression_factor` are unchanged from
    C# 3.0 / 0.4 across the entire port history** — verified the
    `lib.rs:84,85` values landed in Phase B Batch 1 (`10-impl-b.md`) and
    were never edited. Any sweep would be the *first* deviation in this
    config knob's port history.
