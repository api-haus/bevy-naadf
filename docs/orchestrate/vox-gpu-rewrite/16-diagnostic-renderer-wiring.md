# vox-gpu-rewrite — renderer/wiring diagnostic (2026-05-18)

## TL;DR

**The bug is NOT in the renderer's bind-group/wiring path.** It is in the
**ModelData encoder**: `build_constructed_world_sparse` produces a
`ConstructedWorld` whose `voxels[]` buffer encodes empty voxels with
12-bit AADF bits in the low bits of the half-word. The W5 GPU producer
chain (`generator_model.wgsl::get_voxel_data_in_model`) reads those
half-words assuming the **C# `ImportFromVox` encoding convention** — in
which empty voxels in mixed blocks are LITERALLY ZERO. When the
generator hits an AADF-bearing empty voxel half-word like `0x0886`, it
masks to 15 bits (`0x886`), then the caller (`fillChunkDataWithModelData`
at `generator_model.wgsl:148-151`) applies
`voxel1 |= voxel1 > 0 ? (1 << 15) : 0` — falsely marking the empty
voxel as FULL with type bits `0x886` (= 2182, out-of-palette).

The renderer descends into this voxel, reads the full flag, sets bit 30,
the hit test fires (`cur_node & 0x40000000`), and `hit_type = cur_node &
0x7FFF = 0x886 = 2182`. The renderer indexes `voxel_types[2182]` —
out-of-bounds on the 257-entry palette — and decodes a zero material.
Result: a "thousands-typed" voxel that renders black.

The Stage 9 production-scale readback is byte-correct end-to-end
**from the producer's perspective**: the GPU writes EXACTLY what the
CPU oracle (`generate_segment_cpu`) writes, both of which are wrong
because they're both faithful ports of `generatorModel.fx`'s bit-for-bit
behaviour AGAINST the wrong input encoding.

The legacy CPU path renders correctly because it bypasses the generator
round-trip: it uploads `voxels_cpu` directly into the renderer's
`voxels` buffer. The renderer's `cur_node >> 15` check correctly treats
bit-15-clear half-words as empty (with AADF bits in low 12) and bit-15-set
as full (with type in low 15). The renderer is AADF-aware; the
**generator's `getVoxelDataInModel` is NOT**.

**Confidence: HIGH.** Verified by direct inspection of the model's
`data_voxel[32515] = 0x08830886` (empty voxel with AADF bits), the CPU
oracle `generate_segment_cpu` returns `0x8886` (with spurious full
flag), and the C# `ImportFromVox` source code explicitly stores `0`
for empty voxels in mixed blocks (proving the C# convention is "no AADF
in ModelData empty voxels").

## Symptom recap

Stage 9 (`15-diagnostic-production-scale-readback.md`) proved that the W5
GPU producer chain produces voxels[] byte-equal to the CPU oracle
`generate_segment_cpu` at 25/25 sampled Oasis positions, at both
checkpoints (post-producer and post-bounds-calc). The producer chain is
faithfully running `generator_model.wgsl` + `chunk_calc.wgsl` and
writing what the CPU oracle says it should write.

The visible bug (`oracle_gpu.png`): correct architectural geometry, but
mostly-black surfaces with sparse bright (cream/green) specks — the
"voxel types in thousands" symptom. The renderer's chunk → block →
voxel descent hits the correct cells; the leaf voxel half-words at those
positions have bit 15 set (so the hit test fires) but their low 15 bits
decode to invalid palette indices (often > 256), so
`voxel_types[hit_type]` OOB-reads zero and decodes to black.

The brief asked: investigate 4 specific candidates (buffer wiring,
upload ordering, world-size, palette decode). All four turn out to be
**non-bugs**. The actual bug is upstream of the renderer — in the
**ModelData encoding** that the W5 producer chain CONSUMES.

## Candidate 1 — buffer-handle binding mismatch — NOT-BUG

The producer and renderer bind the SAME buffer handles.

**Producer path** (`naadf_gpu_producer_node` at
`crates/bevy_naadf/src/render/construction/mod.rs:2320-2651`):

- Uses `construction_bind_groups.construction_world` (line 2359), which
  was built in `prepare_construction` at lines 1939-1953 binding
  `world_gpu.chunks_buffer`, `world_gpu.blocks.buffer()`,
  `world_gpu.voxels.buffer()` (lines 1943-1945).

**Renderer path** (`graph.rs:103` + `graph_b.rs:213,500`):

- Uses `world_gpu.bind_group`, which was built in `prepare_world_gpu` at
  `crates/bevy_naadf/src/render/prepare.rs:552-565` binding the SAME
  `chunks_buffer`, `blocks.buffer()`, `voxels.buffer()` (lines 556-559).

**Verification: same `Buffer` handle on both sides.** The construction
bind group derives from `world_gpu` (mutable reference at `mod.rs:1109`),
not a separate allocation. `prepare_construction` reuses
`world_gpu.chunks_buffer` etc. directly.

The D1 CPU mirror readback (`03-impl.md:2475-2658`) populates
`chunks_cpu/blocks_cpu/voxels_cpu` AFTER the GPU producer ran (per the
`Stage 5 (D1)` log line in the live run) — **but the renderer does NOT
consume the CPU mirror.** The CPU mirror is for the editing-path
consumer and the bit-exact oracle. The renderer reads from
`world_gpu.{chunks,blocks,voxels}.buffer()` via the bind group.

**Verdict: NOT-BUG.** Producer writes and renderer reads land on the
same `Buffer` handles. Stage 9's byte-equality at the sample positions
confirms the renderer would read the producer's exact writes (the Stage
9 diagnostic uses the same descent path the renderer uses, just with
host-readback instead of in-shader reads).

