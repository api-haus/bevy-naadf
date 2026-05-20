# D5 — GPU construction (Phase-C) exploration

**Author**: refactor-explorer (delegate orchestration — codebase-tightening).
**Date**: 2026-05-20.
**Scope**: `crates/bevy_naadf/src/render/construction/**` (16 062 Rust LOC) +
the 7 construction WGSL files in `crates/bevy_naadf/src/assets/shaders/`
(2 343 WGSL LOC) ≈ **18 405 LOC** — the single biggest refactor target in the
port (audit side-note #7).
**Discipline**: read-only investigation; no code edits; W0 seam contract
respected (`render/gpu_types.rs`, `render/prepare.rs`,
`render/pipelines.rs::NaadfPipelines` treated as read-only — D4's territory).

---

## Verified scope inventory

Submodule line counts (`wc -l render/construction/*.rs`):

```
  619 bounds_calc.rs
  391 change_handler.rs
  314 chunk_calc.rs
  326 config.rs
  441 entity_handler.rs
  401 entity_update.rs
  303 generator_model.rs
  241 hashing.rs
  177 map_copy.rs
11043 mod.rs              ← 70 % of D5's Rust LOC
  400 shader_drift_guard.rs
 1165 world_change.rs
─────
15821 total submodules
```

WGSL files (`wc -l assets/shaders/{chunk,bounds,world,entity,generator,map_copy,bounds_common}.wgsl`):

```
  577 chunk_calc.wgsl
  572 bounds_calc.wgsl
  579 world_change.wgsl
  191 bounds_common.wgsl
  137 entity_update.wgsl
  160 generator_model.wgsl
  127 map_copy.wgsl
─────
 2343 total WGSL
```

`mod.rs` top-level item map (verified `grep -nE "^pub fn |^fn |^pub struct |^pub enum |^impl |^mod "`):

| line | item | LOC est. | category |
|---:|---|---:|---|
| 106 | `pub struct ConstructionGpu` | ~210 | **W0 seam resource** (28 `Option<…>` fields) |
| 352 | `pub enum ReadbackStage` | ~16 | **W0 seam resource** (cpu mirror state machine) |
| 376 | `pub struct CpuMirrorReadback` | ~30 | **W0 seam resource** |
| 414 | `pub struct ConstructionBindGroups` | ~45 | **W0 seam resource** (8 `Option<BindGroup>` fields) |
| 482 | `pub struct ConstructionPipelines` | ~90 | **W0 seam resource** (25 `pub` fields) |
| 573 | `impl FromWorld for ConstructionPipelines` | ~170 | **registry build-out** |
| 755 | `pub struct ConstructionEvents` + `impl` | ~60 | **W0 seam resource** (per-frame edit batch) |
| 836 | `pub fn clear_world_data_pending_edits` | 4 | **no-op stub** (see Finding 4) |
| 855 | `pub struct MainWorldEntities` | ~25 | **W0 seam resource** |
| 883 | `pub struct RenderWorldEntityState` + `Default` | ~25 | **W0 seam resource** |
| 910 | `pub fn extract_world_changes` | 180 | **Extract system body** (W2 + W4 fan-in) |
| 1092 | `pub fn populate_cpu_mirror_from_gpu_producer` | 566 | **GPU→CPU readback state machine** |
| 1658 | `pub fn prepare_construction` | 1 418 | **the monster Prepare system** (Finding 1) |
| 3076 | `pub fn naadf_gpu_producer_node` | 434 | **render-graph node** (W5 producer chain) |
| 3510 | `pub fn run_gpu_construction_startup` | 17 | **Startup log-only stub** |
| 3547 | `pub struct ConstructionPlugin` | 2 | wiring tag |
| 3559 | **`pub struct AadfDelayedProbe`** | ~280 | **DIAG — DELETE** (Finding 2) |
| 3594 | **`pub fn aadf_delayed_probe`** | (inside above) | **DIAG — DELETE** |
| 3873 | **`pub struct AadfPerCallProbe` + `enum PerCallProbeStage`** | ~170 | **DIAG — DELETE** |
| 3915 | **`pub fn aadf_per_call_probe`** | (inside above) | **DIAG — DELETE** |
| 4088 | **`pub struct AadfCpuGpuParity` + `enum CpuGpuParityStage`** | ~485 | **DIAG — DELETE** |
| 4133 | **`pub fn aadf_cpu_gpu_parity`** | (inside above) | **DIAG — DELETE** |
| 4597 | **`pub fn aadf_cpu_gpu_parity_maybe`** | 20 | **DIAG wrapper — DELETE** |
| 4618 | `impl Plugin for ConstructionPlugin` | 93 | wiring (3 of its 4 `add_systems` are DIAG — see Finding 2) |
| 4727 | `pub fn build_segment_voxel_buffer_from_dense` | 92 | **W5 production CPU helper** (production caller — `prepare_construction:1894`) |
| 4820 | `pub fn build_segment_voxel_buffer` | 54 | **test-only CPU helper** (only `validate_gpu_construction` + `mod tests`) (Finding 6) |
| 4874 | `fn voxel_at_block_local` | 54 | test-only helper |
| 4928 | `pub fn validate_gpu_construction` | 362 | **E2E GATE — MOVE OUT** (Finding 3) |
| 5290 | `pub fn validate_gpu_construction_scaled` | 191 | **E2E GATE — MOVE OUT** |
| 5481 | `fn discover_populated_oasis_voxels` | 140 | E2E gate helper |
| 5621 | `pub fn validate_gpu_construction_production_scale` | 714 | **E2E GATE — MOVE OUT** |
| 6335..6622 | `impl VoxelReadback`, `readback_cursor`, `map_single_u32`, `map_single_pair`, `sample_voxel_readback`, `render_results_table` | ~290 | E2E gate helpers (used by `…_production_scale`) |
| 6623 | `fn run_one_fixture_byte_diff` | 511 | E2E gate helper |
| 7134 | `fn run_one_fixture_multiseg_byte_diff` | 472 | E2E gate helper |
| 7606 | `fn run_one_generator_model_byte_diff` | 226 | E2E gate helper |
| 7832 | `fn run_one_tiled_byte_diff` | 499 | E2E gate helper |
| 8331 | `fn decode_segment_voxels_into_volume` | 59 | E2E gate helper |
| 8390 | `fn load_oasis_model_data` | 18 | E2E gate helper |
| 8408 | `fn run_oasis_segment_byte_diff` | 496 | E2E gate helper |
| 8904 | `fn decode_segment_voxels_to_volume` | 54 | E2E gate helper |
| 8958 | `fn build_mixed_model_data` | 58 | E2E gate helper |
| 9016 | `fn build_segment_voxel_buffer_for_region` | 43 | E2E gate helper |
| 9059 | `fn chunk_kind` | 8 | E2E gate helper |
| 9067 | `fn block_kind` | 12 | E2E gate helper |
| 9079 | `fn build_segment_voxel_buffer_for_world` | 89 | E2E gate helper |
| 9168 | `pub fn validate_edit_mode` | 137 | **E2E GATE — MOVE OUT** |
| 9305 | `pub fn validate_runtime_edit_mode` | 138 | **E2E GATE — MOVE OUT** |
| 9443 | `fn built_pre_edit_state` | 7 | E2E gate helper |
| 9450 | `pub fn validate_entity_handler` | 68 | **E2E GATE — MOVE OUT** |
| 9518 | `mod tests` | 295 | W5 GPU↔CPU oracle test |
| 9813 | `mod tests_w1` | 792 | W1 GPU↔CPU oracle test |
| 10605 | `mod tests_w4` | 438 | W4 GPU↔CPU oracle test |
| 11043 | EOF | | |

**Distribution by class (verified):**

| class | lines | % of mod.rs |
|---|---:|---:|
| Resource defs + `FromWorld` (`106-754`, `755-908`) | ~700 | ~6 % |
| W0-seam systems (`extract_world_changes` + `populate_cpu_mirror_from_gpu_producer` + `prepare_construction` + `naadf_gpu_producer_node`) | ~2 580 | ~23 % |
| **DIAG probes (`AadfDelayedProbe` + `AadfPerCallProbe` + `AadfCpuGpuParity`, lines 3559–4617)** | **~1 060** | **~10 %** |
| Plugin wiring + Startup stub | ~110 | ~1 % |
| `build_segment_voxel_buffer*` (the 2 production ones) | ~145 | ~1 % |
| **E2E gates (`validate_*` × 6 + every `run_one_*_byte_diff` + every readback helper, lines 4874–9517)** | **~4 640** | **~42 %** |
| `mod tests` + `mod tests_w1` + `mod tests_w4` (embedded GPU↔CPU oracle tests) | ~1 525 | ~14 % |
| Module doc + section dividers + glue | ~280 | ~3 % |

The **two big chunks of moveable mass** are confirmed:
**~1 060 LOC of DIAG to delete** and **~4 640 LOC of E2E gates + ~1 525 LOC of
test modules** to relocate to dedicated submodules. Combined: ~7 220 LOC
(65 % of `mod.rs`) is structurally relocatable WITHOUT touching D4-owned
shared files.

---

## Findings

| # | severity | location | category | one-line description |
|---|---|---|---|---|
| 1 | **high** | `render/construction/mod.rs:1658-3075` | god-system | `prepare_construction` is 1 418 LOC of all five workstreams' allocation + bind-group code in one body |
| 2 | **high** | `render/construction/mod.rs:3559-4617` + `4653-4709` + ~10 ConstructionGpu fields | diagnostic-residual | `AadfDelayedProbe` / `AadfPerCallProbe` / `AadfCpuGpuParity` — ~1 060 LOC of investigation residual, USER DIRECTIVE: DELETE outright |
| 3 | **high** | `render/construction/mod.rs:4928-9517` | misplaced-fixtures | `validate_gpu_construction{,_scaled,_production_scale}` + 4 `run_one_*_byte_diff` + 4 `validate_*_mode` + helpers (~4 640 LOC) — live e2e gates living inside the production module |
| 4 | **medium** | `render/construction/mod.rs:836-838` + plugin reg `:4640-4642` | dead-stub | `clear_world_data_pending_edits` is a documented no-op kept "so registration need not be ripped out" |
| 5 | **medium** | `render/construction/mod.rs:1109,1168,1698,1699,3102-3111,3604,3614,3924,4143` | bevy-idiom | `prepare_construction` + `naadf_gpu_producer_node` open with 4–6 sequential `let Some(...) = ... else { return; }` bails — `.run_if(resource_exists::<X>)` would express the precondition declaratively |
| 6 | **medium** | `render/construction/mod.rs:4727-4823,8331-8390,8904-9100` | duplication | Six functions encode/decode the segment_voxel_buffer with overlapping bodies (DUP-7 + extensions) |
| 7 | **medium** | `assets/shaders/{chunk_calc,world_change,bounds_calc}.wgsl` vs `bounds_common.wgsl` + `shader_drift_guard.rs:1-400` | duplication-by-test | `MASK_*` constants + `cached_cell` + 3 helper fns inlined into 2-3 shader copies; a 400-LOC parser-test enforces the agreement |
| 8 | **medium** | `render/construction/hashing.rs:43` ↔ `aadf/block_hash.rs:395` | SSoT (cross-domain) | Two Rust copies of the same `31^(64-i)` polynomial table — D1 owns one, D5 owns the other |
| 9 | **low** | `render/construction/mod.rs:106-317` (`ConstructionGpu`) | bevy-idiom (BEV-3) | One `Resource` with 28 `Option<...>` fields + 4 `Option<&'static str>` label fields + 4 booleans tracking gate state |
| 10 | **low** | `render/construction/mod.rs:482-741` (`ConstructionPipelines`) | over-abstraction (OA-2) | An empty-sibling resource of `NaadfPipelines` enforced by W0 seam — now W1..W5 have all landed, the parallelism rationale is gone |
| 11 | **low** | WGSL `4u` / `64u` literals across `chunk_calc.wgsl`, `bounds_calc.wgsl`, `world_change.wgsl` | SSoT-3 (under-abstraction) | The paper's `CELL_DIM = 4` / `CELL_CHILDREN = 64` are bare integer literals in 30+ WGSL sites |

---

## Expanded entries

### Finding 1 — `prepare_construction` is a 1 418-LOC god-system (severity: high)

**Location:** `render/construction/mod.rs:1658-3075`.

**Current state:** the single `pub fn prepare_construction(...)` body holds
allocation + upload + bind-group construction for **every Phase-C workstream**:
W1 (Algorithm-1 buffers — hash_map, segment_voxel_buffer, hash_coefficients,
block_voxel_count); W3 (bound_queue_starts, bound_queue_sizes,
bound_group_queues, bound_group_masks, bound_refined_info,
bound_dispatch_indirect, chunks_mirror_buffer, prepare_probe_history); W2
(changed_* dynamic uploads); W4 (entity_chunk_instances + history +
entity_update_params); W5 (model_data_{chunk,block,voxel,params}_buffer); the
construction_world / construction_bounds / construction_change /
construction_entity / construction_generator_model / bound_dispatch /
prepare_probe_history bind-group builders; plus the "Phase-C followup #1
runtime GPU producer pre-allocation" branch (`:1701-2000`-ish) that mirrors
W5's per-segment work. The function takes **9 system parameters**, including
4 `Option<…>` ones, and opens with 4 sequential `else { return; }` bails
(verified: `mod.rs:1684,1691,1698,1699`).

**Why it's a problem:** the W0 seam contract (`15-design-c.md` §1.1, §2.1
W0 row) intentionally placed allocations in `prepare_construction` so each
workstream could land its block additively without merge-conflicting on
struct shape. **That parallelism is now spent** — W0..W5 + the followup +
wave-3 entity merge have all landed. What remains is a 1 418-line function
that holds ~5 independent allocator-and-binder sub-tasks behind a single
function-level early-bail ladder, making the W3 prepare path depend on the
W1 hash buffers existing (the early-return on
`let Some(world_gpu) = world_gpu else { return; };` blocks every workstream's
allocations from running until *every* workstream's preconditions are
satisfied). Each workstream's own submodule (`bounds_calc.rs`, `world_change.rs`,
etc.) already owns its dispatch + layout descriptor; the matching
prepare-side code stayed centralized.

