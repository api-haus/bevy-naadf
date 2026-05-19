# 03-design — .vox/.cvox magic-byte dispatch

## delegate-architect findings (2026-05-19)

## Summary

The Bevy port currently parses only MagicaVoxel `.vox` via `dot_vox` + a sparse
scene-graph walk that emits `ConstructedWorld { chunks, blocks, voxels }`. This
design adds:

1. A faithful Rust port of C# `ModelData.Load` (`/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:181-258`)
   that reads `.cvox` — a ZIP archive containing a single deflate-compressed
   entry `"data"` with a fixed-width little-endian binary header + three flat
   `u32` arrays (`dataChunk` / `dataBlock` / `dataVoxel`). The output lands as
   the same shape `install_imported_vox` already consumes (`ImportedVox` —
   `crate::voxel::vox_import::ImportedVox`).
2. A single dispatch entry point `parse_voxel_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxelParseError>`
   that peeks the first 4 bytes and routes to either `vox_import::parse_vox_bytes`
   or the new `cvox_import::parse_cvox_bytes`.
3. A refactor of every existing call site (`grid::parse_to_imported_vox`,
   `native_vox_drop_listener`, `async_vox::spawn_native_vox_parse{_from_bytes}`,
   `web_vox::spawn_wasm_vox_parse`, `web_vox::startup_fetch_default_vox` URL
   path, and the desktop drop-extension filter) to go through the dispatch.
4. A new `zip = "2"` workspace dep (pure-rust, wasm-compatible) used as the
   `.cvox` container reader, paired with `flate2` (already transitive).

Verification is unit-test only (no boot, no new e2e gate, no asset-change):
two new tests in `crates/bevy_naadf/src/voxel/cvox_import.rs` assert
`(1033, 386, 1082)` against the bytes of `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox`,
plus a magic-dispatch test in `crates/bevy_naadf/src/voxel/voxel_dispatch.rs`
that exercises both routing arms against existing in-tree fixtures.

---

## .cvox format specification (ported from C# ModelData.Load)

### Container layer

`/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:192-194`:

```csharp
using (var fileStream = File.Open(fileName, FileMode.Open))
using (var archive = new ZipArchive(fileStream, ZipArchiveMode.Read, false))
var entry = archive.GetEntry("data");
using (var zipStream = entry.Open())
```

`Save` uses `ZipArchiveMode.Create` with one `CreateEntry("data")` (line 137).
.NET's `ZipArchive` defaults to DEFLATE for `CreateEntry` without explicit
compression level. Verified by raw bytes — `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox`
starts with `50 4B 03 04` (`PK\x03\x04`, ZIP local file header) and lists exactly
one entry named `data` (header at offset 0x0E: name length = 4, name bytes
`64 61 74 61` = `"data"`).

**Reader choice:** `zip` crate v2 (pure-rust, no C deps, wasm-compatible) opens
the archive and yields a `Read`-impl on the single `"data"` entry. The `zip`
crate handles DEFLATE transparently via `flate2`.

### Header layout (uncompressed stream, little-endian, fixed-width)

Read sequence quoted from `ModelData.cs:199-209`:

```csharp
int version = zipStream.ReadInt();              // 4 bytes LE i32

int modelSizeX = zipStream.ReadInt();           // 4 bytes LE i32
int modelSizeY = zipStream.ReadInt();           // 4 bytes LE i32
int modelSizeZ = zipStream.ReadInt();           // 4 bytes LE i32
modelSize = new Point3(modelSizeX, modelSizeY, modelSizeZ);

int typeCount = zipStream.ReadInt();            // 4 bytes LE i32
chunkCount = zipStream.ReadUInt();              // 4 bytes LE u32
blockCount = zipStream.ReadUInt();              // 4 bytes LE u32
voxelCount = zipStream.ReadUInt();              // 4 bytes LE u32
```

Total fixed header: **32 bytes** (8 × 4-byte little-endian ints).

Underlying primitives (`/mnt/archive4/DEV/NAADF/NAADF/Common/Extensions/File/ExtFileRead.cs`):

- `ReadInt`: `BitConverter.ToInt32(buffer, 0)` — little-endian on .NET x64
  (line 30-34).
- `ReadUInt`: `BitConverter.ToUInt32(buffer, 0)` — little-endian (line 36-40).
- `ReadFloat`: `BitConverter.ToSingle(buffer, 0)` — little-endian f32 (line 42-46).
- `ReadNullTerminated`: read bytes until `0`, ASCII (line 14-22). **Caveat:**
  C# casts each byte to `(char)` (line 19) — this is Latin-1 / ISO-8859-1 for
  bytes < 128 == ASCII. We mirror as ASCII-only string read (any non-ASCII
  byte will be reproduced via `char::from_u32(b as u32)` to match the C#
  behaviour bit-for-bit).

### `VoxelType` palette entries (`typeCount` entries follow header)

`ModelData.cs:212-216`:

```csharp
types = new VoxelType[typeCount];
for (int i = 0; i < typeCount; ++i)
    types[i] = App.worldHandler.voxelTypeHandler.ApplyVoxelType(LoadVoxelType(zipStream));
```

`LoadVoxelType` at `ModelData.cs:61-72`:

```csharp
private static VoxelType LoadVoxelType(Stream stream) {
    VoxelType type = new VoxelType();
    string id = stream.ReadNullTerminated();             // ASCII null-terminated
    type.ID = id.StartsWith("_") ? null : id;            //
    type.colorBase = stream.ReadVector3();               // 3 × f32 LE = 12 bytes
    type.colorLayered = stream.ReadVector3();            // 3 × f32 LE = 12 bytes
    type.materialBase = (MaterialTypeBase)stream.ReadInt(); // 4 bytes LE i32
    type.materialLayer = (MaterialTypeLayer)stream.ReadInt();// 4 bytes LE i32
    type.roughness = stream.ReadFloat();                 // 4 bytes LE f32
    return type;
}
```

Per-entry size: variable (null-terminated string + null) + 36 bytes fixed.

Enum value mapping (verified from
`/mnt/archive4/DEV/NAADF/NAADF/World/VoxelTypeHandler.cs:14-27`):

- `MaterialTypeBase`: `Diffuse=0`, `Emissive=1`, `MetallicRough=2`,
  `MetallicMirror=3` — directly matches Bevy `crate::voxel::MaterialBase`
  (`voxel/mod.rs:91-97`).
