# Orchestration: NAADF → Bevy Port

**Topic slug:** `naadf-bevy-port`
**Started:** 2026-05-14
**Mode:** `/delegate` — orchestrator scopes/briefs/synthesizes; all work is dispatched to sub-agents.
**To resume:** read [`RESUME.md`](RESUME.md) — current state + the next dispatch. Continue with `/delegate continue naadf-bevy-port`.

## Goal (one line)

Port the NAADF C#/MonoGame voxel global-illumination engine (`/mnt/archive4/DEV/NAADF`)
into Rust/Bevy at `/mnt/archive4/DEV/bevy-naadf`, informed by the research doc
`docs/research/ulschmid-2026-naadf-voxel-gi.md`. Scope: core engine (voxel + AADF + world +
render), no editor GUI / persistence / importers.

## Files in this directory

| file | owner group | purpose |
|---|---|---|
| `README.md` | orchestrator | this index + phase checklist |
| `RESUME.md` | orchestrator | resume pointer — current state + the next dispatch; read first to continue |
| `00-reuse-audit.md` | `delegate-auditor` | what already exists in `bevy-naadf` vs. what is greenfield (DONE) |
| `01-context.md` | orchestrator | canonical context bundle — every agent reads this first |
| `02-research.md` | `research` group | structured map of NAADF C# subsystems + AADF GI algorithm → Rust/Bevy porting reference |
| `03-design.md` | `design` group | crate/module layout, ECS decomposition, render-graph plan, subsystem→Bevy mapping |
| `04-impl.md` | `impl` group | phased porting work log |
| `05-review.md` | `review` group | Phase-A verification + the two review-gate fixes |
| `06-design-a2.md` | `design` group | Phase A-2 (TAA) architecture design |
| `07-impl-a2.md` | `impl` group | Phase A-2 (TAA) implementation log |
| `08-review-a2.md` | `review` group | Phase A-2 (TAA) verification — faithful-port + 0.25-spp-readiness (sample-count signal) |
| `09-design-b.md` | `design` group | Phase B (GI) architecture design |
| `10-impl-b.md` | `impl` group | Phase B (GI) implementation log |
| `11-review-b.md` | `review` group | Phase B (GI) fresh-eyes review brief + findings |
| `12-alignment-gap.md` | `analysis` | port-vs-NAADF alignment gap analysis — subsystem faithfulness table, divergence/open-question reconciliation, open bugs, prioritized "what's left to fully align" list |
| `e2e-render-test.md` | `delegate-architect` | headless e2e integration render-test harness design — replaces the live `cargo run` smoke-run as the impl-agent verification step |
| `design-exploration-qa.md` | orchestrator | methodology/capability/VRAM Q&A reference (lineage, PBR texturing, dynamic entities, microvoxels, LOD, TAA-history VRAM lever) — read before scoping features it covers; holds one binding decision (§6) |

