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

---

## impl W5.2 findings (2026-05-17)

### Files touched

- `crates/bevy_naadf/src/render/construction/mod.rs:192-217` — added 4 new
  `Option<Buffer>` fields to `ConstructionGpu`
  (`model_data_chunk_buffer`, `model_data_block_buffer`,
  `model_data_voxel_buffer`, `model_data_params_buffer`). All inherit the
  `#[derive(Default)]` initialiser → `None` on construction.
- `crates/bevy_naadf/src/render/construction/mod.rs:246-258` — added one
  new `Option<BindGroup>` field `construction_generator_model` to
  `ConstructionBindGroups`. Inherits the `#[derive(Default)]` → `None`.
- `crates/bevy_naadf/src/render/construction/mod.rs:867-872` — added the
  `model_data: Option<Res<crate::render::extract::ModelDataRender>>`
  parameter at the END of `prepare_construction`'s signature
  (parallel-to-`world_data_meta` per design §W5.2).
- `crates/bevy_naadf/src/render/construction/mod.rs:1240-1369` — inserted
  the W5 prepare block AFTER the `bound_dispatch` bind-group block and
  BEFORE the "First-frame seed" comment for `add_initial_groups_to_bound_queue`.
  The block is `if let Some(model_data) = model_data.as_deref()`-gated, with
  every sub-step gated on its own `is_none()` check (build-once seam pattern).

No other files touched.

### Verification results

- `cargo build --workspace` — **clean** (0 errors, 0 new warnings); finished
  in 29.40s (`dev` profile, optimized + debuginfo).
- `cargo test --workspace --lib` — **198 passed, 1 ignored** across 3 suites
  in 4.68s. Matches baseline exactly. No new failures, no test-count drift.
- Quick grep — `dispatch_generator_model_with_encoder` is NOT defined
  anywhere in `generator_model.rs` (W5.3 cascade NOT landed).
  `git status` confirms `generator_model.rs` and `generator_model.wgsl` are
  untouched.
- Quick grep — `tile_buckets_into_world` (`vox_import.rs:287`),
  `parse_dot_vox_data_into_world` (`:259`), and `load_vox_into_world`
  (`:193`) all still exist (W5.4 cascade NOT landed).

### Design adherence

Followed the W5.2 spec in `02-design.md` lines 347-574 verbatim. Three
small intentional adjustments:

1. **`segment_voxel_buffer` size constant.** The design pseudocode in the
   prepare block (lines 460-471) uses the WRONG sizing (`world_chunk_count
   * 2048 * 4` = full-world cubic), then the REVISED note further down
   (lines 1533-1548) overrides to per-segment cubic. I followed the
   REVISED note (binding) and computed the size as:
   ```
   const SEGMENT_CHUNKS: u64 = (crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS as u64) * 4; // = 16
   size = SEGMENT_CHUNKS * SEGMENT_CHUNKS * SEGMENT_CHUNKS
        * (generator_model::CHUNK_DATA_U32S as u64) * 4;
   ```
   No hard-coded `16`; derived from the constants in `lib.rs:224` +
   `generator_model.rs:66`.
2. **Zeroed `GpuGeneratorModelParams` initialisation.** Design lines
   509-521 manually zero each field; I used the simpler
   `bytemuck::Zeroable::zeroed()` cast (the struct derives `Zeroable` per
   `generator_model.rs:75`). Semantically identical.
3. **Bind-group entry layout-lookup site.** Design uses
   `pipeline_cache.get_bind_group_layout(&construction_pipelines.generator_model_layout)`
   to retrieve the layout — same pattern the W3 / W1 / W2 bind groups in
   this file use. Verified by reading the surrounding bind-group construction
   sites (`mod.rs:1192-1208` etc.).

No semantic deviations from the W5.2 spec.

### `segment_voxel_buffer` sizing confirmation

**Allocated size:** `16 × 16 × 16 × 2048 × 4 bytes = 4096 chunks × 8192 B/chunk
= 33,554,432 bytes = 32 MiB`.

**Formula used:**
```
SEGMENT_CHUNKS = WORLD_GEN_SEGMENT_SIZE_IN_GROUPS (4) × 4 (chunks/group) = 16
size = SEGMENT_CHUNKS³ × CHUNK_DATA_U32S × 4
     = 16³ × 2048 × 4
     = 4096 × 2048 × 4
     = 33,554,432 bytes
     = 32 MiB
```

**Sanity vs design:** the design's REVISED note (line 1535) cites
"16³ chunks × 2048 u32 × 4 B = 128 MiB". That arithmetic is off by 4×:
`16³ × 2048 × 4 = 33,554,432 B = 32 MiB`, not 128 MiB. The formula in
my code matches the design's STATED formula exactly (per-segment cubic;
`SEGMENT_CHUNKS^3 * CHUNK_DATA_U32S * 4`); only the design's
human-readable "= 128 MiB" annotation is arithmetically incorrect. The
actual allocation is 32 MiB, well inside the 256 MiB wgpu Vulkan-baseline
`max_buffer_size` (and well inside the 134 GiB full-world cubic that
the REVISED note correctly rejects). **Not a deviation; the binding
constraint (per-segment cubic, NOT full-world cubic) is satisfied.**

