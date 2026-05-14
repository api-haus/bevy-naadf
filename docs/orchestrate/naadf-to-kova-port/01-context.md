# Canonical context: NAADF -> Kova port

Every sub-agent dispatched under this orchestration reads this file first, in full, before touching any other file.

## Goal (verbatim)

User invocation: **"lets try to port NAADF to Kova"**.

Interpretation: port NAADF's voxel engine subsystems from the Windows-only MonoGame DX11 project at `/mnt/archive4/DEV/NAADF` into the cross-platform .NET 10 / WebGPU / Silk.NET / Friflo ECS project at `/mnt/archive4/DEV/Kova`. The port targets desktop Linux + Windows + Web (WASM) per Kova's existing build matrix. All code changes land **in Kova**. NAADF is read-only reference material — do not edit, build, or run NAADF.

## NAADF inventory (source codebase)

- **TFM**: `net8.0-windows`, `UseWindowsForms=true` (`/mnt/archive4/DEV/NAADF/NAADF/NAADF.csproj:4`).
- **Engine**: MonoGame Framework, cpt-max compute-capable fork (`MonoGame.Framework.Compute.WindowsDX.NoMemoryLeak`).
- **Shaders**: 22 HLSL `.fx` files under `NAADF/Content/shaders/`, compiled via `MonoGame.Content.Builder.Task.Compute`.
- **Major subsystems** (see `00-reuse-audit.md` for full file inventory):
  1. Voxel data structure (3 levels: chunk/block/voxel, 4³ nesting).
  2. AADF (axis-aligned directional distance fields).
  3. Ray-traversal renderer with ReSTIR-style resampling + TAA + atmosphere.
  4. GPU world generation.
  5. CPU editing & entity logic.
  6. Voxel file IO (.cvox, .vox, .vl32, .obj/.stl via external `obj2voxel.exe`).
  7. Compute infrastructure (dynamic structured buffers, dispatch wrappers).
  8. Camera + first-person input (`SharpDX.Direct3D9` + `System.Windows.Forms.Cursor`).
  9. ImGui-based debug GUI.
  10. Math primitives (`XYZ`, `BoundsXYZ`, `Color`, `Point3`).

## Kova inventory (destination codebase)

- **TFM**: `net10.0` (`/mnt/archive4/DEV/Kova/Directory.Build.props:3`).
- **Backends**: WebGPU via Silk.NET 2.23 + wgpu-native (desktop) + Emscripten WebGPU (browser). WebGL2 fallback via naga (WGSL -> GLSL ES 3.0 at build time, `Kova.App.csproj:41-55`).
- **ECS**: Friflo.Engine.ECS 3.5 referenced but **no ECS components defined yet**.
- **Project layout**:
  - `src/Kova.Core` — interfaces, types, ECS host.
  - `src/Kova.Graphics.WebGPU` — WebGPU backend, desktop + browser variants.
  - `src/Kova.Graphics.WebGL2` — WebGL2 fallback backend.
  - `src/Kova.App` — entry, shaders, JS interop.
  - `src/Kova.AssetPipeline`, `src/Kova.AssetCompiler`, `src/Kova.Assets` — offline content cooking + runtime loader stub.
- **State of art**: hello-triangle. `IGraphicsDevice` has render-only API (`CreateVertexBuffer`, `CreateShader` for vs+fs, `CreatePipeline` for render, `BeginFrame/Clear/Draw/EndFrame`). **No compute, no storage buffers, no bind groups, no uniforms, no textures, no depth attachment.** `Kova.App/Program.cs` displays a single hard-coded triangle.

## Project rules (`/mnt/archive4/DEV/Kova/CLAUDE.md`)

- No backwards compatibility. Break anything, any time.
- No public-API design patterns (no semver, no deprecation, no stability guarantees).
- Delete dead code immediately. No `[Obsolete]`, no shims.
- TDD: every bug fix starts with a failing test. (For greenfield port work, TDD is not required — but where a behavioral bug is encountered during porting, follow the rule.)

## Orchestrator's scope decisions (made without Q&A per user directive)

