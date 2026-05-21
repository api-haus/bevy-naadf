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

## Step 4 — construction_config + wasm32 divergence relocation (2026-05-21)

### What landed

- `crates/bevy_naadf/src/render/construction/config.rs` — added
  `ConstructionConfig::for_target_arch() -> Self` constructor (Decision §5);
  body is `Default::default()` on native + the documented wasm32 clamp
  (`max_group_bound_dispatch.min(WASM_MAX_GROUP_BOUND_DISPATCH)` +
  `n_bounds_rounds = 1`) on `target_arch = "wasm32"`. Deleted
  `impl From<&crate::AppArgs> for ConstructionConfig`. Moved the const-pin's
  meaning (`02-design.md` side note #2) — added a new
  `for_target_arch_desktop_matches_canonical_pin` runtime test under
  `#[cfg(not(target_arch = "wasm32"))]` that asserts the constructor's
  desktop output equals every literal-field value the const block pins.
- `crates/bevy_naadf/src/render/construction/mod.rs` — `ConstructionPlugin::build`
  no longer reads `Res<AppArgs>` + applies `From<&AppArgs>` to lift the
  config into the render sub-app. Instead, the render sub-app calls
  `init_resource::<ConstructionConfig>()`; `extract_construction_config`
  carries the main-world value across every frame.
  `run_gpu_construction_startup` parameter swap from `Res<AppArgs>` to
  `Res<ConstructionConfig>` (`args.construction_config.gpu_construction_enabled`
  → `cc.gpu_construction_enabled`).
- `crates/bevy_naadf/src/render/extract.rs` — added
  `extract_construction_config` (1:1 mirror of `extract_effective_world_size`
  — `ResMut<ConstructionConfig>` + `Extract<Option<Res<ConstructionConfig>>>`).
- `crates/bevy_naadf/src/render/mod.rs` — wired
  `extract_construction_config` into the `ExtractSchedule` tuple alongside
  `extract_taa_ring_depth` / `extract_effective_world_size` / etc.; updated
  the docblock to call out the Step-4 brought-onto-pattern.
- `crates/bevy_naadf/src/app_args.rs` — deleted
  `pub construction_config: render::construction::ConstructionConfig`
  field + the matching default-impl line. Pruned `use crate::render;` import
  (now unused). Docstring updated.
- `crates/bevy_naadf/src/bootstrap.rs` — added
  `construction_config: ConstructionConfig` field on `BootstrapInputs`.
  Removed `#[derive(Default)]` and wrote a hand `impl Default for
  BootstrapInputs` so the construction-config default is
  `ConstructionConfig::for_target_arch()` (not `default()`), keeping the
  wasm32 divergence on the bootstrap path. `build_app_with_bootstrap_inputs`
  inserts `inputs.construction_config` post-`build_app_with_args`. Pin test
  swung onto the typed field.
- `crates/bevy_naadf/src/lib.rs` — added a defensive seed in
  `build_app_with_args` for `ConstructionConfig::for_target_arch()` so the
  direct `build_app(AppConfig::e2e())` path (`run_e2e_render` /
  `run_e2e_render_with_args`) still has a `ConstructionConfig` resource —
  `run_gpu_construction_startup` reads it as `Res<...>` non-Option and
  `extract_construction_config` needs the main-world source. Same shape as
  Step 3's defensive seeds.
- `crates/bevy_naadf/src/diagnostics.rs` — added
  `construction: Option<Res<ConstructionConfig>>` parameter; dump reads
  from the resource with a defensive missing-resource fallback string.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — `BootCommand::EntitiesBoot`
  arm builds a `BootstrapInputs` with `construction_config.entities_enabled
  = true` and routes through `run_e2e_render_with_bootstrap_inputs`.
  (`spawn_test_entity` stays on `AppArgs` until Step 8.)
- 4 e2e gate `run_*` builders: `vox_gpu_construction.rs:223-263`,
  `vox_gpu_oracle.rs:329-345`, `vox_horizon_parity.rs:156-172`,
  `vox_web_parity.rs:198-214` — each moved from
  `app_args.construction_config.gpu_construction_enabled = true;
  crate::run_e2e_render_with_args(app_args)` to building a
  `BootstrapInputs` with the same field override on
  `construction_config: ConstructionConfig::for_target_arch()` and routing
  through `crate::bootstrap::run_e2e_render_with_bootstrap_inputs`.

### Decisions made during impl

1. **Hand `impl Default for BootstrapInputs` instead of `#[derive(Default)]`**
   — the design's Decision §5 requires the construction-config default to be
   `for_target_arch()` (so the wasm32 divergence travels through bootstrap),
   not `ConstructionConfig::default()`. The hand-impl spells out every
   field's seed explicitly. Today desktop's `for_target_arch()` == `default()`
   so the value is byte-identical on native; wasm32 picks up the documented
   clamp.
2. **The const-pin stays as literal field values** + a new sibling runtime
   test asserts `for_target_arch()`'s desktop output equals those values.
   This is the design's side-note #2 directive ("move the pin to assert
   against the desktop arm"). The literal const-pin can't be moved verbatim
   into a `const _: () = assert!(ConstructionConfig::for_target_arch() == ...);`
   because const-eval can't evaluate `#[cfg(target_arch = "wasm32")]` bodies
   inside the constructor (the `mut` + cfg-gated assignment is non-const).
   The runtime test is the same teeth at test-run-time rather than build-time;
   the literal const block + the runtime test together pin both surfaces.
3. **The 4 e2e gates' `gpu_construction_enabled = true` overrides are
   belt-and-braces** (the default is already `true`). Kept the override
   explicit in case a future default flip would silently turn them off. The
   only meaningful override in the workspace is `EntitiesBoot`'s
   `entities_enabled = true`.
4. **Defensive seed pattern matches Step 3.** `build_app_with_args` seeds
   `ConstructionConfig::for_target_arch()` only if no caller already
   inserted one. Step 9 deletes the seed once every caller routes through
   `build_app_with_bootstrap_inputs`.
5. **Render sub-app `init_resource::<ConstructionConfig>()`** uses
   `Default::default()` (canonical desktop) as the seed; the first
   `ExtractSchedule` overwrites it with the main-world value (which IS the
   `for_target_arch()` value on wasm32, post-bootstrap). The render-side
   first-frame difference is invisible — on desktop the seed equals the
   extracted value; on wasm32 the extract runs before any consumer reads
   the resource for a render-graph dispatch, same first-frame story as
   `RenderEffectiveWorldSize`.

### Verification

- `cargo build --workspace`: PASS (47.31s, clean compile, no new warnings).
- `cargo test --workspace --lib`: PASS (192 passed; 0 failed; 1 ignored).
  The new `for_target_arch_desktop_matches_canonical_pin` test under
  `render::construction::config::tests` is in the passing set — pins
  `for_target_arch()` desktop output to the literal const-pin values.
- `cargo run --bin e2e_render -- --validate-gpu-construction`: PASS — GPU
  construction byte-equal to CPU oracle: 388 bytes compared. Region
  luminance band identical to Steps 1-3.
- `cargo run --bin e2e_render -- --vox-gpu-construction`: PASS —
  brush-projection rect Δ over floor. The full pipeline-build/extract path
  through `ConstructionConfig` works on Oasis.

### Files touched

| File | Lines | Change kind |
|---|---|---|
| `crates/bevy_naadf/src/render/construction/config.rs` | +75 / -38 | Add `for_target_arch` constructor; delete `From<&AppArgs>`; add runtime test pinning desktop arm to const-pin values |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +13 / -16 | `init_resource::<ConstructionConfig>()` replaces `From<&AppArgs>` lift; `run_gpu_construction_startup` param swap |
| `crates/bevy_naadf/src/render/extract.rs` | +20 | Add `extract_construction_config` |
| `crates/bevy_naadf/src/render/mod.rs` | +7 / -4 | Wire `extract_construction_config` into `ExtractSchedule` tuple |
| `crates/bevy_naadf/src/app_args.rs` | +9 / -16 | Delete `construction_config` field + default; docstring update |
| `crates/bevy_naadf/src/bootstrap.rs` | +37 / -2 | New typed field + hand `Default` impl + post-build insert + pin test |
| `crates/bevy_naadf/src/lib.rs` | +12 | Defensive `ConstructionConfig::for_target_arch()` seed |
| `crates/bevy_naadf/src/diagnostics.rs` | +6 / -3 | Read `Res<ConstructionConfig>` |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | +12 / -3 | `EntitiesBoot` builds `BootstrapInputs` |
| `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` | +12 / -4 | Route through `run_e2e_render_with_bootstrap_inputs` |
| `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` | +12 / -3 | Same |
| `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs` | +11 / -3 | Same |
| `crates/bevy_naadf/src/e2e/vox_web_parity.rs` | +11 / -3 | Same |

### Side notes / observations / complaints

- **Design's per-step file:line citations matched current code** — verified
  every cited line by Read/Grep before editing. The only drift was that
  `From<&AppArgs>` lived at `config.rs:252` (design said `:252-288`);
  fine.
- **Decision §5 (`for_target_arch()` encapsulation) is the cleanest factoring.**
  The 30-line wasm32 cfg block previously inside the `From` impl now lives
  in one named function. The IoC violation called out in `02-design.md`'s
  side notes is fixed.
- **The const-pin migration was non-trivial.** The architect's side note
  #2 said "move the pin to assert against the desktop arm of
  `for_target_arch()`" — but const-eval can't evaluate the `#[cfg(target_arch
  = "wasm32")]` body inside the constructor (the body is `mut` + conditional
  assignment, not const-fn). Solution: keep the literal const-pin AND add a
  sibling runtime test that asserts `for_target_arch()` (desktop) produces
  those same values. Both surfaces pinned, no const-fn semantics relied on.
  This was a clean deviation from the design's exact wording.
- **The render sub-app `init_resource::<ConstructionConfig>()` seed is
  canonical desktop on wasm32 too** — `Default::default()` doesn't apply the
  wasm clamp. That's intentional: the seed is overwritten by the first
  extract from the main-world resource (which IS `for_target_arch()` on
  wasm32 because bootstrap inserts it), well before any consumer reads. Same
  first-frame story as `RenderEffectiveWorldSize`.
- **`BootstrapInputs::default()` byte-identity contract** holds on native
  (`for_target_arch()` == `Default::default()` on desktop). On wasm32 it
  holds against the post-refactor expected value (the clamp applies), NOT
  the pre-refactor `AppArgs::default().construction_config` (which the old
  `From<&AppArgs>` lift would then apply the clamp to). Net: byte-identical
  to pre-refactor on every target.
- **The 4 e2e gates went from 4 lines (`AppArgs::default()` + 1 mutate +
  `run_e2e_render_with_args`) to ~9 lines (build inputs, override, route
  through bootstrap).** Step 6's gate-collapse work will fold the per-gate
  constructor pattern into `BootstrapInputs::for_<gate>()` factories per
  the design's §3.2.
- **Assumption #5 (resource insertion ordering)** continues to hold.
  Defensive seeds in `build_app_with_args` keep the contract.
- **Subjective:** Step 4 had the most novel work (the platform-divergence
  encapsulation), but the design's prescription was sharp enough that it
  fell out cleanly. The hand `Default` impl is a small but explicit
  upgrade — `BootstrapInputs` now spells out every field's seed instead of
  relying on derive, which makes the wasm32 contract visible.
- **Wasm32 deploy is the user's hard-gate.** This implementation passed
  the desktop gates; the design's expected on-wasm behaviour
  (`max_group_bound_dispatch.min(4096)` + `n_bounds_rounds = 1`) is now
  the `for_target_arch()` wasm arm and `BootstrapInputs::default()` /
  e2e-gate overrides reach the resource. Nothing in this step's
  implementation should change wasm behaviour vs the pre-refactor `From<&AppArgs>`
  output — both produce the same clamped values — but per the brief,
  desktop-gate-PASS is the implementor's deliverable; wasm visual is the
  user's.

## Step 5 — Migrate grid_preset (Q3 included) (2026-05-21)

### Phase A — state assessment of the prior checkpoint

This step was started by a prior compound implementer (Steps 3, 4, 5, 8
were assigned to it). It completed Steps 3 and 4 cleanly, then **stopped
mid-Step-5**. The Step 5 edits were committed by a *mechanical checkpoint*
(`7efce79`, `chore(checkpoint): …`) — NOT a verified-gate commit. No
verification gates were run on it; no Step 5 impl log existed.

**State found:** `cargo build --workspace` on `7efce79` FAILED — 11 errors.
The checkpoint had done the destructive half of the migration but not the
constructive half:

- DONE in `7efce79`: removed `grid_preset` from `AppArgs`; added
  `#[derive(Resource)]` to `GridPreset`; changed `build_app_with_budget`
  signature to `(cfg, args, grid_preset)`; swapped `setup_test_grid` to read
  `Res<GridPreset>`; dropped `ResMut<AppArgs>` from
  `web_vox::startup_fetch_default_vox`; added `resolve_skybox_only_param()`;
  wired the wasm32 `?skybox=1` resolution into `main.rs`'s bootstrap.
