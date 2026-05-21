# codebase-tightening — orchestration index

**Goal**: tighten bevy-naadf — IoC + idiom-fit first, LOC reduction as consequence — by parallel domain-scoped exploration + architecture, then sequential implementor dispatches.

**Mode**: distributed with parallel read-only fan-out for analytics (rule 8), strictly sequential for code-mutating impl.

**Date opened**: 2026-05-20.

## Files

- `00-reuse-audit.md` — auditor output (LOC comparison, domain decomposition, crosscutting reuse map). Authoritative for scope.
- `01-context.md` — canonical context bundle every non-review agent reads first.
- `<domain>/02-exploration.md` — per-domain `refactor-explorer` output (orchestrator does NOT read).
- `<domain>/03-architecture.md` — per-domain `refactor-architect` output (orchestrator does NOT read).
- `<domain>/04-refactoring.md` — per-domain `refactor-implementer` execution log.

## Agent groups

| group | agents | model | concurrency |
|---|---|---|---|
| audit | `delegate-auditor` ×1 | inherited (Opus) | n/a — done |
| analytics-explore | `refactor-explorer` ×8 (one per domain) | inherited (Opus) | **parallel batch** |
| analytics-architect | `refactor-architect` ×8 (one per domain) | inherited (Opus) | **parallel batch** (after all explorers done) |
| impl | `refactor-implementer` ×N (sequenced) | inherited (Opus) | **strictly sequential** |
| checkpoint | `general-purpose` commit agent | sonnet | before each substantive dispatch |

## Domain list (audit §2)

| # | slug | LOC | dir |
|---|---|---|---|
| D1 | `aadf-data-structures` | 6 470 | `aadf-data-structures/` |
| D2 | `editor-and-settings-ui` | 3 120 | `editor-and-settings-ui/` |
| D3 | `voxel-io-and-grid` | 5 790 | `voxel-io-and-grid/` |
| D4 | `render-pipeline` | 13 665 | `render-pipeline/` |
| D5 | `gpu-construction` | 18 405 | `gpu-construction/` |
| D6 | `e2e-and-playwright` | 12 725 | `e2e-and-playwright/` |
| D7 | `app-and-camera` | 2 396 | `app-and-camera/` |
| D8 | `asset-pipeline` | 1 161 | `asset-pipeline/` |

## Impl phase order (user-decided, Q&A)

