# Design-Exploration Q&A — Methodology, Capabilities, VRAM

**Provenance:** exploratory chat session, 2026-05-14. **Not a gated phase** — this is reference
context for future scoping (Phase A-2 / B / C designs, and any feature-scoping conversation).
Self-contained: all paths absolute or repo-root-relative.

**How to use:** when a future phase or feature touches texturing, dynamic entities, LOD,
streaming, microvoxels, or the GI VRAM budget, read the relevant section here first — it records
the conclusions already reached so they are not re-litigated. One **binding decision** is
recorded in §6 (TAA history-size lever); one item is **under review** (DLSS / DLSS-RR).

**Primary sources referenced:**
- NAADF paper: `/mnt/archive4/PAPERS/Prepared/ulschmid-2026-naadf-voxel-gi.md` (Ulschmid et al.,
  TU Wien, EUROGRAPHICS 2026 / CGF). Read end-to-end for this session.
- Prepared research corpus: `/mnt/archive4/PAPERS/Prepared/` (global, shared, MegaSync-synced).
- NAADF C# reference impl: `/mnt/archive4/DEV/NAADF/NAADF/`.
- Port context/design: `01-context.md`, `02-research.md`, `03-design.md` (this directory).

---

## 1. NAADF in the voxel-raytracing lineage — cross-reference & evaluation

### 1.1 Two lineages, one synthesis

NAADF sits at the confluence of two previously-separate arcs:

- **Spatial-hierarchy ray traversal:** `amanatides-woo-1987-voxel-traversal` (DDA inner loop, no
  empty-space skip) → `frisken-perry-2002-quadtree-octree-traversal` (locational codes, cheap
  tree traversal) → `gobbetti-marton-iglesias-guitian-2008-single-pass-gpu-raycasting` (stackless
  GPU brick-octree, index texture, neighbour pointers) → `crassin-2009-gigavoxels-ray-guided-streaming`
  / `crassin-2011-gigavoxels-thesis` (compact 64-bit N³-tree node, kd-restart, ray-guided
  streaming, cone LOD) → `laine-karras-2010-sparse-voxel-octrees` (ESVO: sparsity + contours) →
  SVDAG / SSVDAG / TSVDAG / HashDAG (subtree merging for compression).
- **Distance-field empty-space skipping:** `hart-1995-sphere-tracing` (safe step = distance to
  nearest surface, isotropic Euclidean radius) → `keinert-2014-enhanced-sphere-tracing`
  (over-relaxation) → `cuntz-kolb-2007-hierarchical-3d-distance-transform` (discrete Euclidean DT
  on a grid, cheap GPU rebuild) → `soderlund-2022-sdf-grid-raytracing` (sparse SDF grids as a
  production primitive).

**NAADF is the first structure to put a directional distance field *inside the cells of a
shallow nested grid* and let a DDA exploit it.** Its nearest prior is Directed Distance Fields
[KBSS01], which stores one per-axis *surface* distance; NAADF stores **all 6 axis directions to
form an empty bounding cuboid** around each empty cell.

### 1.2 Cross-reference

