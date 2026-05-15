//! MagicaVoxel `.vox` ingestion — Track A of the feature-completeness
//! orchestration (`docs/orchestrate/feature-completeness/02a-design-vox-loading.md`).
//!
//! Parses a `.vox` file into a [`DenseVolume`] + a [`VoxelType`] palette, then
//! hands the pair off to [`crate::aadf::construct::construct`] / the existing
//! `WorldData` build path that `voxel/grid.rs::setup_test_grid` already drives
//! for the hard-coded test grid. The GPU side does not learn about `.vox` —
//! once the loader produces a `DenseVolume`, the existing
//! `construct()` → `WorldData` → `prepare_world_gpu` chain takes over
//! unchanged.
//!
//! ## Pipeline shape (mirrors C# `MagicaVoxel.cs` + `ModelData.ImportFromVox`)
//!
//! 1. `dot_vox::load_bytes(&[u8])` → `DotVoxData { models, palette, materials,
//!    scenes, layers, .. }` — exactly the shape `MagicaVoxel.cs`'s chunk-tagged
//!    parser produces.
//! 2. [`flatten_scene`] folds the scene graph into one `DenseVolume`. The first
//!    cut walks the scene from `scenes[0]` and treats every `nTRN` as identity
//!    (no translation, no rotation) — multi-model `.vox` files with non-trivial
//!    transforms render at the origin only (per
//!    `02a-design-vox-loading.md` Decision 6).
//! 3. [`vox_palette_to_voxel_types`] promotes the 256-entry `RGBA` palette +
//!    `MATL` chunks into a `Vec<VoxelType>`. Mirrors C# `ModelData.cs:502-522`:
//!    one [`VoxelType`] per source palette entry, sRGB→linear via gamma 2.2,
//!    `emission = _emit * (1 + _flux)^2 * 5` slotted into
//!    [`VoxelType::color_layered`]`.x` when `_emit > 0`. **No K-means** —
//!    K-means in `ModelData.cs:528-560` is `ImportFromVL32`-only (audit/brief
//!    overrided; see `02a-design-vox-loading.md` Decision 2).
//! 4. The voxel-coordinate convention is swapped: MagicaVoxel `(x, y, z)` →
//!    NAADF `(x, z, y)`. Matches C# `ModelData.cs:386` + `:438`
//!    (`02a-design-vox-loading.md` Decision 5).
//!
//! ## What's out of scope (Track A first cut)
//!
//! - Full scene-graph transform composition (Decision 6 — first cut walks
//!   scenes under identity only).
//! - K-means palette reduction (Decision 2 — `.vox` doesn't use it).
//! - `obj2voxel` integration (deferred entirely per `01-context.md` §5).
//! - `.vl32` import (Track A is `.vox` only).
//! - Bevy `AssetLoader` registration / hot-reload (Decision 4 — synchronous
//!   `std::fs::read` at `Startup` only).
//! - Pre-bake to a port-native binary format (Decision 4).

use std::path::Path;

use bevy::math::Vec3;
use thiserror::Error;

use crate::aadf::construct::DenseVolume;
use crate::voxel::{MaterialBase, MaterialLayer, VoxelType, VoxelTypeId};

/// Conservative soft-cap on world axis dimensions, in chunks, the loader will
/// accept. A typical Vulkan minimum `max_texture_dimension_3d` is 1024, which
/// is the chunks-texture cap (`render/prepare.rs:206-280`). A `.vox` model is
/// always ≤256³ voxels (≤16³ chunks) per `dot_vox::Voxel`'s `u8` dims, so this
/// only ever trips on a pathological multi-model file with composed dimensions
/// past the ceiling (`02a-design-vox-loading.md` `## Size ceilings`).
pub const MAX_CHUNKS_PER_AXIS: u32 = 1024;

/// Conservative soft-cap on the `dense_voxel_types: Vec<u16>` mirror budget,
/// in bytes (512 MiB → ≈645³ voxels). The check protects against accidentally
/// allocating a many-gigabyte `Vec<u16>` if a file declares a huge size
/// (`02a-design-vox-loading.md` `## Size ceilings`).
pub const MAX_DENSE_BYTES: u64 = 512 * 1024 * 1024;

