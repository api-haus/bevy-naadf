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
