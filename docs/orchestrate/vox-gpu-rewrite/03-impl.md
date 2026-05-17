# vox-gpu-rewrite — implementation log

Per-subtask impl findings appended in landing order (W5.1 → W5.2 → W5.5 →
W5.3 → W5.4 → W5.6). Each section reports files touched, verification
outcomes, design-adherence confirmation, and any surprises.

---

## impl W5.1 findings (2026-05-17)

### Files touched

- `crates/bevy_naadf/src/aadf/generator.rs:46` — added
  `use bevy::prelude::Resource;` import (the module previously used no Bevy
  types).
- `crates/bevy_naadf/src/aadf/generator.rs:74` — changed `ModelData` derive
  from `#[derive(Clone, Debug)]` → `#[derive(Resource, Clone, Debug)]` so it
  can be inserted as a main-world resource (per design §W5.1).
- `crates/bevy_naadf/src/render/extract.rs:121-145` — added the
  `ModelDataRender` render-world resource (vox-gpu-rewrite W5.1). Field set
  mirrors `aadf::generator::ModelData` exactly.
- `crates/bevy_naadf/src/render/extract.rs:205-237` — added the
  `stage_model_data_buildonce` ExtractSchedule system. Gates on
  `Option<Res<ModelDataRender>>::is_none()` and clones from the main-world
  `ModelData` resource exactly once. Mirrors `stage_world_gpu_buildonce`
  shape 1:1.
- `crates/bevy_naadf/src/render/mod.rs:42-46` — added
  `stage_model_data_buildonce` + `ModelDataRender` to the `extract` use
  block.
- `crates/bevy_naadf/src/render/mod.rs:122-129` — added
  `.init_resource::<ModelDataRender>()` immediately after
  `.init_resource::<WorldDataMeta>()`.
- `crates/bevy_naadf/src/render/mod.rs:138-150` — added
  `stage_model_data_buildonce` to the `ExtractSchedule` system tuple,
  immediately after `stage_world_gpu_buildonce`.
- `crates/bevy_naadf/src/voxel/grid.rs:300-430` — rewrote
  `install_vox_in_fixed_world` per design §W5.1. Parse path swapped from
  `vox_import::load_vox_into_world` (CPU XZ-tile stop-gap, soon-to-be-deleted
  in W5.4) to `vox_import::parse_dot_vox_data` (single-tile import). Converts
  the parsed `ConstructedWorld` → `aadf::generator::ModelData` and inserts
  it as a main-world Resource. Inserts an **empty** `WorldData` at
  `WORLD_SIZE_IN_CHUNKS` (chunks/blocks/voxels CPU buffers empty;
  `dense_voxel_types = Vec::new()` preserves the existing `if meta.
  dense_voxel_types.is_empty() { return; }` gate at `naadf_gpu_producer_node`).
  Camera spawn + load-failure fallback to `install_default_embedded_in_fixed_world`
  preserved.

### Verification results

- `cargo build --workspace` — **clean** (0 errors, 0 new warnings); finished
  in 57.71s (`dev` profile, optimized + debuginfo).
- `cargo test --workspace --lib` — **198 passed, 1 ignored** across 3 suites
  in 4.37s. Matches the baseline reported in
  `01-context.md:302` exactly. No new failures, no test-count drift.

### Design adherence

Followed the W5.1 spec in `02-design.md` lines 85–346 verbatim:

- **Derive delta** (design §W5.1 lines 99-116): `Resource` added; existing
  `Clone, Debug` preserved. `use bevy::prelude::Resource;` import added at
  the top of `aadf/generator.rs`. Project convention (per
  `render/construction/config.rs:27`) is `bevy::prelude::Resource` rather
  than `bevy::ecs::resource::Resource`; I used the former for consistency.
  (Brief language allowed either; design used `bevy::prelude::Resource`.)
- **`ModelDataRender` resource** (design lines 118-148): inserted in
  `render/extract.rs` immediately after `WorldDataMeta` with the exact
  docstring + field set the design specifies.
