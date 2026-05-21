# codebase-tightening-followup — shared context bundle

Every investigator agent reads this file first. It is self-contained:
file paths, line numbers, decisions, and constraints inlined.
Per-item drilldowns are in `00-reuse-audit.md` (audit) and the investigator's
own per-item file (output).

---

## Goal (verbatim user words from handoff)

> Investigate 5 deferred items from the codebase-tightening orchestration to
> (a) verify or refute the bailing implementors' stated blockers by reading
> the actual code, (b) diagnose the true blocker per item, (c) recommend a
> path forward per item (re-architect / re-implement / accept-as-is /
> bundle), (d) specify a per-item verification recipe.
>
> NO implementation in this orchestration — investigation only, with user
> direction-setting at the hard gate before any follow-up dispatches fire.

User constraint from handoff:

> "Do NOT begin implementing item-by-item without surfacing the
> investigation findings to the user first."

**The investigators are READ-ONLY.** No source-code edits. They may only
Write/Edit to their per-item file under
`docs/orchestrate/codebase-tightening-followup/`. They may NOT run builds
beyond reading existing build output, may NOT modify tests, may NOT touch
the codebase.

---

## The 5 deferred items (overview)

| # | item | bailing implementor's stated blocker | code surface |
|---|---|---|---|
| 1 | D5 Step 4: `prepare_construction` split | "5 cross-workstream coupling gaps in architect's §2.1" (twice deferred) | `crates/bevy_naadf/src/render/construction/` |
| 2 | D6 Steps 3+4: gate trait + driver decomp | trait `apply_edit` signature missing per-gate State resources; ~600 LOC dead trait impls if landed alone | `crates/bevy_naadf/src/e2e/gate.rs`, `bin/e2e_render.rs` |
| 3 | D4 Step 3: `ShaderType` cutover for `GpuGiParams` | architect §3.4 recipe wrong on trailing `_pad5/6/7/_pad8/9/10` non-natural std140 alignment | `crates/bevy_naadf/src/render/gpu_types.rs`, `assets/shaders/gi_params.wgsl` |
| 4 | D4 Step 5: plugin-per-subsystem | "dispatch budget" (twice deferred); no analytical blocker reported | `crates/bevy_naadf/src/render/` plugin organisation |
| 5 | `window_config.rs` → e2e dep-arrow | production reads `crate::e2e::{E2E_*, HORIZON_*, ...}` constants — backwards arrow | `crates/bevy_naadf/src/window_config.rs` |

Full audit per item: `docs/orchestrate/codebase-tightening-followup/00-reuse-audit.md`.

---

## Decisions from architectural Q&A (2026-05-21)

1. **Mode:** distributed + parallel read-only fan-out. 5 investigators dispatched in one message.
2. **Item scope:** full investigator for all 5 items (uniform deliverable shape), even though the audit's diagnoses for items 3 and 5 are already strong.

---

## Required reading per investigator (in addition to this file)

Every investigator reads:

1. `docs/orchestrate/codebase-tightening-followup/00-reuse-audit.md` — full audit.
   Within that file, **the investigator's item section is load-bearing**;
   the other items are context.
2. The investigator's per-item handoff section below.
3. The item's parent architect doc + impl log (cited per item in `00-reuse-audit.md` and inlined in each investigator's brief).
4. The item's code surfaces (cited per item).

---

## Verification discipline (binding — project CLAUDE.md)

> Never run `cargo run --bin bevy-naadf` as a verification step. It proves
> nothing the deterministic gates haven't.

Verification surface investigators may reference:

