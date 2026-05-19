//! NAADF `.cvox` ingestion — faithful Rust port of C# `ModelData.Load`
//! (`/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:181-258`).
//!
//! The `.cvox` format is a ZIP archive containing a single DEFLATE-compressed
//! entry named `"data"`. The decompressed payload is a fixed-width
//! little-endian binary header followed by a positional palette of
//! [`VoxelType`] entries and three flat `u32` arrays (`dataChunk` /
//! `dataBlock` / `dataVoxel`) that exactly match the encoding the Bevy port's
//! [`ConstructedWorld`] consumes (`crate::aadf::construct::ConstructedWorld`
//! at `aadf/construct.rs:94-107`).
//!
//! ## Payload layout (uncompressed, little-endian, fixed-width)
//!
//! ```text
//!  offset | bytes | field            | C# source
//! --------+-------+------------------+------------------------------------
//!     0   |   4   | version (i32)    | ModelData.cs:199
//!     4   |   4   | modelSizeX (i32) | ModelData.cs:201
//!     8   |   4   | modelSizeY (i32) | ModelData.cs:202
//!    12   |   4   | modelSizeZ (i32) | ModelData.cs:203
//!    16   |   4   | typeCount (i32)  | ModelData.cs:206
//!    20   |   4   | chunkCount (u32) | ModelData.cs:207
//!    24   |   4   | blockCount (u32) | ModelData.cs:208
//!    28   |   4   | voxelCount (u32) | ModelData.cs:209
//!    32   |  ...  | types[typeCount] | ModelData.cs:212-216 (see below)
//!   ...   |  ...  | dataChunk[chunkCount]  × u32 LE | ModelData.cs:219-221
//!   ...   |  ...  | dataBlock[blockCount]  × u32 LE | ModelData.cs:223-225
//!   ...   |  ...  | dataVoxel[voxelCount/2]× u32 LE | ModelData.cs:227-240
//! ```
//!
//! Each palette `VoxelType` entry is variable-width (null-terminated string +
//! 36 fixed bytes); see [`read_voxel_type`].
//!
//! For `version < 3`, an in-place migration is applied to `dataBlock`
//! (`ModelData.cs:242-250`): mixed blocks have their child-pointer payload
//! halved. The reference `oasis.cvox` is version 3 so this is dead code for
//! the canonical target but the faithful-port rule mandates the branch.
//!
//! ## Output contract
//!
//! [`parse_cvox_bytes`] returns an [`ImportedVox`] — the same shape
//! [`crate::voxel::vox_import::parse_vox_bytes`] produces. The returned
//! `palette: Vec<VoxelType>` has **exactly `typeCount` entries** — slot 0 is
//! the on-disk placeholder that C# `CreateFromWorldData` writes
//! unconditionally as the first compacted entry (`ModelData.cs:313-316`,
//! id=`"_"` + all-zero colors), and slots `1..N-1` are the real types. The
//! on-disk `dataChunk` / `dataBlock` / `dataVoxel` arrays index this palette
//! **directly with no shift** (0 = placeholder, 1..N-1 = real types) — the
//! `.cvox` format is self-contained.
//!
//! This is intentionally different from the `.vox` import path
//! (`vox_import.rs:967-969`), which prepends a synthetic placeholder *and*
//! shifts the data side by `+1` at `vox_import.rs:627`. The MagicaVoxel
//! convention is "slot 0 = empty" with positional 0..255 indices; the NAADF
//! `.cvox` convention is "slot 0 already encoded as placeholder on disk" —
//! the parser must NOT add another. See
//! `docs/orchestrate/oasis-vox-instance-count/06-palette-diagnostic.md`.

use std::io::Read;

use bevy::math::Vec3;
use thiserror::Error;

use crate::aadf::construct::ConstructedWorld;
use crate::voxel::vox_import::ImportedVox;
use crate::voxel::{MaterialBase, MaterialLayer, VoxelType};

