# Implementation log — config-as-resource refactor

Implementation log for the migration plan defined in
`02-design.md` §4. Each section corresponds to one numbered step. Verification
gates are recorded inline.

## Step 1 — Introduce BootstrapInputs (2026-05-21)

### What landed
- New file: `crates/bevy_naadf/src/bootstrap.rs` (96 LOC including the test
  module).
- `crates/bevy_naadf/src/lib.rs:19` — `pub mod bootstrap;` declaration added
  adjacent to `pub mod app_args;` / `pub mod app_config;` / `pub mod app_mode;`.

The new module exposes:
- `pub struct BootstrapInputs { pub args: AppArgs }` with `#[derive(Clone,
  Default)]`. (No `Debug` derive — `AppArgs` is `#[derive(Resource, Clone)]`
  only, not `Debug`. Forcing `Debug` here would force the derive on `AppArgs`
  and its inner types, which is out of scope for Step 1. Surfaced in side
  notes.)
- `pub fn build_app_with_bootstrap_inputs(cfg: AppConfig, inputs:
  BootstrapInputs) -> App` — forwards to `crate::build_app_with_args(cfg,
  inputs.args)`.
- `pub fn run_e2e_render_with_bootstrap_inputs(inputs: BootstrapInputs) ->
  AppExit` — forwards to `crate::run_e2e_render_with_args(inputs.args)`.
- `#[cfg(test)] mod tests` with one test
  (`default_wraps_canonical_app_args_defaults`) that spot-checks the 15
  load-bearing default fields field-by-field.

No `pub use bootstrap::BootstrapInputs;` re-export was added — see decisions
below.

### Decisions made during impl

1. **Return type for the e2e wrapper.** Brief sketch suggested `ExitCode`;
   the actual `crate::run_e2e_render_with_args` signature at
   `lib.rs:412` returns `bevy::prelude::AppExit`. Used `AppExit` — matches
   the wrapped function and avoids a conversion at the wrapper layer.
2. **`AppConfig` parameter on `build_app_with_bootstrap_inputs`.** The brief
   sketch omitted the `cfg` parameter; verified `crate::build_app_with_args`
   actually takes `(cfg: AppConfig, args: AppArgs)` at `lib.rs:175`, and the
   design's §3.3 sketch also takes `cfg`. Wrapper signature is
   `(cfg: AppConfig, inputs: BootstrapInputs)`.
3. **No `Debug` derive.** `AppArgs` is not `Debug`; adding `Debug` to
   `BootstrapInputs` would require deriving on `AppArgs` (and recursively on
   `GiSettings`, `ConstructionConfig`, …) which goes beyond Step 1's
   no-behaviour-change scope. Documented inline.
4. **No `pub use` re-export.** The design doc refers to the type as
   `crate::bootstrap::BootstrapInputs` throughout. Adding a `pub use`
   re-export to `lib.rs` would force a choice between `crate::BootstrapInputs`
   and `crate::bootstrap::BootstrapInputs` for downstream callers; defer to
   Step 2 (the first step that actually migrates a caller) so the import
   path is settled once with a real call site visible.
5. **Test shape.** `AppArgs` is neither `PartialEq` nor `Debug`. Used a
   field-by-field spot-check on the 15 default fields rather than a single
   equality assertion via debug-format. The test guards the e2e-determinism
   contract by construction — if the inner `AppArgs::default()` drifts, the
   migration's pin tests already catch it, but this also pins
   `BootstrapInputs::default().args` to those values explicitly.

### Verification
- `cargo build --workspace`: PASS (41.15s — clean compile, no new warnings
  introduced by this change).
- `cargo test --workspace --lib`: PASS (190 passed; 0 failed; 1 ignored).
  The new `bootstrap::tests::default_wraps_canonical_app_args_defaults`
  test is in the passing set.
- (Optional `cargo run --bin e2e_render -- baseline` sanity ping: not run —
  Step 1 introduces no resource inserts and no caller migration, so no
  e2e-path behaviour can have changed; cargo build + cargo test are
  sufficient and the e2e gate is a 60+ s sanity ping with no signal to
  recover.)

