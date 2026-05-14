# 02 — Research / Porting Reference

## research findings (2026-05-14)

This document maps the **whole NAADF paper** and the **whole in-scope NAADF C# tree** into
terms a Bevy/Rust architect and implementer can act on. It is a *reference*, not an
architecture design. Per 01-context.md decision **Q3**, the AADF data structure's source of
truth is the **paper**; the C# is the correctness cross-check. Every subsystem / data type /
algorithm / shader is **phase-tagged**:

- **Phase A** — NAADF substrate + albedo first-hit render. Research-doc §3 + `Libraries/VoxelsCore`
  + `World/{Data,Generator}` + `World/Model`. Maps to NAADF's `WorldRenderAlbedo` version.
- **Phase B** — the GI pipeline. Research-doc §4. Maps to `WorldRender{Base,PathTracer}` + the
  bulk of `Content/shaders/render/**`.

Verified file/line references throughout; nothing here is invented.

---

# 1. Paper digest (precise)

## 1.1 The NAADF data structure (research §3.1–3.4) — **Phase A**

### 1.1.1 Three-layer cell hierarchy (§3.1, Figure 2; paper lines 135–143)

A **shallow three-layer hierarchy**, each layer a 4×4×4 (= 64) grid of the layer below:

```
chunk  (4³ blocks)  →  block  (4³ voxels)  →  voxel
 = 64³ voxels span        = 4³ voxels span      = 1 voxel
```

Each layer is stored in **its own buffer** (chunk buffer / block buffer / voxel buffer) — no
interleaving, no pointer-chasing tree. NAADF generalises all three layers to the term **cell**.
The paper notes a simpler two-layer 8³ hierarchy was considered but the third layer "improves
compression without degrading performance" for larger scenes.

Each **voxel** is a 15-bit type pointer into a **material buffer**; each material entry is 16
bits and stores albedo color + emissivity/reflectivity + roughness. (C# cross-check: NAADF's
shipping material entry is actually wider — see §4.6 — the paper's "16 bits" is the minimal
description; the C# `VoxelType.compressForRender()` produces 4×32 bits. **Divergence noted in §4.**)

### 1.1.2 Cell states & bit layout (§3.1, Figure 2)

Every cell has one of **three states**:

| state | meaning | what the cell stores |
|---|---|---|
| empty (`00` / `0`) | no geometry | the **AADF** (6-direction distances) |
| uniformly full (`01` / `1`) | all child voxels same type | the **15-bit voxel type** |
| mixed (`10`) | partially filled / mixed types | a **child pointer** to the lower layer |

**Bit budgets (paper):**

- **Chunk** = 32 bits: `2` bits state + `30` bits payload.
  - empty → 30-bit AADF = **5 bits per direction × 6 directions** (max value 31 = 496 voxel
    distances).
  - uniform → 15-bit voxel type.
  - mixed → child pointer to a consecutive array of 64 blocks in the block buffer.
- **Block** = 32 bits, layout "similar to a chunk", except:
  - mixed → pointer to a **hashed** group of 64 voxels (deduplication of equal voxel groups).
  - AADF has only **2 bits per direction** (max distance 3 — sufficient to reach the chunk's
    4³ bounding box).
- **Voxel** = 16 bits: `1` bit state (full/empty) + `15` bits for voxel type **or** AADF
  (2 bits per direction).

> **C# cross-check (verified):** The shipping engine encodes chunk/block state in the **top 2
> bits** (`>> 30`), payload in the low 30 bits (`& 0x3FFFFFFF`). State `2` = mixed/child,
> `1` = uniform-full, `0` = empty. A *voxel* uses bit 15 as the full/empty flag (`>> 15`),
> low 15 bits as type/AADF. Voxels are stored **packed two per `uint`** (`voxel1 | voxel2<<16`)
> in the voxel buffer, so a "voxel index" addresses `dataVoxel[index/2]` then masks
> `>> (16 * (index & 1))`. AADF direction bit positions in the C# (`rayTracing.fxh`,
> `boundsCommon.fxh`): voxel/block use 2-bit fields at shifts `0,2,4,6,8,10` for
> `-x,+x,-y,+y,-z,+z`; chunk uses 5-bit fields at shifts `0,5,10,15,20,25`. **The paper's bit
> budget matches the C# exactly.** Confirmed in `WorldData.RayTraversal` (`World/Data/WorldData.cs:396`),
> `shootRay` (`Content/shaders/render/rayTracing.fxh:73`), and `boundsCommon.fxh`.

### 1.1.3 Construction via hashing (§3.2, Algorithm 1; paper lines 145–157, 161–188)

Built **on the GPU** in thread groups of 64 (one thread per block, one group per chunk):

1. For each **block**, test if all 64 voxels are equal → if so, block = that uniform type.
2. Otherwise **hash** the block's 64 voxels with:
   `H = c₀ + Σᵢ cᵢ·vᵢ` for `i ∈ [0,63]`, where `vᵢ` is the voxel type ID and the coefficients
   are `cᵢ = 31^(64-i) mod 2³²` precomputed on the CPU.
3. **Open addressing with linear probing**: atomically check the hash slot. If occupied, compare
   all 64 voxels for equivalence → reuse the slot if equal; else probe up to **100** subsequent
   slots; else append a new block to the buffer. If the buffer reaches **75 % occupancy**,
   resize by **100 %** (double).
4. Group-sync; thread 0 determines if all 64 blocks of the chunk are identical → uniform chunk,
   or empty → empty chunk, or else reserve 64 slots in the block buffer and store the child
   pointer; all threads then write their block in parallel.

Algorithm 1 (paper line 161): per-thread `voxels ← voxelInput[blockID]`; `hash ← getHash`;
uniform → `block ← uniform type`, else `block ← storeOrReuse(hash, voxels)`; sync; thread 0
sets `chunkBuffer` to uniform type or `blocksPointer`; sync; non-uniform → write `block` to
`blockBuffer[blocksPointer + threadIndex]`.

> **C# cross-check (verified):** `Content/shaders/world/data/chunkCalc.fx` `calcBlockFromRawData`
> is Algorithm 1 verbatim — `[numthreads(4,4,4)]`, the exact hash loop, `GetVoxelPointer` doing
> `InterlockedCompareExchange` open-addressing linear probing (max 250 probes, not 100 — minor
> divergence), busy-wait on a `0x80000000`-tagged pending pointer. The CPU-side mirror is
> `BlockHashingHandler.AddBlock` / `getHashOfBlock` (`World/Data/BlockHashingHandler.cs:63,86`)
> — note the CPU hash *masks voxel comps to `0x7FFF`* (strips state bit) while the GPU also
> masks `& 0x7FFF`. CPU resize trigger is `wantedEmptyRatio = 0.5` (so resize at 50 %, not 75 %).
> **Note these constants for the design phase: paper says 100 probes / 75 %, C# uses 250 / 50 %.**

### 1.1.4 AADF augmentation (§3.3, Figure 3; paper lines 190–202)

The AADF of a cell is a **cuboid bounding box empty of geometry** extending around the cell by
some number of cells in each of the **6 axis-aligned directions** (x+, x−, y+, y−, z+, z−).

**Construction algorithm (load-bearing — step by step):**

1. Start with a cuboid equal to the cell itself (all 6 direction values = 0).
2. Iterate, **alternating between the three dimensions**, expanding concurrently in *both*
   positive and negative directions by **one cell per iteration**.
3. The expansion in a given dimension is bounded by either the max AADF size (2-bit → 3,
   5-bit → 31) **or** the containing upper-layer cell.
4. If the cells that *would be added* in the other two dimensions (the new slice) are all
   empty, increment that direction's distance value.
5. **Optimisation to avoid cubic complexity:** synchronise iterations & dimension expansions
   across *all* cells, so each cell just **merges its cuboid with the neighbour cell's
   already-computed cuboid**, giving **O(3·d·n)** linear time (n cells, d = distance to cell
   boundary).

`d` is small: for block/voxel cells (4³ size) `d ≤ 3` → `3·3` iterations total across the 3
axes; stored in **2 bits**. For chunks `d` can be large but is capped at **31** (5 bits) →
`3·31` iterations. Construction is **not time-critical**: modified cells are queued separately
per layer, AADFs computed **in the background during rendering**. Chunks get separate `3·31`
queues, one per iteration; the implementation handles **one queue per frame** (adjustable).
Changing the axis order can produce slightly different cuboids — rare, averages out, not
measured.

