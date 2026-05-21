//! Phase-C wave-3 fixture-entity spawner.
//!
//! Houses the `spawn_phase_c_test_entity` `Startup` system the `--entities`
//! e2e gate uses to populate [`super::MainWorldEntities`] with one 4×4×4
//! green-emissive entity at the test grid centre. Per-frame
//! `extract_world_changes` then runs the `EntityHandler` + uploads the result
//! into `ConstructionEvents`; the wave-3 dispatch chain
//! (`naadf_entity_update_node` + the `ray_tracing.wgsl::shoot_ray` entity
//! sub-traversal) folds it into the framebuffer.
//!
//! Gated on the [`super::SpawnTestEntity`] resource (`SpawnTestEntity(true)`)
//! via a `.run_if` registered in [`super::ConstructionPlugin::build`]. The
//! resource is set by the `--entities` boot in `bin/e2e_render.rs`. Step 8 of
//! the config-as-resource refactor migrated the gate off the former
//! `AppArgs::spawn_test_entity` boolean onto this per-domain resource.
//!
//! **Dependency note**: this fn reads
//! [`crate::voxel::grid::demo_origin_v`] to translate the
//! small-default-scene-relative entity position to the fixed-world coordinate
//! space. The function lives next to
//! [`crate::voxel::grid::DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS`] (the
//! small-world footprint it reads) so production code does not depend on
//! the `e2e` module — the D7 architect's Side note 6 dep-arrow inversion.
//! The `crate::e2e::gates::demo_origin_v` re-export still exists for the
//! e2e harness's pre-existing imports.

use bevy::math::Vec3;
use bevy::prelude::*;

use crate::aadf::entity::EntityData;
use crate::render::gpu_types::EntityInstance;

use super::MainWorldEntities;

/// Phase-C wave-3 — startup system that spawns one W4 fixture entity into
/// the main-world [`MainWorldEntities`] resource.
///
/// Fixture: a 4×4×4-voxel green-emissive block at the (sky-visible) world
/// position that the e2e camera frames in front of the look target — the
/// camera at `(86, 42, 90)` looking at `(32, 16, 32)` sees this entity high
/// + central in the framebuffer. All voxels are voxel-type 11 (green
/// emissive, `voxel/grid.rs:192-199`). The entity is at identity rotation;
/// one entity instance, `entity = 0`, `voxel_start = 0` (the first 64 u32s
/// of `entity_voxel_data`). The entity sits ~3 voxels above the existing
/// scene's tallest emissive block so the screen position is distinct.
pub fn spawn_phase_c_test_entity(mut entities: ResMut<MainWorldEntities>) {
    // 4×4×4 green-emissive entity, every voxel type = 11.
    let size = [4u32, 4, 4];
    let voxel_count = (size[0] * size[1] * size[2]) as usize;
    let types: Vec<u32> = vec![11u32; voxel_count];
    let data = EntityData::from_types(size, &types);

    // Pad to 64 u32s (NAADF `EntityHandler.cs:325-329` indexes
    // `voxelStart * 64 + voxelIndex`, and a 4×4×4 entity uses 64 voxels).
    let mut voxel_data = data.voxels.clone();
    while voxel_data.len() < 64 {
        voxel_data.push(0);
    }
    entities.voxel_data = voxel_data;
    entities.voxel_data_generation = entities.voxel_data_generation.wrapping_add(1);

    // Place at (30, 24, 30) RELATIVE TO THE SMALL DEFAULT-SCENE DEMO
    // ORIGIN. vox-gpu-rewrite Stage 2 (2026-05-18): the demo now lives
    // centered in the fixed `(4096, 512, 4096)`-voxel world, so the entity
    // position must translate through `demo_origin_v` to land in the same
    // relative spot the e2e camera frames.
    // e2e fixture entity assumes the canonical desktop world size (the fixture
    // is desktop-test-only — mobile budgets never reach this entity spawner).
    let demo_off = crate::voxel::grid::demo_origin_v(crate::WORLD_SIZE_IN_CHUNKS);
    let entity_pos = demo_off + Vec3::new(30.0, 24.0, 30.0);
    entities.instances = vec![EntityInstance {
        position: entity_pos,
        quaternion: [0.0, 0.0, 0.0, 1.0],
        voxel_start: 0,
        entity: 0,
        size,
    }];

    info!(
        "phase-c wave-3 — spawned fixture entity: 4×4×4 green-emissive @ {:?} \
         (demo-relative (30, 24, 30) + demo origin {:?}); voxel_data {} u32s",
        entity_pos,
        demo_off,
        entities.voxel_data.len()
    );
}
