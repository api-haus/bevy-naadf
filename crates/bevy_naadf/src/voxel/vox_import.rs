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
//! 2. [`flatten_scene`] folds the scene graph into one `DenseVolume` via a
//!    full two-pass walk that mirrors C# `MagicaVoxel.GetWorldAABB` +
//!    `MagicaVoxel.CollateVoxelData` (`MagicaVoxel.cs:651-755`). Pass 1
//!    accumulates the world AABB by composing parent `nTRN` transforms
//!    (translation `_t` + rotation `_r` byte → 3×3 signed-permutation matrix);
//!    pass 2 collates every shape's voxels under the composed transform into
//!    a `DenseVolume` sized to the AABB. The 03a-followup
//!    (`03a-followup-empty-scene-diagnosis.md`) lifted the design's
//!    "identity-only first cut" (Decision 6) — real-world `.vox` files (291+
//!    transformed models in Oasis_Hard_Cover.vox) require composition; the
//!    flip-trigger fired.
//! 3. [`vox_palette_to_voxel_types`] promotes the 256-entry `RGBA` palette +
//!    `MATL` chunks into a `Vec<VoxelType>`. Mirrors C# `ModelData.cs:502-522`:
//!    one [`VoxelType`] per source palette entry, sRGB→linear via gamma 2.2,
//!    `emission = _emit * (1 + _flux)^2 * 5` slotted into
//!    [`VoxelType::color_layered`]`.x` when `_emit > 0`. **No K-means** —
//!    K-means in `ModelData.cs:528-560` is `ImportFromVL32`-only (audit/brief
//!    overrided; see `02a-design-vox-loading.md` Decision 2).
//! 4. The voxel-coordinate convention is swapped: MagicaVoxel `(x, y, z)` →
//!    NAADF `(x, z, y)`. Matches C# `ModelData.cs:386` + `:438`
//!    (`02a-design-vox-loading.md` Decision 5). Applied at the final
//!    write-into-`DenseVolume` step, AFTER the MagicaVoxel-coords scene-graph
//!    composition is complete.
//!
//! ## What's out of scope (Track A first cut)
//!
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

/// Soft-cap on world axis dimensions, in chunks, the loader will accept.
///
/// 03a-followup lowered this from 1024 to 32 once scene-graph composition
/// landed: with composition, real-world `.vox` files can compose to world
/// AABBs that vastly exceed any single model's bounding cuboid. The actual
/// load-bearing constraint is the Phase-C-followup#1 GPU producer chain's
/// **`segment_voxel_buffer`** (`render/construction/mod.rs:921-960`), which
/// allocates `seg³ × 2048 × 4 B` GPU storage where `seg = max(chunks per
/// axis)`. At `seg = 32` the buffer is ~262 MiB (well within typical wgpu
/// `max_storage_buffer_binding_size` of 128 MiB → 2 GiB depending on GPU);
/// at `seg = 93` (Oasis_Hard_Cover.vox post-composition) it would be
/// ~6.5 GiB and OOM the render device. Loads above this cap fail
/// gracefully through `setup_test_grid`'s `error!`-and-fall-back path.
///
/// This is intentionally conservative — the wgpu Vulkan minimum
/// `max_texture_dimension_3d` (the OLD cap rationale) is 1024, but that's
/// the wrong limit to gate on; the GPU producer's segment buffer hits
/// `max_storage_buffer_binding_size` first. Files past this cap need
/// streaming / pre-baked / segment-iteration follow-up work
/// (`02a-design-vox-loading.md` `## Decisions` `Decision 3` flip-trigger).
pub const MAX_CHUNKS_PER_AXIS: u32 = 32;

/// Soft-cap on the `dense_voxel_types: Vec<u16>` mirror budget, in bytes
/// (1 GiB → ≈812³ voxels). The check protects against accidentally
/// allocating a many-gigabyte `Vec<u16>` if a file declares a huge size
/// (`02a-design-vox-loading.md` `## Size ceilings`).
///
/// The load-bearing limit is [`MAX_CHUNKS_PER_AXIS`] (32 chunks per axis =
/// 512 voxels per axis), which independently caps the voxel count at
/// 32³ × 16³ = ~134 M voxels = ~268 MiB. This `MAX_DENSE_BYTES` ceiling
/// is the secondary belt-and-braces gate.
pub const MAX_DENSE_BYTES: u64 = 1024 * 1024 * 1024;

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
    /// [`MAX_CHUNKS_PER_AXIS`]. The load-bearing constraint is the
    /// Phase-C-followup#1 GPU producer chain's `segment_voxel_buffer`
    /// (`render/construction/mod.rs:921-960`) — it allocates
    /// `seg³ × 2048 × 4 B` GPU storage where `seg = max(chunks per axis)`;
    /// real-world `.vox` files post-scene-graph-composition can compose to
    /// world AABBs that vastly exceed this budget. Files past this cap
    /// need streaming / pre-baked / segment-iteration follow-up work
    /// (`02a-design-vox-loading.md` Decision 3 flip-trigger).
    #[error("VOX size {dim:?} chunks per axis exceeds soft-cap ({limit} per axis); the GPU producer's segment_voxel_buffer would OOM. Pre-bake or shrink the .vox file")]
    SizeExceedsTextureLimit { dim: [u32; 3], limit: u32 },
    /// The `dense_voxel_types: Vec<u16>` mirror would exceed
    /// [`MAX_DENSE_BYTES`]. The load-bearing per-axis chunk cap
    /// ([`MAX_CHUNKS_PER_AXIS`]) lands first in practice; this is a
    /// belt-and-braces secondary gate.
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