**Suggested direction (NOT a design):** move each workstream's allocation +
upload + bind-group block out of `prepare_construction` into a
`prepare_<workstream>` system in the workstream's own submodule
(`chunk_calc::prepare_chunk_calc`, `bounds_calc::prepare_bounds`,
`world_change::prepare_change`, `entity_update::prepare_entity`,
`generator_model::prepare_generator`). Register each as a peer in
`RenderSystems::PrepareResources` with `.after(prepare_world_gpu)` and gate
by `resource_exists` (Finding 5). The W0-seam-shared bind groups
(`construction_world`, `construction_bounds_world`) can stay in a thin
`mod.rs::prepare_construction_shared` for the few cross-workstream bind
groups. Architect to decide whether the four shared bind groups need a
shared driver or can each be owned by the first workstream that consumes
them.

**Out-of-scope ripple (if any):** none — every callee submodule is D5
territory; `prepare_world_gpu` (D4) is only referenced as `.after(...)` and
that ordering edge transfers verbatim to whichever D5 system replaces it.

---

### Finding 2 — Diagnostic probes: DELETE outright (severity: high)

**Location:**
- `mod.rs:3559-3870` — `AadfDelayedProbe` struct + `aadf_delayed_probe` system.
- `mod.rs:3873-4087` — `AadfPerCallProbe` struct, `PerCallProbeStage` enum,
  `aadf_per_call_probe` system.
- `mod.rs:4088-4596` — `AadfCpuGpuParity` struct, `CpuGpuParityStage` enum,
  `aadf_cpu_gpu_parity` system.
