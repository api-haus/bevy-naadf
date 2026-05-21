# mobile-budget — orchestration index

Topic slug: `mobile-budget`
Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build`
Branch: `feat/android-build`
Started: 2026-05-21

## Goal (one line)

Design and ship a **startup-time GPU budget preselection routine** that reads `device.limits()` and writes safe sizes for `voxels` / `blocks` / `taa_sample_accum` / `taa_samples` storage-buffer bindings BEFORE the world install fires — so the full Naadf world boots on mobile WebGPU/Vulkan (`max_storage_buffer_binding_size = 256 MiB` cap).

## Decisions locked in Step 4 Q&A (2026-05-21)

| # | Decision | Choice |
|---|---|---|
| Q1 | Execution mode | **Distributed** — architect → user-approval gate → impl |
| Q2 | World-size lever shape | **Const + parallel runtime override** — keep `pub const WORLD_SIZE_IN_SEGMENTS` + its C#-canonical compile-time pin; add `Res<EffectiveWorldSize>` that 15 consumer sites read from. Desktop: `EffectiveWorldSize == WORLD_SIZE_IN_SEGMENTS`. Mobile: diverges. Preserves the faithful-port invariant. |
| Q3 | Lever #3 (internal-res scale) | **Deferred** — strictly TAA ring + world size. Add later if FPS gauge demands it. |
| Q4 | `device.limits()` read site | **Pre-`build_app_with_args` probe-app** — mirror `validate_gpu_construction_production_scale`'s `validation.rs:1037-1046` + `world/buffer.rs:246-264` technique. Spin up throwaway `App` + `DefaultPlugins` + `Plugin::ready/finish`, extract `RenderDevice`, read limits, drop, then build the real App with overrides. |

## Files

| file | purpose | written by |
|---|---|---|
| `README.md` | This index | orchestrator |
| `00-reuse-audit.md` | Re-implementation audit — 7 candidates, 3 borderline | `delegate-auditor` |
| `01-context.md` | Canonical context bundle for every non-review agent | orchestrator |
| `02-design.md` | Architect's design (probe app, budget routine, world-size override, TAA ladder, headroom constants) | `delegate-architect` |
| `03-impl.md` | Implementer's execution log | `general-purpose` Opus |

## Agent groups

- **audit** — `00-reuse-audit.md` — read-only reuse search. Done.
- **design** — `02-design.md` — architectural design pass producing a complete plan the implementer can execute without further design decisions.
- **impl** — `03-impl.md` — code-mutating implementation pass that lands the design, runs verification gates, and writes an execution log.

## Phase checklist

- [x] Step 1 — scope
- [x] Step 2 — reuse audit
- [x] Step 2.5 — mode select (distributed)
- [x] Step 3 — method presented to user
- [x] Step 4 — Q&A (4 decisions locked)
- [x] Step 5 — context files written
- [x] Step 6a — checkpoint commit before architect (`76463ed`)
- [x] Step 6b — architect dispatch (delegate-architect — 02-design.md, 9 sections, 10 decisions, 10 assumptions, 10 side notes)
- [x] Step 7a — synthesize architect result; user approved Mali pick `(taa=8, world=(6,2,6))`, design as-is. Context-doc fix: `taa_sample_accum` corrected (3 big bindings, not 4).
- [x] Step 6c — checkpoint commit before impl (`ff5e89d`)
- [x] Step 6d — impl dispatch (general-purpose Opus — Phases A-E landed; Phase F deferred — no tethered device; 187/187 lib tests green; APK built at `android/app/build/outputs/apk/debug/app-debug.apk`)
- [ ] Step 7b — user on-device check (HARD GATE — install APK on Mali-G52 tablet, verify `[budget]` log line appears + device does not OOM-reboot)
- [ ] Step 8 — exit when on-device check passes

## Verification surface (project rule — see `CLAUDE.md`)

- `cargo build --workspace`
- `cargo test --workspace --lib`
- Android APK rebuild + install on Mali-G52 tablet (the user does the visual check)
- Existing e2e gates as relevant (no new visual e2e gate planned — symptom is "boots vs OOM-reboots" which is a binary observable on-device)
