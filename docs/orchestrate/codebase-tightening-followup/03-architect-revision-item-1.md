# Item 1 architect revision — D5 §2.1 addendum

Author: delegate-architect (codebase-tightening-followup, 2026-05-21).
HEAD verified against: `2bb03d1` (D4 final cleanup).
Scope: closes the 3 open spec questions the item-1 investigator surfaced
(`02-investigation-item-1-d5-step-4.md`). Does NOT re-architect §2.1's
per-workstream-prepare shape — that shape is sound and unchanged.

---

## Original §2.1 context

§2.1 of `docs/orchestrate/codebase-tightening/gpu-construction/03-architecture.md`
specifies the split of the 1418-LOC `prepare_construction` monolith
(`render/construction/mod.rs:459` at HEAD `2bb03d1`) into six sibling
systems registered under `RenderSystems::PrepareResources`, each
`.after(prepare_world_gpu)`: `prepare_construction_resources` (the
ensure-init shell), `prepare_chunk_calc` (W1), `prepare_bounds_calc`
(W3), `prepare_world_change` (W2), `prepare_generator_model` (W5),
`prepare_entity_update` (W4), plus a trailing `prepare_shared_bind_groups`
that wires the `construction_world` and `construction_bounds_world` bind
groups after every per-workstream prepare has populated `gpu.<…>` slots.

