# Canonical context — configuration-as-resource refactor

Read this file in full before doing anything else. Everything the consolidated agent needs is inlined here or pointed at by absolute path.

---

## The user's stated principle (verbatim — load-bearing, do not paraphrase)

> the fact that args.taa_ring_depth is not a resource bothers me, lets follow this all up with configuration-as-resource refactor, reconsidering the shape of application configuration and structuring it all as resources
>
> args insert resources, app consumes resources. the concept of args only makes sense during the application bootstrap, any application code outside of bootstrap domain must not read the configuration conceptualising anything as "args" - this is just bad software design.

Design FROM this principle. Do not relitigate.

---

## The CLI-parser three-bucket taxonomy (load-bearing decision from the Q&A)

The user's Q&A answer to the cross-cutting-reader question re-framed the entire problem:

> "some args are not configuration parameters, but gates to dispatch various functions. this a job for CLI ARGS PARSER that separates parameters that set values from parameters that set modes from action verbage"

This is the architectural backbone of the design. Every field on the current `AppArgs` must be classified into exactly ONE of three buckets:

### Bucket A — **Parameter** (value the running app reads)
A configuration *value* the running app consumes at runtime. Becomes a per-domain Bevy `Resource`. Examples: TAA on/off, TAA ring depth, GI knobs, GPU construction config, grid preset.

The runtime app reads these via `Res<TheDomainResource>`. The bootstrap path constructs the resource value (from CLI / build constants / device probe) and inserts it. Mutable-at-runtime parameters (settings-panel knobs) use `ResMut<TheDomainResource>`.

### Bucket B — **Mode** (mutually-exclusive runtime branch selection)
A *mode selection* among finitely-many mutually-exclusive runtime branches. Becomes a single enum-shaped Bevy `Resource` whose value is the active branch.

