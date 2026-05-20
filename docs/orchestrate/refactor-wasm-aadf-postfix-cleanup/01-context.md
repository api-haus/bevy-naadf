# 01-context — refactor-wasm-aadf-postfix-cleanup

This is the canonical context bundle every agent in this refactor reads first.

## Restated goal (verbatim from invocation)

> Three-item structural cleanup of the wasm-chunk-aadf-nondeterminism
> fix's artifacts (4 commits landed: `a426441 + 960eeb2 + c6b0deb +
> 1fdd256`):
>
> 1. `naadf_bounds_compute_node` cleanup — coherent docblock + tightened
>    control flow.
> 2. chunks-RMW + 18% parity — explore-only; propose structural fix OR
>    document accepted tradeoff.
> 3. `tests.rs` probe-buffer hardcode — couple to production const.

## Target scope (exact paths)

Primary targets — these are the files the explorer + architect + implementer
work inside:

- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` — host of
  `naadf_bounds_compute_node` (item 1 + relevant to item 2's chunks-write
  site analysis).
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` —
  has the hardcoded probe-buffer at line ~529 (item 3).
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` — chunks-RMW
  write site + chunks_mirror RO bindings (item 2 surface).
- `crates/bevy_naadf/src/render/construction/mod.rs` — `PREPARE_PROBE_HISTORY_ENTRIES`
  const + chunks_mirror_buffer allocation (item 3 + item 2 surface).
- `crates/bevy_naadf/src/render/construction/config.rs` — already
  updated in `c6b0deb`; the architect may want to verify the docblock
  story stays coherent after item 1's changes.

Out-of-scope: every other file in the workspace. No moves, no API breaks,
no dependency changes.

## User constraints from Q&A

**Question 1 — Item 2 commit policy:** "Allow small+obvious+low-risk
fixes if surfaced." If the architect identifies a clear-cut improvement
(e.g. "this 5-line restructure removes the mixed atomic/non-atomic view
fragility AND moves parity from 18% to 30%+ with no behavioral change"),
the implementer applies it. Larger restructures still escape to a
separate session via the architect's recommendation. The architect's
deliverable should classify each item-2 finding as one of:

- **EXPLORE-ONLY** — document analysis, no implementer action.
- **SMALL+OBVIOUS+LOW-RISK** — implementer applies + runs gates.
- **ESCAPE** — too large for this refactor; architect names the scope
  for a separate session.

**Question 2 — Item 1 restructure scope:** "Comments + control-flow
tightening." Item 1 includes:

- Docblock rewrite (the 250+ line function gets ONE coherent top-level
  docblock explaining the wasm regime-2 cross-frame propagation pattern).
- Minor control-flow tightening: collapse dead `let _ = ...` patterns,
  consolidate cfg branches, extract natural-boundary sub-blocks into
  helpers.
- All behavior preserved; verified via the 3-run e2e gate.

NOT in scope for item 1: full structural rewrite, splitting into multiple
functions across files, renaming public API, moving chunks_mirror into a
shared utility (these would qualify as "full restructure" — option C
that the user rejected).

## Architectural anchor

The fix mechanism is documented across these prior-orchestration files —
the architect reads them to understand WHY the current code is shaped
the way it is:

- `docs/orchestrate/wasm-chunk-aadf-nondeterminism/12-brute-force-summary.md`
  — the WIN dispatch's mechanism description (n_bounds_rounds=1 +
  chunks_mirror cross-frame propagation).
- `docs/orchestrate/wasm-chunk-aadf-nondeterminism/13-minimal-fix-verify.md`
  — confirms chunks_mirror is load-bearing (iter-2 NOT inert); iter-3
  atomicStore was inert (reverted in `960eeb2`).
- `docs/orchestrate/wasm-chunk-aadf-nondeterminism/14-cleanup-sweep.md`
  — 10/10 SSIM PASS sweep confirming minimal-fix stability + the
  bimodal trajectory observation (8 runs in lucky cluster ~0.93, 2
  runs marginal ~0.91).
- `docs/orchestrate/wasm-chunk-aadf-nondeterminism/00-handoff-verbatim.md`
  — original bug context + FORBIDDEN MOVES list (non-negotiable
  constraints; same as below).

No paper / external doc anchors this; the architect designs from first
principles + the orchestration-doc trail.

## Verification gates (exact commands)

For ANY change to `bounds_calc.rs`, `bounds_calc.wgsl`, or `mod.rs`:

```bash
cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-aadf-postfix-cleanup

# 1. Native compile + unit tests
timeout 120s cargo check --workspace
timeout 300s cargo test -p bevy-naadf --lib

# 2. Native e2e baseline (≥2 runs, deterministic on native)
for i in 1 2; do
  timeout 300s cargo run --release --bin e2e_render -- --vox-horizon-native
done

# 3. Web build
timeout 1500s just web-build-release

# 4. Web e2e parity (≥3 runs, ALL must SSIM ≥ 0.91)
for i in 1 2 3; do
  cd e2e && timeout 240s npx playwright test vox-horizon-parity.spec.ts --headed
  cd ..
done
```