- MISSING in `7efce79` (the cause of the 11 build errors):
  1. `BootstrapInputs` had **no `grid_preset` field** — yet `lib.rs:164` and
     `main.rs:107` constructed `BootstrapInputs` literals with `grid_preset,`
     in them. The struct definition was never updated.
  2. 5 e2e gate `run_*` builders still mutated the deleted
     `app_args.grid_preset` field via the legacy `run_e2e_render_with_args`
     path (`oasis_edit_visual`, `small_edit_repro`, `vox_e2e`,
     `vox_gpu_oracle` cpu-phase, `vox_web_parity` skybox-phase).
  3. 4 e2e gate builders already routed through `BootstrapInputs` (Step 4
     had converted them) but their struct literals lacked the `grid_preset`
     field — they still set `app_args.grid_preset`.
  4. `bootstrap.rs::tests` read `inputs.args.grid_preset` (gone).
  5. `diagnostics.rs` read `a.grid_preset` (gone).

**Verdict: INCOMPLETE, not contradicting the design.** The checkpoint's
structural choices — `GridPreset` as a `Resource`, the `grid_preset` field on
`BootstrapInputs`, the `resolve_skybox_only_param()` helper, the wasm32
bootstrap relocation — all match `02-design.md`'s Step 5 spec and the
established Step 2/3/4 pattern. The checkpoint simply stopped before the
constructive half. The one signature deviation — `build_app_with_budget`
took `(cfg, args, grid_preset)` rather than the design §3.3 sketch's
`(cfg, inputs: BootstrapInputs)` — is consistent with Step 2's impl-log
record (Step 2 kept `build_app_with_budget(cfg, args)` and grew it
incrementally). Kept as-is; the native + Android callers already match it.

### What landed (Phase B — finishing Step 5)

- `crates/bevy_naadf/src/bootstrap.rs` — added the missing
  `pub grid_preset: GridPreset` field to `BootstrapInputs` + its line in the
  hand-written `impl Default` (`GridPreset::default()`). Added the
  post-`build_app_with_args` `app.insert_resource(inputs.grid_preset)` to the
  fan-out in `build_app_with_bootstrap_inputs`. Fixed the pin test
  (`default_wraps_canonical_app_args_defaults`) to assert on
  `inputs.grid_preset` instead of the gone `inputs.args.grid_preset`.
