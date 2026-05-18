# web-vox-color-divergence — orchestration index

Topic: diagnose-first investigation of a web-only voxel color divergence
that landed after the `web-vox-async-loading` orchestration closed.

The async-loading half is unambiguously won (`.vox` fetches, parses
off-thread on rayon, installs via `poll_pending_vox_parse`). The user
verified this live: geometry correct, voxel types correct, colors
near-black. Native renders the same fixture with full colors.

**Worktree:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming`
**Branch:** `feat/web-vox-streaming` (HEAD `okd76481c` at start)

## Files

- `00-reuse-audit.md` — read-only audit of the install / palette-upload /
  extract-prepare / change-detection landscape (24 candidates, 6
  hypothesis axes, top reuse recommendation).
- `01-context.md` — canonical context bundle (goal, Q&A decisions, audit
  summary, required reading, forbidden moves). Every non-review agent
  reads this first.
- `02-research.md` — diagnose-first observation findings (instrumentation
  log readout + verified root cause).
- `03-design.md` — fix architecture; written by `delegate-architect`.
- `04-impl.md` — implementation log; written by the implementer.
- `05-review.md` — fresh-eyes review brief (success criteria + artifact
  pointer only; deliberately excludes rationale + required reading from
  `01-context.md`).

## Execution mode

**Distributed.** Confirmed by user at Step 4 Q&A. Rationale: handoff
explicitly invokes `/diagnose-first` (observation before action);
multiple plausible fix shapes need a Q&A pause between observation and
design; render-world gating fix is moderate blast radius rather than low.

## Phase checklist

- [x] Step 1 — Scope (orchestrator chat)
- [x] Step 2 — Reuse audit (`delegate-auditor` → `00-reuse-audit.md`)
- [x] Step 2.5 — Mode selection (distributed)
- [x] Step 3 — Method preview
- [x] Step 4 — Q&A (4 decisions captured in `01-context.md`)
- [x] Step 5 — Shared-context files written
- [ ] Step 6a — Diagnose-first research dispatch (`02-research.md`)
- [ ] Step 6a hard gate — user confirms root cause from observation
- [ ] Step 6b — Architecture dispatch (`03-design.md`)
- [ ] Step 6b hard gate — user approves fix shape
- [ ] Step 6c — Implementation dispatch (`04-impl.md`)
- [ ] Step 6c hard gate — user confirms web build + native gates green
- [ ] Step 6d — Fresh-eyes review dispatch (`05-review.md`)
- [ ] Step 6d hard gate — user confirms reviewer flags reconciled
- [ ] Exit

## Q&A decisions (binding)

Captured here as one-liners; full prose in `01-context.md`.

1. **Mode**: Distributed.
2. **Diagnose posture**: Instrument first, then fix.
3. **Fix-shape**: Architect picks the fix direction; user reviews at
   design hard gate.
4. **Gate extension**: Yes — extend `assert_vox_geometry_visible` and
   `vox_web_parity_loaded` with per-channel color-spread assertion in
   this orchestration.