If ANY web run drops below SSIM 0.91, the refactor regressed something.
Stop, document in `04-refactoring.md`, and pause for orchestrator
direction. Do NOT push through.

For item 3 (test hardcode) only: gate 1 is sufficient if the change is
purely numerical-constant alignment with no behavioral effect.

## Forbidden moves (non-negotiable)

- DO NOT lower `HORIZON_SSIM_SIMILARITY_MIN` (currently 0.91).
- DO NOT raise `MAX_RAY_STEPS_PRIMARY`.
- DO NOT raise `WASM_MAX_GROUP_BOUND_DISPATCH` from 4096.
- DO NOT touch the `n_bounds_rounds = 1` wasm clamp in
  `crates/bevy_naadf/src/render/construction/config.rs:From<&AppArgs>`.
  That's the LOAD-BEARING config change.
- DO NOT remove the `chunks_mirror` per-encoder `copy_buffer_to_buffer`
  infrastructure. It may be restructured (rename, extract, comment
  rewrite) but the TRANSFER-stage cross-frame visibility mechanism
  stays.
- DO NOT commit (orchestrator handles checkpoints between phases).
- DO NOT push.
- DO NOT touch `crates/bevy_naadf/assets/test/oasis.cvox`. It shows
  as `M` in `git status` due to a known LFS clean-filter ghost (sha256
  of working-tree content matches indexed blob; `git lfs status` reports
  `Git: 1696006 -> File: 1696006`). The orchestrator's checkpoint
  sub-agent uses `git update-index --skip-worktree` to exclude it from
  commits. No agent in this refactor touches the cvox file.
- DO NOT modify the `[probe1-call]`, `[cpu-gpu-parity]`, `[aadf-probe]`,
  `[device-snapshot]` diagnostic instrumentation outside of the scope
  items above. They're independent surfaces from the fix mechanism.
- DO NOT `npx playwright install`. System Chrome is used via
  `channel: "chrome"` in `e2e/playwright.config.ts`.

## Project conventions

From the project's `CLAUDE.md` (verification discipline, binding):

> Never run `cargo run --bin bevy-naadf` as a "verification" step. It
> boots a windowed app for 30 seconds and proves nothing the
> deterministic gates haven't already proven. Verification surface is
> `cargo build --workspace` + `cargo test --workspace --lib` + named
> e2e gates.

> The user does the live visual check on the binary. Don't pre-empt it.

From the global `CLAUDE.md` (smell-driven escape):

> If you read code that's obviously rotten — conflated concerns, IoC
> violations, accidentally-global state, two addressing schemes for
> the same buffer, "dead memory nobody reads", a one-shot offline
> mechanism shoehorned into a streaming context, abstractions that
> fight the standard pipeline for the domain — you have explicit
> permission AND mandate to **stop the in-scope task and call the
> smell.** Every deliverable doc must include a `## Side notes /
> observations / complaints` section.

## What's in the working tree when this refactor starts

Branched from `1fdd256`. The dispatched fix is in:

- `crates/bevy_naadf/src/render/construction/config.rs:215-247` — the
  `WASM_MAX_GROUP_BOUND_DISPATCH` const + its updated docblock
  describing the post-fix mechanism + the `From<&AppArgs>` wasm clamp.
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` — chunks +
  chunks_mirror bindings, `prepare_group_bounds`, `compute_group_bounds`,
  `end_of_encoder_noop` entry points, `[probe1-call]` instrumentation.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` —
  `naadf_bounds_compute_node` (the target of item 1), the W3
  per-round encoder+submit loop with iter-2 chunks_mirror copy
  infrastructure, the bind-group layout.
- `crates/bevy_naadf/src/render/construction/mod.rs` —
  `PREPARE_PROBE_HISTORY_ENTRIES = 256`, `chunks_mirror_buffer`
  allocation, the W3 dispatch chain.
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` —
  the `W3Fixture` unit test surface; line ~529 has the hardcoded
  probe-buffer at `2048 * 16` (item 3 target — verify exact line).
- `crates/bevy_naadf/src/render/gpu_types.rs` — `GpuBoundQueueInfo`
  and other GPU layout types.
- `crates/bevy_naadf/src/diagnostics.rs` — DeviceSnapshotPlugin +
  CPU-vs-GPU parity instrumentation + probe-1B sentinel emission.
- `e2e/tests/vox-horizon-parity.spec.ts` — the load-bearing e2e gate.

## Phase outputs (where each agent writes)

- Explorer → `docs/orchestrate/refactor-wasm-aadf-postfix-cleanup/02-exploration.md`
- Architect → `docs/orchestrate/refactor-wasm-aadf-postfix-cleanup/03-architecture.md`
- Implementer → `docs/orchestrate/refactor-wasm-aadf-postfix-cleanup/04-refactoring.md`
