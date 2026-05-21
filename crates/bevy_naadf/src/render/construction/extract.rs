//! Phase-C — `ExtractSchedule` system: pulls per-frame edit state + entity
//! state from the main world into the render-world resources.
//!
//! Owns:
//!   - [`MainWorldEntities`] — main-world Resource holding live entity
//!     instances + per-entity voxel-volume data.
//!   - [`RenderWorldEntityState`] — render-world Resource holding the
//!     across-frame `EntityHandler` state.
//!   - [`extract_world_changes`] — the `ExtractSchedule` system that drains
//!     `WorldData::pending_edits` + folds `EntityHandler::update` into the
//!     render-world `ConstructionEvents`.
//!
//! The render-world `ConstructionEvents` resource lives in `mod.rs` because
//! it is the cross-workstream W2+W4 edit-batch consumed by the producer node
//! + every regime-3 dispatch.

use bevy::prelude::*;

use super::{change_handler, entity_handler, ConstructionEvents};

/// Phase-C wave-3 — main-world resource holding the live entity list + the
/// `EntityHandler` state (`15-design-c.md` §3.6, `16-impl-c-W4.md` integration
/// notes).
///
/// **Optional**: the resource is absent on the no-entities path (the normal
/// e2e / baseline render). When present, the per-frame extract calls
/// `EntityHandler::update(&self.instances)` and forwards the resulting
/// `EntityUpdateUploads` to the render-world `ConstructionEvents`. The
/// renderer-side dispatch chain (W4 `entity_update.wgsl` 3 entry points + the
/// `shoot_ray` entity sub-traversal) fires automatically once the bind groups
/// are built.
///
/// The `--entities` e2e mode inserts one entity here so the rendered
/// framebuffer carries the entity hit on top of the world geometry.
#[derive(Resource, Default)]
pub struct MainWorldEntities {
    /// Live entity instances for this frame.
    pub instances: Vec<crate::render::gpu_types::EntityInstance>,
    /// Per-entity-id voxel-volume builds (`EntityData` from
    /// `aadf::entity::EntityData::from_types`). Index = entity id; each
    /// entry's 64 u32s is concatenated into the `entity_voxel_data` GPU
    /// buffer in upload order. The render path consumes this through
    /// `EntityInstance::voxel_start` (the C# pre-computes the offset; for
    /// the test fixture all entities are the same and `voxel_start = 0`).
    pub voxel_data: Vec<u32>,
    /// Generation counter — bumped whenever `voxel_data` changes. The
    /// render-world extract sees the change via the `Last`-set value vs the
    /// stored render-side mirror and triggers re-upload.
    pub voxel_data_generation: u32,
}

/// Phase-C wave-3 — main-world flag: when `true`, the `Startup` system
/// [`super::test_fixture::spawn_phase_c_test_entity`] runs and populates
/// [`MainWorldEntities`] with one 4×4×4 emissive-voxel fixture block.
///
/// Migrated out of `AppArgs.spawn_test_entity` in **Step 8** of the
/// config-as-resource refactor (`docs/orchestrate/config-as-resource-refactor/02-design.md`,
/// Decision §4 — kept as a plain `bool` newtype, not `Option<TestEntityFixture>`,
/// because the fixture is content-static with no per-fixture parameters).
/// `SpawnTestEntity::default()` = `SpawnTestEntity(false)` — the fixture is
/// off unless `e2e_render --entities` flips it on. Main-world only; the
/// e2e driver also reads it via `Option<Res<SpawnTestEntity>>` to pick the
/// entity-aware ASSERT baseline.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpawnTestEntity(pub bool);