### Files touched
| File | Lines | Change |
|---|---|---|
| `crates/bevy_naadf/src/bootstrap.rs` | new (96) | created |
| `crates/bevy_naadf/src/lib.rs` | +1 line (`:19`) | added `pub mod bootstrap;` |

### Side notes / observations / complaints

- **The Step-1 shape (`BootstrapInputs.args: AppArgs`) is the minimal correct
  thing**, but it's worth flagging that the `args` field WILL be drained over
  Steps 2-8 and the struct's "real" shape only appears at Step 8 or so. The
  intermediate steps will all have a half-drained `AppArgs` next to typed
  fields — that's the design's intent (incremental migration) but it does
  mean the codebase is "uglier" between Step 1 and Step 8 than at either
  end. The orchestrator should expect impl logs in Steps 2-7 to describe
  somewhat awkward intermediate shapes — that's the cost of bisectability,
  not a smell to chase.
- **`AppArgs` not deriving `Debug` is a minor inconvenience.** The
  three-line workaround (field-by-field spot-checks in the test) is fine for
  one test, but if more tests need to assert default-identity over the
  course of the migration, adding `Debug` to `AppArgs` + its inner types
  (`GiSettings` already derives `Debug` per `settings/canonical.rs`;
  `ConstructionConfig` would need to be verified) is a one-line PR worth
  doing early. Not required by Step 1's brief so I didn't touch it; flagging
  for the orchestrator.
- **No `pub use bootstrap::BootstrapInputs;` deliberately deferred.** See
  decision 4 above. Step 2's first caller migration is the natural place to
  pick the import path; doing it now is premature.
- **The design's Step 1 spec is unusually loose** ("introduce a struct and
  two wrappers, change nothing") — which is the right shape for an
  introductory commit on a 9-step migration; the deferred work in Step 2 is
  where the actual extract-pattern test happens. Good plan structure.
- **The foundation looks sound.** Read enough of the design doc to be
  comfortable that the per-domain resource decomposition + transient
  `BootstrapInputs` carrier is the right move. No concerns about the design
  itself.

## Step 2 — Migrate taa_ring_depth (2026-05-21)

### What landed

- `crates/bevy_naadf/src/render/taa.rs` — renamed existing render-world
  `TaaRingConfig` to `RenderTaaRingConfig`; added new main-world canonical
  `TaaRingConfig` (with `Default` = `crate::DEFAULT_TAA_RING_DEPTH`).
  Relocated the two pin tests from `app_args.rs::tests` into
  `render/taa.rs::tests` and added one new test
  (`render_mirror_default_is_canonical`) on the render-world mirror's
  `Default` impl. Both `Default` impls produce `depth = 32`.
- `crates/bevy_naadf/src/render/extract.rs` — added `extract_taa_ring_depth`
  (mirror of `extract_effective_world_size`). Source =
  `Extract<Option<Res<TaaRingConfig>>>`; sink = `ResMut<RenderTaaRingConfig>`.
- `crates/bevy_naadf/src/render/mod.rs` — deleted the plugin-build snapshot
  (previously at `:113-126` reading `args.taa_ring_depth`). Replaced with
  `.init_resource::<RenderTaaRingConfig>()` on the render sub-app. Wired
  `extract_taa_ring_depth` into the `ExtractSchedule` block. Updated the
  `use taa::{...}` import + the multi-line comment to describe the new
  shape.
- `crates/bevy_naadf/src/render/pipelines.rs` — `NaadfPipelines::from_world`
  now reads `RenderTaaRingConfig` (was `TaaRingConfig`). Updated import +
  docblock.
- `crates/bevy_naadf/src/bootstrap.rs` — added
  `pub taa_ring_depth: TaaRingConfig` field on `BootstrapInputs`.
  `build_app_with_bootstrap_inputs` now inserts the resource post-build
  (Bevy's `insert_resource` overwrite-in-place semantic carries the
  budget-selected value through when callers supply one).
  `run_e2e_render_with_bootstrap_inputs` rewritten to route through
  `build_app_with_bootstrap_inputs` (was forwarding to
  `run_e2e_render_with_args`) so the new `taa_ring_depth` field actually
  reaches the App. Pin test updated to read `inputs.taa_ring_depth.depth`
  rather than `inputs.args.taa_ring_depth`.
