# mobile-budget — context bundle

This is the canonical context every non-review agent reads first.
Worktree root: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build`.
All file paths below are relative to that worktree unless otherwise stated.

## Restated goal (verbatim user words preserved)

> Design and ship a **startup-time GPU budget preselection routine** that reads `device.limits()` and picks safe sizes for the oversized storage-buffer bindings (`voxels`, `blocks`, `taa_samples` — see "What's broken" below; `taa_sample_accum` is NOT depth-scaled and does NOT exceed the cap) BEFORE the world install fires — so the full Naadf world runs on mobile (Android Mali / iOS Safari WebGPU), where `max_storage_buffer_binding_size = 256 MiB` is the universal hard cap.

User's tightening of scope (2026-05-21, mid-orchestration):

> so basically we want to determine optimal budget layout for world size, taa buffer frame size or off and perhaps something else that we have control over. world size and taa ring buffer size or no taa are the big ones.

→ **Two big levers in scope: world size + TAA ring depth (including "TAA off" = ring depth 0). Internal-resolution scale (lever #3) is deferred.**

## Device facts (Mali-G52 MP2 / Galaxy Tab A8, captured 2026-05-21)

| limit | value | note |
|---|---|---|
| `max_storage_buffer_binding_size` | **256 MiB** (268,435,456) | THE cap. WebGPU spec minimum. Same on iOS Safari WebGPU and confirmed for Adreno. **Universal mobile ceiling.** |
| `max_buffer_size` | 2 GiB (2,147,483,647) | A buffer can be huge — only the bound *slice* is capped. Sliding-window binding into a single big buffer is spec-valid (out of scope for this task). |
| `max_uniform_buffer_binding_size` | 64 KiB | |
| `max_bind_groups` | 4 | Spec floor. |
| `max_storage_buffers_per_shader_stage` | 35 | |
| Subgroup size | 8 (min == max) | `SUBGROUP` + `SUBGROUP_BARRIER` features present. |
| Total system RAM | 2.5 GiB unified | ~640 MiB free with stock Samsung firmware. |
| Empty-probe footprint (DefaultPlugins only) | 250 MiB PSS, 67 MiB GPU mtrack | Baseline overhead before any Naadf state. |

**Treat 256 MiB as the universal mobile per-binding ceiling.** Desktop reports 1-4 GiB; do not conflate the two paths.

**Headroom target: ≤ 75% × 256 MiB = 192 MiB per binding.**

## What's broken (current state in `crates/bevy_naadf/src/render/prepare/world.rs:320-346` + `render/taa.rs:476-505`)

**Correction (architect, 2026-05-21):** the original handoff (`docs/todo/android-build.md:41`) claimed four bindings exceed the cap. After verification at `crates/bevy_naadf/src/render/taa.rs:489-495`, only **three** bindings exceed it. `taa_sample_accum` is sized `pixel_count × 8 B` (NOT depth-scaled) → ~24 MiB at 3 MP, fits any mobile cap.

| buffer | current sizing | mobile cap (256 MiB) | over by |
|---|---|---|---|
| `voxels` | 1024 MiB (`chunk_count × 128 × 4`) | 256 MiB | 4× |
| `blocks` | 512 MiB (`chunk_count × 64 × 4`) | 256 MiB | 2× |
| `taa_samples` @ iPhone-like res | ~720 MiB (`pixels × 32 × 8`) | 256 MiB | ~2.8× |
| `taa_sample_accum` @ same | ~24 MiB (`pixels × 8`, NOT depth-scaled) | 256 MiB | **fits** |
| `chunks` | 2.1 MiB | 256 MiB | fits |

Mali OOMs the kernel and reboots the device on first launch. iOS Safari WebGPU refuses the binding outright.

## Locked design decisions (from Step 4 Q&A — 2026-05-21)

These are **decisions, not options.** The architect designs around them; do not re-litigate.

1. **Execution mode:** Distributed. Architect designs → hard user-approval gate → impl lands. Implementation is a separate downstream dispatch after the user OKs the design.
2. **World-size lever expression:** **Const + parallel runtime override.** Keep `pub const WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)` in `crates/bevy_naadf/src/world_size.rs:16` and the C#-canonical compile-time pin at `world_size.rs:46-54` intact. Introduce a runtime `Res<EffectiveWorldSize>` (exact name is the architect's call — `EffectiveWorldSize` is illustrative) that the 15 consumer sites read from instead of the `pub const`. On desktop: `EffectiveWorldSize == WORLD_SIZE_IN_SEGMENTS`. On mobile (budget routine decides): diverges to e.g. `(8, 2, 8)` or `(4, 2, 4)`. The faithful-port C# invariant lives in the const and its test; the mobile divergence lives in the resource.
3. **Lever #3 (internal-resolution scale):** Deferred. Architect must NOT design it in. If post-fix the FPS gauge says we still need it, that's a follow-up task. Architect MAY note its absence in side-notes if relevant.
4. **`device.limits()` read site:** Pre-`build_app_with_args` probe-app pattern. Mirror `validate_gpu_construction_production_scale` (`crates/bevy_naadf/src/render/construction/validation.rs:1037-1046`) + the headless `RenderDevice` extraction technique at `crates/bevy_naadf/src/world/buffer.rs:246-264`. Spin up a throwaway `App` with `DefaultPlugins` + `Plugin::ready` + `Plugin::finish`, extract `RenderDevice`, call `.limits()`, drop the probe app, then build the real `App` with budget-overridden `AppArgs` + freshly-inserted `EffectiveWorldSize` resource.

## Three levers — exact mechanics

### Lever #1 — TAA ring depth (already plumbed end-to-end)

- Source of truth: `AppArgs.taa_ring_depth: u32`. Default constant `DEFAULT_TAA_RING_DEPTH = 32` at `crates/bevy_naadf/src/lib.rs:121`. Default test pin at `crates/bevy_naadf/src/app_args.rs:228-235` accepts `{16, 24, 32}` for the default value — does **not** restrict the runtime value, so the budget routine can pick `8`, `4`, or `0` without breaking the test.
- Plumbing pattern (the **template** for any future lever):
  - `crates/bevy_naadf/src/render/mod.rs:105-118` — reads `AppArgs.taa_ring_depth` at plugin-build, inserts `TaaRingConfig` into the render sub-app.
  - `crates/bevy_naadf/src/render/taa.rs:46-50` — `TaaRingConfig` `Resource` definition (one `u32`).
  - `crates/bevy_naadf/src/render/taa.rs:476-505` — consumes `ring_depth` for both buffer sizing (`taa_samples = pixel_count × depth × 8 B`) AND ladders the WGSL shader-def.
  - `crates/bevy_naadf/src/render/pipelines.rs:363-365` — injects `#{TAA_SAMPLE_RING_DEPTH}` into WGSL at pipeline specialization.
