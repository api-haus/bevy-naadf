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
