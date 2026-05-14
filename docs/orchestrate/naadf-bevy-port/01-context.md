# 01 — Canonical Context Bundle

**Every agent in this orchestration reads this file first, in full, before doing anything else.**
It is self-contained: all paths are absolute or repo-root-relative, no conversation-relative
references.

---

## 1. Goal (verbatim user request)

> "porting `/mnt/archive4/DEV/NAADF` to bevy" — informed by the research document
> `docs/research/ulschmid-2026-naadf-voxel-gi.md`

NAADF = **Nested Axis-Aligned Distance Fields** (Ulschmid et al., CGF 2026): a C#/MonoGame
voxel engine whose two contributions are (1) an efficient voxel ray-marching data structure
and (2) a real-time global-illumination pipeline built on top of it.

**Target repo:** `/mnt/archive4/DEV/bevy-naadf` — a Rust/Bevy 0.19-rc.1 codebase that currently
holds only a toolchain proof-of-concept (Bevy + Solari + DLSS scaffold around a placeholder
Cornell-box scene). No voxel code exists yet.

**Source repo:** `/mnt/archive4/DEV/NAADF/NAADF/` — the C#/MonoGame engine to port from.

---

## 2. User decisions (Architectural Q&A, 2026-05-14)

These four decisions are binding. Cite them, do not relitigate them.

| # | Question | User's choice | Consequence |
|---|---|---|---|
| Q1 | How much of NAADF is in scope? | **Core engine, no editor** | IN: `Libraries/VoxelsCore`, the AADF cell structure, `World/Data`, `World/Generator`, `World/Render`. OUT: `Gui/` editor panels, `.cvox` persistence, `Settings.cs`/`IO.cs`, the `obj2voxel`/file-format importers. Target result: a runnable voxel-GI scene, no editor. |
| Q2 | Renderer strategy? | **Faithfully port the HLSL pipeline** | NAADF's `Content/shaders/render/**` is ported to **WGSL** as custom Bevy render-graph nodes. **Bevy Solari is reference-only / unused for the GI pipeline.** This deliberately overrides the reuse audit's "lean on Solari" recommendation. |
| Q3 | Source of truth for the AADF data structure? | **Re-derive from the paper** | Primary source = `docs/research/ulschmid-2026-naadf-voxel-gi.md`. Cross-check correctness details against `Libraries/VoxelsCore/*.cs`. Produce idiomatic Rust, not a line-by-line C# transliteration. |
| Q4 | Where does new Rust code live? | **Single crate, modules** | One binary crate. New modules under `src/` (e.g. `src/voxel/`, `src/aadf/`, `src/world/`, `src/render/`). **No Cargo workspace.** |

### Phasing decision (refined in chat 2026-05-14 — restructured to FOUR gated phases)

