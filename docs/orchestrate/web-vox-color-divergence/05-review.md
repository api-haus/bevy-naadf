# 05-review — web-vox-color-divergence (fresh-eyes review brief)

You are a fresh-eyes reviewer dispatched by an orchestration. You have
been **deliberately denied** the design rationale, the audit trace, and
the research findings. That is the point: a reviewer who shares the
implementer's context rubber-stamps the implementer's assumptions.

Read **only** this file. Do **not** read `01-context.md`,
`00-reuse-audit.md`, `02-research.md`, `03-design.md`, or
`04-impl.md`. If your tools won't let you avoid them — flag this in
your output and stop. The orchestrator reconciles your flags against
full context at synthesis.

---

## Artifact under review

A code change made on the worktree branch
`feat/web-vox-streaming` at
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming`.

The change addresses a **web-only voxel rendering bug**: on the web
build, `.vox` voxels load with correct geometry and types but render
near-black; the native build renders the same fixture with full colors.

To see what the change does, read the **diff between the
implementation commit and its parent**. Find the implementation commit
with:

```
git -C /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming log --oneline -10 feat/web-vox-streaming
```

The implementation commit is the most recent non-checkpoint commit
authored under this orchestration; commit subjects are
conventional-commit style (`feat:`, `fix:`, `refactor:`,
`checkpoint:`). Inspect the diff with:

```
git -C /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming diff <commit>^..<commit>
```

You may Read any source file the diff touches. You may NOT Read the
`docs/orchestrate/web-vox-color-divergence/` directory (apart from this
file). Use Grep / Read on the actual code.

---

## Success criteria

Verify each. Cite file:line for every yes/no.

### Bug-fix correctness

1. **Web build no longer renders voxels near-black.** Verify by reading
   the implementation log path
   `docs/orchestrate/web-vox-color-divergence/04-impl.md` *only for the
   verification-results section*, which must include a colorful web
   capture as evidence. (You may read that one file *for the
   verification section alone* — not the design rationale or
   step-by-step changelog.) If `04-impl.md`'s verification section
   doesn't claim a colorful web capture, flag it as inconclusive.
2. **Native build still renders the Oasis fixture with full colors.** The
   `--vox-web-parity`, `--vox-e2e`, `--oasis-edit-visual`, and
   `--vox-gpu-oracle` gates must all be passing per the impl log.
3. **`cargo test --workspace --lib` still passes** (184 tests was the
   baseline at orchestration start). The impl log must confirm.

### Code-change quality (read the diff yourself)

4. **No `#[cfg(target_arch = "wasm32")]` branches added in the render
   path.** The fix must not special-case the renderer for web.
5. **No new sync `.vox` path on web.** The async-loading deliverable
   must remain intact.
6. **No `--no-verify` on any commit.** Inspect `git log` commits in this
   orchestration's range; none should reference `--no-verify`.
7. **No mock-GPU added to any test.** Real wgpu/WebGPU pipelines only.
8. **No reverts of async-loading work** (commits `1ac6f0b6` →
   `4e54c7a7` → `7dc739a` → `162c40b8`). Verify by running
   `git -C <worktree> log --oneline <commit>..HEAD` on each.
9. **Performance plausibility check.** If the fix re-extracts /
   re-prepares world GPU data on a `Changed<T>` trigger, confirm by
   reading the diff that the re-fire is **not unconditional every
   frame** (would silently regress frame time). If it removes a
   `WorldGpu` resource at install time, confirm there's no
   double-removal / orphan-bind-group hazard. If it suppresses the
   default scene during pending vox, confirm the suppression releases
   when the .vox lands or fails.

### Gate extension (Decision 4 from the orchestration's Q&A)

10. **`assert_vox_geometry_visible`** (search `crates/bevy_naadf/src/e2e/vox_e2e.rs`) — the assertion must include a per-channel color-spread check (max ≥ a numeric floor on the 0–255 scale, OR an equivalent per-channel-minimum metric). Luminance-only is insufficient.
11. **`vox_web_parity` loaded-phase assertion** (search `crates/bevy_naadf/src/e2e/vox_web_parity.rs`) — must include a per-channel color-spread assertion in addition to the SSIM compare on `vox_web_parity_loaded.png`.
12. **Both extended gates must have been demonstrated to fail on the pre-fix state.** The impl log must record a temporary revert + gate-fires-on-near-black verification. If not, flag it.

### Diagnostic instrumentation (Decision 2)

13. **Palette-upload diagnostic logging exists** in
    `crates/bevy_naadf/src/render/prepare.rs` (around the
    `voxel_types.upload_all(...)` site) AND in
    `crates/bevy_naadf/src/voxel/grid.rs`'s `install_imported_vox`. The
    logs must use a stable `[palette-...]` prefix and emit at `debug!`
    level (not `info!`) so they're off by default but available with
    `RUST_LOG=bevy_naadf=debug`. If the logs were removed after the fix
    landed instead of demoted, flag it.

### Scope discipline

14. **No out-of-scope work** has crept in. Specifically:
    - The `wasm-smoke.spec.ts` CORS-on-404 bug must NOT have been
      touched (it's a separate session's work).
    - No fresh-eyes reviewer dispatch of the prior
      `web-vox-async-loading` orchestration was run here.
15. **Single cohesive change.** The diff should be readable as one
    coherent fix + gate extension + diagnostic logging. Excessive
    refactoring of unrelated code is a flag.

---

## Deliverable

Write your review as a numbered checklist matching the items above. For
each item:

- **Verdict**: ✓ / ✗ / inconclusive.
- **Evidence**: file:line citation OR git command + commit hash.
- **Flag**: only if ✗ or inconclusive; one sentence on what's wrong /
  what to verify.

After the checklist, write a `## Overall recommendation` section: one
of "approve as-is", "approve with follow-ups (list)", "reject — see
items N, M". Do not approve work that lacks evidence for items 1, 2,
10, 11, or 12.

**Required last action:** use the `Write` or `Edit` tool to append your
review under the heading `## delegate-reviewer findings (<ISO date>)`
to this file
(`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming/docs/orchestrate/web-vox-color-divergence/05-review.md`)
before returning. The orchestrator reads from disk, not from your
return text.

Return only: file path + verdict count (e.g. "11✓ / 3✗ / 1 inconclusive")
+ the one or two most concerning ✗ items.
