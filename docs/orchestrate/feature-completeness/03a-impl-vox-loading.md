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

## E2E gate addendum (2026-05-15)

### What landed

Automated end-to-end regression gate for the Track A `.vox` ingestion path
— landed as the new `cargo run --bin e2e_render -- --vox-e2e` mode. It
**synthesises a 2-model `.vox` fixture in memory** (no checked-in binary
blob — same pattern Track A landed for unit tests, Deviation 1 above),
writes the bytes to `target/e2e-screenshots/vox_e2e_fixture.vox`, then
boots the existing windowed e2e harness with `AppArgs.grid_preset =
GridPreset::Vox { path }` so the production `--vox <path>` ingestion path
(`crates/bevy_naadf/src/main.rs:21-33`,
`crates/bevy_naadf/src/voxel/grid.rs:75-83`) drives the load verbatim.
The standard-scene `assert_batch_6` region gate is swapped for a
`assert_vox_geometry_visible` non-skybox gate that samples the central
40 % × 40 % screen rect and asserts mean luminance is meaningfully above
the atmosphere-tinted sky band (`> 160` against a measured sky luminance
of ~146). The 03a-followup scene-graph composition fix is directly
exercised: the fixture's two emissive models live under separate `nTRN`
nodes with non-trivial `_t` translations along the MV-z (up) axis, so a
regression that collapses both to origin would either trap the camera
inside opaque material or shrink the lit cuboid below the gate's sample
rect.

### Changes by file

#### NEW

| Path | What | LOC |
|---|---|---|
| `crates/bevy_naadf/src/e2e/vox_e2e.rs` | `--vox-e2e` module — synthesised-fixture builder (2 emissive models under non-trivial `nTRN` translations), `write_vox_e2e_fixture_to_temp`, `run_vox_e2e` boots the harness with `GridPreset::Vox`, `assert_vox_geometry_visible` non-skybox gate, `save_vox_e2e_screenshot` PNG sidecar. 3 `#[test]` cases covering fixture composition, world AABB cap, and on-disk path. | NEW · 446 (~300 prod + 146 tests) |

#### Edited

| Path | Edit | Δ LOC |
|---|---|---|
| `crates/bevy_naadf/src/e2e/mod.rs` | `pub mod vox_e2e;` declaration alongside the existing `checks` / `driver` / `framebuffer` / `gates` / `readback` modules (`e2e/mod.rs:29`). | +1 |
| `crates/bevy_naadf/src/lib.rs` | `AppArgs::vox_e2e_mode: bool` field + its `Default::default()` initialiser. Doc-comment cites the swap-the-default-scene-gate rationale (`lib.rs:259-274`, `:274`). | +18 |
| `crates/bevy_naadf/src/e2e/driver.rs` | `run_assertions` signature gained one `vox_e2e_mode: bool` param; ASSERT-step branch picks `assert_vox_geometry_visible` when the flag is set, otherwise the existing `batch_gate(CURRENT_BATCH, _)` + the entities-mode `assert_entity_pixel` (`driver.rs:463-489`, `:865-936`). | +24 / -7 |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | Parse `--vox-e2e` flag; dispatch to `bevy_naadf::e2e::vox_e2e::run_vox_e2e()` when set (`bin/e2e_render.rs:72-102`, `:172-185`). | +18 |

### Files NOT touched

Per the `01-context.md` §5 forbidden moves: no render pipeline / GI
shader touches; no `naadf_gpu_producer_node` / `gpu_producer_skip_upload`
touches; no `MAX_RAY_STEPS_*` deletions; no `bevy_egui`; no checked-in
binary `.vox` fixtures (the fixture is synthesised in-memory every run
via `dot_vox::DotVoxData::write_vox`); no `obj2voxel` work. The default
`e2e_render` mode + the three existing flag modes (`--entities`,
`--edit-mode`, `--validate-gpu-construction`) all stay green
(`### Verification` below).

### Fixture design