/// Errors emitted by [`parse_cvox_bytes`].
///
/// Symmetric with [`crate::voxel::vox_import::VoxImportError`] so the dispatch
/// entry-point (`voxel/voxel_dispatch.rs`) can union both via `thiserror`.
#[derive(Debug, Error)]
pub enum CvoxImportError {
    /// `std::io` / `zip` I/O failure (read off the end of the buffer, truncated
    /// archive, mid-stream decompression error, ...).
    #[error("I/O error reading .cvox: {0}")]
    Io(#[from] std::io::Error),

    /// `zip` crate rejected the archive (corrupt local file header, unsupported
    /// compression method, missing central directory, ...).
    #[error("ZIP archive parse failed: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// The archive was readable but contained no entry named `"data"` (or had
    /// zero entries entirely). C# `ZipArchive.GetEntry("data")` returns `null`
    /// in this case and `Load` then NRE-throws on `entry.Open()` — we surface
    /// the failure cleanly instead.
    #[error("ZIP archive does not contain a 'data' entry")]
    MissingDataEntry,

    /// `voxelCount` field was odd; on-disk `dataVoxel.len() == voxelCount / 2`,
    /// which requires an even count (each `u32` packs two voxel `u16`s). The
    /// C# code does not validate this — but a corrupt or hand-modified file
    /// with odd `voxelCount` would walk the stream off the end of the buffer
    /// at runtime, so we surface it as a clean error. See assumption A3 in
    /// `docs/orchestrate/oasis-vox-instance-count/03-design.md`.
    #[error("voxelCount field is odd ({0}); .cvox stores voxelCount/2 packed u32s")]
    OddVoxelCount(u32),

    /// The on-disk `chunkCount` field did not match the derived
    /// `prod(size_in_chunks)`. C# computes `chunkCount = (uint)(sizeInChunks.X
    /// * sizeInChunks.Y * sizeInChunks.Z)` from `modelSize` in its constructor
    /// (`ModelData.cs:40-41`) and trusts the on-disk value to match. A
    /// mismatch indicates a corrupt file; surface explicitly rather than
    /// crash on slice-OOB later.
    #[error(
        "chunkCount mismatch: on-disk={on_disk}, derived={derived} \
         (size_in_chunks={size_in_chunks:?})"
    )]
    ChunkCountMismatch {
        on_disk: u32,
        derived: u32,
        size_in_chunks: [u32; 3],
    },
}