| Question | Decision |
|---|---|
| Which subsystems are in scope for v1? | Data structure, AADF, ray-traversal renderer, GPU world gen, voxel file IO, math primitives, CPU editing tools, first-person debug camera. |
| Out of scope for v1? | ImGui-based debug GUI. Defer until a UI layer lands in Kova generally. |
| Top-down game camera vs. NAADF first-person camera? | The game's top-down camera is unrelated. The voxel viewer needs a first-person debug camera, added as a separate component used only by a voxel-viewer scene. |
| WebGL2 voxel rendering? | **Out.** WebGL2 has no compute shaders, no storage buffers, no atomics. The voxel pipeline is WebGPU-only (desktop + browser-wasm WebGPU). WebGL2 remains as a fallback for non-voxel content. |
| Reuse `IGraphicsDevice` or bypass it? | **Reuse and extend.** Extending the abstraction is mandatory; bypassing into `WgpuGraphicsDevice` direct is forbidden. WebGL2 backend throws `NotSupportedException` for compute calls. |
| ECS integration? | **Defer.** Port `WorldData` / `EntityHandler` as handler-style classes first. Friflo ECS rework comes after the renderer is up. Entity layer is the natural future ECS candidate. |
| Where does `Kova.VoxelsCore` live? | New project `src/Kova.VoxelsCore/Kova.VoxelsCore.csproj`. Files lifted with namespace `Kova.Voxels` (replacing NAADF's `Voxels` namespace). |
| Where do voxel runtime types live? | New project `src/Kova.Voxels/Kova.Voxels.csproj` for `WorldData`, AADF logic, generator dispatch, renderer dispatch. References `Kova.Core`, `Kova.VoxelsCore`. |
| `obj2voxel.exe` Windows binary? | **Drop.** Either ship a cross-platform CPU voxelizer later or skip .obj/.stl import entirely in v1. AssimpNet already imports .obj as mesh in `ModelImporter.cs` — that path stays for mesh, not voxelization. |
| `Accord.MachineLearning` (palette clustering in `ModelData.cs`)? | **Drop.** Find a smaller alternative or hand-roll k-means on top of `System.Numerics`. Not on the v1 critical path. |
| Shader hot-reload? | **Skip in v1.** WGSL files reload on next build; nice-to-have later. |
| Naga transpile for voxel WGSL? | **Skip.** Voxel shaders are WebGPU-only; the naga step in `Kova.App.csproj:46-54` is for the WebGL2 fallback and does not affect voxel shaders. |

## Required reading for every sub-agent

Read these files (with line ranges where given) at the start of every dispatch:

| File | Why |
|---|---|
| `/mnt/archive4/DEV/Kova/docs/orchestrate/naadf-to-kova-port/01-context.md` | This file — canonical brief. |
| `/mnt/archive4/DEV/Kova/docs/orchestrate/naadf-to-kova-port/00-reuse-audit.md` | What exists already; the reuse-vs-port matrix. |
| `/mnt/archive4/DEV/Kova/docs/orchestrate/naadf-to-kova-port/02-design.md` | Phased plan (created by design agent; subsequent agents must read it). |
| `/mnt/archive4/DEV/Kova/CLAUDE.md` | Kova project rules. |
| `/mnt/archive4/DEV/Kova/Directory.Build.props` | TFM + global props. |
| `/mnt/archive4/DEV/Kova/src/Kova.Core/Graphics/IGraphicsDevice.cs` | The seam every GPU change goes through. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGPU/WgpuGraphicsDevice.cs` | Existing backend impl style. |
| `/mnt/archive4/DEV/Kova/src/Kova.AssetPipeline/Importers/ModelImporter.cs` | Reference for adding new importers. |
| `/mnt/archive4/DEV/NAADF/README.md` | Subsystem overview from author. |
| Files cited in `00-reuse-audit.md` | Per-subsystem source-of-truth file paths. |

## Forbidden moves

- Do **not** edit anything under `/mnt/archive4/DEV/NAADF/`. NAADF is read-only reference.
- Do **not** add `Microsoft.Xna.Framework`, `MonoGame.Framework.*`, or `SharpDX.*` to any Kova csproj. These are NAADF's legacy stack and must not enter Kova.
- Do **not** add `System.Windows.Forms` or `System.Drawing` (beyond what `System.Drawing.Primitives` already provides on .NET 10) to any Kova csproj. The codebase must remain Linux/Mac/web-capable.
- Do **not** bypass `IGraphicsDevice` to call into `WgpuGraphicsDevice` directly from voxel code. Extend the abstraction instead.
- Do **not** add HLSL `.fx`/`.fxh` shader files to Kova. All shaders are WGSL.
- Do **not** wire the WebGL2 backend into voxel rendering. Voxels are WebGPU-only.
- Do **not** ship NAADF's `obj2voxel.exe` binary. It is Windows-only.
- Do **not** add `[Obsolete]` markers, shims, or backwards-compat wrappers. Kova's rules forbid this.
- Do **not** invent design decisions the orchestrator has already made (see "Scope decisions" table above). Cite this file when asked why a decision was made.

## Reuse audit headlines (summary)

From `00-reuse-audit.md`:

- **8 of 10 NAADF subsystems are flat-out missing in Kova.** Greenfield port.
- **Cleanly liftable**: every file under `NAADF/Libraries/VoxelsCore/` (verified XNA/SharpDX/WinForms-free). Lift wholesale into `Kova.VoxelsCore`.
- **1-line `using` swap to lift**: `NAADF/World/Render/Atmosphere.cs` — replace `Microsoft.Xna.Framework` with `System.Numerics`.
- **Must rewrite**: everything in `NAADF/Common/`, `NAADF/World/Data/`, `NAADF/World/Render/`, `NAADF/World/Generator/`, `NAADF/World/Model/`, `NAADF/Gui/`, plus all HLSL shaders.
- **Top reuse opportunities**: (a) lift `VoxelsCore`; (b) reuse `AssetPipeline` framework for voxel-format cooking; (c) extend `IGraphicsDevice` rather than bypass it.

## Phase plan (starting hypothesis — design agent owns final phase ordering)

P0. Extend `IGraphicsDevice` with compute pipeline + storage buffer + bind group + dispatch. WebGPU full impl, WebGL2 stubs throw.
P1. Lift `VoxelsCore` into `src/Kova.VoxelsCore/`.
P2. Wire voxel-format importers into `Kova.AssetPipeline`.
P3. Port `WorldData` chunk/block/voxel hierarchy.
P4. Port AADF generation compute shaders to WGSL.
P5. Port GPU world generator to WGSL.
P6. Port ray-traversal renderer (primary + GI + ReSTIR + TAA + atmosphere).
P7. Port CPU editing tools + entity handler.
P8. Debug first-person camera + voxel-viewer scene in `Kova.App`.

The design agent re-validates this order, sizes each phase, and produces the file-level diff plan in `02-design.md`.
