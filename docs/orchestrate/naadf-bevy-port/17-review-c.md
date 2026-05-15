# 17 — Phase C review brief (`delegate-reviewer`)

**This is the fresh-eyes review brief for Phase C.** You read **only this file** as your guide — deliberately not `01-context.md`, `15-design-c.md`, or the per-workstream impl logs. Your job is to verify the artifact against the success criteria below using your own reading of the code + the NAADF C# reference + the canonical paper, surfacing assumptions the implementers may have silently baked in.

You are an independent reviewer. You ARE allowed to read the impl logs (`16-impl-c-W*.md` + `16-impl-c.md`) but ONLY to find what claims to have been verified — then independently re-verify those claims against the actual code on disk. Treat every impl-log claim as something to confirm, not as ground truth.

---

## Artifact under review

**The Phase-C diff** = every commit on `main` from `409cce0` (pre-Phase-C, the docs-checkpoint immediately before W0 dispatched) through `2fc0b1e` (current `main` HEAD, the wave-3 integration merge). That covers seven workstreams + the wave-3 integration:

```
2fc0b1e wave-3 final integration — activate W4 renderer-side entity path
3c200a9 W2 — editing + flood-fill AADF invalidation
5f2cc92 W4 — dynamic entities + chunks Rg32Uint widening
48835b5 W3 — background AADF queue (boundsCalc.fx port)
53a4c8f W1 — GPU Algorithm 1 (chunkCalc + mapCopy + BlockHashingHandler)
912c984 W5 — world generator (generatorModel.fx port)
564a1f4 W6 — O(3·d·n) AADF rewrite (paper §3.3 neighbour-merge)
c10b6bd W0 — seam construction (empty extension surface)
```

Plus the post-merge docs commits between them (six `docs:` commits — informational, not subject to review).

**Code surfaces touched:**
- New WGSL: `crates/bevy_naadf/src/assets/shaders/{chunk_calc,bounds_calc,bounds_common,world_change,map_copy,generator_model,entity_update}.wgsl`.
- New Rust modules: `crates/bevy_naadf/src/render/construction/` (a sibling of `atmosphere.rs`/`gi.rs`/`taa.rs`), plus `aadf/{generator,edit,entity}.rs`.
- Edited: every place that reads the chunks 3D texture (the `R32Uint`→`Rg32Uint` widening + `.x` sweep); `render/mod.rs` (chain); `render/prepare.rs` (chunks texture descriptor); `render/pipelines.rs` (`world_layout` extension — wave-3 only); `render/gpu_types.rs` (new GPU structs); `world/data.rs` (`set_voxel`); `bin/e2e_render.rs` (new `--validate-gpu-construction`, `--entities`, `--edit-mode` modes); `lib.rs`, `main.rs`, `Cargo.toml` (plugin wiring).

**Canonical references** for your independent verification:
- The paper: `/mnt/archive4/PAPERS/Prepared/ulschmid-2026-naadf-voxel-gi.md` — especially Method §3.2–§3.6.
- NAADF C# / HLSL: `/mnt/archive4/DEV/NAADF/` — `Content/shaders/world/data/{chunkCalc.fx, boundsCalc.fx, boundsCommon.fxh, worldChange.fx, mapCopy.fx, entityUpdate.fx}`, `Content/shaders/world/generator/generatorModel.fx`, `Content/shaders/render/rayTracing.fxh` (entity sub-traversal), and `World/Data/{BlockHashingHandler, WorldBoundHandler, ChangeHandler, EditingHandler, EntityHandler, EntityData}.cs`, `World/Generator/WorldGenerator*.cs`, `World/Render/Versions/WorldRenderBase.cs`.

---

## Success criteria (from the Phase-C architectural Q&A E1–E4)

The reviewer's verdict is per-criterion. Each criterion below has a brief restatement of what it means, plus what you confirm/reject.

### C1. Canonical paper methodology §3.2–3.6 is implemented

The paper specifies, in order:
- **§3.2** — GPU hashing construction (Algorithm 1): the `chunkCalc.fx` 3-pass GPU build, the open-addressing hash table with `31^(64-i) mod 2^32` coefficients, occupancy-resize at 50%, 250-probe cap, GPU 64-thread groups.
- **§3.3** — AADFs: the 6-direction 5/2-bit empty-cuboid distance field per cell + the O(3·d·n) synchronised-iteration neighbour-merge construction algorithm + the per-layer background queues for "one queue per frame" recompute.
- **§3.4** — DDA traversal exploiting AADFs (already in Phases A/B before Phase C; not in scope to re-verify).
- **§3.5** — Editing: CPU world mirror + CPU→GPU sync + the flood-fill that resets AADFs in the 63³-chunk volume around an edit (in 4³-chunk groups), 7-round BFS, distance step 4.
- **§3.6** — Dynamic entities: per-chunk 32-bit entity pointer (`Rg32Uint` chunks), entity instance buffer, per-entity AADF voxel volumes, hash-dedup of chunk-entity-instances, traversal-time entity sub-traversal in `shoot_ray`.

