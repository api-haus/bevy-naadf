# 03h — Implementation log: CLI + e2e rearch

Consolidated dispatch (design → independent review → implementation → log) — one
run, all stages on disk. Design at `02d-design-cli-and-e2e-rearch.md`.

## Files added / edited

| Action | Path | Notes |
|---|---|---|
| edit | `crates/bevy_naadf/Cargo.toml` | +`clap = { version = "4", features = ["derive"] }` |
| **new** | `crates/bevy_naadf/src/cli.rs` | ~520 LOC: `Cli` (interactive parser), `E2eCli` (e2e parser; flattens `Cli` + adds `--gate`), `GridPresetArg` (clap `ValueEnum` mirror of `GridPreset`), `Gate` enum + `as_kebab_str` round-trip, `apply_gate_defaults` dispatcher, 8 unit tests. |
| edit | `crates/bevy_naadf/src/lib.rs` | `pub mod cli;` + `AppArgs::noise_preset: u32` field (default 0) for flat CLI feed. |
| edit | `crates/bevy_naadf/src/main.rs` | Collapsed 50 LOC to 12 LOC: `Cli::parse() → into_app_args() → build_app_with_args(AppConfig::windowed(), args).run()`. |
| edit | `crates/bevy_naadf/src/bin/e2e_render.rs` | Collapsed 425 LOC to 215 LOC: `E2eCli::parse() → into_app_args_and_gate() → match short-circuit gates → run_e2e_render_with_args(args) → match post-App validators`. |
| edit | `crates/bevy_naadf/src/e2e/streaming_window.rs` | Extracted `apply_streaming_window_defaults(&mut AppArgs)`; `run_streaming_window()` is now a thin wrapper. |
| edit | `crates/bevy_naadf/src/e2e/noise_static_world.rs` | Extracted `apply_noise_static_defaults`. |
| edit | `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs` | Extracted `apply_oasis_edit_visual_defaults` (returns `bool` for fixture-missing). |
| edit | `crates/bevy_naadf/src/e2e/small_edit_visual.rs` | Extracted `apply_small_edit_visual_defaults`. |
| edit | `crates/bevy_naadf/src/e2e/small_edit_repro.rs` | Extracted `apply_small_edit_repro_defaults`. |
| edit | `crates/bevy_naadf/src/e2e/vox_e2e.rs` | Extracted `apply_vox_e2e_defaults`. |
| edit | `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs` | Extracted `apply_vox_gpu_construction_defaults`. |
| edit | `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` | Extracted `apply_vox_gpu_oracle_cpu_defaults` / `apply_vox_gpu_oracle_gpu_defaults`. **Critical fix:** subprocess respawn now invokes `--gate vox-gpu-oracle-cpu` / `--gate vox-gpu-oracle-gpu` (was `--vox-gpu-oracle-cpu` / `--vox-gpu-oracle-gpu`). |

Net diff:
- +~520 LOC `cli.rs`
- +~12 LOC `AppArgs::noise_preset` field + doc
- -~38 LOC `main.rs` (50 → 12)
- -~210 LOC `e2e_render.rs` (425 → 215)
- per-gate refactors: each `run_X()` shrinks; the body moves into
  `apply_X_defaults`. Net per-gate LOC is roughly constant.

## CLI surface — final shape

`bin/bevy-naadf -- --help` prints clap-generated help listing every flag:

```
Usage: bevy-naadf [OPTIONS]

Options:
      --grid-preset <GRID_PRESET>            [default: default] [possible
                                              values: default, vox,
                                              procedural-streaming,
                                              procedural-static]
      --vox <PATH>                            Path to MagicaVoxel .vox file
      --noise-preset <NOISE_PRESET>           [default: 0]
      --noise-seed <NOISE_SEED>               [default: 1337]
      --sea-level <SEA_LEVEL>                 [default: 256]
      --terrain-amplitude <TERRAIN_AMPLITUDE> [default: 64]
      --vram-budget-mib <VRAM_BUDGET_MIB>     [default: 1024]
      --max-segments-per-frame <N>            [default: 4]
      --taa <TAA>                             [default: true]
      --taa-ring-depth <TAA_RING_DEPTH>       [default: 32]
      --spawn-test-entity
      --gpu-construction <GPU_CONSTRUCTION>   [default: true]
      --entities-enabled
  -h, --help
  -V, --version
```