- `mod.rs:4597-4617` — `aadf_cpu_gpu_parity_maybe` wrapper.
- `mod.rs:4653-4709` — three `.init_resource` + three `.add_systems(ExtractSchedule, …)` registrations.
- `ConstructionGpu` field `prepare_probe_history` (`mod.rs:155`) is the GPU-side
  storage written by `prepare_group_bounds` shader; the readback is drained by
  `aadf_per_call_probe`. The field is also referenced by
  `prepare_construction` (allocation) + `bounds_calc.rs` (bind group binding).
- WGSL `bounds_calc.wgsl:418-423` writes the probe history; consult before
  deletion of the GPU-side storage.

**Verified caller audit** (`grep -rn "AadfDelayedProbe\|aadf_delayed_probe\|AadfPerCallProbe\|aadf_per_call_probe\|AadfCpuGpuParity\|aadf_cpu_gpu_parity"`):

All three probe types and their systems are referenced **only in
`mod.rs`**. No `bin/`, no `e2e/`, no `crates/bevy_naadf/src/{lib,main}.rs`,
no Playwright TS. Zero external consumers. (`grep` returned a single
matching file — `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/construction/mod.rs`.)

**Why it's a problem:** User directive (`01-context.md` Q2): "everything
else can go." These probes are investigation residuals from
`wasm-chunk-aadf-nondeterminism` and the `horizon-parity` debugging — kept
"in case the bug recurs". They run in `ExtractSchedule` every frame after
`cpu_mirror_populated`, allocating ~16 MiB of staging buffers on first fire
(`chunks_staging` at production scale: 256×32×256 × 8 B = 16.7 MiB),
issuing per-frame `Device::poll` calls + `map_async` callbacks; on web
this materially extends per-frame work. The `bounds_calc.wgsl` has a
chunk of dead-write code (lines 418-423) that writes to
`prepare_probe_history` — that write becomes dead too when the consumer
is removed.

**Suggested direction (NOT a design):** delete the three structs +
their systems + the three `.init_resource` + `.add_systems(ExtractSchedule, …)`
registrations + the `prepare_probe_history` field on `ConstructionGpu` + the
matching `prepare_probe_history` bind-group field on `ConstructionBindGroups`
(`mod.rs:441-443`) + the `prepare_probe_history_layout` field on
`ConstructionPipelines` (`mod.rs:?`) + the WGSL write block in
`bounds_calc.wgsl:418-423` (plus the storage binding declaration). The
`PREPARE_PROBE_HISTORY_ENTRIES` / `PREPARE_PROBE_HISTORY_BYTES` consts
(`mod.rs:340-344`) also become dead. Architect to confirm whether
`prepare_construction`'s `prepare_probe_history_staging` buffer allocation
(if any) needs co-deletion.

**Out-of-scope ripple (if any):** the probe writes inside
`bounds_calc.wgsl:418-423` are inside D5's WGSL — fine. **However**, this
finding edits `bounds_calc.rs`'s layout count (`prepare_probe_history_layout`
becomes orphaned), which is fine since `bounds_calc.rs` is D5. **No D4
ripple.**

---

### Finding 3 — E2E gates live inside the production module (severity: high)

**Location:** `mod.rs:4727-9517` — the e2e gate functions + their helpers.
**Verified wiring** (`grep -n` in `bin/e2e_render.rs`):

- `validate_gpu_construction` (`mod.rs:4928`) ← `e2e_render.rs:420` (`--validate-gpu-construction`)
- `validate_gpu_construction_scaled` (`mod.rs:5290`) ← `e2e_render.rs:214` (`--validate-gpu-construction-scaled`)
- `validate_gpu_construction_production_scale` (`mod.rs:5621`) ← `e2e_render.rs:230` (`--validate-gpu-construction-production-scale`)
- `validate_edit_mode` (`mod.rs:9168`) ← `e2e_render.rs:452` (`--edit-mode`)
- `validate_runtime_edit_mode` (`mod.rs:9305`) ← `e2e_render.rs:469` (`--runtime-edit-mode`)
- `validate_entity_handler` (`mod.rs:9450`) ← `e2e_render.rs:440` (`--entities`)

Each of these boots `MinimalPlugins + AssetPlugin + ImagePlugin + RenderPlugin`,
queues pipelines via `pipeline_cache.set_shader(...)`, dispatches the full
W1+W3+W5 chain, maps buffers with `map_async`, and asserts against the CPU
oracle (`aadf::construct::construct`).

`run_one_fixture_byte_diff` / `run_one_fixture_multiseg_byte_diff` /
`run_one_generator_model_byte_diff` / `run_one_tiled_byte_diff` /
`run_oasis_segment_byte_diff` (all of `mod.rs:6623-9165`) are called only
from `validate_gpu_construction_scaled` (`:5314, 5351, 5384, 5416`) and
`validate_gpu_construction_production_scale` (`:5460`). Internal-only —
zero callers outside `mod.rs`.

**Why it's a problem:** **~4 640 LOC of test fixtures occupy the same file
as ~700 LOC of production resource definitions and ~1 420 LOC of the
production `prepare_construction` system.** Anyone trying to navigate the
production code paths has to scroll past ~42 % of the file's mass that has
zero runtime presence in the normal binary execution path. The
`validate_gpu_construction_production_scale` body alone (714 LOC,
`:5621-6334`) boots a full `MinimalPlugins+RenderPlugin` `App` from inside
the production crate — production code should not host the test fixtures
that exercise it.

**Suggested direction (NOT a design):** move the six `validate_*` functions
+ every helper they reach (the `VoxelReadback` struct + every `run_one_*` +
`load_oasis_model_data` + every decode/build helper) into a new
`render/construction/validation/` submodule (or split into 6 files
`validation/{gpu_construction,gpu_construction_scaled,gpu_construction_production_scale,edit_mode,runtime_edit_mode,entity_handler}.rs`).
`pub(crate)` is sufficient — `bin/e2e_render.rs` is in the same crate.
Architect to decide whether the embedded `mod tests`, `mod tests_w1`,
`mod tests_w4` test modules should live in a sibling `tests.rs` /
`tests_w1.rs` / `tests_w4.rs` (Rust convention — `#[cfg(test)] mod tests;`)
or whether they belong inside the workstream submodules (e.g. `tests_w1`
inside `chunk_calc.rs` since it tests the W1 chain).

**Out-of-scope ripple (if any):** **none for D4** — every test boot uses
`MinimalPlugins`/`AssetPlugin`/`RenderPlugin` directly + `pipeline_cache.set_shader`
inlines; no path into `render::prepare`/`render::gpu_types`/`render::pipelines`.
`e2e_render.rs` qualifying path (`bevy_naadf::render::construction::validate_…`)
gets one path segment added (`::validation::`).

---

### Finding 4 — `clear_world_data_pending_edits` is a no-op stub kept for the registration (severity: medium)

**Location:** `mod.rs:836-838` (definition) + `mod.rs:4640-4642` (registration in
`ConstructionPlugin::build`).

**Current state:**
```rust
pub fn clear_world_data_pending_edits(_world_data: Option<ResMut<crate::world::data::WorldData>>) {
    // No-op — drain moved into `extract_world_changes` (see doc above).
}
```
registered in `Last` schedule: `app.add_systems(Last, clear_world_data_pending_edits);`.

The 16-line docblock above the function says: *"This function is kept (now a
no-op stub) so the system registration in `ConstructionPlugin::build` need not
be ripped out in this dispatch; the orchestrator's follow-up may delete the
registration entirely."*

**Why it's a problem:** literal dead code, documented as such by the author,
left for "a follow-up dispatch" that this is. Per the user's `/deadcode`
discipline and the Q2 directive ("everything else can go"), this should
have been removed when the drain moved into `extract_world_changes`. Today
it adds: (a) a system-registration entry in the `Last` schedule that the
scheduler walks every frame, (b) reader confusion about whether the
no-op is load-bearing.

**Suggested direction (NOT a design):** delete the function + the
`add_systems(Last, …)` registration. Architect to confirm zero
schedule-ordering edges depend on it.

**Out-of-scope ripple (if any):** none.

---

