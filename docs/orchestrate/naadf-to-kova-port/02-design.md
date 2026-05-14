# NAADF -> Kova Port: Architecture

**Date:** 2026-05-13
**Architect:** delegated architecture agent (no parent-conversation memory)

## 0. Recap and invariants

Port the NAADF voxel engine (Windows-only MonoGame DX11, HLSL .fx compute) into Kova (.NET 10, WebGPU via Silk.NET 2.23 + Emscripten WebGPU, WebGL2 fallback, WGSL shaders). NAADF is read-only reference. Kova is a hello-triangle codebase with a render-only `IGraphicsDevice`, a working `Kova.AssetPipeline`, and no compute, no storage buffers, no bind groups, no input, no camera (per `00-reuse-audit.md` lines 19–47). Invariants (from `01-context.md` lines 48–63, 82–92):

- All GPU work goes through `IGraphicsDevice`. WebGL2 throws `NotSupportedException` for compute. WebGPU desktop + browser are full impls.
- No XNA, no MonoGame, no SharpDX, no WinForms in any Kova csproj.
- All shaders WGSL. No HLSL files staged "for later translation."
- `NAADF.Common.Point3` is forbidden in Kova — `Voxels.XYZ` replaces every usage.
- `obj2voxel.exe`, `Accord.MachineLearning`, ImGui-based GUI, hot-reload, ECS-ification, game-camera changes are all out of scope for v1.
- Voxel WGSL skips the `naga -> GLSL ES 3.0` transpile step at `src/Kova.App/Kova.App.csproj:45-55`.
- Friflo ECS is deferred — NAADF handler classes port as plain handler classes for v1; ECS migration is a follow-up.

## 1. New + modified project layout

### Diagram

```
src/
  Kova.Core/                        (modified — Handles, IGraphicsDevice, IInput)
  Kova.Graphics.WebGPU/             (modified — compute impl)
  Kova.Graphics.WebGL2/             (modified — compute stubs throw)
  Kova.AssetPipeline/               (modified — voxel importers + KvoxWriter)
  Kova.AssetCompiler/               (modified — register voxel importers)
  Kova.Assets/                      (modified — LoadVoxelModel, LoadVoxelWorld)
  Kova.App/                         (modified — voxel-viewer scene, WGSL shaders)
  Kova.VoxelsCore/                  (NEW — lift of NAADF/Libraries/VoxelsCore/)
  Kova.Voxels/                      (NEW — WorldData, AADF, generator, renderer)
  Kova.Voxels.Editing/              (NEW — editing tools, threading via locks)
  Kova.Voxels.Viewer/               (NEW — debug viewer scene + FP camera)
```

### Per-project responsibility

| Project | Owns | References |
|---|---|---|
| `Kova.Core` | `IGraphicsDevice` (compute extension), handle types, `IInput` abstraction | (none new) |
| `Kova.Graphics.WebGPU` | Full WebGPU compute impl, both desktop (Silk.NET unsafe) + browser (JS interop) | `Kova.Core` |
| `Kova.Graphics.WebGL2` | Compute methods throw `NotSupportedException` | `Kova.Core` |
| `Kova.VoxelsCore` | NAADF's `Voxels.XYZ`, `BoundsXYZ`, `Color`, `Voxel`, `Material`, `VoxelData*`, `VoxFile`, `MagicaVoxel`, `Voxlap`, `VoxelImport` | (none) |
| `Kova.AssetPipeline` | `VoxImporter`, `Vl32Importer`, `CvoxImporter` and `KvoxWriter` (custom `.kvox` runtime container) | `Kova.Core`, `Kova.VoxelsCore` |
| `Kova.Assets` | `AssetLoader.LoadVoxelModel(string) -> VoxelData` from cooked `.kvox` | `Kova.Core`, `Kova.VoxelsCore` |
| `Kova.Voxels` | `WorldData`, `VoxelSettings`, `BlockHashingHandler`, `WorldBoundHandler`, `ChangeHandler`, `EntityHandler`, `WorldGenerator`/`WorldGeneratorModel` dispatcher, `WorldRender` dispatcher, `Atmosphere` (CPU) | `Kova.Core`, `Kova.VoxelsCore` |
| `Kova.Voxels.Editing` | `EditingHandler`, `EditingTool`, `EditingToolCube`, `EditingToolSphere`, `EditingToolFloodFill`, `EditingToolPaint`, `EditingToolModel` | `Kova.Core`, `Kova.VoxelsCore`, `Kova.Voxels` |
| `Kova.Voxels.Viewer` | `FirstPersonCamera`, `VoxelViewerApp` (a `GameApp` subclass that wires WorldData + WorldRender + camera) | `Kova.Core`, `Kova.Voxels`, `Kova.Voxels.Editing`, `Kova.Assets` |
| `Kova.App` | `Program.cs` entry; switches between `TriangleApp` and `VoxelViewerApp` (gated by a CLI flag for now), WGSL shaders under `Shaders/voxels/` | + `Kova.Voxels.Viewer` |

`Kova.slnx` gains `Kova.VoxelsCore`, `Kova.Voxels`, `Kova.Voxels.Editing`, `Kova.Voxels.Viewer` under `/src/`.

---

## 2. `IGraphicsDevice` compute extension (signatures + rationale)

NAADF's compute usage (from `WorldData.GenerateWorld`, `WorldRenderBase.RenderInternal`, all `.fx` files inspected) requires the following surface. Every method below is required by at least one porting phase; nothing speculative.

### New handle types — added to `src/Kova.Core/Graphics/Handles.cs`

```csharp
public readonly record struct ComputeShaderHandle(ulong Id);
public readonly record struct ComputePipelineHandle(ulong Id);
public readonly record struct StorageBufferHandle(ulong Id);
public readonly record struct UniformBufferHandle(ulong Id);
public readonly record struct Texture3DHandle(ulong Id);
public readonly record struct BindGroupLayoutHandle(ulong Id);
public readonly record struct BindGroupHandle(ulong Id);
public readonly record struct IndirectBufferHandle(ulong Id);
```

The unused `MeshHandle` stays (used later by mesh rendering, out of scope here). `TextureHandle` is reserved for 2D textures; the voxel renderer needs a **3D** texture for the chunk volume so `Texture3DHandle` is distinct.

### New types in `src/Kova.Core/Graphics/`

```csharp
// Compute.cs — small DTOs for compute config

public enum StorageBufferUsage : uint {
    Storage         = 1 << 0, // RW from compute
    ReadOnlyStorage = 1 << 1, // RO from compute
    CopySrc         = 1 << 2,
    CopyDst         = 1 << 3,
    Indirect        = 1 << 4, // for DispatchIndirect
}

public enum Texture3DFormat : byte {
    R32Uint,
    Rg32Uint, // for NAADF's ENTITIES path (uint2 per chunk)
}

public enum BindingType : byte {
    UniformBuffer,
    StorageBuffer,        // read-write
    ReadOnlyStorageBuffer,
    StorageTexture3D,     // read-write texture
}

public readonly record struct BindGroupLayoutEntry(
    uint Binding,
    BindingType Type,
    Texture3DFormat? StorageTextureFormat = null);

public readonly record struct BindGroupEntry(uint Binding, ulong ResourceId);
// (BindGroupEntry.ResourceId is the .Id field of whichever handle is bound;
//  the device looks it up against the layout's expected BindingType.)
```

### Extended `IGraphicsDevice` (in `src/Kova.Core/Graphics/IGraphicsDevice.cs`)

```csharp
public interface IGraphicsDevice : IDisposable
{
    // --- existing (unchanged) ---
    BufferHandle CreateVertexBuffer<T>(ReadOnlySpan<T> data) where T : unmanaged;
    ShaderHandle CreateShader(string vertexSource, string fragmentSource);
    PipelineHandle CreatePipeline(ShaderHandle shader, VertexLayout layout);
    void BeginFrame();
    void Clear(float r, float g, float b, float a);
    void Draw(PipelineHandle pipeline, BufferHandle buffer, int vertexCount);
    void EndFrame();

    // --- compute (new) ---
    ComputeShaderHandle CreateComputeShader(string wgslSource, string entryPoint);
    ComputePipelineHandle CreateComputePipeline(
        ComputeShaderHandle shader,
        ReadOnlySpan<BindGroupLayoutHandle> bindGroupLayouts);

    BindGroupLayoutHandle CreateBindGroupLayout(ReadOnlySpan<BindGroupLayoutEntry> entries);
    BindGroupHandle CreateBindGroup(
        BindGroupLayoutHandle layout,
        ReadOnlySpan<BindGroupEntry> entries);

    // --- buffers (new) ---
    StorageBufferHandle CreateStorageBuffer(ulong sizeInBytes, StorageBufferUsage usage);
    StorageBufferHandle CreateStorageBuffer<T>(ReadOnlySpan<T> data, StorageBufferUsage usage)
        where T : unmanaged;
    void WriteStorageBuffer<T>(StorageBufferHandle handle, ReadOnlySpan<T> data, ulong byteOffset)
        where T : unmanaged;
    void ReadStorageBuffer<T>(StorageBufferHandle handle, Span<T> dst, ulong byteOffset)
        where T : unmanaged;
    StorageBufferHandle ResizeStorageBuffer(
        StorageBufferHandle old, ulong newSizeInBytes); // returns new handle; old released
    void ReleaseStorageBuffer(StorageBufferHandle handle);

    UniformBufferHandle CreateUniformBuffer(ulong sizeInBytes);
    void WriteUniformBuffer<T>(UniformBufferHandle handle, in T data) where T : unmanaged;
    void ReleaseUniformBuffer(UniformBufferHandle handle);

    // --- 3D textures (new — needed for chunks volume) ---
    Texture3DHandle CreateTexture3D(uint width, uint height, uint depth, Texture3DFormat format);
    void WriteTexture3D<T>(Texture3DHandle handle, ReadOnlySpan<T> data) where T : unmanaged;
    void ReadTexture3D<T>(Texture3DHandle handle, Span<T> dst) where T : unmanaged;
    void ReleaseTexture3D(Texture3DHandle handle);

    // --- indirect dispatch (NAADF uses DispatchComputeIndirect at WorldRenderBase.cs:323, :356, :359) ---
    IndirectBufferHandle CreateIndirectBuffer(uint groupCountX, uint groupCountY, uint groupCountZ);
    void ReleaseIndirectBuffer(IndirectBufferHandle handle);

    // --- recording (new) ---
    void BeginComputePass();
    void SetComputePipeline(ComputePipelineHandle pipeline);
    void SetBindGroup(uint groupIndex, BindGroupHandle group);
    void Dispatch(uint groupCountX, uint groupCountY, uint groupCountZ);
    void DispatchIndirect(IndirectBufferHandle indirect);
    void EndComputePass();
}
```

