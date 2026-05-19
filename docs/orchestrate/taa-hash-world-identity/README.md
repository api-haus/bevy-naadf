# Orchestration: TAA hash — world-data identity

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/`
Branch: `feat/streaming-world` (last commit before this work: `cf538e6`)
Mode: **consolidated** (user-confirmed at Step 4 Q&A 2026-05-19)

## Files

| File | Purpose | Status |
|---|---|---|
| `00-reuse-audit.md` | Reuse audit — concludes no existing world-identity hash; extend `taa_hash_from_data` + `taa_compress_sample`. Documents `FirstHitResult` layout and the `cam_pos_int` correction. | [x] |
| `01-context.md` | Canonical context bundle for the consolidated agent (mirrors handoff + audit corrections + user Q&A decisions). | [x] |
| `05-impl-taa-hash-world-identity.md` | Consolidated agent's deliverable: `## Design`, `## Decisions & rejected alternatives`, `## Assumptions made`, `## Independent review`, `## Diffs landed`, `## Verification`, `## Stretch result`, `## Out-of-scope findings`. Filename mandated by handoff §Deliverable. | [x] |

## Agent groups (consolidated mode)

| Group | Agent | Owns |
|---|---|---|
| Audit | `delegate-auditor` (sonnet) | Reuse audit. Done. |
| Single-pass | `delegate-consolidated` (inherited Opus, 1M context) | Design → self-review → implementation → impl log. |
| Commit checkpoint | `general-purpose` (sonnet) | Pre-dispatch `git add -A . && git commit` snapshot. |

## Phase tracker

- [x] Phase A — orchestration setup (audit, mode select, context files)
- [x] Phase B — checkpoint commit + consolidated dispatch
- [x] Phase C — verification: 5 commands per handoff §Verification (all 5 PASS, single-attempt; baseline 289 → 291 unit tests after 2 new world-identity primitive guards)
- [x] Phase D — user hard-gate review: live visual check shows blink persists. Root cause confirmed as `## Independent review` Finding 8 (4+4+4+1 packing collides on 32-voxel single-axis shifts, very common in 64-voxel-wide streaming window).
- [x] Phase E — refinement dispatch: `taa_data_id_lo13` body replaced with `pcg_hash` avalanche over `vec3<i32>` voxel coord (signature unchanged). Imported `pcg_hash` from `ray_tracing_common.wgsl`. All 5 gates PASS (cold-start, streaming-window after one re-run for first-frame pipeline-compile hitch, oasis-edit-visual, build, lib tests 291/291). Iteration-2 section appended to `05-impl-taa-hash-world-identity.md`.
- [x] Phase F — second user hard-gate review: live visual check shows "pretty much same blinking". The pcg_hash avalanche should have fully eliminated Finding 8's collision class; that it didn't move the artefact strongly suggests our root-cause story (hash collision) is wrong.
- [ ] Phase G — diagnostic investigator dispatch (read-only). Map the full TAA reproject + history-accumulation flow. Test 4 hypotheses against actual code: (1) hash-reject path not load-bearing where assumed (other accumulation path?); (2) artifact isn't TAA reproject (denoiser? variance buffer?); (3) hash encode/decode mismatch; (4) `params.cam_pos_int` vs `cnts_params.cam_pos_int` frame-skew.
- [x] Phase H — synthesis: hash was never the bug. Actual root cause is `CameraHistory.positions[]` storing window-local PositionSplit values that jump by ±256 voxels/axis on origin shift, breaking `cam_pos_from_cur_int` deltas → screen-pos and 0.2%-dist rejects fire on all post-shift history (hash test never reached). Same bug also wipes ReSTIR-GI sample ring. Two secondary bugs flagged: 8-neighbour hash fallback broken post-fix; `data_id_lo13` mis-labelled "world-anchored".
- [ ] Phase I — instrumentation dispatch: add `info!` logging `positions[K] - positions[K-1]` across origin shifts to empirically confirm the 256-voxel jump.
- [ ] Phase J — user live run + log capture, then redirect to structural fix design.

## Iteration 3 decision (2026-05-19)

Approach: instrument-first to confirm the diagnostic. Then design the structural fix (rebase `CameraHistory.positions[]` on origin shift) bundled with the two secondary bug fixes.

## Iteration 2 decision (2026-05-19)

User picked **pcg_hash mix** over the additive packing. Reason: pcg_hash mix avalanches all 3 voxel components, eliminating all axis-aligned collision classes that the additive packing exhibits.

## User Q&A decisions (2026-05-19)

1. **`data_id_lo13` derivation**: world-absolute. Add `vec3<f32>(cam_pos_int)` before `floor` so same world voxel = same hash across origin shifts.
2. **Execution mode**: consolidated. Bounded context, cohesive scope, low blast radius (shader-only), tight design↔impl coupling.
3. **Unit test**: yes — port `taa_hash_from_data` to Rust, test that ≥100 distinct `data_id_lo13` inputs produce distinct 16-bit-masked outputs (primitives-first per memory `feedback-primitives-then-analytical-invariants.md`).
