# Item 4 architect revision — D4 Step 5 post-merge re-design

**Author:** delegate-architect (codebase-tightening-followup, Item 4 revision).
**Date:** 2026-05-21.
**Verified against:** HEAD `2bb03d1` (`refactor(render): D4 final cleanup`).
**Source-code edits performed:** zero (read-only by brief).

This revision supersedes §3.3 + §3.4 + §3.6 of
`docs/orchestrate/codebase-tightening/render-pipeline/03-architecture.md`
for the post-Resolution-D-merge world. It does **not** re-author the
plugin/SystemSet/node-extraction half of the original spec (§3.3's plugin
template, §3.5 sample-refine collapse, §3.7 `BUCKET_STORAGE_COUNT` shader-def,
§2 file-tree shape) — that half survives the merge structurally intact and
the original spec stays in force for those parts. Decomposition plan + four
load-bearing decisions follow.

---

## Original §3.3 + §3.4 + §3.6 context

### What the original spec specified

- **§3.3 — Plugin-per-subsystem template** (`03-architecture.md:257-329`).
  Every render-side subsystem owns its node + its `SystemSet` label + its
  layouts + its pipeline-ids in a `*Pipelines` resource of its own. The
  17-element `.chain()` collapses into ~11 `add_plugins((…))` calls + per-
  plugin `.before(…)/.after(…)` edges. Bevy 0.19's `Core3d`-schedule-as-
  graph idiom is `SystemSet` + ordering edges; there is no
  `RenderLabel`/`add_render_graph_edges` migration target here (§9.4,
  `:1043`).
- **§3.4 — `ShaderType` cutover** (`:353-463`). Out of scope for this
  revision — covered by Item 3 (separate architect revision); only the
  *file-tree shape* implied by the per-subsystem decomposition (§3.4's
  pipeline-resource ownership) touches §3.4's surface.
- **§3.6 — `cell_shader_defs()` helper** (`:569-616`). The helper lives at
  `render/pipelines/mod.rs` so D4 + D5 both import it. D5 reuses verbatim.
- **§1.10 — W0 retirement coordination** (`:48-49`). Architect assumed D5
  would keep `ConstructionPipelines` per-workstream-split; `NaadfPipelines`
  would shrink to ~5-7 fields holding the cross-subsystem core
  (`world_layout`, `frame_layout`, `blit_layout`, `empty_layout`,
  `blit_pipelines`, `blit_vertex`, `blit_shader`).
- **§6 Conflict 1** (`:892-911`). Anticipated the alternative: if D5
  architect lands the *literal merge*, D4 cannot also split — D4 keeps the
  existing wide `NaadfPipelines`, only thins `from_world` via helper
  extraction, and each plugin reads its needs off `Res<NaadfPipelines>`
  rather than `Res<FirstHitPipelines>`/etc. Spec calls this the
  "partial-landing fallback."

### What Resolution D actually changed

Verified at `crates/bevy_naadf/src/render/pipelines.rs:118-348`:
`NaadfPipelines` is now a **57-field** unified resource (counted by `awk`
field-extract; comment block confirming field-name preservation at
`pipelines.rs:285-290`). The former `ConstructionPipelines` resource at
`render/construction/mod.rs::ConstructionPipelines` is gone — its 25
fields are absorbed verbatim into `NaadfPipelines` (lines `:294-347`).
`ConstructionPlugin::build` no longer registers a separate
`ConstructionPipelines`; comment at `render/construction/mod.rs:1867-1870`
explicitly says "Construction pipelines now live on `NaadfPipelines`
(Resolution D — W0 seam retired). `NaadfRenderPlugin::build` registers
`NaadfPipelines` once via `init_gpu_resource`; no second-register needed
here."

