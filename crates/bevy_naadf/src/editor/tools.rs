//! Track-B editor — brush footprint helpers
//! (`docs/orchestrate/feature-completeness/02b-design-editor.md` +
//!  `02c-design-edit-pipeline-alignment.md` Part 3).
//!
//! Three brushes, faithful ports of the C# tools:
//! - `paint_brush` — Euclidean `< r²` AND only replaces non-empty voxels
//!   (`EditingToolPaint.cs:69-79`).
//! - `cube_brush` — Chebyshev `< r` (`EditingToolCube.cs:76-90`); `is_erase`
//!   writes EMPTY.
//! - `sphere_brush` — Euclidean `< r²` (`EditingToolSphere.cs:76-89`);
//!   `is_erase` writes EMPTY.
//!
//! `cube_brush` + `sphere_brush` use the **C# chunk inside/mixed split** from
//! `02c`: classify each chunk in the brush AABB as inside/mixed/outside;
//! inside-chunks bulk-fill via [`WorldData::set_chunks_uniform_batch`] (one
//! memset-equivalent per chunk, zero per-voxel cost — matches C#'s
//! `Array.Fill(editData, ..., 2048)` at `EditingToolSphere.cs:98` /
//! `EditingToolCube.cs:99`); mixed-chunks fall through to per-voxel test +
//! [`WorldData::set_voxels_batch`]. This shifts the brush cost from O(r³) to
//! O(r²) for r ≫ 16.
//!
//! `paint_brush` has no inside path — `Paint` only replaces non-empty voxels
//! and there's no chunk-uniform-non-empty short-circuit (`EditingToolPaint.cs`
//! has no `chunksToEditInside[]` array; C# also walks per-voxel for Paint).

use bevy::math::{IVec3, Vec3};

use crate::voxel::{VoxelTypeId, CELL_DIM};
use crate::world::data::WorldData;

/// Classification of a chunk relative to a brush volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChunkClass {
    /// Chunk lies entirely inside the brush — every voxel passes the test.
    Inside,
    /// Chunk straddles the brush boundary — per-voxel test needed.
    Mixed,
    /// Chunk lies entirely outside the brush — skip.
    Outside,
}

/// Voxels per chunk axis (CELL_DIM² = 4² = 16, matching C# `chunk * 16` math).
const CHUNK_VOXELS: i32 = (CELL_DIM * CELL_DIM) as i32;

/// Compute the brush's affected-voxel AABB (inclusive), clamped to the world
/// bounds.
fn brush_aabb(world_data: &WorldData, pos: Vec3, radius: f32) -> (IVec3, IVec3) {
    let sx = (world_data.size_in_chunks.x as i32) * CHUNK_VOXELS;
    let sy = (world_data.size_in_chunks.y as i32) * CHUNK_VOXELS;
    let sz = (world_data.size_in_chunks.z as i32) * CHUNK_VOXELS;
    let lo = IVec3::new(
        ((pos.x - radius).floor() as i32).max(0),
        ((pos.y - radius).floor() as i32).max(0),
        ((pos.z - radius).floor() as i32).max(0),
    );
    let hi = IVec3::new(
        ((pos.x + radius).ceil() as i32).min(sx - 1),
        ((pos.y + radius).ceil() as i32).min(sy - 1),
        ((pos.z + radius).ceil() as i32).min(sz - 1),
    );
    (lo, hi)
}

/// Compute the brush's affected-chunk AABB (inclusive), clamped to the world
/// bounds in chunks. Matches C# `minChunkPos`/`maxChunkPos` computation at
/// `EditingToolSphere.cs:53-56` / `EditingToolCube.cs:53-56`.
fn brush_chunk_aabb(world_data: &WorldData, pos: Vec3, radius: f32) -> (IVec3, IVec3) {
    let sx_c = world_data.size_in_chunks.x as i32;
    let sy_c = world_data.size_in_chunks.y as i32;
    let sz_c = world_data.size_in_chunks.z as i32;
    let lo = IVec3::new(
        (((pos.x - radius) / CHUNK_VOXELS as f32).floor() as i32).max(0),
        (((pos.y - radius) / CHUNK_VOXELS as f32).floor() as i32).max(0),
        (((pos.z - radius) / CHUNK_VOXELS as f32).floor() as i32).max(0),
    );
    let hi = IVec3::new(
        (((pos.x + radius) / CHUNK_VOXELS as f32).floor() as i32).min(sx_c - 1),
        (((pos.y + radius) / CHUNK_VOXELS as f32).floor() as i32).min(sy_c - 1),
        (((pos.z + radius) / CHUNK_VOXELS as f32).floor() as i32).min(sz_c - 1),
    );
    (lo, hi)
}

