# 00-reuse-audit — Bevy-side scan for Oasis .vox instance-count

## delegate-auditor findings (2026-05-19)

---

## The Oasis .vox load + placement + wrap pipeline (Bevy)

### 1. Entry point — CLI/preset dispatch

`setup_test_grid` (`crates/bevy_naadf/src/voxel/grid.rs:104`) is a Bevy Startup
system. It matches on `AppArgs.grid_preset`:

- `GridPreset::Vox { path }` (the Oasis `.vox` case) → calls
  `install_vox_in_fixed_world(&mut commands, path)` (`grid.rs:136`).
- `vox_gpu_oracle_cpu_phase = true` (test-only) → `install_vox_sized_to_model` (`grid.rs:134`).

### 2. Parse — `install_vox_in_fixed_world` → `parse_to_imported_vox`

`install_vox_in_fixed_world` (`grid.rs:422`) reads bytes from disk then calls
`install_vox_bytes_in_fixed_world` (`grid.rs:435`) → `parse_to_imported_vox`
(`grid.rs:502`) → `vox_import::parse_dot_vox_data` (`vox_import.rs:193`) →
`parse_dot_vox_data_tiled(data, 1)` (`vox_import.rs:207`).

The `tiles = 1` call means: **no CPU-side replication is done**. The `.vox` file's
scene-graph is walked (`compose_to_sparse_world`), giving a `ChunkBuckets` whose
`size_in_chunks` matches the `.vox` file's composed AABB, ceiling-rounded to
chunk boundaries. For Oasis_Hard_Cover.vox that is `93×34×84` chunks
= 1488×544×1344 voxels (per module doc `vox_import.rs:9`).

`parse_dot_vox_data_tiled` returns an `ImportedVox { world: ConstructedWorld,
palette }`. The `world.size_in_chunks` is the **model's natural bounds** in chunks.

### 3. Install — `install_imported_vox`

`install_imported_vox` (`grid.rs:521`) inserts:

- `WorldData` with `size_in_chunks = WORLD_SIZE_IN_CHUNKS` (= `(256,32,256)`)
  and `bounding_box.max = WORLD_SIZE_IN_VOXELS - 1` (= `(4095,511,4095)`)
  (`grid.rs:602-618`).
- `ModelData` with `size_in_chunks = model_size_in_chunks` (= `(93,34,84)` for
  Oasis) (`grid.rs:586-592`).

The world is always the fixed 4096×512×4096 container. The model is NOT padded or
altered; it keeps its natural dimensions.

### 4. GPU producer — `prepare_construction` dispatch loop

`prepare_construction` (`render/construction/mod.rs:2870` et seq.) runs once per
app lifecycle once `ModelData` is present. It iterates over
`WORLD_SIZE_IN_SEGMENTS` (= `(16,2,16)`) segments (`mod.rs:2941-2943`). Each
segment dispatches `generator_model.wgsl` with:

- `size_in_voxels = WORLD_SIZE_IN_VOXELS` (= `[4096,512,4096]`) (`mod.rs:2899-2903`)
- `model_size_in_chunks = model_data.size_in_chunks` (Oasis: `[93,34,84]`) (`mod.rs:2959`)
- `group_offset_in_chunks` = current segment's chunk offset in the world (`mod.rs:2944-2948`)
- `group_size_in_chunks_x/y = segment_chunks = WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 = 16`

### 5. Wrap / modulo logic — `generator_model.wgsl`

`generator_model.wgsl:68-70` (Bevy WGSL) — the critical wrap path:

```wgsl
let msc = params.model_size_in_chunks;
let model_extent_v = msc * 16u;            // model extent in voxels
let vpim = voxel_pos % model_extent_v;     // WRAP: position within model
```

This is a **faithful port** of C# `generatorModel.fx:20`:
```hlsl
uint3 voxelPosInModel = voxelPos % (int3(modelSizeInChunksX, modelSizeInChunksY, modelSizeInChunksZ) * 16);
```

Both perform `voxel_pos mod model_size_in_voxels`. The out-of-bounds guard at
`generator_model.wgsl:64` (`if any(voxel_pos >= params.size_in_voxels) { return 0 }`)
clips to `size_in_voxels = WORLD_SIZE_IN_VOXELS = [4096,512,4096]`.

### 6. Tile-count math

With Oasis at `[93,34,84]` chunks = `[1488,544,1344]` voxels in a `[4096,512,4096]` world:

- X tiles: `4096 / 1488 ≈ 2.75` (2 full + 0.75 partial)
- Z tiles: `4096 / 1344 ≈ 3.05` (3 full + 0.05 partial)