## Candidate 2 — `upload_all(&[0u32], …)` ordering — NOT-BUG

Per the brief: `prepare.rs:418-432` might zero data after producer
writes.

Reading `prepare.rs:466-486`:

```rust
if gpu_producer_skip_upload {
    blocks.upload_all(&[0u32], &render_device, &render_queue);
    voxels.upload_all(&[0u32], &render_device, &render_queue);
} else {
    let blocks_data: Vec<u32> = if extracted.blocks.is_empty() {
        vec![0]
    } else {
        extracted.blocks.clone()
    };
    ...
    blocks.upload_all(&blocks_data, &render_device, &render_queue);
    voxels.upload_all(&voxels_data, &render_device, &render_queue);
}
```

At `prepare.rs:247`: `let gpu_producer_skip_upload = false;` (hard-coded).
So the `else` branch always runs. For the W5 install path,
`extracted.blocks.is_empty() == true` (per `grid.rs:411`), so
`blocks_data = vec![0]` and `voxels_data = vec![0]` — uploading ONE zero
u32 at offset 0 of each buffer.

**Ordering**: `prepare_world_gpu` runs in `RenderSystems::PrepareResources`,
before any render-graph node. The W5 producer runs in
`naadf_gpu_producer_node` which is in the Core3d render graph — AFTER
`prepare_world_gpu`. So the upload happens BEFORE the producer. The
producer's writes (which start at slot 64 for blocks and slot 32 for
voxels, per the cursor seeds) land AFTER the single-zero upload.

**This is a per-app-startup pattern, not per-frame.** `prepare_world_gpu`
is build-once gated (`existing.is_some()` early-return at line 202).

**Verification via Stage 9**: Stage 9 reads back voxels[VoxelPtr+pair_idx]
at sample positions like `0x8001f300+3 = 0x1f303`. These are well past
slot 0. The single-zero upload at slot 0 cannot affect slot 0x1f303 or
chunk_idx > 0.

**Verdict: NOT-BUG.** The upload writes 1 zero u32 at slot 0, BEFORE
the producer runs, and the producer's writes start at slot 32/64 with
the cursor seed. No race, no overwrite.

## Candidate 3 — `world_data_meta.size_in_chunks` mismatch — NOT-BUG

The brief: if the renderer's `size_in_chunks` differs from the
producer's `WORLD_SIZE_IN_CHUNKS = (256, 32, 256)`, every chunk index is
off and the renderer reads wrong slots.

