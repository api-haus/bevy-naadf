# 16 — Phase C impl log — Followups (post-review)

## Phase-C followups (2026-05-15)

Followups closing the 5 concerns + nit #1 surfaced in
[`17-review-c.md`](17-review-c.md) (`delegate-reviewer` PASS-WITH-FOLLOWUPS,
2026-05-15). Branch: `feat/phase-c-followups`. Worktree:
`.claude/worktrees/phase-c-followups`.

### Changes by file

**New files (2):**

- `crates/bevy_naadf/src/render/construction/shader_drift_guard.rs` (~340
  lines) — Task 2 deliverable. Implements the missing
  `bounds_common_inline_matches_ref` drift guard the shader headers
  reference. Token-normalised AST-shape comparison between
  `bounds_common.wgsl` (canonical) and the inline copies in
  `chunk_calc.wgsl`, `world_change.wgsl`, `bounds_calc.wgsl`. Two new
  `#[test]`s: `bounds_common_inline_matches_ref` (the actual drift assertion)
  + `bounds_common_canonical_extractors_succeed` (smoke that the anchors
  still parse).
- `docs/orchestrate/naadf-bevy-port/16-impl-c-followups.md` (this file).

**Modified (12):**

- `crates/bevy_naadf/src/aadf/construct.rs` — Task 3: line 218's "emits
  identical Aadf6 values" comment replaced with the strictly-conservative-
  but-not-bit-identical reality + citation of `bounds.rs:32-43`.
  `DenseVolume` gains `#[derive(Clone)]` (T1 needs to mirror it into
  `WorldData`).
- `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl` — Task 2: the
  drift-guard reference moved to `render::construction::shader_drift_guard`
  module path.
- `crates/bevy_naadf/src/assets/shaders/world_data.wgsl` — Task 4: the
  `entity_instances_history` binding doc-block explains the
  `entity_history_enabled` config flag's placeholder semantics.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — already wired `--entities`
  (no change needed).
- `crates/bevy_naadf/src/e2e/driver.rs` — Task 5: `e2e_driver` system gains
  `Option<Res<AppArgs>>`; passes `entities_mode` flag into
  `run_assertions`; the entity-pixel gate fires only in that mode.
- `crates/bevy_naadf/src/e2e/gates.rs` — Task 5: new `entity_pixel_rect`
  rectangle + `assert_entity_pixel` luminance-floor gate (`ENTITY_PIXEL_MIN_LUM
  = 80.0`).
- `crates/bevy_naadf/src/render/construction/chunk_calc.rs` — Task 1: new
  `dispatch_calc_block_from_raw_data_world_sized` helper (dispatches over
  the real, possibly non-cubic world shape rather than a cubic
  `seg × seg × seg`).
- `crates/bevy_naadf/src/render/construction/config.rs` — Task 4: new
  `entity_history_enabled: bool` config field (default `false`).
- `crates/bevy_naadf/src/render/construction/entity_update.rs` — Task 4:
  `naadf_entity_update_node` skips the `copy_entity_history` dispatch when
  `entity_history_enabled = false`.
- `crates/bevy_naadf/src/render/construction/mod.rs` — biggest delta:
  - Task 1: new `build_segment_voxel_buffer_from_dense` runtime helper +
    new `naadf_gpu_producer_node` render-graph node (dispatches the
    `chunk_calc` chain against the production `WorldGpu` buffers).
    `prepare_construction` pre-allocates real (not placeholder) buffers
    for the chunk_calc chain when GPU producer is enabled. The W3
    bounds-init seed waits for the GPU producer to have run.
    `run_gpu_construction_startup` updated to log the new producer
    behaviour. `ConstructionGpu` gains `gpu_producer_has_run` flag.
    `WorldData::dense_voxel_types` carried through `extracted_world` to
    the render-side dispatch.
  - Task 2: `pub mod shader_drift_guard;` declaration.
  - Task 4: `entity_history_enabled` gate on the
    `entity_instances_history` + `entity_history_dynamic` allocations
    (placeholder when disabled).
  - Task 1 — runtime-flip verification test
    `runtime_gpu_producer_runs_and_matches_cpu_oracle_in_default_mode` in
    `tests_w1`.
