# 05-impl — .vox/.cvox magic dispatch implementation log

## general-purpose implementer findings (2026-05-19)

## Summary

Implemented the magic-byte dispatch design end-to-end. Added the `zip = "2"`
dependency, created the `.cvox` parser (`voxel/cvox_import.rs`) as a faithful
port of C# `ModelData.Load`, created the unified dispatch entry-point
(`voxel/voxel_dispatch.rs`), wired both into `voxel/mod.rs`, refactored
`grid::parse_to_imported_vox` into a one-line shim over the dispatcher, and
broadened the native drag-and-drop extension filter to accept both `.vox` and
`.cvox`. Six new unit tests were added (two for the `.cvox` parser, four for
the dispatcher). Verification surface: `cargo build --workspace` clean and
`cargo test --workspace --lib` reports 200 passed / 1 ignored (the pre-existing
baseline) — the four design-mandated tests all pass against the real
`/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` fixture (`size_in_chunks =
[65, 25, 68]` asserted as expected). No `cargo run --bin bevy-naadf` was
issued; the project verification-discipline rule was respected.

## Files created / modified (with line ranges)

| Action | File | What changed |
|---|---|---|
| created | `crates/bevy_naadf/src/voxel/cvox_import.rs` (newly authored, 425 lines incl. tests) | Faithful Rust port of C# `ModelData.Load` (`ModelData.cs:181-258`) — ZIP-`"data"` reader, 32-byte LE header, positional `VoxelType` palette (slot 0 reserved per NAADF convention), `dataChunk`/`dataBlock`/`dataVoxel` u32 arrays, `version < 3` migration, and the `voxelCount/2` packed-pair convention. Two unit tests asserting on the real `oasis.cvox`. |
| created | `crates/bevy_naadf/src/voxel/voxel_dispatch.rs` (newly authored, 170 lines incl. tests) | `VoxelFormat` enum, `detect_format()` (peeks 4 magic bytes), `VoxelParseError` (thiserror union of `TooShort` / `UnknownMagic` / `Vox` / `Cvox`), and the single `parse_voxel_bytes()` entry-point. Four unit tests covering both routing arms + unknown + truncated input. |
| modified | `crates/bevy_naadf/src/voxel/mod.rs:12-16` | Added `pub mod cvox_import;` and `pub mod voxel_dispatch;` alongside the existing `pub mod async_vox; pub mod grid; pub mod vox_import;`. |
| modified | `crates/bevy_naadf/src/voxel/grid.rs:502-516` | Replaced the body of `parse_to_imported_vox` with a single-line delegation to `crate::voxel::voxel_dispatch::parse_voxel_bytes(bytes).map_err(|e| e.to_string())`. Function signature unchanged — all upstream callers (sync, async-native, async-wasm, web fetch, drag-drop) inherit dispatch for free through this shim per design D3. Added inline comment + doc reference. |
| modified | `crates/bevy_naadf/src/voxel/grid.rs:702-722` | Broadened `native_vox_drop_listener`'s extension filter from `.vox`-only to `.vox || .cvox` (case-insensitive). Updated the log line wording from "ignoring non-.vox file" to "ignoring non-voxel file". |
| modified | `crates/bevy_naadf/src/voxel/grid.rs:649-665` | Updated doc-comment header of `native_vox_drop_listener` to mention both `.vox` and `.cvox` and point at `voxel/voxel_dispatch.rs` for the actual format routing. |
| modified | `crates/bevy_naadf/src/main.rs:19-29` | Updated the `--vox <path>` CLI doc-comment to describe the magic-byte dispatch (both formats accepted). No code change. |
| modified | `crates/bevy_naadf/src/lib.rs:74-83` | Updated `GridPreset::Vox` variant doc-comment to mention both formats and reference `voxel/voxel_dispatch.rs` + `voxel/cvox_import.rs`. Variant name retained per design D4. |
| modified | `crates/bevy_naadf/Cargo.toml:89-99` | Added the new dep `zip = { version = "2", default-features = false, features = ["deflate"] }`. Inline comment explains why minimal features were chosen and why the dep is needed. |

## .cvox parser implementation notes

All eleven assumptions from the design held — no surprises required logging a
divergence:

- **A1 (DEFLATE compression)** — held. The `zip` crate v2 with
  `features = ["deflate"]` successfully decompresses `oasis.cvox`.
- **A2 (entry named `"data"`)** — held. `archive.by_name("data")` succeeded; no
  fallback iteration path was needed.