**Decisively NOT full-world cubic** (which would be
`WORLD_SIZE_IN_CHUNKS.x * y * z * 2048 * 4 = 256 * 32 * 256 * 2048 * 4
≈ 17.2 GiB`, well past every realistic wgpu cap).

### Bind-group entry order confirmation

Order used in `BindGroupEntries::sequential` (`mod.rs:1352-1360`):

| Position | Binding | Buffer |
|---|---|---|
| 0 | binding 0 = chunk_data_rw | `segv` (`gpu.segment_voxel_buffer`) |
| 1 | binding 1 = model_data_chunk_ro | `mdc` (`gpu.model_data_chunk_buffer`) |
| 2 | binding 2 = model_data_block_ro | `mdb` (`gpu.model_data_block_buffer`) |
| 3 | binding 3 = model_data_voxel_ro | `mdv` (`gpu.model_data_voxel_buffer`) |
| 4 | binding 4 = params | `params` (`gpu.model_data_params_buffer`) |

Matches the design's W5.2 bind-group entry ordering table (design lines
564-569) and `generator_model::generator_model_layout_descriptor`
(`generator_model.rs:131-147`) byte-for-byte.

### Assumption-verification findings

- **Assumption 5** ("`segment_voxel_buffer` is allocated at the per-segment
  cubic extent ... NOT the full-world cubic extent"): **followed.** Size
  formula matches the assumption exactly.
- **Assumption 10** ("The existing W1 path's `want_gpu_producer` gate at
  `mod.rs:888-890` will NOT allocate `segment_voxel_buffer` for the W5
  path"): **verified true by Read.** Lines 886-890 compute:
  ```
  let dense_data_ready = world_data_meta
      .as_deref()
      .is_some_and(|w| !w.dense_voxel_types.is_empty());
  let want_gpu_producer =
      construction_config.gpu_construction_enabled && dense_data_ready;
  ```
  Since the W5.1 install path inserts an empty `WorldData` with
  `dense_voxel_types = Vec::new()`, `dense_data_ready = false` →
  `want_gpu_producer = false` → the block at `:891-1015` (which contains
  the `segment_voxel_buffer` allocation at `:988-1015`) is skipped. The
  W5.2 prepare block MUST allocate `segment_voxel_buffer` itself — exactly
  as the design specifies.
- **Assumption 2** ("`bevy::render::renderer::RenderQueue` is the correct
  import name"): not exercised by W5.2 directly (only `create_storage_buffer_u32`
  + `create_params_uniform` consume `&RenderQueue`, both via the existing
  `render_queue` already in `prepare_construction`'s signature). Will be
  re-verified by W5.3.
- **Assumption 7** ("`generator_model.wgsl` is FIXED"): respected — `git
  status` confirms the file is untouched.

### Surprises

One — the W2-placeholder allocation of `segment_voxel_buffer` at
`mod.rs:1486` (the OLD pre-W5 placeholder, 4-byte size) would clobber
the W5 allocation if the W5 block ran AFTER the W2 placeholder. Verified
the W5 block runs FIRST (insertion site `:1240-1369` is BEFORE the W2
block at `:1486`), so when the W2 placeholder reaches its
`if gpu.segment_voxel_buffer.is_none()` check, the W5 allocation has
already populated `gpu.segment_voxel_buffer = Some(_)` and the W2
placeholder is skipped. **No race; the ordering happens to be correct.**

(Long-term, the W2 placeholder allocation block should be deleted once the
W5 chain is the only producer — but that's W5.4+ scope, not W5.2.)

### What's NOT yet working

**The `.vox` → fixed-world path still renders empty (sky-only) until W5.3
lands.** This is the expected intermediate state. After W5.2:

- The 4 W5 buffers (3 storage + 1 uniform) are allocated and populated.
- The `construction_generator_model` bind group is built and ready.
- `gpu.segment_voxel_buffer` is allocated at per-segment cubic extent
  (32 MiB) and ready to receive the per-segment generator dispatches.

What is STILL missing (W5.3 scope):

- The `dispatch_generator_model_with_encoder` sibling helper in
  `generator_model.rs`.
- The W5 branch + segment loop in `naadf_gpu_producer_node` that:
  - Iterates 16 × 2 × 16 = 512 segments in Z/Y/X order (per C# loop order
    in `WorldData.cs:136-140`).
  - Writes the per-segment `GpuGeneratorModelParams` into the params buffer.
  - Dispatches `generator_model.wgsl` per segment.
  - Dispatches `chunk_calc::dispatch_calc_block_from_raw_data_world_sized`
    per segment.
  - Runs the bounds chain ONCE after the loop.
  - Flips `gpu.gpu_producer_has_run = true`.

Until W5.3 lands, `gpu_producer_has_run` never flips on the W5 path,
`WorldGpu::chunks` stays zeroed, and the renderer decodes every chunk as
Empty → sky-only framebuffer for the `.vox` fixed-world load path. The
existing `--vox-e2e`, `--oasis-edit-visual`, `--small-edit-repro` gates
that use the non-fixed-world `install_vox_sized_to_model` path are
unaffected by W5.2.
