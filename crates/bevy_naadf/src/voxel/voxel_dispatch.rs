//! Magic-byte voxel-file dispatch.
//!
//! Both supported voxel formats start with a 4-byte fixed magic:
//! - MagicaVoxel `.vox` — `"VOX "` (`0x56 0x4F 0x58 0x20`), per
//!   `dot_vox-5.2.0/src/parser.rs:23`.
//! - NAADF `.cvox` — `"PK\x03\x04"` (`0x50 0x4B 0x03 0x04`), the
//!   ZIP-local-file-header magic; verified by hexdump of
//!   `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox`.
//!
//! [`parse_voxel_bytes`] peeks the first four bytes, picks the right parser,
//! and returns a unified [`ImportedVox`] — the same shape
//! [`crate::voxel::grid::install_imported_vox`] consumes for both source
//! formats. Single-dispatch is the load-bearing invariant: every caller
//! (`grid::install_vox_bytes_in_fixed_world`, drag-and-drop, autoload, async
//! helpers in `voxel::async_vox` and `voxel::web_vox`) goes through this
//! module, so adding a third format only touches this file + the new
//! format's parser.

use crate::voxel::cvox_import::{self, CvoxImportError};
use crate::voxel::vox_import::{self, ImportedVox, VoxImportError};

/// Voxel-file container format, identified by the first four bytes of the
/// input.
///
/// Returned by [`detect_format`]. Exposed publicly so debugging code / future
/// tooling can sniff a file's format without performing a full parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxelFormat {
    /// MagicaVoxel `.vox` — magic `"VOX "`.
    DotVox,
    /// NAADF `.cvox` (ZIP-wrapped binary) — magic `"PK\x03\x04"`.
    Cvox,
}

/// Union error type for [`parse_voxel_bytes`].
///
/// `VoxImportError` and `CvoxImportError` both implement `thiserror::Error`,
/// so each format-specific failure flows through unchanged via `#[from]`.
#[derive(Debug, thiserror::Error)]
pub enum VoxelParseError {
    /// Input shorter than 4 bytes — can't even check the magic.
    #[error("voxel file too short for magic-byte check ({0} bytes)")]
    TooShort(usize),

    /// First 4 bytes don't match any known voxel-file magic.
    #[error("unrecognised voxel-file magic bytes: {magic:02x?}")]
    UnknownMagic { magic: [u8; 4] },

    /// MagicaVoxel `.vox` parse failure.
    #[error(transparent)]
    Vox(#[from] VoxImportError),

    /// NAADF `.cvox` parse failure.
    #[error(transparent)]
    Cvox(#[from] CvoxImportError),
}

/// Sniff the first 4 bytes of `bytes` and return the matched [`VoxelFormat`],
/// or `None` if the input is too short / has an unknown magic.
///
/// Pure inspection — does no decoding, no allocation.
pub fn detect_format(bytes: &[u8]) -> Option<VoxelFormat> {
    if bytes.len() < 4 {
        return None;
    }
    match &bytes[..4] {
        b"VOX " => Some(VoxelFormat::DotVox),
        [0x50, 0x4B, 0x03, 0x04] => Some(VoxelFormat::Cvox),
        _ => None,
    }
}

/// Dispatch entry-point: detect the input format from its magic bytes and
/// route to the appropriate parser.
///
/// Both arms produce an [`ImportedVox`] — the same type the downstream
/// install path ([`crate::voxel::grid::install_imported_vox`]) consumes
/// agnostic to source format. The "AADF-strip" pass at `grid.rs:574-585` is a
/// no-op for `.cvox` data (empties are already literal `0`) and remains
/// correct for `.vox` data; see design D6.
pub fn parse_voxel_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxelParseError> {
    match detect_format(bytes) {
        Some(VoxelFormat::DotVox) => Ok(vox_import::parse_vox_bytes(bytes)?),
        Some(VoxelFormat::Cvox) => Ok(cvox_import::parse_cvox_bytes(bytes)?),
        None if bytes.len() < 4 => Err(VoxelParseError::TooShort(bytes.len())),
        None => {
            let mut magic = [0u8; 4];
            magic.copy_from_slice(&bytes[..4]);
            Err(VoxelParseError::UnknownMagic { magic })
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// In-tree MagicaVoxel test fixture. Always present.
    const VOX_FIXTURE_PATH: &str = "assets/test/oasis_hard_cover.vox";

    /// External `.cvox` reference (gracefully skipped if absent — see
    /// `cvox_import` tests + design D7).
    const CVOX_FIXTURE_PATH: &str = "/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox";

    /// Test 3a — `.vox` magic dispatches to the `dot_vox` parser.
    #[test]
    fn dispatch_routes_vox_to_dot_vox_parser() {
        let bytes = std::fs::read(VOX_FIXTURE_PATH)
            .expect("crates/bevy_naadf/assets/test/oasis_hard_cover.vox fixture missing");
        assert_eq!(
            detect_format(&bytes),
            Some(VoxelFormat::DotVox),
            "in-tree .vox fixture should have VOX magic"
        );
        // Full parse round-trip — the dispatch layer must successfully reach
        // and complete the `.vox` parser arm.
        parse_voxel_bytes(&bytes).expect("dispatch + .vox parse should succeed");
    }

    /// Test 3b — `.cvox` magic dispatches to the `.cvox` parser. Gracefully
    /// skipped if the external NAADF reference is unavailable.
    #[test]
    fn dispatch_routes_cvox_to_cvox_parser() {
        if !std::path::Path::new(CVOX_FIXTURE_PATH).exists() {
            eprintln!("skipping dispatch_routes_cvox_to_cvox_parser (no NAADF reference)");
            return;
        }
        let bytes = std::fs::read(CVOX_FIXTURE_PATH).expect("oasis.cvox read");
        assert_eq!(
            detect_format(&bytes),
            Some(VoxelFormat::Cvox),
            "oasis.cvox should start with the ZIP local file header magic"
        );
        let imp = parse_voxel_bytes(&bytes).expect("dispatch + .cvox parse should succeed");
        assert_eq!(imp.world.size_in_chunks, [65, 25, 68]);
    }

    /// Test 4a — bytes that don't match any known magic produce
    /// `VoxelParseError::UnknownMagic`. The dispatch does NOT call into
    /// either format-specific parser on unknown input.
    #[test]
    fn dispatch_rejects_unknown_magic() {
        // Valid GIF89a header — 9 bytes, certainly not a voxel file.
        let junk = b"GIF89a...";
        assert!(matches!(detect_format(junk), None));
        match parse_voxel_bytes(junk) {
            Err(VoxelParseError::UnknownMagic { magic }) => {
                assert_eq!(&magic, b"GIF8");
            }
            other => panic!("expected UnknownMagic, got {other:?}"),
        }
    }

    /// Test 4b — input shorter than 4 bytes produces
    /// `VoxelParseError::TooShort(n)` with the actual byte count.
    #[test]
    fn dispatch_rejects_truncated_input() {
        let short = b"VO";
        assert!(matches!(detect_format(short), None));
        match parse_voxel_bytes(short) {
            Err(VoxelParseError::TooShort(2)) => {}
            other => panic!("expected TooShort(2), got {other:?}"),
        }
    }
}