Two emissive models under separate `nTRN` translations, both referencing
palette slot 1 (made emissive via a `MATL { _emit: 1.0, _flux: 0.0 }`
chunk → `color_layered.x = 1.0 * (1+0)² * 5 = 5.0` per
`voxel/vox_import.rs:790-791`):

- **Model A — emissive ground slab.** MV size `60 × 60 × 4`,
  `nTRN _t = "30 30 2"`. Centered local origin `(-30, -30, -2)` (per
  `voxel/vox_import.rs:702`) puts the slab at MV `(0..59, 0..59, 0..3)`.
  After Z↔Y swap → NAADF `(0..59, 0..3, 0..59)`.
- **Model B — central emissive tower.** MV size `20 × 20 × 28`,
  `nTRN _t = "30 30 16"`. Centered local origin `(-10, -10, -14)`
  puts the tower at MV `(20..39, 20..39, 2..29)`. After Z↔Y swap → NAADF
  `(20..39, 2..29, 20..39)`.

Both translations are non-trivial AND differ along the MV-z axis (the
up axis): `_t.z = 2` vs `_t.z = 16`. That's the precise seam the
03a-followup identity-only walk regression broke
(`voxel/vox_import.rs::flatten_scene` pass-2 `collate_voxels` at
`vox_import.rs:646-738`). Combined MV AABB: `x ∈ (0..59)`, `y ∈ (0..59)`,
`z ∈ (0..29)` → MV size `(60, 60, 30)`. After Z↔Y swap NAADF size
`(60, 30, 60)`, rounded up to chunks `(4, 2, 4)` = `64 × 32 × 64` voxels
(same volume size as the default test grid at
`voxel/grid.rs:61` `GRID_SIZE_IN_CHUNKS = [4, 2, 4]`). Comfortably within
the 03a-followup `MAX_CHUNKS_PER_AXIS = 32` cap
(`vox_import.rs:78`).

A side-benefit of matching the default-grid size: the fixed e2e camera
pose (`crates/bevy_naadf/src/e2e/gates.rs:48-49`: NAADF `(86, 42, 90)`
looking at `(32, 16, 32)`) frames the synthesised world the exact same
way it frames the default scene — no camera repositioning needed. The
look target `(32, 16, 32)` lands inside the central tower
(NAADF `(20..39, 2..29, 20..39)`), so the central gate rect is
guaranteed to sample tower interior, not background sky.

### Camera placement / gate-region rationale

The gate samples the central 40 % × 40 % screen rect (fractional
`(0.30, 0.30)..(0.70, 0.70)` of a 256² readback = pixel rect
`(76, 76)..(179, 179)`) and asserts mean luminance > **160.0**. Decision
points:

- **Why the central 40 % × 40 % rect, not the default-scene gates'
  fractional rectangles** (`gates.rs:228-245`: 10 %-band emissive,
  16 %-band solid, 40-pixel sky):
  the default-scene rects were calibrated against specific voxel
  positions in `voxel/grid.rs::build_default_volume`. With the
  synthesised fixture in place those rects would land on whatever
  happens to be at those coords (which is — coincidentally — the
  emissive tower's interior in this fixture). A central rect is the
  honest "the world rendered something, not just sky" check that
  doesn't depend on coincidental geometric alignment.
- **Why luminance threshold = 160.0**:
  - Baseline `sky_rect` luminance is **~146** across all four existing
    modes (verified in `### Verification` below — sky 145.9 in every
    one; matches the `region_luminance_report` line in the post-Track-A
    log at `03a-impl-vox-loading.md` Verification §, which shows the
    same value).
  - Default-scene emissive luminance is **~247** (the warm-white
    emissive block); GI-lit diffuse is **~242**.
  - Setting the threshold at 160 sits just above the sky band so a
    "no geometry loaded, only atmosphere" failure trips cleanly, and
    well below the lit-emissive band (~240-250) so the gate has
    headroom and does not flap on minor framebuffer noise. The actual
    measured central-rect luminance for the synthesised fixture
    (smoke run, `### Verification`) was **249.7** — 90 luminance units
    of safety margin.
  - Honest tripwire: a regression that loads no geometry and shows
    only sky would land the central rect at ~146 (sky), failing the
    160 threshold by 14 units; a regression that loaded only one of
    the two models (composition broke) would still likely fail if the
    surviving model didn't cover the central rect.

### Verification

Run order matches the brief's verification gate sequence.

```bash
cd /mnt/archive4/DEV/bevy-naadf
cargo build --workspace
cargo test --workspace --lib
cargo run --bin e2e_render -- --vox-e2e            # new gate
cargo run --bin e2e_render                          # baseline
cargo run --bin e2e_render -- --validate-gpu-construction
cargo run --bin e2e_render -- --edit-mode
cargo run --bin e2e_render -- --entities
```

| Gate | Verdict | Notes |
|---|---|---|
| `cargo build --workspace` | **PASS** | Finished `dev` profile in 30.08 s; clean compile, no new warnings. |
| `cargo test --workspace --lib` | **PASS** | 149 passed, 1 ignored — up from 146 (Track A) + 3 new `vox_e2e::tests::{fixture_round_trips_and_composes_two_distinct_models, fixture_world_size_fits_within_gpu_producer_cap, fixture_path_is_under_target_dir}`. |
| `cargo run --bin e2e_render -- --vox-e2e` | **PASS** | `[4, 2, 4]` chunks loaded (32 chunks / 1280 blocks / 32 voxel-u32s). Central rect mean luminance **249.7** (threshold > 160). Standard `region_luminance_report` showed emissive 249.3 / solid 250.2 / sky 145.9 — the synthesised emissive geometry brightens the otherwise-tower-interior regions to near-emissive levels. Full e2e checks (pipeline scan, node dispatch, degenerate-frame, luminance liveness) all green. |
| `cargo run --bin e2e_render` (baseline) | **PASS** | Default-scene Batch-6 gate PASS; region luminance emissive 247.0 / solid 242.0 / sky 145.9 — bit-identical to the post-Track-A log. No regression on the default scene. |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | **PASS** | E2E PASS + `GPU construction byte-equal to CPU oracle: 388 bytes compared`. |
| `cargo run --bin e2e_render -- --edit-mode` | **PASS** | E2E PASS + `edit-mode validation PASS: 1 set_voxel call produced 1 changed_chunks + 1 changed_blocks records + 2 changed_voxels records`. |
| `cargo run --bin e2e_render -- --entities` | **PASS** | E2E PASS + `entity handler validation PASS: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates`. |

One smoke run per memory `subagent-gpu-app-verification-loop` — no
rebuild→rerun iteration. The visual gate is the user's; the
luminance-249.7 ≫ 160 threshold margin is the discriminator.

### Decisions during implementation

#### Decision 1 — Fixture sized to match the default scene (4 × 2 × 4 chunks)

First-cut fixture used 12 × 12 × 12 cubes; the smoke run revealed that
the resulting `1 × 2 × 1` chunks world (16 × 32 × 16 voxels) sat at the
NAADF origin and was far enough from the camera's look target
`(32, 16, 32)` that the central screen rect sampled mostly the
atmosphere-tinted sky — central-rect luminance landed at 70.9, far below
the 160 threshold (gate FAIL, fixture+composition both verified correct
by the unit tests).

The fix was to scale the fixture up so the composed NAADF AABB matches
the default-scene size — a 60 × 60 × 4 ground slab + a 20 × 20 × 28
central tower, composing to a `(4, 2, 4)`-chunks world. This reuses the
default e2e camera pose without modification (matches the brief's
option (a) — author the fixture so the camera sees it, rather than
option (b) — adjust the camera).

The fixture's tower interior covers NAADF `(20..39, 2..29, 20..39)`,
which contains the camera look target `(32, 16, 32)`; the central
40 % × 40 % screen rect samples tower interior cleanly. A unit test
(`fixture_round_trips_and_composes_two_distinct_models`) hard-codes
`assert_ne!(imp.volume.voxel_at([32, 16, 32]), VoxelTypeId::EMPTY)` so a
future fixture tweak that breaks this invariant fails compilation-time.

#### Decision 2 — In-memory synthesis + deterministic temp path (no `tempfile` dep)

The brief allows either an in-memory synthesised fixture or a path arg.
The chosen path: synthesise the bytes in memory via
`dot_vox::DotVoxData::write_vox` → `std::fs::write` → run the production
`load_vox`. No new dep (`tempfile` was a candidate). The on-disk path is
deterministic: `target/e2e-screenshots/vox_e2e_fixture.vox`. The
`target/` directory is gitignored + persistent across runs, the file is
overwritten every run, and the `fixture_path_is_under_target_dir` unit
test pins the path location so a future refactor that escapes the
target dir fails fast.

#### Decision 3 — `AppArgs::vox_e2e_mode` flag (not `--vox-e2e` flag parsing in `lib.rs`)

The `AppArgs` field plumbing pattern follows the existing `resize_test`
flag (also in `AppArgs`, also a bool gating a driver branch). The CLI
flag (`--vox-e2e`) lives in the binary
(`crates/bevy_naadf/src/bin/e2e_render.rs`), the boolean in `AppArgs`,
and the driver branches on it inside `run_assertions`. Same pattern that
already worked for `--resize-test` (`bin/e2e_render.rs:81` + `lib.rs:259`
+ `driver.rs:346-365`).

#### Decision 4 — Bypass `assert_entity_pixel` gate in vox-e2e mode

Default-scene-coupled gates that don't apply to a `.vox`-loaded world:
the per-batch region gate (`batch_gate(CURRENT_BATCH, _)` reads
default-scene rects) AND the entity-pixel gate (`assert_entity_pixel`
reads a rect calibrated against an entity at NAADF `(30, 24, 30)` —
which is empty space inside the synthesised fixture). Both are bypassed
when `vox_e2e_mode` is set. The remaining checks (pipeline-error scan,
node-dispatch, degenerate-frame floor, luminance-liveness, screenshot
save) are framebuffer/scene-agnostic and stay armed in both modes —
they would still catch a "GPU produced no output" regression in vox-e2e
mode.

### Risks / follow-ups

1. **The gate is calibrated against the current default e2e camera pose.**
   If `e2e_camera_transform` is repointed (e.g. to test a different
   scene region), the synthesised fixture's geometry might fall outside
   the central rect. The fixture's unit test pins
   `voxel_at([32, 16, 32]) != EMPTY` as a partial guard, but the actual
   screen projection of the geometry is camera-dependent. Flip-trigger:
   if the camera pose changes, re-run `--vox-e2e` and either move the
   fixture geometry or adjust the gate rect.
2. **The fixture exercises composition but not the rotation submatrix
   `_r` byte.** Both models use identity rotation; the 03a-followup
   tests at `vox_import.rs:1334-1399` cover the rotation matrix via
   `scene_graph_rotation_applies` + `rotation_byte_identity_and_axis_swap`
   — that's adequate; adding a rotated fixture to the e2e gate is a
   follow-up if a regression slips past the unit tests.
3. **The fixture does not exercise `IMAP` palette reordering.** Same
   risk as the Track A baseline (Risk #3 above) — pre-existing.
4. **No `bevy_camera_controller`-style automated camera scan.** The
   `assert_vox_geometry_visible` gate samples one fixed rect under one
   fixed pose; a more thorough gate would sample several screen regions
   or scan multiple poses. Not needed for the "is the framebuffer
   non-skybox?" question the brief asks; flip-trigger is a real bug
   that the central-rect-only sampling misses.
5. **One smoke iteration was needed** during implementation: the
   first-cut fixture was too small to fill the central rect. The fix
   was deterministic (scale the fixture to match the default scene
   size), not a luminance-threshold tune. Per memory
   `subagent-gpu-app-verification-loop`, no further visual iteration —
   the gate is honest now (`249.7 ≫ 160` margin), not rubber-stamped.