- `MaterialTypeLayer`: `None=0`, `MetallicRough=2`, `MetallicMirror=3` (`1`
  intentionally absent) — directly matches Bevy `crate::voxel::MaterialLayer`
  (`voxel/mod.rs:103-108`).

Note: `ApplyVoxelType` at `VoxelTypeHandler.cs:73-86` registers the type into
a deduplicating dictionary and assigns a `renderIndex`. The Bevy port has no
global `VoxelTypeHandler`; the parsed palette is consumed directly by
`commands.insert_resource(VoxelTypes { types: imp.palette })`
(`grid.rs:646`). Faithful-port behaviour for `.cvox`: simply build a
`Vec<VoxelType>` of `palette.len() + 1` entries (index 0 = reserved empty
placeholder, same as `.vox` palette path at `vox_import.rs:967-969`).

### Voxel-data arrays (variable-width, raw u32 stream)

`ModelData.cs:218-240`:

```csharp
dataChunk = new uint[chunkCount];
Span<byte> dataAsBytes = MemoryMarshal.AsBytes(new Span<uint>(dataChunk));
zipStream.ReadExactly(dataAsBytes);              // chunkCount × 4 bytes LE

dataBlock = new uint[blockCount];
dataAsBytes = MemoryMarshal.AsBytes(new Span<uint>(dataBlock));
zipStream.ReadExactly(dataAsBytes);              // blockCount × 4 bytes LE

int voxelDataCount = (int)(voxelCount / 2);
dataVoxel = new uint[voxelDataCount];
if (voxelDataCount > 0x1FFF0000) {
    // Split read (workaround for some .NET 32-bit Span limit)
    dataAsBytes = MemoryMarshal.AsBytes(new Span<uint>(dataVoxel, 0, 0x1FFF0000));
    zipStream.ReadExactly(dataAsBytes);
    dataAsBytes = MemoryMarshal.AsBytes(new Span<uint>(dataVoxel, 0x1FFF0000, ...));
    zipStream.ReadExactly(dataAsBytes);
}
else {
    dataAsBytes = MemoryMarshal.AsBytes(new Span<uint>(dataVoxel));
    zipStream.ReadExactly(dataAsBytes);          // (voxelCount/2) × 4 bytes LE
}
```

`voxelCount` is the **total voxel count** but on-disk `dataVoxel.len()` is
`voxelCount / 2` (two voxel u16s packed per u32, as documented in
`aadf/generator.rs:71-73`). The 0x1FFF0000 split is .NET-specific and not
relevant to Rust (we can read in one `read_exact`).

The encoding of the read u32 arrays exactly matches the Bevy
`crate::aadf::generator::ModelData` fields (`generator.rs:65-85`):

- `data_chunk[c]` — top 2 bits = node type (0/1/2), low 30 bits = payload.
- `data_block[i]` — same encoding.
- `data_voxel[i]` — packs two voxel u16s; bit 15 = full flag, low 15 = type id.