Reading `voxel/grid.rs:413` (W5 install path):

```rust
let mut world_data = WorldData {
    chunks_cpu: Vec::new(),
    blocks_cpu: Vec::new(),
    voxels_cpu: Vec::new(),
    size_in_chunks: WORLD_SIZE_IN_CHUNKS, // = UVec3::new(256, 32, 256)
    bounding_box: ...,
    ...
};
```

`stage_world_gpu_buildonce` at `render/extract.rs:222` then copies this
into `WorldDataMeta`:

```rust
meta.size_in_chunks = world_data.size_in_chunks;
```

`prepare_world_gpu` at `render/prepare.rs:227` reads
`extracted.size_in_chunks` (where `extracted` is the
`WorldGpuStaging`-derived view). It computes `size = (256, 32, 256)` and
uploads `world_meta.size_in_chunks = (256, 32, 256)` at line 501.

Producer's per-segment dispatch at `mod.rs:2510-2515` uses
`crate::WORLD_SIZE_IN_CHUNKS.x = 256` etc.

Renderer's chunk index computation at `ray_tracing.wgsl:290-294`:
```wgsl
let chunk_idx = flatten_index(
    chunk_pos,
    world_meta.size_in_chunks.x,
    world_meta.size_in_chunks.x * world_meta.size_in_chunks.y,
);
```
With `world_meta.size_in_chunks = (256, 32, 256)`, this is exactly
`cx + cy*256 + cz*256*32` — matching the producer's
`chunk_calc.wgsl:424-426`:
```wgsl
let chunk_idx = chunk_pos.x
    + chunk_pos.y * params.size_in_chunks.x
    + chunk_pos.z * params.size_in_chunks.x * params.size_in_chunks.y;
```

**Verdict: NOT-BUG.** Renderer and producer agree on `size_in_chunks =
(256, 32, 256)` and on the x-fastest flatten formula. Stage 9 explicitly
uses the SAME `cx + cy*256 + cz*256*32` formula and reads back data that
matches what `generate_segment_cpu` says the producer should have written
at that index.

## Candidate 4 — palette / decompress_voxel_type wiring — NOT-BUG, but it IS the proximate cause of the visible artifact

`naadf_first_hit.wgsl:227-228`:
```wgsl
let voxel_type: VoxelType =
    decompress_voxel_type(voxel_types[ray_result.hit_type]);
```

`render_pipeline_common.wgsl:105-117` `decompress_voxel_type` unpacks
a `vec4<u32>` material entry. For an in-bounds `hit_type` (0..=256),
this returns the correct material. For OOB `hit_type`, WebGPU's
implementation-defined behaviour on NVIDIA Vulkan returns
`vec4<u32>(0,0,0,0)`, which decodes to a zero-material (black surface).

The palette buffer is built in `prepare_world_gpu` at
`prepare.rs:380-388` from `extracted.voxel_types`:
```rust
let voxel_types_data: Vec<GpuVoxelType> = if extracted.voxel_types.is_empty() {
    vec![GpuVoxelType { data: [0; 4] }]
} else {
    extracted.voxel_types.iter().map(GpuVoxelType::from_voxel_type).collect()
};
```

Stage 6 install ran with 257 palette entries (verified by the live log:
`"...257 palette entries..."`). Bound at slot 3 in the world bind group
(`prepare.rs:559`).

The palette is built CORRECTLY (same as the legacy path which renders
correctly). The bug is NOT that `voxel_types[N]` is mis-indexed, but
that the **renderer is asked to look up `voxel_types[N]` where N >
256**. The OOB read returns zero, which is the mechanical cause of
black surfaces, but the **N > 256** value is what's actually wrong.

`hit_type = cur_node & 0x7FFF` is read from the voxel half-word
(`ray_tracing.wgsl:336-340`):
```wgsl
let cur_voxel_pair = voxels[voxel_start_index];
cur_node = (cur_voxel_pair >> (16u * (voxel_index_in_block & 0x1u))) & 0xFFFFu;
if ((cur_node >> 15u) != 0u) {
    cur_node = cur_node | (1u << 30u);
}
```