Direct hit: the 11 e2e-mode booleans on the current `AppArgs` collapse into one `E2eGateMode` enum resource (the user's answer to Q2). The TODO at `crates/bevy_naadf/src/app_args.rs:7-9` already calls this out as the right shape; the half-done `GateKind` dispatch at `crates/bevy_naadf/src/bin/e2e_render.rs:111-122` is the dovetail point.

### Bucket C — **Action verb** (CLI verb that dispatches into a function)
A CLI *verb* whose job is to dispatch into a function / entry point. **Does NOT become a Bevy resource** — the parser uses it to choose WHICH entry point (or `run_*` builder) to call. The verb is consumed by the parser, not by the running app.

The architect must identify which `AppArgs` fields are actually action verbs in disguise (likely: `--vox <path>` is parameter+verb hybrid; each `--<mode>` flag is verb+mode hybrid; the e2e gate names are verbs that the parser routes into per-gate `run_*` builders). The action-verb bucket is where the existing three-layer parser shape in `bin/e2e_render.rs:134-157` already lives — the design extends that shape, not invents it.

---

## Q&A decisions (binding — design must satisfy all four)

### Q1 — Execution mode
**CHOSEN: Consolidated, Research → Architect.** One Opus agent does investigation → diagnosis → design → migration plan in a single 1M-context trace. (Recorded for posterity; this file is read by the consolidated agent, which already runs in this mode.)

### Q2 — E2e mode shape
**CHOSEN: Single `E2eGateMode` enum resource.** Collapse the 11 mutually-exclusive booleans into one enum. Pair the migration with the half-done `GateKind` dispatch refactor at `bin/e2e_render.rs:111-122` so they land together. Largest single migration step but biggest cleanup. Bucket B.

### Q3 — Web `?skybox=1` URL param
**CHOSEN: In scope.** Move the `?skybox=1` resolution from `voxel/web_vox.rs:390-405` (a `Startup` system that mutates `args.grid_preset`) into the wasm32 bootstrap in `crates/bevy_naadf/src/main.rs:73-90`. After refactor, `GridPreset` is inserted as a resource at bootstrap with the URL-derived value already baked in, and the Startup-time mutation is deleted. Bucket A.

### Q4 — Cross-cutting readers (re-framed)
**CHOSEN: Re-frame at the parser layer via the three-bucket taxonomy above.** Diagnostics and settings-panel readonly knobs only need to read **parameters** (Bucket A), not modes or action verbs. The fan-out from "1 god-Res to N per-domain Res" is real but bounded — the architect picks per-consumer between fanning out to N system params vs introducing a thin per-consumer aggregator. The audit listed both options; pick per cross-cutting consumer rather than committing globally.

---

## Required reading

In this order:

1. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/docs/orchestrate/config-as-resource-refactor/00-reuse-audit.md`** — the auditor's full enumeration of existing resources, extract patterns, parsers, and tally counts. Read in full; cite into the design as the "existing precedent" backbone.
2. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/app_args.rs`** — current `AppArgs` definition. 16 fields per the audit (collapsing the embedded structs to one slot each). The full field-by-field inventory is YOUR (architect's) job — this file is the source of truth.
3. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/render/budget.rs`** — the worked example for the cross-world resource pattern: const-defined canonical (`DEFAULT_TAA_RING_DEPTH`, `CANONICAL_INVALID_SAMPLE_STORAGE_COUNT`, etc.), main-world `Resource`, render-world mirror `Resource`, extract-driven copy. Reuse this pattern verbatim for every Bucket A field with a render-world consumer.
4. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/render/extract.rs`** — extract systems. Pay particular attention to `extract_taa_config` (lines 452-459), `extract_gi_config` (lines 511-520), `extract_invalid_sample_storage_count` (lines 470-477), `extract_effective_world_size` (lines 489-496). The first two are the smell shape (extract reads from `Res<AppArgs>` directly); the last two are the clean shape (extract reads from a per-domain main-world `Resource`).
5. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/lib.rs`** — `build_app`, `build_app_with_args`, `build_app_with_budget`. The bootstrap orchestration. The `build_app_with_budget` function already does the right pattern for budget-derived values: probe → `BudgetCaps` → decompose into per-domain resources. Extend this shape to cover the rest of `AppArgs`.
6. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/main.rs`** — native entry. Argv-scan for `--vox <path>` at lines 41-52. Calls `build_app_with_budget`.
7. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/android_main.rs`** — Android entry. Calls `build_app_with_budget`.
8. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/bin/e2e_render.rs`** — e2e binary. **Three-layer parser** at lines 134-157 (orchestrator), 168-185 (Layer 1: no-boot short-circuits), 256-324 (Layer 2: boot command), 447-454 (Layer 3: post-app validations). The `GateKind` dispatch at 111-122 is the half-done refactor that pairs with Q2's `E2eGateMode` collapse.
9. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/app_config.rs`** — clean precedent for a per-domain bootstrap resource that NEVER passes through `AppArgs`. Use as the structural template for new per-domain configs.
10. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/diagnostics.rs`** — cross-cutting consumer at lines 104-123 (reads `args.grid_preset / taa / taa_ring_depth / spawn_test_entity / gi / construction_config`). Per Q4, the architect picks fanout-vs-aggregator for this site.
11. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/settings/mod.rs`** — cross-cutting consumer at lines 138-140 (`KnobKind::Readonly { value: fn(&AppArgs) -> String }`) + the only `ResMut<AppArgs>` sites at lines 458 and 528 (settings-panel mutation of `args.gi`). The GI mutability is binding — `GiSettings` is a Bucket-A parameter with runtime mutability (use `ResMut<GiSettings>` post-refactor).
12. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/voxel/web_vox.rs`** — `startup_fetch_default_vox` at lines 390-405. The Startup-time `args.grid_preset` mutation that Q3 puts in scope.
13. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/render/construction/config.rs`** — `ConstructionConfig` (already a standalone resource) + `From<&AppArgs>` lift at lines 252-288, including the wasm32-specific `max_group_bound_dispatch` + `n_bounds_rounds` platform divergence at lines 265-288. After refactor, that platform divergence needs a new home (likely the bootstrap-side resource constructor on wasm32).
14. **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/render/taa.rs`** — `TaaRingConfig` (lines 46-50) + `CameraHistory` (lines 64-94). `TaaRingConfig` is the named-by-user `taa_ring_depth` field's existing per-domain resource; the smell is the plugin-build-time snapshot at `render/mod.rs:123-126`.

Auxiliary (read as needed during design):

- **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/render/mod.rs`** — render-sub-app plugin wiring (`init_resource` calls at 139-140, 146, 163-166; `add_systems(ExtractSchedule, …)` block at 172-191).
- All e2e gate `run_*` builders (~15 files under `crates/bevy_naadf/src/e2e/`) — each constructs `AppArgs::default()` + flips 1-3 e2e booleans + calls `run_e2e_render_with_args(app_args)`. After Q2 these collapse to constructing `E2eGateMode::TheGate` + calling the same entry point.
- **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/settings/canonical.rs`** — `GiSettings` definition (re-exported as `crate::GiSettings`).
- **`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build/crates/bevy_naadf/src/window_config.rs`** — `WindowConfig` and `window_for_e2e_args` dispatcher (lines 153-163). The dispatcher reads `AppArgs` booleans to pick the e2e mode; after Q2 it reads `Res<E2eGateMode>`.

---

## Borderline calls the audit surfaced (you decide; design must address each)

1. **`args.gi: GiSettings` is runtime-mutated by the settings panel.** Not bootstrap-only. Resolution per Q&A: `GiSettings` becomes its own `Resource`; settings panel takes `ResMut<GiSettings>`; extract reads `Res<GiSettings>`. Bucket A with runtime mutability.

2. **`args.grid_preset` is mutated at Startup by `web_vox::startup_fetch_default_vox`.** Q3 says in scope — move `?skybox=1` resolution into the wasm32 bootstrap.

3. **`args.taa_ring_depth` is the user's named smell.** `TaaRingConfig` already exists as a render-sub-app resource (`render/taa.rs:46-50`) but is plumbed via plugin-build-time snapshot. Refactor: bring `TaaRingConfig` into the main world, default = `DEFAULT_TAA_RING_DEPTH`, budget override path inserts via `init_resource` then mutates, render world reads via a new extract (mirror `extract_effective_world_size`).

4. **`args.construction_config` already has its own resource (`render/construction/config.rs`).** The `From<&AppArgs>` lift goes away; bootstrap inserts `ConstructionConfig` directly. The wasm32 platform divergence (`max_group_bound_dispatch` + `n_bounds_rounds` clamping at lines 265-288) needs a new home — likely a `ConstructionConfig::for_target_arch() -> Self` constructor or a wasm32-specific bootstrap branch.

5. **`args.spawn_test_entity` is dual-purpose** (Startup gate + e2e driver flag — `render/construction/mod.rs:1853`, `e2e/driver.rs:680`). Classify under Bucket A (parameter) or split into a separate Bucket B (mode) — your call. The audit suggests reframing as "if the Phase-C test-entity resource is present, spawn it" — that's a Bucket-A-but-Option<Resource> shape.

6. **The 11 e2e-mode booleans → one `E2eGateMode` enum** (Q2). Pair with the `GateKind` dispatch at `bin/e2e_render.rs:111-122`.

7. **Diagnostics + settings-readonly cross-cutting reads** (Q4 re-frame). Pick per consumer.

8. **No transient `BootstrapInputs` type exists today.** The architect likely introduces one (mirroring `BudgetCaps` from `render/budget.rs:261-281`) as the parser output that bootstrap fans into per-domain resources.

9. **17 `AppArgs::default()` direct construction sites** (1 in `lib.rs::build_app`, 1 in `main.rs`, 1 via `build_app_with_budget` from `android_main.rs`, 2 in `bin/e2e_render.rs`, 10 in e2e gate `run_*` builders, 1 in `settings::mod.rs` test only). All become "construct the relevant Bucket A/B values" calls in the refactored world.

---

## Forbidden moves

- **NEVER run `cargo run --bin bevy-naadf` as a verification step** — binding project rule per `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md`. Verification surface is `cargo build --workspace`, `cargo test --workspace --lib`, `cargo run --bin e2e_render -- <mode>`, plus on-device deploys for mobile-affected fields.
- **C# faithful-port rule.** No Bevy-only behaviour not in C# NAADF. Refactoring INTO C#-canonical structure is allowed; renaming away from C# semantics is NOT. The mobile-divergence resources (`EffectiveWorldSize`, `InvalidSampleStorageCount`) are explicitly approved divergences — their existence is fine; their shape may not change without re-approval.
- **Do not touch `crates/bevy_naadf/src/world_size.rs:46-54`** — the C# canonical pin test. The compile-time constants stay intact.
- **No amend commits.** Every state is a new commit.
- **Incremental migration mandatory.** The migration plan must move fields out one at a time (or in tight semantic groups), with verification gates green between commits. A big-bang rewrite loses bisectability.
- **The e2e_render binary uses `build_app_with_args` directly for canonical determinism.** Any new resource-decomposition entry point must produce byte-identical canonical defaults when reached via the e2e path. Existing e2e gates failing is a refactor regression, not a discovery.
- **Never `git checkout` to revert** (binding global rule). Use selective Edit from diffs.
- **No `git push`** — only the user pushes.
- **THIS ORCHESTRATION WRITES NO CODE.** The deliverable is the design document at `02-design.md`. Do not edit any source file. Do not run `cargo build` / `cargo test` / `cargo run`. The architect's job is to read and write the design — implementation is a downstream orchestration the user scopes separately.

---

## Vigilance preamble

This is a significant task in a complex graphics/rendering crate. Be vigilant: verify every file:line reference with Read/Grep before citing it. Opus 4.7 has produced incorrect source claims in this codebase before by pattern-matching off file names; the cure is to actually open the file and confirm the cited symbol exists at that line. The audit was vigilant — your design must be too.
