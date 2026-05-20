# codebase-tightening — orchestration index

**Goal**: tighten bevy-naadf — IoC + idiom-fit first, LOC reduction as consequence — by parallel domain-scoped exploration + architecture, then sequential implementor dispatches.

**Mode**: distributed with parallel read-only fan-out for analytics (rule 8), strictly sequential for code-mutating impl.

**Date opened**: 2026-05-20.

## Files

- `00-reuse-audit.md` — auditor output (LOC comparison, domain decomposition, crosscutting reuse map). Authoritative for scope.
- `01-context.md` — canonical context bundle every non-review agent reads first.
- `<domain>/02-exploration.md` — per-domain `refactor-explorer` output (orchestrator does NOT read).
- `<domain>/03-architecture.md` — per-domain `refactor-architect` output (orchestrator does NOT read).
- `<domain>/04-refactoring.md` — per-domain `refactor-implementer` execution log.

## Agent groups

| group | agents | model | concurrency |
|---|---|---|---|
| audit | `delegate-auditor` ×1 | inherited (Opus) | n/a — done |
| analytics-explore | `refactor-explorer` ×8 (one per domain) | inherited (Opus) | **parallel batch** |
| analytics-architect | `refactor-architect` ×8 (one per domain) | inherited (Opus) | **parallel batch** (after all explorers done) |
| impl | `refactor-implementer` ×N (sequenced) | inherited (Opus) | **strictly sequential** |
| checkpoint | `general-purpose` commit agent | sonnet | before each substantive dispatch |

## Domain list (audit §2)

| # | slug | LOC | dir |
|---|---|---|---|
| D1 | `aadf-data-structures` | 6 470 | `aadf-data-structures/` |
| D2 | `editor-and-settings-ui` | 3 120 | `editor-and-settings-ui/` |
| D3 | `voxel-io-and-grid` | 5 790 | `voxel-io-and-grid/` |
| D4 | `render-pipeline` | 13 665 | `render-pipeline/` |
| D5 | `gpu-construction` | 18 405 | `gpu-construction/` |
| D6 | `e2e-and-playwright` | 12 725 | `e2e-and-playwright/` |
| D7 | `app-and-camera` | 2 396 | `app-and-camera/` |
| D8 | `asset-pipeline` | 1 161 | `asset-pipeline/` |

## Impl phase order (user-decided, Q&A)

1. **D5** — `gpu-construction` (biggest single win; splitting `render/construction/mod.rs` 11 043 → ~2.5k core + extracted subdirs).
2. **D4** — `render-pipeline` (lands onto a cleaned-up construction-side surface).
3. **D1, D2, D3, D6, D8** — interleave (architect docs land first; orchestrator picks order from there).
4. **D7** — `app-and-camera` last (touches all other domains' `Plugin`s).

## Phase checklist

- [x] `00` — audit
- [x] `01` — context bundle
- [ ] `02` — explorers (D1..D8, parallel batch)
- [ ] `03` — architects (D1..D8, parallel batch)
- [ ] `04` — implementor D5
- [ ] `04` — implementor D4
- [ ] `04` — implementor D1
- [ ] `04` — implementor D2
- [ ] `04` — implementor D3
- [ ] `04` — implementor D6
- [ ] `04` — implementor D8
- [ ] `04` — implementor D7

## Orchestrator discipline

- Per user directive: **orchestrator does NOT read `<domain>/02-exploration.md` or `<domain>/03-architecture.md`**. The implementor agents read those directly. The orchestrator's only direct reads are `00-reuse-audit.md` (done), `01-context.md` (this), agent return status lines, and the impl log for verification confirmation.
- Per `feedback-vigilance-preamble-for-cg-work`: every brief opens with "This is a significant task in computer graphics — be vigilant; verify every file:line ref with Read/Grep before citing".
- Per `feedback-multiple-runs-rule-out-false-positives`: impl agents must re-run e2e gates ≥2× on non-deterministic gates (oasis_edit_visual, vox_gpu_oracle).
- Per `bevy-naadf-faithful-port-rule`: no behavioural divergence from C# NAADF without explicit user approval + docs entry.
