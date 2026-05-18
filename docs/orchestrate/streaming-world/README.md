# streaming-world — procedural generation + sliding-window residency

Orchestration topic. Goal: implement procedural voxel-world generation with a
sliding-window residency layer that streams chunks into a fixed-VRAM budget,
laying groundwork for large/infinite coordinate systems and a future streamable
sparse-voxel world format (`.vox`, Minecraft conversions). **This session
scope:** procedural-noise generation feeding the sliding window. Pre-made-world
import is out of scope but the design must not preclude it.

## Mode

**Distributed.** Renderer-touching, high blast radius, design-approval gate
needed before code lands. Per Step 2.5: criteria 1 (bounded context), 3 (low
blast radius), 4 (tight design↔impl coupling) all fail for this work →
consolidated mode disqualified.

## Files

| File | Owner | Purpose |
|---|---|---|
| `README.md` | orchestrator | this file — index + phase checklist |
| `00-reuse-audit.md` | `delegate-auditor` | reuse candidates / gaps / borderline / forbidden |
| `01-context.md` | orchestrator | canonical context for non-review agents (goal, Q&A decisions, required reading, forbidden moves) |
| `02-design.md` | `delegate-architect` | the design — `## Design`, `## Decisions & rejected alternatives`, `## Assumptions made` |
| `03-impl.md` | `general-purpose` impl agent | implementation log — what changed by file, verification gates run |
| `04-review.md` | orchestrator | fresh-eyes review brief (criteria + artifact pointer ONLY; no rationale) |
| `05-review-findings.md` | `delegate-reviewer` | review findings against `04-review.md` |

## Agent groups

| Group | Subagent type | Reads | Writes |
|---|---|---|---|
| audit | `delegate-auditor` | repo | `00-reuse-audit.md` |
| design | `delegate-architect` | `01-context.md`, `00-reuse-audit.md`, repo, reference project | `02-design.md` |
| impl | `general-purpose` | `01-context.md`, `00-reuse-audit.md`, `02-design.md` (Design + Decisions + Assumptions) | code + `03-impl.md` |
| review | `delegate-reviewer` | **only `04-review.md`** (deliberately denied the design rationale) | `05-review-findings.md` |

## Phase checklist

- [x] 00 — Reuse audit
- [x] Step 2.5 — Mode selection (distributed)
- [x] Step 4 — Architectural Q&A
- [x] Step 5 — Shared-context files (`README.md`, `01-context.md`)
- [ ] 02 — Architecture design (`delegate-architect` → `02-design.md`)
- [ ] **Hard gate** — submit design to user, wait for confirmation
- [ ] 03 — Implementation (`general-purpose` → code + `03-impl.md`)
- [ ] **Hard gate** — submit impl to user, wait for confirmation
- [ ] 04 — Fresh-eyes review brief (`04-review.md` written by orchestrator)
- [ ] 05 — Fresh-eyes review (`delegate-reviewer` → `05-review-findings.md`)
- [ ] **Hard gate** — synthesise review against `01-context.md`, submit to user

## Q&A decisions (from Step 4)

| Question | Choice |
|---|---|
| Coordinate widening | Residency-only `i32` widening — GPU bind layout stays `(cx:11,cy:10,cz:11)` window-local |
| Residency unit | Per-segment (16×16×16 chunks) |
| Block dedup | Per-resident-chunk-local |
| Noise backend | `voxel_noise` (cross-platform: native + web-workers via existing JS bridge) |