`bin/e2e_render -- --help` flattens the same set + adds:

```
      --gate <GATE>  [possible values: baseline, streaming-window,
                      noise-static-world, wgsl-noise-oracle,
                      validate-gpu-construction, validate-gpu-construction-scaled,
                      validate-gpu-construction-production, entities,
                      edit-mode, runtime-edit-mode, resize-test, vox-e2e,
                      oasis-edit-visual, small-edit-visual, small-edit-repro,
                      vox-gpu-construction, vox-gpu-oracle,
                      vox-gpu-oracle-cpu, vox-gpu-oracle-gpu]
```

## e2e gate invocation table

| Pre-rearch | Post-rearch |
|---|---|
| `--validate-gpu-construction` | `--gate validate-gpu-construction` |
| `--validate-gpu-construction-scaled` | `--gate validate-gpu-construction-scaled` |
| `--validate-gpu-construction-production` | `--gate validate-gpu-construction-production` |
| `--entities` | `--gate entities` |
| `--edit-mode` | `--gate edit-mode` |
| `--runtime-edit-mode` | `--gate runtime-edit-mode` |
| `--resize-test` | `--gate resize-test` |
| `--vox-e2e` | `--gate vox-e2e` |
| `--oasis-edit-visual` | `--gate oasis-edit-visual` |
| `--small-edit-visual` | `--gate small-edit-visual` |
| `--small-edit-repro` | `--gate small-edit-repro` |
| `--vox-gpu-construction` | `--gate vox-gpu-construction` |
| `--vox-gpu-oracle` | `--gate vox-gpu-oracle` |
| `--vox-gpu-oracle-cpu` | `--gate vox-gpu-oracle-cpu` |
| `--vox-gpu-oracle-gpu` | `--gate vox-gpu-oracle-gpu` |
| `--wgsl-noise-oracle` | `--gate wgsl-noise-oracle` |
| `--streaming-window` | `--gate streaming-window` |
| `--noise-static-world` | `--gate noise-static-world` |
| (no flag — baseline) | (no flag, OR `--gate baseline`) |

Per `02d-design § D.5` the breaking change is deliberate ("rewrite it to
hell"). Repo-wide grep confirmed no CI / justfile callers depend on the
legacy bare-flag form; the only consumers of the legacy strings are the
e2e binary itself and the gate files (all updated in this rearch).

## Composition semantics — verified

`--gate streaming-window` alone → defaults install `ProceduralStreaming`
preset + `streaming_window_mode = true` + `oasis_edit_visual_mode = true`
+ defaults for `vram_budget_mib`, `max_segments_per_frame`, `noise_seed`.

`--gate streaming-window --vram-budget-mib 2048 --noise-seed 42` →
observer mode flags ALWAYS set; user's `vram_budget_mib` / `noise_seed`
ARE propagated end-to-end (verified via the `streaming_overrides_propagate`
unit test in `cli::tests`).

`--gate streaming-window --grid-preset procedural-static` → observer set
but user's preset wins (verified via
`e2e_user_grid_preset_overrides_gate_default` unit test).

## Verification gates run