If the voxel half-word has bit 15 set, the renderer treats it as full
and the low 15 bits are the type. For an Oasis palette of 257 entries,
valid full half-words are `0x8001..=0x8100`. **Stage 9 shows the
producer writes `0x8886` etc. at some positions — bit 15 + type bits
0x886 (= 2182). That's the bug.**

**Verdict: NOT-BUG in the palette/decode wiring itself.** The wiring is
correct. The lookup correctly indexes a 257-entry palette. The bug is
that the producer wrote `0x8886` instead of (say) `0x8086` or `0x0886`
(the empty-with-AADF case). Why it wrote `0x8886` is Candidate 5 —
ModelData encoding mismatch.

## Candidate 5 (NEW) — ModelData empty-voxel AADF encoding mismatch — **THE BUG**

### Evidence chain

1. **Confirmed: the Oasis model's `data_voxel` is INTERNALLY CONSISTENT**.
   A scan of all 10,498,368 u32s in the Oasis model's `data_voxel` finds
   ZERO half-words with `(bit 15 set) AND (type bits > 256)`. Every full
   voxel has a valid 8-bit type. (Diagnostic test ran in this dispatch;
   reverted before reporting.)

2. **Confirmed: at one specific Oasis position (186, 189, 252) the
   model's relevant `data_voxel` slot holds**:
   ```
   model.data_chunk[48464]    = 0x8004a600  (chunk_disc=2 = mixed)
   model.data_block[304702]   = 0x80007f00  (block_disc=2 = mixed)
   model.data_voxel[32515]    = 0x08830886  (low half = 0x0886 — EMPTY voxel with AADF bits)
     → half (parity 0)        = 0x0886
     → bit 15 (full flag)     = 0   ← EMPTY
     → bits 0-11 (AADF)       = 0x886
   ```

3. **The CPU oracle `generate_segment_cpu` returns `0x8886` for this
   position** — bit 15 set + type bits = 0x886. (Diagnostic test ran
   in this dispatch; reverted before reporting.)

4. **The GPU producer writes the SAME `0x8886` to `voxels[...]`** per
   Stage 9.

### Root cause: AADF bits get misread as type bits

The CPU oracle path (mirrors the GPU shader exactly):

`crates/bevy_naadf/src/aadf/generator.rs:186-190` (`get_voxel_type_in_model`):
```rust
ty = if model_voxel_index % 2 == 0 {
    voxel_comp & 0x7FFF
} else {
    (voxel_comp >> 16) & 0x7FFF
};
```

**For the empty-with-AADF half-word `0x0886`, this returns `ty = 0x886`
WITHOUT CHECKING bit 15.** The function returns a 15-bit "type" that's
actually 12 bits of AADF distance data.

Then the caller `generate_segment_cpu` at
`crates/bevy_naadf/src/aadf/generator.rs:314-316`:
```rust
if voxel1 > 0 {
    voxel1 |= 1 << 15;
}
```

`voxel1 = 0x886 > 0`, so `voxel1 |= 0x8000` → `voxel1 = 0x8886`. The
empty voxel is **falsely promoted to FULL** with AADF bits as the
"type".

The GPU shader is identical:

`crates/bevy_naadf/src/assets/shaders/generator_model.wgsl:99-103`:
```wgsl
if ((model_voxel_index % 2u) == 0u) {
    ty = model_voxel_comp & 0x7FFFu;
} else {
    ty = (model_voxel_comp >> 16u) & 0x7FFFu;
}
```

And `generator_model.wgsl:148-154`:
```wgsl
if (voxel1 > 0u) {
    voxel1 = voxel1 | (1u << 15u);
}
if (voxel2 > 0u) {
    voxel2 = voxel2 | (1u << 15u);
}
```

**Same bug in the WGSL shader.**

### Why the bug ONLY affects the W5 path, not the legacy CPU path

