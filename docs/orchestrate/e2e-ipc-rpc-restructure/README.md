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
- [x] Design phase — `02-design.md` complete
- Implementation phase (`03-impl.md`):
  - [x] Phase 0 — transport spike (A1 + A2 confirmed)
  - [x] Phase 1 — BRP server scaffold (3 verbs, all gates green)
  - [x] Phase 2 — full verb set + `naadf_e2e` crate + `oasis_edit_visual` (dual-path green)
  - Phase 3 — migrate remaining gates:
    - [x] Phase 3a — 6 gates migrated (dual-path green)
    - [x] Phase 3b — 4 special gates + `nodes_dispatched` verb — 13/13 gates dual-path green
  - [ ] Phase 4 — repoint Playwright cross-target gate
  - [ ] Phase 5 — delete legacy harness

## Phase 5 carry-forward notes

- `E2eGateMode::VoxGpuOracleCpu` is still read by `setup_test_grid` and is load-bearing
  for the `--e2e-vox-oracle-cpu` spawn flag. `E2eGateMode` is NOT fully dead — Phase 5
  must replace that one variant's role (a minimal marker / spawn-contract signal) rather
  than blindly deleting the enum (design Assumption A3 anticipated this).
- 4 new spawn flags landed on `bin/bevy-naadf`: `--e2e-vox-oracle-cpu`, `--e2e-entities`,
  `--e2e-empty-world`, `--e2e-resizable`.
- `resize_test`: the `hyprctl` resize path is a real tiling-Wayland constraint, not rot —
  a client cannot self-resize on a tiling compositor. The migrated gate keeps `hyprctl`
  under Hyprland with a BRP-verb fallback elsewhere. D10's "drop Hyprland entirely" is
  not fully met by design necessity, not by migration shortfall.