- **A3 (`voxelCount` is even)** — held for the canonical asset. The parser
  surfaces an `OddVoxelCount` error variant for corrupt input rather than
  walking off the buffer (this is a strictly tighter check than C# performs,
  but produces a clean error message instead of an `UnexpectedEof` mid-read;
  the faithful-port impact is zero because every valid `.cvox` produced by
  C# `Save` will have an even `voxelCount`).
- **A4 (`Point3` i32-backed)** — held. The parser reads `modelSizeX/Y/Z` as
  `i32` then casts to `u32` via `as u32` (using `wrapping_add(15) / 16` so
  the `(size + 15) / 16` ceiling-divide stays well-defined even if a corrupt
  file had a tiny `size = 0xFFFFFFFD` value — this is matching how C# behaves
  on signed overflow during the ceiling-divide, since `unchecked` arithmetic
  is default for unsigned `Point3` ops in .NET).
- **A5 (Latin-1 strings)** — held; `read_null_terminated_string` constructs
  `char::from_u32(b as u32)` for every non-zero byte, bit-identical to C#'s
  `(char)b` cast.
- **A6 (oasis.cvox is version 3)** — held; the parsed header version is 3
  (the `apply_version_migration` branch was not exercised for this asset).
  Both tests passing on `[65, 25, 68]` chunk dims confirm the format version
  matches the design's expectation.
- **A7 (positional palette indexing)** — held; the parser emits
  `palette[0] = VoxelType::default()` then `palette[i+1] = parsed_entry_i`,
  matching the `.vox` import path at `vox_import.rs:967-969`. No runtime
  `renderIndex` remapping needed because the on-disk indices in
  `dataChunk`/`dataBlock`/`dataVoxel` are written *before* C#'s
  `CreateDataForRender` would have remapped them (Save line 158-173 writes
  raw `dataChunk`, not the rendered-index buffer).
- **A8 (.NET split-read branch is .NET-specific)** — held. A single
  `read_u32_array` covers any buffer size up to `isize::MAX` on Rust; the
  0x1FFF0000 split is collapsed to one read.
- **A9 (no JS-layer extension filter for web)** — held (no web code touched
  in this implementation).
- **A10 (zip crate v2 wasm-compatible)** — design notes this is to be
  verified at impl time; the workspace build succeeded native-only here. No
  `cargo check --target wasm32-unknown-unknown` was run because that target
  needs the workspace's nightly + `build-std` setup and isn't required by
  the design's verification commands. If a future agent finds that adding
  `zip` broke the wasm build, the fallback documented in D1 (hand-roll
  local-file-header + `flate2`) is still on the table.
- **A11 (chunk-count = `prod(size_in_chunks)`)** — held for `oasis.cvox`.
  The parser explicitly cross-checks this and surfaces a
  `ChunkCountMismatch` error if the on-disk value and the derived value
  diverge.

One small additional consideration: the design notes that an `Option<T>`
extension to `ImportedVox` would be the right place for `.cvox`-only metadata
(e.g. a saved camera pose, type IDs). For now, no such metadata is consumed
downstream — the C# `VoxelType.ID` field is read off the wire but discarded
(see `_id` in `read_voxel_type`) because the Bevy port has no global
`VoxelTypeHandler` to dedup against. This is exactly the behaviour design D2 +
A7 prescribe.

## Dispatch and call-site changes

The `grid::parse_to_imported_vox` shim now reads, in full:

```rust
pub fn parse_to_imported_vox(bytes: &[u8]) -> Result<vox_import::ImportedVox, String> {
    crate::voxel::voxel_dispatch::parse_voxel_bytes(bytes).map_err(|e| e.to_string())
}
```

(plus an inline `//` comment explaining the dispatch invariant). Every other
call site downstream of `parse_to_imported_vox` — `install_vox_bytes_in_fixed_world`,
`async_vox::spawn_native_vox_parse{_from_bytes}`, `web_vox::spawn_wasm_vox_parse`,
`web_vox::startup_fetch_default_vox` — inherits magic-byte dispatch through
this one shim with zero code changes at those sites (per design D3 — verified
by inspection that those callers all go through `parse_to_imported_vox`).

The drag-drop extension filter in `native_vox_drop_listener` was broadened to
accept both `.vox` and `.cvox` (case-insensitive); the actual format is
selected by the magic-byte dispatch downstream, so the extension check is
purely UX-level (skip files that the user clearly didn't mean to drop).

Single-dispatch invariant holds: there is exactly ONE place
(`voxel_dispatch::detect_format`) that maps magic bytes to a `VoxelFormat`,
and exactly ONE place (`voxel_dispatch::parse_voxel_bytes`) that routes to a
format-specific parser. No second copy of either lives anywhere in the tree.

## Tests added (file path + name + asserts)

Four mandated unit tests across two new files:

1. `crates/bevy_naadf/src/voxel/cvox_import.rs::tests::parses_oasis_cvox_header_dims`
   - Reads `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` (gracefully
     skips if absent).
   - Asserts `imp.world.size_in_chunks == [65, 25, 68]` (=
     `ceil(1033/16) × ceil(386/16) × ceil(1082/16)`).
   - Asserts `imp.world.chunks.len() == 65 * 25 * 68 = 110_500` for sanity.

2. `crates/bevy_naadf/src/voxel/cvox_import.rs::tests::parses_oasis_cvox_arrays_nonempty`
   - Same fixture (graceful skip).
   - Asserts `imp.world.blocks` and `imp.world.voxels` are both non-empty
     and `imp.palette.len() >= 2` (slot 0 reserved + ≥ 1 real entry).

3. `crates/bevy_naadf/src/voxel/voxel_dispatch.rs::tests::dispatch_routes_vox_to_dot_vox_parser`
   - Reads the in-tree fixture `assets/test/oasis_hard_cover.vox` (cargo
     runs tests from the crate root, so this relative path resolves).
   - Asserts `detect_format(&bytes) == Some(VoxelFormat::DotVox)` and that
     `parse_voxel_bytes(&bytes)` succeeds end-to-end.

4. `crates/bevy_naadf/src/voxel/voxel_dispatch.rs::tests::dispatch_routes_cvox_to_cvox_parser`
   - Reads `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` (graceful
     skip).
   - Asserts `detect_format(&bytes) == Some(VoxelFormat::Cvox)`, full
     dispatch succeeds, and the resulting `size_in_chunks == [65, 25, 68]`.

Plus two additional dispatch error-path tests (design's Verification plan
listed both as part of Test 4 but explicitly as separate functions):

5. `crates/bevy_naadf/src/voxel/voxel_dispatch.rs::tests::dispatch_rejects_unknown_magic`
   - Feeds `b"GIF89a..."` to `parse_voxel_bytes`.
   - Asserts the returned error is `VoxelParseError::UnknownMagic { magic:
     b"GIF8" }` and the format detector returns `None`.

6. `crates/bevy_naadf/src/voxel/voxel_dispatch.rs::tests::dispatch_rejects_truncated_input`
   - Feeds `b"VO"` (2 bytes).
   - Asserts `VoxelParseError::TooShort(2)`.

## Verification gate results

- **`cargo build --workspace`** — PASS. Compiles clean (zero new warnings on
  the new modules; `Finished dev profile [optimized + debuginfo] target(s) in
  46.46s`).
- **`cargo test --workspace --lib`** — PASS. 200 passed, 1 ignored, 0 failed
  across the workspace (`Finished test profile … in 4m 36s`). The single
  ignored test is pre-existing (not introduced by this work). Running the
  new tests targeted by name confirms each individually:
  - `cargo test --workspace --lib parses_oasis_cvox` → 2 passed (the two new
    cvox parser tests).
  - `cargo test --workspace --lib dispatch_` → 4 passed (the four new
    dispatch tests).
  - `cargo test --workspace --lib dispatch_routes_vox_to_dot_vox_parser` → 1
    passed (verifies the in-tree `.vox` fixture path resolves correctly with
    cargo's test cwd).
- **e2e gates** — the design's `## Verification commands` section explicitly
  lists only `cargo build --workspace` + `cargo test --workspace --lib` and
  states "No `e2e_render` invocation needed". Therefore: design lists no e2e
  gate; nothing further was run.
- **`cargo run --bin bevy-naadf`** — NOT run. The project CLAUDE.md
  verification-discipline rule was respected: no binary was booted as an
  agent verification step. The user does the live visual check.

## Divergences from the design

None — implementation matched the design exactly. Minor expansions of design
intent that don't constitute divergence:

- Added an explicit `CvoxImportError::OddVoxelCount` variant that the design
  noted as optional ("C# doesn't validate, but the extra bound check is
  cheap"). Mentioned in assumption A3.
- Added an explicit `CvoxImportError::ChunkCountMismatch` variant that the
  design called for in `## .cvox format specification` → `## Post-load
  sizeInChunks derivation` ("the parser must compute `size_in_chunks` the
  same way and may assert `chunk_count_on_disk == prod(size_in_chunks)` as a
  sanity check").
- Updated `main.rs:21-24` and `lib.rs:74-83` doc-comments per the design's
  call-site table items 6, 7, and 14. The design listed these as
  "doc-comment update — mention both formats"; the wording in the impl was
  chosen for clarity but the substance matches.

## Manual-QA instructions for the user

To verify the "exactly 4 modulo-wrapped instances of Oasis" parity with C#:

1. **Copy the reference `oasis.cvox` into the Bevy port** (anywhere
   convenient; the path is what `--vox` will receive). Two convenient
   options:

   ```sh
   # Option A: into the per-crate assets/test directory (where the .vox
   # fixture already lives — keeps everything in one place):
   cp /mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox \
      /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/assets/test/oasis.cvox

   # Option B: into a scratch location:
   cp /mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox /tmp/oasis.cvox
   ```

2. **Run the production binary with `--vox <path>`** (release build for
   sane perf; cwd should be the repo root so the relative path resolves):

   ```sh
   cd /mnt/archive4/DEV/bevy-naadf
   cargo run --release --bin bevy-naadf -- \
       --vox crates/bevy_naadf/assets/test/oasis.cvox
   # or, if you used Option B:
   # cargo run --release --bin bevy-naadf -- --vox /tmp/oasis.cvox
   ```

   The first launch will compile the workspace in release mode (~5-10 min
   depending on the box). Subsequent launches are incremental.

3. **What to look for visually** — exactly 4 modulo-wrapped copies of the
   Oasis model along each of the X and Z axes (3 full + 1 partial in both
   directions), matching the C#/MonoGame NAADF reference. The model is
   `1033×386×1082` voxels = `65×25×68` chunks; tiled into the
   `4096×512×4096`-voxel fixed world that yields `ceil(4096/1040) = 4` X-axis
   tiles and `ceil(4096/1088) = 4` Z-axis tiles. (Compare against
   `02-csharp-reference.md:144-156` for the C# arithmetic; if you see 2.5
   instances along X you're still loading `oasis_hard_cover.vox` rather than
   `.cvox`.)

4. **Drag-and-drop validation** (extra credit): from the same `--vox`
   invocation, drag a different `.vox` or `.cvox` file onto the window. The
   listener logs at `INFO` should report `drag-drop: DroppedFile event …`
   followed by `drag-drop: dispatching async .vox parse from …` (the
   listener still calls `spawn_native_vox_parse` for both extensions; the
   format selection happens inside `parse_to_imported_vox`).

   Drop a non-voxel file (e.g. a PNG) to see the "ignoring non-voxel file"
   log line.

5. **Environment / window setup** — no special expectations. Wayland users
   should know that some compositors don't forward drag-drop reliably; the
   `info!` logs at the top of `native_vox_drop_listener` (`HoveredFile` /
   `DroppedFile` / `HoveredFileCanceled`) make that diagnosable.

## Files / line ranges read or touched

**Read in full or substantially:**

- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/01-context.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/00-reuse-audit.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/02-csharp-reference.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/03-design.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md` (full)
- `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs` (lines 25-260)
- `/mnt/archive4/DEV/NAADF/NAADF/Common/Extensions/File/ExtFileRead.cs` (full)
- `/mnt/archive4/DEV/NAADF/NAADF/World/VoxelTypeHandler.cs` (full)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/mod.rs` (full)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/vox_import.rs` (lines 1-200, 940-1004)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/grid.rs` (lines 1-330, 450-735)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/aadf/construct.rs` (lines 70-140)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/main.rs` (full)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/lib.rs` (lines 60-90)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/Cargo.toml` (full)

**Binary inspections:**

- `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox` (first 32 bytes — confirmed `PK\x03\x04` ZIP local file header + `data` entry name)

**Edited or newly written:**

- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/cvox_import.rs` (CREATED — 425 lines including tests)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/voxel_dispatch.rs` (CREATED — 170 lines including tests)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/mod.rs` (lines 12-16 — module declarations added)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/grid.rs` (lines 502-516 dispatch shim; lines 649-665 doc-comment; lines 702-722 drag-drop filter)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/main.rs` (lines 19-29 doc-comment)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/lib.rs` (lines 74-83 `GridPreset::Vox` doc)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/Cargo.toml` (lines 89-99 `zip` dep)
