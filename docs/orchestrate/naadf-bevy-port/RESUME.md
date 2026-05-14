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

## THE NEXT DISPATCH — do this first

A single "compound" agent (designs + implements in one continuous dispatch, grounded in the
NAADF C# reference at `/mnt/archive4/DEV/NAADF/`) for **two reported TAA bugs**:

1. **PRIMARY — TAA never resolves / perpetually noisy.** The port's TAA output stays noisy
   and never resolves to a clean image, unlike the C# NAADF version. Tasks: (a) establish an
   e2e test that analyzes noise in the captured screenshot up-close — a per-pixel /
   local-high-frequency / local-variance noise metric, gated; (b) study NAADF's C# TAA +
   denoiser to learn how it resolves clean (the long-term-memory TAA accumulation, the
   16-deep `taa_samples` ring, sample-count weighting, the sparse bilateral denoiser's role,
   frame-over-frame convergence); (c) find why the port stays noisy where C# resolves;
   (d) mitigate faithfully — port what C# does.
2. **SECONDARY — TAA goes black on window resize.** A framebuffer-resize resource-lifecycle
   bug: on window/framebuffer resize, the TAA history/accumulation buffers (possibly also GI
   buffers, the camera-history ring) are likely not correctly reallocated/reset → TAA reads
   stale/zero/wrong-sized data → black. Investigate the resize path, fix faithfully, add e2e
   resize coverage if feasible. (The e2e harness uses a fixed window size — structurally
   blind to this.)

Verification: `cargo build` + `cargo test` (currently 46) + `cargo run --bin e2e_render`
(with the new noise metric + ideally resize coverage) + Read `target/e2e-screenshots/e2e_latest.png`.
Cap e2e runs ~10. Deliverable: append a section to `10-impl-b.md`.

## Other open items (after the above)

- **Review follow-ups** (`11-review-b.md` — non-blocking): #1 the e2e harness's dead
  "temporal-stability gate" scaffolding (implement or delete); #3 `expected_spans(6)` not
  `is_denoise`-config-aware; #5 dead plumbing debris from the B2/B6 seams
  (`FLAG_BLIT_FINAL_COLOR`, dormant `taa_layout`, the `taa_sample_accum` no-op touch); #6
  advisory — add a mechanical GPU-struct-offset assert harness (the `vec3`-then-scalar layout
  class recurred 3×).
- **Phase C** (GPU-based world construction/editing) — a deliberate future phase, explicitly
  deferred; out of the current scope.

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