- **`stage_model_data_buildonce` system** (design lines 150-184): inserted
  after `stage_world_gpu_buildonce` with the exact body the design
  specifies. Gated on `existing.is_some()` short-circuit then `model_data`
  binding.
- **Registration** (design lines 187-220): registered both the
  `init_resource` and the ExtractSchedule system slot exactly where the
  design said. Use-block was extended to import `stage_model_data_buildonce`
  + `ModelDataRender` alongside the existing imports.
- **`install_vox_in_fixed_world` rewrite** (design lines 223-336): copied
  the design's Rust body. Two small intentional changes from the literal
  design source:
  1. Wrapped the `WORLD_SIZE_IN_VOXELS.x/y/z` literals across lines
     identically to the design but rendered as a `let world_voxels = [
     WORLD_SIZE_IN_VOXELS.x, …, …];` block to satisfy `rustfmt`'s
     line-width preference — semantically identical.
  2. Reformatted the long `info!` argument list across more lines, again
     for `rustfmt` agreement — semantically identical.

  No semantic deviations.

### Assumption-verification findings (per `02-design.md` §Assumptions made)

- **Assumption 1** ("`ModelData` derives only `Clone + Debug` today"):
  **verified true**. Pre-edit derive at `aadf/generator.rs:72` was
  `#[derive(Clone, Debug)]`. W5.1 added `Resource`.
- **Assumption 2** ("`bevy::render::renderer::RenderQueue` is the correct
  import name"): not exercised by W5.1 (RenderQueue access is W5.3 scope).
  Noted for the next dispatch.
- **Assumption 7** ("`generator_model.wgsl` is FIXED"): respected — not
  touched in this dispatch.
- The other assumptions (3-6, 8-11) are W5.2+ / W5.5 scope and not
  exercised by W5.1.

### Surprises

None at the load-bearing level. One minor note:

- The orchestrator brief's text said "Build a single-tile `ImportedVox` →
  `build_world_from_vox(imp)` → produces `(WorldData, VoxelTypes)`," which
  conflicts with the design's actual W5.1 spec (which constructs the empty
  fixed-size `WorldData` directly, *without* calling `build_world_from_vox`,
  because `build_world_from_vox` would size the WorldData to the model's
  chunks rather than to `WORLD_SIZE_IN_CHUNKS`). I followed the design's
  spec (authoritative per the brief's "Follow the design's W5.1 section
  spec exactly" clause). `build_world_from_vox` is therefore unused by the
  new `install_vox_in_fixed_world` body; the design correctly notes the
  function is "KEPT" because it's still used by the non-fixed-world
  `install_vox_sized_to_model` path.
- The W5.4 deletion candidates (`tile_buckets_into_world` at
  `vox_import.rs:287`, `parse_dot_vox_data_into_world` at `:259`,
  `load_vox_into_world` at `:193`) are confirmed to still exist after W5.1
  (verified by grep). They are no longer called from
  `install_vox_in_fixed_world` after this dispatch, but other call sites
  (`parse_dot_vox_data_into_world` is called by `load_vox_into_world`,
  which currently has no caller after this edit but is a `pub fn`) keep
  them alive at the type-check level until W5.4 deletes them.

### What's NOT yet working

**The `.vox` → fixed-world path will not render correctly until W5.2 +
W5.3 land.** This is the **expected intermediate state**. W5.1's empty
`WorldData` + populated `ModelData` resource is the input to the
yet-to-be-built GPU producer chain (W5.2 builds the storage buffers + bind
group; W5.3 wires the per-segment dispatch loop). Until both land, the
W5 `.vox` fixed-world boot will show empty fixed-world geometry (sky-only
or whatever the empty `WorldGpu::chunks` decodes to). The existing
`install_vox_sized_to_model` path (used by `--vox-e2e`, `--oasis-edit-visual`,
`--small-edit-repro` gates) is untouched and continues to use the legacy
`build_world_from_vox` flow.
