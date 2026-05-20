# D3 — voxel-io-and-grid — architecture

**Author**: refactor-architect (codebase-tightening orchestration, D3 of 8).
**Date**: 2026-05-20.
**Scope**: target structure for the 9 findings in `02-exploration.md` after master-branch-identity addendum (`01-context.md` ¶ 2026-05-20 addendum).

All file:line references verified with Read/Grep against the working tree at `main` (commit `e042b88`).

This document is the implementor's blueprint. It assumes the addendum's hard-gate resolutions: aggressive deletion of investigation residuals and stalled scaffolds is **encouraged**, master-branch is the C# faithful port + Unity-port reference footnote, PBR / streaming-world futures live on separate branches.

---

## 1. Target structure (post-refactor)

### 1.1 File layout (delta only)

```
crates/
  bevy_naadf/
    src/
      voxel/
        mod.rs                    (unchanged — D1 territory; bit-layout consts)
        vox_import.rs       ▼     (1733 → ~1500 LOC; tiled family deleted)
        cvox_import.rs            (unchanged — F8 deferred, side-note only)
        voxel_dispatch.rs         (unchanged)
        async_vox.rs              (unchanged)
        web_vox.rs           ▼    (608 → ~600 LOC; F5 + F6 fold)
        grid.rs              ▼    (1354 → ~1100 LOC; F3 helper + F9 cleanup)
      camera/
        mod.rs                    (unchanged surface; +pub use poses re-export)
        position_split.rs         (unchanged)
        poses.rs             ◆    NEW (~30 LOC; F6 destination — moves
                                  HORIZON_CAMERA_POS/_ROT here)

crates/voxel_noise/                 ✗ DELETED (entire crate — F1)
```

`▼` = LOC-reduced edit. `◆` = new file. `✗` = deletion.

### 1.2 Net LOC delta

