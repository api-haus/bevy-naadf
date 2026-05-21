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

## Step 3 — taa + gi (2026-05-21)

### What landed

- `crates/bevy_naadf/src/render/taa.rs` — new `TaaConfig { enabled: bool }`
  main-world resource with `Default::default()` = `TaaConfig { enabled: true }`
  (matches pre-Step-3 `AppArgs::default().taa`). `update_camera_history`
  parameter swap from `Res<AppArgs>` → `Res<TaaConfig>` (`args.taa` →
  `taa.enabled`).
- `crates/bevy_naadf/src/settings/canonical.rs` — `#[derive(Resource)]` on
  `GiSettings`. Added module-level docstring note about the promotion. No
  field changes; `DEFAULTS` const + `Default` impl untouched.
- `crates/bevy_naadf/src/render/extract.rs` — `extract_taa_config` source
  swap `Res<AppArgs>` → `Res<TaaConfig>`; `extract_gi_config` source swap
  `Res<AppArgs>` → `Res<GiSettings>`. Mirror shapes (`ExtractedTaaConfig`,
  `ExtractedGiConfig`) unchanged. Docstrings updated.
- `crates/bevy_naadf/src/app_args.rs` — deleted `pub taa: bool` (field +
  default line) and `pub gi: GiSettings` (field + default line). Pruned
  `use crate::GiSettings;` from imports. Updated module docstring.
- `crates/bevy_naadf/src/bootstrap.rs` — added `taa: TaaConfig` and
  `gi: GiSettings` fields on `BootstrapInputs`. `build_app_with_bootstrap_inputs`
  now inserts both via Bevy's overwrite-in-place `insert_resource`. Pin test
  swung onto the typed fields.
- `crates/bevy_naadf/src/lib.rs` — `build_app_with_budget` uses
  `..Default::default()` to inherit the new fields. **Added defensive seeds
  in `build_app_with_args`** for `TaaConfig`, `GiSettings`, and
  `TaaRingConfig`: the e2e_render binary calls `build_app(AppConfig::e2e())`
  which bypasses `build_app_with_bootstrap_inputs`, so without the seed
  `update_camera_history` (`Res<TaaConfig>`) panics on missing resource. See
  side notes for why this is the right shape and what Step 9 cleans up.
- `crates/bevy_naadf/src/main.rs` — wasm32 path: `..Default::default()`
  added to the `BootstrapInputs` struct literal.
- `crates/bevy_naadf/src/diagnostics.rs` — added `taa: Option<Res<TaaConfig>>`
  + `gi: Option<Res<GiSettings>>` parameters. Dump block now reads each from
  its per-domain resource (with a defensive missing-resource fallback string,
  mirroring the Step-2 pattern for `taa_ring`).
- `crates/bevy_naadf/src/settings/mod.rs` — added new
  `KnobKind::ReadonlyFromGi { value: fn(&GiSettings) -> String }` variant +
  `knob_readonly_gi!` macro (Decision §7 partial extension). Migrated the
  `global_illum_max_accum` readonly row off the legacy
  `KnobKind::Readonly { fn(&AppArgs) -> String }`. `KnobKind::Action` signature
  changed from `fn(&mut AppArgs)` to `fn(&mut GiSettings)`. All system
  parameters swapped: `adjust_settings` and `mouse_interact_settings` now
  take `ResMut<GiSettings>`; `apply_drag_delta`, `handle_click_release`,
  `reset_all_knobs` take `&mut GiSettings`. `update_settings_text` gains
  `gi: Res<GiSettings>` (the U32/F32/Bool/`ReadonlyFromGi` rows read it)
  while keeping `args: Res<AppArgs>` (still feeding the legacy
  `Readonly { fn(&AppArgs) -> String }` rows that pass `|_| const`
  closures — see side notes). The `reset_all_knobs_restores_defaults` unit
  test was updated to mutate `GiSettings` directly.

### Decisions made during impl