This is the user-approved structural endpoint per
`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1208-1217`
("D5 architect's reading wins because it's a closer match to the user's
verbatim Q&A approval").

### What's silent post-merge

The architect spec wrote the per-subsystem `*Pipelines` decomposition
against a `NaadfPipelines` of ~7 fields. The actual `NaadfPipelines` is
57. The original spec is **silent** on:

1. **Whether the merge should be undone for D4's benefit.** §1.10 assumes
   a thin core resource; the actual world has a 57-field resource. The
   spec provides no post-merge decomposition recipe.
2. **Whether the 25 construction fields travel with a D4 decomposition.**
   The original §3.3 plugin table lists 4 construction sets
   (`GpuProducerSet`, `BoundsCalcSet`, `WorldChangeSet`, `EntityUpdateSet`)
   as "(D5-owned)" (`03-architecture.md:307-322`). Under the merge those
   pipelines live on `NaadfPipelines`. A 9-way split that excludes the
   construction fields leaves `NaadfPipelines` as a hybrid (thin render-
   side core + 25 construction-side fields); a 13-way split that includes
   them cross-cuts D5's surface.
3. **Whether D5 declares the SystemSets the original spec's `.after(...)`
   edges depend on.** D5 did not split per-workstream;
   `ConstructionPlugin::build` (`construction/mod.rs:1827-1913`) registers
   all 4 construction node systems off one plugin with **no** `SystemSet`
   labels declared (verified: zero matches for
   `SystemSet|GpuProducerSet|BoundsCalcSet|WorldChangeSet|EntityUpdateSet`
   in `construction/mod.rs`). The first edge each render-side D4 plugin
   needs (`AtmospherePlugin: .after(construction::EntityUpdateSet)`) has
   no target to bind to.
4. **What survives of the 104-LOC docblock at `render/mod.rs:194-297`.**
   The original spec says "delete it — the new edges live in each
   Plugin's body where the reader looks first." Verified at
   `render/mod.rs:194-297`: the docblock carries WHY each node lands at
   its slot (Phase B Batch 2/3/4/5/6, the `taa_dist_min_max` cross-batch
   wiring, the W2/W3/W4 construction insertion rationale, the C# fidelity
   reference points). The "delete it" instruction is silent on where the
   WHY moves.

The plugin/SystemSet/node-extraction half of the spec (the §3.3 template,
the §3.5 sample-refine collapse, the §3.7 `BUCKET_STORAGE_COUNT` shader-
def, the §2 flat-sibling file shape) carries forward intact. The
pipeline-decomposition half + the cross-domain coordination is what this
revision rewrites.

### The 15-element chain (corrected count)

The original spec talks about a "17-element `.chain()`" in places.
Verified at `render/mod.rs:298-331`: the current chain is **15 elements**
post-sample-refine-collapse, ordered exactly as:

```
1.  naadf_gpu_producer_node          (construction — D5 surface)
2.  naadf_bounds_compute_node        (construction — D5 surface)
3.  naadf_world_change_node          (construction — D5 surface)
4.  naadf_entity_update_node         (construction — D5 surface)
5.  naadf_atmosphere_node            (D4)
6.  naadf_first_hit_node             (D4)
7.  naadf_taa_reproject_node         (D4)
8.  naadf_sample_refine_clear_node   (D4)
9.  naadf_ray_queue_node             (D4)
10. naadf_global_illum_node          (D4)
11. naadf_sample_refine_continuous_node (D4 — collapsed 4-of-5)
12. naadf_spatial_resampling_node    (D4)
13. naadf_denoise_node               (D4)
14. naadf_calc_new_taa_sample_node   (D4)
15. naadf_final_blit_node            (D4)
```

`.chain().in_set(Core3dSystems::PostProcess).before(tonemapping)`
(`:328-330`). Eleven D4-owned nodes + four D5-owned construction nodes.

---

## Revised spec (the deliverable)

### Decision 1 — Post-merge pipeline-decomposition policy: **path (b), Conflict-1's partial-landing fallback**

**Choice.** `NaadfPipelines` stays the unified 57-field resource. **No
per-subsystem `*Pipelines` resources are introduced.** Each new
subsystem plugin holds a `Res<NaadfPipelines>` and reads only the fields
it needs.

**Rationale.**

1. **Resolution D was the user-approved structural endpoint.** Per
   `04-refactoring.md:1208-1217`, "D5 architect's reading wins because
   it's a closer match to the user's verbatim Q&A approval ('architect
   proposes the merge')." Re-splitting the merged resource three
   commits after the merge landed would be undoing user-approved
   structural work — the orchestration would be flapping. The faithful-
   port rule does not forbid this, but it does mean we need a stronger
   argument than "the original spec said per-subsystem" before
   re-introducing the structural cost the merge just paid down.
2. **The structural goal (plugin-per-subsystem ownership of node + edges
   + spans) is orthogonal to pipeline-resource ownership.** The original
   spec's own §6 Conflict-1 (`03-architecture.md:899-902`) says so
   verbatim: "the plugin-per-subsystem refactor still lands (the
   `SystemSet` + plugin shape is orthogonal to pipeline-resource
   ownership), but each plugin reaches into `Res<NaadfPipelines>` for
   its pipeline ids instead of `Res<FirstHitPipelines>`/etc." The
   architect explicitly carved this as a legitimate landing.
3. **Tool-call budget arithmetic.** Investigator estimates
   (`02-investigation-item-4-plugin-per-subsystem.md:201-207`): full-
   decomposition path is ~160-180 calls; partial-landing path is
   ~110-130 calls. The partial-landing path puts a per-subsystem
   dispatch comfortably inside the 100-call ceiling once each is its own
   focused dispatch (~15 calls each per `:175-178`); the full-
   decomposition path forces aggregate slicing of 57 fields across 9-13
   homes, with `from_world` splitting being the single non-mechanical
   surgery (3 implementor passes would converge on different splits).
4. **Faithful-port discipline.** Step 5 is structural cleanup, not
   foundation work. Re-splitting a freshly-merged resource into 9-13
   small resources to satisfy the original spec's locality preference is
   the kind of "Bevy-idiomatic over-engineering" the master-branch-
   identity rule guards against. The merged resource is greppable, the
   field names are workstream-tagged (`pipelines.rs:285-347` —
   `W5 — generator_model`, `W1 — chunk_calc`, etc.), and adding the
   plugin shell on top of it produces a fully-decomposed system layer
   over a unified resource layer. That's the right shape.

**Mechanical consequence.**

- `NaadfPipelines` stays in `render/pipelines/mod.rs` after the file
  split. Its 57-field shape is unchanged.
- `NaadfPipelines::from_world` (`pipelines.rs:350+`) stays as one
  monolithic body. The investigator's "splitting it across 9
  `*Pipelines::from_world`s while preserving the shared `taa_ring_depth`
  shader-def, the `pipeline_cache.get_bind_group_layout(...)` calls, and
  the inter-pipeline shared layouts" non-mechanical-surgery problem
  (`02-investigation-item-4-plugin-per-subsystem.md:228-235`) **does not
  arise**. The architect's spec for thinning `from_world` via helper
  extraction (`:48-49`, "thinning the `from_world` body through helper
  extraction") is deferred to a future micro-refactor; not part of Step 5.
- Each per-subsystem plugin's `build` body reads `Res<NaadfPipelines>` at
  the node-system level (existing pattern — `graph.rs`/`graph_b.rs` node
  bodies already do exactly this). Plugin-build registers only the node
  system + its `SystemSet` label; it does **not** `init_gpu_resource` a
  `*Pipelines` (the resource is registered once by `NaadfRenderPlugin`
  per `render/mod.rs:147`).
- The original §3.3 template needs ONE edit: drop the `init_gpu_resource::<FirstHitPipelines>()`
  line. The template body becomes:

```rust
// e.g. src/render/first_hit.rs
impl Plugin for FirstHitPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return; };
        render_app
            .add_systems(
                Core3d,
                naadf_first_hit_node
                    .in_set(FirstHitSet)
                    .in_set(Core3dSystems::PostProcess)
                    .before(tonemapping)
                    .after(crate::render::atmosphere::AtmosphereSet),
            );
    }
}
```

**Rejected alternatives.** Two paths rejected; see § Decisions & rejected alternatives below.

**Implementor lift instruction.** Each per-subsystem implementor brief
copies the §3.3 template **with the `init_gpu_resource` line removed**
and references this decision verbatim. No `*Pipelines` resource is
introduced; the node body's `Res<NaadfPipelines>` parameter is preserved
verbatim from the existing `graph.rs`/`graph_b.rs` signature.

---

### Decision 2 — Per-subsystem decomposition for dispatch budget

**Choice.** **Five separate per-subsystem dispatches** of ~15-25 tool
calls each, landed sequentially as separate commits on one branch
(single PR with multiple commits — see Decision 5). One coordination
dispatch precedes the per-subsystem dispatches (Decision 4's D5
SystemSet coordination is the prerequisite). One final-coordination
dispatch follows the last per-subsystem dispatch to delete `graph.rs` /
`graph_b.rs` and rewrite `render/mod.rs`'s `.add_systems(Core3d, ...)`
block.

**The 9 D4 subsystems** (verified against `graph.rs:74-258` +
`graph_b.rs:65-451`):

| # | subsystem | node fn | source today | target file (new vs absorb) |
|---|---|---|---|---|
| 1 | first_hit | `naadf_first_hit_node` | `graph.rs:74` | **new** `render/first_hit.rs` |
| 2 | final_blit | `naadf_final_blit_node` | `graph.rs:258` | **new** `render/final_blit.rs` |
| 3 | ray_queue | `naadf_ray_queue_node` | `graph_b.rs:121` | **new** `render/ray_queue.rs` |
| 4 | sample_refine | `naadf_sample_refine_clear_node` + `naadf_sample_refine_continuous_node` | `graph_b.rs:242, :296` | **new** `render/sample_refine.rs` |
| 5 | spatial_resampling | `naadf_spatial_resampling_node` | `graph_b.rs:394` | **new** `render/spatial_resampling.rs` |
| 6 | denoise | `naadf_denoise_node` | `graph_b.rs:451` | **new** `render/denoise.rs` |
| 7 | atmosphere | `naadf_atmosphere_node` | `graph_b.rs:65` | **absorb** into existing `render/atmosphere.rs` |
| 8 | gi | `naadf_global_illum_node` | `graph_b.rs:183` | **absorb** into existing `render/gi.rs` |
| 9 | taa | `naadf_taa_reproject_node` + `naadf_calc_new_taa_sample_node` | `graph.rs:137, :200` | **absorb** into existing `render/taa.rs` |

**Dispatch ordering.** Subsystems land in **chain order** so each new
plugin's `.after(<predecessor>::PredecessorSet)` edge has a landed target
on `main` to reference:

```
Dispatch 0 — D5 SystemSet declaration (Decision 4 prerequisite, cross-domain)
Dispatch 1 — atmosphere + first_hit       (chain slots 5, 6)
Dispatch 2 — taa + sample_refine_clear    (chain slots 7, 8 — taa_reproject is taa.rs absorb #1)
Dispatch 3 — ray_queue + gi               (chain slots 9, 10)
Dispatch 4 — sample_refine_continuous + spatial_resampling + denoise
                                          (chain slots 11, 12, 13)
Dispatch 5 — taa_calc_new + final_blit    (chain slots 14, 15 — taa.rs absorb #2)
Dispatch 6 — chain dissolution + render/mod.rs rewrite + docblock relocation
              (Decision 3 prerequisite — graph.rs + graph_b.rs DELETE,
               render/mod.rs:298-331 rewrites to .add_plugins, the 104-LOC
               docblock relocated per Decision 3)
```

Five subsystem-pair dispatches + one coordination prerequisite + one
final tear-down dispatch = **7 dispatches total**. Each subsystem-pair
dispatch fits in ~25-35 tool calls (two ~15-call subsystem extractions +
~5 calls of cross-coordination — the cheaper subsystems pair with the
heavier ones).

**Per-subsystem dispatch contents.** Each implementor brief contains:

1. The §3.3 template **with `init_gpu_resource` line removed** (Decision 1).
2. The node-body's exact source range (e.g. `graph.rs:74-135` for
   `naadf_first_hit_node`).
3. The predecessor's `SystemSet` import path (e.g. for
   `FirstHitPlugin`: `crate::render::atmosphere::AtmosphereSet`). The
   first dispatch (atmosphere + first_hit) imports
   `construction::EntityUpdateSet` from D5 surface (Decision 4).
4. The span-const to move (e.g. `FIRST_HIT_SPAN: &str = "naadf_first_hit"`,
   declared in `graph.rs:18` area — verify at edit time).
5. The slot-rationale docblock fragment (Decision 3) to copy from
   `render/mod.rs:194-297` into the new plugin file's `impl Plugin::build`
   body comment.
