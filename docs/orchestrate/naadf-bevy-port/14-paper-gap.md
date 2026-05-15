# 14 — Canonical-Paper Gap Analysis: Rust/Bevy port vs. the NAADF paper

## paper-gap findings (2026-05-15)

**Author:** delegated paper-gap analysis agent (read-only on code).
**Scope:** the Rust/Bevy port (`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/`,
branch `main` at commit `047afba` — Phase C DONE) measured against the
**canonical paper** `ulschmid-2026-naadf-voxel-gi.md` (Ulschmid et al., CGF
2026). The audit runs **post-Phase-C** — after the GPU construction +
editing + entities + background AADF queue + worldgen workstreams (W0–W6 +
wave-3 integration) and the TAA-fidelity track landed on `main`. Where this
and `12-alignment-gap.md` disagree, the paper text and the port code were
both re-checked; see the Reconciliation section.

This document answers one question: **for every algorithm, data structure,
pipeline stage and methodology step the paper describes, what does the port
actually do?** Every row cites a paper section/concept AND a port file path.

---

### Summary — how close the port is to the canonical paper methodology

The port now faithfully implements the paper's **full methodology surface
(§3 + §4)** — both the rendering half and the construction/editing/dynamism
half. Specifically:

- **§3.1** — three-layer chunk/block/voxel cell hierarchy. FAITHFUL.
- **§3.2** — GPU Algorithm 1 hashing construction (`31^(64-i)` coefficients,
  open-addressing linear probe, occupancy-resize). FAITHFUL (with port-vs-paper
  numeric deviations: probe-cap 250 / wanted-empty-ratio 0.5, following C#
  per Q3; the paper quotes 100 / 75%).
- **§3.3** — AADF definition + the O(3·d·n) synchronised-iteration
  neighbour-merge construction + per-layer background AADF queues. FAITHFUL.
- **§3.4** — DDA traversal exploiting AADFs, including the entity
  sub-traversal branch. FAITHFUL.
- **§3.5** — Editing: CPU world mirror + CPU→GPU sync + flood-fill AADF
  invalidation (7-round BFS, distance step 4, cap 28 over the 63³-chunk
  affected volume). FAITHFUL.
- **§3.6** — Dynamic entities: per-chunk 32-bit entity pointer (chunks
  texture widened `R32Uint`→`Rg32Uint`), entity instance buffer, per-entity
  AADF voxel volumes, hash-dedup, traversal-time entity sub-traversal.
  FAITHFUL.
- **§4** — Compact G-buffer, long-term-memory TAA (sample-ring depth now
  default 32, paper-canonical; 16 / 24 still available as configurable
  levers), compressed ReSTIR GI (Algorithm 2), sparse bilateral denoiser,
  atmosphere. FAITHFUL or FAITHFUL-with-deviations.

What is **not** implemented (and was either never in the paper's
*methodology* or is un-portable from the NAADF source tree):

- **§4.3 SVGF alternative denoiser** — un-portable from the NAADF source
  tree (`Content/shaders/render/**` ships only the sparse bilateral); the
  paper's SVGF impl was never released. The paper itself favours the sparse
  bilateral, which IS ported faithfully.
- **§5 evaluation** — benchmark scenes (OASISX240 / tera-voxel scalability
  claims), Tables 1–5, Figs 8–10. Not methodology — there is nothing to
  port; the port's e2e harness is a functional gate, not a perf benchmark.

One **non-methodology residual** remains on the implemented surface: a
wgpu/Vulkan storage-texture barrier hazard that prevents the *full*
upload-skip path on the runtime producer flip. The GPU producer chain DOES
dispatch Algorithm 1 every startup via `naadf_gpu_producer_node`, and
bit-exact `--validate-gpu-construction` proves output equivalence on a
deterministic fixture — the workaround keeps the CPU upload path active.
This is a wgpu infrastructure issue, **not a paper-methodology gap**;
recorded as a Phase-D backlog item.

---

### Main gap table

Legend: **FAITHFUL** — implemented as the paper specifies. **DEVIATION** —
implemented but with a documented deviation (F = forced by
wgpu/naga/MonoGame↔wgpu conventions; S = deliberate sanctioned shortcut; U =
deliberate user-directed override). **PARTIAL** — partially implemented.
**MISSING** — paper specifies it, port does not implement it.