The legacy CPU path (`install_vox_sized_to_model` →
`build_world_from_vox`) puts the `ConstructedWorld`'s voxels DIRECTLY
into `WorldData.voxels_cpu`. `prepare_world_gpu` uploads `voxels_cpu`
into `world_gpu.voxels.buffer()`. The renderer reads from
`world_gpu.voxels` via `ray_tracing.wgsl:335-341`:
```wgsl
let cur_voxel_pair = voxels[voxel_start_index];
cur_node = (cur_voxel_pair >> (16u * (voxel_index_in_block & 0x1u))) & 0xFFFFu;
if ((cur_node >> 15u) != 0u) {
    cur_node = cur_node | (1u << 30u);
}
```

**The renderer DOES check bit 15.** Empty voxels (bit 15 clear) are
correctly treated as empty (with AADF bits in the low 12 for the DDA
skip-distance computation). Full voxels (bit 15 set) get bit 30 set and
the hit test fires.

The renderer is AADF-aware. The W5 GPU producer's
`get_voxel_data_in_model` is NOT.

### Why C# renders this correctly

The C# reference at `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs`
has TWO `dataVoxel` encoders:

- **`CreateDataForRender`** (lines 98-108) — for the worldData→model
  round-trip when saving as `.cvox`. This DOES check bit 15:
  ```csharp
  if ((voxel1 >> 15) != 0)
      voxel1 = (1 << 15) | types[voxel1 & 0x7FFF].renderIndex;
  if ((voxel2 >> 15) != 0)
      voxel2 = (1 << 15) | types[voxel2 & 0x7FFF].renderIndex;
  ```
  Empty voxels keep whatever bits they had (the WorldData's encoder DOES
  emit AADF bits for empty mixed-block voxels — same as the Rust port).

- **`ImportFromVox`** (lines 356-526) — for first-time `.vox` import.
  This is the path Oasis takes when loaded fresh from disk. The relevant
  encoder at lines 442-446:
  ```csharp
  typeImport1 = typeImport1 | (typeImport1 > 0 ? (1u << 15) : 0);
  typeImport2 = typeImport2 | (typeImport2 > 0 ? (1u << 15) : 0);
  newVoxels[v / 2] = typeImport1 | (typeImport2 << 16);
  ```
  Note: `typeImport1` is the raw `.vox` palette index (1..=256) for full
  voxels, OR **literal zero** for empty voxels. There is NO AADF
  computation in `ImportFromVox`. The encoder produces:
  - Full voxel half-word = `(1 << 15) | (palette_index)` ∈ {0x8001..=0x8100}
  - Empty voxel half-word = `0` (literal zero, NO AADF bits)

**The C# `ImportFromVox` ModelData encoding has empty voxels as literal
zero.** When `getVoxelDataInModel` reads `voxel_comp & 0x7FFF` for an
empty position, it gets 0; `voxel1 |= voxel1 > 0 ? (1<<15) : 0` is a
no-op. Empty voxels remain empty. ✓ Works.

The Rust port unified the model encoder with the world encoder
(`build_constructed_world_sparse`). Both paths now produce
AADF-bearing empty voxels. The world encoder is correct for the
renderer; the model encoder is **incorrect for the W5 GPU producer's
generator** which expects the C# `ImportFromVox` zero-empty convention.

## Legacy vs W5 install path diff

| Aspect | Legacy `install_vox_sized_to_model` (`grid.rs:254`) | W5 `install_vox_in_fixed_world` (`grid.rs:317`) |
|---|---|---|
| World size | sized to model (e.g. 93×34×84 chunks → 1488×544×1344 voxels) | fixed 256×32×256 chunks → 4096×512×4096 voxels |
| WorldData.chunks_cpu/blocks_cpu/voxels_cpu | populated via `build_constructed_world_sparse` | empty (W5 producer writes them via GPU) |
| ModelData inserted | NO | YES (`grid.rs:393-399`) |
| W5 GPU producer chain runs | NO (`dense_voxel_types.is_empty() = true` AND no ModelData → skip) | YES (`model_data_present = true` → branch (a)) |
| Render-time voxels source | `voxels_cpu` uploaded via `prepare_world_gpu` to `world_gpu.voxels` | GPU producer writes to `world_gpu.voxels` directly |
| ModelData empty-voxel encoding | N/A (no model data) | uses `build_constructed_world_sparse` → emits AADF for empty voxels **← BUG** |
| What the renderer reads | The `voxels_cpu` AADFs (correctly interpreted via bit-15 check) | The producer's writes (incorrectly AADF-promoted to "full" with type=AADF-bits) |