/// Parsed-and-flattened `.vox` data, ready to install into a NAADF world.
///
/// Produced by [`parse_vox_bytes`] / [`load_vox`]. Consumed by
/// [`build_world_from_vox`] (or any caller that drops the volume into
/// [`crate::aadf::construct::construct`] directly).
#[derive(Clone, Debug)]
pub struct ImportedVox {
    /// The flattened dense voxel volume, sized to the smallest cuboid that
    /// covers every visible voxel across the file's scene graph (round up to
    /// whole chunks). Mirrors C# `MagicaVoxel.Flatten` at
    /// `MagicaVoxel.cs:677-689`.
    pub volume: DenseVolume,
    /// The voxel-type palette derived from the `.vox` `RGBA` + `MATL` chunks.
    /// Index 0 is the reserved empty placeholder; indices 1..=N mirror the
    /// MagicaVoxel palette entries 0..N-1 (the `+1` shift keeps slot 0 empty
    /// per NAADF convention — `voxel/mod.rs:65-71`).
    pub palette: Vec<VoxelType>,
}

/// Errors emitted by [`parse_vox_bytes`] / [`load_vox`].
#[derive(Debug, Error)]
pub enum VoxImportError {
    /// `dot_vox` rejected the bytes as a malformed `.vox` file. The crate
    /// produces `&'static str` errors only; we wrap that.
    #[error("dot_vox parse failed: {0}")]
    Parse(&'static str),
    /// `std::fs::read` failed (file not found, permission denied, …).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The resulting `DenseVolume`'s per-axis chunk count exceeds
    /// [`MAX_CHUNKS_PER_AXIS`] — typically only on a hypothetical
    /// many-thousand-voxel composed scene (`02a-design-vox-loading.md` Risk #2).
    #[error("VOX size {dim:?} chunks per axis exceeds soft-cap ({limit} per axis); refusing to allocate the chunks 3D texture this large")]
    SizeExceedsTextureLimit { dim: [u32; 3], limit: u32 },
    /// The `dense_voxel_types: Vec<u16>` mirror would exceed
    /// [`MAX_DENSE_BYTES`]. 512 MiB at 2 B/voxel ≈ 645³ — comfortably above
    /// any test fixture.
    #[error("VOX size {dim:?} voxels would exceed the {bytes}-byte CPU mirror budget")]
    SizeExceedsBudget { dim: [u32; 3], bytes: u64 },
    /// `dot_vox` parsed a file with `models.is_empty()`. Mirrors C#
    /// `MagicaVoxel.cs:687` `else { return Models[0]; }` panic-on-empty.
    #[error("VOX contains no models")]
    Empty,
}

/// Parse `.vox` bytes and flatten the scene graph into a single
/// [`ImportedVox`].
///
/// Pure CPU, no Bevy resources, no filesystem — the unit-testable entry point.
/// Mirrors C# `MagicaVoxel.Flatten` at `MagicaVoxel.cs:677-689`.
pub fn parse_vox_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxImportError> {
    let data = dot_vox::load_bytes(bytes).map_err(VoxImportError::Parse)?;
    parse_dot_vox_data(&data)
}

/// Convenience: load a `.vox` file from disk via `std::fs::read` + parse.
///
/// Used by `voxel/grid.rs::setup_test_grid` when `args.grid_preset` is
/// [`crate::GridPreset::Vox`]. On error the caller logs + falls back to the
/// hard-coded test grid (`02a-design-vox-loading.md` `## How loading
/// integrates with setup_test_grid`).
pub fn load_vox(path: impl AsRef<Path>) -> Result<ImportedVox, VoxImportError> {
    let bytes = std::fs::read(path.as_ref())?;
    parse_vox_bytes(&bytes)
}

/// Internal entry point: convert a parsed `DotVoxData` into [`ImportedVox`].
///
/// Pulled out so unit tests can drive it with a hand-built `DotVoxData` (e.g.
/// the emissive-material / Z↔Y swap fixtures) without going through the
/// binary parser.
pub fn parse_dot_vox_data(data: &dot_vox::DotVoxData) -> Result<ImportedVox, VoxImportError> {
    if data.models.is_empty() {
        return Err(VoxImportError::Empty);
    }
    let volume = flatten_scene(data)?;
    let palette = vox_palette_to_voxel_types(&data.palette, &data.materials);
    Ok(ImportedVox { volume, palette })
}

/// Apply an [`ImportedVox`] to fresh [`crate::world::data::WorldData`] +
/// [`crate::world::data::VoxelTypes`] resources, exactly the way
/// `setup_test_grid` builds them from `build_default_volume` + `build_palette`
/// today (`voxel/grid.rs:66-110`). Returns the two resources the caller
/// inserts via `Commands::insert_resource`.
///
/// Kept separate from [`load_vox`] so a future caller (Bevy `AssetLoader`
/// extension, pre-bake binary) can install an `ImportedVox` it produced via
/// some other route.
pub fn build_world_from_vox(
    imported: ImportedVox,
) -> (crate::world::data::WorldData, crate::world::data::VoxelTypes) {
    use crate::aadf::construct::construct;
    use crate::world::data::{IAabb3, VoxelTypes, WorldData};
    use bevy::math::{IVec3, UVec3};

    let volume = imported.volume;
    let world = construct(&volume);
    let size = volume.size_in_voxels();

    // Phase-C followup #1 — preserve the dense voxel-type stream so the
    // runtime GPU construction dispatch can rebuild `segment_voxel_buffer`
    // without going through a CPU `construct()` re-run.
    let dense_voxel_types: Vec<u16> = volume.voxels.iter().map(|t| t.0).collect();

    let world_data = WorldData {
        chunks_cpu: world.chunks,
        blocks_cpu: world.blocks,
        voxels_cpu: world.voxels,
        size_in_chunks: UVec3::from_array(world.size_in_chunks),
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(size[0] as i32 - 1, size[1] as i32 - 1, size[2] as i32 - 1),
        },
        dirty: true,
        pending_edits: Default::default(),
        dense_voxel_types,
    };