6. The intermediate verification recipe (Decision 5 — single PR with
   commits + non-deterministic gates at the end, NOT after each commit).
7. The "do NOT introduce `*Pipelines` resources" hard rule (Decision 1).

**Why pair dispatches.** Single-subsystem dispatches at ~15 calls each
would produce 9 dispatches; the orchestrator-side overhead is
non-trivial. Pairing chain-adjacent subsystems lets one dispatch handle
both "extract the predecessor + extract the successor + wire the edge
between them" — the second extraction in a pair is partly verified by
the first (the `.after(<predecessor>::PredecessorSet)` edge lights up
the predecessor's `SystemSet` declaration). Pair dispatches max out at
~35 calls — well under budget.

**Implementor lift instruction.** Per-subsystem dispatches lift items
1-7 above + Decision 1's template + Decision 3's docblock fragment
+ Decision 4's prerequisite + Decision 5's verification discipline.

---

### Decision 3 — Docblock survival plan: **path (c), split into both**

**Choice.** The 104-LOC docblock at `render/mod.rs:194-297` splits two
ways:

1. **Per-plugin slot-rationale docblock** (the WHY): each
   `*Plugin::build` body carries a Rust-doc-comment block at the top of
   the `add_systems(Core3d, ...)` call explaining the subsystem's slot
   rationale + the cross-batch dependency the slot encodes. Source:
   the relevant paragraph of `render/mod.rs:194-297` for that subsystem,
   copied verbatim (with file-path-reference adjustments).
2. **Top-level canonical-order docblock** (the WHAT): `render/mod.rs`
   carries a short top-of-file `//!`-style module docblock that lists
   the 15-element canonical chain order + a one-line-per-set summary of
   which plugin owns which slot. ~25-30 LOC. Lists the chain order so a
   reader looking at `render/mod.rs` sees the canonical order without
   having to walk 11 plugin files to reconstruct it.

**Rationale.** The 104-LOC docblock encodes TWO kinds of information:
(a) per-slot rationale (Phase B Batch 2/3/4/5/6, `taa_dist_min_max`
cross-batch wiring, sample-refine collapse rationale, C# fidelity
reference points to `WorldRenderBase.cs:352-362`); (b) canonical chain
order (which is currently implicit in the 15-line tuple at `:298-327`).
Path (a) "each plugin carries its own slot-rationale" loses (b) — a
reader cannot see the chain order without walking 11 plugin files.
Path (b) "top-level docblock lists the order + rationale" preserves the
single-source-of-truth shape but forces a reader of `gi.rs` who's
debugging "why is the GI bounce visible at end-of-Batch-5" to navigate
to `render/mod.rs` first. Path (c) — both — keeps the canonical-order
shape on `render/mod.rs` (where the `NaadfRenderPlugin` `add_plugins`
tuple already orders the plugins) AND co-locates per-slot WHY with the
plugin's `build` body where the implementer-of-future-changes looks
first. The per-plugin WHY references `render/mod.rs`'s top-level
docblock for the canonical-order anchor; the top-level docblock points
to per-plugin docblocks for slot rationale. Two-way reference, no
single-source-of-truth violation.

**Per-plugin docblock template** (example — `first_hit.rs`):

```rust
impl Plugin for FirstHitPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return; };
        // Slot rationale (canonical chain order at `render/mod.rs:1-40` docblock):
        // `naadf_first_hit_node` runs at slot 6, AFTER `AtmosphereSet`. Phase B
        // Batch 2 (`09-design-b.md` §11 Batch 2 step 8): atmosphere precompute
        // -> 4-plane first-hit -> final-blit fullscreen, all in PostProcess
        // and before tonemapping so the HUD draws over. The first-hit pass
        // raytraces independently of the main 3D pass; `naadf_atmosphere_node`
        // runs first per NAADF's dispatch order (`WorldRenderBase.cs:205-228`,
        // `09-design-b.md` §4.2). Batch 2 wires its output (`atmosphere_comp`)
        // into the first-hit pass at `@group(3)`.
        render_app.add_systems(
            Core3d,
            naadf_first_hit_node
                .in_set(FirstHitSet)
                .in_set(Core3dSystems::PostProcess)
                .before(tonemapping)
                .after(crate::render::atmosphere::AtmosphereSet),
        );
    }
}
```

**Top-level `render/mod.rs` docblock** (~25 LOC, replaces the existing
crate-level `//!` block + the 104-LOC `add_systems` comment block):

```rust
//! `NaadfRenderPlugin` — registers the Phase-A + Phase-B render pipelines,
//! bind-group layouts, render-world resources, and render-graph nodes
//! (`03-design.md` §5, `09-design-b.md` §11).
//!
//! ## Canonical render chain order
//!
//! Run in `Core3d` schedule, `Core3dSystems::PostProcess` set,
//! `.before(tonemapping)`. Per-slot WHY lives in each subsystem's
//! `*Plugin::build` body docblock; the chain order itself is encoded by
//! the `.after(predecessor::PredecessorSet)` edge each plugin declares.
//!
//! 1.  `construction::GpuProducerSet`       — W5 runtime GPU producer (D5)
//! 2.  `construction::BoundsCalcSet`        — W3 background AADF queue (D5)
//! 3.  `construction::WorldChangeSet`       — W2 world-change (D5)
//! 4.  `construction::EntityUpdateSet`      — W4 entity-update (D5)
//! 5.  `atmosphere::AtmosphereSet`          — Phase B Batch 2 atmosphere precompute
//! 6.  `first_hit::FirstHitSet`             — Phase B Batch 2 4-plane first-hit
//! 7.  `taa::TaaReprojectSet`               — Phase B Batch 6 `ReprojectOld`
//! 8.  `sample_refine::SampleRefineClearSet` — Phase B Batch 4 per-frame reset
//! 9.  `ray_queue::RayQueueSet`             — Phase B Batch 3 ray-queue builder
//! 10. `gi::GiSet`                          — Phase B Batch 3 `globalIllum`
//! 11. `sample_refine::SampleRefineContinuousSet` — Phase B Batch 4 collapsed-4 (C# fidelity, `WorldRenderBase.cs:352-362`)
//! 12. `spatial_resampling::SpatialResamplingSet` — Phase B Batch 5 Algorithm 2
//! 13. `denoise::DenoiseSet`                — Phase B Batch 5 sparse-bilateral
//! 14. `taa::CalcNewTaaSampleSet`           — Phase B Batch 6 `CalcNewTaaSample`
//! 15. `final_blit::FinalBlitSet`           — Phase A fullscreen blit
//!
//! Cross-batch wiring: `taa_dist_min_max` is written by `TaaReprojectSet` and
//! read by `SampleRefineContinuousSet`; until B6 wired the write, the refine
//! validity test rejected everything (correct-but-empty B4/B5). The reader
//! debugging a slot should read the slot's owning plugin's `build` body for
//! the per-slot rationale (Phase B Batch N reference + cross-batch reads).
```

**Mechanical edits.**

- `render/mod.rs:194-297` (the 104-LOC `add_systems(Core3d, ...)` comment
  block) DELETE. The per-slot WHY moves into each plugin's `build` body
  docblock as part of the per-subsystem dispatches (Decision 2).
- `render/mod.rs:1-15` (the existing `//!` block) REPLACE with the
  ~25-LOC canonical-order docblock above. Lands in **Dispatch 6** (the
  final chain-dissolution dispatch).
- Each per-subsystem dispatch (Dispatch 1-5) lifts its slot's paragraph
  from `:194-297` and lands it as a comment block inside the plugin's
  `build` body. The lift map:
  - `atmosphere`: paragraphs at `:202-205` (NAADF atmosphere dispatch
    order + Batch 2 wiring).
  - `first_hit`: paragraphs at `:194-205` (Batch 2 step 8).
  - `taa_reproject`: paragraphs at `:207-219` (Batch 6 `ReprojectOld`).
  - `sample_refine_clear`: paragraphs at `:242-250` (Batch 4 clear).
  - `ray_queue`: paragraphs at `:232-241` (Batch 3).
  - `gi`: paragraphs at `:232-241` (Batch 3 `globalIllum`).
  - `sample_refine_continuous`: paragraphs at `:251-265` (collapsed 4-of-5 + cross-batch `taa_dist_min_max`).
  - `spatial_resampling`: paragraphs at `:267-279` (Batch 5).
  - `denoise`: paragraphs at `:267-279` (Batch 5 denoiser).
  - `calc_new_taa_sample`: paragraphs at `:220-225` (Batch 6 `CalcNewTaaSample`).
  - `final_blit`: paragraphs at `:226-231` (Batch 2 + Batch 6 wiring).
  - Construction-node paragraphs at `:280-297` move into the D5
    construction plugin's `build` body docblock (Decision 4's
    prerequisite dispatch).

