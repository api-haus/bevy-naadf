# Phase B (GI) — Review Brief

**This file is the review agent's entire brief.** Read ONLY this file as your context. Do
NOT read `01-context.md`, `09-design-b.md`, or any other orchestrate file — the point of a
fresh-eyes review is that you do not share the implementer's context or design rationale, so
you can catch assumptions that were silently baked in. You MAY (and should) read the artifact
itself: the code, the NAADF reference source, and the impl log.

## What was built

Phase B is a port of NAADF's real-time `WorldRenderBase` global-illumination pipeline from
C#/MonoGame+HLSL into Rust/Bevy 0.19-rc.1 WGSL. NAADF ("Nested Axis-Aligned Distance Fields",
Ulschmid et al., CGF 2026) is a voxel GI engine. The port lives on branch `feat/phase-b-gi`
in the worktree `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`. It was built in
6 batches plus 3 bug-fix passes (logged in `10-impl-b.md`).

## Artifact under review (use ABSOLUTE paths)

- **The branch diff.** Worktree root: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`,
  branch `feat/phase-b-gi`. The full diff from the branch base (the `main` commit at Phase-A-2
  close) to HEAD is the Phase B work. Find the base with `git merge-base HEAD main` and diff.
- **The impl log:** `docs/orchestrate/naadf-bevy-port/10-impl-b.md` — what the implementers
  claim they did, batch by batch, plus the 3 fix sections. Read it to see the claims, then
  verify them against the actual code. Treat it as claims to check, not as ground truth.
- **The Phase B render code:** `src/render/` (`gi.rs`, `graph_b.rs`, `prepare.rs`, `taa.rs`,
  `atmosphere.rs`, `color_compression.rs`, `gpu_types.rs`, `pipelines.rs`, `extract.rs`,
  `mod.rs`) and `assets/shaders/` (the GI/atmosphere/TAA WGSL).
- **The e2e verification harness:** `src/lib.rs`, `src/bin/e2e_render.rs`, `src/e2e/`.
- **The NAADF reference source (ground truth):** `/mnt/archive4/DEV/NAADF/` — the C#/HLSL
  engine this is a port of. The port is measured against this. Research digest also available
  at `docs/research/ulschmid-2026-naadf-voxel-gi.md`.

## Success criteria — verify each

1. **Faithful port of NAADF's real-time `WorldRenderBase` GI.** Every in-scope subsystem
   should faithfully port NAADF's source behaviour: the 4-plane first-hit; `rayQueueCalc`
   adaptive ~0.25-spp sampling; compressed ReSTIR GI (`globalIllum`, `sampleRefine`'s 5
   passes, `spatialResampling`); the sparse bilateral denoiser; the atmosphere model; the
   `base/` long-term-memory TAA. Spot-check each against the NAADF HLSL/C# source — flag
   divergences in algorithm, constants, bit-layouts, or dispatch structure.
2. **The adaptive ~0.25-spp signal is real.** `rayQueueCalc` should produce a per-pixel
   sample-count signal that actually drives the GI sampling — verify it is wired through and
   consumed, not decorative.
3. **Render graph.** The GI pipeline should be wired as NAADF's compute-node dispatch order
   (~13 nodes). Verify the node order and the inter-node buffer dependencies are coherent.
4. **Scope discipline — these must NOT be present:** a reference pathtracer; DLSS / DLSS-RR;
   editor GUI; persistence; asset importers. They were explicitly out of scope. Flag any
   trace of them.
5. **GPU struct layout correctness.** Every Rust `#[repr(C)]` GPU struct and its WGSL
   counterpart must have matching byte layouts. In particular: WGSL packs a scalar at offset
   +12 immediately after a `vec3`, but a `#[repr(C)]` Rust struct with explicit padding puts
   the next field at +16 — a `vec3`-then-scalar shape that is not handled (e.g. by declaring
   the row `vec4`) is a silent corruption bug. This exact class recurred multiple times in
   this port. **Audit every uniform and storage struct shared between Rust and WGSL for an
   unfixed instance of this or any other layout mismatch.**
6. **Correctness gates — run them yourself.** From the worktree root: `cargo build` must be
   clean; `cargo test` must pass (expected: 46 tests); `cargo run --bin e2e_render` (the
   windowed e2e render-test harness) must exit 0 with all gates green — including the
   GI-visible gates. After the e2e run, `Read` `target/e2e-screenshots/e2e_latest.png` and
   judge it independently: is the voxel scene genuinely lit by colored GI bounce from the
   emissive blocks, or does it look wrong / under-converged / faked?