    let voxel_types = VoxelTypes {
        types: imported.palette,
        dirty: true,
    };

    (world_data, voxel_types)
}

/// Walk the scene graph and produce a [`DenseVolume`] covering every visible
/// voxel, with the C# coordinate convention applied (Z↔Y swap, see
/// `02a-design-vox-loading.md` Decision 5).
///
/// First-cut limitation (Decision 6): every `nTRN` is treated as
/// translation=0 + rotation=identity. Multi-model `.vox` files at non-trivial
/// transforms render at the origin only.
fn flatten_scene(data: &dot_vox::DotVoxData) -> Result<DenseVolume, VoxImportError> {
    if data.models.is_empty() {
        return Err(VoxImportError::Empty);
    }

    // Collect every model id referenced by a `nSHP` in the scene graph. For
    // older `.vox` versions without scene graphs we fall through to using
    // models[0] (mirrors C# `MagicaVoxel.cs:687` `else { return Models[0]; }`).
    let referenced_model_ids: Vec<u32> = if data.scenes.is_empty() {
        vec![0]
    } else {
        let mut ids = Vec::new();
        collect_referenced_model_ids(data, 0, &mut ids);
        if ids.is_empty() {
            // Scene graph present but no `nSHP` found — fall back to models[0]
            // so we still produce something renderable.
            vec![0]
        } else {
            ids
        }
    };

    // Compute the per-axis size in MagicaVoxel coordinates. Identity walk:
    // every referenced model sits at the origin, so the bounding cuboid is
    // the per-axis max over all referenced model sizes.
    let mut max_size = [0u32; 3];
    for &id in &referenced_model_ids {
        let Some(model) = data.models.get(id as usize) else {
            continue;
        };
        max_size[0] = max_size[0].max(model.size.x);
        max_size[1] = max_size[1].max(model.size.y);
        max_size[2] = max_size[2].max(model.size.z);
    }
    if max_size == [0, 0, 0] {
        return Err(VoxImportError::Empty);
    }

    // Apply the Z↔Y swap to convert MagicaVoxel (right-handed Z-up) to NAADF
    // (Y-up). Mirrors C# `ModelData.cs:386`:
    //   `Point3 modelSize = new Point3(totalBounds.Size.X,
    //                                  totalBounds.Size.Z,
    //                                  totalBounds.Size.Y);`
    let naadf_size = [max_size[0], max_size[2], max_size[1]];

    // Round up to whole chunks (`CHUNK_DIM_VOXELS = 16`).
    let size_in_chunks = [
        round_up_to_chunks(naadf_size[0]),
        round_up_to_chunks(naadf_size[1]),
        round_up_to_chunks(naadf_size[2]),
    ];

    // Soft-cap pre-flight: refuse to allocate a chunks 3D texture larger than
    // a typical Vulkan minimum `max_texture_dimension_3d` (1024).
    if size_in_chunks[0] > MAX_CHUNKS_PER_AXIS
        || size_in_chunks[1] > MAX_CHUNKS_PER_AXIS
        || size_in_chunks[2] > MAX_CHUNKS_PER_AXIS
    {
        return Err(VoxImportError::SizeExceedsTextureLimit {
            dim: size_in_chunks,
            limit: MAX_CHUNKS_PER_AXIS,
        });
    }

    // Soft-cap pre-flight: refuse to allocate a many-gigabyte `Vec<u16>`.
    let voxels_per_axis = [
        size_in_chunks[0] * 16,
        size_in_chunks[1] * 16,
        size_in_chunks[2] * 16,
    ];
    let total_voxels =
        voxels_per_axis[0] as u64 * voxels_per_axis[1] as u64 * voxels_per_axis[2] as u64;
    let total_bytes = total_voxels.saturating_mul(2);
    if total_bytes > MAX_DENSE_BYTES {
        return Err(VoxImportError::SizeExceedsBudget {
            dim: voxels_per_axis,
            bytes: MAX_DENSE_BYTES,
        });
    }

    // Allocate the empty volume + collate every referenced model into it under
    // identity transform (first-cut: no translation, no rotation).
    let mut volume = DenseVolume::empty(size_in_chunks);
    for &id in &referenced_model_ids {
        let Some(model) = data.models.get(id as usize) else {
            continue;
        };
        for voxel in &model.voxels {
            // Z↔Y swap in voxel coords too (`ModelData.cs:438`):
            //   `dataImport[new Voxels.XYZ(voxelPos.X, voxelPos.Z, voxelPos.Y)]`
            let nx = voxel.x as u32;
            let ny = voxel.z as u32;
            let nz = voxel.y as u32;
            // Guard against the (impossible-in-practice but possible-in-pathological)
            // case where a Voxel's coords exceed the model's declared `size`.
            if nx >= voxels_per_axis[0] || ny >= voxels_per_axis[1] || nz >= voxels_per_axis[2]
            {
                continue;
            }
            // `Voxel.i` is already 0-based in `dot_vox` (`model.rs:74` does the
            // 1→0 conversion). Map to a 1-based `VoxelTypeId` so slot 0 stays
            // the empty placeholder per NAADF convention (`voxel/mod.rs:65-71`).
            let ty = VoxelTypeId(voxel.i as u16 + 1);
            volume.set([nx, ny, nz], ty);
        }
    }

    Ok(volume)
}