**Phase B is being done in a git worktree:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi` (branch `feat/phase-b-gi`, from `main` at the Phase-A-2-close commit). All Phase-B orchestrate files, briefs, and code use absolute paths under that worktree.

## Agent groups

- **`research`** — Reads the NAADF C# tree + the research doc. Produces a structured porting
  reference: every in-scope subsystem, its data types, its algorithms, its shaders, and a
  first-pass note on the Bevy/Rust equivalent. Writes to `02-research.md`.
- **`design`** — Consumes `01-context.md` + `02-research.md`. Designs the Bevy architecture:
  `src/` module layout, ECS components/resources/systems, the custom render-graph plan for the
  ported WGSL pipeline, and the NAADF-subsystem → Bevy-feature mapping. Writes to `03-design.md`.
- **`impl`** — Consumes context + research + design. Executes the port in phases, runs builds.
  Writes to `04-impl.md`.
- **`review`** — Verifies the port against NAADF's source behaviour and the research paper.
  Writes to `05-review.md`.

## Phase checklist

Phase order (canonical defs in `01-context.md` §2 "Phasing decision"): **A → A-2 (TAA) →
B (GI) → C (GPU construction/editing)**. One gated phase at a time.

- [x] Step 2 — Re-implementation audit → `00-reuse-audit.md`
- [x] Step 4 — Architectural Q&A → Q1–Q4 (`01-context.md` §2)
- [x] Design-phase Q&A → D1–D5 + 4-phase restructure (`01-context.md` §2b)
- [x] Step 5 — Context files written (`README.md`, `01-context.md`)
- [x] `research` phase → `02-research.md` (whole paper + in-scope C# tree, phase-tagged, ~36 KB)
- [x] `design` phase (**Phase A**) → `03-design.md` (~33 KB; 12-step Phase-A impl sequence)
- [x] `impl` phase (**Phase A**) → `04-impl.md` — Batch 1 (steps 1–6) + Batch 2 (steps 7–12) done 2026-05-14; 39 tests pass, builds + smoke-runs clean
- [x] `review` phase (**Phase A**) → `05-review.md` — **Phase A review gate PASSED**. Two regressions found, fixed, and user-confirmed: (1) camera→ray perspective (3 compounding MonoGame↔wgpu convention bugs), (2) out-of-volume concentric-line artifacts (wrong AABB clip-box values — NAADF insets by 0.1 voxel as `float3`). 39 tests pass; builds + runs coherent inside and outside the volume.
- [x] **Phase A-2 (TAA) — COMPLETE.** Context (`01-context.md` §2c) + design (`06-design-a2.md`) + impl (all 9 steps, `07-impl-a2.md`) + review (`08-review-a2.md`): 0.25-spp readiness READY, faithful HLSL→WGSL port + `M*v` matrix convention verified, leftover step-8 instrumentation reverted, 39 tests pass, smoke-runs clean. Deliverable: NAADF's 16-frame long-term-memory TAA, on by default; per-pixel sample-count signal exposed for Phase B.
- [~] Phase B (GI) — in worktree `feat/phase-b-gi`. Context (`01-context.md` §2d) + **design done** (`09-design-b.md`, ~1711 lines: 13-node render graph, 6-batch impl sequence). Scope = NAADF's real-time `WorldRenderBase` GI only (compressed ReSTIR GI + sparse bilateral denoiser + 4-plane first-hit + `rayQueueCalc` adaptive 0.25-spp + atmosphere); reference pathtracer + DLSS-RR OUT (future). **impl in progress** — Batches 1–5 done + B6 implemented (`10-impl-b.md`: B1 shared WGSL + GPU types + atmosphere subsystem; B2 4-plane first-hit restructure; B3 rayQueueCalc + globalIllum; B4 sampleRefine ×5 passes; B5 spatialResampling + denoiser; B6 base/ TAA rewire + final blit + integration). Verification: windowed e2e render-test harness (`cargo run --bin e2e_render`, design `e2e-render-test.md`) replaces the live smoke-run; saves `target/e2e-screenshots/e2e_latest.png` for vision review. Streak/ring artifact fixed (`10-impl-b.md` "Streaking artifact fix": `update_camera_history` query filter froze the frame counter for non-`FreeCamera` cameras → atmosphere precompute stuck on 1/4 of its octahedral buffer). e2e test scene expanded — shared test grid now has 5 emissive blocks + towers/wall/pillars/spheres (richer GI test scene); e2e gates recalibrated. **Phase B impl FEATURE-COMPLETE.** Three bugs surfaced + fixed once the GI data flow was un-blocked: the streaking artifact (frozen TAA frame counter), the Batch-6 TAA-path black frame (`GpuTaaParams` `vec3`-then-scalar WGSL vs Rust `#[repr(C)]` layout mismatch), and GI-bounce invisibility (same layout-mismatch class in `GpuGiParams` → `bucket_count` mis-decoded → the whole `sampleRefine`→`spatialResampling` reservoir chain produced nothing; fix: `vec3`→`vec4` rows, consumers read `.xyz`). GI bounce now **VISIBLE** — the voxel structure (towers, wall+arch, pillars, spheres, ground) is fully lit by colored bounce from the 5 emissive blocks; e2e frame budget raised 8→96 for ReSTIR temporal convergence. 46 tests pass, `cargo run --bin e2e_render` exits 0, all gates green (`assert_batch_6` honest at `MIN_GI_BOUNCE_LUMINANCE=12.0`, 99.2% GI-lit). **Phase B review gate PASSED** (`11-review-b.md`: 0 blockers, 2 concerns, 5 nits — all coverage gaps / debris / advisory, no correctness defects; faithful port confirmed line-by-line vs NAADF source, all 8 GPU struct layouts audited clean). **Post-review production-app bug found** — under camera *motion* the TAA reprojection path degrades shadowed regions to pitch black (a static-camera e2e doesn't catch it; the `base/` TAA running-average is convergent + audited faithful for a static camera). Tangential `sync_position_split` `With<FreeCamera>` query-filter bug found + fixed (`position_split.rs`) — unblocks moving-camera e2e coverage. → TAA camera-motion reprojection **audited faithful vs NAADF C#** (`10-impl-b.md`) — no shader fix needed; the real motion-decay was the already-fixed `sync_position_split` trap; a deterministic moving-camera e2e mode + `assert_batch_6` motion-stability gate were added as permanent coverage. **Two new TAA bugs reported:** (a) TAA goes black on window resize (framebuffer-resize resource-lifecycle bug; the fixed-size e2e is structurally blind to it); (b) TAA never resolves — output stays perpetually noisy, unlike the C# version. → e2e noise-analysis test + C#-grounded noise mitigation + window-resize fix → review-follow-up cleanup → Phase B COMPLETE
- [x] Gap analysis (`12-alignment-gap.md`) — port vs NAADF C#, in-scope core engine: 16 subsystems assessed (7 faithful, 9 faithful-with-documented-deviations, 0 diverging); all ~11 `02-research.md` divergences + ~7 open questions reconciled + 5 new ones documented; 1 blocking open bug (TAA camera-motion reprojection decay), 4 review nits/concerns open; Phase C deliberately deferred. Bottom line: in-scope port functionally complete + faithful, one well-scoped fix from a clean temporally-stable production gate.
- [ ] Phase C (GPU construction/editing): design → impl → review

## Pacing

One dispatch at a time. After each agent returns, the orchestrator pauses and submits to the
user before the next dispatch. Each substantive dispatch is preceded by a delegated checkpoint
commit.