### Finding 5 — `else { return; }` ladders that should be `.run_if(resource_exists::<X>)` (severity: medium)

**Location:** verified count = 20 `else { return; }` patterns in
`mod.rs`. Concentrated at:
- `mod.rs:1109,1168` — `populate_cpu_mirror_from_gpu_producer` opens with 2 bails.
- `mod.rs:1684,1691,1698,1699` — `prepare_construction` opens with 4 sequential bails.
- `mod.rs:3102,3106,3110,3111,3115,3118` — `naadf_gpu_producer_node` opens
  with 6 sequential bails on the system parameters.
- `mod.rs:3604,3614,3620,3621,3697,3698,3699` — `aadf_delayed_probe` (DIAG; removed by Finding 2).
- `mod.rs:3924,3937` — `aadf_per_call_probe` (DIAG; removed).
- `mod.rs:4143,4156` — `aadf_cpu_gpu_parity` (DIAG; removed).
- Plus 4 in `world_change.rs:370-373` and 5 in `bounds_calc.rs:464-495`.

**Why it's a problem:** Bevy idiom (BEV-6 in the audit). Each bail-line is
expressing a precondition ("this system can't run without resource X") in
the function body where the scheduler can't see it. `.run_if(resource_exists::<X>)`
+ `.run_if(|gpu: Res<ConstructionGpu>| !gpu.gpu_producer_has_run)` move
the conditions to system-registration time, where they're inspectable and
where the scheduler can elide the system invocation entirely. The
production-relevant systems (`prepare_construction`,
`naadf_gpu_producer_node`, `populate_cpu_mirror_from_gpu_producer`,
`extract_world_changes`) total ~12 bail-lines that translate to `.run_if`
clauses.

**Suggested direction (NOT a design):** add `.run_if(resource_exists::<X>)`
conditions to the `add_systems` calls in `ConstructionPlugin::build` for
each system parameter that today opens with an `else { return; }`. The
boolean-flag bails (e.g. `if gpu.gpu_producer_has_run { return; }`) become
`.run_if(|gpu: Option<Res<ConstructionGpu>>| gpu.is_some_and(|g| !g.gpu_producer_has_run))`
or similar. Architect to decide whether to lift the
`gpu_producer_has_run` / `cpu_mirror_populated` / `bounds_initialized`
booleans into their own zero-sized "Step done" marker resources
(`#[derive(Resource)] struct GpuProducerHasRun;`) so the gate becomes
`resource_exists::<…>` rather than an inner boolean — that's a deeper
re-design.

**Out-of-scope ripple (if any):** none — `ConstructionPlugin::build` is
D5.

---

### Finding 6 — Six segment-voxel-buffer encoders/decoders (severity: medium)

**Location:**

- `mod.rs:4727` `pub fn build_segment_voxel_buffer_from_dense(&[u16], world_size, seg_size)` — production caller `prepare_construction:1894`.
- `mod.rs:4820` `pub fn build_segment_voxel_buffer(&DenseVolume, seg)` — test caller `validate_gpu_construction:4998` and `mod tests:10082`.
- `mod.rs:8331` `fn decode_segment_voxels_into_volume(...)` — test helper.
- `mod.rs:8904` `fn decode_segment_voxels_to_volume(...)` — test helper.
- `mod.rs:9016` `fn build_segment_voxel_buffer_for_region(...)` — test helper.
- `mod.rs:9079` `fn build_segment_voxel_buffer_for_world(...)` — test helper.

All six share the inner triple-nested `for chunk × for block × for voxel`
encoding loop with the same 2048-u32-per-chunk / 32-u32-per-block /
`(lo | (hi << 16))` packing. The input shape (DenseVolume vs `&[u16]` dense
vs `ModelData`) is the only meaningful difference.

**Why it's a problem:** the encoding rule is paper-canonical (chunkCalc.fx
ABI). Six near-parallel implementations means a layout change requires
editing six functions in lock-step. The `_from_dense` / non-`_from_dense`
split was the audit's DUP-7; the four additional decoders/encoders (region,
world, into_volume, to_volume) extend the family.

**Suggested direction (NOT a design):** propose one canonical
`encode_chunk(chunk_pos, voxel_getter: impl Fn(IVec3) -> u16) -> [u32; 2048]`
inner-loop + thin per-input-shape wrappers. **`_from_dense` is the only
production caller; the rest are test fixtures.** Architect to decide
whether the test fixtures need to move with Finding 3's e2e gate
relocation (the test helpers should travel with the test fixtures), and
whether the `_from_dense` production helper deserves to move into
`generator_model.rs` or `chunk_calc.rs` (it's the segment-voxel-buffer
the chunk_calc shader consumes). One natural home: `chunk_calc.rs::cpu_segment_encoder`.

**Out-of-scope ripple (if any):** none. The `_from_dense` callers in
`prepare_construction:1894` are D5.

---

### Finding 7 — WGSL helpers inlined across 2-3 shader copies, defended by a 400-LOC parser-test (severity: medium)

**Location:**

- Canonical reference: `assets/shaders/bounds_common.wgsl` (191 LOC).
- Inline copy #1: `assets/shaders/chunk_calc.wgsl:138-310` (the `MASK_*` consts
  + `cached_cell` + `check_matching_bounds` + `add_bounds_voxels_or_blocks` +
  `compute_bounds_4`).
- Inline copy #2: `assets/shaders/world_change.wgsl:161-340` (same).
- Partial inline (`MASK_*` only): `assets/shaders/bounds_calc.wgsl`.
- Drift guard: `render/construction/shader_drift_guard.rs:1-400` — a
  string-parser that extracts marked regions from each shader and asserts
  byte-after-normalisation equality.

**Why it's a problem:** the docblock at `shader_drift_guard.rs:1-30` openly
admits the rationale: *"Bevy's WGSL `#import` surface is unpredictable
across naga versions."* So instead of using `#import "shaders/bounds_common.wgsl"`,
the project carries 2.5 copies of ~150 WGSL LOC and a 400-LOC Rust parser to
catch divergence. The maintenance cost is borne by every workstream that
touches the masks or helpers — a fix has to be propagated to 3 copies
under threat of CI failure. The audit (15-design-c.md §1) cited "5 new
WGSL files (~15 entry points)" — the actual count is now 6 + 1
common, but the "common" is duplicated rather than imported.

**Suggested direction (NOT a design):** verify whether the Bevy/naga
version the project is on (Bevy 0.19) supports `#import` reliably for the
specific WGSL forms used (workgroup-shared array, atomics, struct
declarations). If it does, replace the inline copies with imports +
delete `shader_drift_guard.rs`. If it does not, leave the guard in
place but consider whether the parser approach can be simpler (e.g.
single-source-of-truth with a build-time `concat!` into shader
strings — though that loses asset-server live-reload). Architect to
investigate Bevy 0.19's `naga_oil`-based shader-import status.

**Out-of-scope ripple (if any):** none — every shader cited is D5's.

---

### Finding 8 — SSoT-6 confirmed: two Rust copies of the `31^(64-i)` hash table (severity: medium)

**Location:**

- `render/construction/hashing.rs:43 pub fn hash_coefficients() -> [u32; 65]` — D5's copy. Caller: `prepare_construction` uploads result into the GPU `hash_coefficients` storage buffer.
- `aadf/block_hash.rs:395 fn build_polynomial_coefficients() -> [u32; 65]` — D1's copy. Caller: `BlockHashingHandler::new` (the CPU-side oracle for D1's `aadf/construct.rs`).

Both implement the same algorithm: `c[64] = 1; for i in 63..0: c[i] = c[i+1].wrapping_mul(31);`.

**Audit refinement:** the audit at `00-reuse-audit.md:600` claimed SSoT-6 had
**three** implementations including "hardcoded in WGSL `chunk_calc.wgsl` as
`chunk_coefficients` array literal." **Verified refuted** — the WGSL
shaders (`chunk_calc.wgsl:134`, `map_copy.wgsl:69`, `world_change.wgsl:129`)
declare `hash_coefficients` as `var<storage, read> hash_coefficients: array<u32>;`
and the buffer is uploaded by `prepare_construction` from the CPU
`hashing::hash_coefficients()` result. **There is no WGSL array literal.**
The real SSoT divergence is exactly 2 Rust functions, not 3.

