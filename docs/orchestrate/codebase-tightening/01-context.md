# codebase-tightening — canonical context

Every non-review agent reads this file first, in full.

## Restated goal (verbatim, from the user)

> consider the size of original c# codebase @../NAADF/
>
> our codebase - seems larger
>
> needs /refactor
>
> IoC, reduction of lines of code, tight ideomatic rust, ideomatic bevy
>
> consider dispatching scoped /refactor parallel analytics agents in domains, each writing their own documentation that you dont get involved with, then dispatching sequential refactor implementors

## Empirical scope (audit §1)

- C# reference (`/mnt/archive4/DEV/NAADF/NAADF/`) in-scope: **13 073 LOC** total (9 467 `.cs` + 3 606 `.fx` shader).
- Rust port (`/mnt/archive4/DEV/bevy-naadf/`) total source: **~66 008 LOC** (52 410 Rust + 8 727 WGSL + 1 638 e2e Playwright TS + ~3 k workspace misc).
- Ratio: **~4.0×**. Concentrated in three places, not uniform:
  1. `render/construction/mod.rs` — **11 043 LOC single file**, 21% of all Rust, 84% the size of the entire in-scope C# target. **~half is test/validation/diagnostic infrastructure living in the production module.**
  2. `e2e/` directory — **10 292 LOC** (Rust) + 1 638 (Playwright TS); has no C# counterpart by design (the project's CLAUDE.md forbids `cargo run --bin bevy-naadf` as a verification step). Deliberate verification discipline.
  3. Heavy doc-comment headers + Rust↔WGSL shader-mirror duplication. Largely irreducible by the project's faithful-port + verbose-docs ethos.

## User decisions from the Q&A (2026-05-20)

### Q1 — Tightening goal

**Chosen:** *IoC + idiom-fit first, LOC reduction is consequence.*

Architects prioritise:
- Bevy idioms (`RenderGraph` labels over `.chain()`, Plugin-per-subsystem, `Reflect`-driven settings, `Added<T>`/`Changed<T>` filters, `ShaderType` vs hand-padded `Pod`).
- Tighter Rust types (named structs over anonymous tuples, `enum Dir6` over `[usize; 6]` indices).
- IoC seams (extract diagnostic-only paths from `WorldData` API; extract test fixtures from production module; plugin-ize inline registrations in `lib.rs`).