- `crates/bevy_naadf/src/lib.rs` — `build_app_with_budget` now constructs a
  `BootstrapInputs { args, taa_ring_depth: TaaRingConfig { depth: caps.taa_ring_depth } }`
  and routes through `build_app_with_bootstrap_inputs` instead of mutating
  `args.taa_ring_depth` + calling `build_app_with_args` directly. Docblock
  updated.
- `crates/bevy_naadf/src/main.rs` — wasm32 path likewise: constructs
  `BootstrapInputs` carrying the budget-selected TAA depth + forwards
  through `build_app_with_bootstrap_inputs`. (Native path was already
  routed through `build_app_with_budget` so no edit here for native.)
- `crates/bevy_naadf/src/android_main.rs` — **untouched**. It calls
  `build_app_with_budget` which is the single place the Android entry
  picks up the new bootstrap shape, so the migration carries through
  transparently.
- `crates/bevy_naadf/src/app_args.rs` — deleted `pub taa_ring_depth: u32`
  field + its line in `impl Default for AppArgs` + the two pin tests
  (relocated to `render/taa.rs::tests`). Pruned the unused
  `DEFAULT_TAA_RING_DEPTH` import. Updated module docblock.
- `crates/bevy_naadf/src/diagnostics.rs` — added
  `taa_ring: Option<Res<TaaRingConfig>>` system parameter. The `KeyP` dump
  block now formats `taa_ring_depth` from the new resource (showing
  `<TaaRingConfig resource missing>` if absent; defensive but should
  never fire because `build_app_with_bootstrap_inputs` always inserts it).
- `crates/bevy_naadf/src/settings/mod.rs` — added new
  `KnobKind::ReadonlyFromTaa { value: fn(&TaaRingConfig) -> String }`
  variant + `knob_readonly_taa!` macro (the partial Decision §7 split per
  the brief). Migrated the `taa_ring_depth` readonly row at `:262` to use
  it. `update_settings_text` gained a `taa_ring: Res<TaaRingConfig>`
  parameter + a new match arm for the new variant. The legacy
  `KnobKind::Readonly { fn(&AppArgs) -> String }` variant stays in place
  for the `global_illum_max_accum` row (Step 3 splits that to
  `ReadonlyFromGi`); the `is_interactive` / `reset_all_knobs` /
  `mouse_interact_settings` sites use wildcard patterns so they correctly
  fall through.

### Decisions made during impl

1. **`BootstrapInputs.taa_ring_depth: TaaRingConfig`** (Decision §2 of the
   design — carry resources, not raw values). `inputs.taa_ring_depth.depth`
   IS the value the resource gets inserted with. Mobile-budget overrides
   write the whole `TaaRingConfig { depth: caps.taa_ring_depth }` value.
