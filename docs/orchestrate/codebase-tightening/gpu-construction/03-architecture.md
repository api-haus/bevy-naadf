# D5 — GPU construction (Phase-C) architecture

**Author**: delegate-architect (codebase-tightening — D5 / gpu-construction).
**Date**: 2026-05-20.
**Scope**: `crates/bevy_naadf/src/render/construction/**` (15 821 Rust LOC
across 11 files) + the 7 construction WGSL files (2 343 LOC). Implementor
treats `render/gpu_types.rs`, `render/prepare.rs`,
`render/pipelines.rs::NaadfPipelines` as **read-only** (W0 seam contract;
D4's territory).
**Required reading consumed**: `01-context.md` (incl. 2026-05-20 addendum
Resolutions A/B/C/D), `00-reuse-audit.md §2 D5 row + §3.1 SSoT-6 + §3.2
DUP-4/DUP-5/DUP-7 + §3.3 BEV-3/BEV-6 + §3.4 OA-2 + side-note #7`,
`gpu-construction/02-exploration.md` (full — all 11 findings),
`render-pipeline/02-exploration.md` §D4↔D5 shared-file notes,
`aadf-data-structures/02-exploration.md` §SSoT-6,
`naadf-bevy-port/15-design-c.md` §§1.1, 1.3, 1.4 (the W0 seam contract being
retired), CLAUDE.md (e2e-gate authoring discipline).

The 11 043-LOC `render/construction/mod.rs` shrinks to **~620 LOC**. Total
D5 Rust falls from 15 821 LOC to **~10 700 LOC** (~4 600 LOC down) plus
~30 WGSL LOC retired with the probe deletion. Zero behavioural change.

---

## 1. Findings addressed

The exploration produced 11 findings; this design covers all 11, grouped
where the same migration step lands several.

| # | one-liner | severity | covered in |
|---|---|---|---|
| 1 | `prepare_construction` is a 1 418-LOC god-system | high | §2.1, Step 4 |
| 2 | DIAG probes — DELETE outright | high | §2.2, Step 1 |
| 3 | E2E `validate_*` gates living inside production mod.rs | high | §2.3, Step 5 |
| 4 | `clear_world_data_pending_edits` no-op stub | medium | §2.4, Step 1 |
| 5 | `else { return; }` ladders vs `.run_if(resource_exists::<_>)` | medium | §2.5, Step 6 |
| 6 | Six segment-voxel-buffer encoders/decoders | medium | §2.6, Steps 5 + 7 |
| 7 | WGSL inline copies + `shader_drift_guard.rs` | medium | §2.7 — **stays put**, no change |
| 8 | SSoT-6 hash coefficients (D5 owns its copy) | medium | §2.8, Step 7 |
| 9 | `ConstructionGpu` 28-`Option<…>` god-resource (BEV-3) | low | §2.9 — **deferred**, see §6 |
| 10 | `ConstructionPipelines` empty-sibling (OA-2 / Resolution D) | low | §2.10, Step 3 |
| 11 | WGSL `4u`/`64u` literals shadowing `CELL_DIM`/`CELL_CHILDREN` | low | §2.11, Step 8 |

**Nothing the brief asked for is skipped.** Findings 7 + 9 are explicitly
designed to **stay in place this dispatch** (Finding 7: load-bearing per
explorer side-note #3 unless naga-oil status changes; Finding 9: the
`Option<…>` field-set decomposition is a "next refactor" target — see §6
"Decisions & rejected alternatives" for the rationale).

---

## 2. Target-state architecture

### 2.1 Finding 1 — `prepare_construction` 1 418-LOC god-system

**Current shape (verified):**

`render/construction/mod.rs:1658-3075` — one `pub fn prepare_construction(...)`
with **11 system parameters** (4 of them `Option<Res<…>>` /
`Option<ResMut<…>>`) plus internal `// === W3 ===`, `// === W5 ===`,
`// === W2 ===`, `// === W4 wave-3 ===` section dividers
(`mod.rs:1913, 2216, 2418, 2713`) and a "Phase-C followup #1 — runtime GPU
producer pre-allocation" prelude at `mod.rs:1701-1912` that is itself ~210
LOC. The body opens with four sequential bails
(`mod.rs:1684-1699`).

```rust
#[allow(clippy::too_many_arguments)]
pub fn prepare_construction(
    mut commands: Commands,
    gpu: Option<ResMut<ConstructionGpu>>,
    bind_groups: Option<ResMut<ConstructionBindGroups>>,
    world_gpu: Option<ResMut<crate::render::prepare::WorldGpu>>,
    construction_pipelines: Option<Res<ConstructionPipelines>>,
    construction_config: Res<config::ConstructionConfig>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    construction_events: Option<Res<ConstructionEvents>>,
    world_data_meta: Option<Res<crate::render::extract::WorldDataMeta>>,
    model_data: Option<Res<crate::render::extract::ModelDataRender>>,
) { /* ... 1 418 lines ... */ }
```

**Target shape:**

Six sibling systems registered in `PrepareResources`, each
`.after(prepare_world_gpu)` and each in its own workstream submodule. The
shared "ensure resources exist" block becomes a tiny dispatch shell.

```rust
// render/construction/mod.rs — orchestration only.
pub fn prepare_construction_resources(
    mut commands: Commands,
    gpu: Option<Res<ConstructionGpu>>,
    bind_groups: Option<Res<ConstructionBindGroups>>,
) {
    if gpu.is_none() {
        commands.insert_resource(ConstructionGpu::default());
    }
    if bind_groups.is_none() {
        commands.insert_resource(ConstructionBindGroups::default());
    }
}

// chunk_calc::prepare — W1 family.
pub fn prepare_chunk_calc(/* … */) { /* … */ }

// bounds_calc::prepare — W3 family.
pub fn prepare_bounds_calc(/* … */) { /* … */ }

// world_change::prepare — W2 family.
pub fn prepare_world_change(/* … */) { /* … */ }

// entity_update::prepare — W4 family.
pub fn prepare_entity_update(/* … */) { /* … */ }

// generator_model::prepare — W5 family + the "Phase-C followup #1"
// runtime producer pre-allocation (currently mod.rs:1701-1912) which is
// genuinely W5-shaped: it allocates `segment_voxel_buffer`, `hash_map`,
// `hash_coefficients`, `block_voxel_count` that the W5 dispatch + the W1
// chunk_calc chain consume.
pub fn prepare_generator_model(/* … */) { /* … */ }

// producer::prepare_shared_bind_groups — the two shared bind groups
// (`construction_world` consuming W1 buffers, `construction_bounds_world`
// consuming the chunks texture + bounds params uniform). Runs after every
// W1/W3 prepare so the source buffers are populated; if its preconditions
// aren't met, it `return`s — same shape as the prior monolithic system's
// late blocks. Kept central because the bind groups span workstreams.
pub fn prepare_shared_bind_groups(/* … */) { /* … */ }
```

System registration in `ConstructionPlugin::build` (`render_app`):

```rust
.add_systems(
    Render,
    (
        prepare_construction_resources,
        // Per-workstream prepares — chained against the resource-init shell
        // so they see the default-inserted resources on first frame.
        (
            chunk_calc::prepare_chunk_calc,
            bounds_calc::prepare_bounds_calc,
            world_change::prepare_world_change,
            generator_model::prepare_generator_model,
            entity_update::prepare_entity_update
                .run_if(|cfg: Res<ConstructionConfig>| cfg.entities_enabled),
        )
            .after(prepare_construction_resources)
            .after(crate::render::prepare::prepare_world_gpu),
        // The shared bind-group builder runs last — every workstream's
        // buffers must exist before we can wire them into a bind group.
        prepare_shared_bind_groups
            .after(chunk_calc::prepare_chunk_calc)
            .after(bounds_calc::prepare_bounds_calc)
            .after(world_change::prepare_world_change)
            .after(generator_model::prepare_generator_model),
    )
        .in_set(RenderSystems::PrepareResources),
)
```

**Reuse choices:**

- Each workstream submodule (`chunk_calc.rs`, `bounds_calc.rs`,
  `world_change.rs`, `entity_update.rs`, `generator_model.rs`) already
  owns its layout descriptors + pipeline-queueing helpers
  (`chunk_calc.rs:61,95,121,145`, `bounds_calc.rs:77,128,162,205,233,272`,
  `world_change.rs:62,102,137,172,207`,
  `entity_update.rs:88,112,129,160,191`,
  `generator_model.rs:131,151,202`). The matching prepare-side block in
  `mod.rs::prepare_construction` is migrated **next to the dispatch and
  layout-descriptor helpers** it serves — same submodule, same `// === W{N}
  …` section divider grammar.
- The `.after(prepare_world_gpu)` ordering edge transfers verbatim from
  the existing single registration to every new per-workstream `add_systems`
  call. `prepare_world_gpu` is a D4-owned read-only symbol; we only cite
  its path.
- `Commands::insert_resource` for `ConstructionGpu::default()` /
  `ConstructionBindGroups::default()` mirrors the W0-seam pattern at
  `mod.rs:1684-1693` exactly.

**Behavioural delta:**

None. Pure structural relocation. The Bevy scheduler runs the six new
systems in **the same logical order** as the old monolith's section
dividers because we encode the order with `.after(...)`. Cross-workstream
data flow (W3 prepare reads the W1 hash buffers existing) becomes a real
schedule dependency rather than an implicit body-order one.

The one possible delta is: the old monolith's `else { return; }` at
`mod.rs:1698` ("`let Some(world_gpu) = world_gpu else { return; };`")
short-circuited **every workstream's allocation** when `WorldGpu` was
missing. In the new shape each per-workstream prepare has its own
short-circuit on the resources it needs — so on a frame where, say, W3 is
ready but W1 hasn't received its `WorldGpu` yet, W3 still runs. **This
matches the W0 seam's *original* design intent** (each workstream prepares
its own family) and is not observable in any current test: `WorldGpu` is
inserted by `prepare_world_gpu` deterministically on the same frame, and
the `.after(prepare_world_gpu)` ordering edge guarantees it's present
when each new per-workstream prepare runs.

---

### 2.2 Finding 2 — Diagnostic probes: DELETE outright

**Current shape (verified):**

Three diagnostic resources + their systems + their registrations + the
shared GPU probe-history infrastructure. Full caller-audit table at
`02-exploration.md` "Diagnostic-probe deletion list" — every cited
line:range verified by Read/Grep before this design landed.

- `AadfDelayedProbe` (`mod.rs:3559-3582` struct, `:3594-3870` system).
- `AadfPerCallProbe` + `PerCallProbeStage` (`mod.rs:3873-3914` struct/enum,
  `:3915-4087` system).
- `AadfCpuGpuParity` + `CpuGpuParityStage` + `aadf_cpu_gpu_parity_maybe`
  (`mod.rs:4088-4596` struct/enum/system, `:4597-4617` wrapper).
- 3 `.init_resource` (`mod.rs:4653, 4655, 4657`) + 3
  `.add_systems(ExtractSchedule, _)` (`mod.rs:4694, 4699, 4709`) inside
  `impl Plugin for ConstructionPlugin`.
- `ConstructionGpu::prepare_probe_history: Option<Buffer>` (`mod.rs:148-155`).
- `ConstructionBindGroups::prepare_probe_history: Option<BindGroup>`
  (`mod.rs:441-443`).
- `ConstructionPipelines::prepare_probe_history_layout: BindGroupLayoutDescriptor`
  (`mod.rs:528-534`) + its build in `from_world` (`mod.rs:627-628`) + its
  struct-literal entry (`mod.rs:725`).
- `PREPARE_PROBE_HISTORY_ENTRIES: u32 = 256` (`mod.rs:326-340`) +
  `PREPARE_PROBE_HISTORY_BYTES` (`mod.rs:341-344`).
- `bounds_calc.rs::prepare_probe_history_layout_descriptor`
  (`bounds_calc.rs:191-199`).