The user observes "2.5 modulo-wrapped instances" — this aligns with the X-axis
being the narrower-tile count (≈2.75) viewed from an angle where roughly 2.5
repetitions are visible.

### 7. C# reference path (mirror of the above)

C# flow: `WorldHandler.Initialize()` → `LoadModelScene("Content\\oasis.cvox")` →
`ModelData.Load(fileName)` → `worldData.GenerateWorld(worldGenerator)` →
`WorldData.GenerateWorld` loops `sizeInWorldGenSegments` segments (`WorldData.cs:136`) →
each calls `worldGenerator.CopyToChunkData(segmentPosInChunks, ..., actualSizeInVoxels, ...)` →
`WorldGeneratorModel.CopyToChunkData` (`WorldGeneratorModel.cs:32`) dispatches
`generatorModel.fx` with `sizeInVoxelsX/Y/Z = actualSizeInVoxels` and
`modelSizeInChunksX/Y/Z = modelData.sizeInChunks`.

**The wrap logic is identical in both codebases.** The modulo operand on the
left-hand side (`voxelPos`) and the denominator (`modelSizeInChunks * 16`) come
from the same sources:
- World size: both use `(16,2,16) × 4 × 64 = 4096×512×4096` (C# `WorldHandler.cs:18-19,31`
  vs Bevy `lib.rs:235,241`).
- Model size: both use the natural bounds of the loaded model file.

**However**: C# loads `oasis.cvox` (pre-saved NAADF binary format), while Bevy
loads `oasis_hard_cover.vox` (raw MagicaVoxel format). The `.cvox` format may
store a PADDED model size (e.g. rounded to the nearest 16-chunk segment boundary).
C# `ModelData.sizeInChunks` at `ModelData.cs:40` uses `ceil(modelSize / 16)`
without segment padding. But if the stored `.cvox` model was originally built from
a differently-sized source (or the `.cvox` export padded to a multiple of 64
chunks = 4 tiles into a 4096-voxel world), the tile count WOULD differ.

**The "4 EXACTLY" claim in C# is NOT explained by a different world-size constant
— the world sizes are identical in both codebases.** The divergence is likely in
the MODEL dimensions stored in `oasis.cvox` vs `oasis_hard_cover.vox`, or the C#
user was using a non-default world size (the GUI slider in `UiHeaderBar.cs:127`
lets users change `worldSizeToUseInWorldGenSegments` from `(16,2,16)` to any
value between 1 and 32).

---

## Scene-size / world-extent / wrap-modulo constants found

| # | file:line | symbol / context | value | reads/consumers | clearly in wrap path? |
|---|---|---|---|---|---|
| 1 | `crates/bevy_naadf/src/lib.rs:235` | `WORLD_SIZE_IN_SEGMENTS` | `UVec3(16, 2, 16)` | `lib.rs:1055`, `render/construction/mod.rs:2941-2943`, `voxel/grid.rs:543-545`, `render/construction/mod.rs:4314-4316` | **yes** — drives segment loop that populates the world; determines how much world the modulo wraps into |
| 2 | `crates/bevy_naadf/src/lib.rs:241` | `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS` | `4` | `lib.rs:1055`, `render/construction/mod.rs:2909` | **yes** — segment size × this = 16 chunks/segment; determines `segment_chunks` fed to `generator_model` |
| 3 | `crates/bevy_naadf/src/lib.rs:251` | `WORLD_SIZE_IN_CHUNKS` | `UVec3(256, 32, 256)` | `voxel/grid.rs:170-175,192-198,253,261,277-279,308,322-324,540-542`, `render/construction/mod.rs:2992-3007,4303-4305`, `e2e/gates.rs:34-38`, `render/prepare.rs:455` | **yes** — determines world AABB uploaded to shader as `size_in_chunks`; directly bounds how many model tiles are placed |
| 4 | `crates/bevy_naadf/src/lib.rs:254` | `WORLD_SIZE_IN_VOXELS` | `UVec3(4096, 512, 4096)` | `voxel/grid.rs:173-175,196-198,264,322-324,552-554,610-612`, `render/construction/mod.rs:2899-2903,4308-4310`, `render/prepare.rs:455`, `voxel/grid.rs:551-555` | **yes** — passed as `size_in_voxels` to `generator_model.wgsl`; the out-of-bounds guard at `generator_model.wgsl:64` clips the world at this extent, so it is the numerator of the `N_tiles = world_voxels / model_voxels` ratio |
| 5 | `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl:68-70` | `vpim = voxel_pos % model_extent_v` (where `model_extent_v = msc * 16u`) | runtime (= `model_size_in_chunks × 16`) | The wrapping itself is in this shader; consumed by `get_voxel_data_in_model` at `wgsl:62` | **yes** — THIS IS THE WRAP MODULO. The denominator `model_extent_v` is the model's natural voxel size, not a hardcoded constant |
| 6 | `crates/bevy_naadf/src/voxel/grid.rs:62` | `GRID_SIZE_IN_CHUNKS` / `DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS` | `[4, 2, 4]` | `grid.rs:67`, `e2e/gates.rs:34-38`, `e2e/small_edit_visual.rs:293-299` | **no** — this is the Phase-A test-scene footprint embedded INSIDE the fixed world; not involved in the `.vox` Oasis wrap path at all |
| 7 | `crates/bevy_naadf/src/voxel/vox_import.rs:87` | `MAX_CHUNKS_PER_AXIS` | `1024` | `vox_import.rs:246` (pre-flight validation) | **no** — a safety ceiling on the parsed model size, not a scene-extent constant |

