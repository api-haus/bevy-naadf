# streaming-world — procedural generation + sliding-window residency

Orchestration topic. Goal: implement procedural voxel-world generation with a
sliding-window residency layer that streams chunks into a fixed-VRAM budget,
laying groundwork for large/infinite coordinate systems and a future streamable
sparse-voxel world format (`.vox`, Minecraft conversions). **This session
scope:** procedural-noise generation feeding the sliding window. Pre-made-world
import is out of scope but the design must not preclude it.

## Mode

**Distributed.** Renderer-touching, high blast radius, design-approval gate
needed before code lands. Per Step 2.5: criteria 1 (bounded context), 3 (low
blast radius), 4 (tight design↔impl coupling) all fail for this work →
consolidated mode disqualified.

## Files

| File | Owner | Purpose |
|---|---|---|
| `README.md` | orchestrator | this file — index + phase checklist |
| `00-reuse-audit.md` | `delegate-auditor` | reuse candidates / gaps / borderline / forbidden |
| `01-context.md` | orchestrator | canonical context for non-review agents (goal, Q&A decisions, required reading, forbidden moves) |
| `02-design.md` | `delegate-architect` | the design — `## Design`, `## Decisions & rejected alternatives`, `## Assumptions made` |
| `03-impl.md` | `general-purpose` impl agent | implementation log — what changed by file, verification gates run |
| `04-review.md` | orchestrator | fresh-eyes review brief (criteria + artifact pointer ONLY; no rationale) |
| `05-review-findings.md` | `delegate-reviewer` | review findings against `04-review.md` |

## Agent groups

| Group | Subagent type | Reads | Writes |
|---|---|---|---|
| audit | `delegate-auditor` | repo | `00-reuse-audit.md` |
| design | `delegate-architect` | `01-context.md`, `00-reuse-audit.md`, repo, reference project | `02-design.md` |
| impl | `general-purpose` | `01-context.md`, `00-reuse-audit.md`, `02-design.md` (Design + Decisions + Assumptions) | code + `03-impl.md` |
| review | `delegate-reviewer` | **only `04-review.md`** (deliberately denied the design rationale) | `05-review-findings.md` |

## Phase checklist

- [x] 00 — Reuse audit
- [x] Step 2.5 — Mode selection (distributed)
- [x] Step 4 — Architectural Q&A
- [x] Step 5 — Shared-context files (`README.md`, `01-context.md`)
- [x] 02 — Architecture design v1 (`delegate-architect` → `02-design.md`, Plan A — CPU noise)
- [x] **Hard gate v1** — user redirected: Plan B (WGSL noise via GLSL port, W5 gate inverted)
- [ ] 02b — Architecture design v2 (`delegate-architect` → `02b-design-plan-b.md`)
- [ ] **Hard gate v2** — submit revised design to user
- [ ] 03a — Phase-1 impl: WGSL FastNoiseLite port (`general-purpose` → code + `03a-impl-wgsl-noise.md`)
- [ ] **Hard gate** — noise port + CPU↔GPU oracle test passes
- [ ] 03b — Phase-2 impl: residency layer + W5 gate inversion (`general-purpose` → code + `03b-impl-residency.md`)
- [ ] **Hard gate** — submit impl to user
- [ ] 04 — Fresh-eyes review brief (`04-review.md` written by orchestrator)
- [ ] 05 — Fresh-eyes review (`delegate-reviewer` → `05-review-findings.md`)
- [ ] **Hard gate** — synthesise review against `01-context.md`, submit to user

## Q&A decisions

### Step 4 (initial)

| Question | Choice |
|---|---|
| Coordinate widening | Residency-only `i32` widening — GPU bind layout stays `(cx:11,cy:10,cz:11)` window-local |
| Residency unit | Per-segment (16×16×16 chunks) |
| Block dedup | Per-resident-chunk-local |
| Noise backend | ~~`voxel_noise` CPU~~ → **WGSL FastNoiseLite port (Plan B)** — see addendum below |

### Plan-B addendum (post-design redirect)

User redirected at the design hard gate after seeing the architect's CPU-noise
choice (D.2). Throughput analysis showed GPU noise is ~30–100× faster per
segment, which directly addresses the "empty patches under fast traversal"
failure mode (D.7) the architect named as Plan A's cost. The brief explicitly
prioritises fast traversal of large worlds.

**New plan:**
- **Noise:** port `FastNoiseLite.glsl`
  (https://github.com/Auburn/FastNoiseLite/blob/master/GLSL/FastNoiseLite.glsl)
  to WGSL. GLSL chosen over HLSL because built-in name parity (`mix` / `fract`
  / `inverseSqrt`) and the absence of HLSL preprocessor / `static` / `cbuffer`
  / `register` baggage cuts the porting diff by half.
- **GPU producer:** noise WGSL feeds the existing W5 GPU pipeline
  (`ModelData → chunks/blocks/voxels`). W5 stops being dead code in the
  streaming preset — it becomes the primary consumer.
- **Driver:** the W5 once-at-startup gate is **inverted to per-frame**
  (newly-resident segments dispatch generator+chunk_calc on demand) — NOT
  disabled as in Plan A's D.10.
- **Order of work:** **WGSL noise port goes first** (user directive). It is a
  self-contained, independently verifiable deliverable (CPU↔GPU oracle test).
  The residency layer comes after, consuming it.

Impl scope estimate revised: ~1500–2000 new LOC (most of it shader). The
`voxel_noise` crate stays in the workspace as the CPU oracle source but is
**not** wired into `bevy_naadf` as a runtime dep for the streaming preset.

The Q1/Q2/Q3 choices above are unchanged.
