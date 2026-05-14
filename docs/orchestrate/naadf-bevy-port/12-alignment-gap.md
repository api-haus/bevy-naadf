# 12 — Alignment Gap Analysis: Rust/Bevy port vs. NAADF C# reference

**Date:** 2026-05-15
**Author:** delegated gap-analysis agent (read-only on code)
**Scope:** comparative assessment of how far the Rust/Bevy port
(`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`, branch
`feat/phase-b-gi`) is from the NAADF C#/MonoGame reference
(`/mnt/archive4/DEV/NAADF`), and what is left to fully align *within the agreed
scope*.

This builds on `02-research.md` (the subsystem map), the design docs
(`03/06/09-design*.md`), the impl logs (`04/07/10-impl*.md`), and the review
docs (`05/08/11-review*.md`); claims were spot-verified against the actual code
of both trees. Where this document and an orchestrate doc disagree, the code
was checked and the code wins.

---

## 1. Scope recap

### In scope (the core engine)

Per `01-context.md` Q1–Q4 + the four-phase split: **voxel grid + AADF data
structure + world data/buffers + the real-time render pipeline.** Concretely:

- The three-layer chunk/block/voxel cell hierarchy, CPU-side AADF construction,
  DDA-with-AADF traversal (Phase A).
- The `PositionSplit` int+frac camera (D1), a hard-coded procedural test grid
  (D2), the `GrowableBuffer` GPU-buffer abstraction.
- NAADF's 16-frame long-term-memory TAA (Phase A-2 — the §6 VRAM lever: 16-deep
  sample ring instead of 32).
- The full real-time `WorldRenderBase` GI pipeline (Phase B): 4-plane first-hit,
  `rayQueueCalc` adaptive ~0.25-spp sampling, compressed ReSTIR GI
  (`globalIllum` + the 5-pass `sampleRefine` + `spatialResampling` Algorithm 2),
  the sparse bilateral denoiser, the atmosphere model, the `base/` TAA rewire,
  and the final blit.

### Intentionally OUT of scope — NOT gaps

These are deliberate non-goals, not deficiencies (see §5 for the full list):
**editor GUI; `.cvox` persistence/serialization; the `.vox`/`obj2voxel`/Voxlap
asset importers; the reference pathtracer (`WorldRenderPathTracer` /
`pathTracer/**`); DLSS / DLSS-RR; and Phase C (GPU world construction/editing —
`chunkCalc.fx` / `boundsCalc.fx` / `worldChange.fx`).** Anything in this list
appearing as "missing" below is correct-by-design, not a gap.

### Current status

Phase A (substrate + albedo) and Phase A-2 (TAA) are **review-gated PASS**.
Phase B (GI) is **impl feature-complete and review-gated PASS** (`11-review-b.md`
— 0 blockers, 2 concerns, 5 nits) — but with **one open production bug** (the
camera-motion TAA decay, §4) outstanding after the review. 48 `#[test]`
functions in-tree, all passing (the orchestrate docs say "46" — see §4 finding;
the discrepancy is two later substrate tests, not a regression). `cargo build`
clean; `cargo run --bin e2e_render` exits 0, all gates green at 99.2% GI-lit.

---

## 2. Subsystem-by-subsystem alignment table

Faithfulness legend: **faithful** = behaviourally matches NAADF with only
mechanical HLSL→WGSL / MonoGame→wgpu adaptations; **faithful-with-deviations** =
faithful but carries documented, justified divergences; **diverges** = a real
behavioural difference.