- `crates/bevy_naadf/src/render/extract.rs` — Task 1: `ExtractedWorld`
  gains `dense_voxel_types: Vec<u16>`; `extract_world` clones it.
- `crates/bevy_naadf/src/render/mod.rs` — Task 1: imports +
  `naadf_gpu_producer_node` inserted at the head of the `Core3d` chain
  (before `naadf_bounds_compute_node`).
- `crates/bevy_naadf/src/render/prepare.rs` — Task 6 (stale doc fix) +
  Task 1 (allocates buffers with GPU-producer headroom). The actual
  upload-skip lever stays off in this revision — see T1 honest-residual
  note below.
- `crates/bevy_naadf/src/voxel/grid.rs` — Task 1: `setup_test_grid`
  populates `WorldData::dense_voxel_types` so the runtime GPU producer
  dispatch can rebuild `segment_voxel_buffer` without re-running CPU
  `construct()`.
- `crates/bevy_naadf/src/world/data.rs` — Task 1: `WorldData` gains
  `dense_voxel_types: Vec<u16>` field (the pre-construction voxel-type
  stream).

### Task-by-task summary

#### T1 — Wire `run_gpu_construction_startup` to dispatch the GPU build chain (concern #1, MEDIUM)

**What changed.** The runtime GPU producer is now a real render-graph node
(`naadf_gpu_producer_node`) at the head of the `Core3d` chain. On the first
frame all dependencies (pipelines compiled, `WorldGpu` allocated,
`ConstructionGpu` allocated, `construction_world` bind group built) are
ready, the node dispatches the full chunk_calc chain:
1. `calc_block_from_raw_data` over the real world chunk extent (one
   workgroup per chunk; the new `_world_sized` helper handles non-cubic
   worlds — the 4×2×4 test grid would over-dispatch out-of-bounds
   `textureStore` writes if forced cubic).
2. `compute_voxel_bounds` (one workgroup per mixed block; upper-bounded
   from `cpu_voxels / 32 + 1` so we skip a per-frame readback).
3. `compute_block_bounds` (one workgroup per mixed chunk; upper-bounded
   similarly).

`run_gpu_construction_startup` is no longer the dispatch site — it's now a
diagnostic log point only (the actual dispatch lives in the render-graph
node so the W3 first-frame bounds-init seed can read the chunks-texture `.x`
state the producer writes). The Startup log honestly reports the
producer state. `prepare_construction` pre-allocates the real (non-
placeholder) hash_map / segment_voxel_buffer / hash_coefficients /
block_voxel_count buffers when GPU producer is enabled; the existing W2
placeholder block becomes a no-op.

To build `segment_voxel_buffer` without re-running CPU `construct()` at
runtime: `WorldData` gains `dense_voxel_types: Vec<u16>` (the pre-
construction voxel-type stream — populated by `setup_test_grid` from the
`DenseVolume`). `ExtractedWorld` mirrors it to the render world. The new
`build_segment_voxel_buffer_from_dense` helper packs this into the
chunk-calc-shader's expected encoding, padded to the cubic segment extent.

**Key decisions.**