/// Parse `.cvox` bytes into an [`ImportedVox`].
///
/// Faithful port of C# `ModelData.Load`. Pure CPU; no Bevy resources, no
/// filesystem. Symmetric with `vox_import::parse_vox_bytes`.
pub fn parse_cvox_bytes(bytes: &[u8]) -> Result<ImportedVox, CvoxImportError> {
    // --- Container layer: unzip the single `"data"` entry (ModelData.cs:192-198).
    let payload = read_zip_data_entry(bytes)?;
    let mut cursor = Cursor::new(&payload);

    // --- Header (ModelData.cs:199-209). 32 bytes, 8 little-endian i32/u32.
    let version = cursor.read_i32()?;

    let model_size_x = cursor.read_i32()?;
    let model_size_y = cursor.read_i32()?;
    let model_size_z = cursor.read_i32()?;

    let type_count = cursor.read_i32()?;
    let chunk_count_on_disk = cursor.read_u32()?;
    let block_count = cursor.read_u32()?;
    let voxel_count = cursor.read_u32()?;

    // --- Palette (ModelData.cs:212-216). Variable-width entries.
    //
    // The Bevy palette has **exactly `typeCount` entries**, read 1:1 from
    // disk. C# `CreateFromWorldData` (`ModelData.cs:313-316`) writes a
    // placeholder VoxelType (id="_", all-zero colors) as on-disk slot 0
    // before any real types, then writes the compacted real palette into
    // slots 1..N-1. The on-disk `dataChunk` / `dataBlock` / `dataVoxel`
    // arrays index this palette directly with no shift (value 0 = the
    // on-disk placeholder, values 1..N-1 = the real types).
    //
    // Do NOT prepend a synthetic `VoxelType::default()` here — that would
    // shift every real-type lookup by -1, rendering each voxel with the
    // previous palette slot's color (the "blue palm trees" bug). See
    // `docs/orchestrate/oasis-vox-instance-count/06-palette-diagnostic.md`.
    //
    // This intentionally diverges from the `.vox` parser at
    // `vox_import.rs:967-969` (which prepends a placeholder *and* shifts
    // voxel data by +1 at `vox_import.rs:627`) because the on-disk format
    // is different: MagicaVoxel uses raw 0-based palette indices and needs
    // an explicit slot-0 reservation; NAADF `.cvox` already bakes the
    // placeholder into on-disk slot 0.
    let type_count_usize = if type_count < 0 { 0 } else { type_count as usize };
    let mut palette: Vec<VoxelType> = Vec::with_capacity(type_count_usize);
    for _ in 0..type_count_usize {
        palette.push(read_voxel_type(&mut cursor)?);
    }

    // --- Voxel data arrays (ModelData.cs:218-240). Raw little-endian u32s.
    //
    // C# casts modelSizeX/Y/Z (i32) into `Point3` (also i32) and trusts them
    // as positive. Negative values would be silently re-interpreted as huge
    // positives by `as u32`. We mirror that behaviour exactly (no validation
    // beyond what C# performs).
    let size_in_chunks = [
        ((model_size_x as u32).wrapping_add(15)) / 16,
        ((model_size_y as u32).wrapping_add(15)) / 16,
        ((model_size_z as u32).wrapping_add(15)) / 16,
    ];

    // Sanity check: the on-disk chunkCount must equal the derived product.
    // C# ModelData.cs:40-41 recomputes chunkCount in the constructor and
    // ignores the on-disk value (it overwrites `this.chunkCount` from
    // sizeInChunks); the on-disk value is only used to size `dataChunk`.
    // Catching a mismatch keeps the parser from walking off the end of the
    // payload buffer mid-array.
    let chunk_count_derived = size_in_chunks[0]
        .saturating_mul(size_in_chunks[1])
        .saturating_mul(size_in_chunks[2]);
    if chunk_count_on_disk != chunk_count_derived {
        return Err(CvoxImportError::ChunkCountMismatch {
            on_disk: chunk_count_on_disk,
            derived: chunk_count_derived,
            size_in_chunks,
        });
    }

    let mut data_chunk: Vec<u32> = vec![0u32; chunk_count_on_disk as usize];
    cursor.read_u32_array(&mut data_chunk)?;

    let mut data_block: Vec<u32> = vec![0u32; block_count as usize];
    cursor.read_u32_array(&mut data_block)?;

    if voxel_count % 2 != 0 {
        return Err(CvoxImportError::OddVoxelCount(voxel_count));
    }
    let voxel_data_count = (voxel_count / 2) as usize;
    let mut data_voxel: Vec<u32> = vec![0u32; voxel_data_count];
    cursor.read_u32_array(&mut data_voxel)?;

    // --- Version migration (ModelData.cs:242-250).
    //
    // For `version < 3`, mixed blocks need their child-pointer payload halved
    // (the encoding doubled at some point in the format's history). Verbatim
    // port — `oasis.cvox` is v3 so this branch is dead for the canonical
    // asset, but the faithful-port rule applies.
    if version < 3 {
        apply_version_migration(&mut data_block);
    }

    let world = ConstructedWorld {
        chunks: data_chunk,
        blocks: data_block,
        voxels: data_voxel,
        size_in_chunks,
    };

    Ok(ImportedVox { world, palette })
}

// ----------------------------------------------------------------------------
// Internal helpers
// ----------------------------------------------------------------------------

