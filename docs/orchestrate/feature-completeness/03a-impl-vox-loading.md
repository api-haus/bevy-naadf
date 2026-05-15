# 03a — Implementation log — Track A: VOX loading

**Date:** 2026-05-15
**Author:** delegate-implementer
**Branch:** `main` (uncommitted; orchestrator dispatches a separate commit agent)
**Design:** `docs/orchestrate/feature-completeness/02a-design-vox-loading.md`

## Summary

Track A (large MagicaVoxel `.vox` world loading) landed end-to-end against the
architect's design with one new file (`crates/bevy_naadf/src/voxel/vox_import.rs`,
791 LOC including 10 tests), one new dep (`dot_vox = "5.2"` in
`crates/bevy_naadf/Cargo.toml:48`), and 5 edited files wiring a new
`GridPreset::Vox { path }` arm through `voxel/grid.rs::setup_test_grid` plus a
minimal `--vox <path>` CLI flag in `src/main.rs`. All 10 unit tests pass; the
existing 132 lib tests stay green; the e2e baseline harness (`cargo run --bin
e2e_render`) passes its batch-6 luminance gate unchanged. The default content
path (`GridPreset::Default` → `build_default_volume`) is byte-identical to
pre-Track-A — `.vox` loading is strictly additive, opt-in via `--vox <path>`.

## Changes by file

### NEW

| Path | What | LOC |
|---|---|---|
| `crates/bevy_naadf/src/voxel/vox_import.rs` | Track A module — `dot_vox::DotVoxData` → `(DenseVolume, Vec<VoxelType>)` glue. Public API: `ImportedVox`, `VoxImportError`, `parse_vox_bytes(&[u8])`, `load_vox(impl AsRef<Path>)`, `parse_dot_vox_data(&DotVoxData)`, `build_world_from_vox(ImportedVox) -> (WorldData, VoxelTypes)`. Internal: `flatten_scene` (scene-graph walk, identity transform, Z↔Y swap), `vox_palette_to_voxel_types` (sRGB→linear + `_emit`/`_flux` → `MaterialBase::Emissive`), `collect_referenced_model_ids`, `round_up_to_chunks`. 10 `#[test]` cases. | NEW · 791 (~470 prod + 320 tests/fixtures) |

### Edited