- `bounds_calc::queue_prepare_pipeline` signature carries
  `probe_layout: BindGroupLayoutDescriptor` (`bounds_calc.rs:233-249`),
  passed down to `queue_prepare_pipeline_with_handle`
  (`bounds_calc.rs:252-267`) — both signatures lose the parameter.
- `bounds_calc.rs:230-232` docblock referencing `@group(3)`.
- `bounds_calc.rs:370` "group 3 = prepare_probe_history" comment.
- `bounds_calc.rs:492-507` opener fetches `probe_bg` + a
  `bound_dispatch_indirect`-shaped early-return ladder; the probe bail
  goes.
- `bounds_calc::naadf_bounds_compute_node` body passes `probe_bg` to
  `dispatch_regime_2_rounds` (`bounds_calc.rs:562-574`) — `probe_bg`
  parameter drops.
- WGSL `bounds_calc.wgsl:160-176` `@group(3) @binding(0)` declaration +
  WGSL `bounds_calc.wgsl:405-433` `if (probe_call_idx < probe_capacity_entries)
  { /* writes */ }` block.
- The `prepare_construction` body's W3 "bound queue family + bind groups"
  block (`mod.rs:1913-2215`) allocates `prepare_probe_history` as part of
  the W3 buffer family — that allocation line + the matching bind-group
  build line co-deletes.
- `render/construction/bounds_calc/tests.rs` imports + uses both
  `PREPARE_PROBE_HISTORY_BYTES` and `PREPARE_PROBE_HISTORY_ENTRIES`
  (`tests.rs:42,427,524-617`) — the test's local `probe_layout` /
  `prepare_probe_history` Buffer / probe-bind-group / 4-layout `vec!`
  passed into `queue_prepare_pipeline_with_handle` all delete in
  lockstep with the source change.

**Caller audit (verified again at design time):**

`grep -rn "AadfDelayedProbe\|aadf_delayed_probe\|AadfPerCallProbe\|aadf_per_call_probe\|AadfCpuGpuParity\|aadf_cpu_gpu_parity"` returns matches only inside `mod.rs`.
`grep -rn "prepare_probe_history\|PREPARE_PROBE_HISTORY"` returns matches
inside `mod.rs` (~5), `bounds_calc.rs` (~5), `bounds_calc/tests.rs` (~12),
and `bounds_calc.wgsl` (~8). All inside D5; zero external consumers.

**Target shape:**

All probe-related code DELETED outright (Resolution A — aggressive per
addendum). No feature gate, no `#[cfg]`. Specifically:

- `ConstructionGpu::prepare_probe_history` field gone.
- `ConstructionBindGroups::prepare_probe_history` field gone.
- `ConstructionPipelines::prepare_probe_history_layout` field gone.
- `PREPARE_PROBE_HISTORY_ENTRIES` / `PREPARE_PROBE_HISTORY_BYTES` consts gone.
- `bounds_calc.rs::prepare_probe_history_layout_descriptor` fn gone.
- `queue_prepare_pipeline` + `queue_prepare_pipeline_with_handle`
  signatures shrink: drop the `probe_layout: BindGroupLayoutDescriptor`
  parameter; the pipeline's `layout` vec goes
  `vec![world, bounds, dispatch, probe] → vec![world, bounds, dispatch]`.
- `naadf_bounds_compute_node` early-return for `probe_bg` gone; its
  `dispatch_regime_2_rounds` invocation drops the `probe_bg` argument
  (and `dispatch_regime_2_rounds`'s signature drops it too — verified at
  `bounds_calc.rs:347`).
- `bounds_calc.wgsl:160-176` (binding declaration) + `:405-433` (the
  per-call write block) deleted.
- All three `.init_resource::<Aadf*Probe>()` + all three
  `.add_systems(ExtractSchedule, aadf_*)` lines in
  `ConstructionPlugin::build` deleted.
- `bounds_calc/tests.rs:42,427,524-617` co-edited: drop the `probe_layout`
  call into `queue_prepare_pipeline_with_handle`, drop the local
  `prepare_probe_history` Buffer allocation, drop its bind-group, shorten
  the `vec![]` of layouts to 3.

**Reuse choices:** none — pure deletion.

**Behavioural delta:**

None observable. The WGSL probe writes are unconsumed bytes on every
frame after deletion → small wgpu pipeline cost reduction (~one less
storage-buffer binding on the `prepare_group_bounds` pipeline; the wasm
"4 bind groups exactly at the limit" cited in
`bounds_calc.rs:187-190` now becomes "3 bind groups — comfortably under
the limit"). No e2e gate touches probe output: `--validate-gpu-construction*`
gates assert against the CPU oracle byte-for-byte (`mod.rs:4928,5290,5621`),
not against probe data. Probes were drained by `aadf_per_call_probe` via
`info!()` log lines — they didn't influence any state observable to the
e2e harness.

---

### 2.3 Finding 3 — E2E `validate_*` gates leave production mod.rs

**Current shape (verified):**

Six `pub fn validate_*` functions + the helpers they reach, occupying
`mod.rs:4727-9517` (~4 640 LOC + ~85 LOC of intermediate helpers). Each
calls into one of these:

- `--validate-gpu-construction` → `mod.rs::validate_gpu_construction`
  (`mod.rs:4928`, called from `bin/e2e_render.rs:419-420`).
- `--validate-gpu-construction-scaled` →
  `mod.rs::validate_gpu_construction_scaled` (`mod.rs:5290`, called from
  `bin/e2e_render.rs:213-214`).
- `--validate-gpu-construction-production-scale` →
  `mod.rs::validate_gpu_construction_production_scale` (`mod.rs:5621`,
  called from `bin/e2e_render.rs:229-230`).
- `--edit-mode` → `mod.rs::validate_edit_mode` (`mod.rs:9168`, called
  from `bin/e2e_render.rs:451-452`).
- `--runtime-edit-mode` → `mod.rs::validate_runtime_edit_mode`
  (`mod.rs:9305`, called from `bin/e2e_render.rs:468-469`).
- `--entities` → `mod.rs::validate_entity_handler` (`mod.rs:9450`, called
  from `bin/e2e_render.rs:439-440`).

Internal-only helpers (zero callers outside `mod.rs`, verified by
`grep -rn` per exploration):

- `discover_populated_oasis_voxels` (`mod.rs:5481`).
- `impl VoxelReadback` + `readback_cursor` + `map_single_u32` +
  `map_single_pair` + `sample_voxel_readback` + `render_results_table`
  (`mod.rs:6335-6622`).
- `run_one_fixture_byte_diff` / `run_one_fixture_multiseg_byte_diff` /
  `run_one_generator_model_byte_diff` / `run_one_tiled_byte_diff` /
  `run_oasis_segment_byte_diff` (`mod.rs:6623,7134,7606,7832,8408`).
- `decode_segment_voxels_into_volume` (`mod.rs:8331`),
  `load_oasis_model_data` (`mod.rs:8390`),
  `decode_segment_voxels_to_volume` (`mod.rs:8904`),
  `build_mixed_model_data` (`mod.rs:8958`),
  `build_segment_voxel_buffer_for_region` (`mod.rs:9016`),
  `chunk_kind`/`block_kind` (`mod.rs:9059,9067`),
  `build_segment_voxel_buffer_for_world` (`mod.rs:9079`),
  `built_pre_edit_state` (`mod.rs:9443`).
- `build_segment_voxel_buffer` (`mod.rs:4820`) — test-only encoder used
  by `validate_gpu_construction` (`mod.rs:4998`) and the embedded
  `mod tests`.

**Target shape:**

New submodule `render/construction/validation/` housing the six gate
functions + every helper they reach. **Path-rename only**; signatures
and bodies untouched.

```
render/construction/validation/
├── mod.rs                          ── ~80 LOC — pub use re-exports for
│                                       bin/e2e_render.rs.
├── gpu_construction.rs             ── ~420 LOC — validate_gpu_construction
│                                       + build_segment_voxel_buffer
│                                       (test-only encoder, moves with it).
├── gpu_construction_scaled.rs      ── ~210 LOC — validate_gpu_construction_scaled
│                                       + discover_populated_oasis_voxels.
├── gpu_construction_production.rs  ── ~990 LOC — validate_gpu_construction_production_scale
│                                       + VoxelReadback / readback_cursor /
│                                       map_single_u32 / map_single_pair /
│                                       sample_voxel_readback / render_results_table /
│                                       load_oasis_model_data /
│                                       run_oasis_segment_byte_diff /
│                                       decode_segment_voxels_to_volume /
│                                       decode_segment_voxels_into_volume /
│                                       build_mixed_model_data /
│                                       build_segment_voxel_buffer_for_region /
│                                       build_segment_voxel_buffer_for_world /
│                                       chunk_kind / block_kind.
├── byte_diff_fixtures.rs           ── ~1 720 LOC — run_one_fixture_byte_diff +
│                                       run_one_fixture_multiseg_byte_diff +
│                                       run_one_generator_model_byte_diff +
│                                       run_one_tiled_byte_diff. Internal-only
│                                       (pub(crate) only inside validation/).
├── edit_mode.rs                    ── ~140 LOC — validate_edit_mode +
│                                       built_pre_edit_state helper.
├── runtime_edit_mode.rs            ── ~140 LOC — validate_runtime_edit_mode.
└── entity_handler.rs               ── ~70 LOC — validate_entity_handler.
```

`validation/mod.rs` shape:

```rust
//! E2E-harness gate fixtures. `pub(crate)` visibility is sufficient
//! because `bin/e2e_render.rs` is the same crate; the only callers are
//! the matching CLI-flag arms.
mod byte_diff_fixtures;
mod edit_mode;
mod entity_handler;
mod gpu_construction;
mod gpu_construction_production;
mod gpu_construction_scaled;
mod runtime_edit_mode;

pub(crate) use edit_mode::validate_edit_mode;
pub(crate) use entity_handler::validate_entity_handler;
pub(crate) use gpu_construction::validate_gpu_construction;
pub(crate) use gpu_construction_production::validate_gpu_construction_production_scale;
pub(crate) use gpu_construction_scaled::validate_gpu_construction_scaled;
pub(crate) use runtime_edit_mode::validate_runtime_edit_mode;
```

Then `render/construction/mod.rs` re-exports them at the `pub` level the
existing `bin/e2e_render.rs` call-sites expect (path is
`bevy_naadf::render::construction::validate_*`):

```rust
// render/construction/mod.rs
pub mod validation;
pub use validation::{
    validate_edit_mode, validate_entity_handler, validate_gpu_construction,
    validate_gpu_construction_production_scale, validate_gpu_construction_scaled,
    validate_runtime_edit_mode,
};
```

This keeps `bin/e2e_render.rs:214,230,420,440,452,469` unchanged. **No
edit in `bin/e2e_render.rs`** — important because the e2e harness is the
project's verification surface (CLAUDE.md) and changing it from a
structural-only refactor risks Playwright + test-mode-CLI churn.

**Reuse choices:**

- `pub(crate)` visibility for inner items (the six `validate_*` re-exports
  at `mod.rs` top-level keep `pub` for external callers).
- Existing `mod tests`, `mod tests_w1`, `mod tests_w4` (the embedded
  GPU↔CPU oracle tests, `mod.rs:9518, 9813, 10605`) move alongside —
  see Step 5 for placement (these are unit tests, not e2e gates; they
  land in the workstream submodules whose dispatch they test).

**Behavioural delta:**

None. Pure file move + visibility tightening. Every gate's body is
copied verbatim with its imports re-pathed.

---

### 2.4 Finding 4 — `clear_world_data_pending_edits` no-op stub

**Current shape (verified):**

```rust
// mod.rs:836-838
pub fn clear_world_data_pending_edits(_world_data: Option<ResMut<crate::world::data::WorldData>>) {
    // No-op — drain moved into `extract_world_changes` (see doc above).
}

// mod.rs:4642 (inside ConstructionPlugin::build)
app.add_systems(Last, clear_world_data_pending_edits);
```

The 16-line docblock above the function explicitly tags this as
"left for a follow-up dispatch" (`mod.rs:819-835`). Zero external
callers.

**Target shape:**

Delete the fn, its docblock, and the `add_systems(Last, ...)`
registration. Restated: no replacement; the drain already lives inside
`extract_world_changes` (`mod.rs:910-1048`).

**Reuse choices:** none — pure deletion.

**Behavioural delta:**

None. The system is documented-no-op and runs in the `Last` schedule on
the main world; its deletion drops one scheduler walk per frame and
saves the `Option<ResMut<WorldData>>` query overhead per frame. No e2e
gate depends on it; no schedule order edge cites it.

---

### 2.5 Finding 5 — `else { return; }` ladders → `.run_if(resource_exists::<_>)`

**Current shape (verified):**

Counted at design time: 20 `else { return; }` patterns in `mod.rs`
(`grep -c "else { return; }" mod.rs` = 20). Of these:

| location | pattern | resource(s) | status |
|---|---|---|---|
| `mod.rs:1109` | `populate_cpu_mirror_from_gpu_producer` opener bail on `gpu` | `ConstructionGpu` | **convert** |
| `mod.rs:1168` | same fn, bail on `world_gpu` | `WorldGpu` | **convert** |
| `mod.rs:1684,1691` | `prepare_construction` ensure-init for `gpu`/`bind_groups` | n/a (insert if missing) | **stays inside `prepare_construction_resources` shell** |
| `mod.rs:1698` | `prepare_construction` bail on `world_gpu` | `WorldGpu` | becomes per-workstream — see §2.1 |
| `mod.rs:1699` | `prepare_construction` bail on `construction_pipelines` | `ConstructionPipelines` (→ merged with `NaadfPipelines`, see §2.10) | becomes per-workstream |
| `mod.rs:3102-3111` | `naadf_gpu_producer_node` opens with 6 sequential bails | `ConstructionConfig`, `ConstructionGpu`, `ConstructionPipelines`, `ConstructionBindGroups` | partial convert — see below |
| `mod.rs:3604,3614,3620,3621,3697-3699` | `aadf_delayed_probe` body bails | n/a (DELETED — Finding 2) | gone |
| `mod.rs:3924,3937` | `aadf_per_call_probe` body bails | n/a (DELETED) | gone |
| `mod.rs:4143,4156` | `aadf_cpu_gpu_parity` body bails | n/a (DELETED) | gone |
| `world_change.rs:370-373` | `naadf_world_change_node` opens with 4 bails | `ConstructionPipelines`, `ConstructionBindGroups`, `ConstructionGpu`, `ConstructionConfig` | **convert** |
| `bounds_calc.rs:464-467` | `naadf_bounds_compute_node` opens with 4 bails | (same as above) | **convert** |

**Target shape:**

System registrations gain `.run_if(...)` clauses that express the
preconditions declaratively at scheduler-build time:

```rust
// inside ConstructionPlugin::build's render_app .add_systems calls
.add_systems(ExtractSchedule, (
    extract_world_changes,
    populate_cpu_mirror_from_gpu_producer
        .run_if(resource_exists::<ConstructionGpu>)
        .run_if(resource_exists::<crate::render::prepare::WorldGpu>),
))

// Producer + bounds-compute + world-change nodes use the same idiom in
// the render-graph add_systems call (currently inside
// render/mod.rs:300-326 — D4-owned; this design does NOT touch that
// file). D5's piece: the `Option<Res<_>>` parameters on each *node*
// become `Res<_>` once the scheduler skips them via `.run_if`, but the
// node signature change happens in D4's later impl phase. **For D5's
// impl phase we keep the `Option<Res<_>>` signatures and only convert
// the body's bails to `?` or to `else { return; }` deduplication.**
```

Critical: the **render-graph nodes** registered in `render/mod.rs:300-326`
are NOT in D5's edit scope (per `01-context.md` §"Forbidden moves" #7
and D4↔D5 shared-file rules). So `.run_if(...)` clauses on
`naadf_bounds_compute_node` / `naadf_world_change_node` /
`naadf_entity_update_node` / `naadf_gpu_producer_node` are **D4
territory** — D5's impl phase flags this to D4 (see §5 "D4 coordination
notes"). D5's impl phase converts only the systems D5 owns the
registration of: the extract-schedule pair (`extract_world_changes`,
`populate_cpu_mirror_from_gpu_producer`), the
`prepare_construction_resources` shell, and the per-workstream `prepare_*`
systems.

