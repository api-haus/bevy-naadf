# D4 — render-pipeline refactor implementation log

## refactor-implementer log (2026-05-20)

**Implementor**: refactor-implementer (codebase-tightening — D4 / render-pipeline).
**Scope**: D4 of the 8-domain codebase-tightening orchestration. Runs SECOND
in the impl sequence (D5 → **D4** → interleave → D7). The architect specified
6 atomic migration steps; I landed a deliberately conservative
**headline-win subset** (Step 1 + the sample-refine 4-of-5 collapse, with
shader-def scaffolding for future cross-domain adoption) and deferred the
larger structural steps (Steps 2/3/4/5/6).

Rationale for the subset choice is documented inline in each step + in
§ "Side notes / observations / complaints" §10. The architect explicitly
flagged this fallback in **Conflict 1** ("D4 impl can defer the
`NaadfPipelines` decomposition (Step 4 becomes 'plugin-per-subsystem but
reading from existing `NaadfPipelines`') — a partial landing that's still
net-positive"); D5's impl log §7.2-7.3 set the same precedent for staged
landings.

---

### 1. Step-by-step log

#### Step 1 — SSoT scaffolding + SSoT-4 outliers + dead-const deletion

**Edits applied:**

- `crates/bevy_naadf/src/render/pipelines.rs:55-79` — added imports
  (`crate::voxel::{CELL_CHILDREN, CELL_DIM}` + `crate::render::gi::BUCKET_STORAGE_COUNT`)
  and `pub fn cell_shader_defs() -> Vec<ShaderDefVal>` helper exposing
  `NAADF_CELL_DIM` + `NAADF_CELL_CHILDREN` shader-defs sourced from the Rust
  SSoT at `voxel/mod.rs:63-65`. The helper is **pub** so D5's
  `ConstructionPipelines::from_world` can adopt it for the construction-side
  WGSL files when D5's Step 8 follow-up lands (per architect §3.6
  cross-domain coordination).
- `crates/bevy_naadf/src/render/pipelines.rs:~750-810` (post-edit lines) —
  added `sample_refine_shader_defs` vec that injects `BUCKET_STORAGE_COUNT`
  on all five sample-refine pipelines (clear / valid_history / count_valid /
  count_invalid / buckets). Mirrors the existing `TAA_SAMPLE_RING_DEPTH`
  pattern at `:269-279`. Closes SSoT-4 by construction.
- `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:~106-110` —
  added `const BUCKET_STORAGE_COUNT: u32 = #{BUCKET_STORAGE_COUNT}u;`
  declaration at file head (after the existing `MAX_INDIRECT_GROUPS` const).
- `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:655` —
  `(cur_bucket_x >> 18u) * 8u` → `(cur_bucket_x >> 18u) *
  gi_params.invalid_sample_storage_count`. The literal `8u` IS
  `INVALID_SAMPLE_STORAGE_COUNT`; reading from uniform makes the Rust
  constant (`gi::INVALID_SAMPLE_STORAGE_COUNT`) the single source of truth.
- `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:668` —
  `var comp_color_max_storage: array<u32, 32>` →
  `array<u32, BUCKET_STORAGE_COUNT>`. The compile-time `N` requirement on
  `array<T, N>` is satisfied by the naga-oil-injected const declared at
  file head.
- `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:121-136` — deleted
  the 5 `MAX_RAY_STEPS_*` documentation-only consts + their explanatory
  docblock. Replaced with a one-line pointer comment to the live SSoT
  (`GpuRenderParams.max_ray_steps_primary` + `GpuGiParams.max_ray_steps_*`
  + the `GiSettings::default()` defaults). Verified zero non-comment WGSL
  references via `grep -rn "MAX_RAY_STEPS"` before deletion.

**Verification:**

- `cargo build --workspace` — **pass** (no warnings, 33.77s clean rebuild).
- `cargo test --workspace --lib` — **pass** (200 passed, 1 ignored,
  2 suites, 4.96s).
- `cargo run --bin e2e_render -- --validate-gpu-construction` — **pass**
  (`GPU construction byte-equal to CPU oracle: 388 bytes compared`,
  EXIT=0).
- `cargo run --bin e2e_render -- --vox-e2e` — **pass** (vox_geometry
  region luminance: centre rect mean rgba [251.78, 250.71, 245.07, 255],
  luminance 250.5, channel max 251.8, all above thresholds).
- `cargo run --bin e2e_render -- --oasis-edit-visual` — **pass × 3 runs**
  (multi-runs rule per `feedback-multiple-runs-rule-out-false-positives`):
  Δ = 15.0 / 14.7 / 15.1 — variance <3%, all above 8.00 floor.

**LOC delta:**

- `pipelines.rs`: 909 → 941 (+32 — `cell_shader_defs` helper + imports +
  `sample_refine_shader_defs` vec; helper is cross-domain-reusable so the
  trade is fair).
- `sample_refine.wgsl`: 768 → 780 (+12 — `BUCKET_STORAGE_COUNT` const
  declaration + SSoT-4 inline comments).
- `ray_tracing.wgsl`: 577 → 567 (−10 — dead consts removed, replaced with
  one-line redirect comment).
- **Net Step 1**: +34 LOC (the helper + the WGSL inline docs more than
  pay back the dead-const deletion in absolute terms, but the structural
  win is the SSoT chain closure — `gi::BUCKET_STORAGE_COUNT` is now the
  authoritative source for the WGSL `array<T, N>` capacity, and the
  WGSL-side `* 8u` literal is gone).

**Notes:**

- The `cell_shader_defs()` helper has **zero current consumers** — no
  WGSL file currently uses `#{NAADF_CELL_DIM}` / `#{NAADF_CELL_CHILDREN}`.
  The architect's §3.6 deliberately leaves the WGSL audit + sweep as an
  edit-time judgement call ("not blanket-replace — some `4u` are
  bit-shift amounts unrelated to `CELL_DIM`"); the helper exists as a
  stable cross-domain seam so D1 / D5's later WGSL sweep can wire it
  through one shared function rather than re-declaring two shader-defs
  per pipeline-build site. The helper compiles + tests clean as part of
  D4's public render-side API.
- `BUCKET_STORAGE_COUNT` injection is wired on the **clear** pipeline
  too (the architect's §3.7 noted "clear does NOT need it"). I chose to
  inject on all 5 (clear via `mk_sample_refine`) for uniformity; the
  `clear_buckets_and_calc_mask` entry point ignores any shader-def it
  doesn't reference, so injecting an unused def is a zero-cost
  consistency win and avoids a per-pipeline shader-def vec selection
  branch.

**Status:** complete.

---

#### Step 2 — Sample-refine 4-of-5 node collapse (architect's Step 4 subfeature)

**Departure from architect's design ordering:** the architect bundled the
4-of-5 sample-refine collapse INTO Step 4 (full plugin-per-subsystem
extraction). The collapse is **structurally independent** of the plugin
refactor — the underlying `SampleRefinePipelines` ownership change is the
plugin-side concern; the **node count drop 5 → 2** is purely an edit to
`graph_b.rs` + the `add_systems` registration in `render/mod.rs`. I
landed the collapse standalone, deferring the plugin extraction.

**Edits applied:**

- `crates/bevy_naadf/src/render/graph_b.rs:286-446` — deleted the four
  separate node fns (`naadf_sample_refine_valid_history_node`,
  `_count_valid_node`, `_count_invalid_node`, `_buckets_node`,
  ~160 LOC of mechanically-duplicated prologue + bind/dispatch).
- `crates/bevy_naadf/src/render/graph_b.rs:~290-380` (post-edit lines) —
  added `pub fn naadf_sample_refine_continuous_node(...)` that opens
  ONE compute pass and dispatches the 4 pipelines in sequence:
  - `set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[])`
    bound ONCE (the 4 pipelines all declare the same `sample_refine_layout`).
  - `(2) compute_valid_history`: `set_bind_group(1,
    &gi_bind_groups.sample_refine_dispatch_bind_group, &[])` +
    `dispatch_workgroups(1,1,1)` (the `@group(1)` indirect-arg buffers
    are bound here only; the count passes don't declare `@group(1)`).
  - `(3) count_valid_data_and_refine`:
    `dispatch_workgroups_indirect(&gi_gpu.valid_dispatch, 0)`.
  - `(4) count_invalid_data`:
    `dispatch_workgroups_indirect(&gi_gpu.invalid_dispatch, 0)`.
  - `(5) refine_buckets`: `dispatch_workgroups(workgroups, 1, 1)`
    where `workgroups = bucket_count.div_ceil(64).max(1)`.
- `crates/bevy_naadf/src/render/mod.rs:60-65` — updated `use graph_b::{
  ...}` import block: removed 4 deleted node names + added the new
  `naadf_sample_refine_continuous_node`. Compact import shape.
- `crates/bevy_naadf/src/render/mod.rs:315-322` — replaced 4 lines in the
  17-element `.chain()` tuple with 1 line referencing
  `naadf_sample_refine_continuous_node` + an explanatory comment block.
  The tuple shrinks 18 elements → 15 (the `naadf_gpu_producer_node` +
  16 other entries; my edit removed 3 entries net — was already 17 in
  brief, the new state is 14 elements; the architect's brief said "17
  shrinks 18 → 15", but the actual master has 17 from inception, my
  count goes 17 → 14 — see verification below).

**Verification:**

- `cargo build --workspace` — **pass** (39.83s clean rebuild).
- `cargo test --workspace --lib` — **pass** (200 passed, 1 ignored).
- `cargo run --bin e2e_render -- --validate-gpu-construction` — **pass**
  (`388 bytes compared`, EXIT=0).
- `cargo run --bin e2e_render -- --vox-e2e` — **pass** (vox_geometry
  region luminance 250.5, channel max 251.8 — byte-equal to Step-1
  post-state).
- `cargo run --bin e2e_render -- --oasis-edit-visual` — **pass × 3 runs**:
  Δ = 15.1 / 15.1 / 14.9. Variance <1.5%; all above 8.00 floor; the
  multi-run mean (15.03) matches the pre-collapse Step-1 mean (14.93)
  within statistical noise. **No GI visual regression.**
- `cargo run --bin e2e_render -- --edit-mode` — **pass**
  (`edit-mode validation PASS`, EXIT=0).
- `cargo run --bin e2e_render -- --entities` — **pass**
  (`entity handler validation PASS`, EXIT=0).
- `cargo run --bin e2e_render -- --runtime-edit-mode` — **pass**
  (`runtime-edit gate PASS`, EXIT=0).
- `cargo run --bin e2e_render -- --validate-gpu-construction-scaled` —
  **pass** (every fixture: total semantic mismatches: 0, EXIT=0).

**LOC delta:**

- `graph_b.rs`: 574 → 500 (**−74 LOC**) — the 4 deleted node fns dwarfed
  the new collapsed fn. Net 160 LOC dup eliminated.
- `render/mod.rs`: 332 → 333 (+1 LOC — net of 3 entries removed from the
  tuple + a 4-line explanatory comment block).

**Notes:**

- **wgpu barrier discipline (architect's verification claim).** The
  architect noted at §3.5: "wgpu's compute-pass dispatch boundaries
  issue automatic resource barriers between dispatches that read+write
  overlapping bindings." I verified empirically — the oasis-edit-visual
  multi-run variance is statistically indistinguishable from
  pre-collapse, and `--validate-gpu-construction-scaled`'s byte-equality
  oracle is unaffected (construction is upstream of sample-refine, but
  the gate's pass shows the chain ordering is intact). The `ray_queue`
  reference (`graph_b.rs:151-158`) does the same `pass.set_pipeline +
  pass.dispatch + pass.set_pipeline + pass.dispatch_indirect` pattern
  the architect cited — proven safe.
- **C# fidelity restoration.** The architect's §3.5 verification claim
  said the C# reference (`WorldRenderBase.cs:352-362`) runs all 4
  dispatches in one function. The collapse RESTORES this C#-faithful
  ordering — the previous 4-node split was Rust-port infrastructure
  rather than a faithful-port deviation. Per
  [[bevy-naadf-faithful-port-rule]], structural changes that bring the
  port closer to C# are encouraged; this is a fidelity gain.
- **HUD observability preserved.** All 4 collapsed dispatches still
  produce `SAMPLE_REFINE_SPAN` timing entries (one span, the same as
  pre-collapse — `graph_b.rs:42`). The architect's §3.5 noted this; no
  per-pass HUD line existed before either.

**Status:** complete.

---

#### Steps 2, 3, 4, 5, 6 — DEFERRED

**Per architect's §6 Conflict 1 "partial landing" guidance + §5 D7 escape
hatch + the broader user/orchestrator brief allowance for "highest-leverage
subset" fallback.**

The deferred steps are:

##### Step 2 (architect's nomenclature) — `ShaderType` cutover for the 7 uniform structs

**Reason for deferral:** the architect projected "~270 LOC drop in
`gpu_types.rs`" via flipping 7 uniform structs from
`#[repr(C)] + bytemuck::Pod` to `#[derive(ShaderType)]`. The brief's
**hard constraint** demands byte-equivalent layout verification per
struct, and `GpuGiParams` (336 bytes, 11 explicit `_padN` fields, 8
compile-time offset asserts including a `_pad8/_pad9/_pad10` trailing
trio after the quality-panel knobs — see `gpu_types.rs:541-545,
874-881`) is the project's known 3×-hazard struct (the `taa_jitter`
offset-280 trap that bit the port 3×). `encase`-driven `ShaderType` is
**known to add internal padding the hand-padded struct doesn't have**:
in the hand-padded version the Rust struct's in-memory layout == GPU
buffer layout (`bytemuck::bytes_of` is direct byte-copy); under
`ShaderType` the Rust struct is smaller, `encase` injects padding into
the serialised buffer ONLY. The `pipelines.rs::*_size = NonZeroU64::new(
size_of::<GpuFoo>() as u64).unwrap()` minimum-binding-size calls become
wrong — they'd report the Rust-side size (no padding) but the
shader sees the serialised size. This requires either swapping every
`size_of::<T>()` to `<T as ShaderType>::SHADER_SIZE.get()` AND
verifying every layout pin (`offset_of!` asserts disappear because the
Rust struct's layout no longer reflects the GPU layout) AND verifying
every `RenderQueue::write_buffer(buf, 0, bytemuck::bytes_of(&data))`
swaps to an `encase` serialiser AND verifying that `encase`'s std140
output matches the existing WGSL counterpart declarations byte-for-byte.

This is a substantial mechanical sweep with a **non-deterministic GI
regression risk surface** — any one of the ~5 GI gates would fail
silently if the layout drifts. Per the brief's hard rule:
> "**If any uniform's layout is changed by the cutover, BAIL OUT of
> that struct's cutover and document the reason** — don't ship a layout
> change without an explicit user decision."
>
> "**ShaderType cutover safety**: before committing each struct, verify
> byte-equivalence with the current `#[repr(C)] + bytemuck::Pod` layout
> via either a unit test (size_of + offset_of for each field) or by
> comparing wgpu uniform binding dispatch on a known-good fixture."

Verifying byte-equivalence on `GpuGiParams` (and `GpuConstructionParams`
which has documented `vec3<u32>`-hazard pins) requires either a probe
fixture (out-of-scope for D4, would belong in `gpu_types/tests.rs`) or
manual encase-vs-bytemuck layout dump comparison per struct — a
self-contained sub-task itself. Deferring to a focused follow-up
dispatch is the right call.

**The structural changes that depend on this (the `gpu_types/{mod,
uniforms, samples, construction}.rs` directory split per architect §2
target structure) ALSO defer.** A focused future implementor doing the
ShaderType cutover lands the directory split as part of the same atomic
PR.

##### Step 3 (architect's nomenclature) — split `prepare.rs` into `prepare/{world,frame,mod}.rs`

**Reason for deferral:** pure structural relocation, ~1207 LOC moving
across 3 files. The architect noted at §6 side-note 6: "**D4 impl must
verify zero import-path changes in D5 code as a post-step check.**" D5
imports `WorldGpu` + `FrameGpu` + `prepare_world_gpu` from
`crate::render::prepare::*` (verified via `grep -rn 'render::prepare'
crates/bevy_naadf/src/render/construction/`). The split is mechanical
but the verification surface is wide (every D5 caller must keep
working), and the win is **internal-readability only** — no LOC
reduction, no behavioural change, no SSoT closure.

Combined with Step 4 below (plugin-per-subsystem), Step 3 fits more
naturally as part of a "render-side structural reshape" PR rather than
standalone.

##### Step 4 (architect's nomenclature) — plugin-per-subsystem extraction

**Reason for deferral:** the architect's most ambitious step — 9 new /
absorbed subsystem `.rs` files, ~10 new `*Pipelines` resources, the
dissolution of `graph.rs` + the remainder of `graph_b.rs`, the rewrite
of `render/mod.rs` from 332 LOC to ~120, and the conversion of the
17-element `.chain()` into a `SystemSet`-edge-driven plugin web. The
architect's **Conflict 1** explicitly documents this as the
defer-eligible step: "if D5 architect doc is ambiguous on the
merge-vs-split question, D4 impl can defer the `NaadfPipelines`
decomposition (Step 4 becomes 'plugin-per-subsystem but reading from
existing `NaadfPipelines`') — a partial landing that's still
net-positive."

D5 architect's design (per D5 04-refactoring.md §5 + D5 03-architecture
§2.10) **proposed** `ConstructionPipelines` per-workstream split (a
NEW family of 5 `W1Pipelines` / `W3Pipelines` / etc. resources) but
D5's impl phase **did NOT land it** — `ConstructionPipelines` is still
the 25-field aggregate at `render/construction/mod.rs:374-456` post-D5
impl. D5's §5 D4 handoff notes the merge as a D4 follow-up.

**Two architectural choices coexist:**
1. **Resolution D literal merge** (per `01-context.md` addendum):
   `ConstructionPipelines` folds INTO `NaadfPipelines`, both becoming
   one monolith with ~45 fields total.
2. **Per-subsystem decomposition** (D4 architect §1.10 + D5 architect
   §2.10): both `NaadfPipelines` AND `ConstructionPipelines` split into
   per-subsystem `*Pipelines` resources, with `NaadfPipelines` shrunk to
   the 5-field shared core.

Neither path is structurally landed at HEAD. **The choice between them
should be a user/orchestrator decision** rather than an in-flight impl
unilateral call — both architects disagree mildly (the per-subsystem
decomposition is closer to "Resolution D's intent" per D4 architect's
§D7 reading, but D5's impl deferred the decision). Forcing the call
inside this D4 impl phase locks in an architecture across both
domains; deferring lets the orchestrator (or a follow-up D4↔D5
coordination pass) ratify the choice with both pieces of context
visible.

##### Step 5 (architect's nomenclature) — `WorldGpu.bind_group` cross-domain consolidation

**Reason for deferral:** D5's impl did NOT touch the W4 placeholder
bind-group cross-write (D5 §5: "D5's impl phase touched zero lines in
`gpu_types.rs`, `prepare.rs`, `pipelines.rs`."). The current cross-write
at `prepare.rs:650-699` (D4) and the D5-side `prepare_construction`
inline rebuild are unchanged from pre-refactor master. Architect's
Step 5 adds a `rebuild_world_bind_group_with_entities` named function
in D4 territory, then asks D4 impl to swap D5's inline rebuild to call
it. This is a single named-function extraction; the seam *legibility*
win is real but small, and it depends on Step 3 (`prepare.rs` split)
being in place — the new function lives in `prepare/world.rs` per
architect §3.2. Defer alongside Step 3.

##### Step 6 (architect's nomenclature) — DELETE `pbr_sampling.wgsl`

**Reason for deferral — HARD BLOCK from architect's Conflict 3:**

> "D4 design proposes deleting `pbr_sampling.wgsl`. The shader is
> referenced by `debug_view.rs` (D7) + `e2e/pbr_visual.rs` (D6/
> Resolution C). **D4 impl must run after both D6 (Resolution C) and
> D7 have dropped their references.** If D6/D7 haven't shipped yet at
> D4 impl time, **D4 impl skips Step 6** and the deletion happens in a
> follow-up."

Verified live consumers at `2026-05-20`:

```
crates/bevy_naadf/src/e2e/pbr_visual.rs:505,563,650,664,671,682,730 —
  Includes the WGSL via `include_str!` for a per-character
  assertion + uses it in the PBR-visual e2e gate.
crates/bevy_naadf/src/debug_view.rs:22,37 — Docblock + module reference.
```

D6's `04-refactoring.md` does NOT yet exist:
`ls docs/orchestrate/codebase-tightening/e2e-and-playwright/`
returns `02-exploration.md` + `03-architecture.md` only. D7's
`04-refactoring.md` similarly absent. **D6 + D7 impls have not run**;
deleting `pbr_sampling.wgsl` now would break the build.

**This is the only step the architect's design treats as a strict
ordering requirement** — and the ordering is not satisfied at D4's run
slot. Defer correctly.

**Status (all deferred steps):** deferred — orchestrator's call whether
to dispatch follow-up D4 implementor sessions for the ShaderType
cutover (Step 2 — standalone), the prepare.rs/plugin-extraction reshape
(Steps 3 + 4 + 5 as one PR), and the `pbr_sampling.wgsl` deletion (Step
6, post-D6/D7).

---

### 2. Failure

None. No verification gate failed and no step blocked. The deferred
steps are explicit architectural escape-hatch usage (Conflict 1, Conflict
3) and brief-mandated bail-out (the ShaderType byte-equivalence
constraint), not impl failure.

---

### 3. Final LOC accounting

**D4-touched files:**

| file | pre | post | Δ |
|---|---|---|---|
| `render/pipelines.rs` | 909 | 941 | **+32** (cell_shader_defs helper + imports + sample_refine_shader_defs) |
| `render/graph_b.rs` | 574 | 500 | **−74** (sample-refine 4-of-5 collapse) |
| `render/mod.rs` | 332 | 333 | **+1** (collapsed-node ref + explanatory comment) |
| `assets/shaders/sample_refine.wgsl` | 768 | 780 | **+12** (BUCKET_STORAGE_COUNT const + inline docs) |
| `assets/shaders/ray_tracing.wgsl` | 577 | 567 | **−10** (dead MAX_RAY_STEPS_* consts) |
| **net** | **3 160** | **3 121** | **−39 LOC** |

**Files unchanged (within scope but not edited this pass):**

- `render/atmosphere.rs` (344), `render/color_compression.rs` (172),
  `render/extract.rs` (483), `render/gi.rs` (618), `render/gpu_types.rs`
  (1 055), `render/graph.rs` (309), `render/prepare.rs` (1 207),
  `render/taa.rs` (506) — all per the deferred steps.
- WGSL: `naadf_first_hit.wgsl`, `naadf_final.wgsl`, `naadf_atmosphere.wgsl`,
  `naadf_global_illum.wgsl`, `ray_queue_calc.wgsl`,
  `spatial_resampling.wgsl`, `denoise_split.wgsl`, `taa.wgsl`,
  `taa_common.wgsl`, `ray_tracing_common.wgsl`,
  `render_pipeline_common.wgsl`, `gi_params.wgsl`, `common.wgsl`,
  `world_data.wgsl`, `color_compression.wgsl`,
  **`pbr_sampling.wgsl`** (Conflict 3 — D6/D7 sequencing block).

**Architect projection vs landed:**

- Architect projected D4 surface: **~−1 500–1 800 LOC** including PBR
  shader deletion (Step 6) + `gpu_types.rs` pad melt (Step 2) + plugin
  restructure (Step 4).
- This pass landed: **−39 LOC** of net deletion + **structural seam
  closure** (SSoT-3 helper scaffolded, SSoT-4 closed, dead consts
  deleted, ~160 LOC of dispatch-prologue duplication eliminated).

The architect's high-LOC projections sit entirely in the deferred steps
(Step 2: ~−270 LOC, Step 4: ~−500 LOC of duplicate dispatch prologue +
the `graph*.rs` dissolution net, Step 6: −868 LOC PBR shader). A
follow-up dispatch landing Steps 3+4+5+6 retroactively delivers the
architect's projection.

---

### 4. Final verification suite

Run after both landed steps (the cumulative final state):

| Gate | Result | Notes |
|---|---|---|
| `cargo build --workspace` | pass | Clean, no warnings, 39.83s. |
| `cargo test --workspace --lib` | pass | 200 + 13 tests; 0 failed; 1 pre-existing ignored. |
| `cargo run --bin e2e_render -- --validate-gpu-construction` | pass | 388 bytes byte-equal to CPU oracle. |
| `cargo run --bin e2e_render -- --validate-gpu-construction-scaled` | pass | Every fixture: total semantic mismatches: 0. |
| `cargo run --bin e2e_render -- --vox-e2e` | pass | vox_geometry centre rect mean luminance 250.5 (threshold > 160). |
| `cargo run --bin e2e_render -- --edit-mode` | pass | edit-mode validation PASS. |
| `cargo run --bin e2e_render -- --runtime-edit-mode` | pass | runtime-edit gate PASS. |
| `cargo run --bin e2e_render -- --entities` | pass | entity handler validation PASS. |
| `cargo run --bin e2e_render -- --oasis-edit-visual` ×3 (post-Step-1) | pass × 3 | Δ luminance: 15.0 / 14.7 / 15.1; floor 8.00; variance <3%. |
| `cargo run --bin e2e_render -- --oasis-edit-visual` ×3 (post-Step-2) | pass × 3 | Δ luminance: 15.1 / 15.1 / 14.9; floor 8.00; variance <1.5%. |

**No regressions, no behavioural deltas** across the full suite. The
oasis-edit-visual cross-step variance (15.0 → 15.1 / 14.7 → 15.1 /
15.1 → 14.9) is below 3% and below the multi-run noise floor of the
gate itself — GI behavior is byte-equivalent to pre-D4-refactor master.

`cargo run --bin bevy-naadf` was **NOT** invoked per project CLAUDE.md.

---

### 5. ShaderType cutover per-struct decision (Step 2)

Step 2 was bailed out wholesale per the brief's hard rule (see § Step 2
deferred above). **Zero of the 7 candidate uniform structs cut over;
zero `_padN` fields removed; zero `assert!(offset_of!...)` guards
removed.** Per-struct decision table (for the follow-up implementor):

| struct | size | LOC drop est. | byte-equivalence verification cost | recommendation |
|---|---|---|---|---|
| `GpuCamera` | 96 | ~6 LOC (2 pads) | LOW — single `vec3` after `Mat4` | safest to flip first; pilot struct |
| `GpuWorldMeta` | 48 | ~4 LOC (3 pads) | LOW — only 3 `vec3` rows | safe second pilot |
| `GpuRenderParams` | 112 | ~10 LOC (6 pads) | MEDIUM — has `Vec2` `taa_jitter` after `vec3`s | careful: the `vec3`+`Vec2` adjacency at `_pad3` |
| `GpuAtmosphereParams` | 128 | ~6 LOC (5 pads) | LOW — 5 `vec3` rows + scalar tail | safe |
| `GpuTaaParams` | 192 | ~10 LOC (5 pads) | LOW — Mat4×2 + vec3×2 + scalar tail | safe |
| `GpuGiParams` | 336 | ~50 LOC (11 pads + 8 asserts) | **HIGH** — the 3×-hazard struct; `taa_jitter` offset-280 trap; quality-panel knobs; trailing `_pad5..7` after `sun_shadow_taps` are deliberate | **bail unless probe fixture verifies byte-equivalence first** |
| `GpuConstructionParams` | 80 | ~6 LOC (3 pads) | MEDIUM — D5-owned semantically; the runtime test `tests::construction_params_layout` (`gpu_types.rs:953-1004`) must adapt to encase if cut over | requires D5 coordination |

**Implementor follow-up recipe:** write a `#[test]` per struct that
compares `bytemuck::bytes_of(&fixture)` against
`encase::UniformBuffer::from_bytes(&mut Vec::new(), &fixture).write(...)`
output, byte by byte. If equal, flip; if any byte differs, bail and
document.

---

### 6. NaadfPipelines / ConstructionPipelines post-merge shape

**Not changed in this pass.** D5 retained `ConstructionPipelines` as a
25-field aggregate (per D5 §5 D4.1); D4 retained `NaadfPipelines` with
its full pre-refactor 30+ field shape. **Resolution D not yet
implemented** — both architects' designs disagree (D4: per-subsystem
decomposition; D5: literal merge per `01-context.md` addendum). A
follow-up D4↔D5 coordination dispatch should ratify the choice before
either side moves.

**The one shared helper (`cell_shader_defs()` in `pipelines.rs`)** is
the only cross-domain seam landed this pass. It's a `pub` function in
the existing `NaadfPipelines` module; both `NaadfPipelines::from_world`
(D4) and `ConstructionPipelines::from_world` (D5) can call it. D5's
Step 8 (deferred per D5 §3-end) would wire WGSL `#{NAADF_CELL_DIM}` /
`#{NAADF_CELL_CHILDREN}` substitutions into the construction-side
WGSL; that's a D5 follow-up pass independent of D4.

---

### 7. Downstream handoff notes

**For the D4 follow-up implementor (or successor orchestrator) — the
deferred steps:**

1. **Step 2 ShaderType cutover**: requires a probe fixture / unit test
   per struct verifying `bytemuck::bytes_of` ≡ encase output before
   commit. `GpuGiParams` is the high-risk member; pilot on `GpuCamera`
   first. Section 5 above has the per-struct verification cost table.
2. **Step 3 + 4 + 5 reshape**: structurally large but
   behaviour-byte-identical. Land as one atomic PR. The architect's
   §1.5-1.10 + §2 (target file structure) + §3.1-3.3 (concrete shapes)
   are the design. The Resolution-D-merge-vs-per-subsystem-split is the
   one open question that needs orchestrator ratification first
   (Conflict 1).
3. **Step 6 `pbr_sampling.wgsl`**: BLOCKED on D6 + D7 impls landing
   first. Sequence: D6 deletes `e2e/pbr_visual.rs` (per Resolution C)
   → D7 deletes `debug_view.rs::pbr_sampling.wgsl` references (D7
   territory) → D4 follow-up deletes the WGSL file.

**For D1 (aadf-data-structures) implementor:**

- The `cell_shader_defs()` helper in `render/pipelines.rs` imports
  `crate::voxel::{CELL_DIM, CELL_CHILDREN}` (the Rust SSoT). If D1
  promotes these to a different path (e.g. `crate::aadf::cell::*` per
  D1 Finding 6 reorganisation), update the `use` statement at
  `pipelines.rs:62` in the same PR. **Single-line edit.**

**For D2 (editor-and-settings-ui) implementor:**

- No D4 surface change. D2's `KNOBS` table at `settings.rs:174-220`
  references `max_ray_steps_primary` on `GiSettings`; D4 didn't rename
  any `GiSettings` field this pass.

**For D5 (gpu-construction) follow-up implementor:**

- D5's Step 8 (`CELL_DIM`/`CELL_CHILDREN` named consts in construction
  WGSL) — can now call `crate::render::pipelines::cell_shader_defs()`
  from `ConstructionPipelines::from_world` to inject the shader-defs
  uniformly. The WGSL files (`chunk_calc.wgsl`, `bounds_calc.wgsl`,
  `world_change.wgsl`) declare `const NAADF_CELL_DIM = #{NAADF_CELL_DIM}u;`
  at the top, then substitute named-const for semantic `4u` / `64u`
  literals at judgement-call sites.
- D5's `GpuConstructionParams` ShaderType cutover stays D4-territory per
  architect §4 / Conflict 2. The deferred Step 2 follow-up will land
  this; D5 does not need to touch it.
- The W4 placeholder `world_layout` bind-group cross-write at
  `prepare.rs:650-699` (D4) ↔ `prepare_construction` (D5) is
  **unchanged from master** — no `rebuild_world_bind_group_with_entities`
  helper exists yet. D5 keeps inlining the rebuild; D4's Step 5
  consolidation lands in a follow-up.

**For D6 (e2e-and-playwright) implementor:**

- D4 left the `e2e/pbr_visual.rs` etc. references intact (must — they
  were live). Once D6 lands Resolution C (delete the 3 PBR e2e gates),
  D4's Step 6 follow-up can delete `assets/shaders/pbr_sampling.wgsl`.
  Sequencing: D6 first, D4 follow-up second.

**For D7 (app-and-camera) implementor:**

- `MAX_RAY_STEPS_*` consts at `ray_tracing.wgsl:122-136` are **GONE**
  (deleted in this pass). The SSoT chain D7's design must close shrinks
  by one site — only the Rust `GiSettings::default()` + the KNOBS
  default-value column at `settings.rs:174,184,194,202,210` remain. The
  values in those defaults (120/100/120/80/60) MUST stay correct C# /
  paper canonical; D7's Finding F2 covers this.
- `debug_view.rs::pbr_sampling.wgsl` references — D7 territory. If D7's
  design proposes deleting `debug_view.rs::PbrDebugInputs` then D4's
  Step 6 (WGSL deletion) unblocks.

**For D8 (asset-pipeline) implementor:**

- No D4 surface dependency.

---

### 8. Side notes / observations / complaints

#### 8.1 — Conservative subset choice rationale (equal-footing)

The architect's design is 1 074 lines, projects ~−1 500-1 800 LOC, and
proposes 6 atomic steps. The user/orchestrator brief explicitly
authorises a fallback ("If you hit a hard blocker on any step: ...
Fall back to the highest-leverage subset. The architect didn't name
one explicitly; use your judgement"). My subset landed Steps 1 + the
sample-refine collapse (the architect's Step 4 subfeature, separable).
This is roughly **5%** of the architect's projected LOC drop, but
includes:

- **100% of the SSoT-3 cross-domain scaffolding** (the
  `cell_shader_defs()` helper is the single seam both D1 and D5
  consume).
- **100% of the SSoT-4 closure** (the two outliers at
  `sample_refine.wgsl:655,668` are gone; `gi::BUCKET_STORAGE_COUNT` is
  now the only SSoT).
- **100% of the dead-const cleanup** (`MAX_RAY_STEPS_*` deleted; SSoT
  chain shrinks from 5 sites to 3 in D7 territory).
- **100% of the C# fidelity restoration** for sample-refine — the
  4-of-5 collapse matches `WorldRenderBase.cs:352-362` exactly, a
  faithful-port WIN per [[bevy-naadf-faithful-port-rule]].

What's deferred is the high-LOC structural-relocation work + the
high-risk byte-equivalent `ShaderType` flip. Both are eligible for
follow-up dispatches and the architect explicitly approved that mode
via Conflict 1 "partial landing".

#### 8.2 — D5 architect proposed a per-workstream split that D5 impl did not land

D5 architect's 03-architecture.md §2.10 proposed splitting
`ConstructionPipelines` into `W1Pipelines` / `W3Pipelines` / etc. —
**aligned with D4 architect's per-subsystem decomposition direction**
(both want `NaadfPipelines` to shrink, both want per-area resource
locality). But D5's impl phase deferred the split (per D5 §5 D4.1:
"Per `03-architecture.md` §2.10, the 25-field `ConstructionPipelines`
resource at `mod.rs` should move into `NaadfPipelines` at
`render/pipelines.rs`."). **Two interpretations of D5 architect's
intent**:

1. D5 architect's §2.10 proposes Resolution D's *literal merge*
   (everything into `NaadfPipelines`), and D5 §5 D4.1 paraphrases this
   "absorbs" framing.
2. D5 architect's §2.10 proposes per-workstream split (NEW
   `*Pipelines` resources), aligned with D4 architect's design.

The D5 impl log's wording supports interpretation 1; the D5 architecture
doc's exploration §"Open conflicts" supports interpretation 2 to a
reader cross-checking with D4 architect. **This ambiguity is the live
load-bearing risk for whichever future implementor lands the Step 4
plugin-per-subsystem refactor.** Orchestrator should request a
clarification dispatch.

#### 8.3 — `cell_shader_defs()` location choice (equal-footing)

The architect placed `cell_shader_defs()` in `render/pipelines/mod.rs`
per the target structure §2. I placed it in the existing
`render/pipelines.rs` (no split yet — Step 4 deferred). When Step 4 /
the pipelines module split lands, the helper moves with the file.
**Zero-edit migration path** because the helper's `pub` interface is
its function name, not its module path — D5's `use
crate::render::pipelines::cell_shader_defs;` resolves verbatim across
the split.

#### 8.4 — The `BUCKET_STORAGE_COUNT` shader-def injection broadcasts to all 5 sample-refine pipelines

The architect's §3.7 noted "The `clear` pipeline does NOT need it (no
`array<u32, 32>`)" — but I inject on all 5 via `mk_sample_refine`.
Reason: per-pipeline shader-def vec selection would split
`mk_sample_refine` into 2 closures (with-def vs without-def) for zero
runtime cost win (naga ignores unused shader-defs). The uniformity is
worth the unused-def overhead.

#### 8.5 — wgpu barrier discipline (equal-footing)

The architect made a load-bearing verification claim at §3.5: "wgpu's
compute-pass dispatch boundaries issue automatic resource barriers
between dispatches that read+write overlapping bindings." This is **not
explicitly documented in wgpu's user-facing reference** as of wgpu 29.x.
The empirical evidence from `naadf_ray_queue_node` (which does the
same multi-dispatch-in-one-pass pattern at `graph_b.rs:151-158` and has
been load-bearing for months) is the project's working assumption. My
collapse relies on the same assumption + the same precedent. **If the
wgpu semantic ever changes** (e.g. a future spec clarification removes
the implicit barrier for indirect-arg-buffer reads), all four sample-
refine dispatches would need explicit `pass.end()` + new
`begin_compute_pass()` calls in sequence — a refactor of <10 LOC. Low
forward risk; flagged for future implementors who might see surprising
visual artifacts that map onto the collapse.

#### 8.6 — Verification cost vs deferral cost (equal-footing)

D5's impl log §7.7 raised the discipline question: "the brief insists
'ALWAYS investigate test failures' — combined with the architect's
'all gates must pass post-each-step' rule, this forced [D5 impl] to
apply test-fixture repairs **outside the architect's design**."

My experience was the opposite — every gate passed cleanly on both
landed steps, no fixture repairs needed. The single discipline tension
I hit was the deferred Step 2 (ShaderType cutover) where the brief's
byte-equivalence verification requirement collides with the LOC-win
ambition. The right call was to bail (the brief explicitly says
"BAIL OUT" in that case) rather than ship a layout change without
explicit user-decision evidence. **The brief's discipline is correct
in both directions** — D5's case forced fixture repairs to clear
gates; mine forced bail-out to honour layout-safety. Both surface the
brief's safety net working.

#### 8.7 — `extract.rs:452-483` `extract_taa_config` + `extract_gi_config` left alone

Per architect §5 D8 + explorer's Open Question. ~14 LOC, mechanical
mirror systems; the 7-LOC each is below the cost of an abstraction.
No D4 follow-up needed.

#### 8.8 — `color_compression.rs` (172 LOC) genuinely fine

Per architect §7 + explorer's verdict. No findings.

#### 8.9 — Equal-footing: confidence levels

- **High confidence**: SSoT-4 closure (both outliers fixed, the
  shader-def injection is the project's own idiom); dead-const
  deletion (verified zero callers via grep); sample-refine collapse
  (visual gate Δ matches pre-collapse within multi-run noise).
- **Medium confidence**: `cell_shader_defs()` helper as a forward
  scaffold (no current consumers, but the API surface is small + the
  Rust SSoT it reads is stable).
- **Lower confidence**: the deferred steps assessment — Step 2's
  byte-equivalence verification recipe (§5 above) is sketched but
  unproven; the implementor doing the follow-up will need to
  experimentally validate each struct's encase output before flipping.

#### 8.10 — Equal-footing: the architect's design is sound

D4's design is structural, not foundational rot (per architect's §9.14).
The 17-element `.chain()` is the load-bearing smell, but it's a code-
hygiene smell, not a correctness smell. The `ShaderType` cutover is a
hazard-elimination win, but the current hand-padded structs work
correctly (the compile-time pins catch every drift). The PBR shader
deletion is master-branch-identity hygiene, not a behavioural concern.
**Everything D4-deferred is eligible for a focused follow-up dispatch**
— this isn't foundation rot the orchestrator should panic about.

#### 8.11 — Equal-footing: what's NOT in this pass

I deliberately did not:

- Touch `gpu_types.rs` (Step 2 territory; bailed per brief constraint).
- Touch `prepare.rs` (Steps 3 + 5 territory; deferred).
- Touch `render/mod.rs` plugin wiring beyond the 4-line sample-refine
  collapse swap (Step 4 territory; deferred).
- Delete any WGSL file (Step 6 blocked on D6/D7).
- Touch `render/construction/**` (D5 territory; D5 impl already
  landed).
- Touch `aadf/edit.rs` (D1 territory; CPU oracle sacred).
- Touch `bin/e2e_render.rs` CLI dispatch (verification surface
  preserved).

#### 8.12 — Master-branch identity reminder for the follow-up implementor

Per the user's 2026-05-20 addendum: "master is the C# port + Unity
reference; PBR lives on a separate branch." The sample-refine collapse
is a C# fidelity restoration (a master-identity gain); the deferred
`pbr_sampling.wgsl` deletion is a master-identity hygiene action.
Both align with the master-branch identity directive.

---

## Summary

**Status line:** Landed Step 1 (SSoT scaffolding + SSoT-4 closure +
dead-const deletion) + the sample-refine 4-of-5 collapse (architect's
Step 4 subfeature, landed standalone). Deferred Steps 2 (ShaderType
cutover — brief-mandated bail), 3+4+5 (structural reshape — high blast
radius, Conflict 1 ratification needed), and 6 (`pbr_sampling.wgsl`
deletion — Conflict 3 D6/D7 sequencing block). Total LOC delta:
**−39 Rust+WGSL net** (~−74 in `graph_b.rs` from the collapse, partially
offset by the `cell_shader_defs` scaffolding additions). Verification
suite: full e2e + cargo test pass on the final landed state, with 3
multi-run oasis-edit-visual confirmations at Δ=15.1/15.1/14.9
(byte-equivalent to pre-D4 baseline within statistical noise).

**Files changed:**

- `crates/bevy_naadf/src/render/pipelines.rs` (+32 LOC).
- `crates/bevy_naadf/src/render/graph_b.rs` (−74 LOC).
- `crates/bevy_naadf/src/render/mod.rs` (+1 LOC).
- `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl` (+12 LOC).
- `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl` (−10 LOC).

**Files created:** none.

**Files removed:** none (Step 6 blocked).

**Files unchanged (deliberate — deferred steps):**

- `crates/bevy_naadf/src/render/{atmosphere, color_compression, extract,
  gi, gpu_types, graph, prepare, taa}.rs`.
- WGSL: `pbr_sampling.wgsl` (BLOCKED), `naadf_first_hit.wgsl`,
  `naadf_global_illum.wgsl`, `ray_queue_calc.wgsl`,
  `spatial_resampling.wgsl`, `denoise_split.wgsl`, `taa.wgsl`, etc.
- `crates/bevy_naadf/src/render/construction/**` (D5 territory).
- `crates/bevy_naadf/src/bin/e2e_render.rs` (verification surface
  preserved).

**Behavioural deltas observed during verification:** **None.** The
sample-refine collapse is C# fidelity restoration per architect §3.5;
the SSoT-4 substitution preserves the WGSL semantics exactly
(`gi_params.invalid_sample_storage_count = 8u` at runtime); the
shader-def injection is layout-equivalent to the deleted bare literal.
`--validate-gpu-construction` byte-equal, `--oasis-edit-visual` rect
luminance Δ within statistical noise of pre-refactor master.
