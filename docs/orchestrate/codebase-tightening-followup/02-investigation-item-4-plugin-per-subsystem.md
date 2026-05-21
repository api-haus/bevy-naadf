# Item 4 — D4 Step 5: plugin-per-subsystem

Read-only investigation. No code edits performed.
Verified at HEAD `2bb03d1`. All file:line references re-checked with Read/Grep.

---

## Bailing implementor's stated blocker

Two separate bailouts on the same item — the D4 follow-up dispatch
(2026-05-20) and the D4 final-cleanup dispatch (2026-05-20/21). Verbatim
quotes follow with file:line cites.

### Bailout 1 — D4 follow-up

`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1081-1121`:

> **Step 5 brief / Step 4 architect — plugin-per-subsystem extraction (the
> big one) — DEFERRED**
>
> **Reason for deferral (both):**
>
> These are the large-blast-radius structural-relocation steps. The
> architect projects:
>
> - Step 3/Step 4 brief: split 1207 LOC across 3 new files; absorb
>   `apply_voxel_types_refresh` extraction; verify zero D5 import-path
>   changes (architect §6 side-note 6).
> - Step 4/Step 5 brief: dissolve `graph.rs` (309 LOC) + `graph_b.rs` (500
>   LOC post-D4-main); create 6 new subsystem files (`first_hit.rs`,
>   `ray_queue.rs`, `sample_refine.rs`, `spatial_resampling.rs`,
>   `denoise.rs`, `final_blit.rs`); split `pipelines.rs` (940 LOC
>   pre-Resolution-D, 1170 LOC post-merge) into `pipelines/{mod,shaders}.rs`
>   + per-subsystem `*Pipelines` resources; convert the 17-element
>   `.chain()` into a `SystemSet`-edge web with 9 plugins each declaring
>   `.before(...)/.after(...)` edges; rewrite `NaadfRenderPlugin::build`
>   from 17-element chain to 11-element plugin tuple.
>
> **Per the dispatch brief's bailout permission** ("If you hit a Step that
> requires more than 100 tool uses to land coherently, then bail out
> cleanly"): combined, these two steps would land 6-8 new files + edit 8
> existing files + require a full e2e suite per intermediate state to bound
> the multi-step risk. Even individually they push past the 100-tool-use
> bound.
> [...]
> **Conflict 1 from architect's §6 is now PARTIALLY-RESOLVED:** Resolution
> D's literal merge is landed (flat merge into `NaadfPipelines`). D4's per-
> subsystem `*Pipelines` decomposition (architect's §1.10 alternate path)
> is implicitly **superseded** by the merge — a future plugin-per-subsystem
> extraction would split `NaadfPipelines`'s 57-field shape into 9
> `*Pipelines` resources, not 2-into-9.

### Bailout 2 — D4 final cleanup

`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1585-1619`:

> **Item 2 / D4 Step 5 brief / Step 4 architect — plugin-per-subsystem
> extraction — DEFERRED**
>
> **Reason for deferral:** the architect's §3.3 + Step 4 / brief's Step 5
> spec is genuinely large:
> - 6+ new files (`first_hit.rs`, `final_blit.rs`, `ray_queue.rs`,
>   `sample_refine.rs`, `spatial_resampling.rs`, `denoise.rs`) + body
>   absorption into 3 existing files (`atmosphere.rs`, `taa.rs`, `gi.rs`);
> - dissolution of `graph.rs` (309 LOC) + `graph_b.rs` (500 LOC
>   post-D4-main);
> - decomposition of `NaadfPipelines` from 57-field shape into ~9
>   per-subsystem `*Pipelines` resources (OR per architect Conflict 1, kept
>   as monolith with per-subsystem plugins reading from it — the partial
>   landing option);
> - replacement of `render/mod.rs:298-330` 17-element `.chain()` with
>   `.add_plugins((…))` over ~11 subsystem `Plugin`s, each declaring its
>   own `SystemSet` label + `.before(…)/.after(…)` edges to its neighbours.
>
> This dispatch's effective tool budget was ~30% consumed by required
> reading + Items 1/3/4 + their verification passes (the multi-run e2e
> discipline). The remaining budget cannot land plugin-per-subsystem
> coherently — even the partial landing (architect Conflict 1's
> "plugin-per-subsystem but reading from existing `NaadfPipelines`") needs
> the 6 new files + the chain rewire + intermediate e2e gates between each
> subsystem's extraction. The D4 follow-up implementor's §4.5 conclusion is
> the same: "A future dispatch focused EXCLUSIVELY on the
> plugin-per-subsystem extraction is the right shape — single
> architecturally-coherent PR, single set of e2e gates to run, no other
> in-flight changes to confound with."

Both bailouts are explicit about the framing: budget. Neither cites an
analytical blocker. Both surface the Resolution-D supersession concern as a
**side effect**, not a blocker.

---

## Verification of the claim

### Step inventory (verified against architect §3.3 + Step 4)

Architect spec at `docs/orchestrate/codebase-tightening/render-pipeline/03-architecture.md:728-751`
(Step 4 — plugin-per-subsystem extraction). The work breaks down as:

**A. New files (6 — currently nonexistent — verified via `ls` listing
absent: `first_hit.rs`, `final_blit.rs`, `ray_queue.rs`, `sample_refine.rs`,
`spatial_resampling.rs`, `denoise.rs`):**

For each new file, the architect template at
`03-architecture.md:258-300` requires (per file):

- Module docblock (cross-domain SystemSet contract + dispatch position
  rationale).
- A `use` import block (≈5-8 imports — `bevy::prelude::*`,
  `bevy::render::{Render, RenderApp, RenderSystems}`,
  `Core3dSystems`, `Core3d` schedule, `tonemapping`, the subsystem's
  shader/pipeline path).
- `pub const FOO_SPAN: &str = "…"` (moved from `graph.rs` or `graph_b.rs`).
- `#[derive(Resource)] pub struct FooPipelines { … }` — pipeline-id +
  layout fields specific to this subsystem (post-merge, these fields
  currently live as the corresponding subset on `NaadfPipelines`).
- `#[derive(SystemSet, …)] pub struct FooSet;`
- `pub fn naadf_foo_node(…) { … }` — body lifted verbatim from
  `graph.rs` / `graph_b.rs` (no behavioural change).
- `pub struct FooPlugin; impl Plugin for FooPlugin { fn build(…) { … } }`
  — register the resource, register the node system with the four
  ordering edges (`.in_set(FooSet)`, `.in_set(Core3dSystems::PostProcess)`,
  `.before(tonemapping)`, `.after(<predecessor>::PredecessorSet)`).

**B. Absorptions into 3 existing files (`atmosphere.rs`, `gi.rs`, `taa.rs`):**

These already host their own `prepare_*` system + GPU resource (verified at
`atmosphere.rs:234` `AtmosphereGpu`, `:262` `prepare_atmosphere`; `gi.rs:108`
`GiGpu`, `:224` `prepare_gi`; `taa.rs:235` `TaaGpu`, `:286` `prepare_taa`).
Each absorbs the corresponding node body from `graph.rs` / `graph_b.rs`
plus the same `*Pipelines` resource + `*Set` + `*Plugin` triad. The
absorptions are smaller than the new-file work — the file scaffolding
(use block, docblock) is already there.

**C. `pipelines.rs` decomposition:**

Either (i) split into `pipelines/{mod,shaders}.rs` + per-subsystem
`*Pipelines` resources distributed across subsystem files (architect's
preferred) — verified target shape at `03-architecture.md:78-87,
:740-741`; OR (ii) Conflict-1 partial landing where `NaadfPipelines` stays
intact and each plugin reads the relevant fields off it. The latter is
~10× lower effort but produces a non-decomposed pipeline state.

**D. Dissolution of `graph.rs` (verified 309 LOC) + `graph_b.rs` (verified
500 LOC; impl-log statement of "500 LOC post-D4-main" matches HEAD):**

DELETE both files. All ~14 nodes' bodies move into A/B above. Net deletion
= 809 LOC textual remove + ≈10 LOC of duplicated header-block prose.

**E. Rewrite `render/mod.rs:298-331` 17-element `.chain()`:**

Becomes `.add_plugins((…))` over 11 plugins (architect §3.3 table). The
17-tuple is verified at `render/mod.rs:298-331`; the ordering-rationale
docblock at `:194-297` (a ~104-LOC docblock) is to be deleted per
architect Step 4 last bullet ("the new edges live in each Plugin's body
where the reader looks first").

### Tool-call estimate per subsystem

A single new-file subsystem extraction (the smallest path — pick
`ray_queue.rs`, ~80 LOC target per architect §2 file budget) requires:

| step | tool calls | rationale |
|---|---|---|
| Read source node body in `graph_b.rs:75-130` (the `naadf_ray_queue_node` range) | 1 Read | precise body extraction needs the actual signature + use-imports |
| Read corresponding pipeline fields on `NaadfPipelines` (slice of `pipelines.rs:286-…`) | 1 Read | identify what `*Pipelines` will own |
| Read predecessor's `*Set` import path (sample_refine clear, when it lands) | 1 Read (or 0 if already known) | for the `.after(…)` edge |
| Write the new `ray_queue.rs` file | 1 Write | one shot, ≈80 LOC |
| Edit `render/mod.rs` to drop the `naadf_ray_queue_node` from the 17-tuple and add `RayQueuePlugin` to the `add_plugins` tuple | 1 Edit (or 2 if old+new edits are separate) | one removal, one addition |
| Edit `pipelines.rs` to remove the ray-queue fields from `NaadfPipelines` and its `from_world` (architect's full-decomposition path) OR skip if partial-landing | 1-2 Edits | the load-bearing variable |
| `cargo build --workspace` | 1 Bash | verify it compiles |
| `cargo test --workspace --lib` (only if a unit test touches it) | 1 Bash | quick gate |
| `cargo run --bin e2e_render -- --vox-e2e` + `--validate-gpu-construction` (cheap deterministic gates first; only run `--oasis-edit-visual` ×3 if those pass) | 2-3 Bash | per-intermediate verification |

Per-subsystem floor: **~10-12 tool calls** assuming everything goes
right on first compile. Per-subsystem realistic (one
compile-error-and-recover cycle, which always happens with `pub`/`pub(crate)`
visibility splits and `use`-path renames): **~15-18 tool calls.**

### Total project estimate

Architect spec has 9 subsystems (per the §3.3 plugin table — 5 new files
in `*ray_queue, sample_refine, spatial_resampling, denoise, final_blit` +
`first_hit`, and 3 absorptions into existing `atmosphere, gi, taa`):

- 6 new files × ~15 tool calls = **~90 tool calls** (just the per-file
  scaffolding + per-subsystem intermediate verification).
- 3 absorptions × ~10 tool calls = **~30 tool calls**.
- `pipelines.rs` decomposition (the hard part — slicing a 57-field
  resource into 9 sub-resources and updating every consumer): **~25-40
  tool calls** if full decomposition; **~5 tool calls** if partial-landing
  (read-from-existing).
- `render/mod.rs` rewrite (17-tuple → 11-plugin tuple, docblock deletion,
  use-block cleanup): **~5-8 tool calls**.
- DELETE `graph.rs` + `graph_b.rs` after all nodes are relocated:
  **~2 tool calls** (one Bash rm each, plus update of `render/mod.rs`'s
  `pub mod graph; pub mod graph_b;` declarations).
- Final full-suite verification (8 e2e gates including `--oasis-edit-visual`
  ×3, `--vox-gpu-oracle` ×3 if applicable, full lib tests): **~12-15
  tool calls**.

Full-decomposition path: **~160-180 tool calls.**
Partial-landing path (Conflict-1, read from `NaadfPipelines`): **~110-130
tool calls.**

Both exceed the 100-tool-use bailout permission. The partial landing is
just barely over; the full decomposition is well over.

### Is the budget claim plausible? Yes.

The estimates above assume **every step lands on first compile** and
**every intermediate e2e gate passes on first run**. Neither holds in
practice. Real-world friction multipliers:

1. **Visibility errors:** the prior implementor hit one on Item 1
   (`pub use rebuild_world_bind_group_with_entities` on a `pub(crate)`
   item; verified at `04-refactoring.md:1626-1629`). Plugin-per-subsystem
   has ~30+ visibility-decision boundaries (every moved const, every
   moved struct, every moved field on the decomposed `*Pipelines`).
   Expect 2-5 visibility-fix cycles.
2. **SystemSet edge-ordering off-by-one:** the 17-tuple's order is
   load-bearing (`render/mod.rs:194-297` docblock spells out the WHY for
   each node's slot). A misplaced `.after(...)` edge produces a behaviour
   regression that only `--oasis-edit-visual` / `--vox-gpu-oracle`
   surfaces (non-deterministic gates → ≥3 runs each per
   `feedback-multiple-runs-rule-out-false-positives`). Each false negative
   is a ~5-tool-call diagnostic + fix cycle.
3. **`from_world` decomposition cascade:** `NaadfPipelines::from_world`
   (currently at `pipelines.rs:350+`) builds all layouts + pipelines in
   one body. Splitting it across 9 `*Pipelines::from_world`s while
   preserving the shared `taa_ring_depth` shader-def, the
   `pipeline_cache.get_bind_group_layout(...)` calls, and the
   inter-pipeline shared layouts (`world_layout`, `frame_layout`,
   `empty_layout`) is non-mechanical surgery — not a rename.

**Hidden analytical issues found?** None I can identify *that would
block* the refactor. The Resolution-D merge changes *which* path is
recommended (see next section), but neither path is blocked by an
unspoken analytical gap. The bailouts read as honest dispatch-arithmetic
acknowledgements, not framing covers for hidden architectural blockers.

---

## Resolution-D-merge supersession check

### What the architect spec assumed

`03-architecture.md:48-49` (architect §1.10):

> **W0 retirement coordination (Resolution D).** Post-D5,
> `ConstructionPipelines` either folds into `NaadfPipelines` (D5's call)
> or stays split. The design here assumes **D5 keeps
> `ConstructionPipelines` per-workstream-split** (D5 Finding 10's
> proposal) and `NaadfPipelines` does **NOT** absorb construction-side
> fields. D4's `NaadfPipelines` decomposition is the natural mirror: each
> render-side subsystem gets its own `*Pipelines` resource.
> `NaadfPipelines` shrinks to ~5 fields holding only the cross-subsystem
> core (`world_layout`, `frame_layout`, `blit_layout`, `empty_layout`,
> `blit_pipelines: HashMap<TextureFormat, _>`, `blit_vertex`,
> `blit_shader`).

Architect §6 Conflict 1 (`:892-911`) anticipated the alternative path
("if D5 architect lands the literal `ConstructionPipelines` →
`NaadfPipelines` merge ... D4 then keeps the existing 30+ field
`NaadfPipelines` shape, only thinning the `from_world` body through helper
extraction; the per-subsystem `*Pipelines` resources do not happen. The
plugin-per-subsystem refactor still lands ... but each plugin reaches
into `Res<NaadfPipelines>` for its pipeline ids instead of
`Res<FirstHitPipelines>`/etc.").

### What Resolution D actually landed

Verified at `crates/bevy_naadf/src/render/pipelines.rs:285-347`:

`NaadfPipelines` now holds **all** former `ConstructionPipelines` fields
(`generator_model_layout`, `generator_model_pipeline`,
`construction_world_layout`, `chunk_calc_pipeline_calc_block`,
`chunk_calc_pipeline_voxel_bounds`, `chunk_calc_pipeline_block_bounds`,
`map_copy_layout`, `map_copy_pipeline_copy`, `map_copy_pipeline_test`,
`construction_bounds_world_layout`, `construction_bounds_layout`,
`bound_dispatch_indirect_layout`, `bounds_calc_pipeline_add_initial`,
`bounds_calc_pipeline_prepare`, `bounds_calc_pipeline_compute`,
`entity_world_layout`, `construction_entity_layout`,
`entity_update_pipeline_update_chunks`,
`entity_update_pipeline_copy_entity_chunk_instances`,
`entity_update_pipeline_copy_entity_history`,
`construction_change_layout`, `world_change_pipeline_apply_group_change`,
`world_change_pipeline_apply_chunk_change`,
`world_change_pipeline_apply_block_change`,
`world_change_pipeline_apply_voxel_change`).

The merge note in the source comment (`pipelines.rs:285-290`):

> // === Phase-C construction pipelines + layouts (Resolution D — W0 seam retired) ===
> // Folded in from the former `render/construction/mod.rs::ConstructionPipelines`
> // resource per the codebase-tightening D5 architect §2.10 + Resolution D
> // approval. Field names preserved verbatim from the prior struct to
> // minimise consumer-call-site churn; the 5 prepare/node systems that read
> // them now access them off the merged `NaadfPipelines` instead.

Plus the D4 follow-up impl log confirms the structural endpoint at
`04-refactoring.md:1115-1119`:

> Resolution D's literal merge is landed (flat merge into `NaadfPipelines`).
> D4's per-subsystem `*Pipelines` decomposition (architect's §1.10
> alternate path) is implicitly **superseded** by the merge — a future
> plugin-per-subsystem extraction would split `NaadfPipelines`'s 57-field
> shape into 9 `*Pipelines` resources, not 2-into-9.

### Does the architect spec still apply?

**Partially. The `SystemSet` web + `Plugin` shape survives the merge
intact; the `*Pipelines` decomposition does not.**

- **Survives:** the per-subsystem `Plugin` + `SystemSet` + `*Set::after(predecessor::*Set)`
  edge web (`03-architecture.md:303-329`). This is orthogonal to pipeline
  resource ownership. Architect's Conflict-1 partial-landing path
  (`:907-911`) explicitly carves this out as the standalone-deliverable.
- **Superseded:** the per-subsystem `*Pipelines` decomposition (§1.10,
  §3.3 plugin template `FirstHitPipelines`/`RayQueuePipelines`/etc., §3.6
  `cell_shader_defs()` re-organisation, the file-tree shape in §2 that
  shows `pipelines/{mod,shaders}.rs` + per-subsystem `*Pipelines` files).
  The architect was writing for a world where `NaadfPipelines` had ~7
  fields; the actual world has 57. Decomposing 57-into-9 is a different
  surgery (more cuts, more cross-cut consumer call-sites in D5's
  prepare_construction sub-modules) than the spec authored.
- **New unanswered question:** if `NaadfPipelines` is decomposed
  9-ways, does *each construction-side workstream* (W1/W2/W3/W4/W5) get
  its own `*Pipelines` too? The architect's §3.3 plugin table
  (`:307-322`) lists 4 construction sets (`construction::GpuProducerSet`,
  `BoundsCalcSet`, `WorldChangeSet`, `EntityUpdateSet`) as "(D5-owned)".
  But under the merge, those pipelines live on `NaadfPipelines` — so
  D4's decomposition either has to *include* the construction side
  (cross-domain edit) or *exclude* it (leaving `NaadfPipelines` as a
  hybrid: thin core + 25 construction fields). Neither is in the spec.

### The conflict walk

The architect-spec path:

1. D5 splits `ConstructionPipelines` per-workstream.
2. D4 splits the resulting thin `NaadfPipelines` per-subsystem.
3. Each `Plugin` owns its `*Pipelines` + `*Set` + node + body.

The actual-landed path:

1. D5 merged `ConstructionPipelines` into `NaadfPipelines` (verified).
2. D4 now must either:
   - (a) **Decompose the merged 57-field `NaadfPipelines` 9-ways
     (or 13-ways with the 4 construction subsystems).** This is the
     biggest scope possible; it cross-cuts D5's `prepare_construction`
     and its sub-modules. Architect spec does not describe this surgery.
   - (b) **Partial-landing per Conflict 1: leave `NaadfPipelines`
     intact; each plugin reads what it needs off `Res<NaadfPipelines>`.**
     This is the smallest scope; the SystemSet web + Plugin scaffold
     lands without touching pipeline ownership.
   - (c) **Re-split `NaadfPipelines` back into render-side vs
     construction-side, leaving D5's side as one resource and D4's side
     as one decomposed family.** This is the "redo the W0-seam retirement
     halfway" path — re-introducing some structural cost the merge just
     paid down.

The architect's spec maps cleanly to none of (a/b/c). The spec was
internally consistent under its assumed precondition (`NaadfPipelines` is
already thin); under the actual precondition it is silent on the
specifically-load-bearing question.

---

## Diagnosis

**Mixed: budget-genuinely-tight + already-superseded-by-Resolution-D.**

- **Budget is genuine** — the tool-call inventory (≈110-180 calls
  depending on partial-vs-full) exceeds the 100-call dispatch ceiling on
  both readings of the spec, even before friction multipliers. The two
  bailouts are honest. There is no hidden analytical blocker masquerading
  as a budget concern; the spec is mechanical, just bulky.

- **The spec is partially obsolete.** The plugin/SystemSet/node-extraction
  half of the spec (§3.3, §3.5 sample-refine collapse, the file-shape
  prose in §2) carries forward intact. The pipeline-decomposition half
  (§1.10, §3.4-related, §3.6 cross-domain helper placement, the file-tree
  diagram around `pipelines/{mod,shaders}.rs` + per-subsystem
  `*Pipelines` files) is silent on the load-bearing question post-merge:
  *how* a 57-field merged resource should split, and *whether* the
  construction-side fields go with it.

- **Resolution D was the user-approved structural endpoint** per
  `04-refactoring.md:1208-1217` ("D5 architect's reading wins because
  it's a closer match to the user's verbatim Q&A approval"). The
  architect spec's preferred path (per-workstream split + per-subsystem
  split) is no longer the agreed direction; the architect needs to revise
  for the post-merge world before another implementor walks in.

- **Re-dispatching the same brief would loop.** Memory:
  `feedback-implementor-private-shared-notes-and-dual-angle` —
  re-dispatching with the same architect spec produces the same outcome.
  Two Opus sessions already converged on "this is too big for one
  dispatch, and the pipeline-decomposition piece is now ambiguous." A
  third would produce the same answer.

---

## Proposed path forward

**Pick (a) — fresh `delegate-architect` dispatch to revise the spec for
the post-merge world** + **bundle that with the decomposed
re-dispatch shape (d-ish: architect-then-implementor-decomposed-per-subsystem).**

Justification (4 sentences):

1. The Resolution-D merge changed the structural precondition the
   architect designed against; the existing spec is silent on the most
   load-bearing decision (how to decompose 57 fields, whether
   construction-side travels with the split). A fresh architect pass with
   the merged `NaadfPipelines` as input is non-optional before any
   implementor restart — re-dispatching the obsolete spec is the
   anti-pattern the orchestration's "don't re-dispatch the same brief
   that bailed" rule (`01-context.md:99`) was written to prevent.

2. The architect's revised spec should explicitly choose between Conflict-1
   paths (a/b/c above) and decompose Step 4 into per-subsystem
   sub-orchestrations so a budget-bounded implementor can land them one
   at a time — `first_hit.rs` extraction as the proof-of-concept first
   subsystem (≈15 tool calls), then `final_blit.rs`, `ray_queue.rs`,
   etc., each in its own focused dispatch.

3. Per-subsystem dispatches let the orchestrator gate on the actual
   landed cost of subsystem #1 — if the proof-of-concept lands in
   ≤20 tool calls (as estimated), the remaining 8 subsystems are
   8 × ~15 = ~120 tool calls of mechanical repetition, naturally chunked
   into 4 dispatches of 2 subsystems each.

4. Conflict-1 partial-landing (read from existing `NaadfPipelines`,
   skip the resource-decomposition entirely) is also a legitimate
   architect choice given Resolution D's spirit ("the merge reduces
   cross-cutting concerns; don't immediately un-merge them again");
   the revised architect dispatch should *state* the choice rather than
   leave the implementor to infer it from a §6 escape hatch.

**Not (c) accept-as-is**: the 17-element `.chain()` is still the
load-bearing edit-magnet smell the architect identified at §9.1
(`03-architecture.md:1037`); leaving it unresolved keeps the per-PR
friction. The smell is real; the spec is just obsolete in its specifics.

---

## Verification recipe

Per `01-context.md:73-83` (verification surface + multi-run discipline).
The 17-element `.chain()` at `render/mod.rs:298-331` is the canonical
post-refactor truth: the resolved schedule order must match it node-for-node.

### Build floor

```bash
cd /mnt/archive4/DEV/bevy-naadf
cargo build --workspace
```

### Lib tests

```bash
cargo test --workspace --lib
# Expected: 179 passed (+ 1 ignored).
```

### Deterministic e2e gates (single-run each)

```bash
timeout 120s cargo run --bin e2e_render -- --validate-gpu-construction
timeout 120s cargo run --bin e2e_render -- --vox-e2e
timeout 120s cargo run --bin e2e_render -- --edit-mode
timeout 120s cargo run --bin e2e_render -- --entities
timeout 120s cargo run --bin e2e_render -- --runtime-edit-mode
timeout 120s cargo run --bin e2e_render -- --baseline
timeout 120s cargo run --bin e2e_render -- --vox-gpu-construction
```

Each must print its PASS message. `--validate-gpu-construction` is the
load-bearing signal that node order + bind-group identity survived (388
bytes byte-equal CPU↔GPU).

### Non-deterministic e2e gates (≥3 runs on suspect side, ≥2 on reference)

```bash
# Suspect: post-refactor branch.
for i in 1 2 3; do timeout 120s cargo run --bin e2e_render -- --oasis-edit-visual; done
for i in 1 2 3; do timeout 120s cargo run --bin e2e_render -- --vox-gpu-oracle; done
```

Per `feedback-multiple-runs-rule-out-false-positives`: aggregate variance
across runs; record Δ-luminance per `--oasis-edit-visual` run (baseline at
HEAD = 15.0 / 15.1 / 14.7 per `04-refactoring.md:1654-1657`).

### Plugin-extraction-specific verification

After each per-subsystem extraction (per-subsystem dispatch):

1. `cargo build --workspace` — single compile probe.
2. `cargo run --bin e2e_render -- --vox-e2e` — cheapest deterministic
   smoke test that exercises the full chain through to final blit
   (`lum >= 160` threshold; HEAD reports 250.5).
3. After all 9 subsystems extracted: full deterministic e2e set +
   non-deterministic ≥3-run set above.

### Schedule-order mental walkthrough

Compare the resolved system order against the canonical 17-element
`.chain()` at `render/mod.rs:298-331`:

```
naadf_gpu_producer_node
naadf_bounds_compute_node
naadf_world_change_node
naadf_entity_update_node
naadf_atmosphere_node
naadf_first_hit_node
naadf_taa_reproject_node
naadf_sample_refine_clear_node
naadf_ray_queue_node
naadf_global_illum_node
naadf_sample_refine_continuous_node
naadf_spatial_resampling_node
naadf_denoise_node
naadf_calc_new_taa_sample_node
naadf_final_blit_node
```

(15 nodes after the sample-refine 5→2 collapse landed; the "17" figure
in the bailout language predates the collapse. `render/mod.rs:298-327`
lists 15 nodes today.)

The new `Plugin`s must produce the same order via their
`.after(predecessor::PredecessorSet)` edges. The architect spec table at
`03-architecture.md:307-323` lists the expected edges; the revised
architect's spec should re-state it post-merge.

### Dep-arrow / unused-file check

After dissolution:

```bash
# Both must report zero matches (files DELETED).
ls crates/bevy_naadf/src/render/graph.rs    # expect: no such file
ls crates/bevy_naadf/src/render/graph_b.rs  # expect: no such file
```

```bash
# Confirm zero stale imports.
grep -rn "use crate::render::graph" crates/bevy_naadf/src/
grep -rn "use crate::render::graph_b" crates/bevy_naadf/src/
# Expected: zero matches.
```

---

## Side notes / observations / complaints

1. **The audit's "implement one subsystem as proof-of-concept"
   suggestion is the right shape — but per investigator brief I'm
   forbidden from running it. The estimate above (≈15 tool calls for a
   single subsystem when the architect spec is post-merge-revised) is
   reasoned, not measured. If the actual revised dispatch lands subsystem
   #1 at 25 tool calls instead of 15, the 9-subsystem total revises to
   ~225 — comfortably one dispatch per subsystem with a 100-call budget.

2. **The "17-element" figure in the bailout language is stale.** The
   sample-refine 5→2 collapse landed earlier (verified at
   `render/mod.rs:298-327` — current count is 15 nodes, including the
   collapsed `naadf_sample_refine_continuous_node`). The architect's
   §3.3 table at `03-architecture.md:307-322` was already written for
   the post-collapse 15-node world. Bailout language should be updated
   to "15-element `.chain()`" for clarity in the next dispatch's brief.

3. **`ConstructionPlugin::build`** at
   `render/construction/mod.rs:1827-1913` is the precedent template and
   it's a tight, readable ~85 LOC. The architect's §3.3 plugin template
   (`03-architecture.md:286-300`) is even tighter (~15 LOC of `impl
   Plugin::build` body per subsystem). The 9-plugin scaffold is
   genuinely **light** in *per-file* surface. The aggregate cost is
   real, but no single file is hard to write. This makes
   per-subsystem dispatching attractive: each plugin file is a 2-3
   tool-call write.

4. **The docblock at `render/mod.rs:194-297`** (104 LOC) is genuinely
   load-bearing prose explaining *why* each node lands at its slot
   (Phase B Batch 2/3/4/5/6, cross-batch dependencies, `taa_dist_min_max`
   wiring, etc.). The architect's Step 4 spec says "delete it" but the
   WHY information must survive somewhere. A revised architect should
   either (a) require each `*Plugin::build` body to carry a slot-rationale
   docblock referencing the predecessor + successor relationships, or
   (b) preserve a `render/mod.rs` top-of-file docblock that just lists
   the canonical order + per-slot reasoning. The current spec implicitly
   assumes (a); the revised dispatch should be explicit.

5. **D5 has 4 construction subsystems** (`GpuProducerSet`, `BoundsCalcSet`,
   `WorldChangeSet`, `EntityUpdateSet` per architect §3.3 table) that the
   architect *assumed* D5 would declare as part of D5's own
   per-workstream split. **D5 did not split per-workstream** —
   `ConstructionPlugin::build` (`render/construction/mod.rs:1827-1913`)
   registers all four construction node systems off one plugin, no
   `SystemSet`s declared. The first edge each D4 subsystem needs
   (`AtmospherePlugin: .after(construction::EntityUpdateSet)`) cannot be
   wired until D5 declares those SystemSets — that's a cross-domain
   coordination point the revised architect must flag.

6. **The dispatch-brief's "100 tool-use bound" is itself worth
   surfacing.** The audit's suggestion (`00-reuse-audit.md:159`) of
   "implement one subsystem end-to-end as proof-of-concept" implies the
   bound is the right metric; this investigation's tool-call inventory
   confirms a single subsystem fits well under it (~15 calls). The
   project pattern emerging across items 1, 2, and 4 is that any single
   atomic refactor sub-step fits in a single dispatch — what doesn't fit
   is the *bundle* of related sub-steps. Decomposing-then-dispatching is
   the natural mode; the architect+implementor briefs should be aware of
   this systematically.

7. **No foundation rot in D4's surface** (echoing audit
   `00-reuse-audit.md:299-305` and architect §9.14 / `03-architecture.md:1072`).
   The render layer is well-architected; the bloat is structural-tax
   (the 17-tuple, the dual graph files). The plugin-per-subsystem
   refactor is structural cleanup, not foundation repair. This is good
   news for sequencing — it can land late in the orchestration without
   blocking other work.

8. **Risk of over-fragmenting per-subsystem dispatches.** If subsystem
   #1 lands but #2 produces a regression that the deterministic gates
   don't catch (only `--oasis-edit-visual` or `--vox-gpu-oracle`
   surfaces it), debugging is harder once 4 of 9 subsystems are
   half-extracted (which file owns the broken edge?). The revised
   architect should propose either a **single PR with 9 commits** (so
   `git bisect` works at subsystem granularity) or a **single dispatch
   with mandatory non-deterministic-gate verification at each
   subsystem-extracted intermediate state** (per
   `feedback-multiple-runs-rule-out-false-positives` ≥3 runs).
   Per-subsystem dispatches *with separate PRs* would lose this bisect
   property and amplify orchestration overhead.

9. **The `cell_shader_defs()` helper at `pipelines.rs:76-81` already
   landed** in D4-main per the prior dispatches; it's not part of Step 5
   work and survives the merge cleanly. The architect's §3.6 placement
   target (`render/pipelines/mod.rs`) translates trivially after the
   file split — same `pub` interface, same call sites, just moves with
   the file rename.