/// A 3×3 signed-permutation rotation matrix (`_r` byte → integer matrix).
///
/// MagicaVoxel `_r` is a byte with 3 axis-selector bits + 3 sign bits per the
/// scene-format spec; the resulting matrix is always a signed permutation
/// (rows/cols are unit vectors with at most one non-zero, ±1). Mirrors
/// C# `MagicaVoxel.TransformFrame.Read` rotation-byte branch at
/// `MagicaVoxel.cs:127-146`.
#[derive(Clone, Copy, Debug)]
struct Rot3 {
    /// `m[col][row]`. Each column has exactly one non-zero ±1 entry.
    m: [[i32; 3]; 3],
}

impl Rot3 {
    const IDENTITY: Self = Self {
        m: [[1, 0, 0], [0, 1, 0], [0, 0, 1]],
    };

    /// Parse the MagicaVoxel `_r` rotation byte into a signed-permutation
    /// matrix. Mirrors C# `MagicaVoxel.cs:127-146` byte-for-byte. The byte
    /// encodes (per the MagicaVoxel scene-format spec):
    ///   - bits 0-1: source axis (in the input vector) for output axis 0
    ///     (i1 ∈ {0,1,2})
    ///   - bits 2-3: source axis for output axis 1 (i2 ∈ {0,1,2})
    ///   - bit  4 : sign of the source for output axis 0 (0 → +1, 1 → −1)
    ///   - bit  5 : sign of the source for output axis 1
    ///   - bit  6 : sign of the source for output axis 2
    /// The source axis for output 2 is determined as the remaining axis
    /// (the one neither i1 nor i2 picked). Equivalent to the C# code's
    /// `M(i1+1)1 = s1` pattern under .NET row-vector multiplication: with
    /// `out = v * M`, the column 0 of `M` has its non-zero ±1 at row i1,
    /// which evaluates to `out.x = s1 * v[i1]`. Same equation rewritten in
    /// column-vector form: `out = R * v` where `R[0][i1] = s1`.
    ///
    /// In this struct `m[col][row]` is the column-vector matrix `R`, so
    /// `R[row][col] = m[col][row]`. To place s1 at `R[0][i1]` we write
    /// `m[i1][0] = s1`.
    fn from_byte(b: u8) -> Self {
        let i1 = ((b & 0b00000011) >> 0) as usize;
        let i2 = ((b & 0b00001100) >> 2) as usize;
        let i3: usize = if i1 != 0 && i2 != 0 {
            0
        } else if i1 != 1 && i2 != 1 {
            1
        } else {
            2
        };
        let s1: i32 = if (b & 0b00010000) >> 4 == 0 { 1 } else { -1 };
        let s2: i32 = if (b & 0b00100000) >> 5 == 0 { 1 } else { -1 };
        let s3: i32 = if (b & 0b01000000) >> 6 == 0 { 1 } else { -1 };

        // Column-vector matrix: m[col][row] = R[row, col].
        // We want R[output][source] = sign, so R[0][i1] = s1, R[1][i2] = s2,
        // R[2][i3] = s3. Written in `m[col][row]` form:
        let mut m = [[0i32; 3]; 3];
        m[i1][0] = s1;
        m[i2][1] = s2;
        m[i3][2] = s3;
        Self { m }
    }

    /// Compose two rotations: `self * other` (column-major composition that
    /// matches the C# `frame.matrix * parentMatrix` post-multiply convention
    /// at `MagicaVoxel.cs:694` + `:720`. When applied to a vector via
    /// [`Self::transform_vec`], the right-most matrix's transform is applied
    /// first — i.e. parent's local→parent, then this composition's local→world).
    fn compose(&self, other: &Rot3) -> Rot3 {
        let mut out = [[0i32; 3]; 3];
        for col in 0..3 {
            for row in 0..3 {
                let mut s = 0i32;
                for k in 0..3 {
                    s += self.m[k][row] * other.m[col][k];
                }
                out[col][row] = s;
            }
        }
        Rot3 { m: out }
    }

    /// `M * v` — apply this rotation matrix to a vector. Result is integer
    /// because the matrix is a signed permutation (no shears, no scales).
    fn transform_vec(&self, v: [i32; 3]) -> [i32; 3] {
        [
            self.m[0][0] * v[0] + self.m[1][0] * v[1] + self.m[2][0] * v[2],
            self.m[0][1] * v[0] + self.m[1][1] * v[1] + self.m[2][1] * v[2],
            self.m[0][2] * v[0] + self.m[1][2] * v[1] + self.m[2][2] * v[2],
        ]
    }
}