**The bug is exactly in the W5 path's ModelData encoding.** The legacy
path doesn't have a ModelData, so it doesn't hit the bug.

## Identified bug

**File**: `crates/bevy_naadf/src/voxel/vox_import.rs`

**Lines**: `build_constructed_world_sparse` at `:860-1053`, specifically
the call to `encode_block_voxels(&block_voxels, new_ptr, &mut voxels_cpu)`
at `:977-981`.

**Indirect line: `crates/bevy_naadf/src/aadf/construct.rs:372-376`** —
the `encode_block_voxels` function emits `VoxelCell::Empty(aadf).encode()`
for empty voxels in mixed blocks. This produces a u16 half-word with the
12-bit AADF in the low bits, bit 15 clear.

**For the W5 ModelData path, this encoding is WRONG.** The C# reference
encoder (`NAADF/World/Model/ModelData.cs:442-446` in `ImportFromVox`)
emits literal `0` for empty voxels in the ModelData's `dataVoxel`. The
GPU shader (`generator_model.wgsl`) assumes the C# convention; it does
not check bit 15 before reading the type bits.

The same encoding IS correct for the renderer's `voxels[]` buffer —
which is why the legacy CPU path renders correctly. The bug is that
**the Rust port uses the renderer's voxel encoding for the model data
too**, when these are semantically different formats.

### Byte-level evidence

- Position (186, 189, 252) in the Oasis 4096×512×4096 world:
  - Model chunk (cx=11, cy=11, cz=15) → `data_chunk[48464] = 0x8004a600` (mixed, BlockPtr=0x4a600).
  - Model block (bx=2, by=3, bz=3 within chunk → block_index=62) → `data_block[0x4a600+62 = 304702] = 0x80007f00` (mixed, VoxelPtr=0x7f00).
  - Model voxel (lx=2, ly=1, lz=0 within block → mvi=6, mvi/2=3) → `data_voxel[0x7f00+3 = 32515] = 0x08830886`.
  - Half-word at parity 0 = `0x0886` (= empty voxel, bit 15 clear, AADF bits = 0x886).
- CPU oracle `generate_segment_cpu` for this position returns voxel pair `0x88838886` with half `0x8886` (= full, type 0x886 = 2182).
- GPU producer's `voxels[VoxelPtr+pair_idx]` is byte-identical: `0x88838886` (per Stage 9 row 5).
- Renderer reads `0x8886`, sees bit 15 set, sets bit 30, hit fires, `hit_type = 0x886`.
- `voxel_types[2182]` is OOB on the 257-entry palette → returns zero → black surface.

Other affected sample positions per Stage 9 (all share the same
empty-voxel-with-AADF-bits pattern):
- (1302, 231, 168) → type 0x8c34 → 0xc34 = 3124 (also bits 11-12 set in
  AADF representation).
- (1116, 210, 756) → 0x8028 valid but pair `0x8c688028` — the OTHER voxel
  in the pair (0x8c68, type 0xc68 = 3176) is wrong.
- (930, 210, 84) → 0x8866, type 0x866 = 2150.
- Etc.

## Recommended fix (NOT to be implemented)

### Minimal fix: emit literal zero for empty voxels in `ModelData.data_voxel`

The Rust port needs a SEPARATE encoder for `ModelData.data_voxel` that
mirrors C# `ImportFromVox`'s "literal zero for empty voxels" convention.
The renderer's `voxels[]` encoding (with AADFs in empty voxels) is
correct and should NOT change.

**Surface 1 — preferred**: add a flag or a separate function path in
`build_constructed_world_sparse` (or its inner `encode_block_voxels`)
that, when building for `ModelData`, emits `VoxelCell::Empty(_).encode()
= 0` (literal zero, no AADF computation) for empty voxels.

