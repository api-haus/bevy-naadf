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
- [x] 02b — Architecture design v2 (`delegate-architect` → `02b-design-plan-b.md`)
- [ ] **Hard gate v2** — submit revised design to user, resolve OQ.2 before Phase 1
- [x] 03a — Phase-1 impl: WGSL FastNoiseLite port (`general-purpose` → code + `03a-impl-wgsl-noise.md`)
- [x] **Hard gate (Phase 1)** — user confirmed, Phase 2 OQ.1/OQ.3 + composition scope resolved
- [x] 03b — Phase-2 impl: residency + noise_terrain.wgsl + W5 gate inversion + --streaming-window gate (`general-purpose` → code + `03b-impl-residency.md`)
- [ ] **Hard gate (Phase 2)** — submit impl to user; one camera-translation gap surfaced
- [x] 03c — Diagnostic: read-only investigation of skybox-only false-pass + minutes-long hang
- [x] **Hard gate (diagnostic)** — user directive: verify noise→visible-terrain in a static scene FIRST, then sliding window
- [ ] 03d — Phase-2.4: static-scene noise verification — one-shot noise_terrain dispatch over full world + strict `--noise-static-world` gate (no residency)
- [ ] **Hard gate (Phase 2.4)** — if static-scene gate green → proceed to 2.5; if red → diagnose noise/encoding before residency
- [ ] 03e — Phase-2.5 fix (contingent on 2.4 passing): `Generating → Resident` transition + sliding-window gate threshold tightening + wall-clock budget
- [ ] **Hard gate (Phase 2.5)** — confirm visible sliding-window streaming + strict gate passes
- [ ] 04 — Fresh-eyes review brief (`04-review.md` written by orchestrator, scoped to BOTH Phase 1 + Phase 2)
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

### Scope amplifier (post-Q2 confirmation, applies to Phase 1)

User directive after confirming the Plan B design:

> "we dont have to port all of the fastnoiselite controls, but all of its
> features - yes. we'd like to have a chefs kitchen of tools so that we can
> mix&match a beautiful fast 3d voxel noise generator for our world, extended
> with biome types and complex merges and stuff"

**Overrides architect decision D.B2** (which scoped Phase 1 to
`OpenSimplex2 + Perlin + FBM` only).

**New Phase 1 scope:** port the **full FastNoiseLite feature surface** —
every noise family, every fractal type, every domain-warp variant, every
cellular distance/return-type combination. **Controls** can be simplified:
runtime-configurable via a unified uniform struct that exposes the essential
parameters; no need to mirror FastNoiseLite's C++-style getter/setter
pattern. The shader exposes a unified `fnl_get_noise_3d(state, x, y, z) -> f32`
dispatcher that internally branches on `state.noise_type` + `state.fractal_type`
+ `state.domain_warp_type` (same shape as the GLSL).

**Functions in scope for Phase 1 (full list from `FastNoiseLite.glsl`):**

- **Noise families:** OpenSimplex2 (`_fnlSingleOpenSimplex23D`), OpenSimplex2S
  (`_fnlSingleOpenSimplex2S3D`), Cellular (`_fnlSingleCellular3D`), Perlin
  (`_fnlSinglePerlin3D`), Value-Cubic (`_fnlSingleValueCubic3D`), Value
  (`_fnlSingleValue3D`). All 2D variants too if the GLSL has them (note the
  user-facing API is 3D-first for voxels; 2D ports are a nice-to-have).
- **Fractal types:** FBm (`_fnlGenFractalFBM3D`), Ridged (`_fnlGenFractalRidged3D`),
  PingPong (`_fnlGenFractalPingPong3D`), the plain "fractal off"
  pass-through.
- **Domain warps:** OpenSimplex2 (`_fnlSingleDomainWarpOpenSimplex2`),
  OpenSimplex2Reduced, BasicGrid. Each composes with the FBm / Independent
  fractal types per the GLSL function table.
- **Cellular configurations:** all distance functions (`Euclidean`, `EuclideanSq`,
  `Manhattan`, `Hybrid`) × all return types (`CellValue`, `Distance`, `Distance2`,
  `Distance2Add`, `Distance2Sub`, `Distance2Mul`, `Distance2Div`).
- **Hash + gradient tables:** all `_fnlHash`, `_fnlGradCoord*` helpers + the
  `RAND_VECS_3D` / `GRADIENTS_3D` / `RAND_VECS_2D` / `GRADIENTS_2D` constants
  the algorithms depend on.

**Phase 1 oracle test (D.B3) scope expands accordingly:** the
`--wgsl-noise-oracle` gate exercises every noise family × every fractal type
× a representative subset of domain-warps + cellular configs at fixed sample
points. Bit-near-equal (`< 1e-5`) against the Rust CPU oracle for every
combination.

**Phase 1 LOC estimate revised:** shader ~2000–2500 LOC (was ~1000), CPU
oracle ~600–800 LOC (was ~300), GPU oracle test harness ~200 LOC, e2e gate
~100 LOC. Total Phase 1 ~2900–3600 LOC, ~2.2× the architect's narrow-scope
estimate. Phase 2 unchanged at ~960 LOC.

**What "chef's kitchen" implies for the API shape:** the unified uniform
struct carries the noise-graph configuration. Future biome/composition work
will read multiple `FnlState`s and combine their outputs (lerp, max, masked
blend, etc.). The unified dispatcher is the primitive; the composition
layer is out of scope this session but the API must enable it (multiple
`FnlState` uniforms or a flat array of them).

### Phase 2 design refinements (post-Phase-1 hard-gate Q&A)

After Phase 1 landed and the user confirmed, three Phase-2 questions were
resolved via Step-4-shape Q&A:

| Question | Choice |
|---|---|
| OQ.1 — Noise → solid/empty classification | **Height-relative (Minecraft-style)** — `noise(x,y,z) + (sea_level - world_y) / amplitude > 0 → solid`. Produces ground + rolling hills + caves. `noise_terrain.wgsl` carries `sea_level: f32` + `terrain_amplitude: f32` uniforms in addition to `FnlState`. CLI knobs: `--sea-level` (default at half world-height in voxels) + `--terrain-amplitude` (default architect-picks-a-reasonable-value, justify in `03b-impl-residency.md`). |
| Composition scope (new question) | **Single FnlState noise only** in Phase 2. One noise call per voxel, single classification, single palette assignment. Multi-noise biome composition (terrain + caves + biome temperature/humidity) is deferred to a Phase 3 follow-up. The WGSL primitives from Phase 1 already support it — Phase 3 is a localised edit. |
| OQ.3 — Bounds-chain refresh policy on user edits | **Inherit existing W2 edit behavior.** Streaming admissions / evictions re-run the full bounds chain (per D.B7); user brush edits do NOT (matches existing `world_change.wgsl` path). No edits to W2 in Phase 2. |