Confirm each subsystem exists in the port code AND faithfully ports its C# counterpart. Spot-check the algorithm at the equation level: the 65 hash coefficients, the `wanted_empty_ratio = 0.5`, the 250-probe cap, the BFS distance step, the entity-pointer bit layout, the `compress_quaternion` smallest-three encoding, the per-axis `bound_group_masks` atomic, the indirect-dispatch split (`bound_dispatch_indirect` on its own layout).

**Out of scope per the Q&A:** the SVGF alternative denoiser (`14-paper-gap.md` item #8) is excluded — un-portable from NAADF source.

### C2. The CPU `aadf/construct.rs` 3-phase build is kept as a bit-exact validation oracle + fallback (E4)

Confirm `aadf/construct.rs` is NOT deleted. Confirm the W1 dispatch's `--validate-gpu-construction` e2e flag actually maps the GPU `blocks` / `voxels` / chunks texture back and byte-compares to `construct::construct` output. Confirm there is a `cargo test` that runs both paths and asserts byte-equality on at least one deterministic scene.

Note: W1's impl log states the bit-exact comparison was scaled down to a 1×1×1 single-voxel scene because on the full grid the CPU `HashMap` iteration order vs the GPU open-addressing-by-hash assigns *different* `VoxelPtr` values (block *contents* still agree). Confirm this is documented honestly and that the consumer-level (W2, W3) tests verify semantic correctness on the full grid through their own oracle compares (not requiring byte equality at the pointer-numbering level).

### C3. Faithful-port principle holds for every Phase-C subsystem

Confirm: every Phase-C subsystem is grounded line-by-line in `/mnt/archive4/DEV/NAADF/` (the C#/HLSL sources). Documented deviations are acceptable; novel inventions are not.

**Deviations the implementers explicitly documented and which you DO NOT need to re-litigate** (already user-approved deviations):
- Bevy `TonyMcMapface` tonemapping replaces NAADF's custom Reinhard tonemap (port emits raw linear HDR; the post-Phase-B TAA-fidelity track established this — user-directed deviation).
- TAA sample-ring depth configurable, default 32 (user-directed deviation, supersedes `design-exploration-qa.md` §6).
- The wgpu `STORAGE_READ_WRITE` × read-only layout split (a parallel construction-mode `@group(0)`).
- The wgpu `STORAGE_READ_WRITE` × `INDIRECT` exclusivity split for `bound_dispatch_indirect`.
- The `vec3`-then-scalar WGSL alignment fix via `vec4` rows + `offset_of!` guards (the hazard bit the port 3× in Phase B; this is the hardened pattern).
- The entity-track chunks `R32Uint` → `Rg32Uint` widening + `.x` sweep on every chunks read site.

**Deviations the W6 implementer surfaced** (re-verify independently):
- The O(3·d·n) neighbour-merge algorithm produces *different (both valid)* empty cuboids than the old per-cell slice-empty algorithm in the general case. The CPU oracle now implements the merge algorithm (matching GPU `ComputeBounds4`). Verify the new oracle is what `bounds.rs::compute_aadf_layer` ships and that `bounds.rs::compute_aadf` (the legacy per-cell function) either still exists as a test reference OR was removed cleanly.

**Deviations the W3 implementer surfaced** (re-verify):
- The CPU oracle for W3's bounds-queue convergence is a fresh CPU port of `boundsCalc.fx`'s algorithm (not W6's `compute_aadf_layer`), because of a "chunk-world-edge OOB-permissive divergence" the W6 impl log flagged. Verify this rationale by reading both algorithms.

### C4. All verification gates green

Run yourself (do not trust the impl logs):
- `cargo build -p bevy-naadf` — clean, no warnings on Phase-C files.
- `cargo test -p bevy-naadf --lib` — all 109 lib tests pass. No regressions.
- `cargo run --bin e2e_render` — baseline e2e exits 0; region luminances stay in their existing bands.
- `cargo run --bin e2e_render -- --validate-gpu-construction` — exits 0; oracle compare passes.
- `cargo run --bin e2e_render -- --edit-mode` — exits 0; the edit gate passes (a scripted `set_voxel` produces the expected changed records).
- `cargo run --bin e2e_render -- --entities` — exits 0; the entity dispatch fires every frame; the entity is functionally rendering.
- Cap e2e runs ≤6 (you don't need to re-run every gate multiple times; just one run per mode confirms).

The **`offset_of!` guard pattern** applies to every new GPU struct. Confirm by grepping the codebase: every Phase-C `#[repr(C)]` GPU struct should have a `const _: () = assert!(size_of == N)` + per-field offset guards on hazardous rows.

### C5. The seam-first design held — parallel workstreams did not collide outside the predicted integration points

Confirm the wave-1 + wave-2 + wave-3 sequencing actually delivered isolated workstreams. The predicted integration-collision points were:
- `render/construction/mod.rs` (the seam's `ConstructionGpu` / `ConstructionPipelines` / `ConstructionBindGroups`).
- `render/gpu_types.rs` (additive GPU-struct registry).
- `render/mod.rs` (the `Core3d` chain `.chain()` tuple).

Confirm conflicts occurred only at those files. The W4 rebase needed an additive resolution at exactly those files (per the impl log). The W2 cross-cutting fix (W3's stale `R32Uint` storage-texture decl + zeroed `.y` channel) is a separate integration finding — confirm W2 actually fixed both issues in the W2 commit (`3c200a9`).

### C6. The user-directed scope is fully delivered

The user's verbatim ask (2026-05-15):
> "next order of importance is GPU build Algorithm and complete canonical NAADF+GI methodology as per the original paper [...] lets work with teams instead of just local agents on this one [...] if we can parralelise work, its trivial with worktrees on a rust codebase"

Plus the Q&A answers (E1: all 4 paper contributions; E2: fix B-1 first; E3: seam-first; E4: keep CPU oracle).

Confirm: the GPU build algorithm IS implemented and IS the default producer. The canonical methodology §3.2–3.6 IS implemented. SVGF (#8) is OUT (correct per E1). Parallel workstreams via git worktrees DID happen as specified. The B-1 TAA-fidelity fix landed (pre-Phase-C, commit `8995c88`) — confirm it is still in place.

### C7. Known honest residuals are correctly bounded

The wave-3 impl log flags one residual: `--entities` mode's per-pixel visible-entity luminance gate is not calibrated. The dispatch fires and the entity pixel-data changes vs baseline, but there is no dedicated gate region asserting a specific luminance threshold at the entity's expected screen position.

Confirm whether this is:
- **Acceptable** (functional rendering verified by frame-diff; gate calibration is a polish task, not a correctness gap), or
- **A blocker** (without the gate the e2e harness will not regress-trap a future bug that breaks entity rendering).

Your verdict here gates whether Phase C is "complete" or "complete-with-followups."

---

## Deliverable shape

Write your review findings to `docs/orchestrate/naadf-bevy-port/17-review-c.md` (append to this file; preserve the header above; add your section at the bottom). Structure:

```markdown
## delegate-reviewer findings (<ISO date>)

### Per-criterion verification

| # | Criterion | Verdict (PASS / PASS-WITH-FOLLOWUP / FAIL) | Notes (one sentence) |
|---|---|---|---|
| C1 | Paper §3.2–3.6 implemented | … | … |
| C2 | CPU oracle preserved | … | … |
| C3 | Faithful-port principle | … | … |
| C4 | All gates green | … | … |
| C5 | Seam-first held | … | … |
| C6 | User-directed scope delivered | … | … |
| C7 | Residuals correctly bounded | … | … |

### Findings

Use the Phase-B review precedent: classify each finding as **blocker** / **concern** / **nit**.

- **Blockers** — gate the verdict FAIL. The Phase-C deliverable cannot be considered complete until these are fixed.
- **Concerns** — surface real risks the implementers may have missed; do not block PASS but warrant a fast-followup.
- **Nits** — polish-level items.

Be specific: each finding cites a file and line range, restates the actual code behaviour, and contrasts with what the NAADF C# does (or what the paper specifies). Surface anything that looks like an assumption an implementer silently baked in.

### Verdict

One of: **PASS** (all 7 criteria green, no blockers); **PASS-WITH-FOLLOWUPS** (≤2 minor blockers or several concerns; landed work is correct and complete; followups can be done in a separate dispatch); **FAIL** (≥1 substantial blocker — the artifact is not Phase-C-complete).

### Recommended follow-up scope (only on PASS-WITH-FOLLOWUPS)

A short numbered list of follow-up tasks. Each: file path, the change, scope estimate (small/medium/large).
```

---

## Hard rules for the reviewer

- You do NOT read `01-context.md`, `15-design-c.md`, or `design-exploration-qa.md`. Withholding the design rationale is the entire point — your fresh eyes catch what the implementers' eyes missed.
- You MAY read the `16-impl-c*.md` files, the paper, the NAADF C# / HLSL, and the actual code on disk. You MUST independently verify any claim from the impl logs against the code.
- Run the verification gates yourself; do not trust impl-log numbers.
- Be honest about residuals: surface anything that looks unfinished even if the implementer marked it "out of scope."
- Faithful-port is the bar for everything except the explicitly user-approved deviations enumerated under C3.
- Your output is appended to this file as `## delegate-reviewer findings (<ISO date>)` — write it there before returning.
