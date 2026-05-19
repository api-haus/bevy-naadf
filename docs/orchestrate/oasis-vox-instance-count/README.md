# Oasis .vox instance-count parity (Issue #1)

## Topic
The Bevy port loads **~2.5** modulo-wrapped instances of the Oasis `.vox` asset, while the original C#/MonoGame NAADF renders **exactly 4**. Diagnose, fix to match C# exactly.

(Issue #2 — grazing-angle ray termination — is held for a separate `/delegate`. Do not address here.)

## Agent groups & files

| File | Owner | Purpose |
|---|---|---|
| `01-context.md` | orchestrator | Canonical context bundle — every non-review agent reads first |
| `00-reuse-audit.md` | `delegate-auditor` | Bevy-side audit: where Oasis .vox is loaded, scene-extent constants, wrap/modulo path |
| `02-csharp-reference.md` | `general-purpose` (sonnet) | C#-side reference scan: equivalent path in `/mnt/archive4/DEV/NAADF/NAADF/` |
| `03-design.md` | `delegate-architect` | Minimal-fix design (or refactor-to-SSoT if multiple divergent constants found) |
| `04-review.md` | orchestrator | Fresh-eyes review brief — criteria + artifact pointer only |
| `05-impl.md` | `general-purpose` (Opus) | Implementation log |
| `06-review-findings.md` | `delegate-reviewer` | Review report |

## Phase checklist

- [x] Phase 1 — Parallel investigation (Bevy audit + C# reference) — read-only, parallel batch
- [x] Phase 2 — Design
- [x] Phase 3 — Implementation (build clean; 200 tests pass, 1 ignored; all design assumptions held)
- [x] Phase 4a — Manual QA #1: 4×4 instance count visually confirmed ✓
- [x] Phase 4b — Palette diagnostic (`06-palette-diagnostic.md`): off-by-one in synthetic slot-0 injection
- [x] Phase 4c — Palette fix (`07-palette-fix-impl.md`): option α applied; gates green
- [ ] Phase 5 — Manual QA #2: user re-verifies palette correctness (no blue palm trees)
- [ ] Phase 6 — Review (optional)

## Verification rule
User performs the visual side-by-side check on the binary. No new e2e gate added (user choice in Step 4 Q&A). Agents do NOT run `cargo run --bin bevy-naadf` as a verification step (project CLAUDE.md rule).
