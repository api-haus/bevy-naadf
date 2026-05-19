# 07-palette-fix-impl — .cvox palette off-by-one fix

## general-purpose implementer findings (2026-05-19)

## Summary

Applied option α from `06-palette-diagnostic.md`: removed the spurious
synthetic slot-0 `VoxelType::default()` push in `cvox_import.rs` so the
parser now emits exactly `typeCount` palette entries that the on-disk
`dataChunk` / `dataBlock` / `dataVoxel` arrays index directly. The on-disk
slot-0 placeholder C# writes at `ModelData.cs:313-316` becomes Bevy
`palette[0]` unchanged; real types map 1:1. Capacity hint dropped by one,
one unit-test assertion relaxed from `>= 2` to `>= 1`, and the surrounding
doc/comment block rewritten to document the new (correct) NAADF-on-disk
convention. `cargo build --workspace` clean; `cargo test --workspace --lib`
reports 200 passed / 1 ignored — identical to the pre-fix baseline.

## Diff

All edits are in `crates/bevy_naadf/src/voxel/cvox_import.rs`. Net: **2 lines
of executable code changed** (capacity hint + push removal), plus
comment/doc-string rewrites in the same file.

### Edit 1 — module-level doc-comment (lines 39-46 before; replaced)

Updated the `## Output contract` section of the module docs to document the
new (correct) convention: palette has exactly `typeCount` entries; on-disk
slot 0 is C#'s baked-in placeholder; data arrays index palette directly. Old
text said "slot 0 is the reserved empty placeholder (NAADF convention,
mirroring the `.vox` import path at vox_import.rs:967-969)", which was the
diagnostic's identified misconception. New text explicitly calls out the
divergence from the `.vox` parser and points readers at
`06-palette-diagnostic.md`.

### Edit 2 — palette construction (lines 127-140 before; replaced)

```rust
// BEFORE
let type_count_usize = if type_count < 0 { 0 } else { type_count as usize };
let mut palette: Vec<VoxelType> = Vec::with_capacity(type_count_usize + 1);
palette.push(VoxelType::default());
for _ in 0..type_count_usize {
    palette.push(read_voxel_type(&mut cursor)?);
}
```

```rust
// AFTER
let type_count_usize = if type_count < 0 { 0 } else { type_count as usize };
let mut palette: Vec<VoxelType> = Vec::with_capacity(type_count_usize);
for _ in 0..type_count_usize {
    palette.push(read_voxel_type(&mut cursor)?);
}
```