/// Phase-C wave-3 — render-world resource holding the W4 `EntityHandler`
/// state (across-frame: per-chunk entity-count u32 table + last-frame
/// overlapped chunks list). Lives in the render world so the `ExtractSchedule`
/// system can `&mut` it (Bevy's `Extract<>` only supports read-only main-world
/// access; render-world state goes through `Res` / `ResMut`).
///
/// Updated each frame by [`extract_world_changes`]: reads main-world
/// `MainWorldEntities::instances` and calls `handler.update(instances)` to
/// produce the per-frame uploads. The previous-tracked voxel-data generation
/// is also stored here so the extract can decide whether to copy the
/// (potentially large) voxel-data buffer.
#[derive(Resource)]
pub struct RenderWorldEntityState {
    pub handler: Option<entity_handler::EntityHandler>,
    pub last_uploaded_voxel_data_generation: u32,
}

impl Default for RenderWorldEntityState {
    fn default() -> Self {
        Self {
            handler: None,
            // Distinct from MainWorldEntities default (0) so the first frame
            // with any non-empty voxel_data triggers an upload.
            last_uploaded_voxel_data_generation: u32::MAX,
        }
    }
}

/// `ExtractSchedule` system: mirror the main-world [`crate::world::data::WorldData::pending_edits`]
/// into the render-world [`ConstructionEvents`] resource.
///
/// Drains the main-world `pending_edits.batches` + `pending_edits.edited_groups`
/// each frame: aggregates the per-batch `changed_*` arrays into the render-world
/// resource and runs the CPU flood-fill via
/// [`change_handler::compute_change_groups`] to produce `changed_groups`.
///
/// Phase-C wave-3 — also reads the optional [`MainWorldEntities`] and folds
/// the per-frame `EntityHandler::update` result into
/// [`ConstructionEvents::entity_uploads`].
pub fn extract_world_changes(
    mut commands: Commands,
    main_world: ResMut<bevy::render::MainWorld>,
    entity_state: Option<ResMut<RenderWorldEntityState>>,
) {
    // `02f-followup` — pull `WorldData` mutably from the main world via the
    // `ResMut<MainWorld>` pattern. The previous `Extract<Res<WorldData>>`
    // read-only path coexisted with a separate `clear_world_data_pending_edits`
    // system in main-world `Last`, but `Last` runs BEFORE the render sub-app's
    // ExtractSchedule in the standard Bevy schedule order (both with and
    // without pipelined rendering — see bevy_render-0.19's
    // `pipelined_rendering.rs` lines 75-92 schedule diagram). So the clear
    // raced ahead of the extract, the queue was empty by the time this system
    // ran, and the W2 GPU dispatch never fired on user-driven edits. Mutating
    // main-world from `ExtractSchedule` via `MainWorld` is the
    // Bevy-sanctioned pattern (`bevy::render::MainWorld` doc); it folds the
    // drain into the consume site, eliminating the race.
    let main_world: &mut bevy::ecs::world::World = &mut **main_world.into_inner();

    // Read `MainWorldEntities` (optional, read-only) before we take a mut
    // borrow on `WorldData`. Clone the small struct so we can drop the
    // borrow.
    let main_world_entities: Option<(
        Vec<crate::render::gpu_types::EntityInstance>,
        Vec<u32>,
        u32,
    )> = main_world
        .get_resource::<MainWorldEntities>()
        .map(|me| (me.instances.clone(), me.voxel_data.clone(), me.voxel_data_generation));

    // Now take the mutable WorldData borrow + drain.
    let Some(mut world_data) = main_world.get_resource_mut::<crate::world::data::WorldData>() else {
        commands.insert_resource(ConstructionEvents::default());
        return;
    };

    let mut events = ConstructionEvents::default();
    // Drain every batch's per-buffer payload into the render-world resource.
    // The main-world `WorldData::pending_edits` accumulates per-set_voxel
    // batches; we move them out here so the next main-world tick starts
    // with an empty queue — no separate `Last`-schedule clear needed.
    let drained_batches: Vec<crate::aadf::edit::EditBatch> =
        std::mem::take(&mut world_data.pending_edits.batches);
    let drained_groups: Vec<[u32; 3]> =
        std::mem::take(&mut world_data.pending_edits.edited_groups);
    for batch in &drained_batches {
        events.changed_chunks.extend_from_slice(&batch.changed_chunks);
        events.changed_blocks.extend_from_slice(&batch.changed_blocks);
        events.changed_voxels.extend_from_slice(&batch.changed_voxels);
    }
    events.changed_chunk_count = events.changed_chunks.len() as u32;
    // Block/voxel counts = number of 65-u32 / 33-u32 records.
    events.changed_block_count = (events.changed_blocks.len() / 65) as u32;
    events.changed_voxel_count = (events.changed_voxels.len() / 33) as u32;

    // `02f-followup` — debug-log when the extract sees a non-trivial edit
    // batch. Useful for regression diagnosis (if a future change re-breaks
    // the drain, this trace surfaces it in `RUST_LOG=debug` runs). Cheap
    // when empty — the `if` guard means no log allocation on no-edit frames
    // (the steady state).
    if !drained_batches.is_empty() {
        bevy::log::debug!(
            "extract_world_changes drained: {} batches, {} changed_chunks, \
             {} changed_blocks, {} changed_voxels, {} edited_groups",
            drained_batches.len(),
            events.changed_chunk_count,
            events.changed_block_count,
            events.changed_voxel_count,
            drained_groups.len(),
        );
    }

    // CPU flood-fill — produce `changed_groups_dynamic`.
    let size_in_chunks = world_data.size_in_chunks;
    if !drained_groups.is_empty()
        && size_in_chunks.x > 0
        && size_in_chunks.y > 0
        && size_in_chunks.z > 0
    {
        let size_in_groups = [
            size_in_chunks.x / 4,
            size_in_chunks.y / 4,
            size_in_chunks.z / 4,
        ];
        // Skip the flood fill if any axis would be 0 groups (test grid sizes
        // smaller than 4 chunks have no bound groups at all — W3 layout is
        // dormant there).
        if size_in_groups[0] > 0 && size_in_groups[1] > 0 && size_in_groups[2] > 0 {
            // Dedup directly-edited groups (multiple voxel edits in the same
            // group count once).
            let mut uniq: Vec<[u32; 3]> = Vec::new();
            for &g in &drained_groups {
                if !uniq.contains(&g) {
                    uniq.push(g);
                }
            }
            let groups = change_handler::compute_change_groups(size_in_groups, &uniq);
            events.changed_group_count = groups.entries.len() as u32;
            events.changed_groups = groups.entries;
        }
    }

    // Drop the WorldData borrow so the entity handler logic can run without
    // borrow conflicts (it reads world_data size_in_chunks which we cached
    // above).
    drop(world_data);

    // === Phase-C wave-3 — entity uploads ====================================
    // When the main-world `MainWorldEntities` resource exists and carries at
    // least one instance, run `EntityHandler::update` and fold the result into
    // `ConstructionEvents.entity_uploads`. The render-side dispatch + the
    // chunks-texture `.y` write fire next frame.
    if let (Some((instances, voxel_data, voxel_data_generation)), Some(mut state)) =
        (main_world_entities, entity_state)
    {
        // Mirror voxel-data into `ConstructionEvents` whenever the generation
        // counter changes.
        if voxel_data_generation != state.last_uploaded_voxel_data_generation {
            events.entity_voxel_data = voxel_data;
            events.entity_voxel_data_dirty = true;
            state.last_uploaded_voxel_data_generation = voxel_data_generation;
        }

        if !instances.is_empty() {
            if state.handler.is_none() {
                state.handler = Some(entity_handler::EntityHandler::new([
                    size_in_chunks.x,
                    size_in_chunks.y,
                    size_in_chunks.z,
                ]));
            }
            if let Some(handler) = state.handler.as_mut() {
                events.entity_uploads = handler.update(&instances);
            }
        }
    }

    commands.insert_resource(events);
}
