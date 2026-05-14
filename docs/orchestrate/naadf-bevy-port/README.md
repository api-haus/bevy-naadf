# Orchestration: NAADF → Bevy Port

**Topic slug:** `naadf-bevy-port`
**Started:** 2026-05-14
**Mode:** `/delegate` — orchestrator scopes/briefs/synthesizes; all work is dispatched to sub-agents.

## Goal (one line)

Port the NAADF C#/MonoGame voxel global-illumination engine (`/mnt/archive4/DEV/NAADF`)
into Rust/Bevy at `/mnt/archive4/DEV/bevy-naadf`, informed by the research doc
`docs/research/ulschmid-2026-naadf-voxel-gi.md`. Scope: core engine (voxel + AADF + world +
render), no editor GUI / persistence / importers.

## Files in this directory

| file | owner group | purpose |
|---|---|---|
| `README.md` | orchestrator | this index + phase checklist |
| `00-reuse-audit.md` | `delegate-auditor` | what already exists in `bevy-naadf` vs. what is greenfield (DONE) |
| `01-context.md` | orchestrator | canonical context bundle — every agent reads this first |
| `02-research.md` | `research` group | structured map of NAADF C# subsystems + AADF GI algorithm → Rust/Bevy porting reference |
| `03-design.md` | `design` group | crate/module layout, ECS decomposition, render-graph plan, subsystem→Bevy mapping |
| `04-impl.md` | `impl` group | phased porting work log |
| `05-review.md` | `review` group | verification against source behaviour + the paper |

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
- [~] `impl` phase (**Phase A**) → `04-impl.md` — Batch 1 (steps 1–6) done 2026-05-14, 32 tests pass; Batch 2 (steps 7–12) pending
- [ ] `review` phase (**Phase A**) → `05-review.md`
- [ ] Phase A-2 (TAA): design → impl → review
- [ ] Phase B (GI): design → impl → review
- [ ] Phase C (GPU construction/editing): design → impl → review

## Pacing

One dispatch at a time. After each agent returns, the orchestrator pauses and submits to the
user before the next dispatch. Each substantive dispatch is preceded by a delegated checkpoint
commit.
