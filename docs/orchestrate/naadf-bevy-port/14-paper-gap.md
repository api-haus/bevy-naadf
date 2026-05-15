# 14 — Canonical-Paper Gap Analysis: Rust/Bevy port vs. the NAADF paper

## paper-gap findings (2026-05-15)

**Author:** delegated paper-gap analysis agent (read-only on code).
**Scope:** the Rust/Bevy port (`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/`)
measured against the **canonical paper** `ulschmid-2026-naadf-voxel-gi.md`
(Ulschmid et al., CGF 2026) — *not* against the in-scope subset the orchestration
was originally scoped to. Where this and `12-alignment-gap.md` disagree, the
paper text and the port code were both re-checked; see the Reconciliation
section.

This document answers one question: **for every algorithm, data structure,
pipeline stage and methodology step the paper describes, what does the port
actually do?** Every row cites a paper section/concept AND a port file path.

---

### Summary — how close the port is to the canonical paper methodology

The port faithfully implements the paper's **rendering-side methodology** — the
data-structure *layout*, the DDA-with-AADF traversal, the full real-time GI
pipeline (4-plane compact G-buffer, long-term-memory TAA, adaptive ~0.25-spp
sample generation, compressed lit/unlit ReSTIR resampling with the 8×8
screen-space regions and 12-iteration spatial pass, the sparse bilateral
denoiser, atmosphere). On the *runtime rendering* axis the port is close to
paper-complete: contributions **#2 (long-term memory TAA)** and **#3 (compressed
ReSTIR GI)** are implemented essentially in full, and the traversal half of
**#1 (NAADF)** is faithful.

Where the port is **not** canonical is everything the paper presents as the
*construction, maintenance and dynamism* of the structure — i.e. the half of
contribution **#1** that makes NAADF "NAADF" rather than "a static voxel grid
with distance caches", plus contribution **#4** entirely:

- The **GPU hashing construction (Algorithm 1)** is replaced by a CPU
  `HashMap` re-derivation. Functionally produces equivalent buffers for a small
  static grid, but the paper's Algorithm 1 — the open-addressing linear-probe
  hash, the `31^(64-i)` coefficients, the 64-thread-group GPU build, the
  buffer-resize-at-occupancy — is **not** in the port.
- The **AADF construction is the naïve O(cubic-ish) per-cell expansion**, not
  the paper's §3.3 O(3·d·n) *synchronised-iteration neighbour-merge* algorithm,
  and there is **no background per-layer queue** ("one queue per frame") — AADFs
  are built once, eagerly, at load.
- **Editing (§3.5)** — CPU world copy + CPU→GPU sync + the **flood-fill AADF
  invalidation** over the 63³-chunk affected volume — is **entirely absent**.
- **Dynamic entities (§3.6)** — the per-chunk 32-bit entity pointer, the entity
  instance buffer, per-entity AADF voxel volumes, hash-dedup, the traversal-time
  entity sub-traversal — is **entirely absent** (every `#ifdef ENTITIES` block
  was omitted by design).
- **Tera-voxel scalability (§5, Table 1 OASISX240)** and **world generation**
  are absent (a hard-coded test grid is the only content path).

