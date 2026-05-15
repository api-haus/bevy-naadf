# RESUME — `naadf-bevy-port` orchestration

**To continue:** `/delegate continue naadf-bevy-port`
Read this file first, then `README.md` (index + phase checklist), then `01-context.md` (canonical context).

## Where this stands (2026-05-15)

Port of **NAADF** — a C#/MonoGame voxel global-illumination engine ("Nested Axis-Aligned
Distance Fields", Ulschmid et al. CGF 2026) at `/mnt/archive4/DEV/NAADF` — into
**Rust/Bevy 0.19-rc.1** at `/mnt/archive4/DEV/bevy-naadf`.

- **Phase A** (NAADF substrate: voxel + AADF + world + camera; albedo first-hit DDA) — COMPLETE, review-gated.
- **Phase A-2** (NAADF's 16-frame long-term-memory TAA) — COMPLETE, review-gated.
- **Phase B** (real-time `WorldRenderBase` GI: 4-plane first-hit, atmosphere, `rayQueueCalc`
  ~0.25-spp, compressed ReSTIR GI, sparse bilateral denoiser, `base/` TAA, final blit) —
  impl-complete, **review gate PASSED** (`11-review-b.md`).
- **Gap analysis** (`12-alignment-gap.md`): 16 in-scope subsystems — 7 faithful,
  9 faithful-with-documented-deviations, 0 diverging. The in-scope port is functionally
  complete and faithful; GI bounce is genuinely visible.
- Branch `feat/phase-b-gi` was merged into `main` on 2026-05-15. **To continue: create a
  fresh worktree from local `main`** — `docs/orchestrate/naadf-bevy-port/` and all code are
  on `main`.

## PRIORITY REDIRECT — 2026-05-15 (third Architectural Q&A)

The user redirected the next priority away from the TAA bug-fixes (below, now folded into
Phase C as the B-1 fix-first item) toward **Phase C — canonical methodology completion**:
the GPU build algorithm + the complete canonical NAADF+GI methodology per the paper. Scope
locked by Q&A E1–E4 (`01-context.md` §2e):

- **E1 scope:** all 4 paper contributions — GPU hashing construction (Algorithm 1), O(3·d·n)
  AADF construction, world generation, editing + flood-fill AADF invalidation, background
  AADF queues, dynamic entities. SVGF OUT (un-portable from NAADF source).
- **E2:** the TAA-never-resolves / camera-motion reprojection-decay bug (B-1) is **fixed
  first**, before any Phase-C construction work.
- **E3:** seam-first design — a self-contained construction sub-module/sub-graph owns the
  shared render-graph wiring; workstreams then fan out into parallel worktrees.
- **E4:** the CPU construction path (`src/aadf/construct.rs`) is kept as a bit-exact test
  oracle + fallback — not deleted.
- **Execution:** distributed `/delegate` via the **team** system + parallel git worktrees,
  one per workstream; the orchestrator stays the coordinator.

Foundation docs done: `13-reuse-audit-c.md` (GPU-construction reuse audit) +
`14-paper-gap.md` (canonical-paper gap table + prioritized completion list).

## THE NEXT DISPATCH — do this first (TAA-fidelity track, per E2 — explore-first)

The user refined the B-1 scope (2026-05-15):
- **Camera-motion reprojection-decay — NOT a live bug.** The user and `12-alignment-gap.md`
  agree it was already resolved (the `sync_position_split` fix). Dropped from scope.
- **Black-on-resize — confirmed real.** A framebuffer-resize resource-lifecycle bug: on
  window/framebuffer resize, the TAA history/accumulation buffers (possibly also GI buffers,
  the camera-history ring) are likely not correctly reallocated/reset → TAA reads
  stale/zero/wrong-sized data → black. The fixed-size e2e harness is structurally blind to it.
- **TAA noisier than C# / barely resolves — the real problem.** The port's TAA is noticeably
  noisier than the C# NAADF version and barely resolves; the C# is only slightly noisy when
  zoomed into a shadow-band area between two surfaces. Cause unknown — could be implementation
  details, entirely missing pipeline parts, or plain configuration differences.

Approach: **explore first, then fix.** Dispatch a read-only diagnosis agent that compares the
port's full TAA + denoiser + GI-accumulation pipeline against the NAADF C# reference + the
paper, and produces a ranked list of suspected causes → `18-taa-fidelity.md`. Then a
code-mutating fix dispatch (in a worktree branched from local `main`) brings TAA fidelity to
at least the C# level and fixes black-on-resize. **Bar = the C# version, not a perfect
renderer.**

**Diagnosis COMPLETE** → `18-taa-fidelity.md` (5 ranked causes; pipeline structurally complete,
no missing passes). Top causes: #1 GI rays unjittered (`GpuGiParams` has no jitter field) — the
dominant "barely resolves" mechanism; #2 `exposure`/`tone_mapping_fac` swapped vs C#; #3 16-deep
ring (secondary). Black-on-resize root cause pinned: `extract_camera`'s `.unwrap_or(UVec2::new(1,1))`.

**Fix dispatched** (one `general-purpose` agent, worktree `.claude/worktrees/taa-fidelity`,
branch `fix/taa-fidelity`) with two user-directed scope changes (2026-05-15):
- **#2 revised** — do NOT just swap the tonemap constants; **switch to Bevy's built-in
  tonemapping**: output raw linear HDR from the final/raymarching pass, drop the port's custom
  Reinhard tonemap. A deliberate user-directed deviation from the faithful-port principle.
- **#3 revised** — not deferred; **make the TAA ring depth configurable, default 32**
  (supersedes the binding 16-deep decision in `design-exploration-qa.md` §6 / `01-context.md`
  §2c).
Plus #1 (jitter GI rays), #4 (black-on-resize), audit #5. Verification: `cargo build` +
`cargo test` (currently 46) + `cargo run --bin e2e_render` (cap ~10) + Read
`target/e2e-screenshots/e2e_latest.png` — note the Bevy-tonemapping switch will change the
output image and likely needs honest e2e-gate recalibration. Deliverable: `18-taa-fidelity.md`.

## Then — Phase C proper

`design` (`delegate-architect` → `15-design-c.md`: seam-first extension design + the
worktree/workstream decomposition plan respecting the construction→editing→queues dependency
DAG) → team-based parallel `impl` across worktrees (`16-impl-c.md`) → fresh-eyes `review`
(`17-review-c.md`). See `README.md` Phase-C checklist + `01-context.md` §2e.

## Other open items

- **Review follow-ups** (`11-review-b.md` — non-blocking): #1 the e2e harness's dead
  "temporal-stability gate" scaffolding (implement or delete); #3 `expected_spans(6)` not
  `is_denoise`-config-aware; #5 dead plumbing debris from the B2/B6 seams
  (`FLAG_BLIT_FINAL_COLOR`, dormant `taa_layout`, the `taa_sample_accum` no-op touch); #6
  advisory — add a mechanical GPU-struct-offset assert harness (the `vec3`-then-scalar layout
  class recurred 3×). Fold into Phase-C work where they overlap.

## Key working rules (carry these into any continuation)

- **`/delegate` mode:** orchestrator scopes/briefs/synthesizes; all code work is dispatched.
  A checkpoint-commit (a `general-purpose` agent on `model: sonnet`, commit-only,
  `git add -A .`) precedes every substantive dispatch. One substantive dispatch at a time.
  Sub-agents write deliverables to disk; the orchestrator reads only short status returns.
- **User pacing preference (as of 2026-05-15):** the user asked to run without per-dispatch
  confirmation pauses ("assume all confirmed, design and delegate"). A fresh session may want
  to re-confirm this still holds.
- **Faithful-port principle:** port what NAADF's C#/HLSL actually does; ground every fix in
  `/mnt/archive4/DEV/NAADF/`. Documented deviations are acceptable (MonoGame↔wgpu coordinate
  conventions, `M*v` matrix order, forced wgpu bind-group splits, explicit truncation casts,
  naga-oil naming constraints); novel inventions are not.
- **Verification = the e2e harness.** `cargo run --bin e2e_render` is a single deterministic
  windowed invocation: boots the real app, runs a fixed frame budget (incl. a moving-camera
  mode), a `PipelineCache` `CachedPipelineState::Err` scan catches every
  shader/naga-oil/pipeline error in one run, region/statistic gates, and it saves
  `target/e2e-screenshots/e2e_latest.png` for agentic-vision review. It replaced the live
  windowed smoke-run. Always cap e2e runs in a brief; never let an agent loop windowed-app
  restarts. Design + impl log: `e2e-render-test.md`.
- **Hazard class — WGSL `vec3`-then-scalar layout:** WGSL packs a scalar at +12 after a
  `vec3`; a Rust `#[repr(C)]` struct with explicit padding puts it at +16. This
  silent-corruption bug bit 3× in Phase B (`AtmosphereParams`, `GpuTaaParams`, `GpuGiParams`
  — all fixed by widening the affected rows to `vec4`). Audit any new shared GPU struct.
- **Worktree:** branch from local `main`, absolute paths for all operations inside
  `.claude/worktrees/`.

## Canonical docs (all under `docs/orchestrate/naadf-bevy-port/` on `main`)

`README.md` (index + phase checklist) · `01-context.md` (canonical context) ·
`02-research.md` (NAADF subsystem map) · `03/06/09-design*.md` (phase designs) ·
`04/07/10-impl*.md` (impl logs — `10-impl-b.md` has all Phase B batches + every bug-fix
section) · `05/08/11-review*.md` (review gates) · `12-alignment-gap.md` (port-vs-NAADF gap
analysis) · `e2e-render-test.md` (the e2e harness design + impl log) ·
`design-exploration-qa.md` (methodology Q&A).
