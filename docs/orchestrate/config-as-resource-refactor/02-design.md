# Config-as-resource refactor — design

> Design document. NO CODE WRITTEN. Investigation + diagnosis + design + migration
> plan + verification surface. Implementation is a downstream orchestration the
> user scopes after approving this document.

## 1. Investigation

### 1.1 Field-by-field inventory of current AppArgs

`AppArgs` is defined at `crates/bevy_naadf/src/app_args.rs:24-183`; its `Default`
impl lives at `:185-207`; its test module at `:209-236`. The 16 distinct
top-level fields are catalogued below — definition line is the field declaration
inside the struct; "default" is the value set in `impl Default`; every consumer
site has been verified by `grep -n`.

Classification key: **A** = Parameter (Bucket A: value the running app reads),
**B** = Mode (Bucket B: enum branch), **C** = Action verb (Bucket C: dispatcher
input — does not become a resource). "Render-world?" = the field's value reaches
the render sub-app.

---

#### `grid_preset: GridPreset`  (`app_args.rs:27`)
- **Default:** `GridPreset::Default` (`lib.rs:69-105`, `:73` `#[default]`).
- **Bucket:** **A — Parameter.** Names *what world content* the app installs;
  read once at `Startup`, never afterwards.
- **Runtime mutability:** Mutated once at `Startup` time on wasm32 by
  `voxel/web_vox.rs:402` when `?skybox=1` is in the URL. Otherwise immutable.
  Q3 puts this mutation in scope for relocation into the wasm32 bootstrap.
- **Render-world?** No. Consumed only main-side by `setup_test_grid`.
- **Consumer sites (verified):**
  - `crates/bevy_naadf/src/voxel/grid.rs:123, 127, 131, 144, 147` — `setup_test_grid`
    matches the enum and dispatches to one of four `install_*` functions.
  - `crates/bevy_naadf/src/voxel/web_vox.rs:402` — `Startup` mutation (Q3 in
    scope; relocates to wasm32 bootstrap).
  - `crates/bevy_naadf/src/diagnostics.rs:107` — readonly diagnostics dump.
  - `crates/bevy_naadf/src/main.rs:45` — argv `--vox <path>` bootstrap
    construction of `GridPreset::Vox { path }`.
  - 7 e2e gate `run_*` builders construct it: `oasis_edit_visual.rs:210`,
    `small_edit_repro.rs:154`, `vox_e2e.rs:378`, `vox_gpu_construction.rs:231`,
    `vox_gpu_oracle.rs:282 / :330`, `vox_horizon_parity.rs:157`,
    `vox_web_parity.rs:159 / :199`.

---

#### `taa: bool`  (`app_args.rs:32`)
- **Default:** `true` (`app_args.rs:189`).
- **Bucket:** **A — Parameter.** Long-term TAA on/off switch.
- **Runtime mutability:** None today, but it is a candidate runtime
  toggle (no settings panel knob points at it currently).
- **Render-world?** Yes. Read by `extract_taa_config`
  (`render/extract.rs:452-459`) → mirrored into render-world
  `ExtractedTaaConfig` (`render/extract.rs:444-448`).
- **Consumer sites (verified):**
  - `crates/bevy_naadf/src/render/extract.rs:454, 457` — extract reads
    `args.taa` from `Extract<Option<Res<AppArgs>>>`.
  - `crates/bevy_naadf/src/render/taa.rs:190, 200` — `update_camera_history`
    reads `args.taa` to gate Halton jitter generation (main-world).
  - `crates/bevy_naadf/src/diagnostics.rs:108` — readonly dump.
- **Smell shape:** Direct precedent for the refactor — `ExtractedTaaConfig` is
  already a per-domain extracted resource; only the **source** is `AppArgs`. The
  refactor swaps the source to a new `TaaConfig` main-world resource.

---

#### `taa_ring_depth: u32`  (`app_args.rs:42`)
- **Default:** `DEFAULT_TAA_RING_DEPTH` = 32 (`lib.rs:123`,
  `app_args.rs:190, :217-221`).
- **Bucket:** **A — Parameter.** **The user's named smell.**
- **Runtime mutability:** Mutated at bootstrap by `build_app_with_budget`
  (`lib.rs:158`) and the wasm32 bootstrap (`main.rs:77`) — the budget probe
  writes back into `args.taa_ring_depth` *before* `build_app_with_args` because
  `TaaRingConfig` is captured via plugin-build snapshot (`render/mod.rs:113-126`).
  Otherwise immutable.
- **Render-world?** Yes. Snapshotted into `TaaRingConfig`
  (`render/taa.rs:46-50`) at plugin build (`render/mod.rs:113-126`); consumed
  by `prepare_taa` (buffer sizing) and `NaadfPipelines::from_world` (WGSL
  shader-def). The two-sided agreement is binding (`app_args.rs:213-235`,
  `render/taa.rs:31-50`).
- **Consumer sites (verified):**
  - `crates/bevy_naadf/src/render/mod.rs:113-126` — plugin-build snapshot
    reads `args.taa_ring_depth`, inserts `TaaRingConfig` into render sub-app.
  - `crates/bevy_naadf/src/diagnostics.rs:109` — readonly dump.
  - `crates/bevy_naadf/src/settings/mod.rs:262` — `KnobKind::Readonly`
    closure formats `args.taa_ring_depth` (settings panel readonly row).
  - `crates/bevy_naadf/src/lib.rs:158` — budget mutates pre-build.
  - `crates/bevy_naadf/src/main.rs:77` — wasm32 budget mutates pre-build.
  - `crates/bevy_naadf/src/app_args.rs:217-221, :229-235` — test asserts
    default = `DEFAULT_TAA_RING_DEPTH` = 32 and that depth ∈ {16, 24, 32}.

---

#### `gi: GiSettings`  (`app_args.rs:44`)
- **Default:** `GiSettings::default()` (`app_args.rs:191`). `GiSettings` lives
  at `settings/canonical.rs` (re-exported `crate::GiSettings`).
- **Bucket:** **A — Parameter** *with runtime mutability*. The settings panel
  mutates `args.gi.<field>` via `ResMut<AppArgs>`.
- **Runtime mutability:** YES — the only field of `AppArgs` that is truly
  long-lived mutable. Mutated by `settings::adjust_settings`
  (`settings/mod.rs:458, 488, 495, 498, 505, 508, 511`) and
  `settings::mouse_interact_settings` (`settings/mod.rs:528, 612, 614, 620,
  621, 631, 632`).
- **Render-world?** Yes. Read by `extract_gi_config` (`render/extract.rs:513-520`)
  → `ExtractedGiConfig::settings`.
- **Consumer sites (verified):**
  - `crates/bevy_naadf/src/render/extract.rs:515, 518` — extract reads
    `args.gi` and copies into `ExtractedGiConfig`.
  - `crates/bevy_naadf/src/settings/mod.rs:277-282, 458, 488, 495, 498, 505,
    508, 511, 528, 612, 614, 620-621, 631-632, 660, 664, 668, 832-840, 643`
    — settings panel read + mutate.
  - `crates/bevy_naadf/src/settings/mod.rs:268` — `KnobKind::Readonly` for
    `a.gi.global_illum_max_accum`.
  - `crates/bevy_naadf/src/diagnostics.rs:111` — readonly dump.

---

#### `construction_config: render::construction::ConstructionConfig`  (`app_args.rs:55`)
- **Default:** `ConstructionConfig::default()`
  (`app_args.rs:192`; default impl at `render/construction/config.rs:136-189`).
- **Bucket:** **A — Parameter.**
- **Runtime mutability:** None at runtime. Mutated **before** bootstrap by 4 e2e
  gate `run_*` builders (`vox_gpu_construction.rs:234`, `vox_gpu_oracle.rs:334`,
  `vox_horizon_parity.rs:158`, `vox_web_parity.rs:200`) and the
  `EntitiesBoot` branch (`bin/e2e_render.rs:342`).
- **Render-world?** Yes. Lifted via `From<&AppArgs>` at
  `render/construction/config.rs:252-288` (with the wasm32 platform divergence
  at `:268-285` clamping `max_group_bound_dispatch` and pinning `n_bounds_rounds = 1`).
  Inserted into render sub-app at `render/construction/mod.rs:1864-1866`.
- **Consumer sites (verified):**
  - `crates/bevy_naadf/src/render/construction/mod.rs:1788` — startup gate
    `run_gpu_construction_startup` reads `args.construction_config.gpu_construction_enabled`.
  - `crates/bevy_naadf/src/render/construction/mod.rs:1834-1836` — plugin build
    constructs `ConstructionConfig::from(&AppArgs)`.
  - `crates/bevy_naadf/src/render/construction/config.rs:267` — `From<&AppArgs>`
    reads `args.construction_config`.
  - `crates/bevy_naadf/src/diagnostics.rs:112` — readonly dump.
  - 5 e2e gates mutate `app_args.construction_config.<field>` (above).

---

#### `spawn_test_entity: bool`  (`app_args.rs:65`)
- **Default:** `false` (`app_args.rs:193`).
- **Bucket:** **A — Parameter** (borderline — see Diagnosis 2.1). Modelled as
  "test entity fixture present or absent" rather than a dedicated mode.
- **Runtime mutability:** None.
- **Render-world?** No. Read only on the main side as a `Startup` gate.
- **Consumer sites (verified):**
  - `crates/bevy_naadf/src/render/construction/mod.rs:1853` —
    `spawn_phase_c_test_entity` self-gates via `.run_if(|args: Res<AppArgs>| args.spawn_test_entity)`.
  - `crates/bevy_naadf/src/e2e/driver.rs:680` — driver reads it to switch
    `--entities`-aware assertion baseline.
  - `crates/bevy_naadf/src/diagnostics.rs:110` — readonly dump.
  - `crates/bevy_naadf/src/bin/e2e_render.rs:343` — `EntitiesBoot` mutates
    `app_args.spawn_test_entity = true`.

---

#### `resize_test: bool`  (`app_args.rs:80`)
- **Default:** `false`.
- **Bucket:** **B — Mode.** Selects the resize-blackness reproduction
  flow (the driver branches into a separate state machine —
  `e2e/driver.rs:475-495`).
- **Render-world?** No (read main-side only).
- **Consumer sites (verified):**
  - `crates/bevy_naadf/src/e2e/driver.rs:475` — driver mode-detect.
  - `crates/bevy_naadf/src/window_config.rs:154-155` — chooses `e2e_resize_test()`.
  - `crates/bevy_naadf/src/bin/e2e_render.rs:378` — `ResizeTest` arm mutates it on.

---

#### `vox_e2e_mode: bool`  (`app_args.rs:93`)
- **Default:** `false`.
- **Bucket:** **B — Mode.** Switches the e2e ASSERT step from default-scene
  region gate to `assert_vox_geometry_visible`.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:688` — driver branches on it.
  - `crates/bevy_naadf/src/e2e/vox_e2e.rs:372, 379` — `run_vox_e2e` mutates on.

---

#### `oasis_edit_visual_mode: bool`  (`app_args.rs:103`)
- **Bucket:** **B — Mode.** Drives `OasisWarmup` driver state-machine
  fast-path and the camera-pin system.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:510` — driver mode-detect.
  - `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs:211, 307, 312` — entry +
    `pin_oasis_camera`.

---