These were *deliberately* deferred by the orchestration (Phase C + binding scope
decisions D2/D3/Q1 + open-question #5). They are real gaps **against the paper**,
but they are not *drift* — they are scoped-out work. The honest framing: the
port is a faithful implementation of the paper's **rendering pipeline** on top of
a **statically-constructed, non-editable, entity-free** NAADF structure. To be
"canonically complete per the paper" it needs Phase C (GPU construction +
editing) and an entities track — roughly the back half of the paper's Method
section (§3.2 GPU build, §3.3 background AADF, §3.5 editing, §3.6 entities).

One open *correctness* bug remains on the implemented surface: the TAA
camera-motion reprojection decay / never-resolves-clean issue (`12-alignment-gap.md`
B-1 + the RESUME "next dispatch") — that is a faithfulness defect *within* an
implemented subsystem, tracked separately and not re-litigated here.

---

### Main gap table

Legend: **FAITHFUL** — implemented as the paper specifies. **DEVIATION** —
implemented but with a documented deviation (F = forced by wgpu/naga/MonoGame↔wgpu
conventions; S = deliberate sanctioned shortcut). **PARTIAL** — partially
implemented. **MISSING** — paper specifies it, port does not implement it.

#### Contribution #1 — the NAADF data structure (paper §3.1–3.4, Algorithm 1, Figs 2–3)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §3.1 Three-layer cell hierarchy (chunk/block/voxel, each 4³) | Shallow 3-layer hierarchy, each layer a 4³ grid of the layer below; each layer in its own buffer; 64³ voxels per chunk | **FAITHFUL** | `src/aadf/cell.rs`, `src/aadf/construct.rs`, `src/world/data.rs` | 3-layer chunk/block/voxel, `CELL_DIM=4`, `CELL_CHILDREN=64`, separate chunk/block/voxel buffers. Re-derived from paper per Q3. |
| §3.1 Cell state encoding (empty / uniform-full / mixed; chunk 32-bit 2+30, block 32-bit, voxel 16-bit 1+15) | Chunk = 2-bit state + 30-bit payload (AADF 5b×6 / 15-bit type / child ptr); block similar w/ 2-bit AADF; voxel = 1-bit state + 15 payload | **FAITHFUL** | `src/aadf/cell.rs` (`ChunkCell`/`BlockCell`/`VoxelCell` encode/decode) | Replicates the C# traversal's top-bit/uniform-full re-encoding (research divergence #3) — verified bit-positions. |
| §3.1 Voxel = 15-bit type pointer into a material buffer; material entry stores albedo + emissivity/reflectivity + roughness | "16 bits per material entry" (paper's minimal figure) | **DEVIATION (S)** | `src/voxel/mod.rs` (`VoxelType`), `src/render/gpu_types.rs` (`GpuVoxelType`, 16 B / 128-bit) | Port follows the C# 128-bit `Uint4` entry (base+layer material enum, f16 roughness, two RGB half-float colors) — the correct call per Q3 (C# is the correctness cross-check; paper's "16 bit" is illustrative). |
| §3.2 Construction via hashing — Algorithm 1, GPU 64-thread groups | GPU build: per-block uniform test → else hash 64 voxels with `H = c₀+Σcᵢ·vᵢ`, `cᵢ=31^(64-i) mod 2³²`; open-addressing linear probing (≤100 probes); buffer resize at 75% occupancy; group-sync; thread-0 chunk classify | **MISSING** (GPU) / **PARTIAL** (functional equivalent) | `src/aadf/construct.rs` (`construct`, `classify_block`) | CPU `HashMap`-keyed-on-64-voxel-array re-derivation. Produces layout-equivalent buffers for a static grid, but the paper's Algorithm 1 — the exact hash function, GPU open-addressing, probe limit, occupancy-resize, 64-thread-group dispatch (`chunkCalc.fx`) — is **not** implemented. Deliberately deferred to **Phase C**. |
| §3.2 Block deduplication (mixed blocks hash to a shared 64-voxel group) | Hashing deduplicates blocks with equal voxels — "permitting deduplication of blocks with equal voxels" | **FAITHFUL** | `src/aadf/construct.rs` (`block_dedup: HashMap`, `identical_blocks_dedup` test) | Dedup *behaviour* is faithful (identical mixed blocks share one `VoxelPtr`); the *mechanism* (a Rust `HashMap`, not the GPU probe table) is the deviation captured in the row above. |
| §3.3 AADF definition — 6-direction empty-cuboid bounding box | Per empty cell, a cuboid empty of geometry extending in x±,y±,z±; 5 bits/dir for chunks (max 31 = 496 voxels), 2 bits/dir for blocks & voxels (max 3) | **FAITHFUL** | `src/aadf/bounds.rs` (`compute_aadf`, `Aadf6`), `src/voxel/mod.rs` (`AADF_MAX_CHUNK=31`, `AADF_MAX_SMALL=3`) | 6-direction `Aadf6`, per-direction caps 31/3, alternating-axis expansion, slice-empty test — all match §3.3. Block/voxel AADF bounded by the chunk's 4³ extent (`CellBox::cube(4)`), chunk AADF bounded by the world — matches the paper. |
| §3.3 AADF construction algorithm — O(3·d·n) synchronised-iteration neighbour-merge | "synchronise the iterations and dimension expansions for all cells, [...] just merge the cuboid with the respective cuboid of the neighbor cell" → O(3·d·n) linear | **DEVIATION (S)** | `src/aadf/bounds.rs` (`compute_aadf` — per-cell, no merge) | Port does the straightforward **per-cell** expansion (re-running the slice test for each cell independently), NOT the neighbour-merge optimisation. Result cuboids are correct; complexity is worse. Explicitly sanctioned for "Phase A's tiny static grid" (`03-design.md` §6.1 step 3). Becomes load-bearing only at scale / for background recompute. |
| §3.3 Background AADF computation — per-layer queues, "one queue per frame" | Modified cells queued separately per layer; AADFs computed in the background during rendering; chunks get `3·31` queues, one per iteration; the impl handles one queue per frame | **MISSING** | — (no port file; would map to NAADF `boundsCalc.fx` / `WorldBoundHandler`) | AADFs are built once, eagerly, at load (`src/aadf/construct.rs`). There is no per-layer queue, no per-frame background recompute, no `boundQueueInfo`-style queue system. Part of **Phase C** (research §1.1.4). Not exercised because nothing edits the world. |
| §3.4 DDA traversal exploiting AADFs (first-hit) | DDA (Amanatides & Woo) that advances many voxels in one iteration using the AADF empty cuboid; start at chunk layer, empty→AADF-skip, mixed→descend, full→intersect & terminate | **FAITHFUL** | `src/assets/shaders/ray_tracing.wgsl` (`shoot_ray`), `ray_tracing_common.wgsl` | Phase-A core, review-gated PASS. chunk→block→voxel descent + AADF single-step skip ported from `rayTracing.fxh:73`. Reused unchanged for GI secondary + visibility rays. |
| §3.4 Traversal entity sub-traversal | When a chunk contains entities, record it; after main traversal, bbox-test + AADF-traverse the entity voxel volumes | **MISSING** | `src/assets/shaders/ray_tracing.wgsl:7` ("entity branch omitted — Phase A is entity-free") | The `#ifdef ENTITIES` traversal branch is omitted. See §3.6 row. |
| §3.5 Editing — CPU world copy + CPU→GPU sync + flood-fill AADF invalidation | A CPU copy of voxels/blocks/chunks; every change synced CPU→GPU; on a chunk's empty↔non-empty flip, all AADFs in the surrounding 63³-chunk volume reset; flood-fill (in 4³-chunk groups) marks each chunk for recompute once | **MISSING** (editing) / **PARTIAL** (CPU mirror + re-upload plumbing) | `src/world/data.rs` (`dirty` flag), `src/render/extract.rs` / `prepare.rs` (dirty-gated re-upload) | A CPU world mirror and a `dirty`→re-upload path exist (so re-uploading edited buffers is *plumbed*), but there is **no edit pipeline, no flood-fill, no AADF invalidation, no 63³-volume reset**. Nothing produces edits and nothing recomputes AADFs after a change. Entire §3.5 algorithm — **Phase C**. |
| §3.6 Dynamic entities — per-chunk 32-bit entity pointer, entity instance buffer, per-entity AADF voxel volumes, hash-dedup, ~10% overhead | Each chunk +32-bit (24-bit ptr + 8-bit counter); entity instance buffer (ID/pos/rot/voxel-ptr); each entity voxel 32-bit with AADFs; chunk-entity-instance hash dedup; entity movement triggers the same empty↔non-empty AADF reset | **MISSING** | `src/assets/shaders/naadf_global_illum.wgsl:26-28`, `ray_tracing.wgsl:7` (entity branches omitted) | Entirely absent. `entitySample` is a hard-wired `ENTITY_FREE` constant everywhere; no entity buffers, no `EntityHandler` equivalent, no 64-bit chunk-texture widening. Deferred by open-question #5. |

#### Contribution #2 — long-term-memory TAA (paper §4.1, Fig 6)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §4 Compact G-buffer — store bounce planes, not position/depth/normal | Per pixel store each plane a ray bounced off (3-bit normal + 14-bit distance-along-normal); record up to 4 planes until a non-specular hit; reconstruct virtual path by reflecting the camera ray | **FAITHFUL** | `src/assets/shaders/naadf_first_hit.wgsl` (`compress_first_hit_data`), `render_pipeline_common.wgsl` (`get_hit_data_from_planes`) | 4-plane G-buffer, `i==4` mirror-tail, plane bit-layout verified vs `commonRenderPipeline.fxh` (`12-alignment-gap.md` subsystem #8). |
| §4 Per-pixel Halton jitter | Sample positions jittered with a Halton sequence; jitter + TAA consider albedo, resampling/denoise consider indirect only | **DEVIATION (S)** | `src/render/taa.rs` (`halton_jitter`, bases 3 & 7) | Port uses fixed Halton bases (3,7); the C# computes `coprimes` via `findCoprime`. Research §1.2.1 records the C# as Halton base (3,7), so this matches the C# in practice — minor. |
| §4.1 Long-term history — store the last 32 frames @ 64 bits/sample | Instead of one history buffer, retain 32 past frames; 64 bits per sample → ~29 MB/frame @1440p | **DEVIATION (S)** | `src/render/taa.rs` (`TAA_SAMPLE_RING_DEPTH = 16`), `src/assets/shaders/taa_common.wgsl` | **16-deep** sample ring, not 32 — the binding `design-exploration-qa.md` §6 VRAM lever (~501 MB vs ~973 MB @1440p; the paper's own Table 4 lists "Ours (16 samples)" as a sanctioned configuration). Pipeline fully intact, modest quality cost. Deliberate, paper-sanctioned. |
| §4.1 64-bit sample layout — color R/G/B 8b each + dist 16b + roughness 5b + normal 3b + hash 16b | The exact Figure-6 field budget | **FAITHFUL** | `src/assets/shaders/taa_common.wgsl` (`compress_sample`/`decompress_sample`) | Ported from `commonTaa.fxh`; the `uint2` packed layout matches (research §1.2.2 cross-check). |
| §4.1 Exponential color compression `f(x)=12·log₂(x/100 + 2^(-255/12))+255` | The compression formula, x∈[0,100] | **FAITHFUL** | `src/assets/shaders/taa_common.wgsl`, `color_compression.wgsl`; test `src/render/color_compression.rs` | The C#'s algebraically-equal form is ported with a `#[test]` recomputing from the source formula (research divergence #10). |
| §4.1 128-deep camera-history ring | A 128-deep ring of camera matrices/positions/jitters the reprojection indexes into (research divergence #5) | **FAITHFUL** | `src/render/taa.rs` (`CAMERA_HISTORY_DEPTH = 128`) | Kept at NAADF's depth (tiny VRAM); the §6 lever is the *sample* ring only. |
| §4.1 Depth-based rejection — 3×3 min/max depth precompute + hash match in 9-neighbourhood + 1-px reprojection-distance check | Pre-compute 3×3 min/max depth; per-pixel hash; reprojected sample accepted iff depth in range AND hash matches one of 9 neighbours AND projects within 1 px of origin; non-diffuse samples reduced by roughness/direction-change; pick closest-matching 3×3 pixel (not current) | **FAITHFUL** | `src/assets/shaders/taa.wgsl` (`reproject_old_samples`), `src/render/taa.rs` | `base/` `ReprojectOld` ported; writes `taa_dist_min_max`; uses the genuine `base/`-variant `screenPosDistanceSqr > 16.0` threshold (research D-B). **Carries open bug B-1** — camera-motion reprojection decay (`12-alignment-gap.md` §4); a faithfulness defect *within* this otherwise-faithful subsystem. |
| §4 TAA placed early in the pipeline (before GI, informs GI + guides denoiser) | TAA runs right after primary rays; its output informs GI sample generation and guides the denoiser | **FAITHFUL** | `src/render/mod.rs:207-228` (graph order: first_hit → taa_reproject → … → denoise → calc_new_taa_sample → final) | Render-graph order verified line-by-line vs `WorldRenderBase.cs` (`12-alignment-gap.md` subsystem #16). |

#### Contribution #3 — compressed ReSTIR GI resampling (paper §4.2, Algorithm 2, Fig 7)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §4.2 Material-based sampling only (no explicit light-source storage) | No explicit light-source storage; material-based sampling only — too costly to store light sources in editable voxel worlds | **FAITHFUL** | `src/assets/shaders/naadf_global_illum.wgsl`, `spatial_resampling.wgsl` | Secondary rays sample by material; the only "light" terms are the sun sample + emissive voxels hit by rays. No light-source buffer — matches the paper. |
| §4.2 Sample generation driven by the TAA accumulated sample count → adaptive ~0.25–1 spp | Leverage the long-term TAA accumulated sample count to selectively generate samples (disoccluded pixels prioritised, converged pixels skipped); ≤3-bounce secondary rays; effective 0.25–1 spp | **FAITHFUL** | `src/assets/shaders/ray_queue_calc.wgsl` (`rayQueueCalc`), `src/render/gi.rs` | Consumes `taa_sample_accum.x` → drives the indirect `globalIllum` dispatch; `skipSamples` toggles 1↔0.25 spp. Adaptive signal real + wired end-to-end (`12-alignment-gap.md` subsystem #9). |
| §4.2 ≤3-bounce secondary rays | Secondary rays with at most 3 bounces | **FAITHFUL** | `src/assets/shaders/naadf_global_illum.wgsl` | ≤3-bounce secondary-ray tracer ported from `renderGlobalIllum.fx`. |
| §4.2 Lit/unlit separation + compression — unlit 16 B (primary only), lit 32 B (primary+secondary), stored as structured-buffer lists | Samples stored as lists in structured buffers (not textures); unlit = primary-hit data only (16 B), lit = primary+secondary (32 B); Figure-7 three-block layout | **FAITHFUL** | `src/assets/shaders/naadf_global_illum.wgsl` (`compress_sample_valid`/`compress_sample_invalid`), `gi_params.wgsl` | Lit/unlit classification + the Figure-7 primary/secondary/refined layouts ported (research §1.2.3 cross-check, `12-alignment-gap.md` subsystem #10). |
| §4.2 Unlit 8:1 compression — store every 8th unlit sample, weighted ×8 | Store only every eighth unlit sample, weighted 8× (higher ratio adds noise) | **FAITHFUL** | `src/assets/shaders/naadf_global_illum.wgsl:451-458` (`is_skip = !is_valid && next_rand > 1/8`) | The "every 8th unlit sample, weighted ×8" rule is ported. |
| §4.2 Temporal resampling — project into 8×8 disjoint screen-space regions; ≤32 lit samples/region; unlit → region counter | Samples projected into 8×8 disjoint screen-space pixel regions (not per-pixel reservoirs); up to 32 lit samples stored per region; unlit increment a region counter | **FAITHFUL** | `src/assets/shaders/sample_refine.wgsl` (`valid_history`, `count_valid`, `count_invalid`), `src/render/gi.rs` | 8×8 regions, the `BucketStorageCount=32` lit-per-region buffer, region counters ported across the 5 sample-refine passes. |
| §4.2 Brightness-leveling — compare each region sample to region max brightness, remove weakly-lit, compensate by removal probability, ≤8 refined survivors | Filter the 32 region samples against region-max brightness; remove weakly-lit, boost survivors by removal probability; ≤8 refined, discard excess | **FAITHFUL** | `src/assets/shaders/sample_refine.wgsl` (`refine_buckets`, `COLOR_DIF_PROB` table) | `RefineBuckets` brightness-leveling with the `COLOR_DIF_PROB[31]` exponential-difference table ported as WGSL literals + a recompute `#[test]` (research divergence #10). |
| §4.2 Spatial resampling (Algorithm 2) — 12 iterations, single final visibility check, no initial sample, adaptive per-pixel radius, Jacobian-weighted reservoir merge | 12-iteration neighbour-region loop (vs ReSTIR GI's 3), visibility tested only for the final selected sample, no initial sample, adaptive radius, Jacobian `\|J\|`, `cₙ = Rₙ.color·(litCount/totalCount)`, reservoir merge | **FAITHFUL** | `src/assets/shaders/spatial_resampling.wgsl` (`sample_neighbors`) | Algorithm 2 ported: 12-iteration loop, adaptive-radius 12-tap pre-pass, Jacobian, single 3-step visibility ray, no initial sample (research §1.2.3, `12-alignment-gap.md` subsystem #12). |
| §4.2 Independent sun sample inside spatial resampling | A uniform-hemisphere sun sample added per pixel — direct-sun bounce light on diffuse surfaces, independent of the refine buffers | **FAITHFUL** | `src/assets/shaders/spatial_resampling.wgsl:529-538` | Ported from `renderSpatialResampling.fx:321-339` — sun ray via `shoot_ray` + `MAX_RAY_STEPS_SUN`, `sunColor*weight`. |
| §4.2 lit/refined storage budgets — lit 2 frames' worth (covers ≤64 frames), unlit 4 frames' worth (≥32 frames) | "two frames' worth of storage" for lit (max 64 past frames), "four frames worth" for unlit (≥32 frames) | **FAITHFUL** | `src/render/gi.rs` (`GiGpu` buffer sizing), `gi_params.wgsl` | Follows the C# `WorldRenderBase` `globalIllumValidSampleStorageCount=2` / `InvalidSampleStorageCount=8` (research §1.2.3). |
| §5.2 Soft sun shadows during resampling | (Limitation, paper §6) "soft shadows from the sun are not handled during resampling, resulting in slightly increased noise" | **FAITHFUL** | `src/assets/shaders/spatial_resampling.wgsl` | The port reproduces the paper's *stated limitation* — the sun sample is a single hemisphere sample, not a soft-shadow integration. Faithful to NAADF as shipped. |

#### Denoising (paper §4.3)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §4.3 Sparse bilateral denoiser — kernel 21, σ=10, separable H+V, ~½ pixels processed, color + geometry weights, applied to indirect only, result folded into TAA history | Sparse bilateral filter (kernel 21, σ=10), horizontal-then-vertical, on average every 2nd pixel, color (TAA result / albedo → luminance) + geometry (normal + depth-as-plane) weights; result added to the TAA color and stored in the 32-frame history | **FAITHFUL** | `src/assets/shaders/denoise_split.wgsl`, graph: `naadf_denoise_node` → `naadf_calc_new_taa_sample_node` | Kernel 21 / σ=10 / separable / sparse / color+geometry weights ported exactly; denoiser result folded into the (16-deep) TAA history (`12-alignment-gap.md` subsystem #13). |
| §4.3 / §5.2 SVGF alternative denoiser | The paper provides an SVGF alternative ("we leave the choice of denoising variant to the user") | **MISSING** | — | No SVGF in the port. **Not in the in-scope NAADF source tree** either (`Content/shaders/render/**` ships only the sparse bilateral) — research divergence #11. A gap *against the paper's prose*, but un-portable from the NAADF source; the paper's SVGF impl was never released. Low-value: the paper itself favours the sparse bilateral. |

#### Atmosphere / sky (paper §4, Fig 4 — supporting subsystem)

| Paper section / concept | Paper specifies | Port state | Port location | Note |
|---|---|---|---|---|
| §4 Atmosphere precompute + apply (tone mapping in post) | Pipeline Fig 4 includes atmosphere; "tone mapping is applied in a post-processing step"; Table 5 lists an "Atmosphere" 0.12 ms pipeline step | **FAITHFUL** | `src/render/atmosphere.rs`, `src/assets/shaders/atmosphere.wgsl`, `naadf_atmosphere.wgsl` | Multiple-scattering sky model + CPU sun-color sampling + the octahedral quarter-per-frame precompute ported from `Atmosphere.cs` + `renderAtmosphere.fx`. The paper barely specifies the sky model (it is a supporting subsystem, not a contribution) — the port faithfully ports NAADF's (`12-alignment-gap.md` subsystem #7). |
| §4 Post-process tone mapping | Tone mapping in a post-processing step | **FAITHFUL** | `src/assets/shaders/naadf_final.wgsl` (`tone_mapping_fac`) | `base/` final-blit tonemap ported. |

#### Evaluation-only / non-methodology paper content (not gaps — listed for completeness)

| Paper section / concept | Paper specifies | Port state | Note |
|---|---|---|---|
| §5 Tera-voxel scalability (OASISX240, 2T voxels, Table 1) | NAADF still delivers 2765/817 Mrays/s where competitors OOM | **MISSING** | No large-world / segmented-world path; a single hard-coded test grid is the only content (D2). Scalability is a *consequence* of the structure + GPU construction; not separately portable. Not a methodology step. |
| §5 Performance comparison vs Grid/Octree/SVDAG (Tables 1–5, Figs 8–10) | Benchmark methodology + results | **N/A** | Paper evaluation, not methodology — nothing to port. The port's e2e harness is a functional gate, not a perf benchmark. |
| §5.3 Limitations (DX11 32-bit pointer limit, voxelization → many distinct types) | Stated limitations of the NAADF *implementation* | **N/A / IMPROVED** | The DX11 32-bit-pointer limit is a MonoGame artefact the paper itself says "can easily be eliminated by moving to another graphics API" — the wgpu port is on that other API. Not a gap. |

---

### Prioritized completion list

The MISSING + PARTIAL items, ordered by how load-bearing each is to **"canonical
methodology completeness per the paper."** Each carries a one-line scope
estimate. (Items 1–2 are the bulk of the paper's Method §3.2–3.6 — they are what
makes the structure *actually* a NAADF rather than a static distance-cached
grid.)

1. **GPU hashing construction — Algorithm 1 (paper §3.2).** — *MISSING (GPU);
   PARTIAL functional equivalent exists.* Port `chunkCalc.fx`: the `31^(64-i)`
   hash, 64-thread-group GPU build, open-addressing linear probe, occupancy
   resize. The single biggest "is this really NAADF" gap — the paper's headline
   construction algorithm. **Scope: large** — a whole compute-shader subsystem +
   the GPU hash-map buffer + the `BlockHashingHandler` equivalent. This is the
   core of **Phase C**.
2. **Editing + flood-fill AADF invalidation (paper §3.5).** — *MISSING.*
   CPU→GPU edit sync + the flood-fill that resets AADFs in the 63³-chunk
   affected volume (in 4³-chunk groups) + `worldChange.fx` apply passes. Without
   it the structure is static — contradicting the paper's central "editable
   voxel worlds" claim. **Scope: large** — a flood-fill system + `worldChange`
   compute passes + the four `changedX` growable buffers. Part of **Phase C**.
3. **Background AADF computation queues (paper §3.3).** — *MISSING.* The
   per-layer queues, "one queue per frame" background recompute (`boundsCalc.fx`
   / `WorldBoundHandler`). Required for editing to be non-stalling and for large
   worlds; pointless until #1/#2 land (nothing changes the world). **Scope:
   medium** — a render-graph node group with indirect dispatch + the per-size
   queue buffers. Part of **Phase C**.
4. **Dynamic entities (paper §3.6).** — *MISSING.* Per-chunk 32-bit entity
   pointer (+ the 64-bit chunk-texture widening), entity instance buffer,
   per-entity AADF voxel volumes, hash-dedup, traversal-time entity
   sub-traversal, entity-movement AADF reset. Contribution **#4**, entirely
   absent. **Scope: large** — touches the chunk buffer format, `shoot_ray`, a
   new `EntityHandler` system, and `entityUpdate.fx`. Open-question #5; its own
   track.
5. **O(3·d·n) synchronised-iteration AADF construction (paper §3.3).** —
   *DEVIATION (S) — currently naïve per-cell.* Replace the per-cell expansion in
   `src/aadf/bounds.rs` with the neighbour-cuboid-merge algorithm. Only
   load-bearing at scale / for background recompute (item 3); the current
   version is *correct*, just slower. **Scope: medium** — algorithmic rewrite of
   `compute_aadf`, no new subsystems. Naturally bundles with #1/#3 (GPU
   construction).
6. **World generation (paper §5 test scenes; NAADF `WorldGenerator`).** —
   *MISSING.* No procedural/segmented world generation; a hard-coded grid is the
   only content path (D2). Not a paper *methodology* step (the paper uses
   pre-authored scenes), but required for the tera-voxel scalability story and
   any non-toy content. **Scope: medium** — a generation compute pass + the
   segmented-world buffer-growth machinery. Bundles with #1.
7. **Halton jitter coprimes (paper §4).** — *DEVIATION (S) — fixed bases
   (3,7).* The C# computes `coprimes` via `findCoprime`; the port hard-codes
   (3,7). Matches the C# *in practice* per research §1.2.1. **Scope: trivial** —
   only worth doing if a future change varies the jitter base count.
8. **SVGF alternative denoiser (paper §4.3).** — *MISSING.* Un-portable from
   the NAADF source (not shipped). Only relevant if re-implemented from the SVGF
   literature. **Scope: large; lowest priority** — the paper itself favours the
   sparse bilateral, which *is* ported faithfully.

**Not on this list** (correctly): the open TAA camera-motion bug (B-1) — it is a
*faithfulness defect within an implemented subsystem*, tracked by
`12-alignment-gap.md` §4 + RESUME, not a missing methodology step.

---

### Reconciliation with `12-alignment-gap.md`

`12-alignment-gap.md` measured the port against the **NAADF C# reference within
the agreed in-scope subset**. This document measures it against the **paper, in
full**. They are consistent — the difference is the *denominator*, and that
difference is the whole point of this dispatch.

**Where they agree:**

- Every subsystem `12-alignment-gap.md` calls "faithful" or
  "faithful-with-deviations" — the traversal, the 4-plane G-buffer, the
  long-term TAA, `rayQueueCalc`, compressed ReSTIR GI, sample-refine, spatial
  resampling, the sparse bilateral denoiser, atmosphere, the render-graph order
  — this document **also** finds FAITHFUL or DEVIATION against the paper. The
  *rendering pipeline* is faithfully ported by both yardsticks.
- The 16-deep TAA ring, the CPU-`HashMap` construction, the entity-branch
  omission, the hard-coded test grid: `12-alignment-gap.md` classes these as
  "deliberate / deferred / not a gap *within scope*." This document agrees they
  are **deliberate and sanctioned**, but — measured against the *paper* — the
  CPU construction, the entity omission, and the missing editing/background-queue
  machinery **are genuine gaps in canonical-methodology coverage**. Same facts,
  honestly different framing for a different question.

**Where this document differs (in framing, not in facts):**

- `12-alignment-gap.md` §5 lists Phase C, entities, and `WorldGenerator` under
  "Intentionally deferred / out-of-scope — **NOT gaps**." That is correct *for
  its question* ("how far from the in-scope target"). For *this* question ("how
  far from the canonical paper") they are **MISSING items #1–#4, #6** above —
  the back half of the paper's Method section. This is not a contradiction: a
  thing can be both "deliberately deferred" and "a gap against the paper."
- `12-alignment-gap.md` summarises "16 in-scope subsystems — 7 faithful, 9
  faithful-with-deviations, **0 diverging**." This document **confirms 0
  behavioural divergence** on everything implemented — but adds that the paper
  specifies a *larger* methodology surface than those 16 subsystems, and ~half
  of the paper's §3 (construction §3.2, background AADF §3.3, editing §3.5,
  entities §3.6) is unimplemented. `12-alignment-gap.md`'s table is complete for
  the *rendering* subsystems; it does not have rows for the construction/editing/
  entity methodology because those were scoped out — so their absence does not
  show up as a "gap" there. This document adds those rows.
- The one item both documents treat identically: **bug B-1** (TAA camera-motion
  reprojection decay). `12-alignment-gap.md` calls it "the one blocking item";
  this document agrees and explicitly keeps it *off* the completion list as a
  defect-within-an-implemented-subsystem rather than a missing-methodology item.

**Bottom line of the reconciliation:** `12-alignment-gap.md` is right that the
**in-scope port is functionally complete and faithful**. This document adds the
wider truth: the **in-scope subset is the paper's rendering half**, and a
canonically-complete NAADF additionally needs the paper's construction/editing/
entities half — Phase C plus an entities track — none of which is drift, all of
which is scoped-out work the orchestration always knew about.