The `install_vox_in_fixed_world` at `grid.rs:393-398` builds
`ModelData` from `imp.world.{chunks, blocks, voxels}` where
`imp.world` is the output of `build_constructed_world_sparse`. The
simplest fix is to **post-process `imp.world.voxels` in place** before
inserting into `ModelData`: walk every half-word, and for any half-word
where bit 15 is clear, zero it out.

**Concrete patch** at `crates/bevy_naadf/src/voxel/grid.rs:393-399`:

```rust
let model_data = crate::aadf::generator::ModelData {
    data_chunk: imp.world.chunks,
    data_block: imp.world.blocks,
    data_voxel: imp.world.voxels.iter().map(|&pair| {
        // C# `ImportFromVox` convention: empty voxels in ModelData are
        // literal zero, NOT AADF-encoded. The renderer's `voxels[]` uses
        // AADFs (and that's correct for the renderer), but the GPU
        // generator (`generator_model.wgsl::get_voxel_data_in_model`)
        // reads the half-word as a raw type without checking bit 15;
        // any non-zero AADF bits get falsely promoted to "full" with
        // garbage type. Strip AADF bits from empty voxels here.
        let lo = pair & 0xFFFF;
        let hi = (pair >> 16) & 0xFFFF;
        let lo_out = if (lo & 0x8000) != 0 { lo } else { 0 };
        let hi_out = if (hi & 0x8000) != 0 { hi } else { 0 };
        lo_out | (hi_out << 16)
    }).collect(),
    size_in_chunks: model_size_in_chunks,
};
```

This is a 1-place fix that preserves the renderer's encoding while
making the ModelData faithful to C# `ImportFromVox`.

**Alternative — preferred for long-term correctness**: fix
`generator_model.wgsl::get_voxel_data_in_model` to check bit 15 of the
half-word before returning the type bits:

```wgsl
let half = select(
    model_voxel_comp & 0xFFFFu,
    (model_voxel_comp >> 16u) & 0xFFFFu,
    (model_voxel_index % 2u) == 1u,
);
if ((half & 0x8000u) == 0u) {
    ty = 0u;  // empty voxel — ignore AADF bits
} else {
    ty = half & 0x7FFFu;
}
```

And mirror the fix in `crates/bevy_naadf/src/aadf/generator.rs:186-190`
for `get_voxel_type_in_model`.

**This alternative diverges from C# `generatorModel.fx` byte-for-byte
faithfulness**, but it makes the function self-defending against the
encoding mismatch. The "faithful port" defense for the C# shader's
behavior breaks down here because the C# shader is paired with a C#
input encoder that emits zero for empty voxels — and the Rust port
removed that property.

**Project-rule note** (`CLAUDE.md` "faithful port"): the C# port rule is
"match C# behavior, even when C# has the bug, unless explicit user
approval + docs entry". The Surface-1 patch (strip AADFs from
ModelData) restores C#-faithfulness end-to-end. The alternative
(self-defending shader) is a divergence and would need explicit
approval.

### Why this fix is minimal

- No change to the renderer.
- No change to `prepare_world_gpu`, `prepare_construction`, or the GPU
  producer chain.
- No change to the world's `voxels_cpu` encoding (the legacy CPU path
  stays byte-equal to current behaviour).
- Only the W5 install path's `ModelData.data_voxel` is touched, and
  only the empty-voxel half-words.

### Expected outcome

- Stage 9 byte-equality will REMAIN green (CPU oracle and GPU both
  consume the fixed ModelData → both produce the same byte-correct
  voxels[]).
- The voxels[] half-words at "bad" positions like (186, 189, 252) will
  flip from `0x8886` → `0x0000` (empty). The renderer's chunk → block →
  voxel descent will correctly classify the cell as empty, NOT hit, and
  the ray continues through. Surfaces that were rendering black with
  invalid types will now show what the model actually has at those
  positions (which may be empty space — letting the ray hit the actual
  surface BEHIND that voxel slot).
- The Oasis architecture renders correctly with the proper palette.

### Expected confirmation tests

1. **Existing**: `--validate-gpu-construction-production` should still
   pass at 25/25 positions, but the GOOD oracle should now be the
   AADF-stripped one. Update the oracle in
   `validate_gpu_construction_production_scale` to match the fix.
