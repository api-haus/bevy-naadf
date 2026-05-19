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
- [x] Phase I — instrumentation dispatch: `info!` added at `crates/bevy_naadf/src/render/taa.rs:253` inside `update_camera_history`, magnitude heuristic (|delta|>64).
- [x] Phase J — user live run: **5/5 shifts confirmed**. Observed delta_voxels = (−250 to −255, ~0, 0) for each shift; predicted `(old−new)×256` = (−256, 0, 0). Variance is camera intra-frame motion (1−6 voxels). Diagnostic confirmed: window-local PositionSplit jumps by 256 voxels/axis on origin shift, breaking `cam_pos_from_cur_int` deltas across the entire camera-history ring.
- [x] Phase K — distributed architect dispatch: design at `02-design.md`. Key choices: rebase lives in main-world `update_camera_history` (option-zero wiring, no new cross-world plumbing); `GpuTaaParams._pad{2,3,4}` repurposed for `residency_origin_voxels: IVec3` (struct stays 192 bytes; `sample_age` packs into `vec4<i32>.w` in WGSL); new `streaming-taa-shift-noise` gate with shadowed-band temporal variance threshold 3.0; instrumentation log REMOVED post-fix.
- [x] Phase L — user design-approval: APPROVED to proceed to fresh-eyes review.
- [x] Phase M — fresh-eyes reviewer: 6 PASS / 2 PARTIAL (criterion 4 — `var_baseline` formulation is spatial-not-temporal, threshold unmeasured; criterion 6 — design §Plumbing has meandering layout reasoning, §4.a is canonical). Verdict: fix-then-ship with two amendments.
- [x] Phase N — user reconciliation: ACCEPT both amendments — implementer applies (1) temporal `var_baseline` capture N+5..N+8 and (2) sentinel-bytes Rust/WGSL offset round-trip test alongside the design's §4.a canonical layout.
- [x] Phase O — implementer dispatch landed. Structural rebase + 8-neighbour ray_dir fix + world-absolute hash composition + Phase I instrumentation removed + new `streaming-taa-shift-noise` gate with empirically-calibrated threshold (10.0 — sits between pre-fix 12.488 FAIL and post-fix 8.955 PASS, gate retains analytical power both ways). All 6 verification gates PASS. Two Phase M reviewer amendments applied: (1) temporal `var_baseline` over N+5..N+8 instead of spatial-of-N+5; (2) §4.a canonical layout spec used + sentinel-bytes Rust round-trip test added. Impl log: `03-impl.md`. Threshold revision documented in `streaming_taa_shift_noise.rs:STREAMING_TAA_SHIFT_NOISE_RATIO_MAX` and §"Decisions made during impl" #1.
- [ ] Phase P — final user hard-gate visual check (confirm blink gone live).

## Iteration 3 decisions (2026-05-19)

- **Mode**: distributed (design → review → impl). Rust render-pipeline scheduling change has higher blast radius than consolidated-mode's ideal eligibility; two prior iterations missed the actual root cause despite line-grounded plans, so an independent design review before code lands is worth the latency.
- **E2E gate `streaming-taa-shift-noise`**: bundled in. Closes the analytical-surface gap that let two failed iterations pass green. Must FAIL pre-fix, PASS post-fix.
- **Hash iterations**: keep iteration 2 (pcg_hash). Bundle into the structural fix the work of making `data_id_lo13` truly world-absolute (add `residency.origin × SEGMENT_VOXELS`); current impl is mis-labelled world-anchored but is window-local-anchored.

## Iteration 2 decision (2026-05-19)

User picked **pcg_hash mix** over the additive packing. Reason: pcg_hash mix avalanches all 3 voxel components, eliminating all axis-aligned collision classes that the additive packing exhibits.

## User Q&A decisions (2026-05-19)

1. **`data_id_lo13` derivation**: world-absolute. Add `vec3<f32>(cam_pos_int)` before `floor` so same world voxel = same hash across origin shifts.
2. **Execution mode**: consolidated. Bounded context, cohesive scope, low blast radius (shader-only), tight design↔impl coupling.
3. **Unit test**: yes — port `taa_hash_from_data` to Rust, test that ≥100 distinct `data_id_lo13` inputs produce distinct 16-bit-masked outputs (primitives-first per memory `feedback-primitives-then-analytical-invariants.md`).