- **Ladder (per the handoff):** `{32, 24, 16, 8, 4, 0}`. `0` = TAA disabled (existing supported path via `taa_ring_depth=0`). At iPhone-native ~3M pixels: 32 → ~720 MiB, 8 → ~180 MiB, 4 → ~90 MiB, 0 → 0.

### Lever #2 — world size (mobile XZ shrink)

- Source of truth today: `pub const WORLD_SIZE_IN_SEGMENTS: UVec3 = UVec3::new(16, 2, 16)` at `crates/bevy_naadf/src/world_size.rs:16`. Derived constants `WORLD_SIZE_IN_CHUNKS` (× 16 per segment) and `WORLD_SIZE_IN_VOXELS` (× chunks × 16) at `world_size.rs:36, :40`. Compile-time C#-canonical pin at `world_size.rs:46-54`.
- **Per the locked Q2 decision: const + parallel runtime override.** Keep all three `pub const`s + the pin intact. Introduce a runtime `Res<EffectiveWorldSize>` (architect picks final name) that the consumer sites read instead. Desktop: identical to const. Mobile: scaled-down by budget routine.
- **15 consumer sites the architect must enumerate** (from the audit):
  - `crates/bevy_naadf/src/voxel/grid.rs:34, :187, :191-193, :245-250, :307-325, :527-532, :1184, :1266`
  - `crates/bevy_naadf/src/render/construction/producer.rs:138-140, :147, :179-181, :230-247, :334-336`
  - `crates/bevy_naadf/src/render/construction/validation.rs:887-900, :3090`
  - `crates/bevy_naadf/src/render/construction/mod.rs:1003`
  - `crates/bevy_naadf/src/lib.rs:38-39`
  - Test-only sites at `voxel/grid.rs:1184, :1266` consume the const directly as test targets (expect the C#-canonical 256×32×256). Architect decides whether these stay on the const (testing the C# pin) or migrate (testing the runtime value).
- **Shader good news (audit side-note #4):** WGSL files at `bounds_calc.wgsl:78, :236-238, :261-262`, `chunk_calc.wgsl:72-73`, `entity_update.wgsl:78` already read world size from runtime uniform `params.size_in_chunks`. The Rust side fills these from `crate::WORLD_SIZE_IN_CHUNKS` (`producer.rs:230-247`, `validation.rs:887-900`). **The migration is Rust-only; no shader rewrite.** The dispatch loop at `producer.rs:179-181` (`for sz in 0..crate::WORLD_SIZE_IN_SEGMENTS.z`) must read the runtime size.
- **Worst-case sizing math (segments → chunks → voxels at 16× per step):**
  - Default `(16, 2, 16)` segments → `(256, 32, 256)` chunks (131,072 chunks) → `(4096, 512, 4096)` voxels.
  - `voxels` buffer = `chunk_count × 128 × 4 B` = 131,072 × 512 = **64 MiB per quartering**. Default: 1024 MiB. Halve XZ → 32,768 chunks → 256 MiB (fits at exactly the cap, no headroom). Quarter XZ → 8,192 chunks → **64 MiB (fits with headroom)**.
  - `blocks` buffer = `chunk_count × 64 × 4 B` = halve to 128 MiB, quarter to 32 MiB.

### Lever #3 — internal-resolution scale (**DEFERRED, out of scope**)

Mentioned for context; architect must NOT design it in. Lever #3 has no existing wiring; its surface is `extract_camera::extract_camera` at `crates/bevy_naadf/src/render/extract.rs:357`.

## Composition algorithm (what the budget routine actually decides)

Architect designs the precise algorithm. Sketched constraints:

1. Read `limits.max_storage_buffer_binding_size` from probe-app's `RenderDevice`.
2. Compute headroom target: `cap × 0.75` (or whatever constant the architect picks — must be ≥ 75%).
3. Compute candidate per-binding sizes for each (TAA ring depth, world-size) combination.
4. Pick the **largest** world size and **deepest** TAA ring that **all four** bindings (`voxels`, `blocks`, `taa_sample_accum`, `taa_samples`) fit under the headroom.
5. Output:
   - `AppArgs.taa_ring_depth` = chosen value.
   - `EffectiveWorldSize` resource = chosen `UVec3` (desktop: const; mobile: scaled).
   - A startup log line documenting the chosen values + the cap that drove the choice + the headroom factor.

## Reuse highlights (from `00-reuse-audit.md`)

- **`TaaRingConfig` mirror pattern** — the **complete template** for "configure on `AppArgs`, mirror into render sub-app at plugin-build, consume from both Rust buffer sizing and WGSL shader-def". Lever #1 already follows this; the new world-size lever should copy the shape with `EffectiveWorldSize`.
- **`validate_gpu_construction_production_scale`** at `validation.rs:1037-1080` — the prior art for "spin up a headless render world, read device limits, decide". Mirror its `App::new() + DefaultPlugins + Plugin::ready + Plugin::finish` setup; reuse `world/buffer.rs:246-264`'s `RenderDevice` extraction technique.
- **`prepare_world_gpu` Q4 limits check** at `render/prepare/world.rs:390-426` — the existing per-binding comparison math (`limits.max_storage_buffer_binding_size / (1024 * 1024)` etc.). Lift the math upstream into the new budget routine; the existing diagnostic at `prepare/world.rs:391` can either stay (defense-in-depth) or be deleted once the upstream routine is in place — architect's call.
- **`ConstructionConfig::from(&AppArgs)`** at `render/construction/config.rs:252-288` — the cfg-conditional override layer pattern. Inspirational, not directly reusable (runs too late).

## New module to introduce

`crates/bevy_naadf/src/render/budget.rs` (or `crates/bevy_naadf/src/mobile_budget.rs` — architect picks the location). Owns:

- `MIN_STORAGE_BINDING_CAP_BYTES = 256 * 1024 * 1024` (the WebGPU spec minimum / universal mobile ceiling).
- `MOBILE_HEADROOM_FACTOR = 0.75` (or architect's choice).
- The TAA ring ladder constant: `[32, 24, 16, 8, 4, 0]`.
- The world-size ladder constant: e.g. `[(16, 2, 16), (8, 2, 8), (4, 2, 4)]` (architect picks the ladder).
- The probe-app function + the `Limits → (TaaRingDepth, EffectiveWorldSize)` selection function.

## Forbidden moves (anti-patterns this orchestration explicitly avoids)

- **Migrating `pub const WORLD_SIZE_IN_SEGMENTS` to a runtime resource.** Q2 chose const + parallel override. Do not touch the const or its compile-time pin.
- **Designing lever #3 in.** Q3 deferred it.
- **Reading limits inside the real App's `Startup` schedule.** Main-world `Startup` can't reach `RenderDevice` cleanly (lives in render sub-app); attempting it fights Bevy's extract flow. Q4 picked the probe-app pattern instead.
- **Buffer splitting / sliding-window bindings.** Off-scope per the handoff.
- **iOS-specific build path.** Off-scope. The cap fix is shared; iOS toolchain is a separate session.
- **Touch input, release-mode optimization.** Off-scope.

## Required reading (in addition to this file)

1. `docs/todo/android-build.md` — full handoff context, build commands, device facts.
2. `docs/orchestrate/mobile-budget/00-reuse-audit.md` — the audit. Read it cover-to-cover; the architect's design must reckon with all 3 borderline calls and the 7 side-notes.
3. `crates/bevy_naadf/src/lib.rs:38-39, :121, :300` — `WORLD_SIZE_IN_*` re-exports, `DEFAULT_TAA_RING_DEPTH`, `build_app_with_args`.
4. `crates/bevy_naadf/src/app_args.rs` — `AppArgs` struct and its CLI parse.
5. `crates/bevy_naadf/src/world_size.rs` — const definitions + the compile-time pin.
6. `crates/bevy_naadf/src/render/mod.rs:105-118` — the `TaaRingConfig` plumbing template (read it carefully — this is what the world-size lever copies).
7. `crates/bevy_naadf/src/render/taa.rs:46-50, :476-505` — `TaaRingConfig` resource + its consumers.
8. `crates/bevy_naadf/src/render/pipelines.rs:363-365` — WGSL shader-def injection.
9. `crates/bevy_naadf/src/render/prepare/world.rs:320-346, :390-426` — the four oversized allocations + the existing diagnostic Q4 limits check.
10. `crates/bevy_naadf/src/render/construction/validation.rs:1037-1080` — the headless-probe-app pattern to mirror.
11. `crates/bevy_naadf/src/world/buffer.rs:246-264` — the `RenderDevice` extraction technique used by that pattern.
12. `crates/bevy_naadf/src/android_main.rs` — current minimal probe (will flip back to the real entry once budgets land).
13. `crates/bevy_naadf/src/main.rs:39-52` — production binary CLI parse (architect decides whether a `--probe` flag is in scope or deferred).

## Verification surface

Per `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md`:
- `cargo build --workspace` — compiles
- `cargo test --workspace --lib` — unit + integration
- e2e gates via `cargo run --bin e2e_render -- <mode>` as relevant (none of the existing modes specifically cover budget; architect may decide whether to add one or whether the symptom is binary enough to rely on the device install)

**The user does the live visual check on the Mali-G52 tablet.** Implementer does NOT run `cargo run --bin bevy-naadf` as verification (project rule, binding).

## Useful prior context outside this orchestration

- `docs/orchestrate/naadf-bevy-port/01-context.md` — original naadf-bevy-port orchestration scope.
- `docs/orchestrate/feature-completeness/01-context.md` — current feature-completeness orchestration scope.
- The user's `~/.claude/CLAUDE.md` faithful-port rule: "no Bevy-only microoptimizations or behaviors not in C# NAADF; default = match C#, even when C# has the bug. Deliberate divergences require explicit user approval + docs entry." — **The world-size mobile divergence is approved (Q2). The docs-entry requirement is satisfied by this orchestration's existence + the architect's design doc. No separate docs entry required.**
