# 02d — Design: CLI rearch + e2e drives actual main

Group: consolidated (design → independent self-review → implementation → log).
HEAD at design time: `607340c` (Phase 2.6 checkpoint).

User directive (verbatim):

> "obviously e2e path is once again not e2e
>
> it literally must do:
>
> void main_e2e() {
>   begin_running_actual_main();
>   control_actual_main();
> }
>
> if its shape is not like that - rewrite it to hell.
>
> run compounded implementation agent right away."

The two concrete bugs to fix:

- **Problem 1** — `cargo run --release --bin bevy-naadf -- --grid-preset
  procedural-streaming --vram-budget-mib 1024 --max-segments-per-frame 4 --noise-seed
  1337` boots the **default** scene because `crates/bevy_naadf/src/main.rs:29-49`
  parses **only** `--vox <path>`; every other AppArgs field stays at default
  regardless of CLI. `--help` doesn't print clap help either — `argv` is just
  iterated by hand.
- **Problem 2** — `crates/bevy_naadf/src/bin/e2e_render.rs:71-355` has a 180-line
  short-circuit dispatch ladder where each branch builds a **fresh
  `AppArgs::default()` internally** (see e.g. `streaming_window.rs:362-372`,
  `noise_static_world.rs:230-240`, `oasis_edit_visual.rs:203-211`) — user-supplied
  flags on the `e2e_render` invocation are ignored by every gate. The shape is
  "build a parallel App for this gate," not "start the real App and observe it."

## Design

### A. AppArgs full clap wiring

`AppArgs` lives at `crates/bevy_naadf/src/lib.rs:289-463`. Every field is enumerated
below with its CLI long form and default. The struct currently has no `clap` derive
at all; both binaries do `std::env::args()` manual scanning.

**Decision:** add `clap` (derive feature) as a workspace-root version-pin (workspace
already has `serde`, `thiserror`, `ron`, `bytemuck` direct deps). Pin `clap = "4"`
with `derive` feature. The derive `Parser` lives on a new struct
`bevy_naadf::cli::Cli` (NOT on `AppArgs` itself — `AppArgs` is a `Resource` consumed
by Bevy systems with many fields that shouldn't be CLI-exposed, like the deep
`gi: GiSettings` nest). `Cli` carries a flat set of CLI-relevant fields, then
exposes a method `Cli::into_app_args() -> AppArgs` (or `into_app_args_and_gate()`
for the e2e binary) that mutates a `AppArgs::default()` to reflect the CLI.

This keeps `AppArgs::default()` as the source of truth for defaults while making
each CLI override an explicit field assignment in one place
(`Cli::into_app_args`). Every CLI flag has a default that matches
`AppArgs::default()`.

#### A.1 CLI surface — long form, default, and target AppArgs field

The clap `Cli` carries (group: streaming-world session relevance bolded):

| Long flag | Type | Default | Target AppArgs field | Notes |
|---|---|---|---|---|
| `--grid-preset` | `GridPresetArg` (clap `ValueEnum`) | `Default` | `grid_preset` | See § A.2 for variant handling |
| `--vox <path>` | `Option<PathBuf>` | `None` | `grid_preset` | When present, overrides `--grid-preset` → forces `Vox { path }`. Preserves the existing `--vox <path>` UX |
| **`--noise-preset <N>`** | `u32` | `0` | feeds into `ProceduralStreaming`/`ProceduralStatic` variant payloads | |
| **`--noise-seed <I>`** | `i32` | `1337` | `noise_seed` + variant payload | |
| **`--sea-level <F>`** | `f32` | `256.0` (= `WORLD_SIZE_IN_VOXELS.y * 0.5`) | `sea_level` | |
| **`--terrain-amplitude <F>`** | `f32` | `64.0` | `terrain_amplitude` | |
| **`--vram-budget-mib <N>`** | `u32` | `1024` | `vram_budget_mib` | |
| **`--max-segments-per-frame <N>`** | `u32` | `4` | `max_segments_per_frame` | |
| `--taa` / `--no-taa` | `bool` | `true` | `taa` | clap `action = SetTrue/SetFalse` |
| `--taa-ring-depth <N>` | `u32` | `32` (DEFAULT_TAA_RING_DEPTH) | `taa_ring_depth` | |
| `--spawn-test-entity` | `bool` flag | `false` | `spawn_test_entity` | |
| `--resize-test` | `bool` flag | `false` | `resize_test` | e2e-only knob (but exposed via shared CLI for uniformity) |
| `--vox-e2e-mode` | `bool` flag | `false` | `vox_e2e_mode` | |
| `--gpu-construction` / `--no-gpu-construction` | `bool` | `true` | `construction_config.gpu_construction_enabled` | |
| `--entities-enabled` | `bool` flag | `false` | `construction_config.entities_enabled` | |