### Rationale, line by line

- `CreateComputeShader(string, string)` — WebGPU `ShaderModule`s can hold many entry points; NAADF .fx files have multiple passes per file (e.g. `chunkCalc.fx` has 4 passes). Passing the entry point at shader creation time mirrors WGSL's `@compute @workgroup_size(...) fn <name>` syntax. Two callers re-using one shader with different entry points create two `ComputeShaderHandle`s pointing at the same `ShaderModule` — the WebGPU impl can dedupe internally; the consumer doesn't care.
- `CreateComputePipeline` takes a span of `BindGroupLayoutHandle` because WGSL allows up to 4 bind groups (`@group(0)..@group(3)`), and NAADF compute shaders bind many resources at once. Per-pipeline layout is necessary for the WGPU `ComputePipelineDescriptor.Layout` field.
- `CreateBindGroupLayout` / `CreateBindGroup` mirror the WGPU concept directly. NAADF's HLSL "global" parameter setting (`effect.Parameters["x"].SetValue(...)`) collapses into "compose one bind group per pass."
- `CreateStorageBuffer<T>(span, usage)` covers `DynamicStructuredBuffer`'s "create + upload" combo (`NAADF/Common/DynamicStructuredBuffer.cs:18-23`). The uninitialized overload covers `new StructuredBuffer(...)` with no initial data.
- `ResizeStorageBuffer` returns a **new handle** because WebGPU buffers are immutable in size — the impl performs `CreateBuffer(newSize) + CommandEncoderCopyBufferToBuffer + Release(old)`. Returning the new handle forces call-sites to swap their cached value, mimicking how `DynamicStructuredBuffer.Resize` swaps the underlying `StructuredBuffer` (`NAADF/Common/DynamicStructuredBuffer.cs:38-49`).
- `CreateUniformBuffer + WriteUniformBuffer<T>` covers NAADF's `effect.Parameters["whatever"].SetValue(scalar/struct)` — these become packed UBOs the consumer writes per-frame.
- `CreateTexture3D` is mandatory: NAADF allocates `dataChunkGpu` as a `Texture3D` (`NAADF/World/Data/WorldData.cs:82`) keyed by `SurfaceFormat.Rg64Uint` (ENTITIES) or `SurfaceFormat.R32Uint`. WGSL `texture_storage_3d<r32uint, read_write>` is the analog. (See "Limit risk" in section 11 for what to do if device limits force a fallback.)
- `CreateIndirectBuffer` returns a buffer pre-populated with `(x, y, z)` group counts. NAADF uses it both as a passive container that compute passes mutate (`rayQueueCalc.fx` writes `groupCount.Store`) and as a `Dispatch*Indirect` source (`WorldRenderBase.cs:323`). The impl puts `BufferUsage.Indirect | BufferUsage.Storage` on the underlying buffer so a compute pass can write into it directly via a bound storage buffer alias. WebGPU exposes both usages on one buffer.
- `BeginComputePass / EndComputePass` bracket dispatch recording. Compute and render passes are mutually exclusive within one command encoder in WebGPU. NAADF often interleaves compute + sprite-draw within one frame; in WebGPU we'll end-compute-pass before starting a render pass and vice versa.
- No `BeginFrame/EndFrame` change: compute work can happen between `BeginFrame()` and `EndFrame()`, but `BeginComputePass / EndComputePass` are required to bracket dispatches. The WebGPU impl already owns a `CommandEncoder` between `BeginFrame` and `EndFrame` (`WgpuGraphicsDevice.cs:300-302`), so compute passes share that encoder.

### WebGL2 unsupported semantics

In `src/Kova.Graphics.WebGL2/WebGL2GraphicsDevice.cs`, every new method throws:

```csharp
throw new NotSupportedException(
    "Compute, storage buffers, 3D textures, and bind groups are not supported on the WebGL2 fallback. " +
    "Voxel rendering requires WebGPU.");
```

Call sites that touch voxels are gated upstream — `Kova.Voxels.Viewer` refuses to run on WebGL2 by checking the backend name (`KovaJsInterop.GetBackendName()` already exists at `KovaPlatform.cs:46`).

### Browser WebGPU impl

`src/Kova.Graphics.WebGPU/Browser/BrowserGraphicsDevice.cs` mirrors the desktop impl method-for-method. Each new method gets a `WebGpuInterop.Gpu*` JS-import counterpart, and the matching JS lands in `src/Kova.Graphics.WebGPU/Kova.Graphics.WebGPU.lib.module.js`. Implementation detail (sizes / base64 encoding for upload) follows the existing pattern at `BrowserGraphicsDevice.cs:19-27`.

---

## 3. `Kova.VoxelsCore` lift specification

### Project file: `src/Kova.VoxelsCore/Kova.VoxelsCore.csproj`

```xml
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <AllowUnsafeBlocks>true</AllowUnsafeBlocks>
    <RootNamespace>Kova.Voxels</RootNamespace>
  </PropertyGroup>
</Project>
```

No package references — the lifted code only depends on `System.Numerics`, `System.IO`, `System.Collections`. Verified XNA-free (`/mnt/archive4/DEV/NAADF/NAADF/Libraries/VoxelsCore/` grep for `Microsoft.Xna|SharpDX|System.Windows` returns nothing). See audit at `00-reuse-audit.md` lines 88–100.

### Files copied verbatim, namespace `Voxels` -> `Kova.Voxels`

| Source (NAADF) | Destination (Kova) | Change |
|---|---|---|
| `Libraries/VoxelsCore/XYZ.cs` | `src/Kova.VoxelsCore/XYZ.cs` | `namespace Voxels` -> `namespace Kova.Voxels`. |
| `Libraries/VoxelsCore/BoundsXYZ.cs` | `src/Kova.VoxelsCore/BoundsXYZ.cs` | same |
| `Libraries/VoxelsCore/Color.cs` | `src/Kova.VoxelsCore/Color.cs` | same |
| `Libraries/VoxelsCore/Material.cs` | `src/Kova.VoxelsCore/Material.cs` | same |
| `Libraries/VoxelsCore/Voxel.cs` | `src/Kova.VoxelsCore/Voxel.cs` | same |
| `Libraries/VoxelsCore/VoxelData.cs` | `src/Kova.VoxelsCore/VoxelData.cs` | same |
| `Libraries/VoxelsCore/VoxelDataT.cs` | `src/Kova.VoxelsCore/VoxelDataT.cs` | same |
| `Libraries/VoxelsCore/VoxelDataBytes.cs` | `src/Kova.VoxelsCore/VoxelDataBytes.cs` | same |
| `Libraries/VoxelsCore/VoxelDataColors.cs` | `src/Kova.VoxelsCore/VoxelDataColors.cs` | same |
| `Libraries/VoxelsCore/MagicaVoxel.cs` | `src/Kova.VoxelsCore/MagicaVoxel.cs` | same |
| `Libraries/VoxelsCore/Voxlap.cs` | `src/Kova.VoxelsCore/Voxlap.cs` | same |
| `Libraries/VoxelsCore/VoxFile.cs` | `src/Kova.VoxelsCore/VoxFile.cs` | same |
| `Libraries/VoxelsCore/VoxelImport.cs` | `src/Kova.VoxelsCore/VoxelImport.cs` | same |

Verified file presence by listing `/mnt/archive4/DEV/NAADF/NAADF/Libraries/VoxelsCore/` (13 files, all listed above). All declare `namespace Voxels` per grep at investigation time.

### Referenced by

- `Kova.AssetPipeline` (cook-time use by voxel importers — reads `.vox` via `VoxFile.Load`)
- `Kova.Assets` (runtime — re-uses `XYZ`, `BoundsXYZ`, `VoxelType` / `Material` types)
- `Kova.Voxels` (runtime — `WorldData` carries `XYZ` sizes, `Material` palette)
- `Kova.Voxels.Editing` (transitive)
- `Kova.Voxels.Viewer` (transitive)

The reuse audit (`00-reuse-audit.md` line 142) asks whether `Kova.VoxelsCore` should be referenced by `AssetPipeline`, `Voxels`, or both. Answer: **both**. The library is leaf-level pure data; sharing it costs nothing. No project cycle results because `Kova.VoxelsCore` has no upward dependencies.

---

## 4. `Kova.Voxels` runtime — files, types, constants migration

### Project file: `src/Kova.Voxels/Kova.Voxels.csproj`

```xml
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <AllowUnsafeBlocks>true</AllowUnsafeBlocks>
  </PropertyGroup>
  <ItemGroup>
    <ProjectReference Include="..\Kova.Core\Kova.Core.csproj" />
    <ProjectReference Include="..\Kova.VoxelsCore\Kova.VoxelsCore.csproj" />
  </ItemGroup>
</Project>
```

### Files