/// Read the single `"data"` entry from a `.cvox` ZIP archive and return the
/// decompressed payload bytes.
///
/// Mirrors C# `ModelData.cs:192-197`:
/// ```csharp
/// using (var fileStream = File.Open(fileName, FileMode.Open))
/// using (var archive = new ZipArchive(fileStream, ZipArchiveMode.Read, false))
/// var entry = archive.GetEntry("data");
/// using (var zipStream = entry.Open())
/// ```
fn read_zip_data_entry(bytes: &[u8]) -> Result<Vec<u8>, CvoxImportError> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader)?;
    let mut entry = match archive.by_name("data") {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => {
            return Err(CvoxImportError::MissingDataEntry)
        }
        Err(e) => return Err(CvoxImportError::Zip(e)),
    };
    let mut out: Vec<u8> = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut out)?;
    Ok(out)
}

/// Tiny byte-cursor that mirrors C# `ExtFileRead` (`ReadInt` / `ReadUInt` /
/// `ReadFloat` / `ReadNullTerminated`) one method at a time.
///
/// We use a hand-rolled struct rather than `std::io::Cursor<&[u8]>` so the
/// little-endian readers stay inline + non-allocating; the C# extension
/// methods use a static 16-byte buffer (`ExtFileRead.cs:12`) for the same
/// reason.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Advance the cursor by `n` bytes and return the next `n`-byte slice, or
    /// an `UnexpectedEof` if past the end. Mirrors C# `Stream.Read` failing
    /// off the end of the buffer.
    fn take(&mut self, n: usize) -> Result<&'a [u8], std::io::Error> {
        let end = self.pos.checked_add(n).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "cursor position overflow",
            )
        })?;
        if end > self.bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "tried to read {} bytes at position {} but payload is {} bytes",
                    n,
                    self.pos,
                    self.bytes.len(),
                ),
            ));
        }
        let s = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn read_i32(&mut self) -> Result<i32, std::io::Error> {
        let s = self.take(4)?;
        let mut buf = [0u8; 4];
        buf.copy_from_slice(s);
        Ok(i32::from_le_bytes(buf))
    }

    fn read_u32(&mut self) -> Result<u32, std::io::Error> {
        let s = self.take(4)?;
        let mut buf = [0u8; 4];
        buf.copy_from_slice(s);
        Ok(u32::from_le_bytes(buf))
    }

    fn read_f32(&mut self) -> Result<f32, std::io::Error> {
        let s = self.take(4)?;
        let mut buf = [0u8; 4];
        buf.copy_from_slice(s);
        Ok(f32::from_le_bytes(buf))
    }

    fn read_vec3(&mut self) -> Result<Vec3, std::io::Error> {
        let x = self.read_f32()?;
        let y = self.read_f32()?;
        let z = self.read_f32()?;
        Ok(Vec3::new(x, y, z))
    }

    /// Read the next `dst.len() × 4` bytes as a little-endian `u32` array into
    /// `dst`. Mirrors C# `MemoryMarshal.AsBytes(new Span<uint>(arr));
    /// stream.ReadExactly(...)` at `ModelData.cs:219-225`.
    ///
    /// On little-endian hosts (every Bevy target we care about — x86_64,
    /// aarch64-le, wasm32) this is a `copy_from_slice` + `bytemuck`-style
    /// reinterpret; we use explicit `from_le_bytes` so the parser stays
    /// portable + dependency-free.
    fn read_u32_array(&mut self, dst: &mut [u32]) -> Result<(), std::io::Error> {
        let n_bytes = dst
            .len()
            .checked_mul(4)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "u32 array byte length overflow",
                )
            })?;
        let src = self.take(n_bytes)?;
        for (i, chunk) in src.chunks_exact(4).enumerate() {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(chunk);
            dst[i] = u32::from_le_bytes(buf);
        }
        Ok(())
    }

    /// Read an ASCII / Latin-1 null-terminated string.
    ///
    /// Mirrors `ExtFileRead.cs:14-22`:
    /// ```csharp
    /// var sb = new StringBuilder();
    /// int nc;
    /// while ((nc = stream.ReadByte()) > 0)
    ///     sb.Append((char)nc);
    /// ```
    ///
    /// The `(char)nc` cast is Latin-1 — for `nc < 128` it's ASCII, for
    /// `128 <= nc <= 255` the resulting `.NET` `char` is U+0080..U+00FF. We
    /// preserve that bit-for-bit via `char::from_u32(b as u32)`.
    ///
    /// Note: the C# loop exits on `nc > 0`, which means end-of-stream
    /// (`ReadByte` returns `-1`) is treated the same as the terminating null
    /// byte. We replicate that: an EOF in mid-string returns the bytes read
    /// so far rather than erroring (faithful-port rule).
    fn read_null_terminated_string(&mut self) -> String {
        let mut s = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            if b == 0 {
                break;
            }
            // Latin-1: byte → U+0000..U+00FF (always a valid Unicode scalar).
            s.push(char::from_u32(b as u32).expect("Latin-1 byte is always a valid scalar"));
        }
        s
    }
}