| Ancestor | Inherited | Diverged |
|---|---|---|
| Amanatides-Woo 1987 | DDA *is* NAADF's inner loop | runs it at 3 nested levels; AADF jump adds a multi-cell skip |
| Frisken-Perry 2002 | direct integer-coordinate addressing | **rejects the pointer tree** — three flat buffers |
| Gobbetti 2008 | flat GPU index, empty-cell tagging, descend-then-step | 6 neighbour pointers → 6-direction distance field (hop → jump) |
| GigaVoxels 2009/2011 | "empty cell = single skip, constant cell = analytic"; the 6-direction anisotropic per-cell tuple | fixed 3-level grid not N³-tree; **no out-of-core streaming, no mip-brick LOD** |
| ESVO / SVO / SVDAG | hash-based dedup of identical geometry (explicitly inspired by HashDAG) | only 3 layers, separate buffers → editable without rebuild |
| Hart 1995 → Söderlund 2022 | "distance to nearest geometry = safe step length" | axis-aligned per-direction 2–5-bit *integer* distance, not isotropic Euclidean float — exact along axes, cheap, DDA-native, **no diagonal skip** |
| Cuntz-Kolb 2007 | "distance stored per cell, cheaply rebuildable" | drops the Euclidean metric, closest-point channel, multi-pass transform, and √3·Δ error — AADF is exact-by-construction, O(3dn) linear |
| Crassin VCT 2011 / VXGI | "accelerate GI through the voxel structure" | **the anti-VXGI**: true rays through the exact structure + ReSTIR + long-term TAA, not biased cones through a pre-filtered mip pyramid |
| Lefebvre-Hoppe 2006 | a GPU hash table as compact voxel storage | hashes *block content* for instancing/dedup (tolerates collisions, linear probing), not *positions* for perfect hashing |
| Schwarz-Seidel 2010 | GPU scan-conversion into nested 4³ sub-grids, flood-fill propagation | NAADF's contribution is the hash-dedup layer, not the voxelization |

**One-line placement:** NAADF re-expresses the octree mainstream's empty-space-skipping
intelligence (min/max tagging, analytic empty-cell skip, anisotropic per-cell data, hash dedup)
on Amanatides-Woo's *flat uniform grid* — the hierarchy survives as three stacked grids + a
directional distance field, not a pointer tree. A considered step back toward grids, executed as
synthesis, not regression.

### 1.3 Pros (paper's own numbers)

- **Traversal throughput.** San Miguel @2160p, primary/secondary Mray/s: Grid 794/100, Octree
  1076/249, SVDAG 1074/250 → **NAADF+AADF 7029/2105**. Ablation: nesting alone ≈2×, +isotropic
  SDF ≈4× primary, +AADF → 7× primary / 10× secondary. **AADF beats a true SDF** (7029 vs 4459).
- **GPU-friendliness is the mechanism:** three flat buffers, fixed ≤3-level descent, no stack, no
  random pointer loads — designs out the octree/DAG family's Achilles heel.
- **Editability without rebuild:** 3 layers + separate buffers + flood-fill local AADF
  invalidation, background recompute. Construction 203 ms structure + 6.2 ms AADF (San Miguel)
  vs 6.3–6.6 s octree/SVDAG (caveat: GPU vs CPU).
- **Memory competitive:** 75 MB San Miguel (1 chunks + 7.9 blocks + 66.1 voxels) vs octree 60 MB,
  SVDAG 38.5 MB, dense grid 2.0 GB.
- **Scales where competition OOMs:** 2-teravoxel OASIS×240 renders at 2765/817 Mray/s.
- **GI half exploits structure compactness:** ~2.1× faster pipeline (12.8 vs 27.4 ms @1440p),
  less ghosting/flicker/blur. The 64-bit TAA sample is *only affordable because* the voxel
  structure quantizes normals (3-bit), distance (f16), etc.

### 1.4 Cons / limitations

**Paper's own (§5.3, §6):** more biased than ReSTIR GI (darkening in complex geometry, no
fallback sample); ~32 frames of indirect-lighting temporal lag; memory ~2× (32-frame TAA history
≈ 973 MB @1440p; total pipeline 1645 MB vs 811 MB baseline); voxelized textures → many distinct
types → compression collapse → fewer cache hits; CPU-side editing/entities → CPU-GPU sync
bottleneck; soft sun shadows not handled in resampling.

**Lineage-derived (underplayed by the paper):**
- **No diagonal skipping** — axis-aligned-vs-Euclidean trade. A ray crossing an open void at 45°
  advances only by the smaller axis component per step. NAADF bets empty runs are mostly
  axis-aligned corridors (true for terrain/structured scenes).
- **The chunk layer is a *dense* grid** — O(extent³) memory. At OASIS×240 the chunk texture alone
  is 2147 MB of 3419 MB total. "Scales to teravoxels" is true for *throughput*; the chunk grid
  still pays the cubic cost. This is **the structural wall for large worlds.**