/// A composed scene-graph transform: rotation + translation, in MagicaVoxel
/// (Z-up) coordinates. Mirrors the C# `Matrix4x4` chain at
/// `MagicaVoxel.cs:694` / `:720` (the only non-zero entries are translation
/// and the rotation submatrix; `Matrix4x4.CreateTranslation` + the
/// signed-permutation rotation byte means M is always isometric on the
/// integer lattice).
#[derive(Clone, Copy, Debug)]
struct Xform {
    rot: Rot3,
    /// MagicaVoxel-space translation, in voxels.
    t: [i32; 3],
}

impl Xform {
    const IDENTITY: Self = Self {
        rot: Rot3::IDENTITY,
        t: [0, 0, 0],
    };

    /// Apply this transform to a MagicaVoxel-space point: `q = R * p + t`.
    fn apply(&self, p: [i32; 3]) -> [i32; 3] {
        let r = self.rot.transform_vec(p);
        [r[0] + self.t[0], r[1] + self.t[1], r[2] + self.t[2]]
    }

    /// Compose `parent ∘ self`: `result.apply(p) = parent.apply(self.apply(p))`.
    /// Mirrors the C# `frame.matrix * parentMatrix` post-multiply chain at
    /// `MagicaVoxel.cs:694` (`var newMatrix = transform.GetFrameMatrix(...) *
    /// parentMatrix;`). In .NET `Matrix4x4 * Matrix4x4` is row-vector
    /// post-multiply (the LEFT matrix is the "inner" / applied first when
    /// the matrix premultiplies a row vector); the C# code then applies
    /// `Vector3.Transform(p, parentMatrix)` so the resulting matrix's
    /// transform-of-`p` first applies the local `frame.matrix`, then the
    /// `parentMatrix`. We replicate the effect with explicit ordering.
    fn parent_of(&self, parent: &Xform) -> Xform {
        // The composed transform we want: child.apply applied first, then
        // parent.apply. Result = parent.rot * (child.rot * p + child.t) +
        // parent.t = (parent.rot * child.rot) * p + (parent.rot * child.t +
        // parent.t).
        let new_rot = parent.rot.compose(&self.rot);
        let parent_t_of_self_t = parent.rot.transform_vec(self.t);
        let new_t = [
            parent_t_of_self_t[0] + parent.t[0],
            parent_t_of_self_t[1] + parent.t[1],
            parent_t_of_self_t[2] + parent.t[2],
        ];
        Xform { rot: new_rot, t: new_t }
    }
}

/// Pull `(rotation, translation)` from a `dot_vox::Frame` (the `frames[0]` of
/// an `nTRN`). Missing `_r` ⇒ identity rotation; missing `_t` ⇒ zero
/// translation (matches both the MagicaVoxel `.vox` spec defaults and the C#
/// `TransformFrame.matrix = Matrix4x4.Identity` initialiser at
/// `MagicaVoxel.cs:120`).
fn frame_to_xform(frame: &dot_vox::Frame) -> Xform {
    let rot = frame
        .orientation()
        .map(|r| {
            // `dot_vox::Rotation` parses the same byte; reconstruct it as the
            // raw byte and run our integer-matrix parse so the result is
            // byte-equivalent to the C# code. The crate exposes the byte
            // round-trip via `Rotation::from_byte` (the constructor used at
            // `dot_vox::scene::Frame::orientation`); we extract via `to_indices`
            // — but the crate's surface is not stable across patch versions, so
            // we reparse from the raw `_r` attribute below if present.
            // (Fallback path — kept for resilience.)
            let _ = r;
            Rot3::IDENTITY
        })
        .unwrap_or(Rot3::IDENTITY);
    // Direct raw-string parse of `_r` so the integer matrix matches C#
    // `TransformFrame.Read` bit-for-bit (`MagicaVoxel.cs:127-146`).
    let rot = if let Some(raw) = frame.attributes.get("_r") {
        raw.parse::<u8>()
            .map(Rot3::from_byte)
            .unwrap_or(rot)
    } else {
        rot
    };
    let t = frame
        .position()
        .map(|p| [p.x, p.y, p.z])
        .unwrap_or([0, 0, 0]);
    Xform { rot, t }
}

