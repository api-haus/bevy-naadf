# 02a — Design — Track A: VOX loading

**Date:** 2026-05-15
**Author:** delegate-architect
**Branch:** `main` at HEAD (post `1c35c7f`)
**Brief:** orchestrator-supplied; covers Track A only — large MagicaVoxel `.vox` world loading. `obj2voxel` is deferred entirely (per `01-context.md` §2 Q&A row 2 + §5 forbidden moves).

## Overview

The loader uses the `dot_vox` 5.2 crate to parse a `.vox` file into a `DotVoxData` struct, then a new `voxel/vox_import.rs` module flattens the scene-graph models into a `DenseVolume` (Phase-A path, `aadf/construct.rs:129`), promotes the 256-entry MagicaVoxel palette into `VoxelType` entries (one `VoxelType` per used palette index, no K-means), and installs the resulting `(DenseVolume, VoxelType[])` pair into the existing `setup_test_grid` flow as an alternate `GridPreset::Vox { path }`. The first cut is **runtime-via-`std::fs` at startup** (no Bevy `AssetLoader`, no offline pre-bake) — the simplest one-shot consistent with the audit's `≤256³` recommendation.

Rationale: this lands `.vox` rendering on a single CLI flag with zero new render-side seams, reuses the entire Phase-C/A CPU oracle pipeline (`construct()` → `WorldData` → existing GPU upload / GPU producer chain), and side-steps the AssetLoader-vs-pre-bake split until either is shown to be load-bearing. The audit's K-means K-means stage turns out to be a misread of `ModelData.cs` — the `.vox` C# path at `:502-522` creates one `VoxelType` per palette entry directly (256 entries max); K-means at `:528-560` is only called from `.vl32` import (`:587+`). Avoiding K-means simplifies the first cut considerably and removes a borderline dep call.

## Architecture

### Module layout

```
crates/bevy_naadf/src/voxel/
├── mod.rs                  # (existing — VoxelType, VoxelTypeId, MaterialBase, MaterialLayer)
├── grid.rs                 # (existing — setup_test_grid; gains GridPreset::Vox dispatch + load_vox_preset)
└── vox_import.rs           # NEW — DotVoxData → (DenseVolume, Vec<VoxelType>) glue + tests
```

The new module sits in `voxel/` because that's where `VoxelType` palette construction lives (`voxel/mod.rs:111`) and where the test-grid content path lives (`voxel/grid.rs`). It is **not** placed under `aadf/` because it doesn't author any AADF state — it produces inputs for `aadf::construct::construct()` (the existing Phase-A oracle).

No new files under `render/`, `world/`, or `aadf/`. No edits to `aadf/generator.rs`, `aadf/construct.rs`, `world/data.rs`, or any shader. The GPU side does not learn about `.vox` at all; the moment the loader has produced a `DenseVolume`, the existing `construct()` → `WorldData` → `extract` → `prepare_world_gpu` chain takes over unchanged.

### Cargo.toml dep changes

One new line added to `crates/bevy_naadf/Cargo.toml` under `[dependencies]` (lines 34-72 region):

```toml
# dot_vox = MagicaVoxel .vox parser. Returns `DotVoxData { models, palette, materials, scenes }`
# in the same shape the C# `MagicaVoxel.cs` parser produces; we run a port-side
# flattening pass (`voxel/vox_import.rs`) to fold the scene-graph into a DenseVolume.
# Used only by Track A `.vox` loading; no transitive native deps (pure-Rust nom parser).
dot_vox = "5.2"
```

Default features (`ahash`) are fine — `ahash` is permissive-licensed and already in many Bevy transitive trees.