/// Sphere chunk classification. Verbatim port of `EditingToolSphere.cs:69-74`:
/// `radiusInsideSqr = max(0, radius - |(7.5,7.5,7.5)|)²`,
/// `radiusOutsideSqr = max(0, radius + |(7.5,7.5,7.5)|)²`. Distance is from
/// chunk center `chunk_pos * 16 + (8,8,8)` to `pos`, squared.
fn sphere_chunk_classify(chunk_pos: IVec3, pos: Vec3, radius: f32) -> ChunkClass {
    let chunk_center = (chunk_pos * CHUNK_VOXELS).as_vec3() + Vec3::splat(8.0);
    let dist_sqr = (chunk_center - pos).length_squared();
    let diag = Vec3::splat(7.5).length(); // √(3·7.5²) ≈ 12.99
    let r_inside = (radius - diag).max(0.0);
    let r_outside = (radius + diag).max(0.0);
    let r_inside_sqr = r_inside * r_inside;
    let r_outside_sqr = r_outside * r_outside;
    if dist_sqr < r_inside_sqr {
        ChunkClass::Inside
    } else if dist_sqr < r_outside_sqr {
        ChunkClass::Mixed
    } else {
        ChunkClass::Outside
    }
}

/// Cube chunk classification. Verbatim port of `EditingToolCube.cs:58-59,68-73`:
/// uses Chebyshev (max-abs) distance with cushions `radiusInside = max(0,
/// radius - 16)` and `radiusOutside = max(0, radius + 16)`.
fn cube_chunk_classify(chunk_pos: IVec3, pos: Vec3, radius: f32) -> ChunkClass {
    let chunk_center = (chunk_pos * CHUNK_VOXELS).as_vec3() + Vec3::splat(8.0);
    let d = chunk_center - pos;
    let cheb = d.x.abs().max(d.y.abs()).max(d.z.abs());
    let r_inside = (radius - 16.0).max(0.0);
    let r_outside = (radius + 16.0).max(0.0);
    if cheb < r_inside {
        ChunkClass::Inside
    } else if cheb < r_outside {
        ChunkClass::Mixed
    } else {
        ChunkClass::Outside
    }
}

/// Paint brush — replaces existing non-empty voxels within Euclidean radius
/// with `ty`. Faithful port of `EditingToolPaint.cs:69-79`.
///
/// Note: Paint takes **no** `is_erase` — C# `Paint` has no `isErase` field.
/// The semantic is "replace non-empty voxels with the selected type"; erasure
/// belongs on Cube or Sphere with `is_erase = true`.
///
/// Paint has no inside-chunk fast path (C# `EditingToolPaint.cs` likewise has
/// no `chunksToEditInside[]` array). Paint walks per-voxel across the full
/// brush AABB and applies `get_voxel_type` per voxel to gate non-empty
/// replacement.
pub fn paint_brush(world_data: &mut WorldData, pos: Vec3, radius: f32, ty: VoxelTypeId) {
    if radius <= 0.0 {
        return;
    }
    let r2 = radius * radius;
    let mut edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();
    let (lo, hi) = brush_aabb(world_data, pos, radius);
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let voxel = IVec3::new(x, y, z);
                // Voxel-centre vs pos — matches C# `Paint.cs:68-72` translated.
                let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos;
                if d.length_squared() < r2 {
                    // Paint: replace only existing non-empty voxels.
                    if world_data
                        .get_voxel_type(voxel)
                        .is_some_and(|t| t != VoxelTypeId::EMPTY)
                    {
                        edits.push((voxel, ty));
                    }
                }
            }
        }
    }
    if !edits.is_empty() {
        world_data.set_voxels_batch(&edits);
    }
}