> **C# cross-check (verified):** `boundsCommon.fxh` `ComputeBounds4` is the block/voxel version
> — 3 iterations, alternating x/y/z, `checkMatchingBounds` does the 6-direction `>=` slice
> test, neighbour cuboids merged via `cachedCell[]` groupshared. `boundsCalc.fx` does the
> chunk-level background AADF: a **per-bound-size queue system** (`boundQueueInfo[32*3]` —
> 32 sizes × 3 axes), `prepareGroupBounds` picks one queue, `computeGroupBounds` processes a
> group of 4³ chunks (`addBoundsGroup` does the cross-group neighbour merge), re-enqueues into
> the next-larger queue. `WorldBoundHandler.Update` runs 5 prepare+compute rounds per frame,
> throttled by `maxGroupBoundDispatch` (the "AADF speedup" UI slider). The chunk AADF is done
> in **groups of 4³ chunks** ("queue groups") — this is the same grouping as flood-fill (§1.1.6).

### 1.1.5 DDA traversal exploiting AADFs (§3.4; paper lines 204–216)

A **DDA** (Amanatides & Woo) that **advances many voxels along multiple axes in a single
iteration** when inside an empty AADF cuboid:

1. Start at the highest layer — the **chunk** containing the ray origin.
2. **Empty** cell → use its AADF to intersect the empty cuboid with the ray, advancing the ray
   position to the cuboid boundary in one step.
3. **Mixed** cell → descend to the lower layer (block, then voxel).
4. **Full** cell → compute intersection properties (normal, distance, type) and terminate.
5. The DDA computes which axis the next intersection occurs on, advances the ray to that
   boundary (lies on a voxel face), and uses the previously-intersected axis to decide whether
   the next voxel was hit, else continues.

This is the structure's core speed lever: **7× primary / up to 10× secondary** ray throughput
vs. SVDAG (7029 vs. 1074 Mrays/s, San Miguel 2160p, Table 1).

> **C# cross-check (verified):** The reference DDA is `shootRay` in
> `Content/shaders/render/rayTracing.fxh:73`. Per iteration: read chunk from `chunks[chunkPos]`
> 3D texture; if `curNode.x >> 31` (has children — note the C# uses the **top bit** as a
> "has-children" test here, distinct from the 2-bit state) descend into `blocks[]`, then if the
> block has children descend into the packed `voxels[]`; compute `boundsInDir` from the AADF
> bit-fields (`shiftMaskChunk` / `shiftMaskVoxelAndBlocks` select the correct direction field
> by ray sign); if `curNode.x & 0x40000000` it is a full cell → emit hit; else
> `distForIntersect = (1 + boundsInDir - (1-mask)·abs(isNegative - frac(curPos))) · invRayDirAbs`,
> step `curDist` by the min component. `boundsInDir` carries the AADF cell-count so a single
> step jumps the whole empty cuboid. The CPU mirror (`WorldData.RayTraversal`) is the same
> algorithm without the AADF skip on the chunk level fully fleshed out — used for editing
> raycasts only. `MAX_RAY_STEPS_PRIMARY = 120`, `_SECONDARY = 100`, `_SUN = 120`,
> `_SUN_SECONDARY = 80`, `_VISIBILITY = 60`.

### 1.1.6 Editing (§3.5; paper lines 218–220) — **Phase A (data) / editing later**

A **CPU copy of the world** (voxels/blocks/chunks) is kept — also needed for path-finding,
collision, triggers. After GPU world-gen the data is copied to CPU; thereafter every
voxel/block/chunk change is synced CPU→GPU. **When a chunk changes empty↔non-empty, all AADFs
in the surrounding 63³ chunk volume must be reset** (otherwise traversal skips real geometry).
Overlapping reset volumes are deduplicated with a **flood-fill** that marks each chunk for
recompute **only once**; the flood-fill works in **groups of 4³ chunks** ("queue groups").
Marked groups are reset and gradually recomputed in the background.

> **C# cross-check (verified):** `ChangeHandler.UpdateWorld` (`World/Data/ChangeHandler.cs:69`)
> is the flood-fill: `distanceFloodFill[]` per queue-group, BFS via `floodFillQueue`,
> `addBounds`/`checkMatchingBoundCell` propagate per-direction distances 4 cells at a time,
> distance capped at 28, then `changedGroupsWithDist` (Uint2 of packed group pos + distance) is
> uploaded and `worldChange.fx` `applyGroupChange` resets chunk AADFs on the GPU. Edits flow:
> `EditingHandler.processChunks` (`World/Data/EditingHandler.cs:75`) — parallel per edited
> chunk: hash each block, dedup via `BlockHashingHandler`, free old voxel slots, write
> `changedChunks`/`changedBlocks`/`changedVoxels` arrays; `ChangeHandler` uploads them and
> `worldChange.fx` `ApplyChunkChange`/`ApplyBlockChange`/`ApplyVoxelChange` apply them on GPU
> (the block/voxel passes also recompute the local 4³ AADF via `ComputeBounds4`).

### 1.1.7 Dynamic entities (§3.6; paper lines 222–226) — **Phase A (data) / later**

Each chunk is extended with an extra **32-bit value**: 24-bit pointer + 8-bit overlap counter.
The extra buffer holds entity instance data (entity ID, position, rotation, pointer into a
separate entity voxel buffer). Each entity voxel is 32 bits and **also carries AADF values**.
Many chunks share the same entity-set pointer → **hash-based deduplication** of chunk entity
instances (CPU). During traversal, chunks containing entities are recorded; after the main
scene traversal, those chunks' entities are bbox-tested then voxel-traversed with their AADFs.
**Chunk AADFs must not span chunks containing entities** — entity movement triggers the same
empty↔non-empty reset as edits. Measured entity overhead ≈ 10 %.

> **C# cross-check (verified):** Gated behind `BuildFlags.Entities` (`settings.fxh` `#define
> ENTITIES`), which widens the chunk 3D-texture format to `Rg64Uint` (the `.y` channel is the
> entity pointer+counter). `EntityHandler` (`World/Data/EntityHandler.cs`) does the CPU-side
> overlap counting, prefix-sum, and `EntityChunkInstanceHash` dedup; `EntityData` (`EntityData.cs`)
> builds an entity's per-voxel AADF on the CPU with 31 iterations of the same alternating-axis
> algorithm. `entityUpdate.fx` syncs to GPU. `shootRay` in `rayTracing.fxh` has the
> `#ifdef ENTITIES` branch: collects up to 16 `chunksWithEntities`, after the main loop iterates
> them, `rayAABB`-tests each, then runs a 20-step DDA over `entityVoxelData` using the entity's
> AADF. **For the port: entities are Phase A data-model but the brief defers editing/entity
> *behaviour*; recommend the design phase treat entities as a deferred sub-feature even within
> Phase A.**

## 1.2 Section 4 — GI pipeline (research §4.1–4.3) — **Phase B**

### 1.2.1 Pipeline overview (§4, Figure 4; paper lines 230–236)

Deferred-style pipeline, **TAA placed unusually early** (before GI, not after):
`G-Buffer Creation → TAA → GI (Resampling) → Denoiser → Post-processing (tone mapping)`.
TAA output **informs GI sample generation** (the accumulation rate drives the secondary-ray
budget) and **guides the denoiser**.

**Compact G-buffer:** instead of position/depth/normal buffers, NAADF stores **each plane a
ray bounced off** as `(3-bit normal, 14-bit distance along the normal)`. The primary ray path
records **up to four planes** the ray bounced at until a non-specular surface is hit (including
the last). Final position/depth is reconstructed by re-deriving the camera ray and reflecting
it through the stored planes (Figure 5 "virtual path"). Material + entity properties live in
separate buffers. Within each pixel, sample positions are **jittered** (Halton) to reduce
aliasing/Moiré. Jitter + TAA consider albedo; resampling + denoising work with **indirect
illumination only**.

> **C# cross-check:** the 4-plane G-buffer is `firstHitData` (a `Uint4` per pixel). Plane
> encoding in `commonRenderPipeline.fxh`: each `uint` holds a 15-bit payload + `normalTang`
> in bits 15+ (`>> 15`); `normalTang` packs a 3-bit normal index (`& 0x7`) + a distance-along-
> normal (`>> 3`). `getHitDataFromPlanes` reconstructs the virtual path by reflecting the ray
> through up to 3 stored planes (`SPECULAR_MIRROR_FAC`, `NORMAL[8]` lookup tables).
> `getJitter` (`WorldRender.cs:137`) is Halton-2D base (3,7).

### 1.2.2 Long-term-memory TAA (§4.1, Figure 6; paper lines 238–265)