- `cargo build --workspace` — compiles
- `cargo test --workspace --lib` — unit + integration (179 passing, 1 ignored, 43 GPU lib tests blocked by host NVIDIA driver — pre-existing, not investigator's concern)
- `cargo run --bin e2e_render -- <mode>` — `baseline`, `--validate-gpu-construction`, `--validate-gpu-construction-scaled`, `--validate-gpu-construction-production-scale`, `--edit-mode`, `--entities`, `--vox-e2e`, `--small-edit-visual`, `--vox-gpu-construction`, `--vox-gpu-oracle`, `--vox-web-parity`, `--oasis-edit-visual`, `--runtime-edit-mode`, `--ssim-compare`

For non-deterministic gates (`--oasis-edit-visual`, `--vox-gpu-oracle`,
similar): verification recipes must specify **≥3 runs on the suspect side,
≥2 runs on the reference side** per memory
`feedback-multiple-runs-rule-out-false-positives`.

For e2e dispatches: wrap `cargo run` in `timeout 120s` per memory
`feedback-e2e-gates-must-fail-fast`.

The investigators **do not run** these gates themselves — they only specify
the recipes the user (or a downstream impl dispatch) would run.

---

## Forbidden moves

- **Never run `cargo run --bin bevy-naadf`.** Project CLAUDE.md is binding.
- **Faithful-port rule:** the `bevy-naadf` master branch is a minimal C# NAADF port + Unity-port footnotes. PBR lives on a separate ready branch. Do not propose PBR-restoration as part of any item's path-forward. Memory: `master-branch-identity`, `bevy-naadf-faithful-port-rule`.
- **CPU oracle (`aadf/edit.rs`) is sacred.** Keep, do not optimize away.
- **No commits without user instruction.** Investigators write to `docs/orchestrate/codebase-tightening-followup/<their-file>` only. No source edits, no builds, no commits.
- **No `--no-verify` on commits.** Hooks failing = real diagnostic signal.
- **Do not re-dispatch the same brief that already bailed.** If you recommend re-dispatch, change the framing — likely architect-first rather than implementor-first.
- **Do not trust a single implementor return.** Two implementors bailed on D5 Step 4 with the same claim; could be (a) real architect-design gap, (b) shared dead-end framing across two Opus sessions, or (c) tooling/measurement issue. Investigate before assuming.

---

## Repro / env

- Worktree: `/mnt/archive4/DEV/bevy-naadf`
- Branch: `main`
- HEAD: `2bb03d1` ("refactor(render): D4 final cleanup — prepare.rs split into prepare/{mod,frame,world}.rs + alias rename + dep-arrow inversion")
- Ahead of `origin/main`: 18 commits
- Build: `cargo build --workspace` clean
- Lib tests: `cargo test --workspace --lib` → 179 passed
- Deterministic e2e gates: all green at HEAD per D4 final-cleanup impl log
- Non-deterministic `--oasis-edit-visual`: green ×3 at HEAD per same log
- No live user visual check since orchestration started compounding follow-ups

---

## Per-investigator deliverable contract

Each investigator writes to **one** per-item file under
`docs/orchestrate/codebase-tightening-followup/`:

- `02-investigation-item-1-d5-step-4.md`
- `02-investigation-item-2-d6-gate.md`
- `02-investigation-item-3-gpugiparams.md`
- `02-investigation-item-4-plugin-per-subsystem.md`
- `02-investigation-item-5-window-config.md`

Each per-item file must contain (verbatim section headers):

1. `# Item N — <name>`
2. `## Bailing implementor's stated blocker` — quote verbatim from the impl log; cite file:line.
3. `## Verification of the claim` — what the investigator actually found in the source. Did the bailing implementor get the structural facts right? cite file:line. The audit's claims must be independently verified — do not just cite the audit.
4. `## Diagnosis` — what the *real* blocker is (or that there isn't one). Three categories:
   - (a) Real and architect-fixable.
   - (b) Real but implementor-fixable (framing was wrong).
   - (c) Not real / premise flawed / non-issue.
5. `## Proposed path forward` — pick from:
   - (a) Fresh `delegate-architect` dispatch with focused brief on <specific scope>.
   - (b) Re-dispatch implementor with corrected brief.
   - (c) Accept-as-is / close as non-issue.
   - (d) Bundle with another item into one focused orchestration.
   Justify the choice in 2-4 sentences.
6. `## Verification recipe` — exact commands (build, lib tests, e2e gates with run-count) that prove the item landed cleanly when it does land. For items in category (c), specify what would prove the "non-issue" diagnosis (e.g. a re-run + grep).
7. `## Side notes / observations / complaints` — required. Bullet anything you noticed outside the brief that the orchestrator should know.

---

## Anti-patterns this orchestration must avoid

- **Re-dispatching the same brief that bailed.** If recommending re-dispatch, the brief must change. Architect-revision-first is often the right shape.
- **Trusting the audit verbatim.** The audit is signal, not gospel. Investigators independently verify per-item claims against the source.
- **Implementing during investigation.** Investigators are read-only.
- **Roaming.** Stay scoped to your one item.
- **Conversation-relative references.** Sub-agents have no parent conversation; cite file:line and full paths.