NAADF's contributions are separable, and the port is split into **four sequential, gated
phases**. The split maps onto NAADF's own renderer version-split (`World/Render/Versions/
WorldRender{Albedo,Base,PathTracer}`) plus its construction/runtime split, so the seams are
natural. **A phase's `design`/`impl` does not begin until the prior phase is reviewed and
confirmed runnable.**

- **Phase A — NAADF substrate + albedo (do this first).** Port the three-layer cell hierarchy
  (chunk / block / voxel, 4³ each), CPU-side AADF construction, and DDA traversal with AADF
  empty-space skipping. Render path = primary-ray first-hit producing albedo + normal only,
  **no bounce lighting, no TAA** — maps to NAADF's `WorldRenderAlbedo` version. Source surface:
  research-doc **Section 3** + `Libraries/VoxelsCore` + `World/Data` (data structures only).
  Deliverable: a voxel scene the user can fly through with correct geometry, flat-lit. Fully
  designed in implementable detail in `03-design.md`.
- **Phase A-2 — long-term-memory TAA.** Port research-doc **§4.1**: the 32-frame / 64-bit
  long-term TAA — `renderTaaSampleReverse` reprojection, the 32-deep `taaSamples` ring, the
  128-deep camera-history ring, accumulation. The TAA node slots between first-hit and the
  final blit; it replaces Phase A's `shaded_color` blit-source stand-in (`03-design.md` §5.3)
  with the real `taaSampleAccum` buffer. Built on Phase A. Sequenced **before** Phase B.
- **Phase B — GI pipeline (minus TAA).** Port research-doc **§4.2–4.3**: compressed ReSTIR GI
  (lit/unlit separation, 8×8 screen-space regions, 12-iteration spatial pass), the sparse
  bilateral denoiser, the 4-plane-bounce first-hit, atmosphere precompute. Maps to
  `WorldRender{Base,PathTracer}` + the bulk of `Content/shaders/render/**`. Built on Phase A-2.
- **Phase C — GPU world construction & editing.** Port the GPU hashing construction
  (`chunkCalc.fx` = paper Algorithm 1), the background chunk-AADF queue (`boundsCalc.fx` /
  `WorldBoundHandler`), and flood-fill edit invalidation (`ChangeHandler` / `worldChange.fx`).
  This is a **speed-up / scalability + editability track, NOT a rendering foundation** — the
  CPU construction path in Phase A produces bit-identical buffers and the traversal shader is
  agnostic to who built them. Needed only for large GPU-generated or editable worlds. Last.

**The `research` phase already mapped the whole paper + whole in-scope C# tree in one pass**,
tagging subsystems / shaders / data types Phase A vs. Phase B. That tagging **predates the
4-phase restructure** — read `02-research.md`'s "Phase B" tags as "Phase A-2 + Phase B", and
its construction shaders (`chunkCalc.fx`, `boundsCalc.fx`, `worldChange.fx`, `mapCopy.fx`) as
**Phase C**. `design` and `impl` proceed **one gated phase at a time**.

---

## 2b. Design-phase Q&A decisions (2026-05-14)

After the `research` phase surfaced 7 open questions, a narrow second Q&A resolved the four
that change the design brief. These are binding alongside Q1–Q4.

| # | Question | User's choice | Consequence |
|---|---|---|---|
| D1 | Camera precision — `PositionSplit` is pervasive; every render shader uses `camPosInt`+`camPosFrac` | **Port `PositionSplit` faithfully** | Implement NAADF's int+frac camera (`pos_int: IVec3` + `pos_frac: Vec3`) and thread both through every WGSL render pass; G-buffer plane reconstruction / TAA / GI reprojection all in int+frac space. User note: *"port with its own camera-relative rendering, then explore this problemspace later"* — faithful NAADF camera-relative rendering now; alternatives (origin rebasing, plain f32) explicitly deferred. **Deferred-exploration reference:** `big_space` (https://github.com/aevyrie/big_space) solves large-world precision at the entity-transform level — a candidate if/when volume-renders are added later. Large-world precision is its own future problemspace; the shader-level mitigation (the ported `PositionSplit`) is kept regardless, since stripping it risks more issues than it solves for the demo-app port. |
| D2 | Phase-A content path — importers are out of scope (Q1) but Phase A needs voxels on screen | **Hard-coded test grid** | Phase A builds a voxel grid procedurally in Rust (primitives / simple shapes). NO `.vox` reader, NO `WorldGenerator` port in Phase A — the generator is deferred. Smallest content path. |
| D3 | Bevy Solari — currently wired into the scaffold | **Strip entirely** | Remove `bevy_solari` from `Cargo.toml`; delete `SolariPlugins` from `src/main.rs`; delete the Solari camera components (`SolariLighting`/`Pathtracer`, `CameraMainTextureUsages` STORAGE_BINDING, `Msaa::Off` if Solari-only) from `src/camera.rs`. No reference renderer kept. **This resolves the "open tension" noted in §3 below — strip, do not keep dormant.** |
| D4 | Long-term 32-frame TAA — Phase A or Phase B? | **Its own gated phase: Phase A-2 (between A and B)** | Originally answered "Phase B, don't pre-design"; then refined — TAA is pulled out into its own gated phase **Phase A-2**, sequenced **after Phase A, before Phase B**. Phase A itself ships with **no TAA** and keeps the design's `shaded_color` blit-source stand-in (`03-design.md` §5.3); Phase A-2 swaps that stand-in for the real `taaSampleAccum` + accumulation. Phase B is then the GI pipeline *minus* TAA. |
| D5 | Is GPU world construction a rendering foundation or a speed-up? | **Speed-up → its own gated phase: Phase C (last)** | GPU Algorithm 1 (`chunkCalc.fx`), the background AADF queue (`boundsCalc.fx`), and flood-fill invalidation (`worldChange.fx`) are a scalability/editability track, **not** required for rendering — the CPU construction path (`03-design.md` §6) produces bit-identical buffers and the traversal shader is producer-agnostic. Postponed to **Phase C**, after Phase B. |

**Net effect — Phase A is the smallest runnable slice:** `PositionSplit` camera + hard-coded
voxel test grid + AADF data structure + CPU-side AADF construction + DDA-with-AADF traversal +
albedo first-hit WGSL render. No Solari, no TAA, no world generator, no file I/O, no GPU
construction. Phase order: **A → A-2 (TAA) → B (GI) → C (GPU construction/editing)**.

---

## 2c. Phase A-2 (TAA) working context

Phase A is complete and has passed its review gate (two regressions found + fixed +
user-confirmed — see `05-review.md` and the README checklist). **Phase A-2 ports NAADF's
long-term-memory TAA.** This section is the canonical Phase-A-2 context — the A-2 `design`
agent reads it first.

### Binding decision (from `design-exploration-qa.md` §6, 2026-05-14)

> **TAA stays NAADF's own long-term-memory TAA** — not Bevy Solari, not a DLSS replacement.
> The VRAM lever is the **sample-count knob NAADF already exposes: run a 16-sample history**
> (~501 MB @1440p) instead of 32-sample (~973 MB) — a ~470 MB saving, pipeline fully intact,
> modest quality cost. **The `taaSamples` ring is 16-deep, not 32-deep.**

- The **camera-history ring stays at NAADF's depth** (128-deep ring of camera matrices /
  positions / jitters — `02-research.md` divergence #5). The §6 lever is specifically the
  *sample* ring (32→16); the camera-matrix ring is tiny in VRAM — leave it as NAADF has it.
- **DLSS / DLSS-RR is UNDER REVIEW, not decided** — only as a possible upscaler/denoiser
  *pairing over* NAADF (would need G-buffer extensions NAADF doesn't materialise), explicitly
  NOT a replacement renderer and NOT touching the 16-sample decision. Phase A-2 does **not**
  depend on DLSS; the `dlss` / `force_disable_dlss` Cargo plumbing stays dormant. DLSS
  evaluation is a separate later thread — do not design for it in A-2.

### 0.25 spp — the GI sampling target (user directive, 2026-05-14)

NAADF's headline 2× GI speedup comes from running GI at an **adaptive ~0.25 spp**, and that
adaptive rate is *driven by the TAA's per-pixel accumulated sample count*
(`design-exploration-qa.md` §6). The GI sampler itself is **Phase B** — 0.25 spp is not
exercised in A-2 — but it is a **binding constraint on the Phase-A-2 TAA design**: the ported
TAA MUST preserve and expose the per-pixel accumulated **sample-count** signal, not just the
accumulated colour. If A-2 strips the sample-count tracking, Phase B forfeits the adaptive
0.25-spp sampling and the 2× speedup. **Record 0.25 spp as the GI sampling target; design the
A-2 TAA to feed it.**

### Phase A-2 scope (research-doc §4.1 + the Phase-A seam)

Port research-doc **§4.1** — the long-term-memory TAA (paper lines ~238–265 in
`docs/research/ulschmid-2026-naadf-voxel-gi.md`; full porting detail in `02-research.md`
§1.2.2, pipeline placement in §1.2.1):
- The **16-deep** `taaSamples` ring (64-bit/sample) + the **128-deep** camera-history ring.
- `renderTaaSampleReverse` reprojection + the accumulation logic + colour-compression
  (`commonColorCompression.fxh` / `commonTaa.fxh`). Port the **albedo** version's TAA
  (`Content/shaders/render/versions/albedo/renderTaaSampleReverse.fx`) — Phase A is the albedo
  path.
- A new TAA render-graph node slotting **between** `naadf_first_hit` and `naadf_final_blit`
  (NAADF places TAA unusually early — `02-research.md` §1.2.1).
- **Replace Phase A's `shaded_color` blit-source stand-in** (`03-design.md` §5.3) with the real
  `taaSampleAccum` buffer: `naadf_first_hit` writes TAA samples, the TAA node accumulates,
  `naadf_final_blit` reads `taaSampleAccum`. The Phase-A blit was deliberately built to the
  `taaSampleAccum` element format, so this is a designed-in drop-in swap.

### Phase A-2 must also fix (carried from `05-review.md` §4 secondary issues)

- **`frame_count` / `rand_counter` misuse** (`src/render/prepare.rs`, ~lines 268-269):
  currently set from `time.elapsed()` (millis / `elapsed_secs*1000`). NAADF's `frameCount` is an
  integer frame *counter* and `randCounter` indexes `randValues[]`
  (`WorldRenderAlbedo.cs:94-95`). TAA reprojection needs a **real monotonic frame counter** — A-2
  fixes this properly.
- `05-review.md` §4 also flags a pre-existing `prepare_world_gpu`-runs-every-frame inefficiency
  and the zeroed `GpuRenderParams.bbox` fields — those are **NOT** Phase-A-2 scope unless they
  actively block TAA; leave them for a later cleanup pass.

### Phase A-2 deliverable / done-bar

Phase A's albedo first-hit render, now with the ported **16-frame** long-term TAA: temporally
stable (jitter-AA'd, no per-frame shimmer), the real `taaSampleAccum` in place, and the
per-pixel sample-count signal exposed and ready for Phase B's adaptive 0.25-spp sampler. Build +
the existing test suite green; user interactive re-test confirms temporal stability (the A-2
review gate).

---

## 2d. Phase B (GI) working context

Phase A and Phase A-2 are complete and review-gated (the albedo first-hit render + NAADF's
16-frame long-term-memory TAA with the per-pixel sample-count signal exposed). **Phase B ports
NAADF's real-time raytraced GI pipeline.** This section is the canonical Phase-B context — the
B `design` agent reads it first. **Phase B is being done in a git worktree:**
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`, branch `feat/phase-b-gi`, branched
from `main` at the Phase-A-2-close commit. All Phase-B file operations use absolute paths under
that worktree.

### Binding scope decision (user, 2026-05-14)

> **Implement only the raytraced GI + TAA, fully — NAADF's `WorldRenderBase` version.**
> - **Reference pathtracer (`WorldRenderPathTracer` / `pathTracer/**` shaders) — OUT of Phase B
>   scope** (future work — do not port).
> - **DLSS / DLSS-RR pairing — OUT of Phase B scope** (future work; the `dlss` /
>   `force_disable_dlss` Cargo plumbing stays dormant — do not design for it).
>
> Phase B is the complete NAADF real-time GI pipeline and nothing else.

### Phase B scope — research-doc §4.2–4.3 + NAADF's `WorldRenderBase`

Port the full real-time GI pipeline (research-doc §4.2–4.3; digest in `02-research.md` §1.2.1
pipeline overview / §1.2.3 compressed ReSTIR GI / §1.2.4 sparse bilateral denoiser / §1.2.5
atmosphere; the `base/` shader inventory in `02-research.md` §5.4; maps to
`Content/shaders/render/versions/base/**` + the Phase-B functions of the shared `.fxh` headers):

- **4-plane-bounce first-hit** — the Phase-B variant of the first-hit pass
  (`base/renderFirstHit.fx`). Phase A/A-2's `naadf_first_hit.wgsl` fills only G-buffer plane 0;
  Phase B fills planes 1–3 (specular bounces) — needs the specular-path `getHitDataFromPlanes`
  (a Phase-B function of `commonRenderPipeline.fxh`) and VNDF/GGX sampling (a Phase-B function
  of `commonRayTracing.fxh`).
- **`rayQueueCalc`** (`base/rayQueueCalc.fx`) — the adaptive sampler. **This is where 0.25 spp
  is realised.** It reads the TAA per-pixel accumulated sample-count signal (Phase A-2 exposed
  it in `taa_sample_accum.x`) to decide which pixels need GI rays this frame → the adaptive
  ~0.25-spp rate, NAADF's headline 2× GI speedup. The "0.25 spp" the user flagged is *this
  pass*, now actually exercised.
- **Compressed ReSTIR GI** (`base/renderGlobalIllum.fx`) — lit/unlit sample separation, the
  5-bit/channel colour compression (`commonColorCompression.fxh` — Phase-B-tagged in
  `02-research.md` §5.1, in scope now), 8×8 screen-space regions, the 12-iteration spatial-
  resampling pass with a *single* visibility check.
- **`renderSampleRefine`** (`base/renderSampleRefine.fx`) — the `RefineBuckets`
  brightness-leveling; uses the `COLOR_DIF_PROB` exponential-difference probability table
  (`02-research.md` divergence #10).
- **`renderSpatialResampling`** (`base/renderSpatialResampling.fx`) — the spatial resampling pass.
- **Sparse bilateral denoiser** (`base/renderDenoiseSplit.fx`) — research-doc §4.3.
- **Atmosphere precompute** — Phase A/A-2 used only the inline sun+ambient term; Phase B needs
  the full `Atmosphere` model + `base/renderAtmosphere.fx` + the atmosphere `.fxh` headers
  (`02-research.md` divergence #7).
- **The Phase-B `renderFinal`** (`base/renderFinal.fx`) — the Phase-B variant of the final blit.

### Pipeline shape (from `02-research.md` §1.2.1)

The Phase-A-2 render graph is `naadf_first_hit → naadf_taa_reproject → naadf_final_blit`. Phase
B expands it — the existing TAA node stays where it is (NAADF places TAA early, between
first-hit and the GI passes):
```
[atmosphere precompute] [first_hit (4-plane G-buffer)] → [TAA reproject] → [rayQueueCalc]
   → [globalIllum] → [sampleRefine] → [spatialResampling] → [denoiseSplit] → [final blit]
```

### What Phase B builds on (Phase A + A-2 — already in the tree)

- The **AADF traversal `shoot_ray`** (`src/assets/shaders/ray_tracing.wgsl`) — reused unchanged
  for GI secondary rays + visibility rays.
- The **16-frame long-term TAA** + the **`taa_sample_accum` per-pixel sample-count signal**
  (Phase A-2 — `06-design-a2.md`, verified in `08-review-a2.md`). `rayQueueCalc` consumes the
  sample-count signal.
- The **`PositionSplit` int+frac camera** (D1) + the **`M*v` glam matrix convention** (the
  Phase-A perspective fix, `05-review.md`) — every new WGSL projection multiply in Phase B MUST
  use `M*v` + the `w`-divide, NOT verbatim HLSL `mul(v,M)`. This bug class has bitten the port
  twice; do not reintroduce it.
- The render-world plumbing (`extract`/`prepare`/`pipelines`/`graph`/`gpu_types`), the
  `GrowableBuffer`, the `TaaGpu` / `WorldGpu` / `FrameGpu` resources.

### Forbidden / out of Phase B scope

- The reference pathtracer (`WorldRenderPathTracer`, `pathTracer/**` shaders) — future work.
- DLSS / DLSS-RR — future work; do not design for it, Cargo plumbing stays dormant.
- Phase C (GPU world construction — `chunkCalc.fx` / `boundsCalc.fx` / `worldChange.fx` — and
  editing) — separate later phase.
- The `05-review.md` §4 non-A-2 secondary issues, unless one actively blocks the GI pipeline.

### Phase B deliverable / done-bar

The full NAADF real-time raytraced GI pipeline running on the Phase-A-2 base: 4-plane first-hit
→ adaptive ~0.25-spp GI rays via `rayQueueCalc` → compressed ReSTIR GI + refine + spatial
resampling → sparse bilateral denoiser → atmosphere → final. Build + the existing test suite
green; user interactive re-test confirms GI is rendering (bounce lighting visible, temporally
stable via the A-2 TAA, no obvious artifacts) — the Phase-B review gate.

---

## 3. Reuse audit summary

Full audit: `docs/orchestrate/naadf-bevy-port/00-reuse-audit.md`. Condensed verdict:

`bevy-naadf` is a **toolchain proof-of-concept, not a partial port** — zero voxel code, zero
AADF structure, zero NAADF subsystems. What exists:

| asset in `bevy-naadf` | location | verdict |
|---|---|---|
| App wiring & plugin setup | `src/main.rs:33-69` | **reuse** — NAADF plugins slot in as more `add_plugins` calls |
| `AppArgs` CLI resource | `src/main.rs:27-31` | **extend** — grows new fields for world size / seed / render version |
| Camera spawn + Solari/DLSS component wiring | `src/camera.rs:41-79` | **reuse with caveat** — see "open tension" below; Solari camera components may become reference-only |
| Runtime DLSS-RR toggle (`D` key) | `src/camera.rs:83-105` | **reuse** — generic debug control |
| Free-fly camera (`bevy_camera_controller`) | `src/main.rs:51`, `src/camera.rs:8-9,54-58` | **reuse** — start here; only port NAADF's integer/frac `PositionSplit` camera if large-world precision demands it |
| Diagnostics HUD overlay | `src/hud.rs:19-107` | **extend** — stays as always-on diagnostics layer |
| Procedural Cornell-box scene | `src/scene.rs:16-175` | **replace** — throwaway placeholder; the voxel grid replaces it wholesale |
| Cargo / `.cargo` / toolchain config | `Cargo.toml`, `.cargo/config.toml`, `rust-toolchain.toml` | **reuse + extend** — solid base; port adds deps |
| Bevy ECS / render graph / asset system | `DefaultPlugins`, `src/main.rs:49` | **reuse (built-in)** — NAADF's `*Handler` orchestration maps onto ECS + `Assets` + custom render nodes; do **not** port the handler architecture verbatim |
| Bevy Solari | `Cargo.toml:13-17`, `src/main.rs:20,59`, `src/camera.rs:12,67-69` | **reference-only** — per Q2, NOT the GI substrate; see open tension |

**NAADF subsystems that are entirely greenfield** (the actual port surface): VoxelsCore library;
the AADF multi-layered cell structure (the paper's core contribution, hardest piece); `World/Data`
chunk/edit/entity subsystem; `World/Generator` world generation; `World/Model` model subsystem;
the voxel type / layered-material system; `World/Render` + the `Content/shaders/render/**` HLSL
tree; the long-term-memory TAA & resampling pipeline. (`Gui/`, persistence, settings, IO,
importers are greenfield too but **out of scope** per Q1.)

### Open tension — RESOLVED by D3

The scaffold currently wires in `bevy_solari` (`Cargo.toml` features; `SolariPlugins` in
`src/main.rs`; Solari camera components `SolariLighting`/`Pathtracer` + `CameraMainTextureUsages`
STORAGE_BINDING in `src/camera.rs`). Per Q2, Solari is **not** the GI substrate. **Decision D3
(see §2b): strip Solari entirely** — remove the dependency, the plugins, and the Solari camera
components. Do not keep it dormant.

---

## 4. Required reading

### 4.1 The research paper — `docs/research/ulschmid-2026-naadf-voxel-gi.md` (~101 KB)

The primary source of truth for the AADF data structure (Q3). Known landmarks (line numbers in
that file):

| lines | content | phase |
|---|---|---|
| :40 | "Why this matters" framing note | context |
| :56 | Abstract — "3-5× from nesting, ×2 from AADFs (total 10×), ×2 for GI" | context |
| :135-143 | Three-layer cell hierarchy (chunk / block / voxel, each 4³) | **A** |
| :145-220 | Construction: hashing + flood-fill invalidation for editing | **A** |
| :190-216 | AADFs — per-empty-cell 6-direction axis-aligned distance fields (5 bits/dir for chunks, 2 bits/dir for blocks/voxels); let a DDA ray skip large empty regions in one iteration | **A** |
| §3.4 (traversal) | DDA traversal exploiting AADFs; first-hit | **A** |
| §3.5-3.6 | Edits / dynamic entities | **A** (data) / later |
| :230 | Section 4 title: "Application: Accelerating Global Illumination" | **B** |
| :234 | How the GI pipeline depends on NAADF (compact G-buffer, 64-bit TAA samples) | **B** |
| :238-265 | Long-term-memory TAA — 32 past frames @ 64 bits/sample | **B** |
| :267-323 | Compressed ReSTIR GI — lit/unlit separation, 8×8 screen-space regions, 12-iteration spatial pass, single visibility check | **B** |
| :325-327 | Sparse bilateral denoiser | **B** |

The `research` agent must read the whole document, not just these landmarks — the landmarks are
a map, not a substitute.

### 4.2 NAADF C# source — `/mnt/archive4/DEV/NAADF/NAADF/`

In-scope subsystems (per Q1). The `research` agent reads these; line counts below are
approximate, from the reuse audit's breadth-first skim — verify with Read.

| path | what it is | phase |
|---|---|---|
| `App.cs`, `Program.cs` | app entry / loop wiring (for understanding orchestration only — `Gui/` is out of scope) | context |
| `Libraries/VoxelsCore/` | `VoxelData`, `VoxelDataBytes`, `VoxelDataColors`, `Voxel`, `Material`, `Color`, `XYZ`, `BoundsXYZ`, `VoxelImport` + `MagicaVoxel`/`VoxFile`/`Voxlap` importers (importers OUT of scope) | **A** |
| `Common/DataTypes/Point3.cs`, `Common/Cube.cs`, `Common/Helper.cs` | math/util — mostly replaceable by `glam` + Bevy | **A** |
| `Common/DynamicStructuredBuffer.cs` | a growable GPU buffer abstraction — needs a `wgpu`/Bevy equivalent | **A** |
| `Common/Camera.cs` (~212 lines) | NAADF's camera incl. integer+frac `PositionSplit` world-space camera | **A** (port only if precision demands) |
| `World/Data/` | `WorldData` (~522 lines), `ChangeHandler`, `BlockHashingHandler`, `WorldBoundHandler`, `EntityData`/`EntityHandler`, `EditingHandler` + `EditingTools/` (cube/sphere/paint/floodfill/model) | **A** (storage/hashing) / editing later |
| `World/Generator/` | `WorldGenerator`, `WorldGeneratorModel` — GPU-driven world gen into chunk buffers/3D textures | **A** |
| `World/Model/` | `ModelData`, `ModelHandler` — voxel model placement/instancing | **A** |
| `World/VoxelTypeHandler.cs` (~169 lines) | voxel type / layered-material system (`MaterialTypeBase`/`MaterialTypeLayer`: Diffuse/Emissive/MetallicRough/MetallicMirror) | **A** (types) / **B** (emissive/material use) |
| `World/Render/WorldRender.cs` + `World/Render/Versions/WorldRender{Albedo,Base,PathTracer}.cs` | the render orchestration + the three version paths. **`WorldRenderAlbedo` is the Phase-A render path.** | **A** (Albedo) / **B** (Base, PathTracer) |
| `World/Render/Atmosphere*` | atmosphere model | **A**/**B** |
| `Content/shaders/render/**` | the HLSL render tree: `renderGlobalIllum.fx`, `renderSpatialResampling.fx`, `renderSampleRefine.fx`, `rayQueueCalc.fx`, `commonTaa.fxh`, `renderTaaSampleReverse.fx`, `renderDenoiseSplit.fx`, atmosphere | first-hit/albedo **A**, GI/TAA/resampling/denoise **B** |
| `Content/shaders/world/generator/generatorModel.fx`, `Content/shaders/world/model/typeMapping.fx` | generation + model-typing shaders | **A** |