**Why it's a problem:** two `pub fn`s computing the same fixed table; a
silent algorithm change in one (e.g. a typo, a different `wrapping_mul`
behavior on a different `Wrapping<u32>` flavor) wouldn't trip a compile
error. Two test sites (`block_hash.rs:417` + `hashing.rs:165`) both assert
against the C# values, so divergence would surface in test, but it's
defense-in-depth on two parallel sources.

**Suggested direction (NOT a design):** ONE of:
(a) D5's `hashing::hash_coefficients()` calls D1's `block_hash::build_polynomial_coefficients()`
    (or a renamed pub version) and re-exports — this collapses to a single
    definition, owned by D1 (the paper-canonical "BlockHashingHandler" home).
(b) Move the polynomial table to a shared utility module
    (`crate::aadf::hash` or similar) and both D1 + D5 import.

Per D5 ↔ D1 cross-domain item, architect coordinates with D1's architect.

**Out-of-scope ripple (if any):** D1's `aadf/block_hash.rs:395` is read-only
from D5. **This finding is flagged as a D1↔D5 cross-domain item — D1's
implementor handles the merge.**

---

### Finding 9 — `ConstructionGpu` is a 28-`Option<…>`-field god-resource (severity: low)

**Location:** `render/construction/mod.rs:106-317` — the `Resource`.

**Current state:** I counted via `grep -c "pub .*: Option<"` ⇒ 51 `Option<…>`
fields across the whole file; restricted to the `ConstructionGpu` struct
body (lines 106-317), the field set is:

- **W1** (4 buffer Options): segment_voxel_buffer, block_voxel_count, hash_map, hash_coefficients.
- **W3** (8 + 1 bool): bound_queue_starts, bound_queue_sizes, bound_group_queues, bound_group_masks, bound_refined_info, bound_dispatch_indirect, prepare_probe_history (→ DELETE per Finding 2), bounds_params_buffer, chunks_mirror_buffer; plus `bounds_initialized: bool`.
- **W2** (4 buffer Options): changed_{groups,chunks,blocks,voxels}_dynamic.
- **W4** (7 buffer Options + 1 bool + 1 bool): entity_chunk_instances, entity_voxel_data, entity_instances_history, chunk_updates_dynamic, entity_chunk_instances_dynamic, entity_history_dynamic, entity_update_params_buffer; plus `world_bind_group_has_entities: bool`, `gpu_producer_has_run: bool`, `cpu_mirror_populated: bool`.
- **W5** (4 buffer Options): model_data_{chunk,block,voxel,params}_buffer.
- **Q4 label stash** (4 `Option<&'static str>`): block_voxel_count_label, segment_voxel_buffer_label, hash_map_label, hash_coefficients_label.
- **Q3 readback**: `cpu_mirror_readback: CpuMirrorReadback` (itself 4 `Option<Buffer>` + 4 `Arc<AtomicBool>` + 4 `u64` + 1 enum + 1 u32).

**Why it's a problem:** BEV-3 in the audit. Every system that consumes
ConstructionGpu has to begin with a `let Some(...) = ... else { return; }`
on each field it touches (Finding 5). Each workstream's "is my family
allocated?" check is a separate `.is_none()` on its 4-8 fields. The W0
seam contract documented this trade-off (one resource so every workstream
adds to the same struct rather than fighting for a new `Resource` slot),
but with all five workstreams now landed, the seam's parallelism rationale
is done.

**Suggested direction (NOT a design):** split into per-workstream resources:
`ChunkCalcGpu` (W1), `BoundsCalcGpu` (W3), `WorldChangeGpu` (W2),
`EntityUpdateGpu` (W4), `GeneratorModelGpu` (W5). Each workstream's
prepare-system (Finding 1) owns allocation of its resource; systems
`.run_if(resource_exists::<…>)` (Finding 5) elide cleanly. The remaining
"cross-workstream gate booleans" (`gpu_producer_has_run`,
`cpu_mirror_populated`, `bounds_initialized`, `world_bind_group_has_entities`)
become zero-sized marker resources or move into a shared
`ConstructionState` resource. The `Q4 label stash` fields are pure debug
assertion infrastructure and can move to whatever shell holds the buffers
they describe. Architect to decide the granularity (per-workstream vs
finer-grained).

**Out-of-scope ripple (if any):** none — D4 doesn't reference
`ConstructionGpu`'s field set; only `prepare.rs::WorldGpu` is the read-only
shared resource and that's untouched.

---

### Finding 10 — `ConstructionPipelines` "empty sibling" abstraction has outlived its purpose (severity: low)

**Location:** `render/construction/mod.rs:482-741`. 25 `pub` fields (the entire
`from_world` impl is 168 lines, each workstream adding ~30 LOC).

**Current state:** the W0 seam contract (`mod.rs:459-480` docblock)
explicitly justified `ConstructionPipelines` as an "empty sibling of
NaadfPipelines"  to enable parallel W1..W5 workstream merges:
*"The FromWorld impl is the SINGLE seam each later workstream extends:
add a field, add the corresponding layout build + pipeline queue in
from_world, register the resulting handle in the struct literal at the
bottom. The clone-cost of BindGroupLayoutDescriptor keeps the seam trivial
for parallel-merge — every workstream's field is an additive edit."*

**Why it's a problem:** OA-2 in the audit. All five workstreams have
landed. The parallel-merge property is no longer being used. What
remains is a single 25-field resource owned by a 170-LOC `from_world`,
mirrored by every workstream's submodule that already owns its
`queue_*_pipeline` helper + `*_layout_descriptor` fn. Each workstream
submodule could just `init_gpu_resource::<ChunkCalcPipelines>()` on its
own and bind its own pipelines without a central registry. This would
mean `chunk_calc.rs::naadf_gpu_producer_node` (and the prepare-side of
Finding 1) look up `Res<ChunkCalcPipelines>` rather than reaching into
`Res<ConstructionPipelines>` to pluck out chunk-calc-specific fields.

**Suggested direction (NOT a design):** split `ConstructionPipelines` per
workstream (`ChunkCalcPipelines`, `BoundsCalcPipelines`, `WorldChangePipelines`,
`EntityUpdatePipelines`, `GeneratorModelPipelines`, `MapCopyPipelines`).
Each owns its layouts + pipeline IDs + `FromWorld` impl. **However**, this
finding's severity is `low` because the `from_world` is already invoked
once at startup — the structural cost is purely a readability/seam-cleanliness
concern, not a runtime cost.

**Out-of-scope ripple (if any):** `NaadfPipelines` (D4) stays exactly as
it is; that's the whole point of the construction-side split staying
construction-side.

---

### Finding 11 — WGSL `4u` / `64u` literals shadowing `CELL_DIM` / `CELL_CHILDREN` (severity: low)

**Location:** verified `grep -n "\b4u\b\|\b64u\b"` results:
- `chunk_calc.wgsl:168-543` — 14 sites of `4u` / `64u` literals.
- `bounds_calc.wgsl:222-479` — 6 sites (including `(gp.x * 4u + local_id.x)`).
- `world_change.wgsl:207-556` — 10 sites.

Rust SSoT is in `voxel/mod.rs:63-65` (`CELL_DIM = 4`, `CELL_CHILDREN = 64`).
These constants are not piped into the shaders via shader-defs or
include-string interpolation.

**Why it's a problem:** SSoT-3 (cross-cutting). The paper hardcodes 4 forever
(`CELL_DIM` is paper-canonical), so the actual risk of divergence is low
— but every shader's `4u` / `64u` is unnamed magic at the WGSL level, and
a reader of the shader has to mentally bind each `4u` to the right meaning
(workgroup size? cell side? bit-shift amount?). Several sites are
ambiguous on a quick read (e.g. `bounds_calc.wgsl:421-422` —
`probe_call_idx * 4u` is "4 u32s per entry", `gp.x * 4u` is "cell side").