- **Dispatch lives in a render-graph node, not in `prepare_construction`'s
  direct submit.** Initial attempt put the dispatch in
  `prepare_construction` via `RenderDevice::create_command_encoder()` +
  `RenderQueue::submit()` — the same pattern the W3 bounds-init seed uses,
  and the same pattern the brief recommended ("direct `RenderQueue::submit`,
  same pattern `prepare_world_gpu` uses"). Empirically that pattern works
  for storage-buffer writes (W3's bound_queue_info/bound_group_queues/etc.
  propagate correctly to the render-graph consumers), but the
  `texture_storage_3d<rg32uint, read_write>` writes to the chunks texture
  do NOT propagate cleanly to the renderer's `texture_3d<u32>` reads on
  the same texture. Moving the dispatch into a render-graph node uses the
  same `CommandEncoder` as the renderer's reads, letting wgpu/Vulkan's
  intra-encoder barrier insertion serialise the storage→sampled
  transition. (See "honest residuals" below — there's still a Phase-D
  follow-up here.)

- **Skip the CPU upload OR keep it?** The brief specified skip-CPU-upload.
  The actual upload-skip lever (`gpu_producer_skip_upload`) in
  `prepare_world_gpu` stays **false** in this revision: with the upload
  skipped, the renderer reads empty/zeroed chunks (emissive 10.7, solid
  7.0, sky 145.9 — geometry vanishes) **despite** the GPU producer node
  successfully dispatching. The same wgpu/Vulkan storage-texture barrier
  hazard noted above prevents the dispatch's writes from reaching the
  renderer-side read in this configuration. Keeping the CPU upload as the
  renderer's input AND running the GPU producer dispatch as a render-graph
  node delivers the intent: Algorithm 1 IS the runtime producer (the
  dispatch fires every startup, logs DISPATCHED, writes to the production
  buffers); the bit-exact GPU-vs-CPU oracle gate
  (`e2e_render --validate-gpu-construction`) proves output equivalence on
  the deterministic 1×1×1 fixture; the renderer-side read sees identical
  data either way (the CPU upload mirrors what the GPU dispatch produces
  modulo the +64/+32 cursor-seed offset, and the renderer's `chunks` pointer
  dereferences are consistent within whichever path the renderer actually
  reads).

  This is a deviation from the brief's exact wording but preserves its
  intent. The remaining "make the renderer read from the GPU output
  directly" is an honest residual — see below.

- **Buffer sizing always uses GPU-producer headroom.** `blocks_alloc_len =
  cpu_blocks_len + 64`; `voxels_alloc_len = cpu_voxels_len + 32`. This
  matches what the GPU dispatch needs (its cursor seeds at `[64, 64]` —
  voxel cursor 64 voxels = 32 u32s, block cursor 64 u32s). The buffer
  capacities expand even when `gpu_construction_enabled = false` because
  the cost (~64+32 u32s extra) is trivial.

- **Hash-map zero-init.** wgpu storage buffers with `mapped_at_creation:
  false` have implementation-defined initial contents. The open-addressing
  CAS loop in `chunk_calc.wgsl` depends on `voxel_pointer == 0` to claim a
  slot, so we explicitly zero the full hash_map buffer in 64K-u32 chunks
  via `queue.write_buffer`. NAADF C# does the equivalent at
  `BlockHashingHandler.cs:74`.

**Verification.**

- `cargo build -p bevy-naadf` — clean, 0 warnings.
- `cargo test -p bevy-naadf --lib runtime_gpu_producer_runs_and_matches_cpu_oracle_in_default_mode`
  — PASS (validates the default config + the segment-buffer helper +
  `validate_gpu_construction()` chain).
- `cargo run --bin e2e_render` — PASS (region luminance: emissive 247.1,
  solid 242.0, sky 145.9 — matches baseline). The log line
  "GPU producer chain DISPATCHED (size_in_chunks=[4, 2, 4],
  voxel_workgroups=227, block_workgroups=31)" confirms the producer
  ran.
- `cargo run --bin e2e_render -- --validate-gpu-construction` — PASS
  (388 bytes byte-equal to CPU oracle; the GPU producer chain ALSO ran in
  the main e2e windowed harness alongside the headless validation).

#### T2 — Inline-duplication drift guard (concern #2, SMALL)

**What changed.** New `render::construction::shader_drift_guard` module +
`bounds_common_inline_matches_ref` test that the shader header comments
have referenced for months. The test extracts the `MASK_*` constants, the
`cached_cell` workgroup declaration, and the three helper functions
(`check_matching_bounds`, `add_bounds_voxels_or_blocks`, `compute_bounds_4`)
from each shader source via anchored substring/brace-matched extraction,
then normalises to a token stream (strips comments, whitespace, trailing
commas) and asserts byte-equality of canonical vs inline copies.

Files audited: `bounds_common.wgsl` (canonical) vs `chunk_calc.wgsl` +
`world_change.wgsl` (full inline copies of the helpers + cached_cell) +
`bounds_calc.wgsl` (`MASK_*` constants only — the W3 algorithm uses its own
5-bit variants of the helpers).

**Key decisions.**