/// Round `voxels` up to a whole-chunk count (`CHUNK_DIM_VOXELS = 16`).
fn round_up_to_chunks(voxels: u32) -> u32 {
    voxels.div_ceil(16).max(1)
}

/// Walk the scene graph from `node_id` and collect every `nSHP`'s referenced
/// model ids. Cycle-safe via a visited set.
fn collect_referenced_model_ids(
    data: &dot_vox::DotVoxData,
    node_id: u32,
    out: &mut Vec<u32>,
) {
    let mut visited = vec![false; data.scenes.len()];
    let mut stack = vec![node_id];
    while let Some(id) = stack.pop() {
        let idx = id as usize;
        if idx >= visited.len() || visited[idx] {
            continue;
        }
        visited[idx] = true;
        match &data.scenes[idx] {
            dot_vox::SceneNode::Transform { child, .. } => stack.push(*child),
            dot_vox::SceneNode::Group { children, .. } => {
                for &c in children {
                    stack.push(c);
                }
            }
            dot_vox::SceneNode::Shape { models, .. } => {
                for sm in models {
                    out.push(sm.model_id);
                }
            }
        }
    }
}

/// Promote the 256-entry MagicaVoxel `RGBA` palette + `MATL` chunks into a
/// `Vec<VoxelType>` of length `palette.len() + 1`. Index 0 is the reserved
/// empty placeholder; indices 1..=N mirror the source palette entries.
///
/// Mirrors C# `ModelData.cs:502-522`:
/// ```text
/// types = new VoxelType[dataImport.Colors.Length];
/// for (int c = 0; c < dataImport.Colors.Length; c++) {
///     colSRGB = (R, G, B) / 255;
///     colorBase = pow(colSRGB, 2.2f);
///     emission = mat.emit * pow(1 + mat.flux, 2) * 5;
///     materialBase = (emission > 0) ? Emissive : Diffuse;
///     colorLayered.X = emission;
/// }
/// ```
///
/// No K-means — that's `.vl32`'s pipeline, not `.vox`'s
/// (`02a-design-vox-loading.md` Decision 2).
fn vox_palette_to_voxel_types(
    palette: &[dot_vox::Color],
    materials: &[dot_vox::Material],
) -> Vec<VoxelType> {
    let mut out = Vec::with_capacity(palette.len() + 1);
    // Slot 0 — reserved empty placeholder (NAADF convention,
    // `voxel/mod.rs:65-71`).
    out.push(VoxelType::default());

    for (i, color) in palette.iter().enumerate() {
        // MagicaVoxel palette entries are sRGB; NAADF stores linear RGB.
        // Gamma 2.2 matches C# `pow(colSRGB, 2.2f)` (`ModelData.cs:507`).
        let srgb = Vec3::new(color.r as f32, color.g as f32, color.b as f32) / 255.0;
        let linear = Vec3::new(srgb.x.powf(2.2), srgb.y.powf(2.2), srgb.z.powf(2.2));

        // `dot_vox` ships one `Material` per palette index with
        // `materials[k].id == k` (0-based, matching the in-memory `Voxel.i`
        // index — see `dot_vox/src/lib.rs:96-115` placeholder test + the
        // round-trip serializer in `dot_vox_data.rs:167-175`). Look up by id
        // rather than by index — slightly more robust against re-ordered
        // input (the spec doesn't promise order).
        let (emit, flux) = materials
            .iter()
            .find(|m| m.id as usize == i)
            .map(|m| {
                (
                    m.emission().unwrap_or(0.0),
                    m.radiant_flux().unwrap_or(0.0),
                )
            })
            .unwrap_or((0.0, 0.0));

        // C# formula (`ModelData.cs:509`): `emission = emit * (1+flux)^2 * 5`.
        let emission = emit * (1.0 + flux).powi(2) * 5.0;

        let (material_base, color_layered) = if emission > 0.0 {
            (MaterialBase::Emissive, Vec3::new(emission, 0.0, 0.0))
        } else {
            (MaterialBase::Diffuse, Vec3::ZERO)
        };

        out.push(VoxelType {
            color_base: linear,
            material_base,
            material_layer: MaterialLayer::None,
            // C# does not set roughness on this branch (`ModelData.cs:502-522`
            // — only the K-means / vl32 path sets roughness).
            roughness: 1.0,
            color_layered,
        });
    }

    out
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::IVec3;
    use std::collections::HashMap as StdHashMap;
    use std::io::Cursor;

    /// Build a tiny single-voxel `DotVoxData` in MagicaVoxel coords (1×1×1, one
    /// voxel at (0,0,0), index 0).
    fn build_single_voxel() -> dot_vox::DotVoxData {
        dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 1, y: 1, z: 1 },
                voxels: vec![dot_vox::Voxel {
                    x: 0,
                    y: 0,
                    z: 0,
                    // Use a non-zero palette slot so we can tell it apart from
                    // the empty placeholder after the `+1` shift.
                    i: 0,
                }],
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        }
    }

    /// Build a small 8×8×8 cube `DotVoxData` — a 7×7×7 solid cube of one
    /// palette index, plus one emissive voxel at (3,3,3) of another index.
    fn build_small_cube() -> dot_vox::DotVoxData {
        let mut voxels = Vec::with_capacity(7 * 7 * 7 + 1);
        for z in 0..7u8 {
            for y in 0..7u8 {
                for x in 0..7u8 {
                    voxels.push(dot_vox::Voxel { x, y, z, i: 10 });
                }
            }
        }
        // Replace the centre voxel with the emissive index (overwrites the
        // diffuse one at (3,3,3)).
        voxels.retain(|v| !(v.x == 3 && v.y == 3 && v.z == 3));
        voxels.push(dot_vox::Voxel {
            x: 3,
            y: 3,
            z: 3,
            i: 20,
        });

        let mut materials = default_materials();
        // Make palette slot 20 emissive via the MagicaVoxel `_emit` attribute.
        // `dot_vox::Material.id` is the 0-based palette index (see
        // `dot_vox/src/lib.rs:96-115`), so `id == 20` matches palette slot 20
        // — which voxels write as `Voxel.i == 20` and which `vox_palette_to_voxel_types`
        // maps to `VoxelTypeId(21)` after the +1 shift for the empty
        // placeholder.
        for m in &mut materials {
            if m.id == 20 {
                m.properties.insert("_type".into(), "_emit".into());
                m.properties.insert("_emit".into(), "1.0".into());
                m.properties.insert("_flux".into(), "0.0".into());
            }
        }

        dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 8, y: 8, z: 8 },
                voxels,
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials,
            scenes: Vec::new(),
            layers: Vec::new(),
        }
    }

    /// MagicaVoxel's defaults are diffuse with no `_emit` field. Build a
    /// 256-entry one-per-palette-index materials list with the
    /// `dot_vox`-default properties.
    fn default_materials() -> Vec<dot_vox::Material> {
        (0..256)
            .map(|i| dot_vox::Material {
                id: i,
                properties: {
                    let mut d: dot_vox::Dict = StdHashMap::new().into_iter().collect();
                    d.insert("_type".to_owned(), "_diffuse".to_owned());
                    d
                },
            })
            .collect()
    }

    /// Round-trip a `DotVoxData` through `write_vox` → `parse_vox_bytes` so the
    /// binary parser path is exercised end-to-end.
    fn round_trip(data: &dot_vox::DotVoxData) -> ImportedVox {
        let mut buf = Vec::new();
        data.write_vox(&mut Cursor::new(&mut buf))
            .expect("write_vox failed");
        parse_vox_bytes(&buf).expect("parse_vox_bytes failed")
    }

    // -- Test 1 --------------------------------------------------------------

    #[test]
    fn parses_single_voxel_fixture() {
        let data = build_single_voxel();
        let imp = round_trip(&data);

        // Smallest cuboid covering one voxel → rounded up to 1×1×1 chunks.
        assert_eq!(imp.volume.size_in_chunks, [1, 1, 1]);

        // Palette has the placeholder at slot 0 + 256 default-palette entries.
        assert_eq!(imp.palette.len(), 257);
        assert_eq!(imp.palette[0], VoxelType::default());

        // The single voxel is at world origin (Z↔Y swap doesn't move (0,0,0)).
        // `Voxel.i == 0` → `VoxelTypeId(1)` after the `+1` shift.
        assert_eq!(imp.volume.voxel_at([0, 0, 0]), VoxelTypeId(1));

        // Everywhere else inside the chunk is empty.
        assert_eq!(imp.volume.voxel_at([1, 0, 0]), VoxelTypeId::EMPTY);
        assert_eq!(imp.volume.voxel_at([0, 1, 0]), VoxelTypeId::EMPTY);
        assert_eq!(imp.volume.voxel_at([0, 0, 1]), VoxelTypeId::EMPTY);
        assert_eq!(imp.volume.voxel_at([8, 8, 8]), VoxelTypeId::EMPTY);
    }

    // -- Test 2 --------------------------------------------------------------

    #[test]
    fn parses_small_cube_fixture() {
        let data = build_small_cube();
        let imp = round_trip(&data);

        // 8 voxels per axis → rounded up to 1×1×1 chunks (16 voxels/chunk axis).
        assert_eq!(imp.volume.size_in_chunks, [1, 1, 1]);

        // Count the non-empty voxels: 7³ - 1 diffuse (one centre slot is taken
        // by the emissive replacement) + 1 emissive = 7³ = 343 total.
        let total_nonempty: u32 = imp
            .volume
            .voxels
            .iter()
            .filter(|t| **t != VoxelTypeId::EMPTY)
            .count() as u32;
        assert_eq!(total_nonempty, 343);

        // The centre voxel is the emissive one (palette index 20 → VoxelTypeId(21)).
        assert_eq!(imp.volume.voxel_at([3, 3, 3]), VoxelTypeId(21));
        // A non-centre voxel inside the cube is the diffuse one
        // (palette index 10 → VoxelTypeId(11)).
        assert_eq!(imp.volume.voxel_at([0, 0, 0]), VoxelTypeId(11));
        assert_eq!(imp.volume.voxel_at([6, 6, 6]), VoxelTypeId(11));

        // The palette entry at slot 21 (the emissive material) must have
        // MaterialBase::Emissive set (C# `_emit > 0` → Emissive branch).
        assert_eq!(
            imp.palette[21].material_base,
            MaterialBase::Emissive,
            "palette slot 21 must be Emissive after _emit > 0 mapping"
        );
        assert!(
            imp.palette[21].color_layered.x > 0.0,
            "Emissive intensity must be nonzero in color_layered.x"
        );
        // The diffuse palette entry stays Diffuse.
        assert_eq!(imp.palette[11].material_base, MaterialBase::Diffuse);
    }

    // -- Test 3 --------------------------------------------------------------

    #[test]
    fn palette_index_zero_is_empty_placeholder() {
        let data = build_single_voxel();
        let imp = round_trip(&data);
        assert_eq!(imp.palette[0], VoxelType::default());
    }

    // -- Test 4 --------------------------------------------------------------

    #[test]
    fn palette_emissive_from_matl() {
        // Build a `DotVoxData` (no fixture file) with one Material whose
        // `_emit` is 1.0 at palette slot 5 (`dot_vox::Material.id == 5` —
        // 0-based, matches the in-memory palette index per
        // `dot_vox/src/lib.rs:96-115`).
        let mut materials = default_materials();
        for m in &mut materials {
            if m.id == 5 {
                m.properties.insert("_type".into(), "_emit".into());
                m.properties.insert("_emit".into(), "1.0".into());
                m.properties.insert("_flux".into(), "0.0".into());
            }
        }
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 1, y: 1, z: 1 },
                voxels: vec![dot_vox::Voxel {
                    x: 0,
                    y: 0,
                    z: 0,
                    i: 5,
                }],
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials,
            scenes: Vec::new(),
            layers: Vec::new(),
        };
        let imp = parse_dot_vox_data(&data).unwrap();
        // Palette index 5 → VoxelTypeId(6) because we shift by +1 for the
        // empty placeholder at slot 0.
        assert_eq!(imp.palette[6].material_base, MaterialBase::Emissive);
        assert!(
            imp.palette[6].color_layered.x > 0.0,
            "Emissive intensity must be > 0 in color_layered.x"
        );
        // Sanity: emission = _emit * (1 + _flux)^2 * 5 = 1 * 1 * 5 = 5.0.
        assert!((imp.palette[6].color_layered.x - 5.0).abs() < 1e-4);
    }

    // -- Test 5 --------------------------------------------------------------

    #[test]
    fn zy_swap_matches_csharp() {
        // One voxel at (x=1, y=2, z=3) in MagicaVoxel coords → after Z↔Y swap
        // we should find it at NAADF coords (1, 3, 2). The C# import does the
        // same at `ModelData.cs:386` + `:438`.
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                // Size large enough to hold (1,2,3) — but make the bounds tight
                // so we can also verify the size swap.
                size: dot_vox::Size { x: 2, y: 3, z: 4 },
                voxels: vec![dot_vox::Voxel {
                    x: 1,
                    y: 2,
                    z: 3,
                    i: 0,
                }],
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        };
        let imp = parse_dot_vox_data(&data).unwrap();
        // The MagicaVoxel (1, 2, 3) voxel must land at NAADF (1, 3, 2).
        assert_eq!(imp.volume.voxel_at([1, 3, 2]), VoxelTypeId(1));
        // The naive same-coord lookup must be empty.
        assert_eq!(imp.volume.voxel_at([1, 2, 3]), VoxelTypeId::EMPTY);
    }

    // -- Test 6 --------------------------------------------------------------

    #[test]
    fn size_exceeds_texture_limit_errors() {
        // A model with size = 16_400 × 1 × 1 → after div_ceil(16) = 1025
        // chunks per x axis → exceeds MAX_CHUNKS_PER_AXIS = 1024.
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size {
                    x: 16_400,
                    y: 1,
                    z: 1,
                },
                // No voxels — size is independent of voxel count.
                voxels: Vec::new(),
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        };
        let result = parse_dot_vox_data(&data);
        match result {
            Err(VoxImportError::SizeExceedsTextureLimit { dim, limit }) => {
                assert_eq!(limit, MAX_CHUNKS_PER_AXIS);
                assert!(
                    dim[0] > MAX_CHUNKS_PER_AXIS,
                    "expected x dim > {} chunks, got {:?}",
                    MAX_CHUNKS_PER_AXIS,
                    dim
                );
            }
            other => panic!("expected SizeExceedsTextureLimit, got {:?}", other),
        }
    }

    // -- Test 7 --------------------------------------------------------------

    #[test]
    fn empty_models_errors() {
        let data = dot_vox::DotVoxData {
            version: 150,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: Vec::new(),
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: Vec::new(),
            layers: Vec::new(),
        };
        let result = parse_dot_vox_data(&data);
        assert!(
            matches!(result, Err(VoxImportError::Empty)),
            "expected VoxImportError::Empty, got {:?}",
            result
        );
    }

    // -- Test 8 --------------------------------------------------------------

    #[test]
    fn construct_runs_on_imported_volume() {
        // End-to-end: imported volume must feed the existing CPU `construct()`
        // oracle without spinning up Bevy or a GPU.
        let data = build_small_cube();
        let imp = round_trip(&data);

        let world = crate::aadf::construct::construct(&imp.volume);
        // 1×1×1 chunks → exactly 1 chunk in the output.
        assert_eq!(world.chunks.len(), 1);
        // The chunk has geometry → it's mixed → blocks/voxels must be non-empty.
        assert!(
            !world.blocks.is_empty(),
            "construct() must emit a non-empty blocks buffer for a mixed chunk"
        );
        assert!(
            !world.voxels.is_empty(),
            "construct() must emit a non-empty voxels buffer for a mixed chunk"
        );
    }

    // -- Bonus tests --------------------------------------------------------

    #[test]
    fn build_world_from_vox_inserts_dense_voxel_types() {
        let data = build_small_cube();
        let imp = round_trip(&data);
        let (world, types) = build_world_from_vox(imp);
        assert!(!world.dense_voxel_types.is_empty());
        // 1×1×1 chunks = 16³ = 4096 voxels.
        assert_eq!(world.dense_voxel_types.len(), 16 * 16 * 16);
        assert!(world.dirty);
        assert!(types.dirty);
        // BBox covers the 16³ volume.
        assert_eq!(world.bounding_box.min, IVec3::ZERO);
        assert_eq!(world.bounding_box.max, IVec3::new(15, 15, 15));
    }

    #[test]
    fn load_vox_propagates_io_error() {
        let result = load_vox("/this/path/does/not/exist.vox");
        assert!(
            matches!(result, Err(VoxImportError::Io(_))),
            "expected Io error, got {:?}",
            result
        );
    }
}
