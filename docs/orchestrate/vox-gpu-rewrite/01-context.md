# vox-gpu-rewrite — context bundle (non-review agents)

This file is the canonical context any non-review agent (auditor was already
done, design agent next, impl agent after) reads first. Review agents read
[`04-review.md`](04-review.md) **and only that** — by design, fresh-eyes.

---

## Goal (verbatim from handoff)

> Port `bevy-naadf`'s `.vox` → fixed-world load path from a CPU XZ-tiling
> stop-gap to a GPU dispatch chain that mirrors C#
> `NAADF/World/Data/WorldData.cs:120-156`'s per-segment `generator_model` +
> `chunk_calc` invocations. The WGSL shader (`generator_model.wgsl`) and Rust
> dispatch helper (`generator_model.rs::dispatch_generator_model`) already
> exist as audited W5 scaffolding — only the runtime integration into
> `prepare_construction` / `naadf_gpu_producer_node` is missing.

---

## User constraints captured in Step 4 Q&A

### Q1 — Dispatch shape for the W5.3 segment-loop body

**Chosen:** Add a `dispatch_generator_model_with_encoder` sibling helper to
`crates/bevy_naadf/src/render/construction/generator_model.rs`.

**This explicitly LOOSENS the handoff's "treat `generator_model.rs` as a FIXED
dependency" rule.** The user approved one targeted edit: add a sibling helper
taking `encoder: &mut CommandEncoder` and matching `chunk_calc::dispatch_calc_block_from_raw_data_world_sized`'s
shape (`crates/bevy_naadf/src/render/construction/chunk_calc.rs:198-215`).
Existing `dispatch_generator_model(device, queue, ...)` is **untouched**
(the W5 unit test still uses it). The new sibling factors out the inner
`begin_compute_pass + set_pipeline + set_bind_group + dispatch_workgroups`
into a function the production `naadf_gpu_producer_node` calls per segment
with the shared `render_context.command_encoder()` — keeping wgpu's
auto-inserted STORAGE→STORAGE barriers intact across producer + downstream
chunk_calc reads.

**Why:** the alternatives (inline 14 lines, or call submit-per-segment 512
times) both compromise either code duplication or per-frame throughput. A
sibling helper preserves the W5 unit test path AND gives the production node a
clean one-line dispatch.

**How to apply:** the new helper signature is

```rust
pub fn dispatch_generator_model_with_encoder(
    encoder: &mut bevy::render::render_resource::CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    group_size_in_chunks: [u32; 3],
)
```

and the existing `dispatch_generator_model(device, queue, ...)` calls it
internally to keep the body single-sourced.

### Q2 — Extract shape for `ModelData` → render world

**Chosen:** Separate `ModelDataRender` resource + new
`stage_model_data_buildonce` extract system that mirrors
`stage_world_gpu_buildonce` (at `crates/bevy_naadf/src/render/extract.rs:167-203`)
1:1.

**Why:** `WorldDataMeta` carries the explicit "DELIBERATELY MINIMAL" docstring
clause at `render/extract.rs:102-105`. `ModelData` is a substantial `Vec<u32>`
payload (chunks + blocks + voxels) with its own build-once lifecycle —
deserves its own resource, gate, and ownership transfer.