| File | Replaces (NAADF) | Role |
|---|---|---|
| `src/Kova.Voxels/VoxelSettings.cs` | `NAADF/Settings.cs` build flags + the constants implicit in `WorldData.cs:60-68` | Static class holding compile-time constants. |
| `src/Kova.Voxels/WorldData.cs` | `NAADF/World/Data/WorldData.cs` | Voxel hierarchy + GPU resource owner. Calls `IGraphicsDevice` for storage buffers + 3D texture. |
| `src/Kova.Voxels/BlockHashingHandler.cs` | `NAADF/World/Data/BlockHashingHandler.cs` | Hash-map of block contents for deduplication; owns `mapGpu` storage buffer + 65-uint coefficients UBO. |
| `src/Kova.Voxels/WorldBoundHandler.cs` | `NAADF/World/Data/WorldBoundHandler.cs` | Drives `bounds_calc.wgsl` (AADF generation). |
| `src/Kova.Voxels/ChangeHandler.cs` | `NAADF/World/Data/ChangeHandler.cs` | Tracks edited chunks, dispatches `world_change.wgsl`. |
| `src/Kova.Voxels/EntityHandler.cs` | `NAADF/World/Data/EntityHandler.cs` | Maintains `entityChunkInstancesGpu`/`entityVoxelDataGpu` storage buffers + `entity_update.wgsl`. |
| `src/Kova.Voxels/EntityData.cs` | `NAADF/World/Data/EntityData.cs` | Plain DTOs (instance, voxel data). |
| `src/Kova.Voxels/Generator/WorldGenerator.cs` | `NAADF/World/Generator/WorldGenerator.cs` | Abstract base. |
| `src/Kova.Voxels/Generator/WorldGeneratorModel.cs` | `NAADF/World/Generator/WorldGeneratorModel.cs` | Copies a `ModelData`-equivalent into the world via `generator_model.wgsl`. |
| `src/Kova.Voxels/Model/VoxelModelData.cs` | `NAADF/World/Model/ModelData.cs` | Runtime voxel model. **No** `Accord` palette clustering; **no** ZIP I/O — load from `.kvox` via `AssetLoader`. |
| `src/Kova.Voxels/Render/WorldRender.cs` | `NAADF/World/Render/WorldRender.cs` | Render dispatcher base. |
| `src/Kova.Voxels/Render/WorldRenderBase.cs` | `NAADF/World/Render/Versions/WorldRenderBase.cs` | "Base" pipeline (primary + GI + ReSTIR + denoise + TAA + final). Dispatcher only — owns no shaders. |
| `src/Kova.Voxels/Render/Atmosphere.cs` | `NAADF/World/Render/Atmosphere.cs` | XNA `Vector3` -> `System.Numerics.Vector3`; drop `using SharpDX.MediaFoundation;` and `using NAADF.Gui.Main.Debug;`. CPU-side atmosphere math. |
| `src/Kova.Voxels/Render/AtmosphereSettings.cs` | inline constants in `UiSkyDebug` (NAADF) | Plain DTO; sun direction, density, scattering coefs. No ImGui. |
| `src/Kova.Voxels/Render/VoxelType.cs` | spread across `VoxelTypeHandler.cs` | The 16-byte `uint4` packed material data uploaded to GPU as `voxelTypeData`. |
| `src/Kova.Voxels/VoxelTypeHandler.cs` | `NAADF/World/VoxelTypeHandler.cs` | Manages the `voxelTypeData` storage buffer; pure CPU dictionary keyed by material ID. |

`ChangeHandler`, `EntityHandler`, `WorldBoundHandler`, `BlockHashingHandler` all stay as distinct handler classes (not folded into `WorldData`). They're already separated in NAADF and reflect distinct GPU pipelines; collapsing them would muddy the shader-mapping. Per `01-context.md` scope decision: "Port `WorldData` / `EntityHandler` as handler-style classes first; Friflo ECS rework comes after the renderer is up."

### Constants migration: `VoxelSettings`

NAADF's chunk/block/voxel sizes are hardcoded across `WorldData.cs:60-68` ("`worldGenSegmentSizeInVoxels = worldGenSegmentSizeInChunks * 16`", "`sizeInBlocks = sizeInVoxels / 4`", "`sizeInChunks = sizeInBlocks / 4`", "`chunkVolumeInVoxels = 16 * 16 * 16`") and in the shaders as the `4` in `[numthreads(4,4,4)]` and the `16` in `chunkPos = curCell / 16`. These need a single source of truth in Kova.

Decision: **`const` fields on a static `VoxelSettings` class**, not DI. NAADF treats them as build-time constants; runtime configurability isn't a v1 need; ECS DI is deferred.

```csharp
// src/Kova.Voxels/VoxelSettings.cs
public static class VoxelSettings
{
    public const int BlocksPerChunkAxis  = 4;                                   // 4^3 blocks per chunk
    public const int VoxelsPerBlockAxis  = 4;                                   // 4^3 voxels per block
    public const int VoxelsPerChunkAxis  = BlocksPerChunkAxis * VoxelsPerBlockAxis; // 16
    public const int VoxelsPerChunk      = VoxelsPerChunkAxis
                                         * VoxelsPerChunkAxis
                                         * VoxelsPerChunkAxis; // 4096 — note NAADF uses 16^3=4096 not "2048"; "2048" in WorldData.cs:68 is voxels stored as packed uint16-pairs (4096/2)
    public const int BlocksPerChunk      = BlocksPerChunkAxis * BlocksPerChunkAxis * BlocksPerChunkAxis; // 64
    public const int VoxelsPerBlock      = VoxelsPerBlockAxis * VoxelsPerBlockAxis * VoxelsPerBlockAxis; // 64

    public const uint MaxBufferBytes     = 0xFFFF0000u; // matches DynamicStructuredBuffer.cs:31 cap
    public const int  GpuMaxElementsUint = 1024 * 1024 * 511; // matches WorldData.cs:49

    // Build flags (NAADF/Settings.cs:1-9):
    public const bool Entities = true;          // ENTITIES flag — affects chunks Texture3D format
    public const bool Hdr      = false;         // HDR flag

    public static Texture3DFormat ChunkTextureFormat =>
        Entities ? Texture3DFormat.Rg32Uint : Texture3DFormat.R32Uint;
}
```

WGSL mirrors these as const-expressions emitted into a per-shader prelude (see section 5). No `#include` machinery — Kova builds the prelude string in the platform's `LoadShaderAsync` wrapper before handing the WGSL to `CreateComputeShader`.

### WorldData rewrite delta (vs. NAADF/World/Data/WorldData.cs)

```csharp
// src/Kova.Voxels/WorldData.cs sketch
public sealed class WorldData : IDisposable
{
    readonly IGraphicsDevice _gpu;
    public XYZ ActualSizeInVoxels, SizeInVoxels, SizeInBlocks, SizeInChunks,
               SizeInQueueGroups, SizeInWorldGenSegments;
    public int ChunkCount, QueueGroupCount;
    public int WorldGenSegmentSizeInVoxels, WorldGenSegmentSizeInChunks;

    // Storage / 3D-texture handles (REPLACING DynamicStructuredBuffer + Texture3D)
    public StorageBufferHandle DataVoxelGpu;   // resized via ResizeStorageBuffer
    public StorageBufferHandle DataBlockGpu;
    public StorageBufferHandle BlockVoxelCountGpu;
    public Texture3DHandle     DataChunkGpu;
    public StorageBufferHandle SegmentVoxelBuffer;

    // CPU mirrors (unchanged shape)
    public uint[] DataVoxel, DataBlock, DataChunk;
    public readonly ConcurrentQueue<uint> FreeVoxelSlots = new();
    public readonly ConcurrentQueue<uint> FreeBlockSlots = new();

    // Handlers (unchanged ownership)
    public BlockHashingHandler BlockHashing;
    public WorldBoundHandler   Bounds;
    public EntityHandler       Entities;
    public ChangeHandler       Changes;
    // EditingHandler lives in Kova.Voxels.Editing; held by reference here

    public uint BlockCount, VoxelCount;
    readonly object _resizeLock = new();
    // ... methods Port 1:1 from NAADF/World/Data/WorldData.cs, replacing
    //     StructuredBuffer/Texture3D calls with the new IGraphicsDevice ones.
}
```

`AddVoxels`/`SetBlocks`/`SetChunk` translate without algorithmic changes — they touch CPU mirrors and call `ResizeStorageBuffer` (which returns a new handle the field must accept). The `RayTraversal` CPU debug method (NAADF/World/Data/WorldData.cs:396-473) ports trivially after swapping `Point3` for `XYZ` and `Microsoft.Xna.Framework.BoundingBox` for an inline AABB test.

### Atmosphere lift

Per the audit (`00-reuse-audit.md` line 102-104), `NAADF/World/Render/Atmosphere.cs` is a 1-line `using` swap. Decision on where atmosphere lives: **`Kova.Voxels.Render` namespace inside `Kova.Voxels`**, not a separate project. It's <200 lines of CPU code, no GPU resources beyond the precomputed atmosphere storage buffer (managed by `WorldRenderBase`), and tightly coupled to the renderer's sun-direction state. A separate project is gold-plating.

---

## 5. WGSL shader port plan (per-shader table)

### Layout under `src/Kova.App/Shaders/voxels/`

```
src/Kova.App/Shaders/voxels/
  prelude.wgsl                 (constants + small structs, prepended at load time)
  world/
    chunk_calc.wgsl            (replaces chunkCalc.fx)
    bounds_calc.wgsl           (replaces boundsCalc.fx)
    bounds_common.wgsl         (replaces boundsCommon.fxh — module-scope helpers; concatenated)
    entity_update.wgsl         (replaces entityUpdate.fx)
    map_copy.wgsl              (replaces mapCopy.fx)
    world_change.wgsl          (replaces worldChange.fx)
    generator_model.wgsl       (replaces world/generator/generatorModel.fx)
    type_mapping.wgsl          (replaces world/model/typeMapping.fx)
    data_copy.wgsl             (replaces top-level dataCopy.fx)
  render/
    ray_tracing.wgsl           (replaces rayTracing.fxh — pure-function module)
    atmosphere.wgsl            (replaces atmosphere/atmosphereRaw.fxh + atmospherePrecomputed.fxh)
    common.wgsl                (replaces render/common/* helpers)
    base/
      render_atmosphere.wgsl   (replaces base/renderAtmosphere.fx)
      render_first_hit.wgsl    (replaces base/renderFirstHit.fx)
      ray_queue_calc.wgsl      (replaces base/rayQueueCalc.fx)
      render_global_illum.wgsl (replaces base/renderGlobalIllum.fx)
      render_sample_refine.wgsl (replaces base/renderSampleRefine.fx)
      render_spatial_resampling.wgsl (replaces base/renderSpatialResampling.fx)
      render_taa_sample_reverse.wgsl (replaces base/renderTaaSampleReverse.fx)
      render_denoise_split.wgsl (replaces base/renderDenoiseSplit.fx)
      render_final.wgsl        (replaces base/renderFinal.fx — only VS+FS file)
```