**Implementor lift instruction.** Each per-subsystem dispatch brief lifts
the relevant paragraph range from this decision's table and includes it
in the new plugin file. Dispatch 6 deletes the original `:194-297` block
and rewrites the top-level `//!` per the template above.

---

### Decision 4 — D5 SystemSet coordination: **path (a) prerequisite, cross-cutting**

**Choice.** A **Dispatch 0** runs **before** any per-subsystem dispatch
fires. Dispatch 0 is cross-cutting into D5's surface
(`render/construction/`) — it adds 4 `SystemSet` declarations and
wires them onto `ConstructionPlugin::build`'s 4 node-system
`.add_systems(Core3d, ...)` registrations.

**Rationale.** Without `construction::EntityUpdateSet` declared, the
first per-subsystem dispatch (atmosphere) has no `.after(...)` target.
Dispatch 0 is a tiny dispatch (~10-15 tool calls — 4 SystemSet derive
declarations + 4 `.in_set()` calls + the `ConstructionPlugin::build`
docblock for chain slots 1-4 per Decision 3 + a build + a vox-e2e
verification). It cannot be skipped, deferred, or bundled with the
per-subsystem extractions — those depend on it.

**Mechanical shape of Dispatch 0.**

1. In `render/construction/mod.rs` (the D5 surface), add four
   `SystemSet` declarations. Lift from architect §3.3 table
   (`03-architecture.md:307-323`):

```rust
/// Chain slot 1 (`render/mod.rs` canonical order). W5 runtime GPU producer.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GpuProducerSet;

/// Chain slot 2. W3 background AADF queue.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoundsCalcSet;

/// Chain slot 3. W2 world-change.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldChangeSet;

/// Chain slot 4. W4 entity-update.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityUpdateSet;
```

2. The 4 construction node systems currently live in
   `render/construction/{producer,bounds_calc,world_change,entity_update}.rs`
   (verified by `grep -rn "pub fn naadf_gpu_producer_node|naadf_bounds_compute_node|naadf_world_change_node|naadf_entity_update_node"`).
   They are registered today via the `.add_systems(Core3d, ...).chain()`
   block in `render/mod.rs:298-331`. Dispatch 0 does NOT move the
   registration; Dispatch 6 moves it (when the whole chain dissolves).
   Dispatch 0 only **declares** the SystemSets so the per-subsystem
   dispatches can reference them. Subsequent per-subsystem dispatches
   plus Dispatch 6 collectively wire the `.in_set(GpuProducerSet)`
   etc. onto the 4 construction node systems.

3. **Why Dispatch 0 doesn't move the construction-node registrations.**
   Two reasons: (a) the construction nodes are still chained off the
   `render/mod.rs` 15-tuple at this point — the tuple's `.chain()` would
   conflict with a separate `.add_systems(Core3d, naadf_gpu_producer_node.in_set(GpuProducerSet))`
   registration; (b) the D5 implementor's bail on the per-workstream
   prepare_construction split (Item 1) is still in flight — the
   construction-side plugin shape is unsettled. Keeping the construction
   node registrations in `render/mod.rs:298-331` through Dispatches 1-5
   means the chain is still load-bearing for construction-side
   correctness; Dispatch 6 dissolves the tuple atomically as the last
   step.

**Alternative considered:** path (b) — defer the construction-edge
wiring to a final-coordination pass. Rejected because the first per-
subsystem dispatch (atmosphere) **already needs** the
`construction::EntityUpdateSet` target. Deferring means atmosphere
gets a `.after(<predecessor>?)` placeholder OR a temporary
`.after(naadf_entity_update_node)` direct-system reference. Both are
worse than Dispatch 0 — the placeholder produces a wrong chain order
for any intermediate dispatch that lands without the placeholder
resolved; the direct-system reference creates a `system → SystemSet`
asymmetry the spec is trying to eliminate.

**Coordination with Item 1 (D5 Step 4 — `prepare_construction` split).**
If Item 1's architect-revision lands a per-workstream
`*ConstructionPlugin` decomposition (which moves construction-node
registrations off `render/mod.rs:298-331` and into per-workstream
plugins), Dispatch 0's SystemSet declarations still apply — the
per-workstream plugins each declare their `SystemSet` label as part of
the Item 1 work, **and** Dispatch 0's declarations on
`ConstructionPlugin::build` are unaffected because Dispatch 0 only adds
4 named labels, doesn't move registrations. **Net: Item 1 and Item 4
are non-blocking on each other once Dispatch 0 lands.** Item 1 can
proceed in parallel with Dispatches 1-5; Item 1's outcome is
independent.

**Implementor lift instruction.** Dispatch 0's brief is a self-contained
cross-domain edit: 4 SystemSet derive declarations on
`render/construction/mod.rs`, no node-system registration changes, no
per-workstream split. The `.in_set(<SystemSet>)` wiring onto the
construction node systems happens in Dispatch 6 (final tear-down) — at
which point the chain dissolves and the per-subsystem `Plugin`
ownership becomes load-bearing.

---

### Decision 5 — `git bisect` discipline: **path (a), single PR with 7 commits**

**Choice.** All 7 dispatches (Dispatch 0 + Dispatches 1-5 + Dispatch 6)
land as **separate commits on one branch**, opened as **one PR** at
PR-time. The branch is created at the start of Dispatch 0 and stays
open through Dispatch 6. Each dispatch commits + runs build + runs the
cheap deterministic gates (`--validate-gpu-construction`, `--vox-e2e`)
inside its dispatch. **Non-deterministic gates run once at end of
Dispatch 6**, not per-dispatch.

**Rationale.**

1. **Bisect granularity.** If a regression appears post-merge (which is
   the failure mode the investigator's side-note 8 flags —
   non-deterministic gates surface a chain-edge-order slip), `git
   bisect` walks 7 commits, one per dispatch. The bisect lands on a
   single subsystem-pair extraction or on the final chain-dissolution
   commit. That's the smallest debug-radius shape.
2. **Per-dispatch verification cost containment.** Running
   `--oasis-edit-visual` ×3 + `--vox-gpu-oracle` ×3 between every
   dispatch costs ~5-8 tool calls per dispatch × 7 dispatches = ~40-55
   calls of verification overhead. That's a full extra dispatch worth
   of budget burnt on intermediate verification of intermediate states
   that aren't even meaningful (a half-extracted chain is by definition
   not a stable verification target). End-of-Dispatch-6 verification is
   the load-bearing signal.
3. **Intermediate state correctness.** Dispatches 1-5 leave the chain
   half-extracted. Dispatch N for N in 1..=5 has the property:
   subsystems 1..N are plugin-extracted, subsystems N+1..9 are still
   chained off `render/mod.rs:298-331`. The chain `.chain()` provides
   strict ordering for the not-yet-extracted subsystems; the extracted
   subsystems' `.after(<predecessor>::SystemSet)` edges hook the
   already-extracted predecessor into the still-chained tail through
   the `Core3dSystems::PostProcess` set ordering. **The intermediate
   state is correct by construction** as long as Dispatch N's exit
   commit (a) builds, (b) passes
   `cargo test --workspace --lib` (179 passing), (c) passes
   `--validate-gpu-construction` (byte-equal — the canonical signal
   that node-order + bind-group identity survived). Those are cheap
   deterministic gates run per-dispatch; the non-deterministic ones
   come at the end.
4. **Half-extracted-chain debug pattern.** If `--validate-gpu-construction`
   fails at Dispatch N, the bisect-target is Dispatch N's commit.
   `--vox-e2e` is the secondary signal — `lum >= 160` cheap floor.
   Both are deterministic and bound at ~2 tool calls each.