**Suggested direction (NOT a design):** declare `const CELL_DIM: u32 = 4u;`
+ `const CELL_CHILDREN: u32 = 64u;` at the top of each construction shader
(or in `bounds_common.wgsl`); replace `4u` / `64u` with the named constant
**only at sites where the literal IS that semantic** (do not blanket-replace —
some `4u` are bit-shift amounts unrelated to `CELL_DIM`). Architect to
audit each site.

**Out-of-scope ripple (if any):** the Rust SSoT lives in `voxel/mod.rs`
(D1). If the architect chooses to pipe `CELL_DIM` from Rust to WGSL via a
shader-def or include-string concat, that would require a D1 ↔ D5
coordination — but the cheaper "just name the constant inside each WGSL
file" change is D5-internal.

---

## Confirmed / refuted audit suspicions

| audit item | claim | status | evidence |
|---|---|---|---|
| audit §1.5 / §2 D5 row | `mod.rs` is 11 043 LOC | **confirmed** | `wc -l mod.rs` = 11 043 |
| Initial suspicion #1 | Five distinct concerns mashed into mod.rs | **confirmed** | Resource defs (700) + prepare_construction (1 418) + naadf_gpu_producer_node (434) + DIAG probes (~1 060) + e2e gates (~4 640) — see distribution table above |
| Initial suspicion #2 | `validate_gpu_construction_production_scale` (line 5621) is ~700 LOC and boots full RenderApp | **confirmed** | `wc` between 5621-6334 = 714 LOC; uses `MinimalPlugins + AssetPlugin + ImagePlugin + RenderPlugin`; `let Some(render_app) = app.get_sub_app_mut(RenderApp) else …` |
| Initial suspicion #3 | `AadfDelayedProbe` / `AadfPerCallProbe` / `AadfCpuGpuParity` are confined to mod.rs | **confirmed** | `grep -rn` returned only `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/construction/mod.rs` |
| SSoT-3 (CELL_DIM=4 in WGSL) | `4u` / `64u` literals across construction shaders | **confirmed** | `grep -n "\b4u\b\|\b64u\b"` in `chunk_calc.wgsl` / `bounds_calc.wgsl` / `world_change.wgsl` |
| SSoT-6 (hash table 3 impls) | "Implemented THREE times: block_hash.rs, hashing.rs, AND chunk_calc.wgsl array literal" | **refuted (partially)** | WGSL declares `var<storage, read> hash_coefficients: array<u32>` — NO array literal. Only 2 Rust copies exist (block_hash.rs:395 + hashing.rs:43) |
| DUP-4 (3 `validate_gpu_construction*` variants) | three variants in mod.rs | **confirmed** | Lines 4928, 5290, 5621 |
| DUP-5 (4 `run_one_*_byte_diff` fixtures) | four byte-diff fixtures | **confirmed**, plus one bonus | `run_one_fixture_byte_diff:6623`, `run_one_fixture_multiseg_byte_diff:7134`, `run_one_generator_model_byte_diff:7606`, `run_one_tiled_byte_diff:7832` (+ `run_oasis_segment_byte_diff:8408` — also fits the pattern) |
| DUP-7 (2 `build_segment_voxel_buffer*`) | two parallel encoders | **confirmed and extended** | Six functions total in the family — see Finding 6 |
| BEV-3 (`ConstructionGpu` 16+ Options) | 16+ Option fields | **confirmed and extended** | 28 `Option<Buffer>` + 4 `Option<&'static str>` + 4 `bool` gate fields — see Finding 9 |
| BEV-6 (6-9 `else { return; }` ladders) | 6-9 sequential bail patterns | **confirmed and extended** | 20 total bails in mod.rs (Finding 5) |
| OA-2 (`ConstructionPipelines` empty sibling) | parallel-resource workaround | **confirmed but partially outdated** | W1..W5 landed; original parallelism rationale exhausted. Finding 10 |
| UA-3 (chunk-pos raw masks in world_change) | `0x7FF` / `0x3FF` masks bypassing helpers | **partially refuted in D5** | `world_change.rs` has only `0x3FFF_FFFF` (the flood-fill `DIST_UNTOUCHED` sentinel) + a single `<<30`; the raw chunk-pos masking lives in `aadf/edit.rs` (D1). D5 properly uses `pack_chunk_pos` / `unpack_chunk_pos` helpers in `change_handler.rs` |
| Hash-coefficients algorithm | matches `aadf/block_hash.rs` | **confirmed** | both compute `c[64]=1; c[i] = c[i+1]*31; ` with `wrapping_mul` |

---

## Proposed `mod.rs` split (sketch — architect designs)

The architect's design will be the canonical layout. This sketch documents
what the structure looks like in this explorer's head, so the architect has
a starting point rather than a blank canvas.

```
crates/bevy_naadf/src/render/construction/
├── mod.rs                                ── ~600 LOC (was 11 043)
│     ├ pub use … re-exports
│     ├ ConstructionPlugin (Plugin::build)
│     ├ shared ConstructionState resource (gate booleans only:
│     │   gpu_producer_has_run, cpu_mirror_populated, bounds_initialized,
│     │   world_bind_group_has_entities)
│     ├ ConstructionConfig re-export
│     └ ConstructionEvents resource (cross-workstream W2 + W4 edit batch)
│
├── extract.rs                            ── ~210 LOC (new)
│     └ extract_world_changes
│
├── readback.rs                           ── ~660 LOC (new)
│     ├ ReadbackStage + CpuMirrorReadback (currently mod.rs:352-405)
│     ├ READBACK_STALL_BUDGET_FRAMES const
│     └ populate_cpu_mirror_from_gpu_producer
│
├── producer.rs                           ── ~470 LOC (new)
│     └ naadf_gpu_producer_node
│
├── chunk_calc.rs                         ── ~440 LOC (was 314 + W1 prepare-block + W1 fields)
│     ├ existing dispatch helpers
│     ├ pub struct ChunkCalcGpu Resource (W1 buffer family — was inside ConstructionGpu)
│     ├ pub struct ChunkCalcPipelines (W1 pipelines — was inside ConstructionPipelines)
│     ├ prepare_chunk_calc system
│     └ cpu_segment_encoder (the canonical Finding 6 encoder)
│
├── bounds_calc.rs                        ── ~720 LOC (was 619 + W3 prepare-block + W3 fields)
│     (analogous; absorbs W3 buffer family + W3 pipelines + prepare_bounds + bound_dispatch bind group)
│
├── world_change.rs                       ── ~1 300 LOC (was 1 165 + W2 prepare-block + W2 fields)
│     (analogous; absorbs W2 buffer family + W2 pipelines + prepare_change)
│
├── entity_update.rs                      ── ~620 LOC (was 401 + W4 prepare-block + W4 fields)
│     (analogous; absorbs W4 buffer family + W4 pipelines + prepare_entity)
│
├── generator_model.rs                    ── ~430 LOC (was 303 + W5 prepare-block + W5 fields)
│     (analogous; absorbs W5 buffer family + W5 pipelines + prepare_generator)
│
├── map_copy.rs                           ── ~177 LOC (unchanged)
├── hashing.rs                            ── ~241 LOC (unchanged; if Finding 8 lands, becomes a re-export of D1's table)
├── change_handler.rs                     ── ~391 LOC (unchanged)
├── entity_handler.rs                     ── ~441 LOC (unchanged)
├── config.rs                             ── ~326 LOC (unchanged)
├── shader_drift_guard.rs                 ── ~400 LOC (unchanged or DELETE per Finding 7)
│
└── validation/                           ── ~5 800 LOC (was inside mod.rs:4928-9517 + the embedded mod tests)
      ├── mod.rs                          ── 50 LOC (re-exports, `pub(crate) use`)
      ├── gpu_construction.rs             ── 362 LOC (validate_gpu_construction)
      ├── gpu_construction_scaled.rs      ── ~1 200 LOC (validate_gpu_construction_scaled + the 4 run_one_*_byte_diff fixtures it dispatches)
      ├── gpu_construction_production.rs  ── ~990 LOC (validate_gpu_construction_production_scale + VoxelReadback + readback_cursor + map_single_* + sample_voxel_readback + render_results_table + load_oasis_model_data + run_oasis_segment_byte_diff)
      ├── edit_mode.rs                    ── 137 LOC (validate_edit_mode)
      ├── runtime_edit_mode.rs            ── 138 LOC (validate_runtime_edit_mode)
      └── entity_handler.rs               ── 68 LOC (validate_entity_handler)

DELETED (Finding 2):
  - mod.rs:3559-4617 (AadfDelayedProbe, AadfPerCallProbe, AadfCpuGpuParity + their systems + aadf_cpu_gpu_parity_maybe)
  - mod.rs:4653-4709 (3 .init_resource + 3 .add_systems(ExtractSchedule) for the above)
  - ConstructionGpu::prepare_probe_history field + matching layout fields
  - PREPARE_PROBE_HISTORY_ENTRIES / PREPARE_PROBE_HISTORY_BYTES consts (mod.rs:326-344)
  - WGSL probe writes in bounds_calc.wgsl:418-423 + the probe binding declaration

DELETED (Finding 4):
  - clear_world_data_pending_edits + its Last-schedule registration

Embedded tests:
  - mod tests           (line 9518)   → tests/w5_oracle.rs (or generator_model.rs::#[cfg(test)] mod tests)
  - mod tests_w1        (line 9813)   → chunk_calc.rs::#[cfg(test)] mod tests
  - mod tests_w4        (line 10605)  → entity_update.rs::#[cfg(test)] mod tests
```

