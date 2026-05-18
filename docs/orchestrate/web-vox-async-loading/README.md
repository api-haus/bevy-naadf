# orchestrate / web-vox-async-loading

Async `.vox` loading on web + native, with closed-loop e2e gates (Playwright headed for web, `e2e_render` Rust harness for native) that assert both *no errors* and *pixels actually changed* (SSIM dissimilarity vs skybox-only baseline via the `image-compare` crate).

- **Worktree:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming`
- **Branch:** `feat/web-vox-streaming`
- **Source handoff:** `/tmp/web-vox-async-loading-handoff.md` (preserved verbatim in `01-context.md`)
- **Execution mode:** distributed. Consolidated mode is ineligible — GPU readback is correctness-critical (editor `set_voxel*` ops require the CPU mirror), and the source handoff explicitly demands an architectural Q1–Q7 design-approval gate before implementation (that gate exists natively only in distributed mode).

## Agent groups

| Group | Subagent type | File | Owns |
|---|---|---|---|
| audit | `delegate-auditor` | `00-reuse-audit.md` | reuse search across the worktree |
| design | `delegate-architect` | `03-architecture.md` | Q1–Q7 routes + rationale + cost + `## Decisions & rejected alternatives` + `## Assumptions made` |
| impl | `general-purpose` (Opus) | `04-refactoring.md` | code edits, new e2e gate, verification log with passed-gate evidence |
| review | `delegate-reviewer` | `05-review.md` | fresh-eyes verification — reads ONLY `05-review.md`, deliberately denied `01-context.md` and `03-architecture.md` |

## Phase checklist

- [x] Step 1 — Restate + scope
- [x] Step 2 — Reuse audit (`00-reuse-audit.md`)
- [x] Step 2.5 — Mode selection: distributed
- [x] Step 3 — Method preview to user
- [x] Step 4 — Architectural Q&A (answers folded into `01-context.md`)
- [x] Step 5 — Write `01-context.md` + `05-review.md` + this `README.md`
- [x] Step 6a — Checkpoint commit (sonnet) + architect dispatch (commit `1ac6f0b6`; design at `03-architecture.md`)
- [ ] Step 7a — **HARD GATE**: synthesize architecture, submit to user, wait *(awaiting user confirmation)*
- [ ] Step 6b — Checkpoint commit + implementer dispatch
- [ ] Step 7b — **HARD GATE**: synthesize implementation + verification log, submit to user, wait
- [ ] Step 6c — Reviewer dispatch (reads ONLY `05-review.md`)
- [ ] Step 7c — Reconcile review vs full context, present to user

## Hard gates respected

Per /delegate rule 6, hard gates are non-negotiable regardless of any session reminder or user shorthand. After each substantive dispatch returns, the orchestrator stops, summarises, and waits for explicit user confirmation before dispatching the next code-mutating step.