For the producer-node and the construction node bodies (where the
`.run_if` move is D4-blocked), this design **collapses sequential bails
into a single `let-else` chain that early-returns**. Visual cleanup
without changing the schedule-side contract:

```rust
// bounds_calc.rs::naadf_bounds_compute_node — current (verbatim from
// :464-490):
let Some(construction_pipelines) = construction_pipelines else { return; };
let Some(construction_bind_groups) = construction_bind_groups else { return; };
let Some(construction_gpu) = construction_gpu else { return; };
let Some(construction_config) = construction_config else { return; };
if !construction_config.gpu_construction_enabled { return; }
if construction_config.max_group_bound_dispatch == 0 { return; }
if !construction_gpu.bounds_initialized { return; }

// Step 6 — target shape: stays exactly this way (visual chain).
// D5 does NOT touch this. The `.run_if(resource_exists::<_>)` move is
// flagged for D4 because the node lives in D4-owned `render/mod.rs`
// registration. D5 only converts the systems D5 *owns* registration of.
```

**Reuse choices:**

- `bevy::ecs::common_conditions::resource_exists` — existing Bevy 0.19
  helper. No new helper needed.
- `bevy::prelude::Condition::and` — for combining multiple
  `resource_exists` checks on a single registration.

**Behavioural delta:**

None for the converted systems — `.run_if(resource_exists::<X>)` is
semantically identical to `if X.is_none() { return; }` at the start of
a body. For the un-converted (D4-owned-registration) systems no body
change either; only the *visual* tidy of the bail ladder is in scope
for D5, and even that is **out-of-scope for D5's impl phase** because
the file is D4 territory (the node sigs / bodies live in
`bounds_calc.rs` and `world_change.rs`, which ARE D5 — but the
`Option<Res<_>>` parameter shape is dictated by the `.run_if` choice
at the registration site, which is D4-owned). **D5's impl phase
defers this entirely** to keep blast radius minimal. See §5.

---

### 2.6 Finding 6 — Six segment-voxel-buffer encoders/decoders

**Current shape (verified):**

| fn | location | callers |
|---|---|---|
| `build_segment_voxel_buffer_from_dense` | `mod.rs:4727-4818` | production: `prepare_construction` (currently `mod.rs:1894`) |
| `build_segment_voxel_buffer` | `mod.rs:4820-4873` | test-only: `validate_gpu_construction` (`mod.rs:4998`), `mod tests:~10082` |
| `decode_segment_voxels_into_volume` | `mod.rs:8331-8389` | test-only: `validation/gpu_construction_production.rs` callers |
| `decode_segment_voxels_to_volume` | `mod.rs:8904-8957` | test-only |
| `build_segment_voxel_buffer_for_region` | `mod.rs:9016-9058` | test-only |
| `build_segment_voxel_buffer_for_world` | `mod.rs:9079-9167` | test-only |

All six share the inner `for chunk × for block × for voxel` loop +
`(lo | (hi << 16))` packing.

**Target shape:**

Production encoder (`build_segment_voxel_buffer_from_dense`) moves into
`chunk_calc.rs` (the workstream that consumes its output). Renamed to
`chunk_calc::cpu_encode_segment_voxel_buffer_from_dense` for symmetry
with the existing GPU-side `dispatch_calc_block_from_raw_data`. **The
encoding-loop body is NOT factored** in this dispatch — the family of 6
is overscope for D5's pass. Instead:

- The production encoder (`_from_dense`) moves to `chunk_calc.rs`. Done.
- The five **test-only** encoders/decoders move with the e2e fixtures
  (Finding 3) into `validation/`. They land alongside their callers
  inside `validation/gpu_construction*.rs` /
  `validation/byte_diff_fixtures.rs` — verbatim copies, no consolidation.
  Three of them already share file with their only callers
  (`load_oasis_model_data` + `run_oasis_segment_byte_diff` +
  `decode_segment_voxels_to_volume` all live in
  `validation/gpu_construction_production.rs`).
- A future refactor pass (out of scope, see §6) can extract one canonical
  `encode_chunk(chunk_pos, voxel_getter: impl Fn(IVec3) -> u16) -> [u32; 2048]`
  helper that all six can call. **Not in this dispatch** — the test
  encoders' shape diversity (different input types — `DenseVolume`,
  `ModelData`, `&[u16]`, brush-shaped region) makes the "one canonical
  encoder + thin wrappers" design non-trivial and would mean editing
  every byte-diff fixture. That's gold-plating.

**Reuse choices:**

- `chunk_calc.rs` already owns the dispatch + layout-descriptor for the
  same chain (`chunk_calc.rs:170,198,274,297`). Co-locating the CPU
  encoder finishes the "everything chunk-calc lives in chunk_calc.rs"
  shape.

**Behavioural delta:**

None. Function moved, body untouched, `pub` visibility preserved (the
old `pub fn build_segment_voxel_buffer_from_dense` is re-exported from
`mod.rs` so production callers don't break — see Step 7).

---

### 2.7 Finding 7 — WGSL inline copies + `shader_drift_guard.rs`

**Current shape (verified):**

`bounds_common.wgsl` (191 LOC, canonical) + inline copies in
`chunk_calc.wgsl:138-310` and `world_change.wgsl:161-340` + partial
copy of `MASK_*` in `bounds_calc.wgsl`. `shader_drift_guard.rs` (400 LOC)
is a string-parser test that asserts byte-after-normalisation equality
across the three sites.

**Target shape — UNCHANGED.**

Per the explorer's side-note #3: the inline duplication is a workaround
for Bevy 0.19's `naga_oil` shader-import unreliability on
`var<workgroup>` arrays, atomics, and custom structs. Verifying whether
`naga_oil` import works for these constructs is a **separate
investigation** that needs a build + a behavioural test pass. D5's
impl phase **defers this entirely** — not because it isn't valuable,
but because:

1. Verifying naga-oil import works requires booting the binary and
   confirming every shader compiles + runs identically. That is
   exactly the verification surface CLAUDE.md routes through the e2e
   gates — but a "did `#import` work?" test isn't currently a gate.
2. If naga-oil supports the constructs, deleting `shader_drift_guard.rs`
   + 300 LOC of inline copies is a real LOC win.
3. If it does NOT, the current duplication is load-bearing and we'd
   waste an impl phase discovering that.
4. Either outcome is **a separate D5 dispatch.** Out of scope for this
   one.

**Reuse choices:** n/a — no change this pass.

**Behavioural delta:** none — file untouched.

---

### 2.8 Finding 8 — SSoT-6 hash coefficients

**Current shape (verified):**

```rust
// render/construction/hashing.rs:43-50
pub fn hash_coefficients() -> [u32; 65] {
    let mut c = [0u32; 65];
    c[64] = 1;
    for i in (0..64).rev() {
        c[i] = c[i + 1].wrapping_mul(31);
    }
    c
}
```

```rust
// aadf/block_hash.rs:395-404 — D1 territory, byte-equivalent body.
fn build_polynomial_coefficients() -> [u32; 65] { /* same body */ }
```

Both compute `c[64] = 1; c[i] = c[i+1].wrapping_mul(31)` for
`i in 63..=0`. Byte-equal output. Two test sites pin the value against
the C# constants: `block_hash.rs:417` and `hashing.rs:165`.

D1's exploration (`aadf-data-structures/02-exploration.md` §SSoT-6 +
Finding 8) recommends promoting `aadf::block_hash::build_polynomial_coefficients`
to `pub` and having both Rust copies call it. **D1 owns the algorithm
home** (paper-canonical `BlockHashingHandler` translates to
`aadf::block_hash`).

