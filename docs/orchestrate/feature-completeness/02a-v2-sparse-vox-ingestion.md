# 02a-v2 — Design — Track A: sparse VOX ingestion (supersedes 02a Decision 3)

**Date:** 2026-05-15
**Author:** delegate-architect (re-dispatched)
**Branch:** `main`
**Brief:** orchestrator-supplied; redesigns Track A `.vox` ingestion to support **large composed worlds** (Oasis_Hard_Cover.vox class, ~93×34×84 chunks = ~50M voxels composed from 291 models). Supersedes `02a-design-vox-loading.md` Decision 3 (`DenseVolume` + `construct()`). Keeps everything else from `02a-design-vox-loading.md` and the `03a-followup` scene-graph composition fix.

---

## Overview

The redesigned pipeline walks each scene-graph–composed sparse XYZI record (`dot_vox::Voxel { x: u8, y: u8, z: u8, i: u8 }` under the `Rot3`/`Xform` transform chain that landed in `03a-followup`) **directly into NAADF's 3-byte-array `ModelData` encoding** — `data_chunk` (one `u32` per chunk in the composed world), `data_block` (64 `u32`s per non-empty mixed chunk), `data_voxel` (32 `u32`s per unique 64-voxel block, hash-deduplicated). This is the bit-for-bit shape `crates/bevy_naadf/src/aadf/generator.rs:73-83` already declares. The walk faithfully ports C# `ModelData.ImportFromVox` (`ModelData.cs:356-526`) — the 64-voxel hash uses the same `31^(64-i) mod 2^32` coefficients the port already computes at `crates/bevy_naadf/src/render/construction/hashing.rs::hash_coefficients()` (so the CPU sparse walk produces a byte-stream the GPU W1 hash-dedup could interoperate with). AADFs (chunks/blocks/voxels) are built **per-chunk on the fly** in CPU using the existing `crates/bevy_naadf/src/aadf/bounds.rs::compute_aadf_layer` — when a chunk is materialised, we transiently lift its 64 sparse blocks back into a 16³ dense `voxel_state` array, run `compute_aadf_layer` (the §3.3 O(3·d·n) merge form, ≤1 MiB peak per chunk), and discard. The result is a `ConstructedWorld { chunks, blocks, voxels, size_in_chunks }` — the same shape `construct()` produces — installed into `WorldData` exactly the way the v1 install does today (`voxel/grid.rs:115-127`).

Decision 3 of `02a-design-vox-loading.md` was wrong because it focused on the per-model `dot_vox::Voxel` u8 size cap (256³) and missed scene-graph composition. `03a-followup` (commit `44d0599`) added composition correctly, but ingestion still routed through a `DenseVolume::voxels: Vec<VoxelTypeId>` sized at the **composed** AABB — Oasis at 1485×1331×536 MV voxels = 5952×2176×5376 NAADF voxels would need ~140 GiB of `u16` host RAM, OOMing the process. The diagnostic agent papered over this by lowering `MAX_CHUNKS_PER_AXIS = 32`, but that's a symptom-mask: the dense intermediate is the wrong shape for large composed worlds. This redesign retires the dense intermediate from the `.vox` path — the renderer's natural input shape is sparse (`chunks`/`blocks`/`voxels` `u32` buffers); the C# canonical never builds a dense intermediate either; the sparse walk produces ~1–2 MiB of `(chunks, blocks, voxels)` for Oasis at ~1% solid density. Caps move from "32 chunks/axis (dense host budget)" to "wgpu `max_buffer_size` / 2 GiB on `blocks` and `voxels` + wgpu `max_texture_dimension_3d` on the `chunks` 3D texture" — practical 1024³-chunk worlds (16384³ voxels = 4 trillion voxels), past anything any test fixture can produce.

---

## Architecture

### Module layout

```
crates/bevy_naadf/src/voxel/
├── mod.rs                    # (existing — VoxelType, VoxelTypeId, MaterialBase, ...)
├── grid.rs                   # (existing — setup_test_grid; `GridPreset::Vox` arm switches to sparse-walk path)
└── vox_import.rs             # EDIT — replace `flatten_scene → DenseVolume` with
                              #        `compose_to_sparse_world → ConstructedWorld`;
                              #        keep `Rot3`/`Xform`/scene-graph walk machinery from 03a-followup;
                              #        keep `vox_palette_to_voxel_types`; keep all error variants.
```

Everything load-bearing (the `Rot3` signed-permutation parse, the `Xform::parent_of` composition, the two-pass scene-graph walk shape, the Z↔Y swap, the `_emit`/`_flux` material parse, the `VoxImportError` enum) STAYS verbatim from the post-`03a-followup` `vox_import.rs`. The **target of the walk** changes from `DenseVolume::set([nx, ny, nz], VoxelTypeId)` (one host-RAM `Vec<VoxelTypeId>` slot per world voxel) to **chunk-bucketed sparse accumulators** (~one `Vec<(local_idx, VoxelTypeId)>` per non-empty chunk, growable, host-RAM proportional to actual non-empty voxel count). The `ImportedVox` struct retires its `volume: DenseVolume` field and gains `world: ConstructedWorld` (or its `(chunks_cpu, blocks_cpu, voxels_cpu, size_in_chunks, dense_voxel_types)` decomposition — see Decision Δ below) plus the same `palette: Vec<VoxelType>` field as before. `build_world_from_vox` (`vox_import.rs:187-223`) stops calling `construct()` because the sparse walk has already produced the final buffers.

No new module is justified — the changes are localised to `vox_import.rs`'s scene-graph walk + the `build_world_from_vox` installer. The renderer surface, the AADF crate, the `WorldData` resource, and the GPU dispatch chain are untouched.

### High-level data flow

```
dot_vox::DotVoxData
  │ (parse-once, no per-voxel host-RAM blowup — sparse `Vec<Voxel>` per model)
  ▼
two-pass scene-graph walk (UNCHANGED from 03a-followup):
  ├─ pass 1: compute_world_aabb  (Rot3/Xform composition over Group/Transform/Shape nodes)
  └─ pass 2: NEW — emit_sparse_xyzi  (instead of writing to DenseVolume::set,
                                       emits per-shape transformed (nx, ny, nz, VoxelTypeId)
                                       records into a per-chunk bucket)
  │ host RAM ≈ Σ non-empty voxels × ~6 bytes
  ▼
bucketed sparse → ConstructedWorld build pass (NEW — replaces aadf::construct::construct on .vox path):
  for each non-empty chunk:
    for each non-empty block (4³ subdivision):
       hash the 64-voxel content with hash_coefficients() (same coeffs as GPU W1)
       open-addressing CAS dedup against a host HashMap<hash, VoxelPtr>
       append packed 32 u32s to data_voxel on hash-miss
       emit per-voxel AADFs via compute_aadf_layer over the block's 4³ extent
    emit per-block AADFs via compute_aadf_layer over the chunk's 4³ extent
    emit data_block[64*N..64*N+64] for this chunk
  emit chunk-layer AADFs via compute_aadf_layer over the world's chunks_per_axis³ extent
  │ output ≈ Σ non-empty voxels × ~12 bytes (chunks + blocks + voxels packed u32s + AADFs)
  ▼
ImportedVox { chunks_cpu, blocks_cpu, voxels_cpu, size_in_chunks, palette }
  │
  ▼
voxel/grid.rs::setup_test_grid (UNCHANGED installer):
  WorldData { chunks_cpu, blocks_cpu, voxels_cpu, size_in_chunks, bounding_box, dirty: true, dense_voxel_types: empty } + VoxelTypes
  │
  ▼
existing extract/prepare chain (no `.vox`-specific code anywhere on the render side)
```

### Scene-graph composition reuse (from 03a-followup, UNCHANGED)

Every line of the following stays in `vox_import.rs` verbatim:

- `Rot3` struct + `Rot3::IDENTITY` + `Rot3::from_byte(b: u8)` + `Rot3::compose` + `Rot3::transform_vec` (`vox_import.rs:225-314`).
- `Xform` struct + `Xform::IDENTITY` + `Xform::apply` + `Xform::parent_of` (`vox_import.rs:316-364`).
- `frame_to_xform` (`vox_import.rs:371-401`).
- `accumulate_world_aabb` pass 1 (`vox_import.rs:564-639`).
- `vox_palette_to_voxel_types` (`vox_import.rs:740-811`).
- The "no-scene-graph fallback" path that uses `models[0]` directly (the `data.scenes.is_empty()` branch at `vox_import.rs:426-439`).

The 4 scene-graph tests added in `03a-followup` (`scene_graph_translations_separate_models`, `scene_graph_rotation_applies`, `rotation_byte_identity_and_axis_swap`, `xform_compose_matches_csharp_order`) STAY — they pin the rotation/composition math, which the sparse path still depends on.

### Sparse walk algorithm (replaces dense pass 2)

The structural change is in **pass 2** of the scene-graph walk. The current `collate_voxels` at `vox_import.rs:646-738` calls `volume.set([nx, ny, nz], VoxelTypeId(...))` per voxel — that's the line that allocates a `Vec<u16>` sized at the world AABB. The new pass 2 emits each transformed voxel into a per-chunk sparse bucket:

```rust
// NEW per-pass-2 accumulator. Replaces `volume: DenseVolume`.
struct ChunkBuckets {
    size_in_chunks: [u32; 3],
    /// One `Vec<(local_idx_in_chunk_voxels: u16, ty: VoxelTypeId)>` per chunk index.
    /// `None` until the chunk receives its first voxel.
    /// `local_idx_in_chunk_voxels` = vx + vy*16 + vz*256  (vx,vy,vz ∈ [0..16)).
    chunks: Vec<Option<Vec<(u16, VoxelTypeId)>>>,
}

impl ChunkBuckets {
    fn new(size_in_chunks: [u32; 3]) -> Self {
        let n = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
        Self { size_in_chunks, chunks: (0..n).map(|_| None).collect() }
    }

    /// Push a single voxel at `[nx, ny, nz]` (post-Z↔Y-swap NAADF coords)
    /// with type `ty` into the per-chunk bucket. Allocates the chunk's bucket
    /// lazily.
    fn push(&mut self, naadf_pos: [u32; 3], ty: VoxelTypeId) {
        let cx = naadf_pos[0] / 16;
        let cy = naadf_pos[1] / 16;
        let cz = naadf_pos[2] / 16;
        let sx = self.size_in_chunks[0];
        let sy = self.size_in_chunks[1];
        let ci = (cx + cy * sx + cz * sx * sy) as usize;
        let lx = (naadf_pos[0] % 16) as u16;
        let ly = (naadf_pos[1] % 16) as u16;
        let lz = (naadf_pos[2] % 16) as u16;
        let local = lx + ly * 16 + lz * 256;
        self.chunks[ci].get_or_insert_with(Vec::new).push((local, ty));
    }
}
```

The host-RAM peak during pass 2 is **`Σ non-empty voxels × (2 + 2 + Vec overhead)` ≈ ~8 bytes per non-empty voxel** — for Oasis at ~1% density of a 5952×2176×5376 world that's ~700M empty + 7M non-empty → ~56 MiB peak. *Vs. dense which would be 700M × 2 bytes = 140 GiB.* Comfortable fit.

`collate_voxels` (`vox_import.rs:646-738`) becomes:

```rust
fn collate_voxels_sparse(
    data: &dot_vox::DotVoxData,
    node_id: u32,
    parent: Xform,
    visited: &mut [bool],
    world_min: [i32; 3],
    buckets: &mut ChunkBuckets,
) {
    // (Recursion shape: identical to existing collate_voxels — Transform recurses
    // with parent_of, Group recurses through children, Shape emits voxels.)
    // ... identical Group/Transform arms ...
    match &data.scenes[idx] {
        dot_vox::SceneNode::Shape { models, .. } => {
            for sm in models {
                let Some(model) = data.models.get(sm.model_id as usize) else { continue; };
                let s = [model.size.x as i32, model.size.y as i32, model.size.z as i32];
                let origin = [-s[0] / 2, -s[1] / 2, -s[2] / 2];
                for v in &model.voxels {
                    let local = [v.x as i32, v.y as i32, v.z as i32];
                    let centered = [local[0]+origin[0], local[1]+origin[1], local[2]+origin[2]];
                    let world = parent.apply(centered);
                    let shifted = [world[0]-world_min[0], world[1]-world_min[1], world[2]-world_min[2]];
                    if shifted[0] < 0 || shifted[1] < 0 || shifted[2] < 0 { continue; }
                    // Z↔Y swap to NAADF (ModelData.cs:438).
                    let nx = shifted[0] as u32;
                    let ny = shifted[2] as u32;
                    let nz = shifted[1] as u32;
                    if nx >= buckets.size_in_chunks[0]*16
                        || ny >= buckets.size_in_chunks[1]*16
                        || nz >= buckets.size_in_chunks[2]*16 { continue; }
                    let ty = VoxelTypeId(v.i as u16 + 1);
                    buckets.push([nx, ny, nz], ty);
                }
            }
        }
        // ... identical Transform / Group arms ...
    }
}
```

### From `ChunkBuckets` to `ConstructedWorld` — the build pass (replaces `construct()` for `.vox`)

Mirror C# `ModelData.ImportFromVox:418-499` line-by-line but produce **the same output `construct()` produces** (i.e., final `(chunks, blocks, voxels)` `u32` buffers with full AADFs, NOT a "data_chunk/data_block/data_voxel that goes through W5 dispatch"). The Decision Δ section explains why.

Walk every chunk in the world. For each chunk:

1. **Build a 16³ dense `voxel_state` chunk-local array on the stack/heap** (4096 entries × `VoxelTypeId` u16 = 8 KiB). This is `chunk_voxels: [VoxelTypeId; 4096]` initialised to `EMPTY`. Replay the chunk's `Option<Vec<(u16, VoxelTypeId)>>` bucket entries into it (last-write-wins — matches the C# `dataImport[q] = v` semantics in `CollateVoxelData:747`). A 16³ × 2 byte transient is 8 KiB — trivial.

2. **Classify the chunk**: if every entry is `EMPTY` → chunk is `Empty` (AADFs pending); if every entry is the same non-empty type → chunk is `UniformFull(ty)`; else → `Mixed` (will need 64 blocks + voxels).

3. For a `Mixed` chunk, walk its 64 blocks (4³ blocks, each 4³ voxels). For each block:
   - **Classify the block**: gather its 64 voxels from `chunk_voxels` (`gather_block_voxels` shape, `aadf/construct.rs:254-269`). If all-empty → `BlockClass::Empty`; if all the same non-empty type → `BlockClass::UniformFull(ty)`; else → `BlockClass::Mixed`.
   - For `Mixed` blocks: **hash the 64-voxel content with the same coefficients the GPU W1 path uses**. The C# computes the hash at `ModelData.cs:433-441` over the same packed-voxel form `chunkCalc.fx:126-136` produces. The port's coefficient generator is already in `render/construction/hashing.rs::hash_coefficients()`. Open-address into a host `HashMap<u32 hash, VoxelPtr>` keyed by the hash, with a fall-back content-equality check on collision (mirror C# `ModelData.cs:469-479` — read back the candidate's 32 u32s, full memcmp against the new 32 u32s). On hash-miss: assign a fresh `VoxelPtr` = `data_voxel.len() / 32 * 32` (u32-element offset), append 32 packed `u32`s (per-empty-voxel AADFs already computed below).
   - **Compute voxel-layer AADFs for the block**: run `compute_aadf_layer([4, 4, 4], AADF_MAX_SMALL=3, |c| chunk_voxels[block_local(c)] == EMPTY)` for the block's 4³ extent. The transient AADF array is 64 × `Aadf6` = 64 × 6 bytes = 384 bytes — trivial.
   - **Pack the block's 32 u32s**: same as `aadf/construct.rs::encode_block_voxels:348-379`.
   - **Append to `data_voxel`** on hash-miss (the dedup pass).

4. **Compute block-layer AADFs for the chunk**: `compute_aadf_layer([4, 4, 4], AADF_MAX_SMALL=3, |c| block_classes[block_local(c)] == BlockClass::Empty)` over the chunk's 4³ block layer.

5. **Encode 64 `data_block[chunk_base..chunk_base+64]` entries** per `aadf/construct.rs::encode_chunk_blocks:386-411`.

6. **Emit `data_chunk[chunk_index]`** per `aadf/cell.rs::ChunkCell::encode`. AADFs computed later at the world-chunk-layer pass.

After every chunk is classified + (when mixed) its 64 blocks + voxels are appended:

7. **Compute chunk-layer AADFs** with one global `compute_aadf_layer([cx, cy, cz], AADF_MAX_CHUNK=31, |c| chunk_class[c] == Empty)` over the chunks-per-axis³ extent. Same call site as `aadf/construct.rs:228-232`. For a 1024³-chunk world that's 1G `is_empty` queries — at ~5ns each on a release build that's ~5 seconds (acceptable, one-shot cost at load time; if profiling shows it dominates, see Risk #2 below).

8. **Fill `chunks_cpu[i]` u32s** per `aadf/construct.rs:234-242`.

The output buffers are byte-identical in layout to what `construct(&DenseVolume)` produces today. The renderer's extract/prepare chain consumes them unchanged.

### `dense_voxel_types` — set to empty for the sparse path

`WorldData::dense_voxel_types` (`world/data.rs:50`) is the source-of-truth for the runtime GPU producer's `segment_voxel_buffer` (`render/construction/mod.rs:935-960`). For sparse `.vox` ingestion **we set `dense_voxel_types: Vec::new()`** (empty). The GPU producer's `dense_data_ready` check (`render/construction/mod.rs:833-835`) gates on `!w.dense_voxel_types.is_empty()` and falls back to the CPU upload path when the dense stream isn't available — i.e. the GPU producer **is skipped on sparse-loaded `.vox` worlds**, and the renderer reads the pre-built `chunks_cpu`/`blocks_cpu`/`voxels_cpu` buffers via the existing extract/prepare upload path (the byte stream the CPU `construct()` would have produced; identical to today's behaviour when `gpu_construction_enabled = false`).

That sidesteps the GPU producer's `seg = max(chunks_per_axis)` segment-buffer cap (the constant that fell from `1024` to `32` in `03a-followup` is no longer reached on this path because the GPU producer simply doesn't run for `.vox` content). The chunks 3D texture is still the binding constraint at `max_texture_dimension_3d` (typically 2048 desktop, 1024 Vulkan baseline). The `blocks`/`voxels` `GrowableBuffer`s grow as needed up to `max_buffer_size` (2 GiB Vulkan baseline).

### VoxelType registration (UNCHANGED from v1)