7. **Forced/deliberate deviations are sound.** The impl log records several deviations from a
   straight port — among them: a wgpu `STORAGE_READ_WRITE`+`INDIRECT` bind-group split; GI
   settings shipped as fixed constants rather than runtime-tunable; a `screenPosDistanceSqr`
   threshold of `16.0`; several `vec3`→`vec4` WGSL layout fixes; an e2e frame budget of 96
   frames for ReSTIR temporal convergence. For each, assess: is it actually forced /
   justified, and is it faithful to NAADF's intent?
8. **The e2e harness is an honest verification artifact.** Review `src/e2e/` and
   `src/bin/e2e_render.rs`: does the harness genuinely verify (real region/statistic gates, a
   `PipelineCache` error scan that would actually catch shader failures, honest thresholds),
   or does it rubber-stamp? Flag any gate that would pass a broken render.

## Deliverable shape

Write your review to `docs/orchestrate/naadf-bevy-port/11-review-b.md`, appending under the
heading `## delegate-reviewer findings (2026-05-15)`. Structure:

- **Numbered findings.** Each: a severity tag — `BLOCKER` / `CONCERN` / `NIT` — the issue, the
  `file:line` reference, and a recommended action.
- **Per-criterion verdict.** One line per success criterion (1–8 above): met / not met / met
  with caveats.
- **Final verdict:** an explicit `Phase B review gate: PASS` or `Phase B review gate: FAIL`.
  FAIL if any `BLOCKER` finding stands.

Do NOT commit, push, or amend. Do NOT edit code — you are reviewing, not fixing. Your only
write is your findings into this file.

---

## delegate-reviewer findings (2026-05-15)

Fresh-eyes verification pass. Correctness gates run from the worktree root:

- `cargo build` — clean, 0 warnings.
- `cargo test` — **46 passed** (4 suites), 0 failed.
- `cargo run --bin e2e_render` — **exit 0**, all gates green: luminance gate
  `batch 6 — 99.2% non-black; threshold 95%`; per-batch region gate green; 96
  render frames; every pipeline created cleanly; every expected render-graph
  node dispatched.