§2.1's "Behavioural delta" paragraph (`03-architecture.md:186-204`)
acknowledges that per-workstream short-circuits *replace* the monolith's
function-scope `let-else { return; }` ladders, and asserts "this matches
the W0 seam's original design intent and is not observable in any current
test". What §2.1 does **not** spell out: (a) which prepare owns the
defensive W1-family placeholders that the current monolith allocates
inside the W2 `construction_world` build (`mod.rs:1358-1408`); (b) where
the inaugural-frame `bounds_initialized` seed dispatch lands once the
`construction_bounds_world` + `construction_bounds` bind-group build
moves to `prepare_shared_bind_groups` (an ordering crossover the
monolith does not face); (c) whether `want_gpu_producer` is re-derived
per system or held in a shared resource. The investigator's per-coupling
verification (`02-investigation-item-1-d5-step-4.md`) confirmed all three
gaps are real and architect-fixable, with the rebuild-world-bind-group
helper coupling (#5) already resolved by
`render/prepare/world.rs:590::rebuild_world_bind_group_with_entities`
(`render/prepare/mod.rs:47` `pub(crate)` re-export).

---

## §2.1 addendum (the deliverable)

The following addendum lifts verbatim into §2.1, appended after the
existing "Behavioural delta" paragraph (`03-architecture.md:204`) under a
new sub-heading.

### 2.1.1 Placeholder ownership, seed ordering, and `want_gpu_producer` derivation

Three behavioural questions §2.1's per-system split leaves implicit. All
three resolve declaratively — no behavioural divergence from the monolith.

**Rule 1 — W1-family placeholder ownership.**
`prepare_chunk_calc` (W1) is the **sole writer** of the four W1-family
slots `gpu.{block_voxel_count, segment_voxel_buffer, hash_map,
hash_coefficients}`. It allocates them **unconditionally on its first
invocation** (when each slot is `None`), regardless of
`want_gpu_producer`. Size depends on `want_gpu_producer`: when `true`,
the production sizes from the current pre-allocation block at
`mod.rs:543-685` (hash_map = `initial_hash_map_size * 16` B,
hash_coefficients = `65 * 4` B, block_voxel_count = 8 B,
segment_voxel_buffer = dense-derived); when `false`, the placeholder
sizes from the current W2 block at `mod.rs:1363-1407` (hash_map = 16 B,
hash_coefficients = 4 B, block_voxel_count = 8 B, segment_voxel_buffer =
4 B). The W5 `prepare_generator_model` reallocation of
`segment_voxel_buffer` at 128 MiB (`mod.rs:1015-1028`) stays — gated by
`model_data.is_some()` and unchanged, with its existing
`bind_groups.construction_world = None` invalidation. The W2-inline
placeholder block at `mod.rs:1358-1408` is **deleted** in the split;
`prepare_shared_bind_groups` builds `construction_world` reading
`gpu.<W1-fields>` confidently. **Rationale:** the investigator's
verification (`02-investigation-item-1-d5-step-4.md` §"Coupling #2") shows
three independent code paths today write to the same `gpu.<W1-field>`
slots, coordinated by `is_none()` / `size() <= 4` discipline. Collapsing
to one writer eliminates the race. **Lift-into-code:** in
`prepare_chunk_calc`, gate each `gpu.<field>.is_none()` allocation on a
single `if want_gpu_producer { production-size } else { placeholder-size
}` branch; delete `mod.rs:1358-1408`; verify
`gpu.<W1-fields>.is_some()` as a precondition on every read in
`prepare_shared_bind_groups`.

**Rule 2 — inaugural-frame seed ordering.** Pick option (i): keep the
seed dispatch at the tail of `prepare_bounds_calc` AND keep the
`construction_bounds_world` + `construction_bounds` bind-group build
inside `prepare_bounds_calc` (NOT in `prepare_shared_bind_groups`).
Reject (ii) one-shot-seed-system (additional scheduler edge for a
once-per-app event is gold-plating) and (iii) one-frame seed delay
(deterministic byte-equality gates assert from frame ≥2 today, but
shifting the seed by one frame is observable behavioural divergence
from the C# port; faithful-port rule binds). `prepare_shared_bind_groups`
retains the `construction_world` build only. **Rationale:** the seed
reads `construction_bounds_world` and `construction_bounds` — both
narrow to W3 — and is itself a W3 event; co-locating it with its
dependencies preserves byte-identical scheduling against the monolith.
**Lift-into-code:** move the bind-group build at `mod.rs:896-953` and
the seed block at `mod.rs:1124-1153` together into `prepare_bounds_calc`;
preserve the two `let-else { return; }` bails inside the seed block
verbatim (per-system `return;` correctly skips only that workstream).

**Rule 3 — `want_gpu_producer` shared derivation.** Add
`pub fn want_gpu_producer(&self, world_data_meta: Option<&WorldDataMeta>,
model_data: Option<&ModelDataRender>) -> bool` to `ConstructionConfig` in
`render/construction/config.rs` (mirroring the existing
`From<&AppArgs>` impl at `:252-`). Body byte-equal to the current
derivation at `mod.rs:537-542`. The two real consumers
(`prepare_chunk_calc` for placeholder-size selection per Rule 1;
`prepare_bounds_calc` for the seed gate at the bottom of the system)
both already take `Res<ConstructionConfig>` and need
`Option<Res<WorldDataMeta>>` + `Option<Res<ModelDataRender>>` as system
parameters. Reject per-system re-derivation: byte-for-byte agreement
risk is real and the helper is 5 LOC. **Lift-into-code:** add the
method on `ConstructionConfig`; call as
`construction_config.want_gpu_producer(world_data_meta.as_deref(),
model_data.as_deref())` at each of the two callsites.

---

## Decisions & rejected alternatives

For each of the three decisions in the addendum, the alternative(s)
considered and the rejection rationale. Implementor reads this to
understand the design's boundaries.

**Decision 1 — placeholder ownership lands in `prepare_chunk_calc`,
production-or-placeholder size.**

- **Adopted:** investigator's recommended path.
- **Alternative A rejected:** placeholders allocated by
  `prepare_shared_bind_groups` lazily at bind-group-build time.
  Rejected — adds a second writer to `gpu.<W1-fields>`, recreating
  exactly the race-by-`is_none()`-discipline the rule eliminates. Also
  smells: bind-group builder allocating buffers is an IoC violation.
- **Alternative B rejected:** compile-time-static 16-byte zero buffer
  reused across all W2 placeholder bindings. Theoretically valid (W2's
  shader never reads them) but requires inventing a static `Arc<Buffer>`
  registry and breaks the symmetry with the production-path buffers
  (which ARE read).  Investigator flagged as overscope (§"Coupling #2"
  option (c)).
- **Flip condition:** if a future audit finds W2's shader DOES read any
  of the four bindings — Rule 1's "placeholder when `want_gpu_producer
  = false`" path produces wrong results and Alternative A becomes
  forced.

**Decision 2 — seed + bounds bind groups stay in `prepare_bounds_calc`
(option i).**

- **Adopted:** investigator's option (i) — local-to-W3 ordering.
- **Alternative B (option ii) rejected:** separate seed system ordered
  after `prepare_shared_bind_groups`. Adds one scheduler edge for a
  once-per-app event; the seed body already short-circuits cleanly with
  `let-else { return; }` after the bind-group prerequisites exist.
- **Alternative C (option iii) rejected:** accept one-frame seed delay,
  document invariance. Behavioural divergence from C#
  `WorldBoundHandler.cs:53-89` (initialises before any compute pass).
  Faithful-port rule binds — the divergence requires explicit user
  approval + docs entry per `bevy-naadf-faithful-port-rule` memory.
- **Flip condition:** if the `construction_bounds_world` bind group
  later acquires non-W3 dependencies (e.g. cross-workstream sampling
  for a future Phase-D feature), it would have to move to
  `prepare_shared_bind_groups`, forcing Alternative B.

**Decision 3 — `want_gpu_producer` helper on `ConstructionConfig`.**

- **Adopted:** investigator's option (a) / audit-recommended path.
- **Alternative B rejected:** per-system re-derivation from
  `(construction_config, world_data_meta, model_data)` inline. The
  byte-for-byte agreement risk the second bailing implementor cited
  (`04-refactoring.md:1038-1040`) is real — Rule 1's behaviour pivots
  on the exact same predicate, and a slipped condition produces a
  silently-broken producer-vs-placeholder allocation race.
- **Alternative C rejected:** stash `want_gpu_producer` into a shared
  `Resource<WantGpuProducer>` written by one system, read by others.
  Adds scheduler-edge management for a 5-LOC `Copy` value; over-engineered.
- **Helper-home alternative considered (and rejected):** free function
  in `render/construction/mod.rs` taking `(&ConstructionConfig,
  Option<&WorldDataMeta>, Option<&ModelDataRender>)`. Rejected because
  `From<&AppArgs>` already lives on `ConstructionConfig` (`config.rs:252-`)
  — the method is a natural member and both callers already take
  `Res<ConstructionConfig>`.
- **Flip condition:** if a future workstream needs `want_gpu_producer`
  derived from a 4th input (e.g. some new `Res<RuntimeMode>`), the
  method-on-`ConstructionConfig` shape forces a signature change at
  every callsite — the free-function alternative would absorb the new
  param more gracefully. Not relevant at HEAD.

---

## Assumptions made

Preconditions the implementor MUST respect. If any is wrong, the
addendum's rules need re-examination.

1. **Coupling #5 stays resolved.** The named helper
   `rebuild_world_bind_group_with_entities` at
   `render/prepare/world.rs:590` (with `pub(crate)` re-export at
   `render/prepare/mod.rs:47`) is unchanged. The W4
   `prepare_entity_update` system call at the current `mod.rs:1714` lifts
   verbatim. The addendum does not touch this seam.

2. **Production-size constants are the current defaults at HEAD `2bb03d1`.**
   Specifically:
   - `initial_hash_map_size = 1 << 20` (`config.rs:157, :301`) — hash_map
     production size is `initial_hash_map_size * 16` = 16 MiB.
   - `hash_coefficients` production size = `65 * 4` = 260 B
     (`mod.rs:591`).
   - `block_voxel_count` production size = 8 B with seed `[64u32, 64u32]`
     (`mod.rs:624-628`).
   - `segment_voxel_buffer` production size = dense-derived cubic-extent
     × 2048 × 4 B per `mod.rs:632-683`; this allocation is gated on
     `!model_data_present` (`mod.rs:655`) because the W5 path
     reallocates at 128 MiB. **The addendum's Rule 1 preserves this
     gate**: when `want_gpu_producer = true && !model_data_present` AND
     `dense_data_ready`, allocate dense-derived; when
     `want_gpu_producer = true && model_data_present`, allocate the
     4 B placeholder (W5 will reallocate). When `want_gpu_producer =
     false`, allocate the 4 B placeholder unconditionally.

3. **Placeholder sizes from the current W2 block are correct.** 16 B
   hash_map (1 slot, `mod.rs:1393`), 4 B hash_coefficients (1 u32,
   `mod.rs:1402`), 8 B block_voxel_count seeded `[64u32, 64u32]`
   (`mod.rs:1366-1378`), 4 B segment_voxel_buffer (`mod.rs:1384`).
   `block_voxel_count`'s placeholder is identical to its production
   buffer (both 8 B with the same seed) — the only "placeholder"
   distinction is the label string.

4. **`WorldDataMeta` and `ModelDataRender` paths.** Defined at
   `render/extract.rs:129` and `:156` respectively. The method signature
   takes `Option<&WorldDataMeta>` + `Option<&ModelDataRender>` matching
   the `world_data_meta.as_deref()` / `model_data.as_deref()` callsite
   shape at `mod.rs:537-542`.

5. **Per-workstream submodules already own dispatch + layout helpers.**
   Verified at `bounds_calc.rs:78-421` (layouts, queue-pipeline helpers,
   `dispatch_add_initial_groups` at `:270`, node fn at `:421`). The
   addendum's split assumes each workstream's new `prepare_*` system
   lives alongside these — same file, same `// === W{N} …` divider
   grammar.

6. **`.run_if(resource_exists::<_>)` idiom is the §2.5 conversion target
   only for the systems D5 owns registration of.** §2.1's addendum does
   NOT propose `.run_if`-converting the existing `Option<Res<…>>` bails
   inside each per-workstream prepare body — that's §2.5's remit (Step
   6, partially landed already per the SSoT-6 follow-up). Bodies of
   per-workstream prepares keep the `Option<Res<…>>` parameter shape
   from the current monolith.

7. **`.after(prepare_world_gpu)` ordering edge stays unmodified.** The
   single ordering edge at the current `mod.rs:1888` transfers to every
   per-workstream prepare. `prepare_shared_bind_groups` adds further
   `.after(…)` edges to every per-workstream prepare per §2.1's
   registration block at `03-architecture.md:136-163`.

---

## Side notes / observations / complaints

- **The addendum's three decisions are all defensive against a class of
  silent-divergence bug that has bitten this codebase before.** The
  bailing-implementor's "byte-for-byte agreement risk" framing for
  coupling #1 (`04-refactoring.md:1038-1040`) is exactly the
  vox-gpu-rewrite W5.3 inversion failure mode documented at
  `mod.rs:520-536` — that fix was a one-condition widen, and the bug
  surfaced as scattered empty holes in the final render, not as a build
  or test failure. The investigator-recommended helper method
  (`pub fn want_gpu_producer`) is cheap insurance against the same
  failure shape.

- **The investigator's "rule 1 collapses three writers to one" framing
  is the real architectural win in this addendum.** Today there are
  three independent code paths writing to `gpu.<W1-fields>` (the W1
  pre-allocation block, the W2 placeholder block, the W5 generator-model
  block), coordinated by `is_none()` + `size() <= 4` discipline. That
  pattern is the source of every cross-workstream coupling in §2.1.
  Once `prepare_chunk_calc` is the sole writer (with `prepare_generator_model`
  as the lone exception for `segment_voxel_buffer` 128 MiB reallocation),
  the bind-group builder reads with confidence and the racy-by-`is_none()`
  pattern disappears. That's the real refactor; the §2.1 split is the
  containment shape that makes it expressible.

- **§2.1 itself is more right than the two bailout implementors
  acknowledged.** Per investigator's verification (`02-investigation-item-1-d5-step-4.md`
  §"Coupling #1" — 2 real consumers, not 4), 2 of the 5 claimed
  couplings are weaker than reported. The implementors pattern-matched
  the architect's W1/W3/W4/W5 section-divider grammar rather than
  verifying actual variable reads. This addendum should not be read as
  vindicating the "architect was wrong" framing — the architect's
  per-workstream split is correct; what was missing was 3 paragraphs
  on placeholder/seed/derivation. That gap is closeable in ≤500 words,
  as required.

- **The faithful-port rule binds Decision 2 harder than the brief
  acknowledges.** Option (iii) (one-frame seed delay) is technically
  invisible to the deterministic byte-equality gates because they
  assert against the CPU oracle at frame ≥2 (verified —
  `validation.rs:141, :503, :834` are entry points; the gates poll for
  steady state). But the C# `WorldBoundHandler.cs:53-89` seeds before
  any compute pass; introducing a one-frame delay is observable
  behavioural divergence. Memory `bevy-naadf-faithful-port-rule` is
  binding: divergence requires explicit user approval + docs entry.
  The brief gave (iii) equal billing with (i)/(ii); it should not have.

- **Concern about `prepare_shared_bind_groups`'s narrowed scope.** After
  Decision 2 lands, `prepare_shared_bind_groups` builds only
  `construction_world` (W1 + W3 `bounds_params_buffer` + W5
  `segment_voxel_buffer` cross-workstream bind group). The
  `construction_bounds_world` + `construction_bounds` bind groups stay
  in `prepare_bounds_calc`; the `construction_change` bind group stays
  in `prepare_world_change` (it's W2-local — no cross-workstream
  dependencies); the W4 entity bind groups stay in `prepare_entity_update`.
  The shared builder is justified by ONE bind group spanning three
  workstreams. Decision-maker (me) considered folding it back into
  `prepare_chunk_calc` (post-Rule-1, every W1-family field is owned by
  W1 — could W1 build the bind group too?) but rejected: the bind
  group reads W3's `bounds_params_buffer`, which is W3 territory.
  Keep the shared builder. **Side observation:** if `bounds_params_buffer`
  were ever to move to `gpu_types::GpuConstructionParams` upload-once
  (a D4 follow-up), `construction_world` collapses to W1-only and
  `prepare_shared_bind_groups` becomes deletable. Worth flagging for
  the architect of the next refactor pass.

- **The brief's structural contract feels right for this item.** Three
  decisions, ≤500 words, lift-into-code instructions per decision — the
  spec gap genuinely is that narrow. No urge to escape the brief shape.

- **The orchestration's "two implementors bailed" framing is misleading
  but recoverable.** Per investigator §"Side notes" first bullet: only
  the second deferral contains the 5-coupling-gap analysis; the first
  is pure budget-scope. The investigator flagged this clearly. The
  addendum's design does not depend on whether one or two implementors
  bailed — the architect-spec gaps are real either way. Worth carrying
  the framing-correction into the implementor's brief so they don't
  spend tool budget on a "verify the gap is real" pass when the
  investigator already did that.

- **No foundation rot.** Mirrors investigator's conclusion. The
  construction module is in a clean place: validation/test fixtures
  extracted, production encoder moved, readback split off, SSoT-6
  closed, W2 partial probe deletion done. Closing Step 4 lands mod.rs
  at the architect's projected ~620 LOC end-state. The per-workstream
  split is mechanical (~200-300 LOC move) once these three rules are
  in place.

- **Implementor's tool-budget projection.** With the addendum in hand,
  the split is: (1) add `want_gpu_producer` method on `ConstructionConfig`
  (~10 LOC, one Edit); (2) extract per-workstream prepares into their
  workstream submodules (~6 Edits across 5 submodules + mod.rs); (3)
  delete the W2 placeholder block and inline-allocations in W1 pre-alloc
  (~3-4 Edits in mod.rs); (4) move the seed + bounds bind-group build
  into `prepare_bounds_calc` (~2 Edits); (5) register the 7 new
  systems with `.after(...)` chain in `ConstructionPlugin::build`
  (~1 Edit). Plus the `want_gpu_producer` unit-test addition the
  investigator's verification recipe specifies. **Estimated 25-35
  tool calls** — well inside the 100-budget that the bailing implementors
  exceeded. The verification suite (build + 179 lib tests + 8 e2e
  gates + 3 `--oasis-edit-visual` runs) is separate and counted
  outside the impl budget per the project's verification discipline.
