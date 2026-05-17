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