Standard TAA uses a single history buffer + color clamping; color clamping is noise-sensitive
and unsuitable for path tracing. NAADF instead **stores the last 32 frames**, **64 bits per
sample**, and rejects via **depth** (noise-insensitive) instead of color clamping.

**64-bit sample layout (Figure 6):**

| field | bits | notes |
|---|---|---|
| color R / G / B | 8 each (24) | **exponential compression** (see formula below) |
| distance | 16 | float16; distance to first non-specular hit of the primary ray |
| roughness | 5 | material roughness |
| normal | 3 | final normal |
| hash | 16 | extra validation; built from material type + normals of each specular reflection |

**Exponential color compression** (paper line 256, confirmed against page render):
`f(x) = 12·log₂(x/100 + 2^(-255/12)) + 255`, `x ∈ [0,100]`.

One 1440p frame ≈ **29 MB** (vs. 59 MB standard TAA) → 32 frames is affordable. The long
history allows **per-sample** importance assessment (e.g. by viewing-angle change for
reflective materials) instead of the single buffer's implicit exponential falloff.

**Depth-based rejection (per pixel):** pre-compute min/max depth over the **3×3 neighbourhood**;
compute the same hash per pixel; a reprojected sample is accepted only if its depth is in range
**and** its hash matches one of the **9 neighbours**; additionally the sample is projected into
the current frame and must land within **1 pixel** of its origin. Non-fully-diffuse samples
have their contribution reduced by roughness + direction change during movement. Reprojection
picks the **closest matching pixel in the 3×3** (not the current pixel) to avoid edge flicker.
All (re)projections use the sample's **virtual position** (Figure 5).

> **C# cross-check (verified):** `commonTaa.fxh` `compressSample`/`decompressSample` — but
> the *shipping* compressed sample is `uint2` (64 bits) with: `dist` (15-bit f16) | `hash`
> (16 bit) in `.x`; color R/G/B (8+8+8) | `normalComp` (3) | `extraData`/roughness (5) in `.y`.
> The exact compression in code: `12*log2(color + pow(2,-255/12)*100) + (255 - 12*log2(100))`
> — algebraically equal to the paper's formula. `getHashFromData` builds the validation hash.
> The TAA passes are `albedo/renderTaaSampleReverse.fx` (Phase A) and
> `base/renderTaaSampleReverse.fx` (Phase B): `ReprojectOld` does the 3×3 precompute + 32-frame
> reprojection loop + dist/hash/screen-pos checks; `base` version additionally writes
> `taaDistMinMax` and has `CalcNewTaaSample` to fold the new GI result into history. History
> ring buffers are `taaSamples` (`screenW·screenH·32`) and `taaSampleAccum` (`screenW·screenH`),
> `taaIndex = 128 - (frameCount % 128) - 1` (`WorldRender.cs:88`) — note **128**-deep camera-
> matrix history but **32**-deep sample history.

### 1.2.3 Compressed ReSTIR GI resampling (§4.2, Algorithm 2, Figure 7; paper lines 267–323)

Based on ReSTIR GI but adapted for voxel worlds: **no explicit light-source storage** —
**material-based sampling only** (storing light sources is too costly with dynamic/editable
voxels and many small emitters). Challenge: many generated samples hit no light → "unlit".

**Lit/unlit separation & compression (Figure 7):** samples are stored as **lists in structured
buffers** (not textures). **Unlit = 16 bytes** (primary-hit data only), **lit = 32 bytes**
(primary + secondary hit data). To save more memory only **every 8th unlit sample** is stored,
**weighted ×8** (higher ratio adds noise). 4 frames' worth of unlit storage ≈ 32+ frames
accumulated; lit storage is 2 frames' worth but covers up to **64 past frames** (few lit
samples generated). Figure 7 three blocks, each 4×32 bit:
- **Primary hit** (used by lit & unlit): `PLANE0|ENTITY`, `PLANE1|PIXEL X`, `PLANE2|PIXEL Y`,
  `PLANE3|ROUGHNESS|FRAME INDEX`.
- **Secondary hit** (lit only): `PLANE0|color(B,G,R)`, `PLANE1|ENTITY`, `PLANE2|DIRECTION`,
  `DIRECTION`.
- **Refined data** (produced from valid lit temporal samples for the spatial pass):
  `SURFACE Y|color`, `SAMPLE DIST|SAMPLE NORMAL`, `SURFACE X|SAMPLE DIR`,
  `SURFACE Z|SAMPLE DIR|MATERIAL`.

**Sample generation:** uses the long-term-memory TAA accumulated sample count to **selectively
generate samples only where needed** — newly revealed/disoccluded pixels get priority,
well-converged pixels are skipped. Secondary rays use **at most 3 bounces**. Effective rate
≈ **0.25–1 spp** per frame.

**Temporal resampling:** generated/stored samples are projected into **8×8 disjoint
screen-space pixel regions** (not per-pixel reservoirs). Per region: up to **32 lit samples**
stored in an extra buffer; unlit samples just increment a region counter. Lit samples are then
**brightness-filtered**: compare each to the region's max brightness, remove weakly-lit
samples and compensate by boosting the survivors by the removal probability. Up to **8**
survivors are **refined** and stored for spatial resampling; excess (>8) is discarded (some
bias, but kills fireflies).

**Spatial resampling (Algorithm 2; paper lines 341–367):** ReSTIR GI runs 3 iterations each
with a visibility check; NAADF runs **12 iterations** but tests visibility **only for the
final selected sample** (visibility is the most expensive part). It does **not** start with an
initial sample (reduces fireflies; loses the fallback but the better lit-sample-finding
compensates). Per iteration: randomly pick a neighbouring 8×8 region within an **adaptive
per-pixel radius**, retrieve region info, skip if invalid w.r.t. surface `s`, randomly select
a lit sample, skip if geometry invalid, compute & verify the **Jacobian** `|J|`, compute
`cₙ = Rₙ.color · (litCount/totalCount)`, `p̂ₙ = TargetFunction(Rₙ,cₙ)/|J|`, merge into the
reservoir `Rₛ`. After the loop: **single visibility check** — return 0 if occluded, else
`ConvertToColor(Rₛ)`.

> **C# cross-check (verified):** `WorldRenderBase` constants (`World/Render/Versions/WorldRenderBase.cs:57`):
> `globalIllumValidSampleStorageCount=2`, `InvalidSampleStorageCount=8`, `BucketStorageCount=32`,
> `RefinedBucketStorageCount=8`. Buffers: `globalIlumValidSamples` (`Uint8`/32-byte, `2×screen`),
> `globalIlumInvalidSamples` (`Uint4`/16-byte, `8×screen`), `globalIlumValidSamplesRefined`
> (`Uint4`, `bucketCount×32`), `globalIlumValidSamplesCompressed` (`Uint4`, `bucketCount×8`),
> `globalIlumBucketInfo` (`Uint2` per 8×8 bucket), `globalIlumSampleCounts` (`Uint2[128+3]`
> ring of per-frame counts). Pipeline: `rayQueueCalc.fx` `RayQueue` decides which pixels get
> secondary rays (`shouldRay` uses `taaSampleAccum` accum count, `skipSamples` toggles
> 1↔0.25 spp); `renderGlobalIllum.fx` `GlobalIlum` traces ≤3-bounce secondary rays and writes
> compressed lit/invalid samples; `renderSampleRefine.fx` has 5 passes
> (`ClearBucketsAndCalcMask`, `ValidHistory`, `CountValidAndRefine`, `CountInvalid`,
> `RefineBuckets`) — `RefineBuckets` is the brightness-leveling/`COLOR_DIF_PROB` removal step;
> `renderSpatialResampling.fx` `SpatialResampling` is Algorithm 2 — 12-iteration neighbour loop
> (`sampleNeighbors`), Jacobian (`jacobianNow/jacobianNeighbor`), `getTargetFunctionNew`,
> reservoir merge, then a **single 3-step visibility ray** at the end. The "adaptive radius" is
> `isVaryingResmaplingRadius` (a 12-tap pre-pass estimating `radiusFac`).

### 1.2.4 Sparse bilateral denoiser (§4.3; paper lines 325–372) — **Phase B**