**This is byte-identical to what the Bevy `.vox` install path constructs from
the scene-graph walk before stripping AADF bits from empty voxel half-words.**
For `.cvox` the stripping is a no-op (C# `ImportFromVox:442-446` already wrote
empties as literal `0`; `Save` round-tripped through `dataVoxel` without
adding AADF bits — AADF bits in the voxel layer are a Bevy-side artifact of
`build_constructed_world_sparse`, not a C#/`.cvox` encoding).

### Version migration (back-compat for version < 3)

`ModelData.cs:242-250`:

```csharp
if (version < 3) {
    for (int i = 0; i < blockCount; ++i) {
        uint curBlock = dataBlock[i];
        if ((curBlock >> 31) == 1)
            dataBlock[i] = ((curBlock & 0x3FFFFFFF) / 2) | (1u << 31);
    }
}
```

Mirror this branch verbatim. `oasis.cvox` is version 3 (per `02-csharp-reference.md`)
so this is dead code for our target asset, but the faithful-port rule
requires it.

### Post-load `sizeInChunks` derivation

`ModelData.cs:40` (constructor):

```csharp
this.sizeInChunks = new Point3(
    (modelSize.X + 15) / 16,
    (modelSize.Y + 15) / 16,
    (modelSize.Z + 15) / 16);
this.chunkCount = (uint)(sizeInChunks.X * sizeInChunks.Y * sizeInChunks.Z);
```

So `chunkCount` on the wire equals `ceil(X/16) * ceil(Y/16) * ceil(Z/16)`.
For Oasis (`1033, 386, 1082`): `65 * 25 * 68 = 110_500` matches the audit
(`02-csharp-reference.md:71-76`). The parser must compute `size_in_chunks` the
same way and may assert `chunk_count_on_disk == prod(size_in_chunks)` as a
sanity check (mirroring C#'s implicit invariant — failure means corrupt file).

### Endianness and gotchas

- All multi-byte integers are little-endian (.NET on x64 + ARM64). Use
  `u32::from_le_bytes` / `i32::from_le_bytes` / `f32::from_le_bytes`.
- `modelSize` is read as **signed `i32`** in C# but stored into `Point3` (also
  i32). Bevy port uses `u32` for size; convert with `as u32`. Negative size
  values would indicate a corrupt file — match C# by silently casting (don't
  add validation C# doesn't have).
- Null-terminated strings: read byte-by-byte until `0x00`, ASCII / Latin-1
  per `(char)b` cast. No length prefix.
- `voxel_count_on_disk / 2 = data_voxel.len()` — i.e. on-disk array has half
  the count. **Always divide by 2 before allocating.**
- The `name` and `curFilePath` fields C# computes (`ModelData.cs:44-45`) are
  derived from the input path and are not part of the on-disk format — skip
  in Bevy port (no equivalent fields needed; `source_label` is the Bevy port's
  carrier).

### Decompression library

The C# uses `ZipArchive` from `System.IO.Compression` which uses DEFLATE
internally. The Bevy port will use the `zip = "2"` crate which transitively
uses `flate2` (already in our `Cargo.lock` at v1.1.9 via image). No new
native deps; pure-rust + wasm-friendly.

---

## Module / file layout

### New files

1. **`crates/bevy_naadf/src/voxel/cvox_import.rs`** (NEW, ~280 lines)
   - `pub struct CvoxImportError` (mirrors `VoxImportError` shape).
   - `pub fn parse_cvox_bytes(bytes: &[u8]) -> Result<ImportedVox, CvoxImportError>`.
   - Internal helpers: `read_zip_data_entry`, `read_header`, `read_voxel_type`,
     `read_null_terminated_ascii`, `apply_version_migration`.
   - Output: same `ImportedVox` type from `vox_import.rs:112-124`. The
     `ConstructedWorld` is populated as `chunks = data_chunk`,
     `blocks = data_block`, `voxels = data_voxel`, `size_in_chunks` computed
     from `modelSize`. The `palette: Vec<VoxelType>` carries `palette.len() + 1`
     entries (slot 0 = `VoxelType::default()`, slots 1..=N = parsed palette,
     matching the `.vox` path's index-shift convention at `vox_import.rs:967-969`).
   - Three unit tests (see Verification plan).

2. **`crates/bevy_naadf/src/voxel/voxel_dispatch.rs`** (NEW, ~120 lines)
   - `pub enum VoxelFormat { DotVox, Cvox }`.
   - `pub fn detect_format(bytes: &[u8]) -> Option<VoxelFormat>` — peek first
     4 bytes, return `Some(DotVox)` for `b"VOX "`, `Some(Cvox)` for
     `b"PK\x03\x04"`, `None` otherwise.
   - `pub enum VoxelParseError { Dispatch(String), Vox(VoxImportError), Cvox(CvoxImportError) }`
     (thiserror).
   - `pub fn parse_voxel_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxelParseError>`
     — the dispatch entry point.
   - One unit test asserting both routing arms.

### Modified files

1. **`crates/bevy_naadf/src/voxel/mod.rs:12-14`** — add module declarations:
   ```rust
   pub mod cvox_import;
   pub mod voxel_dispatch;
   ```
   (existing `pub mod async_vox; pub mod grid; pub mod vox_import;` stays).

2. **`crates/bevy_naadf/src/voxel/grid.rs:502-505`** — `parse_to_imported_vox`
   becomes the dispatch shim. **Replace** the body:
   ```rust
   pub fn parse_to_imported_vox(bytes: &[u8]) -> Result<vox_import::ImportedVox, String> {
       crate::voxel::voxel_dispatch::parse_voxel_bytes(bytes).map_err(|e| e.to_string())
   }
   ```
   No other change to grid.rs is required — `install_vox_bytes_in_fixed_world`
   already calls `parse_to_imported_vox`, so it inherits dispatch for free.

3. **`crates/bevy_naadf/src/voxel/grid.rs:691-702`** — `native_vox_drop_listener`
   extension filter. **Replace** the `.vox`-only ext check with a
   `.vox || .cvox` filter:
   ```rust
   let ext = path_buf.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
   let is_voxel = matches!(ext.as_deref(), Some("vox") | Some("cvox"));
   if !is_voxel {
       info!("drag-drop: ignoring non-voxel file ({})", path_buf.display());
       continue;
   }
   ```
   The downstream `spawn_native_vox_parse` doesn't care about extension — it
   reads bytes + dispatches. **No other changes needed.**

   Update the `cfg(not(target_arch = "wasm32"))` doc-comment at lines 649-667
   ("filters to `.vox` files") to mention `.cvox` too.

4. **`crates/bevy_naadf/src/voxel/web_vox.rs`** — the web DnD listener filters
   by file extension only via the browser's filename. **No code change is
   strictly required** — `web_vox.rs:234-265`'s `drop` closure does NOT filter
   by extension; it just submits the bytes to `submit_pending_bytes` which
   eventually calls `parse_to_imported_vox` → dispatch. The browser DnD will
   route any dropped file through; the magic-byte dispatch decides the format.

   The startup-fetch URL resolver (`resolve_startup_vox_url`) targets
   `R2_DEFAULT_VOX_URL` — confirm the URL still serves `.vox`; if the user
   wants `oasis.cvox` to be the web default, update this URL separately
   (out-of-scope here per the brief: the user said "extend drag&drop and
   autoload", autoload on web is the URL fetch, but it already routes through
   `parse_to_imported_vox` which now dispatches by magic).

5. **`crates/bevy_naadf/src/voxel/vox_import.rs:154-157`** — `parse_vox_bytes`
   stays as-is (still the format-specific entry; consumed by the dispatch
   module). No change.

6. **`crates/bevy_naadf/src/main.rs:38-46`** — CLI flag `--vox`. **Keep the
   flag name** (justified in Decisions). Update the help-text doc-comment at
   `main.rs:21-24` to say "MagicaVoxel `.vox` or NAADF `.cvox` file". No
   logic change needed — the flag already routes to `GridPreset::Vox { path }`
   → `install_vox_in_fixed_world` → `parse_to_imported_vox` → dispatch.

7. **`crates/bevy_naadf/src/lib.rs:74-80`** — `GridPreset::Vox` doc-comment.
   Update to mention both formats. No code change to the enum variant name
   (kept as `Vox` for source-stability; renaming to `Voxel` would cascade
   through ~16 files — see Decisions).

8. **`crates/bevy_naadf/Cargo.toml`** — add `zip` dep (line ~89, after
   `thiserror`):
   ```toml
   # zip = ZIP archive reader for the NAADF `.cvox` format (a ZIP-wrapped
   # custom binary; see voxel/cvox_import.rs). Pure-rust, no native deps;
   # wasm-compatible. Used only by the .cvox parse path.
   zip = { version = "2", default-features = false, features = ["deflate"] }
   ```
   `default-features = false` drops `aes`/`bzip2`/`zstd`/`time` (we only need
   `deflate`, the storage method `ZipArchive.CreateEntry` produces).

### Untouched (verified)

- `crates/bevy_naadf/src/voxel/async_vox.rs:165-207` — both
  `spawn_native_vox_parse` and `spawn_native_vox_parse_from_bytes` call
  `parse_to_imported_vox(&bytes)` which is the dispatch shim. **No change.**
- `crates/bevy_naadf/src/voxel/web_vox.rs:437-448` — `spawn_wasm_vox_parse`
  calls `crate::voxel::grid::parse_to_imported_vox(&bytes)`. **No change.**
- `crates/bevy_naadf/src/voxel/grid.rs:463-478` —
  `install_vox_bytes_in_fixed_world` calls `parse_to_imported_vox`. **No change.**
- `crates/bevy_naadf/src/voxel/grid.rs:521-647` — `install_imported_vox` is
  format-agnostic (consumes `ImportedVox`). **No change** — the empty-voxel
  AADF strip at `grid.rs:574-585` is a no-op for `.cvox` (already clean)
  and remains correct for `.vox`.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — every `--vox-*` mode either
  builds an in-memory `.vox` (via `dot_vox::DotVoxData`) or points at
  `oasis_hard_cover.vox`. None of these touch `.cvox` and none need to change.
- `crates/bevy_naadf/src/aadf/generator.rs:75-85` — `ModelData` struct is the
  destination type; both formats land into it. **No change.**

---

## Magic-byte dispatch entry point

**Module:** `crates/bevy_naadf/src/voxel/voxel_dispatch.rs`

### Magic table

| Format | First 4 bytes | Source-of-truth |
|---|---|---|
| MagicaVoxel `.vox` | `0x56 0x4F 0x58 0x20` (`"VOX "`) | `dot_vox-5.2.0/src/parser.rs:23` `const MAGIC_NUMBER: &str = "VOX "` + verified by hexdump of `crates/bevy_naadf/assets/test/oasis_hard_cover.vox`. |
| NAADF `.cvox` | `0x50 0x4B 0x03 0x04` (`"PK\x03\x04"`) | ZIP local file header. Verified by hexdump of `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` (`504b 0304 1400 ...`). |

### Function signature

```rust
use crate::voxel::vox_import::ImportedVox;
use crate::voxel::cvox_import::CvoxImportError;
use crate::voxel::vox_import::VoxImportError;

pub enum VoxelFormat { DotVox, Cvox }

#[derive(Debug, thiserror::Error)]
pub enum VoxelParseError {
    #[error("voxel file too short for magic-byte check ({0} bytes)")]
    TooShort(usize),
    #[error("unrecognised voxel-file magic bytes: {magic:02x?}")]
    UnknownMagic { magic: [u8; 4] },
    #[error(transparent)]
    Vox(#[from] VoxImportError),
    #[error(transparent)]
    Cvox(#[from] CvoxImportError),
}

pub fn detect_format(bytes: &[u8]) -> Option<VoxelFormat> {
    if bytes.len() < 4 { return None; }
    match &bytes[..4] {
        b"VOX " => Some(VoxelFormat::DotVox),
        [0x50, 0x4B, 0x03, 0x04] => Some(VoxelFormat::Cvox),
        _ => None,
    }
}

pub fn parse_voxel_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxelParseError> {
    match detect_format(bytes) {
        Some(VoxelFormat::DotVox) => Ok(crate::voxel::vox_import::parse_vox_bytes(bytes)?),
        Some(VoxelFormat::Cvox)   => Ok(crate::voxel::cvox_import::parse_cvox_bytes(bytes)?),
        None if bytes.len() < 4 => Err(VoxelParseError::TooShort(bytes.len())),
        None => {
            let mut magic = [0u8; 4];
            magic.copy_from_slice(&bytes[..4]);
            Err(VoxelParseError::UnknownMagic { magic })
        }
    }
}
```

`detect_format` is intentionally `pub` so debugging code / future tooling can
sniff a file's format without parsing.

### Dispatch logic

The dispatcher does NO format-specific work beyond peeking. The two parsers
each produce `ImportedVox`; the install path consumes that shape unchanged.

---

## Call-site refactor

| Site | File:line | Current state | Proposed change |
|---|---|---|---|
| 1. `parse_to_imported_vox` (the shared parse shim) | `voxel/grid.rs:502-505` | Calls `dot_vox::load_bytes` + `vox_import::parse_dot_vox_data`. | **Replace body** with `crate::voxel::voxel_dispatch::parse_voxel_bytes(bytes).map_err(|e| e.to_string())`. Keeps the `String`-error shim so all callers stay compatible. |
| 2. Native desktop drag-and-drop | `voxel/grid.rs:691-702` | `is_vox` check on the `vox` extension only. | **Broaden** to accept `vox` and `cvox` (case-insensitive). Body unchanged — `spawn_native_vox_parse` already reads bytes + dispatches. |
| 3. `native_vox_drop_listener` doc | `voxel/grid.rs:649-667` | Says "filters to `.vox` files". | **Update wording** to say "voxel files (`.vox`, `.cvox`)". |
| 4. Web drag-and-drop | `voxel/web_vox.rs:234-265` | No extension filter — the closure ingests anything dropped. | **No change.** The browser-side DnD pipes raw bytes into `submit_pending_bytes` → `parse_to_imported_vox` → dispatch. |
| 5. Web startup HTTP fetch | `voxel/web_vox.rs:284-338` | Fetches a `.vox` URL, bytes → `submit_pending_bytes`. | **No change** to the fetch shape. The dispatch handles whatever bytes come back. The URL resolver (`resolve_startup_vox_url`) still serves a `.vox` — changing the default web asset to `.cvox` is a separate decision and the brief did not require it. |
| 6. Native async parse | `voxel/async_vox.rs:165-207` | Both `spawn_native_vox_parse{_from_bytes}` call `parse_to_imported_vox`. | **No change.** Inherits dispatch via the shim. |
| 7. Wasm async parse | `voxel/web_vox.rs:437-448` | `spawn_wasm_vox_parse` calls `crate::voxel::grid::parse_to_imported_vox`. | **No change.** Inherits dispatch. |
| 8. CLI `--vox <path>` | `main.rs:38-46` | Builds `GridPreset::Vox { path }` from any path. | **No logic change.** Update doc-comment at lines 21-24 to mention both formats. |
| 9. `setup_test_grid` arm | `voxel/grid.rs:122-138` | Branches on `GridPreset::Vox { path }` → `install_vox_in_fixed_world(path)`. | **No code change.** The downstream path now dispatches by magic. |
| 10. `install_vox_in_fixed_world` | `voxel/grid.rs:422-436` | Reads bytes from disk + calls `install_vox_bytes_in_fixed_world`. | **No code change.** Path-extension agnostic; magic dispatch does the routing. |
| 11. `install_vox_bytes_in_fixed_world` | `voxel/grid.rs:463-478` | Calls `parse_to_imported_vox` (now dispatch). | **No code change.** |
| 12. `install_imported_vox` | `voxel/grid.rs:521-647` | Consumes `ImportedVox` agnostic to source format. | **No change.** The AADF-strip at lines 574-585 is a no-op for `.cvox` (which has no AADF bits in voxel half-words) and remains correct for `.vox`. |
| 13. e2e_render `--vox-e2e` etc. | `bin/e2e_render.rs:114-130, 350-372` | All e2e modes build/load `.vox` content. | **No change.** None of them touch `.cvox`. |
| 14. `GridPreset::Vox` variant name | `lib.rs:74-80` | Named `Vox` for historical reasons (Track A added it). | **Keep the name** (renaming triggers ~16-file ripple — see Decisions). Update doc-comment to mention `.cvox`. |
| 15. `Cargo.toml` | `crates/bevy_naadf/Cargo.toml:34-89` | No `zip` dep. | **Add** `zip = { version = "2", default-features = false, features = ["deflate"] }`. |
| 16. `voxel/mod.rs` | `voxel/mod.rs:12-14` | Lists existing modules. | **Add** `pub mod cvox_import;` and `pub mod voxel_dispatch;`. |

---

## Verification plan

All verification is unit-test only, per the brief. No new e2e gate. No
booting `cargo run --bin bevy-naadf`. The user does the live visual check.

### Test 1 — `.cvox` parser parses real oasis.cvox

**Location:** new `#[cfg(test)] mod tests { ... }` block at the bottom of
`crates/bevy_naadf/src/voxel/cvox_import.rs`.

```rust
#[test]
fn parses_oasis_cvox_header_dims() {
    // Reads the real reference file from the C# project. Only the parsed
    // model dims are asserted — the load-bearing fact for the user's
    // "4 modulo-wrapped instances" claim.
    let path = "/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox";
    let bytes = std::fs::read(path).expect("oasis.cvox absent from reference path");
    let imp = parse_cvox_bytes(&bytes).expect("parse_cvox_bytes failed");
    // ceil(1033/16)=65, ceil(386/16)=25, ceil(1082/16)=68
    assert_eq!(imp.world.size_in_chunks, [65, 25, 68],
        "Oasis cvox chunk-dims mismatch");
    // Chunk count consistency check (mirrors the C# invariant
    // ModelData.cs:41).
    assert_eq!(imp.world.chunks.len(), 65 * 25 * 68);
}
```

If the reference file is absent (e.g. CI runners without the NAADF clone),
the test should `#[ignore]` itself rather than panic — wrap in
`if !std::path::Path::new(path).exists() { eprintln!("skipping (no NAADF reference)"); return; }`.

### Test 2 — `.cvox` palette and arrays present

```rust
#[test]
fn parses_oasis_cvox_arrays_nonempty() {
    let path = "/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox";
    if !std::path::Path::new(path).exists() { return; }
    let bytes = std::fs::read(path).unwrap();
    let imp = parse_cvox_bytes(&bytes).unwrap();
    assert!(!imp.world.blocks.is_empty(), "data_block empty");
    assert!(!imp.world.voxels.is_empty(), "data_voxel empty");
    // Slot 0 = reserved empty placeholder (Bevy convention; vox_import.rs:967-969).
    assert!(imp.palette.len() >= 2, "palette should have at least 1 real entry");
}
```

### Test 3 — Magic dispatch routes correctly

**Location:** new `#[cfg(test)] mod tests { ... }` block at the bottom of
`crates/bevy_naadf/src/voxel/voxel_dispatch.rs`.

```rust
#[test]
fn dispatch_routes_vox_to_dot_vox_parser() {
    let path = "crates/bevy_naadf/assets/test/oasis_hard_cover.vox";
    let bytes = std::fs::read(path).expect("test fixture absent");
    assert_eq!(detect_format(&bytes), Some(VoxelFormat::DotVox));
    parse_voxel_bytes(&bytes).expect("dispatch + .vox parse should succeed");
}

#[test]
fn dispatch_routes_cvox_to_cvox_parser() {
    let path = "/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox";
    if !std::path::Path::new(path).exists() { return; }
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(detect_format(&bytes), Some(VoxelFormat::Cvox));
    let imp = parse_voxel_bytes(&bytes).expect("dispatch + .cvox parse should succeed");
    assert_eq!(imp.world.size_in_chunks, [65, 25, 68]);
}

#[test]
fn dispatch_rejects_unknown_magic() {
    let junk = b"GIF89a..."; // wrong magic
    assert!(matches!(detect_format(junk), None));
    assert!(matches!(parse_voxel_bytes(junk), Err(VoxelParseError::UnknownMagic { .. })));
}

#[test]
fn dispatch_rejects_truncated_input() {
    let short = b"VO"; // too short for magic check
    assert!(matches!(detect_format(short), None));
    assert!(matches!(parse_voxel_bytes(short), Err(VoxelParseError::TooShort(2))));
}
```

### Verification commands (from project rules)

```
cargo build --workspace          # proves it compiles
cargo test --workspace --lib     # runs all unit tests, including the four new ones
```

No `e2e_render` invocation needed. No `cargo run --bin bevy-naadf`.

The user does the live visual check by copying
`/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` into
`crates/bevy_naadf/assets/test/` (or anywhere convenient) and running
`cargo run --release --bin bevy-naadf -- --vox <path-to-oasis.cvox>`. The
expected result is 4 modulo-wrapped Oasis instances (per `02-csharp-reference.md`).

---

## Decisions & rejected alternatives

### D1. Decompression library: `zip` crate vs hand-rolled `flate2`

**Chosen:** `zip = { version = "2", default-features = false, features = ["deflate"] }`.
**Rejected:** hand-parse the ZIP local file header + decompress with `flate2`
directly (a ~30-line shortcut since the `.cvox` has a fixed one-entry layout).

**Why chosen:** Faithful-port rule prefers using a real ZIP reader (parity with
C# `System.IO.Compression.ZipArchive`). The `zip` crate handles edge cases
(non-default compression levels, padding, CRC validation) that hand-rolled code
would silently get wrong. With `default-features = false` it's a thin pure-rust
shell over `flate2` (already transitive). One new dep, well-maintained,
wasm-compatible.

**What would flip the call:** If `zip` 2.x has a known wasm32 build issue
(check at impl time — `cargo check --target wasm32-unknown-unknown` is the
gate), the hand-rolled fallback is acceptable. Worst case: write
`fn read_zip_data_entry(bytes: &[u8]) -> Result<Vec<u8>, _>` that locates the
local file header magic (`PK\x03\x04`), reads the 30-byte fixed header to find
name length + extra-field length + compressed size, then DEFLATE-decompresses
with `flate2::read::DeflateDecoder`.

### D2. Output type: same `ImportedVox` vs new `ImportedCvox`

**Chosen:** Reuse `crate::voxel::vox_import::ImportedVox` as the output type
for both parsers.
**Rejected:** Separate `ImportedCvox { world, palette, ... extras }` requiring
a `From<ImportedCvox> for ImportedVox` conversion.

**Why chosen:** `install_imported_vox` already consumes `ImportedVox`
agnostically. The `.cvox` format has no fields `.vox` lacks that the install
path uses — both reduce to `(ConstructedWorld, Vec<VoxelType>)`. A second
output type would force the dispatch to either return a sum type
(needing conversion at every install site) or duplicate the install path.
SSoT rule wins.

**What would flip the call:** If `.cvox` carries metadata the install path
should expose downstream (e.g. a saved camera pose, a named author string),
add it to `ImportedVox` as `Option<T>` rather than fork the type. The current
format spec has no such metadata.

### D3. Dispatch lives in a new module (`voxel_dispatch.rs`) vs in `grid.rs` vs in `vox_import.rs`

**Chosen:** New module `crates/bevy_naadf/src/voxel/voxel_dispatch.rs`.
**Rejected A:** Put `parse_voxel_bytes` directly into `voxel/grid.rs`
(alongside `parse_to_imported_vox`).
**Rejected B:** Add a `parse_dispatched_bytes` function to `voxel/vox_import.rs`
that calls into `cvox_import`.

**Why chosen:** `voxel_dispatch.rs` is the only file that owns BOTH parsers'
imports and is therefore the cleanest spot for the union error type
(`VoxelParseError`). Putting dispatch in `grid.rs` adds a parsing concern to
the Bevy-resource install module; putting it in `vox_import.rs` is wrong-way
(the `.vox` module would import `.cvox` — circular-feeling). The dispatcher
module is small (~120 lines), low-traffic, and gives the testing surface a
natural home.

**What would flip the call:** If the dispatch logic ever grows beyond
"peek + route" (e.g. format auto-detection by content scanning, format
conversion), it would stay in its own module — the current decision is already
the right one for any direction. The case to fold it into `grid.rs` is if the
codebase moves toward fewer-modules-per-concern; not currently the trend.

### D4. CLI variant naming: `--vox` and `GridPreset::Vox` retained vs renamed

**Chosen:** Keep `--vox` flag and `GridPreset::Vox` variant name as-is.
**Rejected:** Rename to `--voxel` / `GridPreset::Voxel` to reflect the broader
format support.

**Why chosen:**
1. The `--vox` flag is a user-facing string in the just dev-loop. Changing
   it breaks every developer's muscle memory. The brief asked to "extend …
   to support both based on parsed header magic" — extension implies retention.
2. `GridPreset::Vox` rename triggers a 16+ file ripple: `lib.rs:69-80`,
   `main.rs:40`, `grid.rs:126`, `bin/e2e_render.rs:121-123`, every e2e harness
   that constructs `AppArgs` (~10 files), all the orchestration docs that
   reference the name. The cost is enormous; the value is purely cosmetic.
3. The faithful-port rule says "no Bevy-only improvements" — renaming for
   aesthetic clarity is exactly the kind of "improvement" the rule prohibits.
4. The doc-comment update at `lib.rs:74-80` + `main.rs:21-24` is sufficient
   to surface the broader format support to humans reading the code.

**What would flip the call:** If we ever do a broader CLI/UX refactor and
batch a bunch of renames together. Until then: keep the names.

### D5. Web-default URL: keep `.vox` or switch to `.cvox`

**Chosen:** Keep whatever URL `resolve_startup_vox_url` currently resolves to
(a `.vox`).
**Rejected:** Switch the web default to `.cvox` for parity with C# (which
loads `oasis.cvox`).

**Why chosen:** Out of scope per the brief — the brief said "extend drag&drop
and autoload functionality" but the user's verbatim directive doesn't include
"switch the default served asset". Switching default web asset is a separate
operational concern (re-upload to R2, regenerate the URL config). The user
can drag-drop `.cvox` into the web build today (after this design lands) to
prove the parity locally; whether to make `.cvox` the default web served
asset is a follow-up decision.

**What would flip the call:** Explicit user directive to switch the default.
The dispatch path supports it now; only the URL/asset config needs touching.

### D6. AADF-strip behaviour in `install_imported_vox` for `.cvox` input

**Chosen:** Leave the AADF-strip at `grid.rs:574-585` unchanged.
**Rejected A:** Add a format-aware branch that skips the strip for `.cvox`
(since `.cvox` empties are already `0`).
**Rejected B:** Move the strip from the install path into `vox_import.rs`'s
ImportedVox construction.

**Why chosen:** The strip is `if (lo & 0x8000) != 0 { lo } else { 0 }` —
i.e. "if voxel is full keep it, else zero it". For `.cvox` data the empty
voxels are already `0`, so the strip is a no-op (`(0 & 0x8000) == 0`,
`else 0` → `0`). For `.vox` data the strip is essential (per the existing
comment at `grid.rs:564-573`). Keeping it unconditionally means **the install
path stays format-agnostic**, which is the SSoT goal.

**What would flip the call:** If profiling shows the strip pass is a
measurable hot-spot on `.cvox` loads (it iterates `data_voxel.len()` `u32`s —
~50M for Oasis-class — so on the order of tens of milliseconds). It's a
one-time startup cost so almost certainly invisible. Don't optimise.

### D7. Test fixture path: hardcoded vs CI-aware

**Chosen:** Hardcoded path `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox`
with a `if !exists { return; }` graceful skip.
**Rejected:** Copy `oasis.cvox` into `crates/bevy_naadf/assets/test/` so the
test is hermetic.

**Why chosen:** The user has the NAADF reference at that path (it's the
canonical home). Copying the file (~85 MB, depending on compression)
into the Bevy repo's `assets/test/` would bloat the repo. The graceful-skip
keeps CI happy when running outside the user's environment.

**What would flip the call:** If CI starts running `cargo test` and the
gracefully-skipped test masks a real regression, copy the fixture (or a
smaller .cvox synthesised at test time — but synthesising a valid `.cvox`
means writing a `.cvox` encoder, which is out of scope). Alternative: rely
on the dispatch + magic-byte tests (which use the in-tree `.vox` and
synthesised junk-bytes) as the load-bearing assertion, and treat the
real-`.cvox` tests as opportunistic.

### D8. `version < 3` migration branch

**Chosen:** Port verbatim (mirror `ModelData.cs:242-250`).
**Rejected:** Drop the migration branch since `oasis.cvox` is version 3 and
the user is unlikely to encounter older files.

**Why chosen:** Faithful-port rule. Even though the user's immediate target
asset is version 3, dropping the branch creates a Bevy/C# divergence that
would silently miscompose any v1/v2 `.cvox` a user happens to feed. Tens of
lines, no perf cost.

**What would flip the call:** Nothing reasonable — the cost is trivial.

### D9. Error type: `thiserror::Error` enum vs `String`

**Chosen:** `CvoxImportError` is a `thiserror` enum (mirrors
`VoxImportError` at `vox_import.rs:127-149`).
**Rejected:** Return a `String` directly.

**Why chosen:** Symmetry with `VoxImportError`. Enables `?`-style chaining in
the dispatch's `VoxelParseError`. The `parse_to_imported_vox` shim still
flattens to `String` for downstream compatibility (`grid.rs:502` signature
unchanged).

**What would flip the call:** Nothing — strict improvement on `String`.

---

## Assumptions made

### A1. `.cvox` files only use DEFLATE compression

**Why:** C# `ZipArchive` with `ZipArchiveMode.Create` defaults to DEFLATE for
`CreateEntry` (`ModelData.cs:137`). No code path sets a different compression
level.
**Invalidation:** If anyone ever produces a `.cvox` with a non-DEFLATE
compression method (e.g. STORED for tiny files, or external tooling that
uses different codecs), the `zip` crate config `features = ["deflate"]`
would fail to read it. Easy to fix: add `"bzip2"` or `"deflate-flate2"` to
features.

### A2. The single ZIP entry is always named `"data"`

**Why:** Both `ModelData.cs:137` (Save: `CreateEntry("data")`) and
`ModelData.cs:196` (Load: `GetEntry("data")`) hardcode the name. No other
producer of `.cvox` exists in the C# codebase.
**Invalidation:** A future C# refactor that renames the entry would break the
Rust parser. Add a fallback: if `GetEntry("data")` fails, iterate entries and
take the first one. (Not adding this in v1 — faithful port.)

### A3. `voxelCount` on the wire is even (so `voxelCount / 2 = u32 count`)

**Why:** C# `Save` writes `voxelCount` then writes `dataVoxel.Length` u32s
where `dataVoxel.Length == voxelCount / 2` — and the C# encoding always packs
two voxels per u32 (`ModelData.cs:444-446` puts 32 u32s into a 64-voxel
block). If `voxelCount` were odd the trailing half-word would be undefined.
**Invalidation:** A corrupt or hand-modified file with odd `voxelCount`.
Detect by asserting `voxelCount % 2 == 0` at parse time (C# doesn't, but the
extra bound check is cheap and won't change behaviour for valid files).

### A4. `Point3` is `i32`-backed (signed 32-bit per axis)

**Why:** `Point3` is from `Microsoft.Xna.Framework` — XNA's `Point3` is
three signed int32s. The reader calls `ReadInt` (signed) for all three
`modelSize` components.
**Invalidation:** Models with axis > 2^31 — practically impossible
(max texture limit is 1024 chunks = 16K voxels). The Bevy port casts `i32 as u32`
and accepts only non-negative values; corrupt negative values get silently
re-interpreted as huge positives (same as C#).

### A5. Null-terminated strings are ASCII / Latin-1, never UTF-8

**Why:** `ExtFileRead.cs:14-22` casts each byte to `(char)` (single byte →
single .NET `char`, which is UTF-16 code unit). This is Latin-1 — bytes 0x80+
become U+0080..U+00FF. UTF-8 multi-byte sequences would be silently mangled.
**Invalidation:** No `.cvox` file has been observed with non-ASCII type IDs;
the C# `VoxelType.ID` is typically `"_"`-prefixed identifiers. The Bevy
parser uses `core::str::from_utf8` *fallback* — for any byte > 0x7F it
constructs a `char` via `char::from_u32(b as u32).unwrap()`, matching the C#
Latin-1 cast bit-for-bit.

### A6. The reference file at `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` is version 3

**Why:** `02-csharp-reference.md:60-66` reports "format version 3" for that
file from binary header inspection.
**Invalidation:** If the file is actually v1/v2, the unit test still passes
(the migration branch is executed). If header `version > 3` exists, no
migration is needed (the C# code has no upper-version check, so neither does
the Bevy port).

### A7. The C# `LoadVoxelType` → `ApplyVoxelType` round-trip does not depend on the global type registry

**Why:** C# `ApplyVoxelType` (`VoxelTypeHandler.cs:73-86`) registers the type
into a dictionary keyed by ID + assigns a `renderIndex`. The Bevy port has no
such global registry — `VoxelTypes::types: Vec<VoxelType>` is per-world,
indexed positionally (slot 0 = empty, slot i+1 = palette[i]). For `.cvox`,
the on-disk palette entries map 1:1 to Bevy palette slots (i+1 = on-disk i).
**Invalidation:** If the C# code emits `dataChunk` / `dataBlock` / `dataVoxel`
entries that reference type IDs by `renderIndex` (post-registry remapping)
rather than positionally, the Bevy port's positional mapping would mis-index.
Reading `ModelData.cs:74-108` (`CreateDataForRender`), the entries DO get
remapped through `types[curChunk & 0x3FFFFFFF].renderIndex` at runtime —
but this is GPU-prep, not on-disk encoding. Crucially, `Save` (line 158-173)
writes `dataChunk` / `dataBlock` / `dataVoxel` BEFORE `CreateDataForRender`
would have remapped them (these are runtime structures that exist in-memory).
The on-disk encoding uses the **positional palette index** (the loop variable
`i` in `ImportFromVox:506-521`). So Bevy's positional 1:1 mapping is correct.

### A8. The `voxel_count / 2 > 0x1FFF_0000` split branch is .NET-specific

**Why:** The 0x1FFF0000 limit is the .NET `Span<T>` size cap on 32-bit
targets — Rust has no such limit. A single `read_exact(buf)` handles any
buffer size up to `isize::MAX`. The branch is functionally equivalent for our
Rust port collapsed to one read.
**Invalidation:** None — both produce identical byte streams.

### A9. Drag-and-drop on web doesn't need extension filtering at the JS layer

**Why:** `voxel/web_vox.rs:234-265` reads ANY dropped file's `arrayBuffer()`
and pipes the bytes through the dispatch (post-design). The magic-byte check
in `parse_voxel_bytes` rejects non-voxel files cleanly with
`VoxelParseError::UnknownMagic`. Adding a JS-side extension filter would
duplicate the dispatch logic (SSoT violation).
**Invalidation:** If the UX needs to give users feedback BEFORE the parse
("this isn't a voxel file"), a UI message in the drop handler is acceptable —
but the load-bearing decision (route vs reject) remains the magic-byte check.

### A10. `zip` crate v2 builds cleanly on wasm32

**Why:** Stated in the crate description (mature, pure-rust). Not verified by
this design; the implementer should run
`cargo check -p bevy-naadf --target wasm32-unknown-unknown` after adding the
dep to confirm.
**Invalidation:** If `zip` 2.x pulls in `time` or `iana-time-zone` (it does
have a `time` feature gated off by `default-features = false`), the wasm
build may fail. Mitigation in this design: `default-features = false,
features = ["deflate"]` opts in only the minimal set. If it still fails,
fall back to the hand-rolled local-file-header parser + `flate2` (Decision D1
"What would flip the call").

### A11. The audit's reported chunk count (110,500) is the on-disk `chunkCount` field

**Why:** Audit at `02-csharp-reference.md:66` says "chunk count: 110,500"
read from binary header inspection. This matches `65 * 25 * 68` derived from
`(modelSize + 15) / 16` per `ModelData.cs:40`.
**Invalidation:** If the audit read a different field (e.g. `blockCount` or
some sub-count), Test 1's `imp.world.chunks.len() == 65*25*68` assertion
still holds because Bevy derives `chunks.len()` from `prod(size_in_chunks)`,
not from the on-disk count. Test 1 would still catch a real mismatch.

---

## Out of scope

Items explicitly NOT designed in this phase, per the brief:

1. **Issue #2** — grazing-angle ray termination limiting view distance. A
   separate `/delegate` invocation.
2. **Camera-pose 1024-vs-4096 scaling fix** — the `1024`-base normalization
   in `camera/mod.rs:28` referenced by `02-csharp-reference.md:199-201`. A
   separate `/delegate` at the user's discretion.
3. **New e2e gates** — the user chose "user-eyes only" verification (Q2 in
   `01-context.md:60`). The unit tests in this design are the verification
   surface; no `e2e_render` mode is added.
4. **SSoT refactor of world-size constants** — already verified not needed
   per `00-reuse-audit.md:137-142`.
5. **Switching the default web-served voxel asset to `.cvox`** — out of scope
   per Decision D5. The dispatch supports it; only the R2-URL config would
   need touching, which is an operational decision.
6. **A `.cvox` writer / encoder** — only the reader (`Load`) is in scope.
   `Save` is not ported. If future work needs a Bevy → `.cvox` export, that's
   a separate design.
7. **Asset hot-reload via Bevy `AssetLoader`** — the existing `.vox` path
   doesn't use `AssetLoader` (per `vox_import.rs:39`), and adding it here
   would expand scope. Synchronous `std::fs::read` + parse stays.
8. **Multi-model `.cvox` support** — the C# format spec stores a single model
   per file (no scene graph). Same in Bevy port.
9. **`.vl32` / `.obj`-via-`obj2voxel` import** — these C# code paths
   (`ModelData.cs:356, 587, 764`) are out of scope (already noted in
   `vox_import.rs:36-37`).
10. **Renaming `--vox` / `GridPreset::Vox`** — per Decision D4, the names
    stay despite the broader format support.

---

## Files / line ranges read

**Required by brief, read in full or substantially:**

- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/01-context.md` (full, 75 lines)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/00-reuse-audit.md` (full, 175 lines)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/02-csharp-reference.md` (full, 239 lines)
- `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md` (full, 44 lines)
- `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs` (full, 850 lines — load-bearing range 181-258, plus 50-72 LoadVoxelType / 126-179 Save / 33-48 ctor)
- `/mnt/archive4/DEV/NAADF/NAADF/Common/Extensions/File/ExtFileRead.cs` (full, 74 lines)
- `/mnt/archive4/DEV/NAADF/NAADF/World/VoxelTypeHandler.cs` (lines 1-120 — enums + struct VoxelType + ApplyVoxelType)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/vox_import.rs` (lines 1-620, 960-1004 vox_palette_to_voxel_types; full file is 1733 lines, balance is tests)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/grid.rs` (lines 100-230, 420-735)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/mod.rs` (full, 143 lines)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/async_vox.rs` (full, 208 lines)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/web_vox.rs` (lines 200-455)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/main.rs` (full, 49 lines)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/lib.rs` (lines 50-180, 270-450)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/bin/e2e_render.rs` (lines 75-450)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/aadf/generator.rs` (lines 1-120)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/aadf/construct.rs` (lines 75-132 — ConstructedWorld)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/Cargo.toml` (full, 204 lines)
- `/home/midori/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/dot_vox-5.2.0/src/parser.rs` (line 23 — MAGIC_NUMBER)

**Binary inspections (via `xxd -l N`):**

- `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` (first 32 bytes — confirmed `PK\x03\x04` ZIP local file header + `data` entry name)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/assets/test/oasis_hard_cover.vox` (first 16 bytes — confirmed `VOX ` magic + `MAIN` chunk)

**Cargo metadata:**

- `/mnt/archive4/DEV/bevy-naadf/Cargo.lock` (grep for `flate2`, `zip`, `flate2` v1.1.9 confirmed transitive, `zip` absent)
- `crates.io` API for `zip` crate availability check
