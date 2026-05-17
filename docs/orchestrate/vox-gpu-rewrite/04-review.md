# vox-gpu-rewrite — fresh-eyes review brief

You are the fresh-eyes reviewer. Read **only this file**. Do NOT read
`01-context.md`, `02-design.md`, `03-impl.md`, or `00-reuse-audit.md` — the
parent orchestration is denying you the design rationale on purpose so you
catch assumptions the implementer silently baked in.

The implementer's intent is irrelevant to the review. Your job is to verify
the **artifact under review** against the **success criteria below**, using
only what you can directly observe in the code.

---

## Artifact under review

A git diff representing all changes landed by the `vox-gpu-rewrite`
orchestration on this branch. To produce it, run from the project root:

```bash
git diff main...HEAD
```

(or against whatever base branch this orchestration started from; if `main`
is the current branch, ask the orchestrator for the start ref).

Scope is roughly:

- `crates/bevy_naadf/src/voxel/grid.rs` — `install_vox_in_fixed_world` rewrite.
- `crates/bevy_naadf/src/voxel/vox_import.rs` — three function deletions + two test deletions + docstring updates.
- `crates/bevy_naadf/src/aadf/generator.rs` — `ModelData` gains `#[derive(Resource, Clone)]`.
- `crates/bevy_naadf/src/render/extract.rs` — new `ModelDataRender` resource + `stage_model_data_buildonce` extract system.
- `crates/bevy_naadf/src/render/mod.rs` — extract system registered.
- `crates/bevy_naadf/src/render/construction/mod.rs` — `ConstructionGpu` + `ConstructionBindGroups` extended; `prepare_construction` gains the W5 buffer-allocation + bind-group block; `naadf_gpu_producer_node` gains the W5 segment-loop branch.
- `crates/bevy_naadf/src/render/construction/generator_model.rs` — new `dispatch_generator_model_with_encoder` sibling helper; existing `dispatch_generator_model` refactored to call it internally.
- `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` — new e2e module.
- `crates/bevy_naadf/src/e2e/mod.rs` — module export.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — `--vox-gpu-construction` flag parsing + dispatch branch.
- `docs/orchestrate/naadf-bevy-port/12-alignment-gap.md` — appended W5.6 divergence note.

---

## Success criteria

### Functional correctness

