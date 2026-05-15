# taa-resize-blackness

## Goal
Fix the bug where **shadow regions render pitch black** after the window is resized while TAA (temporal anti-aliasing) is enabled in `bevy-naadf`.

**First deliverable is a failing reproduction test**, before any fix work (strict TDD, confirmed by user at Step-4 Q&A).

## Anchor context (prior work)
`docs/orchestrate/naadf-bevy-port/` — Bevy port history. Esp.:
- `18-taa-fidelity.md` — the TAA implementation pass.
- `20-impl-phase-d-shadow-A.md` — the shadow-rendering implementation.
- `01-context.md` — port-wide canonical context.

## Execution mode
**Distributed.** User's TDD redirect ("set up a test that reproduces it") asks for a design-approval *and* test-failure gate before the fix; consolidated mode (one uninterrupted pass) would defeat that pacing. The cost (trace loss across handoffs) is acceptable for a single-bug scope.

## Agent groups
- **research / audit** — `delegate-auditor` (read-only). Searches `bevy-naadf` for TAA history textures, shadow rendering, resize handlers, AND inventories test infrastructure. Reads prior orchestrate docs. → `00-reuse-audit.md`.
- **design** — `delegate-architect`. Designs the repro test shape AND the fix. → `02-design.md`.
- **impl-A (test)** — `general-purpose` (Opus). Writes the failing repro test, runs once, reports failure mode, stops. → `03a-impl-test.md`.
- **impl-B (fix)** — `general-purpose` (Opus). Writes the fix, runs the test once, stops. → `03b-impl-fix.md`.
- **review** — `delegate-reviewer`. Fresh-eyes review against `04-review.md` criteria.

## Phase checklist
- [x] Phase 1 — Audit (`00-reuse-audit.md`) — auditor `a438446f25a5a8ecf`, checkpoint `2b1d2cf`
- [x] Phase 2 — Write `01-context.md` + `04-review.md` from audit + Q&A answers
- [ ] Phase 3 — Design (`02-design.md`)
- [ ] **HARD GATE** — user confirms design
- [ ] Phase 4 — Impl-A: write failing test (`03a-impl-test.md`)
- [ ] **HARD GATE** — user confirms test fails as expected
- [ ] Phase 5 — Impl-B: write fix (`03b-impl-fix.md`)
- [ ] **HARD GATE** — user confirms test passes
- [ ] Phase 6 — Review (`05-review.md`)
- [ ] Final hard gate — user signs off

## Files
- `README.md` — this file
- `00-reuse-audit.md` — auditor output (Phase 1)
- `01-context.md` — canonical context bundle for non-review agents (written post-audit)
- `02-design.md` — architect output
- `03a-impl-test.md`, `03b-impl-fix.md` — implementer logs
- `04-review.md` — fresh-eyes brief (criteria + artifact pointer only; review agent reads this and NOT `01-context.md`)
- `05-review.md` — reviewer output

## User decisions (Step-4 Q&A)
- **Bug repro:** "the shadows become pitch black" (shadow regions go to zero after window resize while TAA is on).
- **Context anchor:** `docs/orchestrate/naadf-bevy-port/` (typo `orhcestrate` in user input).
- **TDD gating:** Strict — write test → hard gate → write fix.