LOC drops fall out of these. **Faithful-port rule respected** (no behavioural divergence from C# NAADF without explicit user approval + docs entry — see `bevy-naadf-faithful-port-rule` in user memory).

### Q2 — Dead-code stance

**User's verbatim decision (quote):**

> "cpu oracle stays - without it we're blind when gpu yeets out, everything else can go"

**Interpretation:**
- **`aadf/edit.rs` (the CPU oracle for the W2 GPU shader) STAYS.** It is the GPU-divergence verification surface. Do NOT feature-gate it, do NOT move it behind `#[cfg(test)]`, do NOT delete its public surface. Its callers from production paths (the `DIAGNOSTIC-ONLY` set-voxel routes) are the diagnostic ramp users invoke when GPU output looks wrong. Keep them too.
- **Everything else flagged as investigation residual: DELETE outright.** Not feature-gated, not moved behind `cfg`. Deleted from the source tree. Specifically:
  - `crates/voxel_noise/` — entire crate (1 033 LOC, zero callers, workspace docs already say "NOT yet wired"). DELETE.
  - `AadfDelayedProbe` + `aadf_delayed_probe` (`construction/mod.rs:3559`, ~270 LOC). DELETE.
  - `AadfPerCallProbe` + `aadf_per_call_probe` (`construction/mod.rs:3873`, ~170 LOC). DELETE.
  - `AadfCpuGpuParity` + `aadf_cpu_gpu_parity*` (`construction/mod.rs:4088`, ~600 LOC). DELETE.
  - `diagnostics::device_snapshot` submodule (`diagnostics.rs`, ~560 LOC). DELETE.
  - `pbr_debug_modes.rs` (218), `pbr_hard_edge.rs` (1 023), `pbr_visual.rs` (747) — DELETE all 3 PBR e2e gates and remove their `bin/e2e_render.rs` CLI dispatch entries. **EXCEPT** any PBR gate the user is actively iterating on — architect: confirm via `git log -- e2e/pbr_*` whether commits in the last 14 days touch them; if so flag for user confirmation, else delete.
  - `bin/diag_compare.rs` (314 LOC) — architect: audit whether anything still consumes it. If it's a dead CLI partner of `device_snapshot`, delete.
- **Three `validate_gpu_construction*` variants and four `run_one_*_byte_diff` fixtures** (`construction/mod.rs:4928, 5290, 5621, 6623, 7134, 7606, 7832` — ~5 000 LOC) are e2e gates the project still uses. They are NOT investigation residual. They MUST stay — but they should move OUT of `construction/mod.rs` and INTO either `construction/validation/` submodule (gated `#[cfg(any(test, e2e))]` or just `pub(crate)` in a separate file) OR the e2e harness. This is the structural-extraction move, not deletion.

When in doubt: **the rule is "GPU verification CPU oracle = sacred; investigation probes/PBR debug = expendable; e2e gates = move, don't delete"**.

### Q3 — D4 ↔ D5 order

**Chosen:** *D5 first, D4 second, D7 last.* D1, D2, D3, D6, D8 interleave between D5 and D7.

Rationale: D5's `render/construction/mod.rs` split is the single largest LOC + readability win in the port. Doing it first means D4's later refactor sees a clean construction-side surface. D5's architect must respect `render/gpu_types.rs`, `render/prepare.rs`, and `render/pipelines.rs::NaadfPipelines` as **read-only** (W0 seam contract — `docs/orchestrate/naadf-bevy-port/15-design-c.md` §1).

## Crosscutting constraints (all domains)

### Forbidden moves

1. **Do NOT widen scope past the assigned domain's paths.** Each domain has a fixed file list in `00-reuse-audit.md §2`. If an explorer or architect finds rot outside their domain, flag it in their side-notes section — do not edit cross-domain.
2. **Do NOT touch `aadf/edit.rs` public API** (D1). The CPU oracle is sacred per user directive. Internal refactor is fine; deleting public surface is not.
3. **Do NOT delete or rename anything cited by `e2e/`, `bin/e2e_render.rs`, or `e2e/tests/*.ts`** without first auditing the call graph. The e2e harness is the project's verification surface (CLAUDE.md). Architects may PROPOSE deletion in their docs; implementors must verify zero-callers before acting.
4. **Do NOT introduce behavioural divergence from C# NAADF without an explicit `docs/orchestrate/codebase-tightening/<domain>/03-architecture.md` entry flagging it.** The faithful-port rule (`bevy-naadf-faithful-port-rule` in user memory) is binding. Idiomatic Rust/Bevy improvements that preserve behaviour are encouraged; behavioural changes need user sign-off.
5. **Do NOT run `cargo run --bin bevy-naadf` as a "verification" step.** Per CLAUDE.md, the named e2e gates (`baseline`, `--validate-gpu-construction`, `--edit-mode`, `--entities`, `--vox-e2e`, `--oasis-edit-visual`, `--runtime-edit-mode`) are the verification surface. Implementors run the relevant gate; user does live visual checks.
6. **Do NOT delete or substantially modify any `wgsl` shader without a Rust↔WGSL agreement audit.** SSoT-1 / SSoT-3 / SSoT-4 / SSoT-6 in `00-reuse-audit.md §3.1` enumerate where the two sides diverge. Tightening those is in scope; silently breaking them is not.
7. **Do NOT cross-edit `gpu_types.rs`, `prepare.rs`, `pipelines.rs`, or other D4↔D5 shared files from D5's implementor session.** D5 treats them as read-only. D4's implementor refactors them. If D5's architect identifies a need to change them, the architect doc flags it as a D4-blocker.

### Verification gates (per CLAUDE.md)

- `cargo build --workspace` — proves compilation.
- `cargo test --workspace --lib` — proves unit + integration logic.
- `cargo run --bin e2e_render -- <mode>` — runtime gates (`baseline`, `--validate-gpu-construction`, `--edit-mode`, `--entities`, `--vox-e2e`, `--oasis-edit-visual`, `--runtime-edit-mode`).
- Re-run non-deterministic gates (`oasis_edit_visual`, `vox_gpu_oracle`) ≥2× per `feedback-multiple-runs-rule-out-false-positives`.
- Playwright e2e runs `--headed` per `playwright-e2e-must-be-headed`; channel `chrome` per `feedback-playwright-channel-google-chrome-stable`.

## Required reading (in order)

Every explorer / architect / implementor reads these files first:

1. This file (`docs/orchestrate/codebase-tightening/01-context.md`).
2. `docs/orchestrate/codebase-tightening/00-reuse-audit.md` — focus on §2 (domain decomposition) for your domain's row, then §3 (crosscutting) for items flagged in your domain.
3. Your domain's group dir: `docs/orchestrate/codebase-tightening/<your-domain>/02-exploration.md` (architects + implementors).
4. `docs/orchestrate/codebase-tightening/<your-domain>/03-architecture.md` (implementors only).
5. The CLAUDE.md at `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md` — verification discipline.
6. The files cited in your domain's row in `00-reuse-audit.md §2`. Read them in full where small (<500 LOC); read with line-range targeting for the larger ones.

## Crosscutting reuse map (audit §3 summary)

Each domain's explorer + architect should consult `00-reuse-audit.md §3` for items flagged in their domain column:

- **SSoT-1** (max_ray_steps_* family, 5 fields × 5 locations) — `D4 + D7 + D2`.
- **SSoT-2** (WORLD_SIZE_IN_CHUNKS/VOXELS/SEGMENTS derived 3×) — `D7`.
- **SSoT-3** (CELL_DIM = 4 / CELL_CHILDREN = 64, hardcoded in ~25 WGSL files) — `D1 + D4 + D5`.
- **SSoT-4** (storage counts in `render/gi.rs:51-60` vs WGSL literals) — `D4`.
- **SSoT-5** (TAA ring depth — mostly OK, audit complete) — `D4`.
- **SSoT-6** (hash coefficients, 3 implementations agree?) — `D1 + D5`.
- **DUP-1** (3+ set-voxel entry points on WorldData) — `D1`.
- **DUP-2** (3 brush-shape AABB/classify fns) — `D2`.
- **DUP-3** (5 `naadf_sample_refine_*_node` systems) — `D4`.
- **DUP-4** (3 `validate_gpu_construction*` variants) — `D5`.
- **DUP-5** (4 `run_one_*_byte_diff` fixtures) — `D5`.
- **DUP-6** (camera-write boilerplate across 7 e2e pin_*_camera systems) — `D6`.
- **DUP-7** (2 `build_segment_voxel_buffer*`) — `D5`.
- **BEV-1** (17-element `.chain()` instead of RenderGraph labels) — `D4`.
- **BEV-2** (hand-padded `Pod` structs vs `ShaderType`) — `D4`.
- **BEV-3** (`ConstructionGpu` with 16+ `Option<Buffer>` fields) — `D5`.
- **BEV-4** (function-pointer KNOBS table vs `Reflect`) — `D2`.
- **BEV-5** (no `Added<T>`/`Changed<T>` filters) — `D3 / D6`.
- **BEV-6** (`Option<Res<X>>` ladders vs `.run_if(resource_exists::<X>)`) — `D5`.
- **OA-1** (KnobKind reimplementing reflection) — `D2`.
- **OA-2** (`ConstructionPipelines` empty-sibling pattern) — `D5`.
- **UA-1** (anonymous (IVec3, VoxelTypeId) tuples) — `D1 + D2`.
- **UA-2** (WGSL bare literals shadowing storage counts) — `D4`.
- **UA-3** (raw u32 chunk-pos masks bypassing pack/unpack helpers) — `D1 + D5`.
- **UA-4** (raw `DIR_*` indices vs `Dir6` enum) — `D1`.

## Side-notes from the audit you should know

(from `00-reuse-audit.md §Side notes`)

- The Rust port's 4× LOC is real but **not uniform** — concentrated in (a) the 11k mod.rs, (b) deliberate e2e harness, (c) verbose docs ethos. Pursuing pure LOC parity damages (c); pursue idiom-fit instead (Q1 confirms).
- `crates/voxel_noise/` is the easiest single deletion win (1 033 LOC).
- The 4-phase orchestration history (Phase A → A-2 → B → C) left scaffolding in code: `render/graph.rs` vs `render/graph_b.rs`, `aadf/construct.rs` vs `render/construction/`. Audit whether the "completed" port can retire some of this scaffolding.
- D4 ↔ D5 share `render/gpu_types.rs`, `render/prepare.rs`, `render/pipelines.rs`. D5 must treat these read-only (W0 seam contract). Implementor order (D5 first → D4 second) is the user's chosen sequencing.
- The audit ran `wc -l` (not `tokei`/`cloc`); the 4× ratio is "source-lines-including-blanks-and-comments". With `tokei --no-blanks` the multiplier may land closer to ~3×. Order of magnitude is right, exact figure is approximate.

## Auditor confidence

- **High confidence**: file/line citations (verified with Read/Grep), domain boundary placements, SSoT divergences enumerated in §3.1.
- **Medium confidence**: estimated tightening surface per domain (some may turn out unfixable when an explorer looks closely).
- **Low confidence (architects: re-examine)**: the OA-1 / BEV-4 reflect-driven settings recommendation — the project may deliberately keep KNOBS explicit for compile-time safety; architects judge.

---

## Addendum (2026-05-20, after explorer phase) — master-branch identity + resolution of explorer hard-gate flags

### Master-branch identity (user clarification — read this first)

**bevy-naadf master is two things, and nothing else:**

1. **A minimal, complete, fair port of the C# NAADF reference** (`/mnt/archive4/DEV/NAADF/`).
2. **Reference footnotes for the Unity port** (the next downstream target, per [[naadf-getraydir-monogame-conventions]]).

**PBR raymarching work lives on a SEPARATE branch and is already ready.** Master does NOT carry PBR scaffolding. The split is intentional: master demonstrates the paper's algorithm faithfully; PBR compounds on top in its own branch.

**Consequence for every architect:** when weighing a "delete vs keep" call, ask "is this in C#?" or "is this an idiomatic-Bevy improvement that the Unity port would want to know about?" If neither, **delete**. PBR-related code on master is suspect by default — it lives on the PBR branch instead. Aggressive deletion of investigation residuals, dead PBR scaffolding, and stalled-design scaffolds is **encouraged**, not just permitted.

The CPU oracle (`aadf/edit.rs`) is the load-bearing exception per the prior Q2 — it stays because GPU divergence verification needs it. Everything else flagged "investigation residual" goes.

### Hard-gate Q&A resolutions (2026-05-20)

After all 8 explorers returned, four high-severity decisions surfaced that the user resolved before architect dispatch.

#### Resolution A — `device_snapshot` full delete-chain (D7 + D6 coordinated)

User confirmed: **delete the whole chain.** D7 + D6 implementors coordinate. Scope:

- `diagnostics::device_snapshot` submodule (`diagnostics.rs:155-711`, ~560 LOC) — DELETE.
- `bin/diag_compare.rs` (314 LOC) — DELETE.
- `--device-snapshot-native` CLI flag in `bin/e2e_render.rs:139-143,364-375` — DELETE.
- `e2e/tests/device-snapshot.spec.ts` — DELETE.
- Any Playwright config wiring for the above — DELETE.

Total drop: ~900 LOC + the Playwright spec. D7's architect designs the simplified `DiagnosticsPlugin` (press-P dump only). D6's architect notes the coupling so D6 impl phase coordinates with D7's last-position impl phase. **D6 architect may need to land their parts of the deletion ahead of D7** (test spec + CLI flag) — architects coordinate.

#### Resolution B — D8 asset-pipeline: Option A (delete runtime consumers, keep bake binary)

User confirmed: **Option A.** Scope:

- DELETE `texture_array/**` (785 LOC).
- DELETE `baked_material.rs` (225 LOC, including the `extensions() returns &["ron"]` footgun).
- DELETE `material_set/mod.rs` (60 LOC, verbatim-pasted from stalled `pbr-raymarching` design).
- DELETE the `basis-universal` Cargo dep + native C++ encoder build path.
- DELETE the 14 lines in `lib.rs` that register the dead plugins.
- **KEEP `bin/bake.rs` (96 LOC)** — InstaMAT offline batch baker, runs pre-build via justfile per [[instamat-bake-to-disk]]. Stays as scaffolding for InstaMAT integration.
- **KEEP** the `bake-texarrays` justfile recipe (4 lines).

Total drop: ~1 100 LOC + a C++ build dep. **Rationale:** master is the C# port; PBR raymarching lives on a separate branch where the runtime baked-material consumer will be wired. Re-introducing the runtime path here would be a PBR-branch concern, not a master-branch concern.

#### Resolution C — PBR e2e gates: delete

User confirmed: **delete all 3 + CLI dispatch.** Scope:

- DELETE `e2e/pbr_debug_modes.rs` (218 LOC).
- DELETE `e2e/pbr_hard_edge.rs` (1 023 LOC).
- DELETE `e2e/pbr_visual.rs` (747 LOC).
- DELETE `--pbr-debug-modes`, `--pbr-hard-edge`, `--pbr-visual` from `bin/e2e_render.rs`.
- DELETE corresponding `AppArgs.pbr_*_mode` fields (D7 territory — coordinate).
- DELETE `e2e/mod.rs` registry entries for the three.

Total drop: 1 988 LOC. **Rationale:** PBR gates live on the PBR branch with the rest of the PBR work; master stays clean.

#### Resolution D — W0 seam contract retired

User confirmed: **architect proposes the merge, implementor lands it.** D5's architect:

- Proposes folding `ConstructionPipelines` into `NaadfPipelines` (or unifying behind a single `Pipelines` resource).
- Notes the merge in D4↔D5 shared-file notes so D4's later impl phase respects the merged shape.
- Documents the W0-contract retirement in the architecture doc as a deliberate divergence from `15-design-c.md` (which becomes historical).

This is a structural simplification, not a feature change. Behavioural parity with C# preserved.

### Cross-domain coordination notes (for architects)

- **D6 ↔ D7 chain (device_snapshot + diag_compare + spec)**: D6 architect lays the e2e-side deletions; D7 architect lays the production-side deletions. Implementor sequence (user-decided): D5 first, then D4, then interleave D1/D2/D3/D6/D8, then D7 last. D6 impl can land the spec + CLI deletions before D7 lands the production submodule deletion.
- **D7 ↔ D2 ↔ D4 SSoT-1 chain**: `GiSettings` (D7's `lib.rs`) → `KNOBS` table (D2's `settings.rs`) → `GpuRenderParams.max_ray_steps_*` (D4's `gpu_types.rs`). D7's architect proposes the canonical Rust struct location; D2's architect proposes the `Reflect`-or-decl-macro KNOBS shape; D4's architect proposes the uniform consumer shape. All three architects produce designs simultaneously — they must each cite the other two domains' explorer findings (`02-exploration.md`) to land coherent designs.
- **D5 ↔ D4 shared seam**: `gpu_types.rs` / `prepare.rs` / `pipelines.rs` stay read-only for D5. D5 architect notes what D4 needs to change in those files (per D5 explorer `## D4↔D5 shared-file notes`); D4 architect designs the changes.
- **D3 dependency arrow inversion (D3 Finding 6)**: `web_vox::pin_web_horizon_camera` + `grid::install_imported_vox` import camera-pose constants FROM `e2e/vox_horizon_parity`. D3 architect proposes reversing this — constants move out of e2e/, into a non-e2e module, e2e gates import them. **Approved.**
- **D1 state-bit encoding regime mismatch (D1 Finding 2)**: three regimes silently coexist around paper §3.1's encoding (Rust flag-bit, Rust state-nibble-with-magic-literals, WGSL named state-nibble constants). D1 architect picks one canonical encoding and proposes migration. Faithful-port rule: whichever is closest to C# NAADF's encoding wins; D1 architect cross-checks against C# `World/Data/ChangeHandler.cs` or `EditingHandler.cs`.

### Architect-phase brief framing

All 8 architect dispatches receive:
- This `01-context.md` + the resolutions above.
- Their domain's `02-exploration.md` (each architect reads their own, in full).
- A pointer to other domains' explorer outputs ONLY if cross-domain coordination is required (D2/D4/D7 read each other's SSoT-1 sections; D5 + D4 read each other's shared-file notes).

Architect outputs go to `<domain>/03-architecture.md`. The orchestrator does NOT read these per user directive. Implementors read their own domain's architecture + exploration directly.
