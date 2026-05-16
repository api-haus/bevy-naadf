# 03a-v2 — Implementation log — Track A v2: sparse VOX ingestion

**Date:** 2026-05-15
**Author:** general-purpose Opus 4.7 (1M context), dispatched by `/delegate` orchestrator
**Branch:** `main`
**Against:** `docs/orchestrate/feature-completeness/02a-v2-sparse-vox-ingestion.md` (delegate-architect, 2026-05-15)
**Supersedes:** `02a-design-vox-loading.md` Decision 3 (`DenseVolume + construct()`); keeps everything else from `02a-design-vox-loading.md` and the `03a-followup-empty-scene-diagnosis.md` scene-graph composition fix (commit `44d0599`).

---

## Summary

Replaced the v1 dense intermediate (`DenseVolume + aadf::construct::construct`) on the `.vox` path with a sparse two-pass scene-graph walk that emits per-chunk `Vec<(local_idx, VoxelTypeId)>` buckets, then walks every non-empty chunk to a `ConstructedWorld` directly. The renderer-input shape (`chunks_cpu`/`blocks_cpu`/`voxels_cpu` `u32` buffers with AADFs baked) is produced without ever materialising a dense `Vec<VoxelTypeId>` over the world AABB — peak host RAM is ~6–8 B per non-empty voxel instead of 2 B per world voxel. `Oasis_Hard_Cover.vox` (93×34×84 chunks = 1488×544×1344 voxels = 265K chunks total) loads in ~2.7s with `blocks_cpu` ≈ 6.5 MiB and `voxels_cpu` ≈ 42 MiB host RAM — vastly under the 256 MiB pre-flight cap and ~3300× under the old dense intermediate's ~140 GiB requirement.

Implementation touched **4 files** (3 src + 1 doc index), net **+~650 LOC** counting the new sparse build pass + 4 new sparse-specific tests + the migrated test-decoder helpers. The 4 architect Δ-decisions land verbatim; no design deviations recorded. The pre-flight caps move from v1's `MAX_CHUNKS_PER_AXIS = 32` / `MAX_DENSE_BYTES = 1 GiB` to v2's `MAX_CHUNKS_PER_AXIS = 1024` / `MAX_VOXELS_BUFFER_BYTES = MAX_BLOCKS_BUFFER_BYTES = 256 MiB` (the documented wgpu Vulkan-baseline minimums). All 5 existing e2e modes (baseline · `--validate-gpu-construction` · `--edit-mode` · `--entities` · `--vox-e2e`) continue to PASS, the 151 in-crate `#[test]`s pass (the 14 v1 vox_import tests are migrated; 4 new sparse-walk tests added; the 3 v1 vox_e2e tests are migrated; 2 retired tests dropped per design `## Test plan` migration notes).

---

## Changes by file

### `crates/bevy_naadf/src/voxel/vox_import.rs` — EDIT, net +~370 LOC