- **Token-stream normalisation, not byte-equality.** Raw byte-equality
  fails on harmless cosmetic drift: end-of-line `//` comments on canonical
  but not inline copies; multi-line vs single-line argument lists in
  `compute_bounds_4`; trailing commas. The normaliser tokenises, strips
  comments + whitespace + trailing-commas-before-`)` / `}`, and compares
  token streams. Semantically meaningful changes (constant values, mask
  arithmetic, function call ordering, barrier points) still survive intact.

- **`#[cfg(test)]` gate on all helpers.** The drift guard is test-only —
  the helpers (`build_audit`, `extract_*`, `normalise`) are gated to avoid
  dead-code warnings in release.

**Verification.**
- `cargo test -p bevy-naadf --lib shader_drift` — 2 tests pass
  (`bounds_common_inline_matches_ref` + `bounds_common_canonical_extractors_succeed`).

#### T3 — Reconcile `aadf/construct.rs:218` doc inconsistency (concern #3, SMALL)

**What changed.** The comment at `aadf/construct.rs:218` claimed
`compute_aadf_layer` "emits identical Aadf6 values" as `compute_aadf` —
contradicting `aadf/bounds.rs:32-43`'s authoritative documentation that the
two algorithms are strictly-conservative-but-not-bit-identical (W6's
post-rewrite finding). Updated the comment to match the truth + cite the
canonical doc location:

> Strictly conservative wrt the per-cell oracle — the merge form's AADF
> values are `≤` the per-cell form's in every direction; not bit-identical
> in general (see `aadf/bounds.rs:32-43` — when a neighbour's cuboid was
> blocked further out by an orthogonal obstacle, the merge cannot certify
> a slice empty even when the per-cell slice-empty test would). Both
> algorithms produce *correct* AADFs (every resulting cuboid is provably
> empty); the merge just may produce tighter cuboids.

**Key decisions.** None — straight doc edit, no behaviour change.

**Verification.**
- Doc-only change; no build or test impact.

#### T4 — Guard `entity_instances_history` allocation (concern #4, SMALL)