5. **Investigator's side-note 8 risk** (over-fragmentation): the
   single-PR-with-7-commits shape addresses this directly. The branch
   is *one* unit of review at PR time; the bisect property is preserved
   without amplifying orchestration overhead.

**Mechanical shape.**

- **Branch creation:** Dispatch 0's brief opens `<worktree>/branch-name`
  (any name — typically `refactor/d4-step5-plugin-per-subsystem` per
  bevy-naadf naming convention).
- **Per-dispatch commit message:** descriptive of the extraction; e.g.
  "refactor(render): D4 Step 5 Dispatch 1 — atmosphere + first_hit
  plugin extraction".
- **Per-dispatch verification (lightweight, in-dispatch):**
  - `cargo build --workspace`
  - `cargo test --workspace --lib` (expect 179 passing)
  - `timeout 120s cargo run --bin e2e_render -- --validate-gpu-construction`
  - `timeout 120s cargo run --bin e2e_render -- --vox-e2e`
- **End-of-Dispatch-6 verification (heavyweight, gating PR merge):**
  - Full deterministic suite (`--validate-gpu-construction`, `--vox-e2e`,
    `--edit-mode`, `--entities`, `--runtime-edit-mode`, `--baseline`,
    `--vox-gpu-construction`).
  - Non-deterministic gates ≥3 runs each:
    `for i in 1 2 3; do timeout 120s cargo run --bin e2e_render -- --oasis-edit-visual; done`
    + same for `--vox-gpu-oracle`. Δ-luminance baseline at HEAD =
    15.0/14.7/15.1/15.1/15.1/14.9 → 14.98 mean per `04-refactoring.md:1654-1657`.
- **PR opens after Dispatch 6's heavyweight verification passes.**

