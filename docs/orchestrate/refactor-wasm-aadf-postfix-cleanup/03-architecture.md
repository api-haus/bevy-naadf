# 03-architecture — refactor-wasm-aadf-postfix-cleanup

Empty stub. The `refactor-architect` agent writes the target-state
design here under the heading `## refactor-architect findings (<ISO date>)`.

## refactor-architect findings (2026-05-20)

### Findings addressed

All 11 explorer findings in scope, grouped by item:

- Item 1 (`naadf_bounds_compute_node` cleanup) — findings 1A, 1B, 1C, 1D, 1E, 1F, 1G.
- Item 2 (chunks-RMW + 18% parity gap) — findings 2A, 2B, 2C (each classified per user commit-policy).
- Item 3 (`tests.rs` probe-buffer hardcode) — finding 3A.

No findings skipped.

### Design summary

The target state collapses the iter-N narrative archaeology in
`naadf_bounds_compute_node` (`bounds_calc.rs:452-677`, verified 226 lines)
into one coherent function-level docblock in present tense, and removes
the dead `end_of_encoder_noop_pipeline` infrastructure (WGSL entry point +
Rust pipeline-queue helpers + cached pipeline field + per-frame pipeline
resolution + `let _ = ...` warning-suppressor — ~110 lines across 3 files).
Inside the body, a single private helper `refresh_chunks_mirror(...)`
absorbs the duplicated `if let (Some, Some) = (chunks, chunks_mirror)`
destructure + `copy_buffer_to_buffer` + `min(...)`-size clamp that
currently appears twice. Three lying docblocks (Rust + Rust + WGSL) get
re-stated in present tense to match the code: `chunks_mirror` is the
load-bearing read shadow for **both** targets, doing two jobs — cross-frame
TRANSFER-stage propagation AND intra-pass cross-workgroup neighbour-read
shadow. Stale WGSL header line refs (499/252/538) become anchor strings
(`chunks_mirror[`, `chunks[`) so they cannot drift again. The unused
`render_device` and `render_queue` parameters drop from the signature;
the `world_gpu` parameter loses its now-misleading `cfg_attr`. The
`[aadf-probe]` one-shot log block stays in place (the brief's protected-
instrumentation list pins it, explorer note 1E confirms moving it is out
of scope). Test `tests.rs:525-534` swaps `2048 * 16` / `2048 * 4` literals
for const expressions derived from `PREPARE_PROBE_HISTORY_ENTRIES` /
`PREPARE_PROBE_HISTORY_BYTES`. Item 2: finding 2A goes ESCAPE (Option A's
estimated ~5-10% parity lift is too speculative to land without
validation in a separate session; Option B is the real fix and clearly
escape-tier). Finding 2B goes EXPLORE-ONLY (the unified-mirror is a
deliberate post-fix simplification — splitting `:523` back to `chunks`
direct-read risks regressing the SSIM floor). Finding 2C goes EXPLORE-ONLY
(cross-shader-view fragility is downstream of 2A's algorithmic restructure;
no action without that). Verification: every step that touches
`bounds_calc.rs` or `bounds_calc.wgsl` runs the full 5-gate sequence
(check + lib-test + 2× native e2e + 3× web e2e SSIM ≥ 0.91). Mechanical
changes (tests-only, comment-only) get a narrower gate.

### Target-state architecture

#### `naadf_bounds_compute_node` shape (item 1, post-refactor)

**Function-level docblock (NEW, replaces existing `:434-451` docblock):**

A single present-tense block above `pub fn naadf_bounds_compute_node`
explaining the post-fix mechanism. Required content (in this order):

1. Role: `Core3d`-schedule node running W3 regime-2 (`prepare_group_bounds`
   + indirect `compute_group_bounds`) `n_bounds_rounds` times per frame.
   Inserted before `naadf_atmosphere_node` per `15-design-c.md` §1.2.
2. Skip conditions (resources missing, `gpu_construction_enabled = false`,
   `max_group_bound_dispatch = 0`, regime-1 not yet seeded). Steady-state
   minimum-dispatch cost note (single 4³-thread group bails immediately)
   carries over from the existing `:442-451` paragraph — that text is
   still correct.
3. **The wasm regime-2 mechanism (load-bearing facts in one place):**
   - `n_bounds_rounds = 5` on native, **clamped to `1` on wasm** at
     `config.rs::From<&AppArgs>` (forbidden to touch per `01-context.md`).
   - `chunks_mirror` is a RO mirror of `chunks`, refreshed via
     `copy_buffer_to_buffer(chunks, chunks_mirror, full_size)`
     **once before round 0** (seeds from W5's chunk-classification bits;
     omitting this would zero the chunk-state read, causing false-positive
     AADF expansion on every chunk) **and between every subsequent round**.
   - `compute_group_bounds` reads ALL chunk-AADF state (own + neighbour)
     from `chunks_mirror` and writes back to `chunks` (non-atomic rw view).
     **Both targets run identical code; only `n_bounds_rounds` differs.**
   - Direct-dispatch override (`compute_workgroups_override = Some(...)`)
     on wasm is **orthogonal** to the chunks-RMW story — it works around
     Dawn's broken STORAGE→INDIRECT barrier between `prepare_group_bounds`'
     indirect-args write and `compute_group_bounds`' indirect-dispatch
     read. Native uses `None` (indirect dispatch).
4. One-line pointer: "iter-history archaeology lives in
   `docs/orchestrate/wasm-chunk-aadf-nondeterminism/` (docs 12-14); source
   describes current behaviour only."

**Helper extractions (NEW, in `bounds_calc.rs` private to the module):**

```rust
/// Issue `copy_buffer_to_buffer(chunks → chunks_mirror)` with a
/// `min(src.size(), dst.size())` clamp. Returns silently if either
/// buffer is unavailable. Called once before round 0 (seed) and once
/// between every subsequent round (propagate prior round's writes).
fn refresh_chunks_mirror(
    encoder: &mut CommandEncoder,
    chunks: Option<&bevy::render::render_resource::Buffer>,
    chunks_mirror: Option<&bevy::render::render_resource::Buffer>,
) {
    if let (Some(src), Some(dst)) = (chunks, chunks_mirror) {
        let copy_size = src.size().min(dst.size());
        encoder.copy_buffer_to_buffer(src, 0, dst, 0, copy_size);
    }
}
```