Albedo and PathTracer versions are deferred — `01-context.md` scope only mentions "ray-traversal renderer with ReSTIR-style resampling + TAA + atmosphere" which is the Base version. Add `render/albedo/` / `render/path_tracer/` in a later phase if needed; not v1.

### Concatenation strategy

WGSL has no `#include`. Kova's `IPlatform.LoadShaderAsync` currently loads exactly one file (`WgpuPlatform.cs:39-45`). Extend with a new `IPlatform.LoadComputeShaderAsync(string name)` that:

1. Reads the named WGSL file under `Shaders/voxels/...`.
2. Walks a small explicit-deps table baked into the shader name (e.g. `"render/base/render_first_hit"` declares deps `["render/ray_tracing", "render/atmosphere", "render/common"]`).
3. Prepends `prelude.wgsl` + the deps in order, then the requested file.

The deps table lives in `src/Kova.App/Shaders/voxels/manifest.toml` shipped as `Content` (mirrors `Shaders/**` copy at `Kova.App.csproj:24-28`). Adding TOML at runtime requires nothing new — Tomlyn is already in `Kova.AssetPipeline.csproj:9` but not `Kova.App`; we'll do plain `string.Split` parsing of `manifest.toml` instead to avoid pulling Tomlyn into the app. **Implementation alternative if even that feels too much:** emit a single concatenated WGSL per logical shader at cook time via a new no-op `WgslConcatenator` step in `Kova.AssetPipeline` (TextureImporter-style). Either is fine; the implementer picks one — both have the same observable contract from the renderer's POV.

### Per-shader translation table

Legend for "HLSL feature -> WGSL workaround":

| HLSL feature | WGSL replacement |
|---|---|
| `RWStructuredBuffer<T>` | `var<storage, read_write> b : array<T>;` at `@group(g) @binding(b)` |
| `StructuredBuffer<T>` | `var<storage, read> b : array<T>;` |
| `RWTexture3D<uint>` | `var t : texture_storage_3d<r32uint, read_write>;` (entities-off) or `rg32uint` (entities-on) |
| `Texture3D<uint>` (read-only) | `texture_storage_3d<r32uint, read>` (WebGPU 2024 spec — supported in wgpu-native; for browser fallback bind as `read_write` and treat as read-only by convention) |
| `[numthreads(X,Y,Z)]` | `@compute @workgroup_size(X,Y,Z) fn name(...)` |
| `groupshared T x[N];` | `var<workgroup> x : array<T, N>;` |
| `GroupMemoryBarrierWithGroupSync` | `workgroupBarrier()` |
| `InterlockedAdd(buf[i], v)` | `atomicAdd(&buf[i], v)` (declaring buffer element as `atomic<u32>`) |
| `InterlockedAdd(buf[i], v, prev)` | `let prev = atomicAdd(&buf[i], v);` |
| `InterlockedCompareExchange(x, cmp, val, prev)` | `let prev = atomicCompareExchangeWeak(&x, cmp, val).old_value;` |
| `InterlockedOr(x, 0, prev)` | `let prev = atomicLoad(&x);` |
| `InterlockedExchange(x, val, prev)` | `let prev = atomicExchange(&x, val);` |
| `RWByteAddressBuffer .Store(off, val)` | bind a normal `array<u32>` storage buffer; `.Store(off,val)` -> `b[off/4u] = val;` |
| `cs_5_0` profile / `technique`+`pass` blocks | strip; one entry-point per `fn`. The C# side names the entry point at `CreateComputeShader`. |
| `Effect.Parameters["x"].SetValue(scalar)` | per-pass UBO struct; `WriteUniformBuffer<T>` on the C# side. |
| `Effect.Parameters["x"].SetValue(arrayOf128)` | UBO with `array<T, 128>` member; padding rules apply (vec3 must be vec4 in std140-like). |
| `rcp(x)` | `1.0 / x` |
| `mad(a,b,c)` | `fma(a,b,c)` or `a*b+c` |
| `step(a,b)`, `frac`, `trunc`, `sign`, `min`, `max`, `abs` | identical names in WGSL except `frac` -> `fract`, `trunc` -> `trunc`, `mad` -> `fma`. |
| `int3/uint3/float3` | `vec3<i32>/vec3<u32>/vec3<f32>` |
| `static uint x[16]` (function-local fixed array) | `var x : array<u32, 16u>;` inside the function (WGSL function-locals support arrays). |
| `IndirectDrawBuffer` write via `.Store` | normal storage buffer with `Indirect | Storage` usage; binding as storage in shaders, dispatching as indirect from C#. |
| Append/Consume buffers | **Not used in NAADF — verified by grep "append\|consume" returning zero.** No workaround needed. |
| Counter buffers (`counterResetValue`) | NAADF only uses `StructuredBufferType.Basic` in `DynamicStructuredBuffer.cs:18` default — no `counterResetValue` callers in the ported set. Drop. |

Atomic specifics: the buffers NAADF atomicly touches (`blockVoxelCount`, `hashMap[*].voxelPointer`, `boundQueueInfo[*].size`, `globalIlumSampleCounts`, `globalIlumBucketInfo`, `denoise` counters, `groupCount`) are declared in WGSL with their element type wrapped in `atomic<u32>`. Where the buffer holds a struct (e.g. `HashValue { voxelPointer, useCount, hashRaw }` in `chunkCalc.fx:15-20`), at least `voxelPointer` and `useCount` must be `atomic<u32>`; `hashRaw` is set then read non-atomically (a `workgroupBarrier()` after the write covers ordering). This needs careful translation in the implementer phase; the architect's call is: **keep the struct in WGSL but mark per-field atomicity** rather than splitting into two parallel buffers.

### Per-file translation tasks