**Rejected alternative:** single-dispatch landing of all 9 subsystems
+ chain dissolution in one go with mandatory per-intermediate-state
non-deterministic verification (investigator's side-note 8 option b).
Rejected because the dispatch-budget arithmetic (`02-investigation-item-4:201-207`)
puts the total at 110-180 tool calls; even with the partial-landing
discipline (Decision 1) ruling out the upper bound, ~110-130 calls
plus 6 sets of mandatory non-deterministic verification (~30 calls
of run-time overhead) lands at ~140-160 calls — well over the 100-
call budget.

**Implementor lift instruction.** Each dispatch's brief carries: the
branch name (lifted from Dispatch 0), the commit-message convention,
the lightweight per-dispatch verification recipe, and the explicit "do
NOT run non-deterministic gates here — they run at end of Dispatch 6"
hard rule.

---

### Decomposition plan (consolidated)

```
─────────────────────────────────────────────────────────────────────────
Dispatch 0 — D5 SystemSet declaration (cross-cutting, ~12 tool calls)
─────────────────────────────────────────────────────────────────────────
  - Opens branch.
  - Adds 4 SystemSet derives to render/construction/mod.rs.
  - Does NOT add .in_set() to the 4 construction node-system
    registrations (those move with Dispatch 6 when the chain dissolves).
  - Verifies: build + lib tests + --validate-gpu-construction + --vox-e2e.
  - Commit message: "refactor(render): D4 Step 5 Dispatch 0 — declare
    construction SystemSets (GpuProducerSet/BoundsCalcSet/WorldChangeSet/
    EntityUpdateSet)"

─────────────────────────────────────────────────────────────────────────
Dispatch 1 — atmosphere + first_hit (~28 tool calls)
─────────────────────────────────────────────────────────────────────────
  - Absorbs naadf_atmosphere_node into render/atmosphere.rs.
    Declares AtmosphereSet; .after(construction::EntityUpdateSet).
    Lifts :202-205 docblock per Decision 3.
  - Creates render/first_hit.rs.
    Declares FirstHitSet; .after(atmosphere::AtmosphereSet).
    Lifts :194-205 docblock per Decision 3.
  - render/mod.rs: drops naadf_atmosphere_node + naadf_first_hit_node
    from the 15-tuple; adds atmosphere::AtmospherePlugin +
    first_hit::FirstHitPlugin to a new (initially separate) add_plugins
    tuple. The remaining 13 nodes still .chain() in the same
    add_systems(Core3d, ...) block. Intermediate-state correctness:
    AtmosphereSet's .after(construction::EntityUpdateSet) edge + the
    13-tuple's .chain() head (starting at naadf_taa_reproject_node)
    bridge correctly via Core3dSystems::PostProcess set ordering.
  - Verifies: build + lib tests + --validate-gpu-construction + --vox-e2e.
  - Commit message: "refactor(render): D4 Step 5 Dispatch 1 — extract
    atmosphere + first_hit subsystem plugins"

─────────────────────────────────────────────────────────────────────────
Dispatch 2 — taa_reproject + sample_refine_clear (~30 tool calls)
─────────────────────────────────────────────────────────────────────────
  - Absorbs naadf_taa_reproject_node into render/taa.rs (TaaPlugin's
    first node — taa.rs ends up owning two sets, TaaReprojectSet + the
    later CalcNewTaaSampleSet).
    Declares TaaReprojectSet; .after(first_hit::FirstHitSet).
    Lifts :207-219 docblock per Decision 3.
  - Creates render/sample_refine.rs (first half — just the clear node).
    Declares SampleRefineClearSet; .after(taa::TaaReprojectSet).
    Lifts :242-250 docblock.
  - render/mod.rs: drops the 2 nodes from the 11-tuple, adds 2 plugins.
    11-tuple shrinks to 9.
  - Verifies as Dispatch 1.

─────────────────────────────────────────────────────────────────────────
Dispatch 3 — ray_queue + gi (~28 tool calls)
─────────────────────────────────────────────────────────────────────────
  - Creates render/ray_queue.rs.
    Declares RayQueueSet; .after(sample_refine::SampleRefineClearSet).
    Lifts :232-241 docblock.
  - Absorbs naadf_global_illum_node into render/gi.rs.
    Declares GiSet; .after(ray_queue::RayQueueSet).
    Lifts :232-241 docblock (Batch 3 globalIllum portion).
  - render/mod.rs: 9-tuple shrinks to 7.
  - Verifies as Dispatch 1.

─────────────────────────────────────────────────────────────────────────
Dispatch 4 — sample_refine_continuous + spatial_resampling + denoise (~35 tool calls)
─────────────────────────────────────────────────────────────────────────
  - Extends render/sample_refine.rs with the continuous node.
    Declares SampleRefineContinuousSet; .after(gi::GiSet).
    Lifts :251-265 docblock (collapsed 4-of-5 + taa_dist_min_max
    cross-batch).
  - Creates render/spatial_resampling.rs.
    Declares SpatialResamplingSet; .after(sample_refine::
    SampleRefineContinuousSet).
    Lifts :267-279 docblock.
  - Creates render/denoise.rs.
    Declares DenoiseSet; .after(spatial_resampling::SpatialResamplingSet).
    Lifts :267-279 docblock.
  - render/mod.rs: 7-tuple shrinks to 4.
  - Verifies as Dispatch 1.
  - This is the heaviest dispatch — 3 subsystems. Pair-budget rationale
    (Decision 2) supports it: spatial_resampling and denoise are tiny
    new files (~110 + ~120 LOC per architect §2). The sample_refine
    continuous absorption is the substantive piece; the other two are
    short tail extractions.

─────────────────────────────────────────────────────────────────────────
Dispatch 5 — calc_new_taa_sample + final_blit (~25 tool calls)
─────────────────────────────────────────────────────────────────────────
  - Absorbs naadf_calc_new_taa_sample_node into render/taa.rs (second
    node — TaaPlugin now owns both TaaReprojectSet + CalcNewTaaSampleSet).
    Declares CalcNewTaaSampleSet; .after(denoise::DenoiseSet).
    Lifts :220-225 docblock.
  - Creates render/final_blit.rs.
    Declares FinalBlitSet; .after(taa::CalcNewTaaSampleSet).
    Lifts :226-231 docblock.
  - render/mod.rs: 4-tuple shrinks to 2.
  - Verifies as Dispatch 1.

─────────────────────────────────────────────────────────────────────────
Dispatch 6 — chain dissolution + render/mod.rs rewrite + heavyweight verify
              (~40 tool calls including verification)
─────────────────────────────────────────────────────────────────────────
  - render/mod.rs: drops the now-2-element add_systems(Core3d, ...)
    block entirely. Construction node systems (the remaining 4) get
    moved out of render/mod.rs:298-331 and into
    ConstructionPlugin::build's add_systems(Core3d, ...) registration
    with their respective .in_set(<construction-SystemSet>) labels
    declared in Dispatch 0. (Note: this is the only Dispatch that
    touches construction/mod.rs's add_systems body — Dispatch 0 only
    added SystemSet declarations.)
  - render/mod.rs: top-of-file `//!` docblock REPLACES the existing
    one + the deleted 104-LOC :194-297 block, per Decision 3.
  - render/mod.rs: NaadfRenderPlugin::build now contains a single
    .add_plugins(...) tuple over 11 plugins (atmosphere, first_hit,
    taa, sample_refine, ray_queue, gi, spatial_resampling, denoise,
    final_blit) + the existing prepare/extract/init systems.
  - DELETE: render/graph.rs (309 LOC).
  - DELETE: render/graph_b.rs (500 LOC at HEAD).
  - render/mod.rs: drop pub mod graph; pub mod graph_b;.
  - Audit: grep -rn "use crate::render::graph" crates/bevy_naadf/src/
    + grep -rn "use crate::render::graph_b" — expect zero matches.
  - Heavyweight verification:
    * cargo build --workspace
    * cargo test --workspace --lib (179 + 1 ignored)
    * timeout 120s cargo run --bin e2e_render -- <each deterministic gate>
    * for i in 1 2 3; do timeout 120s cargo run --bin e2e_render -- --oasis-edit-visual; done
      (Δ-lum mean within 4% of 14.98 baseline)
    * for i in 1 2 3; do timeout 120s cargo run --bin e2e_render -- --vox-gpu-oracle; done
  - PR opens.
  - Commit message: "refactor(render): D4 Step 5 Dispatch 6 — chain
    dissolution + graph.rs/graph_b.rs DELETE + render/mod.rs top-level
    docblock rewrite"
```

**Total tool-call estimate:** 12 + 28 + 30 + 28 + 35 + 25 + 40 ≈ **198
tool calls across 7 dispatches**. Each dispatch fits comfortably under
the 100-call ceiling.

**Coordination dependencies:**
- Item 1 (D5 Step 4): non-blocking on Item 4. Dispatch 0's SystemSet
  declarations are independent of Item 1's per-workstream split. If
  Item 1 lands before Item 4, Dispatch 0 still adds the 4 SystemSet
  derives on whichever construction-plugin shape exists at that point.
  If Item 4 lands before Item 1, Item 1's eventual per-workstream split
  inherits the 4 SystemSet labels declared in Dispatch 0.
- Items 2/3/5: independent. Item 4 does not block on them.

---

## Decisions & rejected alternatives

### Decision 1 — Pipeline policy: path (b) chosen

**Chose:** Partial-landing per Conflict-1 (`03-architecture.md:907-911`)
— each plugin reads `Res<NaadfPipelines>`, no per-subsystem `*Pipelines`
resources introduced.

**Rejected (a) — Re-split the 57-field `NaadfPipelines` into per-subsystem `*Pipelines` resources.**
Why rejected:
- Resolution D was the user-approved structural endpoint
  (`04-refactoring.md:1208-1217`). Re-splitting three commits later
  flaps the orchestration's structural direction.
- The `from_world` body (a single 800+ LOC layout-and-pipeline build)
  becomes non-mechanical surgery when sliced across 9 sub-resources
  with shared inter-pipeline layouts (`world_layout`, `frame_layout`,
  `empty_layout`, `taa_layout`). Three implementor passes would
  converge on three different splits.
- Tool-call cost: full-decomposition adds ~25-40 calls per investigator
  estimate (`02-investigation-item-4:189-192`). Lands the total over
  budget even with the per-subsystem-dispatch shape.
- Locality benefit (each plugin owns its pipeline ids) is weak —
  pipeline ids are read once per node body and the cohesion is at the
  layout level (which couldn't decompose anyway: shared layouts have
  shared owners).

**Rejected (c) — Hybrid: thin core `NaadfPipelines` + per-render-subsystem `*Pipelines` + monolithic construction-side `ConstructionPipelines` re-introduction.**
Why rejected:
- Half-undoes Resolution D (re-introduces a construction-side
  resource). The orchestration's resolution language ("propose the
  merge") forbids partial unmerging.
- Net structural cost = a worst-of-both-worlds: complicates the render-
  side decomposition while keeping the construction side merged-as-
  hybrid.

**Flip condition.** What would flip the call to (a)? If a future architect
finds a load-bearing reason that `from_world` *must* be sliced (e.g. a
deferred-pipeline-build feature where each subsystem builds its own
pipelines lazily on-demand), then the per-subsystem `*Pipelines`
resources become non-optional. Not currently the case — all 57
pipelines build eagerly in `RenderStartup` and are read-only thereafter.

### Decision 2 — Decomposition: pair-dispatch chosen

**Chose:** 5 subsystem-pair dispatches + 1 prerequisite + 1 final = 7
total. Each ~12-40 tool calls. Chain-order sequencing.

**Rejected — Single dispatch with 9 subsystems + chain dissolution.**
Why rejected: tool-call arithmetic (110-180 calls) over the 100-call
ceiling. Two prior implementors bailed on this shape
(`04-refactoring.md:1081-1121` + `:1585-1619`); a third dispatch with
the same shape would bail identically.

**Rejected — One-subsystem-per-dispatch (9 separate dispatches).**
Why rejected: orchestration overhead. Each dispatch costs setup/teardown
+ context-load. 9 dispatches at ~15 calls each = 135 effective calls vs
7 pair-dispatches at ~28 calls each = 198 effective calls (slightly
higher!), but with substantially less orchestration friction (3 fewer
sub-orchestration sessions to thread through). Pair dispatches are the
local optimum.

**Rejected — Chain-reverse ordering (start at final_blit, walk
backwards).** Why rejected: the `.after(...)` edges point at
predecessors — landing final_blit first means
`FinalBlitPlugin::build` references `CalcNewTaaSampleSet`, which
doesn't exist yet, so the intermediate state has a broken edge that
the compiler doesn't catch (SystemSet identity is by-type, missing
type fails to compile cleanly — but the intermediate `.chain()` order
preservation breaks subtly). Forward ordering keeps intermediate
states correct-by-construction.

**Flip condition.** If a per-subsystem dispatch lands at >40 calls
(the realistic ceiling for a pair-dispatch with friction), the
remaining dispatches need to be re-decomposed into single-subsystem
shape. The Dispatch 1 (atmosphere + first_hit) tool-call count is the
load-bearing signal.

### Decision 3 — Docblock: path (c) split chosen

**Chose:** Top-level `//!` docblock on `render/mod.rs` (canonical-order
list ~25 LOC) + per-plugin `build`-body comment (slot-rationale
paragraph from `:194-297`).

**Rejected (a) — Per-plugin docblocks only, `render/mod.rs` docblock dropped.**
Why rejected: loses the canonical-order anchor. A reader of `render/mod.rs`
sees an unordered `.add_plugins(...)` tuple; the chain order is
implicit in the `.after(...)` edges scattered across 11 files. The
order is the load-bearing structural fact (the whole Step 5 *is* about
the chain) — keeping a top-level reference is correct.

**Rejected (b) — Top-level docblock only, per-plugin docblocks dropped.**
Why rejected: forces a reader debugging `gi.rs` to navigate to
`render/mod.rs` to find the WHY. The WHY (Phase B Batch N, cross-batch
`taa_dist_min_max` wiring, C# fidelity reference) is per-subsystem
knowledge — co-locating it with the subsystem is the natural shape.

**Flip condition.** If implementors find the per-plugin docblock
duplication unwieldy (multiple plugins citing the same Phase B Batch),
the per-plugin docblocks can shrink to one-line references
(`// Slot 6 — Phase B Batch 2 (see render/mod.rs:1-30)`) and the
top-level docblock absorbs the full per-slot rationale. The original
spec's "delete it" framing was the architect mistakenly assuming the
WHY was redundant with the `.after(...)` edges; the WHY is genuinely
load-bearing structural-knowledge transfer.

### Decision 4 — D5 SystemSets: path (a) prerequisite chosen

**Chose:** Dispatch 0 declares the 4 SystemSets before any per-
subsystem dispatch fires. Cross-cutting into D5's surface (allowed —
the SystemSet declarations are render-graph-coordination, not
construction-internal logic).