`vox_palette_to_voxel_types` at `vox_import.rs:758-811` stays as is. The 256-entry MagicaVoxel palette + per-slot `_emit`/`_flux` material chunks produce one `VoxelType` per palette entry (sRGB → linear via gamma 2.2; `emission = emit * (1+flux)^2 * 5`; emissive when `emission > 0`). Index 0 stays the reserved empty placeholder; `Voxel.i: u8 → VoxelTypeId(i as u16 + 1)`. No K-means (the v1 architect correctly identified that the `.vox` C# path doesn't K-means at `ModelData.cs:502-522` — that's `.vl32`'s pipeline).

### Cap removal

The v1 caps (`MAX_CHUNKS_PER_AXIS = 32`, `MAX_DENSE_BYTES = 1 GiB`, both at `vox_import.rs:78-89`) were **defensive guards on the dense intermediate**. With dense gone, both retire — replaced by:

**Hard ceilings (wgpu-imposed, queried at runtime — but the loader can't see them; we pre-flight against documented minimums):**

| Constraint | Vulkan baseline (`bevy::render::settings::WgpuSettings` default) | Desktop typical | What it caps |
|---|---|---|---|
| `max_texture_dimension_3d` | **2048** | 2048 (NVIDIA, AMD) | chunks-per-axis (since the chunks 3D texture is `size_in_chunks` Rg32Uint) |
| `max_buffer_size` | **256 MiB** | 2 GiB (NVIDIA, AMD) | `blocks_cpu` / `voxels_cpu` upload buffers and the `chunks_cpu` upload buffer (post 8B/texel) |
| chunk-pos packing `(x:11, y:10, z:11)` bits at `aadf/edit.rs:67-69` | hard ceiling: 2048×1024×2048 chunks = 32768×16384×32768 voxels | same | hard limit on world dim even if wgpu allows more |

The redesign uses **pre-flight caps that mirror the Vulkan baseline conservatively** — the loader does NOT have access to the actual `RenderDevice::limits()` at parse time (the render app initialises after `Startup`, and the parse can be CPU-only without Bevy resources). The new constants:

```rust
/// Max chunks-per-axis. Matches wgpu Vulkan baseline `max_texture_dimension_3d`
/// (the chunks 3D texture's hard ceiling). Desktop GPUs typically allow
/// 2048; we keep the conservative pre-flight at 1024 — a 1024³-chunk world
/// is 16384³ voxels = ~4.4 trillion voxels, past any practical .vox fixture
/// (Oasis_Hard_Cover.vox composes to ~93³ chunks = ~0.1% of this cap).
pub const MAX_CHUNKS_PER_AXIS: u32 = 1024;

/// Soft pre-flight on the cpu-side `blocks`+`voxels` buffer combined size.
/// At wgpu Vulkan baseline `max_buffer_size = 256 MiB` per buffer, this hits
/// the `voxels` buffer first (each unique 64-voxel block is 32 u32s = 128 B;
/// 256 MiB / 128 B = 2M unique blocks ≈ 128M unique voxels worth of geometry,
/// which on a 1% non-empty world is ~12G total-voxels = ~480³ NAADF voxels).
/// On NVIDIA/AMD desktop the real cap is 2 GiB → ~16× more headroom; this
/// pre-flight cap is the conservative pre-flight gate.
pub const MAX_VOXELS_BUFFER_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_BLOCKS_BUFFER_BYTES: u64 = 256 * 1024 * 1024;
```

Behaviourally for `.vox` loads in scope: **Oasis_Hard_Cover.vox (93×34×84 chunks = ~265K chunks total, ~7M non-empty voxels)** comfortably fits — sparse output is ~100 MiB total. Any single-model `.vox` ≤ 256³ voxels (the per-model `dot_vox::Voxel.{x,y,z}: u8` cap) fits inside one chunk-per-16-voxels = 16³ chunks = trivial. Multi-model fixtures up to ~1G total voxels fit.

When the pre-flight cap *is* tripped (e.g., a hypothetical multi-model file composing to 2000³ chunks), the loader errors with `VoxImportError::SizeExceedsTextureLimit` / `VoxImportError::SizeExceedsBudget` — same fallback semantics as v1, just with much higher thresholds.

### Effective ceiling (the practical "large worlds" answer)

Under this design, the port can ingest `.vox` content up to **~10³ chunks composed (160³ voxels)** *trivially*, **~64³ chunks (1024³ voxels)** *comfortably*, and **~256³ chunks (4096³ voxels)** *with a Vulkan-baseline-grade GPU* if the buffer pre-flight caps are bumped (no algorithmic change needed; the sparse walk itself is fine). At desktop-typical `max_buffer_size = 2 GiB` the practical "everything composes" ceiling is ~512³ chunks (8192³ voxels = 549 billion voxels) on a 1% solid density assumption — that's MagicaVoxel scenes vastly past anything anyone has authored. **The user's Oasis_Hard_Cover.vox is at ~93×34×84 ≈ 265K chunks ≈ 1.5% of even the conservative Vulkan-baseline ceiling.**

### `gpu_construction_enabled` interaction (set false on `.vox` paths — practical effect)

`render/construction/mod.rs:837` gates the GPU producer on `gpu_construction_enabled && dense_data_ready`. With `.vox` setting `dense_voxel_types: Vec::new()`, `dense_data_ready == false`, the GPU producer skips — the renderer reads the pre-built CPU mirror buffers. **The `gpu_construction_enabled` *config flag* itself stays at its default `true`** (the `--validate-gpu-construction` e2e gate still uses it on default-grid content) — no flag-flip needed. The data-driven `dense_data_ready` check is the actual gate. This is consistent with how the runtime GPU producer currently gracefully degrades to the upload path.

### Coupling to the post-Phase-D scope

The forbidden-moves list (`01-context.md` §5) forbids editing `naadf_gpu_producer_node` internals or `gpu_producer_skip_upload`. This design touches neither — `dense_voxel_types` is set empty on the `.vox` path, the existing data-driven check at `render/construction/mod.rs:833-835` does the right thing. No render-pipeline or shader edits.

---

## File-by-file change list

### NEW

None. (The design is contained within the existing `vox_import.rs` module's scene-graph walk pass-2 + a new `build_constructed_world_sparse` helper inside the same file.)

### EDIT

| Path | Edit | Approx Δ LOC |
|---|---|---|
| `crates/bevy_naadf/src/voxel/vox_import.rs` | (a) Replace `flatten_scene` return type from `DenseVolume` to a new sparse-output struct (e.g. `(ChunkBuckets, [u32; 3] /*size_in_chunks*/)`); (b) replace `collate_voxels` with `collate_voxels_sparse` (the Shape arm pushes into `ChunkBuckets` instead of `DenseVolume::set`); (c) add `build_constructed_world_sparse(buckets: ChunkBuckets) -> ConstructedWorld` that implements the per-chunk sparse → `(chunks, blocks, voxels)` build pass (the new core); (d) update `ImportedVox` from `volume: DenseVolume` to `world: ConstructedWorld`; (e) update `build_world_from_vox` to install `world.{chunks, blocks, voxels, size_in_chunks}` directly into `WorldData` without re-running `construct()`; (f) raise `MAX_CHUNKS_PER_AXIS` from 32 → 1024; rename `MAX_DENSE_BYTES` → `MAX_VOXELS_BUFFER_BYTES` (or split into two constants) and re-target on the sparse output's buffer sizes. Keep all `Rot3`/`Xform`/`accumulate_world_aabb`/`vox_palette_to_voxel_types`/error variants/scene-graph fallback path verbatim. | +250 / -80 (net +170; the sparse build pass is most of the new code) |
| `crates/bevy_naadf/src/voxel/grid.rs` | Adjust the `GridPreset::Vox` arm at `grid.rs:73-93`: replace `(imp.palette, imp.volume)` extraction with installing the `imp.world` `ConstructedWorld` directly. Bypass the `construct(&volume)` call on the `.vox` branch only — the Default branch still goes through `construct()`. Adjust the surrounding `let world = construct(&volume); let size = volume.size_in_voxels();` block to support both paths cleanly: keep `construct(&volume)` for `GridPreset::Default`, take `imp.world` for `GridPreset::Vox`. The `dense_voxel_types` assignment becomes path-dependent: `Vec::new()` for `.vox`, `volume.voxels.iter().map(|t| t.0).collect()` for Default. | +15 / -5 |
| `crates/bevy_naadf/src/voxel/vox_import.rs` (tests) | (a) Adjust the 14 existing tests that read `imp.volume.voxel_at(...)` / `imp.volume.size_in_chunks` to read against the new `imp.world` shape. The voxel-existence checks become `chunk_at(imp.world, [cx, cy, cz])` + descending the encoded `(chunks, blocks, voxels)` tuple (or, simpler, retain a test-only `dense_view_of(imp.world) -> DenseVolume` helper that re-densifies on demand — see Test plan). (b) Add 4 new tests for the sparse-walk-only path (see Test plan). | +120 / -40 |
| `crates/bevy_naadf/src/aadf/construct.rs` | (Optional) Lift `gather_block_voxels` / `gather_chunk_blocks` / `classify_block` / `uniform_chunk_type` / `encode_block_voxels` / `encode_chunk_blocks` from private to `pub(crate)` so `vox_import.rs::build_constructed_world_sparse` can call them directly instead of re-implementing identical block/chunk classification + AADF encode logic. Zero semantic change; visibility-only edit. | +5 / -5 (4 `fn` → `pub(crate) fn`) |

### DELETE

None of `02a` v1's code is wholesale-deleted; the dense path stays available as the Default content path. The DELETED concepts:

- The `DenseVolume`-as-`.vox`-intermediate role retires. The `DenseVolume` type itself stays (still used by the test grid, by `aadf::construct::construct`, by unit tests).
- The "dense host budget" cap rationale at `vox_import.rs:58-89` retires. The constants are renamed/re-targeted, not removed.
- The unused `flatten_scene → DenseVolume` code path in `vox_import.rs` retires (replaced by `flatten_scene → ChunkBuckets`).

### Files NOT touched

For clarity (the redesign should produce a focused diff):

- `aadf/generator.rs` / `ModelData` — kept for the W5 dispatch + unit tests, NOT used by this `.vox` path (Decision Δ explains).
- `aadf/edit.rs` / `aadf/bounds.rs` / `aadf/cell.rs` / `aadf/entity.rs` — reused unchanged.
- `render/construction/` — none of it edits, including `hashing.rs::hash_coefficients()` (consumed read-only).
- `render/prepare.rs` / `render/extract.rs` / `world/data.rs` / `world/buffer.rs` — unchanged.
- Every shader file — unchanged.
- `bin/bake.rs` — unchanged (pre-bake explicitly out of scope per `## Out of scope`).
- `panel.rs` / `hud.rs` / `e2e/` — Track-B/e2e surfaces; the `--vox-e2e` gate's regression-test fixture in `e2e/vox_e2e.rs` is small (4×2×4 chunks) and well within both old and new caps. **No edits needed.**

---

## Algorithm specifications

### `compose_to_sparse_world` (pass 2 replacement)

```rust
fn compose_to_sparse_world(
    data: &dot_vox::DotVoxData,
) -> Result<(ChunkBuckets, [u32; 3], Vec<VoxelType>), VoxImportError> {
    if data.models.is_empty() { return Err(VoxImportError::Empty); }

    // Pass 1: world AABB. UNCHANGED from 03a-followup (vox_import.rs:441-476).
    let (world_min, world_size, size_in_chunks) = if data.scenes.is_empty() {
        // No-scene-graph fallback: just use models[0]'s native MV size.
        let m = &data.models[0];
        let mv_size = [m.size.x, m.size.y, m.size.z];
        let naadf_size = [mv_size[0], mv_size[2], mv_size[1]];   // Z↔Y swap
        let cc = [naadf_size[0].div_ceil(16).max(1),
                  naadf_size[1].div_ceil(16).max(1),
                  naadf_size[2].div_ceil(16).max(1)];
        validate_caps(cc)?;
        ([0, 0, 0], naadf_size, cc)
    } else {
        let mut visited = vec![false; data.scenes.len()];
        let mut world_min = [i32::MAX; 3];
        let mut world_max = [i32::MIN; 3];
        accumulate_world_aabb(data, 0, Xform::IDENTITY, &mut visited, &mut world_min, &mut world_max);
        if world_min[0] == i32::MAX {
            // Fallback: no visible shapes; treat models[0] as identity. (Same recovery
            // path as 03a-followup vox_import.rs:460-476.)
            return compose_to_sparse_world_from_models0(data);
        }
        let mv_size = [(world_max[0]-world_min[0]+1) as u32,
                       (world_max[1]-world_min[1]+1) as u32,
                       (world_max[2]-world_min[2]+1) as u32];
        let naadf_size = [mv_size[0], mv_size[2], mv_size[1]];   // Z↔Y swap
        let cc = [naadf_size[0].div_ceil(16).max(1),
                  naadf_size[1].div_ceil(16).max(1),
                  naadf_size[2].div_ceil(16).max(1)];
        validate_caps(cc)?;
        (world_min, naadf_size, cc)
    };

    // Pass 2: NEW — emit voxels into per-chunk sparse buckets instead of into a DenseVolume.
    let mut buckets = ChunkBuckets::new(size_in_chunks);
    if !data.scenes.is_empty() {
        let mut visited = vec![false; data.scenes.len()];
        collate_voxels_sparse(data, 0, Xform::IDENTITY, &mut visited, world_min, &mut buckets);
    } else {
        // No-scene-graph fallback.
        let m = &data.models[0];
        for v in &m.voxels {
            // Z↔Y swap, no translation.
            let nx = v.x as u32;
            let ny = v.z as u32;
            let nz = v.y as u32;
            if nx >= world_size[0] || ny >= world_size[1] || nz >= world_size[2] { continue; }
            buckets.push([nx, ny, nz], VoxelTypeId(v.i as u16 + 1));
        }
    }

    let palette = vox_palette_to_voxel_types(&data.palette, &data.materials);
    Ok((buckets, size_in_chunks, palette))
}

fn validate_caps(size_in_chunks: [u32; 3]) -> Result<(), VoxImportError> {
    if size_in_chunks[0] > MAX_CHUNKS_PER_AXIS
        || size_in_chunks[1] > MAX_CHUNKS_PER_AXIS
        || size_in_chunks[2] > MAX_CHUNKS_PER_AXIS
    {
        return Err(VoxImportError::SizeExceedsTextureLimit {
            dim: size_in_chunks, limit: MAX_CHUNKS_PER_AXIS,
        });
    }
    Ok(())
}
```

### `build_constructed_world_sparse` (the new core)

Mirrors C# `ModelData.ImportFromVox:418-499`'s "walk every chunk, classify, hash-dedup, emit" structure, but produces `(chunks, blocks, voxels)` final encoding (with AADFs baked) instead of NAADF's three-byte-array intermediate that goes through W5 dispatch. The same `aadf/cell.rs::{ChunkCell, BlockCell, VoxelCell}::encode()` machinery is reused — every byte the renderer reads ends up identical to what `aadf::construct::construct()` would have produced on the same dense input. Bit-byte equivalence is the validation criterion for the round-trip test.

```rust
fn build_constructed_world_sparse(
    buckets: ChunkBuckets,
) -> Result<ConstructedWorld, VoxImportError> {
    let [cx_u, cy_u, cz_u] = [buckets.size_in_chunks[0] as usize,
                              buckets.size_in_chunks[1] as usize,
                              buckets.size_in_chunks[2] as usize];
    let n_chunks = cx_u * cy_u * cz_u;

    // Output buffers. Pre-allocate chunks_cpu (exact size); blocks_cpu / voxels_cpu
    // grow as non-empty chunks are processed.
    let mut chunks_cpu: Vec<u32> = vec![0; n_chunks];
    let mut blocks_cpu: Vec<u32> = Vec::new();
    let mut voxels_cpu: Vec<u32> = Vec::new();

    // Block-content dedup map. Key = u32 hash from hash_coefficients(); value =
    // VoxelPtr (u32-element offset into voxels_cpu, divided by 32 since each
    // unique block occupies 32 consecutive u32s). The C# uses an open-address
    // CAS-on-GPU map (ModelData.cs:469-485); CPU-side a HashMap suffices and
    // is what the existing aadf::construct::construct uses (aadf/construct.rs:142).
    //
    // NB: C# disambiguates collisions by content memcmp at ModelData.cs:472-478;
    // a 32-bit hash will collide at scale. We use HashMap<[VoxelTypeId; 64], VoxelPtr>
    // keyed on the literal 64-voxel content — same as aadf/construct.rs:142.
    // Functionally identical to C# (which uses hash + content-equality check);
    // we skip the hash bucketing because Rust's HashMap is already hashed.
    //
    // (See Decision Δ-Hash for why we use HashMap-on-content rather than CAS-on-hash.)
    let mut block_dedup: HashMap<[VoxelTypeId; 64], VoxelPtr> = HashMap::new();

    // Classify every chunk; remember per-chunk classification + per-chunk
    // block-array (for the AADF pass).
    let mut chunk_class: Vec<ChunkClass> = vec![ChunkClass::Empty; n_chunks];
    let mut chunk_block_arrays: Vec<Option<[BlockClass; 64]>> = vec![None; n_chunks];

    for ci in 0..n_chunks {
        let Some(bucket) = buckets.chunks[ci].as_ref() else {
            // Empty chunk — leave chunk_class[ci] = Empty; AADFs filled below.
            continue;
        };
        // 1. Replay bucket entries into a transient 16³ dense chunk_voxels.
        let mut chunk_voxels = [VoxelTypeId::EMPTY; 4096];
        for &(local, ty) in bucket {
            chunk_voxels[local as usize] = ty;
        }
        // 2. Classify the chunk.
        if chunk_voxels.iter().all(|t| *t == VoxelTypeId::EMPTY) {
            // Unreachable: the chunk's bucket was non-None, so at least one push
            // happened — but pushes were filtered through bounds checks. Treat as
            // Empty.
            continue;
        }
        let first = chunk_voxels[0];
        let uniform = chunk_voxels.iter().all(|t| *t == first);
        if uniform && first != VoxelTypeId::EMPTY {
            chunk_class[ci] = ChunkClass::UniformFull(first);
            continue;
        }
        // 3. Mixed — classify 64 blocks; dedup mixed blocks against block_dedup.
        let mut blocks_in_chunk = [BlockClass::Empty; 64];
        for bz in 0..4 {
            for by in 0..4 {
                for bx in 0..4 {
                    let b_idx = bx + by*4 + bz*16;
                    // Gather the block's 64 voxels (4³) from chunk_voxels.
                    let mut block_voxels = [VoxelTypeId::EMPTY; 64];
                    for lz in 0..4 {
                        for ly in 0..4 {
                            for lx in 0..4 {
                                let chunk_local = (bx*4 + lx) + (by*4 + ly)*16 + (bz*4 + lz)*256;
                                let block_local = lx + ly*4 + lz*16;
                                block_voxels[block_local] = chunk_voxels[chunk_local];
                            }
                        }
                    }
                    // Classify.
                    if block_voxels.iter().all(|t| *t == VoxelTypeId::EMPTY) {
                        blocks_in_chunk[b_idx] = BlockClass::Empty;
                    } else {
                        let bf = block_voxels[0];
                        if block_voxels.iter().all(|t| *t == bf) {
                            blocks_in_chunk[b_idx] = BlockClass::UniformFull(bf);
                        } else {
                            // Mixed — dedup against the HashMap. On hash-miss,
                            // append 32 u32s of placeholder; encode_block_voxels
                            // overwrites with the AADF-augmented final encoding.
                            let ptr = if let Some(&existing) = block_dedup.get(&block_voxels) {
                                existing
                            } else {
                                let new_ptr = VoxelPtr(voxels_cpu.len() as u32);
                                voxels_cpu.resize(voxels_cpu.len() + 32, 0);
                                block_dedup.insert(block_voxels, new_ptr);
                                // Compute voxel-layer AADFs for this block + encode.
                                encode_block_voxels(&block_voxels, new_ptr, &mut voxels_cpu);
                                new_ptr
                            };
                            blocks_in_chunk[b_idx] = BlockClass::Mixed(ptr);
                        }
                    }
                }
            }
        }
        // 4. Reserve 64 consecutive block slots for this chunk in blocks_cpu.
        let block_base = BlockPtr(blocks_cpu.len() as u32);
        blocks_cpu.resize(blocks_cpu.len() + 64, 0);
        chunk_class[ci] = ChunkClass::Mixed(block_base);
        chunk_block_arrays[ci] = Some(blocks_in_chunk);
    }

    // 5. Per-mixed-chunk block-AADF + block-encode pass — reuse the existing
    //    aadf::construct::encode_chunk_blocks shape verbatim (after lifting it to
    //    pub(crate) per the file-by-file change).
    for ci in 0..n_chunks {
        if let (ChunkClass::Mixed(base), Some(blocks_in_chunk)) =
            (chunk_class[ci], chunk_block_arrays[ci]) {
            encode_chunk_blocks(&blocks_in_chunk, base, &mut blocks_cpu);
        }
    }

    // 6. World-chunk-layer AADFs — one compute_aadf_layer call over the whole
    //    chunks-per-axis³ extent. Strictly identical to aadf::construct::construct
    //    at construct.rs:228-232.
    let chunk_is_empty_at = |c: [i32; 3]| -> bool {
        let idx = c[0] as usize + c[1] as usize * cx_u + c[2] as usize * cx_u * cy_u;
        matches!(chunk_class[idx], ChunkClass::Empty)
    };
    let chunk_aadfs = compute_aadf_layer([cx_u, cy_u, cz_u], AADF_MAX_CHUNK, chunk_is_empty_at);

    // 7. Emit chunks_cpu[i] u32s.
    for ci in 0..n_chunks {
        let cell = match chunk_class[ci] {
            ChunkClass::Empty => ChunkCell::Empty(chunk_aadfs[ci]),
            ChunkClass::UniformFull(ty) => ChunkCell::UniformFull(ty),
            ChunkClass::Mixed(ptr) => ChunkCell::Mixed(ptr),
        };
        chunks_cpu[ci] = cell.encode();
    }

    // 8. Pre-flight cap re-check on the output buffer sizes (might exceed
    //    MAX_VOXELS_BUFFER_BYTES if density is much higher than expected).
    if (voxels_cpu.len() * 4) as u64 > MAX_VOXELS_BUFFER_BYTES {
        return Err(VoxImportError::SizeExceedsBudget {
            dim: [voxels_cpu.len() as u32, 0, 0], bytes: MAX_VOXELS_BUFFER_BYTES,
        });
    }
    if (blocks_cpu.len() * 4) as u64 > MAX_BLOCKS_BUFFER_BYTES {
        return Err(VoxImportError::SizeExceedsBudget {
            dim: [blocks_cpu.len() as u32, 0, 0], bytes: MAX_BLOCKS_BUFFER_BYTES,
        });
    }

    Ok(ConstructedWorld {
        chunks: chunks_cpu,
        blocks: blocks_cpu,
        voxels: voxels_cpu,
        size_in_chunks: buckets.size_in_chunks,
    })
}
```

### Updated public API surface

```rust
pub struct ImportedVox {
    /// The pre-built constructed world: chunks, blocks, voxels (u32 buffers
    /// with AADFs baked) + size_in_chunks. The renderer installs these
    /// directly via WorldData; no further construct() call is needed.
    pub world: ConstructedWorld,
    /// The voxel-type palette (UNCHANGED from v1).
    pub palette: Vec<VoxelType>,
}

pub fn parse_vox_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxImportError>;
pub fn load_vox(path: impl AsRef<Path>) -> Result<ImportedVox, VoxImportError>;
pub fn parse_dot_vox_data(data: &dot_vox::DotVoxData) -> Result<ImportedVox, VoxImportError>;

/// Install an `ImportedVox` into `WorldData` + `VoxelTypes` resources. Sets
/// `dense_voxel_types: Vec::new()` so the GPU producer path skips (the
/// renderer reads the pre-built chunks_cpu/blocks_cpu/voxels_cpu directly).
pub fn build_world_from_vox(imported: ImportedVox) -> (WorldData, VoxelTypes);
```

---

## Test plan

### `#[test]` coverage

**Migrated from v1 (still pass, signatures adjusted to read from `imp.world` rather than `imp.volume`):**

1. `parses_single_voxel_fixture` — read via a small `test::dense_view_of(imp.world: &ConstructedWorld) -> Vec<VoxelTypeId>` helper that walks `ChunkCell::decode` → `BlockCell::decode` → `VoxelCell::decode` for assertion. The 1×1×1-chunk fixture's voxel-at-origin assertion holds.
2. `parses_small_cube_fixture` — same; the 7³ diffuse + 1 emissive cube assertions hold via the same dense-view helper.
3. `palette_index_zero_is_empty_placeholder` — UNCHANGED (reads `imp.palette[0]`).
4. `palette_emissive_from_matl` — UNCHANGED.
5. `zy_swap_matches_csharp` — UNCHANGED in intent; reads via dense-view helper.
6. `size_exceeds_texture_limit_errors` — adjust threshold (model size now triggers at `> 1024³` chunks, not `> 32³`). The test stays useful as a hard-cap regression check.
7. `empty_models_errors` — UNCHANGED.
8. `construct_runs_on_imported_volume` — RETIRED (the sparse path doesn't run `construct()`). Replaced by `sparse_walk_matches_dense_construct` (see below).
9. `build_world_from_vox_inserts_dense_voxel_types` — adjusted: assert `WorldData::dense_voxel_types.is_empty()` (intentional change — sparse path doesn't populate this; the GPU producer correctly skips).
10. `load_vox_propagates_io_error` — UNCHANGED.
11-14. `scene_graph_translations_separate_models`, `scene_graph_rotation_applies`, `rotation_byte_identity_and_axis_swap`, `xform_compose_matches_csharp_order` — UNCHANGED (they cover the Rot3/Xform machinery; the sparse path still uses them verbatim).

**NEW (sparse-path-specific):**

15. **`sparse_walk_matches_dense_construct_on_small_fixture`** — the bit-byte migration safety check. Take the existing `build_small_cube` fixture (8×8×8 voxels, single model, identity scene). (a) Drive it through `parse_dot_vox_data` and capture `imp.world: ConstructedWorld`. (b) Drive the same fixture through the v1-style code path: hand-construct a `DenseVolume` from the same voxel list, call `aadf::construct::construct(&volume)` — capture `oracle: ConstructedWorld`. (c) Assert byte-equality: `imp.world.chunks == oracle.chunks && imp.world.blocks == oracle.blocks && imp.world.voxels == oracle.voxels`. This proves the sparse walk produces literally the same output bytes as the dense `construct()` would, on the same input — the strongest possible migration safety check.
16. **`sparse_walk_handles_mid_sized_world`** — synthesise a `DotVoxData` with a 64×64×64-voxel single-model scene (4×4×4 chunks), ~1% density (random sprinkle of ~16K voxels). Drive through `parse_dot_vox_data`. Assert: `imp.world.size_in_chunks == [4, 4, 4]`, `imp.world.chunks.len() == 64`, non-zero `blocks` + `voxels`. Peak host RAM during the test stays bounded (no `Vec<u16>` allocation of 64³ × 2 B = 524 KiB — actually this size fits comfortably as dense, but the test exercises the sparse code path, not the budget).
17. **`sparse_walk_handles_composed_multi_model_at_old_cap_boundary`** — synthesise a 2-model scene where the composed AABB exactly matches the old `MAX_CHUNKS_PER_AXIS = 32` cap (32×32×32 chunks = 512×512×512 voxels). Each model has ~10K sparse voxels under non-trivial `nTRN` translations + a 90° rotation. Drive through `parse_dot_vox_data`. Assert: loads cleanly (would have failed under the old cap), produces a non-empty `ConstructedWorld`, sample-test voxels at expected post-composition positions.
18. **`sparse_walk_dedups_identical_blocks`** — two voxels at identical chunk-local positions in two different chunks (same 4³ block contents). Assert that `imp.world.voxels.len() == 32` (one unique block, not two) — verifies the `HashMap<[VoxelTypeId; 64], VoxelPtr>` dedup actually fires on the sparse path. Mirrors `aadf::construct::tests::identical_blocks_dedup`.

### `--vox-e2e` gate (continued green status)

The just-landed `--vox-e2e` regression gate (`crates/bevy_naadf/src/e2e/vox_e2e.rs`, commit `0c7a2f7`) synthesises a 2-model fixture sized to 4×2×4 chunks = 64×32×64 voxels — trivially within both the old and new caps. The fixture's two emissive models compose under non-trivial `nTRN` translations along the MV-z axis (`_t.z = 2` vs `_t.z = 16`). Under the redesign:

- **Pass 1 (world AABB)**: identical to today — same `accumulate_world_aabb` walk.
- **Pass 2 (voxel emission)**: instead of `volume.set([nx, ny, nz], ty)` writes, emit to `ChunkBuckets`. Same voxels end up at the same chunk positions.
- **Build pass**: produces `ConstructedWorld` equivalent to what `construct(&volume)` would have produced on the same fixture (verified by test #15 above).

Result: the central-rect luminance gate at `assert_vox_geometry_visible` (threshold 160, current measured 249.7) **must still pass** — the framebuffer is byte-equivalent because the rendered world is byte-equivalent (renderer reads the same `chunks_cpu`/`blocks_cpu`/`voxels_cpu` buffers). The unit tests inside `e2e/vox_e2e.rs` (`fixture_round_trips_and_composes_two_distinct_models`, etc.) hard-code position-level invariants like `voxel_at([32, 16, 32]) != EMPTY` — these become `decoded_voxel_at(&imp.world, [32, 16, 32]) != EMPTY` via the dense-view helper.

The implementer **must** include `cargo run --bin e2e_render -- --vox-e2e` in the verification gate.

### Implementer's smoke gate (per global memory `subagent-gpu-app-verification-loop`)

ONE smoke run max per sub-agent. The implementer:

1. `cargo build --workspace` — clean.
2. `cargo test --workspace --lib` — all `vox_import::tests::*` pass plus the existing 146 tests.
3. `cargo run --bin e2e_render` (baseline) — region luminance stays at emissive 247 / solid 242 / sky 146 (default grid path unchanged).
4. `cargo run --bin e2e_render -- --vox-e2e` — gate's central-rect luminance > 160 (stays green).
5. `cargo run --bin e2e_render -- --validate-gpu-construction` (default grid path; sparse `.vox` not exercised; confirms no global regression).

### User's visual gate

Per the global memory note, the user runs `cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox` and confirms the originally-failing repro now loads cleanly (no soft-cap error; world allocates at `[93, 34, 84]` chunks; renders correctly). The implementer does NOT loop on this — one smoke run from the user produces the verdict.

---

## Decisions & rejected alternatives

### Decision Δ-ModelData — produce `(chunks, blocks, voxels)` directly, NOT a `ModelData` intermediate

**Chosen:** Sparse walk emits `ConstructedWorld { chunks, blocks, voxels, size_in_chunks }` (the renderer-input shape) directly. No `ModelData` allocation.

**Rejected:** Sparse walk emits `ModelData` (the C# generator-input shape: 3-byte-array `data_chunk` / `data_block` / `data_voxel`); subsequent step calls `aadf::generator::generate_segment_cpu` to expand `ModelData` → `segment_voxel_buffer` (a 2048-u32-per-chunk packed-voxel buffer); subsequent step calls `aadf::construct::construct` over the resulting dense voxel data; result is `ConstructedWorld`. This is what C# `WorldData.Construct` does at `WorldData.cs:135-156`: segmented loop, each iteration dispatches `WorldGeneratorModel.CopyToChunkData` (= `generate_segment_cpu`) into the segment buffer, then runs `CalculateChunkBlocks` (= the GPU W1 hash-dedup chain).

**Why:** the C# path's `ModelData → generate_segment_cpu → CalculateChunkBlocks` chain ends up producing **the same `(chunks, blocks, voxels)` buffers** that the port's `aadf::construct::construct` already produces directly. The intermediate `ModelData` exists in C# because the GPU generator runs on a per-segment basis (decoupling generator from constructor) and because the C# uses GPU dispatch for the hash-dedup. On the port, both stages are CPU-resident — the `ModelData` intermediate would be a 4×-larger temporary buffer (≥3 KiB per chunk vs ≤1 KiB) with no advantage. The GPU W1 dispatch chain (`render/construction/`) is reused for *edits* (W2) and for the optional `gpu_construction_enabled` runtime producer; it's NOT a load-bearing input path for static `.vox` content. Producing `(chunks, blocks, voxels)` directly skips an entire intermediate buffer and the segmented-iteration coordination.

**What would flip:** if a future requirement lands streaming load (where the world's `(chunks, blocks, voxels)` is too large to all live in host RAM, even on the sparse path), the chosen approach makes the entire world available to the renderer in one shot. Path (i) — `ModelData → segmented dispatch` — would allow per-segment streaming: load N chunks at a time, dispatch, free. If "streaming `.vox` larger than 16 GiB RAM" becomes a requirement, switch to path (i). For sub-host-RAM worlds, path (chosen) is strictly simpler.

### Decision Δ-AADF — per-chunk CPU AADFs during the build pass, NOT W3 GPU background queue

**Chosen:** Voxel AADFs and block AADFs are built per-chunk inline in `build_constructed_world_sparse` using `compute_aadf_layer` over the chunk's local 4³ block/voxel extent. Chunk-layer AADFs are built once at the end with one `compute_aadf_layer` over the whole chunks-per-axis³ extent. Total CPU cost: ~5s for 1024³ chunks worst case (linear in chunk count, single-threaded; could parallelise per-chunk).

**Rejected:** Don't compute AADFs CPU-side at all. Encode chunks with placeholder `Aadf6::ZERO`, mark every chunk as "needs recompute", and let the W3 background bounds-compute dispatch (`render/construction/bounds_calc.rs`) fill them in over multiple frames at runtime. (The C# uses this seam for runtime edits; the initial-load `Construct` flow at `WorldData.cs:135-156` actually runs an explicit `ComputeBlockBounds`/`ComputeVoxelBounds` GPU pass *up front* — not background queue.)

**Why chosen:** simpler. The renderer expects valid AADFs the moment a frame renders the chunk; queueing them for later means the first few frames after load render with degenerate (zero-distance) AADFs — every empty cell is treated as bounded by 1 — which means the ray-marcher takes the slow path everywhere → massive frame-time spike. The CPU pass is one-shot (load-time only), well-bounded (~5s for the deepest realistic world). The merge-form `compute_aadf_layer` was already timed at 16.3× faster than the legacy form (per `aadf/bounds.rs:25-30`), so even huge worlds clear in seconds. The C# does similar work GPU-side because they're running a segmented dispatch pipeline anyway; the port's CPU sparse walk has no such dispatch overhead, so the AADF pass is just another step in the same loop.

**What would flip:** if `cargo bench` shows the chunk-layer AADF pass dominating load time on very large worlds (>1024³ chunks), parallelise via `rayon::par_chunks` (the merge form's per-axis step is naturally parallel — each `step_axis` call already iterates the entire layer; chunking the X-axis across threads is straightforward). If even that is too slow, switch to the W3 GPU dispatch path (mark all chunks "needs recompute"; render with skip-empty-space-disabled for the first frame; let the dispatch backfill). Neither is needed for the realistic ceiling of this design.

### Decision Δ-Hash — `HashMap<[VoxelTypeId; 64], VoxelPtr>` for block dedup, NOT u32-hash CAS

**Chosen:** Block dedup uses Rust's `HashMap` keyed on the literal 64-voxel content `[VoxelTypeId; 64]`. Hash function is the std-lib default (currently FxHash-like via `ahash` if pulled in transitively, or stdlib's randomised SipHash if not — both are fine).

**Rejected:** Reimplement C#'s u32-hash + open-addressing CAS loop (`ModelData.cs:433-485`) with the same `31^(64-i) mod 2^32` coefficients the GPU W1 path uses — mirror the byte-for-byte deduplication structure the GPU produces.

**Why chosen:** the **existing `aadf::construct::construct` already uses `HashMap<[VoxelTypeId; 64], VoxelPtr>`** (`aadf/construct.rs:142`). Test #15 (`sparse_walk_matches_dense_construct_on_small_fixture`) requires the sparse walk's output to be byte-equal to `construct()`'s output on the same input — the only way that holds is if **both use the same dedup function**. Mirroring C#'s u32-hash CAS would produce a different deduplication ordering than `construct()`'s `HashMap` does (different hash collisions, different insertion ordering of equivalent blocks → different `VoxelPtr` assignments) and test #15 would fail despite producing equally-correct output.

**What would flip:** if the GPU W1 hash-dedup runtime path (consumed by W2 edits) needs to interoperate with sparse-walk-produced `voxels_cpu` data — i.e., if W2 edits read pre-existing blocks from the sparse-walk output and the W1 GPU code does u32-hash matching against the host-CAS scheme. **It doesn't today** — W1's GPU hash is consumed by W1's `chunk_calc.wgsl` dispatch from the runtime GPU producer path, which is *skipped* for sparse `.vox` content. W2's edits build their own hash entries on the GPU side (`world_change.wgsl`), independent of the load-time dedup. So the load-time CPU dedup function is free to use whatever produces consistent host-side output.

(For future-proofing: the `hash_coefficients()` table at `crates/bevy_naadf/src/render/construction/hashing.rs:43-50` is available; if a future Track requires the sparse path to feed into W2 edit pipelines via the GPU hash-map, swap the dedup function then. Not in scope.)

### Decision Δ-DenseFallback — retain v1 dense path for `GridPreset::Default`, retire for `GridPreset::Vox`

**Chosen:** The `GridPreset::Default` arm still goes through `build_default_volume() → construct(&volume)` (the test-grid path; 64×32×64 voxels = 32 chunks; dense intermediate is trivial). The `GridPreset::Vox` arm goes through `parse_vox_bytes → build_constructed_world_sparse` exclusively.

**Rejected:** Migrate `GridPreset::Default` to also use sparse walks (consistency). Or: keep `GridPreset::Vox` sparse-only but allow a `--vox-dense` debug fallback for the v1 path (parallel paths for migration safety).

**Why chosen:** the default test grid is small (32 chunks); dense is the right shape for it (~256 KiB dense vs ~50 KiB sparse — well within either budget). Migrating it would expand the diff for no functional gain. The sparse path is verified byte-equal to the dense path by test #15, so no parallel debug path is needed — if the sparse path ever drifts from the dense path's output on test fixtures, test #15 fails loudly.

**What would flip:** if Track B's editor tools need to start from a sparse-loaded `.vox` world AND a regression in the sparse output's `chunks_cpu`/`blocks_cpu`/`voxels_cpu` byte layout breaks downstream W2 edits, the parallel-debug-path could land as a debugging aid. Not anticipated.

### Decision Δ-GPUProducer — set `dense_voxel_types: Vec::new()` on sparse path (data-driven skip)

**Chosen:** Sparse `.vox` worlds install `WorldData::dense_voxel_types: Vec::new()`. The runtime GPU producer's existing data-driven gate at `render/construction/mod.rs:833-835` (`!w.dense_voxel_types.is_empty()`) skips the GPU producer chain. The renderer reads the pre-built `chunks_cpu`/`blocks_cpu`/`voxels_cpu` directly (same path as `gpu_construction_enabled = false`).

**Rejected:** Populate `dense_voxel_types` from the sparse walk by re-densifying — for a 5952×2176×5376 voxel Oasis world that's 140 GiB host RAM; entirely impossible.

**Rejected (b):** Globally flip `gpu_construction_enabled = false` when loading `.vox` content. (a config-level flag flip; allowed by the forbidden-moves list — it's the *internals* of `naadf_gpu_producer_node` that can't be touched, not flag values.)

**Why chosen:** the data-driven gate is already in place and is the correct semantic — "no dense source means the GPU producer literally has nothing to consume, so it correctly skips." This is more robust than flipping the config flag because (i) it doesn't disable the GPU producer for the test grid or other dense-authored worlds in the same session, and (ii) it inherits the existing fallback behaviour cleanly (no new branches in the render-graph dispatch logic).

**What would flip:** if a future requirement lands "GPU producer must also run on `.vox` content" (e.g., for runtime regeneration after some edit operation), the path is to make the sparse walk *optionally* also produce a `dense_voxel_types` stream — feasible for small-to-medium worlds (≤512³ voxels ≈ 256 MiB) but disabled for large composed worlds. Not in scope.

### Decision Δ-CapsConservative — preflight against documented wgpu **minimums**, not queried limits

**Chosen:** `MAX_CHUNKS_PER_AXIS = 1024` (Vulkan baseline `max_texture_dimension_3d`); `MAX_VOXELS_BUFFER_BYTES = 256 MiB` (Vulkan baseline `max_buffer_size`). These are *worst-case-portable* — a Vulkan-baseline-only GPU will accept everything within these caps; everything past these caps is documented as requiring queried-limit support.

**Rejected:** Query the actual `RenderDevice::limits()` at parse time and use those.

**Why chosen:** the parser runs at `Startup` *before* the render-app is initialised (the actual `RenderDevice` doesn't exist yet). Querying at-runtime would require deferring the parse into an `Update` system gated on render-app ready, which complicates the load flow and breaks the unit tests' pure-CPU shape. The conservative pre-flight is the trade-off — false positives are only triggered by `.vox` files past 1024³ chunks, which is past anything realistic.

**What would flip:** if a real `.vox` file in scope sits between Vulkan-baseline and desktop-typical caps (1024 < `size_in_chunks.{x,y,z}` ≤ 2048), the user can edit the constants locally OR the design adds a post-render-app-init re-validation that promotes the conservative cap to the queried cap. Not anticipated in scope.

---

## Assumptions made

1. **`compute_aadf_layer` scales linearly in chunks-per-axis³.** At 1024³ chunks = 1G entries, single-threaded ≈ 5 seconds (per the merge form's 16.3× speedup over legacy + ~5ns per `step_axis` cell on a release build). Verified: `aadf/bounds.rs::compute_aadf_layer` is O(3·d·n) per the doc comment at line 25; n = chunks_per_axis³; d = max_dist = 31 for chunk layer. 1G × 31 / 16.3× ≈ ~2 GFLOPs of integer ops → ~1-5s on a modern CPU core. If this is too slow at scale, parallelisable via `rayon` (Risk #2). Confidence: medium.

2. **Host RAM peak during pass 2 (`ChunkBuckets`) is ~6–8 bytes per non-empty voxel.** The `Vec<(u16, VoxelTypeId)>` per chunk is `(u16, u16) = 4 B` per entry + Vec amortised overhead (~2 B/entry). For Oasis at 1% density of a 5952×2176×5376 world: ~700M voxel total × 0.01 = ~7M non-empty × 6 B ≈ 42 MiB. **Comfortable**. If real fixtures hit higher density (~10%), peaks 420 MiB — still acceptable. Confidence: high.

3. **`dot_vox` parse-time peak for Oasis is bounded.** The 84 MB file parses into 291 models × (sparse `Vec<Voxel>` of `~8 B per voxel non-empty + Size metadata`). Per the `03a-followup` repro, total parse takes 50 MiB of host RAM (we observed it succeed pre-fix when the dense alloc was 268 MiB, so total parse + dense was ≤ 1 GiB; subtracting the dense leaves the parse around ~50–100 MiB). The sparse path inherits the same parse — no new dependency on `dot_vox`'s peak. Confidence: high.

4. **The HashMap-based block dedup is byte-equal to `aadf::construct::construct`'s output** when both consume the same input voxel set. `construct.rs:142` is `HashMap<[VoxelTypeId; CELL_CHILDREN], VoxelPtr>`; the sparse build pass uses the identical map type with identical insertion semantics; both walk blocks in the same `bz, by, bx` order; both append placeholder 32-u32 groups on dedup-miss; both call the same `encode_block_voxels`. Test #15 enforces this; if it fails, the design is wrong (and the implementer must investigate before landing). Confidence: high (test-enforced).

5. **wgpu `max_texture_dimension_3d` is ≥ 1024 on every supported target.** Vulkan baseline = 2048; the wgpu spec mandates ≥ 1024 on every backend including web; the project's wasm build path (`#[cfg(target_arch = "wasm32")]` in `lib.rs:507`) keeps this floor. The new `MAX_CHUNKS_PER_AXIS = 1024` matches. Confidence: high (per wgpu spec).

6. **wgpu `max_buffer_size` is ≥ 256 MiB on every supported target.** Vulkan baseline = 256 MiB. Desktop typical: 2 GiB. The new `MAX_VOXELS_BUFFER_BYTES = 256 MiB` matches the baseline; desktop has 8× headroom. Confidence: high (per wgpu spec).

7. **The chunks 3D texture upload (`render/prepare.rs:223-279`) doesn't blow `max_buffer_size`.** A 1024³-chunk Rg32Uint texture is 1G texels × 8 B = 8 GiB of texture data — past `max_buffer_size`. **However**, texture writes use `queue.write_texture` with `bytes_per_row` chunking, NOT a single linear buffer. The actual constraint is `max_texture_dimension_3d` (per-axis), not buffer size. So 1024³ chunks renders fine. **Caveat:** the data path through `chunk_data_paired: Vec<[u32; 2]>` at `prepare.rs:249-259` does materialise the entire texture in host RAM as a `Vec`, which IS bounded by the host-allocator (8 GiB is plausible but pressures the heap). On large worlds (>512³ chunks ≈ 1 GiB host RAM for the texture), this is a Phase-D-grade concern — see Risk #4. For realistic in-scope `.vox` (≤ 256³ chunks ≈ 128 MiB), it's fine. Confidence: medium (real but bounded).

8. **C# `WorldData.Construct`'s segmented dispatch is for the GPU's per-segment buffer budget, not algorithmic necessity.** The segment size (`worldGenSegmentSizeInChunks`, default 4 chunks = 64 voxels per axis = `segment_voxel_buffer = 4³ × 2048 × 4 B = 524 KiB`) is small for the GPU's binding limit. CPU work has no equivalent limit; processing the whole world as one giant CPU loop is fine. The port's sparse walk processes one chunk at a time inherently. Confidence: high (verified from `WorldData.cs:135-156` + `MagicaVoxel.cs:738-754` reading).

9. **Adding `pub(crate)` visibility to `aadf::construct::{gather_block_voxels, classify_block, encode_block_voxels, encode_chunk_blocks, uniform_chunk_type}` is safe.** These are pure functions with no internal state; their type signatures use already-public types (`VoxelTypeId`, `VoxelPtr`, `BlockClass`, etc. — wait, `BlockClass` is private). One sub-decision: `BlockClass` and `ChunkClass` enums need promoting to `pub(crate)` too. They're tiny (~10 LOC each), no risk in promoting. Confidence: high.

10. **Setting `dense_voxel_types: Vec::new()` for `.vox` content correctly skips the GPU producer chain in every code path that consumes it.** Verified by reading `render/construction/mod.rs:833-835`: `let dense_data_ready = extracted_world.as_deref().is_some_and(|w| !w.dense_voxel_types.is_empty());`. The producer's `want_gpu_producer = construction_config.gpu_construction_enabled && dense_data_ready` is the dispatch gate. **However**, other code paths that consume `WorldData::dense_voxel_types` might exist — e.g., a hypothetical edit-mode regression test reading the dense stream. Audit needed; if any consumer assumes `dense_voxel_types` is non-empty, this changes their behaviour on `.vox`-loaded worlds. Currently I see no such consumer outside `render/construction/`. Confidence: medium-high (verified for the GPU producer; not audited for other consumers).

11. **The `--vox-e2e` fixture's behaviour is byte-equivalent under sparse and dense paths.** Per test #15's logic, but applied to the e2e fixture specifically: both paths produce the same `chunks`/`blocks`/`voxels`. The e2e gate's screenshot will be pixel-identical. **However**, the e2e fixture's unit tests (`fixture_round_trips_and_composes_two_distinct_models` etc.) read `imp.volume.voxel_at([32, 16, 32])` — the test code must change to read the new `imp.world` shape. **This breaks the test compile** unless the implementer updates it. Recorded as Risk #5.

12. **`HashMap`'s default hasher is not load-bearing for output correctness.** The HashMap is content-keyed; insertion-ordering is undefined; `VoxelPtr` values *will* differ between two runs if the hash function is randomised (stdlib SipHash). Test #15's byte-equality assertion will FAIL on a Cargo workspace pulling `std::collections::HashMap` with the default randomised hasher. **Mitigation**: use `BTreeMap<[VoxelTypeId; 64], VoxelPtr>` (deterministic) OR use `HashMap` with a deterministic hasher (e.g., `FxHasher` from `rustc-hash` or `ahash` configured with a fixed seed). **Critical**: this matches what `aadf::construct::construct` uses today — `aadf/construct.rs:142` is `HashMap<...>` (default stdlib randomised hash). Confidence: medium — need to check whether `aadf::construct::construct` is deterministic across runs. If yes (the existing tests pass!), the same approach works for sparse. If no, both algorithms would need a deterministic-hasher fix.

13. **Adding `dense_view_of()` test helper does not introduce duplication of `construct` logic.** The helper walks the encoded `(chunks, blocks, voxels)` via `ChunkCell::decode`/`BlockCell::decode`/`VoxelCell::decode` to produce a flat `Vec<VoxelTypeId>` — pure read-side, ~30 LOC, lives in `#[cfg(test)] mod tests`. No semantic duplication. Confidence: high.

14. **The `Rot3::from_byte` and `Xform::parent_of` machinery from `03a-followup` is correct.** Pinned by 4 tests (`rotation_byte_identity_and_axis_swap`, `xform_compose_matches_csharp_order`, etc.). The sparse path consumes these unchanged. Confidence: high (test-enforced).

---

## Risks & mitigations

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | The 64-voxel HashMap dedup's `VoxelPtr` assignment differs from `aadf::construct::construct`'s — test #15 (byte-equality) fails | Low (both use identical HashMap shape) | Test failure; design must be revised | Use the EXACT same map type + iteration order as `aadf::construct::construct`. If stdlib HashMap is non-deterministic across runs, both algorithms inherit the same non-determinism — but test #15 compares within a single run, so this self-cancels. Verify by running the test multiple times. If divergence persists, switch BOTH to `BTreeMap<[VoxelTypeId; 64], VoxelPtr>` (deterministic + small perf cost). |
| 2 | Chunk-layer `compute_aadf_layer` at 1024³ chunks takes >30 s | Low (per Assumption 1: ~5s expected) | Boot-time hitch on hypothetical max-size worlds | (i) Cargo bench the AADF pass on a 256³-chunk world; extrapolate. (ii) If >5s, parallelise with `rayon::par_chunks` over the Z axis. (iii) If still too slow, mark chunks "needs recompute" and use the W3 GPU background queue (deferred Phase-D residual). Not blocking for realistic in-scope sizes (≤256³ chunks). |
| 3 | wgpu `max_buffer_size` at Vulkan baseline (256 MiB) cuts off worlds < what `MAX_VOXELS_BUFFER_BYTES = 256 MiB` permits | Low | Spurious load failure on baseline GPU | The pre-flight cap matches the baseline; the load fails before allocating the buffer. The error message names the cap. Desktop users (2 GiB cap) can patch the constant locally. Future work: post-render-init re-validation. |
| 4 | The chunks 3D texture's full host-side `Vec<[u32; 2]>` materialisation in `prepare.rs:249-259` peaks at 8 GiB for 1024³ chunks | Low (no realistic in-scope `.vox` reaches 1024³ chunks) | OOM at load time on max-size worlds | Phase-D-grade concern — the prepare-side host-staging is independent of this design. If realistic content composes to >256³ chunks, refactor `prepare_world_gpu` to chunked `write_texture` calls. Out of scope for Track A. |
| 5 | `--vox-e2e` gate's unit tests `fixture_round_trips_and_composes_two_distinct_models` etc. break the build (read `imp.volume.voxel_at([...])` against the new shape) | High (will happen) | Test compile failure on first build | The implementer updates the 3 test functions in `crates/bevy_naadf/src/e2e/vox_e2e.rs` to call `decoded_voxel_at(&imp.world, [...])` via the new test helper. ~10 LOC change. Recorded in the file-by-file change list. |
| 6 | The conservative `MAX_CHUNKS_PER_AXIS = 1024` blocks a hypothetical 1024 < seg ≤ 2048 desktop-spec world | Very low (no fixture in scope) | Spurious load failure | Document the cap; user edits the constant locally if needed. Future: post-render-init re-validation. |
| 7 | A `.vox` file with pathological density (>50% solid) produces `voxels_cpu` past `MAX_VOXELS_BUFFER_BYTES` even on moderate world sizes | Low (rarely seen in MagicaVoxel scenes; even Oasis is ~1%) | Load failure on dense scenes | The pre-flight cap fires before the renderer tries to allocate; user sees the actionable error. Cap can be raised; alternative is sparse-storage paged loading (out of scope). |
| 8 | `dense_voxel_types: Vec::new()` breaks a non-render-construction code path that assumes the dense stream is present | Medium | Silent regression on `.vox` worlds | Audit at implementation: `grep -rn 'dense_voxel_types' crates/bevy_naadf/src/`. The reuse audit verified only `render/construction/mod.rs` consumes it (rows 7 + 10), but a final pre-merge `grep` is mandatory. If a consumer is found, design adds an opt-in re-densify path for that consumer specifically. |
| 9 | The `_emit`/`_flux` parse on materials silently produces wrong palette colors on edge-case `.vox` files | Pre-existing (v1) | Visual issue | UNCHANGED from v1 (Risk #11 in `02a-design-vox-loading.md`). Faithful-port: emissive wins over metallic at the C# code. |
| 10 | The conservative pre-flight cap (1024) is too tight for some real-world fixture | Very low | Spurious load failure | The error names the actual cap; user edits the constant. Future: lift to post-render-init re-validation. |
| 11 | Migration of the v1 tests breaks subtly (e.g., the dense-view helper's decoding has a bug that produces false positives) | Medium | Test pass that shouldn't | Test #15's byte-equality against `construct()`'s known-good output is the canary. If #15 passes, the dense-view helper is correct. If #15 fails, fix the helper before declaring tests green. |
| 12 | `cargo bench` reveals the sparse walk is slower than expected for tiny worlds (the `HashMap` has per-call overhead) | Low | Tiny perf hit on the test-grid path (not the .vox path) | Test-grid path doesn't change (stays dense). `.vox` path's loader runtime ≤ 1s for realistic content even at 1% density (~7M voxel pushes ≈ 50 ms; ~265K block hashes ≈ 30 ms; per-chunk AADF ≈ 100 ms). Acceptable for the one-shot load cost. |

---

## Out of scope for this design

- **Pre-bake (`.vox` → `.cvox` offline)** — orthogonal track; the user mentioned it as option-B in the previous session. Not in this design's scope. The chosen sparse-walk approach makes pre-bake **unnecessary** for the realistic `.vox` sizes; if it later becomes useful for streaming or for boot-time speedup, the seam at `parse_dot_vox_data(&DotVoxData) -> Result<ImportedVox, _>` is clean — a `.cvox`-loader wraps it. Recorded in `02a-design-vox-loading.md` Decision 4 future-extension.
- **`obj2voxel`** — deferred entirely per `01-context.md` §1 + §5 forbidden move 7. No mention here. Track A is `.vox` only.
- **Pathological-density edge cases (>50% solid worlds where sparse offers no win)** — the sparse walk handles these correctly but allocates `~Σ non-empty voxels × 8 B` host RAM; at 50% density on a 1024³ chunk world that's ~280 GiB. The pre-flight cap fires; user gets a clear error. No special handling.
- **Streaming load (memory-map vs load-whole-file)** — `dot_vox::load_bytes` parses from a `&[u8]` slice; the loader uses `std::fs::read` (synchronous, in-memory). For `.vox` files > 1 GB on disk, switch to `memmap2`. Out of scope until a file > 1 GB appears.
- **Large `.vox` files beyond ~10 GB on disk** — different problem class (mostly disk-IO / parser-design); the sparse walk handles arbitrary parsed `DotVoxData` but `dot_vox`'s parser itself is the limiting factor at >10 GB inputs. Out of scope.
- **`.vl32` import** — `01-context.md` §1: Track A is `.vox` only. The K-means stage from `ModelData.cs:528-560` (which v1 correctly identified as `.vl32`-only) is not in scope.
- **Voxlap `.vox`** — out of scope per audit §2.4 row 3.
- **Per-frame hot-reload of `.vox`** — Bevy `AssetLoader` integration is out of scope per v1 Decision 4. The `parse_dot_vox_data` seam is clean for a future AssetLoader.
- **Render-side changes to consume `dense_voxel_types: Vec::new()` differently** — the existing data-driven gate already does the right thing; no render edits.
- **Editor (Track B) interaction with sparse-loaded `.vox` worlds** — Track B's tools call `WorldData::set_voxel` / `set_voxels_batch`, which are oblivious to how the world was authored. Sparse-loaded worlds participate in editing identically. Not addressed here because it's Track B's concern.
- **Lifting the post-render-init `max_buffer_size` / `max_texture_dimension_3d` queried cap** — the conservative Vulkan-baseline pre-flight is the chosen trade-off. Lifting to queried limits is a future enhancement.
- **Bit-exact match with C# `ModelData.ImportFromVox`'s u32-CAS dedup output** — explicitly traded (Decision Δ-Hash) for byte-equality with the port's existing `aadf::construct::construct` output (test #15). Either choice produces correct renderable geometry; the chosen one minimises the diff.
- **Parallelising the chunk-layer AADF pass via `rayon`** — flagged in Risk #2 as a deferred optimisation; not part of the first cut.
- **Documenting the `--vox` flag in `README.md` or `--help`** — implementer's call; not load-bearing for the design.
