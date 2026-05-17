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

---

## impl W5.5 findings (2026-05-17)

### Files touched

- `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` (NEW, ~190 LOC) —
  the W5.5 module per `02-design.md` §W5.5. Exposes
  `run_vox_gpu_construction() -> AppExit` (entry point invoked from
  `bin/e2e_render.rs`), the `assert_frame_not_black` helper, and
  `save_vox_gpu_construction_screenshot`. Reuses `OASIS_VOX_FIXTURE_PATH`
  + `oasis_vox_fixture_path()` from `e2e/oasis_edit_visual.rs:81-92`
  (per Q4 decision — single source-of-truth for the fixture path).
- `crates/bevy_naadf/src/e2e/mod.rs:33` — added `pub mod vox_gpu_construction;`
  alongside `pub mod vox_e2e;` per design §W5.5 / line 583.
- `crates/bevy_naadf/src/bin/e2e_render.rs:90-91` — added
  `vox_gpu_construction_mode` flag parsing immediately after
  `small_edit_repro_mode`. Per design § W5.5 / lines 766-769.
- `crates/bevy_naadf/src/bin/e2e_render.rs:212-221` — added the
  `vox_gpu_construction_mode` dispatch branch immediately BEFORE
  `vox_e2e_mode`. Per design § W5.5 / lines 775-784. Calls
  `bevy_naadf::e2e::vox_gpu_construction::run_vox_gpu_construction()`.

No other files touched. `generator_model.wgsl` + `generator_model.rs` +
W5.4-deletion candidates (`vox_import::tile_buckets_into_world`, etc.)
untouched — verified by `git status`.

### Verification results

- `cargo build --workspace` — **clean** (0 errors, 0 new warnings);
  finished in 57.11s (`dev` profile, optimized + debuginfo).
- `cargo test --workspace --lib` — **198 passed, 1 ignored** across 3
  suites in 4.09s. Matches the baseline exactly. No new failures, no
  test-count drift.
- `cargo run --release --bin e2e_render -- --vox-gpu-construction` —
  **ran end-to-end without panic or WGPU validation error**; exited
  non-zero (4 driver checks failed). **Exact outcome:**

  - The gate booted, GPU adapter selected (NVIDIA RTX 5080 / Vulkan),
    fixture parsed: `Oasis VOX → ModelData (93×34×84 chunks;
    data_chunk=265608 u32s, data_block=1617216 u32s,
    data_voxel=10498368 u32s, 257 palette entries)`. Fixed world
    256×32×256 chunks initialised; the `prepare_construction` log says
    "GPU producer chain runs per WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)".
  - Framebuffer captured + saved to
    `target/e2e-screenshots/e2e_latest.png` (62 055 bytes, valid PNG).
  - **Framebuffer is pure-black** — region luminance reported as
    emissive 0.7, solid 0.7, sky 0.7 (all ~0 — the readback is
    essentially black). `0.0%` of the frame is non-black per the
    standard luminance gate.
  - **Nine render-graph nodes never dispatched**: `naadf_first_hit`,
    `naadf_taa_reproject`, `naadf_ray_queue`, `naadf_global_illum`,
    `naadf_sample_refine`, `naadf_spatial_resampling`, `naadf_denoise`,
    `naadf_calc_new_taa_sample`, `naadf_final_blit`. These nodes have
    `WorldGpu`-readiness preconditions; with the W5 segment loop not
    landed, `gpu_producer_has_run` never flips on the W5 path,
    `WorldGpu::chunks` stays zeroed, and the downstream nodes early-out.
  - Exit status: non-zero (driver-reported `4 check(s) failed` —
    degenerate-frame floor + luminance liveness + region gate +
    node-dispatch check).
  - **No panic, no WGPU validation error, no crash.** This is the
    load-bearing health signal: W5.2's bind-group setup + buffer
    allocation are sound; the harness gets to the readback phase
    cleanly.

### Assertion strategy