/// Read one `VoxelType` palette entry from the cursor.
///
/// Mirrors C# `ModelData.LoadVoxelType` (`ModelData.cs:61-72`):
/// ```csharp
/// string id = stream.ReadNullTerminated();
/// type.ID = id.StartsWith("_") ? null : id;          // we discard ID
/// type.colorBase = stream.ReadVector3();
/// type.colorLayered = stream.ReadVector3();
/// type.materialBase = (MaterialTypeBase)stream.ReadInt();
/// type.materialLayer = (MaterialTypeLayer)stream.ReadInt();
/// type.roughness = stream.ReadFloat();
/// ```
///
/// The C# `VoxelType.ID` field is consumed only by
/// `VoxelTypeHandler.ApplyVoxelType` for de-duplication into a global
/// dictionary (`VoxelTypeHandler.cs:73-86`). The Bevy port has no such global
/// registry (it stores palettes per-world in `VoxelTypes::types`), so we
/// drop the ID. See design D2 + A7.
///
/// Unknown enum values get clamped to `Diffuse` / `None` — C# would raise
/// `InvalidCastException` on an out-of-range cast, but the faithful-port rule
/// only mandates matching C# *for files C# accepts*. Robust v1 default.
fn read_voxel_type(cursor: &mut Cursor) -> Result<VoxelType, std::io::Error> {
    // ID — read + discard. C# discards too (sets to null if it starts with
    // "_", otherwise sets it as a key into the global dedup dict that doesn't
    // exist on the Bevy side).
    let _id = cursor.read_null_terminated_string();

    let color_base = cursor.read_vec3()?;
    let color_layered = cursor.read_vec3()?;
    let material_base = decode_material_base(cursor.read_i32()?);
    let material_layer = decode_material_layer(cursor.read_i32()?);
    let roughness = cursor.read_f32()?;

    Ok(VoxelType {
        material_base,
        material_layer,
        roughness,
        color_base,
        color_layered,
    })
}

/// Decode a C# `MaterialTypeBase` enum value
/// (`VoxelTypeHandler.cs:14-20`). Unknown values fall back to `Diffuse` (slot
/// 0 of the enum — the safest default for a corrupt file).
fn decode_material_base(value: i32) -> MaterialBase {
    match value {
        0 => MaterialBase::Diffuse,
        1 => MaterialBase::Emissive,
        2 => MaterialBase::MetallicRough,
        3 => MaterialBase::MetallicMirror,
        _ => MaterialBase::Diffuse,
    }
}

/// Decode a C# `MaterialTypeLayer` enum value
/// (`VoxelTypeHandler.cs:22-27`). Value `1` is intentionally absent in the
/// C# enum. Unknown values fall back to `None`.
fn decode_material_layer(value: i32) -> MaterialLayer {
    match value {
        0 => MaterialLayer::None,
        2 => MaterialLayer::MetallicRough,
        3 => MaterialLayer::MetallicMirror,
        _ => MaterialLayer::None,
    }
}

