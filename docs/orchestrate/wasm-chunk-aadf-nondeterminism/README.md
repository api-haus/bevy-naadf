# wasm-chunk-aadf-nondeterminism — orchestration index

## Topic

Diagnose and fix a wasm32/WebGPU-only non-deterministic ray-termination bug
in bevy-naadf's voxel raymarcher. The cross-target SSIM gate at
`e2e/tests/vox-horizon-parity.spec.ts` measures values that vary run-to-run
(0.69 → 0.79 → 0.928 → 0.94) on identical inputs. Native release renders the
full Oasis city to the world-boundary ocean line; the WASM/WebGPU canvas
truncates distant geometry — distant city → ocean → sky — and the truncation
distance varies run-to-run.

## Worktree

- Path: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism`
- Branch: `fix/wasm-chunk-aadf-determinism` (branched from `main`)
- All WIP from the prior session is in this worktree (stashed off main, popped here).

## Files

| File | Owner | Status |
|------|-------|--------|
| `00-handoff-verbatim.md` | orchestrator (copied) | [x] |
| `00-reuse-audit.md` | `delegate-auditor` | [ ] |
| `02-diagnosis.md` | `general-purpose` (diagnose-first) | [ ] |
| `01-context.md` | orchestrator (post-Q&A) | [ ] |
| `03-design.md` | `delegate-architect` | [ ] |
| `04-impl.md` | `general-purpose` (implementer) | [ ] |
| `05-review.md` | `delegate-reviewer` | [ ] |

## Agent groups

- **research** — owns `00-reuse-audit.md` (existing diagnostic/probe/sync
  infrastructure in this codebase) + `02-diagnosis.md` (diagnose-first
  investigator: fresh-eyes diagnosis with the prior hypothesis class dropped
  as a bias source).
- **design** — owns `03-design.md` (fix design grounded in the confirmed
  diagnosis; cites file:line for every proposed change).
- **impl** — owns `04-impl.md` (implementation + gate runs, including a
  stability verification across 3+ Playwright runs to demonstrate the fix
  holds under the non-determinism the prior session saw).
- **review** — owns `05-review.md` (fresh-eyes verification against the
  success criteria; reviewer reads ONLY `05-review.md`, never the design
  rationale or full context).

## Execution mode

Distributed. Step 2.5 disqualified consolidated mode:

- Bounded-context FAILS — investigation is open-ended; failing subsystem is
  contested across multiple shaders, sync primitives, and Rust dispatch loops.
- Low blast radius FAILS — GPU cross-pass synchronization changes have
  already introduced new non-determinism (atomicization of
  `bound_refined_info` made things strictly worse in the prior session).
- The handoff is itself a demonstration of the failure mode consolidated
  would amplify: one agent ping-ponging between hypotheses without an
  external freshness check.

## Phase checklist

- [ ] Reuse audit (delegate-auditor)
- [ ] Diagnose-first investigation (general-purpose, read-only)
- [ ] Synthesis + architectural Q&A with user
- [ ] `01-context.md` written
- [ ] Design (delegate-architect)
- [ ] Implementation (general-purpose)
- [ ] Stability verification (3+ gate runs)
- [ ] Fresh-eyes review (delegate-reviewer)
- [ ] User visual confirmation

## Diagnose-first lineage

The handoff explicitly reports the prior session's mitigations produced
non-deterministic outcomes and the working theories contradicted each
other. Per `/delegate`'s diagnose-first circuit-breaker rule, the first
substantive dispatch is a read-only diagnostic investigator, NOT a
speculative second-pass fix. The prior diagnosis is dropped as a bias
source by the investigator's brief.