- **No geometric LOD / minification filtering.** Aliasing handled purely temporally (the
  32-frame TAA). The octree mainstream solved this with mip-bricks; NAADF does not.
- **No out-of-core story.** Everything in VRAM, plus a full CPU mirror.

### 1.5 The 6-axis problemspace map

Every voxel paper fights the same four-way tension (memory ↔ traversal speed ↔ editability ↔
scalability). Six axes, with NAADF's bet:

1. **Memory compactness:** dense grid (worst) → **NAADF** → octree → ESVO → SVDAG → TSVDAG (best, static).
2. **GPU traversal speed:** pointer-chased trees (slow) → kd-restart → **NAADF / brickmaps / grid** (fast).
3. **Empty-space skipping:** none → hierarchical descent → min/max metadata → neighbour pointers → isotropic SDF → **directional in-cell DF (NAADF AADF)**.
4. **Editability:** SVDAG (worst) → ESVO → HashDAG → **NAADF** → grid (trivial).
5. **Scalability / out-of-core:** in-core grid → **NAADF** (in-core, dense chunk grid) → GigaVoxels (ray-guided streaming) → OpenVDB/Brickmaps. **NAADF's weakest axis.**
6. **LOD / AA:** GigaVoxels/ESVO solve in the *geometry* domain (mip-bricks); **NAADF solves in the *image* domain** (long-term TAA).

NAADF is a sharp point-pick for **editable, dynamic, GPU-rendered, in-core voxel *game* worlds
with path-traced GI** — trading away out-of-core streaming, geometric LOD, and maximal DAG
compression. Not "a better octree" — a different bet.

---

## 2. PBR textures on voxels — efficient?

**Yes — triplanar projection is unusually efficient on NAADF — but only for repeating-material
detail; unique per-surface detail is what voxels can't do natively.**

- **Current material model:** a voxel = 15-bit type id → palette material entry (paper text says
  16-bit/entry; the C# reference impl uses a 128-bit `Uint4` — see `03-design.md §2.4`,
  divergence #1). Per-type palette, no UVs.
- **Triplanar collapses to single-planar:** voxel faces are axis-aligned and the hit normal is
  already quantized to one of 6 axes (the 3-bit normal). So projection = pick the 2 of 3 world
  coords by the normal axis, **one texture fetch, no blend weights**. World position is exact
  from the DDA.
- **Dedup-preserving (the key efficiency point):** per-type textures projected from *world
  position* are free under block hashing — the texture is a function of `(world_pos, type)`, not
  stored per block, so identical blocks still dedup. Per-voxel UVs or per-block atlas tiles would
  break dedup. For entities, project in *entity-local* space (available during entity-local
  traversal) or the texture swims.
- **Normal/roughness/metallic maps:** sampled triplanar at shade time. The 3-bit G-buffer normal
  stays the *geometric* normal (keeps TAA/ReSTIR stable); the normal-mapped *shading* normal
  lives only in BRDF eval.
- **Costs:** reintroduces random texture fetches into an otherwise cache-tight traversal — fine
  for primary, manageable for GI (only lit samples fully shaded); needs texture mip/LOD from ray
  differentials since NAADF has no geometric LOD.
- **The hard case:** unique non-repeating detail needs per-surface parameterization voxels lack —
  this is the `lefebvre-dachsbacher-2007-tiletrees` problem; that paper is the right reference.
  An atlas-tile pointer breaks dedup + costs memory. NAADF's own future work ("voxels reference a
  3D texture") gestures at the volumetric-texture variant.

---

## 3. Dynamic volumes on physics bodies (Teardown-style) — possible?

**Yes — it is a built-in NAADF feature (paper §3.6, "Adding Dynamic Entities"), and the
architecture is essentially the Teardown model. But "at Teardown scale / with Teardown
destruction" hits limits the paper flags.**

- **Mechanism (§3.6):** each chunk gets an extra 32-bit slot = 24-bit entity-buffer pointer +
  8-bit overlap counter. An entity instance = id + position + rotation + pointer into a separate
  entity-voxel buffer. Entity voxels are 32-bit and **carry their own AADF** — each entity is its
  own NAADF-traversable mini-volume. Instances hash-deduplicated, pointer shared across chunks.
- **Traversal:** main DDA records chunks containing entities; after the main pass, for each such
  chunk it iterates instances → AABB test → on hit, transforms the ray into entity-local space
  and traverses the entity's own AADF volume. Arbitrary rotation supported.
- **Where "like Teardown" strains:**
  - Two-level overlay, **not a full TLAS** — per-chunk list + linear scan of instances in hit
    chunks; no BVH over entities. Dense overlapping debris clouds degrade.
  - **CPU-side entity management + CPU-GPU sync** — §5.3 names "high number of entities" as a CPU
    bottleneck. Teardown-scale post-explosion debris would hit this. Paper future work: GPU.
  - **AADF invalidation churn** — moving entities crossing chunk empty↔nonempty boundaries
    trigger AADF resets; ~10% measured for basic entity handling, more for heavy dynamics.
  - **Destroying entities** — each entity's AADF is precomputed local-space; fracturing means
    re-voxelize + rebuild that entity's AADF (fast/GPU-side, feasible) — but the
    break→refragment→spawn-rigid-bodies loop is application work the paper doesn't cover.
  - **GI + fast dynamics fight** — the 32-frame TAA's lag vs explosive disocclusion.
  - **"Volumes" = solid voxel objects**, rendered by a first-hit *surface* tracer — not
    participating-media (smoke/fire). Those are a separate emission-absorption ray-march.
  - **Physics itself is the app's** — NAADF stores/renders; it *helps* (CPU mirror exists partly
    for collision; voxel structure is good for voxel-vs-voxel + volumetric ops).