`AppArgs` also carries the per-gate **mode** flags (`streaming_window_mode`,
`noise_static_mode`, `oasis_edit_visual_mode`, `small_edit_visual_mode`,
`small_edit_repro_mode`, `vox_gpu_construction_mode`, `vox_gpu_oracle_cpu_phase`,
`vox_gpu_oracle_gpu_phase`). These are NOT user-facing CLI flags on the
interactive binary — they're set by the e2e binary's `--gate` selection. The
interactive binary never sets them; default is `false` everywhere.

The e2e binary additionally accepts `--gate <NAME>` (see § C) and a few
e2e-only top-level mode flags retained for backward-compat with the existing
`--validate-gpu-construction`, `--edit-mode`, `--runtime-edit-mode`,
`--entities` invocation shape (see § D).

#### A.2 `GridPreset` enum — clap ValueEnum + struct-variant population

`GridPreset` (`lib.rs:67-108`) is:

```rust
pub enum GridPreset {
    Default,
    Vox { path: PathBuf },
    ProceduralStreaming { noise_preset: u32, seed: i32 },
    ProceduralStatic { noise_preset: u32, seed: i32 },
}
```

**Decision:** clap doesn't auto-derive `ValueEnum` for struct-variant enums.
Introduce a parallel **CLI-only** enum `GridPresetArg`:

```rust
#[derive(clap::ValueEnum, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum GridPresetArg {
    #[default]
    Default,
    Vox,                  // requires --vox <path>
    ProceduralStreaming,
    ProceduralStatic,
}
```

`Cli::into_app_args()` matches on `(grid_preset_arg, vox_path, noise_preset,
noise_seed)` to build the matching `GridPreset` variant:

- `Default` → `GridPreset::Default`
- `Vox` → requires `--vox <path>` present; otherwise hard error
  ("--grid-preset vox requires --vox <path>"). When `--vox` is present
  without `--grid-preset`, default to `Vox` for backward-compat (preserves
  today's `cargo run --release --bin bevy-naadf -- --vox <path>` UX).
- `ProceduralStreaming` → `GridPreset::ProceduralStreaming { noise_preset,
  seed: noise_seed }`
- `ProceduralStatic` → `GridPreset::ProceduralStatic { noise_preset, seed:
  noise_seed }`

The clap `ValueEnum` formats these as kebab-case (`default`, `vox`,
`procedural-streaming`, `procedural-static`) — verified against the user's
intended invocation `--grid-preset procedural-streaming`.

#### A.3 Field-vs-variant mapping rationale (rejected alternatives in § Decisions)

The streaming-world variant payloads (`noise_preset`, `seed`) are duplicated
between the variant struct (`GridPreset::ProceduralStreaming { noise_preset,
seed }`) and the flat `AppArgs::noise_seed` field. The duplication exists
because Phase 2 / 2.4 / 2.6 added fields incrementally and never reconciled.

**Resolved:** the variant struct stays (it's the install-path input shape that
`voxel/grid.rs:122-127` consumes); `AppArgs::noise_seed` is the flat CLI-feeder
field. `Cli::into_app_args()` reads `noise_seed` once from CLI and copies it
into BOTH (a) `args.noise_seed` and (b) the matching variant's `seed` field.
Same for `noise_preset` (although that one only lives in the variants today —
add a flat `noise_preset: u32` field to `AppArgs` for CLI symmetry; default
`0`). This is the minimum-disruption fix; a future cleanup could collapse the
duplication, but it's NOT in this brief's scope.

### B. `bevy-naadf` interactive binary refactor

#### B.1 Current state (problem)

`crates/bevy_naadf/src/main.rs`:

```rust
fn main() -> AppExit {
    let mut args = AppArgs::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(idx) = argv.iter().position(|a| a == "--vox") {
        if let Some(path) = argv.get(idx + 1) {
            args.grid_preset = GridPreset::Vox { path: path.into() };
        } else {
            eprintln!("error: --vox flag requires a path argument");
            return AppExit::error();
        }
    }
    build_app_with_args(AppConfig::windowed(), args).run()
}
```

Every other CLI flag is silently dropped. `--help` boots the app.

#### B.2 Target state

```rust
use bevy::prelude::AppExit;
use bevy_naadf::cli::Cli;
use bevy_naadf::{build_app_with_args, AppConfig};
use clap::Parser;

fn main() -> AppExit {
    let cli = Cli::parse();   // clap prints --help and exits if requested
    let args = cli.into_app_args();
    build_app_with_args(AppConfig::windowed(), args).run()
}
```

The interactive binary's entire job is "parse CLI → AppArgs → build_app → run."
The four interactive scene-install paths (`Default` / `Vox` / `ProceduralStreaming`
/ `ProceduralStatic`) all flow through `voxel/grid.rs::setup_test_grid`'s
`match &args.grid_preset { ... }` ladder, which already handles every variant.

### C. `e2e_render` refactor — drive actual main

#### C.1 Current state (problem)

The dispatch ladder at `bin/e2e_render.rs:71-355` works like:

```rust
let streaming_window_mode = args.iter().any(|a| a == "--streaming-window");
// (15 more similar lines)

if vox_gpu_oracle_mode { return ExitCode::from(run_vox_gpu_oracle_compare()); }
if wgsl_noise_oracle_mode { return run_wgsl_noise_oracle_gate(); }
if validate_gpu_construction_scaled { ... return ... }
if validate_gpu_construction_production { ... return ... }

let app_exit = if resize_test {
    let mut app_args = AppArgs::default();   // ← rebuilds default every gate
    app_args.resize_test = true;
    run_e2e_render_with_args(app_args)
} else if oasis_edit_visual_mode {
    run_oasis_edit_visual()                  // ← builds AppArgs::default() inside
} else if streaming_window_mode {
    run_streaming_window()                    // ← same
} else if noise_static_mode {
    run_noise_static_world()                  // ← same
} ...
```

The `--streaming-window` invocation pipes through `run_streaming_window()` which
constructs `AppArgs::default()` and overrides three fields — `vram_budget_mib`,
`max_segments_per_frame`, `noise_seed` from CLI are LOST.

#### C.2 Target state — single shared parser + composition

`crates/bevy_naadf/src/cli.rs` exposes a second parser type `E2eCli`:

```rust
#[derive(clap::Parser)]
pub struct E2eCli {
    #[command(flatten)]
    pub app: Cli,                // every interactive flag composes

    /// Which e2e gate to run. Each gate is a Bevy plugin / driver-state branch
    /// attached on top of the SAME `build_app_with_args(AppConfig::e2e(), args)`
    /// chain the interactive binary uses; the gate flag's effect is to set the
    /// matching `AppArgs::*_mode` field + (when present) provide a default
    /// preset / camera pose if the user didn't override.
    #[arg(long, value_enum)]
    pub gate: Option<Gate>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Baseline,
    StreamingWindow,
    NoiseStaticWorld,
    WgslNoiseOracle,
    ValidateGpuConstruction,
    ValidateGpuConstructionScaled,
    ValidateGpuConstructionProduction,
    Entities,
    EditMode,
    RuntimeEditMode,
    ResizeTest,
    VoxE2e,
    OasisEditVisual,
    SmallEditVisual,
    SmallEditRepro,
    VoxGpuConstruction,
    VoxGpuOracle,
    VoxGpuOracleCpu,
    VoxGpuOracleGpu,
}
```

`E2eCli::into_app_args_and_gate(self) -> (AppArgs, Option<Gate>)`:

1. `let mut args = self.app.into_app_args()` — user-supplied CLI fields land
   exactly as the interactive binary would consume them.
2. `match self.gate { Some(Gate::X) => apply_gate_defaults_x(&mut args), ... }`
   — each gate's "apply defaults" function fills in the gate's preset / mode
   flag IF the corresponding user value is still at AppArgs default. E.g.
   `Gate::StreamingWindow` sets:
   ```rust
   if matches!(args.grid_preset, GridPreset::Default) {
       args.grid_preset = GridPreset::ProceduralStreaming {
           noise_preset: args.noise_preset,
           seed: args.noise_seed,
       };
   }
   args.streaming_window_mode = true;
   args.oasis_edit_visual_mode = true;   // routes via OasisXxx state machine
   ```
3. Return `(args, self.gate)`.

#### C.3 Top-level e2e binary main

```rust
fn main() -> ExitCode {
    let cli = E2eCli::parse();
    let (args, gate) = cli.into_app_args_and_gate();

    // The four headless / multi-process gates short-circuit before App boot —
    // each is pure-compute / runs subprocesses of itself. They are NOT a
    // "parallel reality" — they're explicitly NOT App-based. Preserve as-is.
    match gate {
        Some(Gate::VoxGpuOracle) =>
            return ExitCode::from(e2e::vox_gpu_oracle::run_vox_gpu_oracle_compare()),
        Some(Gate::WgslNoiseOracle) =>
            return e2e::wgsl_noise_oracle::run_wgsl_noise_oracle_gate(),
        Some(Gate::ValidateGpuConstructionScaled) =>
            return run_validate_scaled(),
        Some(Gate::ValidateGpuConstructionProduction) =>
            return run_validate_production(),
        _ => {}
    }

    // Resize-test pre-launch hyprctl windowrule install — preserved verbatim;
    // unchanged behavioural surface. Gated on `gate == Some(Gate::ResizeTest)`.
    if matches!(gate, Some(Gate::ResizeTest)) {
        install_hyprland_windowrule();
    }

    // THE ONE PATH that drives the actual app — every other gate composes onto
    // these same args:
    let app_exit = run_e2e_render_with_args(args.clone());

    let e2e_code = match app_exit {
        AppExit::Success => 0u8,
        AppExit::Error(c) => c.get(),
    };

    // Post-run validation gates (these run AFTER the App exits — they spin a
    // separate headless render world for byte-equality checks). Gated on the
    // matching `Gate` variant.
    if matches!(gate, Some(Gate::ValidateGpuConstruction)) {
        match validate_gpu_construction() { ... }
    }
    if matches!(gate, Some(Gate::Entities)) {
        match validate_entity_handler() { ... }
    }
    if matches!(gate, Some(Gate::EditMode)) {
        match validate_edit_mode() { ... }
    }
    if matches!(gate, Some(Gate::RuntimeEditMode)) {
        match validate_runtime_edit_mode() { ... }
    }
    if matches!(gate, Some(Gate::ResizeTest)) {
        cleanup_hyprland_windowrule();
    }

    ExitCode::from(e2e_code)
}
```

#### C.4 Per-gate `run_X()` functions — deprecated, but kept

The current `run_streaming_window()` / `run_noise_static_world()` /
`run_oasis_edit_visual()` etc. construct `AppArgs::default()` internally — this
is the parallel reality. **Decision:** delete the `AppArgs::default()`
construction from each `run_X()`. Refactor each to a thin `apply_gate_defaults_X(args:
&mut AppArgs)` function that fills in the gate's preferred fields when the
user didn't override. The gate's "drive the actual App" path is now the
top-level e2e main, which calls `run_e2e_render_with_args(args)` ONCE.

Concretely, the `run_X()` functions are kept as thin wrappers that
construct a fresh AppArgs (preserving the existing API surface for any unit
tests that call them) but the **top-level e2e binary stops using them** —
it composes its own args via the clap parser + gate defaults. The latch
resets + log prints that currently live in `run_X()` move into the gate's
"apply defaults" function so they fire on the user's invocation path too.

### D. e2e gates that force AppArgs settings — composition pattern

Per the brief's § D guidance, picking **pattern 2 with gate defaults**: each
gate has an `apply_gate_defaults_X(&mut AppArgs)` function. The user can:

1. Run `cargo run --release --bin e2e_render -- --gate streaming-window` →
   gate defaults install the `ProceduralStreaming` preset + default budget.
   This matches the canonical gate invocation.
2. Run `cargo run --release --bin e2e_render -- --gate streaming-window
   --vram-budget-mib 2048 --noise-seed 42` → user's CLI overrides are
   respected; gate fills only what user didn't supply.
3. Run `cargo run --release --bin e2e_render -- --grid-preset
   procedural-streaming --vram-budget-mib 2048` (no `--gate`) → boots the
   streaming world without ANY e2e gate observer — just the interactive
   App in the e2e window config (small fixed window, synchronous pipeline
   compile, e2e camera). That's a useful debugging mode.

The "gate fills only what user didn't supply" semantics: each
`apply_gate_defaults_X` checks `if matches!(args.grid_preset,
GridPreset::Default)` before assigning a preset, etc. The user's CLI
override always wins. Mode flags (e.g. `args.streaming_window_mode = true`)
are ALWAYS set by the gate — they're the gate's "observer attachment",
not a user-tunable knob.

### E. Migration plan — file by file

Implementation order (each step compiles + tests pass at completion):

1. **Add `clap` dep** to `crates/bevy_naadf/Cargo.toml` (workspace root has
   no direct deps section; add to the bevy_naadf crate manifest only,
   matching the project's local-deps convention).
2. **Create `crates/bevy_naadf/src/cli.rs`** — `Cli` (interactive) +
   `GridPresetArg` (ValueEnum) + `Cli::into_app_args()`. Single
   re-export `pub mod cli;` in `lib.rs`. Add `noise_preset: u32` flat field
   to `AppArgs` (default `0`).
3. **Rewrite `crates/bevy_naadf/src/main.rs`** — clap parse + build_app +
   run, ~10 LOC total.
4. **Verify with `cargo build`** + `cargo test --workspace --lib`.
5. **Extend `cli.rs`** — add `Gate` enum + `E2eCli` + `E2eCli::into_app_args_and_gate()`.
   Add per-gate `apply_gate_defaults_X` functions (private to `cli.rs`).
6. **Rewrite `crates/bevy_naadf/src/bin/e2e_render.rs`** — clap parse +
   gate match + `run_e2e_render_with_args(args)`. The headless / multi-process
   short-circuit gates (vox-gpu-oracle, wgsl-noise-oracle,
   validate-gpu-construction-scaled, validate-gpu-construction-production)
   keep their pre-App return paths verbatim. The post-App validation
   passes (validate-gpu-construction, entities, edit-mode, runtime-edit-mode)
   keep their post-`app_exit` shape but gate-flag-driven.
7. **Refactor `run_X()` in each gate file** — keep the function (some
   tests may reference it; verify with grep) but make it a thin wrapper:
   construct AppArgs::default(), call `apply_gate_defaults_X(&mut args)`,
   then `run_e2e_render_with_args(args)`. Remove the duplicated default
   construction logic.
8. **Verify**:
   - `cargo build --workspace --release` clean.
   - `cargo test --workspace --lib --release` (≥232 passing per Phase 2.6).
   - `cargo run --release --bin bevy-naadf -- --help` prints clap help.
   - Smoke-launch each preset (15s timeout, expect 124 from `timeout`):
     - `cargo run --release --bin bevy-naadf -- --grid-preset
       procedural-streaming --vram-budget-mib 1024`.
     - `cargo run --release --bin bevy-naadf -- --grid-preset
       procedural-static`.
     - `cargo run --release --bin bevy-naadf --` (default).
   - Each e2e gate via `--gate <NAME>` (5 priority gates per brief).

### F. Don't touch

- WGSL shaders (Phase 1 / 2 / 2.4 / 2.6 deliverables).
- Strict gate thresholds in `e2e/streaming_window.rs`, `e2e/noise_static_world.rs`.
- The faithful-port rule — no new C# divergences.
- `setup_test_grid`'s install logic (the four-variant match).
- The `OasisXxx` driver state machine.
- The headless gates' compute-only flow (wgsl-noise-oracle,
  validate-gpu-construction-scaled, validate-gpu-construction-production)
  — they intentionally don't boot an App.

### G. File-level diff sketch

| Action | Path | Approx LOC | Purpose |
|---|---|---|---|
| edit | `crates/bevy_naadf/Cargo.toml` | +1 | `clap = { version = "4", features = ["derive"] }` |
| new | `crates/bevy_naadf/src/cli.rs` | ~220 | `Cli`, `E2eCli`, `Gate`, `GridPresetArg`, `into_app_args*` |
| edit | `crates/bevy_naadf/src/lib.rs` | +3 | `pub mod cli;` + `AppArgs::noise_preset: u32` (default 0) |
| edit | `crates/bevy_naadf/src/main.rs` | -40 / +12 | drop manual parse, clap parse + build_app |
| edit | `crates/bevy_naadf/src/bin/e2e_render.rs` | -180 / +130 | clap parse + Gate match |
| edit | `crates/bevy_naadf/src/e2e/streaming_window.rs` | trim `run_streaming_window` to ~10 LOC; add `apply_streaming_window_defaults` ~15 LOC | gate-defaults extraction |
| edit | `crates/bevy_naadf/src/e2e/noise_static_world.rs` | same pattern | gate-defaults extraction |
| edit | `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs` | same | gate-defaults extraction |
| edit | `crates/bevy_naadf/src/e2e/small_edit_visual.rs` | same | gate-defaults extraction |
| edit | `crates/bevy_naadf/src/e2e/small_edit_repro.rs` | same | gate-defaults extraction |
| edit | `crates/bevy_naadf/src/e2e/vox_e2e.rs` | same | gate-defaults extraction |
| edit | `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` | same | gate-defaults extraction |

Total: ~600 LOC new (mostly clap derive struct bodies), ~220 LOC deleted from
the parallel-reality paths.

## Decisions & rejected alternatives

### D1. Use `clap` derive vs hand-rolled CLI

**Chosen:** `clap = "4"` with `derive` feature.

**Rejected:** hand-rolling extends the existing pattern (= more parallel reality
later, harder to read, no `--help`). The user-visible cost of adding clap is
~50 KiB compiled in (clap 4 derives a fast parser). Native Bevy projects
routinely take clap; the runtime cost is single-digit ms one-shot at startup.

**Would flip:** if the user explicitly said "no clap"; otherwise the
ergonomic + maintainability win dominates.

### D2. Separate `Cli` struct vs `#[derive(Parser)]` on `AppArgs` itself

**Chosen:** separate `Cli` struct + `into_app_args()` conversion.

**Rejected:** deriving Parser on `AppArgs` directly. `AppArgs` is a Bevy
`Resource` consumed by ~30 systems; many fields shouldn't be CLI-exposed
(e.g. the `construction_config: ConstructionConfig` nested struct has 11
fields, `gi: GiSettings` has 17). Forcing clap to handle the entire shape
would require either (a) deep `#[command(flatten)]` on every sub-struct,
or (b) `#[arg(skip)]` annotations on ~30 fields, both of which mix CLI
concerns into a Bevy resource type that has many non-CLI consumers.

**Would flip:** if `AppArgs` were small (5–10 fields), inlining the derive
would be cleaner. At ~25+ fields with non-Copy nested structs, the
separation is the right call.

### D3. `GridPresetArg` mirror enum vs custom value parser

**Chosen:** parallel `GridPresetArg` (clap-friendly unit-only enum) +
post-parse conversion using the other flat CLI fields.

**Rejected:** writing a custom `clap::builder::ValueParserFactory` for
`GridPreset` that parses `procedural-streaming:seed=1337,preset=0` etc.
This is more compact at CLI time but breaks the `--noise-seed` / `--noise-preset`
flags' usability (they'd be subsumed into the preset arg). The flat-flag
approach lets the user override the seed without re-typing the preset.