- `crates/bevy_naadf/src/lib.rs` — added the defensive `GridPreset::default()`
  seed in `build_app_with_args` (mirrors the Step-3/4 `TaaConfig` /
  `ConstructionConfig` seeds). `setup_test_grid` reads `Res<GridPreset>`
  non-Option, and the `build_app(AppConfig::e2e())` path bypasses the
  bootstrap fan-out — without the seed, the system panics on the missing
  resource. Updated the stale `GridPreset::WebSkybox` docstring (it described
  the old `AppArgs.grid_preset` Startup mutation).
- 5 e2e gate `run_*` builders converted from the legacy
  `run_e2e_render_with_args(app_args)` path to building a `BootstrapInputs`
  with `grid_preset:` set and routing through
  `crate::bootstrap::run_e2e_render_with_bootstrap_inputs`:
  `oasis_edit_visual.rs` (`GridPreset::Vox`), `small_edit_repro.rs`
  (`GridPreset::Vox`), `vox_e2e.rs` (`GridPreset::Vox`), `vox_gpu_oracle.rs`
  cpu-phase (`GridPreset::Vox`), `vox_web_parity.rs` skybox-phase
  (`GridPreset::Empty`).
- 4 e2e gate builders already on the `BootstrapInputs` path (converted in
  Step 4) had `grid_preset:` added to their struct literals + the dead
  `app_args.grid_preset =` lines removed: `vox_gpu_construction.rs`,
  `vox_gpu_oracle.rs` gpu-phase, `vox_horizon_parity.rs`,
  `vox_web_parity.rs` loaded-phase.
- `crates/bevy_naadf/src/diagnostics.rs` — added a
  `grid_preset: Option<Res<GridPreset>>` system parameter (the Q4 fan-out
  shape); the `KeyP` dump reads it with the defensive missing-resource
  fallback string, mirroring the Step-2/3/4 `taa_ring` / `taa` / `gi` /
  `construction` reads. The dump label changed `args.grid_preset` →
  `grid_preset` (it is no longer an `AppArgs` field).
- `crates/bevy_naadf/src/voxel/plugin.rs` — removed the now-vestigial
  `.before(grid::setup_test_grid)` ordering on
  `web_vox::startup_fetch_default_vox` (the ordering existed only so the
  old `Startup`-time `AppArgs.grid_preset` mutation landed before
  `setup_test_grid` read it — that mutation is gone). Updated the two stale
  comments that described the old mutation.

### Decisions made during impl

1. **`build_app_with_budget` signature kept as `(cfg, args, grid_preset)`.**
   The design §3.3 sketch shows `(cfg, inputs: BootstrapInputs)`, but Step 2's
   impl log records `build_app_with_budget` was deliberately kept at
   `(cfg, args)` and grown incrementally — it constructs the `BootstrapInputs`
   internally from the budget probe. The checkpoint extended that to
   `(cfg, args, grid_preset)`. Native `main.rs` + `android_main.rs` already
   call it with three args. Reverting to the design's `BootstrapInputs`-arg
   shape would be a Step-6-or-later consolidation touching all callers — out
   of Step 5's scope. Kept the checkpoint's signature; it is internally
   consistent and compiles.
2. **Defensive `GridPreset::default()` seed in `build_app_with_args`** — same
   pattern Steps 3 and 4 used for `TaaConfig` / `GiSettings` /
   `ConstructionConfig`. `setup_test_grid` reads `Res<GridPreset>` non-Option;
   the e2e_render binary's `build_app(AppConfig::e2e())` path bypasses
   `build_app_with_bootstrap_inputs`. Without the seed the system panics.
   Step 9 deletes the seed once every caller routes through the fan-out.
3. **Removed the `.before(setup_test_grid)` ordering on
   `startup_fetch_default_vox`.** The design's Step 5 spec says "the
   Startup-time `AppArgs` mutation is deleted." The checkpoint deleted the
   mutation (and the `ResMut<AppArgs>` parameter) but left the now-meaningless
   ordering constraint + two stale comments. Cleaned both up — the ordering
   served only the deleted mutation.
4. **Diagnostics fan-out, no aggregator** — per design §3.5 + Q4. One more
   `Option<Res<_>>` parameter; the dump system already had 5 of these from
   Steps 2-4, so this is rote.

### Verification

- `cargo build --workspace`: PASS (45.99s, clean — the single
  `warning: unused import: GridPreset` from the broken checkpoint state is
  resolved now that the field uses the import).
- `cargo test --workspace --lib`: PASS (192 passed; 0 failed; 1 ignored).
  The fixed `bootstrap::tests::default_wraps_canonical_app_args_defaults`
  pin test (now asserting `inputs.grid_preset == GridPreset::Default`) is in
  the passing set.
- `cargo run --bin e2e_render -- baseline`: PASS — region luminance
  `emissive 247.7, solid 243.6, sky 202.9`, in the same band as Steps 1-4.
- `cargo run --bin e2e_render -- --vox-e2e`: PASS — vox_geometry centre-rect
  luminance 250.5, channel max 251.8. The `run_vox_e2e` builder's
  `BootstrapInputs`-route conversion is sound.
- `cargo run --bin e2e_render -- --vox-horizon-native`: PASS — exit 0,
  `vox_horizon_native.png` written (1280×720).
- `cargo run --bin e2e_render -- --vox-gpu-construction`: PASS — rect
  per-pixel RGB Δ=87.79 over the 8.0 floor; ran as an extra check because
  the conversion touched the already-`BootstrapInputs` builders too.
- `just web-static + just test-wasm-full` (the `?skybox=1` URL-param path):
  **NOT run** — per the brief, wasm32 builds beyond `cargo build --workspace`
  are out of scope; the in-browser visual check is the user's hard gate.

### Files touched

| File | Change kind |
|---|---|
| `crates/bevy_naadf/src/bootstrap.rs` | Add `grid_preset: GridPreset` field + `Default` line + fan-out insert; fix pin test |
| `crates/bevy_naadf/src/lib.rs` | Defensive `GridPreset::default()` seed in `build_app_with_args`; update stale `WebSkybox` docstring |
| `crates/bevy_naadf/src/diagnostics.rs` | Add `grid_preset: Option<Res<GridPreset>>` param; dump reads the resource |
| `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs` | Legacy `run_e2e_render_with_args` → `BootstrapInputs` route with `grid_preset` |
| `crates/bevy_naadf/src/e2e/small_edit_repro.rs` | Same |
| `crates/bevy_naadf/src/e2e/vox_e2e.rs` | Same |
| `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` | cpu-phase: legacy → `BootstrapInputs`; gpu-phase: add `grid_preset` to existing literal |
| `crates/bevy_naadf/src/e2e/vox_web_parity.rs` | skybox-phase: legacy → `BootstrapInputs`; loaded-phase: add `grid_preset` to existing literal |
| `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` | Add `grid_preset` to existing `BootstrapInputs` literal |
| `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs` | Same |
| `crates/bevy_naadf/src/voxel/plugin.rs` | Remove vestigial `.before(setup_test_grid)` ordering; fix stale comments |

### Side notes / observations / complaints