1. **D5** — `gpu-construction` (biggest single win; splitting `render/construction/mod.rs` 11 043 → ~2.5k core + extracted subdirs).
2. **D4** — `render-pipeline` (lands onto a cleaned-up construction-side surface).
3. **D1, D2, D3, D6, D8** — interleave (architect docs land first; orchestrator picks order from there).
4. **D7** — `app-and-camera` last (touches all other domains' `Plugin`s).

## Phase checklist

- [x] `00` — audit
- [x] `01` — context bundle (incl. 2026-05-20 addendum after explorer hard gate)
- [x] `02` — explorers (D1..D8, parallel batch) — all 8 returned with prioritised findings
- [x] `03` — architects (D1..D8, parallel batch) — all 8 returned; cross-architect conflicts triaged (D5 merge wins per Resolution D; D7 pre-lands `GiSettings::DEFAULT` scout commit; D7's C1-C6 deferred to D7 impl; D8 bake.rs in-place edit)
- [ ] `04a` — D7 scout: 3-line `pub const GiSettings::DEFAULT` + `#[derive(PartialEq)]` pre-land (before D2 impl)
- [⚠] `04` — implementor D5 — **4/8 steps landed** (escape-hatch subset: probe delete + readback extract + extract/producer extract + e2e fixtures moved to `validation.rs`). mod.rs 11 043 → 2 280 (−8 763 LOC, 79%). Net D5 Rust −1 246. All deterministic gates green; oasis-edit-visual × 4 stable. Steps 4/6/7/8 deferred for follow-up.
- [⚠] `04` — implementor D4 — **2/6 steps landed**: SSoT scaffolding + dead `MAX_RAY_STEPS_*` deletion + sample-refine 4→1 collapse (C# fidelity restored). Net −39 LOC. ShaderType cutover bailed out per safety rule (recipe in §5). Steps 3/4/5/6 gated on Resolution D shape + D6/D7 pbr_sampling reference-drops.
- [⚠] `04` — implementor D1 — **7/7 steps landed** (Step 8 by design = cross-domain skip). Net −172 LOC; shortfall vs architect's −400 to −500 = deliberate shim retention for `WorldData::set_voxel`/`set_voxels_batch_oracle` so D2/D5 can drop them in their phases (~390 LOC recoverable). State-bit regime A→B migration bit-pattern-identical; oasis-edit-visual ×3 stable.
- [⚠] `04` — implementor D3 — **F1+F2..F7+F9 landed, F8 deferred** (architect's call). Two-dispatch run: first dispatch died with API 529 mid-flight after F1 (voxel_noise crate deletion, −1547 LOC, commit 293ffa8); re-dispatched for F2-F9 (−210 LOC inside `crates/bevy_naadf/src/`, new `camera/poses.rs` arrow-reversal, net D3 Rust ~−1757 across both runs). Build ✓ / 186 lib tests pass / e2e: baseline + --vox-e2e + --oasis-edit-visual all green. Note: 187→186 test count delta — tiled-family test removed with F2 deletion (expected). User-confirmed: CPU `voxel_noise` retires in favour of upcoming GPU compute noise.
- [⚠] `04` — implementor D6 — **2/5 steps landed** (Step 1 diag_compare delete + Step 2 pbr_* e2e module deletes + new `gate.rs` scaffolding). Net −2346 LOC. Build ✓ / 186 lib tests pass / e2e: oasis-edit-visual ×2 + vox-gpu-construction + baseline all green. Steps 3-5 (gate trait migration + driver decomposition + CLI ladder refactor) deferred to follow-up dispatch (~24 verification cycles, scope too large for single dispatch).
- [x] `04` — implementor D8 — **5/5 steps landed** (Q&A Option A: runtime consumers deleted, InstaMAT bake binary path preserved per `instamat-bake-to-disk` memory). Net −1209 LOC. Build ✓ / 180 lib tests pass (186→180, asset-runtime tests removed with consumers) / e2e: --validate-gpu-construction + baseline + bake bin all green.
- [x] `04a` — D7 scout: `pub const GiSettings::DEFAULTS` + `#[derive(PartialEq)]` pre-land at `lib.rs:109` (derive) + `lib.rs:188-214` (impl). Build + 180 lib tests pass. (Note: architect used `DEFAULTS` plural, not `DEFAULT` singular as I'd briefed — D2 brief must reference `DEFAULTS`.)
- [⚠] `04` — implementor D2 — **6/6 steps landed**, modest net delta. Net −20 LOC (reorganization-heavy: plugin extraction into `AppModePlugin`/`EditorPlugin`/`SettingsPlugin`; deletions deferred to D7 wiring). Build ✓ / editor 13 + settings 7 + tools 10 targeted tests pass. 3 open conflicts all expected D7 work: (1) `impl Default` for `GiSettings` still hand-rolls 6 caps in `lib.rs:217-260` instead of consulting the new `DEFAULTS` const — D7 final closes; (2) UA-1 incomplete: D1 kept `set_voxels_batch` / `set_chunks_uniform_batch` tuple signatures despite the named-type intro — closure deferred; (3) `EditorPlugin`/`SettingsPlugin`/`AppModePlugin` are dead until D7 swaps inline registration in `lib.rs:924-1001` to `app.add_plugins((AppModePlugin, EditorPlugin, SettingsPlugin))`. **Environmental note**: `small_edit_repro` e2e gate BLOCKED by host NVIDIA Vulkan driver — same panic in 19 unrelated GPU lib tests (not D2-caused). User-side action item, not orchestration scope.
- [⚠] `04` — implementor D7 — **7/9 steps landed** (Steps 1, 2-partial, 3, 4, 5, 6, 9; Step 7 D2-portion done, D3/D5-portions cross-domain skip; Step 8 deferred per architect). **All 3 D2 conflicts CLOSED** ✓ (GiSettings impl Default → consults DEFAULTS const; UA-1 named-type signatures flipped; plugin wiring `add_plugins((AppModePlugin, EditorPlugin, SettingsPlugin))` landed). Net −519 LOC (lib.rs alone −548). 5 new extracted modules: `app_args.rs`, `app_config.rs`, `dev_font.rs`, `window_config.rs`, `world_size.rs`. Build ✓ / 137 lib tests pass (43-test drop vs prior 180 = pre-existing host NVIDIA Vulkan driver block on GPU lib tests; environmental, not D7-caused) / All 7 e2e gates green: baseline, --validate-gpu-construction, --edit-mode, --runtime-edit-mode, --entities, --vox-e2e, --oasis-edit-visual ×2. Open conflicts for follow-up: D3 `VoxelIoPlugin` extraction never landed; D5 `spawn_phase_c_test_entity` rehome not done; D6 `vox_horizon_parity.spec.ts` has 4 silently-no-op `[device-snapshot]` console-reads; `GiSettings` file relocation to `settings/canonical.rs` deferred.
- [⚠] `04F` — D5 follow-up dispatch — **Step 7 + Step 8 + SSoT-6 + Step 6 (D5-owned portion) landed; Step 4 DEFERRED 2× — needs architect re-design**. Both follow-up implementors hit the same 5 cross-workstream coupling gaps in architect's §2.1 (notably `want_gpu_producer` derivation + W1-placeholder ownership migration from W2). Either re-architect Step 4 or accept simpler "move-only-no-split" deviation. Step 6 D4-portion (render-graph node `.run_if`) belongs to D4 follow-up. Build ✓ / 179 lib tests pass / 8 e2e gates green incl oasis-edit-visual ×2 (Δlum 14.7 / 15.4).
- [⚠] `04F` — D4 follow-up + Resolution D — **Resolution D landed (flat absorption: `ConstructionPipelines` fields merged into `NaadfPipelines`); Step 6 WorldGpu consolidation landed; pbr_sampling.wgsl deleted (−868 LOC alone)**. Net −893 LOC. Build ✓ / 179 lib tests pass / 6 e2e gates green incl oasis-edit-visual ×4. **Steps 3/4/5 deferred**: Step 3 ShaderType cutover hit architect §3.4 recipe error on GpuGiParams std140 padding (non-natural alignment), needs architect doc revision before retry; Steps 4 (prepare.rs split) + 5 (plugin-per-subsystem) bailed per dispatch budget. Cosmetic: rename `construction_pipelines:` → `pipelines:` at 5 callers + drop `pub type` alias.
- [ ] `04F` — D5↔D1 SSoT-6 follow-up: re-export `hash_coefficients` in `render/construction/hashing.rs` (~5 LOC win)
- [x] `04F` — Resolution D `NaadfPipelines` merge shape — **flat absorption landed** in D4 follow-up dispatch ✓
- [⚠] `04F` — D6 follow-up dispatch — **Step 5 landed (CLI ladder refactor, e2e_render.rs 462→523 LOC structural); Steps 3+4 deferred — analytical, not bandwidth**. Implementor identified that gate.rs trait's `apply_edit` signature lacks per-gate State resources each gate's apply phase mutates; landing Step 3 additively without Step 4 produces ~600 LOC of dead trait impls. Recommend atomic Step 3+4 dispatch with the 8-gate verification matrix. Build ✓ / 179 lib tests pass / 5 e2e gates green incl oasis-edit-visual ×2 (Δlum 18.09 / 17.99).
- [x] `04F` — D7 cleanup follow-ups — **all 4 landed** ✓. lib.rs 598→365 LOC (−233 alone); new `voxel/plugin.rs` (VoxelIoPlugin), `settings/canonical.rs` (GiSettings relocation), `render/construction/test_fixture.rs` (extracted), `e2e/tests/vox-horizon-parity.spec.ts` cleaned. Net +83 LOC (module docstring cost). Build ✓ / 137 lib tests pass / 7 e2e gates green incl oasis-edit-visual ×2. New open: minor production→e2e dep arrow `crate::e2e::gates::demo_origin_v()` imported by `render::construction::test_fixture` (deferred).

## Orchestrator discipline

- Per user directive: **orchestrator does NOT read `<domain>/02-exploration.md` or `<domain>/03-architecture.md`**. The implementor agents read those directly. The orchestrator's only direct reads are `00-reuse-audit.md` (done), `01-context.md` (this), agent return status lines, and the impl log for verification confirmation.
- Per `feedback-vigilance-preamble-for-cg-work`: every brief opens with "This is a significant task in computer graphics — be vigilant; verify every file:line ref with Read/Grep before citing".
- Per `feedback-multiple-runs-rule-out-false-positives`: impl agents must re-run e2e gates ≥2× on non-deterministic gates (oasis_edit_visual, vox_gpu_oracle).
- Per `bevy-naadf-faithful-port-rule`: no behavioural divergence from C# NAADF without explicit user approval + docs entry.