**No K-means dep.** The `.vox` palette is ≤256 entries; we create one `VoxelType` per used palette index. K-means is only needed for `.vl32` (per C# `ModelData.cs:528-560` which is called from `ImportFromVL32` at `:587+`, not `ImportFromVox`).

**No `bin/bake.rs` extension.** Pre-bake is intentionally out of scope for the first cut (see `## Decisions` below). The hook lives in `GridPreset::Vox { path }` and is a future-extension point.

### Public API surface

`crates/bevy_naadf/src/voxel/vox_import.rs` exposes:

```rust
/// Parsed-and-flattened `.vox` data, ready to install into a NAADF world.
pub struct ImportedVox {
    /// The flattened dense voxel volume, sized to the smallest cuboid that
    /// covers every visible voxel across the file's scene graph (mirrors C#
    /// `MagicaVoxel.Flatten` at MagicaVoxel.cs:677-689).
    pub volume: DenseVolume,
    /// The voxel-type palette derived from the `.vox` RGBA chunk + MATL chunks
    /// (index 0 is the reserved empty placeholder; indices 1..=N mirror the
    /// MagicaVoxel palette entries 1..=N).
    pub palette: Vec<VoxelType>,
}

/// Errors emitted by [`parse_vox_bytes`] / [`load_vox`].
#[derive(Debug, thiserror::Error)]
pub enum VoxImportError {
    #[error("dot_vox parse failed: {0}")]
    Parse(&'static str),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("VOX size {dim:?} exceeds wgpu max_texture_dimension_3d ({limit} voxels per axis)")]
    SizeExceedsTextureLimit { dim: [u32; 3], limit: u32 },
    #[error("VOX size {dim:?} would exceed the {bytes}-byte budget for `dense_voxel_types`")]
    SizeExceedsBudget { dim: [u32; 3], bytes: u64 },
    #[error("VOX contains no models")]
    Empty,
}

/// Parse `.vox` bytes and flatten the scene graph into a single `ImportedVox`.
///
/// This is the unit-testable entry point — pure CPU, no Bevy resources, no
/// filesystem. Mirrors C# `MagicaVoxel.Flatten` at MagicaVoxel.cs:677-689.
pub fn parse_vox_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxImportError>;

/// Convenience: load a `.vox` file from disk via `std::fs::read` + parse.
pub fn load_vox(path: impl AsRef<std::path::Path>) -> Result<ImportedVox, VoxImportError>;

/// Apply an `ImportedVox` to a fresh `WorldData` + `VoxelTypes` pair, exactly
/// the way `setup_test_grid` builds them from `build_default_volume` +
/// `build_palette` today (`voxel/grid.rs:66-110`). Returns the two resources
/// the caller inserts via `Commands::insert_resource`.
pub fn build_world_from_vox(imported: ImportedVox) -> (WorldData, VoxelTypes);
```

**Why three layers (parse / load-from-disk / install).** Keeping `parse_vox_bytes` separate from `load_vox` lets the `#[test]` cover the parse-and-flatten path against a checked-in `assets/test/*.vox` fixture without a real filesystem dependency (uses `include_bytes!`). Keeping `build_world_from_vox` separate from `load_vox` lets `setup_test_grid` reuse the install half independently if a future caller (Bevy `AssetLoader` extension, pre-bake binary) wants to install an `ImportedVox` it produced through a different path.

### How loading integrates with `setup_test_grid`

`voxel/grid.rs:50` becomes:

```rust
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub enum GridPreset {
    /// The hard-coded Phase-A test scene (ground + emissives + ...).
    #[default]
    Default,
    /// Load a MagicaVoxel `.vox` file from disk (path relative to repo root or
    /// absolute). The file is read once at `Startup`; failure logs an error and
    /// falls back to `Default` so the e2e harness still has a renderable world.
    Vox { path: std::path::PathBuf },
}
```

`voxel/grid.rs::setup_test_grid` (`voxel/grid.rs:66`) gains one new arm:

```rust
let (palette, volume) = match &args.grid_preset {
    GridPreset::Default => (build_palette(), build_default_volume()),
    GridPreset::Vox { path } => match vox_import::load_vox(path) {
        Ok(imp) => (imp.palette, imp.volume),
        Err(e) => {
            error!("VOX load failed ({e}); falling back to default test grid");
            (build_palette(), build_default_volume())
        }
    },
};
// ... unchanged from here down: construct(&volume), build dense_voxel_types,
// build WorldData + VoxelTypes resources, insert.
```

This **preserves the existing e2e content path** exactly: when `args.grid_preset == GridPreset::Default` (the default for both `bevy-naadf` and `e2e_render` binaries), nothing changes — the hard-coded test grid still runs and all four e2e modes (baseline · `--validate-gpu-construction` · `--edit-mode` · `--entities`) stay byte-identical. `.vox` loading is a **new arm** of the preset enum, opt-in only.

### CLI flag

A new `--vox <path>` flag on the production `bevy-naadf` binary (`src/main.rs`). The e2e harness does NOT add this flag — its content path stays the default test grid (per `01-context.md` §5 forbidden move: "Do NOT silently break the e2e harness"). Parsing:

```rust
// in src/main.rs (production app — gets its own CLI parser; e2e binary doesn't)
let args = std::env::args().collect::<Vec<_>>();
let mut app_args = AppArgs::default();
if let Some(idx) = args.iter().position(|a| a == "--vox") {
    if let Some(path) = args.get(idx + 1) {
        app_args.grid_preset = GridPreset::Vox { path: path.into() };
    }
}
```

This is intentionally tiny (no `clap`, no `argh`) — the project already uses ad-hoc argv parsing for similar one-off flags (e.g., `e2e_render`'s mode flags) and adding a CLI-parser crate is out-of-scope for Track A.

### K-means stage — NOT applied to `.vox`

Per C# `ModelData.cs:502-522` (the `.vox` path), **one `VoxelType` is created per source palette entry**:

```csharp
types = new VoxelType[dataImport.Colors.Length];
for (int c = 0; c < dataImport.Colors.Length; c++) {
    Vector3 colSRGB = new Vector3(R, G, B) / 255;
    type.colorBase = pow(colSRGB, 2.2f);       // sRGB → linear
    float emission = mat.emit * pow(1 + mat.flux, 2) * 5;
    type.materialBase = (emission > 0) ? Emissive : Diffuse;
    type.colorLayered.X = emission;            // emissive intensity into the X channel
    types[c] = ApplyVoxelType(type);           // dedup-and-insert into the world's VoxelTypeHandler
}
```

No K-means; the palette is already ≤256 entries (MagicaVoxel max). K-means in `ModelData.cs:528-560` is `MapColorsToPaletteIndices`, called from `ImportFromVL32` (`:587+`) — not from `.vox` import. The brief's row 2 of "K-means stage" (`01-context.md` §2 Q&A) and the audit's §2.3 line on K-means are both based on an over-broad reading of `ModelData.cs:528-560`. **K-means is therefore not part of Track A scope.** See `## Decisions` for the explicit rejection of the audit's recommendation here.

The port-side equivalent in `voxel/vox_import.rs` builds a `Vec<VoxelType>` of length `palette.len() + 1` (index 0 = `VoxelType::default()` placeholder, indices 1..=N = converted palette entries). The conversion mirrors the C# pseudocode above:

```rust
fn vox_palette_to_voxel_types(
    palette: &[dot_vox::Color],
    materials: &[dot_vox::Material],
) -> Vec<VoxelType> {
    let mut out = Vec::with_capacity(palette.len() + 1);
    out.push(VoxelType::default()); // reserved empty placeholder at index 0
    for (i, color) in palette.iter().enumerate() {
        let srgb = Vec3::new(color.r as f32, color.g as f32, color.b as f32) / 255.0;
        let linear = Vec3::new(srgb.x.powf(2.2), srgb.y.powf(2.2), srgb.z.powf(2.2));
        // Look up the material at this palette slot. dot_vox emits 256 Material
        // entries (default `_diffuse` for unset slots); `materials[i]` is safe.
        let mat = materials.get(i);
        let (emit, flux) = mat
            .and_then(|m| {
                let e: f32 = m.properties.get("_emit")?.parse().ok()?;
                let f: f32 = m.properties.get("_flux").and_then(|s| s.parse().ok()).unwrap_or(0.0);
                Some((e, f))
            })
            .unwrap_or((0.0, 0.0));
        let emission = emit * (1.0 + flux).powi(2) * 5.0;   // C# formula
        out.push(VoxelType {
            color_base: linear,
            material_base: if emission > 0.0 { MaterialBase::Emissive } else { MaterialBase::Diffuse },
            material_layer: MaterialLayer::None,
            roughness: 1.0,            // C# does not set roughness in this branch
            color_layered: if emission > 0.0 { Vec3::new(emission, 0.0, 0.0) } else { Vec3::ZERO },
        });
    }
    out
}
```

The `VoxelTypeId` semantics in the port are unchanged: `dot_vox::Voxel.i` is already 0-based (`dot_vox` does the 1-based → 0-based conversion at `dot_vox::model::parse_voxel:74`); we map `voxel.i: u8` → `VoxelTypeId(voxel.i as u16 + 1)` so that index 0 stays the empty placeholder.

### Scene-graph flattening algorithm

The C# `MagicaVoxel.Flatten` (`MagicaVoxel.cs:677-689`) walks the scene-graph from `Nodes[0]` (root transform), accumulates a world AABB via `GetWorldAABB`, allocates a single `VoxelDataBytes` sized to the AABB, then collates every visible `ShapeNode`'s model voxels into it under the accumulated transform. `dot_vox::DotVoxData::scenes` exposes the same scene-graph (`scenes[0]` is always the root per the crate's docs — see `dot_vox::scene` module + `lib.rs:19`); we port the same two-pass algorithm:

```rust
// Pseudocode — port of MagicaVoxel.Flatten (MagicaVoxel.cs:677-689) +
// GetWorldAABB/CollateVoxelData (:651-716).
fn flatten_scene(data: &DotVoxData) -> Result<DenseVolume, VoxImportError> {
    if data.models.is_empty() { return Err(VoxImportError::Empty); }
    if data.scenes.is_empty() {
        // No scene graph (older .vox version) — just use models[0] directly
        // (mirrors C# `MagicaVoxel.cs:687` `else { return Models[0]; }`).
        return Ok(model_to_dense_volume(&data.models[0]));
    }
    // Pass 1: world AABB by walking the scene graph from scenes[0] under identity.
    let aabb = compute_world_aabb(data);
    // Pass 2: allocate DenseVolume sized to aabb, collate every visible shape's
    // model voxels into it (transform applied per shape).
    let mut volume = DenseVolume::empty(round_up_to_chunks(aabb.size()));
    collate_voxel_data(data, &mut volume, aabb.min);
    Ok(volume)
}
```

For the **first cut**, the design **simplifies the scene-graph walk to a single-frame, identity-only walk**: rotations and translations from `SceneNode::Transform` are not applied (a `SceneNode::Transform`'s `frames[0]` rotation+translation is taken as `_t=0 0 0`, `_r=identity`). The flattener treats `nGRP` as a flat list of `nSHP` children and concatenates their model AABBs. This is the **lossy mode** — captured under `## Assumptions made`. It correctly handles single-model `.vox` files (the common case, every test fixture we plan to ship) and multi-model files where every model sits at the origin under the identity transform. Multi-model files with non-trivial transforms render at the wrong position; that's a follow-up.

**Why this simplification.** Full scene-graph transform composition is ~150-200 LOC of careful 4×4-matrix math (the `dot_vox::Rotation::to_quat` / `Frame::position` API + matrix-multiply chain). The brief asks for "simplest one-shot"; identity-only walk is ~30 LOC. Multi-model + transforms can land in a follow-up under the same `parse_vox_bytes` API surface.

### Coordinate-system mapping

MagicaVoxel uses **right-handed Z-up** (per `dot_vox::Voxel` doc at `model.rs:43-44`). The port uses NAADF's coordinate convention; per `aadf/generator.rs::ModelData::sizeInChunks` field comments (`generator.rs:80-83`) and the C# `ModelData.ImportFromVox` at `ModelData.cs:386-387` + `:413-414`, NAADF **swaps Y and Z** when ingesting from `.vox`:

```csharp
// ModelData.cs:386
Point3 modelSize = new Point3(totalBounds.Size.X, totalBounds.Size.Z, totalBounds.Size.Y);
// ModelData.cs:438
uint typeImport1 = dataImport[new Voxels.XYZ(voxelPos.X, voxelPos.Z, voxelPos.Y)].Index;
```

The port mirrors this **exactly**: `dot_vox::Voxel { x, y, z }` → NAADF `[x, z, y]`. The port's `DenseVolume` is x-fastest then y then z (`aadf/construct.rs:62`), so we write each `.vox` voxel `(vx, vy, vz, i)` as `volume.set([vx as u32, vz as u32, vy as u32], VoxelTypeId(i as u16 + 1))`.

### Loading-mechanism choice

**First cut: synchronous `std::fs::read` at `Startup` system inside `setup_test_grid`.** No Bevy `AssetLoader`, no offline pre-bake. See `## Decisions` for why.

Extension point: a future `AssetLoader<DotVoxAsset>` can wrap `parse_vox_bytes` directly and emit `ImportedVox` as the loaded asset; `setup_test_grid` could then be split into a `Startup` system that issues `asset_server.load(path)` and an `Update` system that polls until the asset is ready, then runs `build_world_from_vox`. The clean seam is `parse_vox_bytes` (no Bevy types) → `ImportedVox` (no Bevy types) → `build_world_from_vox` (returns Bevy `Resource`s). Either an `AssetLoader` or a `bake.rs` extension consumes the first two, produces the third.

### Size ceilings

The design **guarantees** correct rendering for `.vox` files up to `256³` voxels per axis (`u8`-per-axis ceiling baked into `dot_vox::Voxel { x: u8, y: u8, z: u8 }` at `model.rs:46-58`). A single `.vox` model can therefore never exceed 256³; multi-model files could in principle compose to a larger AABB, but per the simplified-scene-graph rule above, multi-model `.vox` files at non-trivial transforms aren't fully supported in the first cut anyway.

The design **soft-checks** against the wgpu `max_texture_dimension_3d` ceiling (`render/prepare.rs:206-280` allocates the chunks 3D texture; the audit §1 row 9 cites the typical 2048 / Vulkan-minimum 1024 cap) by emitting `VoxImportError::SizeExceedsTextureLimit` when the resulting `size_in_chunks.{x,y,z}` would exceed `1024` (a conservative ceiling — actual wgpu limit is queried only at render-app init, which is too late for the loader to consult). The error message names the actual limit so the user can rescale.

The design also **soft-checks** the `dense_voxel_types: Vec<u16>` budget in `world/data.rs:50`: for a 1024³ world that's 2 GiB of CPU memory — well past any reasonable budget. The check is `volume.size_in_voxels.x * y * z * 2 > 512_MiB` → `VoxImportError::SizeExceedsBudget`. 512 MiB at 2 B/voxel = `2^28` voxels ≈ 645³ — comfortably above any test fixture.

For files past these limits, the loader emits the error, `setup_test_grid` logs it, and falls back to `GridPreset::Default`. No panic.

### Coupling to the GPU producer chain (`gpu_construction_enabled`)

The Phase-C followup #1 GPU producer chain (`render/construction/mod.rs:815-870`) reads `WorldData::dense_voxel_types` to rebuild `segment_voxel_buffer` from scratch on the GPU at startup. This is **already wired** for any `DenseVolume`-authored world; `setup_test_grid` populates `dense_voxel_types: Vec<u16>` from `volume.voxels.iter().map(|t| t.0)` at `voxel/grid.rs:90`. The `.vox` path goes through the same `construct(&volume)` + `dense_voxel_types: Vec<u16>` assignment, so `gpu_construction_enabled = true` continues to work transparently with `.vox` worlds. **No edits to `render/construction/`.**

## File-by-file change list

### New files

| Path | Purpose | Approx LOC |
|---|---|---|
| `crates/bevy_naadf/src/voxel/vox_import.rs` | `dot_vox::DotVoxData` → `(DenseVolume, Vec<VoxelType>)` glue + scene-graph flattener + palette converter + unit tests | ~250 (180 prod + 70 tests) |
| `crates/bevy_naadf/src/assets/test/single_voxel.vox` | Checked-in 1-voxel `.vox` fixture (≤200 B) — single red voxel at (0,0,0), index 1 in default palette | ~200 bytes |
| `crates/bevy_naadf/src/assets/test/small_cube.vox` | Checked-in 8×8×8 `.vox` fixture — 7×7×7 solid cube of one palette color + 1 emissive voxel; sized to fit one chunk | ~5 KB |

(The two `.vox` fixtures are committed as binary blobs; `voxel/vox_import.rs` `#[test]`s consume them via `include_bytes!`.)

### Edited files

| Path | Edit | Approx LOC delta |
|---|---|---|
| `crates/bevy_naadf/Cargo.toml` | Add `dot_vox = "5.2"` under `[dependencies]` between `bytemuck` and `image` (alphabetical order suggests after `bytemuck`); add a short comment matching the surrounding doc-comment style. | +4 lines |
| `crates/bevy_naadf/src/voxel/mod.rs` | Add `pub mod vox_import;` declaration alongside the existing `pub mod grid;` (currently at `voxel/mod.rs:` — verify with Grep before edit; the existing `pub mod` block is the natural insertion point). | +1 line |
| `crates/bevy_naadf/src/lib.rs` | Extend `GridPreset` enum (`lib.rs:50`) with a `Vox { path: PathBuf }` variant. Update the `Default` impl note + the `#[derive]` (PathBuf isn't `Copy` — drop the `Copy` derive on `GridPreset`; `AppArgs` is `Copy` (line 189), so propagate this through: drop `Copy` from `AppArgs` too, audit the four `Res<AppArgs>` clone sites in the tree — `args: Res<AppArgs>` already uses by-ref access; the only `Copy`-using call site is `args.gi` field reads which are fine). | +10/−2 lines |
| `crates/bevy_naadf/src/main.rs` | Add the `--vox <path>` arg parse, mutate `AppArgs::grid_preset` before `build_app_with_args`. Verify whether `main.rs` already has any argv parsing first; if not, add the minimal `std::env::args` block. | +15 lines |
| `crates/bevy_naadf/src/voxel/grid.rs` | Extend `setup_test_grid` (`grid.rs:66`) with the `GridPreset::Vox` match arm calling `vox_import::load_vox`. Extract `build_default_volume` (`grid.rs:234`) — unchanged. Extract `build_palette` (`grid.rs:114`) — unchanged. | +12 lines |
| `crates/bevy_naadf/src/voxel/grid.rs` | (Optional, follow-up) Mark `fill_box` / `fill_sphere` (`grid.rs:300,315`) `pub` for Track B reuse — **NOT in Track A scope**; leaving for Track B's design. | +0 lines (Track A) |
| `crates/bevy_naadf/README.md` or new `docs/.../vox-loading.md` | Document the `--vox <path>` flag + supported size limits. **Outside the design's scope per `## Out of scope`; implementer may add.** | +0 lines (Track A; doc follows impl) |

**Total new code (Track A):** ~290 LOC (180 prod + 70 tests + 25 wiring + small fixture binaries).

### Files NOT touched

For clarity (these are the surfaces a reviewer should *not* see in the Track A diff, per the audit's reuse plan + `01-context.md` §5 forbidden moves):

- `aadf/construct.rs` / `aadf/generator.rs` / `aadf/edit.rs` / `aadf/bounds.rs` / `aadf/entity.rs` — all reused unchanged
- `render/` (the entire tree) — no GPU-side changes
- `world/data.rs` / `world/buffer.rs` — `WorldData` constructed as today
- `world_change.wgsl` / `chunk_calc.wgsl` / `generator_model.wgsl` — no shader edits
- `bin/bake.rs` — no extension; the InstaMAT-style pre-bake template stays untouched for a future iteration
- `panel.rs` / `hud.rs` — Track B surfaces, no Track A involvement
- `e2e/` — no e2e harness changes; the harness keeps using `GridPreset::Default`

## Test plan

### `#[test]` coverage (in `voxel/vox_import.rs`)

1. **`parses_single_voxel_fixture`** — `parse_vox_bytes(include_bytes!("../assets/test/single_voxel.vox"))` returns `Ok`. Asserts the resulting `ImportedVox` has `volume.size_in_chunks == [1, 1, 1]`, a single non-empty voxel at the expected world position (origin, accounting for Z↔Y swap), and a palette length ≥1 + 1 (one MagicaVoxel default palette entry used + the placeholder).
2. **`parses_small_cube_fixture`** — same, for the 8×8×8 fixture; asserts the volume has exactly 7³ non-empty voxels of the expected `VoxelTypeId` + 1 emissive voxel, and the palette has at least 2 distinct used `VoxelType`s (one diffuse + one emissive). The emissive check verifies the `MATL` chunk → `MaterialBase::Emissive` translation works.
3. **`palette_index_zero_is_empty_placeholder`** — asserts `imp.palette[0] == VoxelType::default()` always.
4. **`palette_emissive_from_matl`** — uses a hand-built `DotVoxData` (no fixture file) with one `Material { _type: "_emit", _emit: "1.0", _flux: "0.0" }` at palette index 5, and asserts `imp.palette[6].material_base == MaterialBase::Emissive` and `color_layered.x > 0.0`. (palette index 5 → `VoxelTypeId(6)` because we shift by +1 for the placeholder.)
5. **`zy_swap_matches_csharp`** — a `DotVoxData` with one voxel at `(x=1, y=2, z=3)` in MagicaVoxel coords → `volume.voxel_at([1, 3, 2])` is non-empty (Z↔Y swap, per `ModelData.cs:386`).
6. **`size_exceeds_texture_limit_errors`** — a `DotVoxData` with `Model { size: { x: 16_400, y: 1, z: 1 } }` returns `Err(VoxImportError::SizeExceedsTextureLimit { .. })`. (Yes, `dot_vox::Size::x` is `u32` even though `Voxel.x` is `u8` — the size is independent of voxel coords; a model can declare a 16k×1×1 size with no voxels. Catches the texture-limit branch.)
7. **`empty_models_errors`** — `DotVoxData { models: vec![], .. }` returns `Err(VoxImportError::Empty)`.
8. **`construct_runs_on_imported_volume`** — calls `aadf::construct::construct(&imp.volume)` on the small-cube fixture's imported volume and asserts the resulting `ConstructedWorld` is non-empty (chunks/blocks/voxels all `> 0`). Wires the loader to the existing oracle path end-to-end without spinning up Bevy or a GPU.

### Smoke gate (`cargo build` + `cargo test`)

- `cargo build --no-default-features` builds clean (workspace) with `dot_vox` added.
- `cargo build` builds clean with the default-features path (DLSS on).
- `cargo test -p bevy-naadf` runs all the new tests + the existing test suite green.

### Manual visual gate (USER's responsibility, per global memory `subagent-gpu-app-verification-loop`)

The user runs `cargo run -- --vox crates/bevy_naadf/src/assets/test/small_cube.vox` once and confirms the 7³ cube renders correctly on screen. The architect / implementer agents do NOT attempt to verify visual output (per the global memory note). The implementer agent's terminal smoke gate is `cargo build && cargo test`; visual gating is human.

### Existing e2e modes must still pass

Per `01-context.md` §5 forbidden move 10: the four e2e modes — baseline · `--validate-gpu-construction` · `--edit-mode` · `--entities` — all use `GridPreset::Default`, so they're byte-identical to today. The implementer's review checklist must include "ran `cargo run --bin e2e_render` in all four modes; all four pass." (Per memory `subagent-gpu-app-verification-loop`, ONE smoke run max per sub-agent; the user can re-verify if anything looks off.)

## Decisions & rejected alternatives

### Decision 1: Parsing library — `dot_vox` crate ✅ vs. transliterate `MagicaVoxel.cs`

**Chosen:** `dot_vox` 5.2 crate.

**Rejected:** Hand-transliterating C# `MagicaVoxel.cs` (~750 LOC: chunk-tagged binary parse, scene-graph nodes, palette + material chunks, `Flatten` + `GetWorldAABB`).

**Why:**
- The crate is MIT-licensed, ~5 years old, dust-engine (well-maintained), pure-Rust nom parser, no native deps. Its `DotVoxData { models, palette, materials, scenes, layers, index_map }` shape (cached source at `/home/midori/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/dot_vox-5.2.0/src/dot_vox_data.rs:6-23`) is a strict superset of what `MagicaVoxel.cs` produces. It even pre-handles the 1-based→0-based palette index off-by-one (`dot_vox::model::parse_voxel:74` — `i.saturating_sub(1)`), which is **the** quirk the audit's borderline-row-1 flagged as a flip-trigger.
- Transliterating ~750 LOC of binary-parse + scene-graph state-machine is high implementer-time cost for zero faithfulness gain — there's no shipping `.vox` whose rendering depends on `MagicaVoxel.cs`'s specific implementation quirks.
- User explicitly said "we dont really care about deps" (`01-context.md` §2 Q&A row K-means).

**Flip-trigger:** if a target `.vox` fixture parses through `dot_vox` but doesn't render the expected geometry, AND the failure can be traced to a `dot_vox` parser bug (e.g., not handling the `IMAP` chunk at `MagicaVoxel.cs:415-424`), fall back to hand-transliterating only the affected chunk handler within the same `voxel/vox_import.rs` module via a `dot_vox`-output post-process. (Audit row 1 also calls this out.)

### Decision 2: K-means impl — NONE (out of scope for `.vox`) ✅ vs. hand-rolled Lloyd's vs. `kmeans` crate

**Chosen:** No K-means in Track A.

**Rejected:** Both hand-rolled Lloyd's (~50 LOC) AND the `kmeans` crate.

**Why:**
- The C# `.vox` import path (`ModelData.cs:356-526`) creates one `VoxelType` per palette entry directly at `:502-522`. The 256-entry palette is already small enough to render as-is; no clustering is performed.
- K-means at `ModelData.cs:528-560` (`MapColorsToPaletteIndices`) is called from `ImportFromVL32` at `:587+`, NOT from `ImportFromVox`. This is a misread in both the audit (§2.3) and the brief (step 5 + the `01-context.md` references at lines 30-31, 56). The fix: K-means is a `.vl32` concern, not a `.vox` concern, and `.vl32` is out of Track A scope (Track A is `.vox` only, per `01-context.md` §1).
- Adding either K-means impl for `.vox` would be **gold-plating** — code path with no caller, increases failure surface.

**Flip-trigger:** if `.vl32` import lands as a follow-up Track, **then** add K-means. Hand-rolled Lloyd's (≤50 LOC) is the simplest one-shot; the `kmeans` crate adds a dep tree (`linfa-clustering` pulls `ndarray` + BLAS) that's overkill for ≤256-cluster runs. **Hand-rolled Lloyd's wins when K-means is needed.** This decision is recorded so the future `.vl32` implementer doesn't re-debate it.

### Decision 3: Ingestion target — `DenseVolume` + `construct()` ✅ vs. `ModelData` + `generate_segment_cpu`

**Chosen:** Parse → `DenseVolume` → `aadf/construct.rs::construct()` (the audit's "simpler path"; Phase-A oracle).

**Rejected:** Parse → `ModelData` (the audit's "faithful path"; mirrors C# `ImportFromVox` end-to-end producing `dataChunk`/`dataBlock`/`dataVoxel` byte arrays + the W5 GPU dispatch driving final world build).

**Why:**
- The brief's "simplest working port first" guidance + user's "we don't care about bit parity" + the audit's note that `construct()` works for `≤256³` is a perfect match for `dot_vox`'s `u8`-per-axis size cap (256³ ceiling baked in). The simpler path covers 100% of in-scope file sizes without any additional GPU-coordination work.
- Going through `ModelData` would require: (a) porting the open-addressing CAS-loop hash-dedup at `ModelData.cs:430-499` (~70 LOC), (b) wiring the W5 GPU dispatch from a runtime `Startup` system (today the W5 dispatch is only fired by the W1 startup chain when `gpu_construction_enabled = true`, AND only with `dense_voxel_types` already populated — and the `dense_voxel_types` source-of-truth IS the `DenseVolume`!), (c) reconciling the per-world `dense_voxel_types` Vec with a per-model `ModelData` byte triple. None of this saves implementer time vs. `DenseVolume`.
- **Critical**: the existing `gpu_construction_enabled = true` path already runs the W5 GPU dispatch — it just sources `segment_voxel_buffer` from `WorldData::dense_voxel_types` (`render/construction/mod.rs:827`), which we already produce. So choosing `DenseVolume` does NOT lose the GPU oracle path; it just means CPU `construct()` is the **start** of the chain, GPU W5 producer is downstream as today.
- The audit's row 7 (`ModelData` "structural target") describes the W5 oracle's input *shape* — but in the port's wired-up code, the source-of-truth for that shape **is** `DenseVolume::voxels` (mirrored as `dense_voxel_types`). The `ModelData` byte triple is only used for the W5 unit tests (`render/construction/mod.rs:2870-3050`); production never consumes it as runtime input.

**Flip-trigger:** if a `.vox` fixture larger than 256³ enters scope (it can't, per `dot_vox::Voxel { x: u8, y: u8, z: u8 }`), AND CPU `construct()` runtime exceeds 5 seconds, fall back to the `ModelData`+W5 path. Neither condition is plausible inside this orchestration.

### Decision 4: Loading mechanism — synchronous `std::fs::read` at `Startup` ✅ vs. Bevy `AssetLoader` vs. offline pre-bake

**Chosen:** Synchronous `std::fs::read` inside the `setup_test_grid` `Startup` system, gated on `GridPreset::Vox { path }`. **No** Bevy `AssetLoader`, **no** `bin/bake.rs` extension.

**Rejected (a):** Bevy `AssetLoader<DotVoxAsset>` registered via `app.init_asset::<DotVoxAsset>().register_asset_loader(DotVoxAssetLoader)` — runtime async load through `AssetServer`, polled in `Update`, world built when handle resolves.

**Rejected (b):** `bin/bake.rs` extension that pre-bakes a `.vox` → port-native binary blob (like `.cvox`), loaded synchronously at startup.

**Why chosen:**
- **Simplicity-in-one-shot.** Synchronous startup-time `std::fs::read` is ~5 LOC; Bevy `AssetLoader` is ~80 LOC plus the polling state machine (`setup_test_grid` would need to split into a "kick off load" `Startup` system + a "world built?" `Update` system + a marker resource). For files ≤10 MiB (every plausible `.vox` fixture), startup-time load adds <100 ms of boot time — invisible.
- **Reuses the existing `setup_test_grid` shape.** The test grid is built synchronously at `Startup` today via `setup_test_grid` (`voxel/grid.rs:66`); the `.vox` arm slots in identically. Zero new render/extract/prepare changes; the `dirty: true` flag on `WorldData` already triggers GPU upload on the first frame after.
- **Test-friendliness.** `parse_vox_bytes` is pure, takes `&[u8]`, returns a Result — trivially `#[test]`-able against `include_bytes!`-loaded fixtures. The Bevy `AssetLoader` path adds the asset pipeline as a test dep (the headless test fixture needs an `AssetServer`, file-system reader, etc.).

**Why audit's "support both" is overridden:**
- The audit's `## 4` borderline-call recommendation was *"support both"* on the grounds that "large `obj2voxel` outputs (1k³ chunks, ~1 GiB encoded) DO want offline pre-bake." But `obj2voxel` is now deferred entirely (per `01-context.md` §2 Q&A row 2 + §5 forbidden move 7), and a pure `.vox` file is capped at 256³ per the `dot_vox::Voxel` u8 dimensions. No `.vox` in scope is "large enough to want pre-bake" — the audit's pre-bake motivation was specifically the deferred `obj2voxel` case.
- Pre-bake **adds** failure surface (a second binary that must stay in sync with the runtime loader, a `.cvox`-like format that must version, an InstaMAT-style `justfile` step) — net negative for the brief's "simplest one-shot."
- AssetLoader **adds** Bevy-API surface area for no in-scope user benefit (no per-frame `.vox` hot-reload requested, no asset server in the test path).

**Extension point:** `parse_vox_bytes(&[u8]) -> Result<ImportedVox, _>` is the seam. If either AssetLoader or pre-bake is needed later, it wraps this function and emits `ImportedVox` — no changes to `setup_test_grid` or any downstream code. The chosen design **does not foreclose** either alternative.

**Flip-trigger:** if a `.vox` fixture above ~64 MiB ships, OR if hot-reload of `.vox` (edit in MagicaVoxel → world updates without restart) becomes a stated requirement, switch to `AssetLoader`. Pre-bake remains a deferred future option (would replace the runtime loader entirely, not supplement it).

### Decision 5: Coordinate convention — apply C#'s Z↔Y swap ✅ vs. preserve MagicaVoxel's Z-up

**Chosen:** Map MagicaVoxel `(x, y, z)` → NAADF `(x, z, y)` (swap Y↔Z). Mirrors C# `ModelData.cs:386` + `:438` exactly.

**Rejected:** Preserve MagicaVoxel's right-handed Z-up convention as-is, document the difference from the C# import.

**Why:** Faithful-port rule (`01-context.md` §2 modulation) preserves the `.vox` → `VoxelType` → renderer pipeline *shape* — coordinate swap is part of that pipeline. The hard-coded test grid (`voxel/grid.rs::build_default_volume`) is authored in NAADF's Y-up convention; if `.vox` worlds came in with a different convention, mixing `.vox` content with editor-placed content (Track B) would silently mismatch.

**Flip-trigger:** if a `.vox` fixture authored with Y-up appears wrong (renders rotated 90° about the X axis), the swap is responsible — verify against the C# `ImportFromVox` behaviour on the same fixture before changing.

### Decision 6: Scene-graph flattening — identity-only walk first cut ✅ vs. full transform composition

**Chosen:** Walk the scene graph from `scenes[0]`, treat every `nTRN` as `t=(0,0,0)` + `r=identity`, concatenate model AABBs as if every `nSHP` references a model at the origin under identity. Multi-model `.vox` files with non-trivial transforms render incorrectly (positions/rotations not applied).

**Rejected:** Full 4×4-matrix transform composition matching `MagicaVoxel.cs::CollateVoxelData` (`:718-770`).

**Why:** ~30 LOC vs ~150-200 LOC. The fixtures we plan to ship are single-model (no transforms) or multi-model-at-origin (identity transforms). The general transform-composition path is a follow-up.

**Flip-trigger:** the first fixture that ships with non-trivial transforms exposes this. Easy to land later because the seam is internal to `voxel/vox_import.rs::flatten_scene`; no API change.

## Assumptions made

1. **`dot_vox` 5.2 is feature-complete enough.** Specifically: it parses every chunk type the in-scope test fixtures use (SIZE, XYZI, RGBA, MATL, optionally nTRN/nGRP/nSHP/LAYR), and its `index_map` correctly handles the IMAP-chunk palette reordering. The crate's CHANGELOG/README claim broad compat with MagicaVoxel ≥0.99. If a specific fixture parses but renders incorrectly, the first-cut workaround is to re-export it from MagicaVoxel with "Save As" (re-canonicalises the chunks). Not blocking design.
2. **The C# `.vox` import path does NOT use K-means.** Verified by reading `ModelData.cs:502-522` (the palette-conversion block of `ImportFromVox`) — the loop creates one `VoxelType` per `dataImport.Colors[c]`, no clustering. The K-means at `:528-560` is reached only via `ImportFromVL32` at `:587+`. Both the audit (§2.3) and brief (steps 2 + 5) say K-means is part of the `.vox` pipeline; the design overrides them on this point.
3. **The Z↔Y coordinate swap at `ModelData.cs:386` + `:438` is consistent.** The two swap points use the same convention; the design assumes there's no asymmetric swap (e.g., size swapped one way, voxel coords swapped another). Verified by reading both lines.
4. **NAADF's coordinate convention is Y-up.** Verified by `voxel/grid.rs::build_default_volume` (`:234-297`) — the ground slab is at `Y=0..2` (low Y = ground = vertical down), towers are tall in Y. Matches the Z↔Y swap from MagicaVoxel's Z-up convention.
5. **`AppArgs` losing `Copy` is acceptable.** `AppArgs::Copy` is bound at `lib.rs:189`. `Vec<u8>`-containing struct (`PathBuf`) can't be `Copy`. Audit of `AppArgs` `Copy` usages: every internal use is by-ref (`Res<AppArgs>`); the `Copy` is convenience for the `Default` impl + tests. Dropping `Copy` adds `.clone()` calls in two-three test spots. Implementer verifies by `cargo build` — if there's a load-bearing `Copy` user, fall back to `AppArgs { grid_preset: GridPreset, ... }` where `GridPreset` is `Clone` (not `Copy`) but `AppArgs` keeps `Copy` by storing `Arc<PathBuf>` for the `.vox` path. Not blocking design.
6. **The MagicaVoxel `MATL` chunk's `_emit` / `_flux` dict keys are string-encoded floats.** Confirmed by `MagicaVoxel.cs:364-371` (`float.Parse(...)`); same is true in `dot_vox::Material::properties: Dict<String, String>` per `dot_vox::parser` source. If a fixture's `_emit` is encoded differently, `parse::<f32>()` returns `Err` and the design defaults to 0.0 (`unwrap_or(0.0)` in `vox_palette_to_voxel_types`) — graceful degrade to diffuse. Recorded so the implementer doesn't second-guess the unwrap.
7. **`render/construction/mod.rs:827`'s reliance on `dense_voxel_types` extends transparently to `.vox`-authored worlds.** The condition checked is `!w.dense_voxel_types.is_empty()`; the `.vox` path goes through the same `setup_test_grid` code that populates `dense_voxel_types` from `volume.voxels.iter().map(|t| t.0).collect()`. Verified by reading `voxel/grid.rs:86-104`.
8. **Bevy 0.19 `AssetLoader` is the same shape as 0.18.** The `bevy_image::HdrTextureLoader` impl (cached at `/home/midori/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_image-0.19.0-rc.1/src/hdr_texture_loader.rs:33-83`) confirms the trait surface (`Asset`, `Settings`, `Error` associated types, async `load`, `extensions()`). Listed as an assumption only because the chosen design does NOT use `AssetLoader`; the future-extension claim ("AssetLoader can wrap `parse_vox_bytes` directly") rests on this surface being available.
9. **The wgpu `max_texture_dimension_3d` is checked at runtime, but our soft-check at parse time is 1024.** The actual limit is queried only after render-app init (`bevy::render::settings::WgpuSettings::limits`); the loader's `1024`-chunk soft-check is a conservative pre-flight. False positives (refusing files that wgpu would actually accept) are possible on devices with the full 2048 cap, but a 256³-voxel ceiling per `dot_vox::Voxel` u8 dims means a single-model `.vox` can never trigger this; only a hypothetical multi-model file with composed dimensions could. The check exists for correctness, not realism.
10. **No `.vox` file in scope uses the `IMAP` chunk in a way that breaks the `dot_vox` 1-based→0-based handling.** Audit row 1 flagged this as a potential flip-trigger (`MagicaVoxel.cs:415-424`). `dot_vox::DotVoxData::index_map` exposes the raw IMAP; the design currently doesn't apply it (palette index = `Voxel.i + 1`). If MagicaVoxel's editor was used with drag-reorder-by-control-click on the palette and that produced an IMAP, the rendered palette will be off. Mitigation: re-save the file in MagicaVoxel without reordering, OR apply `dot_vox::DotVoxData::index_map` in `vox_palette_to_voxel_types` (one extra lookup per voxel). Not blocking design.

## Risks & mitigations

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | `dot_vox` doesn't handle the `IMAP` chunk per MagicaVoxel-editor palette reordering | Low (most users don't reorder) | Wrong colors per voxel | Apply `data.index_map[voxel.i]` as the palette lookup index in `vox_palette_to_voxel_types`. Easy ~3-LOC change inside the loader. |
| 2 | A `.vox` size triggers the `max_texture_dimension_3d` wgpu limit on a Vulkan-1024 device | Very low (single model ≤ 256³) | Loader errors, falls back to default grid | Already covered: `VoxImportError::SizeExceedsTextureLimit` → fallback. |
| 3 | A multi-model `.vox` with non-identity transforms renders at the wrong position | Medium (every Magica scene with grouped objects) | Visually-wrong content | Document the limitation; flip-trigger is a real fixture exposing it (see Decision 6). Implementer can add transform composition in a follow-up without API change. |
| 4 | `AppArgs` losing `Copy` cascades into wider edits | Low (most uses are by-ref) | Implementer time | Audit `Copy` usages with `cargo build` first; fall back to `Arc<PathBuf>` (or `Box<PathBuf>`) if needed. Recorded under Assumption #5. |
| 5 | `dot_vox` adds a transitive dep that conflicts with Bevy's pinned versions | Low (the crate's deps are just `byteorder`, `nom`, `lazy_static`, `log`, optionally `ahash`) | `cargo build` regression | Verify with `cargo tree -p dot_vox` after adding; if `nom` or `lazy_static` versions conflict, the dep tree's already merged. (Cargo workspace's `Cargo.lock` is the authority.) |
| 6 | The Z↔Y swap is applied in the wrong direction (renders inverted) | Low (matches C# verbatim) | First fixture renders rotated | Visual gate (user). Easy fix — swap the swap. The unit test `zy_swap_matches_csharp` catches it pre-render. |
| 7 | Loading `.vox` blocks the `Startup` schedule for >100 ms on a large fixture | Low (files ≤10 MiB typical, `std::fs::read` is fast) | Boot-time hitch | Acceptable for the first cut. If a user complains, the AssetLoader extension is the answer. |
| 8 | `setup_test_grid`'s fallback-to-default on error masks `.vox` failures silently | Medium | User confusion ("I passed `--vox foo.vox` but I see the test grid!") | The fallback emits `error!` via Bevy logging; the user sees the error in stderr/console. Acceptable. |
| 9 | The `--vox` flag conflicts with a future arg-parser landing in `main.rs` | Low (current main.rs has no flag parsing) | Implementer rework | Recorded; if a future `clap`-based parser arrives, port `--vox` to it. |
| 10 | `dense_voxel_types` Vec for a 256³ world is 32 MiB — could spike RAM during edit batches | Medium | Memory pressure | Already in the audit (`world/data.rs:153-155` flags an unbounded-CPU-mirror growth issue across many edits); Track A doesn't add edits, only initial load, so the 32 MiB allocation is one-shot. Track B's `set_voxels_batch` design addresses the edit-time growth separately. |
| 11 | A `.vox` palette entry has both `_emit > 0` AND `_metal` material — design picks `Emissive` and drops `MetallicRough` | Low (mixing in MagicaVoxel is unusual) | Wrong material on edge-case voxels | Matches C# `ModelData.cs:512-518` exactly (emission wins). Faithful-port; not a defect. |
| 12 | The 8×8×8 small-cube fixture's binary contents drift if MagicaVoxel updates its writer | N/A (we author once + commit) | Test instability | Commit the binary; never regenerate. If `dot_vox` ever fails to parse it, the test is the canary. |

## Out of scope for this design

- **`obj2voxel` integration in any form** — deferred per `01-context.md` §5 forbidden move 7. Track A is `.vox` only.
- **Streaming / paged-VOX loading** — the design synchronously reads the entire file at `Startup`. Large-world streaming (the C# `ImportFromVox` `_0.vox`/`_1.vox`/... multi-file accumulator at `ModelData.cs:361-372`) is not implemented; the first cut loads a single `.vox` per session.
- **Palette deduplication across `.vox` and the test-grid hard-coded palette** — when a `.vox` loads, its palette **replaces** the hard-coded grid's palette wholesale. Mixing `.vox`-imported content with editor-placed content (a Track B concern) inherits whatever palette the loaded `.vox` produced. A future merge-and-renumber step is possible.
- **Per-frame hot-reload of `.vox`** — no Bevy `AssetLoader`, no file-watcher; `.vox` is loaded once at boot.
- **Bevy `AssetLoader` registration / `DotVoxAsset` type** — explicit extension point, not in first cut. Recorded under Decision 4 and Assumption 8.
- **Pre-bake binary (`bin/bake.rs --vox <path>`)** — explicit extension point, not in first cut. Recorded under Decision 4.
- **`.vl32` import** — out of Track A scope (Track A is `.vox` only per `01-context.md` §1). The K-means stage that the audit/brief incorrectly attribute to `.vox` is actually `.vl32`'s, and lands when `.vl32` does.
- **Voxlap `.vox`** — `VoxFile.cs:13-15` falls through to `Voxlap.Read` when the magic bytes don't match MagicaVoxel; out of scope (audit §2.4 row 3 confirms).
- **Full scene-graph transform composition** — first cut is identity-only (Decision 6). Multi-model multi-transform `.vox` is a follow-up.
- **CLI subcommand structure / `clap` etc.** — minimal `std::env::args` parsing only.
- **Per-voxel `set_voxel` ingestion path** — explicitly forbidden by audit §2.2 row 3 ("not appropriate for whole-world bulk loads"). The design uses `DenseVolume` exclusively.
- **Documenting the `--vox` flag in `README.md`** — implementer's call; not load-bearing for the design.
- **Visual gating, frame-cap regression tests, or any GPU-runtime correctness check beyond the existing e2e modes** — explicit per the brief's task #8 + the global memory `subagent-gpu-app-verification-loop`. Manual visual is the user's gate.