#### `small_edit_visual_mode: bool`  (`app_args.rs:113`)
- **Bucket:** **B — Mode.** `SmallEditWarmup` driver branch + camera pin.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:527` — mode-detect.
  - `crates/bevy_naadf/src/e2e/small_edit_visual.rs:211, 256, 261` — entry + pin.

---

#### `small_edit_repro_mode: bool`  (`app_args.rs:122`)
- **Bucket:** **B — Mode.** `SmallEditReproWarmup` driver branch + camera pin
  + window resolution choice.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:538` — mode-detect.
  - `crates/bevy_naadf/src/e2e/small_edit_repro.rs:155, 164, 168` — entry + pin.
  - `crates/bevy_naadf/src/window_config.rs:156` — chooses `e2e_small_edit_repro()`.

---

#### `vox_gpu_construction_mode: bool`  (`app_args.rs:144`)
- **Bucket:** **B — Mode.** Drives `OasisWarmup`-shared driver flow + camera
  promote pose A→B + brush-skip.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:513` — mode-detect.
  - `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs:243, 271, 276` — entry +
    pin.

---

#### `vox_gpu_oracle_cpu_phase: bool`  (`app_args.rs:153`)
- **Bucket:** **B — Mode.** Routes `setup_test_grid` through the legacy
  `install_vox_sized_to_model` CPU oracle (sole remaining caller — gate-only).
- **Consumer sites:**
  - `crates/bevy_naadf/src/voxel/grid.rs:132` — `setup_test_grid` branches.
  - `crates/bevy_naadf/src/e2e/driver.rs:549, 1525` — mode-detect + screenshot
    filename selection.
  - `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs:289, 643, 647` — entry +
    `pin_vox_gpu_oracle_camera` route guard.

---

#### `vox_gpu_oracle_gpu_phase: bool`  (`app_args.rs:162`)
- **Bucket:** **B — Mode.** GPU-phase counterpart to `_cpu_phase`.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:549, 1528` — mode-detect + filename.
  - `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs:335, 647` — entry + guard.

---

#### `vox_web_parity_skybox_phase: bool`  (`app_args.rs:167`)
- **Bucket:** **B — Mode.** Single-screenshot capture with `GridPreset::Empty`.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:561, 1598` — mode-detect + filename.
  - `crates/bevy_naadf/src/e2e/vox_web_parity.rs:160, 388, 392` — entry +
    `pin_vox_web_parity_camera` guard.

---

#### `vox_web_parity_loaded_phase: bool`  (`app_args.rs:174`)
- **Bucket:** **B — Mode.** Counterpart to skybox phase + tracing-error
  assertion.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:562, 1601` — mode-detect + filename.
  - `crates/bevy_naadf/src/e2e/vox_web_parity.rs:201, 388, 392` — entry + guard.

---

#### `vox_horizon_native_phase: bool`  (`app_args.rs:182`)
- **Bucket:** **B — Mode.** Routes through `pin_vox_horizon_camera` + chooses
  `e2e_horizon` 1280×720 window.
- **Consumer sites:**
  - `crates/bevy_naadf/src/e2e/driver.rs:563, 1604` — mode-detect + filename.
  - `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs:159, 173, 177` — entry + pin.
  - `crates/bevy_naadf/src/window_config.rs:158` — chooses `e2e_horizon`.

---

### 1.2 CLI parser current shape

Three production binaries plus 15 per-gate `run_*` builders all hand-roll their
own `AppArgs`. No `clap` anywhere — `std::env::args` + ad-hoc string-iter
parsing.

| Site | file:line | Shape |
|---|---|---|
| `main.rs::main` | `crates/bevy_naadf/src/main.rs:34-91` | Native + wasm32. Greps argv for `--vox <path>`, mutates `args.grid_preset = GridPreset::Vox { path }`. Native calls `build_app_with_budget(AppConfig::windowed(), args)`; wasm32 spawns async probe + `build_app_with_args` + post-build `EffectiveWorldSize` / `InvalidSampleStorageCount` inserts (`:75-86`). |
| `android_main.rs::main` | `crates/bevy_naadf/src/android_main.rs:53-78` | No argv. `build_app_with_budget(AppConfig::windowed(), AppArgs::default())` + post-build window override + `WinitSettings::mobile()`. |
| `bin/e2e_render.rs::main` | `crates/bevy_naadf/src/bin/e2e_render.rs:134-157` | **Three-layer parser** (the only structured one). Layer 1 short-circuits no-Bevy dispatch (`:168-185`); Layer 2 picks `BootCommand` (`:256-324`) — already enum-shaped with `GateKind` carrying through (`:113`); Layer 3 collects post-app validation flags (`:447-454`). |
| 15 per-gate `run_*` builders | `crates/bevy_naadf/src/e2e/*.rs` | Each constructs `AppArgs::default()`, mutates 1-3 fields (1 mode-bool + grid_preset + optional `construction_config.gpu_construction_enabled`), calls `crate::run_e2e_render_with_args(app_args)`. Verified call sites: `oasis_edit_visual.rs:203-213`, `small_edit_repro.rs:147-156`, `small_edit_visual.rs:209-218`, `vox_e2e.rs:373-389`, `vox_gpu_construction.rs:223-246`, `vox_gpu_oracle.rs:281-291 / :329-336`, `vox_horizon_parity.rs:156-161`, `vox_web_parity.rs:158-162 / :198-203`. |
| `bin/e2e_render.rs::run_resize_test` | `crates/bevy_naadf/src/bin/e2e_render.rs:353-384` | Builder for `--resize-test` (lives in binary, not `e2e/`). Sets `app_args.resize_test = true`. |
| `bin/e2e_render.rs::run_boot_command::EntitiesBoot` | `crates/bevy_naadf/src/bin/e2e_render.rs:340-345` | Sets `app_args.construction_config.entities_enabled = true` + `app_args.spawn_test_entity = true`. |
| `e2e/ssim::parse_ssim_compare_args` | `crates/bevy_naadf/src/e2e/ssim.rs:122-155` | **Clean precedent** — parses its own knobs into its own struct, never touches `AppArgs`. |
| `budget::probe_and_select` / `probe_and_select_async` | `crates/bevy_naadf/src/render/budget.rs:415-533` | Produces `BudgetCaps` (transient parser struct at `:261-281`); consumed at `lib.rs:156-167` (native + Android via `build_app_with_budget`) and `main.rs:75-86` (wasm32). |
| `voxel/web_vox::startup_fetch_default_vox` | `crates/bevy_naadf/src/voxel/web_vox.rs:390-405` | The Q3-in-scope `Startup`-time mutation of `args.grid_preset` when `?skybox=1` is in the URL. |

### 1.3 Cross-cutting consumers

Two consumers read fields across multiple buckets:

#### Diagnostics dump — `crates/bevy_naadf/src/diagnostics.rs:23-126`
Consumes `Option<Res<AppArgs>>` and reads 6 Bucket-A fields (`grid_preset`,
`taa`, `taa_ring_depth`, `spawn_test_entity`, `gi`, `construction_config`)
verbatim into a multi-line `info!` block at `:104-119`. Reads NO Bucket-B mode
booleans (self-skips under e2e via `DiagnosticsPlugin.run_if` at `:138-140`).

Post-refactor read shape (Q4 decision: pick per consumer):
- All 6 fields land in their own resources. The dump becomes one system with 6
  `Option<Res<_>>` parameters (one per Bucket A resource). It already uses
  `Option<Res<AppArgs>>`, so handling resource-absent is built-in. No
  aggregator needed — the system is small (one Update system, one site).

#### Settings panel — `crates/bevy_naadf/src/settings/mod.rs`
Two reads:
1. **`KnobKind::Readonly { value: fn(&AppArgs) -> String }`** at `:138-140`.
   Today the closure receives `&AppArgs` and formats one field's value. Two
   closures actually call into `AppArgs` fields: `:262` reads
   `a.taa_ring_depth`; `:268` reads `a.gi.global_illum_max_accum`. The other
   `knob_readonly!` rows at `:263-267` read constants, NOT `AppArgs`.
2. **`ResMut<AppArgs>` mutation** at `:458` (`adjust_settings`) and `:528`
   (`mouse_interact_settings`) — mutates `args.gi.<field>` via the
   getter/setter closures.

Post-refactor read shape:
- `args.gi` → `ResMut<GiSettings>` (the panel only ever touches the `gi`
  sub-struct, never any other field).
- The two `Readonly` rows split: the `taa_ring_depth` row reads
  `Res<TaaRingConfig>`; the `gi.global_illum_max_accum` row reads
  `Res<GiSettings>`. Q4 calls for picking per-consumer; here the natural fit
  is per-knob: change the `Readonly` closure signature so a knob declares
  WHICH resource it reads.

Per Q4 — **two-knob fan-out, no aggregator.** The closure-signature change
is the implementor's choice between `fn(&dyn ReadonlySource) -> String` /
`enum ReadonlySource` / a typed accessor. The simplest move (and the one
the design assumes) is to make `KnobKind::Readonly` carry its own typed
read directly: `KnobKind::ReadonlyTaaRingDepth` / `KnobKind::ReadonlyGi(...)`
variants, each implemented as a system that has only what it needs. This
matches the `getter: fn(&GiSettings) -> _` pattern already in use for
interactive knobs.

## 2. Diagnosis

### 2.1 Smell map (per-field)