Applied **only to indirect illumination**. A **sparse bilateral filter** (kernel size **21**,
σ = **10**) in screen space, split into successive **horizontal then vertical** kernels. On
average **every 2nd pixel** is processed (random) — sparsity allows a larger kernel for
medium-frequency noise. Two weight components: **color** (TAA result normalised by material
albedo → luminance) and **geometry** (normal-direction + depth-as-plane difference, reduces
edge bleeding). The denoiser result is added to the TAA color and stored as part of the
32-frame TAA history. An **SVGF** alternative is provided (more aggressive, overblurs, 442 MB /
7.46 ms vs. the sparse filter's 89 MB / 0.67 ms at 1440p). Choice left to the user.

> **C# cross-check (verified):** `renderDenoiseSplit.fx` `CalcDenoiseHorizontal` /
> `CalcDenoiseVertical` — `±10` taps each (kernel 21), `gaussianF(.,10)`, `bilateralFac =
> 1/(1 + |Δtaa| · denoiseThresh)` plus a normal/state match term, sparse via per-tap random
> offset. `denoisePreprocessed`/`denoisePreprocessedHorizontal` are the intermediate buffers
> written by `renderSpatialResampling.fx`. NAADF ships **only the sparse bilateral** in this
> tree — no SVGF shader present (the paper's SVGF impl is not in the in-scope source).

### 1.2.5 Atmosphere model (Phase A/B)

Not a paper-headline contribution but a needed subsystem. A multiple-scattering sky model
(Rayleigh/Mie/Ozone), CPU version in `World/Render/Atmosphere.cs` (used for sun-color sampling),
GPU version in `atmosphereRaw.fxh` (`addLightForDirection` ray-marches the atmosphere) +
`atmospherePrecomputed.fxh` (`applyAtmosphere` samples a precomputed octahedral-mapped
`atmosphereComp` texture). `WorldRenderAlbedo` does **not** precompute atmosphere (it inlines a
simple sun term); `WorldRenderBase`/`PathTracer` precompute it each frame via
`renderAtmosphere.fx`.

---

# 2. Per-subsystem reference — `Libraries/VoxelsCore/` — **Phase A**

This library is the *import/authoring* voxel model — **distinct from** the runtime
chunk/block/voxel NAADF structure (§3 in `World/Data`). It is only used by the `.vox` importer
path and `ModelData`. Most of it can be replaced by Bevy/`glam`/`bevy_color` types.

| C# type (file) | data layout | role | → Bevy/Rust equivalent | phase |
|---|---|---|---|---|
| `XYZ` (`XYZ.cs`) | 3×`i32` struct, full operator set, `Transform(Matrix4x4)` | integer 3D coord / area; "XY horizontal, Z vertical" | **replace with `glam::IVec3`** (mind axis convention) | A |
| `Point3` (`Common/DataTypes/Point3.cs`) | 3×`i32`, `+ - * /  %`, `ToVector3` | runtime integer coord (the actually-used one in `World/`) | **replace with `glam::IVec3`** | A |
| `BoundsXYZ` (`BoundsXYZ.cs`) | inclusive `Min`/`Max` `XYZ` | voxel AABB; `Add` (union), `Transform`, `CreateEmpty` | **replace with a small `IAabb3` or reuse `bevy::math` bounds** | A |
| `Color` (`Color.cs`) | `[StructLayout Explicit]` union of `u32 RGBA` + 4×`u8`; HSV conv | voxel/palette color | **replace with `bevy_color` / `[u8;4]`** | A |
| `Voxel` (`Voxel.cs`) | union of `Color` + `u32 Index` (16 bytes via overlap) | a voxel = palette index *or* direct color | small Rust enum/newtype; only needed on the import path | A |
| `Material` (`Material.cs`) | 5×`f32`: emit, flux, metalic, roughness, ior | per-palette-entry material (from `.vox` MagicaVoxel mat) | plain Rust struct; feeds `VoxelType` derivation | A |
| `VoxelData` / `VoxelData<T>` / `VoxelDataBytes` / `VoxelDataColors` / `VoxelDataT.cs` | abstract grid + 32³-chunk dictionary backing (`VoxelData<T>.Chunk`) | sparse authoring grid, `T` = `byte` (palette) or `Color` | **replace with a simple `HashMap<IVec3, [T;N]>` or just feed the importer** | A |
| `VoxelImport` (`VoxelImport.cs`) | static dispatch on extension | entry to `.vox` import | **out of scope** (importers) — one-line mention | A |
| `MagicaVoxel.cs` / `VoxFile.cs` / `Voxlap.cs` | file-format parsers | `.vox` / `.vl32` parsing | **OUT OF SCOPE** per Q1 — do not port | — |

**Orchestration:** none — pure data types, used by `World/Model/ModelData.cs`.

**First-pass Bevy note:** Almost the entire library collapses to `glam` + `bevy_color` + a thin
authoring grid. The *only* thing worth keeping conceptually is the `Voxel` palette-index vs.
direct-color duality and the `Material` 5-float layout (it is the source of the runtime
`VoxelType`). The runtime NAADF structure does **not** use these types.

---

# 3. Per-subsystem reference — `Common/` — **Phase A**

| C# type (file) | data layout | role | → Bevy/Rust equivalent | phase |
|---|---|---|---|---|
| `Point3` | see §2 | runtime integer coord | `glam::IVec3` | A |
| `Cube` (`Cube.cs`) | MonoGame `VertexBuffer`, 36 verts | the unit-cube VB drawn by `renderFinal` to run the fullscreen PS | **replace with a Bevy fullscreen-triangle / fullscreen pass** | A |
| `Helper` (`Helper.cs`) | `dataCopy.fx` wrapper | large >2 GB structured-buffer copies in 100 M-element chunks (DX11 limit workaround) | **likely unnecessary in wgpu** (different buffer limits); if needed, a `wgpu::CommandEncoder::copy_buffer_to_buffer` loop | A |
| `DynamicStructuredBuffer` (`DynamicStructuredBuffer.cs`) | wraps `StructuredBuffer`; `SetNewMinCount(count, factor)` resizes, `Resize` copies old→new | **growable GPU buffer** — the key reusable abstraction the reuse audit flagged | **needs a `wgpu::Buffer` wrapper** that re-allocates + copies on growth; central to voxel/block buffers, `changedX` buffers, `typesRenderGpu` | A |
| `Camera` + `PositionSplit` (`Camera.cs`) | `PositionSplit` = `Point3 integer` + `Vector3 frac` (split world-space position for large-world precision); `Camera` holds proj/view matrices, free-fly input | NAADF's camera; **integer+frac split** is the large-world precision mechanism | per reuse audit: **start with Bevy `FreeCamera`**; port `PositionSplit` *only if* large-world precision demands it. **But note:** every render shader takes `camPosInt`/`camPosFrac` separately and the whole G-buffer/TAA reprojection math is built on int+frac — see §6 open question | A |
| `CommonExtensions.setCameraPos` | sets `camPosIntX/Y/Z` + `camPosFrac` effect params | the int+frac upload convention | a uniform-struct helper | A |
| `Uint2/3/4/8` (`Common/DataTypes/Other.cs`) | plain packed `u32` tuples | GPU struct element types (the buffers are typed `Uint4`, `Uint8`, etc.) | **replace with `[u32; N]` or `#[repr(C)]` structs** + `bytemuck` | A |

**First-pass Bevy note:** `DynamicStructuredBuffer` is the one genuinely load-bearing piece —
build a `GrowableBuffer` wrapper early. The int/frac camera is the subtle one: it is not
optional cosmetics, it is woven through every shader's coordinate math. The design phase must
decide whether to keep Bevy's `f32`-position camera (and accept reduced large-world range) or
port `PositionSplit`.

---

# 4. Per-subsystem reference — `World/` — runtime NAADF engine

## 4.1 `WorldHandler` (`World/WorldHandler.cs`) — context

Top-level orchestrator: owns `WorldGeneratorModel`, `WorldData`, `VoxelTypeHandler`,
`ModelHandler`, `PathHandler` (camera-path/screenshot — out of scope). `Initialize()` creates a
`WorldData` of `worldSizeToUseInWorldGenSegments (16,2,16) · 4 groups · 64 = (4096, 512, 4096)`
voxels and loads `Content/oasis.cvox`. `Update`/`Render` fan out to the subsystems.

> **→ Bevy:** maps to a Bevy `Plugin` + a few `Resource`s + systems; **do not port the handler
> class hierarchy verbatim** (forbidden move in 01-context.md).

## 4.2 `WorldData` (`World/Data/WorldData.cs`, ~522 lines) — **Phase A** — the core runtime structure

The runtime NAADF structure. **Data members / GPU resources:**

| member | type | layout / role |
|---|---|---|
| `dataChunkGpu` | `Texture3D` `R32Uint` (or `Rg64Uint` if entities) | the **chunk buffer** — one `uint` (or `uint2`) per chunk, indexed `[chunkPos]` |
| `dataBlockGpu` | `DynamicStructuredBuffer<uint>` | the **block buffer** — 64 consecutive blocks per mixed chunk |
| `dataVoxelGpu` | `DynamicStructuredBuffer<uint>` | the **voxel buffer** — 32 `uint`s = 64 packed voxels per mixed block |
| `dataChunk/Block/Voxel` | `uint[]` CPU mirrors | the CPU world copy (§1.1.6) |
| `blockVoxelCountGpu` | `StructuredBuffer<uint>` ×2 | `[0]`=voxel write cursor, `[1]`=block write cursor (atomic during gen) |
| `segmentVoxelBuffer` | `StructuredBuffer<uint>` | scratch buffer for one world-gen segment of raw voxels |
| `freeVoxelSlots`/`freeBlockSlots` | `ConcurrentQueue<uint>` | free-list for editing |
| sizes | `sizeInVoxels/Blocks/Chunks/QueueGroups`, `chunkCount`, `queueGroupCount` | derived geometry; world is sized in **world-gen segments** (`segmentSizeInGroups·4` chunks, `·16` voxels), a "queue group" = 4³ chunks |

**Sub-handlers it owns:** `BlockHashingHandler`, `WorldBoundHandler`, `EntityHandler`,
`ChangeHandler`, `EditingHandler`.

**Key algorithms:**
- `GenerateWorld(WorldGenerator)` — loops world-gen segments, per segment calls
  `worldGenerator.CopyToChunkData` (→ `segmentVoxelBuffer`) then `CalculateChunkBlocks` (→
  `chunkCalc.fx` `VoxelHash` = Algorithm 1), grows `dataBlockGpu`/`dataVoxelGpu` as the
  atomic counters advance; after all segments: copy GPU→CPU, run `ComputeVoxelBounds` /
  `ComputeBlockBounds` (the local-AADF passes), sync the hash map.
- `FillChunkData` / `FillBlockData` — decompress a chunk's region back to a flat 2048-`uint`
  (64 blocks × 32) edit buffer.
- `AddVoxels` / `SetBlocks` / `SetChunk` — CPU-side allocation into the buffers (with
  free-list + `Array.Resize` growth), `SetChunk` notifies `changeHandler` on empty↔non-empty.
- `RayTraversal` — CPU mirror of the DDA (used by editing tools' picking rays).
- `setEffect` — binds all the voxel buffers + voxel-type buffer + entity buffers onto a shader.

> **→ Bevy:** `WorldData` becomes a `Resource` holding the three growable `wgpu::Buffer`s
> (chunk as a 3D `wgpu::Texture`, block/voxel as `GrowableBuffer`) + the CPU mirrors. The
> `*Handler` members become separate systems/resources. `setEffect` becomes a bind-group
> layout. The DDA `RayTraversal` is needed CPU-side for picking even in Phase A.

## 4.3 `BlockHashingHandler` (`World/Data/BlockHashingHandler.cs`, ~213 lines) — **Phase A**

The block-deduplication hash map. `BlockValue { voxelsPointer, blockUseCount, hash }` (12 bytes).
`map` is a power-of-two-sized open-addressing table mirrored CPU (`map[]`) + GPU (`mapGpu`).
`coefficients[65]` = the `31^(64-i)` hash coefficients. `getHashOfBlock` computes `H` (CPU),
`AddBlock`/`DeleteBlock` do linear-probe insert/refcount-decrement, `IncreaseSizeToNewCount`
doubles the table and re-hashes via `mapCopy.fx`. `wantedEmptyRatio = 0.5`.

> **→ Bevy:** a `Resource` holding a CPU `Vec<BlockValue>` + a GPU buffer; the GPU re-hash on
> grow is `mapCopy.fx`. The GPU-side insert lives in `chunkCalc.fx`/`worldChange.fx`.

## 4.4 `ChangeHandler` (`World/Data/ChangeHandler.cs`, ~312 lines) — **Phase A**

The **flood-fill AADF-invalidation** subsystem (§1.1.6). CPU members: `distanceFloodFill[]`
(per queue-group distance), `floodFillQueue`, `changedGroups`/`changedGroupsWithDist`,
`changedChunks`/`changedBlocks`/`changedVoxels`. `AddChangedChunk` seeds the BFS;
`UpdateWorld` runs the flood-fill (BFS expand + 7 rounds of per-direction `addBounds`
propagation, distances stepped by 4, capped 28), packs `changedGroupsWithDist`, uploads the
4 changed-X dynamic buffers, dispatches `worldChange.fx` (`ApplyChunkChange`,
`ApplyBlockChange`, `ApplyVoxelChange`, `ApplyGroupChange`). `MASK_M*/P*` direction masks.

> **→ Bevy:** a system that runs the CPU flood-fill and dispatches the `worldChange` compute
> passes. The four `changedX` dynamic buffers want the `GrowableBuffer` wrapper.

## 4.5 `WorldBoundHandler` (`World/Data/WorldBoundHandler.cs`, ~141 lines) — **Phase A**

The **background chunk-AADF computation** queue manager (§1.1.4). `BoundGroupSize = 4³`,
processes chunk-AADFs in 4³ "bound groups". GPU buffers: `boundQueueInfoGpu` (`BoundQueueInfo
{start,size}` × 32 sizes × 3 axes), `boundGroupQueuesGpu` (the queues), `boundGroupMasksGpu`
(`Uint3` per group — which queues a group is already in), `boundRefinedInfoGpu`,
`boundGroupQueueDispatchCount` (indirect-dispatch args). `Initialize` seeds all groups into the
size-0 queues; `Update` runs 5× (`PrepareGroupBounds` then indirect `ComputeGroupBounds`),
throttled by `maxGroupBoundDispatch`.

> **→ Bevy:** a render-graph node group running the `boundsCalc.fx` passes with indirect
> dispatch each frame; the per-size queue buffers are fixed-size GPU buffers.

## 4.6 `VoxelTypeHandler` (`World/VoxelTypeHandler.cs`, ~169 lines) — **Phase A (types) / B (emissive use)**

The voxel **type / layered-material system**.

```
enum MaterialTypeBase  { Diffuse=0, Emissive=1, MetallicRough=2, MetallicMirror=3 }
enum MaterialTypeLayer { None=0,    MetallicRough=2, MetallicMirror=3 }
struct VoxelType { string ID; uint renderIndex; Vector3 colorBase, colorLayered;
                   MaterialTypeBase materialBase; MaterialTypeLayer materialLayer; float roughness; }
```

`compressForRender()` → `Uint4`: `data1 = base|layer<<2|f16(roughness)<<16`,
`data2/3/4` = the 6 half-floats of `colorBase` + `colorLayered`. The runtime **material buffer**
is `typesRenderGpu : DynamicStructuredBuffer<Uint4>` — voxel 15-bit type pointers index into it.
`ApplyVoxelType` interns by string ID; `MapTypes16bits`/`MapTypesWithState` remap raw import
type IDs → render indices via `typeMapping.fx`. Element 0 is a reserved empty placeholder.

> **C# vs. paper divergence:** the paper says "16 bits per material entry"; the C# material
> entry is **128 bits** (`Uint4`) — base+layer material enums, f16 roughness, and *two* RGB
> half-float colors (`colorBase` + `colorLayered`, the latter doubling as emissive intensity
> for `Emissive` and a second tint for layered metals). The shader-side decompress is
> `decompressVoxelType` in `commonRenderPipeline.fxh` (`VoxelType { materialBase, materialLayer,
> colorBase, colorLayer, roughness }`, `SURFACE_DIFFUSE/EMISSIVE/SPECULAR_ROUGH/SPECULAR_MIRROR`
> = 0/1/2/3). **The port should follow the C# 128-bit entry, not the paper's 16-bit summary.**

> **→ Bevy:** a `Resource` with the `Vec<VoxelType>` + a `GrowableBuffer<UVec4>`; or `Assets<T>`.
> Material *typing* is Phase A (geometry needs it); *emissive contribution* and the
> metal/mirror BRDF use is Phase B.

## 4.7 `EditingHandler` + `EditingTools/` — **Phase A data model, editing behaviour deferred**

`EditingHandler` (`World/Data/EditingHandler.cs`, ~249 lines): `editData` flat buffer,
`editChunkDataPointer[chunkCount]`, `getChunkDataToEdit` (lazily decompresses a chunk via
`FillChunkData`), `getVoxelData`/`setVoxelData` (addresses the packed 2-per-uint layout),
`processChunks` (re-hashes edited chunks in parallel, dedups blocks, frees old voxels, fills
the `changedX` arrays). `EditingTools/`: `EditingTool` base + `Cube`, `Sphere`, `Paint`,
`FloodFill`, `Model` — each `ApplyAnyInput` raycasts via `WorldData.RayTraversal`, computes
affected chunks, writes voxel data. **`EditingToolFloodFill` is a *separate* flood-fill** (BFS
over voxels to replace a connected region of one type) — not the AADF-invalidation flood-fill.

> **→ Bevy:** the data-model parts (`processChunks`, the edit buffer) are Phase A; the
> interactive tools are an editor concern — per the brief's "core engine, no editor" they are
> low priority. The design phase may keep one programmatic edit path for testing.

## 4.8 `EntityData` / `EntityHandler` (`World/Data/`) — **Phase A data model, behaviour deferred**

See §1.1.7. `EntityInstance { Vector3 position; Vector4 quaternion; uint voxelStart, entity;
Point3 size }`. `EntityChunkInstanceGpu { Uint4 data; uint data2 }` (the 5-uint per-chunk-entity
record). `EntityData` builds a per-entity AADF voxel volume on the CPU (`addBounds` 31
iterations). `EntityHandler.Update` (~280 lines): two-pass overlap counting + prefix-sum into
`chunkEntityData`, `EntityChunkInstanceHash` dedup, quaternion compression
(`compressQuaternion` — smallest-three encoding), uploads via `entityUpdate.fx`. Gated behind
`BuildFlags.Entities`.

> **→ Bevy:** keep behind a feature flag; the data model is Phase A but recommend deferring it
> as a Phase-A sub-feature (the brief defers editing/entity behaviour).

## 4.9 `World/Generator/` — **Phase A**

`WorldGenerator` — abstract base (`IsValid`, `CopyToChunkData`). `WorldGeneratorModel` — the
only concrete generator: copies a `ModelData` voxel model into the world's chunk data via
`generatorModel.fx` `CopyData16` (`[numthreads(4,4,4)]`, one group per chunk, reads the model's
chunk/block/voxel buffers, writes packed 16-bit voxel pairs into `chunkData`/`segmentVoxelBuffer`).
Supports tiling the model across the world (`modelIndexY` clamps to one tile vertically).

> **→ Bevy:** a compute pass; the abstract base is trivial — a Rust trait or just one
> generator. Procedural generators (noise terrain) would be new generators in this slot.

## 4.10 `World/Model/` — **Phase A (placement) / mostly out of scope (file I/O)**

`ModelData` (`World/Model/ModelData.cs`, ~849 lines) — a *baked* voxel model: its own
`dataChunk/Block/Voxel` (`uint[]` + `StructuredBuffer`), `VoxelType[] types`, sizes.
`CreateDataForRender` remaps type IDs → render indices. **`Load`/`Save`** = the `.cvox` ZIP
format (**OUT OF SCOPE** per Q1). **`ImportFromVox`/`ImportFromVL32`/`ImportFromMesh`** = the
MagicaVoxel/Voxlap/obj2voxel importers (**OUT OF SCOPE** per Q1) — they each contain a *second*
copy of the hashing-construction logic (CPU). `MapColorsToPaletteIndices` uses Accord K-means
(out of scope). `ModelHandler` is a trivial `List<ModelData>`.

> **→ Bevy:** the *runtime* `ModelData` (chunk/block/voxel buffers a generator consumes) is
> Phase A and small. The loaders/importers are explicitly out of scope — the port needs **one**
> way to get a model in (the design phase decides: a minimal `.vox` reader, or a procedural
> generator, or a hard-coded test model). Do not port `.cvox`, `obj2voxel`, K-means.

## 4.11 `World/Render/WorldRender.cs` + `Versions/` — render orchestration

`WorldRender` — abstract base: owns the static `Camera`, the `Cube` mesh (drawn to run the
final fullscreen PS), `taaIndex = 128-(frameCount%128)-1`, `randValues[32]`, Halton `getJitter`,
sun-angle state. `ApplyRenderVersion` switches between the three versions.

- **`WorldRenderAlbedo`** (~157 lines) — **Phase A render path.** 3 effects:
  `albedo/renderFirstHit`, `albedo/renderTaaSampleReverse`, `albedo/renderFinal`. Buffers:
  `firstHitData` (`Uint4`/pixel), `taaSamples` (`Uint2`, ×32), `taaSampleAccum` (`Uint2`).
  `RenderInternal`: shoot primary rays (first-hit, flat sun+ambient lighting, no GI bounce) →
  optionally reproject 32-frame TAA → `renderFinal` fullscreen tonemap. **This is the Phase A
  target.**
- **`WorldRenderBase`** (~487 lines) — **Phase B.** 10 effects (firstHit, rayQueueCalc,
  globalIllum, sampleRefine, spatialResampling, atmosphere, denoiseSplit, taaSampleReverse,
  renderFinal). The full GI pipeline — see §1.2. ~25 distinct compute dispatches per frame.
  Buffers as listed in §1.2.3.
- **`WorldRenderPathTracer`** (~190 lines) — **Phase B (reference).** A brute-force accumulating
  path tracer (atmosphere → firstHit → globalIllum accumulate into `accumulatedSamples` →
  renderFinal). No TAA/ReSTIR/denoise. The paper's "reference 8192 spp". The renderer the
  scaffold's `--pathtracer` flag conceptually maps to.

> **→ Bevy:** each version becomes a set of **custom render-graph nodes**. Per Q2 these are
> faithful WGSL ports of the HLSL — Solari is **not** used. `WorldRenderAlbedo` is the Phase A
> deliverable; `Base`/`PathTracer` are Phase B. The `Cube`+fullscreen-PS pattern becomes a
> Bevy fullscreen pass.

## 4.12 `World/Render/Atmosphere.cs` — **Phase A / B**

See §1.2.5. CPU sky model for sun color; GPU equivalents in the atmosphere shader headers.

---

# 5. Shader inventory

All shaders are **HLSL `.fx`/`.fxh`** (MonoGame/DX11, `cs_5_0` / `vs_5_0` / `ps_5_0`). Per Q2
they are faithfully ported to **WGSL** as custom Bevy render-graph nodes. `.fxh` = include
headers (compile into the `.fx` that includes them — phase follows the includer).

## 5.1 Common headers (`Content/shaders/render/common/**`, `settings.fxh`)

| file | what it provides | phase |
|---|---|---|
| `settings.fxh` | build flags (`ENTITIES`, `HDR`), `CHUNKTYPE` = `uint`/`uint2` | A (the `ENTITIES` widening) |
| `common/common.fxh` | umbrella include; `FLATTEN_INDEX` macro | A |
| `common/commonConstants.fxh` | `PI` | A |
| `common/commonRayTracing.fxh` | PCG/xoroshiro RNG, hemisphere/VNDF-GGX sampling, `geometryTerm`, octahedral normal encode/decode, quaternion (de)compress (smallest-three) | A (RNG/oct/quat used everywhere) / B (VNDF/GGX used by GI) — **splits** |
| `common/commonRenderPipeline.fxh` | `HIT_*`/`SURFACE_*`/`ENTITY_FREE` consts, `VoxelType`+`decompressVoxelType`, `FirstHitResult`, `NORMAL[8]`/`SPECULAR_MIRROR_FAC[7]` LUTs, `getRayDir`, Fresnel, quaternion ops, **`getHitDataFromPlanes`** (the G-buffer plane→virtual-path reconstruction) | A (`getRayDir`, `decompressVoxelType`, basic `getHitDataFromPlanes`) / B (specular-path reconstruction) — **splits** |
| `common/commonColorCompression.fxh` | `COLORS[32]` exponential LUT, `COLOR_DIF_PROB[31]`, `compressColor` (5-bit/channel exponential), `refineCompColor` | **B** (GI sample color compression) |
| `common/commonOther.fxh` | groupshared counter helpers, `gaussianF`, `gcd`/`findCoprime`, `nextPow2` | A (`addToCounter*`) / B (`gaussianF`, coprime — denoise/refine) — **splits** |
| `common/commonEntities.fxh` | `EntityInstance`/`EntityChunkInstance` structs + (de)compress | A (entity data model, behind flag) |
| `common/taa/commonTaa.fxh` | `neighborOffsets[9]`, `getHashFromData`, **`compressSample`/`decompressSample`** (the 64-bit TAA sample) | **B** primarily; the *albedo* TAA also uses it → **A for the albedo subset** |
| `render/rayTracing.fxh` | **`shootRay` — the AADF DDA traversal** (chunk→block→voxel descent + AADF skip + entity sub-traversal), `rayAABB`, `RayResult`, all the voxel buffer declarations, `MAX_RAY_STEPS_*` | **A** (this is the §3.4 core; the entity branch is the entity sub-feature) |
| `render/common/atmosphere/atmosphereRaw.fxh` | `addLightForDirection` ray-marched sky, Rayleigh/Mie phase, `densityAtHeight`, `raySphere` | A (used by albedo? — no; albedo inlines simple sun) → **B** in practice (Base/PathTracer precompute) |
| `render/common/atmosphere/atmospherePrecomputed.fxh` | `applyAtmosphere` — sample precomputed octahedral `atmosphereComp` | **B** |

## 5.2 World data/generation shaders (`Content/shaders/world/**`)

| file (passes) | what it does | inputs → outputs | phase | splits? |
|---|---|---|---|---|
| `world/data/chunkCalc.fx` (`VoxelHash`, `ComputeVoxelBounds`, `ComputeBlockBounds`, `ChunkCopyToCpu`) | **Algorithm 1** (`VoxelHash` = build chunk/block/voxel from raw voxels + hash dedup); `ComputeVoxelBounds`/`ComputeBlockBounds` = local 4³ AADF; `ChunkCopyToCpu` = GPU→CPU sync | `segmentVoxelBuffer` + `hashMap` → `chunks`/`blocks`/`voxels` + `blockVoxelCount` | **A** | no |
| `world/data/boundsCommon.fxh` | `ComputeBounds4` (block/voxel AADF), `checkMatchingBounds`, `MASK_*` | — | **A** | no |
| `world/data/boundsCalc.fx` (`AddInitialGroupsToBoundQueue`, `PrepareGroupBounds`, `ComputeGroupBounds`) | **background chunk-AADF** queue system (§1.1.4) — per-bound-size queues, indirect dispatch | `chunks` + queue buffers → `chunks` (updated AADFs) + queues | **A** | no |
| `world/data/worldChange.fx` (`ApplyGroupChange`, `ApplyChunkChange`, `ApplyBlockChange`, `ApplyVoxelChange`) | apply CPU edits to GPU + reset/recompute affected AADFs (§1.1.6) | `changedGroups/Chunks/Blocks/Voxels` dynamic buffers → `chunks`/`blocks`/`voxels` + bound queues | **A** | no |
| `world/data/entityUpdate.fx` (`UpdateChunks`, `CopyEntityChunkInstances`, `CopyEntityHistory`) | sync entity instances + per-chunk entity pointers + history to GPU | `chunkUpdatesDynamic` etc. → `chunks` `.y`, `entityChunkInstances`, `entityInstancesHistory` | **A** (entity sub-feature) | no |
| `world/data/mapCopy.fx` (`CopyMap`, `TestHash`) | re-hash the block hash map into a larger table on grow | `oldMap` → `newMap` | **A** | no |
| `world/data/boundsCommon.fxh` | (header, see above) | — | A | — |
| `world/generator/generatorModel.fx` (`CopyData16`) | copy a `ModelData` voxel model into world chunk data | `modelDataChunk/Block/Voxel` → `chunkData` | **A** | no |
| `world/model/typeMapping.fx` (`MapTypes16`, `MapTypesState`) | remap raw import type IDs → render indices in a voxel buffer | `voxelData` + `mapping[300]` → `voxelData` | **A** | no |
| `dataCopy.fx` (`CopyData`) | chunked large GPU buffer copy (DX11 size-limit workaround) | `srcData` → `dstData` | A | no |

## 5.3 Render shaders — albedo version (`render/versions/albedo/**`) — **all Phase A**

| file (passes) | what it does | inputs → outputs | phase | splits? |
|---|---|---|---|---|
| `albedo/renderFirstHit.fx` (`FirstHit`) | primary-ray DDA first hit; albedo + emissive + simple sun + ambient; writes G-buffer + a TAA sample | voxel buffers, cam → `firstHitData`, `taaSamples`, `taaSampleAccum` | **A** | no |
| `albedo/renderTaaSampleReverse.fx` (`ReprojectOld`) | reproject up to 32 past TAA samples, depth/hash/screen-pos reject, accumulate | `firstHitData`, `taaSamples`, cam history → `taaSampleAccum` | **A** | no |
| `albedo/renderFinal.fx` (`P0` VS+PS) | fullscreen tonemap of `taaSampleAccum` | `taaSampleAccum` → backbuffer | **A** | no |

## 5.4 Render shaders — base (GI) version (`render/versions/base/**`) — **all Phase B**

| file (passes) | what it does | inputs → outputs | phase | splits? |
|---|---|---|---|---|
| `base/renderFirstHit.fx` (`FirstHit`) | primary ray with up to 4 specular bounces; writes the 4-plane G-buffer, `firstHitAbsorption`, `finalColor`; applies atmosphere | voxel buffers, `atmosphereComp` → `firstHitData`, `firstHitAbsorption`, `finalColor` | **B** | **mid-file vs. albedo:** this *is* the Phase-B first-hit; the *concept* (first hit) is shared with Phase A but this file is the B variant |
| `base/rayQueueCalc.fx` (`RayQueue`, `RayQueueStore`) | decide which pixels get secondary rays this frame (uses TAA accum count); build the indirect-dispatch ray queue | `firstHitData`, `taaSampleAccum` → `pixelsToRender`, `groupCount` | **B** | no |
| `base/renderAtmosphere.fx` (`Atmosphere`) | precompute the octahedral atmosphere scatter/absorption texture (¼ per frame) | sky params → `atmosphereComp` | **B** | no |
| `base/renderGlobalIllum.fx` (`GlobalIlum`) | trace ≤3-bounce secondary rays for queued pixels; sun sampling; compress into lit (`SampleValid`/`Uint8`) and invalid (`Uint4`) sample lists | voxel buffers, `firstHitData`, `pixelsToRender` → `globalIlumValidSamples`, `globalIlumInvalidSamples`, `globalIlumSampleCounts` | **B** | no |
| `base/renderSampleRefine.fx` (`ClearBucketsAndCalcMask`, `ValidHistory`, `CountValidAndRefine`, `CountInvalid`, `RefineBuckets`) | temporal resampling: project lit/unlit samples into 8×8 regions, count, brightness-level (`COLOR_DIF_PROB` removal), produce ≤8 refined samples per region | sample lists, `firstHitData`, `taaDistMinMax` → `globalIlumValidSamplesRefined`, `globalIlumValidSamplesCompressed`, `globalIlumBucketInfo` | **B** | no |
| `base/renderSpatialResampling.fx` (`SpatialResampling`) | **Algorithm 2** — 12-iteration spatial ReSTIR over neighbouring 8×8 regions, Jacobian, reservoir merge, single visibility ray; sun sampling; writes `denoisePreprocessed` or `finalColor` | `globalIlumValidSamplesCompressed`, `globalIlumBucketInfo`, voxel buffers → `denoisePreprocessed` / `finalColor` | **B** | no |
| `base/renderDenoiseSplit.fx` (`CalcDenoiseHorizontal`, `CalcDenoiseVertical`) | sparse separable bilateral filter (kernel 21, σ=10, ~½ pixels), color+geometry weights | `denoisePreprocessed` → `denoisePreprocessedHorizontal` → `finalColor` | **B** | no |
| `base/renderTaaSampleReverse.fx` (`ReprojectOld`, `CalcNewTaaSample`) | long-term-memory TAA: `ReprojectOld` = 32-frame reproject+reject (also writes `taaDistMinMax`); `CalcNewTaaSample` = fold the denoised GI result into the 32-frame history | `firstHitData`, `taaSamples`, `finalColor`, cam history → `taaSampleAccum`, `taaSamples`, `taaDistMinMax` | **B** | no |
| `base/renderFinal.fx` (`P0` VS+PS) | fullscreen tonemap of `taaSampleAccum` (+ `toneMappingFac`) | `taaSampleAccum` → backbuffer | **B** | no |

## 5.5 Render shaders — pathTracer version (`render/versions/pathTracer/**`) — **all Phase B (reference)**

| file (passes) | what it does | phase |
|---|---|---|
| `pathTracer/renderFirstHit.fx` (`FirstHit`) | primary ray + ≤4 specular bounces (unbounded ray steps = 2000), atmosphere | **B** |
| `pathTracer/renderGlobalIllum.fx` (`GlobalIlum`) | brute-force ≤3-bounce GI, accumulate into `sampleAccumulated` (running average up to `maxSamples`) | **B** |
| `pathTracer/renderAtmosphere.fx` (`Atmosphere`) | same atmosphere precompute as base | **B** |
| `pathTracer/renderFinal.fx` (`P0`) | fullscreen tonemap of `sampleAccumulated` | **B** |

**Shaders that split mid-file between phases:** none of the `.fx` files split — each `.fx` is
cleanly one phase (the `albedo/` tree is A, `base/` + `pathTracer/` are B, `world/` is A). The
**`.fxh` headers split** by *who includes them*: `commonRayTracing.fxh`,
`commonRenderPipeline.fxh`, `commonOther.fxh`, `commonTaa.fxh`, and `rayTracing.fxh` are
included by both A and B `.fx` files — the **first-hit / DDA / RNG / `getRayDir` / basic
plane-reconstruction** parts are exercised in Phase A, the **VNDF-GGX sampling, specular-path
reconstruction, GI color compression, coprime/gaussian** parts only in Phase B. Port the
shared headers in Phase A but expect to *add* the B-only functions when Phase B starts.

---

# 6. Paper vs. C# divergences / C#-only details

1. **Material entry width.** Paper §3.1: "16 bits per [material] entry". C#: the runtime
   material entry (`VoxelType.compressForRender`) is **128 bits** (`Uint4`) — base+layer
   material enums, f16 roughness, two RGB half-float colors. The paper's 16 bits is a
   minimal/illustrative figure. **Follow the C# 128-bit `Uint4` layout** (decompressed by
   `decompressVoxelType` in `commonRenderPipeline.fxh`).
2. **Hash-probe limit & resize threshold.** Paper §3.2: probe up to **100** slots, resize at
   **75 %** occupancy. C#: GPU `chunkCalc.fx` probes **250**; CPU `BlockHashingHandler` uses
   `wantedEmptyRatio = 0.5` (resize at **50 %**). Minor — the design phase can pick either.
3. **Two AADF "state" encodings coexist in the C#.** The 2-bit state field (`>> 30`,
   values 0/1/2) is the paper's state. But the *traversal* shader (`rayTracing.fxh`) tests the
   **top bit** (`>> 31`) as a "has-children" flag and a separate `& 0x40000000` as
   "uniform-full". So a chunk/block payload is read as: bit 31 = has-children, bit 30 =
   uniform-full, bits 0-29 = pointer/type/AADF. This is a valid re-encoding of the 3 states but
   the implementer must replicate it exactly — the paper's "00/01/10" is conceptual.
4. **Voxels are packed two-per-`uint`.** The paper says "voxel = 16 bits"; the C# voxel buffer
   is a `uint[]` with `voxel1 | voxel2<<16`. A "voxel index" → `dataVoxel[index/2]`, then mask
   `>> (16*(index&1))`. Block child pointers point at the *uint* offset (i.e. `voxelPointer*2 +
   voxelIndexInBlock` then `/2`). Easy to get wrong — note for the impl phase.
5. **TAA history depth: 128 vs. 32.** The paper headlines "32 past frames". The C# keeps a
   **128**-deep ring of *camera matrices / positions / jitters* (`taaSampleCamTransform[128]`
   etc., `taaIndex = 128-(frameCount%128)-1`) but only a **32**-deep ring of *samples*
   (`taaSamples` is `screen·32`). The 32 is the sample history; 128 is the camera-history ring
   the reprojection indexes into. The GI sample buffers cover "up to 64 frames" (paper) via the
   ring + the 8:1 unlit compression.
6. **`PositionSplit` int+frac camera is pervasive, not optional.** The paper does not discuss
   it; the C# threads `camPosInt` (`int3`) + `camPosFrac` (`float3`) separately through *every*
   render shader, and the entire G-buffer plane reconstruction (`getHitDataFromPlanes`), TAA
   reprojection, and GI sample reprojection are written in int+frac space. The reuse audit
   calls it "port only if precision demands" — but the shaders assume it. **Open question for
   design (see §7).**
7. **Atmosphere is in-scope-by-necessity but not a paper contribution.** The paper barely
   mentions the sky; the C# has a full `Atmosphere.cs` + two atmosphere shader headers + a
   per-version `renderAtmosphere.fx`. `WorldRenderAlbedo` does *not* precompute atmosphere (it
   inlines a cheap sun+ambient term in `renderFirstHit`); `Base`/`PathTracer` do. So Phase A
   needs only the inline sun term; the full atmosphere model is Phase B.
8. **C#-only orchestration detail: world is sized in "world-gen segments".** Not in the paper.
   `WorldData` is built segment-by-segment (`segmentSizeInGroups·4` chunks per side), and the
   voxel/block buffers grow as segments are processed. The default world is `(4096,512,4096)`
   voxels. The 4³-chunk "queue group" is the unit for both flood-fill invalidation *and*
   background AADF computation.
9. **C#-only: the `Cube` + fullscreen-PS final-blit pattern.** NAADF draws a unit cube whose
   pixel shader runs over the screen to do the final tonemap (`renderFinal.fx` is a VS+PS, not
   a compute shader) — every other render stage is compute. The Bevy port replaces this with a
   fullscreen pass.
10. **`renderSampleRefine.fx` `RefineBuckets` brightness-leveling uses `COLOR_DIF_PROB`** — the
    paper describes "remove weakly lit samples, compensate by removal probability"; the C#
    implements it via the `COLOR_DIF_PROB[31]` exponential-difference probability table in
    `commonColorCompression.fxh`. Implementation detail the paper omits.
11. **SVGF is not in the in-scope source.** The paper mentions providing an SVGF alternative
    denoiser; `Content/shaders/render/**` ships **only** the sparse bilateral (`renderDenoiseSplit.fx`).
    There is no SVGF shader to port.

---

# 7. Open questions for the design phase

1. **`PositionSplit` int+frac camera — port it or not?** The reuse audit says "only if
   precision demands"; but every NAADF render shader is written in `camPosInt`+`camPosFrac`
   space and the G-buffer/TAA/GI reprojection math depends on it. Either port `PositionSplit`
   (and feed shaders int+frac) or rewrite the shader coordinate math for a plain `f32` camera
   and accept reduced large-world range. This is a Phase-A-blocking decision because
   `renderFirstHit` (albedo) already needs it.
2. **`DynamicStructuredBuffer` → wgpu wrapper.** The growable-buffer abstraction is needed in
   Phase A (voxel/block buffers, hash map, `changedX` buffers, `typesRenderGpu`). Design needs
   to spec a `GrowableBuffer` (re-alloc + `copy_buffer_to_buffer` on growth, growth factor 2×).
   Does wgpu's buffer-size limit make `Helper`/`dataCopy.fx`-style chunked copies necessary, or
   can it be ignored?
3. **Chunk buffer as a 3D texture vs. a buffer.** C# uses `Texture3D<uint>` (or `Rg64Uint`)
   for the chunk layer, plain structured buffers for block/voxel. Keep the 3D-texture choice in
   wgpu (`Texture` with `Rg32Uint`/`R32Uint`) or use a buffer? Affects bind-group layout and
   the `ENTITIES` widening.
4. **How does a model get into the world in Phase A?** `.cvox`/`obj2voxel`/MagicaVoxel
   importers are out of scope, but `WorldGeneratorModel` consumes a `ModelData`. Design must
   pick the Phase-A content path: a minimal `.vox` reader, a procedural noise generator, or a
   hard-coded test model. (The reuse audit's `scene.rs` is the throwaway slot this replaces.)
5. **Entities: Phase-A sub-feature or deferred entirely?** The entity data model is Phase A per
   the source split, but it widens the chunk texture to 64-bit, threads through `shootRay`,
   adds `EntityHandler`/`entityUpdate.fx`, and the brief defers editing/entity *behaviour*.
   Recommend design treats entities as an explicitly-deferred Phase-A sub-feature (feature-flag
   it the way the C# `BuildFlags.Entities` does) so Phase A's first deliverable is
   entity-free.
6. **`taaSampleMaxAge` for the albedo path.** `WorldRenderAlbedo` defaults `taaSampleMaxAge = 1`
   (TAA effectively off) and only enables 32-frame TAA when the slider is raised. Is the Phase-A
   deliverable "first-hit albedo, TAA off" (simplest runnable) or "first-hit albedo + the
   32-frame TAA"? The brief says "albedo + normal only, no bounce lighting" — TAA is orthogonal
   to bounce lighting, so the design phase should decide whether the long-term TAA is Phase A
   or Phase B. (It is *implemented* in both `albedo/` and `base/` shader trees, so it is
   genuinely splittable either way.)
7. **Solari strip-vs-dormant** — already flagged in 01-context.md §3 as the design phase's
   call; restated here only so it is not lost: per Q2 Solari is not the GI substrate; design
   decides strip vs. keep-behind-flag.
