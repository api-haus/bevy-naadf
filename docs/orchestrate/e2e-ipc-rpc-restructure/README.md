# Orchestration: e2e-ipc-rpc-restructure

## Goal

Restructure the bevy-naadf e2e harness so the production app is the system-under-test
driven externally via IPC-RPC, rather than e2e scenarios being baked into the app as
in-app driver modes. A test runner spawns the real app in another process and controls
it through an RPC-over-IPC functional interface that exposes enough surface to automate
any/all e2e scenarios as bodies of test cases.

## Worktree

All work happens in `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build`,
branch `feat/android-build`.

## Agent groups

- **audit** — re-implementation reuse search. Owns `00-reuse-audit.md`.
- **design** — IPC-RPC architecture. Owns `02-design.md`.
- **impl** — implementation, if the orchestration proceeds past design. Owns `03-impl.md`.
- **review** — opt-in only, not a default phase.

## Shared-context files

- `README.md` — this index.
- `00-reuse-audit.md` — auditor deliverable.
- `00-control-layer-survey.md` — Bevy control-layer survey (answered "BRP vs custom RPC").
- `01-context.md` — canonical context bundle for all non-review agents.
- `02-design.md` — architect deliverable.
- `03-impl.md` — implementation log (created lazily if impl proceeds).

## Decision log

- Control layer = **Bevy Remote Protocol (BRP)** — first-party, ships inside the
  project's pinned `bevy = "=0.19.0-rc.1"`. Not a from-scratch custom RPC stack.
- Scope = the **13 booted-window e2e gates**; the 9 already-headless validators
  are out of scope. Native-only (no wasm/Android e2e).
- Distributed mode, design phase first → hard gate → implementation phase.

## Phase checklist

- [x] Step 2 — re-implementation audit
- [x] Control-layer survey (BRP vs custom RPC — user chose BRP)
- [x] Step 2.5 — execution mode selection (distributed)
- [x] Step 3/4 — method presented + architectural framing
- [x] Step 5 — `01-context.md` written
- [ ] Design phase
- [ ] Implementation phase (scope confirmed at the design hard gate)