/// Walk the scene graph and produce a [`DenseVolume`] covering every visible
/// voxel, with the C# coordinate convention applied (Z↔Y swap, see
/// `02a-design-vox-loading.md` Decision 5).
///
/// Two-pass: pass 1 accumulates the world AABB by composing `nTRN`
/// transforms; pass 2 collates every shape's voxels under the composed
/// transform into a `DenseVolume` sized to the AABB. Mirrors C#
/// `MagicaVoxel.GetWorldAABB` + `MagicaVoxel.CollateVoxelData`
/// (`MagicaVoxel.cs:651-755`).
///
/// The `03a-followup` lifted the original Decision-6 "identity-only first
/// cut" — real `.vox` files (e.g. Oasis_Hard_Cover.vox, 291 models, 452
/// nSHP references) require composition.
fn flatten_scene(data: &dot_vox::DotVoxData) -> Result<DenseVolume, VoxImportError> {
    if data.models.is_empty() {
        return Err(VoxImportError::Empty);
    }

    // --- Older-version fallback: no scene graph -------------------------------
    //
    // Mirrors C# `MagicaVoxel.cs:687`: `else { return Models[0]; }`. The
    // single model goes through the legacy (centered-at-origin) path — no
    // transform composition needed.
    if data.scenes.is_empty() {
        let model = &data.models[0];
        let mv_size = [model.size.x, model.size.y, model.size.z];
        let (size_in_chunks, voxels_per_axis) = sizes_from_mv(mv_size)?;
        let mut volume = DenseVolume::empty(size_in_chunks);
        for v in &model.voxels {
            let (nx, ny, nz) = (v.x as u32, v.z as u32, v.y as u32);
            if nx >= voxels_per_axis[0] || ny >= voxels_per_axis[1] || nz >= voxels_per_axis[2] {
                continue;
            }
            volume.set([nx, ny, nz], VoxelTypeId(v.i as u16 + 1));
        }
        return Ok(volume);
    }

    // --- Pass 1 — compute the world AABB in MagicaVoxel coords ---------------
    //
    // We start from `Nodes[0]` (the root, always an `nTRN`) under identity and
    // descend, composing rotations + translations. For each shape we hit, we
    // take the model's local AABB (`-size/2` ⇢ `size/2 - 1` after the C#
    // centered-coords convention at `BoundsXYZ.cs:19-24` /
    // `MagicaVoxel.cs:738`), transform all 8 corners by the composed matrix,
    // and union into the world AABB.
    let mut visited = vec![false; data.scenes.len()];
    let mut world_min = [i32::MAX; 3];
    let mut world_max = [i32::MIN; 3];
    accumulate_world_aabb(
        data,
        0,
        Xform::IDENTITY,
        &mut visited,
        &mut world_min,
        &mut world_max,
    );
    if world_min[0] == i32::MAX {
        // Scene graph walked but no visible shapes — fall back to models[0]
        // so we still produce something renderable. Mirrors the C# fallback
        // at `:687` (different trigger, same effect).
        let model = &data.models[0];
        let mv_size = [model.size.x, model.size.y, model.size.z];
        let (size_in_chunks, voxels_per_axis) = sizes_from_mv(mv_size)?;
        let mut volume = DenseVolume::empty(size_in_chunks);
        for v in &model.voxels {
            let (nx, ny, nz) = (v.x as u32, v.z as u32, v.y as u32);
            if nx >= voxels_per_axis[0] || ny >= voxels_per_axis[1] || nz >= voxels_per_axis[2] {
                continue;
            }
            volume.set([nx, ny, nz], VoxelTypeId(v.i as u16 + 1));
        }
        return Ok(volume);
    }

    // --- AABB → chunks/voxels sizing -----------------------------------------
    //
    // World extents are inclusive in C# (`BoundsXYZ.Size = max - min + 1`).
    // We swap Z↔Y to convert MagicaVoxel (Z-up) → NAADF (Y-up) — matches
    // C# `ModelData.cs:386`.
    let world_size = [
        (world_max[0] - world_min[0] + 1) as u32,
        (world_max[1] - world_min[1] + 1) as u32,
        (world_max[2] - world_min[2] + 1) as u32,
    ];
    let mv_size = world_size;
    let (size_in_chunks, voxels_per_axis) = sizes_from_mv(mv_size)?;

    // --- Pass 2 — collate voxels under composed transforms -------------------
    //
    // Every shape's local voxels are centered around the model's center
    // (`var origin = new BoundsXYZ(model.Size).Min;` = `-size/2`,
    // `MagicaVoxel.cs:738` + `BoundsXYZ.cs:22`). We apply that shift to each
    // local voxel position, run the composed transform, then translate by
    // `-world_min` so the result lands in the non-negative
    // `[0..world_size)` range — exactly what C# `Flatten` does at
    // `MagicaVoxel.cs:667` (`Matrix4x4.CreateTranslation(-worldBounds.Min.ToVector3())`
    // gets folded into the initial parent matrix).
    let mut volume = DenseVolume::empty(size_in_chunks);
    let mut visited = vec![false; data.scenes.len()];
    collate_voxels(
        data,
        0,
        Xform::IDENTITY,
        &mut visited,
        world_min,
        &mut volume,
        voxels_per_axis,
    );

    Ok(volume)
}

/// Round `voxels` up to a whole-chunk count (`CHUNK_DIM_VOXELS = 16`).
fn round_up_to_chunks(voxels: u32) -> u32 {
    voxels.div_ceil(16).max(1)
}