**Rejected (b) — Defer the construction-edge wiring to a final-coordination pass.**
Why rejected: the first per-subsystem dispatch (atmosphere) already
needs `construction::EntityUpdateSet` as its `.after(...)` target.
Deferring means atmosphere lands with a placeholder or a direct-system
`.after(naadf_entity_update_node)` reference — both worse than the
proper SystemSet target. The cost saving (one dispatch) is not worth
the structural inconsistency.

**Rejected (c) — Add a hidden SystemSet at the head of each per-subsystem dispatch.**
Why rejected: Dispatch 1's atmosphere needs only
`EntityUpdateSet`; if Dispatch 1 declares all 4 it leaks D5-surface
ownership into a D4 dispatch. Dispatch 0's separation is cleaner.

**Flip condition.** If Item 1 (D5 Step 4) lands per-workstream
`*ConstructionPlugin`s before Dispatch 0 fires, Dispatch 0 must
declare the 4 SystemSets on whichever-workstream-plugin owns each node
system, not on `ConstructionPlugin::build`. The lift is one path-
rename per declaration. No flip on the fundamental shape; only on
file location.

### Decision 5 — Bisect: path (a) single PR with 7 commits chosen

**Chose:** One branch, 7 commits, one PR. Lightweight per-dispatch
verification (build + lib tests + 2 deterministic gates); heavyweight
verification at end of Dispatch 6 only.

**Rejected (b) — Single dispatch with mandatory non-deterministic-gate verification at each subsystem-extracted intermediate state.**
Why rejected: the 100-call dispatch ceiling. ~110-130 calls for the
work + ~30-50 calls for per-intermediate-state non-deterministic
verification = ~150-180 calls. Over budget.

**Rejected — Single dispatch with no intermediate verification.**
Why rejected: defeats the bisect property entirely. If the final
verification fails, the bisect collapses to "the whole dispatch broke
it" — 9-subsystem-and-chain-dissolution diff.

**Rejected — Separate PR per subsystem-pair (7 PRs).**
Why rejected: 7 PR reviews, 7 merges to main, 7 chances for an
intermediate state to ship behaviourally subtle (a half-extracted
chain in main). Per the master-branch-identity rule
(`bevy-naadf-faithful-port-rule`), main must be functionally complete
between merges; half-extracted chains are not functionally complete in
the structural sense (they ship intermediate scaffolding). Single PR
collapses the 7 commits into one merge.

**Flip condition.** If a Dispatch N (N in 1..=5) causes
`--validate-gpu-construction` or `--vox-e2e` to fail, the
implementor immediately drops to non-deterministic-gate verification
at Dispatch N's commit before continuing. The flip is from
"end-of-Dispatch-6 only" to "after the suspect dispatch" — single-
dispatch granularity for the diagnostic, not the routine.

---

## Assumptions made

1. **The chain is 15 elements, not 17.** Verified by Read at
   `render/mod.rs:298-327`. The "17-element" figure in the bailout
   language at `04-refactoring.md:1081-1121` is stale — predates the
   sample-refine 5→2 collapse. If a future dispatch re-introduces a
   collapsed-sample-refine reverse split, the chain count becomes 16+
   and the decomposition table in Decision 2 needs revision (one extra
   subsystem-pair dispatch).
2. **No further Resolution-D-style merges are in flight.** Item 1's
   architect revision (D5 Step 4 `prepare_construction` split) does
   *not* propose a further merge of `NaadfPipelines` with another
   render-side resource. The 57-field shape is the post-merge endpoint.
   If Item 1's revision proposes additional pipeline-resource changes,
   Decision 1's path-(b) policy needs re-checking against the new
   surface.
3. **`cell_shader_defs()` at `pipelines.rs:76-81` survives the plugin
   split unchanged.** Verified: the helper is at
   `pipelines.rs:76-81`, called from `NaadfPipelines::from_world`
   (signature `pub fn cell_shader_defs() -> Vec<ShaderDefVal>`).
   Decision 1 keeps `NaadfPipelines` monolithic, so `from_world` stays
   as one body and the helper stays in `render/pipelines/mod.rs`
   post-file-split. `pub` interface unchanged; D5's call site
   (`use crate::render::pipelines::cell_shader_defs;`) resolves
   verbatim.
4. **D5's `ConstructionPlugin::build` is the canonical construction-
   plugin shape at Step 5 time.** Verified at
   `render/construction/mod.rs:1827-1913`. If Item 1's revision
   restructures `ConstructionPlugin::build` (per-workstream
   decomposition into `Wn*Plugin`s), Decision 4's Dispatch 0 lift
   target shifts to the per-workstream plugin owning each node system.
   The SystemSet declarations themselves are independent.
5. **`graph.rs` is 309 LOC + `graph_b.rs` is 500 LOC.** Verified at
   HEAD: 309 and 500 respectively (architect's "574" for `graph_b.rs`
   in `03-architecture.md:23` is stale — predates the sample-refine
   collapse landing).
6. **`NaadfPipelines` is registered exactly once.** Verified at
   `render/mod.rs:147` (`init_gpu_resource::<NaadfPipelines>()` in
   `NaadfRenderPlugin::build`). `ConstructionPlugin::build` no longer
   registers a separate `ConstructionPipelines`. Per-subsystem plugins
   under Decision 1 do not register pipeline resources — the per-
   subsystem `Plugin::build` body's `add_systems(Core3d, ...)` call
   is its sole render-app mutation.
7. **The 4 construction node systems are registered in
   `render/mod.rs:298-331`, not in `ConstructionPlugin::build`.**
   Verified: `render/mod.rs:298-327` lists all 15 chain elements
   including `naadf_gpu_producer_node`, `naadf_bounds_compute_node`,
   `naadf_world_change_node`, `naadf_entity_update_node`.
   `ConstructionPlugin::build` adds only `prepare_construction` to
   `Render` schedule (`construction/mod.rs:1884-1888`) and 2 extract
   systems (`construction/mod.rs:1899-1911`). The 4 construction node
   systems travel with `render/mod.rs`'s chain dissolution in
   Dispatch 6.
8. **No pre-existing `SystemSet` impl in D4-render-territory beyond Bevy's `RenderSystems`/`Core3dSystems`.** Verified: zero matches for
   `SystemSet` derives in `render/construction/mod.rs`. The only
   SystemSet usage in scope is via Bevy's built-in sets — every
   `*Set` Decision 2 introduces is a new declaration.
9. **The `--oasis-edit-visual` baseline is Δ-luminance 14.98 mean.**
   Per `04-refactoring.md:1654-1657`. Decision 5's verification recipe
   uses this as the cross-run-variance reference.
10. **Bevy 0.19's SystemSet + ordering-edge idiom is the right
    target.** Verified per architect §9.4 (`03-architecture.md:1043`):
    "the actual idiom-fit is `SystemSet` + `.before()`/`.after()`"
    because the port runs node-systems in `Core3d` schedule. No
    `RenderLabel`/`add_render_graph_edges` migration is in scope.

---

## Side notes / observations / complaints

1. **The architect spec is genuinely sound — the merge is what
   destabilised it.** Reading §3.3 + §3.5 + §3.7 + §2 file-tree end-to-
   end shows a coherent design: plugin-per-subsystem ownership, flat
   sibling file layout, per-subsystem `*Pipelines` decomposition
   following the per-workstream `ConstructionPipelines` decomposition.
   The spec internally consistent under its assumed precondition. The
   surprise is that the orchestration approved Resolution D *literal
   merge* (which the D5 architect proposed) over the *D4-spec-assumed*
   per-workstream split. That decision wins on the orchestrator's
   resolution-language fidelity argument and on cross-domain locality
   (one resource per render-domain instead of two), but it leaves D4's
   §1.10 + §3.4-implied + §3.6-locked-in shape obsolete on the
   pipeline-resource axis. **The right fix is what this revision does
   (partial-landing on the merged resource), not a re-split.**