- `target/e2e-screenshots/e2e_latest.png` — judged independently: the voxel
  scene is **genuinely lit by colored GI bounce**. The white towers / back
  wall / ground carry visible pastel tints (pink, blue, green, purple, amber)
  bleeding off the five colored emissive blocks; the emissive blocks read
  bright; the atmosphere sky band is a clean gradient with no streaks or rings.
  This is real, observable, multi-colored indirect bounce — not a faked or
  under-converged frame. The image is slightly noisy/soft (expected of a
  96-frame temporal-ReSTIR accumulation at a fixed pose), but the bounce is
  unambiguous and the done-bar ("bounce lighting visible, no obvious
  artifacts") is met.

### Numbered findings

1. **NIT — the "Batch-6 temporal-stability gate" is documented but never
   implemented.** `src/e2e/gates.rs:160-168` (`GateState.fb_next`),
   `gates.rs:473-478` (`batch_needs_second_frame` → `true` for batch 6),
   `framebuffer.rs:336-349` (`mean_pixel_delta`, "the Batch-6 temporal-stability
   metric"), and `readback.rs:27` all describe a two-frame consecutive-readback
   temporal-stability check for Batch 6. The driver
   (`src/e2e/driver.rs:88-94, 209-212`) shoots exactly one screenshot and always
   passes `fb_next: None`; `assert_batch_6` (`gates.rs:350-391`) never reads
   `state.fb_next`; `mean_pixel_delta` has zero call sites; `batch_needs_second_frame`
   has zero call sites. The harness's comments overstate what it verifies — a
   reviewer reading the code would believe temporal stability is gated when it
   is not. Recommended action: either implement the second-shot path in the
   driver and a real `mean_pixel_delta` check in `assert_batch_6`, or delete the
   dead scaffolding (`fb_next`, `batch_needs_second_frame`, `mean_pixel_delta`)
   and the comments that promise it. Not a blocker — the implemented gates are
   honest — but it is a real "rubber-stamp-shaped hole" in an artifact whose
   whole job is honest verification.

2. **NIT — the §6.1 stability-hash tripwire is entirely dormant.**
   `src/e2e/gates.rs:151-155` — `hash_baseline` is `match batch { _ => None }`
   for every batch, so the "image unchanged" assertions for B3/B4/B5
   (`assert_batch_3/4/5`) collapse to just re-running `assert_batch_2`'s coarse
   3-region gate. The recorded reasoning (a committed hash literal is only
   bit-identical on the same binary/GPU, so it would spuriously fail elsewhere)
   is defensible, but it means there is *no* mechanism catching a subtle
   B3/B4/B5 regression that leaves the three gate rects within tolerance.
   `stability_hash` itself is implemented and correct — only the baseline table
   is empty. Recommended action: accept as-is (the reasoning holds) or gate the
   hash check behind an env var an agent can opt into on a stable box. Cosmetic
   for the Phase-B gate.

3. **CONCERN — `expected_spans(6)` unconditionally requires `naadf_denoise`,
   but the node is runtime-gated on `is_denoise`.** `src/e2e/gates.rs:458-469`
   lists `naadf_denoise` in the batch-6 expected-span set, and
   `src/render/graph_b.rs:525-543` early-returns the denoise node when
   `ExtractedGiConfig.settings.is_denoise` is `false`. With the `GiSettings`
   default (`is_denoise = true`) this is fine and the e2e passes — but the
   node-dispatch check (`checks.rs:157-182`) would hard-fail the harness if
   anyone flipped that default or wired a runtime toggle, even though a skipped
   denoise pass is a *correct* configuration (`spatial_resampling`'s non-denoise
   branch writes `final_color` directly). The expected-span set is not
   config-aware. Recommended action: make the batch-6 expected-span set a
   function of the extracted GI config (drop `naadf_denoise` when
   `is_denoise == false`), or document that the e2e harness only validates the
   `is_denoise = true` configuration. Latent fragility, not a current-config
   bug.

4. **NIT — node-count claims are inconsistent ("13" vs the actual 14 node
   systems).** `10-impl-b.md` and `09-design-b.md`-derived comments say "13
   render-graph nodes"; `src/render/mod.rs:207-228` chains **14** `Core3d`
   systems (atmosphere, first_hit, taa_reproject, sample_refine_clear,
   ray_queue, global_illum, sample_refine_valid_history,
   sample_refine_count_valid, sample_refine_count_invalid, sample_refine_buckets,
   spatial_resampling, denoise, calc_new_taa_sample, final_blit). The "13"
   counts the 5 sample-refine passes as one logical stage but the chain has them
   as 5 separate systems, and `ray_queue` / `denoise` each fold 2 NAADF
   dispatches into 1 node. Purely a bookkeeping discrepancy — the *order* is
   correct (see criterion 3) and nothing functional depends on the count.
   Recommended action: reword the claim to "14 node systems realising NAADF's
   16-dispatch order".

5. **NIT — dead plumbing left in the tree from the Batch-2/6 seams.** The
   impl log itself flags this (`10-impl-b.md` "§6.3 authoritative shape"
   section): `FLAG_BLIT_FINAL_COLOR` (`gpu_types.rs:131`,
   `render_pipeline_common.wgsl:177`), the dormant `taa_layout` descriptor +
   `TaaGpu.taa_first_hit_bind_group` field, and the `taa_sample_accum` no-op
   touch in `naadf_first_hit.wgsl:310-312` are all superseded but still present.
   None is load-bearing or harmful; it is just churn-avoidance debris a
   follow-up cleanup pass should remove. Recommended action: a small dead-code
   sweep, out of Phase-B scope.

6. **CONCERN — three of the four post-batch debug sections were *wrong about
   where the bug was* before landing the real fix.** Not a code finding — a
   process observation that bears on confidence. The Batch-6 diagnosis blamed
   the GI-consumer WGSL; the next dispatch ruled that out and blamed a
   bind-group/submission-order bug; the actual root cause was a `GpuTaaParams`
   `vec3`-then-scalar layout mismatch. Then a *fourth* dispatch found the
   *identical* bug class again in `GpuGiParams` — after the `gi_params.wgsl`
   header comment had explicitly (and wrongly) claimed "verified field-by-field
   — no explicit pad needed". This exact class recurred **three times**
   (`AtmosphereParams`, `GpuTaaParams`, `GpuGiParams`). I re-audited every
   shared uniform/storage struct for a *fourth* unfixed instance (criterion 5
   below) and found none — but the recurrence pattern means any *future* WGSL
   struct edit is high-risk. Recommended action: add a runtime offset-assert
   harness (a tiny compute shader that writes each struct field's observed byte
   offset to a buffer the CPU reads back and checks against the `#[repr(C)]`
   offsets) so this class is caught mechanically, not by a hand-audit that has
   already failed three times. Strongly advisory; does not block the gate.

7. **NIT — `is_diffuse` decode reads `cur_first_hit.y & 0x1u` but the field is
   never used in `reproject_old_samples`.** `src/assets/shaders/taa.wgsl:234` —
   `cur_first_hit_is_diffuse` is computed and (per a scan) not consumed in the
   reproject pass. Harmless dead local, mirrors a structural-fidelity choice the
   port makes elsewhere (keeping HLSL locals for RNG/structure parity). Noting
   only for completeness.

### Per-criterion verdict

| criterion | verdict | evidence |
|---|---|---|
| 1 — Faithful port of `WorldRenderBase` GI | **met** | Spot-checked `naadf_first_hit.wgsl` vs `base/renderFirstHit.fx` (4-plane loop, `i==4` mirror-tail, `applyAtmosphere`/`addLightForDirection` gating, the 3 output writes — verbatim); `ray_queue_calc.wgsl` vs `base/rayQueueCalc.fx` (`shouldRay` `mod_size`, the inline `addToCounterAddressBuffer`, `calcRayQueueStore` — faithful); `naadf_atmosphere.wgsl` vs `renderAtmosphere.fx` (`ID = globalID.x*4 + frameCount%4`, the `pow(abs(y),2)` warp, the pack — verbatim); `atmosphere.wgsl` `apply_atmosphere`/`atmosphere_oct_index` vs `atmospherePrecomputed.fxh` (the `pow(abs(y),0.5)` un-warp, the index, the `lerp(...,1)` no-op — faithful); `compress_first_hit_data` bit-layout matches. NAADF-faithful HLSL→WGSL adaptations (entity branches dropped, `M*v` convention, explicit truncation casts, `ptr<storage>` splits) are consistent and documented. |
| 2 — Adaptive ~0.25-spp signal is real | **met** | `ray_queue_calc.wgsl:107-161` consumes `taa_sample_accum[id].x` → `accum` → `should_ray` (the real `mod_size ∈ {1..4}` spatial-temporal test) → `should_add` → `ray_queue` write + `atomicAdd(&ray_queue_indirect[0], …)`; `calc_ray_queue_store` converts the count to a workgroup count; `naadf_global_illum_node` (`graph_b.rs:223`) dispatches **indirect** off `ray_queue_indirect`. The signal is wired end-to-end and drives GI cost — not decorative. |
| 3 — Render graph node order & dependencies | **met** | `src/render/mod.rs:207-228` chains the 14 nodes in exactly NAADF's `WorldRenderBase.cs` dispatch order (atmosphere → first_hit → ReprojectOld → ClearBuckets → RayQueue(+Store) → GlobalIlum → ValidHistory → CountValid → CountInvalid → RefineBuckets → SpatialResampling → Denoise(H+V) → CalcNewTaaSample → renderFinal — verified line-by-line against `WorldRenderBase.cs:205-441`). Inter-node buffer deps are coherent; `.chain()` + wgpu auto-barriers serialise shared buffers. See finding 4 (count nit). |
| 4 — Scope discipline | **met** | No reference pathtracer, no DLSS-RR wiring, no editor GUI, no persistence, no asset importers in the Phase-B diff. A `#[cfg(feature="dlss")]` module exists in `src/camera/mod.rs` and `default = ["dlss"]` in `Cargo.toml` — but both **predate Phase B** (26 dlss matches in the pre-Phase-B `main` commit `fe76d33`) and `git diff fe76d33 HEAD -- src/camera/` is empty: Phase B did not touch it, and it is dormant (not on the NAADF render path). Phase B's `Cargo.toml` delta is only `[lib]`/`[[bin]]` decls + the `image` PNG dep. |
| 5 — GPU struct layout correctness | **met** | Audited every shared `#[repr(C)]`/WGSL pair: `GpuRenderParams` (112 B), `GpuCamera` (96 B), `GpuWorldMeta` (48 B), `GpuTaaParams` (192 B), `GpuCameraHistorySlot` (160 B — all 3 WGSL copies identical), `GpuAtmosphereParams` (128 B, explicit pads), `GpuGiParams` (288 B), `GpuSampleValid` (32 B). All byte-offsets match. The three historical `vec3`-then-scalar bugs are all fixed: `AtmosphereParams` carries explicit `pad_*` members; `GpuTaaParams` and `GpuGiParams` use `vec4` for the position/colour rows so the Rust `_padN` u32s become `.w` lanes. **No fourth unfixed instance found** — every WGSL `vec3` is either followed by another 16-byte-aligned member, ends the struct, or is widened to `vec4`. (See finding 6 — the recurrence history makes this the highest-risk class for future edits.) |
| 6 — Correctness gates | **met** | `cargo build` clean; `cargo test` 46/46; `cargo run --bin e2e_render` exit 0, all gates green incl. the GI-visible `assert_batch_6` positive check (`solid_block_rect` brightened past `MIN_GI_BOUNCE_LUMINANCE = 12.0`) and the 99.2%-non-black liveness gate. Screenshot independently judged: genuine colored GI bounce, not faked/under-converged. |
| 7 — Forced/deliberate deviations are sound | **met** | (a) `STORAGE_READ_WRITE`+`INDIRECT` `@group(1)` split (`pipelines.rs` `sample_refine_dispatch_layout`) — forced by a real wgpu exclusivity rule, faithful to the design's intent; sound. (b) GI settings as fixed `GiSettings`/`AppArgs` constants — matches the §1 "no GUI" scope decision; the C# slider *defaults* are ported as consts; sound. (c) `screenPosDistanceSqr > 16.0` in the `base/` `reproject_old_samples` (`taa.wgsl`) — verified against `base/renderTaaSampleReverse.fx:138-139` (the `base/` source genuinely uses `16.0`, the `albedo/` source `1.0`); a real per-variant divergence, correctly applied. (d) the `vec3`→`vec4` WGSL layout fixes — correct and idiomatic (see criterion 5). (e) `E2E_RENDER_FRAMES = 96` — a test-infrastructure change justified by NAADF's temporal-ReSTIR `refineBuckets` `<12`-sample gate (`base/renderSampleRefine.fx:411`); the fixed e2e pose makes every extra frame pure deterministic convergence; sound and honestly documented. |
| 8 — e2e harness is an honest verification artifact | **met with caveats** | The load-bearing checks are genuine: the `PipelineCache` scan (`checks.rs:57-109`) catches `Err`, treats still-`Queued`/`Creating` as "a node never ran", and treats `total == 0` as failure — it would catch the whole shader-bug catalogue in one run; the node-dispatch check asserts every expected span has a real measurement; `check_not_degenerate` requires both dark and bright pixels; the region gates use measured, well-separated thresholds with documented margins; the luminance gate is recalibrated to just below the *measured* 99.2% (a real tripwire, not a rubber stamp); `MIN_GI_BOUNCE_LUMINANCE` was honestly *held* at 12.0 (not stamped down) until the bounce genuinely cleared it. **Caveats:** the "Batch-6 temporal-stability gate" is documented but unimplemented (finding 1); `hash_baseline` is entirely dormant (finding 2); `expected_spans(6)` is not config-aware re: `is_denoise` (finding 3). None of these would pass a *broken* render — they are gaps in coverage, not rubber stamps — so the harness is honest, with the caveat that its comments promise slightly more than it delivers. |

### Final verdict

All 8 success criteria are met (criterion 8 with caveats). No `BLOCKER` findings
stand — the 7 findings are 5 `NIT`s and 2 `CONCERN`s, all of which are coverage
gaps, fragilities, or debris rather than correctness defects. The build is
clean, all 46 tests pass, the e2e harness exits 0 with every gate green, and the
screenshot shows genuine multi-colored GI bounce lighting the voxel scene. The
port is faithful to NAADF's `WorldRenderBase` across the spot-checked subsystems,
the render-graph order matches the C# dispatch sequence exactly, the adaptive
sampling signal is real and wired, scope is clean, and every shared GPU struct
layout is correct (the three historical `vec3`-then-scalar bugs are all fixed,
no fourth instance found).

**Phase B review gate: PASS**

Recommended (non-blocking) before merge or as immediate follow-ups: implement or
delete the dead temporal-stability scaffolding (finding 1); make `expected_spans`
config-aware (finding 3); add a mechanical GPU-struct-offset assert harness
(finding 6) given that the `vec3`-then-scalar class recurred three times.