### 4.3 Target Bevy scaffold — `/mnt/archive4/DEV/bevy-naadf/`

| path | what it is |
|---|---|
| `src/main.rs` | app wiring, `DefaultPlugins` + `SolariPlugins` + `FreeCameraPlugin`, `AppArgs` CLI resource, `--pathtracer` flag |
| `src/camera.rs` | camera spawn with Solari/DLSS component set; runtime DLSS-RR toggle on `D` |
| `src/hud.rs` | diagnostics HUD overlay (FPS, renderer mode, GPU pass timings) |
| `src/scene.rs` | placeholder Cornell-box — **to be replaced** |
| `Cargo.toml` | Bevy 0.19-rc.1 + `bevy_solari` + `free_camera`; `dlss`/`force_disable_dlss` gating |
| `.cargo/config.toml`, `rust-toolchain.toml` | `mold` linker, dev opt-level tuning, toolchain pin |
| `README.md` | setup + a milestone roadmap |

---

## 5. Forbidden moves

- **Do not port the `Gui/` editor tree, `.cvox` persistence, `Settings.cs`/`BuildFlags`,
  `IO.cs` input handling, or the `obj2voxel`/MagicaVoxel/VoxFile/Voxlap importers.** Out of
  scope per Q1. (The diagnostics `hud.rs` stays; the *editor* GUI does not get ported.)