**Projected post-refactor LOC for D5 Rust**: ~11 000 LOC total
(vs 15 821 today), distributed across 14 + 6-validation files of
180–1 300 LOC each. **Roughly a 4 800 LOC drop** without touching any
production behaviour — purely structural relocation + DIAG deletion +
the stub deletion.

(The architect will refine these numbers — the sketch is intentionally
optimistic on per-workstream extraction; some prepare-side code may
genuinely need to stay co-located in a shared `mod.rs::prepare_construction_shared`.)

---

## Diagnostic-probe deletion list

| # | item | location | LOC est. | caller audit |
|---:|---|---|---:|---|
| 1 | `pub struct AadfDelayedProbe` | `mod.rs:3559-3582` | 24 | only mod.rs |
| 2 | `pub fn aadf_delayed_probe` (system) | `mod.rs:3594-3870` | 277 | only mod.rs |
| 3 | `pub struct AadfPerCallProbe` | `mod.rs:3873-3888` | 16 | only mod.rs |
| 4 | `pub enum PerCallProbeStage` | `mod.rs:3889-3914` | 26 | only mod.rs |
| 5 | `pub fn aadf_per_call_probe` (system) | `mod.rs:3915-4087` | 173 | only mod.rs |
| 6 | `pub struct AadfCpuGpuParity` | `mod.rs:4088-4104` | 17 | only mod.rs |
| 7 | `pub enum CpuGpuParityStage` | `mod.rs:4105-4132` | 28 | only mod.rs |
| 8 | `pub fn aadf_cpu_gpu_parity` (system) | `mod.rs:4133-4596` | 464 | only mod.rs |
| 9 | `pub fn aadf_cpu_gpu_parity_maybe` (wrapper) | `mod.rs:4597-4617` | 21 | only mod.rs |
| 10 | `.init_resource::<AadfDelayedProbe>()` | `mod.rs:4653` | 1 | Plugin::build |
| 11 | `.init_resource::<AadfPerCallProbe>()` | `mod.rs:4655` | 1 | Plugin::build |
| 12 | `.init_resource::<AadfCpuGpuParity>()` | `mod.rs:4657` | 1 | Plugin::build |
| 13 | `.add_systems(ExtractSchedule, aadf_delayed_probe)` | `mod.rs:4694` | 1 | Plugin::build |
| 14 | `.add_systems(ExtractSchedule, aadf_per_call_probe)` | `mod.rs:4699` | 1 | Plugin::build |
| 15 | `.add_systems(ExtractSchedule, aadf_cpu_gpu_parity_maybe)` | `mod.rs:4709` | 1 | Plugin::build |
| 16 | `ConstructionGpu::prepare_probe_history: Option<Buffer>` field + docblock | `mod.rs:148-155` | 8 | written by `prepare_construction` allocator + bound in bounds_calc; read by item 5 above (DELETED) |
| 17 | `ConstructionBindGroups::prepare_probe_history` field | `mod.rs:441-443` | 3 | written by `prepare_construction`; read by `bounds_calc.rs::queue_prepare_pipeline` |
| 18 | `ConstructionPipelines::prepare_probe_history_layout` field | (verify line) | ~3 | written by `bounds_calc::queue_prepare_pipeline` |
| 19 | `PREPARE_PROBE_HISTORY_ENTRIES` const + docblock | `mod.rs:340` | 14 | only used by probe machinery (DELETED) |
| 20 | `PREPARE_PROBE_HISTORY_BYTES` const | `mod.rs:343-344` | 2 | only used by probe allocator (DELETED) |
| 21 | `bounds_calc.wgsl` probe-write block (lines 418-423) + the `prepare_probe_history` storage binding declaration | `bounds_calc.wgsl:418-423` (write) + the binding decl above | ~10 | written by `prepare_group_bounds` shader; read by item 5 (DELETED). Removing the write also removes the only consumer of `@group(3)` on `prepare_group_bounds` |
| 22 | `prepare_probe_history_layout_descriptor` fn in `bounds_calc.rs` + its `BindGroupLayoutEntries` definition | `bounds_calc.rs` | ~25 | only used by item 18 |
| 23 | `prepare_construction`'s probe-allocation block (verify line range — somewhere inside the 1 418 LOC body) | `mod.rs:~1900-2000` est. | ~50 | only used by items 5 + 17 |

**Total deletion**: ~1 170 LOC (+ register clean-up). The arithmetic
matches the audit's "~1 100 LOC" figure for the three probes + their
infrastructure.

**Verification command for the implementor**: after deletion, the following
greps must return zero:
```
grep -rn "AadfDelayedProbe\|aadf_delayed_probe" /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src
grep -rn "AadfPerCallProbe\|aadf_per_call_probe\|PerCallProbeStage" /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src
grep -rn "AadfCpuGpuParity\|aadf_cpu_gpu_parity\|CpuGpuParityStage" /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src
grep -rn "prepare_probe_history\|PREPARE_PROBE_HISTORY" /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf
```

---

## D4↔D5 shared-file notes

Per the W0 seam contract, D5 treats `render/gpu_types.rs`, `render/prepare.rs`,
and `render/pipelines.rs::NaadfPipelines` as read-only. The following items
this explorer identified would WANT changes in those files — flagged here
for D4's implementor when D4 runs (post-D5).

1. **`render/gpu_types.rs::GpuConstructionParams`** (the construction-side
   per-frame uniform — verified at `gpu_types.rs:?` — referenced by
   `chunk_calc.rs:45`, `bounds_calc.rs:57`, etc. via
   `use crate::render::gpu_types::GpuConstructionParams`). If Finding 9
   splits `ConstructionGpu` per-workstream, the construction-params uniform
   may want a per-workstream split too (each workstream's params subset).
   **D5 can NOT propose this change**. Flag for D4 architect.
2. **`render/prepare.rs::prepare_world_gpu`** is the `.after(...)` ordering
   target for `prepare_construction`. If Finding 1 splits the prepare into
   per-workstream prepares, each new prepare-system needs its
   `.after(prepare_world_gpu)` ordering edge — D5 owns this edge (the
   `add_systems` call lives in `ConstructionPlugin::build`), so this is a
   **non-edit** ripple: D5 references `prepare_world_gpu`'s ident by
   path, the function itself is untouched.
3. **`render/prepare.rs::WorldGpu::chunks_buffer` / `blocks_buffer` /
   `voxels_buffer`** are the production GPU buffer handles every D5
   bind group ties to. If D4's refactor renames any of these fields,
   every D5 bind-group construction site breaks. Flag for D4 architect:
   if D4 considers renaming WorldGpu fields, coordinate the rename
   with D5's implementor session (sequenced — D4 runs after D5 per
   user decision).
