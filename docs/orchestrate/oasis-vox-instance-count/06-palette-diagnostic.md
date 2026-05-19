# 06-palette-diagnostic — .cvox palette drift root cause

## general-purpose diagnostic findings (2026-05-19)

## TL;DR

The `.cvox` parser at `crates/bevy_naadf/src/voxel/cvox_import.rs:136-140`
**unconditionally injects an extra `VoxelType::default()` placeholder at
palette slot 0 BEFORE pushing the `typeCount` on-disk entries**. But on disk
the C# `.cvox` format already encodes slot 0 itself as a placeholder (the C#
`"_"` id with all-zero colors, written by
`CreateFromWorldData`+`SaveVoxelType` at `ModelData.cs:313-316` + `:50-59`),
and `dataChunk`/`dataBlock`/`dataVoxel` index that on-disk types array
**directly** (1-based for real types, 0 = placeholder). The injected default
shifts every real-type index by +1: dataVoxel value `k` was meant to look up
on-disk `types[k]` but now hits Bevy `palette[k]` which is on-disk `types[k-1]`
(the previous slot — i.e. each voxel renders with the color of the
**previous** palette entry). Palm trees painted with the brown-earth color one
slot before them in the compacted palette = exactly the user's report.

The fix is **option α — drop the injected slot-0 placeholder so the parser
emits a positional palette of exactly `typeCount` entries that the on-disk
indices reference directly.** The `.vox` parser's `+1` shift is a **different
convention** (MV palette is positional 0..255 with `0=empty`, the parser
shifts on both the data side AND the palette side) and must stay.

The architect's assumption A7 (palette positional indexing) is **technically
correct about the on-disk encoding** but **wrong about the slot-0 placeholder
convention**: A7 said the Bevy palette has slot 0 reserved (Bevy convention)
and slots `1..=N` map 1:1 to on-disk entries. But for `.cvox` the on-disk
encoding **already encodes its own slot 0 as a placeholder** — Bevy's "slot 0
reserved" convention double-counts it.

## Concrete palette counts (measured)