1. **The `.vox` → fixed-world load path uses the GPU generator chain.** Loading
   a `.vox` file via `GridPreset::Vox + fixed_world_size = true` must NOT
   call any CPU tiling function — those functions must no longer exist in
   `vox_import.rs`. Verify by greping the deleted function names
   (`tile_buckets_into_world`, `parse_dot_vox_data_into_world`,
   `load_vox_into_world`) — they must produce ZERO matches in the whole
   `crates/` tree (other than possibly in docstring history references; if
   present, flag whether they're load-bearing).
2. **The GPU dispatch chain matches C# `WorldData.cs:120-156`.** Read both
   `crates/bevy_naadf/src/render/construction/mod.rs::naadf_gpu_producer_node`'s
   W5 branch AND `/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:120-156`
   side-by-side. Verify:
   - The Rust loop iterates the same number of segments
     (`WORLD_SIZE_IN_SEGMENTS = (16, 2, 16) = 512` segments).
   - The iteration ORDER matches (X/Y/Z vs Y/Z/X — C# is the source of truth).
   - Per-segment, the Rust loop writes a `GpuGeneratorModelParams` uniform
     whose `group_offset_in_chunks` matches the C# `GeneratorModelParams.groupOffsetInChunks`
     set in `/mnt/archive4/DEV/NAADF/NAADF/World/Generator/WorldGeneratorModel.cs:32-60`.
   - Per-segment, the Rust loop dispatches `generator_model` THEN `chunk_calc`
     in that order.
   - After the loop, the bounds chain (`compute_voxel_bounds` +
     `compute_block_bounds`) runs ONCE, not per-segment.
3. **The `generator_model.wgsl` shader is unchanged.** Diff this file (or
   confirm zero changes). It is an audited port; any change is a regression.
4. **The W5 unit test still passes.** `cargo test --workspace --lib` — the
   pre-existing test `generator_model_gpu_vs_cpu_bit_exact` must still pass.
   The implementer was allowed to refactor `dispatch_generator_model` to
   call a new sibling; verify the test still uses the original
   `dispatch_generator_model` entry point (the unit-test caller path).
5. **The default-scene path is unchanged for `--grid-preset Default + fixed-world-size=true`.**
   Loading `bevy-naadf` with NO `--vox` flag must still go through the CPU
   `compose_default_scene_into_fixed_world` path. The W5 producer gate must
   short-circuit when no `ModelDataRender` resource is present.
6. **The legacy non-fixed-world `.vox` path is preserved.** `install_vox_sized_to_model`
   (the `--vox` path that sizes the world to the model's tiled extent) must
   still call `load_vox_tiled` → `replicate_buckets_xz`. These must NOT be
   deleted (only the FIXED-world helpers go away).

### Verification surface

7. **The new W5.5 e2e gate exists and goes green.** Verify:
   ```bash
   cargo run --release --bin e2e_render -- --vox-gpu-construction
   ```
   exits 0. Read the new module `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs`
   and check that it:
   - Boots `GridPreset::Vox + fixed_world_size = true + gpu_construction_enabled = true`.
   - Uses the `OASIS_VOX_FIXTURE_PATH` fixture (or an equivalent in-tree
     `.vox` file; verify the path resolves).
   - Asserts framebuffer is non-empty in a region the camera frames.
   - Optionally asserts pipeline cache has no compile errors.
8. **No pre-existing e2e gate regresses.** Run the full e2e suite and verify
   all pre-existing gates pass:
   ```bash
   cargo run --release --bin e2e_render -- --baseline
   cargo run --release --bin e2e_render -- --validate-gpu-construction
   cargo run --release --bin e2e_render -- --edit-mode
   cargo run --release --bin e2e_render -- --entities
   cargo run --release --bin e2e_render -- --vox-e2e
   cargo run --release --bin e2e_render -- --oasis-edit-visual
   cargo run --release --bin e2e_render -- --runtime-edit-mode
   cargo run --release --bin e2e_render -- --small-edit-visual
   cargo run --release --bin e2e_render -- --small-edit-repro
   ```
   If any regress, flag which one + the failure mode.
9. **`cargo build --workspace` and `cargo test --workspace --lib` pass clean.**
   Baseline at start of orchestration was 198 passed/1 ignored. Verify the
   total is at least 198 + however many new tests the implementer added.

### Architecture / hygiene

10. **`ConstructionGpu`'s `Option<Buffer>` discipline is preserved.** Every
    new field on `ConstructionGpu` must be `Option<Buffer>` initialised to
    `None` (per the seam contract docstring at
    `crates/bevy_naadf/src/render/construction/mod.rs:84-103`). Flag any
    field that isn't.
11. **No new `AppArgs::vox_gpu_construction_mode` flag was added.** Verify
    by reading `crates/bevy_naadf/src/lib.rs` `AppArgs` struct — the new
    e2e gate uses the production path, not a driver mode.
12. **`WorldDataMeta` was NOT extended with model_data fields.** Verify the
    new `ModelDataRender` is a separate render-world resource. Flag if
    `WorldDataMeta` grew model_data fields.
13. **The W5 branch in `naadf_gpu_producer_node` uses
    `render_context.command_encoder()` for ALL segment dispatches** (not
    one-encoder-per-segment with submit). Flag if the implementer
    submit-per-segment'd.
14. **The new W5.6 divergence note exists in
    `docs/orchestrate/naadf-bevy-port/12-alignment-gap.md`** explaining why
    the CPU default-scene compose path is retained.

### Out-of-scope check

15. **The AADF startup convergence race was NOT touched in this PR.** The
    bounds_calc pipeline-compile latency + W3 AADF convergence race
    (~330 ms cold-start single-stepping rays) was explicitly deferred per
    handoff. Any code change touching `compute_aadf_layer`,
    `bounds_calc`-pipeline-compile timing, or
    `synchronous_pipeline_compilation` is out of scope and should be
    flagged.

---

## Review deliverable

Write your review to
`/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/vox-gpu-rewrite/04-review.md`
by **appending** to this file (don't overwrite the brief above) under the
section heading:

```
## delegate-reviewer findings (<ISO date>)
```

Structure:

- `### Verdict` — PASS / FAIL / PASS-WITH-FLAGS. One sentence.
- `### Per-criterion findings` — one paragraph per success criterion #1–#15.
  For each: state OK / FLAG, cite file:line evidence, do NOT speculate about
  intent. Be terse.
- `### Findings` — anything beyond the criteria that you notice. Code smells,
  drift from the codebase's surrounding style, unused fields, dead branches,
  panic paths, `unwrap()` chains in render-graph code, etc. Each finding =
  file:line + observation + severity (HIGH / MEDIUM / LOW).
- `### Out-of-scope discoveries` — anything that looks broken but is clearly
  not what this PR was about. Useful for the orchestrator to file as a
  followup; do not fail the review over it.

The orchestrator will reconcile your flags against the design rationale at
synthesis time. Some of what you flag may already be answered by decisions
in `01-context.md` that you were deliberately not shown — that's fine, it's
how generator-verifier loops work. The orchestrator's job (not yours) is to
say "this flag is real" vs "this flag is answered by Q2 in the Q&A".

---

## Hard rules

- Do not read `01-context.md`, `02-design.md`, `03-impl.md`, or `00-reuse-audit.md`.
- Do not run `cargo run --bin bevy-naadf` as a verification step (per
  project `CLAUDE.md`).
- Do not infer intent from commit messages; review the code.
- Do not propose alternative designs; your job is to flag, not redesign.
- Verify every cited file:line by Read or Grep — no hallucinated line numbers.