2. **`build_app_with_bootstrap_inputs` does the resource insert POST-build**
   (after `crate::build_app_with_args` returns), not pre-build. Reason:
   `build_app_with_args` consumes `inputs.args` by move, so the insert
   either has to happen before the move (out of place — the App doesn't
   exist yet) or after. Post-build is the same pattern
   `build_app_with_budget` uses for `EffectiveWorldSize` /
   `InvalidSampleStorageCount`, and `insert_resource` is
   overwrite-in-place so any defensive seed inside `build_app_with_args`
   would be replaced regardless. The current `build_app_with_args` does
   not insert `TaaRingConfig` at all (the plugin-build snapshot was the
   only inserter, and it's deleted), so there's no double-insert here.
3. **`run_e2e_render_with_bootstrap_inputs` rewritten to route through
   `build_app_with_bootstrap_inputs`.** Step-1 shape was a thin forward to
   `run_e2e_render_with_args`; for Step 2 to be meaningful (the
   `taa_ring_depth` field has to actually reach the App), the wrapper has
   to call the fan-out function, not the legacy one. Replicates the
   `window_for_e2e_args` + `build_app_with_bootstrap_inputs` +
   `e2e::run_with_app` sequence from the design's §3.3 sketch.
4. **`RenderTaaRingConfig` initialised via `init_resource` with default
   `depth = 32`.** Mirrors `RenderEffectiveWorldSize::default` exactly —
   the seed is the canonical value, then `extract_taa_ring_depth`
   overwrites it from the main-world resource every frame. Design's
   Assumption §6 holds: `NaadfPipelines::from_world` runs in
   `RenderStartup` after the first `ExtractSchedule`, so it reads the
   post-extract value when injecting the `#{TAA_SAMPLE_RING_DEPTH}`
   shader-def. The baseline e2e gate validates this end-to-end (the
   shader-def would mismatch the buffer size if the from_world snapshot
   saw the seed instead of the extracted value — silent TAA ring
   corruption, observable as a visual regression).
5. **`KnobKind::ReadonlyFromTaa` is a new variant alongside `Readonly`,
   not a replacement.** Per the brief's "partial Decision §7" instruction
   — leaves the legacy `Readonly { fn(&AppArgs) -> String }` variant in
   place for the `global_illum_max_accum` row (which still reads
   `&AppArgs.gi.global_illum_max_accum`). Step 3 will add
   `ReadonlyFromGi` and migrate that row. Step 9 deletes the legacy
   variant. This kept the diff bounded and minimised collateral churn —
   `is_interactive` / `mouse_interact_settings` / `adjust_settings` all
   use wildcard match arms and need no edits.
6. **Diagnostics dump shows a defensive missing-resource string.** If
   `TaaRingConfig` is somehow absent (shouldn't happen post-Step-2 — it's
   inserted by `build_app_with_bootstrap_inputs`), the dump prints
   `<TaaRingConfig resource missing>` rather than panicking on a missing
   `Res`. Matches the existing `Option<Res<AppArgs>>` defensive pattern.

### Verification

- `cargo build --workspace`: PASS (clean compile, 38.20 s, no new warnings).
- `cargo test --workspace --lib`: PASS (191 passed; 0 failed; 1 ignored),
  including 3 tests in `render::taa::tests` (`default_taa_ring_depth_is_32`,
  `default_taa_ring_depth_is_a_supported_lever_value`,
  `render_mirror_default_is_canonical`).
- `cargo run --bin e2e_render -- baseline`: PASS — `e2e_render: PASS
  (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, …`. The
  region-luminance numbers (`emissive 247.6, solid 243.7, sky 202.9`) are
  in the same band as pre-Step-2 baselines, so the extract pattern
  + the `from_world` post-extract read is sound. The TAA-heavy
  `solid 243.7` value is the critical signal — a buffer-size /
  shader-def disagreement (the core risk of getting the extract pattern
  wrong) would collapse this toward ~4-6 per the `e2e_render` solid-region
  comment.

### Files touched

| File | Lines | Change kind |
|---|---|---|
| `crates/bevy_naadf/src/render/taa.rs` | +90 / -8 | Rename existing `TaaRingConfig` → `RenderTaaRingConfig`; add main-world `TaaRingConfig`; add tests module |
| `crates/bevy_naadf/src/render/extract.rs` | +18 | Add `extract_taa_ring_depth` system + docstring |
| `crates/bevy_naadf/src/render/mod.rs` | +12 / -13 | Delete snapshot, add `init_resource` + extract wire |
| `crates/bevy_naadf/src/render/pipelines.rs` | +9 / -5 | Read `RenderTaaRingConfig` |
| `crates/bevy_naadf/src/bootstrap.rs` | +30 / -8 | New `taa_ring_depth` field + resource insert + e2e wrapper rewrite |
| `crates/bevy_naadf/src/lib.rs` | +10 / -5 | `build_app_with_budget` routes through `BootstrapInputs` |
| `crates/bevy_naadf/src/main.rs` | +14 / -3 | wasm32 path likewise |
| `crates/bevy_naadf/src/app_args.rs` | +6 / -45 | Delete `taa_ring_depth` field + default + pin tests; update docblock |
| `crates/bevy_naadf/src/diagnostics.rs` | +9 / -2 | Read `Res<TaaRingConfig>` |
| `crates/bevy_naadf/src/settings/mod.rs` | +25 / -1 | Add `KnobKind::ReadonlyFromTaa` variant + macro + match arm |

### Side notes / observations / complaints

- **The extract pattern is clean and the precedent transfers verbatim.**
  Mirroring `extract_effective_world_size` produced
  `extract_taa_ring_depth` as a 1:1 structural copy — same `ResMut +
  Extract<Option<Res>>` signature, same `if let Some` pattern. Steps 3-8
  will reuse this verbatim; the design's instruction to follow the
  precedent is the right move. **No surprises** during the extract wiring.
- **`from_world` post-extract read is sound, per Assumption §6.** The
  baseline gate passing with the TAA-heavy region at canonical luminance
  is the empirical proof. No need for a Step-2 follow-up to seed the
  render-world resource via plugin-build snapshot.
- **The Step-1 forward in `run_e2e_render_with_bootstrap_inputs` had to be
  rewritten in Step 2.** Step-1's `crate::run_e2e_render_with_args(inputs.args)`
  shape would have silently dropped `inputs.taa_ring_depth` — the new
  field would never reach the App. Flagged because future steps that
  migrate fields out of `args` will hit the same issue every time, AND
  Step 1's "do nothing but wrap" shape obscured the fact that the
  forwarding wrapper had to grow. Worth calling out for Steps 3-8: if a
  step adds a new typed field to `BootstrapInputs`, it MUST also verify
  that `build_app_with_bootstrap_inputs` AND
  `run_e2e_render_with_bootstrap_inputs` both consume that field.
- **The `KnobKind::ReadonlyFromTaa` partial split was clean — no cascade.**
  The match arms that needed editing were in exactly one place
  (`update_settings_text`); `is_interactive`, `reset_all_knobs`,
  `apply_drag_delta`, `mouse_interact_settings`, and `handle_click_release`
  all use wildcard `_ => {}` patterns for non-interactive variants and
  needed no edits. The macro `knob_readonly_taa!` mirrors `knob_readonly!`
  exactly. Steps 3 and 9 should be similarly bounded.
- **Android entry was a no-op.** `android_main.rs` calls
  `build_app_with_budget` which is the single edited point for the budget
  override path — the JNI entry transparently picks up the new
  `BootstrapInputs` routing. The user's hard-gate visual check on Android
  will exercise the new path; if `taa_ring_depth = 8` doesn't reach the
  shader on Mali-G52, the on-device frame is the canary.
- **`build_app_with_args` is now in a transitional state.** It still
  inserts `AppArgs` (because most fields still live there), but it no
  longer reads `taa_ring_depth` (that field is gone from `AppArgs`). The
  plugin-build snapshot in `render/mod.rs` is gone — `NaadfRenderPlugin`
  no longer reads `Res<AppArgs>` at build time, only the post-Step-2
  `Res<TaaRingConfig>` via extract. As Steps 3-8 progress this transitional
  shape gets uglier (the `AppArgs` field carries fewer and fewer fields)
  before Step 9 deletes it.
- **No `[budget] …` log line was visible in the baseline e2e run.**
  Expected — `cargo run --bin e2e_render -- baseline` routes through
  `build_app_with_args` directly (NOT `build_app_with_budget`), so the
  budget probe never fires. The desktop baseline gate uses canonical
  defaults (TAA depth = 32, world = (16, 2, 16)) and is byte-identical to
  pre-Step-2. The mobile-budget override path is exercised only by the
  Android entry / wasm32 path; visual verification on Android is the
  user's hard-gate.
- **Foundation looks sound.** Step 2 exercised the full extract pattern
  end-to-end; the design's Assumption §6 holds; the e2e baseline is
  byte-identical; the settings panel partial-split landed clean. Steps
  3-8 should be smooth executions of the same pattern. The orchestrator
  can proceed with confidence.