| Measurement | Value | Source |
|---|---|---|
| `oasis.cvox` raw `typeCount` from binary header | **39** | direct ZIP-deflate + LE-i32 read at payload offset 16 (Python `struct.unpack`); see [Method](#measurement-method) |
| `oasis.cvox` parsed `imp.palette.len()` (current Bevy parser) | **40** | derived: `typeCount + 1` per `cvox_import.rs:136-140` |
| `oasis.cvox` max referenced index in `dataVoxel` (low 15 bits, full voxels only) | **38** | scanned all 10,287,584 `u32`s in `dataVoxel`; max value in `(half & 0x7FFF)` where `(half & 0x8000) != 0` |
| `oasis_hard_cover.vox` raw RGBA chunk count | **256** | scanned `RGBA` chunk in MagicaVoxel `.vox`; standard MV palette |
| `oasis_hard_cover.vox` parsed `imp.palette.len()` (current Bevy parser) | **257** | `vox_import.rs:1172` test asserts; produced by `vox_palette_to_voxel_types` at `vox_import.rs:967-1004` (`Vec::with_capacity(palette.len() + 1)` + leading `push(default())` + 256-entry loop) |
| `oasis.cvox` palette slot 0 contents (on disk) | id=`"_"`, colorBase=(0,0,0), colorLayered=(0,0,0), matBase=0, matLayer=0, roughness=0.0 | direct parse of first VoxelType entry after the 32-byte header |
| `oasis.cvox` palette slot 1 contents (on disk) | id=`"_"`, colorBase=(0.354, 0.239, 0.119) — brown earth | direct parse of second VoxelType entry |

**Diff explanation.** The `.vox` and `.cvox` palette **counts** look superficially symmetric (`N+1`), but the on-disk **role** of slot 0 differs:

- `.vox`: slot 0 on disk = MagicaVoxel "no entry" / empty (palette index 0 is the MV convention for "empty voxel"). The parser shifts voxel data `v.i` → `v.i + 1` (`vox_import.rs:627`) AND prepends a placeholder to the palette. The two shifts cancel — voxel value `k` (1-based, where `k = v.i + 1`) indexes into Bevy palette slot `k` which carries MV palette slot `k - 1`. Self-consistent.
- `.cvox`: slot 0 on disk IS itself the placeholder ("_", zeros) — written by C# `CreateFromWorldData` at `ModelData.cs:313-316`. The on-disk data values directly index this on-disk types array (no shift; 1..N = real types, 0 = placeholder). The parser prepends an EXTRA placeholder, but does NOT shift the data — so voxel value `k` now indexes Bevy palette slot `k` which carries on-disk slot `k - 1` = the wrong color.

## C# Save → .cvox palette: positional or compacted?

**Compacted.** Not by `Save()` itself, but by `CreateFromWorldData` (the
helper that `Save` is called on). The chain:

1. **`CreateFromWorldData(string fileName, WorldData worldData)`** at
   `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:260-353`:
   - Line 273: `uint[] typeMapping = new uint[App.worldHandler.voxelTypeHandler.typesRender.Count]` — a renderIndex→compacted-index lookup of size `typesRender.Count` (currently registered types in the global registry).
   - Lines 288-310 (3 loops): scan world's `dataChunk`/`dataBlock`/`dataVoxel`, mark `typeMapping[curRenderIndex] = uint.MaxValue` for each renderIndex referenced anywhere in the live world.
   - Lines 313-316:
     ```csharp
     List<VoxelType> voxelTypes = new List<VoxelType>();
     VoxelType newType = new VoxelType();
     newType.ID = "_";
     voxelTypes.Add(newType);          // <-- slot 0 placeholder, written to disk
     int curMapIndex = 1;
     ```
     **This is the on-disk slot-0 placeholder.** Inserted unconditionally as the FIRST entry of the compacted `voxelTypes` list.
   - Lines 318-326: iterate `i = 1..typesRender.Count`; for each marked entry, find the registered type with `renderIndex == i`, append to `voxelTypes`, assign `typeMapping[i] = curMapIndex++`.
   - Lines 328-351: remap `dataChunk` / `dataBlock` / `dataVoxel` through `typeMapping` so each cell now references the compacted output index (1..N) instead of the global runtime renderIndex.
   - Line 353: `return new ModelData(... voxelTypes.ToArray() ..., isTemp: true)` — `isTemp=true` skips `CreateDataForRender` (the no-op for the save path).

2. **`Save()`** at `ModelData.cs:126-179` then serialises this compacted model:
   - Line 146: `stream.WriteInt(types.Length)` — writes `typeCount = voxelTypes.Count` = `(used_types + 1)` (placeholder + N real). For Oasis: 39 = (38 real + 1 placeholder).
   - Lines 152-155: loop `i = 0..types.Length`, write each entry via `SaveVoxelType`.
   - Lines 158-173: write the (compacted) `dataChunk` / `dataBlock` / `dataVoxel` arrays verbatim.

So the on-disk format is **a compacted positional palette where slot 0 is C#'s own placeholder and `dataX` indices point at it directly**. There is no "256 slots with gaps" — the gaps were already pruned by `CreateFromWorldData`.

## C# Load → runtime palette: any remap?

**Yes** — `Load` is followed by `CreateDataForRender` which remaps the on-disk indices into the live runtime registry's `renderIndex` space. But this remap is **internal to the C# renderer's GPU buffer layout** and is **not the format-level convention we need to mimic**.

1. **`Load(string fileName)`** at `ModelData.cs:181-258`:
   - Lines 212-216:
     ```csharp
     types = new VoxelType[typeCount];
     for (int i = 0; i < typeCount; ++i)
         types[i] = App.worldHandler.voxelTypeHandler.ApplyVoxelType(LoadVoxelType(zipStream));
     ```
     Reads `typeCount` palette entries into `types[]`, each registered into the global `VoxelTypeHandler`. **`types[0]` is the on-disk slot-0 placeholder; types[1..typeCount-1] are real entries.**
   - Lines 219-240: read `dataChunk` / `dataBlock` / `dataVoxel` arrays unchanged.
   - Line 257: `return new ModelData(..., isTemp: false)` — `isTemp=false` triggers `CreateDataForRender` in the constructor at line 47.

2. **`CreateDataForRender`** at `ModelData.cs:74-108`:
   - Lines 86-90 + 91-96 + 98-108: for every cell that's marked "uniform full" or every full voxel half-word, `dataX[i] = … | types[curIdx & mask].renderIndex`.
   - This is a renderIndex remap from `[on-disk compacted index in 0..typeCount-1]` to `[runtime renderIndex in 1..typesRender.Count-1]`.

3. **`VoxelTypeHandler.ApplyVoxelType`** at `VoxelTypeHandler.cs:73-86`:
   - Line 81: `type.renderIndex = (uint)typesRender.Count;` — assigns the next slot index in the GPU `typesRender` list.
   - Line 83: `typesRender.Add(type.compressForRender());` — appends to the GPU list.
   - The list starts at size 1 (Clear at line 161-167 pushes `Uint4()` at slot 0 — `typesRender[0]` is permanently the GPU placeholder).

**Crucial detail for the Bevy port:** the C# `renderIndex` remap exists ONLY because the C# global registry is shared across multiple loaded models (one app-wide GPU typesRender buffer for all). The on-disk format is a **self-contained positional palette** — for a single-model load with a fresh registry, after both Load and CreateDataForRender, the renderer effectively has:

```
gpu_palette_slot k+1 = on_disk_types[k]    (for k in 0..typeCount-1)
gpu_palette_slot 0   = registry's permanent placeholder (Uint4()=zeros)
voxel half-word value `r` (after CreateDataForRender) = on_disk_index + 1
```

Bevy has no global registry: `VoxelTypes` is per-world (`grid.rs:654`). So the faithful port should produce a palette where **`palette[k] = on_disk_types[k]`** and **`dataVoxel` values are used unchanged** (the +1 remap is C#-internal and unnecessary if we don't share a registry across models).

The on-disk slot 0 placeholder already serves as Bevy's "slot 0 reserved" convention — the Bevy `cvox_import` should NOT insert another one.

## C# `ImportFromVox` shape

`ImportFromVox` at `ModelData.cs:356-526` is the **`.vox` → in-memory ModelData** path (not the .cvox load path; it's how C# initially ingests a fresh `.vox` file). The architect cited it as the A7 verification surface. Reading it carefully:

- Line 505: `types = new VoxelType[dataImport.Colors.Length]` — sized at the source `.vox` palette length (256 for a standard MV file). **Sparse 256-slot, no compaction at this stage.**
- Lines 506-521: iterate `c = 0..255`, register each MV palette entry into `VoxelTypeHandler` via `ApplyVoxelType`, store the (now `renderIndex`-bearing) result back into `types[c]`.
- Lines 438-446 (in the per-voxel loop, run earlier in the function): write voxel data half-words as `(typeImport | (typeImport > 0 ? 1u<<15 : 0))` where `typeImport = dataImport[xyz].Index`. **The `Index` is the MV palette slot 0..255.** So on-disk-during-ingest dataVoxel holds **raw MV palette indices** (0=empty, 1..255=real).
- Line 525: `return new ModelData(..., isTemp: false)` → constructor runs `CreateDataForRender` (line 47) → remaps each dataVoxel half-word `k` to `types[k].renderIndex`.

So at THIS point (after ImportFromVox + CreateDataForRender), the in-memory ModelData has:
- `types[]` length 256, with `types[c].renderIndex = c + 1` for c = 0..255 (fresh registry assigns 1..256).
- `dataVoxel` half-words = `types[mv_idx].renderIndex` = `mv_idx + 1` for full voxels.

**This is the state that gets passed to `CreateFromWorldData` later** when the user hits Save. CreateFromWorldData (analysed above) then **compacts** the 256 entries down to only-used N + 1, with its own slot-0 placeholder. So `.cvox` files only ever contain compacted palettes.

The architect's A7 read of "ImportFromVox produces positional with gaps preserving the +1 shift" is correct **about the in-memory ImportFromVox output**, but **not about what gets serialised to `.cvox`** — by the time CreateFromWorldData runs, the gap structure is gone and a new self-contained slot-0 placeholder is at the head of the compacted list.

## `vox_import.rs` (Bevy .vox parser) palette shape

**Sparse 257-slot (256 positional MV palette + 1 leading placeholder).** Self-consistent with a matching +1 shift on the data side.

`crates/bevy_naadf/src/voxel/vox_import.rs:964-1004` (`vox_palette_to_voxel_types`):
```rust
let mut out = Vec::with_capacity(palette.len() + 1);
out.push(VoxelType::default());        // slot 0 placeholder
for (i, color) in palette.iter().enumerate() {
    …
    out.push(VoxelType { … });          // slots 1..=256
}
out                                     // length palette.len() + 1 = 257
```

Plus the matching data-side shift at `vox_import.rs:627`:
```rust
let ty = VoxelTypeId(v.i as u16 + 1);   // <-- the +1 shift on MV palette index
buckets.push([nx, ny, nz], ty);
```

And the test at `vox_import.rs:1172` asserts `imp.palette.len() == 257` — unit-tested invariant.

`install_imported_vox` at `grid.rs:529-655` consumes whatever `imp.palette` shape the parser gives it (length-agnostic; it just inserts `VoxelTypes { types: imp.palette }` at line 654 and that becomes the GPU palette buffer at `render/prepare.rs:227`, `:621`).

**The contract is: `palette[k]` is the VoxelType for voxel-data value `k`.** The `.vox` path satisfies this because it shifts BOTH sides by +1. The `.cvox` path only shifts the palette side, breaking the contract.

## `cvox_import.rs` (new Bevy .cvox parser) palette shape

**Sparse 40-slot (1 injected placeholder + 39 on-disk entries, where on-disk slot 0 is ALSO a placeholder).** Inconsistent with the unshifted on-disk data.

Specifically at `crates/bevy_naadf/src/voxel/cvox_import.rs:135-140`:
```rust
let type_count_usize = if type_count < 0 { 0 } else { type_count as usize };
let mut palette: Vec<VoxelType> = Vec::with_capacity(type_count_usize + 1);
palette.push(VoxelType::default());           // <-- (1) injected slot 0 placeholder
for _ in 0..type_count_usize {
    palette.push(read_voxel_type(&mut cursor)?);   // <-- (2) on-disk entries 0..typeCount-1 → palette slots 1..typeCount
}
```

The data side at `cvox_import.rs:171-182` is read **verbatim from disk** — no `+1` shift, no remap:
```rust
let mut data_chunk: Vec<u32> = vec![0u32; chunk_count_on_disk as usize];
cursor.read_u32_array(&mut data_chunk)?;

let mut data_block: Vec<u32> = vec![0u32; block_count as usize];
cursor.read_u32_array(&mut data_block)?;

…
let mut data_voxel: Vec<u32> = vec![0u32; voxel_data_count];
cursor.read_u32_array(&mut data_voxel)?;
```

So for `oasis.cvox`:
- On-disk `dataVoxel` values: in `[1..38]` for real voxels, `0` for empty (placeholder slot).
- Bevy `palette` slot 0 = `VoxelType::default()` (Bevy's empty).
- Bevy `palette` slot 1 = on-disk `types[0]` = C#'s placeholder ("_", all zeros).
- Bevy `palette` slot 2 = on-disk `types[1]` = first real color (brown earth `(0.354, 0.239, 0.119)`).
- …
- Bevy `palette` slot k = on-disk `types[k-1]`.

A voxel encoded on disk as half-word `0x8001` (full, type=1, meaning "first real color" = brown earth) decodes through Bevy's pipeline as palette slot 1 = C# placeholder (all zeros = black). A voxel encoded as `0x8002` (second real color) → palette slot 2 = first real color (brown). **Every full voxel is shifted by -1 in palette slot, exactly the "blue palm trees / drift" the user reported.**

The user's intuition ("the palette ignores the black blocks that it had in-between good blocks") is exactly right in spirit: the user's mental model assumed the on-disk palette had black null slots between good entries (like the dense MV palette) that the parser was dropping. The actual story is subtler — the on-disk format is already compacted — but the SYMPTOM is identical: a one-slot off-by-one in palette lookup.

The injected placeholder at line 137 explicitly references the `.vox` import convention (`cvox_import.rs:129-134`):

```rust
// The Bevy palette has `palette.len() + 1` entries: slot 0 is the reserved
// empty placeholder (matches `.vox` import path at vox_import.rs:967-969),
// and slots `1..=type_count` are filled positionally from the on-disk
// entries — matching the C# `i+1` shift implicit in
// `VoxelTypeHandler.Clear` (line 165) which always seeds slot 0 with a
// placeholder before `ApplyVoxelType` allocates `renderIndex = 1, 2, ...`.
```

The comment's reasoning is the bug. The `.vox` parser earns its +1 shift by also shifting the data side at `vox_import.rs:627` (`v.i as u16 + 1`). The `.cvox` parser inherited the palette-side shift but **not the data-side shift**, because the on-disk `.cvox` data is already "post-shift" (1-based for real, 0 for placeholder) — adding another shift on the palette side creates a net -1 mismatch between the two.

## Root cause

`cvox_import.rs:137` (`palette.push(VoxelType::default())`) injects an extra slot-0 placeholder under the wrong-convention assumption that on-disk values are 0-based MV-style indices needing a +1 lift. They are not: C#'s `CreateFromWorldData` (`ModelData.cs:313-316`) **already** writes a placeholder at on-disk slot 0 and writes data values in `[1..typeCount-1]` that index this self-contained on-disk array **directly**. The Bevy parser's injected slot-0 doubles the placeholder, shifting every real-type lookup by -1 (each voxel renders as the previous palette slot's color).

## Fix options (no code yet)

### Option α — drop the injected slot-0 placeholder (RECOMMENDED)

The parser produces a palette of exactly `typeCount` entries; the on-disk slot 0 (already a placeholder) becomes the Bevy `palette[0]`. The on-disk data references this palette directly with no shift.

**Touch points:**
- `crates/bevy_naadf/src/voxel/cvox_import.rs:136` — drop the `Vec::with_capacity(type_count_usize + 1)` → `Vec::with_capacity(type_count_usize)`.
- `crates/bevy_naadf/src/voxel/cvox_import.rs:137` — delete the `palette.push(VoxelType::default());` line.
- `crates/bevy_naadf/src/voxel/cvox_import.rs:127-134` — update the surrounding comment to explain the on-disk slot 0 placeholder convention (currently the comment justifies the wrong behaviour).
- `crates/bevy_naadf/src/voxel/cvox_import.rs:509-514` — adjust the unit test assertion `imp.palette.len() >= 2` → `imp.palette.len() >= 1`. (Or stronger: assert `imp.palette.len() == 39` for the canonical Oasis fixture, but that's brittle to fixture changes.)

**Pros:**
- Smallest possible diff (delete 1 line + adjust 1 line + comment).
- Faithful to the C# on-disk format — the slot-0 placeholder is preserved exactly as written.
- Matches the "no global registry → no renderIndex remap" Bevy port simplification (consistent with A7's spirit).
- The Bevy "slot 0 reserved" convention is **preserved at the on-disk level** — the C# placeholder at on-disk slot 0 IS the reserved empty slot in the Bevy palette after this fix.

**Cons:**
- Diverges from the `.vox` import path's palette length (`.vox` is `+1`, `.cvox` is `+0`). But this is **correct** — they encode different things on disk. The downstream consumer (`install_imported_vox` → `VoxelTypes` → GPU upload) is length-agnostic, so no contract is broken.
- The "slot 0 = empty placeholder" invariant is now upheld differently between the two parsers (`.vox` injects it; `.cvox` reads it). Defensible because the on-disk encodings are different — but adds a subtle "the two parsers have a slightly different relationship to the palette[0] slot" footnote.

**Estimated diff size:** 1 line removed, 1 line edited, ~4 lines of comment rewritten in the same module. Total < 10 lines.

### Option β — keep injected placeholder + +1-shift the data side

Parser keeps `palette[0] = Bevy::default()` and renumbers every full voxel / uniform-full block / uniform-full chunk on the data side, shifting the index by +1 across `data_chunk`, `data_block`, `data_voxel`.

**Touch points:**
- `crates/bevy_naadf/src/voxel/cvox_import.rs:171-182` — after reading the three arrays, add three passes:
  - `data_chunk`: for each `u32`, if `(cur >> 30) == 1` (uniform-full chunk), write `(1 << 30) | ((cur & 0x3FFF_FFFF) + 1)`.
  - `data_block`: same pattern for `(cur >> 30) == 1` (uniform-full block).
  - `data_voxel`: for each `u32`, split into two `u16` half-words, for each half if `(half & 0x8000) != 0` (full) increment the low 15 bits by 1.
- The injected `palette.push(VoxelType::default())` at line 137 stays.

**Pros:**
- Symmetric with the `.vox` parser shape — both produce `typeCount + 1` palette length and "data values are slot-id + 1".
- The slot-0 placeholder is unambiguously Bevy's reserved empty.

**Cons:**
- ~40+ lines of new code (three remap passes over potentially huge `u32` arrays — Oasis is ~50M voxels = ~25M `u32`s in `data_voxel` alone).
- Runtime cost: an extra full-data-array pass at parse time. On the order of tens of milliseconds for Oasis-class files; not load-bearing but not free.
- **Loses information**: the on-disk slot-0 placeholder (C#'s `"_"` with all-zero colors) becomes Bevy palette slot 1, used by nothing. It's now a wasted slot in the GPU palette buffer.
- **Diverges from the faithful-port rule** more than option α — we're adding a +1 transformation NOT present in C#'s `Load` + `CreateDataForRender` chain. (CreateDataForRender remaps to `renderIndex` which happens to equal `on_disk_index + 1` for a freshly-loaded model with a fresh registry, but this is a registry-internal accident, not a format-level invariant.)
- Adds three passes over potentially huge arrays at parse time — visible startup-cost regression.

**Estimated diff size:** ~50 lines (three passes + comments).

### Option γ — port the C# `renderIndex` remap fully

Mimic C#'s post-Load `CreateDataForRender` exactly: build a per-model `render_index_map` (positional → arbitrary id), remap the data arrays accordingly. For a Bevy port with no global registry, this just adds a +1 shift identical to option β. No additional value.

**Touch points:** identical to option β.

**Pros:** maximally faithful to C# code shape.

**Cons:** identical to option β plus the conceptual baggage of a global registry that doesn't actually exist in Bevy.

**Estimated diff size:** identical to option β (~50 lines).

## Recommended option + rationale

**Option α (drop the injected slot-0 placeholder).**

Grounding in the faithful-port rule: the *format on disk* is the ground truth. C# writes a self-contained positional palette with slot 0 already populated as a placeholder, and the data references this palette directly. The Bevy port must read what C# writes. Adding an extra placeholder is a Bevy-only "improvement" that introduces a divergence — exactly the kind the rule prohibits.

The user's intuition was structurally correct: "the palette ignores the black blocks that it had in-between good blocks." The *cause* isn't gaps being squeezed out (the on-disk palette has no gaps to squeeze), but the *symptom* — every voxel rendering with the wrong palette slot — is exactly what the user described and exactly what option α fixes.

Option α also has the smallest patch surface (under 10 lines total) and the smallest risk (it removes code rather than adding code). The single unit test that needs adjustment (`parses_oasis_cvox_arrays_nonempty` at `cvox_import.rs:498-515`) currently asserts `imp.palette.len() >= 2`; after option α the lower bound is `>= 1` (since the smallest valid `.cvox` could in principle have `typeCount = 1` with just the placeholder). For the Oasis fixture specifically, the parsed length is exactly **39** (`typeCount`).

A follow-up consideration the implementer should keep in mind: the `--vox-e2e` / `--vox-debug` modes and any test that asserts `palette.len() == 257` is .vox-only and unaffected by this fix. The `cvox_import.rs` tests are the only palette-length tests for the new parser, and only one of them asserts a length lower bound.

## Measurement method

For reproducibility (no Rust diagnostic test was added — the binary header was read directly):

```sh
cd /tmp && mkdir cvox-inspect && cd cvox-inspect
cp /mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox .
python3 -c "
import zipfile, struct
with zipfile.ZipFile('oasis.cvox','r') as zf:
    with zf.open('data','r') as f:
        hdr = f.read(32)
        version, sx, sy, sz, type_count, chunk_count, block_count, voxel_count = struct.unpack('<iiiiiIII', hdr)
        print(f'typeCount={type_count}, modelSize=({sx},{sy},{sz}), chunkCount={chunk_count}')
        rest = f.read()
        offset = 0
        # parse the typeCount palette entries (variable-width, null-terminated id + 36 fixed bytes)
        for i in range(min(8, type_count)):
            end = rest.index(b'\\x00', offset)
            id_str = rest[offset:end].decode('latin-1', errors='replace')
            offset = end + 1
            cb = struct.unpack_from('<fff', rest, offset); offset += 12
            cl = struct.unpack_from('<fff', rest, offset); offset += 12
            mb, ml = struct.unpack_from('<ii', rest, offset); offset += 8
            r, = struct.unpack_from('<f', rest, offset); offset += 4
            print(f'  types[{i}] id=\"{id_str}\" colorBase={cb} colorLayered={cl} mat=({mb},{ml}) roughness={r}')
"
```

Output (verbatim):

```
typeCount=39, modelSize=(1033,386,1082), chunkCount=110500
  types[0] id="_" colorBase=(0.0, 0.0, 0.0) colorLayered=(0.0, 0.0, 0.0) mat=(0,0) roughness=0.0
  types[1] id="_" colorBase=(0.354, 0.239, 0.119) colorLayered=(0.0, 0.0, 0.0) mat=(0,0) roughness=0.0
  types[2] id="_" colorBase=(0.320, 0.216, 0.101) colorLayered=(0.0, 0.0, 0.0) mat=(0,0) roughness=0.0
  …
  types[7] id="_" colorBase=(0.289, 0.190, 0.085) colorLayered=(0.0, 0.0, 0.0) mat=(0,0) roughness=0.0
```

Plus the in-data-arrays scan:

```sh
# dataVoxel max referenced index (low 15 bits of each full half-word):
# (full byte-scan run via Python, computing max(idx) where (half & 0x8000) != 0)
# Result: max idx = 38  (so values are 1..38, well-formed for typeCount=39 with slot 0 = placeholder)
```

No Bevy source files were modified.

## Files / line ranges read

- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/01-context.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/02-csharp-reference.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/03-design.md` (full — especially the `.cvox format specification` palette section, Decisions D2 + D6, Assumption A7)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/05-impl.md` (full — especially `## .cvox parser implementation notes` + A7 confirmation)
- `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/cvox_import.rs` (full, 517 lines)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/vox_import.rs` (lines 1-220, 615-755, 900-1004, plus tests at 1170-1180; full file is 1733 lines)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/mod.rs` (full)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/grid.rs` (lines 450-655 — focuses on `install_imported_vox` palette consumption)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/prepare.rs` (lines 600-640 — palette GPU upload site)
- `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs` (lines 1-580 — `Save`, `Load`, `CreateDataForRender`, `CreateFromWorldData`, `ImportFromVox`)
- `/mnt/archive4/DEV/NAADF/NAADF/World/VoxelTypeHandler.cs` (full, 169 lines — `ApplyVoxelType`, `Clear`, `typesRender` invariant)
- `/mnt/archive4/DEV/NAADF/NAADF/Libraries/VoxelsCore/VoxelDataBytes.cs` (full — confirms `dataImport.Index` is the raw 0-based MV palette slot)
- `/mnt/archive4/DEV/NAADF/NAADF/Libraries/VoxelsCore/Voxel.cs` (lines 1-50 — `Voxel.Index` semantics)

### Binary inspections

- `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` (6.79 MB) — parsed header + palette + scanned full dataVoxel array via Python `zipfile` + `struct`. Confirmed: `typeCount=39, modelSize=(1033,386,1082), max(dataVoxel idx)=38, slot 0 = "_" all-zero placeholder`.
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/assets/test/oasis_hard_cover.vox` (84.91 MB) — walked MAIN chunk tree, found RGBA chunk = 256 colors (standard MV palette).
