# Item 1 — D5 Step 4: prepare_construction split

Investigator: investigator-item-1 (codebase-tightening-followup, 2026-05-21).
HEAD verified against: `2bb03d1` (D4 final cleanup).
Status: read-only; no source edits, builds, or tests run.

## Bailing implementor's stated blocker

The orchestration brief says "two implementors deferred-bailed on D5 Step 4". A
close re-read of `docs/orchestrate/codebase-tightening/gpu-construction/04-refactoring.md`
shows only **one** of the two deferrals contains a 5-coupling-gap analysis:

- The **first** deferral (impl log `:297-329`, headed "Step 4 — Split
  `prepare_construction` per workstream — **DEFERRED**") is a pure
  scope/budget bail: "pure structural re-distribution — bytes move from
  `mod.rs` into the 5 workstream submodules with **zero LOC reduction**" and
  "Step 4's blast radius is the largest in the design". No couplings are
  enumerated; no architect-spec gap is named. The bail is rationalised by
  the architect's headline-win escape hatch (Steps 1 + 5 alone = ~5 700 LOC).
- The **second** deferral (impl log `:1020-1109`, headed "Step 4 — Split
  `prepare_construction` per workstream — **DEFERRED (second time)**") is the
  one with the 5-coupling-gap claim. Quoted verbatim from `04-refactoring.md:1027-1080`:

> Confirmed cross-workstream couplings in the 1357-LOC monolith
> (`mod.rs:727-2055`):
>
> 1. **`want_gpu_producer` (computed at `mod.rs:805-810`)** is consumed by:
>    - W1 block (W1 buffer allocation gate) — line 811
>    - W5 block (skip dense-allocation when model_data is present) — line 923
>    - W3 block (`bounds_initialized` first-frame seed gate) — line 1396
>    - W4 block (the `let _ = (world_data_meta, want_gpu_producer)` at line 2054)
>    - The split needs each workstream to re-derive it, or one workstream to
>      write it into a shared resource. **Architect's §2.1 design does not
>      specify which.** The cheap fix is each system re-derives from
>      `construction_config + world_data_meta + model_data` — but the
>      re-derivations need to agree byte-for-byte, and one slipped condition
>      produces a subtly-broken producer-vs-placeholder allocation race.
>
> 2. **W2's body allocates W1 placeholders inline** (`mod.rs:1632-1676` for
>    `block_voxel_count`, `segment_voxel_buffer`, `hash_map`,
>    `hash_coefficients`). Per architect §2.1, these belong to W1's
>    `prepare_chunk_calc`. **The architect's design moves them but doesn't
>    address the W2-runs-when-W1-is-absent fallback** — currently W2's body
>    defensively allocates them on the legacy code path (when
>    `want_gpu_producer = false` AND no W1 buffers exist yet). After the
>    split: if `prepare_chunk_calc` runs first BUT doesn't allocate the
>    placeholders (because `want_gpu_producer = false`), then
>    `prepare_world_change`'s `construction_world` bind-group build needs
>    placeholders that don't exist. Either prepare_chunk_calc unconditionally
>    allocates placeholders (matching architect's design intent — "everything
>    the W1 buffer family needs"), or the shared bind-group builder allocates
>    them. The architect's spec is ambiguous here.
>
> 3. **First-frame `bounds_initialized` dispatch in the W3 section
>    (`mod.rs:1393-1422`)** runs `bounds_calc::dispatch_add_initial_groups`
>    inline. This `return;`s on missing pipeline / missing bind-groups. After
>    the split into `prepare_bounds_calc`, this dispatch becomes per-workstream
>    — works fine, but the ordering vs the W1's bind-group build matters
>    (the dispatch needs `construction_bounds_world` + `construction_bounds`
>    bind groups, which Step 4's `prepare_shared_bind_groups` builds AFTER
>    per-workstream prepares). **The architect's `.after()` chain handles
>    this**, but the body has 3-tier `return;` ladders that need to be
>    preserved as `else { return; }` patterns inside the new system.
>
> 4. **`construction_world` bind group is built inside the W2 block**
>    (`mod.rs:1627-1712`) but depends on buffers from W1, W3
>    (`bounds_params_buffer`), and W5 (`segment_voxel_buffer`). Architect's
>    design moves this to `prepare_shared_bind_groups` — clean. But the
>    block ALSO contains the W1-placeholder fallback allocations (point 2),
>    which need to move with the bind-group build or to W1. **Unspecified.**
>
> 5. **W4's world-bind-group REBUILD** (`mod.rs:1977-2030`) — rebuilds
>    `world_gpu.bind_group` (the renderer's world layout) with production
>    W4 entity buffers in place of `prepare_world_gpu` placeholders. This is
>    a `world_gpu: ResMut<WorldGpu>` write — D4-shared mutable state, edited
>    from D5 territory. The architect's design has W4 keep this; OK because
>    it's a one-shot.

(Line numbers in the bailout were captured against the file at impl-log
write-time, with the post-Step-7 mod.rs at 2 211 LOC; HEAD `2bb03d1`'s mod.rs
is 1 920 LOC because a deletion pass landed afterward. The verification
section below re-anchors each coupling to current line numbers.)

The same implementor's `5.5` side note (`04-refactoring.md:1261-1269`) reads:

> The prior implementer deferred Step 4 with sound reasoning. My re-read
> deferred again with additional structural reasoning. **Two implementor
> passes have now identified the same gap.** Recommend the orchestrator
> dispatch a fresh D5 architect pass on Step 4 specifically — re-examining
> the 5 specific cross-workstream coupling questions — before another
> implementor attempt. Alternatively, accept the deviation #b in §5.3 above
> as a "good enough" middle ground (single-file move, future split).

## Verification of the claim

For each of the 5 couplings, I re-anchored the citation against
`crates/bevy_naadf/src/render/construction/mod.rs` (1 920 LOC at HEAD `2bb03d1`)
and adjacent files. Verdicts below.

### Coupling #1 — `want_gpu_producer` shared derivation

**Claim:** computed once, consumed across W1/W3/W4/W5 (4 separate workstreams).

**Verified against source.** `want_gpu_producer` is defined at
`mod.rs:541-542`. Full enumeration of references inside `mod.rs`
(`grep -n "want_gpu_producer"`):

- `mod.rs:541-542` — definition.
- `mod.rs:543` — gate on the runtime-GPU-producer pre-allocation block (the
  architect's §2.1 explicitly assigns this block to `prepare_generator_model`:
  "the runtime producer pre-allocation (currently `mod.rs:1701-1912`) which is
  genuinely W5-shaped").
- `mod.rs:1127` — gate on the W3 first-frame `bounds_initialized` seed
  dispatch inside the bounds-queue block at `mod.rs:1104-1153`.
- `mod.rs:1750` — `let _ = (world_data_meta, want_gpu_producer);` — explicit
  no-op suppression of an unused-variable warning at the function tail. The
  comment `// referenced in node.` flags that the actual consumption happens
  in `producer.rs::naadf_gpu_producer_node`, not here.
- `mod.rs:523` — docblock reference only (not a code-path read).

**Real consumption sites: 2** (the pre-allocation gate at :543 and the W3
seed gate at :1127). Two of the bailout's four claimed consumer blocks are
spurious:

- The "W1 block" claim (impl-log line 811) does not survive — the W1 buffer
  allocations live inside the very pre-allocation block at :543-685 that is
  itself gated by `want_gpu_producer`. There is no separate W1 read of the
  variable.
- The "W5 block" claim (impl-log line 923) is wrong — the W5 generator-model
  block at `mod.rs:954-1102` gates on `model_data.as_deref().is_some()` (verified
  at `:979`), not on `want_gpu_producer`. The local `model_data_present`
  boolean at `:540` is what `want_gpu_producer` is derived FROM; the W5 block
  reads `model_data` directly.
- The "W4 block" claim is the line-:1750 no-op silencer — not a meaningful
  consumer.

So the architect's design lands at: `prepare_generator_model` re-derives
`want_gpu_producer` (or equivalent gating logic) for the pre-allocation block;
`prepare_bounds_calc` re-derives it for the first-frame seed. Two re-derivation
sites, not four. The audit's suggested fix — a `pub fn want_gpu_producer(&self,
world_data_meta: Option<&WorldDataMeta>, model_data: Option<&ModelDataRender>)
-> bool` method on `ConstructionConfig` at `crates/bevy_naadf/src/render/construction/config.rs`
(currently no such method; struct ends at `:134` and the `Default`/`From<&AppArgs>`
impl blocks are the only impls present) — is 5-LOC, single-callsite-per-system,
and closes the coupling cleanly.

**Verdict: partially real.** Real coupling but smaller than reported (2 sites,
not 4). The "byte-for-byte agreement risk" the bailout cites collapses to
a single helper method on `ConstructionConfig` — implementor-fixable with one
new method, not architect-spec-blocking.

### Coupling #2 — W2 defensively allocates W1 placeholders

**Claim:** W2's body (`mod.rs:1632-1676`, current `mod.rs:1358-1408`) allocates
W1 placeholders for `block_voxel_count`, `segment_voxel_buffer`, `hash_map`,
`hash_coefficients`. After the split, if `prepare_chunk_calc` doesn't allocate
them (because `want_gpu_producer = false`), `prepare_world_change`'s bind-group
build needs placeholders that don't exist.

**Verified against source.** At `mod.rs:1358-1408`, inside the
`if bind_groups.construction_world.is_none()` block, each of the four
buffers is conditionally allocated via `if gpu.<field>.is_none()` guards
followed by a small placeholder buffer creation (sizes: 8 B / 4 B / 16 B / 4 B).
The bind group at `:1409-1443` then assembles using these (plus `world_gpu`
chunks/blocks/voxels + the W3 `bounds_params_buffer` for the params slot).

The W5 generator-model block at `mod.rs:1015-1028` (the `if segment_needs_realloc`
branch) ALSO allocates `segment_voxel_buffer` at the production 128 MiB extent
when `model_data` is present — and explicitly invalidates
`bind_groups.construction_world = None` so the W2 block's bind-group rebuild
re-runs with the real buffer. The `gpu.segment_voxel_buffer.as_ref().map(|b|
b.size()).unwrap_or(0) <= 4` check at `:654` in the W1 pre-allocation block
also gates on absence/placeholder-only state. **Three independent code paths
write to the same `gpu.<field>` slots**, coordinated only by the `is_none()` /
`size() <= 4` discipline.

**This IS a real architecture smell.** The bailout's gap-claim is correct:
the architect's §2.1 says these placeholders "belong to W1's
`prepare_chunk_calc`" but does not specify behaviour when
`want_gpu_producer = false` (the legacy CPU path). Options:

- **(a)** `prepare_chunk_calc` allocates the placeholders unconditionally
  regardless of `want_gpu_producer`. Architect's intent reading.
- **(b)** `prepare_shared_bind_groups` allocates them at bind-group-build time
  if the slots are still None. Today's behaviour, relocated.
- **(c)** Make the placeholders compile-time-static (a single global 16 B
  zero buffer reused everywhere) since they're literally never read by W2's
  shader.

The architect's text (§2.1's "**Behavioural delta**" paragraph) gestures at
the issue with "on a frame where, say, W3 is ready but W1 hasn't received its
`WorldGpu` yet, W3 still runs" but never resolves the placeholder-ownership
question.

**Verdict: real and architect-fixable.** One paragraph in §2.1 specifying
"`prepare_chunk_calc` allocates the W1 buffer family unconditionally on its
first invocation; size depends on `want_gpu_producer` (production vs placeholder)
but presence is unconditional" would close this. ~3-LOC implementor change once
specified.

### Coupling #3 — first-frame `bounds_initialized` seed bails

**Claim:** the bounds-init seed dispatch in W3 has "3-tier `return;` ladders"
that must be preserved as `else { return; }` inside the new system.

**Verified against source.** At `mod.rs:1124-1153`, the seed block:

```rust
if construction_config.gpu_construction_enabled
    && bound_group_count > 0
    && !gpu.bounds_initialized
    && (!want_gpu_producer || gpu.gpu_producer_has_run)
{
    let Some(initial_pipeline) = pipeline_cache
        .get_compute_pipeline(pipelines.bounds_calc_pipeline_add_initial)
    else { return; };
    let (Some(world_bg), Some(bounds_bg)) = (
        bind_groups.construction_bounds_world.as_ref(),
        bind_groups.construction_bounds.as_ref(),
    ) else { return; };
    /* dispatch + submit + flip gpu.bounds_initialized */
}
```

Two `let-else` bails inside the seed-block, not three. The current monolith's
`return;`s exit the WHOLE `prepare_construction` function — skipping W2 / W4
work below. **Post-split each per-workstream prepare's `return;` only skips
that workstream's body, which is the correct semantic** (the W3 prepare's
bail does not need to stop W2 or W4 from running). The architect's `.after()`
chain already guarantees ordering.

The harder problem the bailout doesn't explicitly call out: the seed reads
`construction_bounds_world` and `construction_bounds` bind groups, which the
architect's `prepare_shared_bind_groups` builds AFTER all per-workstream
prepares. **On the seed frame, the per-workstream `prepare_bounds_calc` would
see those bind groups absent and bail; the seed would land one frame later
than today.** Today's monolith builds them in the W3 block at `mod.rs:687-1102`
before reaching the seed at :1124. The architect's split moves bind-group
building to `prepare_shared_bind_groups` (runs after `prepare_bounds_calc`),
so the seed dispatch in `prepare_bounds_calc` runs BEFORE the bind groups
exist on the inaugural frame.

**Verdict: real and architect-fixable.** Either (a) keep the bind-group
build for `construction_bounds_world` + `construction_bounds` in
`prepare_bounds_calc` (don't shove them into `prepare_shared_bind_groups`), or
(b) move the seed dispatch to a separate system ordered after
`prepare_shared_bind_groups`, or (c) accept a one-frame delay on inaugural seed
(no e2e gate fails — first-frame chunks-mirror is empty anyway). Architect
should pick one in §2.1.

### Coupling #4 — `construction_world` bind group lives in W2 block

**Claim:** the bind group depends on W1 + W3 + W5 buffers; the W2 block that
builds it ALSO holds the W1-placeholder fallback. Both must move together to
`prepare_shared_bind_groups`, but the architect doesn't say where placeholders
go.

**Verified against source.** The `construction_world` bind group build is at
`mod.rs:1358-1444` (inside the section the comment at :1155 labels W2; the
bind group itself is "W1's 8-binding `@group(0)`" per the comment at :1351).
The bindings it pulls together:

- `world_gpu.chunks_buffer` (D4-owned via `WorldGpu`).
- `world_gpu.blocks.buffer()` (D4).
- `world_gpu.voxels.buffer()` (D4).
- `bvc` = `gpu.block_voxel_count` (W1 — but allocated by the W2 placeholder block
  immediately above, or by the W1 pre-allocation block at :615-631).
- `segv` = `gpu.segment_voxel_buffer` (W1/W5 — written by 3 sites: W1
  pre-allocation :654, W2 placeholder :1381, W5 generator-model :1015).
- `hmap` = `gpu.hash_map` (W1 — written by W1 pre-allocation :555 or W2
  placeholder :1390).
- `params` = `gpu.bounds_params_buffer` (W3 — written elsewhere in the bounds
  block).
- `coeffs` = `gpu.hash_coefficients` (W1 — written by W1 pre-allocation :587
  or W2 placeholder :1399).

This bind group genuinely IS cross-workstream (W1+W3+W5 buffer dependencies),
and the placeholder-fallback coupling to W2 is real. **The architect's
`prepare_shared_bind_groups` design IS the right shape for the bind-group
build itself** — but the placeholder coexistence question is unresolved.

The simplest resolution: combine couplings #2 and #4. If `prepare_chunk_calc`
owns all W1-family allocations (production-sized when `want_gpu_producer`, tiny
placeholders otherwise), then `prepare_shared_bind_groups` reads from
`gpu.<W1-fields>` confidently. The W5 generator-model `segment_voxel_buffer`
reallocation stays in `prepare_generator_model` (with its existing
`bind_groups.construction_world = None` invalidation).

**Verdict: real and architect-fixable.** Closed by the same architect-spec
revision that closes coupling #2.

### Coupling #5 — W4 rebuilds `world_gpu.bind_group`

**Claim:** W4 mutates D4-owned `WorldGpu::bind_group` (cross-domain mutable
state), but it's a one-shot so OK.

**Verified against source.** At `mod.rs:1708-1726`, inside the W4 block:

```rust
if !gpu.world_bind_group_has_entities {
    if let (Some(eci_rw), Some(evd), Some(eih_rw)) = (/* … */) {
        let rebuilt = crate::render::prepare::rebuild_world_bind_group_with_entities(
            &render_device, &pipeline_cache, &pipelines, &world_gpu,
            eci_rw, evd, eih_rw,
        );
        world_gpu.bind_group = rebuilt;
        gpu.world_bind_group_has_entities = true;
    }
}
```

The cross-domain rebuild goes through the named helper
`rebuild_world_bind_group_with_entities` at
`crates/bevy_naadf/src/render/prepare/world.rs:590` (re-exported as
`pub(crate) use world::rebuild_world_bind_group_with_entities;` at
`render/prepare/mod.rs:47`). Guarded once by `gpu.world_bind_group_has_entities`.
The audit's claim "coupling #5 is *already* resolved — the named helper is
grep-able" is correct.

**Verdict: not a real coupling.** Already resolved. The bailing implementor
even concedes "OK because it's a one-shot." The one D4-shared `ResMut<WorldGpu>`
parameter on `prepare_entity_update` is the only seam, and it's clean. ~0
architect work needed.

### Cross-coupling audit

The architect's §2.1 already specifies:

- `RenderSystems::PrepareResources` set + `.after(prepare_world_gpu)` —
  verified at `mod.rs:1887-1888` exactly as the audit cites.
- `.run_if(resource_exists::<_>)` idiom — verified in use at
  `mod.rs:1904, :1907` on `populate_cpu_mirror_from_gpu_producer` (Step 6
  follow-up landed). Lifts verbatim to each per-workstream prepare.
- Each workstream submodule (`chunk_calc.rs`, `bounds_calc.rs`,
  `world_change.rs`, `entity_update.rs`, `generator_model.rs`,
  `producer.rs`) already owns its layout descriptors + dispatch helpers
  + node fn (verified by `grep '^pub fn' <each-file>` — see investigation
  scratch). NONE owns a `prepare_*` system yet.

The structural infrastructure for the split is in place. The only unresolved
items are couplings #2/#3/#4 (architect-spec ambiguities) plus coupling #1
(a cheap helper-method choice).

## Diagnosis

The bailout is a **mixed bag — three of five claims real and architect-fixable
(closeable by ~1-2 paragraphs in §2.1), one partially real and implementor-
fixable (a `ConstructionConfig` helper-method), one already resolved.**

| coupling | real? | category | proximate fix |
|---|---|---|---|
| #1 `want_gpu_producer` derivation | partially — 2 sites, not 4 | (b) implementor-fixable | 5-LOC `pub fn want_gpu_producer(&self, ...) -> bool` on `ConstructionConfig` (`render/construction/config.rs`) — re-derive at each callsite. |
| #2 W2 placeholder fallback | yes | (a) architect-fixable | Spec one paragraph in §2.1 specifying placeholder ownership (recommend: `prepare_chunk_calc` allocates W1-family unconditionally; size = production-or-placeholder). |
| #3 bounds-init seed ordering | yes | (a) architect-fixable | Spec which of (i) seed-in-prepare-bounds-calc + keep bind-group-build there, (ii) seed-as-separate-system after `prepare_shared_bind_groups`, (iii) accept one-frame seed delay. |
| #4 `construction_world` bind group cross-workstream | yes | (a) architect-fixable — closed by same revision as #2 | The bind group itself goes to `prepare_shared_bind_groups`; placeholders go to W1 per coupling #2's resolution. |
| #5 W4 `world_gpu.bind_group` rebuild | no | (c) not a real coupling | Resolved by the existing `rebuild_world_bind_group_with_entities` helper at `render/prepare/world.rs:590`. |

The **real blocker** is not implementor incompetence and not architect
incompetence — it's a 1-paragraph spec gap in §2.1 that two implementors,
both Opus, **correctly refused to silently resolve** (per the binding rule
"if a step is underspecified, stop and log the gap"). The first implementor's
deferral was budget-shaped; the second's was structural and surfaced the
real spec gap. The second bailout's structural analysis is **mostly correct
but somewhat inflated** — 2 of 5 couplings are weaker than reported. The
behavioural risk the second implementor cited ("non-deterministic-by-ordering
producer-vs-placeholder allocation race") is genuine but localised to the
placeholder-ownership question (coupling #2/#4) and the seed-ordering question
(coupling #3), not pervasive across the split.

There is no foundation rot. `mod.rs` is in a state where the structural pieces
needed for the split (workstream submodules already own their layout/dispatch
helpers; `.run_if` idiom in use; `.after(prepare_world_gpu)` ordering edge
proven) are all present. The split is a 200-300-LOC mechanical move once the
two architect-spec gaps close.

## Proposed path forward

**Pick (a): fresh `delegate-architect` dispatch with a focused brief.**

The brief should target exactly the three open spec questions:

1. **Placeholder ownership** (couplings #2 + #4 — same fix). Spec which
   prepare system unconditionally allocates the W1 buffer family
   (`block_voxel_count`, `segment_voxel_buffer`, `hash_map`,
   `hash_coefficients`) and at what size when `want_gpu_producer = false`.
2. **Inaugural-frame seed ordering** (coupling #3). Pick one of: (i) keep
   the seed in `prepare_bounds_calc` and keep the `construction_bounds_world`
   + `construction_bounds` bind-group build there too; (ii) move the seed
   to its own system ordered after `prepare_shared_bind_groups`; (iii) accept
   a one-frame seed delay and document the visible-behaviour invariance.
3. **`want_gpu_producer` shared derivation** (coupling #1). Decide between
   the audit-recommended `pub fn want_gpu_producer` on `ConstructionConfig`
   or per-system re-derivation. The fn-on-config approach is recommended
   (single source of truth, no drift risk).

The brief should NOT re-architect the whole §2.1 — the per-workstream-prepare
shape is sound. It should produce a focused **§2.1-revision addendum** of
≤500 words that an implementor can lift directly. Justification: (a) re-running
the same brief that bailed twice violates the orchestration's forbidden moves;
(b) the spec gap is narrow enough for an architect agent to close in one pass;
(c) the implementor side is then a mechanical 200-300-LOC move with deterministic
verification (full e2e gate sweep + `--oasis-edit-visual ×3`).

Alternative (rejected): re-dispatch implementor with a "freelance the
placeholder ownership however you see fit" brief. Two Opus instances have
explicitly refused this; the third would either bail identically or freelance
in incompatible directions (e.g. one might pick prepare_chunk_calc, another
prepare_shared_bind_groups). Architect-first closes it once for all attempts.

Alternative (rejected): the second implementor's "5.3.b workaround" (single-file
extraction without the per-workstream split). Drops ~1300 LOC from mod.rs with
zero coupling risk, but trades the architect's design goal for ergonomic LOC
reduction. Worth keeping in mind as a fallback if architect-revision dispatch
also bails; not the first-line recommendation.

## Verification recipe

The split is structural; verification is byte-equality against the CPU oracle
plus visual stability. Exact commands (per the orchestration's
forbidden-moves discipline: do NOT run `cargo run --bin bevy-naadf`):

**Baseline (capture once, against current HEAD `2bb03d1`, before any change):**

```bash
cargo build --workspace
cargo test --workspace --lib   # expect: 179 passed, 1 ignored
cargo run --bin e2e_render -- --validate-gpu-construction
cargo run --bin e2e_render -- --validate-gpu-construction-scaled
cargo run --bin e2e_render -- --validate-gpu-construction-production-scale
cargo run --bin e2e_render -- --edit-mode
cargo run --bin e2e_render -- --runtime-edit-mode
cargo run --bin e2e_render -- --entities
cargo run --bin e2e_render -- --vox-e2e
# Non-deterministic — capture variance band:
for i in 1 2 3; do cargo run --bin e2e_render -- --oasis-edit-visual; done
```

Record each gate's exit code and (for `--oasis-edit-visual`) the Δ-luminance
the run prints. Current baseline per `04-refactoring.md:1142`: Δ band 14.6-15.4,
floor 8.00.

**Post-change (after each implementor commit; the impl log called out
~3-4 hours for the full 5-way split — split into commits so verification
runs at each):**

Same commands, identical exit codes expected. For `--oasis-edit-visual`:
**≥3 runs** on the post-change side and **≥2 runs** on the
HEAD-`2bb03d1` baseline side (per the memory
`feedback-multiple-runs-rule-out-false-positives`). The Δ-luminance band
must stay within ~10% of baseline.

**Unit-test spine** (the mod-tests-w1/mod-tests-w4/mod-tests inside
`crates/bevy_naadf/src/render/construction/validation.rs:4731, :5026, :5818`
plus `bounds_calc/tests.rs`): these stress dispatch helpers (not the
`prepare_*` systems) and must continue passing byte-equally against the CPU
oracle. They are independent of the split shape — the split touches only
`prepare_*` systems, leaving dispatch + layout-descriptor code untouched.
Verify with `cargo test --workspace --lib`.

**Wrap each `cargo run --bin e2e_render -- ...` invocation in `timeout 120s`**
per memory `feedback-e2e-gates-must-fail-fast`.

**Special-case verification for coupling #1 (`want_gpu_producer` re-derivation):**
add one new `#[test]` to `render/construction/config.rs::tests` (currently no
test module there) that constructs `(ConstructionConfig, Option<WorldDataMeta>,
Option<ModelDataRender>)` over the cross product of `{enabled, disabled} ×
{dense-data, no-dense-data} × {model-data, no-model-data}` and asserts the
8 outputs match the truth table baked into `mod.rs:537-542`. ~30 LOC. Pins
the helper-method behaviour so a future drift surfaces as a test failure,
not a producer-vs-placeholder race in production.

**Special-case verification for coupling #3 (seed ordering):** the
`--validate-gpu-construction*` family is byte-equal-against-CPU-oracle. A
one-frame seed delay (if the architect picks resolution (iii)) would surface
as a different first-frame state in those gates if the first frame is the
sample frame — verify the validate gates assert their byte-equality on a frame
≥2, not frame 1. (Read `validation.rs:4727` family entry point to confirm.)

## Side notes / observations / complaints

- **The brief's "both bailouts" framing is incorrect.** Only the second
  deferral (`04-refactoring.md:1020-1109`) contains a 5-coupling-gap analysis;
  the first (`:297-329`) is pure budget/scope rationale. The orchestrator-side
  audit (`00-reuse-audit.md` item 1) cites this correctly; the per-investigator
  brief I received says "two implementors deferred-bailed on D5 Step 4 (a
  `prepare_construction` split), both citing 5 cross-workstream coupling
  gaps", which is not what the impl log actually shows. **Recommend the
  orchestrator double-check upstream briefs for this kind of paraphrase
  drift.**

- **Two of the five claimed couplings are weaker than the bailout asserts.**
  Coupling #1 has 2 real consumers, not 4 — the W1 / W5 / W4 claims don't
  survive a grep (W1's "consumer" IS the pre-allocation block that defines
  `want_gpu_producer`'s purpose; W5 reads `model_data` directly; W4 has a
  `let _ = ...` no-op silencer). The second implementor may have been
  pattern-matching the architect's W1/W3/W4/W5 section divider grammar
  rather than verifying actual variable reads. This pattern of "inflate the
  coupling count by section-divider arithmetic" is worth flagging — the audit
  also cites "consumed across W1/W3/W4/W5" without verifying each, so the
  inflation propagated.

- **The audit's coupling-#5 verdict ("already resolved by
  `rebuild_world_bind_group_with_entities`") is correct.** The named helper
  exists at `render/prepare/world.rs:590` with a `pub(crate)` re-export at
  `render/prepare/mod.rs:47`. The second implementor's "OK because it's a
  one-shot" concedes the same thing. So the bailout listed 5 couplings of
  which the implementor self-deflated 1; 1 of the remaining 4 is partially
  spurious (the W1/W5/W4 inflation in #1). The real architect-blocking
  surface is **2 spec gaps** (couplings #2/#4 as one fix, plus coupling
  #3), not 5.

- **The first implementor's budget-bail (`:304-318`) was diagnostically
  weaker than the second's.** It cited "zero LOC reduction" + "blast radius
  + merge-conflict risk against in-flight D4 work" as the deferral
  rationale, with no architectural analysis. The orchestration's
  re-dispatch decision should have been "send the second implementor with
  the structural-questions brief, not a duplicate of the first's brief."
  The orchestrator's choice to send a same-shape second dispatch is what
  produced the second bail with the same gap.

- **Foundation is sound; no rot worth flagging.** The construction module
  is in a clean place: validation/test fixtures already extracted
  (`validation.rs:1-6256`), production encoder moved
  (`chunk_calc.rs:337`), readback split off (`readback.rs:1-628`), SSoT-6
  closed (`hashing.rs:30-50` re-exports D1's
  `aadf::block_hash::hash_coefficients`), W2 partial probe deletion done
  (Step 6 partial). The deferred Step 4 is the last large structural item
  in mod.rs; closing it lands the module at ~620 LOC per the architect's
  end-state projection.

- **`prepare_construction` retaining 11 system parameters is itself a soft
  smell** (`mod.rs:459-481`). Bevy's idiomatic limit is 8 before
  `#[allow(clippy::too_many_arguments)]` is required; the architect's split
  drops each per-workstream prepare to ~5-6 parameters. The split's main
  value is **legibility** (per-workstream prepares are individually
  reasonable to read), not LOC. The bailing implementors' "zero LOC
  reduction" framing undersells the readability win.

- **I felt the brief's "both bailouts' verbatim 5-coupling-gap analysis"
  push me toward treating the deferrals as a unified front; on inspection
  they are not.** The first bail is "I am out of budget, this is structural-
  only"; the second is "I read the source carefully and found 5 specific
  spec gaps." The brief is asking the right question (verify the second
  bail's structural claim against source), but the framing of "two
  implementors saw the same gap" obscures the real signal — only one
  implementor surfaced architectural gaps; the other deferred for
  unrelated reasons. If the orchestration insists "two bails = strong
  signal that gap is real", the actual signal is closer to "one bail
  surfaced 3 real gaps + 1 spurious + 1 already-resolved; the other bail
  was tangential."

- **The audit's recommended fix for coupling #1** (`pub fn want_gpu_producer`
  on `ConstructionConfig`) is sound and small. I considered whether the
  helper should live somewhere else (e.g. a free function in
  `render/construction/mod.rs`) and concluded `config.rs` is the right home
  — the `From<&AppArgs>` impl is already there, the struct fields are read
  off `&self`, and the two callers (`prepare_generator_model`,
  `prepare_bounds_calc`) both already take `Res<ConstructionConfig>`. No
  reason to deviate from the audit on this point.

- **No need to bundle Item 1 with another item.** The audit's cross-item
  observation #2 ("`ConstructionPlugin::build` is reuse template for Items
  1 + 4") is correct but doesn't force bundling — Item 4 is a 9-plugin
  decomposition, much larger and orthogonal in scope. The two should not
  share a dispatch; the architect-revision brief for Item 1 is self-contained
  and should land first.