#### Contribution #1 — the NAADF data structure (paper §3.1–3.4, Algorithm 1, Figs 2–3)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §3.1 Three-layer cell hierarchy (chunk/block/voxel, each 4³) | Shallow 3-layer hierarchy, each layer a 4³ grid of the layer below; each layer in its own buffer; 64³ voxels per chunk | **FAITHFUL** | `src/aadf/cell.rs`, `src/aadf/construct.rs`, `src/world/data.rs` | 3-layer chunk/block/voxel, `CELL_DIM=4`, `CELL_CHILDREN=64`, separate chunk/block/voxel buffers. Re-derived from paper per Q3. |
| §3.1 Cell state encoding (empty / uniform-full / mixed; chunk 32-bit 2+30, block 32-bit, voxel 16-bit 1+15) | Chunk = 2-bit state + 30-bit payload (AADF 5b×6 / 15-bit type / child ptr); block similar w/ 2-bit AADF; voxel = 1-bit state + 15 payload | **FAITHFUL** | `src/aadf/cell.rs` (`ChunkCell`/`BlockCell`/`VoxelCell` encode/decode) | Replicates the C# traversal's top-bit/uniform-full re-encoding (research divergence #3) — verified bit-positions. |
| §3.1 Voxel = 15-bit type pointer into a material buffer; material entry stores albedo + emissivity/reflectivity + roughness | "16 bits per material entry" (paper's minimal figure) | **DEVIATION (S)** | `src/voxel/mod.rs` (`VoxelType`), `src/render/gpu_types.rs` (`GpuVoxelType`, 16 B / 128-bit) | Port follows the C# 128-bit `Uint4` entry (base+layer material enum, f16 roughness, two RGB half-float colors) — the correct call per Q3 (C# is the correctness cross-check; paper's "16 bit" is illustrative). |
| §3.2 Construction via hashing — Algorithm 1, GPU 64-thread groups | GPU build: per-block uniform test → else hash 64 voxels with `H = c₀+Σcᵢ·vᵢ`, `cᵢ=31^(64-i) mod 2³²`; open-addressing linear probing (≤100 probes); buffer resize at 75% occupancy; group-sync; thread-0 chunk classify | **FAITHFUL (DEVIATION S on numeric tuning)** | `src/assets/shaders/chunk_calc.wgsl` (3 entries) + `src/assets/shaders/map_copy.wgsl` (2 entries) + `src/render/construction/{chunk_calc,map_copy,hashing}.rs` | Algorithm 1 fully ported in W1. 65-entry `31^(64-i) mod 2^32` hash coefficient table (`hashing::generate_hash_coefficients`, recompute `#[test]` matches `BlockHashingHandler.cs:50-55`). 64-thread groups, per-block uniform test, open-addressing CAS, occupancy-trigger `map_copy` regrow. **Numeric tuning follows C#, not paper:** `probe_cap = 250` (vs paper 100) at `chunk_calc.wgsl:234`; `wanted_empty_ratio = 0.5` (vs paper 75%) at `config.rs:147` — sanctioned per Q3. Bit-exact GPU/CPU oracle via `--validate-gpu-construction` (388 bytes byte-equal on a 1×1×1 deterministic fixture). Runtime GPU producer chain dispatches every startup via `naadf_gpu_producer_node`. |
| §3.2 Block deduplication (mixed blocks hash to a shared 64-voxel group) | Hashing deduplicates blocks with equal voxels — "permitting deduplication of blocks with equal voxels" | **FAITHFUL** | `src/aadf/construct.rs` (CPU oracle, `block_dedup: HashMap`) + GPU dedup via the open-addressing probe loop in `chunk_calc.wgsl` | Dedup behaviour is faithful on both paths. Note (W1 Assumption #7): CPU `HashMap` iteration order vs GPU open-addressing-by-hash produce *different* `VoxelPtr` assignments on full grids — block *contents* are semantically equivalent (dedup behaviour identical), pointer numbering differs. Bit-exact comparison gated to deterministic 1×1×1 fixtures; consumer workstreams (W2/W3) verify semantic correctness at the use-site. |
| §3.3 AADF definition — 6-direction empty-cuboid bounding box | Per empty cell, a cuboid empty of geometry extending in x±,y±,z±; 5 bits/dir for chunks (max 31 = 496 voxels), 2 bits/dir for blocks & voxels (max 3) | **FAITHFUL** | `src/aadf/bounds.rs` (`compute_aadf_layer`, `Aadf6`), `src/voxel/mod.rs` (`AADF_MAX_CHUNK=31`, `AADF_MAX_SMALL=3`) | 6-direction `Aadf6`, per-direction caps 31/3, alternating-axis expansion, slice-empty test — all match §3.3. |
| §3.3 AADF construction algorithm — O(3·d·n) synchronised-iteration neighbour-merge | "synchronise the iterations and dimension expansions for all cells, [...] just merge the cuboid with the respective cuboid of the neighbor cell" → O(3·d·n) linear | **FAITHFUL** | `src/aadf/bounds.rs::compute_aadf_layer` (W6 rewrite) | The W6 rewrite replaced the legacy per-cell expansion (`compute_aadf`, retained as a per-cell reference) with the §3.3 synchronised-iteration neighbour-merge form — **measured 16.3× speedup**. CPU oracle redefinition (`bounds.rs:32-43`): the merge form is strictly conservative wrt the per-cell form and IS what GPU `ComputeBounds4` produces — so W1's bit-exact oracle compares against this new canonical truth. |
| §3.3 Background AADF computation — per-layer queues, "one queue per frame" | Modified cells queued separately per layer; AADFs computed in the background during rendering; chunks get `3·31` queues, one per iteration; the impl handles one queue per frame | **FAITHFUL** | `src/assets/shaders/bounds_calc.wgsl` + `src/assets/shaders/bounds_common.wgsl` + `src/render/construction/bounds_calc.rs` | W3 regime-2 dispatch: `naadf_bounds_compute_node` fires **5 prepare+indirect-compute rounds per frame** (`ConstructionConfig.n_bounds_rounds = 5`, matching `WorldBoundHandler.cs:113`). Per-axis mask + dispatch-indirect buffers (8 + 5 u32s). Convergence oracle: 64 chunks 0-mismatch on the 4×4×4 chunk fixture (the convergence oracle is a fresh CPU port of `boundsCalc.fx`'s algorithm, not W6's `compute_aadf_layer` — a chunk-world-edge OOB-permissive divergence flagged in W6's assumption #2 forced separate CPU ports). |
| §3.4 DDA traversal exploiting AADFs (first-hit) | DDA (Amanatides & Woo) that advances many voxels in one iteration using the AADF empty cuboid; start at chunk layer, empty→AADF-skip, mixed→descend, full→intersect & terminate | **FAITHFUL** | `src/assets/shaders/ray_tracing.wgsl` (`shoot_ray`), `ray_tracing_common.wgsl` | Phase-A core, review-gated PASS. chunk→block→voxel descent + AADF single-step skip ported from `rayTracing.fxh:73`. Reused unchanged for GI secondary + visibility rays. |
| §3.4 Traversal entity sub-traversal | When a chunk contains entities, record it; after main traversal, bbox-test + AADF-traverse the entity voxel volumes | **FAITHFUL** | `src/assets/shaders/ray_tracing.wgsl` (`shoot_ray` entity branch, post-DDA) | The `#ifdef ENTITIES` traversal branch is now ACTIVE (Phase C wave-3, commit `2fc0b1e`). `shoot_ray` collects up to 16 distinct `chunks[pos].y` entity pointers along the main DDA, then bbox-tests + AADF-traverses each entity's per-entity voxel volume, merging the closer hit. `RayResult` grew an `entity: u32` field (`0x3FFFu` = no-entity sentinel). Always-compiled; no-entity-scene cost is ~0. |
| §3.5 Editing — CPU world copy + CPU→GPU sync + flood-fill AADF invalidation | A CPU copy of voxels/blocks/chunks; every change synced CPU→GPU; on a chunk's empty↔non-empty flip, all AADFs in the surrounding 63³-chunk volume reset; flood-fill (in 4³-chunk groups) marks each chunk for recompute once | **FAITHFUL** | `src/assets/shaders/world_change.wgsl` (4 entries) + `src/render/construction/{world_change,change_handler}.rs` + `src/aadf/edit.rs` + `src/world/data.rs::set_voxel` | W2 ports `worldChange.fx` + `ChangeHandler.UpdateWorld`. CPU `set_voxel(IVec3, VoxelTypeId)` main-world API → per-edit batch extraction in `ExtractSchedule` → flood-fill BFS (**7 rounds × 3 axes = 21 sweeps per frame; distance step 4; cap 28** — exact match to `ChangeHandler.cs:73-174`) over the 63³-chunk affected volume → on-edit-event regime-3 GPU dispatch (`naadf_world_change_node`). 6 oracle bit-exact gates PASS: chunk/block/voxel edits ↔ CPU oracle byte-equal; entity-pointer `.y` preserved on chunk writes; bound-queue re-enqueue; flood-fill BFS matches `ChangeHandler.cs`. `--edit-mode` e2e PASS. |
| §3.6 Dynamic entities — per-chunk 32-bit entity pointer, entity instance buffer, per-entity AADF voxel volumes, hash-dedup, ~10% overhead | Each chunk +32-bit (24-bit ptr + 8-bit counter); entity instance buffer (ID/pos/rot/voxel-ptr); each entity voxel 32-bit with AADFs; chunk-entity-instance hash dedup; entity movement triggers the same empty↔non-empty AADF reset | **FAITHFUL** | `src/assets/shaders/entity_update.wgsl` (3 entries); `src/render/construction/{entity_update,entity_handler}.rs`; `src/aadf/entity.rs`; `src/assets/shaders/world_data.wgsl` (entity bindings); `src/assets/shaders/ray_tracing.wgsl` (entity sub-traversal); `src/render/pipelines.rs` (`world_layout` 8-binding extension); `src/render/prepare.rs` (chunks texture `Rg32Uint`) | W4 (entity-side) + wave-3 (renderer-side) port the full §3.6 stack. **Chunks texture widened `R32Uint`→`Rg32Uint`** (`.x` = block-state pointer + AADF, `.y` = entity pointer + counter). `EntityChunkInstance` (5 × u32 / 20 B) carries pos/rot/id. Per-entity AADF voxel volumes via `EntityData::from_types` (31-iteration per-axis neighbour-merge for 5-bit-per-axis AADFs). Smallest-three quaternion compression (`compress_quaternion`). Chunk-entity-instance hash-dedup. `--entities` e2e PASS; the `entity_pixel` luminance gate hits 187.93 vs threshold 80 (2.35× margin). `entity_instances_history` binding plumbed-but-unconsumed by default (`ConstructionConfig.entity_history_enabled = false` → 16 B placeholder; Phase-D feature flag flips the consumer in `shoot_ray` for TAA reprojection of moving entities). |

#### Contribution #2 — long-term-memory TAA (paper §4.1, Fig 6)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §4 Compact G-buffer — store bounce planes, not position/depth/normal | Per pixel store each plane a ray bounced off (3-bit normal + 14-bit distance-along-normal); record up to 4 planes until a non-specular hit; reconstruct virtual path by reflecting the camera ray | **FAITHFUL** | `src/assets/shaders/naadf_first_hit.wgsl` (`compress_first_hit_data`), `render_pipeline_common.wgsl` (`get_hit_data_from_planes`) | 4-plane G-buffer, `i==4` mirror-tail, plane bit-layout verified vs `commonRenderPipeline.fxh`. |
| §4 Per-pixel Halton jitter | Sample positions jittered with a Halton sequence; jitter + TAA consider albedo, resampling/denoise consider indirect only | **FAITHFUL** (with sanctioned numeric deviation on bases) | `src/render/taa.rs` (`halton_jitter`, bases 3 & 7), `src/render/gpu_types.rs::GpuGiParams.taa_jitter` at offset 280, `src/assets/shaders/{naadf_first_hit,naadf_global_illum,spatial_resampling}.wgsl` `get_ray_dir` calls | First-hit ray was always jittered. The TAA-fidelity track (fix #1, commit `8995c88`) **wired the same Halton jitter through to the GI sample-generation and spatial-resampling rays** — `GpuGiParams.taa_jitter` at offset 280 (8-byte aligned, `offset_of!` guard); `prepare_gi` writes `extracted_history.current_jitter` (same source-of-truth as first-hit); `naadf_global_illum.wgsl` + `spatial_resampling.wgsl` `get_ray_dir` call-sites updated. Bases (3,7) are fixed — `02-research.md` §1.2.1 records the C# resolving to (3,7) via `findCoprime`, so this matches the C# in practice. |
| §4.1 Long-term history — store the last 32 frames @ 64 bits/sample | Instead of one history buffer, retain 32 past frames; 64 bits per sample → ~29 MB/frame @1440p | **FAITHFUL** | `src/render/taa.rs` (`TaaRingConfig`), `src/lib.rs::DEFAULT_TAA_RING_DEPTH = 32`, `src/assets/shaders/taa_common.wgsl` (`#{TAA_SAMPLE_RING_DEPTH}` substitution) | **Sample-ring depth default = 32 (paper-canonical), configurable.** TAA-fidelity fix #3 raised the default from 16 to 32; the 16 / 24 / 32 lever values remain available via `AppArgs.taa_ring_depth`. Single source of truth: `TaaRingConfig` resource fed to both `prepare_taa` (buffer sizing — `pixel_count * ring_depth`) and `NaadfPipelines` (a `#{TAA_SAMPLE_RING_DEPTH}` shader-def). Two regression tests pin the default + the supported-lever set. Supersedes the §6 16-deep binding decision in `design-exploration-qa.md`. |
| §4.1 64-bit sample layout — color R/G/B 8b each + dist 16b + roughness 5b + normal 3b + hash 16b | The exact Figure-6 field budget | **FAITHFUL** | `src/assets/shaders/taa_common.wgsl` (`compress_sample`/`decompress_sample`) | Ported from `commonTaa.fxh`; the `uint2` packed layout matches (research §1.2.2 cross-check). |
| §4.1 Exponential color compression `f(x)=12·log₂(x/100 + 2^(-255/12))+255` | The compression formula, x∈[0,100] | **FAITHFUL** | `src/assets/shaders/taa_common.wgsl`, `color_compression.wgsl`; test `src/render/color_compression.rs` | The C#'s algebraically-equal form is ported with a `#[test]` recomputing from the source formula. |
| §4.1 128-deep camera-history ring | A 128-deep ring of camera matrices/positions/jitters the reprojection indexes into (research divergence #5) | **FAITHFUL** | `src/render/taa.rs` (`CAMERA_HISTORY_DEPTH = 128`) | Kept at NAADF's depth (tiny VRAM); the configurable lever is the *sample* ring only. |
| §4.1 Depth-based rejection — 3×3 min/max depth precompute + hash match in 9-neighbourhood + 1-px reprojection-distance check | Pre-compute 3×3 min/max depth; per-pixel hash; reprojected sample accepted iff depth in range AND hash matches one of 9 neighbours AND projects within 1 px of origin; non-diffuse samples reduced by roughness/direction-change; pick closest-matching 3×3 pixel (not current) | **FAITHFUL** | `src/assets/shaders/taa.wgsl` (`reproject_old_samples`), `src/render/taa.rs` | `base/` `ReprojectOld` ported; writes `taa_dist_min_max`; uses the genuine `base/`-variant `screenPosDistanceSqr > 16.0` threshold (research D-B). |
| §4 TAA placed early in the pipeline (before GI, informs GI + guides denoiser) | TAA runs right after primary rays; its output informs GI sample generation and guides the denoiser | **FAITHFUL** | `src/render/mod.rs` (graph order: first_hit → taa_reproject → … → denoise → calc_new_taa_sample → final) | Render-graph order verified line-by-line vs `WorldRenderBase.cs`. |
| §4 Post-process tone mapping | Tone mapping in a post-processing step | **DEVIATION (U)** | `src/assets/shaders/naadf_final.wgsl` (raw HDR output), `src/camera/mod.rs` (`Hdr` + `Tonemapping::default()`) | The TAA-fidelity track (fix #2) **replaced NAADF's custom Reinhard tonemap with Bevy's `TonyMcMapface`**. The port emits raw linear HDR from `naadf_final.wgsl`; Bevy's `tonemapping` render-graph node — running after the NAADF chain via the existing `.before(tonemapping)` ordering — does the tonemap + sRGB encode. The `exposure` / `tone_mapping_fac` `GpuRenderParams` fields were renamed `_pad0a`/`_pad0b` (layout-preserving). **Deliberate user-directed deviation** from the faithful-port principle (Q2), recorded in `naadf_final.wgsl`'s file header. |

#### Contribution #3 — compressed ReSTIR GI resampling (paper §4.2, Algorithm 2, Fig 7)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §4.2 Material-based sampling only (no explicit light-source storage) | No explicit light-source storage; material-based sampling only — too costly to store light sources in editable voxel worlds | **FAITHFUL** | `src/assets/shaders/naadf_global_illum.wgsl`, `spatial_resampling.wgsl` | Secondary rays sample by material; the only "light" terms are the sun sample + emissive voxels hit by rays. No light-source buffer — matches the paper. |
| §4.2 Sample generation driven by the TAA accumulated sample count → adaptive ~0.25–1 spp | Leverage the long-term TAA accumulated sample count to selectively generate samples (disoccluded pixels prioritised, converged pixels skipped); ≤3-bounce secondary rays; effective 0.25–1 spp | **FAITHFUL** | `src/assets/shaders/ray_queue_calc.wgsl` (`rayQueueCalc`), `src/render/gi.rs` | Consumes `taa_sample_accum.x` → drives the indirect `globalIllum` dispatch; `skipSamples` toggles 1↔0.25 spp. Adaptive signal real + wired end-to-end. Correctly unjittered per `rayQueueCalc.fx`. |
| §4.2 ≤3-bounce secondary rays | Secondary rays with at most 3 bounces | **FAITHFUL** | `src/assets/shaders/naadf_global_illum.wgsl` | ≤3-bounce secondary-ray tracer ported from `renderGlobalIllum.fx`. |
| §4.2 Lit/unlit separation + compression — unlit 16 B (primary only), lit 32 B (primary+secondary), stored as structured-buffer lists | Samples stored as lists in structured buffers (not textures); unlit = primary-hit data only (16 B), lit = primary+secondary (32 B); Figure-7 three-block layout | **FAITHFUL** | `src/assets/shaders/naadf_global_illum.wgsl` (`compress_sample_valid`/`compress_sample_invalid`), `gi_params.wgsl` | Lit/unlit classification + the Figure-7 primary/secondary/refined layouts ported. |
| §4.2 Unlit 8:1 compression — store every 8th unlit sample, weighted ×8 | Store only every eighth unlit sample, weighted 8× (higher ratio adds noise) | **FAITHFUL** | `src/assets/shaders/naadf_global_illum.wgsl` (`is_skip = !is_valid && next_rand > 1/8`) | The "every 8th unlit sample, weighted ×8" rule is ported. |
| §4.2 Temporal resampling — project into 8×8 disjoint screen-space regions; ≤32 lit samples/region; unlit → region counter | Samples projected into 8×8 disjoint screen-space pixel regions (not per-pixel reservoirs); up to 32 lit samples stored per region; unlit increment a region counter | **FAITHFUL** | `src/assets/shaders/sample_refine.wgsl` (`valid_history`, `count_valid`, `count_invalid`), `src/render/gi.rs` | 8×8 regions, the `BucketStorageCount=32` lit-per-region buffer, region counters ported across the 5 sample-refine passes. |
| §4.2 Brightness-leveling — compare each region sample to region max brightness, remove weakly-lit, compensate by removal probability, ≤8 refined survivors | Filter the 32 region samples against region-max brightness; remove weakly-lit, boost survivors by removal probability; ≤8 refined, discard excess | **FAITHFUL** | `src/assets/shaders/sample_refine.wgsl` (`refine_buckets`, `COLOR_DIF_PROB` table) | `RefineBuckets` brightness-leveling with the `COLOR_DIF_PROB[31]` exponential-difference table ported as WGSL literals + a recompute `#[test]`. |
| §4.2 Spatial resampling (Algorithm 2) — 12 iterations, single final visibility check, no initial sample, adaptive per-pixel radius, Jacobian-weighted reservoir merge | 12-iteration neighbour-region loop (vs ReSTIR GI's 3), visibility tested only for the final selected sample, no initial sample, adaptive radius, Jacobian `\|J\|`, `cₙ = Rₙ.color·(litCount/totalCount)`, reservoir merge | **FAITHFUL** | `src/assets/shaders/spatial_resampling.wgsl` (`sample_neighbors`) | Algorithm 2 ported: 12-iteration loop, adaptive-radius 12-tap pre-pass, Jacobian, single 3-step visibility ray, no initial sample. |
| §4.2 Independent sun sample inside spatial resampling | A uniform-hemisphere sun sample added per pixel — direct-sun bounce light on diffuse surfaces, independent of the refine buffers | **FAITHFUL** | `src/assets/shaders/spatial_resampling.wgsl` | Ported from `renderSpatialResampling.fx:321-339` — sun ray via `shoot_ray` + `MAX_RAY_STEPS_SUN`, `sunColor*weight`. |
| §4.2 lit/refined storage budgets — lit 2 frames' worth (covers ≤64 frames), unlit 4 frames' worth (≥32 frames) | "two frames' worth of storage" for lit (max 64 past frames), "four frames worth" for unlit (≥32 frames) | **FAITHFUL** | `src/render/gi.rs` (`GiGpu` buffer sizing), `gi_params.wgsl` | Follows the C# `WorldRenderBase` `globalIllumValidSampleStorageCount=2` / `InvalidSampleStorageCount=8`. |
| §5.2 Soft sun shadows during resampling | (Limitation, paper §6) "soft shadows from the sun are not handled during resampling, resulting in slightly increased noise" | **FAITHFUL** | `src/assets/shaders/spatial_resampling.wgsl` | The port reproduces the paper's *stated limitation* — the sun sample is a single hemisphere sample, not a soft-shadow integration. Faithful to NAADF as shipped. |

#### Denoising (paper §4.3)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §4.3 Sparse bilateral denoiser — kernel 21, σ=10, separable H+V, ~½ pixels processed, color + geometry weights, applied to indirect only, result folded into TAA history | Sparse bilateral filter (kernel 21, σ=10), horizontal-then-vertical, on average every 2nd pixel, color (TAA result / albedo → luminance) + geometry (normal + depth-as-plane) weights; result added to the TAA color and stored in the 32-frame history | **FAITHFUL** | `src/assets/shaders/denoise_split.wgsl`, graph: `naadf_denoise_node` → `naadf_calc_new_taa_sample_node` | Kernel 21 / σ=10 / separable / sparse / color+geometry weights ported exactly; denoiser result folded into the (default 32-deep) TAA history. |
| §4.3 / §5.2 SVGF alternative denoiser | The paper provides an SVGF alternative ("we leave the choice of denoising variant to the user") | **MISSING (un-portable)** | — | No SVGF in the port. **Not in the in-scope NAADF source tree** either (`Content/shaders/render/**` ships only the sparse bilateral) — research divergence #11. A gap *against the paper's prose*, but un-portable from the NAADF source; the paper's SVGF impl was never released. Low-value: the paper itself favours the sparse bilateral, which IS ported faithfully. |

#### Atmosphere / sky (paper §4, Fig 4 — supporting subsystem)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §4 Atmosphere precompute + apply | Pipeline Fig 4 includes atmosphere; Table 5 lists an "Atmosphere" 0.12 ms pipeline step | **FAITHFUL** | `src/render/atmosphere.rs`, `src/assets/shaders/atmosphere.wgsl`, `naadf_atmosphere.wgsl` | Multiple-scattering sky model + CPU sun-color sampling + the octahedral quarter-per-frame precompute ported from `Atmosphere.cs` + `renderAtmosphere.fx`. The paper barely specifies the sky model (it is a supporting subsystem, not a contribution) — the port faithfully ports NAADF's. |
| §4 Post-process tone mapping | Tone mapping in a post-processing step | **DEVIATION (U)** | `src/camera/mod.rs` (`Tonemapping::default()` = `TonyMcMapface`), `src/assets/shaders/naadf_final.wgsl` (raw HDR output) | See §4.1 row above — port uses Bevy `TonyMcMapface` instead of NAADF's Reinhard tonemap. User-directed deviation. |

#### Evaluation-only / non-methodology paper content (not gaps — listed for completeness)

| Paper section / concept | Paper specifies | Port state | Note |
|---|---|---|---|
| §5 World generation (NAADF `WorldGenerator`) | The paper's test scenes are generated procedurally; the C# ships `WorldGenerator`/`WorldGeneratorModel` | **FAITHFUL** | W5 ports `generatorModel.fx` → `generator_model.wgsl` + the segmented dispatch shape (`group_offset_in_chunks` + `group_size_in_chunks`). CPU `generate_segment_cpu` is the bit-exact oracle (8192 u32s byte-equal). Active runtime content path is the hard-coded test grid (D2); the W5 chain runs alongside as the W1 GPU/CPU oracle dispatch. |
| §5 Tera-voxel scalability (OASISX240, 2T voxels, Table 1) | NAADF still delivers 2765/817 Mrays/s where competitors OOM | **N/A (scale, not methodology)** | The methodology (Algorithm 1, AADFs, editing, entities, ReSTIR GI, sparse denoiser) IS implemented; the tera-voxel benchmark is a *consequence* of the structure + GPU construction, not separately portable. The e2e harness's test grid is small. |
| §5 Performance comparison vs Grid/Octree/SVDAG (Tables 1–5, Figs 8–10) | Benchmark methodology + results | **N/A** | Paper evaluation, not methodology — nothing to port. The port's e2e harness is a functional gate, not a perf benchmark. |
| §5.3 Limitations (DX11 32-bit pointer limit, voxelization → many distinct types) | Stated limitations of the NAADF *implementation* | **N/A / IMPROVED** | The DX11 32-bit-pointer limit is a MonoGame artefact the paper itself says "can easily be eliminated by moving to another graphics API" — the wgpu port is on that other API. Not a gap. |

---

### Prioritized completion list

Per the new state of the port, the canonical-methodology completion list
collapses dramatically. What remains is mostly polish + the one
wgpu-infrastructure residual.

1. **wgpu/Vulkan storage-texture barrier hazard** — *the one outstanding
   seam.* The chunks 3D texture is bound for construction as
   `texture_storage_3d<rg32uint, read_write>` and for the renderer as
   `texture_3d<u32>`. With CPU upload disabled, GPU writes from the
   construction chain do not propagate to the renderer's read view. The GPU
   producer chain (`naadf_gpu_producer_node`) *does* dispatch Algorithm 1
   every startup; bit-exact `--validate-gpu-construction` proves output
   equivalence on a deterministic fixture; the workaround keeps the CPU
   upload path active. **Scope: medium.** Likely needs an explicit pipeline
   barrier between regime-1 GPU dispatch and the first render frame, or a
   different bind-group aliasing strategy. **This is a wgpu infrastructure
   issue, not a NAADF methodology gap.**
2. **TAA reprojection of moving entities (paper §3.6 follow-on).** —
   *PARTIAL.* The `entity_instances_history` binding is plumbed (per-frame
   GPU upload exists; `copy_entity_history` dispatch is skipped when
   `entity_history_enabled = false`, which is the default — a 16 B
   placeholder occupies the binding slot). Flipping the flag and landing the
   consumer in `ray_tracing.wgsl::shoot_ray` per paper §3.6 is the Phase-D
   feature. **Scope: medium.**
3. **Halton jitter coprimes (paper §4).** — *DEVIATION (S) — fixed bases
   (3,7).* The C# computes `coprimes` via `findCoprime`; the port hard-codes
   (3,7). Matches the C# *in practice* per research §1.2.1. **Scope:
   trivial** — only worth doing if a future change varies the jitter base
   count.
4. **SVGF alternative denoiser (paper §4.3).** — *MISSING (un-portable).*
   Not in the in-scope NAADF source (not shipped). Only relevant if
   re-implemented from the SVGF literature. **Scope: large; lowest
   priority** — the paper itself favours the sparse bilateral, which IS
   ported faithfully.
5. **Flood-fill cap-28 test coverage** (`17-review-c.md` nit #2). The W2
   flood-fill cap-28 edge case is not directly exercised by a dedicated
   test. **Scope: small.**
6. **`render/construction/mod.rs` mega-module split** (`17-review-c.md` nit
   #3). Currently 4510 lines. **Scope: small** — Phase-C-internal polish.
7. **Future shadow-filtering improvements** (user note, post-TAA-fidelity).
   Separate later track.

**Not on this list** (correctly): everything that was on the previous
prioritized completion list — GPU Algorithm 1, editing flood-fill,
background AADF queues, dynamic entities, O(3·d·n) AADF construction, world
generation, ring-depth 32 — has been **delivered**. The list went from "the
back half of the paper's Method section" to "one wgpu-infrastructure
residual + Phase-D entity TAA + small test/polish nits".

---

### Reconciliation with `12-alignment-gap.md`

`12-alignment-gap.md` measures the port against the **NAADF C# reference
within the agreed in-scope subset**. This document measures it against the
**paper, in full**. **After Phase C, both yardsticks agree on the same fact
base** — the prior denominator difference (rendering-half-faithful vs
full-methodology-incomplete) no longer applies in the same way.

**Where they agree:**

- Every subsystem `12-alignment-gap.md` calls "faithful" or
  "faithful-with-deviations" — the rendering pipeline (traversal, 4-plane
  G-buffer, long-term TAA at depth 32, `rayQueueCalc`, compressed ReSTIR GI,
  sample-refine, spatial resampling, sparse bilateral denoiser, atmosphere,
  render-graph order) — this document **also** finds FAITHFUL against the
  paper.
- Every Phase-C subsystem `12-alignment-gap.md` calls "faithful" or
  "faithful-with-deviations" — GPU Algorithm 1 construction, O(3·d·n) AADF
  construction, background AADF queue, editing + flood-fill, world
  generator, dynamic entities, the entity sub-traversal in `shoot_ray` —
  this document **also** finds FAITHFUL against the paper.
- The user-directed deviations (Bevy `TonyMcMapface` tonemapping, default
  TAA ring depth 32 with 16/24 levers preserved) and the sanctioned numeric
  deviations (probe-cap 250 / wanted-empty-ratio 0.5 following the C# per
  Q3) are recorded identically in both documents.
- **B-7** (wgpu storage-texture barrier hazard) is recorded in both
  documents as a Phase-D backlog item, **not** a NAADF correctness gap.

**Where this document still adds (in framing, not in facts):**

- The §4.3 **SVGF alternative denoiser** is MISSING-but-un-portable from
  this document's perspective; `12-alignment-gap.md` records it as "N/A —
  no SVGF shader exists to port." Same finding, different framing.
- The §5 **tera-voxel scalability** claim (OASISX240, 2T voxels) is N/A on
  this document's axis (scale, not methodology) and not on
  `12-alignment-gap.md`'s axis at all (scope-deferred).

**Bottom line of the reconciliation:** the port is **canonically complete
against the paper's methodology surface (§3 + §4) with one
wgpu-infrastructure residual** (B-7 in `12-alignment-gap.md`); SVGF is
un-portable from the NAADF source; §5 evaluation is not portable
methodology. Both documents now agree the port faithfully realises the full
NAADF voxel-GI engine — rendering plus construction plus editing plus
dynamism — against both the C# reference and the canonical paper.
