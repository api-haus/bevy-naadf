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

- [ ] Phase 1 — Parallel investigation (Bevy audit + C# reference) — read-only, parallel batch
- [ ] Phase 2 — Design
- [ ] Phase 3 — Implementation
- [ ] Phase 4 — Review
- [ ] Phase 5 — User visual confirmation (binary side-by-side vs C#)

## Verification rule
User performs the visual side-by-side check on the binary. No new e2e gate added (user choice in Step 4 Q&A). Agents do NOT run `cargo run --bin bevy-naadf` as a verification step (project CLAUDE.md rule).