| Source `.fx`/`.fxh` | Target WGSL | Specific HLSL constructs to translate |
|---|---|---|
| `Content/shaders/world/data/boundsCommon.fxh` | `voxels/world/bounds_common.wgsl` | `groupshared uint cachedCell[64];` -> `var<workgroup>`; `GroupMemoryBarrierWithGroupSync()` -> `workgroupBarrier()`; bit-shift mask helpers port unchanged. |
| `Content/shaders/world/data/chunkCalc.fx` | `voxels/world/chunk_calc.wgsl` | `RWTexture3D<CHUNKTYPE> chunks` (rg32uint or r32uint by `Entities`), `RWStructuredBuffer<uint> blocks/voxels/blockVoxelCount/gpuCpuSyncBuffer`, `RWStructuredBuffer<HashValue> hashMap` (with atomic `voxelPointer`+`useCount`), 4 entry points: `calcBlockFromRawData`, `chunkCopyToCpu`, `computeVoxelBounds`, `computeBlockBounds`. Heavy `InterlockedCompareExchange` + `InterlockedAdd` + `InterlockedOr` + spin loop at lines 88-92 — preserve the spin (`atomicLoad` in a `loop {}`); `[allow_uav_condition][loop]` becomes plain `loop` with `if (count >= 250) { break; }` since WGSL has no equivalent attribute. |
| `Content/shaders/world/data/boundsCalc.fx` | `voxels/world/bounds_calc.wgsl` | 3 entry points: `addInitialGroupsToBoundQueue`, `prepareGroupBounds`, `computeGroupBounds`. `RWByteAddressBuffer.Store(0, count)` at line 92 -> plain storage buffer write of `u32`. `groupshared bool anyBoundsIncrease` -> `var<workgroup> anyBoundsIncrease : atomic<u32>;` (WGSL workgroup bools are awkward; use u32). `InterlockedAdd(boundQueueInfo[..].size, 1, ...)` translates straight. |
| `Content/shaders/world/data/entityUpdate.fx` | `voxels/world/entity_update.wgsl` | Read NAADF; same construct set (atomics, structured buffers, no textures). |
| `Content/shaders/world/data/mapCopy.fx` | `voxels/world/map_copy.wgsl` | Plain copy; storage-buffer to storage-buffer. |
| `Content/shaders/world/data/worldChange.fx` | `voxels/world/world_change.wgsl` | Touches `chunks` Texture3D + voxel/block buffers; no atomics outside `blockVoxelCount`. |
| `Content/shaders/world/generator/generatorModel.fx` | `voxels/world/generator_model.wgsl` | Read source for specifics; structured buffer copy from model into world; no atomics. |
| `Content/shaders/world/model/typeMapping.fx` | `voxels/world/type_mapping.wgsl` | Maps file-loaded type indices to runtime indices. |
| `Content/shaders/dataCopy.fx` | `voxels/world/data_copy.wgsl` | Generic uint-buffer copy used by `App.helper.CopyFromStructuredBufferLarge` (NAADF/Common/Helper.cs); needed at WorldData GPU<->CPU sync points. |
| `Content/shaders/render/rayTracing.fxh` | `voxels/render/ray_tracing.wgsl` | `shootRay` translates verbatim — only built-ins differ (`rcp`->`1/`, `mad`->`fma`, `step`->`step` same, `frac`->`fract`, `trunc`->`trunc`). `static uint chunksWithEntities[16];` is a function-local; in WGSL becomes `var chunksWithEntities : array<u32, 16>;`. The `#ifdef ENTITIES` branches collapse to a WGSL `const ENTITIES : bool = true;` and `if (ENTITIES) { ... }` (the compiler dead-codes the constant branch — verified WGSL `if` on a const works in naga/wgpu). |
| `Content/shaders/render/common/common.fxh`, `commonRayTracing.fxh`, `commonOther.fxh`, `commonColorCompression.fxh`, `commonConstants.fxh`, `commonRenderPipeline.fxh`, `commonEntities.fxh` | folded into `voxels/render/common.wgsl` (with a couple split files if size demands) | helper functions, color packing, common constants. |
| `Content/shaders/render/common/atmosphere/atmosphereRaw.fxh`, `atmospherePrecomputed.fxh` | `voxels/render/atmosphere.wgsl` | analytic + precomputed-table sampling. |
| `Content/shaders/render/common/taa/commonTaa.fxh` | `voxels/render/taa_common.wgsl` | sampling helpers. |
| `Content/shaders/render/versions/base/renderAtmosphere.fx` | `voxels/render/base/render_atmosphere.wgsl` | `[numthreads(64,1,1)]` -> `@workgroup_size(64)`. RWStorage `uint3 atmosphereComp` is fine. |
| `Content/shaders/render/versions/base/renderFirstHit.fx` | `voxels/render/base/render_first_hit.wgsl` | one entry `calcFirstHit`, scalar+vec params (camPosInt, camPosFrac, screen dims, jitter) bundled into a UBO `struct FirstHitU { ... }`. `compressFirstHitData` helper ports unchanged. Reads atmosphere comp + chunks/blocks/voxels + voxelTypeData. Writes firstHitData/firstHitAbsorption/finalColor (all RWStorage). |
| `Content/shaders/render/versions/base/rayQueueCalc.fx` | `voxels/render/base/ray_queue_calc.wgsl` | two entry points: `calcRayQueue` (64-thread workgroup), `calcRayQueueStore` (1-thread workgroup that writes the indirect dispatch count via `groupCount.Store` — translates to plain storage-buffer write into a `Indirect|Storage` buffer). |
| `Content/shaders/render/versions/base/renderGlobalIllum.fx` | `voxels/render/base/render_global_illum.wgsl` | one entry `calcGlobalIlum`. Heavy: workgroup atomics on `sharedResCount`/`globalResCountValid/Invalid`, two `InterlockedAdd` into `globalIlumSampleCounts[3+accumIndex]`. Per-frame UBO with 128-entry `array<vec4f, 128>` for `taaOldCamPosFromCurCamInt[128]` and `taaJitterOld[128]` — WGSL `array<vec3<f32>, 128>` becomes `array<vec4<f32>, 128>` due to std140-style 16-byte alignment of array elements; the C# side packs xyz and ignores `.w`. |
| `Content/shaders/render/versions/base/renderSampleRefine.fx` | `voxels/render/base/render_sample_refine.wgsl` | 5 entry points: `clearBucketsAndCalcMask`, `computeValidHistory`, `countValidDataAndRefine`, `countInvalidData`, `refineBuckets`. Multiple `InterlockedAdd(buf[i].x, 1 << 6, oldBucketValue)` patterns — translate as `atomicAdd(&buf[i].x, 64u)` returning `u32`. The bucket entries' `.x` is the atomic field — declare buckets as `struct Bucket { x : atomic<u32>, y : u32 }`. `RWByteAddressBuffer globalIlumValidDispatch/InvalidDispatch/groupCount` -> plain storage buffer with `Indirect|Storage` usage. |
| `Content/shaders/render/versions/base/renderSpatialResampling.fx` | `voxels/render/base/render_spatial_resampling.wgsl` | one entry `calcSpatialResampling`. Function-heavy (`getBRDF`, `getTargetFunctionNew`, `sampleNeighbors`). No atomics. Pure dispatch with `pixelThreadGroupCount` invocations. Outputs to `finalColor` and `denoisePreprocessed`. |
| `Content/shaders/render/versions/base/renderTaaSampleReverse.fx` | `voxels/render/base/render_taa_sample_reverse.wgsl` | two entry points: `reprojectOldSamples`, `calcNewTaaSample`. 128-entry `taaOldCamPosFromCurCamInt`/`taaJitterOld` UBO same shape as global illum. Heavy reprojection math — translates straight; the `camMatrix`/`invCamMatrix` matrices are mat4x4f UBO members. |
| `Content/shaders/render/versions/base/renderDenoiseSplit.fx` | `voxels/render/base/render_denoise_split.wgsl` | two entry points: `calcDenoiseHorizontal`, `calcDenoiseVertical`. No atomics; bilateral filter. |
| `Content/shaders/render/versions/base/renderFinal.fx` | `voxels/render/base/render_final.wgsl` | **The only render-pipeline shader** (VS + FS) — not compute. Full-screen quad over `finalColor` + `firstHitData`. Translates as `@vertex fn vs_main` + `@fragment fn fs_main` and uses the existing `IGraphicsDevice.CreatePipeline` render path (not compute). One render pipeline; one bind group bound via... wait — current `IGraphicsDevice.CreatePipeline` doesn't accept bind-group layouts. **Decision:** extend `CreatePipeline` to optionally take `ReadOnlySpan<BindGroupLayoutHandle>`, and `Draw` to optionally take `ReadOnlySpan<BindGroupHandle>` to bind for that draw. Add overloads so the triangle path keeps compiling. New overloads in `IGraphicsDevice`:
```csharp
PipelineHandle CreatePipeline(
    ShaderHandle shader, VertexLayout layout,
    ReadOnlySpan<BindGroupLayoutHandle> bindGroupLayouts);
void DrawIndexed(...); // ← not needed v1; renderFinal is a full-screen triangle list with 3 verts
void Draw(PipelineHandle pipeline, BufferHandle buffer, int vertexCount,
          ReadOnlySpan<BindGroupHandle> bindGroups);
``` |

Albedo and PathTracer shaders are out of scope; they live in NAADF under `render/versions/albedo/` and `render/versions/pathTracer/` and translate later with the same recipe.

### Constructs that have no WGSL equivalent — workarounds

- **`[allow_uav_condition][loop]`** (`chunkCalc.fx:61`) — WGSL has no `[allow_uav_condition]`; `loop {}` with explicit `break` covers it. The semantic is "trust me, this loop terminates"; WGSL allows the loop unconditionally.
- **`#ifdef ENTITIES` build-time branching** — turn into `const ENTITIES : bool = true;` at module scope; constant-folding handles the rest. If a future build needs `false`, regenerate the shader text. The C# side already gates this via `VoxelSettings.Entities` so the prelude generator can emit either `true` or `false`.
- **HLSL effect-parameter array slots (`taaSampleCamTransform[128]` etc.)** — UBO size limits in WebGPU: a uniform buffer can be up to 64 KiB per binding in the minimum-supported limits. 128 mat4x4f = 8192 bytes; 128 vec4f = 2048 bytes. Within limits.
- **Workgroup-shared bool (`anyBoundsIncrease`)** (`boundsCalc.fx:34`) — use `var<workgroup> anyBoundsIncrease : atomic<u32>;` with 0/1 semantics and `atomicStore`/`atomicLoad`.
- **`uint hashCoefficients[65]`** (UBO array of u32 declared inside shader at `chunkCalc.fx:40`) — UBO arrays in WGSL must use 16-byte stride; declare `array<vec4<u32>, 17>` and index as `coeffs[i/4][i%4]` or `array<u32, 65>` inside an `@group(g) @binding(b) var<uniform>` — naga supports tight u32 arrays in uniform space on most targets but it's a portability risk. **Decision:** bind `hashCoefficients` as a **read-only storage buffer** not a UBO. 65 × 4 = 260 bytes; performance is irrelevant for a once-per-segment compute. This sidesteps the alignment headache.

---

## 6. Voxel file IO importers + new `.kvox` format spec

### Importers (registered into `ImporterRegistry`)

#### `Kova.AssetPipeline/Importers/VoxImporter.cs`

```csharp
public sealed class VoxImporterSettings { /* none — MagicaVoxel handles all dialect detection */ }

public sealed class VoxImporter : AssetImporter<VoxImporterSettings>
{
    static readonly string[] Exts = [".vox"];
    public override ReadOnlySpan<string> SupportedExtensions => Exts;
    protected override VoxImporterSettings CreateDefaultSettings(string _) => new();
    protected override CookResult Cook(AssetContext ctx, VoxImporterSettings s)
    {
        // Use Kova.Voxels.VoxFile.Import(ctx.SourcePath) -> VoxelData
        // (Auto-dispatches MagicaVoxel vs Voxlap by header)
        var data = VoxFile.Import(ctx.SourcePath);
        var out = Path.Combine(ctx.OutputDirectory,
            Path.GetFileNameWithoutExtension(ctx.SourcePath) + ".kvox");
        KvoxWriter.Write(out, data);
        return new CookResult { OutputFiles = [out], Success = true };
    }
}
```

#### `Kova.AssetPipeline/Importers/Vl32Importer.cs`

Same shape, `Exts = [".vl32"]`, dispatches via `Voxlap.Import` (and falls back to `VoxFile.Import` since `VoxFile` already multiplexes). Could fold into `VoxImporter` by adding `.vl32` to its extensions list. **Decision:** fold — one importer handles `.vox` and `.vl32`; both go through `VoxFile.Import`.

#### `Kova.AssetPipeline/Importers/CvoxImporter.cs`

`.cvox` is NAADF's runtime format (ZIP-of-buffers containing chunk/block/voxel + voxel-type table). It's the *cooked* output of NAADF, not source. Treating a `.cvox` as a source asset in Kova means we either:

