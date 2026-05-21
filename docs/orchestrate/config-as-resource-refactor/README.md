# Orchestration — configuration-as-resource refactor

## Topic
Decompose `AppArgs` from a runtime-read god-resource into per-domain Bevy resources, following the user's stated principle: *"args insert resources, app consumes resources. The concept of args only makes sense during application bootstrap; any application code outside of bootstrap domain must not read the configuration conceptualising anything as 'args' — this is just bad software design."*

## Mode
**Consolidated, Research → Architect shape.** One 1M-context Opus agent runs investigation → diagnosis → design → migration plan → verification surface in a single uninterrupted trace. Design-only — NO code lands this orchestration; implementation is a downstream orchestration the user scopes after approving this design.

## Files

- `00-reuse-audit.md` — auditor's enumeration of existing precedent resources, extract patterns, CLI parsers, AppArgs shape tallies, and borderline calls requiring design decisions. **Status: ✓ written.**
- `01-context.md` — canonical context bundle for the consolidated agent (handoff verbatim + Q&A decisions + required-reading map + the parameter / mode / action-verb taxonomy). **Status: ✓ written.**
- `02-design.md` — the consolidated agent's deliverable: investigation findings + diagnosis + proposed design + migration plan + verification surface. **Status: pending.**

## Phase checklist

- [x] Step 1 — Restate and scope
- [x] Step 2 — Re-implementation audit (delegate-auditor → `00-reuse-audit.md`)
- [x] Step 2.5 — Select execution mode (consolidated, Research → Architect)
- [x] Step 3 — Present method to user
- [x] Step 4 — Architectural Q&A (4 questions answered)
- [x] Step 5 — Write shared-context files (`README.md` + `01-context.md`)
- [ ] Step 6 — Dispatch consolidated agent (Research → Architect)
- [ ] Step 7 — User review of design at hard gate (visual / architectural choice surface)
- [ ] Step 8 — Exit (design approved; implementation is a downstream orchestration)