- **Port status:** entities are an explicitly **deferred, feature-flagged** sub-feature
  (`03-design.md §7.5`) — `entities` Cargo feature off by default, `#ifdef ENTITIES` traversal
  branch omitted in Phase A. wgpu wrinkle: no `Rg64Uint`, so the widened chunk format becomes
  `Rg32Uint` or a `vec2<u32>` storage buffer.

---

## 4. Microvoxels / accurate normals — suitable?

**Partially. NAADF's traversal still beats a plain grid or pointer-octree at high density — but
microvoxels and accurate normals each hit something load-bearing.**

### 4.1 Density / microvoxels

NAADF is tuned for surface-sparse, redundant, ~normal-density worlds. Microvoxels stress its
three weak axes:
- **Dense chunk grid grows cubically** — at microvoxel resolution over any sizable world the
  top-level grid is the memory wall (no sparse index above the chunk layer).
- **Hash-dedup collapses on unique fine detail** — the §5.3 failure case ("large number of
  distinct voxel types").
- **No LOD — the disqualifier.** The microvoxel regime needs LOD most (sub-pixel voxels must be
  filtered); NAADF has zero geometric LOD, only the TAA. Sub-pixel density under motion = shimmer
  the TAA alone can't fully resolve.
- AADF bit caps (2-bit→3 for block/voxel, 5-bit→31 for chunks) were sized for normal-density
  voxels; at microvoxel scale the chunk AADF saturates and takes repeated max-distance jumps.

### 4.2 Accurate normals

NAADF stores a **3-bit normal — 6 axis-aligned directions** — and that is *load-bearing* for the
GI pipeline (plane-based G-buffer, specular-path plane reconstruction, 64-bit TAA sample, ReSTIR
geometry checks). The compactness that makes 32-frame history affordable *is* the axis-quantized
normal.
- **Accurate *shading* normals — yes, bolt-on.** Normal map (triplanar) or 3×3×3 occupancy
  gradient, perturb in BRDF at shade time. Stored normal stays the 3-bit face normal.
- **Accurate *geometric* normals (smooth silhouettes) — no, not without changing what NAADF is.**
  Needs a smooth field whose gradient is the normal — a trilinear SDF (the
  `soderlund-2022-sdf-grid-raytracing` lineage). NAADF voxels are binary occupancy + type; the
  AADF is an empty-space-skip distance, **not** a surface SDF (its gradient doesn't point at the
  surface). True smooth geometry = store per-voxel SDF + swap the occupancy first-hit tracer for
  an isosurface (cubic-solve) tracer = you've left NAADF for Söderlund's design and inflated the
  buffers NAADF compresses.
- ESVO added **contours** to leaf voxels for exactly this (non-axis-aligned surfaces without
  microvoxel density); NAADF has no contour analogue — it took the "blocky aesthetic, lean on
  TAA" branch.

**Escape hatch:** at genuine sub-pixel density the blocky 3-bit normal averaged by the TAA over
many microvoxels per pixel approximates a smooth normal (the Euclideon/Atomontage thesis) — but
that doubles down on every density cost and leans entirely on a motion-fragile TAA.

---

## 5. LOD — native vs. addable; VRAM-bounded world

**Mostly "doesn't offer it natively" (out of scope, not architecturally forbidden) — but two
concrete points genuinely "don't pair well."**

- NAADF has **no LOD mechanism at all** — no mip-bricks, no contours, no prefiltered pyramid. The
  three layers are *spatial-subdivision* layers, not *resolution* layers (all bottom out at the
  same voxel size — unlike an octree where every interior node is a valid coarser
  representation). "Uniformly-full" is compression of homogeneous regions, not LOD.
- **You can add LOD** (e.g. a voxel clipmap — concentric NAADF instances at halving resolution,
  conceptually close to `losasso-hoppe-2004-geometry-clipmaps`); the traversal shader is
  agnostic. **But two things fight it:**
  1. **The voxel "type" isn't filterable** — a 15-bit palette pointer, not a material. Mipping a
     buffer of palette indices is ill-defined. You'd downsample in *material* space (resolve →
     average → re-find/allocate a type) or pick a lossy representative (popping).
  2. **The dense chunk grid resists streaming** — a dense 3D texture indexed by chunk coord. To
     page it you'd re-add a chunk index/page table — the GigaVoxels mechanism NAADF stripped out
     for speed. (Block/voxel buffers are already pointer-referenced + growable, so those page
     more easily.)
