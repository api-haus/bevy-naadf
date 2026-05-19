# 04 — Review brief (fresh-eyes)

> Reviewer: this is the ONLY orchestration file you read. Do NOT read
> `01-context.md`, `00-reuse-audit.md`, or `02-design.md` — the orchestrator
> deliberately withholds the design rationale so you can catch assumptions
> the implementer silently baked in. The orchestrator reconciles your flags
> against full context at the Step 7 synthesis.

## What you are reviewing

The artifact under review is the implementer's diff plus its log file:
- **Diff:** `git diff main..HEAD` in
  `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/pbr-raymarching`
  (branch `feat/pbr-raymarching`).
- **Implementer's log:** `docs/orchestrate/pbr-raymarching/03-impl.md` —
  read only the `## Implementation log` section to identify what was changed
  and which gates were run.

## Success criteria (criteria-only; no design rationale)

The implementer's diff must satisfy ALL of the following:

1. **Compiles workspace-wide.** `cargo build --workspace` from the worktree
   root returns exit 0 with no errors.

2. **Workspace tests pass.** `cargo test --workspace --lib` from the worktree
   root returns exit 0.

3. **Existing e2e gates still pass:**
   - `cargo run --bin e2e_render` (baseline / default Batch 6) — exit 0.
   - `cargo run --bin e2e_render -- --oasis-edit-visual` — exit 0.
   - `cargo run --bin e2e_render -- --small-edit-visual` — exit 0.
   - `cargo run --bin e2e_render -- --validate-gpu-construction` — exit 0.
   - `cargo run --bin e2e_render -- --vox-e2e` — exit 0.

4. **New PBR e2e gate exists and passes.** A new gate flag must exist in
   `bins/e2e_render.rs` (name TBD by implementer, e.g. `--pbr-visual`).
   The gate must:
   - Capture a framebuffer at a deterministic pose looking at a known PBR voxel.
   - Assert specular highlight luminance above a threshold within a known
     window of pixels.
   - Assert albedo texture variation across at least 16 sample points (not
     flat color).
   - For at least one metallic voxel in the scene, assert F0 ≈ sampled albedo
     (not 0.04).

5. **Baker bake succeeds.** `just bake-texarrays` returns exit 0. Output
   files produced under `imported_assets/Default/materials/`:
   - `diffuse.texarray.ron.basis` with 10 layers (3 existing + 7 new).
   - `normal.texarray.ron.basis` with 10 layers.
   - `mrh.texarray.ron.basis` with 10 layers.
   - `emissive.texarray.ron.basis` with 10 layers.

6. **`GpuVoxelType` is still 128 bits.** The struct is still `[u32; 4]` /
   `vec4<u32>` in WGSL. No buffer widening.

7. **Unified BRDF** — there is no longer a `MetallicRough` / `MetallicMirror`
   branch distinction in the shader hit-shading code path (`naadf_first_hit.wgsl`,
   `naadf_global_illum.wgsl`, `spatial_resampling.wgsl`). One PBR branch +
   one Emissive fast-path branch.

8. **Energy conservation in the unified BRDF.** Inspect the BRDF composition
   site: `kS = F` and `kD = (1 - F) * (1 - metallic)` (or equivalent
   energy-conserving form). Flag any divergence.

9. **Reflection rays still work.** Existing `shoot_ray` re-entry loops in
   `naadf_first_hit.wgsl:174–264` (mirror, now glossy) and
   `naadf_global_illum.wgsl:283–442` (GI) must still drive secondary rays.
   Verify by inspection that the rough-specular branch still calls
   `sample_vndf_isotropic` and continues to bounce.

10. **POM is sampled from MRH.B (height) at the voxel face.** Triplanar UVs
    are displaced by the height sample before albedo/normal/MR are fetched.
    Reasonable iteration count (e.g. 4–16 linear-search + 4–8 binary-search).
    The shader must not produce inverted POM (depressions look like bumps)
    on at least the visual check from criterion 4.

11. **Triplanar sampling** — albedo, normal, MR are all sampled triplanarly
    (3 planar projections weighted by face-normal). Verify by reading the
    relevant WGSL function and checking it does 3 `textureSample` per channel
    with normal-derived weights.

12. **`MaterialSet` (or equivalent linked-arrays bundle) exists.** A single
    Rust type bundles the 4 `Handle<Image>` (diffuse_ao, normal, mrh,
    emissive). The render plumbing extracts the 4 `TextureView`s from this
    bundle (or a system that registers them as a render world resource).

13. **All 7 new + 3 existing materials are present** as layer entries in all
    4 `.texarray.ron` definitions. The layer-index ordering is identical
    across the 4 arrays (material N is layer N in every array).

14. **No raymarcher binary smoke-test in the impl log** posing as
    verification — project rule. Live visual check is the user's job, not
    the implementer's.

15. **No `git stash` / `git checkout <file>` / `git restore` / `git reset`
    in any checkpoint commit traces.** The implementer must not use these
    to "clean up" before committing.

## Deliverable

Write your findings to `docs/orchestrate/pbr-raymarching/04-review.md` —
**append a new section at the end** (do not overwrite this brief). Use:

```
---

## delegate-reviewer findings (<ISO date>)

### Pass / fail / flag table
| # | Criterion | Pass/Fail/Flag | Evidence |

### Detailed flags
<For each flag: criterion #, what's wrong, exact file:line, severity {blocker / non-blocker / nit}.>

### Verdict
<one paragraph: ship / fix-and-reship / re-design.>
```

Return only a status confirmation (path + flag count + verdict word), nothing else.