2. **Decision 1 is the most consequential call here and it's the most
   tempting to invert.** The original spec's path (per-subsystem
   `*Pipelines`) has clear locality benefits — a plugin file owning
   its layouts + ids + node + edges is the "complete subsystem cell"
   shape Bevy idiom encourages. The argument against it (Resolution D
   user-approval + `from_world` splitting cost + faithful-port
   discipline) wins on this specific orchestration's constraints. **An
   alternative architecture session reading the same code could
   legitimately come to the opposite conclusion** if it weighted
   locality higher than orchestration-stability and Resolution-D
   adherence. Surfacing this explicitly so the next implementor sees
   the decision is non-trivial and the rejected alternative isn't
   silly — it's structurally defensible.

3. **Foundation rot watch (equal-footing).** Re-reading the D4 surface
   in 2026-05-21 vs the original architect's 2026-05-20 read: the
   surface is still well-architected. The 15-element `.chain()` is
   genuinely the load-bearing smell (echoes architect §9.1 + §9.14 +
   investigator side-note 7). The merged 57-field `NaadfPipelines` is
   *not* a rot signal — its 57 fields are workstream-tagged (W1/W3/W4/
   W5 etc. in `pipelines.rs:285-347` comments) and the merge comment
   block (`:285-290`) explicitly notes "Field names preserved verbatim
   from the prior struct to minimise consumer-call-site churn." That
   minimised-churn discipline is the right move. **The render domain
   is medium-stinky, not foundation-rotten.** Step 5 is the correct
   surgical move.

4. **Item 1 (D5 Step 4) blocks zero of this work.** Item 1 is the
   `prepare_construction` per-workstream split. It's the *prepare*
   layer; Item 4 is the *node* layer. They share no system; they share
   no resource (Item 1 modifies `prepare_construction`; Item 4 modifies
   node-system registrations in `render/mod.rs:298-331`). The
   Dispatch 0 SystemSet declarations land on whichever construction-
   plugin shape exists at Dispatch 0 time. **Item 1 and Item 4 can
   land in either order, or in parallel.**

5. **D5 architect's prescription deserves a re-read by Item 4
   implementor.** Per `04-refactoring.md:1199-1203`: "D5 architect's
   §2.10 'chosen: (a) merge' recipe at lines 858-898 of the
   gpu-construction architecture doc is the load-bearing prescription
   — it enumerates the exact 25 fields with their workstream tags +
   the `construction_*` prefix proposal + the rationale for option (a)
   over option (b)." Implementor extracting this dispatch's subsystem
   for the third time should read D5 architect's §2.10 to understand
   *why* the merge was chosen — the answer is "minimised consumer-
   call-site churn" + "atmospheric pipelines locality wasn't strong
   enough to justify N+M separate resources". That rationale informs
   Decision 1's path-(b) choice.

6. **The 104-LOC docblock at `render/mod.rs:194-297` is genuinely the
   highest-value piece of prose in the D4 surface.** Reading it
   end-to-end after-the-investigator: it's a structured walk through
   the Phase B implementation — each paragraph references the design
   doc section that justifies the slot, the cross-batch dependency,
   the C# fidelity check. The "delete it" instruction in the original
   spec is shallow — the spec assumes the `.after(...)` edges encode
   the WHY, but they only encode the WHAT (ordering). Preserving the
   WHY per Decision 3 is non-optional.

7. **No suggestion to abandon Step 5.** The brief offered "step 5
   should be abandoned" as an equal-footing option. Rejected:
   - The 15-element `.chain()` is genuinely an edit-magnet on every
     PR that adds a render node (the entire chain has to be
     re-reasoned to slot a new node — the investigator's side-note 8
     describes this risk for the wrong direction).
   - The `graph.rs` + `graph_b.rs` files are dual-graph artifacts
     from a structural episode that no longer applies (they were
     split during Phase B's batch landings; now that all batches
     have landed, the split is residue).
   - The plugin shape is the correct Bevy-0.19 idiom (architect
     §9.4 verified).
   - The work is mechanical structural cleanup, not foundation
     work; faithful-port discipline encourages it (the master-
     branch-identity rule explicitly says "aggressive deletion of
     non-C#-parity rot is encouraged on master" —
     `master-branch-identity` memory).
   The merge superseded a *portion* of the original spec, not the
   whole spec. Decision 1's path-(b) is the natural minimum-friction
   landing of the spec that *remains*.

8. **The Dispatch 6 chain dissolution is the highest-blast-radius
   single commit.** Deleting `render/mod.rs:298-331`'s entire
   15-element `.chain()`, replacing with a `.add_plugins((...))`
   tuple, deleting `graph.rs`, deleting `graph_b.rs`, moving the 4
   construction node systems into `ConstructionPlugin::build`, and
   rewriting the top-level docblock — all in one commit. **This is
   the right shape** (intermediate states between Dispatches 1-5 are
   correct-by-construction with the chain + edges co-existing;
   splitting Dispatch 6 into N atomic commits would create N
   half-states that aren't behaviourally identical to either side).
   Estimating ~40 tool calls for Dispatch 6 specifically is the upper
   end; the heavyweight verification is the bulk of the cost
   (deterministic gates ×7 + non-deterministic ×6 = ~13-15 e2e
   gate runs at ~1 call each plus build/test = ~25 calls of verify
   + ~15 calls of actual code edits).

9. **The `prepare_blit_pipeline` system at `pipelines.rs` is owned by
   `FinalBlitPlugin` post-extraction.** Architect §2 file-tree shows
   `final_blit.rs` carrying `BlitPipelines (the per-format pipeline
   HashMap + prepare_blit_pipeline)`. Under Decision 1's "no per-
   subsystem `*Pipelines` resources" rule, this means
   `prepare_blit_pipeline` (currently registered at
   `render/mod.rs:186` in the `RenderSystems::PrepareResources` set)
   moves to `FinalBlitPlugin::build`'s `add_systems(Render, ...)`
   registration. The `blit_pipelines: HashMap<TextureFormat,
   CachedRenderPipelineId>` field stays on `NaadfPipelines`. **One
   subtle structural point worth flagging in the Dispatch 5 brief.**

10. **The investigator's side-note 8 (over-fragmenting per-subsystem
    dispatches) shapes Decision 5.** Sub-agent compliance risk on
    `general-purpose` agents (per `feedback-subagent-research-only-
    compliance` memory) means the implementor dispatches should be
    `delegate-implementor` shape, not `general-purpose`. **The
    orchestrator should ensure each per-subsystem dispatch is
    structurally bounded** — a `general-purpose` agent given Dispatch
    1's brief might try to land Dispatches 2-5 too "since they're
    similar mechanical work." The brief's hard-rule "land *only*
    atmosphere + first_hit; commit; exit" is load-bearing.

11. **The CLAUDE.md verification rule is fully respected by Decision
    5.** Per project CLAUDE.md "Never run `cargo run --bin
    bevy-naadf` as a verification step" — Decision 5's per-dispatch
    verification recipe and Dispatch 6's heavyweight recipe both use
    only the `e2e_render` binary, never the main `bevy-naadf` binary.
    The user's live visual check happens after the PR is opened, not
    during any dispatch.

12. **Equal-footing complaint about the original spec's "delete it"
    framing for the 104-LOC docblock.** The architect spec is otherwise
    exceptional — file:line cited, conflicts surfaced, equal-footing
    rejections marked — but the "delete it; the edges encode it"
    instruction for `:194-297` is a rare misstep. The docblock encodes
    cross-batch *semantic* dependencies (e.g. `taa_dist_min_max`
    written by ReprojectOld, read by sample_refine_continuous); the
    `.after(TaaReprojectSet)` edge encodes only the *ordering*
    dependency, not the *data-flow* dependency. A future implementor
    debugging "why is GI bounce light not visible at end-of-Batch-5"
    needs the *data-flow* WHY, not the ordering WHAT. Decision 3
    corrects this.

13. **Auto-mode behaviour for this dispatch.** Per the `Auto Mode
    Active` system reminder, biased toward making reasonable calls
    without stopping. The 5 load-bearing decisions in this revision
    are each one I'd surface to the user via `AskUserQuestion` in a
    direct interaction; in delegated-architect mode I'm making the
    calls + documenting the trade-off transparency in
    Decisions & rejected alternatives + the flip-conditions. The
    orchestrator's synthesis pause is where these should be reviewed
    if any look wrong.