| change | LOC delta |
|---|---|
| F1: `crates/voxel_noise/` crate + dist + js + Makefile | **−1 033** Rust + ~50 C-ABI/JS + Makefile |
| F1: workspace `Cargo.toml`, `Cargo.lock`, `justfile`, `rust-toolchain.toml`, `scripts/lint/wasm-compat.sh` | ~−25 |
| F2: tiled family (`parse_vox_bytes_tiled`, `load_vox_tiled`, `parse_dot_vox_data_tiled`, `replicate_buckets_xz`, `install_vox_sized_to_model`'s special-case, `tiled_load_expands_world_xz_and_dedups_blocks` test) | ~−180 |
| F3: `install_world` shared helper (extract palette-install log + `WorldData` literal + `InitialCameraPose` write into one place) | ~−180 net (helper adds ~80, removes ~260 from 3 install fns) |
| F4: `GridPreset::WebSkybox` arm + drop `WebSkyboxOverride` marker + `setup_test_grid` simplification | ~−20 |
| F5: drop `Option<Res<_>>` + add `.run_if(resource_exists::<_>)` | ~−10 |
| F6: move `HORIZON_CAMERA_POS/ROT` to `camera/poses.rs`; e2e + production import from there | +30 / −0 (net 0 on D3 surface; +30 in camera/) |
| F7: collapse 5-layer wrapper chain to 3 layers (post-F2) | ~−25 |
| F9: drop bare `let _ = WORLD_SIZE_IN_VOXELS;` and unused-import audit | ~−5 |
| **D3 net delta** | **~−1 500 LOC** (workspace), ~−440 LOC inside `crates/bevy_naadf/src/voxel/` |

The voxel_noise deletion alone is the biggest workspace-LOC win in D3.

---

## 2. F1 — `crates/voxel_noise/` crate deletion (exact steps)

### Why it goes

- Workspace docstring at `Cargo.toml:5-11` self-describes as *"NOT yet wired into the renderer."*
- Verified zero callers in `crates/bevy_naadf/src/` (Grep + Read of source tree). Only references outside the crate are docs, `.claude/worktrees/*` (pre-existing worktree shadows of the same crate — out of scope), and `target/` fingerprints.
- `streaming-world` orchestration (the intended downstream consumer) stopped at design phase 2026-05-18 (`docs/orchestrate/streaming-world/02-design.md`); no impl. Per addendum: master-branch is the C# port — streaming-world futures live on their own branch.
- C++ FFI dependency (`fastnoise2` build-from-source) + separate Emscripten toolchain (`Makefile`) cost every workspace `cargo build` and every CI run.

### Exact files / lines to edit

1. **Delete the crate directory recursively**:
   ```
   rm -rf /mnt/archive4/DEV/bevy-naadf/crates/voxel_noise/
   ```
   Contents (verified `1033` LOC in 4 .rs files + `Cargo.toml` + `Makefile` + `dist/` + `examples/` + `js/`).

2. **`Cargo.toml` (workspace root)**: edit lines 1-16.
   - Delete the docstring block describing voxel_noise at lines `Cargo.toml:5-11` (5 lines of comment).
   - Delete the FastNoise2 / Emscripten reference at lines `Cargo.toml:10-13`.
   - Change the workspace members declaration at line `Cargo.toml:15`:
     - **From**: `members = ["crates/bevy_naadf", "crates/voxel_noise"]`
     - **To**:   `members = ["crates/bevy_naadf"]`

3. **`Cargo.lock`** (workspace root): the next `cargo build` will rewrite the lockfile to drop `voxel_noise` and its FFI deps (`fastnoise2`, etc.). Implementor does NOT hand-edit Cargo.lock — let cargo regenerate. Verified existing entry at `Cargo.lock:5434` (name = "voxel_noise").

4. **`justfile` lines 4-5, 136-148**: delete the voxel_noise documentation line + the entire `voxel_noise (FastNoise2 — native API + Emscripten C-ABI module)` section header + three recipes (`noise-build`, `noise-test`, `noise-clean`). Targets:
   - Line 5: `#   crates/voxel_noise  — the FastNoise2 wrapper (native API + Emscripten module)` — delete.
   - Lines 136-148: the entire `# ── voxel_noise (FastNoise2 — native API + Emscripten C-ABI module) ───────` block + three recipes — delete (13 lines).

5. **`rust-toolchain.toml` lines 17-19**: remove the wasm32-unknown-emscripten target (sole reason it was in the targets list). Edit:
   - Delete line `rust-toolchain.toml:18` (the comment `# wasm32-unknown-emscripten — the voxel_noise FastNoise2 C-ABI module (Makefile).`).
   - Edit line `rust-toolchain.toml:19`: change `targets = ["wasm32-unknown-unknown", "wasm32-unknown-emscripten"]` → `targets = ["wasm32-unknown-unknown"]`.

6. **`scripts/lint/wasm-compat.sh` line 9**: drop `crates/voxel_noise/src` from the lint scan loop. Edit:
   - **From**: `for dir in crates/bevy_naadf/src crates/voxel_noise/src; do`
   - **To**:   `for dir in crates/bevy_naadf/src; do`

7. **Docs (orchestration history, optional housekeeping)**: `docs/orchestrate/streaming-world/*.md` reference `voxel_noise` as a future dep (4 docs). **Decision: leave as-is** — they are read-only orchestration history. The next orchestration that revives streaming-world re-adds the dep from upstream. A brief note can be appended to `docs/orchestrate/streaming-world/README.md` if and only if streaming-world is later revived; not blocking and not in scope for this refactor.

### Reuse choices

No existing match — pure deletion. No replacement type needed (the crate has no live consumer to migrate).

### Behavioural delta

None — the crate has zero callers from `crates/bevy_naadf`. Deletion is observable only via the workspace-build matrix shrinking (no more `fastnoise2` C++ compile, no more wasm32-unknown-emscripten target).

---

## 3. F2 — tiled family fate (delete tiling; KEEP `install_vox_sized_to_model`, CPU oracle, `vox_gpu_oracle_cpu_phase` flag)

### Recommendation: **delete tiling implementation; keep the `--vox-gpu-oracle` gate intact**

This is the master-branch-identity-aligned move:

- **The C# `MagicaVoxel.cs` has zero `tile|replicate|XZ|tiles` code** (verified by explorer Grep + my Read of `MagicaVoxel.cs:651-755`). Tiling in Rust is **not C# faithful** — it's a Rust-specific addition to support one test.
- **The `--vox-gpu-oracle` gate stays** — D6's explorer keeps it (`02-exploration.md ¶6` calls it "the most-grown gate", but does not propose deletion). The CPU phase via `install_vox_sized_to_model` is the load-bearing reference renderer for the SSIM diff (verified `e2e/vox_gpu_oracle.rs:1-110`).
- **`install_vox_sized_to_model` collapses to use the non-tiled `load_vox`** (which already exists at `vox_import.rs:171-174`). The current implementation passes `tiles=1` anyway (`grid.rs:366: vox_import::load_vox_tiled(path, 1)`); the tile param has been dead in this caller since Stage 14.
- **`AppArgs.vox_gpu_oracle_cpu_phase` stays** — it's the gate-routing flag the e2e driver consults at `driver.rs:548-554` to select `VoxGpuOracleWarmup`. Not a tiling flag; not removed.

The only thing that goes is the **tiling implementation** + its one unit test.

### Current shape (verified)

```rust
// vox_import.rs:154-157 — non-tiled entry (KEEPS)
pub fn parse_vox_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxImportError> {
    let data = dot_vox::load_bytes(bytes).map_err(VoxImportError::Parse)?;
    parse_dot_vox_data(&data)
}

// vox_import.rs:161-164 — tiled twin (DELETE)
pub fn parse_vox_bytes_tiled(bytes: &[u8], tiles: u32) -> Result<ImportedVox, VoxImportError> {
    let data = dot_vox::load_bytes(bytes).map_err(VoxImportError::Parse)?;
    parse_dot_vox_data_tiled(&data, tiles)
}

// vox_import.rs:171-174 — non-tiled load (KEEPS)
pub fn load_vox(path: impl AsRef<Path>) -> Result<ImportedVox, VoxImportError> {
    let bytes = std::fs::read(path.as_ref())?;
    parse_vox_bytes(&bytes)
}

// vox_import.rs:184-187 — tiled twin (DELETE)
pub fn load_vox_tiled(path: impl AsRef<Path>, tiles: u32) -> Result<ImportedVox, VoxImportError> {
    let bytes = std::fs::read(path.as_ref())?;
    parse_vox_bytes_tiled(&bytes, tiles)
}

// vox_import.rs:193-195 — wrapper (collapsed)
pub fn parse_dot_vox_data(data: &dot_vox::DotVoxData) -> Result<ImportedVox, VoxImportError> {
    parse_dot_vox_data_tiled(data, 1)
}

// vox_import.rs:207-225 — body that handles `tiles` (DELETE tiling branch; keep
// the body but rename to `parse_dot_vox_data` and drop the `tiles` arg)
pub fn parse_dot_vox_data_tiled(
    data: &dot_vox::DotVoxData,
    tiles: u32,
) -> Result<ImportedVox, VoxImportError> { ... replicate_buckets_xz ... }

// vox_import.rs:235-276 — `replicate_buckets_xz` (DELETE; ~42 LOC)

// vox_import.rs:1675-1731 — `tiled_load_expands_world_xz_and_dedups_blocks` test (DELETE)
```

### Target shape

```rust
// vox_import.rs — non-tiled is the only API
pub fn parse_vox_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxImportError> {
    let data = dot_vox::load_bytes(bytes).map_err(VoxImportError::Parse)?;
    parse_dot_vox_data(&data)
}

pub fn load_vox(path: impl AsRef<Path>) -> Result<ImportedVox, VoxImportError> {
    let bytes = std::fs::read(path.as_ref())?;
    parse_vox_bytes(&bytes)
}

pub fn parse_dot_vox_data(data: &dot_vox::DotVoxData) -> Result<ImportedVox, VoxImportError> {
    if data.models.is_empty() {
        return Err(VoxImportError::Empty);
    }
    let (buckets, _tile_size_in_chunks) = compose_to_sparse_world(data)?;
    let world = build_constructed_world_sparse(buckets)?;
    let palette = vox_palette_to_voxel_types(&data.palette, &data.materials);
    Ok(ImportedVox { world, palette })
}
```

(The body of the renamed `parse_dot_vox_data` is the current `parse_dot_vox_data_tiled` body with the `tiles` param dropped, the `tiles == 1` branch always taken, and the `replicate_buckets_xz` else-arm removed.)

```rust
// grid.rs:365-403 — install_vox_sized_to_model (simplified caller)
fn install_vox_sized_to_model(commands: &mut Commands, path: &std::path::Path) {
    match vox_import::load_vox(path) {                          // was load_vox_tiled(path, 1)
        Ok(imp) => { /* unchanged body */ }
        Err(e) => { /* unchanged body */ }
    }
}
```

### Reuse choices

- `vox_import::load_vox` (`vox_import.rs:171-174`) — already exists, this becomes its sole caller after the tiled twin deletes.
- `vox_import::parse_vox_bytes` (`vox_import.rs:154-157`) — already exists, dispatch + tests use it. No change.

### Behavioural delta

- **None for production**: every production call site already passes `tiles=1` and gets the single-tile path. The function call shape `load_vox_tiled(path, 1)` → `load_vox(path)` produces byte-identical output (verified by the existing assertion `tiled.world.voxels.len() == single.world.voxels.len()` in the test being deleted).
- **One unit test deleted**: `tiled_load_expands_world_xz_and_dedups_blocks` (`vox_import.rs:1675-1731`). This test validates a feature that has no production code path and no C# counterpart. The dedup behaviour it asserts is still tested via the single-tile path elsewhere (the dedup HashMap in `build_constructed_world_sparse` is exercised on every parse).
- **Faithful-port rule**: aligns with C# (which has no tiling); deletes a Rust-specific divergence from the reference.

### D6 coordination

D6 owns `e2e/vox_gpu_oracle.rs`. D3 deletes only the tiling implementation in `vox_import.rs`/`grid.rs`; D6 is **not affected** (the gate continues to invoke `install_vox_sized_to_model` via `vox_gpu_oracle_cpu_phase`; the install function continues to load Oasis at the model's natural bounds; only the never-used `tiles>1` code path goes).

---

## 4. F6 — dependency-arrow reversal (camera-pose constants leave `e2e/`)

### Current shape (verified)

```rust
// e2e/vox_horizon_parity.rs:72-81 — DEFINED HERE
pub const HORIZON_CAMERA_POS: Vec3 = Vec3::new(3880.187, 497.332, 3514.350);
pub const HORIZON_CAMERA_ROT: Quat = Quat::from_xyzw(
    -0.09791362, 0.5846077, 0.07135339, 0.8022191,
);

// Production code imports FROM e2e/ (dependency arrow inversion):
// voxel/grid.rs:571-572
translation: crate::e2e::vox_horizon_parity::HORIZON_CAMERA_POS,
rotation:    crate::e2e::vox_horizon_parity::HORIZON_CAMERA_ROT,

// voxel/web_vox.rs:287-288 (same import)
translation: crate::e2e::vox_horizon_parity::HORIZON_CAMERA_POS,
rotation:    crate::e2e::vox_horizon_parity::HORIZON_CAMERA_ROT,
```

This makes the production binary's camera pose depend on a module the e2e harness owns. Any D6 refactor of `vox_horizon_parity.rs` (DUP-6 pin-camera consolidation per audit §3.2) silently moves the production camera.

### Target shape

Move the two `pub const`s into a new module `crates/bevy_naadf/src/camera/poses.rs`. e2e imports from there; D3 production code also imports from there.

```rust
// camera/poses.rs (NEW, ~30 LOC)
//! Named camera poses shared between production code paths and e2e gates.
//!
//! Production code (`voxel/grid.rs::install_imported_vox`,
//! `voxel/web_vox::pin_web_horizon_camera`) and the cross-target SSIM gate
//! (`e2e/vox_horizon_parity`) both anchor on these constants so a
//! `just web-static` / `just web` / native release boot lands at the same
//! camera the Playwright gate screenshots. **Production code MUST NOT import
//! from `crate::e2e`** — this module is the canonical home.
use bevy::prelude::*;

/// Cross-target horizon-view camera position (voxel units, world coords).
/// User-captured 2026-05-19. See `e2e/vox_horizon_parity.rs` module docs for
/// rationale + the corresponding window-resolution / SSIM threshold.
pub const HORIZON_CAMERA_POS: Vec3 = Vec3::new(3880.187, 497.332, 3514.350);

/// Cross-target horizon-view camera rotation. Forward ≈ `(-0.924, -0.241, -0.297)`.
pub const HORIZON_CAMERA_ROT: Quat = Quat::from_xyzw(
    -0.09791362, 0.5846077, 0.07135339, 0.8022191,
);
```

```rust
// camera/mod.rs — add the re-export
pub mod poses;
pub mod position_split;
```

```rust
// e2e/vox_horizon_parity.rs — import from production
use crate::camera::poses::{HORIZON_CAMERA_POS, HORIZON_CAMERA_ROT};
// delete the const defs at lines 71-81
```

```rust
// voxel/grid.rs:571-572 — import from camera
translation: crate::camera::poses::HORIZON_CAMERA_POS,
rotation:    crate::camera::poses::HORIZON_CAMERA_ROT,

// voxel/web_vox.rs:287-288 — import from camera
translation: crate::camera::poses::HORIZON_CAMERA_POS,
rotation:    crate::camera::poses::HORIZON_CAMERA_ROT,
```

### Reuse choices

- New file. The `camera/` module is the natural home — it already owns `InitialCameraPose` (`camera/mod.rs:50`) and `PositionSplit` (`camera/position_split.rs`). Pose constants alongside pose types is the consistent shape.
- Alternative considered: place the constants in `voxel/grid.rs` next to `GRID_SIZE_IN_CHUNKS`. **Rejected** — `voxel/grid.rs` is the install-builder, not a camera-pose home; cross-domain consumers (e2e + web_vox + grid itself) would still cross the voxel-module boundary. `camera/poses.rs` keeps all camera-related constants in one place.

### Behavioural delta

None — constants move modules; values byte-identical; the e2e gate continues to use them through the new path.

### D6 coordination

D6 owns `e2e/vox_horizon_parity.rs`. The architect's design lands the const defs in `camera/poses.rs`; D6's implementor replaces the `pub const` defs at `vox_horizon_parity.rs:71-81` with the import. **D6 architect should note in their own design that the canonical home for these constants is now `crate::camera::poses` and import from there.** D3's implementor edits the production-side import sites at `grid.rs:571-572` and `web_vox.rs:287-288` directly.

---

## 5. Per-finding targets (the rest)

### Finding 3 — `grid.rs` install fns share boilerplate

**Current shape (verified)**: 3 install fns (`install_empty_world` `grid.rs:164-228`, `install_default_embedded_in_fixed_world` `grid.rs:241-354`, `install_imported_vox` `grid.rs:529-663`) each redundantly construct:
1. A `WorldData { ..., bounding_box: <full fixed world>, pending_edits: Default::default(), dense_voxel_types: Vec::new(), block_hashing: BlockHashingHandler::new() }` literal (`grid.rs:188-204`, `304-330`, `618-634`).
2. A `[palette-install]` debug-log block (3 verbatim copies with "DO NOT REMOVE" markers at `grid.rs:213,339,646`).
3. An `InitialCameraPose` insertion (literal pose / demo-relative pose / horizon-pose at `184-186`, `293-302`, `570-574`).

Each fn intends a different *content* (no voxels / synthesised demo + ground / ModelData-driven) but shares all the *scaffolding*. The "DO NOT REMOVE" markers are admissions that three places that shouldn't be three places are deliberately kept in sync.

**Target shape**: extract a single private helper `install_world_at_fixed_size` in `grid.rs` that owns the `WorldData` literal + the `[palette-install]` log + the `commands.insert_resource(VoxelTypes { ... })` call. The three install fns become small adapters that compute the (chunks_cpu, blocks_cpu, voxels_cpu, dense_voxel_types, palette, source_label, model_data) tuple and call the helper.

```rust
// grid.rs (new private helper — replaces 3 verbatim copies)
struct WorldInstall {
    /// World content. Empty `Vec`s for empty-world / .vox-fixed (W5 GPU producer fills these).
    chunks_cpu: Vec<u32>,
    blocks_cpu: Vec<u32>,
    voxels_cpu: Vec<u32>,
    /// Dense type mirror. Empty for every install path that isn't the small-default
    /// demo (~17 GiB cost at the fixed 4096×512×4096 size).
    dense_voxel_types: Vec<u16>,
    /// Palette to install.
    palette: Vec<VoxelType>,
    /// Camera pose. Each install fn computes this.
    camera_pose: Transform,
    /// Optional W5 generator model. `Some` for `install_imported_vox`, `None` otherwise.
    model_data: Option<crate::aadf::generator::ModelData>,
    /// Source label for the `[palette-install]` smoke-detector log.
    source_label: &'static str,
}

fn install_world_at_fixed_size(commands: &mut Commands, install: WorldInstall) {
    commands.insert_resource(crate::camera::InitialCameraPose(install.camera_pose));

    if let Some(model_data) = install.model_data {
        commands.insert_resource(model_data);
    }

    let mut world_data = WorldData {
        chunks_cpu: install.chunks_cpu,
        blocks_cpu: install.blocks_cpu,
        voxels_cpu: install.voxels_cpu,
        size_in_chunks: WORLD_SIZE_IN_CHUNKS,
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(
                WORLD_SIZE_IN_VOXELS.x as i32 - 1,
                WORLD_SIZE_IN_VOXELS.y as i32 - 1,
                WORLD_SIZE_IN_VOXELS.z as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: install.dense_voxel_types,
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();
    commands.insert_resource(world_data);

    // web-vox-color-divergence smoke detector — ONE place now. The
    // "DO NOT REMOVE" markers at the 3 prior call sites collapse here.
    {
        let preview: Vec<(f32, f32, f32)> = install.palette
            .iter()
            .take(5)
            .map(|t| (t.color_base.x, t.color_base.y, t.color_base.z))
            .collect();
        debug!(
            "[palette-install] label={:?} palette_len={} first_5_color_base={:?}",
            install.source_label, install.palette.len(), preview,
        );
    }
    commands.insert_resource(VoxelTypes { types: install.palette });
}
```

Install adapters then become:

```rust
fn install_empty_world(commands: &mut Commands) {
    let cam = Transform::from_translation(Vec3::new(11.0, 7.0, 17.0))
        .looking_at(Vec3::new(0.0, 4.0, -3.0), Vec3::Y);
    install_world_at_fixed_size(commands, WorldInstall {
        chunks_cpu: Vec::new(),
        blocks_cpu: Vec::new(),
        voxels_cpu: Vec::new(),
        dense_voxel_types: Vec::new(),
        palette: build_palette(),
        camera_pose: cam,
        model_data: None,
        source_label: "skybox-only",
    });
}
```

`install_default_embedded_in_fixed_world` and `install_imported_vox` similarly thin out.

**Reuse choices**: the new helper is private to `grid.rs`. No existing match exists (the C# `WorldHandler.cs` is the structural inspiration but a Bevy-Rust shape needs Bevy commands + insertion-resource semantics — there is no shared utility this consolidates against).

**Behavioural delta**:
- The three `[palette-install]` debug log blocks collapse to one. The smoke-detector signal still fires for every install path; the message body is now uniform (`label={:?}` instead of three slightly-different prose templates). Anyone diffing `RUST_LOG=bevy_naadf=debug` output across before/after will see one log line per install rather than three near-identical lines. This is an *improvement* in observability, not a regression.
- The `web-vox-color-divergence` regression detector continues to work — the same fields (palette length, first-5 colors, source label) reach the log.

### Finding 4 — `setup_test_grid` Startup dispatch + `WebSkyboxOverride` marker → `GridPreset` enum extension

**Current shape (verified)**: `voxel/grid.rs:104-143` is a Startup system with three independent decision axes mashed together:
1. `Option<Res<WebSkyboxOverride>>` early-exit → install_empty_world (`grid.rs:107,114-121`).
2. `match &args.grid_preset` 3-arm match (`grid.rs:122-142`).
3. Inside the `Vox` arm, branch on `args.vox_gpu_oracle_cpu_phase` (`grid.rs:127-138`).

`WebSkyboxOverride` is inserted only by `web_vox::startup_fetch_default_vox:404-411` (web-only) and forces `.before(setup_test_grid)` ordering at `lib.rs:840-841`.

**Target shape**: extend `GridPreset` with a `WebSkybox` arm. The wasm bootstrap mutates `AppArgs.grid_preset` instead of inserting a marker. `WebSkyboxOverride` is deleted. The `vox_gpu_oracle_cpu_phase` branch stays (it's a sub-mode of the `Vox` arm and tied to the active `--vox-gpu-oracle` gate — D6 keeps the gate).

```rust
// lib.rs — extend the enum
#[derive(Debug, Clone, Default)]
pub enum GridPreset {
    #[default]
    Default,
    Vox { path: std::path::PathBuf },
    Empty,
    /// Web-only: `?skybox=1` URL param. Same install behaviour as `Empty`
    /// but the discriminator exists so the wasm bootstrap can express the
    /// decision via `AppArgs.grid_preset` mutation instead of a separate
    /// marker resource + ordering constraint on `setup_test_grid`.
    /// Functionally identical to `Empty`; kept as a distinct arm so log
    /// messages can distinguish the source ("?skybox=1" vs CLI `--empty`).
    WebSkybox,
}
```

```rust
// voxel/grid.rs:104-143 — simplified Startup system (no Option<Res<_>>, no .before(...) needed)
pub fn setup_test_grid(mut commands: Commands, args: Res<AppArgs>) {
    match &args.grid_preset {
        GridPreset::Default => {
            install_default_embedded_in_fixed_world(&mut commands);
        }
        GridPreset::Vox { path } => {
            if args.vox_gpu_oracle_cpu_phase {
                install_vox_sized_to_model(&mut commands, path);
            } else {
                install_vox_in_fixed_world(&mut commands, path);
            }
        }
        GridPreset::Empty => {
            install_empty_world(&mut commands, "cli-empty");
        }
        GridPreset::WebSkybox => {
            install_empty_world(&mut commands, "skybox-only");
        }
    }
}
```

```rust
// voxel/web_vox.rs:404-411 — mutate args instead of inserting marker.
// `setup_test_grid` runs after `startup_fetch_default_vox` (the existing
// `.before` ordering becomes redundant but is still cheap to keep — see
// migration note in §6).
if resolve_skybox_only_param() {
    info!(
        "web_vox: ?skybox=1 detected — switching grid_preset to WebSkybox"
    );
    // ResMut<AppArgs> — see migration step for parameter change.
    args.grid_preset = GridPreset::WebSkybox;
    hide_loading_overlay();
    return;
}
```

```rust
// lib.rs:838-842 — drop the .before() ordering after migration confirms web ok
// (kept as a belt-and-suspenders during migration; deleted in step 5 once
// the AppArgs-mutation path is confirmed working on web)
```

**Reuse choices**: `GridPreset` is the obvious extension point. The `States`-based alternative (`WorldLoadState::{PendingScene, InstallingDefault, Ready}`) is over-engineered for this — we have 4 discrete install paths, not a state machine.

**Behavioural delta**:
- The `WebSkyboxOverride` marker resource is deleted; its semantics move into `GridPreset::WebSkybox`. The `?skybox=1` URL param produces the same empty-world install via a different mechanism. User-visible behaviour identical (skybox-only baseline render).
- The `.before(setup_test_grid)` ordering at `lib.rs:840-841` becomes unnecessary: `startup_fetch_default_vox` and `setup_test_grid` no longer have a write-then-read dependency on `WebSkyboxOverride` insertion. The ordering can be kept as documentation or removed; either is correct (Bevy's Startup is single-threaded by default for `Commands`-mutating systems, so the relative order without an explicit `.before` is "deterministic but unspecified" — for safety, keep the `.before` constraint so the contract is explicit).
- `startup_fetch_default_vox` parameter list changes from `(mut commands: Commands)` to `(mut commands: Commands, mut args: ResMut<AppArgs>)` to allow the `GridPreset` mutation.

### Finding 5 — `pin_web_horizon_camera` + `hide_ui` → `.run_if(resource_exists::<_>())`

**Current shape (verified)**:

```rust
// web_vox.rs:236-267 — hide_ui (opens with early-bail)
pub fn hide_ui(
    override_resource: Option<Res<UiHiddenOverride>>,
    mut editor_hud: Query<...>,
    mut settings_root: Query<...>,
    mut diag_hud: Query<...>,
) {
    if override_resource.is_none() { return; }
    // ...
}

// web_vox.rs:273-296 — pin_web_horizon_camera (same pattern)
pub fn pin_web_horizon_camera(
    override_resource: Option<Res<WebHorizonPoseOverride>>,
    camera: Option<Single<...>>,
) {
    if override_resource.is_none() { return; }
    // ...
}

// lib.rs:851-855,970 — registered without .run_if
.add_systems(Update, voxel::web_vox::pin_web_horizon_camera.after(...));
app.add_systems(Update, voxel::web_vox::hide_ui);
```

**Target shape**:

```rust
// web_vox.rs — drop the Option, no early-bail (system body unchanged below)
pub fn hide_ui(
    mut editor_hud: Query<&mut Visibility, With<crate::editor::hud::EditorHudRoot>>,
    mut settings_root: Query<&mut Visibility, ...>,
    mut diag_hud: Query<&mut Visibility, ...>,
) { /* body w/o the early bail */ }

pub fn pin_web_horizon_camera(
    camera: Option<Single<(&mut Transform, &mut crate::camera::position_split::PositionSplit), With<Camera3d>>>,
) { /* body w/o the early bail */ }

// lib.rs — registration grows .run_if
#[cfg(target_arch = "wasm32")]
app.add_systems(
    Update,
    voxel::web_vox::pin_web_horizon_camera
        .after(voxel::async_vox::poll_pending_vox_parse)
        .run_if(bevy::ecs::common_conditions::resource_exists::<voxel::web_vox::WebHorizonPoseOverride>),
);
// ... same shape for hide_ui with UiHiddenOverride
```

**Reuse choices**: `bevy::ecs::common_conditions::resource_exists` is the Bevy 0.19 stock primitive (verified by searching imports in `lib.rs`). No new abstraction.

**Behavioural delta**: identical user-visible behaviour. The scheduler skips the system call when the resource is absent (instead of the system body short-circuiting). On a production boot without `?pose=horizon` / `?ui=hide`, neither system runs — the per-frame overhead drops from "function call + Option check" to "scheduler condition check" (the `resource_exists` condition is a single archetype query and is amortised across the schedule).

### Finding 7 — wrapper chain collapse (post-F2)

**Current shape** (after F2 lands):

```
grid::parse_to_imported_vox    (`grid.rs:502-513`)
  → voxel_dispatch::parse_voxel_bytes  (`voxel_dispatch.rs:80-91`)
    → vox_import::parse_vox_bytes  (`vox_import.rs:154-157`)
      → vox_import::parse_dot_vox_data  (`vox_import.rs:193-195`)
```

After F2 deletes the `_tiled` variants, the chain is 4 layers deep:
- `parse_to_imported_vox` (String error mapping shim).
- `parse_voxel_bytes` (magic-byte dispatch).
- `parse_vox_bytes` (`dot_vox::load_bytes` + `parse_dot_vox_data`).
- `parse_dot_vox_data` (the actual work).

**Target shape**: collapse to 3 layers, eliminating `parse_to_imported_vox` (whose only job is to map `VoxelParseError` → `String` for callers).

Decision: keep the dispatch boundary (magic-byte dispatch is genuinely independent of either parser); keep `parse_vox_bytes` (it's the magic-byte-dispatched format parser; symmetric with `cvox_import::parse_cvox_bytes`). Delete `parse_to_imported_vox` in `grid.rs` and inline its error mapping at the two callers that need a `String` error.

```rust
// grid.rs — delete parse_to_imported_vox; callers use voxel_dispatch::parse_voxel_bytes directly

// async_vox.rs::spawn_native_vox_parse and async_vox.rs::spawn_wasm_vox_parse
// (whichever currently call parse_to_imported_vox — there are 1-2 sites) become:
let parsed = crate::voxel::voxel_dispatch::parse_voxel_bytes(bytes)
    .map_err(|e| e.to_string());
```

**Reuse choices**: no new types. The `String` error mapping is a single `.map_err(|e| e.to_string())` at the call site — cheaper to inline than to keep a 12-LOC wrapper module-level.

**Behavioural delta**: none. The error mapping moves from the central wrapper to the 1-2 call sites; the resulting `Result<ImportedVox, String>` returned to the async parse task is byte-identical.

**Audit note**: this is a post-F2 cleanup. If F2 implementation has issues, F7 can be deferred without affecting correctness. The 4-layer chain works; it's just verbose.

### Finding 9 — drop `let _ = WORLD_SIZE_IN_VOXELS;` in `install_imported_vox`

**Current shape (verified)**:
```rust
// voxel/grid.rs:569
let _ = WORLD_SIZE_IN_VOXELS;
```

The comment block above at `grid.rs:556-568` documents *why* the constant is still referenced (historical: the deleted code path proportionally scaled the camera pose from `WORLD_SIZE_IN_VOXELS`). The bare `let _` keeps the import live but signals "this is intentionally dead" only via context.

**Target shape**: drop the bare statement. Audit the import list at `grid.rs:34` — `WORLD_SIZE_IN_VOXELS` is also used at `install_default_embedded_in_fixed_world:264` and `install_empty_world:194-197`, so removing the bare-binding statement leaves the import live for legitimate consumers; no further import-list edit needed.

```rust
// grid.rs:569 — delete the line.
// The explanatory comment block at grid.rs:556-568 stays.
```

**Reuse choices**: none.

**Behavioural delta**: none. Pure cleanup of an `unused_must_use`-style scar.

### Finding 8 — `voxel_dispatch::Cursor` collision (deferred / side-note)

**Decision: DEFER.** The collision is documented (see explorer's F8). Renaming `Cursor` → `LeReader` in `cvox_import.rs` is mechanical but cross-cuts the file's 22kLOC and the rename touches ~10 sites. The smell is real but the cost-benefit ratio is poor: the file's docstring at `cvox_import.rs:256` acknowledges the design (hand-rolled non-allocating reader). No callers outside this file see the collision.

If a future architect wants to take it on, the rename is `s/struct Cursor/struct LeReader/` + adjust `use` blocks. Not part of this design.

---

## 6. Migration steps (atomic, ordered)

Each step leaves the workspace in a `cargo build --workspace` + `cargo test --workspace --lib` passing state. Implementor must verify before proceeding to the next step.

### Step 1 — Delete `crates/voxel_noise/` + workspace cleanups (F1)

**Edits:**
- `Cargo.toml:5-11,15` — drop the docstring referencing voxel_noise; change `members = [...]` to only include `crates/bevy_naadf`.
- `Cargo.lock` — let cargo regenerate (do not hand-edit).
- `justfile:5,136-148` — delete the documentation line + the entire voxel_noise section (recipes `noise-build`, `noise-test`, `noise-clean`).
- `rust-toolchain.toml:18-19` — drop wasm32-unknown-emscripten from `targets`.
- `scripts/lint/wasm-compat.sh:9` — drop `crates/voxel_noise/src` from the scan loop.
- `crates/voxel_noise/` — delete recursively.

**Rationale**: Zero-caller deletion. Largest single workspace LOC win.

**Post-step state**: workspace contains only `crates/bevy_naadf`. No FastNoise2 C++ toolchain dep. `cargo build --workspace` builds only bevy_naadf.

**Verification**: `cargo build --workspace`, `cargo test --workspace --lib`. (The e2e gates do not depend on voxel_noise; full `cargo run --bin e2e_render -- baseline` check is overkill for this step but cheap.)

---

### Step 2 — Move `HORIZON_CAMERA_POS` / `HORIZON_CAMERA_ROT` to `camera/poses.rs` (F6)

**Edits:**
- `crates/bevy_naadf/src/camera/poses.rs` — NEW. Defines `HORIZON_CAMERA_POS` and `HORIZON_CAMERA_ROT` with full doc-comments (transplant from `vox_horizon_parity.rs:66-81`).
- `crates/bevy_naadf/src/camera/mod.rs:10` — add `pub mod poses;` after `pub mod position_split;`.
- `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs:71-81` — delete the const defs. Add `use crate::camera::poses::{HORIZON_CAMERA_POS, HORIZON_CAMERA_ROT};` near the top. (Keep the module-level `//!` rationale; only the const defs leave.)
- `crates/bevy_naadf/src/voxel/grid.rs:571-572` — replace `crate::e2e::vox_horizon_parity::HORIZON_CAMERA_POS/ROT` with `crate::camera::poses::HORIZON_CAMERA_POS/ROT`.
- `crates/bevy_naadf/src/voxel/web_vox.rs:287-288` — same replacement.
- `crates/bevy_naadf/src/voxel/web_vox.rs:188` (doc-comment) — update the cross-reference to point at `crate::camera::poses` instead of `crate::e2e::vox_horizon_parity`.

**Rationale**: Reverse the dependency arrow. Production code no longer imports from `e2e/`.

**Post-step state**: `crate::camera::poses` is the canonical home for cross-target camera-pose constants. `e2e/vox_horizon_parity` is a pure consumer of them. Production `voxel/grid.rs` and `voxel/web_vox.rs` import from `camera::poses` only.

**Verification**: `cargo build --workspace`, `cargo test --workspace --lib`, `cargo run --bin e2e_render -- --vox-horizon-native` (sanity check that the SSIM gate continues to function with the relocated constants; the captured PNG must match the prior baseline byte-for-byte since the values are unchanged).

---

### Step 3 — Delete tiled family (F2) + collapse `parse_dot_vox_data` (partial F7)

**Edits:**
- `crates/bevy_naadf/src/voxel/vox_import.rs:161-164` — DELETE `parse_vox_bytes_tiled`.
- `crates/bevy_naadf/src/voxel/vox_import.rs:176-187` — DELETE `load_vox_tiled` (and its `vox-gpu-rewrite Stage 2` docstring at `176-183`).
- `crates/bevy_naadf/src/voxel/vox_import.rs:189-225` — REWRITE `parse_dot_vox_data` to absorb the body of the deleted `parse_dot_vox_data_tiled` (drop the `tiles` parameter; always take the `tiles == 1` branch; drop the `replicate_buckets_xz` call).
- `crates/bevy_naadf/src/voxel/vox_import.rs:227-276` — DELETE `replicate_buckets_xz`.
- `crates/bevy_naadf/src/voxel/vox_import.rs:1675-1731` — DELETE the `tiled_load_expands_world_xz_and_dedups_blocks` unit test (also delete any imports inside `mod tests` that only the deleted test used).
- `crates/bevy_naadf/src/voxel/grid.rs:366` — change `vox_import::load_vox_tiled(path, 1)` to `vox_import::load_vox(path)`. Update the `[Stage 14 docstring]` at `grid.rs:356-364` to drop the "tile-count knob is retained" line; replace with a one-liner noting that `install_vox_sized_to_model` is the CPU oracle for `--vox-gpu-oracle`.

**Rationale**: The tiling code has zero production callers, no C# counterpart, and a single test that validates a feature the codebase doesn't use. Delete the implementation; the gate it serves keeps working through the non-tiled path.

**Post-step state**: `vox_import` has 3 entry points (`parse_dot_vox_data`, `parse_vox_bytes`, `load_vox`) instead of 6. No code in the workspace passes a `tiles` parameter. The `--vox-gpu-oracle` gate continues to function via the non-tiled `load_vox`.

**Verification**: `cargo build --workspace`, `cargo test --workspace --lib`, `cargo run --bin e2e_render -- --vox-gpu-oracle` (the gate must continue to produce a passing SSIM; the CPU oracle phase loads at natural bounds whether or not tiling is available in `vox_import`, since production callers always passed `tiles=1`).

---

### Step 4 — Extract `install_world_at_fixed_size` helper (F3) + `let _` cleanup (F9)

**Edits:**
- `crates/bevy_naadf/src/voxel/grid.rs` — add the `WorldInstall` struct + `install_world_at_fixed_size` private helper (per §5 Finding 3 target shape, ~80 LOC).
- `crates/bevy_naadf/src/voxel/grid.rs:164-228` — rewrite `install_empty_world` to delegate to the helper. Add a `&'static str` source_label parameter so callers can distinguish `"skybox-only"` vs `"cli-empty"` in the unified `[palette-install]` log.
- `crates/bevy_naadf/src/voxel/grid.rs:241-354` — rewrite `install_default_embedded_in_fixed_world` to delegate.
- `crates/bevy_naadf/src/voxel/grid.rs:529-663` — rewrite `install_imported_vox` to delegate. Drop the `let _ = WORLD_SIZE_IN_VOXELS;` at line 569 (F9). Keep the explanatory comment block at `grid.rs:556-568`.
- `crates/bevy_naadf/src/voxel/grid.rs:114-121` — for now, the `WebSkyboxOverride` short-circuit still calls `install_empty_world(commands, "skybox-only")`. Step 5 deletes the marker resource itself.

**Rationale**: Single source of truth for `WorldData` literal + palette-install logging. The "DO NOT REMOVE" markers collapse into one site that holds the smoke-detector invariant.

**Post-step state**: `WorldData` is constructed in one place. Each install adapter is ~30-40 LOC of "compute palette + voxel buffers + camera" + a single `install_world_at_fixed_size` call. The `[palette-install]` debug log fires once per install, with a `label` field distinguishing source.

**Verification**: `cargo build --workspace`, `cargo test --workspace --lib`, `cargo run --bin e2e_render -- baseline` (this passes the default-scene install through the helper; if the log shape diverges from the smoke detector pattern, `web-vox-color-divergence` regression checks would fail). Also `cargo run --bin e2e_render -- --vox-e2e` to exercise the `install_imported_vox` path.

---

### Step 5 — `GridPreset::WebSkybox` arm + delete `WebSkyboxOverride` (F4)

**Edits:**
- `crates/bevy_naadf/src/lib.rs:70-...` — add `WebSkybox` arm to `GridPreset` enum. Add a doc-comment that pins this as the `?skybox=1` URL-param surface.
- `crates/bevy_naadf/src/voxel/grid.rs:104-143` — rewrite `setup_test_grid` to (a) drop the `Option<Res<WebSkyboxOverride>>` parameter, (b) extend the `match` with a `GridPreset::WebSkybox` arm that calls `install_empty_world(commands, "skybox-only")`.
- `crates/bevy_naadf/src/voxel/grid.rs:145-149` — DELETE `WebSkyboxOverride` marker resource (the `#[derive(Resource, Default)] pub struct WebSkyboxOverride;`).
- `crates/bevy_naadf/src/voxel/web_vox.rs:398` — change `pub fn startup_fetch_default_vox(mut commands: Commands)` to `pub fn startup_fetch_default_vox(mut commands: Commands, mut args: ResMut<AppArgs>)`.
- `crates/bevy_naadf/src/voxel/web_vox.rs:404-411` — replace `commands.insert_resource(crate::voxel::grid::WebSkyboxOverride);` with `args.grid_preset = GridPreset::WebSkybox;`. Update the info log.
- `crates/bevy_naadf/src/lib.rs:837-842` — keep the `.before(voxel::grid::setup_test_grid)` ordering on `startup_fetch_default_vox` (still load-bearing: web fetch must complete its decision BEFORE `setup_test_grid` reads `AppArgs.grid_preset`).

**Rationale**: Decision-axis consolidation. `GridPreset` becomes the single source for "what world to install." The web bootstrap expresses its decision through the same channel as the CLI.

**Post-step state**: `setup_test_grid` has one decision axis (the enum). `WebSkyboxOverride` no longer exists. The web `?skybox=1` URL param continues to produce a skybox-only render via `GridPreset::WebSkybox`. The `.before` ordering retains its purpose (the web-side mutation must precede the consumer).

**Verification**: `cargo build --workspace`, `cargo test --workspace --lib`. **Web verification**: build the wasm bundle (`just web-build`) and exercise the `?skybox=1` URL — implementor confirms the empty-world rendering with sky-only output. (Playwright's `vox-web-parity.spec.ts` exercises `?skybox=1`; running that spec via `npm run test:e2e` is the definitive check.)

---

### Step 6 — `.run_if(resource_exists::<_>())` on `pin_web_horizon_camera` + `hide_ui` (F5)

**Edits:**
- `crates/bevy_naadf/src/voxel/web_vox.rs:236-267` — `hide_ui`: drop the `override_resource: Option<Res<UiHiddenOverride>>` parameter; drop the `if override_resource.is_none() { return; }` early-bail.
- `crates/bevy_naadf/src/voxel/web_vox.rs:273-296` — `pin_web_horizon_camera`: drop the `override_resource: Option<Res<WebHorizonPoseOverride>>` parameter; drop the early-bail. Keep `camera: Option<Single<...>>` (that one's still needed since the camera entity may not exist yet on cold-boot frames).
- `crates/bevy_naadf/src/lib.rs:851-855` — add `.run_if(resource_exists::<voxel::web_vox::WebHorizonPoseOverride>)` to `pin_web_horizon_camera`'s system tuple.
- `crates/bevy_naadf/src/lib.rs:970` — add `.run_if(resource_exists::<voxel::web_vox::UiHiddenOverride>)` to `hide_ui`'s registration.
- `crates/bevy_naadf/src/lib.rs` (imports area, around line 1-50) — add `use bevy::ecs::common_conditions::resource_exists;` if not already present.

**Rationale**: Bevy-idiomatic resource-presence gating. The scheduler skips the system call entirely when the resource is absent.

**Post-step state**: `hide_ui` and `pin_web_horizon_camera` only run when their gating resources exist. On a stock web boot without `?ui=hide` / `?pose=horizon`, neither system body executes. The scheduler condition is amortised across the schedule.

**Verification**: `cargo build --workspace`, `cargo test --workspace --lib`. **Web verification**: `?pose=horizon` Playwright run (the `vox-horizon-parity.spec.ts` spec exercises this); confirm the camera continues to pin at the horizon pose.

---

### Step 7 — Collapse `parse_to_imported_vox` (F7)

**Edits:**
- `crates/bevy_naadf/src/voxel/grid.rs:502-513` — DELETE `parse_to_imported_vox`.
- `crates/bevy_naadf/src/voxel/async_vox.rs` (1-2 call sites; verified by grep) — replace `grid::parse_to_imported_vox(bytes)` with `crate::voxel::voxel_dispatch::parse_voxel_bytes(bytes).map_err(|e| e.to_string())`.
- `crates/bevy_naadf/src/voxel/grid.rs:480-513` — clean up the docstring block above the deleted fn (preserve any cross-references that still apply; the "single magic-byte dispatch entry point" prose moves into the relevant call site or `voxel_dispatch.rs:72-79`'s docstring).

**Rationale**: 4-layer wrapper chain becomes 3. The String-error mapping is a 1-line `.map_err()` at the call site, not a wrapper module-level function.

**Post-step state**: Callers reach `voxel_dispatch::parse_voxel_bytes` directly. The dispatch boundary (magic-byte sniff + format-specific parser) remains the load-bearing seam.

**Verification**: `cargo build --workspace`, `cargo test --workspace --lib`, `cargo run --bin e2e_render -- --vox-e2e` (exercises the full async-parse pipeline).

---

### Migration ordering rationale

- **Step 1 (voxel_noise deletion)** first: it's independent and largest. Removes the C++ toolchain dep early so subsequent rebuilds are faster.
- **Step 2 (F6)** before any `voxel/grid.rs` rewrites: minimises diff conflicts in `grid.rs` between Step 2 (which touches lines 571-572) and Step 4 (which rewrites the whole install fn).
- **Step 3 (F2)** before Step 4 (F3): F3's helper extraction is cleaner if `install_vox_sized_to_model` already calls the simplified `load_vox`.
- **Step 4 (F3)** before Step 5 (F4): the helper has to land before `setup_test_grid` is simplified to call the (renamed-signature) `install_empty_world`.
- **Step 5 (F4)** before Step 6 (F5): F4 mutates `AppArgs.grid_preset` via `web_vox`; F5 changes the system parameter lists. Done in either order with care, but Step 5 changing the param list of `startup_fetch_default_vox` and Step 6 changing param lists of `hide_ui` / `pin_web_horizon_camera` are independent — they can swap if implementor prefers.
- **Step 7 (F7)** last: cleanest after F2 is in (and not blocking anything).

---

## 7. What stays / what changes / what's removed

### Stays unchanged

- `crates/bevy_naadf/src/voxel/cvox_import.rs` — F8 `Cursor` naming collision deferred (side-note); no other concerns surfaced in D3.
- `crates/bevy_naadf/src/voxel/async_vox.rs` — async pump (the explorer reframed BEV-5 as a non-issue here; `poll_pending_vox_parse` and `apply_pending_vox` correctly poll every frame for async-completion).
- `crates/bevy_naadf/src/voxel/mod.rs` — D1 territory (bit-layout constants, voxel-type defs).
- `crates/bevy_naadf/src/voxel/voxel_dispatch.rs` — clean, small, well-tested module per explorer side-note #2.
- `compose_default_scene_into_fixed_world` (`grid.rs:815-911`) — load-bearing BlockPtr-sharing composer; touched only via Step 4 which delegates `install_default_embedded_in_fixed_world` to the new helper (the composer itself is unchanged).
- `MagicaVoxel.cs`-faithful scene-graph walk (`vox_import.rs:325-953` Rot3/Xform/accumulate_world_aabb/collate_voxels_sparse/compose_to_sparse_world/build_constructed_world_sparse) — explicitly NOT in scope; the faithful-port rule protects it.
- The `--vox-gpu-oracle` gate's CPU phase (`e2e/vox_gpu_oracle.rs::run_vox_gpu_oracle_cpu_phase`) — D6 keeps the gate; `install_vox_sized_to_model` continues to serve it, just calling the non-tiled `load_vox`.
- `AppArgs.vox_gpu_oracle_cpu_phase` flag — kept; it's a gate routing flag, not a tile knob.
- `native_vox_drop_listener` / `log_native_dnd_registered` / `install_dnd_listeners` / `apply_pending_vox` / `submit_pending_bytes` / `startup_fetch_default_vox` body apart from the marker→args mutation — all unchanged.
- Drag-and-drop pipeline (native + web) — entirely unchanged.
- `WebHorizonPoseOverride` / `UiHiddenOverride` marker resources — kept (Step 6 only changes how they're consumed: as schedule conditions vs Option<Res<_>> ladder).

### Changes

- `Cargo.toml`, `Cargo.lock`, `justfile`, `rust-toolchain.toml`, `scripts/lint/wasm-compat.sh` — voxel_noise deletion (Step 1).
- `crates/bevy_naadf/src/camera/mod.rs` — adds `pub mod poses;` (Step 2).
- `crates/bevy_naadf/src/camera/poses.rs` — NEW (Step 2).
- `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs` — const defs move; `use` line added (Step 2).
- `crates/bevy_naadf/src/voxel/vox_import.rs` — tiled family deleted; `parse_dot_vox_data` body absorbed from the tiled twin (Step 3).
- `crates/bevy_naadf/src/voxel/grid.rs` — extensive: helper extraction (Step 4); enum arm + marker deletion (Step 5); pose constant import (Step 2); `let _` drop (Step 4); `parse_to_imported_vox` deletion (Step 7).
- `crates/bevy_naadf/src/voxel/web_vox.rs` — pose constant import (Step 2); `startup_fetch_default_vox` parameter list + body change (Step 5); `hide_ui` + `pin_web_horizon_camera` parameter list change (Step 6).
- `crates/bevy_naadf/src/voxel/async_vox.rs` — `parse_to_imported_vox` callers update to direct dispatch (Step 7).
- `crates/bevy_naadf/src/lib.rs` — `GridPreset::WebSkybox` arm added (Step 5); `.run_if(resource_exists::<_>)` added to `pin_web_horizon_camera` + `hide_ui` registrations (Step 6); minor `use` additions for `resource_exists`.

### Removed

- `crates/voxel_noise/` entire crate (Step 1).
- `vox_import::parse_vox_bytes_tiled` (Step 3) — callers: zero post-deletion.
- `vox_import::load_vox_tiled` (Step 3) — callers landed on `vox_import::load_vox`.
- `vox_import::replicate_buckets_xz` (Step 3) — callers: zero post-deletion.
- `vox_import::parse_dot_vox_data_tiled` (Step 3) — body absorbed into renamed `parse_dot_vox_data`.
- `vox_import::tiled_load_expands_world_xz_and_dedups_blocks` unit test (Step 3) — covers a feature being deleted.
- `voxel::grid::WebSkyboxOverride` marker resource (Step 5) — replaced by `GridPreset::WebSkybox`.
- `voxel::grid::parse_to_imported_vox` (Step 7) — callers landed on `voxel_dispatch::parse_voxel_bytes` directly.
- `voxel::grid::install_imported_vox` line 569 `let _ = WORLD_SIZE_IN_VOXELS;` (Step 4 / F9) — pure cleanup.
- `e2e::vox_horizon_parity::HORIZON_CAMERA_POS` + `HORIZON_CAMERA_ROT` (`vox_horizon_parity.rs:71-81`) — moved to `camera/poses.rs` (Step 2).

---

## 8. Open conflicts

None. Every finding addressed sits within D3's path list. The cross-domain interactions (F6 with D6, F4's `lib.rs` registration ordering with D7) are handled by D3 supplying the destination module and the imports; D6's architect/implementor adjusts `e2e/vox_horizon_parity.rs:71-81` to import from `crate::camera::poses` and D7's architect/implementor is informed of the `GridPreset::WebSkybox` enum extension (so `lib.rs`'s enum-using code, if any, can be updated in their domain).

The `crates/voxel_noise/` deletion is user-approved per addendum ("aggressive deletion of non-C#-parity rot is encouraged"). The `MagicaVoxel.cs` faithful-port boundary is respected (no behaviour-divergent changes to `vox_import.rs`'s scene-graph walk).

---

## 9. Decisions & rejected alternatives

### Decision — keep `--vox-gpu-oracle` gate, delete only the tiling implementation

**Alternative considered**: delete the entire `--vox-gpu-oracle` gate + CPU oracle phase + `vox_gpu_oracle_cpu_phase` flag + `install_vox_sized_to_model` + the tiled family + driver.rs's `VoxGpuOracleWarmup` arm.

**Rejected** because D6's explorer keeps the gate (it's a live SSIM comparison surface for the W5 GPU producer chain). The CPU oracle phase is the *only* reference renderer the project has for the GPU-driven tiled-into-fixed-world install path. Deleting it would leave no way to catch a regression in the W5 dispatch chain other than user visual inspection.

The tiling code inside `vox_import.rs` is the only part with zero production callers and no C# counterpart. That's what goes; the gate consumer (`install_vox_sized_to_model`) keeps working through the non-tiled `load_vox`.

### Decision — `camera/poses.rs` as the F6 destination module

**Alternative considered**: place pose constants in `voxel/grid.rs` next to `GRID_SIZE_IN_CHUNKS`.

**Rejected** because `voxel/grid.rs` is the install-builder, not a camera-pose home. Cross-domain consumers (e2e + web_vox + grid itself) would still cross the voxel-module boundary just to grab a camera constant. The `camera/` module already owns `InitialCameraPose` (`camera/mod.rs:50`) and `PositionSplit` — pose constants belong here.

### Decision — `GridPreset::WebSkybox` enum extension over Bevy `States` machinery

**Alternative considered**: model install progression as a `States` transition (`WorldLoadState::{PendingScene, InstallingDefault, Ready}`).

**Rejected** because Bevy `States` is for runtime-changing modes, not for "which install path runs at Startup." The 4 install paths (`Default`, `Vox{path}`, `Empty`, `WebSkybox`) are mutually exclusive and pinned at boot — that's an enum, not a state machine. The User-Q1 framing ("idiom-fit first, LOC reduction is consequence") doesn't push toward `States` here: a `States` enum + `OnEnter` system + 4 install-firing systems is *more* infrastructure than the existing match-arm `Startup` system. The enum extension is the smaller, more direct fit.

### Decision — defer F8 (`Cursor` collision in `cvox_import.rs`)

**Alternative considered**: rename `cvox_import::Cursor` → `LeReader` as a small mechanical rename.

**Rejected for this design** because the rename touches ~10 sites inside `cvox_import.rs` and the smell is fully contained in one file (no external callers see the collision). The file's docstring at `cvox_import.rs:256` acknowledges and rationalises the hand-rolled reader. Lower priority than the cross-domain F1/F2/F6 deletions. Defer to a future cleanup pass.

---

## 10. Assumptions made

- **A1**: The `--vox-gpu-oracle` gate continues to be project-relevant past this refactor. Verified by D6 explorer (`02-exploration.md`) — D6 does not propose deleting it.
- **A2**: The Stage 14 docstring in `grid.rs:73-103` about the `vox_gpu_oracle_cpu_phase` flag is descriptive of the current code, not aspirational. Verified by Read + the matching driver-side handling at `e2e/driver.rs:548-554`.
- **A3**: `voxel_noise` has no consumers from outside `crates/bevy_naadf` in the active workspace. Verified by Grep against the whole `/mnt/archive4/DEV/bevy-naadf/` tree excluding `target/` and `.claude/worktrees/*` (the latter are pre-existing worktree shadows — out of scope for this refactor; their voxel_noise copies are orthogonal to the active workspace).
- **A4**: The `[palette-install]` debug log block is the load-bearing smoke detector per the "DO NOT REMOVE" markers; collapsing its three copies into one site (Step 4) preserves the signal as long as `RUST_LOG=bevy_naadf=debug` still produces one `[palette-install]` line per install with `label=`, `palette_len=`, and `first_5_color_base=` fields. The web-vox-color-divergence regression is detected by *any one* of those lines firing with unexpected values — there's no requirement that *three* lines fire for the detection to work.
- **A5**: `bevy::ecs::common_conditions::resource_exists` is available in Bevy 0.19 (this is the stock crate; verified by precedent — other `.run_if(...)` registrations in the codebase use this primitive).
- **A6**: The `.before(voxel::grid::setup_test_grid)` ordering at `lib.rs:840-841` can be retained as documentation after Step 5 lands. The `AppArgs.grid_preset` mutation in `startup_fetch_default_vox` must precede the read in `setup_test_grid`; Bevy's Startup schedule doesn't guarantee that without the explicit `.before(...)`. (An alternative is to inline the `?skybox=1` check into `setup_test_grid` itself, but that would re-introduce the URL parsing into the install-builder — keeping the web-side check in `startup_fetch_default_vox` is cleaner.)

---

## 11. D6 / D7 coordination notes

### D6 coordination

- **F6 destination**: `crate::camera::poses` is the canonical home for `HORIZON_CAMERA_POS` / `HORIZON_CAMERA_ROT`. D6's architect should note this in their design so `e2e/vox_horizon_parity.rs`'s implementor uses `use crate::camera::poses::{HORIZON_CAMERA_POS, HORIZON_CAMERA_ROT};` instead of defining them locally.
- **F2 — `--vox-gpu-oracle` gate boundary**: D3 deletes the tiling implementation inside `vox_import`; D6 keeps the gate. D6 implementor does NOT need to change `vox_gpu_oracle.rs` for F2 — the CPU phase continues to invoke `install_vox_sized_to_model` (now calling non-tiled `load_vox` under the hood).
- **DUP-6 pin-camera consolidation**: when D6 consolidates the 9 `pin_*_camera` systems, the new shared helper should also consume `crate::camera::poses::HORIZON_CAMERA_POS/ROT` (no other change required from D3's side).

### D7 coordination

- **F4 — `GridPreset::WebSkybox` enum extension**: D7 owns `lib.rs`. The new arm + the `setup_test_grid` registration change happen in D3's territory; D7's implementor should be aware that `GridPreset` grows a fourth arm so any pattern-matching in `lib.rs` (e.g. for diagnostics dumps or CLI argument echoing) covers the new variant.
- **F4 — `.before(setup_test_grid)` ordering**: kept at `lib.rs:840-841` after Step 5 lands. D7's implementor should NOT delete the ordering during their domain's `lib.rs` refactor — it's load-bearing for the `?skybox=1` decision.
- **F5 — `.run_if(resource_exists::<_>)` registrations**: D7's implementor will see the `run_if` chains in `lib.rs:851-855` / `lib.rs:970` if pulling those system registrations into a hypothetical `WebVoxPlugin`. The conditions belong with the registrations; they should travel together if the plugin extraction happens.
- **`AppArgs.vox_gpu_oracle_cpu_phase` stays**: D7 might be tempted to "simplify" the AppArgs struct by removing the cpu_phase flag. **Do not** — the `--vox-gpu-oracle` gate needs it (D6 territory). D7 architect should note the dependency.

---

## 12. Side notes / observations / complaints

1. **The `parse_*` wrapper layering survives by docstring inertia.** Each wrapper (`parse_to_imported_vox`, `parse_voxel_bytes`, `parse_vox_bytes`, `parse_dot_vox_data`, `parse_dot_vox_data_tiled`) carries a substantial docstring explaining "the reason this layer exists." But once F2 lands, the *reasons* collapse — the tiling explanation goes, the magic-byte dispatch explanation stays. F7 deletes the topmost layer; the architecture is happy at 3 layers (`load_vox` → `parse_vox_bytes` → `parse_dot_vox_data`). Tempting to also collapse `parse_vox_bytes` and `parse_dot_vox_data` into one fn, but they serve genuinely different signatures (bytes vs parsed `DotVoxData`) and `parse_dot_vox_data` is the unit-testable entry point. Leave the 3-layer chain.

2. **`compose_default_scene_into_fixed_world` (grid.rs:815-911) deserves a `debug_assert!` for the BlockPtr-sharing invariant.** The function relies on every ground chunk owning its own copy of the 64-block slice (`grid.rs:888-899`) so a brush edit to one ground chunk doesn't silently mutate every other ground chunk via a shared `BlockPtr`. The invariant is documented in the function-level doc + the unit tests at `grid.rs:1230-1312`, but a `debug_assert!(out_blocks.len() as u32 / 64 >= /* expected ground chunks */, ...)` near the end of the function would catch a refactor regression at runtime. **Not in D3's brief**, just flagged for the next AADF-touching refactor session that should pair this assertion with the `block_hash` regime work in D1.

3. **The `[palette-install]` smoke-detector log is over-instrumented for the regression it catches.** Three "DO NOT REMOVE" duplicated debug logs to detect one color-divergence bug feels like Maginot Line construction. Step 4 fixes the duplication; the underlying ergonomic issue (the `VoxelTypes` resource's palette payload should be assertable in a unit test, not solely instrumented at install-time) is a D1 / world-data concern. Flagged here because D3 is the place where the smoke detector currently fires.

4. **The `vox_gpu_oracle_cpu_phase` flag name overstates its scope.** It is a *gate routing flag*, not a *CPU oracle install flag* — true, it does both, but most readers will assume "this flag controls whether the CPU oracle install path runs at all," which is not the case (`install_vox_sized_to_model` is reachable only through this flag, but the flag's purpose is dispatch to the `--vox-gpu-oracle` gate's CPU-phase render). A rename to `vox_gpu_oracle_phase_cpu` (mirroring `vox_gpu_oracle_phase_gpu`) would clarify; out of D3's brief.

5. **The Stage-N docstrings stacked across `grid.rs:73-103`, `voxel/grid.rs:18-25`, `vox_import.rs:200-206`, etc. read like a museum of historical orchestration phases.** As the explorer noted (side note #7). After this refactor's deletions, several Stage-N notes become outdated:
   - `grid.rs:74-86` "vox-gpu-rewrite Stage 2 consolidation" — partially obsolete once `install_vox_sized_to_model` simplifies.
   - `grid.rs:87-99` "Stage 14 escape hatch" — still applies (the flag stays).
   - `vox_import.rs:200-206` "vox-gpu-rewrite Stage 2 tiling note" — obsolete after F2.
   - The "DO NOT REMOVE" markers (`grid.rs:213,339,646`) — obsolete after F3.

   Implementor SHOULD update these docstrings during the relevant migration step (not leave them lying about referring to deleted code). The synthesised replacements should be terse — one line about what install paths exist, not five about which Stage-N landed which scaffolding.

6. **One subjective reaction**: the F1 deletion is the cleanest win of the entire D3 surface. Zero callers, zero risk, biggest LOC drop. If the user runs out of refactor budget after only one step, F1 is the step. Frame this for the implementor: ship F1 first, F1 alone is a valid PR.

7. **Equal-footing observation**: this design respects every forbidden move in the brief (no MagicaVoxel.cs behavior divergence, no e2e harness contract break, no cross-domain edits beyond the cited F6 destination move). The voxel_noise deletion is user-approved per addendum. The tiled-family deletion is consistent with both the master-branch identity ("minimal C# port") and the faithful-port rule (tiling has no C# counterpart, so deleting it removes a divergence). The `WebSkyboxOverride` collapse into `GridPreset::WebSkybox` is structural; no observable behaviour change.

8. **The `streaming-world` orchestration's design doc (`docs/orchestrate/streaming-world/02-design.md`) names `voxel_noise` as a future dep.** Deleting the crate forecloses that path in its current shape. The user's "everything else can go" Q2 decision + the addendum's "aggressive deletion … is encouraged" line authorise this; I'm flagging it solely so a future architect/orchestrator who revives streaming-world knows to re-add the dep from the upstream `bevy_voxel_world` crate. Not blocking.

9. **The `wasm32-unknown-emscripten` target drop in `rust-toolchain.toml`** removes the *only* reason that target exists in the workspace. After F1 lands, no Rust code in the workspace targets emscripten. This shaves time off every `rustup` install. Minor but real.

