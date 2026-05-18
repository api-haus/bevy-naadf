# Aokana vs. NAADF — comparison & ideas for `bevy-naadf`

**Created:** 2026-05-14
**Status:** reference notes — not a committed plan. Captures ideas for a possible future
LOD/streaming phase, layered on top of the NAADF port.

## Sources

- **Aokana** — Fang, Wang, Wang, *Aokana: A GPU-Driven Voxel Rendering Framework for Open
  World Games*, ACM PACMCGIT 2025 (art. 3728299). Extracted research doc:
  `/mnt/archive4/PAPERS/Prepared/fang-2025-aokana-voxel-rendering.md`.
- **NAADF** — Ulschmid 2026, the C#/MonoGame voxel-GI engine being ported here. Digest:
  `docs/orchestrate/naadf-bevy-port/02-research.md` (paper digest + C# cross-check). Not yet
  cross-read against the source paper end-to-end.

## TL;DR

Both are recent shallow-chunked-hierarchy GPU voxel renderers that reject the single deep
SVDAG for cache-locality reasons and use hash-based dedup. But they solve **different halves
of the problem** and are **complementary, not competing**:

- **NAADF** = traversal-throughput + global illumination. Core = pointer-free 3-layer cell
  hierarchy + the **AADF** (per-empty-cell 6-direction empty-cuboid distance field) feeding a
  DDA path tracer. No LOD, no streaming — whole structure resident.
- **Aokana** = visibility + memory at open-world scale. Core = chunked shallow **SVDAG** +
  octree LOD aggregation + distance-based streaming (`LODError`) + Hi-Z occlusion culling.
  Says nothing about lighting — produces first-hit color/depth into a forward pipeline.

## Side-by-side

| | **Aokana** | **NAADF** |
|---|---|---|
| Primary goal | Open-world real-time rendering at scale | Voxel global illumination / path tracing |
| Optimizes for | VRAM footprint + primary-visibility throughput | Ray throughput, esp. secondary/GI rays |
| Core structure | Chunked shallow SVDAG (256³ chunks, 64-bit leaf bitmap, child pointers) | 3-layer flat-buffer cell hierarchy (chunk→block→voxel, 4³ each, no pointer tree) |
| Empty-space skip | DAG skips empty subtrees + screen-space Hi-Z occlusion culling | AADF: 6-direction empty-cuboid distance per empty cell; DDA jumps multi-axis in one step |
| Rendering method | Multi-pass compute → ray-march into 64-bit visibility buffer → Color Resolve | Extended Amanatides-Woo DDA traversal feeding a path tracer |
| LOD | Yes — octree-style aggregation, `density` merge threshold | Not in scope — the 3 layers are for compression, not LOD |
| Streaming | Yes — `LODError` metric, ~5% of scene in VRAM | Not in scope — whole structure resident |
| Construction | Preprocessing; CPU implicit octree drives loading | On-GPU hash construction (open-addressing, Algorithm 1) |
| Headline result | 9× memory ↓, 4.8× faster vs. HashDAG | 7× primary / 10× secondary rays vs. SVDAG (7029 vs. 1074 Mrays/s) |
| Engine | Unity 6 / Vulkan | C#/MonoGame (→ porting to Rust/Bevy) |

## Conceptual split

- **Aokana is a visibility + memory paper.** Empty-space handling is *structural* (DAG) and
  *screen-space* (Hi-Z). Its real win is LOD + streaming so tens of billions of voxels fit on
  a consumer GPU.
- **NAADF is a traversal-throughput + GI paper.** The AADF is a *distance-field augmentation*
  baked into empty cells — spiritually close to SDF sphere-tracing, but axis-aligned cuboids.
  Built to make the secondary rays GI needs cheap.
- Both claim multiplier speedups over SVDAG ray-marching (Aokana 2–4× vs. HashDAG; NAADF
  7×/10× vs. SVDAG) — but on different scenes/metrics, so the numbers are **not directly
  comparable**. Aokana's traversal would be a genuine benchmark competitor to NAADF's.
- Shared lineage: both build on SVDAG (Kämpe 2013) and Amanatides-Woo DDA. NAADF positions
  itself as an SVDAG *replacement* for traversal; Aokana as an SVDAG *refinement* for scale.

## Ideas for `bevy-naadf`

They compose rather than conflict. NAADF stays the core GI substrate. Aokana fills exactly
the gap NAADF's scope leaves open — **LOD and streaming**.

1. **Treat Aokana as the blueprint for a later streaming/LOD phase**, layered on top of the
   AADF-DDA core — not a replacement for it. Relevant if/when `bevy-naadf` needs open-world
   scale beyond what fits in VRAM.
2. **`LODError` maps onto `src/camera/position_split.rs`.** Aokana's per-chunk LOD selection
   metric `LODError = (ChunkSize × StreamingFactor) − ‖ChunkCenterPos − CameraPos‖` is a
   camera-relative distance test — the position-split camera is the natural home for the
   distance term.
3. **Octree-style LOD aggregation** (8 children → 1 parent at same resolution, create a voxel
   when ≥ `density` children non-empty, averaged color) is data-structure-agnostic enough to
   adapt to NAADF's 4³ cell hierarchy — adapt the *idea*, not Aokana's SVDAG layout.
4. **Key tension to respect:** Aokana assumes an SVDAG *with child pointers*; NAADF
   deliberately uses *pointer-free flat buffers*. Port Aokana's LOD/streaming **ideas**, keep
   NAADF's data structure. Don't import the chunked-SVDAG wholesale.
5. **Hi-Z occlusion culling** is orthogonal to both data structures and could benefit the
   NAADF primary-visibility pass independently of any streaming work.

## Open questions

- Does the NAADF paper discuss LOD/streaming at all, or is it strictly out of scope? (Digest
  suggests out of scope; confirm against the source paper before committing to idea 1.)
- AADF vs. Aokana's structural empty-space skip — would an AADF-augmented chunk make Aokana's
  Hi-Z pass redundant, or are they additive? Worth a thought experiment before layering
  LOD/streaming on top.