2. **New regression gate**: add a unit test that asserts no full
   half-word in `ModelData.data_voxel` has type > 256 after the fix.
3. **Visual**: `--vox-gpu-oracle` should produce `oracle_gpu.png` that
   matches `oracle_cpu.png` (cream walls, palm trees, sky — same as
   the legacy path).

## Confidence level

**HIGH.**

- **Byte-level evidence** that the model's `data_voxel[32515] = 0x08830886`
  contains an empty voxel with AADF bits (bit 15 clear, bits 11-0 set).
- **Byte-level evidence** that the CPU oracle `generate_segment_cpu`
  returns `0x8886` for that position (= bit 15 set + AADF bits as type).
- **Byte-level evidence** that the GPU producer writes the same value to
  `voxels[]` (Stage 9 row 5).
- **C# reference source** (`NAADF/World/Model/ModelData.cs:442-446`)
  explicitly shows `ImportFromVox` emits `0` for empty voxels in
  `dataVoxel`, confirming the encoding convention the Rust port should
  match.
- **Renderer logic verified** to bit-15-check correctly
  (`ray_tracing.wgsl:339-341`) — explaining why the legacy path
  (uploading the AADF-bearing voxels_cpu directly to the renderer) works.
- **W5-only failure mode verified**: legacy `install_vox_sized_to_model`
  has no ModelData, so the generator chain is bypassed; W5
  `install_vox_in_fixed_world` runs the generator chain and triggers the
  bug.
- The fix is **mechanically straightforward** (1 place, ~10 lines).

## Cross-references

- Stage 9 diagnostic: `15-diagnostic-production-scale-readback.md`
  (byte-equality post-producer & post-bounds-calc at 25/25 positions).
- Stage 8 type-decode hypothesis space: `14-diagnostic-type-decode.md`
  (Q1–Q5, P1–P4 all refuted or downgraded; this dispatch adds the
  Candidate-5 ModelData encoding mismatch).
- Symptom screenshots: `target/e2e-screenshots/oracle_cpu.png` vs
  `oracle_gpu.png` (`14-diagnostic-type-decode.md:11-35`).
- Producer code path:
  - `crates/bevy_naadf/src/render/construction/mod.rs:2384-2566`
    (`naadf_gpu_producer_node` W5 branch).
  - `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl:62-119`
    (`get_voxel_data_in_model`) — the WGSL with the bug.
  - `crates/bevy_naadf/src/aadf/generator.rs:122-207`
    (`get_voxel_type_in_model`) — the CPU oracle mirroring the bug.
  - `crates/bevy_naadf/src/aadf/generator.rs:239-335`
    (`generate_segment_cpu`) — the caller that applies the
    `voxel1 |= voxel1 > 0 ? (1<<15) : 0` false-promotion.
- ModelData encoder paths:
  - `crates/bevy_naadf/src/voxel/grid.rs:393-398`
    (`install_vox_in_fixed_world` — where ModelData is constructed).
  - `crates/bevy_naadf/src/voxel/vox_import.rs:860-1053`
    (`build_constructed_world_sparse` — emits AADF-bearing empty voxels).
  - `crates/bevy_naadf/src/aadf/construct.rs:355-386`
    (`encode_block_voxels` — the canonical AADF-emitting encoder).
- Renderer paths (all verified correct):
  - `crates/bevy_naadf/src/render/prepare.rs:184-583` (`prepare_world_gpu`).
  - `crates/bevy_naadf/src/render/construction/mod.rs:1069-2297`
    (`prepare_construction`).
  - `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:283-401`
    (chunk → block → voxel descent).
  - `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl:227-228`
    (palette lookup).
- C# reference (the "correct" model encoder):
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:356-526`
    (`ImportFromVox`).
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:74-124`
    (`CreateDataForRender` — does check bit 15, for the SAVE path).
- C# generator shader (bit-identical to ours):
  - `/mnt/archive4/DEV/NAADF/NAADF/Content/shaders/world/generator/generatorModel.fx`
    (line 40 — same `& 0x7FFF` without full-flag check).