**Would flip:** if `GridPreset` grew many tightly-coupled variant-specific
fields. Today's `noise_preset` + `seed` are simple enough that flat flags
work.

### D4. Per-gate `run_X()` wrappers — keep or delete

**Chosen:** keep as thin wrappers (delegate to `apply_gate_defaults_X` +
`run_e2e_render_with_args`). The `run_X` functions are NOT used outside the
e2e binary today (no `crate::e2e::streaming_window::run_streaming_window`
references from unit tests), so they could be deleted — but keeping them
preserves the public API for any future test that wants to launch a gate
with `AppArgs::default()` non-CLI'd.

**Rejected:** deletion. Cheap to keep; provides a "Rust API: launch the
gate" shape that mirrors the CLI shape.

**Would flip:** if a future refactor wants to remove the `run_X` surface
entirely. The wrappers are 5 LOC each — trivial to delete later.

### D5. Backward compat — keep bare `--streaming-window` / `--oasis-edit-visual` etc?

**Chosen:** DROP. Move to `--gate streaming-window`. The breaking change is
explicitly authorized by the user's "rewrite it to hell" directive; the
unified `--gate` shape is the CLI rearch's point. The brief explicitly
suggests this: *"recommend a single `#[arg(long = "gate")]` enum"*.

Some long-standing helpers may have downstream callers (CI scripts,
justfiles); search for any:

```
grep -rn '"--streaming-window"\|"--oasis-edit-visual"\|"--noise-static-world"\|"--vox-e2e"' \
  /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/ | grep -v target
```

A quick search of the worktree shows only `bin/e2e_render.rs` and the gate
files themselves reference these strings — no justfile / shell / Makefile
consumers. Safe to drop.

**Rejected:** alias each bare flag to `--gate X` (e.g. `--streaming-window` ≡
`--gate streaming-window`). Adds N more arg parses, multiplies the surface
area, complicates "user passes --streaming-window AND --gate noise-static-world"
disambiguation. The clean break is simpler.

**Would flip:** if a CI script lookup found callers we missed. Add an alias
on demand.

### D6. Where to short-circuit headless gates

**Chosen:** keep them as a `match gate { Some(X) => return ..., ... }` block
BEFORE building the App. They're explicit short-circuits — not violating
the "drive the actual main" principle because they intentionally don't
construct a renderable App at all (they're byte-equality validators that
spin a `MinimalPlugins` headless world or a pure-compute pipeline).

**Rejected:** forcing them through `build_app(args)`. They'd boot a full
windowed App for what is conceptually a pure-compute test. Wasteful.

**Would flip:** if the user explicitly requires "literally everything goes
through build_app." The current "build_app for rendering tests,
short-circuit for pure-compute" split is principled.