- **Option A.** Re-cook it: parse the ZIP at cook time (existing C# `ZipArchive` + `MemoryMarshal.AsBytes` round-trip — see `NAADF/World/Model/ModelData.cs:194-220`), discard the chunk/block hierarchy, and re-emit `.kvox` (which we control). This drops the dependency on the legacy ZIP layout from any runtime code.
- **Option B.** Ship `.cvox` as-is (rename to `.kvox` byte-identical) — but then `.kvox` is yoked to NAADF's container format forever.

**Decision:** Option A. `CvoxImporter` ports the `ZipArchive` reader from `ModelData.Load` (`NAADF/World/Model/ModelData.cs:181-235`, minus the `Accord` clustering and the `App.worldHandler.voxelTypeHandler.ApplyVoxelType` runtime hook — replace with a passthrough that builds `Voxels.Material`/`VoxelType` from the on-disk bytes). The output is a `.kvox` with the same data laid out per the spec below.

`.obj`/`.stl` voxelization (NAADF's `obj2voxel.exe`) is **dropped** per `01-context.md:60`. AssimpNet's existing `.obj` import path remains as a *mesh* importer (`Kova.AssetPipeline/Importers/ModelImporter.cs`), not as voxelization.

#### Registration

In `src/Kova.AssetCompiler/Program.cs` (or wherever the registry is wired — currently the compiler is a stub), add:

```csharp
registry.Register(new VoxImporter());     // .vox + .vl32
registry.Register(new CvoxImporter());    // .cvox
```

### `.kvox` runtime container format

Mirrors `KtexWriter.Write` (`src/Kova.AssetPipeline/KtexWriter.cs:36-49`) and `ModelImporter.WriteMeshFile` (KMSH layout at `src/Kova.AssetPipeline/Importers/ModelImporter.cs:259-330`). All little-endian.

```
offset  size      field
------  --------  ---------------------------------------------------------
0       4         magic                "KVOX"
4       4         version              u32, currently 1
8       4         sizeX                u32 (voxels)
12      4         sizeY                u32 (voxels)
16      4         sizeZ                u32 (voxels)
20      4         materialCount        u32
24      4         chunkCount           u32 (= ceil(sizeX/16) * ceil(sizeY/16) * ceil(sizeZ/16))
28      4         blockCount           u32 (allocated)
32      4         voxelCount           u32 (allocated)
36      4         flags                u32 (bit0 = Entities-format chunks; bit1..31 reserved)
40      8         reserved             two u32 = 0
48      ...       materials            materialCount × MaterialEntry (40 bytes each, see below)
                                       (variable-length names are NOT in the entry — names follow)
...     ...       materialNames        materialCount × (u16 nameLen + UTF8 bytes)
...     ...       dataChunk            chunkCount × (4 bytes if entities-off, 8 bytes if entities-on)
...     ...       dataBlock            blockCount × 4 bytes
...     ...       dataVoxel            voxelCount × 4 bytes (packed pairs of uint16)
```

**`MaterialEntry`** (fixed 40 bytes; matches `Voxels.VoxelType` shape from `NAADF/World/VoxelTypeHandler.cs` + `Voxel.cs`):

```
0       12        colorBase            3 × f32
12      12        colorLayered         3 × f32
24      4         materialBase         u32 (cast from MaterialTypeBase enum)
28      4         materialLayer        u32 (cast from MaterialTypeLayer enum)
32      4         roughness            f32
36      4         reserved             u32 = 0
```

Per-importer cook output is a single `.kvox` file alongside an asset-pipeline manifest entry (existing machinery in `Kova.AssetPipeline/BuildManifest.cs`).

### Runtime loader

In `src/Kova.Assets/AssetLoader.cs`, add:

```csharp
public Kova.Voxels.VoxelModelData LoadVoxelModel(string relativePath, IGraphicsDevice gpu)
{
    var path = Path.Combine(_cookedDirectory, relativePath);
    // Read header, validate magic, then materials + chunk/block/voxel arrays.
    // Upload arrays via gpu.CreateStorageBuffer<uint>(span, ...) and gpu.CreateTexture3D(...).
}
```

`VoxelModelData` is the Kova.Voxels equivalent of NAADF's `ModelData` — read once at scene-load, pass to `WorldGeneratorModel.SetModel(modelData)`, then dispose after `WorldData.GenerateWorld` completes (mirroring `NAADF/World/Data/WorldData.cs:214-215`).

---

## 7. Editing tools + threading stance

### Project: `src/Kova.Voxels.Editing/Kova.Voxels.Editing.csproj`

References `Kova.Core`, `Kova.VoxelsCore`, `Kova.Voxels`. No new packages.

### Files (1:1 ports unless noted)

| File | Replaces (NAADF) | Port type |
|---|---|---|
| `EditingHandler.cs` | `NAADF/World/Data/EditingHandler.cs` | Mostly 1:1; drop `using NAADF.Gui` + ImGui references; drop input-from-`IO.KBStates` (rewire to `IInput` — section 8). |
| `EditingTool.cs` | `NAADF/World/Data/EditingTools/EditingTool.cs` | 1:1 — abstract base. |
| `EditingToolCube.cs` | `EditingToolCube.cs` | 1:1; replace `Microsoft.Xna.Framework.Vector3` -> `System.Numerics.Vector3`. |
| `EditingToolSphere.cs` | `EditingToolSphere.cs` | 1:1; same swap. |
| `EditingToolFloodFill.cs` | `EditingToolFloodFill.cs` | 1:1; same swap. |
| `EditingToolPaint.cs` | `EditingToolPaint.cs` | 1:1; same swap. |
| `EditingToolModel.cs` | `EditingToolModel.cs` | 1:1; depends on `VoxelModelData`. |

`Point` (`System.Drawing.Point`, used at NAADF/EditingHandler.cs:57) becomes a vec2 or `System.Numerics.Vector2`; `Point` from `Microsoft.Xna.Framework` becomes `XYZ` where the value is 3D-integer.

### Threading stance

NAADF uses:

- `_resizeLock` on `WorldData` for atomic buffer-resize-or-grow (`WorldData.cs:55, 309-322, 357-371`).
- `_editDataLock`, `_editProcessInternalLock`, `ReaderWriterLockSlim editLock` on `EditingHandler` (`EditingHandler.cs:30-33`).
- `ConcurrentQueue<uint>` for free-slot pools (`WorldData.cs:39`).
- `Interlocked.Add(ref voxelCount, 64)` (`WorldData.cs:303, 351`).

These exist because NAADF's editing tools update voxel state from worker threads while the main loop reads it for rendering. Per `01-context.md:57`: "Port `WorldData` / `EntityHandler` as handler-style classes first. Friflo ECS rework comes after the renderer is up."

**Decision for v1:**

- Keep `_resizeLock` (a plain `lock` on a `private readonly object _resizeLock = new();` field) — preserves NAADF semantics, requires no Kova-side abstraction.
- Keep `ConcurrentQueue<uint>` for `FreeVoxelSlots` / `FreeBlockSlots` — same primitive in BCL.
- Keep `Interlocked.Add` — same primitive in BCL.
- Keep `ReaderWriterLockSlim` in `EditingHandler` — preserves NAADF behavior.
- Do **not** invent a Kova-wide synchronization abstraction (no `IWorkScheduler`, no actor model). ECS migration will replace these later.

This stance is consistent with the orchestrator's directive (`01-context.md:57`) and avoids gold-plating.

---

## 8. Debug viewer scene + first-person camera

### Project: `src/Kova.Voxels.Viewer/Kova.Voxels.Viewer.csproj`

```xml
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <AllowUnsafeBlocks>true</AllowUnsafeBlocks>
  </PropertyGroup>
  <ItemGroup>
    <ProjectReference Include="..\Kova.Core\Kova.Core.csproj" />
    <ProjectReference Include="..\Kova.Voxels\Kova.Voxels.csproj" />
    <ProjectReference Include="..\Kova.Voxels.Editing\Kova.Voxels.Editing.csproj" />
    <ProjectReference Include="..\Kova.Assets\Kova.Assets.csproj" />
  </ItemGroup>
  <ItemGroup>
    <PackageReference Include="Silk.NET.Input" Version="2.23.0" />
  </ItemGroup>
</Project>
```

**Silk.NET.Input** is not yet referenced anywhere in Kova (confirmed: grep -rn "Silk.NET.Input" returns no matches). This is the v1 add. The same package version (`2.23.0`) matches existing Silk.NET refs in `Kova.Graphics.WebGPU.csproj:13-15`.

### Input abstraction

To keep input out of the graphics device while still letting browser builds work later, add a small `IInput` interface in `Kova.Core`:

`src/Kova.Core/Input/IInput.cs`:

```csharp
namespace Kova.Core.Input;

public interface IInput
{
    bool IsKeyDown(Key key);
    bool WasKeyPressed(Key key);          // edge: just-pressed this frame
    bool IsMouseButtonDown(MouseButton b);
    System.Numerics.Vector2 MousePosition { get; }
    System.Numerics.Vector2 MouseDelta { get; }     // since last poll
    int ScrollDelta { get; }
    void BeginFrame();                     // platform calls this to swap edge state
}

public enum Key { W, A, S, D, Space, ShiftLeft, ControlLeft, Escape, F1, ArrowLeft, ArrowRight /* extend as needed */ }
public enum MouseButton { Left, Right, Middle }
```

Browser builds get a stub that throws / returns false until a Silk-on-WASM input layer arrives. **Decision:** the v1 voxel viewer is **desktop-only** — `KovaPlatform.CreateAsync` already branches on backend (`KovaPlatform.cs:14-39`); browser falls back to `TriangleApp`. Acceptable per `01-context.md:54` (voxels are desktop-and-browser-webgpu — browser input layer is a v1.5 concern; in v1, browser displays the triangle).

`IPlatform` gains:

```csharp
IInput Input { get; }
```

`WgpuPlatform` (`src/Kova.Graphics.WebGPU/WgpuPlatform.cs`) creates a `SilkInput` wrapper around `_window.CreateInput()` (Silk.NET.Windowing exposes `IInputContext` via `WindowExtensions.CreateInput()`).

### `FirstPersonCamera`

`src/Kova.Voxels.Viewer/FirstPersonCamera.cs`:

```csharp
public sealed class FirstPersonCamera
{
    public Vector3 Position;
    public float Yaw, Pitch;
    public float FieldOfViewDegrees = 90f;
    public float NearPlane = 0.1f, FarPlane = 10_000f;
    public float MoveSpeed = 64f;            // voxels per second; Shift multiplies by SpeedBoostMultiplier
    public float SpeedBoostMultiplier = 8f;
    public float MouseSensitivity = 0.002f;

    public Matrix4x4 GetView()           { /* yaw/pitch -> forward; lookAt */ }
    public Matrix4x4 GetProjection(float aspect) { /* perspective */ }
    public Vector3 Forward            => ...; // for ray-cast tools

    public void Update(IInput input, float deltaSeconds, bool captureMouse) { ... }
}
```

NAADF's `Camera.cs` (`NAADF/Common/Camera.cs`) is too tangled with `Microsoft.Xna.Framework.Input` and `SharpDX.Direct3D9` to lift (`00-reuse-audit.md:80`). Hand-rewrite from scratch using `System.Numerics`. Controls match NAADF (`/mnt/archive4/DEV/NAADF/README.md:41-47`): WASD movement, Shift speed boost, Space up, Left-Ctrl down, Right-click captures mouse for rotation. Sun-inclination keys (Left/Right arrows) wire into the renderer's `AtmosphereSettings.SunDir` rather than the camera.

### `VoxelViewerApp`

`src/Kova.Voxels.Viewer/VoxelViewerApp.cs`:

```csharp
public sealed class VoxelViewerApp : GameApp
{
    readonly string _modelPath;
    FirstPersonCamera _camera = null!;
    WorldData _world = null!;
    WorldRenderBase _renderer = null!;

    public VoxelViewerApp(string modelPath) { _modelPath = modelPath; }

    public override async Task LoadAsync()
    {
        // 1. Load voxel model
        var loader = new AssetLoader(Path.Combine(AppContext.BaseDirectory, "CookedAssets"));
        var modelData = loader.LoadVoxelModel(_modelPath, GraphicsDevice);

        // 2. Create world sized to fit the model (or fixed 256^3 for v1 smoke test)
        var sizeInVoxels = new XYZ(256, 256, 256);
        _world = new WorldData(GraphicsDevice, sizeInVoxels, worldGenSegmentSizeInGroups: 2);

        // 3. Run GPU world generation
        var gen = new WorldGeneratorModel(GraphicsDevice, Platform);
        gen.SetModel(modelData);
        _world.GenerateWorld(gen);
        modelData.Dispose();

        // 4. Build the render dispatcher
        _renderer = new WorldRenderBase(GraphicsDevice, Platform);
        _renderer.CreateScreenTextures(ScreenWidth, ScreenHeight);

        // 5. Camera at a sane vantage point
        _camera = new FirstPersonCamera { Position = new Vector3(128, 200, -64), Pitch = -0.3f };
    }

    public override void Update()
    {
        var input = Platform.Input;
        _camera.Update(input, /*dt*/ 1f/60f, captureMouse: input.IsMouseButtonDown(MouseButton.Right));
        _world.Update(/*gameTime*/ 0);
    }

    public override void Render()
    {
        GraphicsDevice.BeginFrame();
        GraphicsDevice.Clear(0, 0, 0, 1);
        _renderer.Render(_world, sun: SunColor(), camera: _camera);
        GraphicsDevice.EndFrame();
    }
}
```

### Switching from triangle to voxel viewer

`src/Kova.App/Program.cs` becomes:

```csharp
var platform = await KovaPlatform.CreateAsync("Kova", 800, 600);

// Browser falls back to the triangle until input-layer work lands.
GameApp app =
#if BROWSER_WASM
    new TriangleApp();
#else
    args.Length > 0
        ? new VoxelViewerApp(args[0])   // dotnet run -- "voxels/oasis.kvox"
        : new TriangleApp();
#endif

await platform.RunAsync(app);
```

The viewer is opt-in via CLI argument to keep the triangle path runnable as a smoke test for the platform.

---

## 9. Build matrix changes (csproj, naga exclusion, package adds)

### `Kova.slnx` additions

New `<Project>` lines under `/src/`:

```xml
<Project Path="src/Kova.VoxelsCore/Kova.VoxelsCore.csproj" />
<Project Path="src/Kova.Voxels/Kova.Voxels.csproj" />
<Project Path="src/Kova.Voxels.Editing/Kova.Voxels.Editing.csproj" />
<Project Path="src/Kova.Voxels.Viewer/Kova.Voxels.Viewer.csproj" />
```

### Package adds

- `Silk.NET.Input` 2.23.0 in `Kova.Voxels.Viewer.csproj` (only project that needs it).

No other package additions. `Tomlyn`, `StbImageSharp`, `AssimpNet` are already in `Kova.AssetPipeline.csproj` (`src/Kova.AssetPipeline/Kova.AssetPipeline.csproj:9-12`) and serve the existing importers.

### Excluding voxel shaders from naga transpile

The naga step at `src/Kova.App/Kova.App.csproj:43-55` runs on `<WgslShaders Include="Shaders/*.wgsl" />`. **Top-level glob, not recursive** — so any shader under `Shaders/voxels/**` is already excluded. **No change needed**, as long as voxel shaders live exclusively under `Shaders/voxels/` (which the layout in section 5 enforces).

Verification: `naga` is invoked once per `@(WgslShaders)` item, and `Shaders/*.wgsl` does not glob `Shaders/voxels/world/chunk_calc.wgsl`. The `triangle.wgsl` at `Shaders/triangle.wgsl` is the only file currently matching. Confirmed by inspection of `src/Kova.App/Shaders/` (one file).

If a future voxel shader needs a non-voxel sibling shader at the top level, this is still fine; only the new voxel ones live under `voxels/`.

### Copying voxel shaders to the app bundle

`Kova.App.csproj:24-28` copies `Shaders/**` recursively at build time. The existing step covers voxel shaders without modification.

### `Kova.App.csproj` reference additions

```xml
<ProjectReference Include="..\Kova.Voxels.Viewer\Kova.Voxels.Viewer.csproj" />
```

Transitive references pull `Kova.Voxels`, `Kova.Voxels.Editing`, `Kova.VoxelsCore`, `Kova.Assets`.

### Removed dependencies (nice-to-have)

None. The current Kova csprojs are minimal; this port doesn't remove anything.

---

## 10. Phase ordering with exit criteria

The starting hypothesis (`01-context.md:105-115`) is sound. Adjusted ordering reorders **P1 before P0** because lifting `VoxelsCore` is a no-risk, no-build-impact step that unblocks the importer work and gives later phases concrete types. Otherwise the order matches. P3 splits into P3a/P3b to size the WorldData port honestly.

| # | Phase | Files created | Files modified | Exit criteria | Risk |
|---|---|---|---|---|---|
| **P1** | Lift `VoxelsCore` | `src/Kova.VoxelsCore/Kova.VoxelsCore.csproj` + 13 .cs files (XYZ, BoundsXYZ, Color, Material, Voxel, VoxelData, VoxelDataT, VoxelDataBytes, VoxelDataColors, MagicaVoxel, Voxlap, VoxFile, VoxelImport) | `Kova.slnx` adds project. | `dotnet build src/Kova.VoxelsCore/Kova.VoxelsCore.csproj` succeeds. New unit test: load `images/oasis.vox` and assert non-zero voxel count. | Low. |
| **P2** | Voxel-format importers + `.kvox` writer | `src/Kova.AssetPipeline/KvoxWriter.cs`, `src/Kova.AssetPipeline/Importers/VoxImporter.cs`, `src/Kova.AssetPipeline/Importers/CvoxImporter.cs` | `src/Kova.AssetPipeline/Kova.AssetPipeline.csproj` adds `ProjectReference` to `Kova.VoxelsCore`; `src/Kova.AssetCompiler/Program.cs` registers the new importers (or wherever registry is wired). | `dotnet run --project src/Kova.AssetCompiler -- --assets <path-to-oasis-vox> --out <tmp>` produces `<tmp>/<name>.kvox`. Round-trip test (`KvoxReader` reads back the header + data). | Low — pattern matches `TextureImporter`/`ModelImporter`. |
| **P0** | Compute extension to `IGraphicsDevice` | `src/Kova.Core/Graphics/Compute.cs` (new types), `src/Kova.Graphics.WebGPU/WgpuComputeImpl.cs` (extracted helpers — optional, keep in same file if cleaner); browser-impl additions in `BrowserGraphicsDevice.cs` + `WebGpuInterop.cs` + `Kova.Graphics.WebGPU.lib.module.js` | `IGraphicsDevice.cs` extended; `Handles.cs` extended (8 new handle types); `WgpuGraphicsDevice.cs` adds compute path + 3D-texture path; `WebGL2GraphicsDevice.cs` adds all new methods throwing `NotSupportedException`. | A new e2e smoke test in `Kova.App`: dispatches a 1×1×1 compute shader that writes `42` into a storage buffer, reads back, asserts `42`. Both desktop and browser-WebGPU paths pass. WebGL2 path throws `NotSupportedException` as expected (tested via a unit test that catches). | **High** — most of the unknowns live here. wgpu-native compute pass APIs in Silk.NET.WebGPU 2.23 must be exercised. Risk #1 in `00-reuse-audit.md`. |
| **P3a** | Port `WorldData` skeleton + `BlockHashingHandler` (no AADF, no rendering) | `src/Kova.Voxels/Kova.Voxels.csproj`, `VoxelSettings.cs`, `WorldData.cs`, `BlockHashingHandler.cs`, `ChangeHandler.cs` (stubs), `EntityHandler.cs` (stubs — entity logic deferred until P3b), `Render/Atmosphere.cs` (CPU), `Render/AtmosphereSettings.cs`, `VoxelTypeHandler.cs`, `Render/VoxelType.cs` | `Kova.slnx` adds project. | Unit test: create a 256³ `WorldData`, verify GPU buffers + 3D texture allocate and dispose without errors. No shader dispatch yet — `BlockHashingHandler.Initialize` just allocates `mapGpu`. | Medium — WebGPU device-limit gotchas surface here (3D texture size, total buffer count). |
| **P3b** | Wire `WorldData.GenerateWorld` to GPU world gen path | (no new files) `WorldGenerator.cs`, `WorldGeneratorModel.cs`, `Model/VoxelModelData.cs` plus the WGSL `generator_model.wgsl`, `chunk_calc.wgsl`, `bounds_common.wgsl` | `Kova.Voxels.csproj` extended; `WorldData.cs` `GenerateWorld` filled in. | E2E: `VoxelViewerApp.LoadAsync` runs `WorldData.GenerateWorld(WorldGeneratorModel(oasisModel))` and the call returns with non-zero `BlockCount` / `VoxelCount`. No rendering yet — verify via storage-buffer readback in a debug-only assertion. | High — the hash + atomic patterns at `chunkCalc.fx:117-181` are the trickiest WGSL translation in the port. |
| **P4** | Port AADF generation | (no new files) `WorldBoundHandler.cs`, `voxels/world/bounds_calc.wgsl`, `voxels/world/bounds_common.wgsl` | `WorldData.cs` calls `Bounds.Update()` from `Update()`. | E2E: `WorldData.Update(0)` runs without errors; readback the chunks `Texture3D` and assert that at least one chunk's bound bits transitioned from 0 to non-zero across N frames. | Medium — multi-pass indirect dispatch sequencing. |
| **P5** | Port GPU world generator (already partly in P3b) + entity update | (no new files unless `EntityHandler` was stubbed in P3a, then add `voxels/world/entity_update.wgsl`) | `EntityHandler.cs` fleshed out. | E2E: entity-bearing world (NAADF/Settings.cs `Entities=true`) generates without errors. | Low — entity update mirrors world gen path. |
| **P6a** | Port primary-ray + atmosphere + final pass (render the world without GI) | `src/Kova.Voxels/Render/WorldRender.cs`, `Render/WorldRenderBase.cs`, plus WGSL: `ray_tracing.wgsl`, `atmosphere.wgsl`, `common.wgsl`, `base/render_atmosphere.wgsl`, `base/render_first_hit.wgsl`, `base/render_final.wgsl` | `IGraphicsDevice.cs` gains the bind-group-aware `CreatePipeline`/`Draw` overloads for `render_final` (section 5 last row). | E2E: `VoxelViewerApp` renders a non-black image showing voxel surfaces with sky in the background. No GI; surface lit with direct sun only. Eyeball check + PSNR vs NAADF screenshot (stretch goal). | High — first frame on screen is the integration sentinel. |
| **P6b** | Port secondary rays (GI) + ReSTIR + denoise + TAA | (no new files) plus WGSL: `base/ray_queue_calc.wgsl`, `base/render_global_illum.wgsl`, `base/render_sample_refine.wgsl`, `base/render_spatial_resampling.wgsl`, `base/render_denoise_split.wgsl`, `base/render_taa_sample_reverse.wgsl`, `taa_common.wgsl` | `WorldRenderBase.cs` dispatches the full pipeline. | E2E: `VoxelViewerApp` shows GI + temporal accumulation visible across frames. | High — same complexity class as the AADF port. |
| **P7** | Port CPU editing tools + entity logic | `src/Kova.Voxels.Editing/Kova.Voxels.Editing.csproj` + EditingHandler + 5 EditingTool*.cs + EntityData.cs (if not yet) | `Kova.Voxels.csproj` adds reference; `WorldData.cs` exposes hook for `EditingHandler`. | E2E: in-viewer test (manual): place a 5³ cube of stone via `EditingToolCube` and confirm the next-frame render shows the change. | Medium. |
| **P8** | First-person camera + voxel-viewer scene | `src/Kova.Voxels.Viewer/Kova.Voxels.Viewer.csproj`, `FirstPersonCamera.cs`, `VoxelViewerApp.cs`, `src/Kova.Core/Input/IInput.cs` + `Key`, `MouseButton`, `src/Kova.Graphics.WebGPU/SilkInput.cs` (concrete impl) | `IPlatform.cs` gains `Input` property; `WgpuPlatform.cs` provides it; `BrowserWebGpuPlatform.cs` stub-implements. `Kova.App/Program.cs` switches on CLI arg. | E2E: `dotnet run --project src/Kova.App -- oasis.kvox` opens a window, mouse + WASD navigate, sun-inclination keys move the sun. | Low to medium — Silk.NET.Input integration. |

Phase exit criterion conventions:
- **"`dotnet build` succeeds"** applies cumulatively; every phase must build cleanly on top of the previous.
- E2E criteria require an actual run, not just compilation. Per the global CLAUDE.md rule: "Compilation alone proves nothing — always run end-to-end."

---

## 11. Risks + mitigations carried from audit

| # | Risk | Mitigation |
|---|---|---|
| R1 | **WebGPU 3D-texture size cap < world size.** WebGPU minimum spec is 2048³ for 3D textures, NAADF allocates a 3D texture sized to `sizeInChunks` (`NAADF/World/Data/WorldData.cs:82`). For a 16k³-voxel world that's 1024³ chunks — close to the cap. (`00-reuse-audit.md` risk #2.) | (a) v1 worlds are 256³–1024³ voxels (16³–64³ chunks) — comfortably under any cap. (b) Add a `WorldData` ctor assertion: `if (sizeInChunks.X > 2048) throw new NotSupportedException("Chunk volume exceeds WebGPU 3D texture limits.")` (c) If we later need larger worlds, swap `Texture3D` for a flat storage buffer indexed `[chunkIndex]` — costs a buffer load instead of a texture sample but lifts the cap. (d) The voxel-data storage buffers (`dataVoxelGpu`, `dataBlockGpu`) are already segmented at `MaxBufferBytes = 0xFFFF0000` (`VoxelSettings`), tracking NAADF's existing cap. |
| R2 | **WGSL atomics + workgroup memory portability.** WGSL atomics are u32/i32 only, and `atomicCompareExchangeWeak` is required (not `Strong`). (`00-reuse-audit.md` risk #3.) | All NAADF `Interlocked*` usages are u32; the `CompareExchange` pattern at `chunkCalc.fx:67` translates to `atomicCompareExchangeWeak` returning a struct with `.old_value` and `.exchanged`. Be ready for spurious failures (the "weak" suffix means it can fail without conflict). Wrapping in the existing spin-loop (`chunkCalc.fx:60-113`) tolerates this. |
| R3 | **No `RWByteAddressBuffer` / indirect-write fusion in WGSL.** `rayQueueCalc.fx:groupCount.Store(0, max(1,n))` writes a single u32 into a buffer that the same frame uses for `DispatchIndirect`. | WebGPU allows a buffer with `STORAGE | INDIRECT` usage to be written by a compute pass and then sourced by `dispatchWorkgroupsIndirect`. Wire the `IndirectBufferHandle` to alias a `StorageBufferHandle` internally (one WebGPU buffer, two C# handles to the same underlying pointer). |
| R4 | **`renderFinal.fx` needs bind-group on draw.** Existing `IGraphicsDevice.Draw` takes only pipeline + vertex buffer (`IGraphicsDevice.cs:11`). | Add the new `Draw(... ReadOnlySpan<BindGroupHandle>)` overload + `CreatePipeline(... bindGroupLayouts)` overload. Triangle path unaffected. |
| R5 | **Shader-source assembly fragility.** Hand-crafted concat order is brittle. | Either ship `manifest.toml` with explicit deps lists (chosen path) or pre-concatenate at cook time in `Kova.AssetPipeline`. Both options stated in section 5; implementer picks one and documents the choice. |
| R6 | **Silk.NET.Input on .NET 10 + Linux unverified.** Project uses Silk.NET 2.23.0 elsewhere; input subsystem is in the same package family. | Smoke test the package import in P8. If a runtime issue, fall back to a minimal Linux-keyboard reader via stdin or the X11 surface event hooks Silk already exposes (`IWindow.Input`). |
| R7 | **Browser builds lose voxel functionality.** Compute on browser-WebGPU works, but input/camera don't yet. (`01-context.md:54` decision pushes browser-WebGPU as a supported voxel target — but no browser input layer is planned in v1.) | v1 ships browser builds with `TriangleApp`. v1.1 adds a tiny `KeyboardJsInterop` + `PointerLockJsInterop` to wire browser input through `IInput`. Out of scope for this design. |

---

## 12. Open questions deferred to impl phase

These are decisions deliberately punted to the implementer. They are scoped tightly and will not propagate ambiguity into other phases.

1. **WGSL shader concat strategy** — `manifest.toml` runtime stitching vs. cook-time concatenation (section 5). Both meet the architectural contract; the implementer picks one and adds a brief comment-block in the chosen file.
2. **`ComputeShaderHandle` vs `ShaderHandle` dedupe** — whether to share one underlying `ShaderModule` when the same WGSL text is used for two entry points, or always create a new module. Both work; the dedupe is a perf-only concern that doesn't affect correctness.
3. **`renderFinal.wgsl` quad vs. triangle** — NAADF uses a 6-vertex quad mesh (`WorldRenderBase.cs:442-443`). WebGPU full-screen renders typically use a 3-vertex oversized triangle without a vertex buffer. Implementer picks whichever they want; no architectural impact.
4. **Where `worldGenSegmentSizeInGroups` is configured** — passed to `WorldData` ctor (current design) vs. on `VoxelSettings`. Defaulting to NAADF's value of `2` covers v1.
5. **Whether `EntityHandler` is stubbed empty during P3a or filled in P5** — both work; depends on impl pacing.
6. **CookedAssets vs. Assets layout** — the viewer references `Path.Combine(AppContext.BaseDirectory, "CookedAssets")`. The asset compiler currently writes wherever invoked; the implementer wires the output path to the app's runtime expectation either via msbuild copy or by adjusting the path constant.
7. **Naga build-time validation of voxel shaders** — currently no validator runs over voxel WGSL. Optional addition: a separate msbuild target invoking `naga --validate` (no transpile) on `Shaders/voxels/**` to catch syntax errors early. Optional, not gating.

---

## delegate-architect findings (2026-05-13)

Design written. Architectural decisions are concrete: 8 new handles, ~20 new `IGraphicsDevice` methods, 4 new csprojs (`Kova.VoxelsCore`, `Kova.Voxels`, `Kova.Voxels.Editing`, `Kova.Voxels.Viewer`), a `.kvox` runtime format spec mirroring `.ktex`/`.mesh`, per-file WGSL translation table for every NAADF shader in scope, and a sized 9-phase plan (P1 -> P2 -> P0 -> P3a -> P3b -> P4 -> P5 -> P6a -> P6b -> P7 -> P8) with exit criteria.