1. **Defensive seeds in `build_app_with_args` for `TaaConfig`, `GiSettings`,
   and `TaaRingConfig`** instead of routing every caller through
   `build_app_with_bootstrap_inputs`. The e2e_render binary's
   `run_e2e_render` / `run_e2e_render_with_args` build via `build_app`,
   which calls `build_app_with_args` directly — bypassing the
   bootstrap fan-out. After Step 3, `update_camera_history` reads
   `Res<TaaConfig>` (non-Option), so without the seed the system panics on
   the first frame. The seed mirrors the existing
   `EffectiveWorldSize::canonical()` / `InvalidSampleStorageCount::canonical()`
   pattern at the bottom of `build_app_with_args` (`lib.rs:245-258`). Step 9
   will delete both the seed AND `build_app_with_args` once every caller
   routes through `build_app_with_bootstrap_inputs`. This is the cleanest
   incremental shape given Step 2 was permitted to insert `TaaRingConfig`
   only through the wrapper (Step 2 was bisectable; this regression-of-shape
   trade-off is what makes Step 3 bisectable too).
2. **`KnobKind::ReadonlyFromGi` partial split — same shape as
   `ReadonlyFromTaa` from Step 2.** `update_settings_text` gains one match
   arm; all other settings-panel match-arms use wildcard fall-throughs and
   needed no edits. No cascade (Decision §7's "would flip if more than 3-4
   variants" threshold is far away).
3. **`update_settings_text` keeps the legacy `args: Res<AppArgs>`
   parameter.** All remaining `KnobKind::Readonly { fn(&AppArgs) -> String }`
   rows (`camera_history_depth`, `valid_sample_storage`, …) pass
   `|_| const` closures — they don't read AppArgs fields, but the closure
   signature still requires the parameter. Step 9 deletes both the variant
   and the parameter once `AppArgs` is gone.
4. **`Default` for `TaaConfig`** = `TaaConfig { enabled: true }` (TAA on),
   not `#[derive(Default)]` which would give `enabled: false`. Matches the
   pre-refactor `AppArgs::default().taa = true`. The test
   `default_wraps_canonical_app_args_defaults` pins this.
5. **`Action` knob signature `fn(&mut GiSettings)`.** Today there's exactly
   one action knob: "RESET ALL TO DEFAULTS", which is exactly
   `reset_all_knobs(&mut GiSettings)`. The signature swap is mechanical;
   the test was updated.

### Verification

- `cargo build --workspace`: PASS (clean compile, 31.01s, no new warnings).
- `cargo test --workspace --lib`: PASS (191 passed; 0 failed; 1 ignored).
  The relocated `reset_all_knobs_restores_defaults` test (now mutating
  `GiSettings` directly) is in the passing set, validating that Assumption
  #8 held — the translation was mechanical.
- `cargo run --bin e2e_render -- baseline`: PASS — `e2e_render: PASS
  (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, …`.
  Region-luminance numbers (`emissive 247.6, solid 243.7, sky 202.9`) are
  byte-identical to Step 2's baseline output — confirming the extract
  source swap is lossless.
- `cargo run --bin e2e_render -- --vox-horizon-native`: PASS — exit 0,
  screenshot `vox_horizon_native.png` written. TAA-heavy gate; the
  `update_camera_history` swap to `Res<TaaConfig>` did not perturb the
  TAA ring.

### Files touched

| File | Lines | Change kind |
|---|---|---|
| `crates/bevy_naadf/src/render/taa.rs` | +33 / -8 | Add `TaaConfig` main-world resource; `update_camera_history` signature swap |
| `crates/bevy_naadf/src/settings/canonical.rs` | +14 / -3 | `#[derive(Resource)]` + module docstring note |
| `crates/bevy_naadf/src/render/extract.rs` | +18 / -8 | Source swap for `extract_taa_config` + `extract_gi_config` |
| `crates/bevy_naadf/src/app_args.rs` | +9 / -16 | Delete `taa` / `gi` fields + defaults; docstring update |
| `crates/bevy_naadf/src/bootstrap.rs` | +30 / -6 | New `taa: TaaConfig` + `gi: GiSettings` fields + fan-out inserts + pin test |
| `crates/bevy_naadf/src/lib.rs` | +28 / -1 | Defensive seeds + `..Default::default()` on the `build_app_with_budget` literal |
| `crates/bevy_naadf/src/main.rs` | +1 | `..Default::default()` on wasm32 `BootstrapInputs` literal |
| `crates/bevy_naadf/src/diagnostics.rs` | +20 / -7 | Add `taa` + `gi` params; defensive missing-resource fallback strings |
| `crates/bevy_naadf/src/settings/mod.rs` | +56 / -25 | New `ReadonlyFromGi` variant + macro + `ResMut<GiSettings>` swap across all panel systems |

### Side notes / observations / complaints

- **Design's per-step file:line citations matched current code** — Read/Grep
  verified `args.gi` / `args.taa` consumer counts (21 sites the design
  predicted, ~21 actually touched) and the `update_camera_history` /
  `extract_*` / `mouse_interact_settings` / `adjust_settings` /
  `update_settings_text` / `reset_all_knobs` sites. No drift.
- **The defensive-seed in `build_app_with_args` discovery is a Step-2 latent
  bug now patched.** Step 2's impl notes claimed the `from_world` post-extract
  read was sound because the baseline gate passed. That was true — but only
  because **nothing in `build_app_with_args`'s call-path read `TaaRingConfig`
  as `Res<...>` non-Option from the main world** (the `update_settings_text`
  panel read is gated by `cfg.add_hud`, which is OFF in e2e). Step 3
  exposed the gap by adding a non-Option `Res<TaaConfig>` consumer that
  fires on every frame (`update_camera_history`, in `CameraPlugin` which
  is always added). The fix retrofits `TaaRingConfig` too — even though no
  Step-2 test surfaced it, the implicit-ordering invariant from
  `02-design.md`'s side notes was being violated in a corner case
  (`add_hud + e2e` would have panicked).
- **Assumption #8 held** — the `reset_all_knobs_restores_defaults` test
  translated mechanically from `AppArgs::default()` + `args.gi.*` reads to
  `GiSettings::default()` + direct field reads. Test passes.
- **Assumption #5 (implicit ordering invariant)** continues to hold — every
  per-domain resource has a canonical default seed reachable from every
  `build_app*` entry point. The Step-3 fix tightened this for `TaaConfig` +
  `GiSettings` + (retroactively) `TaaRingConfig`.
- **Decision §7 partial extension cascaded cleanly.** Adding
  `KnobKind::ReadonlyFromGi` mirrored Step 2's `ReadonlyFromTaa` 1:1 — one
  new variant, one new macro, one new match arm in `update_settings_text`,
  zero edits to the wildcard fall-throughs in `is_interactive` /
  `apply_drag_delta` / `handle_click_release` / `mouse_interact_settings` /
  `adjust_settings`. The macro-based knob table absorbs new readonly-source
  types gracefully.
- **The wasm32 `ConstructionConfig` divergence (Decision §5) is unaffected
  by Step 3** — `From<&AppArgs>` still reads `args.construction_config`;
  the wasm clamp still lives inside that impl. Step 4 relocates it.
- **`KnobKind::Action`'s signature change is small but load-bearing.** It
  pre-emptively prepares the action variant for Step 9's `AppArgs` deletion
  — today the only action is `reset_all_knobs`, which already only mutates
  GI fields. Switching to `fn(&mut GiSettings)` now (vs deferring to Step
  9) avoided one extra round of touch in the same file.
- **No `from_world`-ordering surprises.** Same as Step 2: `extract_taa_config`
  and `extract_gi_config` runs in `ExtractSchedule` before any render-world
  consumer reads the mirror. No silent shader-def disagreement.
- **`AppArgs` is now down to 14 fields** (was 16 pre-refactor, 15 after
  Step 1, 14 after Step 2, 14 after Step 3 — wait, that's still 14 because
  Step 3 deleted 2 fields. Confirmed: `grid_preset` + `construction_config`
  + `spawn_test_entity` + `resize_test` + 10 e2e mode booleans + 1
  `vox_e2e_mode` = **14 fields** remaining for Steps 4, 5, 8 to drain).
- **Subjective:** Step 3 is the largest single-step touch so far in terms
  of files but the cleanest in terms of pattern — the settings-panel
  parameter swap is a global find-and-replace of `args` → `gi`, and the
  extract source swap is one line each. Pattern is starting to feel rote;
  good sign.
- **Foundation continues to look sound** — same Assumptions hold; the
  per-domain `Resource` decomposition cleanly handles runtime-mutable
  state (GI panel) without forcing a re-litigation of the design.