| Path | Edit | Δ LOC |
|---|---|---|
| `crates/bevy_naadf/Cargo.toml` | Added `dot_vox = "5.2"` under `[dependencies]` between `bytemuck` and `image` with a doc comment matching the surrounding style (`Cargo.toml:43-49`). | +7 |
| `crates/bevy_naadf/src/voxel/mod.rs` | `pub mod vox_import;` declaration next to `pub mod grid;` (`voxel/mod.rs:13`). | +1 |
| `crates/bevy_naadf/src/lib.rs` | Dropped `Copy` from `GridPreset` and `AppArgs` (PathBuf isn't Copy — Design Assumption #5). Added `GridPreset::Vox { path: PathBuf }` variant (`lib.rs:53-66`). Changed `AppArgs` derive from `Clone, Copy` to `Clone` (`lib.rs:196-205`). Cloned `args` into the `insert_resource` site so the subsequent `args.spawn_test_entity` / `args.resize_test` reads still compile (`lib.rs:469-472`). | +20 / -4 |
| `crates/bevy_naadf/src/main.rs` | Switched from `build_app(AppConfig::windowed())` to `build_app_with_args(AppConfig::windowed(), args)` with a minimal `std::env::args` parser for `--vox <path>`. No `clap` (`main.rs:1-32`). | +24 / -2 |
| `crates/bevy_naadf/src/voxel/grid.rs` | Imported `vox_import`; extended the `match args.grid_preset` in `setup_test_grid` with a `GridPreset::Vox { path } => match vox_import::load_vox(path) { Ok(imp) => (imp.palette, imp.volume), Err(e) => { error!(...); (build_palette(), build_default_volume()) } }` arm. The default arm is unchanged. Changed match-by-value to match-by-ref so the new path-carrying variant compiles (`grid.rs:66-91`). | +24 / -2 |
| `crates/bevy_naadf/src/aadf/construct.rs` | Added `Debug` to `DenseVolume`'s derive so the new `ImportedVox` (which embeds it) can derive `Debug` — required by test panics formatting `{:?}` on a `Result<ImportedVox, _>` (`construct.rs:33-34`). | +1 / -1 |

### Files NOT touched (per design's "Files NOT touched" §)

`aadf/construct.rs` (only a trivial `#[derive(Debug)]` add, no semantic change),
`aadf/generator.rs`, `aadf/edit.rs`, `aadf/bounds.rs`, `aadf/entity.rs`,
the entire `render/` tree, `world/data.rs`, `world/buffer.rs`,
shader assets, `bin/bake.rs`, `panel.rs`, `hud.rs`, `e2e/`. The e2e harness
content path stays `GridPreset::Default`.

## Cargo.toml + Cargo.lock changes

**Added (workspace `Cargo.lock` diff):**
- `dot_vox v5.2.0` — pure-Rust MagicaVoxel `.vox` parser (MIT, dust-engine,
  edition 2024, no native deps). Imports `byteorder`-ish parsing via `nom`,
  uses `lazy_static`+`log`+default-feature `ahash`. `ahash` was already in the
  Bevy 0.19 transitive tree, so no duplicate copy.
- `nom v8.0.0` — `dot_vox`'s parser dependency. `nom v7.1.3` is already in the
  tree via another crate; cargo carries both side-by-side (acceptable — this
  is a leaf dep of `dot_vox` only, not in the hot path).

`cargo fetch` reports: `Adding dot_vox v5.2.0`, `Adding nom v8.0.0`. Total
`Cargo.lock` delta: +23 lines (one new `[[package]]` block for each new dep).

## Verification

### `cargo build --workspace`

**PASS.**

```
   Compiling bevy-naadf v0.1.0 (/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf)
    Finished `dev` profile [optimized + debuginfo] target(s) in 56.88s
```

No warnings beyond pre-existing ones; clean compile on both first and
incremental runs.

### `cargo test --workspace --lib`

**PASS — 142 total tests across 3 suites; 0 failures.**

```
    Finished `test` profile [optimized + debuginfo] target(s) in 13.70s
     Running unittests src/lib.rs (target/debug/deps/bevy_instamat-...)
     Running unittests src/lib.rs (target/debug/deps/bevy_naadf-...)
     Running unittests src/lib.rs (target/debug/deps/voxel_noise-...)
cargo test: 142 passed, 1 ignored (3 suites, 4.28s)
```

`cargo test --workspace --lib voxel::vox_import` confirms all 10 new VOX tests
pass:

```
cargo test: 10 passed, 133 filtered out (3 suites, 0.00s)
```

The 10 VOX tests (matching the design's `## Test plan` test-by-test, plus 2
bonus tests):

1. `parses_single_voxel_fixture` — round-trips a single-voxel `DotVoxData`
   through `write_vox` → `parse_vox_bytes`; asserts `size_in_chunks == [1, 1, 1]`,
   palette length 257, voxel at origin with `VoxelTypeId(1)`.
2. `parses_small_cube_fixture` — 8×8×8 fixture (7³ diffuse + 1 emissive
   replacing centre); asserts 343 non-empty voxels, emissive material maps to
   `MaterialBase::Emissive` with `color_layered.x > 0`.
3. `palette_index_zero_is_empty_placeholder` — `imp.palette[0] == VoxelType::default()`.
4. `palette_emissive_from_matl` — hand-built `DotVoxData` with `_emit: "1.0"`
   on palette slot 5; asserts `palette[6].material_base == Emissive` and
   `color_layered.x ≈ 5.0` (the `emit * (1 + flux)^2 * 5` formula).
5. `zy_swap_matches_csharp` — single voxel at MagicaVoxel `(1, 2, 3)` lands
   at NAADF `(1, 3, 2)`.
6. `size_exceeds_texture_limit_errors` — model size `(16_400, 1, 1)` → `Err(VoxImportError::SizeExceedsTextureLimit { .. })`.
7. `empty_models_errors` — `models: vec![]` → `Err(VoxImportError::Empty)`.
8. `construct_runs_on_imported_volume` — end-to-end: imported volume feeds
   `aadf::construct::construct()`, returns 1 chunk + non-empty blocks/voxels.
9. **Bonus** `build_world_from_vox_inserts_dense_voxel_types` — verifies the
   install half (`WorldData::dense_voxel_types` populated, `dirty` set, bbox
   correct for 16³).
10. **Bonus** `load_vox_propagates_io_error` — non-existent path returns
    `VoxImportError::Io(_)`.

### Smoke run

**One smoke run** of the e2e harness baseline (per memory
`subagent-gpu-app-verification-loop` — visual gating is the user's job).

```bash
cargo run --bin e2e_render 2>&1 | tail -10
```

Output (last frames):

```
[INFO] bevy_naadf::voxel::grid: NAADF test grid (Default): 32 chunks, 1920 blocks, 7232 voxel-u32s (64x32x64 voxels)
[INFO] bevy_naadf::render::construction: phase-c followup#1 — GPU producer chain DISPATCHED (size_in_chunks=[4, 2, 4], voxel_workgroups=227, block_workgroups=31). Algorithm 1 is now the runtime producer for chunks/blocks/voxels.
e2e_render: screenshot saved to target/e2e-screenshots/e2e_latest.png
e2e_render: luminance gate (batch 6) — 100.0% of the frame is non-black (luminance > 2); threshold 95%
e2e_render: region luminance — emissive 247.0, solid(GI-lit diffuse) 242.0, sky 145.9
e2e_render: PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, framebuffer read back & non-degenerate, per-batch region gate green through camera motion, every pipeline created cleanly, every expected render-graph node dispatched.
```

Track-A regression verdict: **none**. `GridPreset::Default` path is byte-identical (same 32 chunks / 1920 blocks / 7232 voxel-u32s as pre-Track-A; same emissive 247.0 / solid 242.0 / sky 145.9 luminance regions).

## Decisions made during implementation

### Decision A — `dot_vox::Material.id` mapping convention

The design's pseudocode (`02a-design-vox-loading.md` `## K-means stage — NOT
applied to .vox`) wrote material lookup as `materials.get(i)` (positional
index). When implementing, I checked `dot_vox v5.2.0`'s actual convention:
`dot_vox/src/lib.rs:96-115` (placeholder test) generates materials with
`id: i for i in 0..256` (0-based, **matching the in-memory palette index
directly** — same as `Voxel.i`). The crate's `dot_vox_data.rs:167-175`
`write_materials` writes `material.id` raw, so a round-tripped file's material
ids match the in-memory ones. I switched the lookup from positional indexing
to `materials.iter().find(|m| m.id as usize == i)` for robustness against
re-ordered material lists (the `.vox` spec doesn't promise order). Rationale
recorded in the doc-comment at `vox_import.rs:280-285`. The 4 emissive tests
all pass against this convention.

### Decision B — Test fixtures synthesized in-memory, not committed as binaries

The design (`02a-design-vox-loading.md` `## File-by-file change list` → New
files) called for two checked-in `.vox` binary fixtures
(`single_voxel.vox`, `small_cube.vox`) consumed via `include_bytes!`. I chose
instead to synthesize the fixtures in-memory using
`dot_vox::DotVoxData::write_vox` and parse them back with `parse_vox_bytes`.
This still exercises the binary parser path end-to-end (the round-trip writes
real `.vox` bytes and reads them back through `dot_vox::load_bytes`) without
committing opaque binary blobs that would drift if the `dot_vox` writer
changed. `02a-design-vox-loading.md` Risk #12 ("the 8×8×8 small-cube fixture's
binary contents drift if MagicaVoxel updates its writer") goes away under this
approach.

Test helper functions live alongside the tests:
- `build_single_voxel()` — `vox_import.rs:485-505`
- `build_small_cube()` — `vox_import.rs:508-548`
- `default_materials()` — `vox_import.rs:551-562`
- `round_trip()` — `vox_import.rs:565-572`

### Decision C — `DenseVolume` derives `Debug`

`Cargo build` initially failed because `ImportedVox: Debug` couldn't be
derived (`DenseVolume` wasn't `Debug`). Added `Debug` to `DenseVolume`'s
derive at `aadf/construct.rs:34`. One-line, no semantic change, lets the test
`panic!("got {:?}", result)` patterns compile cleanly. Recorded under
"Changes by file" above.

### Decision D — `app.insert_resource(args.clone())` instead of cascading borrow refactor

`build_app_with_args` consumes `args: AppArgs` by value. With `Copy` dropped,
the existing `app.insert_resource(args)` move at `lib.rs:469` invalidates the
subsequent `if args.spawn_test_entity` / `if args.resize_test` reads at
`lib.rs:573-580` / `lib.rs:625`. Two choices: (a) clone the args once at the
insert site; (b) re-order the function so the field reads happen before the
move. (a) is one line; (b) ripples through 100+ lines of plugin wiring. Chose
(a) — `args.clone()` is cheap (PathBuf is the only heap allocation) and it's
once at app boot, not per-frame. Recorded as a minimal Design Assumption #5
realisation.

## Deviations from design

### Deviation 1 — In-memory fixtures (instead of `assets/test/*.vox`)

**Design said:** check in two binary `.vox` fixtures at
`crates/bevy_naadf/src/assets/test/single_voxel.vox` (≈200 B) and
`crates/bevy_naadf/src/assets/test/small_cube.vox` (≈5 KB), include them via
`include_bytes!`.

**What was done:** Synthesized `DotVoxData` structs in-memory inside the test
module via `dot_vox::DotVoxData::write_vox` → bytes → `parse_vox_bytes`. No
binary files committed.

**Why:** Equivalent coverage of the binary parse path with zero binary-blob
drift risk (Design Risk #12 goes away). Future fixtures from real MagicaVoxel
can still be added via `include_bytes!` — the public `parse_vox_bytes(&[u8])`
API supports both. No `assets/test/` directory was created.

### Deviation 2 — `Material.id` lookup is by `m.id` not by index

See Decision A above. The design's pseudocode showed `materials.get(i)` /
`materials[i]`. The implementation uses `materials.iter().find(|m| m.id ==
palette_index)` instead. This is **functionally equivalent** when
`materials[i].id == i` (the `dot_vox` convention, verified in
`dot_vox/src/lib.rs:96-115`) and is more robust against re-ordered inputs.

## Risks / known issues / follow-up

1. **`.vox` fixture testing is synthetic.** Only test-author-controlled
   `DotVoxData` structs go through the parser. Real-world MagicaVoxel `.vox`
   files (with `nTRN`/`nGRP`/`nSHP` scene graphs and `IMAP` reordering, see
   Design Assumption #10 / Risk #1) haven't been parsed yet. **The user's
   manual visual gate is the canary.**
2. **Identity-transform-only scene graph walk (Design Decision 6).** A `.vox`
   file with multiple models grouped under non-trivial `nTRN` transforms will
   render every model at the origin, AABBs composed. Follow-up: port C#
   `MagicaVoxel.CollateVoxelData` (`MagicaVoxel.cs:718-770`) — the seam is
   internal to `flatten_scene`; no public API change needed.
3. **`Material.id` lookup falls back to default values** if the file lacks a
   `MATL` chunk for a palette index. Behaviour: that palette entry is treated
   as diffuse (`emit = flux = 0`). Mirrors C# default-zero behaviour.
4. **`IMAP` chunk handling** — `dot_vox::DotVoxData::index_map` is parsed but
   NOT applied in `vox_palette_to_voxel_types`. Per Design Assumption #10,
   files where the user reordered palette entries in MagicaVoxel's editor will
   render with wrong colors per voxel. Mitigation: apply `data.index_map[voxel.i]`
   as the palette lookup index (one extra `[]` indirection inside
   `vox_palette_to_voxel_types`). Not blocking design; flip-trigger is a real
   fixture exposing it.
5. **`AppArgs` lost `Copy`** — propagates a tiny ergonomic cost (one explicit
   `.clone()` at `lib.rs:471`). Recorded under Design Assumption #5; verified
   no other call sites break.
6. **`dot_vox 5.2` requires edition 2024.** Builds fine on `rustc 1.95.0` (the
   workspace's pinned toolchain); listed for awareness only.
7. **Two `nom` versions in the tree** (v7.1.3 and v8.0.0 — `dot_vox` pulled
   v8 in). Both compile cleanly; cargo carries them side-by-side. Acceptable.

## What the user should manually verify

The implementer's terminal gate is `cargo build && cargo test && cargo run
--bin e2e_render` — all green per the Verification section above. The
**visual** gate is the user's, per global memory
`subagent-gpu-app-verification-loop`:

- [ ] **Production binary boots with the default test grid:**
      `cargo run --bin bevy-naadf` should render the expanded test scene
      (ground + towers + arch + emissives + spheres) — byte-identical to
      pre-Track-A.
- [ ] **Production binary loads a real `.vox` file:** pick a small `.vox`
      authored in MagicaVoxel ≤ 64³ voxels (anything reasonable will do), then
      `cargo run --bin bevy-naadf -- --vox /absolute/path/to/your.vox`.
      Expectations:
      - Console log line `NAADF .vox loaded from .../your.vox: N palette entries, [cx, cy, cz] chunks`.
      - A non-test-grid world renders in the on-screen window.
      - Voxels appear at the correct positions (note: NAADF is Y-up; `.vox` is
        Z-up; the Z↔Y swap is automatic and matches C# `ModelData.cs:386`).
      - Emissive materials (any `_emit > 0` in the `.vox` `MATL` chunks)
        render bright pre-GI; their bounce-light shows up after GI settles.
- [ ] **Bad path gracefully falls back:**
      `cargo run --bin bevy-naadf -- --vox /nonexistent.vox` should log an
      `error!` line and boot into the default test grid (no panic, no crash).
- [ ] **Multi-model `.vox` files with transforms render wrong** (expected per
      Design Decision 6 — first cut is identity-only). If this is a blocker
      for any specific shipping fixture, that's the flip-trigger for the
      follow-up transform-composition pass.
- [ ] **Existing e2e modes still pass** (the implementer ran the baseline; the
      other three are sanity-checks):
      - `cargo run --bin e2e_render` (baseline — already PASS above)
      - `cargo run --bin e2e_render -- --validate-gpu-construction`
      - `cargo run --bin e2e_render -- --edit-mode`
      - `cargo run --bin e2e_render -- --entities`