/// Apply the `version < 3` block-payload migration in-place.
///
/// Verbatim port of `ModelData.cs:242-250`:
/// ```csharp
/// if (version < 3) {
///     for (int i = 0; i < blockCount; ++i) {
///         uint curBlock = dataBlock[i];
///         if ((curBlock >> 31) == 1)
///             dataBlock[i] = ((curBlock & 0x3FFFFFFF) / 2) | (1u << 31);
///     }
/// }
/// ```
///
/// Mixed-block payloads (top bit set, low 30 bits = child pointer) get their
/// child pointer halved. Empty / uniform-full blocks are untouched.
fn apply_version_migration(data_block: &mut [u32]) {
    for slot in data_block.iter_mut() {
        let cur = *slot;
        if (cur >> 31) == 1 {
            *slot = ((cur & 0x3FFF_FFFF) / 2) | (1u32 << 31);
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Path to the canonical C# reference asset. The `.cvox` parser is verified
    /// against this single file (Test 1 + Test 2); CI runners without the
    /// NAADF reference clone gracefully skip rather than hard-fail. See
    /// design D7.
    const OASIS_CVOX_PATH: &str = "/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox";

    /// Test 1 — parsing the real `oasis.cvox` produces the C#-derived
    /// `size_in_chunks = [65, 25, 68]` (i.e. `ceil(1033/16) × ceil(386/16) ×
    /// ceil(1082/16)`). The load-bearing fact for the user's "4 instances"
    /// claim — see `02-csharp-reference.md:71-76`.
    #[test]
    fn parses_oasis_cvox_header_dims() {
        if !std::path::Path::new(OASIS_CVOX_PATH).exists() {
            eprintln!("skipping parses_oasis_cvox_header_dims (no NAADF reference)");
            return;
        }
        let bytes = std::fs::read(OASIS_CVOX_PATH).expect("oasis.cvox read");
        let imp = parse_cvox_bytes(&bytes).expect("parse_cvox_bytes failed");

        // ceil(1033/16)=65, ceil(386/16)=25, ceil(1082/16)=68.
        assert_eq!(
            imp.world.size_in_chunks,
            [65, 25, 68],
            "Oasis cvox chunk-dims mismatch"
        );

        // Sanity: derived chunk count == prod(size_in_chunks). The cvox parser
        // already cross-checks this against the on-disk `chunkCount` field
        // (`CvoxImportError::ChunkCountMismatch`); this assertion verifies the
        // happy path landed.
        assert_eq!(imp.world.chunks.len(), 65 * 25 * 68);
    }

    /// Test 2 — palette and arrays are non-empty for the real `oasis.cvox`.
    #[test]
    fn parses_oasis_cvox_arrays_nonempty() {
        if !std::path::Path::new(OASIS_CVOX_PATH).exists() {
            eprintln!("skipping parses_oasis_cvox_arrays_nonempty (no NAADF reference)");
            return;
        }
        let bytes = std::fs::read(OASIS_CVOX_PATH).expect("oasis.cvox read");
        let imp = parse_cvox_bytes(&bytes).expect("parse_cvox_bytes failed");

        assert!(!imp.world.blocks.is_empty(), "data_block empty");
        assert!(!imp.world.voxels.is_empty(), "data_voxel empty");
        // Palette has exactly `typeCount` entries read 1:1 from disk — slot 0
        // is C#'s on-disk placeholder (`CreateFromWorldData` at
        // `ModelData.cs:313-316`), slots 1..N-1 are the real types. The
        // smallest valid `.cvox` has `typeCount = 1` (just the placeholder),
        // so the lower bound is `>= 1`.
        assert!(
            imp.palette.len() >= 1,
            "palette should contain at least the on-disk slot-0 placeholder (got {})",
            imp.palette.len()
        );
    }
}