---

## Divergence verdict

The world-size constants in the Bevy port form a **single canonical chain** with exactly four names (`WORLD_SIZE_IN_SEGMENTS`, `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS`, `WORLD_SIZE_IN_CHUNKS`, `WORLD_SIZE_IN_VOXELS`) that all derive from the same two roots, with a compile-time test (`lib.rs:1054`) enforcing their consistency — these match C#'s `WorldHandler.cs:18-19` defaults identically. **No divergent constants found; the "2.5 vs 4" tile-count difference is NOT caused by world-size constants.**

The root cause of the discrepancy lies elsewhere: the **model dimensions** (`model_size_in_chunks`) embedded in `oasis.cvox` (C#'s pre-saved binary) differ from those parsed from `oasis_hard_cover.vox` (the raw MagicaVoxel source), OR the C# user was observing a non-default world size set via the GUI slider (`UiHeaderBar.cs:127`). This is the architect's investigation surface.

---

## Borderline calls

**Item 5 (wrap modulo at `generator_model.wgsl:70`)**: The denominator `model_extent_v = msc * 16u` is dynamically computed from `params.model_size_in_chunks`, which ultimately comes from the loaded model file's parsed chunk count. This is not a "hardcoded constant" but it IS the exact value that controls tile count. It is borderline only in classification: it belongs in the wrap path, but any fix would be on the model-file side or on how `model_size_in_chunks` is set before being passed in, not in this shader line itself. The line faithfully mirrors C# `generatorModel.fx:20`.

**The "1024×128×1024" comment in `camera/mod.rs:28`**: the Bevy camera code states "In C# the default world is a fixed 1024×128×1024 voxels". This is factually wrong — C# `WorldHandler.cs:19,31` gives `4096×512×4096`. The `1024` is used ONLY as a normalization constant for the camera-pose scaling formula (the C# camera was tuned for a NOW-SUPERSEDED smaller world, and the Bevy code rescales it proportionally). This comment error does not affect runtime behavior but it could mislead future readers into thinking the world-size constants differ.

**C# `.cvox` model size**: C# loads `oasis.cvox` (a pre-baked binary, `WorldHandler.cs:34`) rather than `oasis_hard_cover.vox`. The `.cvox` format stores model data with `sizeInChunks = ceil(model_voxels / 16)` per-axis (no segment-alignment padding per `ModelData.cs:40`). However the `.cvox` file was originally produced from the Oasis source model, and if that source had different raw voxel dimensions (e.g. some multi-model composition that rounds to exactly 64 chunks = 1024 voxels = 4 tiles in a 4096 world), the tile count would be exactly 4. This requires reading the actual `oasis.cvox` binary to verify — not possible from source code alone. This is the highest-uncertainty item in the audit and the most likely root-cause path.

---

## Files surveyed

- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/01-context.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/naadf-bevy-port/01-context.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/lib.rs` (lines 220-268, 1045-1069)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/grid.rs` (lines 60-478, 479-648)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/vox_import.rs` (lines 1-280)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/assets/shaders/generator_model.wgsl` (full)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/construction/mod.rs` (lines 2870-3050)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/prepare.rs` (lines 320-660)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/camera/mod.rs` (full)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` (lines 120-160)
- `/mnt/archive4/DEV/NAADF/NAADF/World/WorldHandler.cs` (full)
- `/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs` (lines 1-170)
- `/mnt/archive4/DEV/NAADF/NAADF/World/Generator/WorldGeneratorModel.cs` (full)
- `/mnt/archive4/DEV/NAADF/NAADF/Content/shaders/world/generator/generatorModel.fx` (full)
- `/mnt/archive4/DEV/NAADF/NAADF/World/Render/WorldRender.cs` (lines 1-65)
- `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs` (lines 27-50, 381-440)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/e2e/vox_e2e.rs` (lines 655-680)