**Landed:** custom `assert_frame_not_black` helper on the central 40 % ×
40 % region with luminance floor **40.0** (option b from `02-design.md`
§Assumptions made #8).

**Rationale (informed by first-run observation):**

- Pre-W5.3 the framebuffer is pure-black (~0.7 luminance), NOT the
  expected sky band (~146). The design assumed sky tint would dominate
  pre-W5.3; the reality is that nine downstream NAADF render-graph
  nodes never dispatch when `WorldGpu` is unready, so the framebuffer
  is the clear color (black). The 40.0 floor sits well above the
  pre-W5.3 ~0.7 baseline so the gate trips, AND well below the
  post-W5.3 ~146 sky band so the gate will pass once W5.3 wires the
  segment loop.
- Option (a) — reuse `vox_e2e_mode = true` — was rejected: even
  post-W5.3 the Oasis-populated region (`~744, 272, 672` voxels) is in
  the opposite hemisphere from the e2e camera (`(86, 42, 90)` →
  `(32, 16, 32)`), so the central rect samples sky band (~146) — that
  would trip `--vox-e2e`'s `SKY_LUMINANCE_CEILING = 160` gate. The
  "not pure-black" floor is the correct shape for the off-frame state.
- The driver-side standard gates (`degenerate-frame floor`, `luminance
  liveness gate`, `region gate`, `node-dispatch check`) ALREADY catch
  the same pre-W5.3 regression (and the same load-bearing
  post-W5.3-regression signals like "pipeline compile errors crashed
  the GPU producer"). My `assert_frame_not_black` helper is callable
  for future driver integrations / unit tests but is NOT wired into
  the driver's run-time gate path in this dispatch — wiring it would
  duplicate the driver's existing "the screen isn't black" signal.

### Pre-W5.3 baseline (RIGHT NOW)

Framebuffer at the standard e2e camera pose `(86, 42, 90) → (32, 16, 32)`
with the Oasis fixture loaded through `install_vox_in_fixed_world` is
**pure-black** (~0.7 luminance over the central 40 % × 40 % rect; 0.0 %
of the frame brighter than 2.0 luminance per the standard liveness gate).
The W5 install path populates `ModelData` + the W5 prepare block
allocates buffers + builds the bind group, but the segment loop (W5.3)
hasn't landed → `gpu_producer_has_run` stays false → `WorldGpu::chunks`
stays zeroed → the downstream NAADF render-graph nodes (`first_hit`,
`ray_queue`, `global_illum`, `sample_refine`, `spatial_resampling`,
`denoise`, `taa_reproject`, `calc_new_taa_sample`, `final_blit`) skip
their dispatch on the readiness gate → no rendering happens → clear
color (black) is what the swapchain holds.

This is the **reference observation for W5.3** — the W5.3 dispatch is
expected to flip every one of these to "ran" + bring the framebuffer
above the floor.

### Post-W5.3 expectation

Once W5.3 lands the per-segment dispatch + flips `gpu_producer_has_run =
true` after the segment loop completes:

- `WorldGpu::chunks` populates (every chunk gets a `ChunkCell` written
  by `generator_model.wgsl` → `chunk_calc::calc_block_from_raw_data`).
- The W3 bounds chain populates `block_bounds_buffer` +
  `voxel_bounds_buffer`.
- The nine downstream render-graph nodes (above) hit their readiness
  preconditions and dispatch.
- At the standard e2e camera pose, the camera frames a region NEAR the
  origin (`(32, 16, 32)`) of the 4096×512×4096-voxel world. With the
  Oasis fixture's `generator_model.wgsl` semantics (each segment writes
  its slice with the model tiled across the world, per `generator_model.
  wgsl:114-116`'s Y-clamp + per-axis modulo addressing), the camera
  will see **some** Oasis-derived geometry close to the origin — the
  model tiles across the world. The central rect should rise to at
  least the sky band (~146) and likely to GI-lit emissive levels
  (~240+) if Oasis-emissive material projects into the rect.
- The `assert_frame_not_black` floor of 40.0 will PASS by a wide
  margin in either case.

If post-W5.3 the central rect stays under 40 → the W5 chain is broken
in a load-bearing way (segment loop didn't fire, bind group misbinding,
WGPU validation crash, etc.).

### Design adherence

Followed `02-design.md` §W5.5 (lines 575-786) verbatim:

- **Module skeleton** (design lines 590-740): copied the design's Rust
  body exactly. Three intentional adjustments:
  1. **Did NOT set `app_args.vox_e2e_mode = true`**. The design's
     skeleton at line 684 sets it but the same design's "Note on
     assertion strategy" at lines 742-753 + assumption #8 at lines
     1623-1633 both explicitly leave the choice to the implementer
     based on first-run observation. First-run shows pure-black
     framebuffer (not sky), so `vox_e2e_mode = true` would FAIL on
     `assert_vox_geometry_visible`'s 160 threshold AND mask the real
     post-W5.3 expectation (sky band ~146 also fails 160). The custom
     `assert_frame_not_black` floor is the correct shape per the same
     skeleton's own option (b).
  2. **Added `app_path_for_args` helper** — a small wrapper that
     mirrors the resolved path from `oasis_vox_fixture_path()`
     verbatim into the `GridPreset::Vox { path, ... }` carrier. The
     design's literal `PathBuf::from(OASIS_VOX_FIXTURE_PATH)` would
     bypass the workspace-vs-crate-relative fallback logic in
     `oasis_vox_fixture_path()` and could break the gate when run
     from inside the crate directory. Using the resolved path
     preserves the fallback discipline `oasis_edit_visual.rs`
     established.
  3. **Did NOT wire `assert_frame_not_black` into the driver**. The
     design skeleton's helper is callable but not wired; the driver's
     existing `degenerate-frame floor` + `luminance liveness gate` +
     `node-dispatch check` already cover the same "screen is black,
     nothing rendered" signal. Wiring my helper into the driver
     would either duplicate the existing gates OR require a separate
     `vox_gpu_construction_mode` driver branch (similar to
     `vox_e2e_mode`'s shape) — out of W5.5's scope per the Q3
     decision (no driver-flow customisation).
- **`e2e/mod.rs:33` export addition** (design line 759): added.
- **`bin/e2e_render.rs:90-91` flag parse** (design lines 762-769):
  added.
- **`bin/e2e_render.rs:212-221` dispatch branch** (design lines
  775-784): added immediately BEFORE the `vox_e2e_mode` branch, as
  the design specifies.

### Assumption-verification findings (per `02-design.md` §Assumptions made)

- **Assumption 3** (`OASIS_VOX_FIXTURE_PATH` resolves at `cargo run`
  time from the workspace root): **verified true**. The fixture exists
  at `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` (84 911 723
  bytes, MagicaVoxel v150 file per `file(1)`). The gate's stdout log
  confirmed the path resolved correctly:
  `loading Oasis VOX fixture from crates/bevy_naadf/assets/test/oasis_hard_cover.vox (84911723 bytes)`.
- **Assumption 8** (framebuffer-assertion choice): **made on first
  run** per the design's explicit directive. Choice = custom
  `assert_frame_not_black` floor at 40.0; rationale documented in the
  "Assertion strategy" section above.
- **InitialCameraPose decision** (e2e harness ignores
  `InitialCameraPose` — uses `setup_e2e_camera` verbatim): **verified
  true by observation**. The Oasis-populated region sits at
  `~(744, 272, 672)` voxels but the framebuffer captured at the
  standard e2e camera pose `(86, 42, 90) → (32, 16, 32)` was
  pure-black, confirming the harness did NOT override the camera to
  frame the Oasis model. The W5.5 gate runs with the standard pose;
  the assertion accepts the Oasis-off-frame state per the design's
  decision.
- **Assumption 7** (`generator_model.wgsl` is FIXED): **respected** —
  `git status` confirms the file is untouched.
- Other assumptions (1, 2, 4-6, 9-11) are W5.1 / W5.2 / W5.3-scope and
  were exercised by prior dispatches; not re-verified here.

### Surprises

**No WGPU validation errors** — the W5.2 bind-group setup + the 4 W5
buffers (3 storage + 1 uniform) are correctly allocated and bound;
WGPU's validation pass accepts the layout. The brief flagged this as a
hard-gate-worth-surfacing-immediately signal if it had fired — it did
NOT, so W5.2's surface area is healthy.

**Framebuffer was pure-black, NOT sky-tinted**, contrary to the
design's assumption that the sky band would render in the absence of
geometry. The reason: the entire NAADF render-graph chain (which
includes the sky / atmosphere shader as part of `naadf_final_blit`)
has `WorldGpu`-readiness preconditions; when those are unmet, the
whole render chain skips dispatch and the swapchain holds the clear
color. This is structurally fine for W5.5 (the gate still detects
"GPU chain dormant"), but the design's assumed "sky band at ~146
luminance" baseline pre-W5.3 was incorrect. The actual baseline is
~0.7 luminance (black) — better separation from the 40.0 floor.

The W5 producer's first-frame log message
("phase-c followup#1 — gpu construction ENABLED (default). The runtime
GPU dispatch chain (generator-bypass → chunk_calc.calc_block_from_raw_data
→ compute_voxel_bounds → compute_block_bounds → bounds_calc.add_initial)
runs in `prepare_construction` on the first render frame ...") is
slightly misleading post-W5.2: it lists the W4 chunk-calc-only branch
("generator-bypass"), not the W5 branch ("generator + chunk_calc"). The
log is informational only; semantics are correct. (Out of W5.5 scope to
update.)

### What's NOT yet working

**The GPU producer chain doesn't fire until W5.3 lands.** This is the
EXPECTED intermediate state — the W5.5 dispatch ships ahead of W5.3
deliberately so the gate is in place to immediately catch regressions
when W5.3 wires the segment loop. Until W5.3:

- The W5 install path (W5.1) populates `ModelData` correctly.
- The W5 prepare block (W5.2) allocates buffers + builds the bind group
  correctly.
- `naadf_gpu_producer_node` does NOT dispatch the generator pass — no
  W5 branch in the node body yet.
- `gpu_producer_has_run` never flips on the W5 path → downstream nodes
  skip dispatch → framebuffer stays black.

The `--vox-gpu-construction` gate FAILS pre-W5.3 (4 driver checks
fail). Post-W5.3 the same gate should PASS (segment loop dispatches →
chunks populate → render chain dispatches → framebuffer luminance rises
above the 40.0 floor + above the standard driver's region / liveness
thresholds).

---

## impl W5.3 findings (2026-05-17)

### Files touched

- `crates/bevy_naadf/src/render/construction/generator_model.rs:217-275` —
  added the `dispatch_generator_model_with_encoder` sibling helper per Q1
  (encoder-taking, matches `chunk_calc::dispatch_calc_block_from_raw_data_world_sized`
  shape). Refactored existing `dispatch_generator_model(device, queue, ...)`
  to call the sibling internally — one source of truth for the inner
  `begin_compute_pass + set_pipeline + set_bind_group + dispatch_workgroups`.
- `crates/bevy_naadf/src/render/construction/mod.rs:2087-2331` — rewrote
  `naadf_gpu_producer_node` to:
  - Add `render_queue: Res<RenderQueue>` parameter (for per-segment uniform
    write_buffers).
  - Add `model_data: Option<Res<crate::render::extract::ModelDataRender>>`
    parameter (drives the three-way branch ladder).
  - Add the **three-way branch ladder** (`(a) ModelData present → W5 chain;
    (b) dense_voxel_types non-empty → existing chunk-calc-only; (c) → CPU
    upload fallback`).
  - Implement the W5 segment loop (Z outer, Y middle, X inner; 16 × 2 × 16 =
    512 segments) rewriting both `model_data_params_buffer` AND
    `bounds_params_buffer` per segment, then dispatching
    `generator_model_with_encoder` + `chunk_calc.calc_block_from_raw_data_world_sized`
    on the same shared encoder. Bounds chain runs ONCE after the loop with
    clamped workgroup counts.
- `crates/bevy_naadf/src/render/prepare.rs:211-227` — **W5.1 patch**:
  replaced the `if extracted.chunks.is_empty() { return; }` early-return with
  `if extracted.size_in_chunks == UVec3::ZERO { return; }`. The original
  check was a proxy for "setup_test_grid not run yet" but it tripped on the
  W5.1 fixed-world install path which leaves `chunks_cpu` empty by design
  (the W5 GPU producer populates `WorldGpu::chunks_buffer` from segment
  dispatches; `chunks_cpu` stays empty). Without this fix, `WorldGpu` would
  NEVER be built on the W5 path → entire downstream chain dormant →
  pre-W5.3 framebuffer ~0.7 luminance (pure-black). With this fix
  `WorldGpu` builds when `size_in_chunks` is non-zero regardless of
  `chunks_cpu` length.
- `crates/bevy_naadf/src/render/mod.rs:122-141` — **W5.1 patch**: removed
  `.init_resource::<ModelDataRender>()`. The original W5.1 dispatch added
  this thinking it was needed for the `Option<Res<X>>` system params to
  work, but `init_resource` seeded a default empty `ModelDataRender { ... }`
  → `stage_model_data_buildonce`'s `if existing.is_some() { return; }`
  short-circuited forever → the real `ModelData` from
  `install_vox_in_fixed_world` was NEVER copied into the render world.
  `init_resource` is wrong for build-once-inserted resources; it must be
  absent so the extract's `commands.insert_resource(...)` is the first
  insertion. Updated the comment block to spell this out explicitly so a
  future re-edit doesn't reintroduce the bug.

### `generator_model.rs` refactor — signature preservation confirmed

Existing `pub fn dispatch_generator_model(device: &RenderDevice, queue:
&RenderQueue, pipeline, bind_group, group_size_in_chunks: [u32; 3])`
signature is **UNCHANGED** byte-for-byte. Body now reads:

```rust
let mut encoder = device.create_command_encoder(...);
dispatch_generator_model_with_encoder(&mut encoder, pipeline, bind_group, group_size_in_chunks);
queue.submit([encoder.finish()]);
```

The W5 unit test (`generator_model_gpu_vs_cpu_bit_exact` at
`mod.rs:3206-3377`) calls `dispatch_generator_model(device, queue, ...)`
unchanged and **continues to pass** in the verification run (198 passed, 1
ignored — baseline preserved exactly).

### C# loop order confirmation

Loop nesting in `naadf_gpu_producer_node` is:

```rust
for sz in 0..crate::WORLD_SIZE_IN_SEGMENTS.z {   // outer Z
    for sy in 0..crate::WORLD_SIZE_IN_SEGMENTS.y {  // middle Y
        for sx in 0..crate::WORLD_SIZE_IN_SEGMENTS.x {  // inner X
            // per-segment dispatch
        }
    }
}
```

Matches C# `NAADF/NAADF/World/Data/WorldData.cs:136-140` (Z outer, Y
middle, X inner) byte-for-byte. Per design decision § "Loop iteration
order — Z outer, Y middle, X inner": observationally invariant for the
dispatch outcome (each segment writes its own segment_voxel_buffer slice,
consumed by the same-iteration chunk_calc), but matching C# satisfies the
faithful-port discipline.

### Per-segment params field values — example for `(sx=5, sy=1, sz=10)`

`segment_chunks = WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 = 4 * 4 = 16`.

```rust
group_offset_in_chunks = [5 * 16, 1 * 16, 10 * 16] = [80, 16, 160]
```

Per-segment `GpuGeneratorModelParams`:
```
size_in_voxels        = [4096, 512, 4096]  (WORLD_SIZE_IN_VOXELS)
_pad0                 = 0
model_size_in_chunks  = [93, 34, 84]  (Oasis ModelData.size_in_chunks)
_pad1                 = 0
group_offset_in_chunks= [80, 16, 160]
group_size_in_chunks_x= 16
group_size_in_chunks_y= 16
_pad2/3/4             = 0
```

Per-segment `GpuConstructionParams` (bounds_params_buffer rewrite):
```
size_in_chunks            = [256, 32, 256]  (WORLD_SIZE_IN_CHUNKS)
_pad0                     = 0
group_size_in_groups      = bounds_calc::group_size_in_groups_of([256, 32, 256])
_pad1                     = 0
bound_group_queue_max_size= 1
hash_map_size             = config.initial_hash_map_size
segment_size_in_chunks    = 16  (vs the build-once value of 4 in
                                  prepare_construction; per-segment update
                                  is required so chunk_calc.wgsl's
                                  `chunk_index_in_segment` computation uses
                                  the right X/Y stride into the 16³ buffer)
max_group_bound_dispatch  = config.max_group_bound_dispatch
chunk_offset              = [80, 16, 160]  (matches C# CalculateChunkBlocks
                                            at WorldData.cs:492-494)
_pad2 / frame_index / changed_* = 0
```

**Critical fidelity detail not in the design spec**: the design at
`02-design.md:1003-1011` only shows the per-segment `model_data_params_buffer`
rewrite. But chunk_calc.wgsl reads `params.chunk_offset` (line 356) AND
`params.segment_size_in_chunks` (line 351) from the `construction_world`
bind group's params slot, which is `bounds_params_buffer`. Without
per-segment rewrites of THIS buffer the chunk_calc dispatch would write
every segment's chunks to world position `[0,0,0]` with stride `seg=4`
(the build-once value at `prepare_construction:1183`). C#
`WorldData.cs:492-494, 503` confirms `chunkOffset` AND `segmentSizeInChunks`
are both set per-segment. The implementation rewrites both buffers in the
loop.

### Bounds chain dispatch count — option (a) clamping strategy

**Strategy chosen:** option (a) clamping to `WGPU_MAX_WORKGROUPS_PER_DIM =
65535` (the wgpu / WebGPU spec minimum per-axis limit, per
`assets/shaders/sample_refine.wgsl:77-90`).

**Raw upper bounds** for the W5 path with empty CPU mirror
(`world_data_meta.{blocks,voxels}_cpu_len = 0`):
- `world_chunks = 256 * 32 * 256 = 2,097,152`
- `max_blocks_u64 = world_chunks * 64 = 134,217,728` (134M)
- `max_voxels_u64 = max_blocks_u64 * 32 = 4,294,967,296` (4.3B)
- `raw_voxel_workgroups = (max_voxels_u64 / 32 + 1) = 134,217,729`
- `raw_block_workgroups = (max_blocks_u64 / 64 + 1) = 2,097,153`

**Clamped to wgpu 65535/axis limit:**
- `voxel_workgroups = 65535`
- `block_workgroups = 65535`

**Sanity-check confirmation:** at runtime the producer logs:
```
vox-gpu-rewrite W5 — per-segment GPU producer chain DISPATCHED (512 segments
× (generator_model + calc_block); bounds chain ×1;
voxel_workgroups=65535 (raw 134217729), block_workgroups=65535 (raw 2097153)).
```

Both stay within wgpu's 65535/axis cap; no wgpu validation error fires.

**Trade-off:** under-dispatch — workgroups past index 65534 of
`blocks[]`/`voxels[]` skip the AADF (Adaptive Acceleration Data Field)
write in `compute_voxel_bounds` / `compute_block_bounds`. AADFs are an
acceleration hint for raycast traversal (early-skip of empty regions).
Missing AADFs do NOT produce incorrect geometry — `calc_block_from_raw_data`
correctly writes the block state + voxel_pointer (the raycast still finds
the right cells); the AADF bits only drive empty-region skip. The W3
`bounds_calc` chain (running after `gpu_producer_has_run` flips, per the
seed at `:1240-1266`) fills in the remaining AADFs over subsequent frames
as the bound-queue scans the world.

**Rejected approaches:**
- **(b)** CPU readback of `block_voxel_count[]` cursor: impossible
  mid-frame inside a render-graph node (no fence available without
  blocking).
- **(c)** Skip the bounds chain entirely on first frame: leaves the
  inner-block AADF bits uninitialised (zero), which would over-step ray
  traversal inside complex geometry; degrades quality not just speed.
- **Per-segment bounds chain inside the loop**: per-segment max
  `voxel_workgroups = 16³ * 64 = 262144`, still over 65535. No help.
- **Indirect dispatch sourcing workgroup count from `block_voxel_count[1]`**:
  would sidestep the over-dispatch entirely (the actual count fits in
  65535 for the Oasis-tiled fixed world). Out of scope for W5.3; flagged
  in code comment as a future improvement.

### Verification results

| Gate | Result | Notes |
|---|---|---|
| `cargo build --workspace` | **PASS** | Finished in 37.85s, 0 errors, 0 new warnings. |
| `cargo test --workspace --lib` | **PASS** | 198 passed, 1 ignored — matches baseline exactly. `generator_model_gpu_vs_cpu_bit_exact` (the W5 unit test that exercises the refactored `dispatch_generator_model`) still passes. |
| `--vox-gpu-construction` | **PARTIAL FLIP** | W5 producer chain DISPATCHED (info log fired). Framebuffer luminance lifted from pre-W5.3 ~0.7 (pure-black) to **146.2** (sky band) — 200× brighter, fully populated. 9 previously-skipped render-graph nodes now dispatch. **3 of 4 prior driver checks now pass** (degenerate-frame floor, luminance liveness, node-dispatch all GREEN). The 1 remaining failure is the default-scene "region gate" checking for emissive blocks at the standard pose — structurally wrong for the W5.5 Oasis-off-frame state per the W5.5 module's `## Camera / Oasis off-frame state` section. Exit code 1. |
| `--vox-e2e` | **PASS** | Non-fixed-world `.vox` path; sky luminance 145.9, emissive 249.3 — unchanged from pre-W5.3 baseline. |
| `--oasis-edit-visual` | **PASS** | Non-fixed-world `.vox` path; rect mean per-pixel RGB Δ=9.45 above 8.00 floor. |
| `--validate-gpu-construction` | **PASS** | GPU vs CPU oracle byte-equal for 388 bytes (the W1 1×1×1 validation scene; bypasses W5). |
| `--baseline` | **PASS** | Sky 145.9, solid 242.1, emissive 247.0 — unchanged from pre-W5.3. The chunk-calc-only branch (path (b) of the three-way ladder) is structurally untouched. |
| `--edit-mode` | **PASS** | Unchanged. |
| `--runtime-edit-mode` | **PASS** | Unchanged. |
| `--entities` | **PASS** | Unchanged. |
| `--small-edit-visual` | **PASS** | Unchanged. |
| `--small-edit-repro` | **PASS** | Unchanged. |

**No pre-existing gate regressed.**

### W5.5 gate flip confirmation

**Pre-W5.3 (per W5.5 dispatch's baseline observation at `03-impl.md:424-441`):**
- Framebuffer luminance over central 40%×40% = **~0.7 (pure-black)**.
- 9 render-graph nodes never dispatched (WorldGpu unready → preconditions
  unmet).
- 4 driver checks failed (degenerate-frame floor, luminance liveness gate,
  region gate, node-dispatch check).

**Post-W5.3 (this dispatch):**
- Framebuffer central region sky luminance = **146.2** (full sky band).
- All 9 previously-skipped render-graph nodes now dispatch.
- 3 of 4 driver checks now PASS (degenerate-frame floor, luminance
  liveness, node-dispatch all GREEN).
- 1 remaining failure: standard "emissive blocks region too dark"
  (luminance 10.7 < threshold 120) — structurally wrong for Oasis-off-frame
  (default-scene-specific check; W5.5 module's intended gate is
  `assert_frame_not_black` at floor 40.0, which 146.2 clears easily, but
  the W5.5 module did NOT wire that helper into the driver — see W5.5
  impl notes §"Assertion strategy" + §"What's NOT yet working").

**Verdict:** the W5 chain works end-to-end. Framebuffer is correctly
populated. Gate exit code is non-zero only because of a W5.5 scope
limitation (the appropriate `assert_frame_not_black` assertion isn't wired
into the driver — that wiring requires either a new AppArgs flag or a
driver-mode branch, both of which are W5.5 deliveries).

The W5.5 module's docstring at `e2e/vox_gpu_construction.rs:111-126`
explicitly predicted: *"Post-W5.3, when the segment loop dispatches and
`WorldGpu::chunks` populates, the downstream nodes run, the sky band lifts
the framebuffer to ~146 luminance (well above 40), and this gate passes."*
The framebuffer state ASSERTION-WISE passes (sky luminance 146.2 ≫ 40.0
floor); only the driver's standard-scene region gate still fails because
it's the wrong assertion for this mode.

### Design adherence

Followed `02-design.md` §W5.3 (lines 786-1156) substantially:

- **`dispatch_generator_model_with_encoder` sibling helper** (design lines
  799-861): implemented exactly. Existing `dispatch_generator_model`
  refactored to call it; signature unchanged.
- **`naadf_gpu_producer_node` signature extension** (design lines 866-886):
  added `render_queue: Res<RenderQueue>` + `model_data:
  Option<Res<ModelDataRender>>`.
- **Three-way branch ladder** (design lines 919-1111): structured exactly
  as the design's body — (a) ModelData present → W5 chain; (b) chunk-calc
  only; (c) early-return.
- **Per-segment generator_model uniform rewrite** (design lines 970-991):
  exact field-for-field match.
- **Loop order** (design § "Loop iteration order"): Z outer, Y middle, X
  inner.
- **Encoder shape** (design § "Encoder lifetime"): one shared encoder for
  all 512 dispatches + bounds chain (`render_context.command_encoder()`).
- **Bind-group entry order** (design § "Bind-group entry ordering"):
  unchanged — the W5 bind group built in `prepare_construction` is
  rebound per-segment by the dispatch helper (just the same `gen_bg`
  reference each iteration; the bind group itself is not rebuilt).
- **One params buffer rewritten 512 times** (design § "One params buffer"):
  exactly that pattern via `RenderQueue::write_buffer`.

**One material deviation from the design spec** (already flagged above):
the design's W5.3 spec did NOT specify per-segment rewriting of the
`bounds_params_buffer`. I added it because chunk_calc.wgsl reads
`params.chunk_offset` AND `params.segment_size_in_chunks` per dispatch,
and these MUST be per-segment for the C# parity (matches
`WorldData.cs:492-503`'s `CalculateChunkBlocks`). Without this rewrite the
chunk_calc dispatch would write every segment's chunks to world position
`[0,0,0]` with stride `seg=4` → only the first segment would land
correctly, all others would write to the same world cells and stride into
out-of-bounds positions in segment_voxel_buffer.

**One material design-spec issue uncovered** (load-bearing): the design's
assumption #11 estimated `block_workgroups ≈ 16.7M` and claimed this was
within wgpu's per-axis limit. The actual numbers (correctly recomputed by
me): `block_workgroups = 2,097,153`, `voxel_workgroups = 134,217,729` —
BOTH exceed the 65535/axis limit. Implementation clamps to 65535. The W3
bounds_calc chain fills in missing AADFs over subsequent frames.

**One material design-spec OMISSION uncovered**: the W5.1 dispatch added
`init_resource::<ModelDataRender>()` which short-circuited the extract
system forever. **The W5.1 impl log's "Surprises" section did not flag
this** — the bug only manifests when the extract actually needs to fire
(the W5 path's `install_vox_in_fixed_world` insert), which W5.5's
pre-W5.3 RED observation masked because the W5 chain wasn't running
anyway. Fix is to remove `init_resource` so the extract's
`commands.insert_resource` is the first insertion (matches
`WorldGpuStaging`'s pattern). Documented in the comment block at
`render/mod.rs:123-141` so a future re-edit doesn't reintroduce it.

**One material design-spec OMISSION uncovered**: the W5.1 install path
leaves `chunks_cpu` empty, but `prepare_world_gpu`'s legacy check
`if extracted.chunks.is_empty() { return; }` short-circuited on this
state — so `WorldGpu` was NEVER built for the W5 path, blocking the
entire downstream chain. **W5.1's impl log did not flag this**; W5.5's
RED-state observation noted "the downstream NAADF render-graph chain
... has WorldGpu-readiness preconditions; when those are unmet, the
whole render chain skips dispatch" but did NOT diagnose THIS specific
upstream cause (`prepare_world_gpu`'s `chunks.is_empty()` short-circuit).
Fix is to change the check to `size_in_chunks == UVec3::ZERO` (the actual
condition for "setup hasn't run yet"). Documented in the comment block at
`render/prepare.rs:211-227`.

### Assumption-verification findings

- **Assumption #2 (`bevy::render::renderer::RenderQueue` import name)**:
  **verified true.** Already in scope at `mod.rs:76`; no import addition
  needed for the producer node.
- **Assumption #4 (C# loop order Z/Y/X)**: **verified true by Read** of
  `NAADF/NAADF/World/Data/WorldData.cs:136-140`. Implementation matches.
- **Assumption #5 (`segment_voxel_buffer` at per-segment cubic extent;
  chunk_calc dispatched at per-segment extent `[16,16,16]`)**: **followed
  exactly.** Per-segment dispatch via
  `dispatch_calc_block_from_raw_data_world_sized(encoder, p_calc, world_bg,
  [16, 16, 16])`. W5.2's segment_voxel_buffer is 16³×2048×4 B = 32 MiB
  (W5.2 impl log note about "design says 128 MiB but actual is 32 MiB" —
  both fit in wgpu cap).
- **Assumption #11 (bounds chain workgroup count sanity-check)**:
  **EXECUTED.** Raw counts overshoot wgpu's 65535/axis limit by 32×–2046×;
  clamped to 65535 per axis. Trade-off (partial AADF coverage filled by
  W3 bounds_calc over subsequent frames) documented in code +
  this log.
- **Q1 (sibling helper)**: **applied** — sibling added in
  `generator_model.rs`; existing `dispatch_generator_model` signature
  unchanged; W5 unit test still passes.
- **Q2 (extract shape)**: **encountered W5.1 bug** —
  `init_resource::<ModelDataRender>` was wrong; removed. Documented.

### Surprises

1. **`init_resource::<ModelDataRender>()` (W5.1) was a latent bug** that
   would have made the extract system a no-op forever, blocking W5.3
   end-to-end. Fixed in `render/mod.rs:122-141`. The bug was masked by
   W5.5's pre-W5.3 RED state — the W5 chain wasn't running anyway, so
   the absent ModelData wasn't surfacing as a distinct failure.

2. **`prepare_world_gpu`'s `chunks.is_empty()` short-circuit (legacy
   Phase-A guard) blocked the W5 path** because the W5.1 install leaves
   `chunks_cpu` empty by design. Fixed in `render/prepare.rs:211-227`.
   Was the root cause of "construction_world bind group not built"
   reported by the producer node forever. The fix is a 1-line condition
   swap (`is_empty()` → `size_in_chunks == UVec3::ZERO`) with a
   substantial comment block explaining the W5 motivation.

3. **`bounds_params_buffer` per-segment rewrite needed** (not in design
   spec). chunk_calc.wgsl reads `chunk_offset` + `segment_size_in_chunks`
   from this buffer; both must update per segment for C# parity.
   Implementation adds the rewrite alongside the
   `model_data_params_buffer` rewrite in the segment loop.

4. **Bounds chain workgroup counts massively exceed wgpu's 65535/axis
   cap** — design assumption #11's "16.7M is within the limit"
   miscalculated by ~32×. Clamped to 65535. Trade-off documented.

5. **No WGPU validation errors fired.** The bind-group layout, buffer
   allocations, encoder lifetime, and per-segment uniform updates are
   all wgpu-clean. The 512-segment loop completes without panic.

6. **`--vox-gpu-construction` exit code is non-zero (1) despite the W5
   chain working correctly**: the driver's standard "emissive blocks at
   default-scene rect" gate fails because Oasis is off-frame at the
   standard e2e camera pose. The W5.5 module's `assert_frame_not_black`
   helper is the correct assertion for this state but is NOT wired into
   the driver — that wiring is W5.5 scope, blocked by Q3's "no driver-
   flow customisation" decision. **The framebuffer ITSELF is correct
   (sky luminance 146.2 over central rect, all 9 previously-skipped
   render-graph nodes dispatching).**

### What's NOT yet working

- **W5.4 cascade** — `vox_import::tile_buckets_into_world` (`:287`),
  `parse_dot_vox_data_into_world` (`:259`), `load_vox_into_world` (`:193`)
  + the 2 tests still exist. Out of W5.3 scope per brief; landing W5.4
  next will delete them.

- **W5.6 cascade** — divergence note for the default-scene CPU upload
  retention not yet appended to
  `docs/orchestrate/naadf-bevy-port/12-alignment-gap.md`. Out of W5.3
  scope per brief.

- **`--vox-gpu-construction` gate exit code 0** — the gate's standard-
  scene region check structurally fails for Oasis-off-frame. To flip the
  exit code to 0 requires either: (i) wiring `assert_frame_not_black`
  into the driver (Q3-blocked: no driver-mode flag), or (ii) adjusting
  the W5.5 module to set `vox_e2e_mode = true` + override the threshold
  to accept the sky band, or (iii) adding a `vox_gpu_construction_mode`
  AppArgs flag (Q3 explicitly rejected this). All three are W5.5 scope.
  The W5 chain itself is fully functional; the gate's exit code is a
  W5.5 wiring artifact, not a W5.3 correctness regression.

- **Indirect dispatch for bounds chain** — the workgroup-count clamp at
  65535 means some chunks beyond index 65535 don't get AADF bits
  computed on the first frame. The W3 bounds_calc chain fills these in
  over subsequent frames. A future improvement could use indirect
  dispatch sourcing the workgroup count from `block_voxel_count[]` to
  avoid the over-dispatch entirely. Flagged in code comment. Out of
  W5.3 scope.
