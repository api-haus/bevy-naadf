# vox-gpu-rewrite

Port `bevy-naadf`'s `.vox` → fixed-world load path from a CPU XZ-tiling stop-gap
to a GPU dispatch chain mirroring C# `WorldData.cs:120-156`'s per-segment
`generator_model + chunk_calc` invocations. The WGSL shader
(`generator_model.wgsl`) and Rust dispatch helper
(`generator_model.rs::dispatch_generator_model`) already exist as audited W5
scaffolding — only the runtime integration into `prepare_construction` /
`naadf_gpu_producer_node` is missing.

Origin: `/tmp/naadf-vox-gpu-rewrite-handoff.md` (in-session handoff, may not
survive — every load-bearing fact is inlined into `01-context.md`).

## Mode

**Distributed.** Step 2.5 disqualified consolidated-mode on criterion 3 (low
blast radius): the production GPU dispatch path is correctness-critical; a
subtle bug in the W5.3 segment loop renders the world wrong everywhere. The
handoff also cites W1/W3/W4 precedent which used the distributed flow.

## Agent groups

| Group | Role | Subagent type | Model | Group file |
|---|---|---|---|---|
| audit | Reuse audit (find existing scaffolding) | `delegate-auditor` | inherited (Opus) | [`00-reuse-audit.md`](00-reuse-audit.md) |
| design | Architect the W5.1–W5.6 integration | `delegate-architect` | inherited (Opus) | [`02-design.md`](02-design.md) |
| impl | Land the code, run gates between subtasks | `general-purpose` | inherited (Opus) — code-mutating in production GPU path | [`03-impl.md`](03-impl.md) |
| review | Fresh-eyes verification | `delegate-reviewer` | default | [`04-review.md`](04-review.md) |

## Files

- [`README.md`](README.md) — this index
- [`00-reuse-audit.md`](00-reuse-audit.md) — reuse audit (8.3 KB; **DONE**)
- [`01-context.md`](01-context.md) — canonical context bundle (non-review agents)
- [`02-design.md`](02-design.md) — design agent output (per-subtask spec)
- [`03-impl.md`](03-impl.md) — implementer's per-subtask change log
- [`04-review.md`](04-review.md) — fresh-eyes review brief (criteria + artifact only; NO design rationale)

## Phase checklist

- [x] Step 1 — Restate + scope
- [x] Step 2 — Reuse audit dispatched + landed at `00-reuse-audit.md`
- [x] Step 2.5 — Mode selected: distributed
- [x] Step 3 — Method presented to user
- [x] Step 4 — Architectural Q&A (4 decisions captured in `01-context.md`)
- [x] Step 5 — Shared-context files written
- [x] Step 6 — Checkpoint commit + design dispatch (commit `4063d55`)
- [x] Step 6 — Design agent landed `02-design.md` (1757 lines)
- [x] Hard gate — design submitted, user confirmed
- [x] Step 6 — Checkpoint commit + impl W5.1 landed (commit `483d86b` pre-impl; W5.1 changes uncommitted pending next checkpoint)
- [ ] Hard gate — submit W5.1, wait for user  ← CURRENT
- [ ] Step 6 — Checkpoint commit + impl W5.2 (prepare_construction buffer + bind-group allocation)
- [ ] Hard gate — submit, wait
- [ ] Step 6 — Checkpoint commit + impl W5.5 (e2e gate, lands BEFORE W5.3 to catch regressions)
- [ ] Hard gate — submit, wait
- [ ] Step 6 — Checkpoint commit + impl W5.3 (segment-loop dispatch in naadf_gpu_producer_node)
- [ ] Hard gate — submit, wait (W5.5 gate should go green here)
- [ ] Step 6 — Checkpoint commit + impl W5.4 (delete CPU stop-gap)
- [ ] Hard gate — submit, wait
- [ ] Step 6 — Checkpoint commit + impl W5.6 (document default-scene divergence)
- [ ] Hard gate — submit, wait
- [ ] Step 6 — Fresh-eyes review against `04-review.md`
- [ ] Reconcile review against `01-context.md`; submit to user

Landing order rationale: W5.5 lands BEFORE W5.3 so the e2e gate exists to catch
regressions the moment the segment loop lands.

## Followups (out of scope for this PR)

- `w3-startup-convergence-race` — bounds_calc pipeline-compile latency (~12
  frames) + W3 AADF convergence (~7 frames) means rays single-step for the
  first ~330 ms. **Out of scope per handoff.** File as a separate topic.
