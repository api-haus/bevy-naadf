# 02-csharp-reference — C# NAADF reference for Oasis .vox wrap pipeline

## C# reference findings (2026-05-19)

---

## The Oasis .vox load + placement + wrap pipeline (C# NAADF)

### 1. Entry point — `WorldHandler.Initialize()`

**File:** `World/WorldHandler.cs:29–55`

```csharp
public void Initialize()
{
    worldData = new WorldData(
        worldSizeToUseInWorldGenSegments * worldGenSegmentSizeInGroups * 64,
        worldGenSegmentSizeInGroups);   // ← single call that fixes world size

    LoadModelScene("Content\\oasis.cvox");   // ← asset load
}
```

The world size is fixed at startup by two constants on lines 18–19 of the same file:

```csharp
public int worldGenSegmentSizeInGroups = 4;                        // line 18
public Point3 worldSizeToUseInWorldGenSegments = new Point3(16, 2, 16);  // line 19
```

### 2. World size derivation

`WorldData` constructor (`World/Data/WorldData.cs:57–93`) computes:

```
wantedSizeInVoxels = (16,2,16) × 4 × 64 = (4096, 512, 4096)

worldGenSegmentSizeInChunks = 4 × 4         = 16
worldGenSegmentSizeInVoxels = 16 × 16       = 256

sizeInWorldGenSegments = ceil(actualSizeInVoxels / 256)
                       = (16, 2, 16)         (exact — no rounding needed)
sizeInVoxels           = (16,2,16) × 256   = (4096, 512, 4096)
sizeInChunks           = sizeInVoxels / 16  = (256, 32, 256)
```

The bounding box passed to all render shaders (`WorldData.setEffect`, line 477–478):
```
boundingBoxMin = (0.1, 0.1, 0.1)
boundingBoxMax = sizeInVoxels − (0.1, 0.1, 0.1) = (4095.9, 511.9, 4095.9)
```

### 3. Model loading — `LoadModelScene("Content\oasis.cvox")`

**File:** `World/WorldHandler.cs:37–55`  
**Model loader:** `World/Model/ModelData.cs:181–258` (`ModelData.Load`)

`oasis.cvox` is a zipped binary. Verified dimensions (read from binary header):

| field | value |
|---|---|
| format version | 3 |
| size X (voxels) | 1033 |
| size Y (voxels) | 386 |
| size Z (voxels) | 1082 |
| chunk count | 110 500 |

`ModelData` computes `sizeInChunks = ceil(modelSize / 16)` (`ModelData.cs:40`):

```
sizeInChunksX = ceil(1033 / 16) = 65    (wrap extent X = 65 × 16 = 1040 voxels)
sizeInChunksY = ceil(386  / 16) = 25    (wrap extent Y = 25 × 16 = 400  voxels)
sizeInChunksZ = ceil(1082 / 16) = 68    (wrap extent Z = 68 × 16 = 1088 voxels)
```

Confirmed: `65 × 25 × 68 = 110 500 = chunkCount` — consistent.

### 4. World generation — `WorldData.GenerateWorld(worldGenerator)`

**File:** `World/Data/WorldData.cs:120–218`

Iterates over all `(sx, sy, sz)` world-gen segments:

```csharp
for (int z = 0; z < sizeInWorldGenSegments.Z; ++z)   // 0..15
for (int y = 0; y < sizeInWorldGenSegments.Y; ++y)   // 0..1
for (int x = 0; x < sizeInWorldGenSegments.X; ++x)   // 0..15
{
    worldGenerator.CopyToChunkData(segmentPosInChunks, segmentSize, actualSizeInVoxels, …);
    CalculateChunkBlocks(segmentPosInChunks);
}
```

`WorldGeneratorModel.CopyToChunkData` (`World/Generator/WorldGeneratorModel.cs:32–60`) passes:
- `sizeInVoxels = actualSizeInVoxels = (4096, 512, 4096)` — the **world** bounds
- `modelSizeInChunks = (65, 25, 68)` — the **model** chunk extent

### 5. The wrap — `generatorModel.fx::getVoxelDataInModel`

**File:** `Content/shaders/world/generator/generatorModel.fx:16–52`

```hlsl
uint3 voxelPosInModel = voxelPos % (int3(modelSizeInChunksX, modelSizeInChunksY, modelSizeInChunksZ) * 16);
uint modelIndexY = voxelPos.y / (modelSizeInChunksY * 16);   // line 21

if (any(voxelPos >= uint3(sizeInVoxelsX, sizeInVoxelsY, sizeInVoxelsZ)))
    return 0;   // line 18-19 — out-of-world early-exit

// … fetch voxel from model using voxelPosInModel …

if (modelIndexY > 0)
    type = 0;   // line 48-49 — Y-clamp: only ground-level materialises
```

Two operations govern the tiling:

1. **Modulo wrap (`%`)** on line 20: maps any world voxel position into the model using `voxelPos % (modelSizeInChunks * 16)`. This tiles the model infinitely across the world; the world-extent guard (`line 18`) limits the tiles to the world AABB.
2. **Y-clamp** on line 48–49: zeroes out anything whose Y coordinate exceeds one model height. Because the world is 512 voxels tall and the model is 400 voxels tall (`25 * 16`), `modelIndexY` is `0` for all world-Y positions in [0..399] and `1` for [400..511] — the top 112 voxels of the world are empty sky. There is no vertical stacking.

### 6. Camera spawn

**File:** `World/Render/WorldRender.cs:48`

```csharp
camera.SetPos(new Vector3(500, 200, 40));
```

Absolute position in voxels, not proportional. The camera starts at voxel (500, 200, 40) inside the 4096×512×4096 world.

---

## Canonical scene-size / world-extent / wrap constants

| # | file:line | symbol / context | value | reads/consumers |
|---|---|---|---|---|
| 1 | `World/WorldHandler.cs:18` | `worldGenSegmentSizeInGroups` | `4` | `WorldData` ctor, `worldGenSegmentSizeInChunks = 4×4 = 16`, `worldGenSegmentSizeInVoxels = 256`. Drives segment dispatch loop in `GenerateWorld`. |
| 2 | `World/WorldHandler.cs:19` | `worldSizeToUseInWorldGenSegments` | `(16, 2, 16)` | `WorldData(wantedSizeInVoxels = (16,2,16)×4×64 = (4096,512,4096), ...)`. **The singular canonical constant.** |
| 3 | `Content/shaders/world/generator/generatorModel.fx:20` | `% (modelSizeInChunks * 16)` — no named constant | wrap divisor = `(1040, 400, 1088)` | Derived at dispatch time from `ModelData.sizeInChunks = (65,25,68)`. This is NOT a hard-coded "4"; it is `ceil(modelSize/16) × 16`. |
| 4 | `Content/shaders/world/generator/generatorModel.fx:48–49` | `if (modelIndexY > 0) type = 0` | Y-clamp at model height | Prevents vertical stacking; only one layer of the model materialises in Y. |

---

## How does C# derive "exactly 4" Oasis instances?

There is **no literal `4` anywhere** in the codebase. "4 copies" emerges from dividing the world extent by the model wrap-extent:

```
world_size_X      =  4096  voxels   (= worldSizeToUseInWorldGenSegments.X × 4 × 64)
wrap_extent_X     =  1040  voxels   (= ceil(1033/16) × 16 = 65 × 16)
continuous_tiles  =  4096 / 1040   =  3.9385…   →  4 visible copies (ceil = 4)

world_size_Z      =  4096  voxels
wrap_extent_Z     =  1088  voxels   (= ceil(1082/16) × 16 = 68 × 16)
continuous_tiles  =  4096 / 1088   =  3.7647…   →  4 visible copies (ceil = 4)
```

In both the X and Z axes, the model fits 3 complete tiles and then a partial 4th tile fills the rest of the world. The "exactly 4" the user reports is the *ceiling* count — you see 4 copies (the 4th is partial). Both axes round up to 4, which is why the user's perception is "exactly 4 modulo-wrapped instances."

### Why does the Bevy port show "~2.5 instances"?

The Bevy port loads **`oasis_hard_cover.vox`** (MagicaVoxel scene-graph format), which composes to a larger model: **1488 × 544 × 1344 voxels (93 × 34 × 84 chunks)**. In the same 4096-voxel world:

```
wrap_extent_X     =  1488  voxels   (= 93 × 16)
continuous_tiles  =  4096 / 1488   =  2.7527…   →  3 visible copies (ceil = 3)

wrap_extent_Z     =  1344  voxels   (= 84 × 16)
continuous_tiles  =  4096 / 1344   =  3.0476…   →  4 visible copies (ceil = 4)
```

The X-axis only fits 2 complete tiles + a partial 3rd (~2.75 tiles), matching the user's "~2.5" observation. The world size itself is correct (both use 4096×512×4096); the divergence is **entirely in the model file used** — `oasis.cvox` is smaller than `oasis_hard_cover.vox`.

This also means: the Bevy wrap constant infrastructure (`WORLD_SIZE_IN_SEGMENTS`, `WORLD_SIZE_IN_VOXELS`, the `generator_model.wgsl` modulo logic) is **correct and faithful to C#**. The only mismatch is the input model dimensions.

---

## Singular-constant verdict

**Yes, a single canonical constant controls the wrap factor:** `worldSizeToUseInWorldGenSegments = Point3(16, 2, 16)` at `World/WorldHandler.cs:19`. Combined with the fixed `worldGenSegmentSizeInGroups = 4` (line 18), this produces the `4096 × 512 × 4096` world. The wrap count then emerges implicitly from `world_size / (model_sizeInChunks × 16)` — it is not a separately named "wrap count" constant. The "4 instances" is a consequence of the `oasis.cvox` model fitting ~3.94 times in 4096 voxels, not a literal value encoded anywhere.