### D7. Move `run_X()` defaults to a gate-defaults function vs gate-impl files

**Chosen:** `apply_gate_defaults_X(&mut AppArgs)` lives in the per-gate
e2e file (e.g. `e2e/streaming_window.rs::apply_streaming_window_defaults`),
called from `cli.rs::E2eCli::into_app_args_and_gate()` via a match on
`Gate`. Centralizes the per-gate knowledge in the gate's own module.

**Rejected:** inlining the defaults in `cli.rs`. Spreads gate-specific
knowledge into the CLI layer; the gate file is the right home.

### D8. `bake` binary — does it need clap?

**Decision:** no change. `bake.rs` is a headless asset-processor runner with
no user-facing flags today; it doesn't read `AppArgs`. Outside this brief's
scope.

## Assumptions made

1. **Clap 4 derive features are sufficient.** The flag set is simple (bool
   flags, scalars, two ValueEnums); no subcommands, no arg groups, no
   conditional requireds beyond `--vox` path. Verified by inspection of the
   AppArgs shape.

2. **No CI / justfile consumers of the bare `--streaming-window` etc.
   strings.** Verified by repo-wide grep (only the e2e binary itself and the
   gate files reference these). If a CI script existed and we missed it, it
   would surface at CI runtime; not a build break.

3. **The `OasisXxx` driver state machine is unchanged.** The streaming-window
   and noise-static gates route via `args.oasis_edit_visual_mode = true`
   today; this stays. The driver still distinguishes via the per-gate mode
   flag (`streaming_window_mode`, `noise_static_mode`).

4. **The headless / multi-process gates run as-is.** wgsl-noise-oracle,
   vox-gpu-oracle, validate-gpu-construction-scaled, and
   validate-gpu-construction-production are NOT App-based; they intentionally
   short-circuit before `build_app`. The brief's "e2e drives actual main"
   principle is about App-rendering gates, not pure-compute validators.

5. **The Bevy 0.19 + Solari toolchain compiles `clap = "4"` without
   collision.** clap 4 has no transitive deps that overlap problematically
   with Bevy's tree; verified by inspection of `Cargo.lock` (no existing
   clap-N entry).