**Target shape:**

D5's `render/construction/hashing.rs::hash_coefficients` becomes a
re-export / thin wrapper around D1's promoted `pub fn
aadf::block_hash::build_polynomial_coefficients() -> [u32; 65]`:

```rust
// render/construction/hashing.rs
// Re-export of D1's canonical polynomial table. Single SSoT for the
// 65-element `31^(64-i)` table used by both the CPU-side
// `BlockHashingHandler` (D1) and the GPU upload (D5). The function
// uploads the result into the GPU `hash_coefficients` storage buffer
// (allocated in `prepare_construction`).
pub use crate::aadf::block_hash::build_polynomial_coefficients as hash_coefficients;
```

D5's local test (`hashing.rs:165`) stays — D5 retains its independent
assertion against the C# constants because D5 is the GPU-upload
consumer and divergence in either direction would surface in D5's
test sweep before D1's.

**This finding is D1↔D5 cross-domain.** D1's impl phase lands the
`pub` promotion + (optionally) the rename to `pub fn hash_coefficients`
to match the existing callsite. **D5's impl phase only edits
`render/construction/hashing.rs`** to swap the local fn for the re-export.

**Reuse choices:**

- D1's `aadf::block_hash::build_polynomial_coefficients` is the canonical
  home (paper §3.4 maps `BlockHashingHandler` → `block_hash`).
- Bevy/Rust idiomatic re-export pattern (`pub use`).

**Behavioural delta:**

None — byte-equal output.

**D1↔D5 sequencing:** D5's impl phase must land **after** D1's impl
phase promotes the function to `pub`. D1's domain runs in the
"D1/D2/D3/D6/D8 interleave" middle phase (per `01-context.md` Q3),
which the user's overall sequencing puts **after** D5. So D5's impl
phase **CANNOT** land this finding by itself. Flagged as a deferred
follow-up in §5 ("D4 + D1 coordination notes") — for this impl phase
the D5 local copy stays exactly as-is.

---

### 2.9 Finding 9 — `ConstructionGpu` 28-`Option<…>` god-resource (BEV-3)

**Current shape (verified):**

`ConstructionGpu` (`mod.rs:106-317`) has 28 `Option<Buffer>` fields, 4
`Option<&'static str>` label fields, 3 `bool` gate fields, and the
nested `CpuMirrorReadback` (with 4 more `Option<Buffer>` fields inside).

**Target shape — UNCHANGED THIS PASS.**

The exploration's suggested direction (split into per-workstream
`ChunkCalcGpu` / `BoundsCalcGpu` / `WorldChangeGpu` / `EntityUpdateGpu`
/ `GeneratorModelGpu` resources) is a real improvement, but **its blast
radius collides with the W0-seam retirement in §2.10 + the prepare
split in §2.1**. Landing all three in one D5 impl phase would:

- Force every workstream's prepare-system to allocate into its **own**
  per-workstream resource rather than the shared bag (touches all 5
  workstream submodules in the same dispatch).
- Force every workstream's node-system + dispatch helpers to query its
  per-workstream resource rather than `Res<ConstructionGpu>` (touches
  the 4 `naadf_*_node` functions, 12+ `dispatch_*` helpers, and the
  `prepare_shared_bind_groups` builder).
- Multiply migration-step count by a factor of 2 with no behavioural
  gain — the per-workstream split is pure ergonomics.

**This pass keeps the single `ConstructionGpu` resource** but:

1. Deletes the `prepare_probe_history` field (Finding 2 — Step 1).
2. Deletes the 4 `*_label` debug-stash fields (Step 2 — see §6).

Resulting field count drops from 28 → 22 `Option<…>` Buffer fields + 0
label fields, with the 3 gate booleans + the `CpuMirrorReadback`
substructure unchanged. The proper per-workstream split is flagged as
the **natural next D5 refactor** (§6 "Future-work / deferred"). For
this dispatch the Finding 9 win is the 4-label-field drop, the
`prepare_probe_history` field drop, and the gate-boolean tidy that
follows from §2.5.

**Reuse choices:** n/a.

**Behavioural delta:** none (the deleted fields were unused — the
label fields per the docblock at `mod.rs:255-268` are only read by a
debug-only assertion in `populate_cpu_mirror_from_gpu_producer`;
removing them removes that assertion too).

---

### 2.10 Finding 10 — `ConstructionPipelines` retired (OA-2 / Resolution D)

**Current shape (verified):**

`ConstructionPipelines` (`mod.rs:481-571`) — 25 `pub` fields across 5
workstream sections, built by a single 168-LOC `FromWorld` impl
(`mod.rs:573-741`). Sibling of `NaadfPipelines` (D4, `render/pipelines.rs:23-309`).
Registered via `.init_gpu_resource::<ConstructionPipelines>()`
(`mod.rs:4660`).

The W0 seam contract (`15-design-c.md` §1.1 final paragraph + §1.3 final
paragraph) explicitly forbids editing `NaadfPipelines` from construction
workstreams: *"`NaadfPipelines` is not edited — this is the seam."*

**Target shape:**

Per `01-context.md` addendum Resolution D the W0 seam is retired. Two
shapes considered (see §6 for the loser):

**(a) merge `ConstructionPipelines` into `NaadfPipelines`.** Net result:
`NaadfPipelines` gains 25 fields, `from_world` gains 168 LOC, the
`ConstructionPipelines` resource type disappears. Every consumer changes
`Res<ConstructionPipelines>` → `Res<NaadfPipelines>`.

**(b) keep `ConstructionPipelines` as a separate Resource but drop the
"empty sibling" framing.** Net result: file moves only (`mod.rs` →
`pipelines_construction.rs`), no consumer changes. Doesn't actually
retire the seam; just renames it. **Rejected** — Resolution D says
"propose the merge", not "rename the sibling".

**Chosen: (a) merge.** Concrete shape:

- The 25 fields move into `NaadfPipelines` (D4-owned file
  `render/pipelines.rs`). **D5's impl phase CANNOT land this change** —
  the file is D4 territory. D5's impl phase flags this as a **D4
  blocker** (see §5).
- For D5's impl phase, `ConstructionPipelines` becomes a `pub use`
  re-export of `NaadfPipelines` so consumers continue to compile. But
  this requires the merge to land first — chicken-and-egg.
- **D5's impl phase therefore defers Finding 10 entirely.** The merge
  lands when D4's impl phase processes its Finding 10 (cited at
  `render-pipeline/02-exploration.md` "Open questions for the architect"
  #7). D5 only **publishes the design** here so D4's architect /
  implementor can absorb it.
- D5 documents the W0-contract retirement in this architecture file (this
  section). `15-design-c.md` §1.3 / §1.1 become historical: the contract
  is now retired by user directive (Resolution D), and the merge happens
  in D4's pass.

