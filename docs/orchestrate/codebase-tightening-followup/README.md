# codebase-tightening-followup — orchestration index

Follow-up investigation orchestration for 5 deferred items from the parent
`codebase-tightening` orchestration. Investigation-only — no implementation
in this orchestration; per-item user direction sets downstream work.

Parent orchestration: `docs/orchestrate/codebase-tightening/` (8 domains,
HEAD `2bb03d1`).
Handoff source: `/tmp/codebase-tightening-followup-handoff.md`.

## Files

| file | purpose | written by |
|---|---|---|
| `README.md` | this index | orchestrator |
| `00-reuse-audit.md` | reuse-and-verification audit across the 5 items | `delegate-auditor` |
| `01-context.md` | shared context bundle (rules, forbidden moves, items overview) | orchestrator |
| `02-investigation-item-1-d5-step-4.md` | item 1 investigator findings | investigator (item 1) |
| `02-investigation-item-2-d6-gate.md` | item 2 investigator findings | investigator (item 2) |
| `02-investigation-item-3-gpugiparams.md` | item 3 investigator findings | investigator (item 3) |
| `02-investigation-item-4-plugin-per-subsystem.md` | item 4 investigator findings | investigator (item 4) |
| `02-investigation-item-5-window-config.md` | item 5 investigator findings | investigator (item 5) |

## Agent groups

- **audit** — re-implementation / verification-infrastructure audit. *Done.*
- **investigation** — 5 parallel read-only investigators, one per item.
  Each verifies the bailing implementor's claim against the actual source,
  diagnoses the real blocker, recommends a path forward, and writes a
  per-item verification recipe.
- *(Conditional, post user-direction)* **design** / **impl** — per-item
  fresh dispatches the user elects to action. NOT part of this orchestration.

## Phase tracker

- [x] Step 1 — restate & scope
- [x] Step 2 — reuse audit (`00-reuse-audit.md`)
- [x] Step 2.5 — mode selection (distributed + parallel read-only fan-out)
- [x] Step 3 — present method to user
- [x] Step 4 — architectural Q&A
- [x] Step 5 — write shared-context files (`README.md`, `01-context.md`)
- [x] Step 6 — dispatch 5 parallel investigators (checkpoint `a33567e`); all 5 returned
- [x] Step 7 — synthesize per-item findings at hard gate; item 5 closed (commit `e9a6e3e`)
- [x] Architect-revision phase — 4 parallel `delegate-architect` dispatches; all 4 landed (`03-architect-revision-item-{1..4}.md`)
- [x] **Orchestration exit (2026-05-21):** user elected to stop at architect-revision deliverable. Implementor dispatches deferred to separate sessions per-item. All 5 investigation files + 4 architect-revision files + item 5 close (commit `e9a6e3e`) are the durable artefact.

## Pick-up notes for downstream sessions

Each remaining item is unblocked and lift-ready:

- **Item 1** — single implementor dispatch. Brief: lift the ≤500-word §2.1 addendum in `03-architect-revision-item-1.md` into D5 implementation. Verify against 5 assumptions before landing.
- **Item 2** — single implementor dispatch. Brief: lift the 5-item revised Step 3 spec from `03-architect-revision-item-2.md`. Pre-flight verify the 5 flagged assumptions (especially A-1: Apply arms count; A-3: Bevy 0.19 API surface; A-5: cross-gate signal timing across `resource_scope`).
- **Item 3** — single implementor dispatch. Brief: lift the full §3.4 from `03-architect-revision-item-3.md` (includes 10-step lift instructions + 6-struct cutover table).
- **Item 4** — 7-dispatch sequence per the decomposition plan in `03-architect-revision-item-4.md`. Start with Dispatch 0 (D5 SystemSets prerequisite). Single PR with 7 commits per the bisect-discipline call.

Items 1 and 4 are non-blocking on each other and can land in either order or parallel.

## Execution mode

**Distributed + parallel read-only fan-out.** All 5 investigators dispatched
in one message; each writes to its own per-item file (no write races); each
read-only by design. User-confirmed in Step 4 Q&A.