6. **`AppArgs` field additions are non-breaking.** Adding `noise_preset: u32`
   (default 0) doesn't break existing code: every `AppArgs::default()` call
   gets `0`; every existing field-by-field constructor in tests must be
   updated, but the resource is `Clone` so updates are mechanical. Grep
   `AppArgs {` to find structural literals.

7. **`build_app_with_args(cfg, args)` accepts every shape.** The function's
   signature accepts `AppArgs` by value; it consumes the arg once into a
   `Resource` insert (`lib.rs:689`). The streaming-world preset path
   (`voxel/grid.rs:122-127`) already handles every grid preset; no install
   path needs new code.

8. **`--help` printing prints all flags.** clap derive emits a complete
   `--help` output for every `#[arg]`-tagged field with a doc comment. Doc
   comments on the `Cli` struct's fields propagate as the flag help text.

## Independent review

Self-review against the brief's success criteria, deliberately adversarial.

### Criterion 1 — `cargo run --release --bin bevy-naadf -- --help` prints clap help

**Self-certifiable.** Clap `Parser::parse()` intercepts `--help` and `--version`
before user code runs; it writes to stdout + exits 0. As long as clap is in
scope and `Cli::parse()` is the first line of `main()`, this works.

### Criterion 2 — `--grid-preset procedural-streaming --vram-budget-mib 1024 --max-segments-per-frame 4 --noise-seed 1337` launches the streaming preset

**Self-certifiable** IF the field mapping in `Cli::into_app_args()` is
correct. Risk: if I miss copying the `noise_seed` CLI value into the
`GridPreset::ProceduralStreaming { seed }` variant payload (the install path
reads from the variant, not from `args.noise_seed`), the seed becomes hardcoded
`1337` regardless. Mitigation: explicit field-pair check in `into_app_args`
that the variant's `seed` field equals `args.noise_seed` when the variant is
selected. Plus the impl-log smoke-launch tests this end-to-end.

### Criterion 3 — `--grid-preset procedural-static` launches the static preset

**Self-certifiable.** Same shape as criterion 2; the static-preset variant
exists at `lib.rs:101-107` and `voxel/grid.rs:125-127` routes it.

### Criterion 4 — `--grid-preset default` (or no flag) launches default scene

**Self-certifiable.** `--grid-preset` clap default = `GridPresetArg::Default`;
when no `--vox` is supplied, `into_app_args` produces `GridPreset::Default`.
The existing `setup_test_grid` default arm is unchanged.

### Criterion 5 — ALL e2e gates still work via `--gate <NAME>`

**HIGH-RISK** for the streaming-window + noise-static gates specifically.

Reason: those gates have wall-clock budgets that were calibrated under the
existing run pattern (where `run_streaming_window()` constructs
`AppArgs::default()` internally). After the rearch, the same gate runs via
the new e2e binary `--gate streaming-window` → `apply_streaming_window_defaults`
→ `run_e2e_render_with_args(args)`. The args path SHOULD be byte-equivalent
to the old run_X path when no extra CLI flags are passed, but a missed default
(e.g. `app_args.streaming_window_mode = true` not being set, or the `noise_seed`
not being propagated into the variant) would silently break the gate.

Mitigation: write `apply_streaming_window_defaults` so it sets BOTH `args.streaming_window_mode = true`
AND `args.oasis_edit_visual_mode = true` AND `args.grid_preset = GridPreset::ProceduralStreaming { ... }`
verbatim per today's `run_streaming_window()` body (`e2e/streaming_window.rs:362-372`).
The rearch should be a behaviourally-identity-preserving refactor when invoked
via `--gate streaming-window` with no other CLI overrides.

**Recommendation for fresh-eyes follow-up:** if the streaming-window gate fails
on the first run after rearch, dispatch a fresh `delegate-reviewer` to compare
the AppArgs values at the call site (just before `build_app_with_args`)
between the pre-rearch run_streaming_window() body and the post-rearch
`apply_streaming_window_defaults` path, field by field. Print AppArgs as
`{:?}` from both paths during a smoke run and diff. (I'll add a debug
`info!()` log in `apply_streaming_window_defaults` that prints the resolved
AppArgs.)

### Criterion 6 — `cargo build --workspace --release` clean