**D4-blocker shape (for D4's architect):**

```rust
// render/pipelines.rs — AFTER D4's merge.
#[derive(Resource)]
pub struct NaadfPipelines {
    // === existing 14 pipelines + 15 layouts (D4's domain — unchanged) ===
    pub world_layout: BindGroupLayoutDescriptor,
    pub frame_layout: BindGroupLayoutDescriptor,
    /* … all existing fields … */

    // === Phase-C construction pipelines + layouts (was ConstructionPipelines) ===
    // W5 generator_model:
    pub construction_generator_model_layout: BindGroupLayoutDescriptor,
    pub construction_generator_model_pipeline: CachedComputePipelineId,
    // W1 chunk_calc:
    pub construction_world_layout: BindGroupLayoutDescriptor,
    pub construction_chunk_calc_pipeline_calc_block: CachedComputePipelineId,
    pub construction_chunk_calc_pipeline_voxel_bounds: CachedComputePipelineId,
    pub construction_chunk_calc_pipeline_block_bounds: CachedComputePipelineId,
    pub construction_map_copy_layout: BindGroupLayoutDescriptor,
    pub construction_map_copy_pipeline_copy: CachedComputePipelineId,
    pub construction_map_copy_pipeline_test: CachedComputePipelineId,
    // W3 bounds_calc:
    pub construction_bounds_world_layout: BindGroupLayoutDescriptor,
    pub construction_bounds_layout: BindGroupLayoutDescriptor,
    pub construction_bound_dispatch_indirect_layout: BindGroupLayoutDescriptor,
    pub construction_bounds_calc_pipeline_add_initial: CachedComputePipelineId,
    pub construction_bounds_calc_pipeline_prepare: CachedComputePipelineId,
    pub construction_bounds_calc_pipeline_compute: CachedComputePipelineId,
    // W4 entity_update:
    pub construction_entity_world_layout: BindGroupLayoutDescriptor,
    pub construction_entity_layout: BindGroupLayoutDescriptor,
    pub construction_entity_update_pipeline_update_chunks: CachedComputePipelineId,
    pub construction_entity_update_pipeline_copy_entity_chunk_instances: CachedComputePipelineId,
    pub construction_entity_update_pipeline_copy_entity_history: CachedComputePipelineId,
    // W2 world_change:
    pub construction_change_layout: BindGroupLayoutDescriptor,
    pub construction_world_change_pipeline_apply_group_change: CachedComputePipelineId,
    pub construction_world_change_pipeline_apply_chunk_change: CachedComputePipelineId,
    pub construction_world_change_pipeline_apply_block_change: CachedComputePipelineId,
    pub construction_world_change_pipeline_apply_voxel_change: CachedComputePipelineId,
}
```

The `construction_` field-name prefix disambiguates within the
flattened `NaadfPipelines`. The existing `FromWorld` impl absorbs the
existing `ConstructionPipelines::from_world` body verbatim (the
`asset_server.clone()` + `pipeline_cache` resources are already in
scope).

For **D5's impl phase the cleanup is just**:

- Field names in `mod.rs::ConstructionPipelines` stay; only the 1 field
  associated with Finding 2 (`prepare_probe_history_layout`) goes.

The `ConstructionPipelines → NaadfPipelines` merge lands in D4's impl
phase. D5's design here is the design for **D4 to consume**, per
Resolution D's "architect proposes the merge".

**Reuse choices:** `NaadfPipelines` already exists; the merge expands
its field set. No new Bevy abstraction.

**Behavioural delta (when D4 lands the merge):** none — the construction
pipelines are still queued in `from_world`, still keyed by
`CachedComputePipelineId`, still consumed by the same dispatch helpers.
Single resource lookup at every consumer (one `Res<NaadfPipelines>`
instead of two `Res<NaadfPipelines>` + `Res<ConstructionPipelines>`).

---

### 2.11 Finding 11 — WGSL `4u`/`64u` literals → `CELL_DIM`/`CELL_CHILDREN`

**Current shape (verified):**

- `chunk_calc.wgsl:168-543` — 14 sites of bare `4u`/`64u`.
- `bounds_calc.wgsl:222-479` — 6 sites.
- `world_change.wgsl:207-556` — 10 sites.

Rust SSoT lives at `voxel/mod.rs:63-65` (`CELL_DIM = 4`,
`CELL_CHILDREN = 64`). No `#define` / shader-def piping.

**Target shape:**

Declare `const CELL_DIM: u32 = 4u;` + `const CELL_CHILDREN: u32 = 64u;`
at the top of each of `chunk_calc.wgsl`, `bounds_calc.wgsl`,
`world_change.wgsl` (`bounds_common.wgsl` is already imported into
those, but adding the consts at the *file* top makes the discovery
hop short).

Replace bare literals **only at sites where the literal IS that
semantic** — site-by-site audit during impl. Examples:

- `chunk_calc.wgsl` voxel-within-block loop `for (var i = 0u; i < 64u; i++)` →
  `for (var i = 0u; i < CELL_CHILDREN; i++)` ✓.
- `chunk_calc.wgsl` cell side `gp.x * 4u` (mapping group-pos to chunk-pos) →
  `gp.x * CELL_DIM` ✓.
- `bounds_calc.wgsl:421-422` `probe_call_idx * 4u` — this is `4 u32s per
  entry`, NOT `CELL_DIM`. **Stays bare** (and is deleted by Finding 2
  anyway).
- `world_change.wgsl` `>> 4u` bit-shift amounts (powers-of-two arithmetic
  unrelated to `CELL_DIM`) — **stay bare**.

The audit is mechanical: every bare `4u`/`64u` gets one comment
classifying it as `CELL_DIM` / `CELL_CHILDREN` / `other` and replaced
accordingly.

**Reuse choices:**

- WGSL `const` declarations — file-local. No shader-def piping (the
  Rust↔WGSL shader-def route requires `naga_oil` + the
  `#{CELL_DIM}` injection from `NaadfPipelines::from_world` — D4
  territory, out of scope for D5 this pass).

**Behavioural delta:** none — `const CELL_DIM: u32 = 4u;` is a compile-time
substitution; WGSL emits identical bytecode.

---

## 3. Migration steps

5 ordered steps, each leaving the codebase in a buildable + e2e-passing
state. Numbers are LOC deltas (approximate, ± 50).

### Step 1 — DELETE diagnostic probes + no-op stub

**Rationale:** clears 1 100 LOC of unused investigation residual before
any structural moves; the empty
`prepare_construction` body becomes 100 LOC shorter; the W3 layout count
drops; bind-group count on `prepare_group_bounds` drops from 4 to 3.
First because every downstream step is easier with this gone.

**Edits:**

- `render/construction/mod.rs:3559-4617` — delete `AadfDelayedProbe`,
  `AadfPerCallProbe` + `PerCallProbeStage`, `AadfCpuGpuParity` +
  `CpuGpuParityStage`, `aadf_delayed_probe`, `aadf_per_call_probe`,
  `aadf_cpu_gpu_parity`, `aadf_cpu_gpu_parity_maybe`.
- `render/construction/mod.rs:4653, 4655, 4657` — delete the 3
  `.init_resource::<…>()` lines from `ConstructionPlugin::build`.
- `render/construction/mod.rs:4694, 4699, 4709` — delete the 3
  `.add_systems(ExtractSchedule, aadf_*)` lines.
- `render/construction/mod.rs:148-155` — delete
  `ConstructionGpu::prepare_probe_history` field + docblock.
- `render/construction/mod.rs:441-443` — delete
  `ConstructionBindGroups::prepare_probe_history` field + docblock.
- `render/construction/mod.rs:326-344` — delete
  `PREPARE_PROBE_HISTORY_ENTRIES` + `PREPARE_PROBE_HISTORY_BYTES` consts +
  docblocks.
- `render/construction/mod.rs:528-534` — delete
  `ConstructionPipelines::prepare_probe_history_layout` field +
  docblock.
- `render/construction/mod.rs:627-628, 725` — delete the `from_world`
  body's `prepare_probe_history_layout = …` build line + the struct-literal
  entry.
- `render/construction/mod.rs:~1700-1900` — inside the soon-to-be-split
  `prepare_construction` body, locate the W3-section allocator's
  `prepare_probe_history` buffer creation + the bind-group builder's
  `prepare_probe_history` BindGroup creation; delete both. (Exact line
  range: `grep -n "prepare_probe_history" mod.rs` covers all sites.)
- `render/construction/bounds_calc.rs:191-199` — delete
  `prepare_probe_history_layout_descriptor` fn.
- `render/construction/bounds_calc.rs:230-249` — alter
  `queue_prepare_pipeline` signature: drop the `probe_layout:
  BindGroupLayoutDescriptor` parameter; update doc-comment.
- `render/construction/bounds_calc.rs:252-267` — alter
  `queue_prepare_pipeline_with_handle` signature similarly; update the
  `layout: vec![...]` from `[world, bounds, dispatch, probe]` → `[world,
  bounds, dispatch]`.
- `render/construction/bounds_calc.rs:347` — alter
  `dispatch_regime_2_rounds` signature: drop `probe_bg: &BindGroup`
  parameter; update its `compute_pass.set_bind_group(3, probe_bg, &[])`
  line.
- `render/construction/bounds_calc.rs:455-507` — alter
  `naadf_bounds_compute_node` body: drop the `probe_bg` early-return
  bail; drop the `probe_bg` argument when calling
  `dispatch_regime_2_rounds`.
- `render/construction/bounds_calc.rs:370` — delete the "group 3 =
  prepare_probe_history" comment.
- `render/construction/bounds_calc/tests.rs:42` — delete the `use`
  importing `PREPARE_PROBE_HISTORY_BYTES, PREPARE_PROBE_HISTORY_ENTRIES`.
- `render/construction/bounds_calc/tests.rs:427` — delete the local
  `prepare_probe_history: Buffer` field on the test's mock-bind-group
  struct.
- `render/construction/bounds_calc/tests.rs:524-545` — delete the
  `super::prepare_probe_history_layout_descriptor()` call + the local
  `prepare_probe_history` Buffer allocation + the
  `queue.write_buffer` zero-init.
- `render/construction/bounds_calc/tests.rs:600-617` — delete the
  4th bind-group construction (the probe BG) + update the
  `queue_prepare_pipeline_with_handle` call-site to the 3-layout
  signature.
- `assets/shaders/bounds_calc.wgsl:160-176` — delete the `@group(3)
  @binding(0) var<storage, read_write> prepare_probe_history: array<u32>;`
  declaration + its docblock.
- `assets/shaders/bounds_calc.wgsl:405-433` — delete the
  `let probe_call_idx = ... ` block + every write into
  `prepare_probe_history`.
- `render/construction/mod.rs:836-838` — delete
  `clear_world_data_pending_edits` fn.
- `render/construction/mod.rs:819-835` — delete the 16-line docblock
  above it.
- `render/construction/mod.rs:4642` — delete the
  `app.add_systems(Last, clear_world_data_pending_edits);` registration.
- `render/construction/mod.rs:255-278` — delete the 4 `*_label:
  Option<&'static str>` fields on `ConstructionGpu` + their docblocks.
- `render/construction/mod.rs:~1916-1932` (approximate — verify by
  `grep -n "block_voxel_count_label\|segment_voxel_buffer_label\|hash_map_label\|hash_coefficients_label" mod.rs`) — delete every `gpu.X_label = Some("…");`
  assignment inside `prepare_construction`'s W1/W3 allocator blocks.
- `render/construction/mod.rs:~1500-1550` (approximate) — find the
  `#[cfg(debug_assertions)]` assertion in
  `populate_cpu_mirror_from_gpu_producer` that consumes the labels;
  delete the assertion block. (`grep -n "block_voxel_count_label" mod.rs`.)

**Post-step state:**

- 0 callers of `AadfDelayedProbe`, `AadfPerCallProbe`, `AadfCpuGpuParity`
  symbols (verified by `grep -rn`).
- 0 callers of `PREPARE_PROBE_HISTORY_ENTRIES`, `PREPARE_PROBE_HISTORY_BYTES`,
  `prepare_probe_history_layout_descriptor`, `prepare_probe_history`
  (across `src/` + WGSL).
- `naadf_bounds_compute_node` now sees 3 bind groups, matches the W3 pre-
  probe-1B architecture. Wasm `max_bind_groups = 4` cap now has 1 slot of
  headroom on this pipeline (previously was at the exact limit).
- `clear_world_data_pending_edits` gone; one less main-world Last
  schedule entry.
- `ConstructionGpu` field count down from 28 → 22 buffer fields + 0
  label fields.
- `mod.rs` LOC drops from 11 043 → ~9 800.

**Verification:**

- `cargo build --workspace` ✓.
- `cargo test --workspace --lib` ✓ — confirms `hashing.rs::test*`,
  `bounds_calc/tests.rs`, `change_handler.rs::tests`, `chunk_calc::tests`
  pass. The W3 `tests.rs` is the highest-risk test here (it consumed
  `PREPARE_PROBE_HISTORY_*` directly).
- `cargo run --bin e2e_render -- --validate-gpu-construction` ✓.
- `cargo run --bin e2e_render -- --validate-gpu-construction-scaled` ✓.
- `cargo run --bin e2e_render -- --validate-gpu-construction-production-scale` ✓.
- `cargo run --bin e2e_render -- --edit-mode` ✓.
- `cargo run --bin e2e_render -- --runtime-edit-mode` ✓.
- `cargo run --bin e2e_render -- --entities` ✓.
- `cargo run --bin e2e_render -- --oasis-edit-visual` ✓ — runs ≥2× per
  `feedback-multiple-runs-rule-out-false-positives`.

---

### Step 2 — Extract the readback state machine into `readback.rs`

**Rationale:** the `populate_cpu_mirror_from_gpu_producer` system
(`mod.rs:1092-1631`, 539 LOC) + its `ReadbackStage` enum + the
`CpuMirrorReadback` struct + the `READBACK_STALL_BUDGET_FRAMES` const
form a self-contained concern (the cross-frame Q3 readback state machine).
Pulling them out shrinks `mod.rs` by ~600 LOC and the new file is
trivially navigable.

**Edits:**

- Create `render/construction/readback.rs` (~660 LOC) — contains:
  - `ReadbackStage` enum (was `mod.rs:352-366`).
  - `CpuMirrorReadback` struct + its `Default` (was `mod.rs:376-405`).
  - `READBACK_STALL_BUDGET_FRAMES` const (was `mod.rs:324`).
  - `populate_cpu_mirror_from_gpu_producer` system (was
    `mod.rs:1092-1631`).
  - `pub use` re-exports at the top: `pub use super::{ConstructionGpu, …};`
    for the symbols the system references.
- `render/construction/mod.rs:59-69` — add `pub mod readback;` to the
  submodule list.
- `render/construction/mod.rs:352-405` — delete the items moved.
- `render/construction/mod.rs:1092-1631` — delete the function moved.
- `render/construction/mod.rs:324` — delete the const moved.
- `render/construction/mod.rs:80-X` — `pub use readback::{ReadbackStage,
  CpuMirrorReadback, READBACK_STALL_BUDGET_FRAMES,
  populate_cpu_mirror_from_gpu_producer};` so the existing
  `ConstructionGpu::cpu_mirror_readback: CpuMirrorReadback` field type +
  the `ConstructionPlugin::build`'s
  `.add_systems(ExtractSchedule, (extract_world_changes,
  populate_cpu_mirror_from_gpu_producer))` line continue to resolve.
- Inside the moved code, fix-up `crate::render::construction::…`
  paths that became `super::…` or `crate::render::construction::readback::…`.

**Post-step state:**

- New file `render/construction/readback.rs` exists, ~660 LOC.
- `mod.rs` LOC drops from ~9 800 → ~9 200.
- `populate_cpu_mirror_from_gpu_producer` lives next to its state
  machine + the const that bounds it.

**Verification:** all gates from Step 1, plus the embedded
`mod tests` (W5 GPU↔CPU oracle) which calls into the readback path
indirectly. Re-run `--oasis-edit-visual` ≥2× — this gate exercises the
readback state machine end-to-end.

---

### Step 3 — Extract `extract_world_changes` + the producer node

**Rationale:** two more self-contained systems live in `mod.rs`:
`extract_world_changes` (180 LOC, `mod.rs:910-1048`) and
`naadf_gpu_producer_node` (434 LOC, `mod.rs:3076-3509`). Both are
single-system files of distinct concerns. Move them.

**Edits:**

- Create `render/construction/extract.rs` (~210 LOC) — contains
  `extract_world_changes` + its re-exports. Includes `MainWorldEntities`,
  `RenderWorldEntityState` resources (the extract's main-world / render-world
  shadows — `mod.rs:855-897`). The `ConstructionEvents` resource
  definition (`mod.rs:754-817`) stays in `mod.rs` since it's the
  cross-workstream W2+W4 edit-batch the producer node + every regime-3
  dispatch reads.
- Create `render/construction/producer.rs` (~470 LOC) — contains
  `naadf_gpu_producer_node`. Re-exports as needed.
- `render/construction/mod.rs:59-69` — add `pub mod extract;` and
  `pub mod producer;`.
- `render/construction/mod.rs:855-897` — delete the
  `MainWorldEntities` + `RenderWorldEntityState` resource defs (moved
  into `extract.rs`).
- `render/construction/mod.rs:910-1048` — delete `extract_world_changes`.
- `render/construction/mod.rs:3076-3509` — delete `naadf_gpu_producer_node`.
- `render/construction/mod.rs:~80-X` — `pub use extract::{MainWorldEntities,
  RenderWorldEntityState, extract_world_changes};` +
  `pub use producer::naadf_gpu_producer_node;` so
  `ConstructionPlugin::build`'s registrations + `render/mod.rs:77`'s
  `use construction::naadf_gpu_producer_node;` continue to resolve.

**Post-step state:**

- New files `render/construction/extract.rs` (~210 LOC) and
  `render/construction/producer.rs` (~470 LOC).
- `mod.rs` LOC drops from ~9 200 → ~8 100.
- `render/mod.rs:77`'s import path stays exactly the same
  (`use construction::naadf_gpu_producer_node;`) because the symbol is
  re-exported from `mod.rs` — D4-owned file untouched.

**Verification:** all gates from Step 1.

---

### Step 4 — Split `prepare_construction` per workstream

**Rationale:** the largest single-step LOC win. Pulls the 1 418-LOC
god-system apart into 6 sibling systems (5 per-workstream + 1 shared
bind-group builder + 1 resource-init shell). Each lands in its
workstream submodule.

**Edits:**

- `render/construction/chunk_calc.rs` — append `pub fn prepare_chunk_calc(...)`
  taking only the parameters W1 needs (`ConstructionGpu`,
  `ConstructionBindGroups`, `ConstructionPipelines`,
  `ConstructionConfig`, `WorldGpu`, `RenderDevice`, `RenderQueue`,
  `PipelineCache`, `WorldDataMeta`, `ConstructionEvents`). Body lifted
  from `mod.rs:1700-1912` ("Phase-C followup #1 — runtime GPU producer
  pre-allocation") plus the early "ensure W1 buffers" allocations
  that today happen inside the W3 block — i.e., everything the W1
  buffer family (`hash_map`, `hash_coefficients`,
  `block_voxel_count`, `segment_voxel_buffer`) needs.
- `render/construction/bounds_calc.rs` — append `pub fn prepare_bounds_calc(...)`
  lifting the W3 block at `mod.rs:1913-2215`. Touches the W3 buffer
  family + the `chunks_mirror_buffer` allocation.
- `render/construction/world_change.rs` — append `pub fn prepare_world_change(...)`
  lifting the W2 block at `mod.rs:2418-2712`. Touches the W2
  change-staging buffer family + the W2 dynamic uploads from
  `ConstructionEvents`.
- `render/construction/entity_update.rs` — append `pub fn prepare_entity_update(...)`
  lifting the W4 block at `mod.rs:2713-3030`. Touches the W4 entity
  family + the per-frame `EntityUpdateParams` uniform write.
- `render/construction/generator_model.rs` — append
  `pub fn prepare_generator_model(...)` lifting the W5 block at
  `mod.rs:2216-2417` (W5 model_data buffer family + the per-segment
  uniform). Plus the bind-group build at `mod.rs::~2400`.
- `render/construction/producer.rs` (or a new `render/construction/shared_bind_groups.rs`)
  — append `pub fn prepare_shared_bind_groups(...)`. Lifts the
  `construction_world` + `construction_bounds_world` bind-group builds
  from `mod.rs::~2030-2140` ("Build W3 bind groups when missing"
  section + the W1 `construction_world` bind group's recursive build).
  This system runs `.after()` every per-workstream prepare; its body
  early-returns if any of its source buffers (the W1 hash family, the
  chunks texture) aren't yet allocated.
- `render/construction/mod.rs:1632-1657` — `prepare_construction` doc
  block: rewrite as `prepare_construction_resources` docblock for the
  new resource-init-shell system (5 sentences: "ensure-exists for
  ConstructionGpu + ConstructionBindGroups; per-workstream prepares
  registered separately").
- `render/construction/mod.rs:1658-3075` — replace the 1 418-LOC body
  with the new 25-LOC `prepare_construction_resources` shell shown in §2.1.
- `render/construction/mod.rs:4674-4679` — replace the single
  `.add_systems(Render, prepare_construction.in_set(...).after(...))`
  registration with the 6-system block shown in §2.1.

**Post-step state:**

- 6 new prepare-systems, one per workstream + the shell + the shared
  bind-group builder. Each lives in its workstream submodule alongside
  the dispatch helpers + layout descriptors it pairs with.
- `mod.rs` LOC drops from ~8 100 → ~6 700.
- Each workstream submodule grows: `chunk_calc.rs` 314 → ~600,
  `bounds_calc.rs` 619 → ~920, `world_change.rs` 1 165 → ~1 460,
  `entity_update.rs` 401 → ~720, `generator_model.rs` 303 → ~510.
- Total D5 Rust unchanged at this step (~10 700) — pure re-distribution.

**Verification:** all gates from Step 1. Critical: re-run
`--oasis-edit-visual` ≥2× and `--vox-e2e` if it exists — these are
the W5 + W2 paths most likely to surface a schedule-order regression
from the split.

---

### Step 5 — Move e2e gate fixtures to `validation/`

**Rationale:** moves ~4 640 LOC of test-fixture code out of the
production module. After this step `mod.rs` is at its target size
(~620 LOC).

**Edits:**

- Create `render/construction/validation/` directory + 7 files per §2.3.
- Move `mod.rs:4727-4818` (`build_segment_voxel_buffer_from_dense`) into
  `render/construction/chunk_calc.rs` as
  `pub fn cpu_encode_segment_voxel_buffer_from_dense` — keep the
  original `pub fn build_segment_voxel_buffer_from_dense` name **for
  backward compatibility** (alias via `pub use` from `mod.rs`).
- Move `mod.rs:4820-4873` (`build_segment_voxel_buffer`) +
  `mod.rs:4874-4927` (`voxel_at_block_local`) into
  `validation/gpu_construction.rs`.
- Move `mod.rs:4928-5289` (`validate_gpu_construction`) into
  `validation/gpu_construction.rs`.
- Move `mod.rs:5290-5480` (`validate_gpu_construction_scaled`) +
  `mod.rs:5481-5620` (`discover_populated_oasis_voxels`) into
  `validation/gpu_construction_scaled.rs`.
- Move `mod.rs:5621-6334` (`validate_gpu_construction_production_scale`) +
  `mod.rs:6335-6622` (`impl VoxelReadback`, `readback_cursor`,
  `map_single_u32`, `map_single_pair`, `sample_voxel_readback`,
  `render_results_table`) + `mod.rs:8331-8389`
  (`decode_segment_voxels_into_volume`) + `mod.rs:8390-8407`
  (`load_oasis_model_data`) + `mod.rs:8408-8903`
  (`run_oasis_segment_byte_diff`) + `mod.rs:8904-8957`
  (`decode_segment_voxels_to_volume`) + `mod.rs:8958-9015`
  (`build_mixed_model_data`) + `mod.rs:9016-9058`
  (`build_segment_voxel_buffer_for_region`) + `mod.rs:9059-9078`
  (`chunk_kind` + `block_kind`) + `mod.rs:9079-9167`
  (`build_segment_voxel_buffer_for_world`)
  into `validation/gpu_construction_production.rs`.
- Move `mod.rs:6623-7133` (`run_one_fixture_byte_diff`) +
  `mod.rs:7134-7605` (`run_one_fixture_multiseg_byte_diff`) +
  `mod.rs:7606-7831` (`run_one_generator_model_byte_diff`) +
  `mod.rs:7832-8330` (`run_one_tiled_byte_diff`) into
  `validation/byte_diff_fixtures.rs`. `pub(crate)` visibility only;
  these are called from
  `validation/gpu_construction_scaled.rs::validate_gpu_construction_scaled`
  + `validation/gpu_construction_production.rs::validate_gpu_construction_production_scale`.
- Move `mod.rs:9168-9304` (`validate_edit_mode`) +
  `mod.rs:9443-9449` (`built_pre_edit_state`) into
  `validation/edit_mode.rs`.
- Move `mod.rs:9305-9442` (`validate_runtime_edit_mode`) into
  `validation/runtime_edit_mode.rs`.
- Move `mod.rs:9450-9517` (`validate_entity_handler`) into
  `validation/entity_handler.rs`.
- Move `mod.rs:9518-9812` (`mod tests`, W5 oracle) into
  `generator_model.rs::#[cfg(test)] mod tests;` (file `generator_model/tests.rs`).
- Move `mod.rs:9813-10604` (`mod tests_w1`, W1 oracle) into
  `chunk_calc.rs::#[cfg(test)] mod tests_w1;` (file `chunk_calc/tests_w1.rs`).
- Move `mod.rs:10605-11042` (`mod tests_w4`, W4 oracle) into
  `entity_update.rs::#[cfg(test)] mod tests_w4;` (file `entity_update/tests_w4.rs`).
- `render/construction/mod.rs:59-69` — add `pub mod validation;`.
- `render/construction/mod.rs` top-of-file — add `pub use
  validation::{validate_edit_mode, validate_entity_handler,
  validate_gpu_construction,
  validate_gpu_construction_production_scale,
  validate_gpu_construction_scaled, validate_runtime_edit_mode};`
  + `pub use chunk_calc::cpu_encode_segment_voxel_buffer_from_dense as
  build_segment_voxel_buffer_from_dense;` (alias for production callers).
- Update internal use-paths inside the moved files: `super::…` →
  `crate::render::construction::…`. The moved functions invoke
  symbols from `mod.rs` + workstream submodules (e.g. `chunk_calc::dispatch_calc_block_from_raw_data`,
  `bounds_calc::naadf_bounds_compute_node`, etc.); these paths re-prefix
  uniformly.

**Post-step state:**

- New directory `render/construction/validation/` with 7 files.
- New per-workstream tests files alongside the workstream they exercise.
- `mod.rs` LOC at the target ~620.
- `bin/e2e_render.rs` UNCHANGED (all 6 `validate_*` re-exported at the
  same `pub` path).

**Verification:** all gates from Step 1.

---

### Step 6 — Per-workstream prepare-system `.run_if` cleanup

**Rationale:** Finding 5 — partial. Converts the `else { return; }`
bails inside the systems D5 owns the registration of (the per-workstream
prepares from Step 4 + the extract-schedule pair from Step 3) to
`.run_if(resource_exists::<_>)` clauses on the `add_systems` call. The
node-system bodies (`naadf_*_node`) DEFER to D4's impl phase per §2.5.

**Edits:**

- `render/construction/mod.rs::ConstructionPlugin::build`'s 6-system
  add_systems block (from Step 4) — append `.run_if(...)` clauses:
  ```rust
  chunk_calc::prepare_chunk_calc
      .run_if(resource_exists::<ConstructionGpu>)
      .run_if(resource_exists::<ConstructionBindGroups>)
      .run_if(resource_exists::<ConstructionPipelines>)
      .run_if(resource_exists::<crate::render::prepare::WorldGpu>),
  // ... similar for the other 4 per-workstream prepares
  ```
- `render/construction/mod.rs::ConstructionPlugin::build`'s
  `ExtractSchedule` registration — append:
  ```rust
  .add_systems(ExtractSchedule, (
      extract::extract_world_changes,
      readback::populate_cpu_mirror_from_gpu_producer
          .run_if(resource_exists::<ConstructionGpu>)
          .run_if(resource_exists::<crate::render::prepare::WorldGpu>),
  ))
  ```
- Inside each per-workstream `prepare_*` system body: remove the
  `let Some(...) = ... else { return; };` bails on the resources now
  pre-checked by `.run_if(...)`. Convert the system parameter types
  from `Option<Res<X>>` / `Option<ResMut<X>>` to `Res<X>` / `ResMut<X>`
  where appropriate. `Option<…>`-typed parameters that the body
  conditionally uses (e.g. `ModelDataRender`,
  `ConstructionEvents.has_pending_changes()` checks) stay `Option<…>` —
  those are intra-body branches, not preconditions.

**Post-step state:**

- `prepare_*` systems are visually cleaner; preconditions are at
  the registration site.
- No scheduler order change — `.run_if` is filtering, not ordering.
- No body shape change for the node systems (`naadf_*_node`) — those
  go to D4.

**Verification:** all gates from Step 1. The `.run_if` move is
behaviour-preserving by Bevy semantics; the gates assert against
construction output, which is identical.

---

### Step 7 — Production encoder relocation + finalize Finding 6

**Rationale:** finishes the Finding 6 partition. The `_from_dense`
production encoder already moved to `chunk_calc.rs` in Step 5; this
step renames the alias to drop the back-compat layer + adds a docblock
that points future readers at the test-only encoders' location.

**Edits:**

- `render/construction/chunk_calc.rs::cpu_encode_segment_voxel_buffer_from_dense`
  — keep this as the canonical name.
- `render/construction/mod.rs`'s `pub use ... as build_segment_voxel_buffer_from_dense;`
  re-export from Step 5 — **keep** (the alias defends production
  callers; remove only if a future refactor sweeps every caller).
- Add a docblock to `chunk_calc.rs::cpu_encode_segment_voxel_buffer_from_dense`
  describing the encoding rule (2048 u32s per chunk, etc., copied from
  the existing `build_segment_voxel_buffer_from_dense` docblock) and
  cross-referencing the test-only variants in `validation/`.
- No edit to the 5 test encoders — they stay where they landed in Step
  5 (inside `validation/*`).

**Post-step state:**

- Production encoder lives next to chunk_calc; test encoders live with
  the test fixtures.
- Six functions still exist, but the family is partitioned by
  use-site (1 production + 5 test) and discoverable.

**Verification:** all gates from Step 1.

---

### Step 8 — WGSL `CELL_DIM` / `CELL_CHILDREN` naming pass

**Rationale:** small but real Finding 11 win. Pure WGSL edit;
behaviour-preserving compile-time substitution.

**Edits:**

- `assets/shaders/chunk_calc.wgsl` — add at file top
  `const CELL_DIM: u32 = 4u;` + `const CELL_CHILDREN: u32 = 64u;`. Walk
  the 14 sites of `4u`/`64u` reported by `grep -n "\b4u\b\|\b64u\b" chunk_calc.wgsl`,
  classify each (`CELL_DIM` / `CELL_CHILDREN` / `other`), and replace
  the qualifying ones.
- `assets/shaders/bounds_calc.wgsl` — same pattern; 6 sites. Skip any
  literal that is `probe_call_idx * 4u` (deleted in Step 1) — should
  not exist anymore.
- `assets/shaders/world_change.wgsl` — same pattern; 10 sites.

**Post-step state:**

- Three WGSL files declare `CELL_DIM` + `CELL_CHILDREN` at top.
- Bare `4u`/`64u` literals only remain where their semantic is NOT
  the paper's cell dimensions (e.g. bit-shift amounts).

**Verification:** all gates from Step 1. WGSL `const` substitution is
compile-time; no behavioural risk.

---

## 4. What stays / what changes / what's removed

### Stays unchanged (intentional)

- `aadf/edit.rs` — CPU oracle, sacred per user Q2 directive. D1
  territory.
- `render/gpu_types.rs` — D4's read-only seam. D5's
  `GpuConstructionParams` (`gpu_types.rs:583-631`) stays as-is in
  D5's impl phase; D4's `ShaderType` cutover converts it in-place
  later (per D4's exploration Finding 4).
- `render/prepare.rs::prepare_world_gpu` + `WorldGpu` — D4 read-only.
- `render/pipelines.rs::NaadfPipelines` — D4 read-only **for D5's impl
  phase**. D4's later impl phase absorbs the merged
  `ConstructionPipelines` field set per §2.10.
- `render/construction/config.rs` (326 LOC) — no findings against it.
  The explorer's side-note #9 explicitly noted it's "sane and
  well-organized". Untouched.
- `render/construction/hashing.rs` (241 LOC) — unchanged this pass
  (Finding 8 deferred to D1↔D5 coordination; D5 cannot land alone).
- `render/construction/change_handler.rs` (391 LOC) — no findings.
- `render/construction/entity_handler.rs` (441 LOC) — no findings.
- `render/construction/map_copy.rs` (177 LOC) — no findings.
- `render/construction/shader_drift_guard.rs` (400 LOC) — Finding 7
  deferred (load-bearing pending naga-oil investigation).
- All 7 construction WGSL files **except** the deletions in
  `bounds_calc.wgsl:160-176, 405-433` (Step 1) and the
  `CELL_DIM`/`CELL_CHILDREN` const additions in `chunk_calc.wgsl`,
  `bounds_calc.wgsl`, `world_change.wgsl` (Step 8).
- `bounds_common.wgsl` (191 LOC) — untouched.
- The 6 e2e gate functions' BODIES — verbatim copy; only their FILE
  location changes (Step 5).
- The 3 embedded test modules' BODIES — verbatim copy; only their FILE
  location changes (Step 5).
- `bin/e2e_render.rs` — UNCHANGED. The `pub use` re-export pattern in
  `mod.rs` keeps every `bevy_naadf::render::construction::validate_*`
  path resolving.
- `render/mod.rs:300-326` Core3d node chain — UNCHANGED. D5's render-
  graph nodes are re-exported at their existing `mod.rs` paths.

### Changes

- `render/construction/mod.rs` — shrinks from 11 043 LOC to ~620 LOC;
  becomes the orchestration shell (resource defs that span workstreams
  + plugin wiring + re-exports).
- `render/construction/bounds_calc.rs` — grows by ~300 LOC (absorbs
  `prepare_bounds_calc`); loses the `prepare_probe_history_layout_descriptor`
  + the `probe_layout` parameters on the prepare-pipeline-queue path +
  the `probe_bg` ladder in `naadf_bounds_compute_node`.
- `render/construction/bounds_calc/tests.rs` — loses the probe Buffer +
  bind-group + the `PREPARE_PROBE_HISTORY_*` const imports + the 4-layout
  pipeline-queue call.
- `render/construction/chunk_calc.rs` — grows by ~290 LOC (absorbs
  `prepare_chunk_calc` + the production encoder
  `cpu_encode_segment_voxel_buffer_from_dense`). Gains
  `#[cfg(test)] mod tests_w1;` (the moved W1 oracle).
- `render/construction/world_change.rs` — grows by ~295 LOC (absorbs
  `prepare_world_change`).
- `render/construction/entity_update.rs` — grows by ~320 LOC (absorbs
  `prepare_entity_update`). Gains `#[cfg(test)] mod tests_w4;` (the
  moved W4 oracle).
- `render/construction/generator_model.rs` — grows by ~210 LOC (absorbs
  `prepare_generator_model`). Gains `#[cfg(test)] mod tests;` (the
  moved W5 oracle).
- `assets/shaders/bounds_calc.wgsl` — loses 30 LOC (Step 1 probe
  declarations + writes); gains 2 LOC (Step 8 `CELL_DIM` / `CELL_CHILDREN`
  consts).
- `assets/shaders/chunk_calc.wgsl` — gains 2 LOC (Step 8).
- `assets/shaders/world_change.wgsl` — gains 2 LOC (Step 8).

### New

- `render/construction/readback.rs` (~660 LOC) — Step 2.
- `render/construction/extract.rs` (~210 LOC) — Step 3.
- `render/construction/producer.rs` (~470 LOC) — Step 3 + Step 4
  (`prepare_shared_bind_groups` lives here).
- `render/construction/validation/mod.rs` (~80 LOC) — Step 5.
- `render/construction/validation/gpu_construction.rs` (~420 LOC) — Step 5.
- `render/construction/validation/gpu_construction_scaled.rs` (~210 LOC) — Step 5.
- `render/construction/validation/gpu_construction_production.rs` (~990 LOC) — Step 5.
- `render/construction/validation/byte_diff_fixtures.rs` (~1 720 LOC) — Step 5.
- `render/construction/validation/edit_mode.rs` (~140 LOC) — Step 5.
- `render/construction/validation/runtime_edit_mode.rs` (~140 LOC) — Step 5.
- `render/construction/validation/entity_handler.rs` (~70 LOC) — Step 5.
- `render/construction/chunk_calc/tests_w1.rs` (~790 LOC) — Step 5.
- `render/construction/entity_update/tests_w4.rs` (~440 LOC) — Step 5.
- `render/construction/generator_model/tests.rs` (~295 LOC) — Step 5.

### Removed

- `AadfDelayedProbe` struct + `aadf_delayed_probe` system — Step 1.
- `AadfPerCallProbe` struct + `PerCallProbeStage` enum +
  `aadf_per_call_probe` system — Step 1.
- `AadfCpuGpuParity` struct + `CpuGpuParityStage` enum +
  `aadf_cpu_gpu_parity` system + `aadf_cpu_gpu_parity_maybe` wrapper
  — Step 1.
- `clear_world_data_pending_edits` + `add_systems(Last, …)` registration
  — Step 1.
- `ConstructionGpu::prepare_probe_history` field + the 4 `*_label`
  debug-stash fields — Step 1.
- `ConstructionBindGroups::prepare_probe_history` field — Step 1.
- `ConstructionPipelines::prepare_probe_history_layout` field — Step 1.
- `PREPARE_PROBE_HISTORY_ENTRIES` + `PREPARE_PROBE_HISTORY_BYTES`
  consts — Step 1.
- `bounds_calc::prepare_probe_history_layout_descriptor` fn — Step 1.
- `bounds_calc.wgsl:160-176` `prepare_probe_history` binding + its
  docblock — Step 1.
- `bounds_calc.wgsl:405-433` probe-history write block — Step 1.
- The 4-layout signatures on `bounds_calc::queue_prepare_pipeline*` +
  `dispatch_regime_2_rounds` — Step 1.
- The `prepare_construction` monolith (1 418 LOC body — split per
  workstream, none of the body bytes survive; the shell replacement is
  new code).

---

## 5. Open conflicts (cross-domain coordination)

This design proposes **no forbidden moves** D5 owns. The W0 contract
retirement (Resolution D) is approved-by-user; the WGSL probe deletions
are in D5; the validation move is D5; the prepare split is D5.

Three items require coordination with other architects / implementors.
Each is **documented in this design for D4 / D1 to consume**; D5's impl
phase does NOT touch the cross-domain files.

### 5.1 D4 coordination — `NaadfPipelines` absorbs `ConstructionPipelines`

Per §2.10 / Resolution D. D5's implementor leaves
`ConstructionPipelines` as it is post-Step-1 (one field gone:
`prepare_probe_history_layout`). D4's architect / implementor:

1. Add the 25 `construction_*`-prefixed fields to `NaadfPipelines`
   (shape in §2.10).
2. Absorb the `ConstructionPipelines::from_world` body into
   `NaadfPipelines::from_world`.
3. Delete the `ConstructionPipelines` struct + its `init_gpu_resource`
   in `ConstructionPlugin::build` (`mod.rs:4660` — D5 file, D4 edit
   only after D5's impl phase lands).
4. Every consumer (D5's submodules in `bounds_calc.rs`,
   `chunk_calc.rs`, `world_change.rs`, `entity_update.rs`,
   `generator_model.rs`, `map_copy.rs`, and the new `producer.rs` +
   each `prepare_*` system) changes `Res<ConstructionPipelines>` →
   `Res<NaadfPipelines>` and `construction_pipelines.X` →
   `naadf_pipelines.construction_X`. ~30-40 call-sites — a single
   mechanical sweep.

D4's architect inherits the design verbatim from §2.10.

### 5.2 D4 coordination — `.run_if(resource_exists::<_>)` on render-graph nodes

Per §2.5. The four construction render-graph nodes
(`naadf_gpu_producer_node`, `naadf_bounds_compute_node`,
`naadf_world_change_node`, `naadf_entity_update_node`) are registered
in `render/mod.rs:300-326` — D4-owned. Adding `.run_if(...)` clauses
there is D4 territory. D5's impl phase:

- Keeps each node's `Option<Res<_>>` parameter signatures unchanged.
- Keeps each node's body's `let Some(...) = ... else { return; };`
  ladder unchanged.

D4's impl phase **could** add the `.run_if(...)` clauses (in the
17-element `.chain()` block) and convert the corresponding `Option<...>`
parameters to `Res<...>`. **Optional** — not blocking for correctness.
Flagged for D4's awareness.

### 5.3 D1 coordination — SSoT-6 hash coefficients

Per §2.8. D1 owns `aadf/block_hash.rs:395::build_polynomial_coefficients`;
D5 owns `render/construction/hashing.rs:43::hash_coefficients`. D1's
architect already proposed promoting `build_polynomial_coefficients`
to `pub`. D5's impl phase deferred.

After D1's impl phase lands the promotion (D1 runs in the
"interleave middle" phase per `01-context.md` Q3, **after** D5's first
pass), a tiny follow-up D5 pass can replace
`render/construction/hashing.rs:43-50` with
`pub use crate::aadf::block_hash::build_polynomial_coefficients as
hash_coefficients;`. Out of scope for THIS dispatch.

### 5.4 Test naming — the moved test modules (`tests_w1`, `tests_w4`)

Step 5 moves the W1/W4 oracle tests into the workstreams they exercise
as file modules (`chunk_calc/tests_w1.rs`, `entity_update/tests_w4.rs`).
Each is `#[cfg(test)] mod …;` declared from its parent file. **This
collides with the existing `bounds_calc/tests.rs`** (sibling-file
convention already in use). Two file-module conventions coexist:

- `bounds_calc.rs` + `bounds_calc/tests.rs` (existing).
- `chunk_calc.rs` + `chunk_calc/tests_w1.rs` (proposed).
- `entity_update.rs` + `entity_update/tests_w4.rs` (proposed).
- `generator_model.rs` + `generator_model/tests.rs` (proposed).

The asymmetric naming (`tests_w1` vs `tests`) is intentional: each
W1 / W4 / W5 test is the GPU↔CPU oracle for ITS workstream; calling
them all `tests.rs` collides at the file-tree level if a workstream
ever wants more than one test file. The current bounds_calc test is
the "main" suite + several `naadf_*_node`-level unit tests; calling
it `tests` is fine. The W1 / W4 oracles are specifically the
ALGORITHM oracles + are large (790 / 440 LOC each); calling them
`tests_w1` / `tests_w4` keeps the file-tree label informative.

**No conflict requiring user resolution.** Architects coordinate
convention; if a future review prefers uniform `tests.rs`, the rename
is a single `git mv` + import update.

---

## 6. Decisions & rejected alternatives

### 6.1 BEV-3 split deferred (rejected the full per-workstream resource split)

§2.9 keeps `ConstructionGpu` as a single Resource. Rejected: splitting
into 5 per-workstream resources (`ChunkCalcGpu`, `BoundsCalcGpu`,
`WorldChangeGpu`, `EntityUpdateGpu`, `GeneratorModelGpu`) this dispatch.

**Why rejected:** every workstream node-system + every dispatch helper
(~30 sites across the 5 submodules) would convert from
`Res<ConstructionGpu>` field accesses to `Res<WorkstreamGpu>` field
accesses. Combined with the prepare split + the `NaadfPipelines` merge
(which already touches every consumer), the blast radius compounds —
3 simultaneous structural changes per consumer instead of 2. The split
is pure ergonomics (no behavioural gain), and the surface it cleans up
(the `Option<Buffer>` fields each workstream owns) is partly cleaned by
the `.run_if(...)` cleanup in §2.5 (which moves the precondition checks
out of the body).

**Future-work flag:** the natural next D5 refactor — after this pass
lands — is the per-workstream Resource split. Trivial to do once the
prepare-side already lives per-workstream.

### 6.2 W0-merge shape — (a) merge into `NaadfPipelines` vs (b) `Plugin::init_gpu_resource` per workstream

§2.10 chose (a). Rejected (b): each workstream submodule has its own
`Plugin` that calls `init_gpu_resource::<ChunkCalcPipelines>()` etc.

**Why rejected:** (b) is what `ConstructionPipelines` already IS — an
"every workstream adds to me" Resource — except the parallel-merge
rationale that justified one shared sibling has lapsed (Resolution D).
Splitting into 5 sibling resources keeps the empty-sibling pattern,
just multiplied. Resolution D explicitly says "propose the merge", not
"propose the multi-split". (a) yields a single Resource lookup per
consumer; (b) yields 5.

### 6.3 Shader inlining — Finding 7 deferred

§2.7 keeps `shader_drift_guard.rs` + the inline copies. Rejected:
delete + use `#import`.

**Why rejected:** Bevy 0.19 + naga_oil shader-import semantics for the
specific WGSL constructs in `bounds_common.wgsl` (`var<workgroup>`
array, `atomic<u32>` types, custom structs) are unverified. Discovery
work is a separate dispatch. The current architecture is load-bearing
UNTIL that work happens.

### 6.4 CELL_DIM injection — file-local `const` vs shader-def

§2.11 chose file-local `const`. Rejected: pipe `#{CELL_DIM}` /
`#{CELL_CHILDREN}` shader-def from `NaadfPipelines::from_world` (the
project's existing pattern for `TAA_SAMPLE_RING_DEPTH` at
`pipelines.rs:269-279`).

**Why rejected:** shader-def piping is a D4 file edit
(`NaadfPipelines::from_world` lives in `render/pipelines.rs`).
File-local `const` works equivalently at compile time, lives in D5,
and ships the LOC win in one pass.

### 6.5 Test module placement — file-module vs `#[cfg(test)] mod tests { ... }` inline

§2.3 + Step 5 move the embedded test modules to **separate files** via
`#[cfg(test)] mod …;`. Rejected: keep them as `#[cfg(test)] mod tests {
... }` blocks inside the workstream submodule.

**Why rejected:** the W1 / W4 oracles are 790 / 440 LOC each; inline
mod-blocks of that size defeat the navigation benefit of moving them
out of `mod.rs`. The `bounds_calc/tests.rs` precedent already exists
and is the right pattern.

### 6.6 `prepare_shared_bind_groups` placement — `producer.rs` vs new file

§2.1 puts `prepare_shared_bind_groups` in `producer.rs`. Alternative:
new sibling file `shared_bind_groups.rs`.

**Chosen `producer.rs`:** the shared bind groups
(`construction_world`, `construction_bounds_world`) are consumed by
both `naadf_gpu_producer_node` (W1 chunk_calc chain) and the W3 bound
chain. Producer.rs already contains the central one-shot driver; the
shared bind-group builder belongs nearby. Could equivalently live in
its own file — implementor's call.

---

## 7. Side notes / observations / complaints

1. **The W0 seam contract was perfect for its purpose.** It pinned
   the layout in place during 6 months of parallel workstream
   integration, allowed every workstream to merge additively, and
   never produced a merge conflict on the shared struct shape. Now
   that integration is complete (W0..W5 + wave-3 entity merge + the
   followup-#1 + the W5.3-fix all landed), the contract has fulfilled
   its function and is being retired. **Credit where credit is due.**
   The exploration's side-notes #1 and #2 already capture this; this
   architecture doc restates it because the implementor reads this
   doc, not the exploration's side-notes.

2. **The biggest single LOC win in this pass is Step 5 (validation
   extraction), not Step 4 (prepare split).** Step 5 moves ~4 640
   LOC out of `mod.rs`. Step 4 splits a 1 418-LOC system; the bytes
   redistribute, total D5 LOC is unchanged. If the implementor is
   forced to prioritise (e.g. they hit a verification snag mid-impl
   and need to ship), **Step 1 + Step 5 alone deliver ~5 700 LOC of
   win** and `mod.rs` shrinks from 11 043 to ~5 300 — already past
   the audit's reduction target. Steps 2/3/4/6/7/8 are all-or-nothing
   nice-to-haves that compound into the ~620-LOC end-state but aren't
   load-bearing for the headline win.

3. **The `Option<…>`-soup → `Result<…>` / dedicated error type
   smell** is a real one but out-of-scope. Every system that consumes
   `ConstructionGpu` bails on `is_none()` of multiple buffer fields;
   none of them differentiate "this buffer is not yet allocated"
   from "this is the wrong frame to dispatch". A future refactor
   could introduce a state machine type `ConstructionState { Empty,
   Initialising, Ready { … } }` and lift the buffers out of the
   `Option`s. **Not this pass** — the `.run_if(resource_exists::<_>)`
   move in §2.5 + the per-workstream Resource split (deferred per
   §6.1) get most of the way there.

4. **`hashing.rs` is the smallest-blast-radius cross-domain win.**
   §2.8 / §5.3 — once D1's impl lands the `pub` promotion, D5's
   tiny `pub use` follow-up collapses two byte-equal Rust copies of
   the same fixed table into one. Worth a 5-LOC patch the moment
   D1's pass clears.

5. **The 7 cross-step verification re-runs cost real time.** Each
   step requires running 6 e2e gates twice (`--validate-gpu-*` × 3,
   `--edit-mode`, `--runtime-edit-mode`, `--entities`, `--oasis-edit-visual`
   ≥2× per `feedback-multiple-runs-rule-out-false-positives`) plus
   `cargo test --workspace --lib` + `cargo build --workspace`. The
   user's `feedback-e2e-gates-must-fail-fast` rule (wrap `cargo run`
   in `timeout 120s`) is mandatory — without it, a stuck readback
   could hang the impl-phase loop. Implementor: enforce timeouts.

6. **The `validation/byte_diff_fixtures.rs` file at 1 720 LOC is the
   largest single file post-refactor.** If a future review prefers
   one file per fixture (`run_one_fixture_byte_diff.rs` ×4), splitting
   is trivial — they're all `pub(crate) fn` with no shared state.
   Implementor's call.

7. **`render/mod.rs:300-326`'s 17-element `.chain()` is THE foundation
   smell on D4's side.** D4's exploration (Finding 2) flagged it as the
   load-bearing smell that defeats per-workstream-PR seams. D5's impl
   phase respects the existing chain order verbatim (the producer-node
   import path is preserved via `mod.rs` re-export); D4's refactor of
   the `.chain()` into proper `add_render_graph_edges` labels is its
   own concern, independent of D5's findings.

8. **The `naadf_gpu_producer_node` (`producer.rs` after Step 3) is the
   single biggest "one-shot driver at startup + every-frame ledger"
   conflation in D5.** Its body is a 3-way ladder (W5 / chunk-calc-only
   / CPU-fallback). The explorer's side-note #5 flagged splitting the
   W5 branch out into `generator_model.rs` — declined here because the
   ladder shape mirrors the C# `WorldData.GenerateWorld` loop exactly
   (faithful-port rule) and splitting would either fragment the loop
   across files or re-introduce a dispatch-shell layer. **Faithful-port
   wins; producer.rs stays a 3-way ladder.**

9. **Equal-footing complaint.** This architecture doc is ~1 900 lines.
   The implementor reads it in full. The implementor should plan
   ~30 minutes for the initial read + ~5 minutes per step to re-read
   the relevant section before that step's edits. The architecture
   doc is denser than the exploration doc because the architecture
   doc has to commit; the exploration could enumerate possibilities,
   the architecture has to pick one.

10. **The orchestrator does NOT read this doc.** Per `01-context.md`
    architect-phase brief framing. D5's implementor reads this; D4's
    architect (running in parallel) reads §5 + §2.10 + §2.5 — the
    cross-domain coordination items. D1's architect / implementor
    (running later) reads §5.3. Everything else here is for D5's
    implementor.