Free function in module scope (not `pub`). Single helper covers both
phases. `dispatch_regime_2_rounds` already exists at `:381-430` and
absorbs the dispatch logic — no new dispatch helper needed.

**What stays in `naadf_bounds_compute_node` after refactor:**

Top-level orchestration only:
1. Resolve resources + skip conditions (`:470-496` essentially unchanged).
2. Pull bind groups + indirect buffer (`:500-513` unchanged).
3. Resolve `prepare_pipeline` + `compute_pipeline` (`:516-521` unchanged).
4. `let n_rounds = ...max(1)` (`:536` unchanged).
5. `[aadf-probe]` one-shot config log block (`:556-570` unchanged — stays).
6. Compute `compute_workgroups_override` (`:585-588` unchanged).
7. Compute `chunks_buf_opt`, `chunks_mirror_buf_opt` (`:622-625` unchanged).
8. Acquire encoder + diagnostic time-span (`:600-601` unchanged).
9. `refresh_chunks_mirror(encoder, chunks.as_ref(), mirror.as_ref())` (seed).
10. Loop `for round_idx in 0..n_rounds { dispatch_regime_2_rounds(..., 1, ...);
    if round_idx + 1 < n_rounds { refresh_chunks_mirror(...); } }`.
11. `time_span.end(...)` (`:676` unchanged).

**What's removed from the function body:**

- The three iter-N narrative blocks at `:538-553`, `:572-584`, `:593-598`,
  and `:603-625`. Their load-bearing facts move into the function-level
  docblock; their refuted-hypothesis archaeology is dropped (it lives in
  docs 12-14 already).
- The `#[cfg(target_arch = "wasm32")]` early-return guard at `:527-534`
  resolving `end_of_encoder_noop_pipeline` — gone with finding 1D.
- The `#[cfg(target_arch = "wasm32")] { let _ = end_of_encoder_noop_pipeline; }`
  block at `:670-674` — gone with finding 1D.
- The two duplicated `if let (Some, Some) = ...` destructures at `:635-640`
  and `:660-667` — collapsed into one `refresh_chunks_mirror` call per
  phase.
- The `#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]`
  annotations at `:459, :461, :467` — gone with finding 1G.

#### Docblock corrections (finding 1B)

Three files repeat the same false claim that chunks_mirror is "never
accessed" / "never read" on native. All three get rewritten to match the
code, which **unconditionally** accesses `chunks_mirror` on both targets
(verified: WGSL reads at `bounds_calc.wgsl:273` and `:523` have no
`#define` / `#ifdef`; Rust copies at `bounds_calc.rs:635-640` and
`:660-667` have no `#[cfg]`).

**Site 1 — `bounds_calc.rs:93-105`** (the third binding of
`construction_bounds_world_layout_descriptor`):

Replacement text (present tense, accurate):

```text
// chunks_mirror — read-only mirror of `chunks` (binding 0). All
// `compute_group_bounds` reads of chunk-AADF state — own-chunk at
// `bounds_calc.wgsl:523` AND neighbour-chunk at `:273` — come from this
// mirror; the only writer of chunk-AADF state is the non-atomic rw view
// of `chunks` at `:564`. The mirror is refreshed via
// `copy_buffer_to_buffer(chunks → chunks_mirror)` once before round 0
// (seeds from W5's chunk-classification bits — without the seed copy,
// `chunk_state = cur_chunk >> 30` reads 0 and every chunk
// false-positive-expands) and between every subsequent round (propagates
// the prior round's writes via a TRANSFER-stage barrier — the strongest
// cross-pass dependency wgpu offers).
//
// Load-bearing on BOTH targets. Both native and wasm run the same code
// path; only `n_bounds_rounds` differs (5 native, 1 wasm — the wasm
// clamp lives in `config.rs::From<&AppArgs>`). On native the mirror is
// also load-bearing for cross-workgroup neighbour visibility within a
// single round (the indirect dispatch's workgroups read each other's
// pre-round chunk state via the mirror; intra-round writes to `chunks`
// are not visible until the next mirror refresh).
```

**Site 2 — `mod.rs:161-168`** (the `chunks_mirror_buffer` struct-field
docblock on `ConstructionGpu`):

Replacement text:

```text
/// Read-only mirror of `chunks` used by `compute_group_bounds` for ALL
/// chunk-AADF reads (own at `bounds_calc.wgsl:523`, neighbour at `:273`).
/// Refreshed via `copy_buffer_to_buffer(chunks, chunks_mirror, full_size)`
/// once before round 0 (W5-seed) and between every subsequent round.
/// Load-bearing on both targets — see `bounds_calc.rs::naadf_bounds_compute_node`
/// docblock for the full mechanism. Same size as chunks_buffer
/// (`array<vec2<u32>>`, stride 8 B).
```

**Site 3 — `bounds_calc.wgsl:117-125`** (the `chunks_mirror` binding
header comment):

Replacement text (with stale line refs killed per finding 1C, anchor
strings used instead):