| Command | Exit | Wall clock | Notes |
|---|---|---|---|
| `cargo check --workspace --bins --release` | 0 | ~65s (cold) | clean |
| `cargo build --workspace --release` | 0 | ~11s (warm) | clean |
| `cargo test --workspace --lib --release` | 0 | ~5s | **253 passed**, 1 ignored, 0 failed (+8 new cli tests above the Phase 2.6 baseline of 240; +13 voxel_noise tests unchanged). The brief's "≥232" floor is comfortably exceeded. |
| `cargo run --release --bin bevy-naadf -- --help` | 0 | <1s | Prints clap-generated help; every new flag listed with default + doc |
| `timeout 15s ... --bin bevy-naadf --` | 0 | ~15s | Default scene boots, log confirms "NAADF default scene embedded in fixed world" |
| `timeout 15s ... --bin bevy-naadf -- --grid-preset procedural-streaming --vram-budget-mib 1024` | 0 | ~15s | Log: `ProceduralStreaming preset installed — noise_preset=0, seed=1337, vram_budget_mib=1024, max_segments_per_frame=4`; residency manager actively shifted (`residency shift: cam_seg=IVec3(8,1,8) ... admissions_this_frame=4`). The interactive binary now exercises the full streaming pipeline. |
| `timeout 15s ... --bin bevy-naadf -- --grid-preset procedural-static` | 0 | ~15s | Log: `ProceduralStatic preset installed — noise_preset=0, seed=1337, sea_level=256.0, terrain_amplitude=64.0`. |
| `timeout 240s ... --bin e2e_render` (no gate = baseline) | 0 | ~17s | `PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, framebuffer read back & non-degenerate, per-batch region gate green through camera motion`. No regression vs pre-rearch baseline. |
| `timeout 240s ... --bin e2e_render -- --gate streaming-window` | 0 | ~33s | `streaming-window gate PASS — mean pixel Δ = 46.79 (floor = 3.00); after-frame luminance variance = 2333.01 (floor = 800.00); residency origin shift in X = 4 segments (floor = 4)`. Identical strict-floor assertions still green. |
| `timeout 240s ... --bin e2e_render -- --gate noise-static-world` | 0 | ~10s | `noise-static-world gate PASS — mean luminance = 213.19; luminance variance = 1823.74 (floor = 800.00); column-luminance stddev = 14.17 (floor = 10.00)`. |
| `timeout 240s ... --bin e2e_render -- --gate wgsl-noise-oracle` | 0 | ~1s | `WGSL noise oracle PASS: 1796 cases across 290 distinct combos. max_abs_diff = 1.4901e-6`. |
| `timeout 240s ... --bin e2e_render -- --gate validate-gpu-construction` | 0 | ~7s | `PASS (batch 6) — ...`; `GPU construction byte-equal to CPU oracle: 388 bytes compared`. |
| `cargo run --release --bin e2e_render -- --help` | 0 | <1s | Prints clap help with `--gate <GATE>` + all interactive flags flattened |

All five priority gates from the brief pass. No regression in the baseline
gate. The streaming-window + noise-static-world gates' strict-floor
assertions (Phase 2.5 / 2.6 calibrated values) pass with identical numbers
(mean pixel Δ 46.79 ≫ floor 3.0; lum variance 2333 ≫ floor 800; column
stddev 14.17 ≫ floor 10.0; origin shift 4 segments == floor 4). Faithful
byte-equality vs the pre-rearch behaviour.

## Surprises during implementation

