# vox-gpu-rewrite

Port `bevy-naadf`'s `.vox` → fixed-world load path from a CPU XZ-tiling stop-gap
to a GPU dispatch chain mirroring C# `WorldData.cs:120-156`'s per-segment
`generator_model + chunk_calc` invocations. The WGSL shader
(`generator_model.wgsl`) and Rust dispatch helper
(`generator_model.rs::dispatch_generator_model`) already exist as audited W5
scaffolding — only the runtime integration into `prepare_construction` /
`naadf_gpu_producer_node` is missing.

Origin: `/tmp/naadf-vox-gpu-rewrite-handoff.md` (in-session handoff, may not
survive — every load-bearing fact is inlined into `01-context.md`).

## Mode

**Distributed.** Step 2.5 disqualified consolidated-mode on criterion 3 (low
blast radius): the production GPU dispatch path is correctness-critical; a
subtle bug in the W5.3 segment loop renders the world wrong everywhere. The
handoff also cites W1/W3/W4 precedent which used the distributed flow.

## Agent groups

| Group | Role | Subagent type | Model | Group file |
|---|---|---|---|---|
| audit | Reuse audit (find existing scaffolding) | `delegate-auditor` | inherited (Opus) | [`00-reuse-audit.md`](00-reuse-audit.md) |
| design | Architect the W5.1–W5.6 integration | `delegate-architect` | inherited (Opus) | [`02-design.md`](02-design.md) |
| impl | Land the code, run gates between subtasks | `general-purpose` | inherited (Opus) — code-mutating in production GPU path | [`03-impl.md`](03-impl.md) |
| review | Fresh-eyes verification | `delegate-reviewer` | default | [`04-review.md`](04-review.md) |

## Files

- [`README.md`](README.md) — this index
- [`00-reuse-audit.md`](00-reuse-audit.md) — reuse audit (8.3 KB; **DONE**)
- [`01-context.md`](01-context.md) — canonical context bundle (non-review agents)
- [`02-design.md`](02-design.md) — design agent output (per-subtask spec)
- [`03-impl.md`](03-impl.md) — implementer's per-subtask change log
- [`04-review.md`](04-review.md) — fresh-eyes review brief (criteria + artifact only; NO design rationale)

## Phase checklist

- [x] Step 1 — Restate + scope
- [x] Step 2 — Reuse audit dispatched + landed at `00-reuse-audit.md`
- [x] Step 2.5 — Mode selected: distributed
- [x] Step 3 — Method presented to user
- [x] Step 4 — Architectural Q&A (4 decisions captured in `01-context.md`)
- [x] Step 5 — Shared-context files written
- [x] Step 6 — Checkpoint commit + design dispatch (commit `4063d55`)
- [x] Step 6 — Design agent landed `02-design.md` (1757 lines)
- [x] Hard gate — design submitted, user confirmed
- [x] Step 6 — Checkpoint commit + impl W5.1 landed (commit `483d86b` checkpoint; W5.1 committed `894fcd1`)
- [x] Hard gate — W5.1 submitted, user confirmed
- [x] Step 6 — Checkpoint commit + impl W5.2 landed (W5.2 committed `59adc31`)
- [x] Hard gate — W5.2 submitted, user confirmed
- [x] Step 6 — Checkpoint commit + impl W5.5 landed (W5.5 committed `c5a5619`)
- [x] Hard gate — W5.5 submitted, user confirmed
- [x] Step 6 — Checkpoint commit + impl W5.3 landed (uncommitted; W5.3 fixed two latent W5.1 bugs)
- [x] Hard gate — user live-tested W5.3, reported empty scene
- [x] Diagnostic dispatch — `05-diagnostic.md` identified TWO bugs: (1) `prepare_world_gpu` buffer underallocation; (2) `InitialCameraPose::from_world_voxels` puts camera Y above world ceiling
- [x] Hard gate — diagnostic submitted, user confirmed Fix #1 + workgroup-distribution; REJECTED Fix #2 (user: "would have surfaced millennia ago")
- [x] Hard gate — user directive: NO parallel paths; staged consolidation (Stage 1 = Fix #1 + workgroup distribution + production-path gate; Stage 2 = legacy-path deletion)
- [x] Step 6 — Checkpoint commit + W5.3-fix Stage 1 dispatch (commit `a4f2697` checkpoint; Stage 1 uncommitted pending next checkpoint)
- [x] Step 6 — Stage 1 landed: 3 fixes (buffer sizing, 3D workgroup distribution, **per-segment encoder/submit — TRUE ROOT CAUSE not in diagnostic**) + W5.5 rewritten as two-frame camera-sweep Δ gate; all 10/10 e2e gates GREEN
- [x] Hard gate — user live-tested; Oasis renders but surfaces inverted (screenshot shared)
- [x] Diagnostic dispatch — `06-diagnostic-inversion.md` identified hash_map placeholder hypothesis (LANDED Stage 1.5; did NOT fix the user-visible bug)
- [x] Stage 1.5 landed (commit `9964105`) — gate widened, bound_group_queue_max_size fixed; user re-tested, same broken rendering
- [x] Diagnostic round 2 — `07-diagnostic-inversion-round-2.md` proposed initial_hash_map_size bump (1<<18 → 1<<20) — MEDIUM confidence
- [x] Compound dispatch (Stage 2) — applied hash_map_size bump (LANDED, harmless, C#-faithful) + 4 iterative experiments. Hash_map saturation REFUTED at 8M slots.
- [x] Stage 3 — top-down birdseye camera (per user directive); agent gamed lum<10 metric (bug at top-down is bright sky-bleed not dark pixels)
- [x] Stage 4 — CPU-vs-GPU per-pixel oracle gate built (`crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs`). Gate WORKS, ungameable: 127.84 mean diff vs 8.0 floor at broken state, 97.8% pixels over per-pixel threshold. CPU oracle sanity guards pass.
- [x] Stage 4 fix iteration — 8 attempts, ALL FAILED: voxels[] atomic, hash_map atomic, single-submit (broke), warmup 480, device.poll-per-segment, skip bounds, hash_map 8M, disable dedup (broke). Root cause STILL unidentified. Round-4 diagnostic recommends GPU readback byte-diff.
- [ ] Hard gate — submit Stage 4: WORKING ungameable gate + 8 failed fix attempts + unidentified root cause  ← CURRENT
- [ ] Step 6 — Checkpoint commit + impl W5.4 (delete CPU stop-gap)
- [ ] Hard gate — submit, wait
- [ ] Step 6 — Checkpoint commit + impl W5.6 (document default-scene divergence)
- [ ] Hard gate — submit, wait
- [ ] Step 6 — Fresh-eyes review against `04-review.md`
- [ ] Reconcile review against `01-context.md`; submit to user

Landing order rationale: W5.5 lands BEFORE W5.3 so the e2e gate exists to catch
regressions the moment the segment loop lands.

## Followups (out of scope for this PR)

- `w3-startup-convergence-race` — bounds_calc pipeline-compile latency (~12
  frames) + W3 AADF convergence (~7 frames) means rays single-step for the
  first ~330 ms. **Out of scope per handoff.** File as a separate topic.