**How to apply:** in `crates/bevy_naadf/src/render/extract.rs`, add a
`ModelDataRender` resource (the W5 brief's "render-world mirror of
`ModelData`") + a `stage_model_data_buildonce` extract system gated on
`ModelDataRender::is_none()` so it only runs once at startup. Register the
system + resource in `crates/bevy_naadf/src/render/mod.rs` next to where
`stage_world_gpu_buildonce` is registered.

### Q3 — `AppArgs::vox_gpu_construction_mode` flag

**Chosen:** Skip the flag. No driver-flow customisation needed.

**Why:** the W5.5 gate just boots `GridPreset::Vox + fixed_world_size = true +
gpu_construction_enabled = true` and asserts framebuffer non-empty after the
GPU dispatch completes. No phase-level customisation needed (no region-rect
override, no phase skip).

**How to apply:** in `crates/bevy_naadf/src/bin/e2e_render.rs`, add the
`--vox-gpu-construction` flag parsing + a dispatch branch that calls into the
new module's `run_vox_gpu_construction()` entry point. Do NOT add a
`vox_gpu_construction_mode: bool` field to `AppArgs` in `lib.rs`. The new e2e
module sets `AppArgs::gpu_construction_enabled = true` + `fixed_world_size =
true` + `grid_preset = GridPreset::Vox { path: OASIS_VOX_FIXTURE_PATH, tiles: 1 }`
directly.

### Q4 — W5.5 `.vox` fixture

**Chosen:** Use the existing in-tree
`crates/bevy_naadf/assets/test/oasis_hard_cover.vox` fixture (the same file
the user mentioned downloading; it's already in-tree, already git-tracked, and
already exercised by `--oasis-edit-visual`, `--small-edit-repro`, and other
gates).

**Why:** the user's instinct was right (real-world Oasis fixture rather than a
synthesised one); the file is already in-tree as
`OASIS_VOX_FIXTURE_PATH = "crates/bevy_naadf/assets/test/oasis_hard_cover.vox"`
at `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs:81`. Reusing the constant
keeps a single source-of-truth for the fixture path and ensures the W5.5 gate
exercises the same model the user's `just dev` loads.

**How to apply:** the new W5.5 module imports + reuses `OASIS_VOX_FIXTURE_PATH`
from `oasis_edit_visual.rs` (or re-exports it). The fixture is ~85 MB; it's
already committed (presumably via Git LFS — verify before adding any new LFS
config). Camera spawn is the C#-faithful `(500, 200, 40)` voxels per
`install_vox_in_fixed_world` at `crates/bevy_naadf/src/voxel/grid.rs:323-326`
— framing depends on the model's own bounds within the fixed
4096×512×4096-voxel world. Verify the camera frames a populated region BEFORE
adding the framebuffer-non-empty assertion (the populated chunks of
oasis_hard_cover.vox are ~93×34×84 chunks per audit + comments in
`vox_import.rs:9`); if the C# spawn point looks at empty space, override
`InitialCameraPose` in the e2e module to frame the model.

---

## Reuse audit summary (from [`00-reuse-audit.md`](00-reuse-audit.md))

The W5 integration is **~80% reuse**. The genuinely greenfield work:

1. The segment-loop body in `naadf_gpu_producer_node` (~30 LOC wrapping calls
   to `dispatch_generator_model_with_encoder` + `dispatch_calc_block_from_raw_data_world_sized`).
2. The 3-buffer + 1-bind-group allocation block in `prepare_construction`
   (~50 LOC, model-after the W3 `construction_bounds_world` block at
   `render/construction/mod.rs:1166-1215`).
3. The `install_vox_in_fixed_world` rewrite (~15 LOC: parse → convert
   ConstructedWorld→ModelData → insert resource).
4. The `--vox-gpu-construction` e2e module (~200 LOC, model-after
   `crates/bevy_naadf/src/e2e/vox_e2e.rs`).
5. The `dispatch_generator_model_with_encoder` sibling helper (~20 LOC in
   `generator_model.rs`).

Per-subtask reuse table is in `00-reuse-audit.md`. Skim it for the
"verbatim / extend / model-after" verdicts before designing.

---

## Required reading (for the design agent and the impl agent)

1. **`/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/vox-gpu-rewrite/00-reuse-audit.md`** — the per-subtask reuse table + drift findings + borderline calls. The audit found `ConstructionPipelines` already has W5 fields wired and `aadf::generator::ModelData` is the perfect resource type with the right field set — both load-bearing for the design.
2. **`/mnt/archive4/DEV/bevy-naadf/CLAUDE.md`** — project rules. Two are load-bearing:
   - (a) NEVER run `cargo run --bin bevy-naadf` as a verification step. The W5.5 e2e gate is the verification surface.
   - (b) Faithful-port discipline — divergences from C# need explicit user approval + a docs entry. The W5.6 default-scene CPU retention IS such a divergence (already approved in the handoff).
3. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/construction/mod.rs`** — the integration target. Especially:
   - `naadf_gpu_producer_node` at line 1914 — the render-graph node the W5 branch is added to.
   - `ConstructionGpu` struct (around line 100) — every field is `Option<Buffer>` per the seam contract; new model_data buffers follow the same pattern.
   - `prepare_construction` — the orchestration function that allocates buffers + builds bind groups. The W3 `construction_bounds_world` block at `:1166-1215` is the cleanest precedent for the new W5 block.
   - `ConstructionPipelines::from_world` at `:251-330, :337-344` — already queues `generator_model_layout` + `generator_model_pipeline`.
4. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/construction/generator_model.rs`** (whole file, 276 lines) — the W5 module. The new `dispatch_generator_model_with_encoder` sibling helper is added here. Existing `dispatch_generator_model` MUST keep working unchanged (the W5 unit test depends on it; refactoring it to call the new sibling internally is the cleanest path, but the device/queue-built encoder + submit MUST still happen for the unit-test caller).
5. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/assets/shaders/generator_model.wgsl`** (whole file, 160 lines) — the WGSL port. FIXED. Do not edit.
6. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/grid.rs`** lines 80–340 — `setup_test_grid` + four `install_*` helpers. `install_vox_in_fixed_world` at line 306 is the one that changes.
7. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/vox_import.rs`** — `parse_dot_vox_data` (single-tile; KEEP), `parse_dot_vox_data_into_world` (line 259; DELETE in W5.4), `tile_buckets_into_world` (line 287; DELETE in W5.4), `load_vox_into_world` (line 193; DELETE in W5.4), `build_world_from_vox` (KEEP — needed to convert single-tile ImportedVox into the right shape for ModelData).
8. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/extract.rs`** — `stage_world_gpu_buildonce` at lines 167-203 + `WorldDataMeta` at lines 107-119. The new `ModelDataRender` resource + `stage_model_data_buildonce` extract system mirror this pattern 1:1.
9. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/aadf/generator.rs`** lines 73-110 — `ModelData` struct + `empty` + `uniform_full` constructors. Add `#[derive(Resource, Clone)]` to the struct (verify `Clone` isn't already derived). Field names are `data_chunk / data_block / data_voxel / size_in_chunks` (NOT `chunks / blocks / voxels` as the handoff brief paraphrased) — refer to the existing names.
10. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/lib.rs`** — `WORLD_SIZE_IN_CHUNKS = UVec3::new(256, 32, 256)` (line ~234) + `WORLD_SIZE_IN_SEGMENTS = UVec3::new(16, 2, 16)` (line ~218) + `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS = 4` (line ~224) + `tests::fixed_world_size_constants_agree` (line ~905-919). These constants are pinned by the test; do not change them.
11. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/e2e/vox_e2e.rs`** — template for the new W5.5 e2e module. The run-fn + assert-fn + entry-point shape is what to copy.
12. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/e2e/oasis_edit_visual.rs`** — defines `OASIS_VOX_FIXTURE_PATH = "crates/bevy_naadf/assets/test/oasis_hard_cover.vox"` at line 81. Reuse this constant in the new W5.5 module.
13. **`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/bin/e2e_render.rs`** — how new modes are wired into the e2e binary. The new `--vox-gpu-construction` dispatch branch goes here (~line 210-227, parallel to the existing `--vox-e2e` branch).
14. **`/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/naadf-bevy-port/16-impl-c-W1.md`** + **`16-impl-c-W3.md`** + **`16-impl-c-W4.md`** + **`16-impl-c-W5.md`** — Phase-C precedent impl logs. The new W5 integration MUST follow the same `prepare_construction`-extension + render-graph-node-hookup pattern these establish.
15. **`/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/naadf-bevy-port/15-design-c.md`** §4.5 — original W5 spec.

C# reference (already known-faithful; do NOT re-port, only consult to verify the Rust loop matches the C# loop):

- **`/mnt/archive4/DEV/NAADF/NAADF/Content/shaders/world/generator/generatorModel.fx`** — HLSL source the WGSL port descends from. The `generator_model.wgsl` is already a line-by-line port; do not re-derive.
- **`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs`** lines 120-156 — `GenerateWorld` per-segment loop the new Rust loop matches.
- **`/mnt/archive4/DEV/NAADF/NAADF/World/Generator/WorldGeneratorModel.cs`** lines 32-60 — per-segment `Effect.Parameters` set + `DispatchCompute`. The Rust loop builds `GpuGeneratorModelParams` per segment with the same field semantics.

---

## Subtask breakdown (W5.1 → W5.6)

Landing order: **W5.1 → W5.2 → W5.5 → W5.3 → W5.4 → W5.6**. W5.5 lands before
W5.3 so the e2e gate exists to catch regressions the moment the segment loop
lands.

### W5.1 — `ModelDataRender` render-world resource + build-once extract

Per Q2 above: separate render-world resource, NOT extending `WorldDataMeta`.

- Add `#[derive(Resource, Clone)]` to existing `aadf::generator::ModelData` (`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/aadf/generator.rs:73`) — also a main-world `Resource`. (Verify `Clone` isn't already derived; add only what's missing.)
- Modify `install_vox_in_fixed_world` (`/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/grid.rs:306`) to:
  - Call `vox_import::parse_dot_vox_data` (single-tile entry; NOT the deleted-in-W5.4 `load_vox_into_world` tiling variant) on the path.
  - Convert the single-tile `ConstructedWorld` returned by `vox_import::build_world_from_vox` into a `ModelData` (the encoding is byte-identical per `aadf/generator.rs:64-71`).
  - Insert the `ModelData` as a main-world `Resource` via `commands.insert_resource(model_data)`.
  - Insert an **empty** `WorldData` with `size_in_chunks = WORLD_SIZE_IN_CHUNKS` (still needed for the renderer's bind groups; `chunks_cpu / blocks_cpu / voxels_cpu` left empty; `dense_voxel_types = Vec::new()` to preserve the existing `if meta.dense_voxel_types.is_empty() { return; }` gate behaviour).
  - Continue to insert `voxel_types` from the model's palette.
  - Continue to insert `InitialCameraPose` with the C# `(500, 200, 40)` spawn.
- Add `ModelDataRender` render-world resource type in `crates/bevy_naadf/src/render/extract.rs` (or wherever `WorldDataMeta` lives). Field set mirrors `ModelData`.
- Add `stage_model_data_buildonce` extract system mirroring `stage_world_gpu_buildonce` (`render/extract.rs:167-203`) 1:1: gate on `ModelDataRender::is_none()` so it runs once, populate from main-world `ModelData`.
- Register the new resource + system in `crates/bevy_naadf/src/render/mod.rs` next to `stage_world_gpu_buildonce` registration.

### W5.2 — Upload buffers + build W5 bind group in `prepare_construction`

- Add four new `Option<Buffer>` fields to `ConstructionGpu` in `crates/bevy_naadf/src/render/construction/mod.rs` (around line 106-190):
  - `model_data_chunk_buffer: Option<Buffer>`
  - `model_data_block_buffer: Option<Buffer>`
  - `model_data_voxel_buffer: Option<Buffer>`
  - `model_data_params_buffer: Option<Buffer>` (the per-segment `GpuGeneratorModelParams` uniform — one buffer, rewritten 512 times in W5.3)
- Add one new field to `ConstructionBindGroups`:
  - `construction_generator_model: Option<BindGroup>` (or similar — match the existing naming convention in the struct).
- In `prepare_construction`, add a new block (model-after `construction_bounds_world` at `:1166-1215`) that:
  - Requires the `ModelDataRender` resource (`Option<Res<ModelDataRender>>`).
  - Requires `ConstructionPipelines::generator_model_layout` (already wired per audit drift #2).
  - Requires `ConstructionGpu.segment_voxel_buffer` to exist (W1's chunk_data_rw target — the same buffer the generator writes to + chunk_calc reads from).
  - Allocates the 3 model_data storage buffers via `generator_model::create_storage_buffer_u32` (reuse verbatim) and uploads via `RenderQueue::write_buffer`.
  - Allocates the params uniform buffer via `generator_model::create_params_uniform` with a zeroed initial `GpuGeneratorModelParams`.
  - Builds the bind group against `generator_model_layout` with: binding 0 = segment_voxel_buffer (chunk_data_rw), binding 1 = model_data_chunk_buffer, binding 2 = model_data_block_buffer, binding 3 = model_data_voxel_buffer, binding 4 = model_data_params_buffer.
  - Stores the bind group on `ConstructionBindGroups::construction_generator_model`.
- Pattern: every step gates on `is_none()` so it only runs the first frame all dependencies are present (the seam contract pattern).

### W5.3 — Per-segment generator + chunk_calc dispatch loop

- Add the `dispatch_generator_model_with_encoder` sibling helper in
  `crates/bevy_naadf/src/render/construction/generator_model.rs` per Q1
  decision. Refactor existing `dispatch_generator_model` to call it
  internally (one source of truth for the inner dispatch).
- Extend `naadf_gpu_producer_node` (line 1914) with a new gate + branch:
  - **Three-way gate (per audit drift #5):** `ModelDataRender` present → run the new W5 branch (per-segment generator + chunk_calc); else if `dense_voxel_types` non-empty → existing chunk_calc-only branch; else → fall back to CPU upload (current behaviour at `:1936-1941`).
  - **W5 branch body** (mirrors C# `WorldData.cs:120-156`):
    ```
    for (sx, sy, sz) in 0..16, 0..2, 0..16:
        group_offset_in_chunks = [sx, sy, sz] * WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4   // = chunk-space offset of this segment
        group_size_in_chunks   = [WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4, ...]            // = 16 chunks per axis per segment
        // 1. rewrite the per-segment uniform
        render_queue.write_buffer(model_data_params_buffer, 0, bytes_of(GpuGeneratorModelParams { ... }))
        // 2. dispatch generator_model.wgsl into segment_voxel_buffer
        dispatch_generator_model_with_encoder(encoder, generator_model_pipeline, construction_generator_model_bg, group_size_in_chunks)
        // 3. dispatch chunk_calc::calc_block_from_raw_data over the same segment extent
        chunk_calc::dispatch_calc_block_from_raw_data_world_sized(encoder, p_calc, world_bg, group_size_in_chunks)
    ```
  - **After the segment loop**, run the bounds chain ONCE (mirrors the existing `:1980-1992` `compute_voxel_bounds` + `compute_block_bounds` chain).
  - Set `gpu.gpu_producer_has_run = true`.
- **Note on encoder lifetime:** the node currently takes `render_context.command_encoder()` once at `:1969`. The W5 branch uses the same encoder for ALL 512 segments + the bounds chain. wgpu auto-inserts STORAGE→STORAGE barriers between adjacent passes on the same buffer alias.
- **Note on `RenderQueue` access:** the per-segment uniform rewrite needs `&RenderQueue`. The node currently does NOT take `Res<RenderQueue>` as a parameter — add it to the system signature.
- **C# loop semantics to match exactly** (read `WorldData.cs:120-156` + `WorldGeneratorModel.cs:32-60` before writing the loop body): outer loops iterate Y then Z then X (or whatever C# does); per-segment `GeneratorModelParams::groupOffsetInChunks` = `segment_idx * WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4`; `groupSizeInChunksX/Y` = the segment's chunk extent per axis (NOT the world's — these strides scope `groupIndex` to the per-segment buffer, see `generator_model.wgsl:57`).

### W5.4 — Delete the CPU tile stop-gap

- Delete `vox_import::tile_buckets_into_world` (line 287).
- Delete `vox_import::parse_dot_vox_data_into_world` (line 259).
- Delete `vox_import::load_vox_into_world` (line 193).
- Delete the two tests `into_world_tiles_xz_and_leaves_y_above_tile_empty` (line 1832) + `into_world_with_target_smaller_than_tile_clips` (line 1877).
- KEEP `vox_import::replicate_buckets_xz` (line 335) — sibling code reached by the non-fixed-world `--vox-grid N` path that other e2e gates use.
- Update three docstring sites that reference the stop-gap:
  - `crates/bevy_naadf/src/render/construction/mod.rs:2017-2020` ("generator_model per segment — currently bypassed for the bevy-naadf test scene") → update to "W5 landed; per-segment dispatch is the runtime producer for `.vox` loads".
  - `crates/bevy_naadf/src/voxel/vox_import.rs:46-56` Δ-GPUProducer comment block → update to "W5 landed; ModelData drives the GPU producer for `.vox` loads. Default scene retains CPU upload by deliberate divergence (see W5.6)."
  - `crates/bevy_naadf/src/voxel/vox_import.rs:382-385` Δ-GPUProducer comment in `build_world_from_vox` → similar update.

### W5.5 — `--vox-gpu-construction` e2e gate

- Add `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` (new module) model-after `crates/bevy_naadf/src/e2e/vox_e2e.rs`:
  - Use `OASIS_VOX_FIXTURE_PATH` constant from `oasis_edit_visual.rs:81` (re-exported or imported).
  - Build `AppArgs` with `grid_preset = GridPreset::Vox { path: PathBuf::from(OASIS_VOX_FIXTURE_PATH), tiles: 1 }`, `fixed_world_size = true`, `gpu_construction_enabled = true`.
  - Call `run_e2e_render_with_args(args)` (entry point at `crates/bevy_naadf/src/lib.rs:780-806`).
  - Assert framebuffer non-empty (luminance > skybox threshold) over a region the C# `(500, 200, 40)` spawn frames — VERIFY visually first using the existing `--oasis-edit-visual` framebuffer capture as a reference; reuse the framebuffer helpers at `crates/bevy_naadf/src/e2e/framebuffer.rs` (`Rect`, `region_mean`, `luminance`, `from_fractional`, `check_not_degenerate`).
  - Optional second assertion: scan `PipelineCache` for compile errors via `e2e/checks.rs::pipeline_scan_result`.
  - Optional third assertion: verify the W5 + W1 nodes dispatched via `e2e/checks.rs::assert_nodes_dispatched`.
- Add `vox_gpu_construction.rs` to `crates/bevy_naadf/src/e2e/mod.rs` exports.
- Add `--vox-gpu-construction` flag parsing in `crates/bevy_naadf/src/bin/e2e_render.rs` (~line 89) + a dispatch branch (~line 210-227, model-after `--vox-e2e`) that calls `bevy_naadf::e2e::vox_gpu_construction::run_vox_gpu_construction()`.
- No `AppArgs::vox_gpu_construction_mode` flag (per Q3 decision).

### W5.6 — Document default-scene CPU-retention divergence

- Per audit additional reuse #5, the divergence is **already implemented**: `install_default_embedded_in_fixed_world` (`grid.rs:156-249`) inserts `dense_voxel_types = Vec::new()`, and `naadf_gpu_producer_node`'s gate at `:1936-1941` short-circuits → CPU upload path takes over. No code change.
- Add a docs entry per `CLAUDE.md`'s faithful-port discipline. Suggested location: `docs/orchestrate/naadf-bevy-port/12-alignment-gap.md` (the canonical alignment-status doc) — append a "vox-gpu-rewrite W5.6 divergence" section explaining: the CPU `compose_default_scene_into_fixed_world` path is retained for the no-`--vox` Default case because synthesising a primitive scene as a `ModelData` would force unwanted 16×16 XZ-tiling of the demo (the C# generator tiles unconditionally via modulo). Mark `compose_default_scene_into_fixed_world` for deletion only when a full-GPU default path lands later.

---

## Forbidden moves

- **No reviving the CPU XZ tiling.** The W5.4 deletions are non-negotiable; `tile_buckets_into_world` / `parse_dot_vox_data_into_world` / `load_vox_into_world` go away.
- **No editing `generator_model.wgsl`.** Audited byte-for-byte port; treat as fixed dependency.
- **No editing `generator_model.rs` BEYOND the `dispatch_generator_model_with_encoder` sibling addition + the one-line refactor of existing `dispatch_generator_model` to call the sibling.** No other changes; do not touch `GpuGeneratorModelParams`, `generator_model_layout_descriptor`, `queue_generator_model_pipeline`, `create_storage_buffer_u32`, `create_params_uniform`.
- **No `cargo run --bin bevy-naadf` as a verification step.** Per `CLAUDE.md`; the W5.5 e2e gate is the verification surface.
- **No changing `AppArgs::fixed_world_size` semantics or `WORLD_SIZE_IN_*` constants.** Load-bearing; pinned by `tests::fixed_world_size_constants_agree` in `lib.rs`.
- **No bundling the AADF-convergence-race fix into the W5 PR.** Out of scope per handoff. File as `w3-startup-convergence-race` followup.
- **No CPU `compute_aadf_layer` precompute at compose time.** Was previously rejected with "the fix must be to align us to c# version, no bandaids."
- **No flipping `synchronous_pipeline_compilation: true` in production.** Was previously rejected with "lets not this and proceed with gpu rewrite."
- **No new `AppArgs` mode flag for the W5.5 gate.** Per Q3 decision.
- **No extending `WorldDataMeta` with model_data fields.** Per Q2 decision — separate `ModelDataRender` resource instead.

---

## Verification gates per `CLAUDE.md`

- `cargo build --workspace` — proves it compiles.
- `cargo test --workspace --lib` — proves unit + integration logic. Baseline: 198 passed, 1 ignored.
- `cargo run --bin e2e_render -- --vox-gpu-construction` — the new W5.5 gate; the verification surface for the W5 integration.
- `cargo run --bin e2e_render -- --vox-e2e` — existing gate; must stay green (W5 must not regress the non-fixed-world `.vox` path).
- `cargo run --bin e2e_render -- --oasis-edit-visual` — existing gate using same fixture; must stay green.
- `cargo run --bin e2e_render -- --validate-gpu-construction` — existing W5 unit-test-like gate (validates GPU vs CPU oracle); must stay green.
- `cargo run --bin e2e_render -- --baseline` + `--edit-mode` + `--entities` + `--runtime-edit-mode` + `--small-edit-visual` + `--small-edit-repro` — full e2e suite; all must stay green.

Pre-existing project state (per handoff): `cargo build --workspace` clean,
`cargo test --workspace --lib` = 198 passed/1 ignored, `cargo clippy` no new
warnings from changed files.

---

## Open items for the design agent

The reuse audit's "Borderline calls" #1 (encoder shape) and #4 (extract
shape) are RESOLVED by the Q1 and Q2 decisions above. The remaining audit
items to thread through the design:

1. **Three-way producer gate** (audit drift #5) — design must spell out the
   exact gate order: ModelDataRender → dense_voxel_types → CPU upload. This
   is not currently in `naadf_gpu_producer_node`.
2. **`RenderQueue` system parameter** (audit W5.3 row) — the segment loop
   needs `Res<RenderQueue>` added to the node signature.
3. **`run_worldgen_only` flag** (audit drift #3) — the flag exists but is
   unused. Design call: does the new W5 branch additionally gate on it
   (debug-only fast path) or ignore it? Recommended: ignore for this PR;
   surface as a followup if needed.
4. **C# loop iteration order** — the design must explicitly state the
   iteration order (Y/Z/X vs X/Y/Z etc.) that matches `WorldData.cs:120-156`.
   The order matters only if any segment's dispatch reads from a previously
   written segment's voxels — which `generator_model.wgsl` does not (each
   segment writes its own slice of `chunk_data_rw`). But matching C# is the
   faithful-port discipline. Design agent must verify the C# order and
   match it.
5. **`InitialCameraPose` for the W5.5 gate** — verify the C# `(500, 200, 40)`
   spawn frames a populated region of `oasis_hard_cover.vox` (the model is
   ~93×34×84 chunks per audit + comments) before the gate's framebuffer
   assertion is added. If the spawn looks at empty space, the gate either
   overrides `InitialCameraPose` (a divergence — flag it) or asserts an
   inverse condition (empty framebuffer means GPU dispatch ran but populated
   wrong region; non-empty means a different correctness signal). Design
   agent decides.

The design output (`02-design.md`) MUST include `## Decisions & rejected alternatives`
+ `## Assumptions made` sub-sections so the implementer (next agent in the
chain) has the full reasoning trace.