- Cost of no-LOD is **aliasing + shading work, not traversal cost** — the AADF still skips
  distant empty space fine; distant *detailed surfaces* shimmer.

**VRAM-bounded world — confirmed.** NAADF is in-core, no streaming → the model is a **bounded
play-volume**. Budget = `VRAM − GI_pipeline_fixed(resolution) − overhead`:
- **GI pipeline fixed cost** (world-independent): ~1645 MB @1440p, of which 32-frame TAA history
  ≈ 973 MB.
- **World structure:** ~75 MB for San Miguel-scale — memory-efficient, competitive with octrees.
- The budget bounds **two separate knobs:** *extent* (bounded hard by the cubic chunk grid — the
  real wall) and *detail/uniqueness* (bounded by the voxel buffer, softened by hashing — a large
  *repetitive* world is far cheaper than a small *unique-detail* one).
- Short of true out-of-core: a **camera-centered sliding chunk window** — manual streaming at
  chunk granularity, feasible because construction is fast (~6.2 ms to rebuild all San Miguel
  AADFs) and block/voxel buffers are growable. This is the direction the port already scoped
  (Phase A is a fixed-extent grid; world-gen + out-of-core deferred per D2).

---

## 6. VRAM: the 32-frame TAA, and the history-size lever — **DECISION**

The 32-frame long-term-memory TAA history (~973 MB @1440p, 32-sample) is the **single fattest
buffer** in NAADF's GI pipeline. Replacing it can save VRAM, but it is **load-bearing for both
headline results**: its per-pixel accumulated *sample count* drives the adaptive ~0.25-spp
sampling (the 2× speedup), and its long history kills ghosting/flicker/blur. Delete it naively
and you forfeit the speedup *and* the quality story — landing back at the paper's baseline.

