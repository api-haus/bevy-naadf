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
- [ ] Phase E — refinement dispatch: replace `taa_data_id_lo13` body with `pcg_hash` avalanche over `vec3<i32>` voxel coord. Re-run all 5 gates.
- [ ] Phase F — second user hard-gate review (live visual check on the refinement).

## Iteration 2 decision (2026-05-19)

User picked **pcg_hash mix** over the additive packing. Reason: pcg_hash mix avalanches all 3 voxel components, eliminating all axis-aligned collision classes that the additive packing exhibits.

## User Q&A decisions (2026-05-19)

1. **`data_id_lo13` derivation**: world-absolute. Add `vec3<f32>(cam_pos_int)` before `floor` so same world voxel = same hash across origin shifts.
2. **Execution mode**: consolidated. Bounded context, cohesive scope, low blast radius (shader-only), tight design↔impl coupling.
3. **Unit test**: yes — port `taa_hash_from_data` to Rust, test that ≥100 distinct `data_id_lo13` inputs produce distinct 16-bit-masked outputs (primitives-first per memory `feedback-primitives-then-analytical-invariants.md`).
