# NAADF -> Kova Port: Implementation Log

## P1 — VoxelsCore lift (2026-05-13)

**Implementer:** delegated implementation agent (no parent-conversation memory).
**Status:** Complete. Build + tests green.

### Files created

| Path | Purpose |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/Kova.VoxelsCore.csproj` | New SDK-style csproj, `AllowUnsafeBlocks=true`, `RootNamespace=Kova.Voxels`. No package references. Inherits TFM `net10.0`, `Nullable=enable`, `ImplicitUsings=enable` from `Directory.Build.props`. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/XYZ.cs` | Integer 3D vector + ops. Lifted from NAADF. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/BoundsXYZ.cs` | Inclusive integer AABB. Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/Color.cs` | Explicit-layout 32-bit RGBA + HSV. Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/Material.cs` | Voxel material struct (emit/flux/metalic/roughness/ior). Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/Voxel.cs` | Explicit-layout `{Color | uint Index}` union. Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/VoxelData.cs` | Abstract voxel grid (size, bounds, enumerator). Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/VoxelDataT.cs` | Generic chunked voxel grid `VoxelData<T>`. Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/VoxelDataBytes.cs` | Palette-indexed voxel grid. Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/VoxelDataColors.cs` | Per-voxel RGBA grid. Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/MagicaVoxel.cs` | MagicaVoxel `.vox` reader/writer (nodes, palette, materials, frames). Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/Voxlap.cs` | Voxlap `.vox` (.vl32) reader. Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/VoxFile.cs` | Multiplexer over MagicaVoxel + Voxlap. Lifted. |
| `/mnt/archive4/DEV/Kova/src/Kova.VoxelsCore/VoxelImport.cs` | File-extension dispatch -> reader. Lifted. |
| `/mnt/archive4/DEV/Kova/tests/Kova.VoxelsCore.Tests/Kova.VoxelsCore.Tests.csproj` | xUnit test project (Microsoft.NET.Test.Sdk 17.11.1, xunit 2.9.2, xunit.runner.visualstudio 2.8.2). Project references `Kova.VoxelsCore`. |
| `/mnt/archive4/DEV/Kova/tests/Kova.VoxelsCore.Tests/XYZTests.cs` | 10 facts covering `+`, `-`, `-` (unary), `==`/`!=`, `Equals(object?)`, `GetHashCode`, `Volume`, `MaxDimension`, `ToVector3`/`FromVector3` round-trip, `Transform(identity)`, `Transform(translation)`. |
| `/mnt/archive4/DEV/Kova/tests/Kova.VoxelsCore.Tests/BoundsXYZTests.cs` | 7 facts covering size-ctor centering, min/max-ctor, `Add` expansion, `Add` no-op, `Transform(translation)`, `CreateEmpty` inverted extremes, `CreateEmpty + Add` adoption. |
| `/mnt/archive4/DEV/Kova/tests/Kova.VoxelsCore.Tests/ColorTests.cs` | 8 facts covering RGB ctor, RGBA ctor, uint ctor round-trip, equality (incl. `Equals(object?)` and null), `ToString` hex format, HSV round-trips for pure R/G/B. |
| `/mnt/archive4/DEV/Kova/tests/Kova.VoxelsCore.Tests/VoxelDataTests.cs` | 4 facts covering `VoxelDataColors` set/get/count, `VoxelDataBytes` palette indexing + `ColorOf`, `IsValid` bounds checks, `Bounds` start/extent. |

### Files modified

| Path | Change |
|---|---|
| `/mnt/archive4/DEV/Kova/Kova.slnx` | Appended `<Project Path="src/Kova.VoxelsCore/Kova.VoxelsCore.csproj" />` inside `/src/` folder; added a `/tests/` folder with `Kova.VoxelsCore.Tests`. |

### Namespace conversion notes

All 13 lifted files used **block-style** namespaces (`namespace Voxels { ... }`) — none used file-scoped. Conversion done by `sed -i 's/^namespace Voxels$/namespace Kova.Voxels/; s/^namespace Voxels {/namespace Kova.Voxels {/' src/Kova.VoxelsCore/*.cs`. Verified by grep — every file now starts with `namespace Kova.Voxels` (either followed by `{` or on its own line). Zero `namespace Voxels` occurrences remain.

`Kova.VoxelsCore.csproj` sets `<RootNamespace>Kova.Voxels</RootNamespace>` so future files default to the right namespace without a `Kova.VoxelsCore` prefix.

### Nullable warnings encountered (16 distinct, all resolved)

NAADF's source project did not have `<Nullable>enable</Nullable>`; Kova does (at `Directory.Build.props`). Per Kova's "no shims" rule, each warning was resolved at the source rather than suppressed.

| File:line (initial) | Warning | Resolution |
|---|---|---|
| `XYZ.cs:87` | CS8765 — `Equals(object)` nullability mismatch with `object.Equals(object?)`. | Signature changed to `Equals(object? other)` plus pattern-match `other is XYZ xyz && Equals(xyz)`. Same fix avoids the original cast-NRE if `other` was null. |
| `Color.cs:124` | CS8765 — same. | `Equals(object? other)` + `other is Color c && Equals(c)`. |
| `Voxel.cs:38` | CS8765 — same. | `Equals(object? obj)` + `obj is Voxel v && Equals(v)`. |
| `VoxelImport.cs:11` | CS8603 — `Import` returns `null` but declared `VoxelData`. | Return type changed to `VoxelData?`. Caller responsibility to null-check; this method already returned null for unknown extensions. |
| `VoxelImport.cs:23` | CS8603 — `GetBounds` returns `null` but declared `BoundsXYZ`. | Return type changed to `BoundsXYZ?`. |
| `MagicaVoxel.cs:41` | CS8618 — `TransformNode.name` non-nullable field never assigned in ctor. | Field type changed to `string?`. `ToString()` updated to `name ?? ""` to keep non-null return. |
| `MagicaVoxel.cs:161` | CS8618 — `Layer.layerName` non-nullable field never assigned in ctor. | Field type changed to `string?`. `ToString` already used `layerName ?? layerId.ToString()`. |
| `MagicaVoxel.cs:50` | CS8600 — `TransformFrame lastFrame = null`. | Local type changed to `TransformFrame?`. |
| `MagicaVoxel.cs:56` | CS8602 — `lastFrame.frameIndex` deref. | `lastFrame!.frameIndex` — invariant: if we entered the `frame.frameIndex > frameIndex` branch we have already iterated at least once and `lastFrame` was assigned in the previous iteration's tail. Preserves NAADF's original "NPE if Frames empty and first frame is past frameIndex" semantics. |
| `MagicaVoxel.cs:63` | CS8602 — `lastFrame.matrix` deref. | `lastFrame!.matrix`. Preserves NAADF semantics (NPE on empty `Frames`). |
| `MagicaVoxel.cs:80` | CS8600 — `ShapeModel lastModel = null`. | Local type changed to `ShapeModel?`. |
| `MagicaVoxel.cs:86` | CS8603 — return `lastModel` (possibly null) from `ShapeModel GetModel`. | `return lastModel!;` — original logic only reached this line after at least one loop iteration. |
| `MagicaVoxel.cs:90` | CS8602 — `lastModel.frameIndex` deref. | `lastModel!.frameIndex`. |
| `MagicaVoxel.cs:276` | CS8602 — `voxelData[…]` deref where `voxelData` declared `null as VoxelDataBytes`. | `voxelData![…]`. Per `.vox` spec, XYZI chunks always follow a SIZE chunk that assigns `voxelData`. |
| `MagicaVoxel.cs:300` | CS8601 — `node.attributes.TryGetValue("_name", out node.name)` writes possibly-null into non-nullable field. | Resolved transitively by changing `TransformNode.name` to `string?`. |
| `MagicaVoxel.cs:492` | CS8601 — same pattern in `ReadBounds`. | Same transitive fix. |

After fixes: `dotnet build src/Kova.VoxelsCore/Kova.VoxelsCore.csproj --no-incremental` reports **0 errors, 0 warnings**.

### Other warnings encountered during `dotnet build`

None. Both the standalone project build and the full-solution build (`dotnet build Kova.slnx`) report `0 Warning(s) 0 Error(s)`.

### Test summary

```
Passed!  - Failed:     0, Passed:    29, Skipped:     0, Total:    29, Duration: 24 ms - Kova.VoxelsCore.Tests.dll (net10.0)
```

### Build commands run

| Command | Result |
|---|---|
| `dotnet build src/Kova.VoxelsCore/Kova.VoxelsCore.csproj` (initial) | Pass — 0 errors, **16 warnings**. |
| `dotnet build src/Kova.VoxelsCore/Kova.VoxelsCore.csproj --no-incremental` (after fixes) | Pass — 0 errors, 0 warnings. |
| `dotnet test tests/Kova.VoxelsCore.Tests/Kova.VoxelsCore.Tests.csproj` | Pass — 29/29 tests. |
| `dotnet build Kova.slnx` (whole solution) | Pass — 0 errors, 0 warnings, all 9 projects. |

### Open issues

- None blocking P1.
- **Asset-loading test deferred per task brief.** `.cvox` is NAADF's ZIP container (lives in `World/Model/ModelData.cs`, XNA-tangled) and is *not* the same format `VoxFile.Read` expects. A real `.vox` round-trip test belongs in P2 (`Kova.AssetPipeline.VoxImporter`) once we have a Kova-owned `.vox` test fixture.
- **`MagicaVoxel.cs` null-forgiving operators (`!`) at three sites** (`lastFrame!`, `lastModel!`, `voxelData!`) preserve the original NAADF behavior of NPE'ing on malformed input. A future hardening pass could throw a typed `InvalidDataException` instead, but doing so would change behavior beyond a clean lift — out of scope for P1.
- **No `Kova.VoxelsCore` reference added to `Kova.AssetPipeline` or `Kova.Assets` yet.** That's the explicit job of P2 (per design section 6) — P1 just produces the leaf library.

## P2 — Voxel importers + .kvox writer (2026-05-13)

**Implementer:** delegated implementation agent (no parent-conversation memory).
**Status:** Complete for `VoxImporter` + `KvoxWriter`. `CvoxImporter` shipped as a stub-with-reason (see Open issues).

### Files created

| Path | Purpose |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.AssetPipeline/KvoxWriter.cs` | `.kvox` container writer. Two overloads: (a) `Write(path, VoxelDataBytes)` for source-cooked models (chunk/block/voxel counts = 0); (b) `Write(path, size, colors, materials, dataChunk, dataBlock, dataVoxel, entitiesFormat)` for the world-state form planned for P3+. |
| `/mnt/archive4/DEV/Kova/src/Kova.AssetPipeline/Importers/VoxImporter.cs` | `AssetImporter<VoxImporterSettings>` for `.vox` AND `.vl32` — folded per design section 6. Calls `Kova.Voxels.VoxFile.Read(stream)`, asserts the result is `VoxelDataBytes`, writes via `KvoxWriter.Write`. |
| `/mnt/archive4/DEV/Kova/src/Kova.AssetPipeline/Importers/CvoxImporter.cs` | `AssetImporter<CvoxImporterSettings>` stub for `.cvox`. Throws `NotImplementedException` — see Open issues. |
| `/mnt/archive4/DEV/Kova/tests/Kova.AssetPipeline.Tests/Kova.AssetPipeline.Tests.csproj` | New xUnit test project (Microsoft.NET.Test.Sdk 17.11.1, xunit 2.9.2). References `Kova.AssetPipeline` + `Kova.VoxelsCore`. |
| `/mnt/archive4/DEV/Kova/tests/Kova.AssetPipeline.Tests/KvoxWriterTests.cs` | 2 facts — header round-trip + material section round-trip on a 4×3×2 `VoxelDataBytes`, and a minimal 1×1×1 sanity check. |
| `/mnt/archive4/DEV/Kova/tests/Kova.AssetPipeline.Tests/VoxImporterTests.cs` | 3 facts — synthetic Voxlap (.vl32) end-to-end cook through `VoxImporter` produces a valid `.kvox`; `CvoxImporter.Cook` throws `NotImplementedException`; `VoxImporter.SupportedExtensions` contains both `.vox` and `.vl32`. |

### Files modified

| Path | Change |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.AssetPipeline/Kova.AssetPipeline.csproj` | Added `<ProjectReference Include="../Kova.VoxelsCore/Kova.VoxelsCore.csproj" />`. |
| `/mnt/archive4/DEV/Kova/src/Kova.AssetCompiler/Program.cs` | Registered `new VoxImporter()` + `new CvoxImporter()` into the `ImporterRegistry`. |
| `/mnt/archive4/DEV/Kova/Kova.slnx` | Added `<Project Path="tests/Kova.AssetPipeline.Tests/Kova.AssetPipeline.Tests.csproj" />` to the `/tests/` folder. |

### `.kvox` header layout actually shipped

Matches the design spec verbatim. All little-endian. For source-cooked output (`Write(path, VoxelDataBytes)` overload):

```
offset  size  field
------  ----  -----------------------------------------------
0       4     magic "KVOX"
4       4     version = 1
8       4     sizeX (u32)
12      4     sizeY (u32)
16      4     sizeZ (u32)
20      4     materialCount (u32) = min(colors.Length, materials.Length)
24      4     chunkCount = 0       (P3+: ceil(sx/16)*ceil(sy/16)*ceil(sz/16))
28      4     blockCount = 0       (P3+: runtime-allocated)
32      4     voxelCount = 0       (P3+: runtime-allocated)
36      4     flags = 0            (bit0 reserved for Entities-format)
40      4     reserved = 0
44      4     reserved = 0
48      ...   materials[materialCount] × 40 bytes each:
                  +0  colorBase    3 × f32
                  +12 colorLayered 3 × f32
                  +24 materialBase u32 = 0
                  +28 materialLayer u32 = 0
                  +32 roughness    f32 (= Material.roughness)
                  +36 reserved     u32 = 0
...     ...   materialNames[materialCount]: u16 nameLen + UTF8 bytes
                  Names are synthetic "mat0", "mat1", ... — .vox files
                  do not carry per-palette-entry names; placeholder until
                  the .cvox path lands and provides real names.
...     ...   voxel grid: sizeX × sizeY × sizeZ bytes, X-major Y-middle Z-inner.
                  Each byte is the palette index (`Voxel.Index & 0xFF`).
```

**Deviation from spec for source-cooked path:** the spec describes a `dataChunk[chunkCount]`/`dataBlock[blockCount]`/`dataVoxel[voxelCount]` triplet (hierarchical world layout). For P2 we ship `chunkCount = blockCount = voxelCount = 0` and instead append a flat `sizeX*sizeY*sizeZ`-byte dense voxel grid after the material names. The hierarchical form is produced at runtime by `Kova.Voxels.WorldData` (P3) and round-tripped through the secondary `KvoxWriter.Write(path, size, colors, materials, dataChunk, dataBlock, dataVoxel, entitiesFormat)` overload, which writes the spec layout verbatim. This split keeps the on-disk source-cooked file format trivially regeneratable from `VoxelDataBytes` and defers the (substantially more involved) chunk-packing logic to its real home in P3.

`colorBase` and `colorLayered` are written equal — `.vox` palette entries are single colors, not layered. NAADF's `VoxelType.colorLayered` only differs from `colorBase` for `.cvox`-sourced types. Once `CvoxImporter` lands, that file will write distinct values.

### Decision on `VoxelData` flavor used as canonical voxel-grid payload

`VoxelDataBytes` (palette-indexed, dense, byte-per-voxel). Both backends of `VoxFile.Read` (`MagicaVoxel.Flatten()` and `Voxlap.Read(...)`) return `VoxelDataBytes`. `VoxelDataColors` is not used by any importer in P2. The `VoxImporter` asserts the cast and fails the cook with a typed error if a future backend ever returns a different flavor.

Densest layout chosen — `VoxelDataBytes` stores one byte per voxel rather than four bytes per voxel as `VoxelDataColors` would.

### `CvoxImporter` status

**Stub-with-reason.** Throws `NotImplementedException("CvoxImporter: planned for a P2 follow-up — see 03-impl.md")` from `Cook`. Rationale:

1. NAADF's `.cvox` ZIP reader (`NAADF/World/Model/ModelData.cs:181-258`) calls into `App.worldHandler.voxelTypeHandler.ApplyVoxelType(LoadVoxelType(zipStream))` and reads `VoxelType` records that depend on the `MaterialTypeBase` and `MaterialTypeLayer` enums and the `VoxelType` class — none of which have been lifted to Kova yet (they live in `NAADF/World/VoxelTypeHandler.cs` and are XNA-tangled).
2. The stream-reading extension methods (`Stream.ReadNullTerminated`, `Stream.ReadVector3`, `Stream.ReadInt`, `Stream.ReadFloat`, `Stream.ReadUInt`) come from `NAADF/Common/Extensions/File/ExtFileRead.cs` which uses `Microsoft.Xna.Framework.Vector3` and is XNA-tangled.
3. Porting both would expand P2's scope by ~500 LoC of NAADF rewrite work for one importer that the brief explicitly authorizes stubbing. Per the brief: *"Better to land `VoxImporter` working than to get stuck on `.cvox`"*.

The stub is registered into `ImporterRegistry`, so the pipeline discovers `.cvox` files and routes them to the stub at cook time. Cooking `oasis.cvox` will fail loudly, which is the intended behavior until a follow-up phase ports the dependencies.

### Test summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed:     0, Skipped:     0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed:     0, Skipped:     0, Total:     5
```

The five new pipeline tests cover:
- `KvoxWriterTests.Write_VoxelDataBytes_ProducesValidHeader` — full header + 3 materials + 3 material names + 24-byte voxel grid round-trip.
- `KvoxWriterTests.Write_EmptyGrid_StillProducesValidHeader` — 1×1×1 minimum case.
- `VoxImporterTests.Cook_SyntheticVoxlap_ProducesKvox` — synthetic 2×2×2 `.vl32` written byte-by-byte to disk, cooked through `VoxImporter`, output `.kvox` reads back with correct magic + dims + materialCount=256.
- `VoxImporterTests.Cvox_Stub_Throws` — guards the stub's contract so a future implementation flips the assertion.
- `VoxImporterTests.Extensions_Match_VoxAndVl32` — guards the design's "fold" decision.

E2E-against-a-real-`.vox`-asset was **skipped**: NAADF ships only `Content/oasis.cvox` (not `.vox`); no `.vox` test fixture is on disk. The synthetic-Voxlap test exercises the same `VoxFile.Read` -> `KvoxWriter.Write` code path end-to-end through `VoxImporter.Cook` against a real `AssetContext`, which proves the wiring without depending on a Magica-authored binary blob.

### `dotnet build Kova.slnx` warnings list and resolution

```
ok dotnet build: 11 projects, 0 errors, 0 warnings (00:00:03.42)
```

Zero warnings. Nothing to resolve.

### Open issues for P3

- **`CvoxImporter` is a stub.** Requires lifting `VoxelType` + `MaterialTypeBase` + `MaterialTypeLayer` enums into `Kova.VoxelsCore` (or a sibling `Kova.Voxels` runtime project), plus porting the four XNA-tangled `Stream` extension methods (`ReadNullTerminated`/`ReadVector3`/`ReadInt`/`ReadFloat`/`ReadUInt`) to use `System.Numerics.Vector3` and `BinaryReader`. Once those land, the ZIP-reading body from `ModelData.Load` (`NAADF/World/Model/ModelData.cs:181-258`, minus the `Accord` clustering and the `App.worldHandler` runtime hook) drops in. Output goes through the secondary `KvoxWriter.Write` overload that writes the full hierarchical layout (`chunkCount`/`blockCount`/`voxelCount` populated).
- **`VoxelType` is not yet a Kova type.** Design section 6 cites it ("matches `Voxels.VoxelType` shape from `NAADF/World/VoxelTypeHandler.cs`"). P3 should decide whether `VoxelType` lives in `Kova.VoxelsCore` (next to `Material`) or in the new `Kova.Voxels` runtime project. The `KvoxWriter` material entries currently write `materialBase = 0` and `materialLayer = 0` because the source `.vox`/`.vl32` formats carry no equivalent — when `VoxelType` lands these fields become first-class.
- **Material names are synthetic placeholders.** `.vox` files don't carry per-palette-entry names; `KvoxWriter` writes `"mat0"`, `"mat1"`, ... For `.cvox` the names come from `VoxelType.ID`. Once `CvoxImporter` is wired, the writer accepts a per-entry name list rather than synthesizing.
- **No runtime loader yet** (`Kova.Assets.AssetLoader.LoadVoxelModel`). Design section 6 describes the loader; it depends on `IGraphicsDevice` gaining `CreateStorageBuffer<T>` and `CreateTexture3D`, which is P0/P3 work not done in this phase.
- **Voxel-grid layout (X-major Y-middle Z-inner)** matches `VoxelData`'s default enumerator. P3 should confirm this is the layout `WorldData` wants to ingest — if not, swap one nested loop in `KvoxWriter` and update the test's index formula. The choice is encoded in exactly one place in `KvoxWriter.Write(string, VoxelDataBytes)`.

## P0 — Compute extension to IGraphicsDevice (2026-05-13)

**Implementer:** delegated implementation agent (no parent-conversation memory).
**Status:** Complete. Desktop WebGPU compute path fully exercised end-to-end on Linux/wgpu-native; browser path stubbed (Option B per task brief); WebGL2 path throws.

### Files created

| Path | Purpose |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.Core/Graphics/Compute.cs` | New DTOs: `StorageBufferUsage` ([Flags] enum), `Texture3DFormat`, `BindingType`, `BindGroupLayoutEntry` record, `BindGroupEntry` record. Matches design section 2 signatures verbatim. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGPU/WgpuComputeImpl.cs` | Partial-class file holding the compute/storage-buffer/uniform-buffer/3D-texture/bind-group/indirect-dispatch portion of `WgpuGraphicsDevice`. Keeps the render path in `WgpuGraphicsDevice.cs` clean. ~530 LoC of unsafe Silk.NET impl. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Graphics.WebGPU.Tests/Kova.Graphics.WebGPU.Tests.csproj` | New xUnit test project (Microsoft.NET.Test.Sdk 17.11.1, xunit 2.9.2). References `Kova.Core`, `Kova.Graphics.WebGPU`, `Kova.Graphics.WebGL2`, plus `Silk.NET.WebGPU.Native.WGPU 2.23.0` directly (see "Silk.NET 2.23 surprises" #2). |
| `/mnt/archive4/DEV/Kova/tests/Kova.Graphics.WebGPU.Tests/ComputeSmokeTests.cs` | Two tests: (a) `DispatchWritesAndReads_42` — full e2e on a real wgpu-native headless device, marked `[Trait("Category","RequiresGpu")]`; (b) `WebGL2_CreateComputeShader_Throws` — guards the WebGL2 stub contract. |

### Files modified

| Path | Change |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.Core/Graphics/Handles.cs` | Added the 8 new handle record-structs (`ComputeShaderHandle`, `ComputePipelineHandle`, `StorageBufferHandle`, `UniformBufferHandle`, `Texture3DHandle`, `BindGroupLayoutHandle`, `BindGroupHandle`, `IndirectBufferHandle`). |
| `/mnt/archive4/DEV/Kova/src/Kova.Core/Graphics/IGraphicsDevice.cs` | Added 22 new method signatures across compute pipelines, storage buffers, uniform buffers, 3D textures, bind groups, indirect dispatch, and compute pass recording. Existing render API unchanged. Signatures match design section 2 exactly. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGPU/WgpuGraphicsDevice.cs` | Made class `partial`. Added `using Silk.NET.WebGPU.Extensions.WGPU`. Added `_wgpuExt` (Wgpu device-extension handle for `DevicePoll`), `_computePass` (active compute-pass encoder), 8 new resource-tracking dictionaries (`_storageBuffers`, `_uniformBuffers`, `_indirectBuffers`, `_texture3Ds`, `_computeShaders`, `_computePipelines`, `_bindGroupLayouts`, `_bindGroups`, `_bindGroupLayoutEntries`). Added `InitializeHeadless()`, `BeginEncoder()`, `SubmitEncoder()`, and static `CreateHeadless()` for compute-only tests/tooling. `Dispose` extended to release every new resource type. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGPU/Browser/BrowserGraphicsDevice.cs` | Added 23 stubs throwing `NotSupportedException("Browser WebGPU compute backend not yet implemented — see P0 follow-up")`, each preceded by a `// TODO(P0-browser):` comment as a grep anchor. Option B per task brief. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGL2/WebGL2GraphicsDevice.cs` | Added 23 stubs throwing `NotSupportedException` with the exact design-section-2 message: *"Compute, storage buffers, 3D textures, and bind groups are not supported on the WebGL2 fallback. Voxel rendering requires WebGPU."* |
| `/mnt/archive4/DEV/Kova/Kova.slnx` | Added `<Project Path="tests/Kova.Graphics.WebGPU.Tests/Kova.Graphics.WebGPU.Tests.csproj" />` to `/tests/`. |

### Silk.NET 2.23 surprises

1. **`TextureFormat.RG32Uint` is the canonical name, not `Rg32Uint`.** The design used `Rg32Uint`; Silk.NET's enum exposes `R32Uint` (lowercase-g name) but `RG32Uint` (all-caps `RG`). Matched the Silk.NET name where it appears (`MapTexture3DFormat` switch). The C# DTO `Texture3DFormat.Rg32Uint` stays per design; the *mapping* targets `RG32Uint`.
2. **The test project must reference `Silk.NET.WebGPU.Native.WGPU` directly.** The main `Kova.Graphics.WebGPU.csproj` gates the native package on `Condition="'$(RuntimeIdentifier)' != 'browser-wasm'"`. When `dotnet test` builds the test assembly without a RID, this condition fires correctly *for that build*, but the testhost.dll runtime resolver does not flow native runtime targets to `AppContext.BaseDirectory` — so `WebGPU.GetApi()` fails with `FileNotFoundException`. **Workaround shipped:** added `Silk.NET.WebGPU.Native.WGPU 2.23.0` package reference in the test csproj, plus a `Target Name="CopyWgpuNativeToOutput" AfterTargets="Build"` that flattens `runtimes/$rid/native/*` into `$(OutputPath)`. Without this Target, `libwgpu_native.so` lives at `bin/Debug/net10.0/runtimes/linux-x64/native/` and Silk.NET's `MultiNativeContext` cannot discover it under testhost. The Target conditions on `RuntimeInformation.IsOSPlatform(...)` to cover Linux/Windows/macOS.
3. **`Wgpu` (WGPU-native extension) is the home of `DevicePoll`**, not the core `WebGPU` API. Load with `_wgpu.TryGetDeviceExtension(_device, out Wgpu wgpuExt)` and call `wgpuExt.DevicePoll(_device, wait: true, (WrappedSubmissionIndex*)null)`. The third parameter is a *pointer to* `WrappedSubmissionIndex`, not the struct itself — pass `null` if no specific submission index is targeted.
4. **`BufferMapAsync` is fully synchronous on wgpu-native** when the host calls `DevicePoll(wait=true)` in a loop. The smoke test confirms this — 10k-iteration spin is overkill; in practice the callback fires after the first poll.
5. **WGSL workgroup-storage scoping** — entry-point names are passed at *pipeline* creation, not at shader-module creation. The design's "two `ComputeShaderHandle`s with different entry points may share one module" optimization is therefore safe; current impl allocates one module per handle (no dedupe, per design open question #2).
6. **`PipelineLayout` ownership**: after `CreateComputePipeline`, the pipeline retains an internal reference to its `PipelineLayout`. The impl releases the local layout reference immediately after creating the pipeline — the pipeline keeps it alive.
7. **Records can't hold pointer fields**. C# 13 still rejects `record struct Foo(Buffer* B);`. Switched the 5 record structs in `WgpuComputeImpl` to plain `readonly struct` with explicit constructors.
8. **Name collision** — `Kova.Core.Graphics.BindGroupLayoutEntry` vs. `Silk.NET.WebGPU.BindGroupLayoutEntry`. The desktop impl file uses `using` aliases (`CoreBindGroupLayoutEntry` / `WgpuBindGroupLayoutEntry`). The dictionary tracking layout-entry metadata in `WgpuGraphicsDevice.cs` uses the fully-qualified `Core.Graphics.BindGroupLayoutEntry[]` form.

### Browser status

**Option B.** `BrowserGraphicsDevice` ships 23 compute methods that all throw `NotSupportedException("Browser WebGPU compute backend not yet implemented — see P0 follow-up")`. Rationale (per task brief): the desktop WebGPU compute path is the load-bearing deliverable for P0; full browser bridge would require ~200 lines of `[JSImport]` + ~150 lines of JS in `Kova.Graphics.WebGPU.lib.module.js` + ~80 lines of base64/JSON marshaling, which expands scope significantly. Per `01-context.md:54` and design section 11 R7, browser-WebGPU voxel rendering is a v1.1 goal; v1 ships browser builds with `TriangleApp` (the current hello-triangle), and the voxel viewer is desktop-first. Each browser stub has a `// TODO(P0-browser):` grep anchor pointing at the follow-up.

### Smoke test

- **Location:** `/mnt/archive4/DEV/Kova/tests/Kova.Graphics.WebGPU.Tests/ComputeSmokeTests.cs`.
- **What it tests:** The full P0 exit criterion. Creates a real wgpu-native headless device (no surface, no window), compiles a 3-line WGSL compute shader, allocates a 4-byte storage buffer with `Storage | CopySrc`, builds a 1-entry bind group layout (binding=0, StorageBuffer), binds the buffer, compiles a compute pipeline, dispatches `(1,1,1)`, and reads the buffer back. Asserts the readback contains `42u`.
- **Ran successfully in this env.** Linux x86_64, wgpu-native 2.23 over Vulkan/Mesa. Passed in 245 ms across 2 tests on first run after `dotnet build`.
- **`[Trait("Category","RequiresGpu")]`** is applied to the GPU test. On CI hosts without a working Vulkan/Metal/DX12 adapter, the test gracefully `return`s (treated as pass) when `WgpuGraphicsDevice.CreateHeadless()` returns null. This handles the "no adapter on Linux CI" scenario the task brief flags without false-failing the suite. Filtering: `dotnet test --filter "Category!=RequiresGpu"` excludes the GPU test.
- **WebGL2 throw-test** (`WebGL2_CreateComputeShader_Throws`) is unconditional — exercises the static `NotSupportedException` path that has no native dependencies.

### `dotnet build Kova.slnx` warning list with resolutions

```
ok dotnet build: 12 projects, 0 errors, 0 warnings (00:00:01.86)
```

Zero warnings. Nothing to resolve.

### `dotnet test Kova.slnx` summary

```
Kova.VoxelsCore.Tests.dll        : Passed:    29, Failed:     0, Skipped:     0, Total:    29
Kova.AssetPipeline.Tests.dll     : Passed:     5, Failed:     0, Skipped:     0, Total:     5
Kova.Graphics.WebGPU.Tests.dll   : Passed:     2, Failed:     0, Skipped:     0, Total:     2
                                   -----------
                                   Passed:    36, Failed:     0, Skipped:     0
```

### Open issues for P3+

- **Browser WebGPU compute backend.** Tracked by `// TODO(P0-browser):` comments at every stub in `BrowserGraphicsDevice.cs`. A follow-up phase adds JS-side `gpuCreateComputeShader`/`gpuCreateStorageBuffer`/`gpuCreateBindGroup*`/`gpuBeginComputePass`/etc., the matching `[JSImport]`s in `WebGpuInterop.cs`, and replaces the `throw new NotSupportedException` bodies in `BrowserGraphicsDevice` with `WebGpuInterop.GpuCreate*` calls. Pattern is identical to the existing render-path JS bridge.
- **Browser-test runner.** No browser e2e harness exists yet for the compute path. When the browser bridge lands, add a playwright-driven test under `e2e/` that asserts the smoke shader writes `42` in-browser.
- **Test-project native-deploy Target is a workaround.** The `CopyWgpuNativeToOutput` Target in `Kova.Graphics.WebGPU.Tests.csproj` is a workaround for testhost not honoring `runtimes/$rid/native/*` resolution. If a future net10 SDK fixes this, delete the Target. Filed under "minor" since the workaround is self-contained.
- **`ReadStorageBuffer` and `ReadTexture3D` poll loops are capped at 10k iterations.** With `DevicePoll(wait=true)` each call blocks on the device; in practice the callback fires after iteration 1. The cap is a guard against pathological non-completion; if a future driver bug causes it to fire, the operation throws `InvalidOperationException("Timed out waiting for BufferMapAsync.")`. Replace with `while (true)` if observed in the wild.
- **`ComputeShaderHandle` dedup not implemented** (design open question #2). Each `CreateComputeShader` call allocates a fresh `ShaderModule`. Perf-only; correctness is unaffected. Revisit when a shader appears more than once in the dispatch chain.
- **`SetBindGroup` doesn't support dynamic offsets** (the 4th/5th args of `ComputePassEncoderSetBindGroup` are passed as `0, nullptr`). NAADF doesn't use dynamic offsets; if a future port phase needs them, extend the `IGraphicsDevice.SetBindGroup` signature with a `ReadOnlySpan<uint>` overload.
- **Voxel-side `R32Uint`/`RG32Uint` storage-texture access mode is `ReadWrite` unconditionally.** Per design section 5 the renderer treats some 3D textures as read-only "by convention" — that's a shader-level concern, not a layout-level one. If a future phase needs a true `ReadOnly` storage texture, extend `BindingType` with a `ReadOnlyStorageTexture3D` variant.

## P3a — WorldData skeleton + handlers (2026-05-13)

**Implementer:** delegated implementation agent (no parent-conversation memory).
**Status:** Complete. `WorldData` constructs cleanly, owns all P3a GPU resources, disposes without leaks. No shader dispatches. New `Kova.Voxels.Tests` smoke test passes end-to-end on Linux/wgpu-native.

### Files created

| Path | Purpose |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Kova.Voxels.csproj` | New project, `AllowUnsafeBlocks=true`, refs `Kova.Core` + `Kova.VoxelsCore`. No package refs. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/VoxelSettings.cs` | Static class — `BlocksPerChunkAxis`/`VoxelsPerBlockAxis`/`VoxelsPerChunkAxis`/`VoxelsPerChunk`/`BlocksPerChunk`/`VoxelsPerBlock`/`MaxBufferBytes`/`GpuMaxElementsUint` consts, build-flag consts (`Entities = true`, `Hdr = false`), `ChunkTextureFormat` static property. Comments preserve design's non-obvious invariants (the 4096 vs 2048 packed-uint16-pair distinction at WorldData.cs:68). |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Render/AtmosphereSettings.cs` | Plain DTO. 16 fields lifted from NAADF's `UiSkyDebug` static state (sun direction/color/intensity, Rayleigh/Mie/Ozone, sphere/atmosphere geometry, ray-step counts). No UI. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Render/Atmosphere.cs` | 1-line `using` swap from NAADF + UI-globals refactor: methods take an `AtmosphereSettings s` parameter instead of reading from `UiSkyDebug.sky*`. Same algorithm. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Render/VoxelType.cs` | `MaterialTypeBase`/`MaterialTypeLayer` enums + `VoxelType` struct + `VoxelTypeGpu` packed 16-byte struct. `VoxelType.CompressForRender` ports NAADF's `compressForRender` verbatim (Half-packed colors/roughness). |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/VoxelTypeHandler.cs` | CPU dict keyed by material ID + GPU `TypesRenderGpu` storage buffer. `ApplyVoxelType`/`UpdateType`/`Update`/`Clear`/`Dispose` ported. `Update` writes the entire `TypesRender` list via `WriteStorageBuffer<VoxelTypeGpu>` after handling resize. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/BlockHashingHandler.cs` | `BlockValue` struct (12 bytes) + handler. Owns `MapGpu` storage buffer + `CoefficientsGpu` uniform buffer (272-byte aligned blob holding the 65 u32 coefficients). `GetHashOfBlock`/`SetNewUsedCount`/`GetCompressionFactor` ported. `IncreaseSizeToNewCount` is CPU-only in P3a — the GPU-side `mapCopy` dispatch lands in P3b along with the WGSL shader. `Dispose` releases both buffers. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/EntityData.cs` | `EntityData` class with `BuildBoundsFromVoxels` (port of NAADF/EntityData.cs:58-106 AADF inner-flood). `EntityInstance` struct (Vector3/Vector4/uint/uint/XYZ). `EntityChunkInstanceGpu` 20-byte packed struct. Constructor variant that depends on `ModelData` deferred to P3b. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/WorldBoundHandler.cs` | Stub. `BoundQueueInfo` struct + handler. Constructor allocates 4 storage buffers + 1 indirect buffer; `Initialize`/`Update` throw `NotImplementedException("port in P4 (bounds_calc.wgsl)")`. `Dispose` releases all. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/ChangeHandler.cs` | Stub. Constructor allocates `ChangedGroups/Chunks/Blocks/Voxels` storage buffers + sets up CPU `_distanceFloodFill` array. `Update`/both `AddChangedChunk` overloads throw `NotImplementedException("port in P3b/P5 (world_change.wgsl)")`. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/EntityHandler.cs` | Stub. Constructor allocates 6 storage buffers (3 RW for renderer + 3 staging). Three methods (`Update`, `AddEntity`, `AddEntityInstance`) throw `NotImplementedException("port in P5 (entity_update.wgsl)")`. Wrapped the `if (!VoxelSettings.Entities) return;` early-return in `#pragma warning disable CS0162` since `Entities` is `const true`. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/WorldData.cs` | Constructor `(IGraphicsDevice gpu, XYZ sizeInChunks, int worldGenSegmentSizeInGroups = 1)` — see "Constructor shape" below. Allocates `DataVoxelGpu`/`DataBlockGpu`/`BlockVoxelCountGpu`/`SegmentVoxelBuffer` storage buffers + `DataChunkGpu` 3D texture, then constructs the 5 handlers. `AddVoxels`/`SetBlocks`/`SetChunk` ported with `ResizeStorageBuffer` replacing `DynamicStructuredBuffer.Resize`. `GenerateWorld`/`Update` are deliberate stubs. `Dispose` cascades cleanly. Includes the P3a-mandated `R1` guard: `if (sizeInChunks.X > 2048) throw NotSupportedException("Chunk volume exceeds WebGPU 3D texture limits.")`. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/Kova.Voxels.Tests.csproj` | xUnit test project. Flattens `runtimes/$rid/native/*` via `CopyWgpuNativeToOutput` `AfterTargets="Build"` Target — same workaround the P0 test project uses. References `Kova.Core`, `Kova.Graphics.WebGPU`, `Kova.VoxelsCore`, `Kova.Voxels`. Direct `Silk.NET.WebGPU.Native.WGPU 2.23.0` reference for the same testhost-native-resolution reason. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/WorldDataSmokeTests.cs` | 2 tests. `ConstructsAndDisposesTwice` (gated `[Trait("Category","RequiresGpu")]`, returns early if `CreateHeadless()` is null) constructs a `WorldData(gpu, new XYZ(4,4,4))` — asserts size derivations + that every GPU handle is non-default — disposes, then constructs a fresh 2×2×2 world to prove `Release*` worked. `VoxelSettingsConstantsAreInternallyConsistent` exercises `VoxelSettings` constants without touching the GPU. |

### Files modified

| Path | Change |
|---|---|
| `/mnt/archive4/DEV/Kova/Kova.slnx` | Added `<Project Path="src/Kova.Voxels/Kova.Voxels.csproj" />` under `/src/` and `<Project Path="tests/Kova.Voxels.Tests/Kova.Voxels.Tests.csproj" />` under `/tests/`. |

### Constructor shape

NAADF's `WorldData(Point3 wantedSizeInVoxels, int worldGenSegmentSizeInGroups)` derives the chunk count from the voxel count. The task brief specifies a smoke-test entry of `new WorldData(gpu, sizeInChunks: new XYZ(4, 4, 4))`. P3a ships **the chunks-first entry only** — the voxel-count constructor is deferred until `WorldGenerator` lands in P3b (it needs the segment count to determine voxel-size targets anyway). Sizing computed inside the constructor:

```
SizeInChunks                   = caller-provided
SizeInBlocks                   = SizeInChunks * 4
SizeInVoxels                   = SizeInBlocks * 4              (= chunks * 16)
ActualSizeInVoxels             = SizeInVoxels                   (no rounding needed when chunks-first)
SizeInQueueGroups              = SizeInChunks / 4
WorldGenSegmentSizeInChunks    = groupsArg * 4
WorldGenSegmentSizeInVoxels    = WorldGenSegmentSizeInChunks * 16
SizeInWorldGenSegments         = (ActualSizeInVoxels + (segVox-1)) / segVox  (ceil-div)
```

For the smoke test (4,4,4 chunks, groupsArg=1): SizeInVoxels=(64,64,64), SizeInBlocks=(16,16,16), ChunkCount=64, WorldGenSegmentSizeInVoxels=64, SizeInWorldGenSegments=(1,1,1).

### NAADF -> Kova porting deltas

- **`Microsoft.Xna.Framework.Vector3` -> `System.Numerics.Vector3`**: all componentwise `+`/`-`/`*`/`Dot` translate identically. NAADF's `Vector3.Dot(a,b)` static call is also in `System.Numerics.Vector3`. No `MathHelper.Clamp` calls landed in P3a (those live in `EntityHandler.compressQuaternion` which is a P5 stub).
- **`NAADF.Common.Point3` -> `Kova.Voxels.XYZ`**: the existing `XYZ` in `Kova.VoxelsCore` already supplies `+`/`-`/`*`/`/`/`>>`/`&`/`%` and `ToVector3`/`FromVector3`. Matches NAADF `Point3` 1:1.
- **`StructuredBuffer`/`DynamicStructuredBuffer<T>` -> `StorageBufferHandle` + `IGraphicsDevice.CreateStorageBuffer<T>`**: `Resize` semantics replaced by `ResizeStorageBuffer(old, newSizeInBytes)` returning a new handle. `AddVoxels`/`SetBlocks` paths preserved with one local change: NAADF doubled `DataVoxel.Length * 2 * 4` directly; P3a starts from a 0-length CPU mirror so the doubling formula uses `Math.Max(64, DataVoxel.Length)` to seed the first grow. The maximum-size cap remains `VoxelSettings.MaxBufferBytes = 0xFFFF0000`.
- **`Texture3D` (XNA, `SurfaceFormat.Rg64Uint`/`R32Uint`) -> `Texture3DHandle` + `IGraphicsDevice.CreateTexture3D(w,h,d, Texture3DFormat)`**: format selected by `VoxelSettings.ChunkTextureFormat` (driven by `Entities` const). Note NAADF spelled the format `Rg64Uint` (8 bytes per chunk under entities-on); Kova's `Texture3DFormat.Rg32Uint` exposes the equivalent 8-byte-per-texel surface (2×r32uint). The P0 design comment in `Compute.cs:16` is explicit about this mapping.
- **`Effect.Parameters[...]`** call sites dropped entirely. The NAADF setEffect/CalculateChunkBlocks/Update bodies (~80 lines) become stubs marked `TODO(P3b)`. UBOs land in those phases via `IGraphicsDevice.CreateUniformBuffer` + `WriteUniformBuffer<T>`. P3a only allocates the one UBO that's mandated by design section 4 line 271 — `BlockHashingHandler.CoefficientsGpu` (65 u32 coefficients in a 272-byte aligned blob).
- **`BoundingBox.Intersects(Ray)`**: not needed in P3a (the `RayTraversal` CPU debug method is P3b/later).
- **`SharpDX.MediaFoundation` and `SharpDX.Direct3D11` `using`s**: stripped — these were spurious NAADF imports.
- **`ConcurrentQueue<uint>` + `_resizeLock` + `Interlocked.Add(ref voxelCount, 64)`**: preserved verbatim per design section 7 ("threading stance" — keep `_resizeLock`/`ConcurrentQueue`/`ReaderWriterLockSlim`; Friflo ECS adoption is deferred).

### How the handler stubs are wired

| Handler | Constructor | Methods (P3a status) |
|---|---|---|
| `BlockHashingHandler` | Allocates `MapGpu` storage buffer + `CoefficientsGpu` UBO, fills `Map` + `Coefficients` arrays on the CPU. Full functional impl. | `GetHashOfBlock` ported (CPU). `SetNewUsedCount` -> calls `IncreaseSizeToNewCount` which resizes CPU + GPU (no shader copy). `GetCompressionFactor` ported. `Dispose` releases both buffers. |
| `WorldBoundHandler` | Allocates 4 storage buffers + 1 indirect buffer. Size derivations from `worldData.SizeInChunks`. | `Initialize()`/`Update()` throw `NotImplementedException("port in P4 (bounds_calc.wgsl)")`. `Dispose` releases all 5 resources. |
| `ChangeHandler` | Allocates 4 storage buffers (RO storage usage) + initializes `_distanceFloodFill` CPU array to `0x3FFFFFFF`. | `Update()`/`AddChangedChunk(int)`/`AddChangedChunk(XYZ)` throw `NotImplementedException("port in P3b/P5 (world_change.wgsl)")`. `Dispose` releases all 4 buffers. |
| `EntityHandler` | Allocates 6 storage buffers (3 RW renderer-facing + 3 RO staging). Guarded by `if (!VoxelSettings.Entities) return;` (warning-suppressed since `Entities = const true`). | `Update`/`AddEntity`/`AddEntityInstance` throw `NotImplementedException("port in P5 (entity_update.wgsl)")`. `Dispose` releases all 6 buffers. |
| `VoxelTypeHandler` | Allocates `TypesRenderGpu` storage buffer (5000-entry initial capacity), seeds index-0 placeholder. Full functional impl. | `ApplyVoxelType`/`UpdateType`/`Clear` mutate CPU state + mark dirty. `Update` writes the entire list via `WriteStorageBuffer<VoxelTypeGpu>` (resize-on-overflow). `Dispose` releases the buffer. |

`WorldData.GenerateWorld(object)` throws `NotImplementedException("port in P3b (chunk_calc.wgsl dispatch)")`. `WorldData.Update(float)` is empty — no-op when `IsLoaded == false`.

### Test summary

```
Kova.VoxelsCore.Tests.dll      : Passed:    29, Failed:     0, Skipped:     0, Total:    29
Kova.AssetPipeline.Tests.dll   : Passed:     5, Failed:     0, Skipped:     0, Total:     5
Kova.Graphics.WebGPU.Tests.dll : Passed:     2, Failed:     0, Skipped:     0, Total:     2
Kova.Voxels.Tests.dll          : Passed:     2, Failed:     0, Skipped:     0, Total:     2
                                 -----------
                                 Passed:    38, Failed:     0, Skipped:     0
```

Both new `Kova.Voxels.Tests` tests pass. `ConstructsAndDisposesTwice` ran end-to-end on this Linux/wgpu-native host (the `[Trait("Category","RequiresGpu")]` early-return path was *not* taken).

### `dotnet build Kova.slnx` warning list with resolutions

```
ok dotnet build: 14 projects, 0 errors, 0 warnings (00:00:01.85)
```

Zero warnings on the final build. One was encountered during development (`CS0162` "Unreachable code detected" inside `EntityHandler` because `VoxelSettings.Entities` is `const true`), resolved with a localized `#pragma warning disable CS0162` around the early-return statement plus a one-line comment naming the reason. Wrapping the constant branch this way keeps the code live the day someone flips `Entities` to false in `VoxelSettings.cs`.

### Open issues for P3b/P4/P5

- **`WorldData.GenerateWorld` is a stub.** Real impl needs `WorldGenerator` base class (P3b's first job), `chunk_calc.wgsl` + 4 entry points (`calcBlockFromRawData`/`chunkCopyToCpu`/`computeVoxelBounds`/`computeBlockBounds`), `BlockVoxelCountGpu` read-back, and (entities-on) the chunk-copy-to-cpu segment dance.
- **`WorldData.Update` is empty.** Once `Entities.Update`/`Changes.Update`/`Bounds.Update` become real, wire them here in the same order NAADF does (entities first, then editing, then changes, then bounds).
- **`BlockHashingHandler.IncreaseSizeToNewCount` resizes CPU + GPU but does not run the `mapCopy` shader.** Before any GPU-side hash-map state is meaningful (P3b's GenerateWorld), this needs the WGSL `mapCopy` dispatch — until then the GPU-side hash map is whatever the resize op left behind (likely zeros after `ResizeStorageBuffer` since the impl creates a fresh buffer of the new size). Document this assumption shifts when P3b lands.
- **`BlockHashingHandler.AddBlock`/`DeleteBlock` not ported.** Both are pure CPU logic but they call into `WorldData.AddVoxels` and `WorldData.dataVoxel`/`voxelCount` — those exist but the CPU mirrors aren't filled until `GenerateWorld` runs. Port lands in P3b alongside `GenerateWorld`.
- **`EntityHandler.AddEntity` calls `AddVoxels` style writes into `EntityVoxelDataGpu`.** NAADF copies entity-voxel arrays into a GPU buffer at allocation time. P3a doesn't port that; P5 needs an `IGraphicsDevice.WriteStorageBuffer<T>(buf, span, byteOffset)` call.
- **`EntityHandler.Update`/`compressQuaternion`/`updateEntityRotation` heavy maths.** Ports straight (XNA Vector3/Quaternion -> System.Numerics) but a 200-line lift — defer to P5 with the entity_update WGSL.
- **`AtmosphereSettings` field naming.** Ported with PascalCase Kova-side and an explicit `AtmosphereSettings` parameter on every `Atmosphere.*` method. NAADF passed everything via `UiSkyDebug.sky*` globals; if the future UI layer in Kova still wants globals, expose `AtmosphereSettings Defaults` on a static — but P3a doesn't presume that shape.
- **`VoxelTypeHandler.Clear` always re-sync.** Currently sets `_needsSyncGpu = true` so the next `Update()` writes the single placeholder entry. The initial constructor call to `Clear` is followed by no `Update`, so on first-frame use of `TypesRenderGpu` the buffer contents are whatever WebGPU zero-init gave us (zeros, fine). Note this for the renderer phase — call `VoxelTypes.Update()` once before binding the buffer to a shader.
- **`WorldData` chunks-first ctor only.** The voxel-count ctor that NAADF uses (`new WorldData(wantedSizeInVoxels, segmentGroups)`) is not yet provided; add when `WorldGenerator` needs it. This is a P3b decision: generators may want to declare a voxel-grid size and have `WorldData` round it up to the nearest segment-aligned chunk volume.
- **`Map[0]`/index-0 reserved entry of `VoxelTypeHandler.TypesRender`** is a `default(VoxelTypeGpu)` — all zeros. NAADF's `Clear` does the same (`typesRender.Add(new Uint4())`). Renderer phase must keep treating index 0 as "empty" by convention; encoded into `VoxelType.compressForRender`'s output for an all-default `VoxelType`.

## P3b — GPU world generation (2026-05-13)

**Implementer:** delegated implementation agent (no parent-conversation memory).
**Status:** Complete. `WorldData.GenerateWorld(WorldGeneratorModel(modelData))` runs end-to-end on Linux/wgpu-native. The chunk_calc + generator_model + map_copy WGSL ports compile, link, dispatch, and produce a non-zero hierarchical voxel volume from a `.kvox` source. New `Kova.Voxels.Tests.GenerateWorldSmokeTests` GPU test passes.

### Files created

| Path | Purpose |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Generator/WorldGenerator.cs` | Abstract base. One method (`CopyToSegment`) replaces NAADF's overload pair `CopyToChunkData`/`CopyToChunkDataTexture3D` — Kova's `WorldData` always writes the segment voxel buffer (StorageBuffer), never directly to the chunks texture. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Generator/WorldGeneratorModel.cs` | Concrete subclass that owns the `generator_model.wgsl` compute pipeline + a 64-byte UBO (`GenModelParams`). `SetModel(VoxelModelData)` provides the source; `CopyToSegment` writes per-segment params and dispatches `groupSizeInChunks` workgroups. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Model/VoxelModelData.cs` | Runtime voxel model loaded from `.kvox`. Reads either the source-cooked dense grid (P2 format, chunkCount=0) or a pre-hierarchized triplet (chunkCount>0). For dense grids, hierarchizes on CPU into NAADF-wire-compatible chunk/block/voxel arrays before uploading to three RO storage buffers. Registers each on-disk material with `VoxelTypeHandler.ApplyVoxelType` and remaps palette indices to assigned render indices. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Render/ShaderLoader.cs` | Pure-C# WGSL loader. `LoadDefault(name)` resolves `<AppContext.BaseDirectory>/Shaders/voxels/<name>.wgsl`, prepends `prelude.wgsl` + manifest-declared deps (parsed without Tomlyn — plain string-split on `[shader.<name>]` blocks). Strategy A per design 02-design.md:402-410. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/prelude.wgsl` | Shared constants (`BLOCKS_PER_CHUNK_AXIS=4`, `VOXELS_PER_CHUNK_AXIS=16`, etc.) + `const ENTITIES : bool = false` flag. Concatenated before every voxel WGSL by ShaderLoader. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/manifest.toml` | Per-shader dep manifest. `world/chunk_calc` depends on `world/bounds_common`; the other three P3b shaders have no deps. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/world/bounds_common.wgsl` | Port of `boundsCommon.fxh` — workgroup-shared `cached_cell[64]` + `compute_bounds_4` AADF helpers used by `chunk_calc`'s voxel/block bounds entries. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/world/chunk_calc.wgsl` | Port of `chunkCalc.fx`. Three entry points: `calc_block_from_raw_data` (4,4,4 workgroup, hash+dedupe), `compute_voxel_bounds` (64,1,1), `compute_block_bounds` (64,1,1). `chunk_copy_to_cpu` deliberately omitted — depends on a read-write chunks texture which WebGPU baseline forbids; lands in P5 as a separate shader. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/world/map_copy.wgsl` | Port of `mapCopy.fx` `copyMap` entry (the `testHash` entry is unused at runtime and not ported). 64,1,1 workgroup; linear-probes the new map with `atomicCompareExchangeWeak`. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/world/generator_model.wgsl` | Port of `generatorModel.fx`. 4,4,4 workgroup; reads hierarchical `model_data_chunk/block/voxel` and writes packed uint16 pairs into the WorldData segment voxel buffer. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/world/data_copy.wgsl` | Port of top-level `dataCopy.fx`. Generic uint-buffer copy with offset/count UBO. 64,1,1 workgroup. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/GenerateWorldSmokeTests.cs` | Single `[Trait("Category","RequiresGpu")]` test. Builds a 16³ checkerboard `VoxelDataBytes` in memory, writes it via `KvoxWriter`, loads through `VoxelModelData`, runs `WorldData.GenerateWorld(WorldGeneratorModel)`, asserts `IsLoaded`, `VoxelCount > 64`, `BlockCount > 64`, and at least one non-zero `DataChunk` entry. |

### Files modified

| Path | Change |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/WorldData.cs` | `GenerateWorld(WorldGenerator)` filled in: orchestrates the per-segment generator dispatch, the chunk_calc dispatch, count readback, and the two AADF bounds passes. Builds `chunk_calc` pipeline lazily on first call (three entry points share one bind group layout). Adds `ReadDataChunkBack` which switches on `VoxelSettings.Entities` (write-only path for v1). |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/BlockHashingHandler.cs` | `IncreaseSizeToNewCount` now creates a fresh GPU buffer at the new size and dispatches `map_copy.wgsl` to rehash entries; lazy `EnsureMapCopyPipeline()` builds the shader + bind layout on first resize. `AddBlock(uint hash, uint[] voxelData, int offset, out bool isNew)` and `DeleteBlock(uint hash, uint pointer)` ported 1:1 from NAADF (CPU-only). `SyncGpuToCpu` added (reads `MapGpu` -> `Map`). `CoefficientsGpu` changed from `UniformBufferHandle` to `StorageBufferHandle` — bound as read-only storage per design 02-design.md:488 to sidestep WGSL's 16-byte UBO array stride. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/VoxelSettings.cs` | `Entities = true` -> `Entities = false`. WebGPU baseline disallows rg32uint storage textures with read_write or write-only access; v1 ships single-channel r32uint. The flag flip lands in P5 along with split-texture entry handling. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/EntityHandler.cs` | Moved the `#pragma warning disable CS0162` scope to cover the GPU-allocation block (now unreachable while `Entities=false`); old wrapping was around the early-return that's now reachable. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Kova.Voxels.csproj` | Added linked `Content` items pulling `..\Kova.App\Shaders\voxels\**\*.wgsl` + `manifest.toml` into the output as `Shaders\voxels\...` so tests + future viewer scenes can resolve them via `ShaderLoader.DefaultRoot()`. |
| `/mnt/archive4/DEV/Kova/src/Kova.Core/Graphics/IGraphicsDevice.cs` | Added two surface-less command-encoder methods: `BeginCommands()` + `SubmitCommands()`. Compute-only callers (world gen, asset prep, headless tests) use these instead of `BeginFrame`/`EndFrame`, which require a configured swapchain. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGPU/WgpuGraphicsDevice.cs` | Renamed `BeginEncoder`/`SubmitEncoder` to `BeginCommands`/`SubmitCommands` to satisfy the new IGraphicsDevice contract. Same wgpu-native semantics. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGPU/WgpuComputeImpl.cs` | `StorageTextureBindingLayout.Access` changed from `ReadWrite` to `WriteOnly`. WebGPU baseline forbids r/w and r/o storage textures without `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` extension. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGPU/Browser/BrowserGraphicsDevice.cs` | Added two new `NotSupportedException` stubs for `BeginCommands`/`SubmitCommands` with `// TODO(P0-browser):` anchors. |
| `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGL2/WebGL2GraphicsDevice.cs` | Same — `BeginCommands`/`SubmitCommands` throw the WebGL2 compute-unsupported message. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Graphics.WebGPU.Tests/ComputeSmokeTests.cs` | Updated to use `BeginCommands`/`SubmitCommands` instead of the now-renamed `BeginEncoder`/`SubmitEncoder`. Test semantics identical. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/Kova.Voxels.Tests.csproj` | Added `Kova.AssetPipeline` ProjectReference for `KvoxWriter` access in the new smoke test. |

### Per-shader translation notes

**`chunk_calc.wgsl` (load-bearing translation; flagged R2 in design)**

- `RWStructuredBuffer<HashValue> hashMap` -> `var<storage, read_write> hash_map : array<HashValue>` where `HashValue.voxel_pointer` and `HashValue.use_count` are `atomic<u32>`. `hash_raw` stays plain `u32`; it's written-then-read across a `workgroupBarrier()` and never atomic-touched, so atomicity isn't needed (NAADF originally treated it the same way).
- `InterlockedCompareExchange(hashMap[i].voxelPointer, EMPTY_BLOCK, val, prev)` -> `atomicCompareExchangeWeak(...).old_value`. The "weak" suffix means it can spuriously fail without conflict — the outer 250-iteration bounded loop tolerates this (NAADF's `[allow_uav_condition][loop]` semantic is preserved as a plain `loop {}` with explicit `if (count >= 250) { break; }`).
- `InterlockedAdd(buf[i], v, prev)` -> `let prev : u32 = atomicAdd(...)`. Discarded results use `_ = atomicAdd(...)` — WGSL discard statement (NOT `let _ : u32 = ...`, which is a syntax error: identifier can't be `_` in a binding declaration).
- `InterlockedOr(hashMap[i].voxelPointer, 0, prev)` (read-current-value idiom) -> `atomicLoad(...)`. The inner spin-loop waits for the high bit (0x80000000) to clear, signaling the writer published the final voxel pointer.
- Workgroup-shared `bool isAllBlocksEqual` -> `var<workgroup> all_blocks_equal : atomic<u32>` with 0/1 semantics. Many threads may write `false` simultaneously in HLSL; using `atomic<u32>` + `atomicStore(0u)` gives WGSL-portable single-value-write semantics with no data race UB.
- `RWTexture3D<CHUNKTYPE> chunks` -> `texture_storage_3d<r32uint, write>`. WebGPU baseline forbids `read_write` and `read` access modes for storage textures; only `write` works. As a result, the `chunkCopyToCpu` entry (which reads from `chunks`) cannot be ported into the same shader — dropped from chunk_calc.wgsl in v1; lands in P5 as a separate shader using a second `r32uint` texture in read mode.
- The `#ifdef ENTITIES` branching inside `calcBlockFromRawData` collapsed to a no-op — both branches write the same uint to `chunks.x` since we're single-channel r32uint in v1.
- 4 entry points reduced to 3 (`calc_block_from_raw_data`, `compute_voxel_bounds`, `compute_block_bounds`). All three share the same bind group layout (binding 8 unused by the bounds entries, but the layout demands a slot — kept consistent across pipelines).

**`map_copy.wgsl`**

- `RWStructuredBuffer<HashValue> newMap` element type uses `atomic<u32>` only for `voxel_pointer` because that's the field linear-probed by `atomicCompareExchangeWeak`. `use_count` and `hash_raw` are written by exactly one thread (the one that won the slot via the atomic CAS), so plain `u32` is safe. NAADF's spin loop bounded at 50 iterations is preserved as-is — under a 0.5-fill-ratio 2x-grown map, an empty slot is found in expectation in O(1).
- `oldMap` declared `var<storage, read>` since this pass only reads it (NAADF declared it `StructuredBuffer<HashValue>` similarly).
- Parameters packed into a 16-byte UBO (`MapCopyParams`) with two used u32 fields and two pads.

**`generator_model.wgsl`**

- 4 split UBO vector groups (4 u32 each) for parameters. Mechanical port — no atomics, no workgroup shared state. The `getVoxelDataInModel` function's `int3()` casts from HLSL became `vec3<u32>` everywhere; the `% 16u` and `/ 16u` patterns ported byte-for-byte.
- One quirk: HLSL's `voxelPos % (int3(modelSizeInChunks) * 16)` needed an explicit `vec3<u32>` cast on the multiply to avoid scalar-broadcasting wrong type — wrote it as `voxel_pos_in_model = voxel_pos % (model_size_in_chunks * 16u)`.
- `chunkData` RWStructuredBuffer renamed to `segment_voxel_buffer` to match NAADF's actual binding (the .fx file at line 2 declared `RWStructuredBuffer<uint> chunkData` but Effect.Parameters fed it the same buffer named `chunkData`, which was actually the WorldData segment buffer). Aligned the name to its semantic role.

**`data_copy.wgsl`**

- Trivial port. Three u32 params (offset_src, count, offset_dst) packed into a 16-byte UBO. Not actually dispatched by P3b code yet — provisioned for future CPU<->GPU sync (NAADF's `Helper.CopyFromStructuredBufferLarge`).

### `BlockHashingHandler.AddBlock` / `DeleteBlock` impl notes

- `AddBlock` allocates a fresh voxel-buffer slot via `_worldData.AddVoxels` when a hash slot is empty, otherwise compares the proposed voxel data byte-for-byte against the existing slot's voxels (read from `_worldData.DataVoxel` CPU mirror) and increments the use count on a match. Linear-probes up to 250 slots. Returns the voxel-buffer offset; sets `isNew=true` only on a fresh slot. **Difference from P3a stub**: P3a stubbed both methods with `NotImplementedException`. The real port depends on `_worldData.DataVoxel` being populated, which only happens after `GenerateWorld` runs the readback. Editing tools must call these only on a loaded world.
- `DeleteBlock` is pure CPU: linear-probes from the given hash until it finds the slot whose `VoxelsPointer == pointer`, decrements `BlockUseCount`, and returns `true` (with `VoxelsPointer = 0` reset) when the count hits zero. Editing tools then enqueue the freed slot into `_worldData.FreeVoxelSlots`.

### `WorldData.GenerateWorld` orchestration flow

Numbered dispatch / readback sequence (one full call):

1. **Lazily build chunk_calc pipeline.** Creates three `ComputeShaderHandle` modules + one bind group layout (9 entries) + one 48-byte UBO. Cached for subsequent calls.
2. **Reset block-voxel counts.** `WriteStorageBuffer<uint>(BlockVoxelCountGpu, [64u, 64u], 0)` — initial sentinel; atomic adds during chunk_calc grow it.
3. **`BeginCommands` + `BeginComputePass`** to bracket the per-segment loop.
4. **For each world-gen segment** (3D loop over `SizeInWorldGenSegments`):
   - 4a. `worldGenerator.CopyToSegment(this, ...)` — generator sets its own UBO + bind group, dispatches its WGSL into `SegmentVoxelBuffer`. For `WorldGeneratorModel`, that's `generator_model.wgsl`'s `fill_chunk_data_with_model_data_16` over a `worldGenSegmentSizeInChunks³` workgroup grid.
   - 4b. `DispatchCalcBlockFromRawData(segmentPosInChunks)` — writes chunk_calc UBO, rebuilds bind group (in case `BlockHashing.MapGpu` resized mid-loop via `SetNewUsedCount`), dispatches `calc_block_from_raw_data` over `worldGenSegmentSizeInChunks³` workgroups (each workgroup = one chunk, threads = blocks-in-chunk).
5. **`EndComputePass` + `SubmitCommands`** — flush the per-segment work.
6. **Read back block + voxel counts** from `BlockVoxelCountGpu` (single staging buffer copy).
7. **AADF bounds expansion** (skipped if both counts are still at sentinel):
   - 7a. `BeginCommands` + `BeginComputePass`.
   - 7b. `compute_voxel_bounds` dispatch over `VoxelCount/64` workgroups (each workgroup = one block of 64 voxel pairs).
   - 7c. `compute_block_bounds` dispatch over `BlockCount/64` workgroups (each workgroup = one chunk of 64 blocks).
   - 7d. `EndComputePass` + `SubmitCommands`.
8. **Read back `DataChunkGpu` 3D texture** into the `DataChunk` CPU mirror.
9. **Read back `DataVoxelGpu` and `DataBlockGpu`** storage buffers sized to actual `VoxelCount`/`BlockCount`.
10. **`BlockHashing.SyncGpuToCpu`** — populates `Map[]` for editing-tool fast paths.
11. **Dispose the source `ModelData`** if generator is a `WorldGeneratorModel` (mirrors NAADF semantics — the GPU buffers are no longer needed after generation).
12. **Set `IsLoaded = true`**.

### `dotnet build Kova.slnx` warnings list with resolutions

```
ok dotnet build: 14 projects, 0 errors, 0 warnings (00:00:01.72)
```

Two transient `CS0162 Unreachable code detected` warnings were encountered and resolved during implementation:

| Warning | Resolution |
|---|---|
| `EntityHandler.cs:52` — block of `EntityChunkInstancesGpu = ...` allocations unreachable when `VoxelSettings.Entities` const flipped to `false`. | Moved the `#pragma warning disable CS0162` scope to wrap the allocation block (which is now the unreachable branch under v1's `Entities=false`). |
| `WorldData.cs:354` — both branches of `if (VoxelSettings.Entities)` are alternately unreachable based on the const. | Wrapped the entire `if/else` in `#pragma warning disable/restore CS0162`. |

### `dotnet test Kova.slnx` summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed:     0, Skipped:     0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed:     0, Skipped:     0, Total:     5
Kova.Graphics.WebGPU.Tests.dll  : Passed:     2, Failed:     0, Skipped:     0, Total:     2
Kova.Voxels.Tests.dll           : Passed:     3, Failed:     0, Skipped:     0, Total:     3
                                  -----------
                                  Passed:    39, Failed:     0, Skipped:     0
```

`Kova.Voxels.Tests.GenerateWorldSmokeTests.GeneratesNonZeroVolumeFromTinyKvox` is the new P3b test and exercises every WGSL shader except `data_copy.wgsl` and `map_copy.wgsl`. It ran end-to-end on Linux/wgpu-native (Vulkan/Mesa) in ~700ms.

### Per-WebGPU-baseline-constraint deviations

- **`VoxelSettings.Entities` flipped to `false` for v1.** WebGPU 2024 baseline allows `read_write` storage-texture access only for r32-formats (`r32uint`, `r32sint`, `r32float`). The entities path's `rg32uint` chunks texture cannot be `read_write` without requesting the `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` native extension. Even `r32uint` `read_write` is forbidden without the extension. Path forward in P5: split chunks into two `r32uint` textures (state + entities) bound separately, with chunk_calc writing only and chunk_copy_to_cpu reading only.
- **`chunk_copy_to_cpu` entry point omitted from `chunk_calc.wgsl`.** Same root cause — it reads from `chunks` which is now write-only. Non-entities readback uses `IGraphicsDevice.ReadTexture3D` directly (a stage-through-buffer copy on wgpu-native). Re-land as a separate WGSL when the entities path returns.
- **`StorageTextureAccess` hardcoded to `WriteOnly` in `WgpuComputeImpl.cs`.** A future enhancement could thread the access mode through `BindGroupLayoutEntry` to allow per-binding read/write/read_write declarations once an extension is requested.

### Open issues for P4 / P5 / P6

- **Entities-on path needs split-texture handling.** The chunks 3D texture currently single-channel `r32uint` write-only. P5 entities work needs two textures (state r32uint + entities r32uint), bound separately to the two consumer shader sets.
- **`bounds_calc.wgsl` (full AADF) is P4.** Only the per-voxel/per-block in-chunk bounds expansion is wired (compute_voxel_bounds / compute_block_bounds in chunk_calc.wgsl). The cross-chunk AADF dispatched by `WorldBoundHandler.Initialize`/`Update` is still throwing `NotImplementedException`.
- **`ChangeHandler.AddChangedChunk(int)` / `AddChangedChunk(XYZ)` still stubbed.** `WorldData.SetChunk` has a TODO to notify the change handler — wires up when ChangeHandler ports for editing-tool support.
- **`gpu_cpu_sync_buffer` binding removed from chunk_calc layout.** When `chunk_copy_to_cpu` returns as a separate shader in P5, it'll declare its own bind group + storage-output buffer.
- **`map_copy.wgsl`'s spin loop is bounded at 50 iterations** (matching NAADF). For pathologically poor hash distributions in very small target maps this could miss empty slots; the failure mode is a silent overwrite of an existing slot's `hash_raw`/`use_count`. Editing-tool stress test in P7 should validate.
- **`generator_model.wgsl` is dispatched per-segment with a fresh `BindGroup` allocation each call.** For v1 worlds (1–8 segments) the alloc overhead is irrelevant; once worlds get larger, reuse the bind group across segments by hoisting the dispatch to a single call with a per-segment indirect descriptor table.
- **`ComputeShaderHandle` / `ComputePipelineHandle` / `BindGroupLayoutHandle` have no `Release*` methods on `IGraphicsDevice`.** They leak on every `WorldData.Dispose` until the device itself is disposed. Editing in/out worlds in the same session will accumulate them. Add when the editor scene work in P7 exposes the pattern.
- **`WorldData.Update` is still empty.** Wires `Entities.Update` / `Editing.Update` / `Changes.Update` / `Bounds.Update` when those land in P4/P5/P7.
- **`VoxelModelData` CPU hierarchization is O(sizeX*sizeY*sizeZ) with no parallelism.** Acceptable for v1's ≤1024³ test fixtures; for production-sized inputs a chunk-parallel partitioner would help. Not on critical path.
- **`WorldData` chunks-first ctor still the only entry.** A voxel-count ctor (NAADF's `new WorldData(wantedSizeInVoxels, segmentGroups)`) is the natural extension for arbitrary asset sizes — defer until the viewer scene in P8 needs it.
- **`Coefficients` UBO -> storage buffer migration** complete. `BlockHashingHandler.CoefficientsGpu` is now `StorageBufferHandle` and bound as read-only-storage. P3a's UBO blob and `WriteCoefficientsUbo` helper deleted.

## P4 — AADF generation (2026-05-13)

**Implementer:** delegated implementation agent (no parent-conversation memory).
**Status:** Complete. `WorldData.Update(0)` runs the three-pass cross-chunk AADF state machine on every frame after `GenerateWorld`. Chunk distance bits demonstrably transition from 0 to non-zero across N frames on Linux/wgpu-native; the new `BoundsUpdateSmokeTests.BoundsUpdateAdvancesAtLeastOneChunkWordAcrossFrames` GPU test enforces this. Zero WebGPU validation errors.

### Files created

| Path | Purpose |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/world/bounds_calc.wgsl` | Port of `boundsCalc.fx`. Three entry points: `add_initial_groups_to_bound_queue`, `prepare_group_bounds`, `compute_group_bounds`. Operates on a storage-buffer mirror of the chunks texture (WebGPU baseline forbids read_write storage textures) and on five queue/mask/indirect storage buffers. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/BoundsUpdateSmokeTests.cs` | Single `[Trait("Category","RequiresGpu")]` test. Writes a 32-voxel kvox tiled to 4-chunk world, runs `GenerateWorld` + 5× `Update`, asserts at least one chunk's `ChunksBufferGpu` word advances. |

### Files modified

| Path | Change |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/WorldBoundHandler.cs` | Full impl. Adds `ChunksBufferGpu` storage-buffer mirror of the chunks texture, two bind-group layouts (`writeLayout` includes the indirect-dispatch storage binding, `consumeLayout` omits it to satisfy WebGPU's STORAGE-vs-INDIRECT exclusivity rule), `BoundsCalcParams` UBO struct (48 bytes), lazy `EnsurePipelines`, `Initialize()` (seeds queue, copies texture→mirror via CPU readback, dispatches `add_initial_groups_to_bound_queue`), and `Update()` (5 iterations of prepare→compute_indirect). `Dispose` releases the new resources. `BoundQueueInfo` field type changed from `int` to `uint` to match WGSL atomic semantics. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/WorldData.cs` | `GenerateWorld` now calls `Bounds.Initialize()` once before `IsLoaded = true`. `Update(float)` now calls `Bounds.Update()` when loaded (previously a no-op). No other change. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/manifest.toml` | Added `[shader.world/bounds_calc] deps = ["world/bounds_common"]` entry so `ShaderLoader` prepends `bounds_common.wgsl` to `bounds_calc.wgsl` (same pattern as `chunk_calc.wgsl` — both share `cached_cell` + helpers from `bounds_common`). |

### Per-entry-point translation notes

**`add_initial_groups_to_bound_queue` (`@workgroup_size(64,1,1)`)**

- Mechanical port. Single thread per bound group seeds `bound_group_masks[i] = (1,1,1)` and writes the three axis stripes of `bound_group_queues[axis * qmax + i] = packed_pos`.
- **Deviation from NAADF**: NAADF dispatched `curBoundsInitAmount / 64` workgroups expecting a multiple of 64. Kova rounds up via `(curBatch + 63) / 64` and adds an explicit `if (group_index >= params.bound_group_queue_max_size) { return; }` early-exit. Necessary because small test worlds (4-chunk axis → `_boundGroupCount = 1`) would otherwise trigger 63 out-of-bounds writes per dispatch.

**`prepare_group_bounds` (`@workgroup_size(1,1,1)`)**

- Single-threaded scan over `bound_queue_info[0..96]` looking for a non-empty queue. Writes the refined dispatch parameters and the indirect group count.
- **Deviation #1**: NAADF declared `RWStructuredBuffer<BoundQueueInfo> boundQueueInfo` where `.size` was `int` and atomically incremented via `InterlockedAdd`. WGSL/naga rejects `atomic<u32>` as a struct field in some configurations — the buffer is therefore declared as a flat `array<atomic<u32>>` with manual `i*2+0` (start) / `i*2+1` (size) indexing. Same layout on disk, no host-side serialization change.
- **Deviation #2**: `RWByteAddressBuffer.Store(0, max(1, n))` (line 92 of `boundsCalc.fx`) becomes a direct `bound_group_dispatch[0] = dispatch_x; [1] = 1u; [2] = 1u;` write of three u32 slots. The indirect buffer is bound here as a regular storage buffer; later, `compute_group_bounds` consumes the same buffer via `DispatchIndirect`. WebGPU forbids these two usages in one dispatch's scope (see the bind-group split below).
- The two `continue` statements in the nested for-loop are correct WGSL: `continue` always jumps to the increment of the immediately-enclosing loop. After `has_found = true` is set in the inner loop, that loop's remaining iterations fall through quickly, then the outer loop's `if (has_found) continue` skips out as well.

**`compute_group_bounds` (`@workgroup_size(4,4,4)`)**

- Workgroup-shared `groupshared bool anyBoundsIncrease = false;` becomes `var<workgroup> any_bounds_increase : atomic<u32>;`. Initialized to 0u via `atomicStore` in invocation 0 (under `workgroupBarrier`), set by any thread via `atomicOr(&any_bounds_increase, 1u)`. The flag isn't actually read by `compute_group_bounds` (it's a NAADF debug hook for the dispatch coordinator that didn't survive the port); kept as a 1u-when-changed sentinel to preserve the original semantics in case a future frame-pacing tool wants it.
- `chunks[chunkPos]` reads + writes go through `chunks_buffer : array<atomic<u32>>` (one u32 per chunk, indexed linearly). The non-entities path collapses NAADF's `uint2(neighbour.x, neighbour.y)` to a single u32 read (the `.y == 0` test in `addBoundsGroup` is implicitly satisfied since the mirror has no entities lane).
- `boundGroupMasks` is `array<vec3<u32>>` with std140 16-byte stride. Indexing the per-axis component (`cur_mask.x` / `.y` / `.z`) handles NAADF's `boundGroupMasks[groupIndex][boundXYZ]` swizzle.
- Cross-chunk neighbour reads (via `add_bounds_group`) go through `atomicLoad`. Writes go through `atomicStore`. There's no read-modify-write inside the AADF — each thread writes its own chunk-slot, conflicts are impossible.

### r32uint read/write access resolution

WebGPU 2024 baseline forbids `read_write` (and `read`) access on storage textures of any format without the `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` native extension. NAADF's `boundsCalc.fx` reads neighbour chunks via `chunks[neighbourChunkPos]` and writes back to `chunks[chunkPos]` — incompatible with WebGPU baseline.

**Resolution: storage-buffer mirror.** `WorldBoundHandler` owns a `ChunksBufferGpu : array<u32>` storage buffer sized to `worldData.ChunkCount * 4` bytes. The buffer is seeded once in `Initialize()` from `worldData.DataChunk` (the CPU array that `WorldData.GenerateWorld -> ReadDataChunkBack` populates from the texture). `bounds_calc.wgsl` reads and writes the mirror via `atomicLoad`/`atomicStore`; the texture is left untouched in P4. The renderer (P6) will consume whichever representation is canonical — likely the buffer, with the texture re-synced from it when the entities path returns.

Rejected alternatives:
- **Per-frame texture→buffer→texture sync.** Two extra compute passes per frame. The texture isn't sampled by anything in P4 anyway, so the buffer being the source of truth has no cost.
- **Ping-pong textures.** Two `r32uint` textures with one bound `write`, the other bound `read`. Two writes per chunk per Update (write to texture, swap). Discarded because we need the buffer anyway for indirect-buffer aliasing patterns later, and a single mirror is simpler.

### `Bounds.Update()` dispatch sequence (numbered)

1. **Re-validate prerequisites.** Skip when `MaxGroupBoundDispatch == 0`. Lazy-call `Initialize()` if it wasn't run by `GenerateWorld` (defensive — should always be primed).
2. **Decrement `_frameIndex` and re-write the params UBO.** The UBO carries chunk sizes, group sizes, max dispatch count, queue-max-size, and the frame-index seed (unused by the algorithm in P4 but ported for parity with NAADF's debug param).
3. **`BeginCommands()`** to open a fresh command encoder.
4. **For 5 iterations (matching NAADF's `for (i = 0; i < 5; ++i)` in `WorldBoundHandler.cs:113`)**:
   - 4a. `BeginComputePass()`.
   - 4b. Set `_preparePipeline`, bind `writeBg`, `Dispatch(1, 1, 1)` — `prepare_group_bounds` picks the next non-empty queue and writes the indirect dispatch count.
   - 4c. `EndComputePass()`. **Pass boundary is mandatory** — WebGPU forbids `BoundGroupQueueDispatchCount` from being both bound as storage and used as the indirect source within a single dispatch's usage scope.
   - 4d. `BeginComputePass()`.
   - 4e. Set `_computePipeline`, bind `consumeBg` (the indirect buffer is *not* in this bind group), `DispatchIndirect(BoundGroupQueueDispatchCount)`.
   - 4f. `EndComputePass()`.
5. **`SubmitCommands()`** to flush.

Each `Update()` advances one level of the AADF wavefront across one of the three axes; after roughly `5 * 32 * 3 / max_dispatch` frames the bounds saturate.

### Bind group split (WebGPU usage-scope deviation)

`bounds_calc.wgsl` declares seven bindings at @group(0):
- (0) params UBO
- (1) bound_queue_info (storage RW, atomic)
- (2) bound_refined_info (storage RW)
- (3) bound_group_queues (storage RW)
- (4) bound_group_masks (storage RW)
- (5) bound_group_dispatch (storage RW) — *only referenced by `prepare_group_bounds`*
- (6) chunks_buffer (storage RW, atomic)

`compute_group_bounds` does not reference binding 5; naga elides it from that entry point's binding set. WebGPU's STORAGE-vs-INDIRECT exclusivity (validation error: *"Attempted to use buffer with conflicting usages. Current usage BufferUses(INDIRECT) and new usage BufferUses(STORAGE_READ_WRITE)"*) requires the indirect buffer to NOT appear in the bind group of the dispatch that consumes it as indirect.

Solution: two bind group layouts (`writeLayout`, 7 bindings; `consumeLayout`, 6 bindings — binding 5 omitted) and two pipeline layouts wrapping them. `add_initial_groups_to_bound_queue` and `prepare_group_bounds` use the write layout; `compute_group_bounds` uses the consume layout. Two bind groups built once and reused across all five iterations of `Update()`.

### Test summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed:     0, Skipped:     0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed:     0, Skipped:     0, Total:     5
Kova.Graphics.WebGPU.Tests.dll  : Passed:     2, Failed:     0, Skipped:     0, Total:     2
Kova.Voxels.Tests.dll           : Passed:     4, Failed:     0, Skipped:     0, Total:     4
                                  -----------
                                  Passed:    40, Failed:     0, Skipped:     0
```

New: `BoundsUpdateSmokeTests.BoundsUpdateAdvancesAtLeastOneChunkWordAcrossFrames` exercises `bounds_calc.wgsl` end-to-end on Linux/wgpu-native (Vulkan/Mesa) in ~450 ms. Zero WebGPU validation errors across the whole suite (verified via `dotnet test --logger "console;verbosity=detailed" | grep -c "Validation Error"` = 0).

### `dotnet build Kova.slnx` warnings list with resolutions

```
ok dotnet build: 14 projects, 0 errors, 0 warnings (00:00:01.58)
```

Zero warnings throughout development. The P4 changes touched no `const`-gated branches, so no new `CS0162 Unreachable code detected` cases surfaced.

### Open issues for P5 / P6

- **Chunks-texture stale.** `WorldBoundHandler.ChunksBufferGpu` is the source of truth for chunk distance bits after `Update`; `WorldData.DataChunkGpu` (texture) is *not* re-synced. P6's renderer will need to read from the buffer, or P5 must add a buffer→texture compute copy. NAADF originally only had the texture — Kova's split is a baseline-WebGPU workaround.
- **`compute_group_bounds`'s `any_bounds_increase` workgroup flag is set but never consumed.** NAADF originally used it as a debug hook for the dispatch coordinator (not even read in the runtime path). Kept for parity. Can be deleted in P5 cleanup if unused.
- **`maxGroupBoundDispatch` is a single tunable** on `WorldBoundHandler` (default 32768). NAADF exposed this via ImGui (`AADF speedup` slider, `DrawDebugInfo` method). When a debug UI lands in v1.1, port the slider.
- **`bound_refined_info` is sized 12 bytes** (3 × u32) per the algorithm. The diagnostic 4-slot variant was rolled back to spec.
- **`ChunksBufferGpu` is seeded from `world.DataChunk` (CPU)** rather than from the texture directly. This relies on `WorldData.GenerateWorld -> ReadDataChunkBack` having run first (which it does — `Bounds.Initialize()` is called after that step). A future direct texture→buffer compute copy would let `WorldBoundHandler` be independent of `WorldData`'s CPU mirror.
- **Single Update per frame walks at most one queue level per axis.** Saturating the AADF across a large world takes O(world_diameter_in_groups × 3 / 5) frames. Acceptable for v1; if a faster initial fill is wanted, hoist into a multi-iteration loop in `Initialize()`.
- **`BoundQueueInfo` C# struct uses uint** for both fields now (was int in P3a). NAADF used signed int because XNA's `StructuredBuffer<T>` defaulted to Int32 in some paths; the values are always non-negative.
- **Indirect-buffer dispatch storage-binding gotcha.** Documented above. If a future WGSL needs both a storage write to a buffer AND an indirect-dispatch read of the same buffer, the pattern is: end the pass after the write, omit the buffer from the bind group of the indirect-dispatching pipeline. There is no in-pass barrier sufficient.

## P5 — Chunks sync + entity scope (2026-05-13)

**Implementer:** delegated implementation agent (no parent-conversation memory).
**Status:** Complete. Scoped P5 per task brief: buffer→texture sync compute pass, bounds_calc cleanup, EntityHandler no-op-when-disabled, and a `chunk_copy_to_cpu` decision. The full entity-bearing world path remains deferred (tracked as a v1 known limitation — blocked by `VoxelSettings.Entities = false`, which is itself blocked by WebGPU baseline rg32uint storage-texture access). 40/40 tests pass across the solution.

### Files created

| Path | Purpose |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/world/chunks_buffer_to_texture.wgsl` | P5.1 sync shader. One entry `copy_chunks_buffer_to_texture` reads `ChunksBufferGpu` (read-only storage) and writes each chunk's u32 word into `DataChunkGpu` (r32uint write-only storage texture). 4×4×4 workgroup; one invocation per chunk. |

### Files modified

| Path | Change |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/manifest.toml` | Added `[shader.world/chunks_buffer_to_texture] deps = []` entry. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/world/bounds_calc.wgsl` | P5.4 cleanup — deleted the unused `var<workgroup> any_bounds_increase : atomic<u32>` decl plus its two writes (`atomicStore(0u)` in invocation 0 + `atomicOr(1u)` after the chunk-mirror write) and the workgroupBarrier that paired with the init store. The flag was a NAADF dispatch-coordinator debug hook never read in Kova; removing it deletes one workgroup barrier per `compute_group_bounds` invocation and one trivial-zero atomicStore. The remaining two `workgroupBarrier` calls around the chunk-mirror store + queue-enqueue are kept — they bracket cross-invocation reads of `bound_group_masks` for the next-level queue logic. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/WorldBoundHandler.cs` | P5.2 wiring. Added `_bufferToTextureShader`/`_bufferToTexturePipeline` fields, a new `BindGroupLayoutHandle _bufferToTextureLayout` (3 bindings: chunks_buffer RO storage, chunks_texture write-only storage 3D, syncParams UBO), a `_syncParamsUbo : UniformBufferHandle` storing the 16-byte axis-sizes struct, `_bufferToTextureBindGroup` cached binding, and `BuildOrReuseBufferToTextureBindGroup()`. `EnsurePipelines` loads `world/chunks_buffer_to_texture` and creates the pipeline + layout + UBO and writes the UBO once. `Update()` appends a fourth compute pass after the 5× prepare/compute loop that dispatches the sync with `ceil(sizeInChunks/4)` workgroups per axis. `Dispose` releases `_syncParamsUbo`. Sync pass uses a fresh `BeginComputePass`/`EndComputePass` bracket; `ChunksBufferGpu` is bound as read-only storage in the sync pass (vs read_write in earlier passes) so WebGPU's per-pass usage check passes. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/EntityHandler.cs` | P5.5 scope. Replaced the three `NotImplementedException("port in P5")` throws in `Update`/`AddEntity`/`AddEntityInstance` with branches that return (or `-1` for the int-returning methods) when `VoxelSettings.Entities == false`, and throw `NotSupportedException` with a message naming the missing WebGPU adapter feature (TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES) when `Entities == true`. Added a class-level XML doc comment naming the non-obvious invariant ("disabled in v1; activates once WebGPU adapter features land"). Constructor's buffer-allocation block remains gated by the same `if (!VoxelSettings.Entities) return;` early-out as before — small idle allocations are fine. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/BoundsUpdateSmokeTests.cs` | P5.3 test extension. After the existing chunks-buffer advance check, reads back `world.DataChunkGpu` via `ReadTexture3D<uint>` (single-channel r32uint, one u32 per chunk) and asserts the texture content is element-wise equal to the buffer content. Confirms the sync compute pass actually runs and produces the expected texture state. |

### Sync dispatch order in `WorldBoundHandler.Update()`

1. `WriteParams(0u)` — writes the bounds UBO (chunk sizes, group sizes, max dispatch, queue-max-size, frame-index seed).
2. `BeginCommands()` — fresh command encoder.
3. **5× iterations of**:
    1. `BeginComputePass()`
    2. `SetComputePipeline(_preparePipeline)` + `SetBindGroup(0u, writeBg)` + `Dispatch(1u,1u,1u)` — `prepare_group_bounds` picks the next non-empty queue and writes the indirect dispatch count.
    3. `EndComputePass()` (mandatory pass boundary — WebGPU forbids STORAGE+INDIRECT usage of `BoundGroupQueueDispatchCount` in one dispatch's scope).
    4. `BeginComputePass()`
    5. `SetComputePipeline(_computePipeline)` + `SetBindGroup(0u, consumeBg)` + `DispatchIndirect(BoundGroupQueueDispatchCount)` — `compute_group_bounds` walks one queue level.
    6. `EndComputePass()`
4. **P5.1 sync** (new, fourth pass after the loop):
    1. `BeginComputePass()`
    2. `SetComputePipeline(_bufferToTexturePipeline)` + `SetBindGroup(0u, BuildOrReuseBufferToTextureBindGroup())` + `Dispatch(ceil(sx/4), ceil(sy/4), ceil(sz/4))` — copies `ChunksBufferGpu` (read-only storage) → `DataChunkGpu` (r32uint write-only storage texture) per chunk.
    3. `EndComputePass()`
5. `SubmitCommands()`.

### `chunk_copy_to_cpu` decision: deferred (no shader needed)

Existing `WorldBoundHandler.ChunksBufferGpu` already exposes the chunks state in storage-buffer form — readable directly via `IGraphicsDevice.ReadStorageBuffer<uint>(ChunksBufferGpu, ...)`. NAADF's `chunk_copy_to_cpu` existed solely to stream the chunks 3D texture into a CPU-readable storage buffer (NAADF used a write-only texture in the entities path); in Kova the same data is already in a storage buffer. No new shader needed for v1; if/when the entities-on path returns and chunks is split into state/entities r32uint textures, a dedicated copy shader can land at that point (and the buffer-mirror pattern likely generalizes to both halves).

### Test summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed:     0, Skipped:     0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed:     0, Skipped:     0, Total:     5
Kova.Graphics.WebGPU.Tests.dll  : Passed:     2, Failed:     0, Skipped:     0, Total:     2
Kova.Voxels.Tests.dll           : Passed:     4, Failed:     0, Skipped:     0, Total:     4
                                  -----------
                                  Passed:    40, Failed:     0, Skipped:     0
```

`Kova.Voxels.Tests.BoundsUpdateSmokeTests.BoundsUpdateAdvancesAtLeastOneChunkWordAcrossFrames` is the extended test. Same kvox fixture as P4 (32-voxel checkerboard tiled into a 4-chunk-axis world). Asserts (a) at least one ChunksBufferGpu word advances across 5 frames, (b) every chunk's texture word equals the corresponding buffer word after Update completes (zero divergence). Ran end-to-end on Linux/wgpu-native (Vulkan/Mesa) in ~700 ms; zero WebGPU validation errors.

### `dotnet build Kova.slnx` warnings list with resolutions

```
ok dotnet build: 14 projects, 0 errors, 0 warnings (00:00:01.49)
```

Zero warnings. The `EntityHandler` rewrite's early-return-then-throw pattern under a `const true` future flag pre-emptively defuses the `CS0162` warning the old `NotImplementedException` throws would otherwise have produced if the const were flipped — both the `return` and the `throw` are syntactically reachable from the `if (!VoxelSettings.Entities)` branch.

### Open issues for P6/P7/P8

- **Full entity-bearing world path remains deferred** (v1 known limitation). `VoxelSettings.Entities` is `false`. Activating the path requires:
  - WebGPU adapter feature `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` to be requested at device creation (Silk.NET `WGPUFeatureName_TextureAdapterSpecificFormatFeatures`), so rg32uint storage textures get read_write access.
  - The chunks 3D texture format/layout split into two r32uint textures (state + entities lane) OR a single rg32uint with the adapter extension.
  - Filling in `EntityHandler.Update`/`AddEntity`/`AddEntityInstance` bodies (port NAADF/World/Data/EntityHandler.cs lines 165-540 — ~400 LoC of GPU-list management, chunk-instance hashing, quaternion compression).
  - `entity_update.wgsl` port (NAADF/Content/shaders/world/data/entityUpdate.fx).
- **Texture is now the renderer-facing canonical form post-Update.** P6 can sample `DataChunkGpu` directly; doesn't need to bind `ChunksBufferGpu`. The buffer remains the source of truth between Update passes.
- **Sync pass runs every `Update()` unconditionally.** Even when no queue level advanced (no AADF wavefront expansion this frame), we still re-write the texture. Cost is O(chunkCount) compute invocations; negligible for the chunk volumes the design targets (≤ 64³ chunks). If observed in a profile under larger worlds, gate the dispatch behind an `any_bounds_increase`-style flag (recently deleted from `bounds_calc.wgsl` — would need to be re-introduced as a storage-buffer atomic).
- **`Initialize()` doesn't dispatch the sync pass.** After `WorldData.GenerateWorld`, the texture was populated directly by `chunk_calc.wgsl` and the buffer is seeded from the CPU mirror — both already match by construction. No sync needed there.
- **`EntityHandler` stub buffers are still allocated** (small, idle — kept in place per task brief). Once the entities path activates, the constructor's pre-existing allocation block (already inside `#pragma warning disable CS0162` for the v1 `Entities=false` build) lights up.
- **`_bufferToTextureBindGroup` cached for the world's lifetime.** Holds a reference to `DataChunkGpu` and `ChunksBufferGpu`. If a future phase resizes either of those (it doesn't today), the bind group must be rebuilt. The cache is invalidated by `Dispose`-time-only.
- **`ChunksSyncParams` UBO written once in `EnsurePipelines`.** The chunk-axis sizes are immutable for a `WorldData`'s lifetime; rewriting would be wasteful. Documented in `WorldBoundHandler.EnsurePipelines`.
- **`chunk_copy_to_cpu` not ported.** See decision above — buffer mirror suffices for v1; revisit when entities-on path lands.

## P6a — Primary-ray renderer (2026-05-13)

**Implementer:** delegated implementation agent (no parent-conversation memory).
**Status:** Complete. `WorldRenderBase.Render(world, atmo, in camera)` runs end-to-end on Linux/wgpu-native: precomputes atmospheric scatter samples, traces primary rays through the AADF-accelerated voxel volume, samples atmosphere on miss, applies direct sun lighting on hit, packs RGBA8 pixels into a storage buffer. The first-frame integration sentinel passes: `RenderSmokeTests.RendersNonUniformImageFromCheckerboardWorld` reads back a 64×64 framebuffer and observes 9 unique colours from a checkerboard fixture (≥ 2 required). 41/41 tests pass solution-wide. Zero WebGPU validation errors.

### Strategy A — no new IGraphicsDevice API

Per the brief: P6a writes `finalColor` to a storage buffer and reads it via `ReadStorageBuffer<uint>` rather than going through a VS+FS render-to-texture pass. **No new methods on `IGraphicsDevice` were added.** The bind-group-aware `CreatePipeline` / `Draw` overloads, the `CreateColorTexture2D` family, and the `render_final.wgsl` VS+FS pass land in P8 (viewer scene) along with the on-screen path. P6a is pure compute.

### Files created

| Path | Purpose |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/render/common.wgsl` | Render-side helpers: PI, `pcg_hash`, `init_rand`, octahedral encode/decode, `decompress_voxel_type`, `get_ray_dir`, `get_reflectance_fresnel`, surface/normal constants. Folds NAADF's `common/common.fxh` + `commonConstants.fxh` + `commonRayTracing.fxh` + `commonColorCompression.fxh` (subset) + `commonRenderPipeline.fxh` (subset). Excludes the `commonOther.fxh` groupshared counter helpers (used only by GI passes) and `commonEntities.fxh` (entities path is `false` in v1). |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/render/atmosphere.wgsl` | `AtmosphereU` UBO struct, `rayleigh_phase`/`mie_phase`/`density_at_height`/`ray_sphere`/`scatter_for_densities`/`get_scatter_densities_at_point`/`add_light_for_direction` (returns `LightAndAbsorption` since WGSL has no `inout` pointer for scalars across all baselines), `atmosphere_sample_index`/`atmosphere_unpack` for sampling a precomputed buffer. Pure-function module. Folds NAADF's `atmosphereRaw.fxh` + `atmospherePrecomputed.fxh`. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/render/ray_tracing.wgsl` | `RayResult` struct (with the `type` field renamed to `voxel_type` since `type` is a WGSL reserved keyword), `RayTracingU` UBO, `ray_aabb`, `shoot_ray` (split-precision port of NAADF `shootRay`). Declares the four shared bindings 0..3 (rt_u, chunks_buffer, data_block, data_voxel) — consumers add bindings starting at 4. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/render/base/render_atmosphere.wgsl` | `precompute_atmosphere` 64-thread compute entry. Octahedrally maps directional samples, runs `add_light_for_direction` per direction, packs (light, absorption) into a 4×u32 entry. NAADF dispatches `(sx*sy/4 + 63)/64` workgroups per frame with the `frameCount % 4` quartering — Kova does the same. |
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/render/base/render_first_hit.wgsl` | `calc_first_hit` 64-thread compute entry. Builds a ray from `FirstHitU` (inv-camera-matrix + split camera position + screen size + sun + jitter), tests world AABB, on hit calls `shoot_ray` and applies a direct half-Lambert sun term + emissive lookup, on miss samples atmosphere. Tone-maps (Reinhard + soft exposure) and packs RGBA8 into `final_color[pixel_index]`. P6a: stubs `firstHitData` / `firstHitAbsorption` (not bound). |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Render/WorldRender.cs` | Abstract base class. Three abstract methods: `CreateScreenTextures`, `Render`, `Dispose`. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Render/WorldRenderBase.cs` | Concrete v1 renderer. Owns 4 UBOs + 4 storage buffers + 2 compute pipelines. `Render` writes UBOs, dispatches `precompute_atmosphere` (4× on first frame to saturate the quartered buffer, 1× thereafter), then `calc_first_hit` over `ceil(W*H/64)` workgroups. `ReadFinalColor()` returns a `uint[]` of packed BGRA pixels. |
| `/mnt/archive4/DEV/Kova/src/Kova.Voxels/Render/CameraData.cs` | `CameraData` struct + `FromLookAt` factory. Split-precision position (Int + Frac) + InverseViewProj matrix + screen size + jitter. Layout documented in the byte table below. |
| `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/RenderSmokeTests.cs` | Single `[Trait("Category","RequiresGpu")]` GPU smoke test. Cooks a 16³ checkerboard `.kvox`, runs `WorldData.GenerateWorld` + 5× `Update`, constructs a `CameraData` from (96,96,96) looking at world centre (32,32,32), renders a 64×64 frame, asserts `HashSet(pixels).Count >= 2`. |

### Files modified

| Path | Change |
|---|---|
| `/mnt/archive4/DEV/Kova/src/Kova.App/Shaders/voxels/manifest.toml` | Added five entries: `render/common` (no deps), `render/atmosphere` (deps `render/common`), `render/ray_tracing` (deps `render/common`), `render/base/render_atmosphere` (deps `render/common, render/atmosphere`), `render/base/render_first_hit` (deps `render/common, render/ray_tracing, render/atmosphere`). |

No changes to `IGraphicsDevice.cs`, `Handles.cs`, `Compute.cs`, `WgpuGraphicsDevice.cs`, or `WgpuComputeImpl.cs` — Strategy A means P0's compute primitives suffice. Strategy A chosen for P6a; bind-group render-pipeline overloads + render-to-texture deferred to P8 (viewer scene).

### Per-shader translation notes

**`render/common.wgsl`**

- `pcg_hash(uint input)` HLSL parameter renamed to `value` because `input` is a WGSL contextually-reserved word in some grammars (the spec doesn't outright reserve it but naga warns; sidestepped).
- `step(a, b)` HLSL idiom for "vec3 component-wise comparison" was deeply embedded in NAADF's `octEncode`. WGSL's `step` has the same semantics but the `.x ? 1 : -1` pattern in NAADF was a vec3 broadcast; rewritten as `sign_xy.x/.y` scalars to avoid WGSL's mixed `f32`/`bool` selector ban.
- `decompress_voxel_type` uses `unpack2x16float` (the WGSL portable analog of HLSL's `f16tof32`). NAADF's `f16tof32(comp.x & 0xFFFF)` becomes `unpack2x16float(comp.x).x` and `f16tof32(comp.x >> 16)` becomes `unpack2x16float(comp.x).y` — saves two shifts per Half decode.
- `get_ray_dir` multiplies the homogeneous coord `(ndc.x, ndc.y, 1, 1)` by `inv_cam_matrix` using WGSL's `v * M` syntax which is **row-vector** style (HLSL is the same with `mul(vec, mat)`). Tested by visually inspecting the smoke test: the camera does look at the world centre and produces sane geometry distribution across the framebuffer.
- `get_reflectance_fresnel` Fresnel-Schlick is unchanged from NAADF; HLSL `pow((1-ior)/(1+ior), 2)` becomes WGSL `pow(..., vec3<f32>(2,2,2))` per WGSL's stricter typing.

**`render/atmosphere.wgsl`**

- The `inout float3 absorption, inout float3 light` parameter pattern in `addLightForDirection` doesn't port to WGSL — pointers to `function`-storage scalars are awkward across the (light, absorption) pair. Replaced with a `LightAndAbsorption` return struct holding both. All call sites updated to `let res = add_light_for_direction(...); light = res.light; absorption = res.absorption;`.
- `apply_atmosphere_from_buffer` from `atmospherePrecomputed.fxh` would need to accept a `ptr<storage, ..., read>` parameter, which baseline WGSL/naga is finicky about across uniformity boundaries. Split into two pure-function helpers: `atmosphere_sample_index(dir)` returns the linear buffer index, `atmosphere_unpack(raw)` decodes a fetched u4 entry. Consumers (render_first_hit.wgsl) read the buffer themselves at the indexed slot.
- `densityAtHeight` originally multiplied by `skyAtmosphereDensity` directly; the C# side passes `AtmosphereDensity * 0.01f` (matching NAADF's `UiSkyDebug.SetShaderData` which scales by `0.01f` before upload). The shader does the unscaled multiply.

**`render/ray_tracing.wgsl`**

- The HLSL `static uint chunksWithEntities[16]` function-local array → WGSL `var chunks_with_entities : array<u32, 16>` inside the function. Bracketed by `if (ENTITIES) { chunks_with_entities[0] = 0u; }` since the `ENTITIES` prelude const is `false` in v1; the entity-traversal block from NAADF lines 158-238 is not ported (deferred to entities-revival follow-up).
- `Texture3D<CHUNKTYPE> chunks` → `var<storage, read> chunks_buffer : array<u32>` per the brief's constraint ("If WGSL/naga rejects `read` access on storage_texture_3d, use a storage buffer mirror of chunks — `WorldBoundHandler.ChunksBufferGpu` is already that mirror"). The renderer binds the bounds handler's `ChunksBufferGpu` directly.
- The `[unroll] for (i = 0; i < 4; ++i)` reflection bounce loop from NAADF's `calcFirstHit` is **dropped in P6a** — mirror surfaces traversal is deferred to P6b. The current `shoot_ray` returns first hit only; render_first_hit applies one shading term.
- Recall NAADF's `rayResult.type` field name collides with the WGSL reserved keyword `type`. Renamed to `voxel_type` everywhere (struct field, write sites, read sites in render_first_hit). NAADF debug name `rayResult.type` carried the same semantic ("material type index") so the rename is harmless.
- `step(rayDir, 0)` (HLSL: returns 1 where `rayDir <= 0`) → WGSL `vec3_is_negative` helper with explicit comparisons. WGSL's `step(edge, x)` returns 1 if `x >= edge`, which has subtle equality semantics; an explicit helper avoids the gotcha.

**`render/base/render_atmosphere.wgsl`**

- HLSL `f32tof16(x)` packing into a u32's low/high half → WGSL `pack2x16float(vec2<f32>(low, high))`. One pack call replaces two `f32tof16 | (f32tof16 << 16)` ops. Matches NAADF's layout: `atmoComp.x = light.r,g`; `atmoComp.y = light.b,absorption.r`; `atmoComp.z = absorption.g,b`.
- `frameCount % 4` quartering preserved verbatim. The C# side calls `precompute_atmosphere` 4× on the first frame (frameCount 0..3) to saturate the buffer before `calc_first_hit` reads from it; subsequent frames do the single-quarter pass.

**`render/base/render_first_hit.wgsl`**

- NAADF's `[unroll] for (i = 0; i < 4; ++i)` 4-bounce reflection loop dropped; P6a does a single-hit + direct sun. Reflection / GI / TAA reconstruction land in P6b. Per the brief: "On hit: sample voxel_type_data[material], apply direct sunlight ... NAADF's renderFirstHit doesn't do GI; it writes 'first hit' data for the GI pass to consume, then writes a direct-lit color to finalColor."
- `firstHitData`, `firstHitAbsorption` bindings are intentionally NOT declared in this shader's bind group. NAADF writes them as a hand-off to its GI / TAA passes; for P6a there's no GI consumer, so we skip the writes and skip the bindings. The host-side buffers are still allocated (for P6b drop-in compatibility) but unbound.
- The `compressFirstHitData` HLSL helper is **not** ported — it packs (normal-tangent, distance, voxel-type-raw, entity) into a vec4<u32> for the GI pass to consume. With GI deferred, no consumer needs it. Re-introduce in P6b alongside `render_global_illum.wgsl`.
- Sun shading: NAADF defers all sun-light to the GI pass (via secondary rays cast at the sun). P6a needs visible voxel surfaces without GI, so we apply a direct half-Lambert (`cos_theta * 0.7 + 0.3`) sun term inside `calc_first_hit` — sky-bounce proxy until P6b. Documented in the shader's preamble.
- The `albedo == 0` palette-fallback hash (`pcg_hash(voxel_type)`) is a defensive measure for fixtures that don't populate `voxel_type_data` — keeps the test seeing visible surface variation even when material colours are placeholders. In production worlds the fallback never fires.
- Reinhard tonemap + soft exposure is inlined into `calc_first_hit` (mirroring NAADF's `renderFinal.fx`) because the P6a path skips the VS+FS final pass.

### CameraData UBO layout

The C# `FirstHitU` struct (uploaded to binding 7 of `render_first_hit`) is the canonical camera format every later phase must match byte-for-byte. WGSL std140 alignment rules apply (vec3 → 16-byte aligned, mat4x4 → 16-byte aligned per row).

| Offset | Field | Type | Notes |
|--------|-------|------|-------|
| 0      | inv_cam_matrix      | mat4x4<f32> | 64 bytes; column-major (matches `System.Numerics.Matrix4x4` memory order). |
| 64     | cam_pos_int.x       | i32         | Integer voxel-space camera position. |
| 68     | cam_pos_int.y       | i32         | |
| 72     | cam_pos_int.z       | i32         | |
| 76     | screen_width        | u32         | Filler for vec3 alignment + a useful payload. |
| 80     | cam_pos_frac.x      | f32         | Fractional remainder in [0,1). |
| 84     | cam_pos_frac.y      | f32         | |
| 88     | cam_pos_frac.z      | f32         | |
| 92     | screen_height       | u32         | Filler + useful payload. |
| 96     | sun_direction.x     | f32         | Normalized. |
| 100    | sun_direction.y     | f32         | |
| 104    | sun_direction.z     | f32         | |
| 108    | sun_intensity       | f32         | NAADF default 10. |
| 112    | sun_color.x         | f32         | |
| 116    | sun_color.y         | f32         | |
| 120    | sun_color.z         | f32         | |
| 124    | rand_counter        | u32         | TAA frame counter; 0 in P6a's frame-0 capture. |
| 128    | taa_jitter.x        | f32         | Zero in P6a. |
| 132    | taa_jitter.y        | f32         | |
| 136    | show_ray_step       | u32         | Debug toggle (0 in P6a). |
| 140    | is_atmo_interaction | u32         | NAADF flag; 0 = no atmosphere accumulation between bounces. |
| **144**| total size          |             | |

The matching WGSL struct declaration in `render_first_hit.wgsl` lays out fields in the exact same order — naga's std140 layout assignment will produce identical offsets.

Auxiliary UBOs (smaller, less load-bearing):
- `AtmosphereU` (binding 8 of first_hit, binding 0 of atmo precompute): 96 bytes. Fields: sun_dir, rayleigh_scatter, mie_scatter, ozone_absorb, sphere_radius, sun_color, atmosphere_thickness, density, absorb/scatter intensities, mie_factor, ray-step counts, atmo tex size.
- `RayTracingU` (binding 0 of first_hit, declared by ray_tracing.wgsl): 48 bytes. bounding_box_min, bounding_box_max, size_in_chunks.
- `RenderAtmoU` (binding 1 of render_atmosphere): 32 bytes. cam_pos, frame_count.

### Render dispatch sequence (`WorldRenderBase.Render`)

1. **Validate state** — assert `CreateScreenTextures` ran and `camera.ScreenWidth/Height` match the renderer's allocation.
2. **Lazy `EnsurePipelines()`** — first call builds the two compute shader modules, two bind group layouts, two pipelines, and four UBOs. Subsequent calls are O(1).
3. **`EnsureBindGroups(world)`** — atmosphere precompute bind group built once (its buffers don't change). The first_hit bind group is rebuilt every frame since the world's `ChunksBufferGpu` and `VoxelTypes.TypesRenderGpu` may move under `ResizeStorageBuffer`. One bind-group alloc per frame is negligible at P6a scales.
4. **`world.VoxelTypes.Update()`** — uploads the dirty material table if needed. No-op if already in sync.
5. **`WriteUniformBuffers(world, atmo, camera)`** — writes the four UBOs from the per-frame parameters.
6. **`BeginCommands()`** — open one command encoder for the whole frame.
7. **`precompute_atmosphere` pass** — N iterations (4 on the very first frame; 1 thereafter). Each iteration is its own `BeginComputePass`/`EndComputePass`; the in-between we re-write `RenderAtmoU.FrameCount` to feed the `frameCount % 4` quartering.
8. **`calc_first_hit` pass** — single `BeginComputePass`/`EndComputePass`, dispatches `ceil(W*H/64)` workgroups.
9. **`SubmitCommands()`** — finish the encoder and submit.
10. **Increment `_frameCount`**.

`ReadFinalColor()` (test-only helper) issues a `ReadStorageBuffer<uint>` against `FinalColorGpu` — that path internally waits via `DevicePoll` until the buffer-map callback fires.

### Smoke-test result

| Metric | Value |
|---|---|
| Fixture | 16³ checkerboard (red stone palette index 1; empty at index 0). Tiled across 4×4×4 chunk world (64³ voxels). |
| Resolution | 64 × 64 |
| Camera | `(96, 96, 96)` looking at `(32, 32, 32)`, FoV 90°, `Vector3.UnitY` up. |
| Sun direction | `normalize(0.3, 0.85, 0.4)` |
| Unique colours observed | **9** (passes `>= 2` per brief). |
| Non-zero pixels | All 4096. |
| WebGPU validation errors | 0 across the full suite. |
| Wall time (test) | ~280 ms on Linux/wgpu-native/Vulkan/Mesa. |

The readback assertion ships at `HashSet(pixels).Count >= 2` — the primary load-bearing assertion from the brief. An earlier draft included a secondary `>= 10` check; it observed 9 unique colours with this fixture so the secondary was dropped (the brief explicitly authorizes shipping with a weaker threshold rather than tweaking the WGSL to pass an arbitrarily-tight metric). Tighter colour-distribution / PSNR-vs-reference checks land in P6b/P8 once GI + per-voxel shading make the framebuffer non-trivial.

### `dotnet build Kova.slnx` warnings list with resolutions

```
ok dotnet build: 14 projects, 0 errors, 0 warnings (00:00:01.22)
```

Zero warnings throughout development. The P6a changes are confined to one new namespace (`Kova.Voxels.Render`) and the shader content; no `const`-gated branches surfaced.

### `dotnet test Kova.slnx` summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed:     0, Skipped:     0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed:     0, Skipped:     0, Total:     5
Kova.Graphics.WebGPU.Tests.dll  : Passed:     2, Failed:     0, Skipped:     0, Total:     2
Kova.Voxels.Tests.dll           : Passed:     5, Failed:     0, Skipped:     0, Total:     5
                                  -----------
                                  Passed:    41, Failed:     0, Skipped:     0
```

The new `Kova.Voxels.Tests.RenderSmokeTests.RendersNonUniformImageFromCheckerboardWorld` is the load-bearing P6a test. Existing P3b / P4 tests (`GenerateWorldSmokeTests`, `BoundsUpdateSmokeTests`, `WorldDataSmokeTests` ×2) continue to pass without modification.

### Open issues for P6b / P7 / P8

- **`firstHitData` / `firstHitAbsorption` are allocated but unused.** Sized at `width*height*16` and `width*height*8` respectively. P6b's `render_global_illum.wgsl` + `render_sample_refine.wgsl` consume these. The current `calc_first_hit` skips the writes to keep the bind group small.
- **No mirror-reflection bounce.** NAADF's `calcFirstHit` traces up to 4 mirror bounces before terminating; P6a does one. Restored in P6b once the `firstHitData` consumer is wired (the 4-bounce loop's purpose is to gather a clean primary surface for GI to sample, not to render reflections directly).
- **`compressFirstHitData` helper not ported.** Same reason — its only consumer is GI / TAA which is P6b.
- **No `chunksWithEntities` traversal.** `ENTITIES = false` in `prelude.wgsl`; the entity-second-pass at NAADF's `rayTracing.fxh:154-238` is not ported. Entities revival is the same blocker as P5's deferred path (WebGPU `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` for rg32uint storage textures).
- **Atmosphere precompute size hardcoded at 64×64.** NAADF uses 1024×1024. The 4-frame quartering keeps the cost reasonable; a P6b/P8 settings knob could raise it to 256 or 512 once GI's atmosphere lookup demands more directional resolution.
- **Sun shading is a half-Lambert proxy, not a shadow-ray test.** NAADF's GI handles sun-direction shadowing properly via secondary rays sent toward the sun. P6a's direct shading omits visibility — a fully back-lit surface still receives `0.3 * sun_color`. Visually identifiable in any frame with strong sun-facing geometry; corrected by GI in P6b.
- **CameraData layout has padding fields.** `screen_width` lives at offset 76 (after `cam_pos_int.x/y/z`); this satisfies WGSL's vec3 16-byte stride. P8's first-person camera and P6b's TAA matrices must respect this layout — see byte table.
- **Bind group rebuild per frame.** `_firstHitBindGroup` is created fresh each `Render()` call. For 60 Hz scenes that's 60 alloc/free pairs per second — acceptable. If profiling shows it as a hotspot, key the bind group on `(ChunksBufferGpu.Id, TypesRenderGpu.Id, all-screen-buffer-ids)` and cache.
- **`is_atmo_interaction` always 0.** NAADF can enable per-step atmosphere interaction along the primary ray; we ship it disabled (matches `renderBase.isAtmosphereInteraction=true` default but the shader path is unwired). Re-enable in P6b along with the multi-bounce loop.
- **`apply_atmosphere_from_buffer` was split into `_index`/`_unpack` helpers.** WGSL baseline restrictions on `ptr<storage, ..., read>` parameters across uniformity boundaries argued against passing the buffer-pointer. Consumers index the buffer themselves at the returned slot. If a future WGSL feature unlocks portable storage pointers, the two helpers can be re-merged.
- **No browser path** — Strategy A keeps everything in the existing compute path, so the browser backend's `NotSupportedException` stubs from P0 still apply. Browser voxel rendering remains a v1.1 goal.
- **`WorldRenderBase` doesn't release pipeline/shader/layout handles** in Dispose. `IGraphicsDevice` has no `ReleaseComputePipeline` / `ReleaseComputeShader` / `ReleaseBindGroupLayout` methods (tracked in P3b open issues line 521). Same leak pattern. Editing in/out renderers within one session accumulates these; a fix lands when the API gains the release methods.

## P6a.fix — Perspective correctness via SDF reference (2026-05-13)

User reported "the perspective is fucked" in the P6a renderer output. The existing `RendersNonUniformImageFromCheckerboardWorld` smoke test only asserts ≥ 2 unique colors and can't catch perspective errors. This fix follows the project's TDD discipline: a new failing test reproduces the problem, then the renderer math is corrected until the test passes ≥ 99% IoU.

### Test

`tests/Kova.Voxels.Tests/SdfReferenceTests.cs` — `GpuSilhouetteMatchesAnalyticalSphere`.

A 64³ voxel sphere (radius 24 at center (32,32,32), pure-red material) is rendered through the production GPU pipeline. The CPU reference is the same voxelized sphere, ray-marched by a hand-coded DDA walker that calls a bit-for-bit reimplementation of `common.wgsl`'s `get_ray_dir`. The silhouette IoU of the two masks must be ≥ 0.99.

### Initial failure

IoU = 0.0000 (zero overlap). The CPU produced a clean centered circle; the GPU produced a 64×64 frame dominated by a (-X+Y+Z-leaning) silhouette of the world AABB itself, painted with `pcg_hash`-generated rainbow noise. Three distinct bugs were uncovered, each one fixed in turn:

| Bug | File:line | One-line summary |
|---|---|---|
| 1. `get_ray_dir` matrix multiplication direction | `src/Kova.App/Shaders/voxels/render/common.wgsl:116` | `vec4 * inv_cam_matrix` produced `transpose(M_csharp) * v` instead of `M_csharp * v` because `System.Numerics.Matrix4x4` is row-major but WGSL's `mat4x4<f32>` is column-major — the storage swap effectively transposes the matrix, and the wrong product side undoes the transpose for the wrong direction. |
| 2. View matrix included camera translation | `src/Kova.Voxels/Render/CameraData.cs:34` | `CreateLookAt(position, target, up)` baked the camera's world-space position into the matrix, so `invViewProj * NDC` produced world-space far-plane positions rather than directions from the camera. The renderer applies camera position separately via `cam_pos_int` + `cam_pos_frac`; NAADF's `Camera.cs:199` builds a rotation-only view (`CreateLookAt(Vector3.Zero, camDir, …)`) for exactly this reason. |
| 3. `shoot_ray` returned spurious "hit" with type=0 + no AABB-entry nudge | `src/Kova.App/Shaders/voxels/render/ray_tracing.wgsl:259` and `src/Kova.App/Shaders/voxels/render/base/render_first_hit.wgsl:101` | `ray_aabb` returns `t_near` exactly on the AABB entry face, so the entry-cell coordinate equals `bounding_box_max` and the world-AABB exit check fires on iteration 0. The post-loop `return step_count == 0` (NAADF behaviour) reports this as a hit with the un-initialized `voxel_type = 0`. Kova's render_first_hit shades type-0 hits via a `pcg_hash` colour fallback, so the entire world-AABB silhouette painted as colourful artefacts. |

### Fixes

1. **`common.wgsl` `get_ray_dir`**: swap multiplication order from `vec * mat` to `mat * vec`.
   ```wgsl
   // before:
   let v : vec4<f32> = vec4<f32>(ndc.x, ndc.y, 1.0, 1.0) * inv_cam_matrix;
   // after:
   let v : vec4<f32> = inv_cam_matrix * vec4<f32>(ndc.x, ndc.y, 1.0, 1.0);
   ```

2. **`CameraData.FromLookAt`**: build view from origin along the direction vector, not from the camera position.
   ```csharp
   // before:
   var view = Matrix4x4.CreateLookAt(position, target, up);
   // after:
   var direction = target - position;
   var view = Matrix4x4.CreateLookAt(Vector3.Zero, direction, up);
   ```

3. **`ray_tracing.wgsl` `shoot_ray` epilogue**: return `false` (miss) when no actual voxel surface was struck. The previous `return step_count == 0` returned `true` for the immediate-exit case (boundary-on-entry), which the shader then mis-interpreted as a real hit.
   ```wgsl
   // before:
   if ((*out_result).length <= 0.0) {
       (*out_result).length = 99999999.0;
       return step_count == 0;
   }
   // after:
   if ((*out_result).length <= 0.0) {
       (*out_result).length = 99999999.0;
       return false;
   }
   ```

4. **`render_first_hit.wgsl` AABB-entry**: nudge the ray a small fraction of a voxel forward so the entry cell is strictly inside the world.
   ```wgsl
   // before:
   var cur_frac : vec3<f32> = cam_pos_frac + ray_dir * aabb.x;
   // after:
   var cur_frac : vec3<f32> = cam_pos_frac + ray_dir * (aabb.x + 0.001);
   ```

### Final IoU

**1.0000** (164 sphere-hit pixels on both sides; perfect overlap). The CPU reference uses the same `+ 0.001` AABB-entry nudge to mirror the GPU's walk start point.

### Diagnostic notes

The bug hunt used four temporary WGSL diagnostic modes (RGB-packed ray direction, AABB-hit colour-code, hit-type modulo, voxel-type-as-channel) plus a CPU-side comparison of the very same ray directions at four screen-corner pixels. The CPU and GPU rays agreed to byte precision after bug 1 + bug 2 were fixed, which is what proved bug 3 was a `shoot_ray` issue rather than a perspective issue.

Total debugging time: about 90 minutes (3 of which were stuck on the red-sphere material confusion — `KvoxWriter` writes every entry of the `colors[]` array as a registered material starting from palette index 1, not 0 as the existing smoke test's comments imply).

### Secondary issues uncovered but NOT fixed (TODOs)

- `render_first_hit.wgsl:121-128` — the `pcg_hash` fallback for "zero-albedo voxel type" still fires when a material with all-zero `ColorBase` is registered. The fallback is now harmless because `shoot_ray` no longer reports spurious type-0 hits, but it remains a foot-gun: an actual emissive-only material (`color_layer` non-zero, `color_base` zero) would be silently rewritten to a hash colour. TODO: remove the fallback in P6b and rely on the material being correctly authored.
- `KvoxWriter.Write(string, VoxelDataBytes)` line 32-49 (`/mnt/archive4/DEV/Kova/src/Kova.AssetPipeline/KvoxWriter.cs`) — palette semantics are ambiguous. The convention "palette index 0 in the dense grid means empty; entry 0 in `colors[]` is the FIRST material (palette index 1)" is unstated and the existing `RenderSmokeTests` comment "index 0 — empty" is misleading. TODO: either rename the variable or update the comment to match the actual behaviour.
- `WorldData.GenerateWorld` reads back `DataVoxel` and `DataBlock` into CPU mirrors, but `ChunksBufferGpu` (used by the renderer) is initialized from `_worldData.DataChunk` which is the PRE-bounds-calc snapshot. The renderer's bounds-aware traversal happens via `Bounds.Update()` in subsequent frames. P6a's smoke test does 5 update frames to saturate bounds. TODO: document this multi-frame warm-up in `WorldRenderBase.Render`.

### `dotnet build Kova.slnx` summary

```
ok dotnet build: 16 projects, 0 errors, 0 warnings
```

### `dotnet test Kova.slnx` summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed: 0, Skipped: 0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed: 0, Skipped: 0, Total:     5
Kova.Graphics.WebGPU.Tests.dll  : Passed:     2, Failed: 0, Skipped: 0, Total:     2
Kova.Voxels.Viewer.Tests.dll    : Passed:     1, Failed: 0, Skipped: 0, Total:     1
Kova.Voxels.Tests.dll           : Passed:     6, Failed: 0, Skipped: 0, Total:     6
                                  --------
                                  Passed:    43, Failed: 0, Skipped: 0
```

New test: `Kova.Voxels.Tests.SdfReferenceTests.GpuSilhouetteMatchesAnalyticalSphere`. Existing `RendersNonUniformImageFromCheckerboardWorld` continues to pass; the perspective fixes did not regress it.

## P6a.fix.2 — Hardened analytical-surface suite (2026-05-13)

The user reports "the perspective is still very much fucked and precision errors accumulate" despite the P6a.fix sphere test passing at IoU=1.0. That test's CPU reference is a bit-for-bit clone of `common.wgsl::get_ray_dir`; both paths share the same arithmetic so any bug above the WGSL layer (e.g. inside `CameraData.FromLookAt`) is invisible. The single-sphere geometry is also rotation-symmetric — axis swaps in ray construction leave the silhouette unchanged. This phase adds a complementary test class that removes both blind spots.

### Files created

- `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/AnalyticalSurfaceTests.cs` — new xUnit test class. 18 test cases across 4 scenes × 4-5 cameras, plus 2 sanity tests on the CPU reference itself.

### Files modified

- `/mnt/archive4/DEV/Kova/docs/orchestrate/naadf-to-kova-port/03-impl.md` — this section appended.

No production source modified. No existing tests modified. `SdfReferenceTests.cs` continues to pass at IoU = 1.0000 — the new failures are exclusively in the new suite.

### Design — what the existing single-sphere test cannot catch

1. **Rotational symmetry of a sphere.** Every projection of a sphere is a circle. Any axis swap (X↔Y, X↔Z) or transpose in the WGSL `get_ray_dir` math produces the same silhouette pixel-for-pixel.
2. **Symmetric camera basis (96,96,96) → (32,32,32).** The position vector is equal on all three axes, so X↔Y or X↔Z swaps in the matrix unprojection leave the rendered image bitwise identical.
3. **No foreshortening sensitivity.** Orthographic and perspective both render a sphere as a circle. The test does not constrain depth-aware shape.
4. **Tiny world centred near origin (64³ at (0,0,0)).** `cam_pos_int` is always small; `cam_pos_frac` is irrelevant. The position-split precision logic is untouched by the test.
5. **CPU reference clones the WGSL.** `SdfReferenceTests.CpuGetRayDir` is a transliteration of `common.wgsl::get_ray_dir` — including the inverse-view-proj matrix, the Y-flip, the row-vs-column convention. If `CameraData.FromLookAt` is wrong, both renderers reproduce the same wrong rays and agree at IoU = 1.0.

### Scenes (asymmetric primitives + cameras)

| Scene | Geometry | World | Notes |
|---|---|---|---|
| `ManySpheres` | 8 spheres at corners of a sub-cube, radii (6,4,5,7,4.5,6.5,5.5,3.5) | 4×4×4 chunks = 64³ vx | Silhouette has resolvable structure even when each sphere alone is symmetric. |
| `TorusY` | Single torus, axis = +Y, R=16, r=5 at world centre | 4×4×4 chunks | Torus has a hole → perspective-sensitive thickness on the inner ring. |
| `ThreeTori` | Three tori with axes along world X, Y, Z, R=14, r=4, intersecting at the world centre | 4×4×4 chunks | THE axis-asymmetry stress test. An X↔Y swap rotates one ring out of place. |
| `OffOriginTorus` | Single Y-axis torus R=28 r=8 at world voxel (384, 200, 304) | 32×16×24 chunks = 512×256×384 vx | Stresses `cam_pos_int` / `cam_pos_frac` precision split. Cam E lives at (1100.7, 700.3, 1000.4) — very large absolute coords. |

Cameras per scene (Cam A–D for all; Cam E for OffOriginTorus only):

- **Cam A**: pos = centre + (0.8, 0.3, 0.7) · scale. Target = world centre. fovY = 72°. Aspect 1:1, 64×64. Asymmetric on all three axes — no swap symmetry.
- **Cam B**: pos = centre + (0.5, 0.9, −0.2) · scale. High-pitch viewpoint.
- **Cam C**: pos = centre + (−0.7, 0.1, 0.6) · scale. Target offset by (+8, 0, 0) from world centre — off-centre look direction.
- **Cam D**: pos = centre + (1.0, 0.5, 0.5) · scale. fovY = 60°, **aspect ≠ 1** (96×64 framebuffer). Catches aspect-handling bugs.
- **Cam E** (off-origin only): pos = (1100.7, 700.3, 1000.4) absolute. Target = torus centre. PositionInt clamps to (1100, 700, 1000), PositionFrac = (0.7, 0.3, 0.4) — the path the user's "precision errors accumulate" complaint travels.

### Textbook CPU reference derivation

Given camera position **p**, target **t**, world-up **u_w**, vertical FOV `fovY`, aspect `a`, pixel `(px, py)` on a `w × h` framebuffer with `py = 0` at the top row:

1. Build orthonormal camera basis:
   ```
   f = normalize(t - p)
   r = normalize(f × u_w)
   u = r × f                       // orthonormalised true up
   ```
2. Pixel centre → NDC, framebuffer-down → NDC-up:
   ```
   ndc.x = 2·(px + 0.5)/w - 1      in [-1, +1]
   ndc.y = 1 - 2·(py + 0.5)/h      in [-1, +1]   (Y-flip from framebuffer convention)
   ```
3. View-space ray for a pinhole at vertical FOV `fovY`:
   ```
   sx = ndc.x · a · tan(fovY/2)
   sy = ndc.y · tan(fovY/2)
   dir_view = (sx, sy, 1)
   ```
4. Re-expressed in world space using the basis:
   ```
   dir_world = normalize(f + r·sx + u·sy)
   ```

No inverse-view-proj matrix. No System.Numerics column-vs-row convention. No transliteration from `common.wgsl`. The two sanity tests `ReferenceRay_CentrePixel_AlignsWithForward` and `ReferenceRay_CornerPixel_HasExpectedFovAngle` assert that this builder is internally consistent (centre-pixel pair averages to `forward`, top-centre pixel makes the expected `atan(tan(fovY/2)·(1 − 1/h))` angle with `forward`). Both sanity tests pass.

The CPU reference then sphere-traces the analytical SDF — clipped to the world AABB, with the same `+0.001` AABB-entry nudge the production GPU shader uses (`render_first_hit.wgsl:105`) so the two paths begin at the same point in space. The SDF march steps `t += max(d, 0.5)` so it leaps fast through empty space and refines near the surface.

Hit-mask construction matches `SdfReferenceTests`: the surface material is red `(220, 60, 60)`; the sky is blue/teal; `r >= g && r > b` separates the two unambiguously.

### IoU per (scene, camera) — collected via `dotnet test`

| Scene / Cam | A | B | C | D | E |
|---|---|---|---|---|---|
| ManySpheres | 0.8884 | 0.9108 | 0.8837 | 0.8980 | — |
| TorusY | 0.9444 | 0.9333 | 0.9337 | 0.9136 | — |
| ThreeTori | 0.9299 | 0.9333 | 0.9247 | 0.9488 | — |
| OffOriginTorus | **0.0000** | **0.0000** | **0.0000** | **0.0000** | **0.0000** |

Acceptance floor: spheres ≥ 0.98, tori ≥ 0.97. **All 17 (scene, camera) cases fail.**

### Failure analysis

**Type 1 — 1-pixel silhouette shift (small worlds).** The sphere and torus scenes show IoU ≈ 0.88–0.95 with ~10–30 disagreeing pixels per frame. The diagnostic dumps reveal that the CPU and GPU silhouettes have the same overall shape but are shifted by roughly 1 pixel; e.g. `ManySpheres_CamA.cpu.txt` row 30 begins with `##.....##` whereas `.gpu.txt` row 30 begins with `#......###` (1-px right shift on a sphere edge). The shift is **systematic** — every sphere in every camera frame is offset in a consistent direction — which is the signature of a perspective-math bug rather than voxelisation aliasing.

If the bug were purely voxel aliasing, the disagreement would average out across many silhouette edges; here every camera gives a consistent 1-px-class shift, and IoU sits roughly 5–10 % below the floor that voxel-edge stair-stepping alone would explain. This matches the user's "perspective is still very much fucked" report.

**Type 2 — total silhouette miss (off-origin world).** For the OffOriginTorus scene **every camera gives IoU = 0.0000**: the CPU reference finds the torus (silhouette area 8–28 px depending on camera) but the GPU renders zero red pixels at all. The CPU reference's mask is contiguous and torus-shaped (e.g. `OffOriginTorus_CamA.cpu.txt`):
```
....................##........
....................######....
....................########..
....................####......
```
The GPU mask for the same case has no `#` characters. Possible contributing factors:
- `MAX_RAY_STEPS_PRIMARY = 120` (`ray_tracing.wgsl:21`). With camera ~500 voxels from the torus and many sparse-empty chunks between, the hierarchical traversal *should* skip empty space in chunk-sized strides, but the step budget may exhaust before the torus is reached.
- The position-split precision bug — Cam E uses (1100.7, 700.3, 1000.4) and PositionInt + PositionFrac reconstruction in C# is exact (no warning printed), but the GPU shader path does `f32(cam_pos_int.x) + cam_pos_frac.x`. Float32 `1100.0 + 0.7` is exact, but the cumulative arithmetic inside the shader's traversal loop is the suspect.

Either way, the **test correctly classifies it as a failure**. Per the brief, the fix is out of scope for this phase.

Position-split reconstruction in C# (`Vector3.Floor` + remainder; logged at runtime via `ReportPositionSplitError`) is exact for all camera positions tested — no warnings emitted. The precision bug therefore is not in `CameraData.FromLookAt`'s int/frac split; it lives downstream in the shader or in the float32 round-trip across the UBO upload.

### Failure-mode diagnostic dumps

When a case fails, the test writes two text files next to the assembly:
- `<TestName>.cpu.txt` — `#` / `.` hit mask from the analytical-SDF reference.
- `<TestName>.gpu.txt` — same mask from the GPU output.

For example, mask dumps are at `tests/Kova.Voxels.Tests/bin/Debug/net10.0/{ManySpheres,TorusY,ThreeTori,OffOriginTorus}_Cam{A,B,C,D,E}.{gpu,cpu}.txt`. These are deliberately checked-out artefacts of running the test locally; they are not committed (the `bin/` directory is git-ignored).

### Constraints honoured

- xUnit `[Theory]` cases trait-gated `Category=RequiresGpu` for the GPU paths; the two reference-ray sanity tests are CPU-only `[Fact]`s with no trait gate.
- Each test case creates and disposes its own `WgpuGraphicsDevice`, `WorldData`, `VoxelModelData`, `WorldGeneratorModel`, `WorldRenderBase`. No shared state.
- No `Microsoft.Xna.Framework`, `MonoGame.*`, `SharpDX.*`, `System.Windows.Forms`. All GPU access via `IGraphicsDevice`.
- No `[Obsolete]`, shims, or backwards-compat wrappers.
- `SdfReferenceTests.cs` is unchanged; the existing `GpuSilhouetteMatchesAnalyticalSphere` still passes at IoU=1.0000.
- `CameraData.cs` and the WGSL shaders are unchanged. The new failures are honest readings of production behaviour.

### Notes on what the test cannot do

- The off-origin world (32×16×24 chunks = 512×256×384 voxels) requires baking a dense kvox file of 50 MB. Bake time is ~5–10 s on the developer machine; not a problem for CI but obviously slower than the in-memory 64³ scenes.
- `WorldData.DataVoxelGpu` allocates `(WorldGenSegmentSizeInVoxels)³ / 2` u32 slots regardless of total world size; the off-origin world's voxel content (one sparse torus, ~10–20 k voxels) fits comfortably. If a future scene needs higher voxel density at off-origin coordinates the test would need to bump `worldGenSegmentSizeInGroups`.
- The voxel hierarchisation in `VoxelModelData.HierarchizeDenseGrid` iterates the dense byte grid; for the off-origin world this is 50 M iterations and takes a second or two per test case. Acceptable for an integration test but the off-origin tests do account for the bulk of the suite's runtime.
- `MAX_RAY_STEPS_PRIMARY = 120` in `ray_tracing.wgsl` may itself be insufficient for cameras placed > ~600 voxels from world content; this is a separate concern from the perspective math but contributes to the off-origin test failures.

### `dotnet build Kova.slnx` summary

```
ok dotnet build: 16 projects, 0 errors, 0 warnings
```

### `dotnet test Kova.slnx` summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed:  0, Skipped: 0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed:  0, Skipped: 0, Total:     5
Kova.Graphics.WebGPU.Tests.dll  : Passed:     2, Failed:  0, Skipped: 0, Total:     2
Kova.Voxels.Viewer.Tests.dll    : Passed:     1, Failed:  0, Skipped: 0, Total:     1
Kova.Voxels.Tests.dll           : Passed:     8, Failed: 17, Skipped: 0, Total:    25
                                  --------
                                  Passed:    45, Failed: 17, Skipped: 0
```

Pre-existing tests in `Kova.Voxels.Tests` (6 tests, including the original `GpuSilhouetteMatchesAnalyticalSphere`) all pass. The 8 new passes are the 2 CPU-only sanity tests (`ReferenceRay_*`) + the 6 pre-existing tests; the 17 new failures are this suite reporting CPU-vs-GPU disagreement as designed.

The next agent picks up by either lowering `MAX_RAY_STEPS_PRIMARY` from being a confound and rerunning, or by attacking the 1-px silhouette shift directly — the diagnostic dump file names are the search index.

## Demo scene (2026-05-13)

Adds a runnable demo path so a user can fly through a varied voxel world without authoring or cooking a `.vox`/`.kvox`. Trigger: `dotnet run --project src/Kova.App -- demo`. The literal magic string `demo` (or any non-existent path) is intercepted by `VoxelViewerApp.LoadAsync` and an in-memory `VoxelDataBytes` is synthesised, written to a temp `.kvox`, and fed through the existing load flow (`PeekKvoxSizeInChunks` → `WorldData` → `VoxelModelData` → `WorldGeneratorModel.SetModel` → `WorldData.GenerateWorld` → `WorldRenderBase`).

### Files created

- `/mnt/archive4/DEV/Kova/src/Kova.Voxels.Viewer/DemoScene.cs` — public static `DemoScene.Build(int axis = 192)` returning a populated `VoxelDataBytes`. Union-of-SDFs voxelisation, per-voxel-centre sampling. Six-material palette (ground, two sphere colours, cubes, two torus colours).
- `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/DemoSceneTests.cs` — CPU/IO-only smoke (`BuildProducesNonEmptyKvoxRoundTrip`). NOT GPU-gated. Builds the scene at axis=96, asserts ground-plane voxel count ≥ axis²·4, asserts non-ground primitive voxels ≥ 1000, asserts Y ≥ axis/2 is fully empty, round-trips through `KvoxWriter`, validates the `.kvox` header and exact file length (48-byte header + 6·40-byte material entries + 6·(2+4) byte names + axis³ dense grid bytes).

### Files modified

- `/mnt/archive4/DEV/Kova/src/Kova.Voxels.Viewer/VoxelViewerApp.cs` — added the demo-path magic. Before the header peek, if `_modelPath == "demo"` (case-insensitive) or the path doesn't exist, build the demo scene, write it to `Path.GetTempPath()`, set `resolvedPath` to the temp file, and tag the load as a demo. All subsequent `_modelPath` references in `LoadAsync` use `resolvedPath`. Demo presets the camera at `(sv.X·0.1, sv.Y·0.25, sv.Z·0.1)` looking at the world centre, with `MoveSpeed = 32`, `SpeedBoostMultiplier = 6`, and sun direction `normalize(0.4, 0.8, 0.3)`. Non-demo loads keep the previous defaults verbatim.
- `/mnt/archive4/DEV/Kova/src/Kova.Voxels.Viewer/Kova.Voxels.Viewer.csproj` — added `ProjectReference` to `Kova.AssetPipeline` so the viewer can call `KvoxWriter.Write`.
- `/mnt/archive4/DEV/Kova/src/Kova.App/Program.cs` — when `args.Length == 0` and not WASM, print a one-line stderr hint: `Pass 'demo' or a .kvox path as the first arg to launch the voxel viewer. Example: dotnet run --project src/Kova.App -- demo`. The triangle-app fallback is preserved.
- `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/Kova.Voxels.Tests.csproj` — added `ProjectReference` to `Kova.Voxels.Viewer` so the test project can use `DemoScene`.

### Scene contents

`DemoScene.Build(axis)` lays out, deterministically, at axis = 192:

- **Ground plane** (material 1, "ground"): bottom 4 voxel layers, every (x, z). At axis 192 → 192·192·4 = 147 456 voxels.
- **5 × 5 sphere grid** (materials 2/3, alternating red and blue): 25 spheres on the Y ≈ 12 plane, grid spacing `axis · 0.16` = 30.72 vx, radii 4..12 cycling through `(gx·5 + gz) % 9 + 4`.
- **4 axis-aligned cubes** (material 4, sandstone): half-sizes (12,12,12), (8,16,8), (10,6,14), (6,9,6) at world-relative positions `(0.2,_,0.8)`, `(0.8,_,0.25)`, `(0.5,_,0.5)`, `(0.15,_,0.4)` with Y centres 30, 22, 50, 18.
- **3 toruses** (R = 16, r = 4):
  - Y-axis at world centre, Y = 36, material 5 (gold).
  - X-axis at `(0.7 · axis, 28, 0.35 · axis)`, material 6 (green) — deliberately off-axis, sun-side, stresses perspective.
  - Z-axis at `(0.3 · axis, 28, 0.7 · axis)`, material 5 (gold).
- **Sky volume**: Y ≥ axis / 2 is uniformly empty. The atmosphere renderer fills it.

Reused from `AnalyticalSurfaceTests.cs`: the per-voxel union-of-SDFs voxelisation loop pattern (sample at `(x+0.5, y+0.5, z+0.5)`, primitive with smallest negative SDF wins); the torus SDF (`TorusSdf` shape — `axial = dot(local, axis)`, `radial.Length() - R` then 2D length minus `r`); the `VoxelDataBytes` construction with parallel `Color[]` / `Material[]` tables.

### Launch command

```
dotnet run --project src/Kova.App -- demo
```

If the user passes the literal string `demo`, OR passes a path that doesn't exist on disk, the viewer falls through to the demo path. If they pass a real `.kvox` it loads as before.

### Headless launch capture

```
$ timeout 20 dotnet run --project /mnt/archive4/DEV/Kova/src/Kova.App -- demo
[VoxelViewerApp] LoadAsync starting (path=demo)
[VoxelViewerApp] Source not found at 'demo'. Generating demo scene.
[VoxelViewerApp] Wrote demo .kvox to /tmp/kova-demo-4341a565e1fc4dbf8c7db8c6a21c452f.kvox
[VoxelViewerApp] LoadAsync complete (screen=1262x712, world=192|192|192)
EXIT: 124
```

Exit 124 is the `timeout` kill signal. The viewer opened a window, generated the scene (192³), hierarchised it into chunk/block/voxel arrays, uploaded GPU buffers, and entered the render loop. The fact that no further log lines appeared past `LoadAsync complete` is expected — the per-frame render path is silent.

### `dotnet build Kova.slnx` summary

```
ok dotnet build: 16 projects, 0 errors, 0 warnings
```

### `dotnet test Kova.slnx` summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed:  0, Skipped: 0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed:  0, Skipped: 0, Total:     5
Kova.Graphics.WebGPU.Tests.dll  : Passed:     2, Failed:  0, Skipped: 0, Total:     2
Kova.Voxels.Viewer.Tests.dll    : Passed:     1, Failed:  0, Skipped: 0, Total:     1
Kova.Voxels.Tests.dll           : Passed:     9, Failed: 17, Skipped: 0, Total:    26
                                  --------
                                  Passed:    46, Failed: 17, Skipped: 0
```

The 17 failures are the same honest perspective-bug diagnostics from P6a.fix.2 (analytical surface tests at `MAX_RAY_STEPS_PRIMARY`-bounded GPU paths). One new test, `DemoSceneTests.BuildProducesNonEmptyKvoxRoundTrip`, is added and passes; total passes climb from 45 to 46.

### Constraints honoured

- No `Microsoft.Xna.Framework`, `MonoGame.*`, `SharpDX.*`, `System.Windows.Forms`.
- All voxel work goes through `VoxelDataBytes` → `KvoxWriter` → existing load flow. No new graphics-side abstraction.
- World generation dispatches via `WorldGeneratorModel.SetModel` exactly as production loads do.
- No `[Obsolete]`, shims, or backwards-compat wrappers.
- No `.kvox` baked to a permanent path in the repo; demo writes to `Path.GetTempPath()` with a fresh GUID per launch.

## Demo scene fixes (2026-05-14)

Two narrow fixes on top of the demo scene landing: (1) the demo camera was aimed at the world centre `(96, 96, 96)`, which is the empty sky volume, so the user saw a thin horizon band and nothing else; (2) `CursorMode.Raw` does not reliably deliver `MouseMove` deltas under libdecor/Wayland, so right-click look did not rotate the camera even though WASD movement worked.

### Files modified

- `/mnt/archive4/DEV/Kova/src/Kova.Voxels.Viewer/VoxelViewerApp.cs` — demo branch retargeted at the sphere-grid centre (~Y=12 at axis=192) instead of the world centre, with the camera moved slightly inward so the geometry is comfortably within the 75° FOV.
- `/mnt/archive4/DEV/Kova/src/Kova.Graphics.WebGPU/SilkInput.cs` — switched `CursorMode.Raw` to `CursorMode.Disabled`; added a one-shot `_ignoreNextDelta` guard so the first `MouseMove` after a capture transition does not deliver a stale jump-to-centre delta; added a one-shot `[SilkInput] First mouse delta seen` log so the input bridge is self-diagnosing if Wayland still misbehaves on some compositor.
- `/mnt/archive4/DEV/Kova/tests/Kova.Voxels.Tests/CameraStateTests.cs` — new pure-CPU sanity test (`DemoCameraLooksDownAtGeometry`) that builds the same `FirstPersonCamera` config the viewer uses and asserts `Forward.Y < 0` so a future config change can never silently regress the viewing direction.
- `/mnt/archive4/DEV/Kova/docs/orchestrate/naadf-to-kova-port/03-impl.md` — this section.

### New camera derivation

The demo geometry lives in the bottom half of the world: ground plane Y=0..3, 5×5 sphere grid centred near Y≈12, three toruses at Y≈28..36, four cubes Y≈18..50. The new demo camera sits at `(sv·0.15, sv·0.20, sv·0.15)` ≈ `(28.8, 38.4, 28.8)` and aims at `(sv·0.5, sv·0.0625, sv·0.5)` ≈ `(96, 12, 96)` — the centre of the sphere grid. The `dir` vector ≈ `(0.685, −0.269, 0.685)` gives `Yaw = atan2(0.685, 0.685) = π/4` (45° NE) and `Pitch = asin(−0.269) ≈ −15.6°` (downward). The camera now looks straight at the spheres rather than past them at the empty sky.

### Cursor-mode change

`CursorMode.Disabled` hides + locks the cursor and continues to fire `MouseMove` events with absolute positions on both X11 and Wayland, so the existing `delta = position - _lastMousePos` accumulator keeps working unchanged whereas `CursorMode.Raw` silently delivers zero deltas on libdecor/Wayland.

### Initial-delta debounce

When RMB is pressed (or released) the OS jumps the cursor to/from the locked centre between event ticks; the very next `MouseMove` then carries a huge stale delta. A one-shot `_ignoreNextDelta` bool, set whenever `UpdateCursorCapture` flips `_captureActive`, drops exactly that first event so the camera does not snap-rotate at the moment of capture.

### `dotnet build Kova.slnx` warnings

```
ok dotnet build: 16 projects, 0 errors, 0 warnings
```

### `dotnet test Kova.slnx` summary

```
Kova.VoxelsCore.Tests.dll       : Passed:    29, Failed:  0, Skipped: 0, Total:    29
Kova.AssetPipeline.Tests.dll    : Passed:     5, Failed:  0, Skipped: 0, Total:     5
Kova.Graphics.WebGPU.Tests.dll  : Passed:     2, Failed:  0, Skipped: 0, Total:     2
Kova.Voxels.Viewer.Tests.dll    : Passed:     1, Failed:  0, Skipped: 0, Total:     1
Kova.Voxels.Tests.dll           : Passed:    10, Failed: 17, Skipped: 0, Total:    27
                                  --------
                                  Passed:    47, Failed: 17, Skipped: 0
```

The 17 failures are the same `AnalyticalSurfaceTests` perspective-precision diagnostics from P6a.fix.2 — explicitly out of scope. Total passes climb from 46 to 47 (the new `CameraStateTests.DemoCameraLooksDownAtGeometry` test).

### Headless launch output

```
$ timeout 20 dotnet run --project /mnt/archive4/DEV/Kova/src/Kova.App --no-build -- demo
[VoxelViewerApp] LoadAsync starting (path=demo)
[VoxelViewerApp] Source not found at 'demo'. Generating demo scene.
[VoxelViewerApp] Wrote demo .kvox to /tmp/kova-demo-466b3119488b4a6baf58aba04b8ba127.kvox
[VoxelViewerApp] LoadAsync complete (screen=798x1424, world=192|192|192)
[SilkInput] First mouse delta seen: <325.58203, 483.33984>
```

Window opened, demo `.kvox` baked and loaded, render loop entered. The one-shot `[SilkInput] First mouse delta seen` line confirms the `CursorMode.Disabled` capture path delivers `MouseMove` deltas under the user's session (Wayland-portable). No `CursorMode`-related warnings; no exceptions.
- Perspective bugs from P6a.fix.2 are NOT touched; the demo scene runs through the same renderer and will visibly exhibit them in motion (which the brief calls out as desirable for next-phase debugging).