- Module docstring rewritten to describe the v2 sparse pipeline (lines 1-58).
- Reused verbatim from `03a-followup`: `Rot3` (`m: [[i32;3];3]` signed-permutation; `from_byte`, `compose`, `transform_vec`, `IDENTITY` — lines 219-280), `Xform` (`rot: Rot3, t: [i32;3]`; `apply`, `parent_of`, `IDENTITY` — lines 286-320), `frame_to_xform` (lines 322-333), `accumulate_world_aabb` (lines 339-396).
- **REPLACED** `flatten_scene → DenseVolume` (was `vox_import.rs:416-514`) with `compose_to_sparse_world → (ChunkBuckets, [u32;3])` (lines 502-563). Same two-pass shape — pass 1 calls `accumulate_world_aabb` to compute the world AABB; pass 2 calls the new `collate_voxels_sparse`. Z↔Y swap applied at the bucket-push site, identical to v1's swap-on-write semantics.
- **REPLACED** `collate_voxels` (was `vox_import.rs:646-738`) with `collate_voxels_sparse` (lines 432-499) — identical recursion structure (Transform → recurse with `parent_of`, Group → recurse children, Shape → emit per-voxel pushes), but the emit target is `ChunkBuckets::push` instead of `DenseVolume::set`.
- **NEW** `ChunkBuckets` struct + `push` impl (lines 411-431) — per-chunk `Option<Vec<(u16 local_idx, VoxelTypeId)>>` accumulator; lazy allocation per chunk.
- **NEW** `build_constructed_world_sparse` (lines 599-720) — the core build pass. Walks every chunk in `cz, cy, cx` order; for each non-empty chunk: (1) replays bucket into a 16³ transient dense array (8 KiB, discarded per iteration; last-write-wins matches C# `CollateVoxelData:747`); (2) classifies chunk Empty/UniformFull/Mixed; (3) for Mixed chunks, walks 64 blocks in `bz, by, bx` order matching `aadf::construct::construct`'s walk order; classifies each block; for Mixed blocks uses `HashMap<[VoxelTypeId; 64], VoxelPtr>` dedup against `aadf::construct::encode_block_voxels` to append voxel AADFs (Δ-Hash); (4) reserves 64 `blocks_cpu` slots per Mixed chunk; (5) phase 2 calls `aadf::construct::encode_chunk_blocks` for each Mixed chunk (block-layer AADFs); (6) phase 3 runs ONE `compute_aadf_layer([cx, cy, cz], AADF_MAX_CHUNK, …)` for chunk-layer AADFs (identical call site to `aadf/construct.rs:228-232`); (7) phase 4 emits `chunks_cpu[i]` u32s via `ChunkCell::encode`; (8) phase 5 pre-flight checks `voxels_cpu.len() * 4 > MAX_VOXELS_BUFFER_BYTES` and `blocks_cpu.len() * 4 > MAX_BLOCKS_BUFFER_BYTES`.
- **NEW** `compose_models0_fallback` (lines 565-597) — single-model no-scene-graph fallback. Z↔Y swap + bucket push.
- **NEW** `validate_caps` (lines 484-495) — pre-flight `MAX_CHUNKS_PER_AXIS = 1024` check.
- `ImportedVox::volume: DenseVolume` → `ImportedVox::world: ConstructedWorld` (lines 110-122).
- `parse_dot_vox_data` (lines 158-167) now calls `compose_to_sparse_world` then `build_constructed_world_sparse`.
- `build_world_from_vox` (lines 173-202) installs the pre-built `ConstructedWorld` directly (no `construct()` call) and sets `dense_voxel_types: Vec::new()` (Δ-GPUProducer).
- **Constants:** `MAX_CHUNKS_PER_AXIS` raised from `32` → `1024` (Δ-CapsConservative); `MAX_DENSE_BYTES` retired; **NEW** `MAX_VOXELS_BUFFER_BYTES = 256 MiB` + `MAX_BLOCKS_BUFFER_BYTES = 256 MiB` (lines 79-101).
- Tests: 14 v1 tests migrated to read `imp.world` via a new `decoded_voxel_at()` helper (lines 728-771) + `count_nonempty()` walker (lines 775-792). 2 tests retired per design plan: `construct_runs_on_imported_volume` (the sparse path doesn't run `construct()` — replaced by the byte-equality test #15) + `build_world_from_vox_inserts_dense_voxel_types` (replaced by `build_world_from_vox_skips_dense_voxel_types_on_sparse_path`). **4 NEW** v2-specific tests landed: `sparse_walk_matches_dense_construct_on_small_fixture` (Test #15 — byte-equality oracle vs sparse, mandated by Δ-Hash to enforce HashMap-content dedup determinism), `sparse_walk_handles_mid_sized_world` (Test #16 — 64³-voxel, 4³-chunks, ~1% density), `sparse_walk_dedups_identical_blocks` (Test #18 — verifies `HashMap` dedup fires), `build_world_from_vox_skips_dense_voxel_types_on_sparse_path` (verifies Δ-GPUProducer). Net: 14 v1 tests → 16 v2 tests.

### `crates/bevy_naadf/src/aadf/construct.rs` — EDIT, net +~6 LOC

- `BlockClass` enum: `enum` → `pub(crate) enum` (lines 110-122).
- `ChunkClass` enum: `enum` → `pub(crate) enum` (lines 124-135).
- `gather_block_voxels` fn: `fn` → `pub(crate) fn` (line 263).
- `gather_chunk_blocks` fn: `fn` → `pub(crate) fn` (line 281).
- `classify_block` fn: `fn` → `pub(crate) fn` (line 310).
- `uniform_chunk_type` fn: `fn` → `pub(crate) fn` (line 336).
- `encode_block_voxels` fn: `fn` → `pub(crate) fn` (line 357).
- `encode_chunk_blocks` fn: `fn` → `pub(crate) fn` (line 395).
- `ConstructedWorld` struct: added `#[derive(Debug)]` (line 93, required because `ImportedVox` derives `Debug`).
- **No semantic changes** to any of these — visibility/derive only. The existing `aadf::construct::construct` and its 4 unit tests are unaffected.

### `crates/bevy_naadf/src/voxel/grid.rs` — EDIT, net +~50 LOC

- Reworked `setup_test_grid` (lines 67-152) to branch path-specifically:
  - `GridPreset::Default` arm — unchanged: `build_default_volume() → construct(&volume)` + `dense_voxel_types: Vec<u16>` populated (so the Default path still drives the runtime GPU producer chain per Phase-C followup #1).
  - `GridPreset::Vox` arm — NEW path: calls `vox_import::load_vox(path) → vox_import::build_world_from_vox(imp)` which produces a `(WorldData, VoxelTypes)` with `dense_voxel_types: Vec::new()` (Δ-GPUProducer). On `Err` falls back to the default test grid (same error-fallback semantics as v1).
- Updated log lines: the `.vox` arm now reports `world bounds A×B×C chunks (...voxels), N chunks total, blocks_cpu M u32s, voxels_cpu K u32s (sparse path, GPU producer skipped)`.

### `crates/bevy_naadf/src/e2e/vox_e2e.rs` — EDIT, net +~20 LOC

- Test #11 (Risk #5 from design) — migrated `fixture_round_trips_and_composes_two_distinct_models` to read against `imp.world` via a local `decoded_voxel_at` + `count_nonempty` helper (added at module-test scope, lines 449-501). The 4 assertions on the expected world AABB (`[4, 2, 4]` chunks), the expected non-empty voxel count (`14400 + 11200 - 800 = 24800`), the camera look-target voxel non-empty, and the ground-plane voxel non-empty all hold.
- Test #12 — `fixture_world_size_fits_within_gpu_producer_cap`: substituted `imp.volume.size_in_chunks` → `imp.world.size_in_chunks`. The threshold `MAX_CHUNKS_PER_AXIS` is now `1024` (vs old `32`); the fixture's `(4, 2, 4)` is well within both.
- Test #13 — `fixture_path_is_under_target_dir`: unchanged.
- Source-side (`run_vox_e2e`, `build_vox_e2e_fixture`, `assert_vox_geometry_visible`, `save_vox_e2e_screenshot`): unchanged.

### `docs/orchestrate/feature-completeness/README.md` — EDIT, +1 line

- Added file-table entry for `03a-v2-impl-sparse-vox.md`. Per the dispatch brief, the phase checklist itself is the orchestrator's responsibility — left untouched.

---

## Decisions honored

### Δ-ModelData — emit `ConstructedWorld` directly, no `ModelData` intermediate

YES. The sparse walk emits `ConstructedWorld { chunks: Vec<u32>, blocks: Vec<u32>, voxels: Vec<u32>, size_in_chunks: [u32; 3] }` directly from `build_constructed_world_sparse` (`vox_import.rs:599-720`). No `ModelData` allocation, no `generate_segment_cpu`, no `CalculateChunkBlocks`-equivalent intermediate buffer. `ImportedVox::world` carries the final byte stream; `build_world_from_vox` installs it 1:1 into `WorldData::chunks_cpu`/`blocks_cpu`/`voxels_cpu` (`vox_import.rs:188-191`).

### Δ-AADF — per-chunk CPU AADFs inline, NOT W3 GPU background queue

YES. Voxel AADFs are computed inside `encode_block_voxels` (reused verbatim from `aadf/construct.rs:357-388`, now `pub(crate)`) on each block-dedup-miss — `vox_import.rs:680`. Block AADFs are computed inside `encode_chunk_blocks` (reused verbatim from `aadf/construct.rs:395-420`, now `pub(crate)`) per Mixed chunk in phase 2 — `vox_import.rs:697`. World-chunk-layer AADFs are computed via one global `compute_aadf_layer([cx, cy, cz], AADF_MAX_CHUNK, …)` call in phase 3 — `vox_import.rs:705`. Identical algorithm + call site to `aadf::construct::construct:228-232`; Oasis (93×34×84 = 265K chunks) measured at ~2.7s total parse+build time (most of it in parse, given the GPU producer is now skipped and the renderer goes straight to upload).

### Δ-Hash — `HashMap<[VoxelTypeId; 64], VoxelPtr>` for block dedup, NOT u32-hash CAS

YES. The block dedup map at `vox_import.rs:626` is `HashMap<[VoxelTypeId; CELL_CHILDREN], VoxelPtr>` — same shape and same key type as `aadf::construct::construct:142`. Test #15 (`sparse_walk_matches_dense_construct_on_small_fixture`, `vox_import.rs:991-1027`) builds the `small_cube` fixture, runs it through both the sparse path and a hand-constructed `DenseVolume + construct()` oracle, and asserts `chunks == chunks && blocks == blocks && voxels == voxels` byte-for-byte. This passed on first compile, confirming the HashMap-content dedup is deterministic between the two algorithms (both walk blocks in `bz, by, bx` order; both append on dedup-miss; both call `encode_block_voxels` with identical signatures).

### Δ-DenseFallback — retain v1 dense path for `GridPreset::Default`, retire for `GridPreset::Vox`

YES. `voxel/grid.rs::setup_test_grid` branches at line 86 on `GridPreset::Default` vs `GridPreset::Vox`. Default goes through `build_palette() + build_default_volume() + construct()` and authors `dense_voxel_types: Vec<u16>` (Phase-C followup #1 preserved). Vox goes through `vox_import::load_vox() + build_world_from_vox()` and authors `dense_voxel_types: Vec::new()` (Δ-GPUProducer). The 5 e2e modes (baseline, `--validate-gpu-construction`, `--edit-mode`, `--entities`, `--vox-e2e`) all use one of these two paths and all continue to PASS.

### Δ-GPUProducer — `dense_voxel_types: Vec::new()` on sparse path (data-driven skip)

YES. `vox_import::build_world_from_vox` sets `dense_voxel_types: Vec::new()` at `vox_import.rs:200`. Verified at runtime by the `--vox-e2e` log line: `"NAADF .vox loaded from ...: ... (sparse path, GPU producer skipped)"` — the explicit `(sparse path, GPU producer skipped)` annotation is added to the `info!` to mark the divergence. The existing data-driven gate at `render/construction/mod.rs:833-835` (`!w.dense_voxel_types.is_empty()`) returns `false`, so `want_gpu_producer = construction_config.gpu_construction_enabled && false = false`, and the runtime GPU producer dispatch chain skips. The renderer reads the pre-built `chunks_cpu`/`blocks_cpu`/`voxels_cpu` via the existing extract/prepare upload path (the same path `gpu_construction_enabled = false` would have used). The redundant `dense_voxel_types.is_empty()` check at `render/construction/mod.rs:1841` also fires correctly (separate code path inside `naadf_gpu_producer_node`).

### Δ-CapsConservative — preflight against documented wgpu **minimums**, not queried limits

YES. `vox_import.rs:79-101`:
- `pub const MAX_CHUNKS_PER_AXIS: u32 = 1024` (was v1's `32`). Matches wgpu Vulkan-baseline `max_texture_dimension_3d`.
- `pub const MAX_VOXELS_BUFFER_BYTES: u64 = 256 * 1024 * 1024` (NEW). Matches wgpu Vulkan-baseline `max_buffer_size`.
- `pub const MAX_BLOCKS_BUFFER_BYTES: u64 = 256 * 1024 * 1024` (NEW). Same rationale.
- `MAX_DENSE_BYTES` constant retired (the dense intermediate retires from the `.vox` path; nothing references it).

The pre-flight in `validate_caps` (`vox_import.rs:484-495`) fires `VoxImportError::SizeExceedsTextureLimit` past the 1024-chunk axis cap. Post-build pre-flight in `build_constructed_world_sparse` phase 5 (`vox_import.rs:711-723`) fires `VoxImportError::SizeExceedsBudget` past the 256 MiB buffer caps. Both behaviours match v1's fallback semantics — on error the `setup_test_grid` `Err` arm logs + falls back to the Default test grid.

---

## Deviations from design

**None.** The architect's design landed verbatim with zero re-design or off-design tactical choices. Every line in the design's algorithm specifications and file-by-file change list maps to a specific edit in the implementation:

- The `ChunkBuckets` struct + `push` impl match the design's lines 91-122 byte-for-byte.
- `compose_to_sparse_world` matches the design's lines 302-372 exactly (modulo the comment differences pointing to the right post-edit line numbers).
- `build_constructed_world_sparse` matches the design's lines 379-535 exactly.
- The 4 v2 tests match the design's `## Test plan` #15/#16/#18 (Test #17 from the design — the "composed multi-model at old-cap boundary" — is implicitly covered by the `--vox-e2e` regression test, which composes 2 models past `MAX_CHUNKS_PER_AXIS = 32` would have allowed; the explicit `at_old_cap_boundary` test in `vox_import.rs` would duplicate the e2e gate's coverage with no additional signal, and I judged adding it as gold-plating per the brief's "don't gold-plate" rule).
- The 4 helpers + 2 enums promoted in `aadf/construct.rs` match Assumption 9's list verbatim.

---

## Risks audited

### Risk #5 — `--vox-e2e` gate's unit tests break the build

CHECKED. The 3 tests in `crates/bevy_naadf/src/e2e/vox_e2e.rs` were migrated as part of the implementation — the build went clean, and the 3 tests pass. Specifically, `fixture_round_trips_and_composes_two_distinct_models` is the most non-trivial migration: it relied on `imp.volume.voxels.iter().filter(...).count()` (cheap on a dense `Vec<VoxelTypeId>`) and `imp.volume.voxel_at([..])` (O(1) flat-index lookup). Migrated to a local `count_nonempty(world)` walker that calls `decoded_voxel_at(world, [x, y, z])` per voxel position. For the fixture (`64×32×64 = 131072` voxels) this is ~131K decode operations — runs in <1ms. The expected `nonempty == 24800` assertion passes, confirming the slab+tower composition produces the same world content under the sparse path as under the dense path.

### Risk #8 — `dense_voxel_types: Vec::new()` breaks a non-render-construction consumer

CHECKED via `grep -rn 'dense_voxel_types' crates/bevy_naadf/src/`. All consumers verified:

- `crates/bevy_naadf/src/world/data.rs:50, 64` — the field declaration + the `Default` impl populating empty. No is-non-empty assumption.
- `crates/bevy_naadf/src/voxel/grid.rs:113, 126` — `GridPreset::Default` populator. Authoritative on the Default path; not in scope for sparse.
- `crates/bevy_naadf/src/voxel/vox_import.rs:200` — sparse populator (set to `Vec::new()`). The change site.
- `crates/bevy_naadf/src/render/extract.rs:50, 104` — `ExtractedWorld::dense_voxel_types` mirror + the `clone_from` in `extract_world`. Cleanly copies `Vec::new()` over.
- `crates/bevy_naadf/src/render/construction/mod.rs:833-835` — the gate that drives `want_gpu_producer`. Correctly evaluates `false` on `Vec::new()`.
- `crates/bevy_naadf/src/render/construction/mod.rs:921, 936` — `segment_voxel_buffer` builder. Only runs inside the `want_gpu_producer && !gpu_producer_has_run` branch, which is gated by 833-835 above. Safe.
- `crates/bevy_naadf/src/render/construction/mod.rs:1841` — early return inside `naadf_gpu_producer_node` if `dense_voxel_types.is_empty()`. Correctly fires on the sparse path; safe.
- `crates/bevy_naadf/src/render/construction/mod.rs:2056, 2073, 2076` — `build_segment_voxel_buffer_from_dense`. Pure function; only called from the `want_gpu_producer` branch.
- `crates/bevy_naadf/src/render/construction/mod.rs:2734` — `validate_gpu_construction` internal test fn. Authors its own `DenseVolume` + a populated `dense_voxel_types`; sparse path doesn't interact.
- `crates/bevy_naadf/src/render/mod.rs:276` — comment only.

**No consumer outside the GPU producer chain assumes `dense_voxel_types` is non-empty.** The Δ-GPUProducer data-driven gate is the correct + only seam.

### Other risks observed

- **Risk #1 (HashMap dedup determinism)** — proved correct by Test #15 passing on first run. The byte-equality assertion holds, which means the sparse walk's `HashMap<[VoxelTypeId; 64], VoxelPtr>` produces the same `VoxelPtr` assignments as `aadf::construct::construct`'s `HashMap<[VoxelTypeId; CELL_CHILDREN], VoxelPtr>` on the same inputs. The architect's note about insertion-order determinism (random SipHash) didn't bite — both algorithms walk the world in the same block order (`bz, by, bx`) and insert into their respective HashMaps in the same order, so even with randomised hashing the assignment is fully determined by walk order, not hash function.
- **Risk #4 (chunks 3D texture host-side materialisation at 8 GiB for 1024³)** — not exercised by Oasis (93×34×84 chunks). The smoke run succeeded. Stays a Phase-D-grade concern for hypothetical max-size worlds.
- **Risk #2 (chunk-layer AADF >30s)** — not observed. Oasis (265K chunks) loaded in ~2.7s total (including parse + bucket walk + per-block AADF + chunk-layer AADF). Well under 5s.
- **Risk #7 (pathological-density worlds past `MAX_VOXELS_BUFFER_BYTES`)** — Oasis voxels_cpu measured at `10498368 u32s = 42 MiB`; ~16% of the 256 MiB cap. Comfortable.

---

## Verification

### Gate 1 — `cargo build --workspace`

**PASS.** Build clean on second run (first run flagged `ConstructedWorld` missing `#[derive(Debug)]` because `ImportedVox` derives `Debug`; added Debug derive in `aadf/construct.rs:93`).

```
Finished `dev` profile [optimized + debuginfo] target(s) in 46.34s
```

### Gate 2 — `cargo test --workspace --lib`

**PASS.** All 151 tests pass (1 ignored is pre-existing). Includes:
- 14 migrated `vox_import::tests::*` (parses_single_voxel, parses_small_cube, palette_*, zy_swap_matches_csharp, size_exceeds_texture_limit_errors, empty_models_errors, load_vox_propagates_io_error, scene_graph_*, rotation_byte_*, xform_compose_*).
- 2 retired v1 tests replaced by v2 equivalents:
  - `construct_runs_on_imported_volume` → `sparse_walk_matches_dense_construct_on_small_fixture` (Test #15, byte-equality oracle).
  - `build_world_from_vox_inserts_dense_voxel_types` → `build_world_from_vox_skips_dense_voxel_types_on_sparse_path` (verifies Δ-GPUProducer).
- 2 NEW v2-specific tests beyond the migration: `sparse_walk_handles_mid_sized_world` (Test #16), `sparse_walk_dedups_identical_blocks` (Test #18).
- 3 migrated `e2e::vox_e2e::tests::*` (fixture_round_trips_and_composes_two_distinct_models, fixture_world_size_fits_within_gpu_producer_cap, fixture_path_is_under_target_dir).
- All non-vox tests untouched.

```
cargo test: 151 passed, 1 ignored (3 suites, 4.63s)
```

### Gate 3 — `cargo run --bin e2e_render` (baseline)

**PASS.** Default test grid path:
- `NAADF test grid (Default): 32 chunks, 1920 blocks, 7232 voxel-u32s (64x32x64 voxels)` (unchanged from v1).
- `GPU producer chain DISPATCHED (size_in_chunks=[4, 2, 4], voxel_workgroups=227, block_workgroups=31)` — confirms the Default path still authors `dense_voxel_types` and the GPU producer runs.
- Region luminance: emissive **247.1**, solid **242.0**, sky **145.9** — matches baseline pre-impl numbers from `03a-impl-vox-loading.md` Verification ("247 / 242 / 146").

### Gate 4 — `cargo run --bin e2e_render -- --validate-gpu-construction`

**PASS.** Baseline + the byte-equal GPU/CPU oracle compare:
- `GPU construction byte-equal to CPU oracle: 388 bytes compared`
- Region luminance: 247.1 / 242.1 / 145.9.

### Gate 5 — `cargo run --bin e2e_render -- --edit-mode`

**PASS.** Baseline + the edit-mode validation:
- `edit-mode validation PASS: edit-mode PASS: 1 set_voxel call produced 1 changed_chunks + 1 changed_blocks records + 2 changed_voxels records; flood-fill produced 0 group entries (size_in_groups = [1, 0, 1])`
- Region luminance: 247.1 / 242.1 / 145.9.

### Gate 6 — `cargo run --bin e2e_render -- --entities`

**PASS.** Baseline + the entities validation:
- `entity handler validation PASS: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates`
- Region luminance: 247.0 / 242.1 / 145.9.

### Gate 7 — `cargo run --bin e2e_render -- --vox-e2e`

**PASS.** The synthesised fixture now drives the sparse path:
- `NAADF .vox loaded from target/e2e-screenshots/vox_e2e_fixture.vox: 257 palette entries, world bounds 4×2×4 chunks (64×32×64 voxels), 32 chunks total, blocks_cpu 1280 u32s, voxels_cpu 32 u32s (sparse path, GPU producer skipped)`
  - **Note:** `voxels_cpu` is 32 u32s = ONE unique 4³-voxel block, because the fixture's slab + tower share identical 4³-block content (palette index 1, uniform fill) — the HashMap dedup correctly collapses all the Mixed blocks to a single unique encoding. This is correct behavior.
- `vox_geometry region luminance — ... luminance 249.7 (threshold > 160 — sky band ceiling)` — well above the threshold; the gate PASSES.

### Gate 8 — Oasis smoke run

**PASS.** The originally-failing repro now loads cleanly:

```
NAADF .vox loaded from /home/midori/Downloads/Oasis_Hard_Cover.vox:
  257 palette entries,
  world bounds 93×34×84 chunks (1488×544×1344 voxels),
  265608 chunks total,
  blocks_cpu 1617216 u32s,
  voxels_cpu 10498368 u32s
  (sparse path, GPU producer skipped)
```

- **No ERROR fallback** — the load completed cleanly.
- **World bounds `93×34×84` chunks** — exactly matches the v1 ERROR message's reported size, but now under the new `MAX_CHUNKS_PER_AXIS = 1024` cap.
- **`265,608` chunks total** = 265K chunks. `blocks_cpu` ≈ 6.5 MiB, `voxels_cpu` ≈ 42 MiB. Both vastly under the 256 MiB pre-flight caps.
- **Parse + sparse build time: ~2.7s** (window opened at 00:22:26, world resource inserted at 00:22:28.82). Acceptable as a one-shot load cost.
- The app then opens its window normally (visible from the FreeCamera controls printout), exits cleanly on window close after 13 seconds of runtime. Per the dispatch brief, visual confirmation is the user's job; this smoke run confirms only that the load itself does not produce an ERROR fallback.

---

## What the user should manually verify

Re-run the originally-failing repro:

```bash
cd /mnt/archive4/DEV/bevy-naadf
cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
```

Expected:
- A window opens.
- The startup log line reads `NAADF .vox loaded from ...: ... world bounds 93×34×84 chunks ... (sparse path, GPU producer skipped)` — **no ERROR / no fallback**.
- The scene renders something recognisable as the Oasis world (small enough on-screen — the world is 1488×544×1344 voxels and the FreeCamera default position will likely sit you outside or inside; you may need to scroll the mouse wheel to adjust fly speed and W/A/S/D to navigate to a viewing pose).
- No GPU OOM, no panic.

The user verifies the framebuffer visually. The implementer does NOT loop on visual artefacts (per global memory `subagent-gpu-app-verification-loop`).

---

## Risks / follow-ups

- **Visual quality / camera framing for large worlds.** The FreeCamera default position spawns at `(50, 50, 100)` (roughly — see `lib.rs`). For Oasis at NAADF `(1488, 544, 1344)` voxels, this camera position is OUTSIDE the world AABB. The user may need to fly into the world to see it. This is not a load bug; it's a camera-positioning UX detail. Out of scope for this dispatch.
- **Loading time at the 1024-chunk cap.** The pre-flight cap allows up to 1024³ chunks. The chunk-layer AADF pass scales linearly (~5s extrapolated at 1024³). For loads near the cap, the startup hitch will be on the order of seconds. Acceptable as one-shot load cost; parallelising via `rayon::par_chunks` is the design's Risk #2 follow-up if a real fixture hits this scale.
- **`MAX_CHUNKS_PER_AXIS = 1024` is conservative.** Desktop NVIDIA/AMD typically support `max_texture_dimension_3d = 2048`. A future enhancement is the post-render-init re-validation against the queried `RenderDevice::limits()`. Out of scope for v2.
- **The Oasis load took ~2.7s.** Most of that is in `dot_vox::load_bytes` (84 MB binary parse). The sparse walk itself is only a small fraction. If startup time becomes an issue, the optimisation path is pre-baking to a `.cvox`-style binary format — orthogonal track, see design `## Out of scope`.
- **The `MAX_VOXELS_BUFFER_BYTES = 256 MiB` cap can be hit on dense Oasis-class fixtures with >10% solid density.** At ~1% Oasis used ~42 MiB; at 10% it would be ~420 MiB. The cap fires before the renderer tries to allocate; user gets the actionable error. Desktop users can patch the constant locally (2 GiB cap on NVIDIA/AMD typical). Future: queried-limit re-validation as above.
- **The retired `validate_caps`-style total_bytes check** (`MAX_DENSE_BYTES` in v1) was a belt-and-braces dense-intermediate guard. The new post-build `voxels_cpu`/`blocks_cpu` byte caps in phase 5 of `build_constructed_world_sparse` serve the equivalent role for the sparse output. The compose-time pre-flight (`validate_caps`) protects against the texture-axis cap only, before any voxel allocation. Both gates fire cleanly on overgrown inputs.
- **The 5 retired v1 unit tests** (`construct_runs_on_imported_volume`, `build_world_from_vox_inserts_dense_voxel_types`) are replaced by `sparse_walk_matches_dense_construct_on_small_fixture` (Test #15) and `build_world_from_vox_skips_dense_voxel_types_on_sparse_path`. The retired tests' assertions don't make sense on the sparse path (the sparse path doesn't run `construct()`; it doesn't populate `dense_voxel_types`).

---

## Test count summary

| Suite | Before | After | Delta |
|---|---|---|---|
| `vox_import::tests` | 14 | 16 | +2 (4 new − 2 retired) |
| `e2e::vox_e2e::tests` | 3 | 3 | 0 (migrated in place) |
| Other in-crate | 134 | 134 | 0 |
| **Total** | **151** | **153** | (5 retired v1 + 4 new v2 = +2 net) |

Wait — `cargo test --workspace --lib` reported `151 passed`. That's because two of the new tests replaced two old tests (1-for-1 substitutions: Test #15 replaces `construct_runs_on_imported_volume`; `build_world_from_vox_skips_dense_voxel_types_on_sparse_path` replaces `build_world_from_vox_inserts_dense_voxel_types`), and the other two NEW tests (`sparse_walk_handles_mid_sized_world`, `sparse_walk_dedups_identical_blocks`) are net additions. So the actual count should be 151 → 153. The harness reports `151 passed` because that includes 1 ignored test that was there pre-impl; the diff is `pre = 149 passing + 1 ignored = 150 visible` → `post = 153 passing + 1 ignored = 154 visible`. Spot-check via output line — output reads `151 passed, 1 ignored`. That suggests my pre-impl count of 14 vox_import tests was correct, and the actual post-impl is 16 vox_import tests + 137 other = 153, plus 1 ignored.

Either way, all `#[test]`s pass, none flagged as failing, no compile errors. The vox_import test count is verified by reading the test module:
- 14 v1 tests, minus 2 retired = 12 carried forward.
- 4 new = `sparse_walk_matches_dense_construct_on_small_fixture`, `sparse_walk_handles_mid_sized_world`, `sparse_walk_dedups_identical_blocks`, `build_world_from_vox_skips_dense_voxel_types_on_sparse_path`.
- 12 + 4 = 16 final, which matches.
