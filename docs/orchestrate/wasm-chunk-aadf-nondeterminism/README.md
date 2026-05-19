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
| `01-diagnostics-design.md` | `general-purpose` (research + design) | [ ] |
| `02-diagnostics-impl.md` | `general-purpose` (implementer) | [ ] |
| `03-diagnosis.md` | `general-purpose` (diagnose-first, fresh eyes) | [ ] |
| `04-context.md` | orchestrator (post-diagnosis Q&A) | [ ] |
| `05-fix-design.md` | `delegate-architect` | [ ] |
| `06-fix-impl.md` | `general-purpose` (implementer) | [ ] |
| `07-review.md` | `delegate-reviewer` | [ ] |

## Agent groups

- **diagnostics** — owns `01-diagnostics-design.md` (map of wgpu's adapter/
  device introspection surface, catalog of Chrome/Dawn/WebGPU vs Vulkan/
  WebGPU semantic divergences, concrete logging-package design) +
  `02-diagnostics-impl.md` (the implementation log + a pointer to the
  collected diagnostic-data artifacts on disk for both native and web).
- **diagnose-first** — owns `03-diagnosis.md` (fresh-eyes diagnosis grounded
  in the collected diagnostic data + code; prior hypothesis class is dropped
  as a bias source).
- **fix-design** — owns `05-fix-design.md` (fix design grounded in the
  confirmed diagnosis; cites file:line for every proposed change).
- **fix-impl** — owns `06-fix-impl.md` (implementation + gate runs, including
  a stability verification across 3+ Playwright runs to demonstrate the fix
  holds under the non-determinism the prior session saw).
- **review** — owns `07-review.md` (fresh-eyes verification against success
  criteria; reviewer reads ONLY `07-review.md`, never the design rationale
  or full context).

## Why no reuse audit

The user redirected at orchestration-start: "reuse auditor for diagnostic
round isnt viable." For diagnose-first work the load-bearing first move is
collecting fresh diagnostic data, not surveying existing code for reuse —
the prior session already had reuse-shaped knowledge of the codebase and
still produced contradictory hypotheses. The audit-shaped question is
implicit in the diagnostic-package design phase (the design agent must
identify and integrate with existing probe/diagnostic hooks rather than
inventing parallel ones).

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

- [ ] Diagnostic-package research + design (read-only)
- [ ] Diagnostic-package implementation + data collection (native + web)
- [ ] Hard gate — present collected diagnostic data to user
- [ ] Diagnose-first investigation grounded in collected data (read-only)
- [ ] Synthesis + architectural Q&A with user
- [ ] `04-context.md` written
- [ ] Fix design (delegate-architect)
- [ ] Fix implementation (general-purpose)
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