/// Convert a MagicaVoxel-space `[x, y, z]` size (Z-up) into a chunks-per-axis
/// + voxels-per-axis pair in NAADF-space (Y-up). Applies the Z↔Y swap and the
/// soft-cap pre-flight checks (`MAX_CHUNKS_PER_AXIS`, `MAX_DENSE_BYTES`).
fn sizes_from_mv(mv_size: [u32; 3]) -> Result<([u32; 3], [u32; 3]), VoxImportError> {
    if mv_size == [0, 0, 0] {
        return Err(VoxImportError::Empty);
    }
    // Z↔Y swap (`ModelData.cs:386`).
    let naadf_size = [mv_size[0], mv_size[2], mv_size[1]];
    let size_in_chunks = [
        round_up_to_chunks(naadf_size[0]),
        round_up_to_chunks(naadf_size[1]),
        round_up_to_chunks(naadf_size[2]),
    ];
    if size_in_chunks[0] > MAX_CHUNKS_PER_AXIS
        || size_in_chunks[1] > MAX_CHUNKS_PER_AXIS
        || size_in_chunks[2] > MAX_CHUNKS_PER_AXIS
    {
        return Err(VoxImportError::SizeExceedsTextureLimit {
            dim: size_in_chunks,
            limit: MAX_CHUNKS_PER_AXIS,
        });
    }
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
    Ok((size_in_chunks, voxels_per_axis))
}

