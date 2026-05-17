# vox-gpu-rewrite — reuse audit

Audit scope: port `bevy-naadf`'s `.vox` → fixed-world load path from the CPU
XZ-tiling stop-gap (`vox_import::tile_buckets_into_world`) to a GPU dispatch
chain that mirrors C# `WorldData.cs:120-156`'s per-segment `generator_model +
chunk_calc` invocations.

Every cited candidate verified against the file/line at audit time.
Fit verdicts:

- `verbatim` — callable as-is from the new code.
- `extend` — needs new fields/branches but core is reused.
- `model-after` — copy the candidate's shape (no direct call).
- `none` — fully greenfield.

---

## Per-subtask candidate table

### W5.1 — `ModelData` main-world `Resource` + extract system

| Need | Existing candidate (file:line) | Fit | Notes |
|---|---|---|---|
| `ModelData` host struct with `chunks/blocks/voxels: Vec<u32>` + `size_in_chunks: [u32; 3]` | `crates/bevy_naadf/src/aadf/generator.rs:73-83` (`pub struct ModelData { data_chunk, data_block, data_voxel, size_in_chunks }`) | **verbatim (struct) / extend (to add `#[derive(Resource)]` + `Clone`)** | Field set is exactly what the brief calls for — names differ (`data_chunk` vs `chunks`). Already used by the W5 unit test + `aadf::generator::generate_segment_cpu`. Re-using it (vs creating a sibling type) keeps one canonical layout. Currently `#[derive(Clone, Debug)]` only — adding `Resource` is one line. |
| Helper constructors for the new resource (`empty`, `uniform_full`) | `crates/bevy_naadf/src/aadf/generator.rs:85-110` | verbatim | Already used by tests; useful for the `--vox-gpu-construction` gate's deterministic fixture (W5.5). |
| Build-once extract pattern (main-world `Resource` → render-world long-lived mirror, no per-frame clone) | `crates/bevy_naadf/src/render/extract.rs:167-203` (`stage_world_gpu_buildonce`) + `WorldDataMeta` at `:107-119` | **model-after** | `stage_world_gpu_buildonce` is the canonical "Extract + gate on `WorldGpu` absence + populate metadata resource" pattern; the new `ModelData` extract should mirror it 1:1 (gate on a `ModelDataRender::is_none()` to stay build-once). `WorldDataMeta` is the long-lived companion `WorldGpuStaging` outlives — same shape the W5 brief needs. Already-registered with `init_resource::<WorldDataMeta>()` at `render/mod.rs:122`. |
| Render-graph node reading the extracted metadata via `Option<Res<...>>` (gives the exact `Option<Res<WorldDataMeta>>` parameter shape) | `crates/bevy_naadf/src/render/construction/mod.rs:1914-1923` (`naadf_gpu_producer_node` signature) | model-after | Same Option-wrapping discipline + early-return-on-missing pattern. |
| Caller site to install the new `ModelData` from VOX | `crates/bevy_naadf/src/voxel/grid.rs:306-343` (`install_vox_in_fixed_world`) | **extend** | This is the exact branch the brief flips. Today it calls `vox_import::load_vox_into_world` (the CPU tile stop-gap to be deleted in W5.4) → `build_world_from_vox`. Needs to be rewritten to: parse the `.vox` into a single-tile `ImportedVox`, convert that single-tile `ConstructedWorld` into a `ModelData` (chunks/blocks/voxels are already byte-identical to the `ModelData` encoding per `aadf::generator.rs:64-71`), insert it as a `Resource`, AND insert an EMPTY `WorldData` (so the renderer's bind groups still allocate). |

### W5.2 — Upload `ModelData` buffers + build the W5 bind group in `prepare_construction`

| Need | Existing candidate (file:line) | Fit | Notes |
|---|---|---|---|
| Bind-group layout descriptor for the W5 pipeline (5 bindings: chunk_data_rw, 3 × model_data_ro, params_uniform) | `crates/bevy_naadf/src/render/construction/generator_model.rs:131-147` (`generator_model_layout_descriptor`) | **verbatim** | Treat as fixed (per brief). Already stored on `ConstructionPipelines::generator_model_layout` (`construction/mod.rs:252-254`) — the new bind group just calls `pipeline_cache.get_bind_group_layout(&construction_pipelines.generator_model_layout)`. |
| Cached compute pipeline ID for `generator_model.wgsl` | `crates/bevy_naadf/src/render/construction/generator_model.rs:151-158` (`queue_generator_model_pipeline`) | **verbatim** | Already queued in `ConstructionPipelines::from_world` at `construction/mod.rs:337-344`; the new node just fetches via `pipeline_cache.get_compute_pipeline(construction_pipelines.generator_model_pipeline)`. |
| Storage-buffer creation helper | `crates/bevy_naadf/src/render/construction/generator_model.rs:182-199` (`create_storage_buffer_u32`) | **verbatim** | Already used by the W5 unit test. Allocates with `STORAGE \| COPY_DST \| COPY_SRC`, zero-pads empty data — matches the brief's "STORAGE \| COPY_DST" requirement. |
| Uniform-buffer creation helper for `GpuGeneratorModelParams` | `crates/bevy_naadf/src/render/construction/generator_model.rs:202-215` (`create_params_uniform`) | **verbatim** | Builds a 64-B `UNIFORM \| COPY_DST` buffer with the params written. Allocate once, then re-write each segment via `RenderQueue::write_buffer` (cheap; brief's W5.3 requires per-segment uniform updates). |
| `GpuGeneratorModelParams` Pod struct + compile-time layout guards | `crates/bevy_naadf/src/render/construction/generator_model.rs:74-119` | **verbatim** | Already pinned to 64 B with `_pad*` + `const _: () = assert!(...)` row offsets. Caller just fills fields. |
| `Option<Buffer>` field pattern on `ConstructionGpu` for new buffers | `crates/bevy_naadf/src/render/construction/mod.rs:106-190` (the whole `ConstructionGpu` struct, every field is `Option<Buffer>` initialised to `None`) | **extend** | The seam contract (`construction/mod.rs:84-103`) explicitly says "Every field is `Option<Buffer>` initialised to `None`" so each workstream owns its family. Add 3 new fields (`model_data_chunk_buffer`, `model_data_block_buffer`, `model_data_voxel_buffer`) + maybe `model_data_params_buffer` + a `BindGroup` field on `ConstructionBindGroups`. **No existing field already does this for ModelData** — there is no abandoned attempt to revive. |
| Precedent allocate-and-build-bind-group block inside `prepare_construction` | `crates/bevy_naadf/src/render/construction/mod.rs:1471-1549` (the `construction_world` bind-group construction block) and `:1166-1215` (the `construction_bounds_world` / `construction_bounds` / `bound_dispatch` blocks) | **model-after** | Same pattern: "allocate placeholder buffers if missing; build bind group if missing; gate on every dependency `is_some()`". The W3 `bind_groups.construction_bounds_world` block at `:1167-1180` is the cleanest minimal precedent (2 bindings: chunks + params) — model the new W5 block after it (5 bindings: chunk_data + 3 model_data + params). |
| `BindGroupEntries::sequential` helper | `crates/bevy_naadf/src/render/construction/mod.rs:1174-1177` (used by every `construction_*` bind group) | verbatim | Standard Bevy helper, no work needed. |

### W5.3 — Extend `naadf_gpu_producer_node` with segment-loop branch

| Need | Existing candidate (file:line) | Fit | Notes |
|---|---|---|---|
| Render-graph node host to extend | `crates/bevy_naadf/src/render/construction/mod.rs:1913-2001` (`naadf_gpu_producer_node`) | **extend** | Brief explicitly says "add a new branch when `ModelData` is present". The current body already: gates on `gpu_construction_enabled`, fetches `Option<Res<ConstructionPipelines>>` + `Option<Res<ConstructionBindGroups>>` + `Option<ResMut<ConstructionGpu>>`, uses `render_context.command_encoder()` so wgpu auto-inserts the STORAGE→STORAGE barrier between producer writes + downstream reads. The new branch adds the W5 dispatch BEFORE the existing chunk_calc dispatches. |
| Per-segment dispatch for `generator_model.wgsl` (one regime-1 dispatch over `(group_size_in_chunks.{x,y,z})` workgroups) | `crates/bevy_naadf/src/render/construction/generator_model.rs:229-254` (`dispatch_generator_model`) | **verbatim** | **Drift note:** This helper builds its OWN `command_encoder` + `queue.submit()` (designed for the W5 unit test). For the production node body the writes must land on `render_context.command_encoder()` (per the existing `naadf_gpu_producer_node` discipline at `mod.rs:1969`) so wgpu inserts the storage barrier across passes. Either: extend `dispatch_generator_model` with a sibling variant that takes `&mut CommandEncoder` (matching `chunk_calc::dispatch_calc_block_from_raw_data` shape at `chunk_calc.rs:170-187`), or inline the 14-line dispatch directly in the node. See **Borderline calls** below. |
| Per-segment `chunk_calc.calc_block_from_raw_data` dispatch (already world-sized variant) | `crates/bevy_naadf/src/render/construction/chunk_calc.rs:198-215` (`dispatch_calc_block_from_raw_data_world_sized`) | **verbatim** | Already takes `&mut CommandEncoder`. The brief says "calls the existing `chunk_calc::dispatch_calc_block_from_raw_data_world_sized` for that segment's chunk extent" — the function exists and is called from exactly this node today (`mod.rs:1973-1978`). For the segment loop, pass the per-segment chunk extent (= `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 = 16` chunks per axis, per `lib.rs:218-224`). |
| Compute-voxel-bounds + compute-block-bounds dispatches (bounds chain after segment loop) | `crates/bevy_naadf/src/render/construction/chunk_calc.rs:219-255` (`dispatch_compute_voxel_bounds`, `dispatch_compute_block_bounds`) | **verbatim** | Called once after the segment loop (brief: "Then runs the bounds chain once"). Already done that way at `mod.rs:1980-1992`. |
| Per-segment uniform rewrite (`Queue::write_buffer` into the existing `GpuGeneratorModelParams` buffer) | `crates/bevy_naadf/src/render/construction/mod.rs:1404-1435` (the existing per-frame uniform rewrite of `GpuConstructionParams`) | model-after | Same pattern: fetch the uniform buffer from `ConstructionGpu`, `render_queue.write_buffer(buf, 0, bytemuck::bytes_of(&params))`. **Limitation**: `RenderContext` exposes `command_encoder()` but not `RenderQueue` directly — the cleanest path is to either re-allocate per-segment params via `create_params_uniform` and queue inside `prepare_construction` (build all 512 buffers, then dispatch in the node), or use the `RenderQueue` resource inside the node (signature change). See `prepare_construction`'s existing `RenderQueue` `Res` parameter at `mod.rs:839`. |
| `WORLD_SIZE_IN_SEGMENTS = UVec3::new(16, 2, 16)` constant + the chunks/voxels constants it derives | `crates/bevy_naadf/src/lib.rs:218`, `:224`, `:234`, `:237` | **verbatim** | All four constants exist (`WORLD_SIZE_IN_SEGMENTS`, `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS = 4`, `WORLD_SIZE_IN_CHUNKS = (256, 32, 256)`, `WORLD_SIZE_IN_VOXELS = (4096, 512, 4096)`). The drift-guard test `fixed_world_size_constants_agree` at `lib.rs:905-919` pins their relationship. Iterate `0..16, 0..2, 0..16` segments; per segment offset = `seg_idx * WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4` chunks. |
| Existing precedent for a per-segment dispatch loop inside a render-graph node | none found | **none** | No existing render-graph node iterates `WORLD_SIZE_IN_SEGMENTS`. The current `naadf_gpu_producer_node` dispatches `chunk_calc` ONCE over the full world (`mod.rs:1973-1978`); the W2 / W3 / W4 nodes do single dispatches per pipeline. The brief's "iterate WORLD_SIZE_IN_SEGMENTS segments and per segment: rewrite uniform + dispatch generator + dispatch chunk_calc" is genuinely new — but it is a straightforward `for` loop wrapping calls to existing dispatch helpers. |

### W5.4 — Delete the CPU tile stop-gap

| Need | Existing candidate (file:line) | Fit | Notes |
|---|---|---|---|
| `vox_import::tile_buckets_into_world` (the function the brief deletes) | `crates/bevy_naadf/src/voxel/vox_import.rs:287-325` | **n/a — target of deletion** | Only caller is `parse_dot_vox_data_into_world` (line 269); the `TODO (Phase-C W5)` comment at `:279-286` explicitly flags this as the stop-gap. |
| `vox_import::parse_dot_vox_data_into_world` (the function the brief deletes) | `crates/bevy_naadf/src/voxel/vox_import.rs:259-273` | **n/a — target of deletion** | Only caller is `load_vox_into_world` (line 199). |
| `vox_import::load_vox_into_world` (the function the brief deletes) | `crates/bevy_naadf/src/voxel/vox_import.rs:193-200` | **n/a — target of deletion** | Only caller is `voxel/grid.rs:308` (`install_vox_in_fixed_world`) — gone after the W5.1 rewrite. |
| Tests `into_world_tiles_xz_and_leaves_y_above_tile_empty` + `into_world_with_target_smaller_than_tile_clips` | `crates/bevy_naadf/src/voxel/vox_import.rs:1831-1889` | **n/a — target of deletion** | Both call `parse_dot_vox_data_into_world` (deleted). The other ~30 tests in this `mod tests` block exercise `parse_dot_vox_data` / `parse_vox_bytes` / `parse_dot_vox_data_tiled` — all keepers per brief. |
| Sibling helper that must NOT be touched: `replicate_buckets_xz` (used by `--vox-grid N` / `install_vox_sized_to_model`) | `crates/bevy_naadf/src/voxel/vox_import.rs:335-376` | **keep** | Reached via `parse_dot_vox_data_tiled` → `load_vox_tiled` → `grid.rs:255 install_vox_sized_to_model` (the legacy non-fixed-world path that e2e gates `--vox-e2e`, `--oasis-edit-visual`, `--small-edit-repro` use). Brief targets only the **fixed-world** path. |
| Comment / docstring sites that reference the stop-gap | `crates/bevy_naadf/src/render/construction/mod.rs:2017-2020` ("`generator_model` per segment — currently bypassed for the bevy-naadf test scene"), `vox_import.rs:46-56` (Δ-GPUProducer comment block), `vox_import.rs:382-385` (`build_world_from_vox` Δ-GPUProducer comment) | **extend** | These notes describe the pre-W5 state and need to be updated to "W5 landed; ModelData drives the GPU producer for `.vox` loads. Default scene retained CPU upload by deliberate divergence." |

### W5.5 — `--vox-gpu-construction` e2e gate + new e2e module

| Need | Existing candidate (file:line) | Fit | Notes |
|---|---|---|---|
| CLI-flag plumbing in `bin/e2e_render.rs` | `crates/bevy_naadf/src/bin/e2e_render.rs:81-89` (existing `--vox-e2e`, `--oasis-edit-visual`, `--small-edit-visual`, `--small-edit-repro`, `--edit-mode`, `--validate-gpu-construction`, `--entities`, `--runtime-edit-mode`) | **extend** | Add `let vox_gpu_construction_mode = args.iter().any(|a| a == "--vox-gpu-construction");` (~line 89) + a dispatch branch (~line 210-227 — same shape as the `--vox-e2e` branch dispatching to `bevy_naadf::e2e::vox_e2e::run_vox_e2e()`). |
| Closest existing e2e module pattern to copy (run-fn + assert-fn + entry point) | `crates/bevy_naadf/src/e2e/vox_e2e.rs` (entire file, 643 lines) | **model-after** | This module is the canonical "synthesise a tiny `.vox` fixture in memory → write to deterministic temp path → boot harness via `run_e2e_render_with_args` with `GridPreset::Vox` → assert a region-luminance gate" pattern. New module: copy the structure, set `args.fixed_world_size = true` (so the new GPU producer path runs), and add an assert that the framebuffer is non-empty after the GPU dispatch (similar to `assert_vox_geometry_visible` at `vox_e2e.rs:394-425`). |
| In-memory `.vox` fixture builder (multi-model with non-trivial nTRN) | `crates/bevy_naadf/src/e2e/vox_e2e.rs:134-270` (`build_vox_e2e_fixture` + helpers `dict_empty`, `dict_with`) | **verbatim or model-after** | The 60×60×4 slab + 20×20×28 tower fixture already exists, sized to the legacy small-world AABB. For W5.5 either reuse it verbatim OR build a smaller fixture sized at exactly one segment (= `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 * 16 = 256` voxels per axis) so the GPU dispatch tile pattern is exercised. |
| Write-fixture-to-temp + screenshot-to-disk helpers | `crates/bevy_naadf/src/e2e/vox_e2e.rs:289-313` (`write_vox_e2e_fixture_to_temp`, `vox_e2e_fixture_path`) + `:430-442` (`save_vox_e2e_screenshot`) | **model-after** | Same shape; new module just changes the filename constants. |
| Framebuffer region/luminance helpers for the assert | `crates/bevy_naadf/src/e2e/framebuffer.rs:23` (`Rect`), `:137` (`Framebuffer`), `:218` (`region_mean`), `:242` (`luminance`), `:282` (`from_fractional`), `:370` (`check_not_degenerate`), `:282` (mean_pixel_delta) | **verbatim** | Generic helpers used by every existing e2e gate. |
| Driver / readback / checks plumbing (booting the harness, screenshot capture, `PipelineCache` scan, node-dispatch verification) | `crates/bevy_naadf/src/e2e/driver.rs:406` (`e2e_driver`), `:60` (`E2ePhase`), `:206` (`E2eState`); `crates/bevy_naadf/src/e2e/readback.rs:34` (`shoot_primary_window`); `crates/bevy_naadf/src/e2e/checks.rs:58` (`scan_pipeline_errors_render_system`), `:132` (`pipeline_scan_result`), `:158` (`assert_nodes_dispatched`) | **verbatim** | All entry-point glue is reused by every gate via `run_e2e_render_with_args` (`lib.rs:780-806`). New module just builds `AppArgs` + calls `run_e2e_render_with_args`. |
| `AppArgs` mode flag for the new gate (parallels existing `vox_e2e_mode` / `oasis_edit_visual_mode`) | `crates/bevy_naadf/src/lib.rs:326-355` (existing five `*_mode: bool` flags + their `Default` initialisers at `:387-390`) | **extend** | Add `pub vox_gpu_construction_mode: bool,` + `false` default. Optional — the new gate may not need a driver mode flag if it just boots `GridPreset::Vox + fixed_world_size = true` and asserts non-empty framebuffer (no driver-flow customisation). |
| Entry point that mirrors `run_vox_e2e()` | `crates/bevy_naadf/src/e2e/vox_e2e.rs:325-361` (`run_vox_e2e`) | **model-after** | Same shape: write fixture, build `AppArgs` with `GridPreset::Vox { path, tiles: 1 }` + `fixed_world_size = true` + `gpu_construction_enabled = true`, call `run_e2e_render_with_args`. |
| Headless GPU oracle precedent for byte-equality (validate the GPU buffer post-dispatch matches `generate_segment_cpu`) | `crates/bevy_naadf/src/render/construction/mod.rs:3206-3377` (`generator_model_gpu_vs_cpu_bit_exact` test) + `:2351-2735` (`validate_gpu_construction`) | **verbatim (helpers) / model-after (driver)** | The W5 unit test already byte-compares GPU `generator_model.wgsl` output against `generate_segment_cpu` — but on a 2×1×2 model, not the full segmented world. If the W5.5 brief wants an in-window oracle gate (not just framebuffer assertion), this test's helpers (`render_fixture`, `readback_u32`) are directly reusable in the new e2e module. |

### W5.6 — Keep CPU default-scene compose path

| Need | Existing candidate (file:line) | Fit | Notes |
|---|---|---|---|
| The CPU default-scene compose function (brief: KEEP, document divergence) | `crates/bevy_naadf/src/voxel/grid.rs:390` (`compose_default_scene_into_fixed_world`) — also called from tests at `:820` + `:902` | **verbatim** | No changes. The brief explicitly retains this path because synthesising a primitive scene as a `ModelData` would force 16×16 GPU tiling. |
| `install_default_embedded_in_fixed_world` caller | `crates/bevy_naadf/src/voxel/grid.rs:156-249` | **verbatim** | Already inserts a fully-built `WorldData` with `dense_voxel_types = Vec::new()` (line 243); the existing `naadf_gpu_producer_node`'s gate at `mod.rs:1936-1941` (`if meta.dense_voxel_types.is_empty() { return; }`) already short-circuits the GPU producer — meaning the existing CPU-upload path takes over. **Divergence is already implemented and tested**; W5.6 is documentation-only (`docs/orchestrate/naadf-bevy-port/12-alignment-gap.md` is the canonical place per `CLAUDE.md`'s faithful-port rule). |
| Existing divergence-marker pattern (data-driven gate on a present-but-empty buffer) | `crates/bevy_naadf/src/render/construction/mod.rs:1936-1941` (`if meta.dense_voxel_types.is_empty() { return; }`), `crates/bevy_naadf/src/voxel/vox_import.rs:46-56` (Δ-GPUProducer note) | **model-after** | The new gate for "ModelData present → run GPU producer, else fall back to CPU upload" follows the same shape: `if model_data.is_none() { ... }` early-return in the W5 branch of `naadf_gpu_producer_node`. The default-scene path inserts no `ModelData` resource → falls into the CPU `dense_voxel_types`-driven branch the node already has. |

---

## Drift findings

1. **`dispatch_generator_model` takes `(device, queue)` not `&mut CommandEncoder`.** Reads (`generator_model.rs:229-254`):
   ```
   pub fn dispatch_generator_model(device: &RenderDevice, queue: &RenderQueue,
       pipeline, bind_group, group_size_in_chunks: [u32; 3])
   ```
   It internally creates `device.create_command_encoder(...)` + `queue.submit([encoder.finish()])`. By contrast `chunk_calc::dispatch_calc_block_from_raw_data_world_sized` (`chunk_calc.rs:198-215`) takes `encoder: &mut CommandEncoder` and lets the caller manage encoder/submit lifetime. The W5 brief assumes both helpers compose into the same render-graph node body, but the helpers' shapes are NOT compatible as-is. The handoff says "treat `generator_model.rs` as a FIXED dependency — must not be edited"; that creates a real friction point. Resolution options: (a) call `dispatch_generator_model` 512 times = 512 separate encoders + submits (wgpu auto-inserts STORAGE barriers across submits via the same buffer alias, but submit-per-segment is heavier than encode-per-segment + one submit); (b) inline the 14-line dispatch in the node directly (skip the helper); (c) loosen the "fixed dependency" rule and add a sibling `dispatch_generator_model_with_encoder(&mut CommandEncoder, ...)`. Option (b) is the lowest-friction path that obeys the brief.

2. **W5 brief says `ConstructionPipelines` is currently the empty-default `FromWorld` shell.** That is stale. As of W5 (already landed — `16-impl-c-W5.md`), `ConstructionPipelines` is a 22-field struct with a real `FromWorld` impl that queues every Phase-C pipeline including the W5 `generator_model_pipeline` + `generator_model_layout` (`construction/mod.rs:251-330` + `:332-499`). W5.2's "build the W5 bind group using the existing layout" is correct — but the layout is already stored on `ConstructionPipelines::generator_model_layout`, not built fresh.

3. **The `ConstructionConfig.run_worldgen_only` flag exists but is unused.** Defined at `construction/config.rs:99,160,221` with no callers. The W5 brief alludes to it indirectly ("behind the `ConstructionConfig.run_worldgen_only` flag" in `generator_model.rs:14`'s docstring); the new W5 integration could choose to gate on it instead of (or in addition to) checking for `ModelData` presence. Worth a design decision in the next phase.

4. **`WORLD_SIZE_IN_SEGMENTS = UVec3::new(16, 2, 16)` is declared but has no runtime callers** beyond the `fixed_world_size_constants_agree` drift-guard test. The W5 integration will be its first production consumer. The const + the drift-guard test are exactly what the brief expects, no surprise.

5. **The renderer's "data-driven GPU-producer gate" is gated on `meta.dense_voxel_types.is_empty()` not on a `ModelData` resource** (`naadf_gpu_producer_node` at `construction/mod.rs:1936-1941`). The new W5 branch needs to add a second gate — present-`ModelData` → GPU model→chunk_calc chain; present-`dense_voxel_types`-only → existing chunk_calc-only chain; neither → CPU upload. This is the right shape; the brief implies it but does not spell out the three-way switch.

6. **No abandoned prior W5 integration attempt is in the tree.** Searched for `model_data_chunk_buffer`, `ModelDataRender`, `ModelDataExtracted`, `model_data_pipeline`, none exist. The W5 unit test is the only existing consumer of `dispatch_generator_model`. Greenfield integration is justified.

---

## Additional reuse not named in handoff

1. **`aadf::generator::ModelData` is the perfect `ModelData` resource type.** The handoff describes building a `Resource` with `chunks/blocks/voxels: Vec<u32>` + `size_in_chunks: [u32; 3]` — those fields already exist (named `data_chunk/data_block/data_voxel/size_in_chunks`) on the audited-faithful struct at `crates/bevy_naadf/src/aadf/generator.rs:73-83`. Adding `#[derive(Resource, Clone)]` is the entire delta. Re-using this type (rather than building a sibling) means W5.5's e2e gate can directly compare against `generate_segment_cpu(&model_data, ...)` for byte-equality (the existing W5 unit-test oracle).

2. **`aadf::generator::ModelData::empty` and `::uniform_full` constructors** (`generator.rs:88-110`) are ready-made fixture helpers for the W5.5 e2e gate's deterministic scene without inventing a new fixture builder.

3. **The `Cargo.toml` `bevy_naadf` crate already pulls in the W5 unit-test helpers via `cfg(test)`.** The new e2e module (`vox_gpu_construction.rs` or similar) can mirror the `oasis_edit_visual.rs` shape — it does not need a new test module.

4. **`ConstructionConfig.gpu_construction_enabled` is the default-ON gate** that already controls the production `naadf_gpu_producer_node`. The W5 branch just needs to require it AND require `ModelData::is_some()` — no new config knob needed.

5. **The render-graph node IS a Bevy `system`, not a `Node` impl.** `naadf_gpu_producer_node` is added to the `Core3d` chain as a system at `render/mod.rs:285` (via `.add_systems(Core3d, (...).chain().in_set(Core3dSystems::PostProcess).before(tonemapping))`). The extension is a system signature change (add `Option<Res<ModelDataRender>>` etc.), not a `Node`/`ViewNode` impl rewrite.

6. **The `build_segment_voxel_buffer_from_dense` function** (`construction/mod.rs:2150-2219`) is the CPU bridge currently used when `dense_voxel_types` is present but no `ModelData` is. It builds a flat segment-voxel-buffer for the W1 chunk_calc dispatch. After W5 lands, the GPU `generator_model.wgsl` writes the same buffer shape (`chunk_data_rw` at binding 0 is the W1 `segment_voxel_buffer` per `15-design-c.md` §4.5) — this function becomes dead code only for the `.vox` path, but is still alive for the default-scene CPU fallback.

7. **`stage_world_gpu_buildonce` already sets `WorldData::dense_voxel_types` to empty** for `.vox` loads (`vox_import.rs:413`) — meaning today's `.vox` fixed-world path falls through to the CPU upload (no GPU producer runs). After W5 lands the new `ModelData`-presence gate carries the load instead, and `dense_voxel_types` stays empty — no main-world data-flow change required.

---

## Borderline calls

1. **`dispatch_generator_model` — `extend` vs `none` for the segment-loop body.** Verbatim use is impossible (it owns its encoder + submit); the brief forbids editing `generator_model.rs`. Recommendation: **inline the 14 dispatch lines in `naadf_gpu_producer_node`'s new branch** (treat as `none` for the dispatch body, while still using `generator_model_layout_descriptor` + `queue_generator_model_pipeline` + `create_params_uniform` + `create_storage_buffer_u32` verbatim). This obeys the "fixed" rule and gets the dispatch onto the node's shared encoder so wgpu auto-barriers work. Flipping the rule to add a `_with_encoder` sibling helper would be cleaner long-term — propose this as a design-time question.

2. **Reuse-vs-fresh for the W5.5 `.vox` fixture.** The existing `build_vox_e2e_fixture` (`vox_e2e.rs:134-270`) is sized for a 64×32×64-voxel world and tuned to the legacy small-world camera. For W5.5 the fixed-world camera at C# `(500, 200, 40)` (per `grid.rs:323-326`) frames a different region — using the existing fixture risks the camera looking at empty space. Recommendation: **build a NEW fixture sized at exactly 1 segment (256 voxels per axis)** so the GPU dispatch's per-segment indexing math is exercised (one full segment populated, the other 511 empty). Flips to `model-after` (copy the helper shape, not the geometry).

3. **`AppArgs::vox_gpu_construction_mode` flag — `extend` vs `none`.** Adding a mode flag follows the existing `vox_e2e_mode` / `oasis_edit_visual_mode` pattern, but the new gate may not need driver-flow customisation (it can just boot the production path + assert framebuffer is non-empty + run the W5 unit test's oracle as an in-process post-step). Recommendation: **skip the mode flag** unless the driver needs to swap region rectangles or skip phases — start with `none` and add a flag only if the gate needs phase-level control.

4. **Whether to extend `WorldDataMeta` with `model_data: Option<ModelData>` vs add a separate `ModelDataRender` resource.** `WorldDataMeta` is "DELIBERATELY MINIMAL" per its docstring at `render/extract.rs:102-105` — but the W5 brief asks for a `ModelData` extracted to the render world, which is exactly what `WorldDataMeta` exists to do for `dense_voxel_types`. Recommendation: **add a separate `ModelDataRender` resource** (more honest: this is a substantial Vec-of-u32 payload, deserves its own resource + extract system + lifecycle gate). Flips to `model-after` cleanly. The MINIMAL clause is load-bearing — re-read it before deciding.

---

## Top reuse recommendation

The W5 integration is **~80% reuse**: every WGSL/shader/pipeline/bind-group-layout/dispatch-helper/extract-pattern piece already exists, and the host-side `ModelData` struct is sitting in `aadf/generator.rs:73-83` waiting for a `#[derive(Resource)]`. The genuinely greenfield work is:

1. The segment-loop body in `naadf_gpu_producer_node` (~30 lines wrapping calls to `dispatch_generator_model` + `dispatch_calc_block_from_raw_data_world_sized`).
2. The 3-buffer + 1-bind-group allocation block in `prepare_construction` (~50 lines, model-after the W3 `construction_bounds_world` block).
3. The `install_vox_in_fixed_world` rewrite (~15 lines: parse → convert ConstructedWorld→ModelData → insert resource).
4. The `--vox-gpu-construction` e2e module (~200 lines, model-after `vox_e2e.rs`).

Everything else is direct calls into existing audited code. The only non-trivial design call is the `dispatch_generator_model` encoder-shape mismatch (Drift #1), which is best resolved by inlining the 14 dispatch lines rather than editing the fixed helper.