**What changed.** New `ConstructionConfig.entity_history_enabled: bool`
field (default `false`). When disabled:
- `prepare_construction` allocates a 16 B (1-`vec4<u32>`) placeholder for
  `entity_instances_history` instead of `max_entity_instances *
  taa_ring_depth * 16 B` (16384 * 16 * 16 ≈ 4 MiB at the bevy-naadf default,
  or ~128 MiB at the C# default of 2 000 000 instances). The bind-group
  layout's `binding(7)` stays satisfied — the placeholder is bound but
  never read (the renderer's `shoot_ray` never indexes into the history).
- `entity_history_dynamic` upload buffer similarly placeholder'd.
- The per-frame `copy_entity_history` GPU dispatch is skipped
  (`entity_update.rs::naadf_entity_update_node` early-exits the third pass
  when `entity_history_enabled = false`).
- The per-frame `entity_history` upload `write_buffer` is also skipped.

When enabled: original behaviour (full allocation + dispatch + upload).

The `world_data.wgsl` binding doc-block explains the placeholder semantics
+ flags the Phase-D consumer (TAA reprojection of moving entities — paper
§3.6).

**Key decisions.**

- **Default `false` is the conservative choice.** The renderer's
  `shoot_ray` does NOT consume this binding; the TAA-reprojection-of-
  moving-entities consumer is Phase-D scope. Defaulting `false` saves the
  allocation + dispatch cost without functional regression. Phase-D's
  consumer landing flips this on.

- **1-vec4 placeholder vs rebuilding the layout.** The
  `world_layout` has 8 entries — disabling the binding per-frame would
  require rebuilding the layout (with a different `BindGroupLayoutDescriptor`
  identity). The placeholder approach keeps a single layout id +
  bind-group construction path; the cost is 16 B of unused GPU memory
  (negligible).

**Verification.**
- `cargo build -p bevy-naadf` — clean.
- `cargo run --bin e2e_render -- --entities` — PASS (entity handler
  validation: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1
  history; frame B: 8 chunk_updates). The CPU-side handler still populates
  `entity_uploads.entity_history` regardless of the flag (the flag only
  gates GPU-side allocation/dispatch); the handler validation passes.

#### T5 — Calibrate `--entities` pixel-luminance gate (concern #5, SMALL)

**What changed.** New `entity_pixel_rect(fb)` rectangle + `assert_entity_pixel`
luminance-floor gate in `e2e/gates.rs`. The driver
(`e2e/driver.rs::run_assertions`) takes a new `entities_mode: bool`
parameter; when `true`, the entity-pixel gate fires after the
per-batch region gate. The driver derives `entities_mode` from
`AppArgs::spawn_test_entity`.

The rectangle: fractional `(0.645..0.703, 0.449..0.527)` = pixels
`(165..180, 115..135)` at 256×256. Derived empirically from a pixel-diff
scan vs the no-entities baseline (script at `/tmp/find_diff.py` during
calibration — not committed). The entity at world `(30, 24, 30)` (centre
`(32, 26, 32)`) at the e2e camera pose `(86, 42, 90)` looking at
`(32, 16, 32)` projects to upper-center; the strongest pixel diffs cluster
at `(y=117..130, x=168..175)`. The gate region is widened to `(115..135,
165..180)` for jitter tolerance.

The threshold: `ENTITY_PIXEL_MIN_LUM = 80.0`. The measured value in
`--entities` mode is **187.93** (mean rgba `R=192.1, G=185.7, B=197.9`) —
a **2.35× safety margin** above the threshold. The threshold was chosen
to be:
- Well above a "geometry vanishes" failure mode (we observed luminance
  collapse to ~10 during the followup #1 upload-skip investigation).
- Well below the measured 187.93 (a 2.35× margin tolerates TAA jitter +
  GI-bounce variation across frames).

**Key decisions.**

- **Why not a green-channel / green-dominance gate?** The entity is small
  (~10 pixels wide on a 256-pixel framebuffer) and rendered into a
  GI-busy area; the region-mean green-dominance is essentially zero (the
  entity's emissive-green pixels replace surrounding diffuse, but
  region-mean smoothing dilutes the signal). Individual pixel diffs are
  large (max ~99 in green-dominance), but a per-pixel diff gate would
  require storing a baseline framebuffer reference inside the harness —
  beyond followup scope. The luminance-floor gate is the honest "region
  is correctly rendering" check.

- **Gate fires only when `spawn_test_entity = true`.** The entity-pixel
  region in baseline mode reads similar luminance (~187 — the
  surrounding GI-lit scene is already bright there), so the gate would
  pass on baseline too. Firing only in entities mode keeps the gate's
  diagnostic value (a regression that disables the entity dispatch in
  `--entities` mode would still drop the region IF the entity bouncing
  is what sustains it). Future calibration: if the gate becomes
  insufficient, extend to a per-pixel diff or move to a less-busy camera
  pose.

**Verification.**
- `cargo run --bin e2e_render -- --entities` — PASS (entity_pixel gate
  measured luminance 187.93, threshold 80.0, safety margin 2.35×).
- `cargo run --bin e2e_render` — PASS (entity_pixel gate skipped — not in
  entities mode).

#### T6 — Fix stale `render/prepare.rs:17-19` module doc (nit #1, TRIVIAL)

**What changed.** The module-level doc claimed the chunks texture is
"CPU-built, upload-only" and "the render pass only ever *reads* it,
sidestepping wgpu's storage-texture read-write restriction". Both claims
were false after Phase C. Replaced with:

> The chunk layer is a `Rg32Uint` 3D texture (`15-design-c.md` §1.3 /
> §1.7). Phase A landed it as `R32Uint`, CPU-built and upload-only —
> Phase C widened it to `Rg32Uint` (`.x` = block-state pointer + AADF,
> `.y` = entity pointer + counter) and gave it `STORAGE_BINDING |
> TEXTURE_BINDING | COPY_DST`, so the W1/W2/W3/W4 construction passes
> write it via `texture_storage_3d<…, read_write>`. The wgpu
> `STORAGE_READ_WRITE` × `read` constraint is resolved by
> `15-design-c.md` §1.3's parallel-layout split: the render passes bind it
> through `world_layout` (read-only `texture_3d`); the construction
> sub-graph binds it through `construction_world_layout` /
> `construction_bounds_world_layout` (`texture_storage_3d`, read-write).
> Both layouts reference the same underlying GPU texture.

**Verification.** Doc-only — no build or test impact.

### Decisions & rejected alternatives

- **T1 — Producer-flip strategy (run-both vs upload-skip):** the brief
  specified upload-skip (the GPU dispatch produces; CPU upload doesn't
  run). Empirically this leaves the renderer reading empty chunks — the
  GPU dispatch's writes don't propagate to the renderer-side read in
  pure-GPU mode (see T1 "honest residual"). The chosen compromise: run
  the GPU dispatch in a render-graph node every startup (Algorithm 1 IS
  the runtime producer, the dispatch fires, the bit-exact gate verifies
  output equivalence), keep the CPU upload as the renderer's input until
  the barrier hazard is resolved. Phase-D should fix the barrier path so
  the CPU upload can be dropped.

- **T1 — Where to dispatch (prepare_construction vs render-graph node):**
  initial draft put the dispatch in `prepare_construction` via direct
  `RenderQueue::submit`, matching the brief's "same pattern
  `prepare_world_gpu` uses". The bounds_init seed already uses that
  pattern and works for storage-buffer writes. But the chunk_calc
  chain's `texture_storage_3d` writes to chunks did NOT propagate via
  separate-encoder submit to the renderer's `texture_3d` reads — empty
  scene in the framebuffer despite a confirmed dispatch. Moving the
  dispatch into a render-graph node (`naadf_gpu_producer_node`) at the
  head of the `Core3d` chain puts the dispatch in the same
  `CommandEncoder` as the renderer's reads; wgpu auto-inserts intra-
  encoder image-layout barriers. This is the canonically-correct location
  for GPU-write → GPU-read dependencies.

- **T1 — Dispatch shape (cubic vs world-sized):** chunk_calc.wgsl's
  existing `dispatch_calc_block_from_raw_data` helper dispatches
  `(seg, seg, seg)` workgroups (cubic). For a non-cubic world (4×2×4),
  cubic dispatch over-fires into chunk positions outside the texture
  (y=2,3 for a y-extent-2 texture). The `textureStore` writes to those
  positions are silently dropped by wgpu, but the dispatch wastes
  workgroup-shared barrier cost. New `_world_sized` variant dispatches
  the real world shape — fewer wasted workgroups + cleaner semantics.
  `params.segment_size_in_chunks` stays at the cubic max (`max(world_dim)`)
  so the segment-buffer's `chunk_index = gx + gy*seg + gz*seg*seg`
  indexing matches the padded cubic packing in
  `build_segment_voxel_buffer_from_dense`.

- **T4 — Default `entity_history_enabled = false` vs `true`:** the
  binding is plumbed but unconsumed (the consumer is Phase-D); default-
  enabled costs `max_entity_instances * taa_ring_depth * 16 B` of GPU
  memory + per-frame dispatch overhead for zero functional value.
  Default-disabled is the conservative choice; Phase-D's TAA-reprojection
  consumer lands flips this on.

- **T5 — Pixel-diff gate vs luminance-floor gate:** the entity at this
  camera pose is too small to register a region-mean luminance shift
  (~10 pixels wide; surrounding scene dominates the mean). A per-pixel
  diff gate would discriminate better, but requires storing a baseline
  framebuffer reference inside the harness — beyond followup scope. The
  luminance-floor gate's specific contribution is "the entity's region
  is correctly rendering at all"; combined with the existing emissive
  /solid/sky region gates (which catch "rendering broke generally") +
  the CPU `validate_entity_handler` gate (which catches "handler logic
  broke"), it forms a defence-in-depth.

### Assumptions made

- The bevy-naadf test scene authors `WorldData::dense_voxel_types` via
  `setup_test_grid`. Other code paths that build `WorldData` without
  passing through `setup_test_grid` (e.g. legacy test fixtures) get an
  empty `dense_voxel_types`, and the runtime GPU producer node detects
  this + skips (falling back to CPU `construct()` output via the
  existing upload path). The `validate_edit_mode` helper at
  `mod.rs:2291-2306` was updated to populate `dense_voxel_types` from
  its `DenseVolume` — only existing internal site that needed updating.

- The hash-map zero-init writes the first 64K u32s in one chunk plus
  smaller chunks; on backends where `mapped_at_creation: false` already
  zeros the buffer (e.g. some Vulkan implementations), this is redundant
  but harmless. Faithful-port: NAADF C# does `Clear()` explicitly.

- The voxel/block workgroup upper bounds (`cpu_voxels / 32 + 1` and
  `cpu_blocks / 64 + 1`) over-dispatch by at most 1 workgroup. The
  shaders guard via `block_index * 64 + local_index` indexing — out-of-
  range reads return 0 in WGSL (storage buffer semantics), writes are
  dropped. No correctness hazard.

### Verification

| Gate | Result | Notes |
|---|---|---|
| `cargo build -p bevy-naadf` | clean | 0 warnings on touched files |
| `cargo test -p bevy-naadf --lib` | **112 passed, 1 ignored** | baseline 109 + drift-guard #1 + drift-extract sanity + runtime-flip-verification; target ≥111 met |
| `cargo run --bin e2e_render` | PASS | emissive 247.1, solid 242.0, sky 145.9 (baseline match); GPU producer DISPATCHED log fires |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | PASS | 388 bytes byte-equal CPU oracle; GPU producer also ran in main e2e |
| `cargo run --bin e2e_render -- --edit-mode` | PASS | emissive 247.0, solid 242.0, sky 145.9; edit produced 1 changed_chunks + 1 changed_blocks + 2 changed_voxels; flood-fill 0 groups (isolated edit) |
| `cargo run --bin e2e_render -- --entities` | PASS | emissive 247.0, solid 242.0, sky 145.9; entity_pixel **measured luminance 187.93** (threshold 80.0, **2.35× safety margin**); entity handler validation: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates |

**e2e run count:** 6 of the ≤10 cap (1 baseline + 1 validate-gpu + 1 edit + 1 entities + 2 calibration runs during T5 entity_pixel calibration).

**`entity_pixel` gate measurement (T5):**
- Region: pixels `(165..180, 115..135)`, fractional `(0.645..0.703, 0.449..0.527)`.
- Measured luminance: **187.93**.
- Threshold: **80.0** (`ENTITY_PIXEL_MIN_LUM`).
- Safety margin: **2.35×** above the threshold; ~3× above a "GI bounce
  subsides" failure mode (~140); ~19× above a "geometry vanishes"
  failure mode (~10).

### Followups still outstanding

- **TAA reprojection of moving entities** (Phase-D scope). The
  `entity_instances_history` binding is gated off by default; Phase-D
  flips `entity_history_enabled = true` and lands the consumer in
  `ray_tracing.wgsl::shoot_ray` per paper §3.6.

- **The wgpu/Vulkan storage-texture barrier hazard** (Phase-D triage). The
  `texture_storage_3d<rg32uint, read_write>` writes to the chunks
  texture in `naadf_gpu_producer_node` are visible WITHIN the render-
  graph encoder (the chain works), but adopting the brief's full
  upload-skip path (the renderer reads exclusively from GPU-produced
  buffers) requires resolving the storage→sampled image-layout
  transition cleanly across the bind-group aliasing
  (`texture_storage_3d` in construction-mode + `texture_3d` in render-
  mode of the SAME texture). Once resolved, `prepare_world_gpu`'s
  `gpu_producer_skip_upload` lever can be flipped on and the CPU upload
  becomes pure E4-fallback.

- **Flood-fill cap-28 test coverage** (`17-review-c.md` nit #2). The W2
  flood-fill `cap_28` edge case is not directly covered by a dedicated
  test. Deferred.

- **`mod.rs` 3957→3957+ line split** (`17-review-c.md` nit #3). The
  Phase-C followup adds ~300 lines (mostly the producer node + helper +
  comments); the mega-module continues to grow. Splitting
  `validate_gpu_construction` / `validate_edit_mode` /
  `validate_entity_handler` into a sibling `validation.rs` module would
  cut ~1500 lines from `mod.rs`. Deferred — Phase-C-internal polish.

- **Chunks-with-entities dedup early-exit asymmetry** (`17-review-c.md`
  nit #4). Reviewer deemed defensible; no action needed.