**Self-certifiable.** Clap 4 is widely compatible. Risk: a name collision
on `Parser` or `ValueEnum` between clap and Bevy. Mitigation: namespace
imports — `use clap::{Parser as ClapParser, ValueEnum as ClapValueEnum};`
if needed (probably not — Bevy doesn't export those names).

### Criterion 7 — `cargo test --workspace --lib --release` ≥232 passing

**MEDIUM-RISK.** Adding `noise_preset: u32` to `AppArgs` requires updating
every structural literal `AppArgs { ... }` in tests. Mitigation: grep
`AppArgs {` and update each test case. Compiler errors will catch any
miss.

### Criterion 8 — `--help` lists every new flag

**Self-certifiable.** clap derive emits help from doc comments on the field;
just make sure every `#[arg]` field has a `///` doc comment.

### Adversarial review of my own design

1. **Q: Does `Cli` shadowing affect downstream code that consumes
   `AppArgs`?** A: No — `Cli` is `cli`-module-local; `into_app_args()` returns
   `AppArgs` exactly as today. The rest of the codebase consumes `Res<AppArgs>`
   the same way.

2. **Q: What about wasm32 target?** A: `main.rs` isn't compiled for wasm32 —
   the web build goes through `crate::e2e` via a different entry path or
   library export. clap is a pure-Rust dep with `getrandom = "0.2"` (its
   tests need it but the runtime parser doesn't). Verified clap 4's release
   target list includes wasm32 — no native dep. Should compile clean for
   `wasm32-unknown-unknown`.

3. **Q: The `vox-gpu-oracle` gate spawns subprocesses of itself. Does the new
   CLI shape break that?** A: The subprocess path passes `--vox-gpu-oracle-cpu`
   or `--vox-gpu-oracle-gpu` to itself (e.g. `vox_gpu_oracle.rs::run_vox_gpu_oracle_compare`).
   Under the rearch these become `--gate vox-gpu-oracle-cpu` and `--gate
   vox-gpu-oracle-gpu`. **The subprocess spawn site must be updated** to use the
   new flag names. Search for `--vox-gpu-oracle-cpu`:
   ```
   grep -rn '"--vox-gpu-oracle-cpu"\|"--vox-gpu-oracle-gpu"' crates/bevy_naadf/src/
   ```
   Verified: the strings appear at `e2e/vox_gpu_oracle.rs` (in the args build).
   Update those to `--gate vox-gpu-oracle-cpu` / `--gate vox-gpu-oracle-gpu`.

4. **Q: What if clap parses a flag that AppArgs doesn't have, like a future
   CLI-only debug flag?** A: That's a `Cli`-struct field that doesn't have
   a matching `AppArgs` field. Fine — `Cli::into_app_args()` may use it
   without writing to AppArgs (e.g. a `--debug-print-args` flag could print
   the resolved AppArgs and exit). Not in this brief's scope.

5. **Q: Does the e2e binary's `clap` setup print confusing help when both
   `Cli` (flattened) AND `Gate` are at the top level?** A: Clap's `#[command(flatten)]`
   flattens the inner struct into the outer's help; the output is one
   combined help with all interactive flags + `--gate`. This matches the
   user's intent (e2e shows every interactive flag + the gate selector).

6. **Q: The `args.iter().any(|a| a == "--validate-gpu-construction")` pattern
   currently allows passing MULTIPLE flags simultaneously (e.g.
   `--validate-gpu-construction --entities`). Does `--gate` lose that?** A:
   Yes — `--gate` is `Option<Gate>` (single choice). The legacy multi-flag
   pattern was actually a bug: each branch in the dispatch ladder is an
   else-if, so `--entities --validate-gpu-construction` runs the entities
   branch only. The post-App validation checks are independent of the main
   App run mode and run additively. Under the rearch, the post-App
   validations are still gate-flag-driven (only one gate at a time), which
   matches the actual existing semantics (the if-else chain selected one
   gate; the post-validations matched it). Not a behaviour regression.

7. **Q: Is there a concrete risk that the streaming-window gate calls
   `apply_streaming_window_defaults` but the function doesn't set every
   field the old `run_streaming_window()` set?** A: YES — this is the
   load-bearing risk. Mitigation: side-by-side diff each `run_X` body
   against the `apply_X_defaults` function during implementation, and
   smoke-launch each gate as a verification step.

### Items flagged for fresh-eyes follow-up

**HIGH-RISK item 1** — `apply_X_defaults` field-by-field fidelity vs the
existing `run_X()` body. If after implementation any of `cargo run
--release --bin e2e_render -- --gate streaming-window` /
`--gate noise-static-world` / `--gate oasis-edit-visual` fails on the first
post-rearch run, the orchestrator should dispatch a fresh
`delegate-reviewer` to diff the AppArgs the gate sees pre-rearch vs
post-rearch.

**HIGH-RISK item 2** — `vox_gpu_oracle` subprocess respawn. The compare
gate spawns itself with `--vox-gpu-oracle-cpu` then `--vox-gpu-oracle-gpu`.
Under the rearch these become `--gate vox-gpu-oracle-cpu` and `--gate
vox-gpu-oracle-gpu`. The respawn site MUST be updated. If we miss this,
the gate hangs (subprocess fails to parse args + exits non-zero). Verify
post-implementation by running `--gate vox-gpu-oracle`.

**MEDIUM-RISK item 3** — test-suite structural literals. Any test that
constructs `AppArgs { grid_preset: ..., taa: ..., ... }` needs the new
`noise_preset: 0,` field. Compiler will catch; mechanical fix.