- **Solari does not compose** — Bevy Solari is a triangle/BVH hardware-RT renderer; it cannot
  trace NAADF's voxel DDA. A Solari-*style* (ReSTIR + denoiser) pipeline on NAADF hit data is
  smaller (~baseline 811 MB shape) but *is* the baseline — baseline quality and speed.
- **DLSS-RR is the realistic lever, with caveats** — collapses [32-frame TAA history + bilateral
  denoiser] into one neural pass (~few-hundred-MB working set); net few-hundred-MB win. But it
  needs conventional guide buffers (motion vectors, depth, normals) NAADF deliberately doesn't
  store → **G-buffer extensions required**, clawing back some saving; it's trained on triangle
  content (voxel sharp edges = quality risk); and it gives no "where don't I need to sample"
  signal → loses the adaptive sampling unless that is preserved separately.

### DECISION (2026-05-14, binding for Phase A-2 design)

> **Going with the TAA history-size lever.** The TAA stays NAADF's own long-term-memory TAA; the
> VRAM dial is the **sample-count knob the paper already exposes** — run **16-sample** history
> (~501 MB) instead of 32-sample (~973 MB): a ~470 MB saving, pipeline fully intact, modest
> quality reduction ("Ours 16s" is slightly noisier but still far better than color-clamping
> TAA). This is the lever NAADF was *designed* to offer for exactly this trade.

### UNDER REVIEW — DLSS / DLSS-RR

> DLSS/DLSS-RR is **under review as an upscaler running over NAADF** (not as a replacement
> renderer). Known prerequisites/notes for whoever picks this up:
> - **Requires G-buffer extensions** — DLSS/DLSS-RR needs conventional guide buffers (motion
>   vectors, depth, normals) that NAADF's plane-based / 3-bit-normal / reconstructed-depth
>   G-buffer does not materialize. Cost of materializing those partially offsets the VRAM saving.
> - **DLSS-RR may help reduce noise on the 16-frame history** — i.e. pair DLSS-RR's denoising
>   with the (now 16-sample) long-term TAA rather than replace it: TAA keeps the sample-count
>   signal + adaptive sampling, DLSS-RR cleans residual noise and upscales. This is the
>   non-destructive composition (does not touch the history-size decision above).
> - Voxel-sharp-edge behaviour under a triangle-trained network is an open quality question.
> - The `dlss` / `force_disable_dlss` Cargo feature plumbing is kept in the scaffold
>   (`03-design.md §1.1`) — dormant, available for this evaluation.

---

## 7. Open threads for future orchestration

Consolidated list of items raised this session that are deferred, conditional, or under review —
none are Phase-A blockers:

| thread | status | where it lands |
|---|---|---|
| 16-sample TAA history (VRAM lever) | **DECIDED** (§6) | Phase A-2 (TAA) design |
| DLSS / DLSS-RR as upscaler over NAADF + required G-buffer extensions | **under review** (§6) | Phase A-2 / B evaluation |
| PBR textures via triplanar/per-type projection | feasible, dedup-preserving (§2) | post-Phase-B feature scoping |
| Unique per-surface texture detail (atlas/TileTrees) | breaks dedup — design cost flagged (§2) | feature scoping if ever needed |
| Dynamic entities (Teardown-style) | built-in NAADF feature; deferred + feature-flagged in port (§3) | post-Phase-C, `entities` feature |
| Geometric LOD (voxel clipmap) | addable; type-buffer not filterable, chunk grid resists streaming (§5) | only if large/streamed worlds are scoped |
| Out-of-core / sliding chunk window | not in NAADF; manual chunk-window is the pragmatic path (§5) | only if world exceeds VRAM budget |
| Microvoxels / smooth geometric normals | not suitable without leaving NAADF's design (§4) | not recommended as a target |
| Dense chunk grid = the cubic-memory wall for large worlds | structural, known | informs any extent-scaling work |
