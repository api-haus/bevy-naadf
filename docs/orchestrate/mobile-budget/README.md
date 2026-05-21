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
- [x] Step 7b — on-device check (deploy agent — installed APK + captured logcat — see `04-device-deploy.md`). **Partial:** device did NOT reboot (improvement); world budget half landed `(6,2,6)` correctly; TAA half landed at depth=16 instead of expected 8 → buffer 281 MiB > 256 MiB cap → Validation RenderError → clean app exit. `[budget]` log line never emitted (subscriber init order). 15 missing WGSL shader assets are a second independent failure.
- [x] Step 7c — consolidated Research → Architect → Implement remediation (`delegate-consolidated` — see `05-consolidated-fix.md` + `06-logcat-post-fix.log`). All three brief items fixed: `[budget]` log line visible, bind-group validation cleared, 15 missing WGSL shaders packaged. Deploy agent's TAA-depth hypothesis was wrong — real culprit was `gi_gpu.invalid_samples` (pixel_count × 128 B), a binding the architect's design didn't enumerate.
- [x] Step 8 — user-visible goal met: **Naadf world boots on Mali-G52 + renders at ~0.001 fps (slideshow)**. Device does not reboot. Verdict from consolidated dispatch: PARTIAL (renders but slow).

## Follow-up scope (NOT part of this orchestration)

- **Swap-chain texture timeouts (52 events / "Timeout" from the wgpu surface).** Bevy's render world can't acquire a surface image in <1 s on the Mali-G52 + GPU producer + 13-pass GI graph. GPU saturation, not budget / cap. Slideshow framerate (~0.001 fps) is the user-visible manifestation. Separate task.
- **Architectural smell flagged by `delegate-consolidated`:** the architect's "mirror snapshot at plugin-build" pattern doesn't compose cleanly with post-build resource overrides. Works for `EffectiveWorldSize` by coincidence (the snapshot is dropped before the world is used); breaks for `invalid_sample_storage_count`. Future refactor candidate — see `05-consolidated-fix.md` side-notes.
- **iPhone 16 Safari WebGPU.** The cap fix is shared (same 256 MiB ceiling). User wants to try the web build next. Miniserve now binds 0.0.0.0 so iOS Safari on the same LAN can reach the desktop's `dist/`.

## Verification surface (project rule — see `CLAUDE.md`)

- `cargo build --workspace`
- `cargo test --workspace --lib`
- Android APK rebuild + install on Mali-G52 tablet (the user does the visual check)
- Existing e2e gates as relevant (no new visual e2e gate planned — symptom is "boots vs OOM-reboots" which is a binary observable on-device)