The architect must decide between:
- **A — value swap**: replace `oasis_hard_cover.vox` with an asset whose dimensions match `oasis.cvox` (1033×386×1082 voxels). This makes the wrap count ≈3.94 ≈ 4 in X and ≈3.76 ≈ 4 in Z, matching C# exactly.
- **B — no asset change**: the world-size constants are already correct. If the user is comparing the same `.vox` file in C# and Bevy, the "fix" is ensuring C# also uses `oasis_hard_cover.vox` as input, not `oasis.cvox` (which was baked from an earlier `.vox`).

The world-size constants in the Bevy port (`WORLD_SIZE_IN_SEGMENTS`, `WORLD_SIZE_IN_VOXELS`) are correct and do NOT need to be refactored.

---

## Cross-references for the Bevy mapping

| C# constant | C# location | Bevy equivalent | Bevy location |
|---|---|---|---|
| `worldSizeToUseInWorldGenSegments = (16,2,16)` | `WorldHandler.cs:19` | `WORLD_SIZE_IN_SEGMENTS = UVec3::new(16, 2, 16)` | `crates/bevy_naadf/src/lib.rs:235` |
| `worldGenSegmentSizeInGroups = 4` | `WorldHandler.cs:18` | `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS: u32 = 4` | `crates/bevy_naadf/src/lib.rs:241` |
| derived `sizeInVoxels = (4096,512,4096)` | `WorldData.cs:63` | `WORLD_SIZE_IN_VOXELS: UVec3 = (4096,512,4096)` | `crates/bevy_naadf/src/lib.rs:254` |
| `getVoxelDataInModel: voxelPos % (modelSizeInChunks*16)` | `generatorModel.fx:20` | `voxel_pos % model_extent_v` | `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl:70` |
| camera spawn `(500, 200, 40)` | `WorldRender.cs:48` | `from_world_voxels([WORLD_SIZE_IN_VOXELS])` — **diverges**: scales proportionally from 1024-base, not 4096-base | `crates/bevy_naadf/src/camera/mod.rs:55–65` |

### Camera placement note

C# places the camera at absolute voxel `(500, 200, 40)` in the 4096-voxel world. The Bevy formula computes `pos.x = world_width × (500/1024)` — the denominator `1024` is the old Phase-A test-grid size, not the 4096-voxel world. For the fixed world this gives `pos.x = 4096 × (500/1024) = 2000`, which differs from C#'s `500`. This is a **secondary** issue (affects which portion of the tiled scene is initially visible, but not the tile count). It is noted here as an additional Bevy–C# parity gap but is **out of scope for this audit** (the user's fix target is the wrap count, not the camera pose).

---

## Files surveyed

**C# source:**
- `World/WorldHandler.cs`
- `World/Data/WorldData.cs`
- `World/Generator/WorldGeneratorModel.cs`
- `World/Generator/WorldGenerator.cs`
- `World/Model/ModelData.cs`
- `World/Render/WorldRender.cs`
- `World/Render/Versions/WorldRenderAlbedo.cs`
- `Settings.cs`
- `App.cs`
- `Gui/Main/HeaderBar/UiHeaderBar.cs`
- `Content/shaders/world/generator/generatorModel.fx`
- `Content/shaders/render/rayTracing.fxh`
- `Content/shaders/render/common/common.fxh`
- `Content/shaders/render/common/commonConstants.fxh`
- `Content/shaders/render/common/commonRayTracing.fxh`
- `Content/shaders/render/common/commonRenderPipeline.fxh`
- `Content/shaders/render/versions/albedo/renderFirstHit.fx`
- `Content/shaders/render/common/atmosphere/atmospherePrecomputed.fxh` (not read — not relevant)

**C# asset (binary read):**
- `Content/oasis.cvox` — header parsed to extract `(sizeX, sizeY, sizeZ) = (1033, 386, 1082)`

**Bevy port (cross-reference):**
- `crates/bevy_naadf/src/lib.rs` — `WORLD_SIZE_IN_SEGMENTS`, `WORLD_SIZE_IN_CHUNKS`, `WORLD_SIZE_IN_VOXELS`, `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS`
- `crates/bevy_naadf/src/voxel/grid.rs` — `install_imported_vox`, `install_vox_in_fixed_world`
- `crates/bevy_naadf/src/voxel/vox_import.rs` — `compose_to_sparse_world`, dimension caps, Oasis size comment
- `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl` — wrap logic
- `crates/bevy_naadf/src/render/construction/mod.rs` — GPU producer params, Oasis dimension comment
- `crates/bevy_naadf/src/render/prepare.rs` — `bounding_box_max` assembly
- `crates/bevy_naadf/src/camera/mod.rs` — `InitialCameraPose::from_world_voxels`
- `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` — SIZE chunks scanned: composed world = 93×34×84 chunks = 1488×544×1344 voxels