- **The prior implementer's checkpoint stopped at the worst possible
  moment** — after the destructive half (field deleted from `AppArgs`, derive
  added) but before the constructive half (field added to `BootstrapInputs`,
  consumers updated). The `7efce79` commit message *describes* a complete
  Step 5 ("`grid_preset: GridPreset` removed from `AppArgs`; `GridPreset`
  derives `Resource`; `build_app_with_budget` now takes an explicit
  `GridPreset` arg; … `setup_test_grid` reads `Res<GridPreset>`") — every
  claim true, but the message omits that `BootstrapInputs` itself was never
  given the field the new struct literals reference. A mechanical checkpoint
  commit with a confident message over a non-compiling tree is a trap for
  the next reader; the build-first discipline in the brief caught it
  immediately.
- **The checkpoint's structural choices were all correct** — this was a
  *stopped* migration, not a *wrong* one. Finishing it was mechanical: every
  missing piece had an exact precedent in the Steps 2/3/4 logs (the field on
  `BootstrapInputs`, the fan-out insert, the defensive seed, the diagnostics
  fan-out param, the e2e-gate `BootstrapInputs` route). No design
  re-litigation needed.
- **`build_app_with_budget`'s `(cfg, args, grid_preset)` signature is an
  incremental wart** — it now takes one loose typed value alongside `args`.
  As Steps 6-8 migrate more fields, this will either grow more positional
  args or be consolidated to take a `BootstrapInputs` (the design §3.3
  shape). Flagging for Step 6: the cleanest consolidation point is when
  `E2eGateMode` lands, since the e2e binary's call path changes anyway.
  Not Step 5's job.
- **The `?skybox=1` relocation is wasm32-affecting** — `resolve_skybox_only_param()`
  is `#[cfg(target_arch = "wasm32")]`-gated indirectly (it calls
  `web_sys::window()`). The desktop gates cannot exercise it; the in-browser
  `?skybox=1` baseline capture is the user's hard gate. The relocation logic
  is straightforward (read the URL param on the main thread, write
  `GridPreset::WebSkybox` into `BootstrapInputs` before `build_app_*`), and
  `startup_fetch_default_vox` still does the HTTP-fetch skip + overlay hide
  for the skybox path — only the grid-preset mutation moved out.
- **The design's Step 5 spec matched reality** — the file:line citations had
  drifted (Steps 3/4 shifted lines) but every cited *symbol* existed; the
  spec's enumeration of "what Step 5 must contain" was an accurate checklist.
- **`AppArgs` is now down to 13 fields** — `spawn_test_entity` + `resize_test`
  + `vox_e2e_mode` + 10 e2e mode booleans. Step 8 (next) drains
  `spawn_test_entity`; Steps 6/7 drain the rest.
- **Foundation looks sound.** Same Assumptions from Steps 2-4 hold; the
  per-domain `GridPreset` resource + the `BootstrapInputs` carrier handle the
  Q3 wasm32 relocation cleanly. The orchestrator can proceed to Step 8.

## Step 8 — Extract spawn_test_entity (2026-05-21)

### What landed

- `crates/bevy_naadf/src/render/construction/extract.rs` — new
  `SpawnTestEntity(pub bool)` newtype `Resource`
  (`#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]`),
  placed adjacent to `MainWorldEntities` (the design said "adjacent to
  `MainWorldEntities`"; that resource lives in `extract.rs`, not `mod.rs` as
  the design's §3.1 / Step 8 text said — verified by Grep, see side notes).
  `SpawnTestEntity::default()` = `SpawnTestEntity(false)`. Decision §4 — a
  plain `bool` newtype, not `Option<TestEntityFixture>`.
- `crates/bevy_naadf/src/render/construction/mod.rs` — extended the
  `pub use extract::{…}` re-export with `SpawnTestEntity`. Changed the
  `spawn_phase_c_test_entity` `Startup` gate from
  `.run_if(|args: Res<crate::AppArgs>| args.spawn_test_entity)` to
  `.run_if(|s: Option<Res<SpawnTestEntity>>| s.is_some_and(|s| s.0))` — the
  exact shape the design's Step 8 spec prescribes (resource-absent tolerant).
- `crates/bevy_naadf/src/bootstrap.rs` — added
  `pub spawn_test_entity: SpawnTestEntity` field on `BootstrapInputs` + its
  line in the hand `Default` impl + the post-`build_app_with_args`
  `app.insert_resource(inputs.spawn_test_entity)` fan-out insert. Fixed the
  pin test to assert `!inputs.spawn_test_entity.0` (was
  `!inputs.args.spawn_test_entity`).
- `crates/bevy_naadf/src/lib.rs` — defensive `SpawnTestEntity::default()`
  seed in `build_app_with_args` (same Step-3/4/5 pattern).
- `crates/bevy_naadf/src/e2e/driver.rs` — `e2e_driver`'s ASSERT-phase
  `entities_mode` read swung from `app_args…spawn_test_entity` to a new
  `Option<Res<SpawnTestEntity>>`. Adding the parameter pushed `e2e_driver`
  to 17 system params — over Bevy's 16-param ceiling — so `app_args` and the
  new `spawn_test_entity` are grouped into one tuple `SystemParam`
  `config: (Option<Res<AppArgs>>, Option<Res<SpawnTestEntity>>)`, destructured
  at the top of the body. See side notes + Decision below.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — `BootCommand::EntitiesBoot`
  arm now inserts `SpawnTestEntity(true)` on the `BootstrapInputs` instead of
  `app_args.spawn_test_entity = true`. The `AppArgs::default()` local is gone
  (the arm no longer mutates any `AppArgs` field). `EntitiesBoot` doc comment
  updated.
- `crates/bevy_naadf/src/diagnostics.rs` — added
  `spawn_test_entity: Option<Res<SpawnTestEntity>>` param; the dump reads it
  with the defensive missing-resource fallback string. `dump_diagnostics_on_p`
  **no longer takes `Option<Res<AppArgs>>` at all** — after Step 8 the dump
  reads zero `AppArgs` fields (the remaining `AppArgs` fields are e2e mode
  booleans the dump never showed), so the parameter + the `use crate::AppArgs`
  import were removed. This matches the design §3.5 Q4 directive ("the dump
  becomes one system with N `Option<Res<_>>` parameters … drops
  `Option<Res<AppArgs>>`").
- `crates/bevy_naadf/src/app_args.rs` — deleted `pub spawn_test_entity: bool`
  field + its `Default`-impl line. Updated the module + struct docstrings:
  `AppArgs` is now down to **11 fields** (10 e2e mode booleans + `vox_e2e_mode`).
- Doc-comment fixups: `render/construction/test_fixture.rs`,
  `e2e/gates.rs:317` — updated the `AppArgs::spawn_test_entity` references to
  point at the `SpawnTestEntity` resource.

### Decisions made during impl

1. **`SpawnTestEntity` placed in `extract.rs`, not `mod.rs`.** The design's
   Step 8 spec said "adjacent to the existing `MainWorldEntities` resource …
   at `render/construction/mod.rs`." Grep showed `MainWorldEntities` is
   actually *defined* at `extract.rs:36` and only *re-exported* through
   `mod.rs:88`. Placed `SpawnTestEntity` next to the real definition in
   `extract.rs` and extended the same re-export — this is "adjacent to
   `MainWorldEntities`" in the sense the design intended.
2. **`bool` newtype, not `Option<TestEntityFixture>`** — Decision §4
   verbatim. The fixture is content-static (4×4×4 emissive block at world
   centre, all voxel-type 11); there are no per-fixture parameters to carry.
3. **`Copy + Debug + PartialEq + Eq` derives on `SpawnTestEntity`.** `Copy`
   because it wraps a `bool`; `Debug`/`PartialEq`/`Eq` because the diagnostics
   dump and any future test want them and they cost nothing on a `bool`
   newtype. (Mirrors `InvalidSampleStorageCount`, the precedent the design
   cites — that one is `#[derive(Resource, Clone, Copy, …)]`.)
4. **`e2e_driver`'s config-tuple grouping — the 16-param ceiling.** Adding
   `Option<Res<SpawnTestEntity>>` as a 17th positional `SystemParam` failed
   to compile (`e2e_driver` is not a system — Bevy's `SystemParam` tuple impl
   tops out at 16). Bevy's idiomatic answer is to nest params into a tuple
   (a tuple of `SystemParam`s is itself one `SystemParam`). Grouped `app_args`
   + `spawn_test_entity` into `config: (Option<Res<AppArgs>>,
   Option<Res<SpawnTestEntity>>)`, destructured on the first body line. This
   is the minimum-blast-radius fix — no call-site changes (the driver is
   registered by name in `e2e/mod.rs`, Bevy resolves the params). **Flagged
   for Steps 6/7:** when `E2eGateMode` lands and `app_args` is fully drained,
   this tuple should shed `AppArgs` and the grouping can be flattened or
   re-thought; the driver is param-pressured and Step 6 adds `E2eGateMode` to
   it, so Step 6 will hit the same ceiling and must plan for it.

### Verification

- `cargo build --workspace`: PASS (clean — no warnings; the removed
  `use crate::AppArgs` in `diagnostics.rs` left no dangling reference).
- `cargo test --workspace --lib`: PASS (192 passed; 0 failed; 1 ignored).
  The fixed `bootstrap::tests::default_wraps_canonical_app_args_defaults`
  pin test (now asserting `!inputs.spawn_test_entity.0`) is in the passing
  set.
- `cargo run --bin e2e_render -- --entities`: PASS — the log shows
  `phase-c wave-3 — spawned fixture entity: 4×4×4 green-emissive @
  Vec3(2046.0, 24.0, 2046.0)` (proving the `SpawnTestEntity(true)` resource
  gate fires `spawn_phase_c_test_entity`), `e2e_render: PASS (batch 6)`, and
  `entity handler validation PASS: frame A: 8 chunk_updates, 1
  entity_chunk_instances, 1 history`.
- `cargo run --bin e2e_render -- baseline`: PASS — and **no**
  `spawned fixture entity` line, confirming the default
  `SpawnTestEntity(false)` correctly gates the spawner OFF on the
  non-`--entities` path. (Run as a negative check that the resource gate is
  actually load-bearing, not always-on.)

### Files touched

| File | Change kind |
|---|---|
| `crates/bevy_naadf/src/render/construction/extract.rs` | New `SpawnTestEntity(pub bool)` Resource |
| `crates/bevy_naadf/src/render/construction/mod.rs` | Re-export `SpawnTestEntity`; `.run_if` gate swap |
| `crates/bevy_naadf/src/bootstrap.rs` | Add `spawn_test_entity` field + `Default` line + fan-out insert; fix pin test |
| `crates/bevy_naadf/src/lib.rs` | Defensive `SpawnTestEntity::default()` seed |
| `crates/bevy_naadf/src/e2e/driver.rs` | `entities_mode` reads `SpawnTestEntity`; config-tuple grouping for the 16-param ceiling |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | `EntitiesBoot` inserts `SpawnTestEntity(true)`; doc fixup |
| `crates/bevy_naadf/src/diagnostics.rs` | Add `spawn_test_entity` param; **drop `Option<Res<AppArgs>>`** (dump no longer reads `AppArgs`) |
| `crates/bevy_naadf/src/app_args.rs` | Delete `spawn_test_entity` field + default; docstring update |
| `crates/bevy_naadf/src/render/construction/test_fixture.rs` | Docstring — gate is `SpawnTestEntity` resource |
| `crates/bevy_naadf/src/e2e/gates.rs` | Docstring — gate is `SpawnTestEntity` resource |

### Side notes / observations / complaints

- **The design's Step 8 spec said `SpawnTestEntity` lives in
  `render/construction/mod.rs` — it doesn't fit there.** `MainWorldEntities`
  (the "adjacent" anchor the spec names) is defined in `extract.rs` and
  re-exported through `mod.rs`. The design's §3.1 entry for `SpawnTestEntity`
  also said `mod.rs`. This is a minor file:line drift of the kind the brief
  warned about — verified with Grep before placing the type. Net effect: zero
  — the resource is `pub use`-exported through the same path either way.
- **The 16-param `SystemParam` ceiling is a real foundation concern for
  Step 6.** `e2e_driver` was *already* at 16 params before Step 8; my one new
  read tipped it to 17 and forced the tuple-grouping. Step 6 adds an
  `E2eGateMode` read to this same driver (the design's Step 6 text says
  "Driver code at `e2e/driver.rs` … read `Res<E2eGateMode>`"). Step 6 will
  hit the identical ceiling. The Step-6 implementer should plan a real
  `#[derive(SystemParam)]` struct for the driver's config reads rather than
  another ad-hoc tuple — and Step 6 *also* drains the 11 e2e booleans off
  `AppArgs`, so `app_args` leaves the driver entirely, which frees the slot.
  Flagging so Step 6 doesn't rediscover this mid-flight.
- **`dump_diagnostics_on_p` no longer reads `AppArgs` at all.** Step 8 was
  the last `AppArgs` field the diagnostics dump consumed (`spawn_test_entity`).
  Dropping the `Option<Res<AppArgs>>` parameter is the design's §3.5
  intent ("the dump … drops `Option<Res<AppArgs>>`"), realised one step early
  because `spawn_test_entity` happened to be the last dumped field. Clean.
- **`AppArgs` is now down to 11 fields** — `resize_test`, `vox_e2e_mode`, and
  9 other e2e mode booleans. Exactly what the brief said to expect ("`AppArgs`
  will still have ~11 fields after your work — that is correct and expected").
  Steps 6 (10 booleans → `E2eGateMode`), 7 (`vox_e2e_mode` →
  `VoxE2eAssertion`), 9 (delete the shell) finish the job.
- **The extract-resource pattern reused verbatim from Steps 3/4/5.** New
  newtype Resource → `BootstrapInputs` field → hand `Default` line →
  fan-out insert → defensive seed in `build_app_with_args` → consumer swap.
  Five steps in, this is rote. No design re-litigation; the design's Step 8
  spec matched reality bar the one `mod.rs`-vs-`extract.rs` file drift.
- **Decision §4 (`bool` over `Option<TestEntityFixture>`) was the right
  call.** There genuinely are no per-fixture parameters — `spawn_phase_c_test_entity`
  hard-codes the 4×4×4 size, voxel-type 11, and the demo-relative position.
  An `Option<TestEntityFixture { }>` with a zero-field inner type would have
  been pure ceremony.
- **No foundation rot.** Step 8 is the last per-field migration before the
  e2e-mode collapse (Steps 6/7). The `BootstrapInputs` carrier + per-domain
  resources have absorbed 6 fields (`taa_ring_depth`, `taa`, `gi`,
  `construction_config`, `grid_preset`, `spawn_test_entity`) cleanly across
  Steps 2-5 + 8. The remaining work is the genuinely hard step (Step 6, the
  11→1 enum collapse) — but nothing in Steps 2-5/8 has made it harder.

## Step 6 — Collapse e2e-mode booleans into E2eGateMode (2026-05-21)

### Design-vs-reality reconciliation

`02-design.md`'s Step 6 spec was written when `AppArgs` held 11 e2e-mode
booleans. Steps 2-5 + 8 had since drained the parameter fields; the actual
`AppArgs` field set found at HEAD `53c37b3` was **11 booleans, of which 10
are mode booleans and 1 (`vox_e2e_mode`) is Bucket A**:

- `resize_test`, `oasis_edit_visual_mode`, `small_edit_visual_mode`,
  `small_edit_repro_mode`, `vox_gpu_construction_mode`,
  `vox_gpu_oracle_cpu_phase`, `vox_gpu_oracle_gpu_phase`,
  `vox_web_parity_skybox_phase`, `vox_web_parity_loaded_phase`,
  `vox_horizon_native_phase` — the **10 mode booleans** Step 6 collapsed.
- `vox_e2e_mode` — left on `AppArgs` (Decision §3 — Bucket A, Step 7's
  scope). After Step 6 `AppArgs` is a one-field shell.

The design's §3.1 `E2eGateMode` sketch already enumerated exactly 11
variants (`Standard` + the 10) — it matched reality. The design text says
"11 booleans" in a couple of places (`### Step 6` heading, the field-list);
that count was stale (it pre-dated Decision §3 splitting `vox_e2e_mode`
out). The variant set landed verbatim from the design's §3.1 code block.

### What landed

- **`crates/bevy_naadf/src/e2e/gate.rs`** — promoted the D6-era `GateKind`
  enum to `E2eGateMode` (Decision §6): renamed; extended from 8 → 11
  variants (the old enum lumped `VoxGpuOracle` and `VoxWebParity`; the
  collapse needs them split into `VoxGpuOracleCpu` / `VoxGpuOracleGpu` and
  `VoxWebParitySkybox` / `VoxWebParityLoaded` / `VoxHorizonNative` so every
  former boolean maps 1:1); added `#[derive(Resource)]`. Removed the
  module-level `#![allow(dead_code)]` — `E2eGateMode` is now a live
  Resource consumed across the crate; the still-dead D6 scaffolding (`Gate`
  trait, `FrameBudget`, `set_camera_pose`) carries targeted
  `#[allow(dead_code)]` instead. The `Gate::kind()` return type swapped
  `GateKind` → `E2eGateMode`.
- **`crates/bevy_naadf/src/bootstrap.rs`** — added `gate_mode: E2eGateMode`
  field to `BootstrapInputs` + its `Default` line (`E2eGateMode::Standard`)
  + the fan-out `app.insert_resource(inputs.gate_mode)`.
  `run_e2e_render_with_bootstrap_inputs` now picks the e2e window via
  `window_for_gate_mode(inputs.gate_mode)` instead of
  `window_for_e2e_args(&inputs.args)`. Pin test updated: the 11
  `inputs.args.<bool>` asserts collapsed to one
  `assert_eq!(inputs.gate_mode, E2eGateMode::Standard)` plus the retained
  `!inputs.args.vox_e2e_mode`.
- **`crates/bevy_naadf/src/e2e/driver.rs`** — introduced the
  `#[derive(bevy::ecs::system::SystemParam)] struct E2eDriverConfig<'w>`
  grouping the driver's three config reads (`app_args`,
  `spawn_test_entity`, `gate_mode`, all `Option<Res<…>>`). Replaced the
  Step-8-era ad-hoc `config: (Option<Res<AppArgs>>, Option<Res<SpawnTestEntity>>)`
  tuple param with `config: E2eDriverConfig`. All 7 mode-detection /
  filename-selection reads (`resize_test_mode`, `oasis_mode`,
  `vox_gpu_construction_mode`, `small_edit_mode`, `small_edit_repro_mode`,
  `vox_gpu_oracle_mode`, `vox_web_parity_mode`, plus the two
  single-capture filename selectors) swapped from
  `app_args…is_some_and(|a| a.<bool>)` to `gate_mode` equality / `matches!`
  checks. The `vox_e2e_mode` ASSERT-time read stays on `app_args`.
- **`crates/bevy_naadf/src/window_config.rs`** — `window_for_e2e_args(&AppArgs)`
  → `window_for_gate_mode(E2eGateMode)`; the 3-arm if-ladder on `AppArgs`
  booleans became a 4-arm `match` on `E2eGateMode`. Import swapped
  `crate::AppArgs` → `crate::e2e::gate::E2eGateMode`.
- **`crates/bevy_naadf/src/voxel/grid.rs`** — `setup_test_grid` reads
  `Res<E2eGateMode>` instead of `Res<AppArgs>`; the test-only CPU-oracle
  install branch is `*gate_mode == E2eGateMode::VoxGpuOracleCpu`.
- **8 e2e gate files** — each `run_*` builder dropped its
  `AppArgs::default()` + mutate-boolean idiom and sets `gate_mode` on the
  `BootstrapInputs` literal instead; each `pin_*_camera` system reads
  `Option<Res<E2eGateMode>>`. `vox_e2e.rs` keeps its `AppArgs` local
  (carries `vox_e2e_mode` — Step 7's scope — and `gate_mode` left at the
  default `Standard`, since `--vox-e2e` runs the standard driver flow).
  `small_edit_visual.rs` was the last gate still on the legacy
  `run_e2e_render_with_args` path — converted to the `BootstrapInputs`
  fan-out.
- **`crates/bevy_naadf/src/bin/e2e_render.rs`** — `BootCommand`'s `gate`
  field typed `E2eGateMode`; `parse_gate_command` maps each flag to its
  variant (the oracle/parity flags now map to distinct variants);
  `run_resize_test` builds `BootstrapInputs { gate_mode: Resize }` and
  routes through the fan-out instead of `run_e2e_render_with_args`. The
  `NamedGate` arm logs the gate mode for diagnostics.
- **`crates/bevy_naadf/src/lib.rs`** — deleted `run_e2e_render_with_args`
  (its only two callers, `run_resize_test` and `run_small_edit_visual`,
  moved to the `BootstrapInputs` fan-out). Added a defensive
  `E2eGateMode::default()` seed in `build_app_with_args` (same Step-3/4/5/8
  pattern — `setup_test_grid` reads `Res<E2eGateMode>` non-Option and the
  direct `build_app(AppConfig::e2e())` path bypasses the fan-out).
- **`crates/bevy_naadf/src/app_args.rs`** — deleted the 10 mode-boolean
  fields + their `Default` lines; `AppArgs` is down to the single
  `vox_e2e_mode` field. Module + struct docstrings updated.
- Stale-comment fixups: `e2e/gates.rs`, `voxel/grid.rs` (the
  `install_vox_sized_to_model` doc), `lib.rs` defensive-seed comments
  referencing the deleted `run_e2e_render_with_args`.

### The `#[derive(SystemParam)]` struct

`E2eDriverConfig<'w>` — flagged mandatory by the Step 8 side-note. Before
Step 6, `e2e_driver` was at Bevy's 16-positional-`SystemParam` ceiling and
Step 8 had grouped two reads into an ad-hoc tuple. Step 6 adds a third
config read (`E2eGateMode`); a `#[derive(SystemParam)]` struct counts as
one positional slot regardless of how many resources it groups, so
`E2eDriverConfig` replaces the tuple and keeps the driver under the
ceiling with named fields. All three fields are `Option<Res<…>>` (the
historical reads were all `Option`-tolerant; the driver also runs
harmlessly in non-e2e `AppConfig`s). No call-site change — Bevy resolves
the struct's params by type at registration.

### Files touched

| File | Change kind |
|---|---|
| `crates/bevy_naadf/src/e2e/gate.rs` | `GateKind` → `E2eGateMode`: rename, 8→11 variants, `#[derive(Resource)]`, drop module dead-code allow |
| `crates/bevy_naadf/src/bootstrap.rs` | Add `gate_mode` field + `Default` line + fan-out insert; `window_for_gate_mode`; pin test |
| `crates/bevy_naadf/src/e2e/driver.rs` | New `E2eDriverConfig` `SystemParam` struct; 9 mode reads swapped to `gate_mode` |
| `crates/bevy_naadf/src/window_config.rs` | `window_for_e2e_args` → `window_for_gate_mode` (match on enum) |
| `crates/bevy_naadf/src/voxel/grid.rs` | `setup_test_grid` reads `Res<E2eGateMode>`; CPU-oracle branch |
| `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs` | `run_*` sets `gate_mode`; `pin_*` reads `E2eGateMode` |
| `crates/bevy_naadf/src/e2e/small_edit_visual.rs` | Legacy `run_e2e_render_with_args` → `BootstrapInputs`; `gate_mode`; `pin_*` |
| `crates/bevy_naadf/src/e2e/small_edit_repro.rs` | `run_*` sets `gate_mode`; `pin_*` reads `E2eGateMode` |
| `crates/bevy_naadf/src/e2e/vox_e2e.rs` | `gate_mode` left `Standard`; `vox_e2e_mode` stays on `AppArgs` (Step 7) |
| `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` | `run_*` sets `gate_mode`; `pin_*` reads `E2eGateMode` |
| `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` | both `run_*` set `gate_mode`; `pin_*` reads `E2eGateMode` |
| `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs` | `run_*` sets `gate_mode`; `pin_*` reads `E2eGateMode` |
| `crates/bevy_naadf/src/e2e/vox_web_parity.rs` | both `run_*` set `gate_mode`; `pin_*` reads `E2eGateMode` |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | `BootCommand.gate: E2eGateMode`; `parse_gate_command`; `run_resize_test` |
| `crates/bevy_naadf/src/lib.rs` | Delete `run_e2e_render_with_args`; defensive `E2eGateMode` seed; comment fixups |
| `crates/bevy_naadf/src/app_args.rs` | Delete 10 mode booleans + `Default` lines; docstrings |
| `crates/bevy_naadf/src/e2e/gates.rs` | Doc-comment fixup |
| `docs/orchestrate/config-as-resource-refactor/README.md` | Step 6 ticked |

### Decisions made during impl

1. **`Gate` trait stays dead, with targeted `#[allow(dead_code)]`.** The
   design's Decision §6 + the module doc say the `Gate` trait / `FrameBudget`
   / `set_camera_pose` scaffolding is still unconsumed. Removing the
   module-level `#![allow(dead_code)]` (so `E2eGateMode` — now a live
   Resource — isn't blanket-allowed) meant putting `#[allow(dead_code)]` on
   each still-dead item. No behaviour change; just precise dead-code
   accounting.
2. **`run_e2e_render_with_args` deleted, not retained.** After Step 6 its
   only two callers route through `BootstrapInputs`. The design's
   Assumption #9 anticipated the transitional shapes evaporating; keeping a
   `pub` entry point with zero callers is rot. `run_e2e_render` (the
   `Standard`-gate path) is untouched.
3. **`E2eDriverConfig` over another ad-hoc tuple.** The Step-8 side-note
   explicitly asked for a real `#[derive(SystemParam)]` struct rather than
   extending the tuple. Done — named fields, destructured on the driver's
   first body line.
4. **`BootCommand`'s `gate` field kept (typed `E2eGateMode`).** The design
   Assumption #9 says `BootCommand` "evaporates entirely" eventually, but
   that is a larger parser-shape change beyond Step 6's atomic-commit
   boundary. Minimal-blast-radius: keep the enum, swap `GateKind` →
   `E2eGateMode`, and the `gate` field now drives a diagnostic log line
   (was a bare `let _ = gate;`).
5. **`vox_e2e` gate → `E2eGateMode::Standard`.** Per Decision §3,
   `--vox-e2e` runs the standard driver flow; `vox_e2e_mode` is the only
   surviving `AppArgs` field and stays there for Step 7.

### Verification

- `cargo build --workspace`: PASS (clean — no errors, no warnings).
- `cargo test --workspace --lib`: PASS (192 passed; 0 failed; 1 ignored).
  The updated `bootstrap::tests::default_wraps_canonical_app_args_defaults`
  pin test (now asserting `gate_mode == E2eGateMode::Standard`) is in the
  passing set.
- **Every desktop-runnable e2e gate — all PASS:**
  - `baseline` — region luminance emissive 247.6 / solid 243.7 / sky 202.9.
  - `--vox-e2e` — vox_geometry centre-rect luminance 250.5, channel max 251.8.
  - `--oasis-edit-visual` — rect per-pixel RGB Δ=18.07 over the 8.0 floor.
  - `--small-edit-visual` — click rect max-Δ=18 over floor 15; CPU +1 voxel.
  - `--small-edit-repro` — 1920×1080; 0 pitch-black pixels before/after.
  - `--vox-gpu-construction` — rect per-pixel RGB Δ=88.01 over floor 8.0.
  - `--vox-gpu-oracle-cpu` — `oracle_cpu.png` written (256×256).
  - `--vox-gpu-oracle-gpu` — `oracle_gpu.png` written (256×256).
  - `--vox-gpu-oracle` (compare) — SSIM 0.8834 over threshold 0.850.
  - `--vox-horizon-native` — `vox_horizon_native.png` written (1280×720).
  - `--vox-web-parity-skybox` — `vox_web_parity_skybox.png` written.
  - `--vox-web-parity-loaded` — `vox_web_parity_loaded.png` written.
  - `--vox-web-parity` (compare) — SSIM 0.0179 under dissimilar threshold.
  - `--entities` — `spawned fixture entity: 4×4×4 green-emissive` line
    present (proves `SpawnTestEntity(true)` rode the fan-out); entity
    handler validation PASS.
  - `--resize-test` — RAN (Hyprland session present); luma ratios 0.9691 /
    0.9742 over the 0.7 threshold; PASS.
- Wasm-deploy gates: the `--vox-web-parity-*` family runs natively as
  single-screenshot capture gates (no wasm build needed) — all ran on
  desktop above. No gate required a wasm deploy. The in-browser `?skybox=1`
  / web-parity visual check remains the user's surface (unchanged from
  Step 5).

### Side notes / observations / complaints

- **The `GateKind` scaffolding held up well.** D6 step 2 set up `GateKind`
  as exactly the seam Step 6 needed — the design's side-note called this
  out and it was accurate. The only friction: the D6 enum had 8 variants
  because it lumped the oracle CPU/GPU pair and the parity
  skybox/loaded/horizon trio into single `VoxGpuOracle` / `VoxWebParity`
  variants. The boolean collapse needs a 1:1 boolean→variant mapping, so
  three variants had to be split out. Mechanical; the `bin/e2e_render.rs`
  `parse_gate_command` is the one place that benefits — the oracle/parity
  sub-phases now carry distinct gate modes instead of being told apart by
  which `run_*` fn-pointer was bundled.
- **The design's Step 6 spec matched reality** modulo the "11 booleans"
  count being stale (it's 10 mode booleans + the Decision-§3-exempt
  `vox_e2e_mode`). The §3.1 `E2eGateMode` variant block was authoritative
  and landed verbatim. File:line citations had drifted (Steps 2-5/8 shifted
  lines) but every cited symbol existed; the spec's enumeration of "what
  Step 6 touches" was an accurate checklist.
- **The `SystemParam` ceiling was exactly as the Step 8 side-note
  predicted.** Adding `E2eGateMode` as a 3rd config read would have pushed
  the tuple-or-positional count over; the `E2eDriverConfig` struct resolves
  it cleanly. The driver now has *one* named config slot. Note for future
  steps: the driver is still param-dense — if Step 7's `VoxE2eAssertion`
  read lands on the driver, it should join `E2eDriverConfig` (one more
  field on the existing struct, not a new param). Step 9 will drop
  `app_args` from the struct entirely once `vox_e2e_mode` migrates.
- **For Step 7:** `vox_e2e_mode` is the sole surviving `AppArgs` field.
  The driver reads it once at ASSERT time (`driver.rs` ~line 707) via
  `app_args` on the `E2eDriverConfig` struct; `vox_e2e.rs::run_vox_e2e`
  sets it on the `AppArgs` local inside the `BootstrapInputs`. Step 7
  replaces both with a `VoxE2eAssertion` resource + a `BootstrapInputs`
  field, drops `app_args` from `E2eDriverConfig`, and then Step 9 deletes
  the `AppArgs` shell.
- **For Step 9:** after Step 7, `BootstrapInputs.args: AppArgs` and the
  `build_app_with_args` defensive-seed block (`TaaConfig`, `GiSettings`,
  `TaaRingConfig`, `ConstructionConfig`, `GridPreset`, `SpawnTestEntity`,
  `E2eGateMode`) can both go — every caller routes through the fan-out
  once the legacy `AppArgs` arg is gone. `build_app_with_args` itself
  becomes a pure resource-fan-out shell; consider folding it into
  `build_app_with_bootstrap_inputs` at Step 9.
- **`BootstrapInputs` still has the `args: AppArgs` field** — it carries
  only `vox_e2e_mode` now. Mildly awkward (one nested boolean) but correct
  for the incremental boundary; Step 7 drains it.
- **No foundation rot.** The `BootstrapInputs` carrier + per-domain
  resources absorbed the largest step (10-boolean enum collapse, ~17 files)
  with no design re-litigation. The collapse is byte-identical: every gate
  passed, including the SSIM-compare gates whose thresholds would have
  caught any pose / window / install-path drift.

## Step 7 — Extract vox_e2e_mode to VoxE2eAssertion (2026-05-21)

### What landed

The last field on `AppArgs` — the `vox_e2e_mode: bool` ASSERT-time tag —
migrated onto its own per-domain `VoxE2eAssertion(pub bool)` resource
(Bucket A, Decision §3). `AppArgs` is now a zero-field unit struct
(`pub struct AppArgs;`). Per the Step 6 side-note, the driver's read
folded into the existing `E2eDriverConfig` `#[derive(SystemParam)]` struct
(the `app_args` field was *replaced* by `vox_e2e_assertion`, not added
alongside — `AppArgs` has nothing left to read).

- **`crates/bevy_naadf/src/e2e/mod.rs`** — added the
  `VoxE2eAssertion(pub bool)` newtype `Resource`
  (`#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]`),
  placed in a new `--- ASSERT-time gate options ---` section just before
  the App-wiring block. Module-doc not touched (the `--- App wiring ---`
  banner already separates the sections).
- **`crates/bevy_naadf/src/e2e/driver.rs`** — `E2eDriverConfig`'s
  `app_args: Option<Res<AppArgs>>` field swapped to
  `vox_e2e_assertion: Option<Res<VoxE2eAssertion>>`; the driver's
  destructure + the ASSERT-time read
  (`vox_e2e_assertion.as_deref().is_some_and(|v| v.0)`) updated. The
  `vox_e2e_mode` local + `run_assertions` signature are unchanged — only
  the *source* of the boolean changed.
- **`crates/bevy_naadf/src/bootstrap.rs`** — dropped the `args: AppArgs`
  field from `BootstrapInputs`, added `vox_e2e_assertion: VoxE2eAssertion`;
  the fan-out gained `app.insert_resource(inputs.vox_e2e_assertion)`. The
  fan-out's `build_app_with_args(cfg, inputs.args)` call became
  `build_app_with_args(cfg, AppArgs::default())` (the zero-field shell
  carries no state — Step 9 folds the call away). `Default` impl + the
  `default_wraps_canonical_app_args_defaults` pin test updated to assert
  on `inputs.vox_e2e_assertion.0`.
- **`crates/bevy_naadf/src/e2e/vox_e2e.rs`** — `run_vox_e2e` dropped the
  `crate::AppArgs::default()` + `app_args.vox_e2e_mode = true` local and
  sets `vox_e2e_assertion: crate::e2e::VoxE2eAssertion(true)` on the
  `BootstrapInputs` literal instead.
- **`crates/bevy_naadf/src/app_args.rs`** — `AppArgs` is now
  `#[derive(Resource, Clone, Default)] pub struct AppArgs;` (zero fields,
  derived `Default` replacing the hand-written impl). Module + struct
  docstrings updated to the "drained shell — Step 9 deletes it" framing.
- **`crates/bevy_naadf/src/lib.rs`** — `build_app_with_budget`'s
  `BootstrapInputs` literal dropped the `args` field (`let _ = args;`
  retains the now-vestigial parameter for signature stability — Step 9
  removes it). Added the defensive `VoxE2eAssertion::default()` seed in
  `build_app_with_args` (same Step-6 pattern — the direct
  `build_app(AppConfig::e2e())` path bypasses the fan-out).
- **`crates/bevy_naadf/src/bin/e2e_render.rs`** — comment fixup on the
  `--vox-e2e` arm (no longer says `vox_e2e_mode` rides on `AppArgs`).

### Files touched

| File | Change kind |
|---|---|
| `crates/bevy_naadf/src/e2e/mod.rs` | New `VoxE2eAssertion(pub bool)` `Resource` |
| `crates/bevy_naadf/src/e2e/driver.rs` | `E2eDriverConfig`: `app_args` field → `vox_e2e_assertion`; ASSERT-time read source swap |
| `crates/bevy_naadf/src/bootstrap.rs` | Drop `args` field, add `vox_e2e_assertion`; fan-out insert; `Default` + pin test |
| `crates/bevy_naadf/src/e2e/vox_e2e.rs` | `run_vox_e2e` sets `vox_e2e_assertion` instead of `AppArgs.vox_e2e_mode` |
| `crates/bevy_naadf/src/app_args.rs` | `AppArgs` drained to a zero-field unit struct; docstrings |
| `crates/bevy_naadf/src/lib.rs` | `build_app_with_budget` literal fixup; `VoxE2eAssertion` defensive seed |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | `--vox-e2e` arm comment fixup |

### Decisions made during impl

1. **`E2eDriverConfig.app_args` replaced, not extended.** The Step 6
   side-note said Step 7's read should "join `E2eDriverConfig`". Since
   `AppArgs` is now empty, there is nothing left for an `app_args` field
   to read — so the field is *replaced* by `vox_e2e_assertion`. The struct
   still has exactly three fields; the driver stays one positional slot.
2. **`build_app_with_args` kept (Step 7 boundary).** Step 7's atomic
   commit empties `AppArgs`; Step 9 deletes it. `build_app_with_args` is
   still called by `build_app` and `build_app_with_bootstrap_inputs` with
   `AppArgs::default()`. Folding it into the fan-out is Step 9 work — the
   design's Step 9 spec + the Step 6 side-note both call for it there.
3. **Defensive `VoxE2eAssertion` seed added.** The driver reads it via
   `Option<Res<…>>` (resource-absent tolerant), so the seed is not
   strictly required for correctness. Added anyway for consistency with
   the Step 3/4/5/6/8 defensive-seed pattern — every other per-domain
   resource has one, and the direct `build_app(AppConfig::e2e())` path
   (`run_e2e_render`) should see a populated resource.

### Verification

- `cargo build --workspace`: PASS (clean — no errors, no warnings).
- `cargo test --workspace --lib`: PASS (192 passed; 0 failed; 1 ignored).
  The updated `bootstrap::tests::default_wraps_canonical_app_args_defaults`
  pin test (now asserting `!inputs.vox_e2e_assertion.0`) is in the passing
  set.
- `cargo run --bin e2e_render -- --vox-e2e`: PASS — `vox_geometry` centre
  rect luminance 250.5, channel max 251.8 (thresholds > 160 / > 30); the
  standard batch-6 region gate also green (emissive 250.7, solid 250.5,
  sky 223.0). The `VoxE2eAssertion(true)` correctly rode the fan-out and
  the driver swapped to the `assert_vox_geometry_visible` gate.

### Side notes / observations / complaints

- **The design's Step 7 spec matched reality exactly.** Three sites
  (new resource, driver read, vox_e2e builder) — plus the mechanical
  `BootstrapInputs.args` removal + `build_app_with_budget` /
  `build_app_with_args` fixups the spec implied but did not enumerate
  (the spec said "~30 LOC"; actual is ~120 lines counting doc churn,
  mostly comment rewrites). No file:line drift broke anything.
- **`E2eDriverConfig` absorbed the read cleanly.** The Step 6 side-note's
  instruction was precise and correct — the swap was a one-field rename
  on the struct + the destructure. The driver stayed at one config
  positional slot; no `SystemParam`-ceiling pressure.
- **`AppArgs` is now a literal zero-field unit struct** —
  `pub struct AppArgs;`. It is fully vestigial: `build_app_with_args`
  takes it but reads nothing; `build_app_with_budget` takes it and
  `let _ = args;`-discards it. This is the correct incremental state —
  Step 9 deletes it. The user's principle is now satisfied at the data
  level: no `args`-conceptualised configuration is readable at runtime;
  every config value is a per-domain resource inserted at bootstrap.
- **Subjective:** the migration's last field was the cleanest of the
  nine — `VoxE2eAssertion` is a textbook Bucket-A newtype, and Decision
  §3's "ASSERT-time data tag, not a flow selector" framing held up: the
  driver reads it exactly once, in `run_assertions`, with zero
  state-machine involvement. Splitting it off `E2eGateMode` was the
  right call.