Plus the comment block immediately above was rewritten from the
"VoxelTypeHandler.Clear slot-0 seed" justification (incorrect — that's a
C# runtime registry concern, not an on-disk format concern) to a description
of C# `CreateFromWorldData`'s on-disk slot-0 placeholder write. Comment now
explicitly forbids re-introducing the synthetic push and explains why this
diverges from the `.vox` parser.

### Edit 3 — unit-test assertion (the line previously at 511)

```rust
// BEFORE
// Slot 0 = reserved empty placeholder + at least 1 real entry.
assert!(
    imp.palette.len() >= 2,
    "palette should contain >= 1 real entry (got {})",
    imp.palette.len()
);
```

```rust
// AFTER
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
```

This is the only palette-length assertion in the `.cvox` test suite (the
other test, `parses_oasis_cvox_header_dims`, asserts only chunk dims). I
verified by reading the full `#[cfg(test)] mod tests` block — no other
`.cvox` test bakes in the old `+1` convention. The `.vox` parser tests
(asserting `palette.len() == 257`) are untouched and remain correct for
that path's separate convention.

## A7 amendment

Design A7 ("The C# `LoadVoxelType` → `ApplyVoxelType` round-trip does not
depend on the global type registry") is **technically correct in its core
claim** — the on-disk `.cvox` palette is a self-contained positional array
and the Bevy port doesn't need a global registry to consume it. But A7's
**closing sentence** ("Bevy's positional 1:1 mapping is correct" with slot
`i+1 = on_disk[i]`) was wrong: it carried over the `.vox` convention's
synthetic slot-0 reservation without realising the `.cvox` format **already
encodes** its own slot-0 placeholder. The corrected understanding: Bevy
`palette[i] = on_disk_types[i]` for `i in 0..typeCount` (no shift), because
C# `CreateFromWorldData` (`ModelData.cs:313-316`) bakes the placeholder
into on-disk slot 0 before serialising. Future readers reviewing A7 should
treat the `i+1` claim as superseded by this impl-log entry; the
`06-palette-diagnostic.md` analysis is the canonical reference for the
on-disk slot-0 convention.

## Verification gate results

- **`cargo build --workspace`** — PASS. `Finished dev profile [optimized +
  debuginfo] target(s) in 24.13s`. Zero new warnings.
- **`cargo test --workspace --lib`** — PASS. **200 passed, 1 ignored, 0
  failed** across both crates (`bevy-naadf` lib + `voxel-noise` lib),
  identical to the pre-fix baseline reported in `05-impl.md`. The single
  ignored test is pre-existing (not introduced by this work or by `05-impl`).
- **Targeted test runs:**
  - `cargo test --workspace --lib parses_oasis_cvox` → **2 passed** (the
    `.cvox` parser's `parses_oasis_cvox_header_dims` and
    `parses_oasis_cvox_arrays_nonempty` — the latter exercises the
    relaxed `>= 1` assertion against the real Oasis fixture, which has
    `palette.len() == 39` after the fix, comfortably ≥ 1).
  - `cargo test --workspace --lib dispatch_` → **4 passed** (all four
    voxel-dispatch tests, including the `.cvox` round-trip via the
    dispatcher).
- **Confirmation: `cargo run --bin bevy-naadf` was NOT run.** Project
  CLAUDE.md verification-discipline rule respected. Agent verification
  surface is `cargo build` + `cargo test` only; the user does the live
  visual check.
- **No e2e gate run.** Per project rules, none of the existing
  `e2e_render` modes targets the `.cvox` palette path, and the brief
  explicitly excludes adding new e2e gates.

## Manual-QA hand-off for the user

The user already verified the **4×4 instance count** parity in
`05-impl.md`'s manual-QA step. This palette-fix re-verification targets
**colour parity**. Run the same command:

```sh
cd /mnt/archive4/DEV/bevy-naadf
cargo run --release --bin bevy-naadf -- \
    --vox crates/bevy_naadf/assets/test/oasis.cvox
```

(or, if the fixture lives at `/tmp/oasis.cvox` from a previous QA pass, use
that path instead).

**Expected visual result:**

- Exactly **4×4 modulo-wrapped Oasis instances** (X × Z axes — unchanged
  from the previous QA pass, this fix touches palette, not instancing).
- **Correct colours** on the rendered Oasis: palm trees should appear
  green / palm-green, terrain brown, sand sand-coloured — i.e. matching
  the C#/MonoGame NAADF reference render. The previously-reported "blue
  palm trees" and other discolouration ("everything one palette slot off")
  should be GONE.

If colours still drift, the most likely remaining causes are unrelated:
sRGB↔linear conversion in the GPU palette buffer upload
(`render/prepare.rs:621`), or a `VoxelType::color_base` field-ordering bug
unrelated to palette indexing. Neither would be caused by this fix.

## Files touched

- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/cvox_import.rs`
  - Lines 39-46 (module doc-comment `## Output contract` section) — rewrote
    to document the new on-disk slot-0 convention and the intentional
    divergence from `.vox`.
  - Lines 127-140 (palette construction) — comment block rewritten +
    capacity hint dropped from `+ 1` to bare `type_count_usize` +
    `palette.push(VoxelType::default())` deleted. **Net code delta: 1 line
    edited, 1 line deleted.**
  - Test assertion previously at line ~511 (`parses_oasis_cvox_arrays_nonempty`)
    — changed lower bound from `>= 2` to `>= 1` and rewrote the comment
    that justified the old bound. **Net code delta: 1 line edited.**

No other files modified. `vox_import.rs` was deliberately left untouched
per the brief (the `.vox` path is self-consistent and uses a different,
matching `+1` shift on both palette and data sides).