- **Do not use Bevy Solari as the GI substrate.** Per Q2 the GI pipeline is a faithful WGSL
  port of NAADF's HLSL `Content/shaders/render/**`. Solari is reference-only/unused. This
  deliberately contradicts the reuse audit's top recommendation — the user overrode it on
  purpose.
- **Do not create a Cargo workspace.** Single binary crate, modules under `src/` (Q4).
- **Do not transliterate the AADF data structure line-by-line from C#.** Re-derive from the
  paper, cross-check against `Libraries/VoxelsCore/*.cs` (Q3). Produce idiomatic Rust.
- **Do not port NAADF's C# `*Handler` orchestration architecture verbatim** (`WorldHandler`,
  `ChangeHandler`, `BlockHashingHandler`, etc.). Map it onto Bevy ECS systems/resources +
  `Assets<T>` + custom render-graph nodes.
- **Do not start a later phase's design or implementation until the prior phase is reviewed and
  confirmed runnable.** Phase order is strictly **A → A-2 (TAA) → B (GI) → C (GPU
  construction/editing)**; `design` and `impl` proceed one gated phase at a time.
- **Do not resolve the "strip vs. keep dormant Solari" tension before the `design` phase** —
  it is the design phase's call; flag it, don't pre-empt it.
- General `/delegate` rule: each agent reads this file + its group file first, and **Writes its
  deliverable to its group file on disk before returning** — agent return text is status only.