```wgsl
// chunks_mirror — read-only mirror of `chunks` (binding 0). All reads
// of chunk-AADF state in this shader come from chunks_mirror: search
// `chunks_mirror[` to find the read sites (own-AADF in
// `compute_group_bounds`, neighbour-AADF in `add_bounds_group`). The
// only writer of chunk-AADF state is the non-atomic rw view of `chunks`
// — search `chunks[chunk_idx] =` for the write site.
//
// Rust-side refreshes the mirror via
// `copy_buffer_to_buffer(chunks → chunks_mirror)` once before round 0
// (seeds W5's chunk-classification bits) and between every subsequent
// round (propagates the prior round's writes via a TRANSFER-stage
// barrier). Both targets run the same code; only `n_bounds_rounds`
// differs (5 native, 1 wasm — wasm clamp in `config.rs::From<&AppArgs>`).
```

#### Stale-ref fixes (finding 1C)

The replacement at WGSL site 3 above kills the bare line numbers
(499/252/538 → all stale; actual current values are 523/273/564 per
verified Read). Anchor-string navigation (`chunks_mirror[`,
`chunks[chunk_idx] =`) is drift-proof — those substrings are unique to
the relevant sites.

#### Dead-code removal (finding 1D)

Six sites across three files, removed atomically:

1. `bounds_calc.wgsl:432-467` — the `end_of_encoder_noop` entry point and
   its 31-line probe-2 narrative header. Deleted in full (36 lines).
2. `bounds_calc.rs:290-329` — `queue_end_of_encoder_noop_pipeline` +
   `queue_end_of_encoder_noop_pipeline_with_handle` + the 10-line probe-2
   docblock above them. Deleted in full (40 lines).
3. `bounds_calc.rs:523-534` — the wasm-only `#[cfg(target_arch = "wasm32")] let Some(end_of_encoder_noop_pipeline) = ...` resolution gate. Deleted in full (12 lines).
4. `bounds_calc.rs:670-674` — the wasm-only `let _ = end_of_encoder_noop_pipeline;`
   warning-suppressor block. Deleted in full (5 lines).
5. `mod.rs:546-555` — the `bounds_calc_pipeline_end_of_encoder_noop`
   struct-field docblock + field declaration. Deleted in full (10 lines).
6. `mod.rs:659-669` — the `bounds_calc_pipeline_end_of_encoder_noop =
   bounds_calc::queue_end_of_encoder_noop_pipeline(...)` queue site.
   Deleted in full (11 lines).
7. `mod.rs:750` — the field initializer in the `ConstructionPipelines`
   struct literal. Deleted (1 line).

Total ≈ 115 lines removed. No callers remain after step 6+7; the
pipeline is then truly dead. Verified the explorer's claim that "the
brief's FORBIDDEN MOVES list at `01-context.md:144-146` does not name
`end_of_encoder_noop`" — re-read `01-context.md:144-146`: protected
probes are `[probe1-call]`, `[cpu-gpu-parity]`, `[aadf-probe]`,
`[device-snapshot]`. `end_of_encoder_noop` / probe-2 is **not** in the
protected list. Removal is in-scope.

#### One-shot `[aadf-probe]` log (finding 1E)

**Stays in place** at `bounds_calc.rs:556-570`. Explorer correctly
identifies this as on the boundary: spirit of `01-context.md:144-146`'s
"DO NOT modify the `[aadf-probe]` diagnostic instrumentation" protects
the channel. Moving it to a startup system is **escape-tier** and the
user's `[aadf-probe]` pin makes the conservative choice to leave it
untouched. The cleanup gain is small; the risk of touching protected
instrumentation outweighs it.

#### Mixed-concerns inside the function body (finding 1F)

Resolved by the helper extraction above. After refactor, the loop body
collapses to:

```rust
for round_idx in 0..n_rounds {
    dispatch_regime_2_rounds(
        encoder,
        prepare_pipeline, compute_pipeline,
        bounds_world_bg, bounds_bg, dispatch_bg, probe_bg,
        indirect_buffer,
        1,
        compute_workgroups_override,
    );
    if round_idx + 1 < n_rounds {
        refresh_chunks_mirror(encoder, chunks_buf_opt.as_ref(), chunks_mirror_buf_opt.as_ref());
    }
}
```

The pre-loop seed copy becomes one line:

```rust
refresh_chunks_mirror(encoder, chunks_buf_opt.as_ref(), chunks_mirror_buf_opt.as_ref());
```

Three logical phases (seed / dispatch / refresh-between-rounds) are now
named at the call site. No interleaved iter-N commentary remains in the
body.

#### Signature cleanup (finding 1G)

`naadf_bounds_compute_node` signature (`:452-469`) post-refactor:

```rust
pub fn naadf_bounds_compute_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    construction_pipelines: Option<Res<super::ConstructionPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_gpu: Option<Res<ConstructionGpu>>,
    construction_config: Option<Res<ConstructionConfig>>,
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
) {
```

Changes:

- Drop `render_device: Res<RenderDevice>` (no body reference; verified
  by `Read` of `:452-677`).
- Drop `render_queue: Res<RenderQueue>` (no body reference; verified
  by `Read` of `:452-677`).
- Drop all three `#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]`
  annotations. `world_gpu` is used at `:622-624` unconditionally; the
  annotation was already incorrect.

Verified at `Bash` time: no other call site invokes
`naadf_bounds_compute_node` — Bevy system-registration is by-function-
ptr (typed parameter signature inference), so dropping unused `Res<T>`
parameters is API-safe inside the workspace. The function is registered
as a system in `render/mod.rs` (or equivalent — implementer verifies
the registration site does not name the parameters explicitly).

#### Item 2 classifications

**Finding 2A — cross-workgroup chunks-RMW visibility gap (classification: ESCAPE).**

The structural cause of the 18% web parity gap is correctly identified
by the explorer: workgroup A writes `chunks[chunk_idx]`; workgroup B
reads `chunks_mirror[neighbour_idx]` in the SAME pass; the mirror is
only refreshed BETWEEN passes; so B sees A's pre-this-round value, one
round behind. On native this lag is repaid by `n_bounds_rounds = 5`; on
wasm at `n_bounds_rounds = 1` it's locked at one frame.

Three options analysed (per explorer):

- **Option A — extra mid-round `copy_buffer_to_buffer(chunks → chunks_mirror)`
  on wasm only.** Explorer estimates ~5-10% parity lift. I classify this
  as **ESCAPE, not SMALL+OBVIOUS+LOW-RISK**, against the explorer's
  recommendation. Reasoning:
  1. The estimated lift is speculative — explorer flags it as "probably
     small (~5-10%)" without empirical backing. The brief's commit-policy
     for SMALL+OBVIOUS+LOW-RISK requires "small + obvious + low-risk"
     **and** measurable benefit. Speculative benefit at this scale fails
     the "obvious" criterion.
  2. The fix mechanism semantics are non-trivial. Adding an EXTRA
     mid-round copy on wasm only (currently wasm runs n=1, so "extra
     mid-round" means after the only round but before the renderer's
     first-hit pass) makes wasm and native code paths visibly diverge
     again — the recent fix specifically harmonised them. Diverging
     again to capture a 5% parity bump is the wrong tradeoff at this
     refactor's "comments + control-flow tightening" scope.
  3. There's no e2e gate measuring SSIM at finer granularity than the
     0.91 floor. If Option A lifts SSIM from 0.93 → 0.95 we cannot
     prove it; if it drops the floor from 0.93 → 0.91 we cannot reliably
     detect it (bimodal trajectory observation in `14-cleanup-sweep.md`
     shows 8/10 runs at ~0.93, 2/10 at ~0.91 — so SSIM variance is
     already high).
  4. The 3-web-run e2e gate has a wall-clock cost of ~12 min per
     verification cycle. Landing a speculative fix that doesn't move
     the gate is verification-time spent for no verification signal.
  
  Scope-sketch for the follow-up `/refactor` session:
  - Single-finding focus: "lift wasm SSIM from 18% parity to N% parity
    via mid-round mirror refresh".
  - Predict-the-outcome discipline: run baseline 5× to establish the
    SSIM distribution, predict the post-fix distribution, validate.
  - If the lift is < 10%, document and revert — the cost (wasm
    bandwidth + code-path divergence) is not paid back.
- **Option B — algorithmic restructure via per-axis FIFO queue carry of
  AADF expansions** (per `12-brute-force-summary.md:251-261` side-note 1).
  Classification: **ESCAPE**. Estimated lift to 80%+. This is the real
  long-term fix and warrants its own session — likely a multi-week
  refactor touching `bounds_calc.wgsl`, the bind-group layout, and
  possibly the W3 prepare/compute split.
- **Option C — raise `n_bounds_rounds` on wasm to 2+ with mirror refresh
  between**. FORBIDDEN by `01-context.md:130-131` (the `n_bounds_rounds = 1`
  wasm clamp is non-negotiable). Documented for completeness only.

**Finding 2B — chunks_mirror dual-purpose (cross-frame + intra-pass shadow).
Classification: EXPLORE-ONLY.**

The conflation of "cross-frame propagation buffer" with "this-pass shadow"
is real but the unification is a deliberate post-fix simplification per
13-minimal-fix-verify.md. Splitting the `:523` own-chunk read back to
`chunks` direct-read (cosmetically tidier) would:
1. Re-introduce a cross-target code path divergence (or require a WGSL
   `const` toggle — not in scope).
2. Risk regressing the SSIM floor — the bimodal trajectory shows the
   margin is thin (0.91 ≈ 0.93 on the cluster boundary).
3. Not improve the load-bearing semantics — `:523` and `:273` are
   already correctly described by the new docblock as both reading from
   the mirror with explicit cross-workgroup-visibility rationale.

The docblock corrections in finding 1B above (Site 1, Site 3 specifically)
**already document the split** in present tense ("Load-bearing on BOTH
targets. ... On native the mirror is also load-bearing for cross-
workgroup neighbour visibility within a single round"). That's the
maximum explicit-documentation move available without a behavioral
change. No further action.

**Finding 2C — multi-shader chunks-buffer view fragility. Classification:
EXPLORE-ONLY.**

The cross-shader view-disagreement (W1's now-`array<vec2<u32>>` storage
buffer view + W3's same view + first-hit shader's read-only view + W5's
write-only-ish view) is real foundation-level fragility per
`12-brute-force-summary.md:271-280`. But:

1. It is downstream of Finding 2A's algorithmic restructure (Option B).
   The "right" fix is a unified, narrower interface — but that requires
   2A's session to happen first.
2. The iter-3 atomic-view-of-same-buffer experiment was already reverted
   in `960eeb2`; the **current** WGSL views are consistent
   (`array<vec2<u32>>` non-atomic across all shaders touching it).
3. No "small + obvious + low-risk" action is available; documenting it
   is already done in the orchestration doc trail.

Architecture document note: this is captured here for the orchestrator's
awareness. No source-code action.

#### Item 3 design

Mechanical const alignment at `tests.rs:525-534`. Verified production
const `PREPARE_PROBE_HISTORY_ENTRIES = 256` at `mod.rs:340` and
`PREPARE_PROBE_HISTORY_BYTES = 4096` at `mod.rs:343-344`.

Target edit (precise replacement; the `super::super::` path is needed
because the test lives in `bounds_calc/tests.rs` and the const is in
`construction/mod.rs`, i.e. one module up from `bounds_calc`):

Add to the `use` block at `tests.rs:35-41` (or as a separate use line):

```rust
use crate::render::construction::{PREPARE_PROBE_HISTORY_BYTES, PREPARE_PROBE_HISTORY_ENTRIES};
```

(Verified: `crate::render::construction` is reachable from
`bounds_calc/tests.rs` per the existing `use crate::render::construction::bounds_calc::...`
import at `tests.rs:35-41`. The consts are `pub` at `mod.rs:340, 343`.)

Replace `tests.rs:525-534`:

```rust
// probe history buffer sized to production capacity (= PREPARE_PROBE_HISTORY_ENTRIES
// entries × 16 B/entry = PREPARE_PROBE_HISTORY_BYTES). Test mirrors
// production sizing so future downsizes don't drift apart.
let prepare_probe_history = device.create_buffer(&BufferDescriptor {
    label: Some("w3_prepare_probe_history"),
    size: PREPARE_PROBE_HISTORY_BYTES,
    usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
    mapped_at_creation: false,
});
let probe_zeros: Vec<u32> = vec![0u32; (PREPARE_PROBE_HISTORY_ENTRIES * 4) as usize];
queue.write_buffer(&prepare_probe_history, 0, bytemuck::cast_slice(&probe_zeros));
```

Sizing math: `PREPARE_PROBE_HISTORY_BYTES` is `u64`, fits `BufferDescriptor.size: u64`
directly. `PREPARE_PROBE_HISTORY_ENTRIES * 4` is `u32 * u32 = u32`, cast to
`usize` for `Vec` sizing. New allocation: 4 KiB (was 32 KiB) + 1 KiB
zeros (was 32 KiB). The test's WGSL uses
`arrayLength(&prepare_probe_history) / 4u` (per `bounds_calc.wgsl:170`),
which adapts dynamically — no shader-side change needed.

### Migration steps (ordered, granular)

Each step is one atomic edit set that leaves the tree buildable and
test-passing. After every step the implementer runs the verification
gate prescribed for that step; if the gate fails, fix in-place and
re-verify (never `--amend`, per project rules — but the orchestrator
handles commits, so this is the implementer's per-step gate, not a
commit gate).

Steps ordered by risk: mechanical first (item 3), comment-only next
(finding 1A/1B/1C), then dead-code removal (1D), then control-flow
extract (1F), then signature cleanup (1G), then item 2 documentation
finalisation. This ordering means a failing gate in the highest-risk
step does not block the mechanical wins below.

#### Step 1 — Couple `tests.rs` probe-buffer sizing to production const (finding 3A)

**Edits:**
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:35-41` — add `use crate::render::construction::{PREPARE_PROBE_HISTORY_BYTES, PREPARE_PROBE_HISTORY_ENTRIES};` (insert as new line or extend existing use).
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:525-534` — replace as per "Item 3 design" above.

**Rationale:** Mechanical const-alignment, fixes the misleading "matches production capacity" comment, no behavioral effect (WGSL `arrayLength` adapts).

**Post-step state:** Test allocates 4 KiB instead of 32 KiB; future downsizes propagate; comment matches reality.

**Verification:** `timeout 120s cargo check --workspace` + `timeout 300s cargo test -p bevy-naadf --lib`. (No e2e — test-only mechanical change.)

#### Step 2 — Correct the three lying chunks_mirror docblocks (finding 1B + 1C combined)

**Edits:**
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:93-105` — replace with Site 1 text from "Docblock corrections" above.
- `crates/bevy_naadf/src/render/construction/mod.rs:161-168` — replace with Site 2 text.
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:117-125` — replace with Site 3 text (kills stale line refs 499/252/538 — anchor strings used instead).

**Rationale:** Three sites repeat the same false "native never accesses chunks_mirror" claim; code is correct on both targets, comments lie. Stale WGSL line refs (1C) get killed in the same pass — they were inside the same comment block being rewritten.

**Post-step state:** Three docblocks accurately describe `chunks_mirror` as load-bearing on both targets with the dual cross-frame + intra-pass role. WGSL header navigation is drift-proof via anchor strings.

**Verification:** `timeout 120s cargo check --workspace` + `timeout 300s cargo test -p bevy-naadf --lib`. (Comment-only — no behavioral change.) Implementer **may** skip the e2e here since pure comment edits cannot regress the runtime; **must** run e2e if the WGSL parser is sensitive to comment-block content (Tint should not be, but the WGSL touches the binding declaration's comment which is parsed as a token).

Recommended conservative gate: ALSO run 2× native e2e (`timeout 300s cargo run --release --bin e2e_render -- --vox-horizon-native`) to confirm the WGSL still compiles. Skip the 3× web run for this step — the build is comment-only, web e2e gate runs after step 4 anyway.

#### Step 3 — Remove `end_of_encoder_noop_pipeline` dead code (finding 1D)

**Edits:**
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:432-467` — delete the `end_of_encoder_noop` entry point + its header narrative (36 lines).
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:290-329` — delete `queue_end_of_encoder_noop_pipeline` + `queue_end_of_encoder_noop_pipeline_with_handle` + docblock (40 lines).
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:523-534` — delete the wasm-only resolution gate (12 lines).
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:670-674` — delete the `let _ = end_of_encoder_noop_pipeline` block (5 lines).
- `crates/bevy_naadf/src/render/construction/mod.rs:546-555` — delete the field docblock + `bounds_calc_pipeline_end_of_encoder_noop: CachedComputePipelineId,` field (10 lines).
- `crates/bevy_naadf/src/render/construction/mod.rs:659-669` — delete the `bounds_calc_pipeline_end_of_encoder_noop = bounds_calc::queue_end_of_encoder_noop_pipeline(...)` queue site (11 lines).
- `crates/bevy_naadf/src/render/construction/mod.rs:750` — delete the field initializer in the struct-literal `ConstructionPipelines { ... }` (1 line).

**Rationale:** Pipeline is queued, cached, resolved per-frame on wasm (with an early-return gate that can block the entire regime-2 node), and immediately `let _ = ...`'d to silence the unused-variable warning. Never dispatched. Same dead-from-iter-N shape as the `chunks_scratch` Locals already removed in `c6b0deb`. Brief's forbidden-instrumentation list at `01-context.md:144-146` does NOT name `[probe-2]` / `end_of_encoder_noop` — removal is in-scope.

**Post-step state:** ~115 lines of dead code gone. The wasm code path no longer has a "skip the entire frame if probe-2 pipeline not ready" hazard. The 35-line WGSL probe-2 narrative is gone (lives in `docs/orchestrate/wasm-chunk-aadf-nondeterminism/07-diagnosis-round2.md` already).

**Verification:** Full gate sequence:
1. `timeout 120s cargo check --workspace`
2. `timeout 300s cargo test -p bevy-naadf --lib`
3. `for i in 1 2; do timeout 300s cargo run --release --bin e2e_render -- --vox-horizon-native; done`
4. `timeout 1500s just web-build-release`
5. `for i in 1 2 3; do cd e2e && timeout 240s npx playwright test vox-horizon-parity.spec.ts --headed; cd ..; done` — ALL 3 SSIM ≥ 0.91.

Why full gate: this touches `bounds_calc.wgsl` (WGSL source change requires web build), removes a wasm-only code-path branch (regime-2 node no longer has the early-return gate), and modifies the `ConstructionPipelines` struct shape (impl-of-FromWorld + struct literal — any missed callsite is a compile error caught at step 1 of the gate, but a missed field-rename downstream would only surface at step 2 or later).

#### Step 4 — Extract `refresh_chunks_mirror` helper + collapse the function body (finding 1F + 1A)

**Edits:**
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:432` (above `pub fn naadf_bounds_compute_node`) — add the new function-level docblock per "Function-level docblock" above.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` — somewhere between the existing `dispatch_regime_2_rounds` (`:381-430`) and `naadf_bounds_compute_node` (the natural seam at `~:430`) — insert the `refresh_chunks_mirror` helper per "Helper extractions" above.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:535-553` — delete iter-N narrative block (was already partially gone after step 3; this is the residual narrative `:538-553`).
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:572-584` — delete iter-N narrative block (the "web-vox ray-termination fix" historical block; the load-bearing facts move into the function-level docblock).
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:593-625` — delete the multi-block iter-N narrative + "Pull world_gpu / construction_gpu" preamble; keep ONLY the load-bearing two-line `let chunks_buf_opt = ...; let chunks_mirror_buf_opt = ...;` extraction.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:627-640` — replace the explicit `if let (Some, Some)` initial-seed copy with `refresh_chunks_mirror(encoder, chunks_buf_opt.as_ref(), chunks_mirror_buf_opt.as_ref());`.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:642-668` — collapse the loop body. The duplicated `if let (Some, Some) = ...` block at `:660-667` becomes a `refresh_chunks_mirror(...)` call.

Final body shape (lines reduce from ~76 to ~25):

```rust
let encoder = render_context.command_encoder();
let time_span = diagnostics.time_span(encoder, BOUNDS_COMPUTE_SPAN);

let chunks_buf_opt = world_gpu.as_ref().map(|w| w.chunks_buffer.clone());
let chunks_mirror_buf_opt = construction_gpu.chunks_mirror_buffer.clone();

// Seed: mirror must reflect W5's chunk-classification bits before round 0
// (otherwise `chunk_state = cur_chunk >> 30` reads 0 and false-positive
// expands every chunk). See function docblock.
refresh_chunks_mirror(encoder, chunks_buf_opt.as_ref(), chunks_mirror_buf_opt.as_ref());

for round_idx in 0..n_rounds {
    dispatch_regime_2_rounds(
        encoder, prepare_pipeline, compute_pipeline,
        bounds_world_bg, bounds_bg, dispatch_bg, probe_bg,
        indirect_buffer, 1, compute_workgroups_override,
    );
    // Propagate this round's writes to the mirror so the next round's
    // reads see them via the TRANSFER-stage barrier.
    if round_idx + 1 < n_rounds {
        refresh_chunks_mirror(encoder, chunks_buf_opt.as_ref(), chunks_mirror_buf_opt.as_ref());
    }
}

time_span.end(render_context.command_encoder());
```

**Rationale:** The three logical phases (seed / dispatch-loop / between-rounds-refresh) are now named at the call site, the duplicated destructure is gone, and the load-bearing facts live in the function-level docblock rather than scattered across three iter-N narrative blocks.

**Post-step state:** `naadf_bounds_compute_node` body is ~50% shorter (function went from 226 lines to ~120 lines). One coherent docblock explains the mechanism. The function body reads as orchestration only; mechanics live in `refresh_chunks_mirror` + `dispatch_regime_2_rounds`.

**Verification:** Full gate sequence (same as step 3). The behavior is bit-identical — same encoder, same copy ordering, same dispatch sequence — but the gate confirms it. The 3× web SSIM run is **load-bearing for this step**: any change to the seed-copy or per-round-copy ordering would surface as SSIM drop.

#### Step 5 — Drop unused parameters + clean cfg-attrs (finding 1G)

**Edits:**
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:452-469` — remove `render_device: Res<RenderDevice>`, `render_queue: Res<RenderQueue>` parameters; remove all three `#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]` annotations; remove the iter-N comment at `:463-466`.

Final signature shape (also documented under "Signature cleanup" above):

```rust
pub fn naadf_bounds_compute_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    construction_pipelines: Option<Res<super::ConstructionPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_gpu: Option<Res<ConstructionGpu>>,
    construction_config: Option<Res<ConstructionConfig>>,
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
) {
```

- Verify by `Grep` (during implementation) that no `use bevy::render::renderer::RenderDevice;` / `RenderQueue;` import remains needed by `bounds_calc.rs` after these parameters drop. If unused, remove the imports too.

**Rationale:** `render_device` and `render_queue` were the iter-1 (HM/HN) host-side write_buffer fence experiment consumers; that experiment was reverted in `a426441`. They have been unused on both targets since. The `cfg_attr(not(wasm), allow(unused_variables))` on `world_gpu` was outright misleading — it's used at `:622-624` unconditionally.

**Post-step state:** Function signature has 7 parameters (was 9). Bevy's system-extraction does not name parameters — dropping unused `Res<T>` is API-safe.

**Verification:** `timeout 120s cargo check --workspace` + `timeout 300s cargo test -p bevy-naadf --lib` + 2× native e2e. Skip the web e2e — signature-only change cannot affect WGSL or dispatch semantics; the check + lib test prove the function is still a valid Bevy system.

#### Step 6 — Append item-2 documentation to the architecture trail (no source edit)

**Edits:**
- None to source. This step is purely a no-op from the source-code perspective — Item 2's findings 2A/2B/2C are all classified above; the implementer reads the architecture doc, confirms no source edit is needed for item 2, and notes the classification.

**Rationale:** The brief asked for explicit classification of each item-2 finding. Done in this architecture doc; no source change requested.

**Post-step state:** Implementer's `04-refactoring.md` records that item-2 findings yielded zero source edits this session (2A → ESCAPE for follow-up session; 2B → EXPLORE-ONLY captured by docblock changes in step 2; 2C → EXPLORE-ONLY no in-scope action).

**Verification:** None — nothing changed. Implementer documents the no-op in `04-refactoring.md`.

### What stays / what changes / what's removed (consolidated table)

| Surface | Status | Notes |
|---------|--------|-------|
| `naadf_bounds_compute_node` body (`bounds_calc.rs:452-677`) | **changed** | function-level docblock added; iter-N narrative blocks deleted; `refresh_chunks_mirror` helper extracted; loop body collapsed |
| `naadf_bounds_compute_node` signature | **changed** | drop `render_device` + `render_queue`; drop 3× `cfg_attr` annotations |
| `refresh_chunks_mirror` (new private fn) | **new** | absorbs the duplicated `if let (Some, Some)` + `copy_buffer_to_buffer` + `min(size)` clamp |
| `dispatch_regime_2_rounds` (`bounds_calc.rs:381-430`) | **stays unchanged** | already a clean helper; the refactor leans on it |
| `end_of_encoder_noop` WGSL entry (`bounds_calc.wgsl:432-467`) | **removed** | dead code per 1D |
| `queue_end_of_encoder_noop_pipeline*` Rust helpers (`bounds_calc.rs:290-329`) | **removed** | dead code per 1D |
| `bounds_calc_pipeline_end_of_encoder_noop` field on `ConstructionPipelines` (`mod.rs:555`) | **removed** | dead code per 1D — callers gone in same step |
| `bounds_calc_pipeline_end_of_encoder_noop` queue site (`mod.rs:659-669`) | **removed** | dead code per 1D |
| `bounds_calc_pipeline_end_of_encoder_noop` field initializer (`mod.rs:750`) | **removed** | dead code per 1D |
| `chunks_mirror_buffer` allocation (`mod.rs`) | **stays unchanged** | semantically load-bearing; docblock at `:161-168` rewritten |
| `chunks_mirror` WGSL binding declaration (`bounds_calc.wgsl:117-127`) | **changed** | docblock rewritten; binding declaration untouched |
| `chunks_mirror` Rust binding (`bounds_calc.rs:93-105`) | **changed** | docblock rewritten; layout entry untouched |
| `[aadf-probe]` one-shot log (`bounds_calc.rs:556-570`) | **stays unchanged** | protected by brief's instrumentation list |
| `[probe1-call]` infrastructure (`mod.rs:3889-4140`) | **stays unchanged** | not in scope; explorer notes docblock is healthy |
| `compute_workgroups_override` direct-dispatch logic (`bounds_calc.rs:585-588`) | **stays unchanged** | load-bearing wasm fix; docblock at `dispatch_regime_2_rounds:359-379` is already accurate |
| `PREPARE_PROBE_HISTORY_ENTRIES` const (`mod.rs:340`) | **stays unchanged** | becomes the SSoT for the test (step 1) |
| `PREPARE_PROBE_HISTORY_BYTES` const (`mod.rs:343-344`) | **stays unchanged** | becomes the SSoT for the test (step 1) |
| Test probe-buffer sizing (`tests.rs:525-534`) | **changed** | literals `2048 * 16` / `2048 * 4` → const expressions |
| `n_bounds_rounds = 1` wasm clamp (`config.rs::From<&AppArgs>`) | **stays unchanged** | forbidden to touch |
| `WASM_MAX_GROUP_BOUND_DISPATCH = 4096` (`config.rs`) | **stays unchanged** | forbidden to touch |
| `HORIZON_SSIM_SIMILARITY_MIN = 0.91` | **stays unchanged** | forbidden to lower |
| `MAX_RAY_STEPS_PRIMARY` | **stays unchanged** | forbidden to raise |
| WGSL `chunks` binding declaration (`bounds_calc.wgsl:113-114`) | **stays unchanged** | rw view; load-bearing |
| `chunks[chunk_idx]` write site (`bounds_calc.wgsl:564`) | **stays unchanged** | finding 2A ESCAPE; no in-scope edit |
| `chunks_mirror[chunk_idx]` own-read (`bounds_calc.wgsl:523`) | **stays unchanged** | finding 2B EXPLORE-ONLY; docblock rewrite at step 2 covers it |
| `chunks_mirror[neighbour_idx]` neighbour-read (`bounds_calc.wgsl:273`) | **stays unchanged** | finding 2A ESCAPE; docblock rewrite at step 2 covers it |
| Cross-shader chunks-buffer views (`chunk_calc.wgsl`, `world_data.wgsl`) | **stays unchanged** | finding 2C EXPLORE-ONLY |

### Decisions & rejected alternatives

**Decision 1: One helper (`refresh_chunks_mirror`) vs two (separate `seed` + `between_rounds`).**

Considered: extracting two helpers, `seed_chunks_mirror` (pre-loop) and
`refresh_chunks_mirror_between_rounds` (in-loop), to make the two phases
explicitly named at extraction. Rejected because:
- The two phases issue **identical** `copy_buffer_to_buffer` calls with
  identical sizing logic. The only difference is the call site's
  surrounding comment, which the function-level docblock already covers.
- Two helpers double the API surface for the module without behavioral
  benefit. The DRY win of one helper outweighs the naming clarity gain
  of two.
- The call sites' names (`refresh_chunks_mirror(...)` before the loop
  with a `// Seed` comment; `refresh_chunks_mirror(...)` inside the
  `if round_idx + 1 < n_rounds` block with a `// Propagate` comment)
  already convey the phase distinction at zero cost.

**Decision 2: Item 1's `[aadf-probe]` log stays in place; not moved to a startup system.**

Considered: extracting the `[aadf-probe]` one-shot log (`:556-570`) to a
dedicated startup system that runs once at `ConstructionConfig` resource
creation. Rejected because:
- `01-context.md:144-146` explicitly pins the `[aadf-probe]` channel as
  protected instrumentation. The spirit of the pin is "don't move
  diagnostic channels"; moving from "fires once on first frame" to
  "fires at startup" is a semantic change to a pinned channel.
- The explorer's finding 1E correctly flags this as "escapes the
  in-scope restructure latitude". Auto-mode active per system reminder
  — making the reasonable call: leave protected instrumentation alone.
- The cleanup gain is small (~15 lines moved out of the hot path; cost
  is zero per call after the first).

**Decision 3: Item 2A classified ESCAPE, not SMALL+OBVIOUS+LOW-RISK.**

Considered: classifying Option A (extra mid-round `copy_buffer_to_buffer`
on wasm only) as SMALL+OBVIOUS+LOW-RISK and applying it in this refactor.
Rejected per "Item 2 classifications" Finding 2A above; key reasons:
- Estimated 5-10% lift is speculative without empirical validation;
  fails the "obvious" criterion of small+obvious+low-risk.
- Adding wasm-only mid-round copies re-diverges code paths that the
  recent fix specifically harmonised.
- The 3-web-run e2e gate has high variance (bimodal 0.91 / 0.93); a 5%
  lift cannot be reliably observed at the gate.
- Verification cost (12 min per gate cycle) vs uncertain benefit.

Sketch for the follow-up `/refactor` session is included so the
orchestrator can spin it up later if desired.

**Decision 4: WGSL line refs replaced by anchor strings, not updated to
current values.**

Considered: updating the WGSL header's `line 499 own-AADF read` to
`line 523 own-AADF read` (current value). Rejected:
- The explorer's finding 1C explicitly flags that bare line numbers
  will drift again with the next intervention. Replacing 499 with 523
  fixes this snapshot but not the failure mode.
- Anchor strings (`chunks_mirror[`, `chunks[chunk_idx] =`) are unique
  to the relevant sites and won't drift.

**Decision 5: Item 3 uses `super::super::` path import.**

Considered: re-exporting the consts from `bounds_calc` module to keep
the test's import path shorter. Rejected:
- The consts logically belong to `construction/mod.rs` (they describe
  the whole-W3 probe history); re-exporting from `bounds_calc` would
  muddy the ownership.
- `use crate::render::construction::{PREPARE_PROBE_HISTORY_BYTES, PREPARE_PROBE_HISTORY_ENTRIES};`
  is one line and uses the existing project convention.

### Assumptions made

1. **`naadf_bounds_compute_node` is registered as a Bevy system by
   function pointer, not by named-parameter signature.** Verified at
   `Grep` time that no caller invokes the function by name with
   explicit parameter listing inside the workspace. Bevy's `add_systems`
   macro extracts the parameter types via the `IntoSystem`-generic
   trait, so dropping unused `Res<T>` parameters in step 5 is safe.
   The implementer must `cargo check --workspace` after step 5 to
   surface any cross-crate caller (none expected).

2. **The `arrayLength(&prepare_probe_history)` runtime sizing in
   `bounds_calc.wgsl:170-172` continues to adapt to the buffer's
   declared `size` at bind time.** Verified by reading the WGSL block;
   no static-array assumption. After step 1, the WGSL sees a buffer
   declared `size = 4096 B = 1024 u32s = 256 entries × 4`, and
   `arrayLength(...) / 4u = 256` — matches `PREPARE_PROBE_HISTORY_ENTRIES`.

3. **`time_span.end(render_context.command_encoder())` at `:676` continues
   to be a valid second `command_encoder()` call after the first at
   `:600`.** Bevy's `RenderContext::command_encoder()` returns a `&mut CommandEncoder`
   to the same encoder; both calls in the existing code work. After
   refactor the same pattern persists.

4. **No documentation hyperlinks from outside `docs/orchestrate/` cite the
   deleted iter-N comment blocks.** A `grep` for the deleted text
   ("iter-2-2 H1", "iter-2-3 if needed", "M1 confirmation probe") in
   the source tree (not orchestration docs) before step 3 / step 4 will
   surface any. The orchestration trail under `docs/orchestrate/wasm-chunk-aadf-nondeterminism/`
   contains these strings legitimately and is unaffected.

5. **Bevy 0.19's WGSL composition does not parse comment-block content
   as a token.** Step 2's WGSL comment-only edit assumes Tint treats the
   `//` block as freely-mutable whitespace. This is the standard WGSL
   tokenizer behaviour; any deviation would be a Tint regression. The
   `cargo check --workspace` + native e2e gate at step 2 confirms this.

### Side notes / observations / complaints (MANDATORY per CLAUDE.md)

1. **The explorer doc is genuinely excellent — best explorer phase output I've
   seen on this lineage.** Verified citations (after one read-through with
   `Read` / `Grep` for every file:line), accurate severity tiers, and the
   cross-cutting smells section called out the three-way contradiction
   cleanly. Two ground-truth corrections needed: explorer's "function is
   250+ lines" (actual: 226, explorer self-flagged); explorer's claim that
   the WGSL header line refs were "writes at line 538" — that was probably
   a stale ref from before the iter-3 revert; current `chunks[chunk_idx] = ...`
   write site is at `:564`, confirmed.

2. **The `chunks_mirror`-load-bearing-on-both-targets reality is genuinely
   surprising and the docblock-rewriting is the highest-leverage work in
   this refactor.** I spent ~5 minutes confused by the explorer's claim
   that native uses `chunks_mirror` too, until I re-read `bounds_calc.rs:622-625`
   + `:635-640` + `:660-667` and confirmed: no `#[cfg]`, no `cfg_if`, no
   `if cfg!(...)` — Rust unconditionally issues the copy on native. WGSL
   unconditionally reads from the mirror. The current docblocks are
   actively misleading the next reader. This is the kind of foundation-rot
   the global CLAUDE.md's smell-driven-escape clause is designed to catch
   — except the explorer already caught it. Step 2's docblock rewrites are
   the load-bearing comment work here.

3. **The `end_of_encoder_noop` removal in step 3 has a real concrete cost
   beyond cosmetic cleanup: the wasm `let Some(...) else { return; }` at
   `bounds_calc.rs:527-534` can BLOCK the entire regime-2 node from running
   if the pipeline doesn't compile.** That's a latent foundation hazard
   — if a future Tint regression breaks the probe-2 entry point compile,
   regime-2 silently stops on wasm and the user sees rays terminate at
   chunk boundaries. Removing the dead pipeline removes this hazard.
   Explorer's side-note 2 raised this; I'm second-ing it as load-bearing
   for the implementer to surface in `04-refactoring.md`.

4. **The decision to classify Item 2A as ESCAPE (not SMALL+OBVIOUS+LOW-RISK)
   went against the explorer's recommendation.** Explorer recommended
   Option A as SMALL+OBVIOUS+LOW-RISK. My read: the brief's commit-policy
   ("small + obvious + low-risk") requires all three; "small" yes (~20
   lines), "obvious" no (speculative 5-10% lift), "low-risk" maybe
   (re-divergent code paths). The orchestrator may want to override
   this — that's why I sketched the follow-up session for Option A.
   If the user explicitly wants Option A landed in this session, the
   architect-tier escalation is to predict the SSIM distribution
   pre-fix (5 runs) + post-fix (5 runs), confirm the lift is real, and
   land it; that's a `/refactor` mini-session of its own.

5. **The architecture-anchor docs (`12-brute-force-summary.md`,
   `13-minimal-fix-verify.md`, `14-cleanup-sweep.md`) are excellent
   provenance — every iter-N narrative in the source has a 50-200-line
   doc explaining what was tried and why.** The natural endpoint of the
   refactor is to lift the iter-N narrative OUT of the source and INTO
   this doc trail (where it's already present); step 4 does exactly
   this. Source ends up describing current behaviour only; archaeology
   stays in `docs/orchestrate/`. This is the right scope-shape for
   "comments + control-flow tightening" refactor latitude.

6. **One foundation-level concern outside the brief's scope:** the
   `naadf_bounds_compute_node` function still carries 7 system parameters
   even after step 5. Bevy idiom would suggest grouping correlated
   resources into a single bundle (`(Res<ConstructionPipelines>, Res<ConstructionBindGroups>, Res<ConstructionGpu>, Res<ConstructionConfig>)`
   are always pulled together — they're a coherent "construction snapshot"
   group). Not in scope here ("comments + control-flow tightening" does
   not include parameter bundling), but if the W3 module ever gets a
   structural refactor, that's where the natural further-cleanup lives.

7. **Tooling note: the rtk-filtered `ls` returns "(empty)" for some
   directories that are not actually empty.** I hit this twice — once
   on the worktree's `docs/orchestrate/wasm-chunk-aadf-nondeterminism/`
   directory which DOES contain files, and once during verification of
   the anchor doc trail. Fall-back to `/bin/ls` worked. Worth surfacing
   to the orchestrator: the rtk filter has a false-empty failure mode
   on some directory listings. Not blocking for this dispatch; mentioning
   for future-architect awareness.

8. **Subjective: the brief was unusually well-prepared.** All 11 findings
   pre-classified by severity/restructure-tier, constraints repeated
   verbatim from `01-context.md`, vigilance preamble pinning the prior
   line-number-error mode, verification gates spelled out as exact
   shell commands. This is the kind of brief that makes the architect
   phase a literal target-state-write task rather than a re-derive-intent
   task. Acknowledging the orchestrator's prep work.