4. **`render/pipelines.rs::NaadfPipelines`** is referenced by D5 only
   through `init_gpu_resource` order. The W0 contract explicitly forbids
   `ConstructionPipelines` field set from merging into `NaadfPipelines`.
   Finding 10 (splitting `ConstructionPipelines` per-workstream) is a
   D5-internal change that does not touch this contract.

**No D5 → D4 shared-file edits.** Every refactor proposed above stays
inside `render/construction/` + the construction WGSL.

---

## Side notes / observations / complaints

1. **The W0 seam contract is now scaffolding from a completed
   orchestration.** Three pieces of `15-design-c.md` §1.1 that were
   load-bearing during parallel-merge are now structurally inert:
   (a) **the empty-shell `ConstructionGpu` + `ConstructionBindGroups`
   + `ConstructionPipelines`** existed to make every workstream's first
   PR a tiny seam-only PR — that's done; (b) **the single
   `prepare_construction` system** existed to keep workstream merge
   conflicts confined to one file — that's done; (c) **the
   non-edit-NaadfPipelines** rule existed to keep render-side pipelines
   stable while construction-side churned — that's done.

   Finding 1 (split prepare_construction), Finding 9 (split
   ConstructionGpu), and Finding 10 (split ConstructionPipelines) all
   propose retiring this scaffolding. The architect should treat the
   W0 contract as **history, not current law** — the seam exists in
   `docs/orchestrate/naadf-bevy-port/15-design-c.md` and was
   load-bearing for ~6 months; what we have today is the "frozen
   parallel-merge layout" that pinned everything in place during the
   wave-3 integration. We can now thaw it.

2. **The W0 contract DID achieve its purpose.** The reuse audit
   (`00-reuse-audit.md` side-note #5) flagged the D4↔D5 shared seam as
   "tight but not airtight" — verified during this exploration that D5's
   construction submodules touch `render/gpu_types.rs` ONLY via the
   single `GpuConstructionParams` use-statement (verified: each of
   `chunk_calc.rs:45`, `bounds_calc.rs:57`, `world_change.rs`, etc. has
   one `use crate::render::gpu_types::GpuConstructionParams`).  No D5
   submodule reaches into `render/prepare.rs::WorldGpu`'s field set or
   `render/pipelines.rs::NaadfPipelines`. The seam held. Credit where
   credit is due.

3. **`shader_drift_guard.rs` is a smell pointing at a deeper Bevy/naga
   problem.** Inline-duplicating 150 LOC of WGSL across 2-3 files +
   shipping a 400-LOC parser to defend the duplication is the
   architectural equivalent of "the import system doesn't work, so we
   wrote a build-time linter." If Bevy 0.19's `naga_oil` shader-import
   works for the WGSL forms used (`var<workgroup>` array, `atomic<u32>`,
   custom structs), this is hundreds of LOC removable + ongoing
   maintenance overhead eliminated. Architect should investigate.
   **If naga_oil import doesn't support these constructs** — then
   `shader_drift_guard.rs` is load-bearing and we should LEAVE it
   alone, full stop.

4. **The `populate_cpu_mirror_from_gpu_producer` 566-LOC
   ExtractSchedule system is questionable but not in D5's stinky-foundation
   list.** It hand-rolls a 4-stage state machine (NotStarted →
   CursorPending → FullSetPending → Done) with `Arc<AtomicBool>` /
   `map_async` because Bevy doesn't ship a render-graph-readback primitive
   that handles cross-frame readback cleanly. This is downstream of a
   real Bevy gap and probably can't be tightened without either (a)
   Bevy 0.20 shipping a readback primitive, or (b) us writing one
   ourselves — both out of scope for D5's refactor. **Leave as-is**;
   it deserves to live in its own `readback.rs` (per the split sketch)
   but the body itself is not stinky, it's just long because the
   bug-class is awkward.

5. **`naadf_gpu_producer_node` (434 LOC, mod.rs:3076) is the W5
   producer chain — and it ALSO dispatches W1's chunk_calc chain.** It's
   not just a W5 node; it's the "regime-1 startup driver" that lives
   inside a `Core3d`-schedule node. The audit's "470-line node-system
   with a 3-way ladder (W5 path / chunk-calc path / CPU fallback)"
   description is accurate. Per the split sketch this becomes
   `producer.rs`. **But it should probably move to `generator_model.rs`
   or a sibling `regime_1.rs`** — calling it `naadf_gpu_producer_node`
   is honest but the file boundary is the question. Architect to
   decide.

6. **The "Phase-C followup #1" block inside `prepare_construction`
   (mod.rs:1701-2200 est.) is itself a misplaced concern.** That code
   says: *"when `gpu_construction_enabled = true` AND the producer has
   not yet run, allocate the FULL hash_map / segment_voxel_buffer /
   hash_coefficients / block_voxel_count buffers."* It's the runtime
   counterpart of the W5 startup-driver — it allocates production-size
   buffers so the W5 dispatch inside `naadf_gpu_producer_node` (a
   different file location!) finds them ready. **This split between the
   allocator (in prepare) and the dispatcher (in producer node) is
   itself structurally suspect.** Each workstream owns dispatch +
   prepare-side; the W5 dispatch in the producer node should probably
   be co-located with W5's prepare-side allocation.

7. **DUP-7 understated; the encoder family is 6 functions.** The audit
   called out 2 `build_segment_voxel_buffer*`; this exploration found
   six related encoders/decoders (Finding 6). Architects re-doing the
   reuse audit for D5 should grep `build_segment_voxel_buffer\|decode_segment_voxels`
   not just `build_segment_voxel_buffer`.

8. **The audit's SSoT-6 claim was wrong about WGSL.** The hash coefficients
   are uploaded as a storage buffer to WGSL, not declared as an array literal
   in shader source. Two Rust copies, not three sites. Architects relying
   on the audit verbatim would mis-scope SSoT-6 work. Finding 8 corrects.

9. **`ConstructionConfig` (`config.rs:326`) is sane and well-organized.**
   No findings against it. `feedback-ssot-vs-agentic-divergence` was
   one of the relevant rules for D5; ConstructionConfig already collapses
   the construction-side knobs to a single SSoT (the C# `WorldData.cs:73`
   constants live in `config.rs::ConstructionConfig::default()`). Good.

10. **The embedded test modules (`mod tests`, `mod tests_w1`, `mod tests_w4`)
    are GPU↔CPU oracle tests — load-bearing per `15-design-c.md` §1.6
    and the user's Q2 directive ("cpu oracle stays").** Do not delete.
    The relocation in Finding 3's `validation/` sketch is just a move
    to make the file boundaries sensible. They could equivalently move
    into the workstream submodules they test (`tests_w1` into
    `chunk_calc.rs::tests`, `tests_w4` into `entity_update.rs::tests`,
    `tests` (W5) into `generator_model.rs::tests`). That's the more
    Rust-idiomatic placement — each module owns its tests.

11. **Equal-footing complaint:** the user directive forbids `cargo run
    --bin bevy-naadf` for verification and routes everything through
    `e2e_render`. The verification gates for D5's refactor are
    `--validate-gpu-construction` + `--validate-gpu-construction-scaled`
    + `--edit-mode` + `--entities` + `--runtime-edit-mode` +
    `--oasis-edit-visual` + `cargo test --workspace --lib`. **The
    relocation of `validate_*` functions in Finding 3 must preserve their
    `pub` path (only the `::construction::validate_*` segment changes to
    `::construction::validation::validate_*`) so `bin/e2e_render.rs` only
    needs an import-path change, not a flag rename.** D5's
    implementor: when you move these, do it as a path rename without a
    behavioural change, and update `bin/e2e_render.rs` use-paths in the
    same commit (`bin/e2e_render.rs` is D6/D7 territory but a path-rename
    follow-edit on a moved item is structural, not behavioural).