| # | NAADF subsystem | NAADF source | Port location | Ported? | Faithfulness | Notes / divergences |
|---|---|---|---|---|---|---|
| 1 | Voxel grid / cell hierarchy (chunk/block/voxel 4³) | `World/Data/WorldData.cs`, paper §3.1 | `src/aadf/cell.rs`, `src/world/data.rs` | **yes** | faithful-with-deviations | 3-layer hierarchy + 2-bit/top-bit state encoding ported. Re-derived from paper per Q3, not transliterated. Voxels packed 2-per-`uint`. |
| 2 | AADF construction | `chunkCalc.fx`, `boundsCommon.fxh`, paper §3.2-3.3 Algorithm 1 | `src/aadf/construct.rs`, `src/aadf/bounds.rs` | **yes (CPU only)** | faithful-with-deviations | CPU re-derivation of Algorithm 1 — a Rust `HashMap` keyed on the 64-voxel array replaces the GPU open-addressing hash (the exact `31^(64-i)` hash is GPU-construction-specific, Phase C). Local 4³ AADF + the alternating-axis expansion ported. **GPU construction is Phase C — intentionally deferred, not a gap.** |
| 3 | DDA-with-AADF traversal (`shoot_ray`) | `render/rayTracing.fxh:73` | `src/assets/shaders/ray_tracing.wgsl` | **yes** | faithful-with-deviations | Phase-A core, reviewed-gate PASS. Entity sub-traversal branch (`#ifdef ENTITIES`) omitted (entities are a deferred sub-feature — §3). Reused unchanged by Phase B for GI secondary + visibility rays. |
| 4 | World data + GPU buffers | `WorldData.cs`, `DynamicStructuredBuffer.cs` | `src/world/data.rs`, `src/world/buffer.rs`, `src/render/prepare.rs` | **yes** | faithful-with-deviations | `WorldData` is a Bevy `Resource`; `GrowableBuffer` is the `DynamicStructuredBuffer` equivalent. Chunk buffer as a wgpu texture/buffer per design. CPU world mirror kept. **No world generator** — D2's hard-coded test grid (`src/voxel/grid.rs`) is the content path; `WorldGenerator` is deferred (acceptable per D2, see §6). |
| 5 | Voxel type / layered-material system | `World/VoxelTypeHandler.cs` | `src/voxel/mod.rs` (`VoxelType`), `src/render/gpu_types.rs` (`GpuVoxelType`, 16 B) | **yes** | faithful | Follows the C# 128-bit `Uint4` material entry (divergence #1 — paper says 16 bit, C# is the source of truth). Diffuse/Emissive/MetallicRough/MetallicMirror enums ported. |
| 6 | `PositionSplit` int+frac camera | `Common/Camera.cs` | `src/camera/position_split.rs` | **yes** | faithful-with-deviations | D1: ported faithfully, threaded through every WGSL pass. `M*v` glam convention replaces HLSL `mul(v,M)` (the Phase-A perspective fix). **Carries the latent `sync_position_split` `With<FreeCamera>` filter bug — fixed 2026-05-15 (§4).** |
| 7 | Atmosphere model (precompute + apply) | `Atmosphere.cs`, `atmosphereRaw.fxh`, `atmospherePrecomputed.fxh`, `base/renderAtmosphere.fx` | `src/render/atmosphere.rs`, `src/assets/shaders/atmosphere.wgsl`, `naadf_atmosphere.wgsl` | **yes** | faithful | Multiple-scattering sky model + CPU `Atmosphere::get_light_for_point` + the octahedral quarter-per-frame precompute. `apply_atmosphere` split into `atmosphere_oct_index` + value-taking fn (wgpu forbids `ptr<storage>` params — forced, faithful). Downward-ray fade-to-dark is NAADF-faithful (no horizon term in the model). |
| 8 | 4-plane first-hit (G-buffer) | `base/renderFirstHit.fx` | `src/assets/shaders/naadf_first_hit.wgsl` | **yes** | faithful | Reviewer spot-checked verbatim: the 4-iteration specular-bounce loop, the `i==4` mirror-tail, `applyAtmosphere`/`addLightForDirection` gating, the 3 output writes. `compress_first_hit_data` bit-layout matches. Entity branches dropped. |
| 9 | `rayQueueCalc` adaptive ~0.25-spp sampler | `base/rayQueueCalc.fx` | `src/assets/shaders/ray_queue_calc.wgsl` | **yes** | faithful | `should_ray` `mod_size`, the inline `addToCounterAddressBuffer` group-shared prefix-counter, `calcRayQueueStore`. The adaptive signal is **real and wired end-to-end**: consumes `taa_sample_accum.x` → drives the indirect `globalIllum` dispatch (reviewer criterion 2 — met). |
| 10 | Compressed ReSTIR GI — `globalIllum` | `base/renderGlobalIllum.fx` | `src/assets/shaders/naadf_global_illum.wgsl` | **yes** | faithful | ≤3-bounce secondary-ray tracer, lit/unlit classification, 5-bit color compression, group-shared sample-count atomics, wrapping ring write. Entity branches dropped. |
| 11 | Compressed ReSTIR GI — `sampleRefine` (5 passes) | `base/renderSampleRefine.fx` | `src/assets/shaders/sample_refine.wgsl` | **yes** | faithful-with-deviations | All 5 passes (`ClearBucketsAndCalcMask`, `ValidHistory`, `CountValidAndRefine`, `CountInvalid`, `RefineBuckets`) ported function-by-function. `COLOR_DIF_PROB` brightness-leveling ported (divergence #10). **Forced wgpu deviation:** the `valid_dispatch`/`invalid_dispatch` indirect-arg buffers split into a dedicated `@group(1)` because wgpu forbids `STORAGE_READ_WRITE`+`INDIRECT` in one dispatch scope — faithful to design intent. |
| 12 | Compressed ReSTIR GI — `spatialResampling` (Algorithm 2) | `base/renderSpatialResampling.fx` | `src/assets/shaders/spatial_resampling.wgsl` | **yes** | faithful | 12-iteration neighbour-reservoir loop, adaptive-radius 12-tap pre-pass, Jacobian, single 3-step visibility ray, independent sun sample, the denoise/non-denoise write split. `spatialVisibilityCount` is a dead uniform in the HLSL — correctly dropped (divergence, see §3). |
| 13 | Sparse bilateral denoiser | `base/renderDenoiseSplit.fx` | `src/assets/shaders/denoise_split.wgsl` | **yes** | faithful | Kernel 21, σ=10, separable horizontal+vertical, sparse per-row/-column random offset, color+geometry bilateral weights, transposed indexing ported exactly. Runtime-gated on `is_denoise` (default `true`). **SVGF alternative not ported — it is not in the in-scope NAADF source (divergence #11), not a gap.** |
| 14 | `base/` long-term-memory TAA (`ReprojectOld` + `CalcNewTaaSample`) | `base/renderTaaSampleReverse.fx`, `commonTaa.fxh` | `src/assets/shaders/taa.wgsl`, `taa_common.wgsl`, `src/render/taa.rs` | **yes** | faithful-with-deviations | 16-deep sample ring (§6 VRAM lever — deliberate, not a gap), 128-deep camera-history ring (NAADF's depth, kept), `taa_dist_min_max` output wired, `screenPosDistanceSqr > 16.0` (the genuine `base/` value vs `albedo/`'s `1.0`). **Carries the open camera-motion reprojection decay bug (§4).** |
| 15 | Final blit (`renderFinal`) | `base/renderFinal.fx` | `src/assets/shaders/naadf_final.wgsl`, `src/render/graph.rs` | **yes** | faithful | `base/` variant: `taa_sample_accum` blit source, `tone_mapping_fac` tonemap denominator, `showRayStep` debug. The `Cube`+fullscreen-PS pattern (divergence #9) replaced with a Bevy fullscreen pass — forced, faithful. |
| 16 | Render-graph dispatch order | `WorldRenderBase.cs:205-441` | `src/render/mod.rs:207-228` | **yes** | faithful | Verified line-by-line against the C#: atmosphere → first_hit → ReprojectOld → ClearBucketsAndCalcMask → RayQueue(+Store) → GlobalIlum → ValidHistory → CountValid → CountInvalid → RefineBuckets → SpatialResampling → Denoise(H+V) → CalcNewTaaSample → renderFinal. 14 node systems realising NAADF's 16-dispatch order. |

**Summary: 16 in-scope subsystems assessed. 7 faithful, 9
faithful-with-deviations, 0 diverging.** Every deviation is either a forced
wgpu/naga adaptation, a deliberate scope decision (16-deep ring, CPU
construction, entity-branch omission), or a documented NAADF-internal
per-variant difference. No subsystem behaviourally diverges from NAADF intent.

---

## 3. Known divergences & open questions — status reconciliation

### The ~11 divergences from `02-research.md` §6

| # | Divergence | Status | Notes |
|---|---|---|---|
| 1 | Material entry width — paper 16 bit vs. C# 128-bit `Uint4` | **DELIBERATE / RESOLVED** | Port follows the C# 128-bit `Uint4` (`GpuVoxelType`, 16 B). Correct call per Q3 (C# is the correctness cross-check). |
| 2 | Hash-probe limit & resize threshold (paper 100/75% vs. C# 250/50%) | **N/A — DEFERRED** | Only relevant to GPU hashing construction (Phase C). The Phase-A CPU construction uses a Rust `HashMap` — no probe limit, no resize threshold. Becomes live only when Phase C ports `chunkCalc.fx`. |
| 3 | Two AADF "state" encodings in C# (2-bit `>>30` vs. traversal's `>>31` + `&0x40000000`) | **RESOLVED** | The port replicates the traversal's top-bit/uniform-full encoding exactly (Phase-A review-gate PASS confirmed traversal coherent in/out of volume). |
| 4 | Voxels packed two-per-`uint` | **RESOLVED** | Ported (`src/world/data.rs` / `ray_tracing.wgsl`). Phase-A review confirmed traversal correctness. |
| 5 | TAA history depth — 128 camera-matrix ring vs. 32 sample ring | **DELIBERATE / RESOLVED** | Camera-history ring kept at 128 (NAADF depth — tiny VRAM). Sample ring is **16-deep**, not 32 — the binding `design-exploration-qa.md` §6 VRAM lever. This is a deliberate, sanctioned scope decision, **not** a gap. |
| 6 | `PositionSplit` int+frac camera is pervasive | **RESOLVED (D1)** | Ported faithfully and threaded through every WGSL pass. |
| 7 | Atmosphere in-scope-by-necessity, not a paper contribution | **RESOLVED** | Full atmosphere model ported in Phase B (subsystem #7). Phase A used the inline sun term; Phase B has the precomputed octahedral model. |
| 8 | World sized in "world-gen segments" | **DELIBERATE / DEFERRED** | The port uses a single hard-coded test grid (D2), not the segment-by-segment `GenerateWorld`. The segment machinery is part of `WorldGenerator` (deferred — §6). Not a gap within the agreed scope. |
| 9 | `Cube` + fullscreen-PS final-blit pattern | **DELIBERATE / RESOLVED** | Replaced with a Bevy fullscreen pass — the design's explicit choice. Forced, faithful. |
| 10 | `renderSampleRefine` `RefineBuckets` uses `COLOR_DIF_PROB` | **RESOLVED** | The `COLOR_DIF_PROB[31]` table is ported as hard-coded WGSL literals + a `#[test]` (`color_compression.rs`) that recomputes from the source formula and asserts a bit-exact match. |
| 11 | SVGF not in the in-scope NAADF source | **N/A** | No SVGF shader exists to port. Not a gap. |

### The ~7 open questions from `02-research.md` §7

| # | Question | Status |
|---|---|---|
| 1 | Port `PositionSplit` or not? | **RESOLVED (D1)** — ported faithfully. |
| 2 | `DynamicStructuredBuffer` → wgpu wrapper; chunked-copy needed? | **RESOLVED** — `GrowableBuffer` (`src/world/buffer.rs`) implements re-alloc + `copy_buffer_to_buffer` on growth. The DX11 `dataCopy.fx` chunked-copy workaround was not needed in wgpu. |
| 3 | Chunk buffer as 3D texture vs. buffer | **RESOLVED** — chosen in `03-design.md` and implemented; the entity-widening (`Rg64Uint`) does not apply since entities are deferred. |
| 4 | Phase-A content path | **RESOLVED (D2)** — hard-coded procedural test grid (`src/voxel/grid.rs`, shared production+e2e). No `.vox` reader, no `WorldGenerator`. |
| 5 | Entities — Phase-A sub-feature or deferred? | **DELIBERATE / DEFERRED** — entities are not ported. Every `#ifdef ENTITIES` block was omitted in all WGSL. The entity data model (`EntityHandler`, `EntityData`, `entityUpdate.fx`, the 64-bit chunk widening) is a deferred sub-feature — see §6. |
| 6 | `taaSampleMaxAge` for the albedo path — TAA in Phase A or B? | **RESOLVED (D4)** — TAA pulled into its own gated Phase A-2. |
| 7 | Solari strip-vs-dormant | **RESOLVED (D3)** — stripped entirely. `bevy_solari` removed from `Cargo.toml`, no Solari symbols remain. |

### Divergences discovered since `02-research.md`

These surfaced during impl/review and are all documented in `10-impl-b.md`:

- **D-A. The `vec3`-then-scalar `#[repr(C)]`-vs-WGSL layout trap — recurred 3×.**
  `AtmosphereParams` (Batch 1), `GpuTaaParams` (Batch 6), `GpuGiParams` (the
  GI-bounce-visibility fix). **All three RESOLVED** — the WGSL structs now use
  `vec4` rows so the Rust `_padN` u32s become `.w` lanes. Verified in code:
  `gi_params.wgsl`, `taa.wgsl`, `atmosphere.wgsl` all carry `vec4`/explicit-pad
  forms. The reviewer re-audited every shared struct and found no fourth
  instance. **This is a faithful-port adaptation hazard, not a NAADF
  divergence** — NAADF's HLSL `cbuffer` packing and its C# uploader agree by
  construction; the bug is purely a wgpu-side porting trap. (Review concern #6:
  the recurrence makes future WGSL struct edits high-risk — see §6 polish.)
- **D-B. `screenPosDistanceSqr` threshold differs per render variant.** The
  `albedo/` TAA genuinely uses `> 1.0`, the `base/` TAA genuinely uses `> 16.0`
  (verified against `albedo/renderTaaSampleReverse.fx:133` vs.
  `base/renderTaaSampleReverse.fx:139`). The port uses the correct value per
  variant. **DELIBERATE / RESOLVED** — a real NAADF per-variant difference,
  faithfully reproduced; no A-2 bug.
- **D-C. `spatialVisibilityCount` is a dead uniform in NAADF's HLSL.**
  `renderSpatialResampling.fx` declares it but `sampleNeighbors` passes the
  `MAX_RAY_STEPS_VISIBILITY` const to `shootRay` directly. The port drops the
  uniform and uses the const. **DELIBERATE / RESOLVED** — faithful to NAADF's
  *actual behaviour* (not its dead declaration).
- **D-D. wgpu `STORAGE_READ_WRITE`+`INDIRECT` exclusivity** — forced the
  `sampleRefine` `@group(1)` indirect-buffer split. **RESOLVED** — a forced
  wgpu adaptation, faithful to design intent (subsystem #11).
- **D-E. naga-oil rejects trailing-digit struct field names** (`data1`,
  `rand_counter2`) and `ptr<storage>` function params. **RESOLVED** — mechanical
  renames + the `apply_atmosphere` split; field names are read positionally so
  not load-bearing.

---

## 4. Open bugs

| Bug | Source | Status | Detail |
|---|---|---|---|
| **B-1. TAA camera-motion reprojection decay** | `10-impl-b.md` "TAA shadow decay-to-black fix" | **STILL OPEN** | Under camera *motion* in the windowed app, shadowed regions of the GI render degrade toward black; only fresh-disoccluded screen-edge geometry stays lit. **Diagnosed but not fixed.** Established: it is NOT a static-camera convergence problem (the `base/` TAA running-average is provably convergent and confirmed stable at 600 static frames); it needs camera motion; and it is NOT the `sync_position_split` bug (the windowed camera *has* `FreeCamera` so `sync_position_split` runs there). Suspect surface: the camera-motion reprojection inside `reproject_old_samples` / `renderSampleRefine`'s `reproject_sample` — the 3×3 `dist_min_max` / hash / screen-position reject tests and `color_sum.a` behaviour under partial reprojection failure. **Root cause not confirmed; no fix applied** (correctly — "diagnose before patching"). The e2e harness cannot yet reproduce it (no moving-camera mode). This is the **one blocking item** between the current state and a clean Phase-B production gate. |
| **B-2. `sync_position_split` `With<FreeCamera>` query-filter trap** | `10-impl-b.md` "TAA shadow decay-to-black fix" | **FIXED** (`ad12f32`) | `sync_position_split` was filtered `With<FreeCamera>`, so a non-`FreeCamera` render camera (the e2e fixed-pose camera) left `PositionSplit` frozen — silently breaking camera-relative rendering the instant the camera moves. The identical trap was already fixed once in `update_camera_history`. Now filtered `With<PositionSplit>`. This was a *prerequisite* for any moving-camera e2e coverage (needed to diagnose B-1), not B-1 itself. |
| **B-3. Dead temporal-stability e2e scaffolding** (review concern #1) | `11-review-b.md` finding 1 | **STILL OPEN (NIT)** | `GateState.fb_next`, `batch_needs_second_frame`, `Framebuffer::mean_pixel_delta`, `readback.rs:27` describe a two-frame consecutive-readback temporal-stability check for Batch 6 that is never wired — the driver shoots one screenshot and always passes `fb_next: None`; the three symbols have zero call sites. The `10-impl-b.md` "TAA shadow decay-to-black fix" section explicitly **left it in place** as scaffolding for the follow-up dispatch that adds a moving-camera gate (which is the natural place to finish it — see B-1). Comments overstate what the harness verifies. |
| **B-4. `expected_spans(6)` not config-aware re: `is_denoise`** (review concern #3) | `11-review-b.md` finding 3 | **STILL OPEN (CONCERN)** | `gates.rs` unconditionally lists `naadf_denoise` in the batch-6 expected-span set, but `graph_b.rs` runtime-gates the denoise node on `ExtractedGiConfig.is_denoise`. With the default (`true`) the e2e passes; flipping the default or adding a runtime toggle would hard-fail the harness even though a skipped denoise pass is a *correct* configuration. Latent fragility, not a current-config bug. |
| **B-5. Dead plumbing debris** (review nit #5) | `11-review-b.md` finding 5 | **STILL OPEN (NIT)** | `FLAG_BLIT_FINAL_COLOR` (`gpu_types.rs`, `render_pipeline_common.wgsl`), the dormant `taa_layout` descriptor + `TaaGpu.taa_first_hit_bind_group` field, the `taa_sample_accum` no-op touch in `naadf_first_hit.wgsl` — all superseded by later batches but still present. None load-bearing or harmful; churn-avoidance debris from the Batch-2/6 seams. |
| **B-6. No mechanical GPU-struct-offset assert harness** (review nit #6, advisory) | `11-review-b.md` finding 6 | **STILL OPEN (advisory NIT)** | The `vec3`-then-scalar layout bug class (D-A) recurred *three times*, twice after a header comment wrongly claimed "verified field-by-field". The reviewer recommends a runtime offset-assert compute shader so the class is caught mechanically. Strongly advisory; the current `const _: assert!` size checks catch *size* mismatches but not *offset* shifts that keep the size constant. |

Note also a **bookkeeping discrepancy**: the orchestrate docs and several code
comments say "13 render-graph nodes" / "46 tests"; the actual chain has **14
node systems** (`mod.rs:207-228`) and the tree has **48 `#[test]` functions**
(the +2 over the docs' "46" are later substrate tests, all passing — not a
regression). Cosmetic; review nit #4 already flagged the node-count side.

---

## 5. Intentionally deferred / out-of-scope (deliberate non-goals)

Listed explicitly so they are **not confused with gaps**. None of these is a
deficiency in the port — each is a binding scope decision from `01-context.md`.

- **Phase C — GPU world construction & editing.** `chunkCalc.fx` (GPU
  Algorithm 1), `boundsCalc.fx` (background chunk-AADF queue), `worldChange.fx`
  (flood-fill edit invalidation), `mapCopy.fx` (hash-map regrow), the
  `WorldBoundHandler` / `ChangeHandler` / `BlockHashingHandler` /
  `EditingHandler` orchestration. The Phase-A CPU construction path produces
  bit-identical buffers and the traversal shader is producer-agnostic — Phase C
  is a scalability/editability track, not a rendering foundation. Last phase,
  not yet started.
- **The reference pathtracer** — `WorldRenderPathTracer` + `pathTracer/**`
  shaders. Future work; explicitly OUT of Phase B scope.
- **DLSS / DLSS-RR.** The `dlss` / `force_disable_dlss` Cargo plumbing stays
  dormant (`Cargo.toml` `default = ["dlss"]`, `dlss = ["bevy/dlss"]`) — it
  predates the port work and is not on the NAADF render path. Under separate
  later review; not designed for in Phase B.
- **Editor GUI** — the entire `Gui/` ImGui tree. The always-on diagnostics
  `hud.rs` stays; the editor panels do not get ported.
- **Persistence / serialization** — `.cvox` ZIP format, `Settings.cs`,
  `IO.cs`, `PathHandler.cs`, screenshot/camera-path tooling.
- **Asset importers** — `MagicaVoxel.cs` / `VoxFile.cs` / `Voxlap.cs` /
  `obj2voxel`, K-means palette mapping. The hard-coded test grid (D2) is the
  content path.
- **`WorldGenerator` / `WorldGeneratorModel`** — GPU-driven world generation
  into chunk buffers. Deferred with the content path (D2 chose the hard-coded
  grid); a future procedural generator would slot here.
- **Dynamic entities** — `EntityHandler`, `EntityData`, `entityUpdate.fx`, the
  64-bit chunk-texture widening, the `shootRay` entity sub-traversal. Treated as
  a deferred Phase-A sub-feature (open question #5); every `#ifdef ENTITIES`
  block was omitted in all ported WGSL.
- **Interactive editing tools** — `EditingTools/` (cube/sphere/paint/floodfill/
  model). Editor concern; deferred with the GUI.

---

## 6. Prioritized "what's left to fully align" — within the agreed scope

Ranked: blocking correctness first, then faithfulness gaps, then polish.

### Blocking correctness

1. **Fix B-1 — the TAA camera-motion reprojection decay.**
   *Where:* `src/assets/shaders/taa.wgsl` (`reproject_old_samples`) and/or
   `src/assets/shaders/sample_refine.wgsl` (`reproject_sample`) — the
   camera-motion 3×3 `dist_min_max` / hash / screen-position reject path and
   `color_sum.a` partial-reprojection-failure behaviour; possibly `first_hit`
   under genuine motion.
   *Effort:* medium-to-high — diagnosed but root cause unconfirmed; needs a
   clean moving-camera repro (now unblocked by the B-2 fix) and a
   frame-over-frame trace of one shadowed pixel's `taa_sample_accum` /
   `color_sum.a` / ring-sample values, confirmed against
   `base/renderTaaSampleReverse.fx`. **This is the critical path** — the
   Phase-B done-bar ("bounce lighting visible, *temporally stable*, no obvious
   artifacts") is not met until it is fixed.
2. **Add a deterministic moving-camera e2e mode + finish/wire the
   temporal-stability gate (B-3).**
   *Where:* `src/e2e/` — a controlled orbit/pan with `PositionSplit` correctly
   synced (B-2's fix is the prerequisite), plus implementing the dormant
   `fb_next` / `batch_needs_second_frame` / `mean_pixel_delta` two-frame check
   that finding 1 describes.
   *Effort:* medium — partly scaffolded already. Naturally bundles with item 1
   (it is the regression gate for B-1's fix; finishing it without a confirmed
   decay repro to gate against would be scaffolding without a target).

### Faithfulness gaps (within scope, lower priority)

3. **Make `expected_spans` config-aware (B-4).**
   *Where:* `src/e2e/gates.rs:458-469` — derive the batch-6 expected-span set
   from the extracted GI config (drop `naadf_denoise` when
   `is_denoise == false`), or document that the harness only validates the
   `is_denoise = true` configuration.
   *Effort:* low.

4. **(Optional, scope-edge) the CPU-construction vs. GPU-construction seam.**
   The port's AADF construction is CPU-only (`src/aadf/construct.rs`) — a
   faithful re-derivation of Algorithm 1, sanctioned by Q3/D2. It is *not* a
   gap against the agreed scope, but it is the one place the port and NAADF's
   *runtime* differ structurally. Full alignment here is **Phase C** and
   explicitly out of the current scope — listed only so the seam is named.
   *Effort:* large — a whole future phase, deliberately not now.

### Polish (review nits)

5. **Dead-code sweep (B-5).** Remove `FLAG_BLIT_FINAL_COLOR`, the dormant
   `taa_layout` / `taa_first_hit_bind_group`, the `taa_sample_accum` no-op
   touch. *Effort:* low.
6. **Add a mechanical GPU-struct-offset assert harness (B-6).** A tiny compute
   shader that writes each struct field's observed byte offset to a buffer the
   CPU checks against the `#[repr(C)]` offsets — catches the `vec3`-then-scalar
   class (D-A) mechanically instead of by hand-audit (which failed 3×).
   *Effort:* low-to-medium; strongly advisory given the recurrence history.
7. **Reconcile bookkeeping** — "13 nodes" → "14 node systems", "46 tests" →
   "48 tests" in the docs/comments. *Effort:* trivial.

---

## 7. Overall assessment

**The in-scope port is functionally complete and faithful — one open bug short
of the production done-bar.**

All 16 in-scope subsystems are ported; 7 are faithful, 9 are
faithful-with-documented-deviations, and **none behaviourally diverges** from
NAADF intent. Every deviation is traceable to a forced wgpu/naga adaptation, a
sanctioned scope decision (the 16-deep TAA ring, CPU-side AADF construction,
entity-branch omission, the hard-coded test grid), or a documented NAADF-internal
per-variant difference — not drift. The render-graph dispatch order matches
`WorldRenderBase.cs` line-by-line, the adaptive ~0.25-spp signal is real and
wired end-to-end, the compressed-ReSTIR GI chain produces genuine multi-colored
bounce (e2e: 99.2% GI-lit, independently judged real), and all three historical
`#[repr(C)]`-vs-WGSL layout bugs are fixed with no fourth instance found. Phase
A, A-2, and B have all passed their review gates.

**The critical path to "fully aligned" is a single item: bug B-1, the TAA
camera-motion reprojection decay.** It is the lone correctness defect
outstanding after the Phase-B review — diagnosed (camera-motion-triggered, not a
static-camera convergence issue, not the `sync_position_split` trap) but with an
unconfirmed root cause and no fix applied. Its prerequisite (the
`sync_position_split` `With<FreeCamera>` trap, B-2) is already fixed, which
unblocks the moving-camera e2e diagnostic needed to trace it. Everything else
remaining within scope is coverage/fragility/debris (B-3 through B-6, all
NIT/CONCERN) — no further *implementation* is needed, only the B-1 fix, its
moving-camera regression gate, and a small polish pass. Beyond that, the next
*scope* milestone is Phase C (GPU construction/editing), which is a deliberate
future phase, not an alignment gap.

**Bottom line:** the port faithfully realises NAADF's core voxel-GI engine; it
is one well-scoped diagnose-and-fix dispatch (B-1 + its e2e gate) away from a
clean, temporally-stable production gate.