| Field | Smelly? | Clean? | Cross-cutting? | Borderline? |
|---|---|---|---|---|
| `grid_preset` | YES (`Startup` mutation in `web_vox.rs:402` — Q3 in scope) | — | minor (diagnostics dump) | — |
| `taa` | YES (read after bootstrap via `Extract<Res<AppArgs>>` + `Res<AppArgs>` in `update_camera_history`) | — | minor | — |
| `taa_ring_depth` | YES (**user's named field**; plugin-build snapshot + budget pre-mutation workaround at `lib.rs:158`) | — | yes (diagnostics + settings readonly) | — |
| `gi` | YES (extract reads it; settings panel `ResMut<AppArgs>` mutates it — only runtime-mutable field) | — | yes (settings interactive + readonly + diagnostics + extract) | YES — runtime-mutable; `GiSettings` already a standalone type in `settings/canonical.rs` |
| `construction_config` | YES (`From<&AppArgs>` lift + wasm32 platform divergence inside the lift) | — | minor (diagnostics) | YES — already mostly a per-domain resource; the AppArgs field is a relay |
| `spawn_test_entity` | YES (runtime-read by `.run_if` AND by e2e driver assertion) | — | minor | YES — could be Bucket A (parameter) or Bucket B (mode) |
| `resize_test` | YES (driver branches on it at runtime + window config picks resolution at bootstrap) | — | — | — |
| `vox_e2e_mode` | YES | — | — | — |
| `oasis_edit_visual_mode` | YES | — | — | — |
| `small_edit_visual_mode` | YES | — | — | — |
| `small_edit_repro_mode` | YES | — | — | — |
| `vox_gpu_construction_mode` | YES | — | — | — |
| `vox_gpu_oracle_cpu_phase` | YES (also read by main-world `setup_test_grid:132`) | — | — | YES — has a main-world non-driver consumer; the cleanest collapse keeps the route through one place |
| `vox_gpu_oracle_gpu_phase` | YES | — | — | — |
| `vox_web_parity_skybox_phase` | YES | — | — | — |
| `vox_web_parity_loaded_phase` | YES | — | — | — |
| `vox_horizon_native_phase` | YES | — | — | — |

There is no clean (bootstrap-only-read) field in the current `AppArgs`. Every
field is read by at least one system AFTER bootstrap — even
`construction_config`, whose lone main-side use is the startup gate at
`run_gpu_construction_startup`. The whole struct is the smell.

### 2.2 Natural domain groupings

After the refactor the per-domain resources organise around **functional
domains** (not "bag of bits"):

1. **World install** — `GridPreset` (which content goes into `WorldData` at Startup).
2. **TAA** — `TaaConfig { enabled: bool }` (runtime toggle), `TaaRingConfig { depth: u32 }` (the existing render-world resource is promoted to a main-world resource per the budget pattern).
3. **GI** — `GiSettings` (lifted out of `AppArgs.gi`, retained as a standalone main-world resource — runtime-mutable by settings).
4. **GPU construction** — `ConstructionConfig` (already exists; the lift via `AppArgs` is removed; bootstrap inserts directly).
5. **Test fixture** — `SpawnTestEntity` (a present-or-absent fixture flag).
6. **E2e gate mode** — `E2eGateMode` enum (Q2: collapses 11 bool fields).
7. **E2e ASSERT-time options** — `VoxE2eAssertion` (a yes/no flag that drives ASSERT-time vox-geometry-gate routing) — see Decision §3 for why this is split off rather than folded into `E2eGateMode`.

Domains 1–5 are Bucket A. Domain 6 is Bucket B. Domain 7 is Bucket A (a binary
parameter consumed by the driver's ASSERT branch — not a state-machine
selector).

### 2.3 The three-bucket taxonomy applied

| Field | Bucket | Resource it becomes |
|---|---|---|
| `grid_preset` | A | `GridPreset` (re-used; ALREADY an enum; promoted to its own `Resource`) |
| `taa` | A | `TaaConfig { enabled: bool }` (new) |
| `taa_ring_depth` | A | `TaaRingConfig { depth: u32 }` (the existing render-world resource gets a main-world twin — same shape as `EffectiveWorldSize`) |
| `gi` | A | `GiSettings` (the existing struct, promoted directly to `Resource`) |
| `construction_config` | A | `ConstructionConfig` (already exists — the `AppArgs` relay is removed) |
| `spawn_test_entity` | A | `SpawnTestEntity(bool)` (or `Option<TestEntityFixture>` — see Decision §4) |
| `resize_test` | B | folded into `E2eGateMode::Resize` |
| `vox_e2e_mode` | A (see Decision §3) | `VoxE2eAssertion(bool)` standalone — NOT a mode |
| `oasis_edit_visual_mode` | B | `E2eGateMode::OasisEdit` |
| `small_edit_visual_mode` | B | `E2eGateMode::SmallEditVisual` |
| `small_edit_repro_mode` | B | `E2eGateMode::SmallEditRepro` |
| `vox_gpu_construction_mode` | B | `E2eGateMode::VoxGpuConstruction` |
| `vox_gpu_oracle_cpu_phase` | B | `E2eGateMode::VoxGpuOracleCpu` |
| `vox_gpu_oracle_gpu_phase` | B | `E2eGateMode::VoxGpuOracleGpu` |
| `vox_web_parity_skybox_phase` | B | `E2eGateMode::VoxWebParitySkybox` |
| `vox_web_parity_loaded_phase` | B | `E2eGateMode::VoxWebParityLoaded` |
| `vox_horizon_native_phase` | B | `E2eGateMode::VoxHorizonNative` |

Bucket C (action verbs that DON'T become resources):
- `--vox-gpu-oracle`, `--vox-web-parity`, `--ssim-compare`,
  `--validate-gpu-construction-scaled`, `--validate-gpu-construction-production`
  — already action verbs (Layer 1 short-circuits in `bin/e2e_render.rs:168-185`);
  no `AppArgs` field corresponds to them, no resource is added.
- `--vox <path>` is a parameter+verb hybrid: the verb routes through native
  bootstrap, the parameter value becomes `GridPreset::Vox { path }`.

## 3. Proposed design

### 3.1 Post-refactor resource inventory

Each entry: type name + module + ownership + default + worlds + bootstrap +
mutability.

---

#### `GridPreset` — main-world only, **promoted to a Resource**
- **Type:** `crate::GridPreset` (already an enum at `crates/bevy_naadf/src/lib.rs:69-105`).
  Add `#[derive(Resource)]`. Already `Clone` (because of the `PathBuf`).
- **Owns:** which world content `setup_test_grid` installs.
- **Default:** `GridPreset::Default`.
- **World:** main only. No render-world mirror — `setup_test_grid` runs at
  `Startup` and the choice never crosses into the render world.
- **Bootstrap:** inserted by `build_app_with_args` from a transient
  `BootstrapInputs.grid_preset` (see §3.2). The native `--vox <path>` flag and
  the wasm32 `?skybox=1` URL param resolution both feed into this single
  insert site BEFORE `build_app_with_args` runs.
- **Mutability:** `Res<GridPreset>`. The Q3-in-scope wasm32 `?skybox=1`
  resolution is moved from `Startup` into `bootstrap_wasm` (see §3.3), so
  the field is set to `WebSkybox` BEFORE the resource is inserted and is
  immutable afterwards. Delete `pub fn startup_fetch_default_vox(mut args: ResMut<AppArgs>...)`'s
  `args.grid_preset = WebSkybox` mutation and the `ResMut` parameter dies.

---

#### `TaaConfig` — main-world + render-world mirror, NEW
- **Type:** `crate::render::taa::TaaConfig { enabled: bool }`. Mirror the
  shape of `ExtractedTaaConfig` at `render/extract.rs:444-448` (which already
  exists as the render-world mirror).
- **Owns:** TAA on/off runtime toggle.
- **Default:** `TaaConfig { enabled: true }` (matches `AppArgs::default().taa`).
- **World:** main + render mirror. The render-world mirror is the EXISTING
  `ExtractedTaaConfig`; only the EXTRACT source changes — from
  `Res<AppArgs>` to `Res<TaaConfig>`.
- **Bootstrap:** `build_app_with_args` inserts the canonical default. No
  CLI flag mutates it today (and the design does NOT add one — out of scope).
- **Mutability:** `Res<TaaConfig>`. Today it's effectively `const true`;
  future settings-panel work could flip it via `ResMut<TaaConfig>` without
  touching the refactor.

---

#### `TaaRingConfig` — main-world canonical + render-world mirror (renamed `RenderTaaRingConfig`)
- **Type:**
  - Main-world new: `TaaRingConfig { depth: u32 }` (same shape as the existing
    render-side struct, but lives in main-world). Default
    `Self { depth: crate::DEFAULT_TAA_RING_DEPTH }`.
  - Render-world: the EXISTING `crate::render::taa::TaaRingConfig` is **renamed**
    `RenderTaaRingConfig` (matches the `EffectiveWorldSize` / `RenderEffectiveWorldSize`
    naming pattern). Keep the `depth: u32` field; nothing in the render-graph
    consumer code shape changes.
- **Owns:** TAA sample-ring depth. C# `WorldRenderBase.cs:17`.
- **Default:** canonical 32 (`DEFAULT_TAA_RING_DEPTH`). The pin test in
  `app_args.rs:217-235` moves with the field — into `taa.rs::tests`.
- **World:** main (canonical / budget-override seat) + render (mirror).
- **Bootstrap:** `build_app_with_args` inserts canonical main-world default.
  `build_app_with_budget` post-build inserts the budget-override value
  (overwrite-in-place — same pattern as the existing `EffectiveWorldSize`
  override at `lib.rs:160-162`). The wasm32 main.rs path does the same.
- **Extract:** new `extract_taa_ring_depth`, mirror of
  `extract_effective_world_size` at `render/extract.rs:489-496`. Reads main
  `Res<TaaRingConfig>`, writes render `RenderTaaRingConfig.depth`.
- **Plugin build:** `NaadfPipelines::from_world` (`render/mod.rs:113-126`)
  continues to read `RenderTaaRingConfig` from the render world; the shader-def
  injection at pipeline specialisation still sees the right value because
  `from_world` runs in `RenderStartup` (AFTER `ExtractSchedule` has run for the
  first time on the render-world resource).

**Critical:** the pre-build mutation of `args.taa_ring_depth` in
`build_app_with_budget` (`lib.rs:158`) is **deleted**. The order becomes:
`build_app_with_args(canonical defaults)` → `app.insert_resource(TaaRingConfig { depth: caps.taa_ring_depth })`
→ extract carries it across.

---

#### `GiSettings` — main-world only, **promoted to a Resource**
- **Type:** `crate::GiSettings` (re-exported from
  `crates/bevy_naadf/src/settings/canonical.rs`). Add `#[derive(Resource)]`.
  Already `Copy + Clone + Default`.
- **Owns:** Phase-B GI pipeline knobs (runtime-tunable via settings panel).
- **Default:** `GiSettings::default()` (which reads `GiSettings::DEFAULTS`).
- **World:** main only. Render-world consumer is the existing
  `ExtractedGiConfig` (`render/extract.rs:503-508`); the EXTRACT source
  changes — from `Res<AppArgs>` reading `args.gi` to `Res<GiSettings>` reading
  the struct directly.
- **Bootstrap:** `build_app_with_args` inserts canonical default.
- **Mutability:** `ResMut<GiSettings>` for settings systems
  (`adjust_settings`, `mouse_interact_settings`, `reset_all_knobs`).

---

#### `ConstructionConfig` — already a resource; the `AppArgs` relay is removed
- **Type:** `crate::render::construction::ConstructionConfig` (unchanged shape).
- **World:** today inserted into render sub-app at
  `render/construction/mod.rs:1864-1866` via `From<&AppArgs>`. Post-refactor:
  inserted into BOTH worlds, with an extract carrying main → render (mirrors
  the `EffectiveWorldSize` / `InvalidSampleStorageCount` pattern).
- **Why both worlds:** the main world needs it for
  `run_gpu_construction_startup` (`render/construction/mod.rs:1788`) and for
  the `spawn_phase_c_test_entity.run_if` (today reading
  `args.spawn_test_entity` — moved to `SpawnTestEntity` resource — see below).
  Render world needs it for the construction systems.
- **Bootstrap:**
  - `build_app_with_args` inserts the canonical main-world default
    `ConstructionConfig::default()`.
  - Wasm32 platform divergence (currently lives in `From<&AppArgs>` at
    `render/construction/config.rs:268-285`) **moves** into a new explicit
    constructor `ConstructionConfig::for_target_arch() -> Self` (see Decision §5).
    `build_app_with_args` calls `ConstructionConfig::for_target_arch()`
    instead of `ConstructionConfig::default()`.
  - The `From<&AppArgs>` impl is **deleted** along with the field.
  - E2e gates that today set `app_args.construction_config.gpu_construction_enabled = true`
    move to `app.insert_resource(ConstructionConfig { gpu_construction_enabled: true, ..ConstructionConfig::for_target_arch() })`
    via a new entry-point helper (see §3.3).
- **Extract:** new `extract_construction_config`, mirror of
  `extract_effective_world_size`. Reads main `Res<ConstructionConfig>`,
  writes render-world `ConstructionConfig`.

---

#### `SpawnTestEntity` — main-world only, new
- **Type:** `crate::render::construction::SpawnTestEntity(pub bool)` (a thin
  newtype tuple Resource, same shape as `InvalidSampleStorageCount`).
- **Owns:** whether `spawn_phase_c_test_entity` fires at `Startup`.
- **Default:** `SpawnTestEntity(false)`.
- **World:** main only. The e2e driver also reads it via
  `Option<Res<SpawnTestEntity>>` (replaces `app_args.spawn_test_entity` at
  `e2e/driver.rs:680`).
- **Bootstrap:** `build_app_with_args` inserts canonical default. The
  `EntitiesBoot` arm in `bin/e2e_render.rs:340-345` inserts
  `SpawnTestEntity(true)` post-build (or pre-build through the entry-point
  shape — see §3.3).

(See Decision §4 for the `Option<TestEntityFixture>` shape considered and
rejected.)

---

#### `E2eGateMode` — main-world only, new enum resource
- **Type:** `crate::e2e::gate::E2eGateMode` (placed alongside the existing
  `GateKind` enum at `crates/bevy_naadf/src/e2e/gate.rs:30-53` — see Decision §6).
- **Variants:** mirrors the existing `GateKind` enum but EXTENDED to fold in
  the 11-bool collapse. Concretely:
  ```
  pub enum E2eGateMode {
      Standard,                  // default — no gate-specific flow
      Resize,                    // was AppArgs.resize_test
      OasisEdit,                 // was AppArgs.oasis_edit_visual_mode
      VoxGpuConstruction,        // was AppArgs.vox_gpu_construction_mode
      SmallEditVisual,           // was AppArgs.small_edit_visual_mode
      SmallEditRepro,            // was AppArgs.small_edit_repro_mode
      VoxGpuOracleCpu,           // was AppArgs.vox_gpu_oracle_cpu_phase
      VoxGpuOracleGpu,           // was AppArgs.vox_gpu_oracle_gpu_phase
      VoxWebParitySkybox,        // was AppArgs.vox_web_parity_skybox_phase
      VoxWebParityLoaded,        // was AppArgs.vox_web_parity_loaded_phase
      VoxHorizonNative,          // was AppArgs.vox_horizon_native_phase
  }
  ```
- **Owns:** which e2e gate flow the driver runs. Single source of truth.
- **Default:** `E2eGateMode::Standard`.
- **World:** main only (driver runs in `Update`).
- **Bootstrap:** `build_app_with_args` inserts `E2eGateMode::Standard`. Each
  per-gate `run_*` builder (after §3.3 refactor) inserts the appropriate
  variant. The `parse_gate_command` in `bin/e2e_render.rs:256-324` already
  builds a `BootCommand` enum that nearly maps 1:1 — see §3.2.
- **Mutability:** `Res<E2eGateMode>`. Once set at boot, immutable.

**Note on `GateKind` already existing:** the existing enum at `gate.rs:30-53`
is the half-done refactor that Q2 mentions. It is currently dead-coded
(`#![allow(dead_code)]` at `:18`). The collapse REPLACES that enum (renames it,
adds the missing variants — `Standard` already had `VoxE2e` cases lumped in
because `vox_e2e_mode` is the only mode-boolean that's NOT a state-machine
selector in the driver, just an ASSERT-time flag; see Decision §3).

---

#### `VoxE2eAssertion` — main-world only, new
- **Type:** `crate::e2e::VoxE2eAssertion(pub bool)`.
- **Owns:** the ASSERT-time choice between default-scene region gate and
  `assert_vox_geometry_visible`. Today gated by `app_args.vox_e2e_mode` at
  `e2e/driver.rs:688`.
- **Default:** `VoxE2eAssertion(false)`.
- **Bucket:** Bucket A (per Decision §3). The driver reads it like an option,
  not like an enum branch.
- **Bootstrap:** `build_app_with_args` inserts canonical default; the
  `run_vox_e2e` builder inserts `VoxE2eAssertion(true)`.

---

#### `EffectiveWorldSize`, `InvalidSampleStorageCount` — UNCHANGED
Per the constraints, these two existing mobile-divergence resources stay
as-is. The refactor's new resources reuse their PATTERN but do not touch
either type.

---

#### `BootstrapInputs` — transient parser struct, NEW (not a Resource)
- **Type:** `crate::bootstrap::BootstrapInputs` in a new `src/bootstrap.rs`
  module (or `app_args.rs` can be repurposed as `bootstrap.rs` — see
  Decision §1).
- **Owns:** all the CLI-parsed / argv-derived / URL-param-derived values that
  feed the resource fan-out at bootstrap time. NOT a Resource — consumed once
  by `build_app_with_args` and dropped.
- **Shape:**
  ```
  pub struct BootstrapInputs {
      pub grid_preset: GridPreset,
      pub gate_mode: E2eGateMode,
      pub vox_e2e_assertion: VoxE2eAssertion,
      pub spawn_test_entity: SpawnTestEntity,
      pub construction_overrides: ConstructionOverrides, // see below
      pub taa: TaaConfig,
      pub taa_ring_depth: TaaRingConfig,
      pub gi: GiSettings,
  }
  ```
- **`ConstructionOverrides`:** a tiny `Option<bool>`-style struct that only
  carries the fields any caller actually mutates today (today only
  `gpu_construction_enabled` and `entities_enabled` are touched at boot
  time). Applied as a delta on top of `ConstructionConfig::for_target_arch()`.
- **Default:** `BootstrapInputs::default()` — fan-out gives canonical defaults
  per resource (matches today's `AppArgs::default()` byte-for-byte).
- **Lifetime:** constructed by the per-binary entry point (with values
  derived from CLI + budget probe + URL params), consumed by
  `build_app_with_args` which inserts each field as its own resource, dropped.

(This is the "transient parser type that exists only at bootstrap" candidate
the brief flags — modelled exactly on `BudgetCaps` at `render/budget.rs:261-281`.)

### 3.2 CLI parser shape

The architectural backbone is the three-bucket taxonomy. The parser layer
becomes:

#### Layer 1 — Bucket C (action verbs): no Bevy boot
**Unchanged.** `parse_top_level_short_circuit` at `bin/e2e_render.rs:168-185`
already implements this. The five short-circuits return an
`ExitCode` without booting any App.

#### Layer 2 — Bucket B + Bucket A: produce a `BootstrapInputs`
**New shape.** The parser produces ONE `BootstrapInputs` struct per invocation.
For `bin/e2e_render.rs`, this replaces `parse_gate_command` returning a
`BootCommand` + the per-gate `run_*` builder mutating `AppArgs`. Concretely:

```
fn parse_bootstrap_inputs(args: &[String]) -> BootstrapInputs {
    let gate_mode = parse_gate_mode(args);              // bucket B
    let grid_preset = parse_grid_preset(args, gate_mode); // bucket A (derived from gate_mode for e2e)
    let vox_e2e_assertion = parse_vox_e2e(args);        // bucket A
    let spawn_test_entity = parse_spawn_test_entity(args, gate_mode); // bucket A
    let construction_overrides = parse_construction(args, gate_mode); // bucket A
    // taa/taa_ring_depth/gi take canonical defaults today (no CLI surface).
    BootstrapInputs {
        gate_mode, grid_preset, vox_e2e_assertion, spawn_test_entity,
        construction_overrides,
        taa: TaaConfig::default(),
        taa_ring_depth: TaaRingConfig::default(),  // override by budget post-build
        gi: GiSettings::default(),
    }
}
```

**Per-gate `run_*` builders collapse.** Today each builder (`run_oasis_edit_visual`,
`run_small_edit_repro`, etc.) is a 5-10-line function that constructs
`AppArgs::default()`, mutates 1-3 fields, and calls `run_e2e_render_with_args(app_args)`.
Post-refactor: each becomes a single `BootstrapInputs::for_<gate>(args)` constructor
returning a fully-populated struct, then the entry point calls
`run_e2e_render_with_bootstrap(inputs)` (renamed from `run_e2e_render_with_args`).

The per-gate constructor encodes the gate's contract — e.g.
`BootstrapInputs::for_oasis_edit_visual(oasis_path)` sets
`gate_mode = OasisEdit`, `grid_preset = GridPreset::Vox { path: oasis_path }`,
everything else canonical default. This is the **clean factoring** of the
"`AppArgs::default()` + flip 1-3 fields" idiom that's 15 times duplicated today.

#### Layer 3 — Bucket C post-app validations
**Unchanged.** `parse_post_app_validations` at `bin/e2e_render.rs:447-454`
already implements this. The four post-app flags (`--validate-gpu-construction`,
`--entities`, `--edit-mode`, `--runtime-edit-mode`) are pure action verbs that
the parser routes into validation functions after the App exits.

#### Production binary (`main.rs`)
`parse_bootstrap_inputs(argv)` returns a `BootstrapInputs` with at most one
non-default field (`grid_preset = Vox { path }` for `--vox`). The rest is the
budget post-build override. No e2e gates, no mode-flag parsing.

#### Android entry (`android_main.rs`)
`BootstrapInputs::default()` — no CLI surface. Budget post-build override
inserts the mobile rungs.

### 3.3 Bootstrap orchestration

Four entry points, all converging on a single fan-out function.

```
                          BootstrapInputs
                                ↓
                  build_app_with_bootstrap_inputs(cfg, inputs)
                                ↓
              inserts per-domain resources, returns App
```

#### `build_app_with_bootstrap_inputs(cfg: AppConfig, inputs: BootstrapInputs) -> App`
Replaces `build_app_with_args`. Fans `inputs` into per-domain resource inserts.
Body sketch:

```
app.insert_resource(cfg);
app.insert_resource(inputs.grid_preset);
app.insert_resource(inputs.gate_mode);
app.insert_resource(inputs.vox_e2e_assertion);
app.insert_resource(inputs.spawn_test_entity);
app.insert_resource(inputs.taa);
app.insert_resource(inputs.taa_ring_depth);
app.insert_resource(inputs.gi);

let mut cc = ConstructionConfig::for_target_arch();
inputs.construction_overrides.apply(&mut cc);
app.insert_resource(cc);

// Defensive seeds for budget-derived resources stay the same — EffectiveWorldSize,
// InvalidSampleStorageCount unchanged.
if !app.world().contains_resource::<EffectiveWorldSize>() {
    app.insert_resource(EffectiveWorldSize::canonical());
}
// ...same for InvalidSampleStorageCount.

// init_resource for CameraHistory stays.
app.init_resource::<render::taa::CameraHistory>();

// Rest of plugin pyramid identical to today.
```

The order matters: every resource must be inserted **before** plugins are added
(specifically, the existing `NaadfRenderPlugin::build` reads `args.taa_ring_depth`
at `render/mod.rs:113-126` — post-refactor it reads `Res<TaaRingConfig>`
instead, which must be present at that point).

**E2e-path byte-identical defaults.** `BootstrapInputs::default()` produces:
- `gate_mode = Standard`, `grid_preset = Default`, `vox_e2e_assertion = false`,
  `spawn_test_entity = false`, `taa = TaaConfig { enabled: true }`,
  `taa_ring_depth = TaaRingConfig { depth: 32 }`, `gi = GiSettings::default()`,
  `construction_overrides = empty` (i.e. canonical `ConstructionConfig::for_target_arch()`).

This is BYTE-IDENTICAL to what `AppArgs::default()` produces today, because
every field default carries through. The defensive seeds at `lib.rs:236-249`
stay in place. The e2e gate determinism requirement is satisfied.

#### `build_app(cfg: AppConfig) -> App` (replaces today's wrapper at `lib.rs:132-134`)
Just `build_app_with_bootstrap_inputs(cfg, BootstrapInputs::default())`.

#### `build_app_with_budget(cfg: AppConfig, inputs: BootstrapInputs) -> App`
Refactored from `lib.rs:156-167`. New shape:
```
pub fn build_app_with_budget(cfg: AppConfig, mut inputs: BootstrapInputs) -> App {
    let caps = render::budget::probe_and_select();
    inputs.taa_ring_depth = TaaRingConfig { depth: caps.taa_ring_depth };
    let mut app = build_app_with_bootstrap_inputs(cfg, inputs);
    app.insert_resource(EffectiveWorldSize::from_segments(caps.world_size_in_segments));
    app.insert_resource(InvalidSampleStorageCount(caps.invalid_sample_storage_count));
    app
}
```
The pre-build mutation that today writes `args.taa_ring_depth` is replaced by
the pre-build `inputs.taa_ring_depth` mutation — same shape, cleaner because
`TaaRingConfig` is its own type.

#### `run_e2e_render_with_bootstrap_inputs(inputs: BootstrapInputs) -> AppExit`
Replaces `run_e2e_render_with_args` at `lib.rs:412-421`:
```
pub fn run_e2e_render_with_bootstrap_inputs(inputs: BootstrapInputs) -> AppExit {
    let mut cfg = AppConfig::e2e();
    cfg.window = window_config::window_for_gate_mode(inputs.gate_mode);  // see below
    let app = build_app_with_bootstrap_inputs(cfg, inputs);
    e2e::run_with_app(app)
}
```

#### Wasm32 bootstrap (`main.rs:73-91`)
The `?skybox=1` resolution moves OUT of `web_vox::startup_fetch_default_vox`
into a new `fn resolve_wasm_url_params() -> WasmUrlOverrides { … }` called
from `main.rs::main()`'s wasm32 branch. The wasm32 branch becomes:
```
wasm_bindgen_futures::spawn_local(async move {
    let caps = bevy_naadf::render::budget::probe_and_select_async().await;
    let overrides = bevy_naadf::voxel::web_vox::resolve_wasm_url_params();
    let mut inputs = BootstrapInputs::default();
    if overrides.skybox_only {
        inputs.grid_preset = GridPreset::WebSkybox;
    }
    // --vox flag (native shape) is irrelevant on wasm32.
    inputs.taa_ring_depth = TaaRingConfig { depth: caps.taa_ring_depth };
    let mut app = bevy_naadf::build_app_with_bootstrap_inputs(AppConfig::windowed(), inputs);
    app.insert_resource(EffectiveWorldSize::from_segments(caps.world_size_in_segments));
    app.insert_resource(InvalidSampleStorageCount(caps.invalid_sample_storage_count));
    app.run();
});
```
After this move, `startup_fetch_default_vox` no longer needs `ResMut<AppArgs>`
(it never needed it for anything other than the `?skybox=1` resolution).
The `?pose=horizon` and `?ui=hide` resolvers are unrelated and stay where
they are (they insert separate marker resources).

#### `window_for_gate_mode(gate: E2eGateMode) -> WindowConfig`
Replaces `window_for_e2e_args(args: &AppArgs)` at `window_config.rs:153-163`.
Reads the gate variant instead of the boolean fields:
```
pub fn window_for_gate_mode(gate: E2eGateMode) -> WindowConfig {
    match gate {
        E2eGateMode::Resize => WindowConfig::e2e_resize_test(),
        E2eGateMode::SmallEditRepro => WindowConfig::e2e_small_edit_repro(),
        E2eGateMode::VoxHorizonNative => WindowConfig::e2e_horizon(),
        _ => WindowConfig::e2e(),
    }
}
```

### 3.4 Cross-world mirroring

Per-resource extract wiring, all in the existing `add_systems(ExtractSchedule, ...)`
block at `render/mod.rs:172-191`.

| Resource | Extract system | Pattern source |
|---|---|---|
| `TaaConfig` | `extract_taa_config` (existing; source switches from `Res<AppArgs>` to `Res<TaaConfig>`) | already at `render/extract.rs:452-459` |
| `TaaRingConfig` → `RenderTaaRingConfig` | NEW `extract_taa_ring_depth` | mirror of `extract_effective_world_size` at `render/extract.rs:489-496` |
| `GiSettings` | `extract_gi_config` (existing; source switches from `Res<AppArgs>` to `Res<GiSettings>`) | already at `render/extract.rs:513-520` |
| `ConstructionConfig` | NEW `extract_construction_config` | mirror of `extract_effective_world_size`; writes render-world `ConstructionConfig` |
| `EffectiveWorldSize` / `InvalidSampleStorageCount` | unchanged | already at `render/extract.rs:470-477, 489-496` |

The render-world `init_resource::<RenderTaaRingConfig>()` replaces the
plugin-build snapshot at `render/mod.rs:113-126`. The plugin's
`taa_ring_depth = app.world().get_resource::<AppArgs>().map(|args| args.taa_ring_depth).unwrap_or(DEFAULT_TAA_RING_DEPTH)`
goes away entirely.

`NaadfPipelines::from_world` (which reads `RenderTaaRingConfig` for the
WGSL `#{TAA_SAMPLE_RING_DEPTH}` shader-def) still works because it runs in
`RenderStartup` AFTER the first `ExtractSchedule` has populated the mirror —
the mirror's `init_resource` default is `DEFAULT_TAA_RING_DEPTH`, and the
extract overwrites it from the main-world value before `RenderStartup`. This
is the same first-frame story as `RenderEffectiveWorldSize`.

For `ConstructionConfig`, two consumers need to be checked:
1. **`run_gpu_construction_startup`** at `render/construction/mod.rs:1788`
   reads `args.construction_config.gpu_construction_enabled`. Post-refactor:
   `Res<ConstructionConfig>`.
2. **Plugin build** at `render/construction/mod.rs:1834-1836`. Post-refactor:
   the plugin-build snapshot is **removed**; the render world `init_resource::<ConstructionConfig>()`
   with `for_target_arch()` default, then `extract_construction_config` carries
   the main-world value across every frame.

### 3.5 Cross-cutting consumer adaptations

#### Diagnostics — per Q4, pick fan-out
`dump_diagnostics_on_p` at `diagnostics.rs:23-126` gains 6 new optional
parameters and drops `Option<Res<AppArgs>>`:
```
pub fn dump_diagnostics_on_p(
    keys: Res<ButtonInput<KeyCode>>,
    grid_preset: Option<Res<GridPreset>>,
    taa: Option<Res<TaaConfig>>,
    taa_ring: Option<Res<TaaRingConfig>>,
    spawn_test_entity: Option<Res<SpawnTestEntity>>,
    gi: Option<Res<GiSettings>>,
    construction: Option<Res<ConstructionConfig>>,
    // ...the existing world_data/voxel_types/window/camera_q parameters stay.
)
```
Each diagnostic line at `:104-119` reformats around the new accessors.

#### Settings panel — per Q4, fan-out via two readonly-knob variants
The two `Readonly` knobs at `settings/mod.rs:262, 268` change signature:
- `:262` — `knob_readonly!("taa_ring_depth", |a: &TaaRingConfig| format!("{} [restart-required]", a.depth))`
- `:268` — `knob_readonly!("global_illum_max_accum", |g: &GiSettings| format!("{} [const]", g.global_illum_max_accum))`

The cleanest factoring is to split `KnobKind::Readonly` into typed variants:
```
KnobKind::ReadonlyFromTaa { value: fn(&TaaRingConfig) -> String }
KnobKind::ReadonlyFromGi  { value: fn(&GiSettings) -> String }
```
…and `update_settings_text` at `:641-711` takes `Res<TaaRingConfig>` and
`Res<GiSettings>` as separate parameters, dispatching the readonly closure
against the right resource based on its variant. The existing
`KnobKind::U32/F32/Bool` interactive knobs already operate against `&GiSettings`
via their `getter` closure, so they need no signature change — only the system
parameter for the panel changes from `ResMut<AppArgs>` to `ResMut<GiSettings>`.

`reset_all_knobs` at `:274-283` signature becomes `fn(&mut GiSettings)`. The
`KnobKind::Action { apply: fn(&mut GiSettings) }` follows.

## 4. Migration plan

Numbered, field-by-field. Each step is independently testable and ends with
a clean commit. Verification gates each step (see §5).

Step counts of "consumer sites touched" come from the inventory in §1.1.

---

### Step 1 — Introduce `BootstrapInputs` (no behaviour change)

Add the new `BootstrapInputs` struct + module + `Default` impl. Add the new
fan-out entry point `build_app_with_bootstrap_inputs` AS A WRAPPER around the
existing `build_app_with_args` — it just constructs an `AppArgs` from the
inputs and forwards. Add `run_e2e_render_with_bootstrap_inputs` as a wrapper
around `run_e2e_render_with_args`.

**Resources introduced:** none yet.

**Sites touched:** 1 (new `src/bootstrap.rs`).

**Verification:** `cargo build --workspace`, `cargo test --workspace --lib`.
Nothing else should change.

**Diff size:** ~150 LOC new file.

---

### Step 2 — Migrate `taa_ring_depth` (the user's named field)

This is the load-bearing first step — it exercises the full extract pattern
and proves the mobile-divergence override path works for non-budget-resources.

- Introduce `TaaRingConfig` main-world resource at
  `crates/bevy_naadf/src/render/taa.rs` (alongside the existing render
  `TaaRingConfig` which is renamed to `RenderTaaRingConfig`).
- Move the pin tests from `app_args.rs:213-235` to `render/taa.rs::tests`.
- Add `extract_taa_ring_depth` in `render/extract.rs` (mirror of
  `extract_effective_world_size`).
- Wire the extract in `render/mod.rs:172-191`.
- `init_resource::<RenderTaaRingConfig>()` in `render/mod.rs` replaces the
  plugin-build snapshot at `:113-126`.
- `build_app_with_bootstrap_inputs` now inserts `TaaRingConfig` and stops
  forwarding it through `AppArgs`. The compatibility wrapper still constructs
  `AppArgs` for the OTHER fields.
- `build_app_with_budget` no longer mutates `inputs.taa_ring_depth` via
  `AppArgs` — it sets `inputs.taa_ring_depth = TaaRingConfig { depth: caps.taa_ring_depth }`.
- `main.rs:77` wasm32 path likewise.
- `AppArgs.taa_ring_depth` field is **deleted**.

**Sites touched:**
- Delete field at `app_args.rs:42`, `:190`.
- Delete tests at `app_args.rs:209-236` (relocated).
- `lib.rs:158` mutation deleted (wrapper now sets `inputs.taa_ring_depth`).
- `main.rs:77` mutation rewritten.
- `render/mod.rs:113-126` snapshot deleted; `init_resource` + extract wire added.
- `diagnostics.rs:109` reads from `Res<TaaRingConfig>`.
- `settings/mod.rs:262` knob signature changes (intermediate: keep
  `fn(&AppArgs)` accepting an empty AppArgs proxy if `AppArgs` is still
  present for other fields — or move this row's update to read
  `Res<TaaRingConfig>` directly via new system parameter).

**Verification gates:**
- `cargo build --workspace` — compile clean.
- `cargo test --workspace --lib` — including the relocated pin tests.
- `cargo run --bin e2e_render -- baseline` — e2e gate green (canonical default
  carries through, byte-identical framebuffer).
- On-device deploy (mobile-affecting field — Android Mali budget overrides
  TAA depth to 8 today, so post-deploy log should show `[budget] … taa_ring_depth = 8`).

**Diff size:** ~80 LOC across 8 files.

---

### Step 3 — Migrate `taa` and `gi` (the other extract-based fields)

These two are mechanically symmetric — both currently flow through
`Extract<Option<Res<AppArgs>>>` to their `Extracted*Config` mirrors. Migrate
together as one commit so the extract system signatures change once.

- Introduce `TaaConfig` main-world resource at
  `crates/bevy_naadf/src/render/taa.rs`. Default `enabled: true`.
- `GiSettings` is already a struct — just add `#[derive(Resource)]` at
  `crates/bevy_naadf/src/settings/canonical.rs`. (Verify the `pub use` at
  `lib.rs:35` still exposes it correctly.)
- Change `extract_taa_config` source from
  `Extract<Option<Res<AppArgs>>>` to `Extract<Option<Res<TaaConfig>>>`.
- Change `extract_gi_config` source likewise to `Extract<Option<Res<GiSettings>>>`.
- `update_camera_history` at `render/taa.rs:188-224` takes `Res<TaaConfig>`
  instead of `Res<AppArgs>`.
- Settings panel: `adjust_settings`, `mouse_interact_settings`, the readonly
  `update_settings_text`, `reset_all_knobs` — all take `ResMut<GiSettings>` /
  `Res<GiSettings>` instead of `ResMut<AppArgs>`.
- Delete `AppArgs.taa` and `AppArgs.gi` fields.

**Sites touched:**
- `app_args.rs:32, 44, 189, 191` field + default deletes.
- `render/extract.rs:452-459, 513-520` extract source change (2 systems).
- `render/taa.rs:188-224` `update_camera_history` parameter swap.
- `settings/mod.rs:138-140, 277-283, 458, 488, 495, 498, 505, 508, 511, 525-528, 612, 614, 620-621, 631-632, 643, 660, 664, 668, 832-840` — 21 sites; mostly
  swap `args.gi` → `gi` (where `gi` is the new `ResMut<GiSettings>` parameter).
- `diagnostics.rs:108, 111` reads from `Res<TaaConfig>` / `Res<GiSettings>`.

**Verification gates:**
- `cargo build --workspace`.
- `cargo test --workspace --lib` (including the existing settings panel test
  at `settings/mod.rs:825-841` which currently mutates `args.gi.*` — assertion
  semantics preserved by swapping `args` for the new resource).
- `cargo run --bin e2e_render -- baseline` — TAA/GI are part of the standard
  render path; if either extract is broken, the GI bounce stops compositing
  and the gate's `solid_block_rect` luminance assertion will fail.
- `cargo run --bin e2e_render -- --vox-horizon-native` — TAA-heavy gate.
- User visual check: launch production binary, press Escape, toggle a couple
  of GI knobs, confirm the live image responds.

**Diff size:** ~100 LOC across ~8 files.

---

### Step 4 — Migrate `construction_config` (the relay-already-resource case)

- Introduce main-world `ConstructionConfig` resource (the type already exists
  in `render/construction/config.rs` — just add `#[derive(Resource)]` if not
  present — wait, the `Resource` derive is at `:35`, so it IS present already.
  Just need to insert it into the MAIN world too).
- Move the wasm32 platform divergence from `From<&AppArgs>` (`config.rs:265-288`)
  into `ConstructionConfig::for_target_arch() -> Self`.
- **Delete** `impl From<&crate::AppArgs> for ConstructionConfig`.
- `render/construction/mod.rs:1832-1836` plugin-build snapshot is replaced by
  `init_resource::<ConstructionConfig>()` on the render sub-app.
- Add `extract_construction_config` to `render/extract.rs` (mirror of
  `extract_effective_world_size`).
- Wire the extract in `render/mod.rs:172-191`.
- `run_gpu_construction_startup` at `:1787` parameter changes from
  `Res<crate::AppArgs>` to `Res<ConstructionConfig>`.
- E2e gates that mutate `app_args.construction_config.<field>` (5 sites) move
  to setting `inputs.construction_overrides.<field> = Some(value)` via the
  `BootstrapInputs` constructor.
- Delete `AppArgs.construction_config` field.

**Sites touched:**
- `app_args.rs:55, 192` field + default deletion.
- `render/construction/config.rs:252-288` `From` impl deletion; relocate
  wasm32 divergence to `for_target_arch`.
- `render/construction/mod.rs:1787-1804, 1832-1836` consumer + plugin
  shape change.
- `render/mod.rs:172-191` extract wire.
- `bin/e2e_render.rs:342` `EntitiesBoot` arm constructs
  `BootstrapInputs::for_entities_boot()`.
- 4 e2e gate `run_*` builders: `vox_gpu_construction.rs:234`,
  `vox_gpu_oracle.rs:334`, `vox_horizon_parity.rs:158`, `vox_web_parity.rs:200`.
- `diagnostics.rs:112` reads from `Res<ConstructionConfig>`.

**Verification gates:**
- `cargo build --workspace`.
- `cargo test --workspace --lib` (including the `const _: () = { … }`
  compile-time pin at `config.rs:290-326`).
- `cargo run --bin e2e_render -- --validate-gpu-construction` (the construction
  GPU oracle gate).
- `cargo run --bin e2e_render -- --vox-gpu-construction` (the main W5 gate).
- On wasm32: the `n_bounds_rounds = 1` clamp at `config.rs:283-284` is on the
  critical path for wasm chunk-AADF non-determinism (see the long docblock at
  `:218-251`). Verify with `just web-static + just test-wasm-full`.

**Diff size:** ~120 LOC across ~10 files.

---

### Step 5 — Migrate `grid_preset` (Q3 included)

- Promote `GridPreset` to a `Resource` (add derive at `lib.rs:69`).
- Move the `?skybox=1` resolution from `web_vox::startup_fetch_default_vox`
  at `voxel/web_vox.rs:390-405` to a new `resolve_wasm_url_params()` helper
  called from `main.rs::main()`'s wasm32 branch BEFORE
  `build_app_with_bootstrap_inputs`.
- Delete the `args.grid_preset = WebSkybox` mutation; `startup_fetch_default_vox`
  loses its `ResMut<crate::AppArgs>` parameter.
- `setup_test_grid` at `voxel/grid.rs:121` takes `Res<GridPreset>` instead of
  `Res<AppArgs>` (the only other AppArgs read in that fn is
  `args.vox_gpu_oracle_cpu_phase` at `:132` — split into a separate
  `Res<E2eGateMode>` read in Step 6 once `E2eGateMode` lands; for now, keep
  reading `AppArgs.vox_gpu_oracle_cpu_phase` and migrate that arm in Step 6).
- The 7 e2e gate `run_*` builders that set `app_args.grid_preset = …` move to
  `BootstrapInputs::for_<gate>(…)` constructors carrying the grid_preset.
- `main.rs:45` argv `--vox <path>` mutates `inputs.grid_preset` instead.
- Delete `AppArgs.grid_preset` field.

**Sites touched:**
- `app_args.rs:27, 188` field + default delete.
- `main.rs:39, 41-52` argv parsing rewritten to build `BootstrapInputs`.
- `voxel/web_vox.rs:390-405` Startup-time mutation deleted; new
  `resolve_wasm_url_params()` helper added.
- `voxel/grid.rs:121, 127, 131, 144, 147` `setup_test_grid` parameter swap.
- 7 e2e gate `run_*` builders (above).
- `diagnostics.rs:107, 113` reads from `Res<GridPreset>`.

**Verification gates:**
- `cargo build --workspace`.
- `cargo test --workspace --lib`.
- `cargo run --bin e2e_render -- --vox-e2e`.
- `cargo run --bin e2e_render -- --vox-horizon-native`.
- `just web-static + just test-wasm-full` — exercises the `?skybox=1` URL-param
  path which was relocated.
- User visual check on the production binary with `--vox <path>`.

**Diff size:** ~150 LOC across ~12 files.

---

### Step 6 — Collapse the 11 e2e-mode booleans into `E2eGateMode`

The biggest single step. Pair this with the half-done `GateKind` dispatch
refactor at `bin/e2e_render.rs:111-122` per Q2.

- Promote/extend `GateKind` at `crates/bevy_naadf/src/e2e/gate.rs:30-53` into
  `E2eGateMode` (rename + add missing variants; remove the
  `#![allow(dead_code)]` at `:18`). Add `#[derive(Resource)]`.
- Add `parse_gate_mode(args: &[String]) -> E2eGateMode` adjacent to the
  existing `parse_gate_command` in `bin/e2e_render.rs`. Cite the AppArgs
  field-by-field mapping in §2.3.
- Per-gate `run_*` builders move from "`AppArgs::default()` + mutate" to
  "construct `BootstrapInputs::for_<gate>(<path>)`". `crate::run_e2e_render_with_bootstrap_inputs`
  takes the new struct.
- Driver code at `e2e/driver.rs` lines 475, 510, 513, 527, 538, 549, 561,
  562, 563, 1525, 1528, 1598, 1601, 1604 (14 sites total) read
  `Res<E2eGateMode>` instead of the matching `AppArgs.<flag>`. Many sites
  become `gate == E2eGateMode::Foo` checks.
- `window_for_e2e_args(args: &AppArgs)` at `window_config.rs:153-163` →
  `window_for_gate_mode(gate: E2eGateMode)`. Three reads on `args` → three
  pattern matches on `gate`.
- 4 per-gate `pin_*_camera` systems read `Option<Res<E2eGateMode>>` instead
  of `Option<Res<AppArgs>>`. Each system body's `if !args.<flag>` becomes
  `if !matches!(gate, &E2eGateMode::Foo)`.
- `voxel/grid.rs:132` `if args.vox_gpu_oracle_cpu_phase` →
  `if matches!(*gate_mode, E2eGateMode::VoxGpuOracleCpu)` (with the system
  taking `Res<E2eGateMode>` instead of via `AppArgs`).
- Delete all 11 boolean fields from `AppArgs`.
- `bin/e2e_render.rs`'s `BootCommand::ResizeTest`, `BootCommand::EntitiesBoot`,
  `BootCommand::Standard` arms move to inserting matching variants of
  `BootstrapInputs::for_resize_test()` / `BootstrapInputs::for_entities_boot()`
  / `BootstrapInputs::default()`.

**Sites touched:**
- `app_args.rs:80, 93, 103, 113, 122, 144, 153, 162, 167, 174, 182, :194-204`
  — 11 field deletions + 11 default-line deletions.
- `e2e/gate.rs:18, 30-53` — rename / extend / remove dead-code allow.
- `e2e/driver.rs:475, 510, 513, 527, 538, 549, 561-563, 1525, 1528, 1598-1604`
  — 14 sites swap.
- `e2e/oasis_edit_visual.rs:211, 307-313`,
  `e2e/small_edit_visual.rs:211, 256-262`,
  `e2e/small_edit_repro.rs:155, 164-168`,
  `e2e/vox_e2e.rs:379, :372 comment`,
  `e2e/vox_gpu_construction.rs:243, 271-276`,
  `e2e/vox_gpu_oracle.rs:289, 335, 643-647`,
  `e2e/vox_horizon_parity.rs:159, 173-177`,
  `e2e/vox_web_parity.rs:160, 201, 388-392` — 8 gate files × 2-3 sites each.
- `window_config.rs:28-29 (import), 153-163` — rename + signature.
- `voxel/grid.rs:121, 132` — parameter swap.
- `bin/e2e_render.rs:111-122, 256-324, 340-345, 377-378, 451-454` —
  enum + builder + arm rewrites.
- `lib.rs:412-421` `run_e2e_render_with_args` → `run_e2e_render_with_bootstrap_inputs`.

**Verification gates:**
- `cargo build --workspace`.
- `cargo test --workspace --lib`.
- **Every e2e gate** — `cargo run --bin e2e_render -- <mode>` for all gates:
  `baseline`, `--vox-e2e`, `--oasis-edit-visual`, `--small-edit-visual`,
  `--small-edit-repro`, `--vox-gpu-construction`, `--vox-gpu-oracle-cpu` +
  `--vox-gpu-oracle-gpu` + `--vox-gpu-oracle` (the compare), `--vox-web-parity-skybox`
  + `--vox-web-parity-loaded` + `--vox-web-parity`, `--vox-horizon-native`,
  `--resize-test`. **The collapse is byte-identical only if every gate
  passes.** This is the largest blast-radius step — flag for fresh-eyes
  review at completion.

**Diff size:** ~300-400 LOC across ~20 files. The single largest step.

---

### Step 7 — Extract `vox_e2e_mode` to `VoxE2eAssertion` (Bucket A, Decision §3)

Even though `vox_e2e_mode` is morphologically a mode-flag, the driver reads it
ONLY at ASSERT time to swap the region-gate assertion — it is NOT a state-machine
selector. Promote to its own Bucket A resource.

- Add `VoxE2eAssertion(bool)` Resource at `crates/bevy_naadf/src/e2e/mod.rs`.
- `e2e/driver.rs:688` reads `Res<VoxE2eAssertion>` instead of `AppArgs.vox_e2e_mode`.
- `e2e/vox_e2e.rs:373-389` `run_vox_e2e` sets `vox_e2e_assertion = VoxE2eAssertion(true)`
  in its `BootstrapInputs::for_vox_e2e()`. Also sets `gate_mode = E2eGateMode::Standard`
  (the vox-e2e gate uses the standard driver flow + standard 256×256 window).

**Sites touched:**
- 3 sites: new resource, driver-side read, vox_e2e builder.

**Verification:**
- `cargo run --bin e2e_render -- --vox-e2e` — the only gate exercising this
  flag.

**Diff size:** ~30 LOC.

---

### Step 8 — Extract `spawn_test_entity` to `SpawnTestEntity`

- Add `SpawnTestEntity(bool)` Resource at
  `crates/bevy_naadf/src/render/construction/mod.rs` adjacent to the existing
  `MainWorldEntities` resource.
- `render/construction/mod.rs:1853` `.run_if(|args: Res<AppArgs>| args.spawn_test_entity)`
  → `.run_if(|s: Option<Res<SpawnTestEntity>>| s.is_some_and(|s| s.0))`.
- `e2e/driver.rs:680` reads `Option<Res<SpawnTestEntity>>`.
- `bin/e2e_render.rs:343` `EntitiesBoot` arm sets `inputs.spawn_test_entity = SpawnTestEntity(true)`.
- `diagnostics.rs:110` reads from `Res<SpawnTestEntity>`.

**Sites touched:** 4.

**Verification:**
- `cargo run --bin e2e_render -- --entities`.

**Diff size:** ~25 LOC.

---

### Step 9 — Delete the now-empty `AppArgs` shell

After Steps 2-8, `AppArgs` has zero fields. `BootstrapInputs` is the canonical
shape.

- Delete `crates/bevy_naadf/src/app_args.rs` entirely (renamed/contents
  superseded by `crates/bevy_naadf/src/bootstrap.rs` from Step 1).
- Remove `pub use app_args::AppArgs;` from `lib.rs:32`.
- Remove all remaining `use crate::AppArgs` / `Res<AppArgs>` imports across
  the workspace (should be NONE remaining after Steps 2-8; the build will
  surface any straggler).

**Sites touched:** 1 file deleted, 1-2 imports removed.

**Verification:**
- `cargo build --workspace` — proves no remaining `AppArgs` reference.
- `cargo test --workspace --lib`.
- Full e2e sweep — same as Step 6.

**Diff size:** ~10 LOC + 240-line file deletion.

---

## 5. Verification surface

The project's verification surface per `CLAUDE.md`:
- `cargo build --workspace`
- `cargo test --workspace --lib`
- `cargo run --bin e2e_render -- <mode>` (with the full mode list above)
- `just web-static + just test-wasm-full` for wasm32 paths
- On-device deploy for mobile-affected fields (TAA ring depth, world size,
  invalid-sample storage)

| Step | `cargo build` | `cargo test --lib` | e2e gates | wasm | mobile |
|---|---|---|---|---|---|
| 1 (BootstrapInputs) | yes | yes | `baseline` | — | — |
| 2 (taa_ring_depth) | yes | yes (incl. pin tests) | `baseline`, `--vox-horizon-native` | — | **yes** (TAA depth mobile-override) |
| 3 (taa + gi) | yes | yes (incl. settings panel test at `settings/mod.rs:825-841`) | `baseline`, `--vox-horizon-native`, `--oasis-edit-visual` (TAA-heavy) | — | — |
| 4 (construction_config) | yes | yes (incl. compile-time pin at `config.rs:290-326`) | `--validate-gpu-construction`, `--vox-gpu-construction`, `--vox-gpu-oracle` | yes (wasm32 platform divergence) | — |
| 5 (grid_preset, Q3 wasm32) | yes | yes | `--vox-e2e`, `--vox-horizon-native` | **yes** (`?skybox=1` URL path) | — |
| 6 (E2eGateMode 11→1) | yes | yes | **EVERY GATE** — the load-bearing verification step | yes (`--vox-web-parity-skybox/loaded`) | — |
| 7 (VoxE2eAssertion) | yes | yes | `--vox-e2e` | — | — |
| 8 (SpawnTestEntity) | yes | yes | `--entities` | — | — |
| 9 (delete AppArgs shell) | yes (proves absence) | yes | full sweep | yes | — |

**User visual checks (not automated, user does these):**
- Step 3 — after switching settings panel to `ResMut<GiSettings>`, launch the
  production binary, press Escape, drag a couple of knobs, confirm the live
  image responds.
- Step 5 — `cargo run --bin bevy-naadf -- --vox <path>` on a known fixture,
  confirm the loaded scene renders.
- Step 6 — full mode sweep with on-screen verification of any gate the user
  has a strong recall of.

**No new unit tests required** for any step except Step 1's `BootstrapInputs`
construction (one trivial Default-test, optional). The conformance gate is
the existing e2e + the existing pin tests as they relocate to new homes.

The TAA ring depth mobile-override path (Step 2) and the wasm32 divergence
on `ConstructionConfig` (Step 4) are the only steps that require deploys.
Both already have device-coverage logs (`[budget] …` line on Android,
`[budget] wasm32 …` line in browser console).

## Decisions & rejected alternatives

### Decision §1 — `BootstrapInputs` lives in a new module, not in repurposed `app_args.rs`

**Decided:** put `BootstrapInputs` in `crates/bevy_naadf/src/bootstrap.rs`.
Delete `app_args.rs` at Step 9.

**Rejected:** repurpose `app_args.rs` in place (rename inside).

**Why:** `app_args.rs` has hundreds of cross-file `use crate::AppArgs;` imports.
Renaming the file in place forces a touch across every consumer in a single
commit, defeating the incremental-migration constraint. A separate file lets
Steps 2-8 each delete fields from `AppArgs` while still constructing the wrapper
for any field not yet migrated, until Step 9 deletes the now-empty shell.

**Would flip if:** the implementor finds an `app_args.rs` consumer that can't
be reached without first knowing about the rename (none surfaced in the
inventory).

### Decision §2 — `BootstrapInputs` carries per-domain RESOURCES, not raw values

**Decided:** `BootstrapInputs.taa_ring_depth: TaaRingConfig` (not `: u32`).
Each field IS the resource that gets inserted.

**Rejected:** carry raw values and let `build_app_with_bootstrap_inputs`
construct each resource.

**Why:** symmetry with `BudgetCaps` (which is the precedent the architecture
audit cites). Per-domain resources are the SoT for their values; the
`BootstrapInputs` struct is just a carrier — making each field already be
the target resource type makes the fan-out a pure `insert_resource` loop, no
construction logic inside the fan-out. The CLI parser is the only place
that translates from CLI strings to typed resource values; the fan-out
becomes mechanical.

**Would flip if:** a domain has a multi-resource fan-out (one CLI flag →
several resources). Today this doesn't happen — `--vox <path>` fans into
just `GridPreset`, the budget probe fans into 3 budget resources that
are inserted SEPARATELY from `BootstrapInputs` (the budget path doesn't
go through `BootstrapInputs` for those — see `build_app_with_budget`).

### Decision §3 — `vox_e2e_mode` is Bucket A, not Bucket B

**Decided:** promote `vox_e2e_mode` to a standalone `VoxE2eAssertion(bool)`
Resource. Do NOT fold into `E2eGateMode`.

**Rejected:** add `E2eGateMode::VoxE2e` as a sibling of the other 10 variants.

**Why:** `vox_e2e_mode` is the only "mode" boolean that does NOT drive the
e2e driver into a separate state machine. The driver reads it ONCE, at
ASSERT time (`e2e/driver.rs:688`), to swap the region-gate assertion. Every
OTHER mode boolean routes the driver into a `*Warmup` fast-path on tick 0
(`driver.rs:476-577`) or chooses a window config — STATE-MACHINE work that
is genuinely mutually exclusive. `vox_e2e_mode` is "is this run loading a
.vox file or the default scene?" — an ASSERT-time data tag, not a flow
selector. The taxonomy distinguishes parameters (read for content) from
modes (selecting which code path runs). Folding it into `E2eGateMode` would
mean the `--vox-e2e` mode's driver flow is "Standard, but assert
differently" — which is exactly what a parameter does.

**Would flip if:** future `--vox-e2e` work adds a state-machine fast-path
(e.g. a Warmup→VoxAssert flow). At that point it earns its own
`E2eGateMode` variant.

### Decision §4 — `SpawnTestEntity` is `bool`, not `Option<TestEntityFixture>`

**Decided:** `SpawnTestEntity(pub bool)`. Simple newtype.

**Rejected:** `Option<TestEntityFixture { … }>` — the "if Phase-C test-entity
resource is present, spawn it" reframing the audit suggested.

**Why:** the test-entity fixture is content-static (a 4×4×4 emissive-voxel
block at world centre — `app_args.rs:60-65`). There are no per-fixture
parameters today; introducing an `Option<TestEntityFixture>` shape would
require inventing the inner type with zero fields. A `bool` matches today's
semantics 1:1 with no information loss. If a future system adds fixture
parameters (size, position, type), the migration to
`Option<TestEntityFixture>` is straightforward.

**Would flip if:** the implementor decides during Step 8 that a marker
component on a fixture entity is cleaner than a global Resource. Either is
acceptable; the design picks the simpler one.

### Decision §5 — Wasm32 `ConstructionConfig` divergence moves into a constructor, not a bootstrap branch

**Decided:** `ConstructionConfig::for_target_arch() -> Self`. Cfg-gated body
applies the wasm32 clamp internally.

**Rejected:** keep `ConstructionConfig::default()` and apply the wasm32
divergence inside `build_app_with_bootstrap_inputs` via a cfg-gated
`#[cfg(target_arch = "wasm32")] { cc.max_group_bound_dispatch = …; }` block.

**Why:** the divergence is a property of `ConstructionConfig` on this target.
Encapsulating it in a constructor mirrors `EffectiveWorldSize::canonical()` /
`InvalidSampleStorageCount::canonical()` — both of which expose a typed
canonical-on-this-target constructor. Putting cfg-gated code inside
`build_app_with_bootstrap_inputs` would scatter target divergence into the
bootstrap path; the architect's principle is "one knob, one home."

The existing const-pin at `config.rs:290-326` validates desktop defaults and
stays intact; the wasm32 path is a separate code path that branches at
`for_target_arch`.

**Would flip if:** more than 1 target divergence accumulates and the
constructor becomes a god-knob. At that point split into per-target
constructors.

### Decision §6 — Promote/extend the existing `GateKind` enum, do not invent a new `E2eGateMode`

**Decided:** rename `crate::e2e::gate::GateKind` to `E2eGateMode`, extend
with the missing variants, derive `Resource`, remove the dead-code allow.

**Rejected:** keep `GateKind` as-is (a non-Resource discriminator) and add
a parallel `E2eGateMode` Resource.

**Why:** the existing enum at `gate.rs:30-53` is explicitly documented at
`:13-16` as "the structural scaffolding introduced by D6 step 2 of the
codebase-tightening refactor" — its sole purpose is to land at the
boundary the configuration-as-resource refactor crosses. Splitting into
two enums (`GateKind` for the half-done `Gate` trait, `E2eGateMode` for the
resource) creates two sources of truth for the same set. The `Gate` trait
itself (`gate.rs:76-127`) is dead code (`#![allow(dead_code)]`), so renaming
through it is mechanical.

**Would flip if:** `GateKind` is being actively consumed by code outside
`gate.rs` (it is not — verified by `rg "GateKind"` showing only declaration
sites and the dispatch comments).

### Decision §7 — `KnobKind::Readonly` splits into typed variants

**Decided:** introduce `KnobKind::ReadonlyFromTaa { value: fn(&TaaRingConfig) -> String }`
+ `KnobKind::ReadonlyFromGi { value: fn(&GiSettings) -> String }` as separate
variants.

**Rejected:** keep `KnobKind::Readonly { value: fn(&dyn ReadonlySource) -> String }`
with a trait object.

**Why:** the existing `KnobKind::U32 / F32 / Bool` interactive variants
already encode their source as a `getter: fn(&GiSettings) -> _`; the
`Readonly` variant is the odd one out today because it uses `fn(&AppArgs) -> String`.
Adding two typed `Readonly*` variants restores symmetry — every knob declares
exactly which resource it reads. A trait-object variant would force every
consumer to implement `ReadonlySource`, which is more boilerplate for the
two knobs that exist today.

**Would flip if:** the number of readonly knob source-types grows beyond 3-4
(today it's exactly 2: `TaaRingConfig` and `GiSettings`).

## Assumptions made

1. **`GridPreset::Vox { path: PathBuf }` non-`Copy` is fine for a Resource.**
   `GridPreset` is already `Clone`-only; promoting to `Resource` keeps that
   shape. Bevy Resources do not require `Copy`. Verified by the existing
   `AppArgs` which is `Clone`-only (`app_args.rs:24`).

2. **`GiSettings` is `Copy`.** The audit says so; verified via
   `render/extract.rs:503-508` which clones it via the `Copy` derive in
   `ExtractedGiConfig`. Promoting to `Resource` doesn't change `Copy`-ness.

3. **`ConstructionConfig` is `Copy`.** Verified at
   `render/construction/config.rs:35` — `#[derive(Resource, Clone, Copy, Debug, PartialEq)]`.

4. **`TaaRingConfig` and `RenderTaaRingConfig` can coexist as same-named types
   in different modules.** They WON'T — the design renames the existing
   render-side `TaaRingConfig` to `RenderTaaRingConfig` to avoid the collision
   (mirrors the `EffectiveWorldSize` / `RenderEffectiveWorldSize` precedent
   exactly).

5. **The order of resource insertion in `build_app_with_bootstrap_inputs`
   doesn't matter** EXCEPT that all per-domain resources are inserted before
   `NaadfRenderPlugin` is added. The current code already inserts `AppArgs`
   before `NaadfRenderPlugin` for the same reason (`lib.rs:219, :305-321`),
   so preserving the resource-before-plugin invariant is mechanical.

6. **`NaadfPipelines::from_world` will see the right `RenderTaaRingConfig`
   value during `RenderStartup` because the first `ExtractSchedule` runs
   before `RenderStartup`.** This is how the existing `RenderEffectiveWorldSize`
   / `RenderInvalidSampleStorageCount` pattern already works
   (`render/budget.rs:174-196, :230-252`). If `from_world` actually runs
   before the first extract, the design needs a Step-2 follow-up to seed
   the render-world resource from a plugin-build snapshot (the way
   `TaaRingConfig` is today) BUT this didn't happen for the budget-resource
   precedent, so it shouldn't happen here either.

7. **The diagnostics dump can take 6 `Option<Res<…>>` parameters without
   running into Bevy's system-parameter ceiling.** Bevy supports up to 16
   system parameters; 6 + the 4 pre-existing parameters = 10, well under.

8. **Settings panel test at `settings/mod.rs:825-841` translates cleanly
   from `ResMut<AppArgs>` to `ResMut<GiSettings>`.** The test mutates
   `args.gi.*` fields and asserts post-reset state — nothing in the
   assertion shape couples to `AppArgs` qua-`AppArgs`; only the parameter
   type changes.

9. **The `parse_gate_command` / `BootCommand` enum at
   `bin/e2e_render.rs:109-122` evaporates entirely.** It's a transitional
   shape that gets superseded by `BootstrapInputs::for_<gate>()` constructors.
   The `RUN` / `BOOT` distinction it tries to make is already captured by
   "Layer 1 (short-circuit) vs Layer 2 (boot a Bevy app)".

10. **No CLI surface today exposes `--taa <on|off>`, `--taa-ring <16|24|32>`,
   or any `--gi-<knob> <value>` flag.** Verified by `grep "--taa\|--gi-"` —
   no matches. The design does NOT add such flags; they're orthogonal future
   work. The migration just moves these fields out of `AppArgs`; the values
   stay at the canonical defaults set by `BootstrapInputs::default()`.

## Side notes / observations / complaints

- **`GateKind` is the half-done refactor that exactly matches Q2's brief.**
  Reading `crates/bevy_naadf/src/e2e/gate.rs:1-127` it's clear this was set
  up as the seam for exactly this orchestration; the prior architect already
  did half the work. Step 6 of the migration plan is essentially "finish
  that refactor + flip its consumer". This is GOOD — it means Step 6's blast
  radius is bounded by an explicitly-designed boundary.

- **The compile-time const-pin at `render/construction/config.rs:290-326` is
  load-bearing.** It compiles a `ConstructionConfig` literal at every build
  and asserts the canonical field values hold. After Step 4 (deleting
  `From<&AppArgs>` and adding `for_target_arch`), the pin should be moved
  to assert against the desktop arm of `for_target_arch()`'s output —
  otherwise the wasm32 divergence isn't pinned. Note this in Step 4's diff
  size.

- **`AppArgs::default()` is constructed 17 times across the workspace.** Each
  e2e gate's `run_*` builder constructs one, mutates 1-3 fields, calls
  `run_e2e_render_with_args`. This is the WORST kind of "every caller
  duplicates the same idiom" smell — the design's `BootstrapInputs::for_<gate>()`
  per-gate constructor is the right factoring. The diff at Step 6 sweeps
  all 15 of these.

- **The wasm32 platform divergence inside `From<&AppArgs> for ConstructionConfig`
  is an IoC violation.** A `From` impl, by convention, is pure value
  translation — no side effects, no target-specific logic. Today it carries
  a 30-line cfg-gated block at `config.rs:265-288` that clamps
  `max_group_bound_dispatch` and forces `n_bounds_rounds = 1` on wasm32.
  Moving this into `ConstructionConfig::for_target_arch()` is the right
  encapsulation (Decision §5).

- **The settings panel `KnobKind::Readonly { value: fn(&AppArgs) -> String }`
  signature is the smell tell — readonly knobs were the ONLY reason
  `&AppArgs` survived as a target type.** Today 2 of 7 readonly knobs read
  AppArgs fields; the other 5 read constants. The non-AppArgs ones could
  trivially have lived as `fn(&()) -> String`. Splitting the variant is the
  right call (Decision §7).

- **`spawn_test_entity` being read by both the `.run_if` AND the e2e driver
  is structurally weird.** The driver reads it at ASSERT time to adjust the
  baseline (`e2e/driver.rs:680`). Why isn't the assert-time signal computed
  from "did `spawn_phase_c_test_entity` actually fire" instead of a separate
  read of the gating flag? Reframing this as "if the fixture entity is in
  the world, the entity-mode assertion runs" (an Option<Resource> shape)
  would be cleaner but adds work outside the brief — flagged as a future
  follow-up.

- **`vox_gpu_oracle_cpu_phase` having a main-world consumer at
  `voxel/grid.rs:132` is unusual for a "mode" boolean.** Most mode booleans
  only drive the e2e driver state machine; this one is the SOLE remaining
  call site of the legacy `install_vox_sized_to_model` path. The cleanest
  factoring would be to invert: make `install_vox_sized_to_model` a separate
  install function only the test-only gate calls, and remove the grid_preset
  branch on `vox_gpu_oracle_cpu_phase` entirely. That's a follow-up to this
  refactor — flagged but not in scope.

- **The brief is well-scoped.** The three-bucket taxonomy is the right
  architectural backbone; the borderline calls (Q1–Q4) are exactly the
  hard decisions that needed user input; the audit was thorough enough that
  this investigation phase didn't surface anything that contradicted the
  audit. Going into implementation with this design should not require
  another round of user input — the open knobs are the design's per-decision
  flip points, all of which are stated explicitly above.

- **Implicit ordering invariant.** Several systems implicitly assume
  `AppArgs` (or its successor resource) is present:
  `update_camera_history` (`render/taa.rs:188`, `Res<AppArgs>` — non-Option),
  `setup_test_grid` (`voxel/grid.rs:121`, `Res<AppArgs>`), `prepare_taa`'s
  reads of `TaaRingConfig`, etc. After the refactor, each post-domain
  resource must have a SAFE default in place at the time its consumers run.
  This is satisfied by `BootstrapInputs::default()` populating EVERY field
  unconditionally + `build_app_with_bootstrap_inputs` inserting EVERY
  resource unconditionally — same shape as today, where
  `AppArgs::default()` is the safety net. Worth calling out so the
  implementor doesn't accidentally make any per-domain resource optional
  at insert.

- **Documentation drift.** Many docblocks reference `AppArgs` by name
  (`render/taa.rs:32-44`, `render/construction/config.rs:8-10`, etc.).
  Each migration step should update the local docblocks alongside the code
  change — this is mechanical but easy to forget. Suggest the implementer
  treat docblock updates as part of the same commit as the code (don't
  defer to a follow-up — they decay otherwise).

- **`AppArgs::default()` semantic preservation is byte-identical IF AND
  ONLY IF `BootstrapInputs::default()` produces every resource at its
  canonical default.** This is THE determinism contract for the e2e_render
  path. The design explicitly lists each Default value in §3.1 against the
  current values from `app_args.rs:185-207`; preserving these is the
  load-bearing correctness property.

- **The architecture-Q&A's "fanout vs aggregator" choice (Q4) didn't need
  to be picked globally.** The two cross-cutting consumers (diagnostics +
  settings) both came out simpler with fanout — diagnostics already takes
  `Option<Res<…>>` and the panel benefits from typed readonly variants
  (Decision §7). Aggregators (e.g. a `ConfigBundle` thin SystemParam) would
  add a layer for no benefit here. If a third cross-cutting consumer
  appears later with 6+ reads, that's when aggregator-vs-fanout becomes a
  real tension; today fanout wins.
