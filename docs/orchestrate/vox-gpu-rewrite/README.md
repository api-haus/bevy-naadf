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
- [x] Stage 4 fix iteration — 8 attempts, ALL FAILED. Round-4 diagnostic recommends GPU readback byte-diff.
- [x] Stage 5 Part A — D1 fix landed (CPU mirror readback after W5 producer; `seed_block_hashing` reseed); addresses symptom 2 (edit raycast). Verified `chunks_cpu.len() = 2.1M, blocks_cpu = 12.9M, voxels_cpu = 10.5M` post-Oasis-boot.
- [x] Stage 5 Part B — inferential byte-diff (cursor ratios), no concrete byte evidence
- [x] Stage 6 — `--validate-gpu-construction-scaled` across 27 fixture configurations including real Oasis: **W5 producer output is BYTE-IDENTICAL to CPU oracle across every fixture**. Generator + chunk_calc{calc_block, compute_voxel_bounds, compute_block_bounds} all byte-equal. Cursor counts match exactly. **The W5 producer chain is provably correct.**
- [x] Hypothesis re-localized: bug is in `bounds_calc.wgsl::{prepare_group_bounds, compute_group_bounds}` — chunk-layer AADF iterative refinement
- [x] User dispatched W3 diagnostic (option 2)
- [x] Stage 7 — `13-diagnostic-w3-bounds-calc.md` identified CONCRETE bug **W3-T1 (HIGH confidence)**: `naadf_bounds_compute_node` runs regime-2 BEFORE `add_initial_groups_to_bound_queue` seeds the queue. Seed gated on `gpu_producer_has_run` (flips in Core3d); compute_group_bounds has NO matching gate → drains queue + re-enqueues all 32768 groups at (0,0,0) before real seed lands → only group (0,0,0)'s chunk-AADFs converge; rest stays zero. Default scene escapes by `want_gpu_producer = false` accidentally inverting the gate polarity.
- [x] Recommended fix: add `if !construction_gpu.bounds_initialized { return; }` early-return to `naadf_bounds_compute_node` at `bounds_calc.rs:311-330` (one-line change)
- [x] W3-T1 fix landed (commit `8039e9b`) — structurally correct but didn't change visible
- [x] Stage 8 — type-decode diagnostic; Q4 max_storage_buffer_binding_size hypothesis (MEDIUM-HIGH confidence)
- [x] Q4 verification — REFUTED (Bevy auto-uses adapter max 2047 MiB; bindings fit)
- [x] Stage 9 — production-scale voxels[] readback at 25 Oasis-populated positions, post-producer AND post-bounds-calc. **25/25 BYTE-MATCH at both checkpoints.** voxels[] IS byte-correct end-to-end at production scale.
- [x] Stage 9 — bug PROVEN to be downstream of producer
- [x] Stage 10 — renderer/wiring diagnostic. **BUG FOUND (HIGH confidence).** All 4 prior candidates REFUTED. Concrete root cause at `crates/bevy_naadf/src/voxel/grid.rs:393-398`: `install_vox_in_fixed_world` builds `ModelData` from `build_constructed_world_sparse` output which **encodes empty voxels with 12-bit AADF distance bits in the low half-word**. Generator shader `& 0x7FFF` + `\|= (1<<15) if > 0` falsely promotes AADF-bearing empty voxels to "full type=AADF". Concrete example: voxel(186,189,252) → `data_voxel[32515] = 0x08830886` (empty+AADF=0x886) → renderer reads hit_type=0x886=2182 → OOB palette → BLACK surface. **Matches symptom exactly.** Legacy path works because it never goes through ModelData (reads voxels_cpu directly + checks bit 15 in `ray_tracing.wgsl:339-341`). C# `ModelData.cs::ImportFromVox:442-446` emits literal 0 for empty voxels — Rust port unified model encoder with renderer encoder, breaking the convention.
- [x] Recommended fix: at `grid.rs:393-398` post-process `imp.world.voxels` to zero out half-words where bit 15 is clear (~10 lines, C#-faithful)
- [x] Stage 11 — fix landed. **`--vox-gpu-oracle` mean per-pixel diff 127.84 → 3.241** (39× reduction, well under 8.0 mean floor). Oracle PNG visually matches CPU oracle closely (sand, palms, water pool, architecture all visible). Per-pixel CEILING (≤655 pixels with Δ>16) still exceeded at 3906 (5.96%) — secondary smaller-class bug remains.
- [x] Hard gate — user live-tested post-Stage-11 binary; dispatched Stage 2 consolidation; residual speckle filed as followup.
- [x] **Stage 2 consolidation landed (2026-05-18)** — single install pathway: production binary and every e2e gate route through `install_vox_in_fixed_world` for `GridPreset::Vox` (or `install_default_embedded_in_fixed_world` for `Default`). Destroyed: `AppArgs::fixed_world_size`, `GridPreset::Vox::tiles`, `--vox-grid` flag, `setup_test_grid` dispatch ladder, three CPU stop-gap functions (`load_vox_into_world` / `parse_dot_vox_data_into_world` / `tile_buckets_into_world`) + two tests, `install_default_small_world`. CPU oracle helpers (`install_vox_sized_to_model`, `build_world_from_vox`, `load_vox_tiled`, `parse_dot_vox_data_tiled`, `replicate_buckets_xz`) retained; reachable only via the `--vox-gpu-oracle` CPU-phase escape hatch (`vox_gpu_oracle_cpu_phase`). E2e camera poses retranslated by demo embed offset `(2016, 0, 2016)` so the baseline / edit / entities / runtime-edit gates frame the centered demo. See `03-impl.md` Stage 2 section.
- [x] Hard gate — Stage 2 e2e suite: 11/13 PASS; `--vox-gpu-oracle` continues to fail on the residual ~6% per-pixel ceiling speckle (pre-existing, unchanged); `--small-edit-repro` regressed (legacy CPU PASS → W5 FAIL surfacing the same residual W5 inversion class on the user-captured edit position) — both flagged as Stage 12+ followups.
- [ ] Stage 12+ — investigate residual ~6% speckle / inversion-on-brush-edit (both `--vox-gpu-oracle` per-pixel ceiling and `--small-edit-repro` surface the same class)
- [ ] Step 6 — Checkpoint commit + impl W5.4 (delete CPU stop-gap)  ← partially landed via Stage 2 (load_vox_into_world / parse_dot_vox_data_into_world / tile_buckets_into_world deleted); pending docstring updates per `00-reuse-audit.md` §W5.4
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
