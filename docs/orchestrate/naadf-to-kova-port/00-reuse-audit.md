# NAADF -> Kova Port: Reuse Audit

**Date:** 2026-05-13
**Auditor:** delegated re-implementation auditor (no parent-conversation memory)

## Scope and confidence

This audit enumerates what currently exists in Kova (`/mnt/archive4/DEV/Kova`) that overlaps with the ten NAADF subsystems the orchestrator listed, and which NAADF source files are clean enough to lift wholesale. The Kova codebase was read end-to-end — it is small (about 25 hand-written C# files plus one WGSL shader); every non-generated `.cs` file under `src/` was opened. The NAADF side was sampled (key headers of each subsystem) rather than read in full; per-shader internals were not audited because no Kova counterpart exists to compare against. Confidence is **high** for Kova-side claims, **medium-high** for NAADF reusability (depends on grep results for XNA/SharpDX references, which were verified).

The headline finding: **Kova is effectively a "hello-triangle" engine.** It owns a working WebGPU/WebGL2 abstraction, an offline asset pipeline (texture/mesh/Substance cooking), and a window+platform loop. It owns *nothing* of NAADF's voxel data structures, GPU world generation, ray traversal, AADF acceleration, GI/ReSTIR, atmospherics, voxel-format IO, voxel editing, compute infrastructure (no compute path exists at all), camera, or ImGui. The reuse story is therefore lopsided: most NAADF subsystems are greenfield in Kova, but several NAADF library files (`Libraries/VoxelsCore/*.cs`) are XNA-free and liftable.

---

## Inventory of Kova

### `Kova.Core` (interfaces, types, ECS host)
- `src/Kova.Core/GameApp.cs` — abstract `GameApp` base with `LoadAsync()`, `Update()`, `Render()` hooks.
- `src/Kova.Core/IPlatform.cs` — `IPlatform`: holds `IGraphicsDevice`, `LoadTextAssetAsync`, `LoadShaderAsync(name)` returning `(vertex, fragment)` WGSL strings, `RunAsync(GameApp)`.
- `src/Kova.Core/BuildTarget.cs` — `Desktop | Web` enum used by the asset pipeline.
- `src/Kova.Core/Graphics/IGraphicsDevice.cs` — minimal device: `CreateVertexBuffer<T>`, `CreateShader`, `CreatePipeline`, `BeginFrame`, `Clear`, `Draw`, `EndFrame`. **No compute, no textures, no uniforms, no index buffers, no depth.**
- `src/Kova.Core/Graphics/IWindow.cs` — `IsClosing`, `Run(onUpdate, onRender)`.
- `src/Kova.Core/Graphics/Handles.cs` — opaque `BufferHandle / ShaderHandle / PipelineHandle / TextureHandle / MeshHandle` (ulong id). The last two handles exist but no API uses them yet.
- `src/Kova.Core/Graphics/Vertex.cs` — `Vertex { Vector3 Position, Vector4 Color }`. Demo-only.
- `src/Kova.Core/Graphics/VertexLayout.cs` — `VertexLayout`, `VertexAttribute`, `VertexFormat (Float32x2/x3/x4)`.
- `src/Kova.Core/Assets/TextureFormat.cs` — `Rgba8Unorm | Rgba8Srgb` only.
- `src/Kova.Core/Assets/MeshSemantic.cs` — `Position, Normal, Tangent, UV0, Color, BoneIndices, BoneWeights`.
- `src/Kova.Core/Assets/MeshVertexFormat.cs` — `Float32x2/3/4, UByte4, UByte4N`.
- `src/Kova.Core/Assets/IndexFormat.cs` — `U16 | U32`.
- `src/Kova.Core/Assets/BlendShape.cs` — morph-target deltas (position + normal).
- Friflo.Engine.ECS 3.5 referenced via `Kova.Core.csproj` — **no ECS component types defined yet**.

### `Kova.Graphics.WebGPU` (Silk.NET wgpu-native desktop + Emscripten browser)
- `src/Kova.Graphics.WebGPU/WgpuPlatform.cs` — desktop `IPlatform` impl, `LoadShaderAsync` reads `Shaders/<name>.wgsl`.
- `src/Kova.Graphics.WebGPU/WgpuWindow.cs` — Silk.NET window wrapper, `CreateDevice()` requests WebGPU instance + surface.
- `src/Kova.Graphics.WebGPU/WgpuGraphicsDevice.cs` — full unsafe Silk.NET WebGPU implementation: adapter/device request, surface configuration with `PresentMode.Fifo`, vertex buffer creation via `QueueWriteBuffer`, WGSL shader-module creation, render pipeline creation, render pass with clear + draw, command-buffer submit + present. Dictionary-based handle tracking. **Render-only. No compute, no bind groups, no uniforms, no textures, no depth attachment.**
- `src/Kova.Graphics.WebGPU/Browser/BrowserWebGpuPlatform.cs` — browser counterpart to `WgpuPlatform`.
- `src/Kova.Graphics.WebGPU/Browser/BrowserGraphicsDevice.cs` — JS interop wrapper, base64-encodes vertex data, builds layout JSON, dispatches to `Kova.Graphics.WebGPU.lib.module.js`.
- `src/Kova.Graphics.WebGPU/Browser/BrowserWindow.cs` — `requestAnimationFrame` loop.
- `src/Kova.Graphics.WebGPU/Browser/WebGpuInterop.cs` — JSImports: `gpuInitialize`, `gpuCreateVertexBuffer`, `gpuCreateShaderModule`, `gpuCreateRenderPipeline`, `gpuBeginFrame`, `gpuClear`, `gpuDraw`, `gpuEndFrame`, `scheduleAnimationFrame`, `fetchText`.

### `Kova.Graphics.WebGL2` (fallback for browsers without WebGPU)
- `src/Kova.Graphics.WebGL2/WebGL2Platform.cs`, `WebGL2BrowserWindow.cs`, `WebGL2GraphicsDevice.cs`, `WebGL2Interop.cs` — same surface as the WebGPU browser path, GLSL programs instead of WGSL. Naga (WGSL -> GLSL ES 3.0) is invoked at build time per `Kova.App.csproj` lines 41–55.

### `Kova.App` (entry point)
- `src/Kova.App/Program.cs` — currently displays a single hard-coded triangle (3 vertices, no input, no camera, no scene).
- `src/Kova.App/KovaPlatform.cs` — platform factory; switches on `getBackendName()` JS interop in WASM, uses `WgpuPlatform` on desktop.
- `src/Kova.App/Shaders/triangle.wgsl` — the only shader.
- `src/Kova.App/Kova.App.lib.module.js`, `wwwroot/index.html` — browser entry.

### `Kova.AssetPipeline` (offline cooker)
- `src/Kova.AssetPipeline/AssetPipeline.cs` — orchestrator: discover -> sync TOML meta files -> filter via manifest -> parallel cook -> save manifest.
- `src/Kova.AssetPipeline/AssetImporter.cs`, `AssetImporterOfT.cs` — base classes for typed importers.
- `src/Kova.AssetPipeline/AssetDiscovery.cs`, `BuildManifest.cs`, `DiscoveredAsset.cs`, `CookResult.cs`, `PipelineResult.cs`, `AssetContext.cs`, `ImporterRegistry.cs` — pipeline plumbing.
- `src/Kova.AssetPipeline/TomlSerializer.cs` — TOML for .meta sidecar files (Tomlyn 0.19).
- `src/Kova.AssetPipeline/KtexWriter.cs` — writes a custom `.ktex` container (KTEX magic + version + width/height/format/mip-count, raw RGBA8 mip chain with box-filter downsampling).
- `src/Kova.AssetPipeline/Importers/TextureImporter.cs` — .png/.tga/.bmp via StbImageSharp -> .ktex.
- `src/Kova.AssetPipeline/Importers/ModelImporter.cs` — .fbx/.obj/.blend via AssimpNet -> custom `.mesh` binary (KMSH magic, interleaved vertex stream, 16/32-bit indices, blend shapes, bones reserved).
- `src/Kova.AssetPipeline/Importers/SubstanceImporter.cs` — .sbsar via sbsario native lib -> .ktex per output map.
- `src/Kova.AssetPipeline/Importers/KtxImporter.cs` — stub.
- `src/Kova.AssetPipeline/Importers/TextureArrayImporter.cs` — stub.
- `src/Kova.AssetPipeline/Substance/SbsarioNative.cs`, `SbsarioTypes.cs`, `SubstanceEngine.cs` — substance engine bindings.

### `Kova.AssetCompiler` / `Kova.Assets`
- `src/Kova.AssetCompiler/Program.cs` — CLI driver for the pipeline.
- `src/Kova.Assets/AssetLoader.cs` — bare stub: stores `cookedDirectory`, comment says "Phase 2 will add LoadTexture, LoadMesh". **No runtime asset loading exists yet.**

---

## Subsystem-by-subsystem audit table

| NAADF subsystem | Kova equivalent (file path or "—") | Status | Recommendation | Notes |
|---|---|---|---|---|
| 1. Voxel data structure (chunk/block/voxel 3-level hierarchy with GPU buffers) | — | Missing | Port | Kova has zero voxel types. `Kova.Core/Graphics/Handles.cs` defines an unused `MeshHandle` but nothing voxel-shaped. NAADF's `WorldData.cs` is tightly bound to MonoGame `StructuredBuffer` / `Texture3D` / `Effect` (see `NAADF/World/Data/WorldData.cs:1-15`) so it must be rewritten against Kova's `IGraphicsDevice`. The architecture (4³×4³×4³, segment-based GPU upload, free-slot queues) is portable; the implementation is not. |
| 2. AADF (axis-aligned directional distance fields) | — | Missing | Port | Pure novel research IP; Kova has no acceleration structure of any kind. Shader code (HLSL `chunkCalc.fx`, `boundsCalc.fx`, `boundsCommon.fxh`) must be hand-translated to WGSL. The algorithm/data layout is the load-bearing artifact, not the C# wrapper. |
| 3. Ray-traversal renderer (primary+secondary rays, ReSTIR, TAA, atmosphere) | — | Missing | Port | Kova ships one rasterized triangle pipeline (`WgpuGraphicsDevice.CreatePipeline` only builds `RenderPipeline`, never `ComputePipeline`). The entire compute-based ray tracer must be ported to WGSL compute. `Atmosphere.cs` is mostly math on `Vector3` but imports `Microsoft.Xna.Framework` for the `Vector3` type and `SharpDX.MediaFoundation` (unused) — line 1-3 — so a small swap to `System.Numerics.Vector3` makes the CPU part liftable; the GPU side is HLSL and is not. |
| 4. GPU world generation (`WorldGenerator.cs`, `WorldGeneratorModel.cs`) | — | Missing | Port | Same shape as #1 and #3: CPU dispatcher uses MonoGame `StructuredBuffer` / `Texture3D` (`NAADF/World/Generator/WorldGenerator.cs:1-26`). Algorithm is portable, infrastructure is not. |
| 5. CPU-side editing & entity logic (flood-fill, brushes, paint, model placement, entity sync) | — | Missing | Port | NAADF/World/Data/EditingHandler.cs + EditingTools/* + EntityHandler.cs are pure CPU C# but reference `Microsoft.Xna.Framework` (`Vector3`, etc.) and depend on `WorldData`. Mostly mechanical port once `WorldData` exists. |
| 6. Voxel file IO (.cvox, .vox, .vl32, .obj/.stl via obj2voxel.exe) | `src/Kova.AssetPipeline/Importers/ModelImporter.cs` covers .obj via AssimpNet (mesh -> KMSH, NOT mesh -> voxelization) | Missing (for voxel formats); Partial (for .obj as mesh) | Lift NAADF VoxelsCore + Port wrapper | `NAADF/Libraries/VoxelsCore/{VoxFile,MagicaVoxel,Voxlap,VoxelData,VoxelDataBytes,VoxelImport,XYZ,BoundsXYZ,Color,Material,Voxel}.cs` are XNA-free (grep confirms no `Microsoft.Xna` / `SharpDX` / `System.Windows` references in that folder) and lift wholesale into Kova as a `Kova.VoxelsCore` library or into `Kova.AssetPipeline`. `.cvox` ZIP IO lives in `World/Model/ModelData.cs` and is XNA-tangled. `obj2voxel.exe` is Windows-only; replace with a CPU voxelizer. |
| 7. Compute shader infrastructure (`DynamicStructuredBuffer.cs`, dispatch wrappers, hot-reload) | — | Missing | Rewrite | Kova's `IGraphicsDevice` has no compute concept (no `CreateComputePipeline`, no `Dispatch`, no `CreateStorageBuffer`, no bind groups). NAADF's `DynamicStructuredBuffer.cs:1-10` is MonoGame `StructuredBuffer` + SharpDX. Recommendation: rewrite around WebGPU storage buffers + compute pipelines, do not port. |
| 8. Camera + input (first-person `Camera.cs`, mouse capture) | — (no input, no camera, no math beyond `System.Numerics`) | Missing | Rewrite | NAADF `Camera.cs:1-10` pulls `Microsoft.Xna.Framework.Input` + `SharpDX.Direct3D9` + `SharpDX.MediaFoundation`. Kova has no input subsystem at all — Silk.NET.Input is not even referenced in any csproj. Top-down 3D game wants a different camera anyway. Rewrite. |
| 9. GUI / ImGui (debug UI, model browser, header bar, editing UI) | — | Missing | Rewrite | NAADF uses `ImGui.NET` 1.91.6.1 via a MonoGame-specific renderer (`NAADF/Gui/Internal/ImGuiRenderer.cs`). Kova has no UI. ImGui.NET works on WebGPU/WebGL via custom renderers, but the NAADF renderer is MonoGame-coupled. If a debug GUI is needed, integrate ImGui.NET fresh against `IGraphicsDevice`; the NAADF debug *contents* (sliders, panels in `WorldRenderBase.RenderImGui`) can be ported as call-site code. |
| 10. Math primitives (`Point3`, `BoundsXYZ`, `XYZ`, other tuples) | `System.Numerics.Vector3` is used throughout Kova; no integer-coordinate type exists | Partial | Lift `XYZ`/`BoundsXYZ` from VoxelsCore; drop `Point3` | NAADF has *two* integer-3D types: `Voxels.XYZ` (XNA-free, in `Libraries/VoxelsCore/XYZ.cs`) and `NAADF.Common.Point3` (XNA-tangled, in `Common/DataTypes/Point3.cs:1`). Recommend lifting `XYZ` + `BoundsXYZ` and dropping `Point3` entirely — replace all `Point3` usages with `XYZ` during the port. |

---

## Reusable code from NAADF itself

### Lift wholesale (no source changes beyond namespace)
All files in `NAADF/Libraries/VoxelsCore/` are XNA-free and SharpDX-free (verified by `grep -rl "Microsoft.Xna\|SharpDX\|System.Windows" NAADF/Libraries/VoxelsCore/` returning nothing):
- `XYZ.cs` — integer 3D vector with operators, hash, `Transform(Matrix4x4)`.
- `BoundsXYZ.cs` — inclusive integer AABB.
- `Color.cs` — `[StructLayout(Explicit)]` 32-bit RGBA with HSV conversion.
- `Material.cs`, `Voxel.cs` — voxel datum types.
- `VoxelData.cs`, `VoxelDataT.cs`, `VoxelDataBytes.cs`, `VoxelDataColors.cs` — in-memory voxel grids.
- `MagicaVoxel.cs` — full MagicaVoxel `.vox` reader (nodes, palette, materials, animation frames).
- `Voxlap.cs` — Voxlap `.vox` (.vl32) reader.
- `VoxFile.cs` — multiplexer over MagicaVoxel + Voxlap.
- `VoxelImport.cs` — file-extension dispatch.

Recommendation: copy this folder into a new `src/Kova.VoxelsCore/` project (or into `Kova.AssetPipeline` if you only need it at cook time). Namespace `Voxels` may need adjusting to fit Kova conventions.

### Lift with minor conversion (XNA `Vector3` -> `System.Numerics.Vector3`)
- `NAADF/World/Render/Atmosphere.cs` — `Vector3`-math only, but `using Microsoft.Xna.Framework;` on line 1 and a stray `using SharpDX.MediaFoundation;` on line 3. The XNA `Vector3` and `System.Numerics.Vector3` have identical APIs for the operations used (`Dot`, `+`, `-`, `*`, `Math.Exp`, `Math.Sqrt`), so this is a 1-line `using` swap. The whole atmospheric scattering CPU model is portable.

### Cannot lift — XNA/SharpDX/WinForms-tangled, must rewrite
All of these contain `Microsoft.Xna.Framework` and/or `SharpDX` imports (verified by grep):
- `NAADF/Common/*.cs` — `Camera.cs`, `Cube.cs`, `DynamicStructuredBuffer.cs`, `Helper.cs`, `PathHandler.cs`, `CommonExtensions.cs`, `DataTypes/Point3.cs`, `DataTypes/Other.cs`, `Extensions/File/ExtFileRead.cs`, `Extensions/File/ExtFileWrite.cs`.
- `NAADF/World/Data/*.cs` — `WorldData.cs`, `EditingHandler.cs`, `EntityHandler.cs`, `ChangeHandler.cs`, `WorldBoundHandler.cs`, `BlockHashingHandler.cs`, `EntityData.cs`, all of `EditingTools/`.
- `NAADF/World/Render/*.cs` — `WorldRender.cs`, `Versions/WorldRenderBase.cs`, `Versions/WorldRenderAlbedo.cs`, `Versions/WorldRenderPathTracer.cs`.
- `NAADF/World/Generator/WorldGenerator.cs`, `WorldGeneratorModel.cs`.
- `NAADF/World/Model/ModelData.cs`, `ModelHandler.cs` — also imports `Accord.MachineLearning` (color clustering for palette compression — note this for the port).
- `NAADF/Gui/**` — ImGui.NET *contents* are portable as call patterns, but the renderer and every host file imports XNA.
- `NAADF/App.cs`, `NAADF/Program.cs`, `NAADF/Settings.cs`, `NAADF/IO.cs`, `NAADF/World/WorldHandler.cs`, `NAADF/World/VoxelTypeHandler.cs`.

### Cannot lift — HLSL not WGSL, but algorithms are reference material
Every `.fx`/`.fxh` under `NAADF/Content/shaders/` is HLSL targeting `MonoGame.Framework.Compute.WindowsDX` (cpt-max fork). They are not portable as-is but are the canonical reference for:
- `Content/shaders/world/data/chunkCalc.fx` + `boundsCalc.fx` + `boundsCommon.fxh` — AADF generation.
- `Content/shaders/render/rayTracing.fxh` — DDA traversal using AADF.
- `Content/shaders/render/versions/base/*.fx` — render pipeline (renderFirstHit, renderGlobalIllum, renderSampleRefine, renderSpatialResampling, renderTaaSampleReverse).
- `Content/shaders/world/generator/*` — terrain compute.

---

## Top 5 risks

1. **No compute pipeline in Kova's graphics abstraction.** `IGraphicsDevice` (`src/Kova.Core/Graphics/IGraphicsDevice.cs`) exposes only `CreateVertexBuffer`, `CreateShader (vs+fs)`, `CreatePipeline (render)`, `BeginFrame/Clear/Draw/EndFrame`. NAADF is overwhelmingly compute-based — world generation, AADF generation, ray tracing, GI, ReSTIR, TAA are all compute dispatches. **Mitigation:** before any voxel work, extend `IGraphicsDevice` (and both WGPU + WebGL2 backends) with compute pipelines, storage buffers, bind groups, push-constant/uniform buffers, and dispatch. WebGL2 has *no compute shaders at all* — the WebGL2 fallback will need a parallel rasterization path or be dropped for voxel rendering.

2. **WebGPU storage-buffer + 3D-texture limits vs. NAADF's `Texture3D dataChunkGpu` (`WorldData.cs:34`).** NAADF allocates one huge `Texture3D` for chunk data (16k³ voxels potential). WebGPU has tighter limits (max storage buffer 128 MiB on many devices; max texture 3D often 2048³). **Mitigation:** plan an early spike to measure on the target hardware; consider segmenting the chunk volume across multiple bound resources or using sparse-ish allocations.

3. **WGSL is a much weaker shader language than the cpt-max HLSL compute fork.** NAADF relies on uint atomics, `InterlockedAdd`, `groupshared` memory, structured buffers with embedded counters (`StructuredBufferType.Append`, `counterResetValue` — see `DynamicStructuredBuffer.cs:18`), and bindless-ish HLSL patterns. WGSL has atomics and workgroup memory but no append buffers and stricter binding rules. **Mitigation:** Reference paper + HLSL are required reading; reserve time for non-trivial shader-architecture changes (counter management via separate atomic buffers, no automatic resource transitions).

4. **The "naga at build time -> GLSL ES 3.0" transpile step (`Kova.App.csproj:45-55`) silently restricts what WGSL features the project can use** if WebGL2 must keep working. GLSL ES 3.00 has no compute, no storage buffers, no UBOs above ~16 KiB, only 4 MRT, and no atomics. Either the WebGL2 fallback is dropped for voxel rendering or the renderer is structured so its non-voxel parts remain GLSL-compatible. **Mitigation:** Decide upfront whether NAADF support on WebGL2 is a goal. If not, gate the voxel pipeline behind a "WebGPU only" path and keep WebGL2 for fallback/login/menu.

5. **NAADF's editing/entity code mutates `WorldData` from multiple threads (see `_resizeLock` on `WorldData.cs:55`, `Concurrent` types throughout) and is interleaved with MonoGame's main-thread requirements.** Porting this to Friflo ECS-friendly code is non-trivial — Friflo expects ECS systems, not "static handler" classes. **Mitigation:** during the design phase, decide whether NAADF's "handler" objects become Friflo systems, or whether voxel state stays out of ECS entirely (singleton service injected into systems). NAADF's `EntityHandler` should probably *become* an ECS query — that's the natural fit.

---

## Top 3 reuse opportunities

1. **`NAADF/Libraries/VoxelsCore/` lifts wholesale.** This is the highest-leverage win: 12+ files, ~XX hundred lines of debugged voxel-file IO + math + palette/material data structures, copyable with namespace-only changes. Do this first; it unblocks the `.vox` import path immediately and gives the port real assets to test against. Land it as a new `src/Kova.VoxelsCore/Kova.VoxelsCore.csproj` (or fold into `Kova.AssetPipeline` if voxels are cook-only). Drop `NAADF.Common.Point3` and use `Voxels.XYZ` everywhere.

2. **Reuse Kova's existing `AssetPipeline` framework for voxel-format cooking.** The pipeline already does discovery + per-asset TOML meta + manifest-based incremental builds + parallel cook (`src/Kova.AssetPipeline/AssetPipeline.cs`). A new `VoxImporter : AssetImporter<VoxSettings>` registered into `ImporterRegistry` is the natural home for `.vox` / `.vl32` / `.cvox` cooking — producing a compact runtime voxel format analogous to how `ModelImporter` produces `.mesh` (KMSH) and `TextureImporter` produces `.ktex`. This avoids inventing a parallel pipeline.

3. **Reuse Kova's `IGraphicsDevice` abstraction as the seam for all GPU work — but extend it, don't bypass it.** The temptation will be to drop into `WgpuGraphicsDevice` directly for compute/voxels. Resist this: keep the `Kova.Core` -> backend split (currently honored by both WebGPU and WebGL2). Extending `IGraphicsDevice` with compute primitives is risk #1's mitigation and reuse opportunity #3 simultaneously — every voxel feature should land first as new `IGraphicsDevice` methods, then as backend implementations, so the WebGL2/web story has a chance.

---

## Top reuse recommendation

**Lift `NAADF/Libraries/VoxelsCore/*.cs` wholesale into a new `Kova.VoxelsCore` project as the first concrete port deliverable.** This is the only NAADF code that crosses the divide cleanly (verified XNA-free, SharpDX-free, WinForms-free), it covers subsystem #6 (voxel file IO) and subsystem #10 (math primitives `XYZ`, `BoundsXYZ`) entirely, and it gives every later port phase a stable, debugged data type to build against. Beyond VoxelsCore, the answer is honest: **no existing Kova code covers the voxel-rendering subsystems (#1–#5, #7–#9); they are greenfield, with reuse limited to extending `IGraphicsDevice` and slotting voxel cookers into the existing `AssetPipeline`.**