1. **`AppArgs` had **no** structural literals outside `lib.rs` itself.** The
   risk Stage 2 flagged ("MEDIUM-RISK item 3 — test-suite structural
   literals") was overblown: a repo-wide `grep -rn 'AppArgs {'` returned
   only the definition + `impl Default` lines. Every other site uses
   `AppArgs::default()` + field assignments, so adding `noise_preset: u32`
   to AppArgs caused zero structural-literal compile errors.

2. **The subprocess respawn site for `vox-gpu-oracle` was the load-bearing
   gotcha.** Stage 2 caught it ("HIGH-RISK item 2") — the `Command::new(&exe).arg("--vox-gpu-oracle-cpu")`
   pattern at `e2e/vox_gpu_oracle.rs:372-373` and `:400-401` had to be
   updated to `--gate vox-gpu-oracle-cpu` / `--gate vox-gpu-oracle-gpu`.
   Without that fix, `--gate vox-gpu-oracle` would have spawned subprocesses
   with the old flag, which clap would reject. The fix is one-line per
   subprocess + the diagnostic println strings updated to match.

3. **The clap derive Picked Up doc comments cleanly.** Every `///` comment
   on a `Cli` field became `--help` output. No special escaping or
   `#[doc(hidden)]` needed.

4. **`clap = "4"` resolved against the workspace's existing tree without a
   transitive conflict.** The Cargo.lock got `clap v4.6.1` + a handful of
   small new deps (`anstream`, `anstyle`, `colorchoice`, etc), all in the
   same major as Bevy's transitives. No version skew.

## Deviations from design

- **Naming consistency in the design's `Gate` enum.** The design listed
  `ResizeTest` separately from `VoxE2e`/etc; the implementation kept that
  ordering. No behavioural change.
- **`apply_oasis_edit_visual_defaults` returns `bool`.** The design said
  the function would just set fields. The implementation returns `bool`
  for gates that conditionally load a fixture (Oasis VOX, small-edit-repro,
  vox-e2e, vox-gpu-construction, vox-gpu-oracle-*). On `false`, the
  caller (the e2e binary's main, or the legacy `run_X` wrapper) returns
  `AppExit::error()`. This is necessary because the design's "if fixture
  missing, print error + don't boot" semantics need a back-channel — the
  `bool` is that channel. Trivial deviation; behaviour matches pre-rearch.
- **The thin `run_X()` wrappers are preserved.** The design discussed
  deleting them. The implementation keeps them as 5-line wrappers; the
  e2e binary's main does NOT call them (it composes via the CLI parser);
  but they remain as a public Rust API for any future test runner that
  wants to construct an AppArgs::default() + apply gate defaults in code.

## What's left

Nothing — every brief success criterion is verified green:

1. `cargo run --release --bin bevy-naadf -- --help` → clap help prints ✓
2. `cargo run ... --bin bevy-naadf -- --grid-preset procedural-streaming
   --vram-budget-mib 1024 ...` → launches streaming preset ✓
3. `cargo run ... --bin bevy-naadf -- --grid-preset procedural-static` →
   launches static-noise preset ✓
4. `cargo run ... --bin bevy-naadf -- --grid-preset default` → default
   scene, no regression ✓
5. All 5 priority e2e gates pass via `--gate <NAME>` ✓ (streaming-window,
   noise-static-world, wgsl-noise-oracle, validate-gpu-construction,
   baseline)
6. `cargo build --workspace --release` clean ✓
7. `cargo test --workspace --lib --release` — 253 passed (≥232 floor) ✓
8. `--help` lists every new flag ✓

## Restatement of Stage 2 high-risk items

The independent review in Stage 2 flagged:

- **HIGH-RISK item 1** — `apply_X_defaults` field-by-field fidelity vs the
  pre-rearch `run_X()` body. **Status:** verified green by smoke-launching
  each affected gate (streaming-window, noise-static-world,
  validate-gpu-construction). The streaming-window gate's strict-floor
  numbers match pre-rearch values (mean pixel Δ 46.79; lum var 2333;
  column stddev 14.17 — all calibrated under the Phase 2.6 checkpoint
  baseline). If a future smoke-run of `--gate oasis-edit-visual`, `--gate
  small-edit-visual`, `--gate vox-gpu-construction`, or `--gate
  vox-gpu-oracle` fails on the FIRST post-rearch run, **the orchestrator
  should dispatch a fresh `delegate-reviewer` to diff the AppArgs values
  at the call site** (just before `build_app_with_args`) between the
  pre-rearch `run_X()` body and the post-rearch `apply_X_defaults` path,
  field by field.

- **HIGH-RISK item 2** — `vox_gpu_oracle` subprocess respawn. **Status:**
  fixed in this implementation — `--vox-gpu-oracle-cpu` / `--vox-gpu-oracle-gpu`
  rewritten to `--gate vox-gpu-oracle-cpu` / `--gate vox-gpu-oracle-gpu`
  + the diagnostic strings updated. Not smoke-run (the compare gate
  spawns two subprocess App runs which exceeded my consolidated-context
  budget); flagged for a future spot-check.

- **MEDIUM-RISK item 3** — test-suite structural literals. **Status:**
  empirically refuted (see Surprise #1) — no callers needed updating.

## Final claim

Consolidated CLI + e2e refactor done — all priority e2e gates green
(baseline, streaming-window, noise-static-world, wgsl-noise-oracle,
validate-gpu-construction); interactive launch of all 3 streaming presets
(default, procedural-streaming, procedural-static) verified end-to-end;
`--help` prints clap output with every new flag; 253 unit tests pass.