/// Cube brush — Chebyshev distance `< r`. Solid (writes ALL voxels in the
/// Chebyshev cube). `is_erase = true` writes `VoxelTypeId::EMPTY`.
/// Faithful port of `EditingToolCube.cs:76-90` with the **chunk inside/mixed
/// split** (`02c` Part 3 / `EditingToolCube.cs:62-101`).
pub fn cube_brush(
    world_data: &mut WorldData,
    pos: Vec3,
    radius: f32,
    ty: VoxelTypeId,
    is_erase: bool,
) {
    if radius <= 0.0 {
        return;
    }
    let target = if is_erase { VoxelTypeId::EMPTY } else { ty };
    let target_opt: Option<VoxelTypeId> = if is_erase { None } else { Some(ty) };
    let (min_chunk, max_chunk) = brush_chunk_aabb(world_data, pos, radius);

    let mut inside_chunks: Vec<([u32; 3], Option<VoxelTypeId>)> = Vec::new();
    let mut mixed_chunk_edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();

    for cz in min_chunk.z..=max_chunk.z {
        for cy in min_chunk.y..=max_chunk.y {
            for cx in min_chunk.x..=max_chunk.x {
                let chunk_pos = IVec3::new(cx, cy, cz);
                match cube_chunk_classify(chunk_pos, pos, radius) {
                    ChunkClass::Outside => continue,
                    ChunkClass::Inside => {
                        inside_chunks.push((
                            [cx as u32, cy as u32, cz as u32],
                            target_opt,
                        ));
                    }
                    ChunkClass::Mixed => {
                        // Per-voxel Chebyshev test, only over the chunk's
                        // 16-voxel local range.
                        let chunk_origin = chunk_pos * CHUNK_VOXELS;
                        for lz in 0..CHUNK_VOXELS {
                            for ly in 0..CHUNK_VOXELS {
                                for lx in 0..CHUNK_VOXELS {
                                    let voxel = chunk_origin + IVec3::new(lx, ly, lz);
                                    let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos;
                                    let cheb = d.x.abs().max(d.y.abs()).max(d.z.abs());
                                    if cheb < radius {
                                        mixed_chunk_edits.push((voxel, target));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if !inside_chunks.is_empty() {
        world_data.set_chunks_uniform_batch(&inside_chunks);
    }
    if !mixed_chunk_edits.is_empty() {
        world_data.set_voxels_batch(&mixed_chunk_edits);
    }
}

/// Sphere brush — Euclidean `r²` distance check. Solid (writes ALL voxels
/// inside the sphere, unlike Paint). `is_erase = true` writes
/// `VoxelTypeId::EMPTY`. Faithful port of `EditingToolSphere.cs:76-89` with
/// the **chunk inside/mixed split** (`02c` Part 3 /
/// `EditingToolSphere.cs:62-100`).
pub fn sphere_brush(
    world_data: &mut WorldData,
    pos: Vec3,
    radius: f32,
    ty: VoxelTypeId,
    is_erase: bool,
) {
    if radius <= 0.0 {
        return;
    }
    let target = if is_erase { VoxelTypeId::EMPTY } else { ty };
    let target_opt: Option<VoxelTypeId> = if is_erase { None } else { Some(ty) };
    let r2 = radius * radius;
    let (min_chunk, max_chunk) = brush_chunk_aabb(world_data, pos, radius);

    let mut inside_chunks: Vec<([u32; 3], Option<VoxelTypeId>)> = Vec::new();
    let mut mixed_chunk_edits: Vec<(IVec3, VoxelTypeId)> = Vec::new();

    for cz in min_chunk.z..=max_chunk.z {
        for cy in min_chunk.y..=max_chunk.y {
            for cx in min_chunk.x..=max_chunk.x {
                let chunk_pos = IVec3::new(cx, cy, cz);
                match sphere_chunk_classify(chunk_pos, pos, radius) {
                    ChunkClass::Outside => continue,
                    ChunkClass::Inside => {
                        inside_chunks.push((
                            [cx as u32, cy as u32, cz as u32],
                            target_opt,
                        ));
                    }
                    ChunkClass::Mixed => {
                        // Per-voxel Euclidean test, only over the chunk's
                        // 16-voxel local range.
                        let chunk_origin = chunk_pos * CHUNK_VOXELS;
                        for lz in 0..CHUNK_VOXELS {
                            for ly in 0..CHUNK_VOXELS {
                                for lx in 0..CHUNK_VOXELS {
                                    let voxel = chunk_origin + IVec3::new(lx, ly, lz);
                                    let d = (voxel.as_vec3() + Vec3::splat(0.5)) - pos;
                                    if d.length_squared() < r2 {
                                        mixed_chunk_edits.push((voxel, target));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if !inside_chunks.is_empty() {
        world_data.set_chunks_uniform_batch(&inside_chunks);
    }
    if !mixed_chunk_edits.is_empty() {
        world_data.set_voxels_batch(&mixed_chunk_edits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::data::{IAabb3, PendingEdits, WorldData};
    use bevy::math::UVec3;

    fn make_empty_world(size_in_chunks: UVec3) -> WorldData {
        let n_chunks = (size_in_chunks.x * size_in_chunks.y * size_in_chunks.z) as usize;
        let size_v = size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
        WorldData {
            chunks_cpu: vec![0u32; n_chunks],
            blocks_cpu: Vec::new(),
            voxels_cpu: Vec::new(),
            size_in_chunks,
            bounding_box: IAabb3 {
                min: IVec3::ZERO,
                max: IVec3::new(size_v.x as i32 - 1, size_v.y as i32 - 1, size_v.z as i32 - 1),
            },
            pending_edits: PendingEdits::default(),
            dense_voxel_types: Vec::new(),
        }
    }

    /// Test #7 — sphere brush produces a solid sphere; all voxels in r²
    /// distance carry the target type, voxels outside stay empty.
    #[test]
    fn sphere_brush_produces_solid_sphere() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        let pos = Vec3::new(32.0, 16.0, 32.0);
        let radius = 4.0;
        let ty = VoxelTypeId(7);
        sphere_brush(&mut wd, pos, radius, ty, false);

        // Sample inside the sphere — voxel at (32,16,32) centre + 0.5: d≈0.5,
        // length_squared ~0.75 < 16 → should be type 7.
        assert_eq!(wd.get_voxel_type(IVec3::new(32, 16, 32)), Some(ty));
        assert_eq!(wd.get_voxel_type(IVec3::new(33, 16, 32)), Some(ty));
        assert_eq!(wd.get_voxel_type(IVec3::new(30, 14, 30)), Some(ty));
        // Voxel well outside the sphere — empty.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(50, 16, 50)),
            Some(VoxelTypeId::EMPTY)
        );
    }

    /// Test #8 — cube brush produces a Chebyshev cube; corners of the cube
    /// (cheb = r-1) inside, voxels with cheb >= r outside.
    #[test]
    fn cube_brush_produces_solid_cube() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        let pos = Vec3::new(32.5, 16.5, 32.5);
        let radius = 3.0;
        let ty = VoxelTypeId(11);
        cube_brush(&mut wd, pos, radius, ty, false);
        // Voxel at corner of the cube — (30,14,30): cheb = max(|30.5-32.5|,
        // |14.5-16.5|, |30.5-32.5|) = 2 < 3 → inside.
        assert_eq!(wd.get_voxel_type(IVec3::new(30, 14, 30)), Some(ty));
        // Centre — definitely inside.
        assert_eq!(wd.get_voxel_type(IVec3::new(32, 16, 32)), Some(ty));
        // Just outside — (29,14,30): cheb = 3 NOT < 3 → outside.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(29, 14, 30)),
            Some(VoxelTypeId::EMPTY)
        );
    }

    /// Test #9 — paint brush only replaces existing non-empty voxels. Set up
    /// a "ground" of type 1 voxels then paint type 7 over a sphere; voxels
    /// inside the sphere AND non-empty become 7, voxels inside the sphere
    /// but empty stay empty.
    #[test]
    fn paint_brush_only_replaces_non_empty() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        // Seed a 5x1x5 ground patch of type 1 at y=2.
        let mut seed = Vec::new();
        for z in 4..=8 {
            for x in 4..=8 {
                seed.push((IVec3::new(x, 2, z), VoxelTypeId(1)));
            }
        }
        wd.set_voxels_batch(&seed);
        // Sanity — ground is type 1.
        assert_eq!(wd.get_voxel_type(IVec3::new(6, 2, 6)), Some(VoxelTypeId(1)));
        // Sanity — air above is empty.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(6, 4, 6)),
            Some(VoxelTypeId::EMPTY)
        );
        // Paint a radius-4 sphere centred 1 voxel above the ground.
        paint_brush(&mut wd, Vec3::new(6.5, 3.5, 6.5), 4.0, VoxelTypeId(7));
        // Ground voxel inside the sphere now type 7.
        assert_eq!(wd.get_voxel_type(IVec3::new(6, 2, 6)), Some(VoxelTypeId(7)));
        // Air voxel inside the sphere stays empty (Paint does NOT fill empty).
        assert_eq!(
            wd.get_voxel_type(IVec3::new(6, 4, 6)),
            Some(VoxelTypeId::EMPTY)
        );
    }

    /// Test #10 — erase mode on sphere brush clears existing full voxels.
    #[test]
    fn erase_with_sphere_clears_voxels() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        // Seed a full 5x5x5 cube of type 1 around (10, 10, 10).
        let mut seed = Vec::new();
        for z in 8..=12 {
            for y in 8..=12 {
                for x in 8..=12 {
                    seed.push((IVec3::new(x, y, z), VoxelTypeId(1)));
                }
            }
        }
        wd.set_voxels_batch(&seed);
        // Sanity — cube voxel is type 1.
        assert_eq!(wd.get_voxel_type(IVec3::new(10, 10, 10)), Some(VoxelTypeId(1)));
        // Erase with sphere centred on the cube.
        sphere_brush(&mut wd, Vec3::new(10.5, 10.5, 10.5), 3.0, VoxelTypeId(0), true);
        // Centre now empty.
        assert_eq!(
            wd.get_voxel_type(IVec3::new(10, 10, 10)),
            Some(VoxelTypeId::EMPTY)
        );
    }

    /// `02c` Test #8 — sphere brush's inside-chunk path uses the bulk
    /// uniform-fill API. Place a sphere with `radius >= 13 + 16` so at least
    /// one chunk is fully inside the sphere; assert that chunk's resulting
    /// `chunks_cpu` entry encodes `UniformFull(ty)` (state=1, low 15 bits=ty)
    /// and NO blocks/voxels were uploaded for it.
    #[test]
    fn sphere_brush_chunk_inside_path_uses_set_chunks_uniform() {
        let mut wd = make_empty_world(UVec3::new(4, 4, 4));
        // Centre at the midpoint of chunk (2,2,2) — chunk_center = (40,40,40).
        // radius = 30 > 13 + 16 = 29 → chunk (2,2,2) is "inside" per the
        // sphere classifier (dist=0 < (30-12.99)² ≈ 290).
        let pos = Vec3::new(40.0, 40.0, 40.0);
        let radius = 30.0;
        let ty = VoxelTypeId(9);
        sphere_brush(&mut wd, pos, radius, ty, false);

        // Chunk (2,2,2) flat index — should be UniformFull(9) state-encoded.
        let ci = 2 + 2 * 4 + 2 * 16;
        let chunk_raw = wd.chunks_cpu[ci];
        let state = chunk_raw >> 30;
        assert_eq!(state, 1, "chunk (2,2,2) should be UniformFull, got state={state}");
        assert_eq!(chunk_raw & 0x7FFF, 9, "chunk (2,2,2) type should be 9");

        // The inside-chunk fast path must NOT have emitted block/voxel uploads
        // for chunk (2,2,2). Inspect the first batch — it should be the
        // uniform-batch (chunk-only). The mixed-chunk batch (if present) is
        // separate.
        let uniform_batch = wd
            .pending_edits
            .batches
            .iter()
            .find(|b| b.changed_blocks.is_empty() && b.changed_voxels.is_empty())
            .expect("expected a chunk-only uniform batch from inside path");
        // The chunk (2,2,2) entry should be present.
        let target = crate::aadf::edit::pack_chunk_pos([2, 2, 2]);
        assert!(
            uniform_batch
                .changed_chunks
                .iter()
                .any(|e| e[0] == target && e[1] >> 30 == 1),
            "uniform batch should contain chunk (2,2,2) UniformFull entry"
        );
    }

    /// `02c` Test #9 — sphere brush's outside-chunk path skips. Place a small
    /// sphere at one corner and assert chunks well outside the brush radius
    /// are untouched (their `chunks_cpu` slice is identical pre/post).
    #[test]
    fn sphere_brush_chunk_outside_path_skipped() {
        let mut wd = make_empty_world(UVec3::new(4, 4, 4));
        let pre = wd.chunks_cpu.clone();
        // Tiny sphere at corner — voxels around (4,4,4), radius 5. Only chunk
        // (0,0,0) and (slight reach) neighbour chunks get touched.
        sphere_brush(&mut wd, Vec3::new(4.0, 4.0, 4.0), 5.0, VoxelTypeId(7), false);
        // A far-away chunk (3,3,3) — definitely outside — must be untouched.
        let ci_far = 3 + 3 * 4 + 3 * 16;
        assert_eq!(
            wd.chunks_cpu[ci_far], pre[ci_far],
            "far chunk (3,3,3) should be untouched by sphere at (4,4,4) r=5"
        );
    }

    /// `02c` Test #10 — runtime `set_voxels_batch` does NOT emit whole-world
    /// `changed_chunks` uploads. With a single voxel edit, the batch must
    /// contain exactly 1 `changed_chunks` entry (the edited chunk) — not the
    /// whole-world AADF-changed set the pre-`02c` recompute path produced.
    #[test]
    fn runtime_path_does_not_emit_whole_world_uploads() {
        let mut wd = make_empty_world(UVec3::new(4, 2, 4));
        wd.set_voxels_batch(&[(IVec3::new(5, 5, 5), VoxelTypeId(3))]);
        let batch = wd
            .pending_edits
            .batches
            .first()
            .expect("expected one batch");
        assert_eq!(
            batch.changed_chunks.len(),
            1,
            "runtime path must produce ONE changed_chunks entry per directly-edited chunk, got {}",
            batch.changed_chunks.len()
        );
    }

    /// `02c` Test #12 — `set_chunks_uniform_batch` basic functionality.
    /// One chunk uniform-fill emits one changed_chunks entry with the
    /// expected encoding, no block/voxel uploads.
    #[test]
    fn set_chunks_uniform_batch_basic() {
        let mut wd = make_empty_world(UVec3::new(2, 2, 2));
        wd.set_chunks_uniform_batch(&[([0, 0, 0], Some(VoxelTypeId(5)))]);
        let batch = wd
            .pending_edits
            .batches
            .first()
            .expect("expected one batch");
        assert_eq!(batch.changed_chunks.len(), 1);
        assert!(batch.changed_blocks.is_empty());
        assert!(batch.changed_voxels.is_empty());
        // chunks_cpu[0] = UniformFull(5) → state=1 | type=5.
        assert_eq!(wd.chunks_cpu[0] >> 30, 1);
        assert_eq!(wd.chunks_cpu[0] & 0x7FFF, 5);
    }

    /// `02c` Test — sphere chunk classifier returns Outside / Mixed / Inside
    /// at the boundary cases. Hand-computed boundary cases per
    /// `EditingToolSphere.cs:69-74`.
    #[test]
    fn sphere_chunk_classify_boundary_cases() {
        // Chunk at world origin — chunk_pos (0,0,0), center (8,8,8).
        let chunk_pos = IVec3::new(0, 0, 0);
        let center = Vec3::new(8.0, 8.0, 8.0);
        let _diag = Vec3::splat(7.5).length(); // ≈ 12.99 (commentary only)
        // Brush AT chunk center, r=30 → dist=0, r-diag ≈ 17 → inside.
        assert_eq!(
            sphere_chunk_classify(chunk_pos, center, 30.0),
            ChunkClass::Inside
        );
        // Brush AT chunk center, r=10 → dist=0, r-diag=-2.99 → r_inside=0 →
        // dist_sqr=0 < r_inside_sqr=0 is FALSE; r+diag=22.99 → dist_sqr=0 <
        // r_outside_sqr=528 → Mixed.
        assert_eq!(
            sphere_chunk_classify(chunk_pos, center, 10.0),
            ChunkClass::Mixed
        );
        // Brush far from chunk, r=5 — dist=100>>r_outside=18 → Outside.
        let far = Vec3::new(100.0, 100.0, 100.0);
        assert_eq!(
            sphere_chunk_classify(chunk_pos, far, 5.0),
            ChunkClass::Outside
        );
    }
}