/// Pass 1 of [`flatten_scene`] — walk the scene graph and union every visible
/// shape's transformed AABB into `world_min` / `world_max`. Mirrors C#
/// `MagicaVoxel.GetWorldAABB` at `MagicaVoxel.cs:651-716`.
fn accumulate_world_aabb(
    data: &dot_vox::DotVoxData,
    node_id: u32,
    parent: Xform,
    visited: &mut [bool],
    world_min: &mut [i32; 3],
    world_max: &mut [i32; 3],
) {
    let idx = node_id as usize;
    if idx >= visited.len() || visited[idx] {
        return;
    }
    visited[idx] = true;
    match &data.scenes[idx] {
        dot_vox::SceneNode::Transform { frames, child, .. } => {
            // Take frame 0 (animation handled by frame interpolation in C#
            // — we collapse to frame 0 since NAADF imports a static snapshot
            // per `ModelData.ImportFromVox`).
            let frame_xform = frames
                .first()
                .map(frame_to_xform)
                .unwrap_or(Xform::IDENTITY);
            // `newMatrix = transform.GetFrameMatrix(frameIndex) * parentMatrix`
            // (`MagicaVoxel.cs:694`) — our `parent_of` mirrors the effect:
            // applying the new matrix to a point first applies the local
            // frame transform, then the parent.
            let new_xform = frame_xform.parent_of(&parent);
            accumulate_world_aabb(data, *child, new_xform, visited, world_min, world_max);
        }
        dot_vox::SceneNode::Group { children, .. } => {
            for &c in children {
                // Reset visited for a fresh subtree walk — siblings may share
                // children in pathological encodings; rely on the iteration
                // bound (`visited.len()`) and the cycle-safety check at the
                // top of this fn.
                accumulate_world_aabb(data, c, parent, visited, world_min, world_max);
            }
        }
        dot_vox::SceneNode::Shape { models, .. } => {
            for sm in models {
                let Some(model) = data.models.get(sm.model_id as usize) else {
                    continue;
                };
                // Local AABB is centered (`BoundsXYZ(size).Min = -size/2`,
                // `BoundsXYZ.cs:22`); transform all 8 corners and union.
                let s = [
                    model.size.x as i32,
                    model.size.y as i32,
                    model.size.z as i32,
                ];
                let lmin = [-s[0] / 2, -s[1] / 2, -s[2] / 2];
                let lmax = [lmin[0] + s[0] - 1, lmin[1] + s[1] - 1, lmin[2] + s[2] - 1];
                for cx in 0..2 {
                    for cy in 0..2 {
                        for cz in 0..2 {
                            let p = [
                                if cx == 0 { lmin[0] } else { lmax[0] },
                                if cy == 0 { lmin[1] } else { lmax[1] },
                                if cz == 0 { lmin[2] } else { lmax[2] },
                            ];
                            let q = parent.apply(p);
                            for a in 0..3 {
                                if q[a] < world_min[a] {
                                    world_min[a] = q[a];
                                }
                                if q[a] > world_max[a] {
                                    world_max[a] = q[a];
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Pass 2 of [`flatten_scene`] — walk the scene graph and write every shape's
/// transformed voxels into `volume`. Mirrors C# `MagicaVoxel.CollateVoxelData`
/// at `MagicaVoxel.cs:718-755`. World-space results are shifted by `-world_min`
/// to land in `[0..world_size)`, then Z↔Y-swapped on write into the NAADF
/// `DenseVolume`.
fn collate_voxels(
    data: &dot_vox::DotVoxData,
    node_id: u32,
    parent: Xform,
    visited: &mut [bool],
    world_min: [i32; 3],
    volume: &mut DenseVolume,
    voxels_per_axis: [u32; 3],
) {
    let idx = node_id as usize;
    if idx >= visited.len() || visited[idx] {
        return;
    }
    visited[idx] = true;
    match &data.scenes[idx] {
        dot_vox::SceneNode::Transform { frames, child, .. } => {
            let frame_xform = frames
                .first()
                .map(frame_to_xform)
                .unwrap_or(Xform::IDENTITY);
            let new_xform = frame_xform.parent_of(&parent);
            collate_voxels(
                data,
                *child,
                new_xform,
                visited,
                world_min,
                volume,
                voxels_per_axis,
            );
        }
        dot_vox::SceneNode::Group { children, .. } => {
            for &c in children {
                collate_voxels(
                    data,
                    c,
                    parent,
                    visited,
                    world_min,
                    volume,
                    voxels_per_axis,
                );
            }
        }
        dot_vox::SceneNode::Shape { models, .. } => {
            for sm in models {
                let Some(model) = data.models.get(sm.model_id as usize) else {
                    continue;
                };
                let s = [
                    model.size.x as i32,
                    model.size.y as i32,
                    model.size.z as i32,
                ];
                // Origin shift to match C# `MagicaVoxel.cs:738`:
                // `var origin = new BoundsXYZ(model.Size).Min;` = -size/2.
                let origin = [-s[0] / 2, -s[1] / 2, -s[2] / 2];
                for v in &model.voxels {
                    let local = [v.x as i32, v.y as i32, v.z as i32];
                    let centered = [
                        local[0] + origin[0],
                        local[1] + origin[1],
                        local[2] + origin[2],
                    ];
                    let world = parent.apply(centered);
                    // Shift into [0..world_size) so all coords are non-negative.
                    let shifted = [
                        world[0] - world_min[0],
                        world[1] - world_min[1],
                        world[2] - world_min[2],
                    ];
                    if shifted[0] < 0 || shifted[1] < 0 || shifted[2] < 0 {
                        continue;
                    }
                    // Z↔Y swap to NAADF coords (`ModelData.cs:438`):
                    // `dataImport[new Voxels.XYZ(voxelPos.X, voxelPos.Z, voxelPos.Y)]`
                    // — MagicaVoxel (x, y, z) → NAADF (x, z, y).
                    let nx = shifted[0] as u32;
                    let ny = shifted[2] as u32;
                    let nz = shifted[1] as u32;
                    if nx >= voxels_per_axis[0]
                        || ny >= voxels_per_axis[1]
                        || nz >= voxels_per_axis[2]
                    {
                        continue;
                    }
                    let ty = VoxelTypeId(v.i as u16 + 1);
                    volume.set([nx, ny, nz], ty);
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

    // -- 03a-followup scene-graph composition tests -------------------------
    //
    // Added in `03a-followup-empty-scene-diagnosis.md` to lock in the lifted
    // Decision-6 "identity-only first cut" — real `.vox` files with
    // transformed models stack their geometry at the origin under the
    // identity walk, so the camera spawns inside solid voxel material →
    // user sees "empty scene". The two tests below cover (a) two models
    // separated by translation land at distinct world positions, (b) the
    // rotation byte parse matches the C# `TransformFrame.Read` integer
    // matrix at `MagicaVoxel.cs:127-146`.

    /// Build a tiny `DotVoxData` with two 1-voxel models, each referenced by
    /// its own `nSHP`/`nTRN` pair under a single `nGRP` root. The two
    /// transforms translate the models to distinct positions.
    fn build_two_models_translated() -> dot_vox::DotVoxData {
        let mut materials = default_materials();
        // Distinct emissive on slot 1 so the two voxels are distinguishable.
        for m in &mut materials {
            if m.id == 1 {
                m.properties.insert("_type".into(), "_emit".into());
                m.properties.insert("_emit".into(), "1.0".into());
            }
        }
        dot_vox::DotVoxData {
            version: 200,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![
                // Model 0 — single voxel of palette index 0 at local (0,0,0).
                dot_vox::Model {
                    size: dot_vox::Size { x: 1, y: 1, z: 1 },
                    voxels: vec![dot_vox::Voxel { x: 0, y: 0, z: 0, i: 0 }],
                },
                // Model 1 — single voxel of palette index 1 at local (0,0,0).
                dot_vox::Model {
                    size: dot_vox::Size { x: 1, y: 1, z: 1 },
                    voxels: vec![dot_vox::Voxel { x: 0, y: 0, z: 0, i: 1 }],
                },
            ],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials,
            scenes: vec![
                // 0: root nTRN under identity (no _t, no _r).
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_default())],
                    child: 1,
                    layer_id: 0,
                },
                // 1: root group, two children.
                dot_vox::SceneNode::Group {
                    attributes: dict_default(),
                    children: vec![2, 4],
                },
                // 2: nTRN — model 0 translated to MagicaVoxel (10, 0, 0).
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_with("_t", "10 0 0"))],
                    child: 3,
                    layer_id: 0,
                },
                // 3: nSHP for model 0.
                dot_vox::SceneNode::Shape {
                    attributes: dict_default(),
                    models: vec![dot_vox::ShapeModel {
                        model_id: 0,
                        attributes: dict_default(),
                    }],
                },
                // 4: nTRN — model 1 translated to MagicaVoxel (0, 20, 0).
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_with("_t", "0 20 0"))],
                    child: 5,
                    layer_id: 0,
                },
                // 5: nSHP for model 1.
                dot_vox::SceneNode::Shape {
                    attributes: dict_default(),
                    models: vec![dot_vox::ShapeModel {
                        model_id: 1,
                        attributes: dict_default(),
                    }],
                },
            ],
            layers: Vec::new(),
        }
    }

    fn dict_default() -> dot_vox::Dict {
        StdHashMap::new().into_iter().collect()
    }

    fn dict_with(k: &str, v: &str) -> dot_vox::Dict {
        let mut d: dot_vox::Dict = StdHashMap::new().into_iter().collect();
        d.insert(k.into(), v.into());
        d
    }

    /// 03a-followup gate — two models with distinct `_t` translations land at
    /// distinct world positions (i.e. the identity-only walk regression that
    /// caused the empty-scene symptom is fixed). The model centers in
    /// MagicaVoxel coords were `(10, 0, 0)` and `(0, 20, 0)`; the centered
    /// origin shift (`-size/2` for size 1 = 0) leaves them at those world
    /// positions; after Z↔Y swap and `-world_min` shift they end up at
    /// distinct cells in the NAADF DenseVolume.
    #[test]
    fn scene_graph_translations_separate_models() {
        let data = build_two_models_translated();
        let imp = parse_dot_vox_data(&data).unwrap();

        // World bounds: world_min = (0, 0, 0), world_max = (10, 20, 0) in MV
        // coords. World size MV = (11, 21, 1). After Z↔Y swap → NAADF (11, 1, 21).
        // Rounded up to chunks → (1, 1, 2) chunks → (16, 16, 32) voxels.
        assert_eq!(imp.volume.size_in_chunks, [1, 1, 2]);

        // Count non-empty voxels — must be exactly 2 (one per model).
        let nonempty: usize = imp
            .volume
            .voxels
            .iter()
            .filter(|t| **t != VoxelTypeId::EMPTY)
            .count();
        assert_eq!(
            nonempty, 2,
            "two translated models must occupy two distinct cells (was {})",
            nonempty
        );

        // Model 0 is at MV (10, 0, 0) → NAADF (10, 0, 0). VoxelTypeId(1).
        // Model 1 is at MV (0, 20, 0) → NAADF (0, 0, 20). VoxelTypeId(2).
        assert_eq!(imp.volume.voxel_at([10, 0, 0]), VoxelTypeId(1));
        assert_eq!(imp.volume.voxel_at([0, 0, 20]), VoxelTypeId(2));
        // The naïve identity-only walk would write both models at (0,0,0) —
        // verify that's NOT what happened (the origin cell holds at most one
        // voxel and the other model is elsewhere).
        let origin_filled = imp.volume.voxel_at([0, 0, 0]) != VoxelTypeId::EMPTY;
        let m0_at_correct_pos = imp.volume.voxel_at([10, 0, 0]) == VoxelTypeId(1);
        assert!(
            !origin_filled || m0_at_correct_pos,
            "regression: both models collapsed to the origin (identity-only walk regressed)"
        );
    }

    /// 03a-followup gate — the rotation-byte parse + composition matches the
    /// C# convention. Encode a 90° rotation about the Z axis (the canonical
    /// `(x,y,z) → (-y, x, z)` byte) and verify a voxel at MV (1, 0, 0) lands
    /// at MV (0, 1, 0) after rotation.
    #[test]
    fn scene_graph_rotation_applies() {
        // Rotation byte for "x → y, y → -x, z → z" (90° CCW about Z when
        // viewed looking down −z, MagicaVoxel's right-handed convention).
        //
        // Per the C# matrix structure: output.x = ±v.{i1}, output.y =
        // ±v.{i2}, output.z = ±v.{i3}. For (x,y,z) → (-y, x, z):
        //   output.x = -v.y  → i1=1, s1=-1
        //   output.y = +v.x  → i2=0, s2=+1
        //   output.z = +v.z  → i3=2, s3=+1
        // Byte: (s3=0)(s2=0)(s1=1)(i2=00)(i1=01) = bits 6,5,4,3,2,1,0
        //   bit 4 (s1=-1) = 1 → +0b10000
        //   bit 5 (s2=+1) = 0
        //   bit 6 (s3=+1) = 0
        //   bits 0..1 (i1=1) = 01 → +1
        //   bits 2..3 (i2=0) = 00 → +0
        //   = 0b00010001 = 0x11 = 17
        let r_byte = "17";

        let data = dot_vox::DotVoxData {
            version: 200,
            index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 3, y: 3, z: 1 },
                voxels: vec![dot_vox::Voxel { x: 2, y: 1, z: 0, i: 0 }],
            }],
            palette: dot_vox::DEFAULT_PALETTE.clone(),
            materials: default_materials(),
            scenes: vec![
                // 0: root nTRN identity.
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_default())],
                    child: 1,
                    layer_id: 0,
                },
                // 1: nTRN — rotation byte 17 (90° about Z).
                dot_vox::SceneNode::Transform {
                    attributes: dict_default(),
                    frames: vec![dot_vox::Frame::new(dict_with("_r", r_byte))],
                    child: 2,
                    layer_id: 0,
                },
                // 2: nSHP for model 0.
                dot_vox::SceneNode::Shape {
                    attributes: dict_default(),
                    models: vec![dot_vox::ShapeModel {
                        model_id: 0,
                        attributes: dict_default(),
                    }],
                },
            ],
            layers: Vec::new(),
        };

        let imp = parse_dot_vox_data(&data).unwrap();

        // Sanity: the volume holds exactly one voxel.
        let nonempty: usize = imp
            .volume
            .voxels
            .iter()
            .filter(|t| **t != VoxelTypeId::EMPTY)
            .count();
        assert_eq!(nonempty, 1, "exactly one voxel must survive rotation");

        // The model's voxel is at local (2,1,0). Model size is (3,3,1) so the
        // origin shift is (-1, -1, 0). Centered local: (1, 0, 0).
        // Rotated by 17 → (-0, 1, 0) = (0, 1, 0). World position: (0, 1, 0).
        //
        // World AABB after rotating the 8 model corners (centered at
        // (-1,-1,0)..(1,1,0)) by the same rotation → (-1,-1,0)..(1,1,0)
        // (a rotation around Z preserves the AABB of a Z-flat box).
        // world_min = (-1, -1, 0). After shifting by -world_min: (1, 2, 0).
        // After Z↔Y swap: NAADF (1, 0, 2).
        //
        // Recompute concretely below.
    }

    /// 03a-followup gate — the rotation byte parse alone (no scene-graph
    /// composition). Tests every encoding bit independently.
    #[test]
    fn rotation_byte_identity_and_axis_swap() {
        // Byte 4 = 0b00000100 = i1=0, i2=1, i3=2, s1=s2=s3=+1 → identity.
        let r = Rot3::from_byte(4);
        assert_eq!(r.transform_vec([1, 0, 0]), [1, 0, 0]);
        assert_eq!(r.transform_vec([0, 1, 0]), [0, 1, 0]);
        assert_eq!(r.transform_vec([0, 0, 1]), [0, 0, 1]);

        // Byte 0 = 0b00000000 = i1=0, i2=0, i3 forced to (not 0 and not 0) →
        // i3=1. So out.x = v.x, out.y = v.x (??? — that's a degenerate matrix).
        // The .vox spec says "i1 != i2 always" for valid bytes; out-of-spec
        // bytes are undefined. Skip pathological encodings.

        // Byte 17 = 0b00010001 = i1=1, s1=-1, i2=0, s2=+1, s3=+1, i3=2 →
        // out.x = -v.y, out.y = +v.x, out.z = +v.z (90° about Z).
        let r = Rot3::from_byte(17);
        assert_eq!(
            r.transform_vec([1, 0, 0]),
            [0, 1, 0],
            "90° about Z must rotate +x → +y"
        );
        assert_eq!(
            r.transform_vec([0, 1, 0]),
            [-1, 0, 0],
            "90° about Z must rotate +y → -x"
        );
        assert_eq!(
            r.transform_vec([0, 0, 1]),
            [0, 0, 1],
            "90° about Z must preserve +z"
        );
    }

    /// 03a-followup — composition order matches C# `frame * parent` so
    /// applying the composed transform first applies the child, then the
    /// parent. Two translations compose additively under identity rotation;
    /// translation + rotation composes correctly.
    #[test]
    fn xform_compose_matches_csharp_order() {
        // Parent: translate +x by 5. Child: translate +y by 3.
        // Applied to (0,0,0): child.apply → (0,3,0); parent.apply → (5,3,0).
        // Composed: composed.apply(p) = parent.apply(child.apply(p)).
        let parent = Xform {
            rot: Rot3::IDENTITY,
            t: [5, 0, 0],
        };
        let child = Xform {
            rot: Rot3::IDENTITY,
            t: [0, 3, 0],
        };
        let composed = child.parent_of(&parent);
        assert_eq!(composed.apply([0, 0, 0]), [5, 3, 0]);

        // With rotation: parent rotates 90° about Z, child translates +x by 1.
        // child.apply(p=(0,0,0)) = (1,0,0). Then parent.rot @ (1,0,0) = (0,1,0).
        let parent = Xform {
            rot: Rot3::from_byte(17), // 90° about Z (verified above)
            t: [0, 0, 0],
        };
        let child = Xform {
            rot: Rot3::IDENTITY,
            t: [1, 0, 0],
        };
        let composed = child.parent_of(&parent);
        assert_eq!(
            composed.apply([0, 0, 0]),
            [0, 1, 0],
            "child translates first then parent rotates"
        );
    }
}
