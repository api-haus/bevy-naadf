# Phase B (GI) — Review Brief

**This file is the review agent's entire brief.** Read ONLY this file as your context. Do
NOT read `01-context.md`, `09-design-b.md`, or any other orchestrate file — the point of a
fresh-eyes review is that you do not share the implementer's context or design rationale, so
you can catch assumptions that were silently baked in. You MAY (and should) read the artifact
itself: the code, the NAADF reference source, and the impl log.

## What was built

Phase B is a port of NAADF's real-time `WorldRenderBase` global-illumination pipeline from
C#/MonoGame+HLSL into Rust/Bevy 0.19-rc.1 WGSL. NAADF ("Nested Axis-Aligned Distance Fields",
Ulschmid et al., CGF 2026) is a voxel GI engine. The port lives on branch `feat/phase-b-gi`
in the worktree `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`. It was built in
6 batches plus 3 bug-fix passes (logged in `10-impl-b.md`).

## Artifact under review (use ABSOLUTE paths)

- **The branch diff.** Worktree root: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`,
  branch `feat/phase-b-gi`. The full diff from the branch base (the `main` commit at Phase-A-2
  close) to HEAD is the Phase B work. Find the base with `git merge-base HEAD main` and diff.
- **The impl log:** `docs/orchestrate/naadf-bevy-port/10-impl-b.md` — what the implementers
  claim they did, batch by batch, plus the 3 fix sections. Read it to see the claims, then
  verify them against the actual code. Treat it as claims to check, not as ground truth.
- **The Phase B render code:** `src/render/` (`gi.rs`, `graph_b.rs`, `prepare.rs`, `taa.rs`,
  `atmosphere.rs`, `color_compression.rs`, `gpu_types.rs`, `pipelines.rs`, `extract.rs`,
  `mod.rs`) and `assets/shaders/` (the GI/atmosphere/TAA WGSL).
- **The e2e verification harness:** `src/lib.rs`, `src/bin/e2e_render.rs`, `src/e2e/`.
- **The NAADF reference source (ground truth):** `/mnt/archive4/DEV/NAADF/` — the C#/HLSL
  engine this is a port of. The port is measured against this. Research digest also available
  at `docs/research/ulschmid-2026-naadf-voxel-gi.md`.

## Success criteria — verify each

1. **Faithful port of NAADF's real-time `WorldRenderBase` GI.** Every in-scope subsystem
   should faithfully port NAADF's source behaviour: the 4-plane first-hit; `rayQueueCalc`
   adaptive ~0.25-spp sampling; compressed ReSTIR GI (`globalIllum`, `sampleRefine`'s 5
   passes, `spatialResampling`); the sparse bilateral denoiser; the atmosphere model; the
   `base/` long-term-memory TAA. Spot-check each against the NAADF HLSL/C# source — flag
   divergences in algorithm, constants, bit-layouts, or dispatch structure.
2. **The adaptive ~0.25-spp signal is real.** `rayQueueCalc` should produce a per-pixel
   sample-count signal that actually drives the GI sampling — verify it is wired through and
   consumed, not decorative.
3. **Render graph.** The GI pipeline should be wired as NAADF's compute-node dispatch order
   (~13 nodes). Verify the node order and the inter-node buffer dependencies are coherent.
4. **Scope discipline — these must NOT be present:** a reference pathtracer; DLSS / DLSS-RR;
   editor GUI; persistence; asset importers. They were explicitly out of scope. Flag any
   trace of them.
5. **GPU struct layout correctness.** Every Rust `#[repr(C)]` GPU struct and its WGSL
   counterpart must have matching byte layouts. In particular: WGSL packs a scalar at offset
   +12 immediately after a `vec3`, but a `#[repr(C)]` Rust struct with explicit padding puts
   the next field at +16 — a `vec3`-then-scalar shape that is not handled (e.g. by declaring
   the row `vec4`) is a silent corruption bug. This exact class recurred multiple times in
   this port. **Audit every uniform and storage struct shared between Rust and WGSL for an
   unfixed instance of this or any other layout mismatch.**
6. **Correctness gates — run them yourself.** From the worktree root: `cargo build` must be
   clean; `cargo test` must pass (expected: 46 tests); `cargo run --bin e2e_render` (the
   windowed e2e render-test harness) must exit 0 with all gates green — including the
   GI-visible gates. After the e2e run, `Read` `target/e2e-screenshots/e2e_latest.png` and
   judge it independently: is the voxel scene genuinely lit by colored GI bounce from the
   emissive blocks, or does it look wrong / under-converged / faked?
7. **Forced/deliberate deviations are sound.** The impl log records several deviations from a
   straight port — among them: a wgpu `STORAGE_READ_WRITE`+`INDIRECT` bind-group split; GI
   settings shipped as fixed constants rather than runtime-tunable; a `screenPosDistanceSqr`
   threshold of `16.0`; several `vec3`→`vec4` WGSL layout fixes; an e2e frame budget of 96
   frames for ReSTIR temporal convergence. For each, assess: is it actually forced /
   justified, and is it faithful to NAADF's intent?
8. **The e2e harness is an honest verification artifact.** Review `src/e2e/` and
   `src/bin/e2e_render.rs`: does the harness genuinely verify (real region/statistic gates, a
   `PipelineCache` error scan that would actually catch shader failures, honest thresholds),
   or does it rubber-stamp? Flag any gate that would pass a broken render.

## Deliverable shape

Write your review to `docs/orchestrate/naadf-bevy-port/11-review-b.md`, appending under the
heading `## delegate-reviewer findings (2026-05-15)`. Structure:

- **Numbered findings.** Each: a severity tag — `BLOCKER` / `CONCERN` / `NIT` — the issue, the
  `file:line` reference, and a recommended action.
- **Per-criterion verdict.** One line per success criterion (1–8 above): met / not met / met
  with caveats.
- **Final verdict:** an explicit `Phase B review gate: PASS` or `Phase B review gate: FAIL`.
  FAIL if any `BLOCKER` finding stands.

Do NOT commit, push, or amend. Do NOT edit code — you are reviewing, not fixing. Your only
write is your findings into this file.
