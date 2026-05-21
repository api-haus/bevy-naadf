# Orchestration — configuration-as-resource refactor

## Topic
Decompose `AppArgs` from a runtime-read god-resource into per-domain Bevy resources, following the user's stated principle: *"args insert resources, app consumes resources. The concept of args only makes sense during application bootstrap; any application code outside of bootstrap domain must not read the configuration conceptualising anything as 'args' — this is just bad software design."*

## Mode
**Consolidated, Research → Architect shape.** One 1M-context Opus agent runs investigation → diagnosis → design → migration plan → verification surface in a single uninterrupted trace. Design-only — NO code lands this orchestration; implementation is a downstream orchestration the user scopes after approving this design.

## Files

- `00-reuse-audit.md` — auditor's enumeration of existing precedent resources, extract patterns, CLI parsers, AppArgs shape tallies, and borderline calls requiring design decisions. **Status: ✓ written.**
- `01-context.md` — canonical context bundle for the consolidated agent (handoff verbatim + Q&A decisions + required-reading map + the parameter / mode / action-verb taxonomy). **Status: ✓ written.**
- `02-design.md` — the consolidated agent's deliverable: investigation findings + diagnosis + proposed design + migration plan + verification surface. **Status: ✓ written.**
- `03-e2e-as-tests-investigation.md` — read-only investigator's findings on whether e2e gates can move to `tests/<gate>.rs` integration tests. 22 entries analysed, 3 options enumerated. **Status: ✓ written.**
- `04-followup-ipc-rpc-direction.md` — captured follow-up direction: the user proposed restructuring e2e as IPC-RPC-controlled app-as-SUT (subprocess + RPC schema) as a long-term cleaner alternative. Viability investigation deferred to a separate orchestration. **Status: ✓ written (direction-capture only, not designed).**
- `05-impl.md` — implementation log, one section per migration step. **Status: Steps 1-5 + 8 logged.**

## Design-phase checklist (the /delegate orchestration that produced `02-design.md`)

- [x] Step 1 — Restate and scope
- [x] Step 2 — Re-implementation audit (delegate-auditor → `00-reuse-audit.md`)
- [x] Step 2.5 — Select execution mode (consolidated, Research → Architect)
- [x] Step 3 — Present method to user
- [x] Step 4 — Architectural Q&A (4 questions answered)
- [x] Step 5 — Write shared-context files (`README.md` + `01-context.md`)
- [x] Step 6 — Dispatch consolidated agent (Research → Architect) → `02-design.md` written
- [x] Step 7 — User review of design + e2e-as-tests viability investigation → `03-e2e-as-tests-investigation.md`; IPC-RPC direction surfaced and captured as `04-followup-ipc-rpc-direction.md`
- [x] Step 8 — Design approved (Option A); e2e harness changes deferred to follow-up orchestrations.

## Implementation progress (the 9-step migration from `02-design.md` §4)

- [x] Step 1 — Introduce `BootstrapInputs` scaffold — commit `a3824ea`
- [x] Step 2 — Migrate `taa_ring_depth` → `TaaRingConfig` + `RenderTaaRingConfig` — commit `4fa1441` (Android on-device verified: `[budget] … taa_ring_depth = 8`)
- [x] Step 3 — Migrate `taa` + `gi` to per-domain resources — commit `9b8347f`
- [x] Step 4 — Migrate `construction_config` + relocate wasm32 divergence to `for_target_arch()` — commit `d89b603`
- [x] Step 5 — Migrate `grid_preset` + relocate `?skybox=1` to wasm bootstrap — commits `7efce79` (partial checkpoint) + `e7a2a4d` (completion)
- [x] Step 6 — Collapse 10 e2e-mode booleans → `E2eGateMode` enum (`GateKind` promoted); `e2e_driver` config reads grouped into the `E2eDriverConfig` `#[derive(SystemParam)]` struct
- [x] Step 7 — Extract `vox_e2e_mode` → `VoxE2eAssertion(bool)`; `AppArgs` drained to a zero-field shell; driver read folded into `E2eDriverConfig`
- [x] Step 8 — Extract `spawn_test_entity` → `SpawnTestEntity(bool)` — commit `53c37b3`
- [x] Step 9 — Delete the now-empty `AppArgs` shell; `app_args.rs` deleted, `build_app_with_args` → `build_app_core` (no `AppArgs` param), every workspace `AppArgs` reference resolved

**Migration complete.** All 9 steps landed. `AppArgs` no longer exists; `crates/bevy_naadf/src/app_args.rs` is deleted. Every configuration value is a per-domain Bevy `Resource` inserted at bootstrap via the transient `BootstrapInputs` carrier — no runtime code reads an `args`-conceptualised configuration object.

Desktop verification at the Step 9 commit: `cargo build --workspace` clean (this PASSING proves zero surviving `AppArgs` references), `cargo test --workspace --lib` 192 passed / 1 ignored, full e2e gate sweep (`baseline`, `--vox-e2e`, `--oasis-edit-visual`, `--small-edit-visual`, `--small-edit-repro`, `--vox-gpu-construction`, `--vox-gpu-oracle-cpu/-gpu`, `--vox-gpu-oracle`, `--vox-horizon-native`, `--vox-web-parity-skybox/-loaded`, `--vox-web-parity`, `--entities`, `--resize-test`) — all 15 PASS. Wasm32 visual checks (Steps 4 + 5) are the user's surface.
