# e2e-as-Rust-tests viability investigation

> Singular Research dispatch — findings only, no design. Produced to inform
> the user + orchestrator decision on whether to move the verification gates
> from `cargo run --bin e2e_render -- <mode>` into `tests/<gate>.rs`
> integration tests runnable via `cargo test`. Re-decides Step 6 of the
> in-flight AppArgs refactor (the `E2eGateMode` enum collapse).

---

## 1. Current e2e harness shape (factual enumeration)

### 1.1 The bin/e2e_render entry point

`crates/bevy_naadf/src/bin/e2e_render.rs` is a hand-rolled three-layer parser.
**No `clap`.** The shape is structured but argv-string-iter-based.

`fn main() -> ExitCode` (`bin/e2e_render.rs:134-157`) — orchestrator:

1. **Layer 1 — `parse_top_level_short_circuit`** (`:168-185`) returns
   `Option<TopLevelShortCircuit>`. **No Bevy boot.** Five variants:
   `VoxGpuOracleCompare`, `VoxWebParityCompare`, `SsimCompare`,
   `ValidateGpuConstructionScaled`, `ValidateGpuConstructionProduction`.
   When matched, `run_top_level_short_circuit` (`:188-246`) calls into a
   pure function and returns the `ExitCode` directly — never touches `App`.
   The first two are subprocess-spawners (re-exec `current_exe()` with sub-flag args
   for the `cpu` / `gpu` phases, then SSIM-compare on disk).
2. **Layer 3 — `parse_post_app_validations`** (`:447-454`) returns
   `PostAppValidations { validate_gpu_construction, entities, edit_mode,
   runtime_edit_mode }`. **Collected pre-boot** (so they're available after
   `app.run()` consumes the App — see §3.2). Run by
   `run_post_app_validations` (`:460-523`) AFTER the boot command exits;
   each calls a `validate_*` function in `render::construction::validation`.
3. **Layer 2 — `parse_gate_command`** (`:256-324`) returns `BootCommand`.
   Four variants: `NamedGate { gate: GateKind, run: fn() -> AppExit }`,
   `ResizeTest`, `EntitiesBoot`, `Standard`. `run_boot_command`
   (`:328-348`) calls the matching builder; for `NamedGate` it invokes
   the supplied `run` function pointer (a `bevy_naadf::e2e::<gate>::run_*`
   entry point). The `gate: GateKind` discriminator is currently
   **unused** (`let _ = gate;` at `:336`) — the D6 step-2 scaffolding
   waiting for the AppArgs refactor.

The final exit code combines `app_exit_to_code(app_exit)` (`:434-439` maps
`AppExit::Success → 0` / `AppExit::Error(code) → code.get()`) with the
post-app validation results: validations can flip the code to 1 even if
the boot succeeded.

### 1.2 Per-gate inventory

Verified by reading each `src/e2e/<gate>.rs` and the routing in
`bin/e2e_render.rs:256-324`. "Builder" = the `run_*` function that
constructs `AppArgs::default()`, flips 1-3 fields, calls
`run_e2e_render_with_args`.

| # | Gate flag | Builder (`run_*`) | What it does | Pass/fail signal | Window | Camera-pin | Subprocess | Notes |
|---|---|---|---|---|---|---|---|---|
| 1 | (none, default — `BootCommand::Standard`) | `bevy_naadf::run_e2e_render` (`lib.rs:403`) → `e2e::run_e2e_render` (`e2e/mod.rs:377`) | Standard Warmup→Motion→Settle→Shoot→Drain→Assert flow with `GridPreset::Default`. Region-gate + degenerate-frame + luminance-liveness + pipeline-scan + node-dispatch checks. | `AppExit` → exit code | 256×256 (`WindowConfig::e2e()`) | — | no | The "`baseline`" gate. |
| 2 | `--vox-e2e` | `vox_e2e::run_vox_e2e` (`vox_e2e.rs:346`) | Loads Oasis `.vox` fixture, runs standard flow, swaps default-scene region gate for `assert_vox_geometry_visible`. | `AppExit` + standard PNG to `target/e2e-screenshots/e2e_latest.png` | 256×256 | (uses standard driver pose) | no | Per Decision §3 of design, NOT a state-machine selector — assert-time tag only. |
| 3 | `--oasis-edit-visual` | `oasis_edit_visual::run_oasis_edit_visual` (`oasis_edit_visual.rs:182-213`) | Loads Oasis fixture; routes driver into `OasisWarmup→Before→Apply→Wait→After→Assert` flow; calls real `sphere_brush` mid-run; asserts framebuffer Δ. | `AppExit` + before/after PNGs | 256×256 | `pin_oasis_camera` (birdseye) | no | |
| 4 | `--small-edit-visual` | `small_edit_visual::run_small_edit_visual` | `SmallEditWarmup`-prefixed flow on `GridPreset::Default`; `cube_brush`; voxel-count + adj-rect assertions. | `AppExit` + before/after PNGs | 256×256 | `pin_small_edit_camera` | no | |
| 5 | `--small-edit-repro` | `small_edit_repro::run_small_edit_repro` (`small_edit_repro.rs:124-157`) | User-captured Oasis brush click repro. Different (large) window resolution. | `AppExit` + before/after PNGs + `assert_no_pitch_black_pixels` | 1920×1080 (`WindowConfig::e2e_small_edit_repro`) | `pin_small_edit_repro_camera` | no | |
| 6 | `--vox-gpu-construction` | `vox_gpu_construction::run_vox_gpu_construction` | Reuses Oasis warm/shoot/edit/wait/shoot/assert flow with C# `(500, 200, 40)` camera; W5 GPU producer chain enabled. Flips `construction_config.gpu_construction_enabled = true`. | `AppExit` + before/after PNGs | 256×256 | `pin_vox_gpu_construction_camera` | no | |
| 7 | `--vox-gpu-oracle-cpu` | `vox_gpu_oracle::run_vox_gpu_oracle_cpu_phase` (`vox_gpu_oracle.rs:257`) | Loads Oasis, routes through legacy `install_vox_sized_to_model` (CPU oracle). Single-screenshot fast-path. Saves `oracle_cpu.png`. | `AppExit` + `oracle_cpu.png` | 256×256 | `pin_vox_gpu_oracle_camera` | sub-phase of #9 | |
| 8 | `--vox-gpu-oracle-gpu` | `vox_gpu_oracle::run_vox_gpu_oracle_gpu_phase` (`vox_gpu_oracle.rs:305`) | Same fixture, W5 GPU path. Saves `oracle_gpu.png`. | `AppExit` + `oracle_gpu.png` | 256×256 | `pin_vox_gpu_oracle_camera` | sub-phase of #9 | |
| 9 | `--vox-gpu-oracle` (TOP-LEVEL) | `vox_gpu_oracle::run_vox_gpu_oracle_compare` (`vox_gpu_oracle.rs:346`) | **No Bevy boot.** Subprocess: re-exec `current_exe() -- --vox-gpu-oracle-cpu`, then `-- --vox-gpu-oracle-gpu`, then SSIM-compare the two PNGs. | `u8` exit code | (no window) | — | spawns 2 subprocesses | Layer-1 short-circuit. |
| 10 | `--vox-web-parity-skybox` | `vox_web_parity::run_vox_web_parity_skybox_phase` (`vox_web_parity.rs:151`) | Empty world (`GridPreset::Empty`), single screenshot. Saves `vox_web_parity_skybox.png`. | `AppExit` + PNG | 256×256 | `pin_vox_web_parity_camera` | sub-phase of #12 | |
| 11 | `--vox-web-parity-loaded` | `vox_web_parity::run_vox_web_parity_loaded_phase` (`vox_web_parity.rs:175`) | Loaded Oasis, single screenshot, asserts `TRACING_ERROR_COUNT == 0`. Saves `vox_web_parity_loaded.png`. | `AppExit` + PNG | 256×256 | `pin_vox_web_parity_camera` | sub-phase of #12 | |
| 12 | `--vox-web-parity` (TOP-LEVEL) | `vox_web_parity::run_vox_web_parity_compare` (`vox_web_parity.rs:212`) | **No Bevy boot.** Subprocess-spawns the two sub-phases, SSIM-compares + channel-max guard. | `u8` exit code | — | — | spawns 2 subprocesses | Layer-1 short-circuit. |
| 13 | `--vox-horizon-native` | `vox_horizon_parity::run_vox_horizon_native_phase` (`vox_horizon_parity.rs:131`) | Loads Oasis at C# horizon pose. Saves `vox_horizon_native.png`. | `AppExit` + PNG | **1280×720** (`WindowConfig::e2e_horizon`) | `pin_vox_horizon_camera` | no | Native half of the cross-target Playwright parity gate. |
| 14 | `--resize-test` | `bin/e2e_render.rs:run_resize_test` (`:353-384`) | Three-step Hyprland-driven resize: boot 800×600 → resize 1920×1080 → resize 2000×1000. Three captures; full-frame luma ratio assertion. **Pre-launch** installs Hyprland windowrule, post-run reloads to discard. | `AppExit` + 3 PNGs | 800×600 → … | `pin_resize_test_camera` (in driver) | no | **Hyprland-only** — driver bails if `HYPRLAND_INSTANCE_SIGNATURE` env var absent. |
| 15 | `--entities` | `bin/e2e_render.rs:run_boot_command::EntitiesBoot` (`:340-345`) | Standard flow + `construction_config.entities_enabled=true` + `spawn_test_entity=true`. | `AppExit` (entity-pixel-aware assert baseline) + PNG | 256×256 | — | no | ALSO a post-app validation flag (Layer 3) — same flag, two effects. |
| 16 | `--ssim-compare <a.png> <b.png> [--ssim-min ..] [--ssim-max ..]` | `e2e::ssim::ssim_compare_command` (`e2e/ssim.rs`) | **No Bevy boot.** Pure PNG diff. | `u8` exit code | — | — | no | Layer-1 short-circuit. |
| 17 | `--validate-gpu-construction` (POST-APP) | `render::construction::validation::validate_gpu_construction` (`validation.rs:141`) | **Already headless** — boots `App::new() + MinimalPlugins + RenderPlugin` (no winit), runs WGSL chunk_calc, reads back GPU buffers, asserts byte-equal CPU oracle. | `Result<usize, String>` | — | — | no | Layer-3 post-app tail. Runs AFTER any boot command. |
| 18 | `--validate-gpu-construction-scaled` | `validation::validate_gpu_construction_scaled` (`:503`) | **Already headless.** Fixture sweep through W5 chunk_calc. | `Result<String, String>` | — | — | no | Layer-1 short-circuit. |
| 19 | `--validate-gpu-construction-production` | `validation::validate_gpu_construction_production_scale` (`:834`) | **Already headless.** Production-scale buffer readback. | `Result<String, String>` | — | — | no | Layer-1 short-circuit. |
| 20 | `--entities` (POST-APP) | `validation::validate_entity_handler` (`:4663`) | **Already headless.** | `Result<String, String>` | — | — | no | Same flag triggers builder #15 AND this tail. |
| 21 | `--edit-mode` (POST-APP) | `validation::validate_edit_mode` (`:4381`) | **Already headless.** CPU-side edit chain end-to-end. | `Result<String, String>` | — | — | no | Layer-3 tail. |
| 22 | `--runtime-edit-mode` (POST-APP) | `validation::validate_runtime_edit_mode` (`:4518`) | **Already headless.** Runtime brush-path edit gate. | `Result<String, String>` | — | — | no | Layer-3 tail. |

**Counting "gates" depends on what counts.** The user's brief said "11 e2e
gates". I count:

- **11 booted-window gates** = rows 1-8 + 10-11 + 13-15. Each opens a real
  winit window, runs the standard or a customised driver flow.
- **3 Layer-1 subprocess orchestrators** (rows 9, 12, 16) — pure logic on
  disk artefacts; no App.
- **2 Layer-1 already-headless validators** (rows 18, 19) — boot a
  `MinimalPlugins + RenderPlugin` App, never open a window.
- **4 Layer-3 already-headless validators** (rows 17, 20, 21, 22) — same.

The 11 user-named gates are the booted-window ones. The Layer-1/Layer-3
validators are technically "additional gates" but they already live as
plain `pub fn ... -> Result<..., String>` calls — they are
*already* drop-in `cargo test` candidates.

### 1.3 The driver state machine

`src/e2e/driver.rs:e2e_driver` (`:452-1846`, ~1400-line system function)
is a giant `match state.phase` over `E2ePhase` (`driver.rs:60-248` —
26 variants). Each gate registers ONE entry into the state machine.

Routing happens at the top of the system (`driver.rs:469-577`) via a chain
of "if mode-flag-from-AppArgs and phase==Warmup and ticks==0 → goto-other-state"
fast-paths:

- `resize_test_mode` (`:475`) → `LaunchSettle`
- `oasis_mode || vox_gpu_construction_mode` (`:514`) → `OasisWarmup`
- `small_edit_mode` (`:528`) → `SmallEditWarmup`
- `small_edit_repro_mode` (`:539`) → `SmallEditReproWarmup`
- `vox_gpu_oracle_mode` (`:551`) → `VoxGpuOracleWarmup`
- `vox_web_parity_mode || vox_horizon_native_phase` (`:565`) → `VoxWebParityWarmup`

Falling through means the standard flow runs (Warmup → Motion → Settle →
Shoot → Drain → Assert at `:580-714`).

Shared infrastructure across gates:
- The shared `Update` system order in `e2e/mod.rs:249-282` — `e2e_driver`,
  then each gate's `pin_*_camera` system `.after(driver::e2e_driver)`
  and `.before(crate::camera::sync_position_split)`. **6 distinct camera-pin
  systems** are wired unconditionally; each self-gates on its
  `AppArgs.<mode>` boolean and early-returns if not active.
- The shared `add_e2e_systems` (`e2e/mod.rs:205-297`) installs the
  fixed-pose camera, all driver state resources, the pipeline-scan
  cross-world channel, and `WinitSettings { focused: Continuous, unfocused: Continuous }`.
- The shared `setup_e2e_camera` (`e2e/mod.rs:321-348`) spawns the
  Camera3d with `Hdr + Tonemapping::default() + Msaa::Off + PositionSplit`.

The standard ASSERT step at `:674-714` calls `run_assertions(...)`
(`:1849-1947`), which writes `target/e2e-screenshots/e2e_latest.png`
unconditionally, then runs the per-batch region gate, the
degenerate-frame check, the luminance-liveness gate, the pipeline-scan
read, and `assert_nodes_dispatched`. Failures are collected into a
single `Vec<String>`; if non-empty, the system writes
`AppExit::error()` + sets `outcome.gate_result = Some(Err(msg))`.

### 1.4 Verification signal exit shape

**Three channels, combined:**

1. **`AppExit`** is written by `e2e_driver` at the terminal phase
   (Assert, ResizeAssert, OasisAssert, SmallEditAssert, etc. — every
   gate's last phase before `Done`). `app_exit_to_code` (`bin/e2e_render.rs:434`)
   maps it to a `u8`.
2. **PNG files** on disk. Always: `target/e2e-screenshots/e2e_latest.png`.
   Gate-specific: `oracle_cpu.png`, `oracle_gpu.png`,
   `vox_web_parity_skybox.png`, `vox_web_parity_loaded.png`,
   `vox_horizon_native.png`, `resize_initial.png`, `resize_a.png`,
   `resize_b.png`, and the before/after pairs for the edit gates.
   Plus per-run funnel directories for the horizon-parity Playwright
   spec (`target/e2e-screenshots/funnel/`).
3. **Log lines** prefixed `e2e_render: PASS …` / `e2e_render: FAIL — …`.
   Playwright specs scrape these (e.g. the SSIM score extractor at
   `e2e/tests/vox-horizon-parity.spec.ts:107`).

The `--vox-gpu-oracle` and `--vox-web-parity` compare phases ALSO read
PNGs from disk and run SSIM — they are pure-disk-artefact orchestrators.

The Playwright `--vox-horizon-native` cross-target spec reads
`vox_horizon_native.png` from disk, captures the WASM canvas
screenshot to its own path, and shells out to `cargo run --bin e2e_render
-- --ssim-compare a.png b.png --ssim-min 0.91` (vox-horizon-parity.spec.ts:236-273).

---

## 2. Existing test/CI infrastructure

### 2.1 Existing tests

- **195 `#[test]` blocks across 36 source files**, all inline
  `#[cfg(test)] mod tests { … }` style. No `tests/` directory exists in
  any workspace member (`find ... -type d -name tests` returns only
  the Playwright dir at `e2e/tests/`).
- **No `[[test]]` targets in `crates/bevy_naadf/Cargo.toml`** (verified
  by reading the manifest — there are only `[[bin]]` entries for
  `bevy-naadf`, `e2e_render`, `bake`).
- **No `tests/common.rs` or any integration-test harness** today.
- Several inline tests **already boot a Bevy App headlessly with
  `MinimalPlugins + RenderPlugin`** — see `render/construction/validation.rs:141, 503, 834, 1851, 2363, 2835, 3060, 3637, 4381, 4518, 4663` and others (24+ occurrences of `MinimalPlugins` across that file alone). The pattern is: build an App with `App::new() + MinimalPlugins + AssetPlugin + RenderPlugin { synchronous_pipeline_compilation: true, … }`, finish/cleanup, grab `RenderDevice + RenderQueue` from the RenderApp, run shaders, map buffers back. **These boot a real wgpu device** but **NEVER open a window** (no `WinitPlugin`).

### 2.2 Existing test invocation

- `just test` = `cargo test --workspace` (`justfile:39-40`).
- `just lint` = `cargo clippy --workspace --all-targets -- -D warnings`
  (`justfile:51-52`).
- **No justfile recipe invokes `e2e_render`** (verified by
  `grep -n e2e_render justfile` returning zero matches).
- The verification surface in `CLAUDE.md` lists `cargo run --bin
  e2e_render -- <mode>` as the way to run gates. **The user / agents
  invoke it directly** — no recipe shortcut.
- CI (`/.github/workflows/deploy-cloudflare.yml`): runs `cargo test -p
  voxel_noise` (a different crate) and `trunk build --release` for
  bevy_naadf. **CI does NOT run `cargo test --workspace`, does NOT run
  `e2e_render`, does NOT run Playwright.** The only post-build gate is
  the deploy step. Verification gating is local/manual.

### 2.3 The Playwright track

`e2e/` carries 4 specs:
- `wasm-smoke.spec.ts` — boot the WASM build at `/`, wait for
  `#loading.hidden`, assert no panics. Pure browser gate.
- `vox-loading.spec.ts` — exercises `?vox=` URL params for in-browser
  `.vox`/`.cvox` parsing.
- `vox-horizon-parity.spec.ts` — **cross-target gate.** Spawns
  `cargo run --bin e2e_render -- --vox-horizon-native` as a subprocess
  to produce `vox_horizon_native.png` (1280×720); then opens
  `/?vox=...&pose=horizon&ui=hide` in Chrome (headed); captures the
  canvas as `vox_horizon_web.png`; shells out to `cargo run --bin
  e2e_render -- --ssim-compare a.png b.png --ssim-min 0.91`. Three
  subprocess invocations of `e2e_render` per run.
- `sw-chrome-extension.spec.ts` — service worker / extension test.

Invoked by `just test-wasm` (= `cd e2e && npx playwright test --headed`)
or `just test-wasm-full` (= `web-build-release + test-wasm`).
**Always headed** per the binding memory + `justfile:162-177` block
comment: headless Chromium's WebGPU SwiftShader fallback panics with
`DeviceLost` mid-render.

### 2.4 The on-device track

`docs/todo/android-build.md:84-107` documents the Android deploy chain:
1. `cargo ndk -t arm64-v8a --platform 31 -o android/.../jniLibs build -p bevy-naadf --lib` (~5min cold)
2. `llvm-strip --strip-debug …libbevy_naadf.so` → 190 MiB
3. `android/gradlew -p android assembleDebug`
4. `adb install -r -t …app-debug.apk`
5. `adb logcat -c && adb shell am start -n io.naadf.bevy/.MainActivity`
6. `adb logcat | grep -E 'naadf-probe|RustStdoutStderr|FATAL|signal'`

iOS Safari deploy track is not yet wired (mentioned in the mobile-256MiB
memory as a future target).

**The on-device track runs `bevy-naadf`, not `e2e_render`.** There is
no Android variant of `e2e_render` — Android boot goes through
`android_main.rs::android_main` which calls `build_app_with_budget` on a
default `AppArgs`. No CLI flags exist on Android. The verification
signal is `adb logcat` text patterns, not exit codes or PNGs.

---

## 3. Constraints — why the current shape exists

### 3.1 GPU / wgpu device constraints

- **Each booted gate constructs a real wgpu device via `DefaultPlugins`.**
  `build_app_with_args` (`lib.rs:175-393`) adds `DefaultPlugins`, which
  includes `RenderPlugin`. `RenderPlugin` instantiates wgpu's `Adapter`
  + `Device` + `Queue` on plugin-build. **Cold start ~3-5 s** on
  desktop with `synchronous_pipeline_compilation: true` (per the
  `AppConfig::e2e` knob).
- **Multiple devices in a single process are technically allowed by
  wgpu but discouraged.** No project code attempts to share a device
  across multiple Apps. The existing `MinimalPlugins + RenderPlugin`
  headless tests in `validation.rs` each build their own App+Device per
  test — and they ARE invoked by `cargo test`, demonstrating that
  multiple `Device` instances per process work in practice. Bevy 0.19's
  `synchronous_pipeline_compilation: true` makes the compile
  deterministic but does not change device-lifecycle semantics.
- **The driver phase budget assumes vsync-paced ticks.** `E2E_WARMUP_FRAMES
  = 96` + `E2E_MOTION_FRAMES = 48` + `E2E_SETTLE_FRAMES = 1` = **145
  ticks** before the standard gate's first ASSERT. With
  `UpdateMode::Continuous + WinitPlugin` on a 60 Hz display this is
  ~2.4 s; without vsync it would run as fast as the GPU allows
  (potentially much shorter, but the phase budget for resize-test
  EXPLICITLY uses wall-clock-equivalent frame counts:
  `E2E_RESIZE_LAUNCH_SETTLE_FRAMES = 300` ≈ 5 s @ 60 fps —
  `e2e/mod.rs:155-159`).

### 3.2 Window / display constraints

- **Every booted gate creates a real winit window.** `WinitPlugin` is in
  `DefaultPlugins` and is NOT optional in `AppConfig::e2e`. The harness's
  own module-level comment (`e2e/mod.rs:3-10`) emphasises: "boots the
  *real* `DefaultPlugins` + `WinitPlugin` windowed app (the same wiring
  as `main.rs`)". This is deliberate — the gates exercise the production
  code path, not a near-copy.
- **`App::run()` consumes the App** with WinitPlugin. The harness's
  `checks.rs:5-12` documents this explicitly: "`App::run()` does
  `core::mem::replace(self, App::empty())` — it moves the `App` into the
  winit runner and leaves an empty `App` behind; the winit runner
  consumes it and never hands it back. **So there is no `App` to inspect
  post-run.**" Every assertion that needs world state lives in the
  driver's `Update`-system ASSERT step.
- **The `--resize-test` gate requires Hyprland.** `driver.rs:482-492`
  bails with `AppExit::error()` if `HYPRLAND_INSTANCE_SIGNATURE` env
  var is absent. The `--resize-test` boot is wrapped in
  `install_resize_test_windowrule` + `cleanup_resize_test_windowrule`
  (`bin/e2e_render.rs:388-431`) which shell out to `hyprctl keyword
  windowrule …` and `hyprctl reload`. This is desktop-Linux-specific,
  even compositor-specific, machine state.
- **The `--small-edit-repro` gate hard-codes 1920×1080.** It exercises
  a user-reported bug at the user's screen resolution; the assertion
  (`assert_no_pitch_black_pixels`) is resolution-independent in
  principle but the camera pose is tuned to that viewport.
- **The `--vox-horizon-native` gate hard-codes 1280×720** to match the
  Playwright spec's `viewport: { width: 1280, height: 720 }` for
  cross-target SSIM compare without resize.

### 3.3 Bevy App startup cost

- `cargo test` in Rust is **single-binary**: all `#[test]` blocks in one
  cdylib/rlib are compiled into one test executable that runs them
  sequentially (or with `--test-threads=N`). Multiple `tests/<file>.rs`
  files in a `tests/` directory each compile to a SEPARATE binary (one
  per file) — each with its own startup cost.
- Each window-booting gate has its own cold App startup (~3-5 s). The
  full e2e sweep over 11 booted gates is ~45-90 s wall-clock today.
- **No project code attempts to run multiple Apps in one process.**
  Bevy supports it (`MinimalPlugins`-based tests do it routinely), but
  `WinitPlugin`-based runs cannot share state easily (winit's event
  loop is global per-process and consuming the App is terminal).

### 3.4 Cross-target reach

| Gate | Native desktop | Native Android | Wasm32 browser |
|---|---|---|---|
| baseline (1) | yes (e2e_render) | not run | yes (wasm-smoke.spec.ts proxies) |
| --vox-e2e (2) | yes | not run | not run |
| --oasis-edit-visual (3) | yes | not run | not run |
| --small-edit-visual (4) | yes | not run | not run |
| --small-edit-repro (5) | yes | not run | not run |
| --vox-gpu-construction (6) | yes | not run | not run |
| --vox-gpu-oracle-cpu/-gpu/- (7-9) | yes | not run | not run |
| --vox-web-parity-skybox/-loaded/- (10-12) | yes (also subprocess-driven) | not run | yes (vox-loading.spec.ts proxies similar surface) |
| --vox-horizon-native (13) | yes | not run | yes (cross-target SSIM via Playwright) |
| --resize-test (14) | yes (Hyprland-only) | not run | not run |
| --entities (15) | yes | not run | not run |
| --ssim-compare (16) | yes (CI-friendly, no GPU) | possible (but never invoked) | not run |
| --validate-gpu-construction (17) and other validators (18-22) | yes (headless MinimalPlugins) | not run (would compile) | not tested |

The user's question about "tests run through a test runner" maps cleanly
onto the **native-desktop** column. The other columns have native
process-spawn (Android) or process-spawn-via-browser (wasm32) constraints
the test runner does not address.

### 3.5 Determinism / flakiness

- **`AppConfig::e2e` enables `synchronous_pipeline_compilation: true`**
  (`app_config.rs:23-26`). Every queued pipeline reaches `Ok`/`Err` the
  same frame it was queued. This is the load-bearing determinism switch.
- **Fixed 256×256 framebuffer for most gates** = identical
  `pixel_count`-sized buffers run-to-run (`e2e/mod.rs:53-58`).
- **`WinitSettings { focused: Continuous, unfocused: Continuous }`**
  (`e2e/mod.rs:221-224`) guarantees the app ticks every frame regardless
  of focus, so the frame budget advances deterministically.
- **Phase B GI 96-frame warmup** (`e2e/mod.rs:74-88`) is empirically
  tuned to the 12-sample-threshold in `refineBuckets`. Insufficient warmup
  → GI bounces don't converge → `solid_block_rect` assertion fails.
- **SSIM-based comparisons** tolerate ~1.5-6% per-pixel stochastic
  TAA/GI shimmer + GPU atomic-cursor nondeterminism. The
  `--vox-gpu-oracle` and `--vox-web-parity` gates rely on this.
- **Known-flaky surfaces:** the horizon-parity Playwright spec's
  "funnel sweep" approach (per-run timestamped PNG + sidecar `.txt`
  in `target/e2e-screenshots/funnel/`) suggests the wasm WebGPU
  chunk-AADF path has non-deterministic attractor states the spec
  explicitly groups via 15-run sweeps (`vox-horizon-parity.spec.ts:300-327`).
- **No `RUST_TEST_THREADS` precedent.** No project test today asserts
  serial execution; the `MinimalPlugins` headless tests run in parallel
  by default. But every windowed gate is implicitly serial — a single
  process can only have one winit event loop active.

---

## 4. Per-gate eligibility analysis

Each row: can it move from `cargo run --bin e2e_render -- <flag>` to
`tests/<gate>.rs` runnable via `cargo test --test <gate>`? Caveats per
gate. **"Move cleanly"** = no architectural change needed beyond
plumbing. **"Move with caveats"** = requires some refactoring but is
structurally possible. **"Cannot move"** = blocked by a constraint.

| # | Gate | Today's host | Move to tests/? | Caveats / blockers |
|---|---|---|---|---|
| 1 | baseline | `--bin e2e_render` (`run_e2e_render` → `run_with_app(app.run())`) | **With caveats** | Opens a real winit window. `cargo test` does not sandbox window creation, but `WinitPlugin` claims the event loop globally — only ONE test in the entire `cargo test --workspace --lib` invocation could run it before becoming poisoned. Needs serial-by-construction (one file per gate, since `cargo test --test foo --test bar` runs each binary sequentially) OR `serial_test` crate annotation. Assertion read must move from in-driver-system `AppExit::error()` to a post-run `Resource` read — but `App::run()` consumes the App. The only way to read the verdict post-run is the existing `outcome.gate_result` resource stash, which currently gets dropped with the App. Would need: write the verdict to disk OR change `run_e2e_render` to not call `app.run()` but instead step `app.update()` until `should_exit()` — Bevy 0.19 supports this, but means rewriting the driver's terminal-state contract. |
| 2 | --vox-e2e | builder | **With caveats** | Same as baseline. Plus loads an LFS-tracked fixture; `git lfs pull` must run before. |
| 3 | --oasis-edit-visual | builder | **With caveats** | Same as baseline + LFS fixture + mid-run `WorldData` mutation via `Commands` system. |
| 4 | --small-edit-visual | builder | **With caveats** | Same as baseline. |
| 5 | --small-edit-repro | builder | **With caveats** | Same as baseline + 1920×1080 window — most CI runners can host this, but a low-res virtual display might not. |
| 6 | --vox-gpu-construction | builder | **With caveats** | Same as baseline + LFS fixture. |
| 7-8 | --vox-gpu-oracle-cpu/-gpu | builder | **With caveats** | Same as baseline + LFS fixture. |
| 9 | --vox-gpu-oracle (compare) | top-level Layer 1 | **Cleanly** | Pure disk-artefact + subprocess orchestration. Subprocess re-execs `current_exe()` — under `cargo test`, `current_exe()` is the test binary, not `e2e_render`. **Subprocess pattern fundamentally breaks if moved to `cargo test`** — would need to refactor to call the cpu/gpu phase functions directly in-process, but the whole point of the subprocess split is that each phase gets a clean App lifecycle. Compromise: keep the compare as `cargo test --test vox_gpu_oracle_compare` that orchestrates subprocess calls to `cargo run --bin e2e_render`. |
| 10-11 | --vox-web-parity-skybox/-loaded | builder | **With caveats** | Same as baseline. |
| 12 | --vox-web-parity (compare) | top-level Layer 1 | **Cleanly** | Same subprocess caveat as #9. |
| 13 | --vox-horizon-native | builder | **With caveats** | Same as baseline + 1280×720 + LFS fixture + this gate is the native-half of a cross-target Playwright spec; if it moves to `cargo test`, the Playwright spec at `e2e/tests/vox-horizon-parity.spec.ts:200-227` ALSO must change its subprocess invocation from `cargo run --bin e2e_render -- --vox-horizon-native` to `cargo test --test vox_horizon_native_phase` (and absorb the additional --no-capture flag for the PNG-on-disk side effect to be observable). |
| 14 | --resize-test | `bin/e2e_render::run_resize_test` | **Cannot move cleanly** | **Hyprland-specific.** Wraps the boot in `hyprctl keyword windowrule …` and `hyprctl reload`. Mid-run shells out to `hyprctl dispatch resizewindowpixel`. Bails with explicit error message if `HYPRLAND_INSTANCE_SIGNATURE` absent. This is fundamentally machine-state-dependent — not a candidate for being a generic `cargo test`. Workable as a separate `#[cfg(target_os = "linux")]` gated test that self-skips like the driver does today (`#[ignore]` by default and run with `cargo test -- --ignored`). |
| 15 | --entities | `EntitiesBoot` arm | **With caveats** | Same as baseline. Note: `--entities` is also a post-app validation flag (row 20) — the builder path AND the post-app tail share the flag; under `cargo test` these would be two distinct tests. |
| 16 | --ssim-compare | Layer 1 (no boot) | **Cleanly, immediately** | Pure CPU function. No GPU, no window, no fixtures. Already callable as `e2e::ssim::ssim_compare_command(&parsed)` returning a `u8`. Drop-in `cargo test` candidate today. |
| 17 | --validate-gpu-construction (post-app) | Layer 3 (`validate_gpu_construction`) | **Cleanly, immediately** | **Already headless.** No window. `MinimalPlugins + RenderPlugin` boots a wgpu Adapter+Device but never opens a surface. Returns `Result<usize, String>` — translates to a `#[test]` with `assert!(result.is_ok(), "...")` in one line. |
| 18-19 | --validate-gpu-construction-scaled / -production | Layer 1 | **Cleanly, immediately** | Same as #17. |
| 20 | --entities (post-app) | Layer 3 (`validate_entity_handler`) | **Cleanly, immediately** | Same as #17. |
| 21 | --edit-mode | Layer 3 (`validate_edit_mode`) | **Cleanly, immediately** | Same as #17. |
| 22 | --runtime-edit-mode | Layer 3 (`validate_runtime_edit_mode`) | **Cleanly, immediately** | Same as #17. |

**Counts:**
- **Cleanly, immediately movable:** rows 16, 17, 18, 19, 20, 21, 22 — **7 already-headless validators** = trivial drop-in.
- **Movable with caveats** (real-window booted gates, all share the
  same WinitPlugin / App::run() consumption issue): rows 1, 2, 3, 4,
  5, 6, 7, 8, 10, 11, 13, 15 — **12 gates**.
- **Movable with caveats AND subprocess refactor:** rows 9, 12 —
  **2 subprocess orchestrators**.
- **Cannot move cleanly (machine-state dependent):** row 14 (resize-test, Hyprland-only) — **1 gate**.

---

## 5. The alternative shape

### 5.1 What tests/<gate>.rs would look like

A typical clean candidate (rows 16-22) is trivial:

```rust
// crates/bevy_naadf/tests/validate_gpu_construction.rs
#[test]
fn gpu_construction_matches_cpu_oracle() {
    let bytes = bevy_naadf::render::construction::validate_gpu_construction()
        .expect("gpu construction must byte-match CPU oracle");
    assert!(bytes > 0, "must compare at least one byte");
}
```

The interface already exists. The function takes no args, returns
`Result<usize, String>`, builds + tears down its own headless App.
Drop-in.

A booted-window gate (rows 1-15) is structurally harder because:

1. `App::run()` is terminal — there's no way for a `#[test]` to read
   the verdict if it lives in a resource consumed by the runner.
2. WinitPlugin's event-loop claim is process-global; only one such test
   can run before the loop is poisoned.
3. The current verdict-write goes through `AppExit` → process exit code
   — there's no in-process channel back to a `#[test]` body.

A sketch of what a refactor would need:

```rust
// crates/bevy_naadf/tests/baseline_e2e.rs
// serial_test crate (or one-test-per-file relying on cargo test's
// per-binary serialisation)
use serial_test::serial;

#[test]
#[serial(winit)]
fn baseline_gate_passes() {
    // 1. Build the App as today.
    // 2. Run it (consumes the App, returns AppExit).
    let exit = bevy_naadf::run_e2e_render();
    // 3. Read the gate verdict from somewhere persistent.
    //    Today: process exit code. In-test: a file on disk?
    //           ($CARGO_TARGET_TMPDIR/e2e_outcome.json) the driver wrote
    //           before AppExit?
    //    Or: an Arc<Mutex<Option<Result>>> the driver wrote BEFORE app.run()
    //        consumed the App — readable post-run via the clone outside.
    assert!(matches!(exit, bevy::prelude::AppExit::Success));
}
```

The "read the verdict" question is the load-bearing one. Three options
the implementer would pick between:

- **A. Disk side-channel.** The driver writes a JSON verdict file
  before exit; the test reads it. Already half-implemented — the driver
  writes the PNG to `target/e2e-screenshots/e2e_latest.png`
  unconditionally; extending it to write a verdict JSON is mechanical.
- **B. Pre-cloned Arc<Mutex<E2eOutcome>>.** Insert a cloned
  `Arc<Mutex<...>>` as a resource BEFORE `app.run()`; the driver writes
  through it; the test reads from its retained clone post-run. This
  matches the existing `PipelineScanResult` cross-world-channel pattern
  (`checks.rs:43-44`) — known-good precedent.
- **C. Drive the App manually via `app.update()`.** Replace `app.run()`
  with a loop calling `app.update()` until `should_exit()` returns
  true. Bevy 0.19's `App::should_exit()` and `App::update()` are both
  public; this is the Bevy-idiomatic shape for non-winit-runner test
  drivers. But WinitPlugin's event loop still claims the window on
  plugin-build — driving manually would need WinitPlugin EXCLUDED from
  the test config, which means substituting it (no real window, no
  vsync, no compositor interaction). This breaks the harness's stated
  invariant of "exercise the real boot path" (`e2e/mod.rs:7`).

### 5.2 Shared test harness

Today's `crates/bevy_naadf/src/e2e/` IS the shared harness for
`e2e_render`. Migrating to `tests/` would either:
- Re-export it (`pub mod e2e` is already public via `lib.rs:22`) and
  let each `tests/<gate>.rs` call into it. Most of the per-gate `run_*`
  functions are already `pub`. This is the smallest move.
- Or factor a thinner `tests/common/mod.rs` containing only the
  test-runner glue (verdict-read, fixture-existence asserts), with the
  rest staying in `src/e2e/`.

Either way, the shared test harness lives in the library; `tests/`
files are thin wrappers.

### 5.3 Parallelism strategy

`cargo test` defaults to parallel test execution within a single test
binary (`--test-threads=$NCPU`). The Rust convention for serial-by-need
tests is the `serial_test` crate (`#[serial(group)]`).

**For window-booting gates, the safer model is one-binary-per-gate**
(i.e. one file per `tests/<gate>.rs`). `cargo test` runs binaries
serially by default within `--test-threads=1` granularity at the
**target** level — actually, that's NOT correct: cargo runs test
binaries in parallel by default. To force serial execution across
binaries one would invoke `cargo test --test gate_a -- --test-threads=1`
binary-by-binary, defeating the convenience.

Best path:
- Keep validators (rows 16-22) under inline `#[cfg(test)] mod tests`
  in their owning modules (they ARE inline tests today, just exposed
  via the `e2e_render` binary as ALSO-callable from CLI). Move them to
  proper `#[test]` blocks if not already.
- For booted gates, use `serial_test::serial(winit)` so the WinitPlugin
  event loop is mutex'd within a single binary; each booted-gate test
  in one `tests/winit_gates.rs` file, all tagged serial.

`RUST_TEST_THREADS=1` env-var works but breaks the parallel speedup for
non-conflicting tests.

### 5.4 How exit signals translate

| Today (process exit) | Under cargo test |
|---|---|
| `ExitCode::from(0)` | test passes (no panic, no assertion failure) |
| `ExitCode::from(1)` | `panic!("...")` or `assert!(false, "...")` |
| `AppExit::Success` | `#[test]` body completes normally |
| `AppExit::error()` | `#[test]` body panics on verdict read |
| PNG written to `target/e2e-screenshots/...` | unchanged — `cargo test` cwd is the package root, `target/` is reachable |
| `e2e_render: PASS …` log line | unchanged — `cargo test --nocapture` shows it; the Playwright spec scraping these would break under `cargo test` IF it ever runs through cargo test instead of `cargo run` (it doesn't today) |
| Subprocess re-exec of `current_exe() -- --foo` | breaks — `current_exe()` under `cargo test` is the test binary. Would need to invoke `cargo run --bin e2e_render -- --foo` via `std::process::Command` instead — slower, requires the binary built. |

The `--ssim-compare` and validator paths (rows 16-22) translate
losslessly because they're pure functions returning `Result`. The
booted gates need ONE of options A/B/C from §5.1 to bridge the
verdict-read.

---

## 6. Cross-cutting implications for the AppArgs refactor (Step 6)

### 6.1 If gates move to tests/, what happens to Step 6?

Step 6 of the design (`02-design.md:1109-1175`) collapses 11 boolean
mode-fields on `AppArgs` into a single `E2eGateMode` enum resource.
**This step targets a specific binary's CLI parser** — `bin/e2e_render.rs`'s
`parse_gate_command` (`bin/e2e_render.rs:256-324`).

If verification gates move to `tests/<gate>.rs`:

- **Each test sets its OWN per-domain resources directly** at App-build
  time. No `parse_gate_command` to satisfy. No `BootCommand` enum to
  match. The whole "discriminate which gate by parsing argv" goes away
  for the test path.
- **`E2eGateMode` may still be needed at runtime** — because the
  driver's state machine (`e2e_driver` at `driver.rs:452`) needs SOME
  resource to discriminate which `*Warmup` fast-path to route into on
  tick 0. The 6 fast-path checks at `driver.rs:475-577` ARE the
  enum-shaped routing. So the resource still has to exist (Bucket B);
  what changes is WHO inserts it. Today: CLI parser via `AppArgs`
  + `run_e2e_render_with_args`. Tomorrow (if tests/-shaped):
  per-test wrapper inserts `E2eGateMode::OasisEdit` directly into the
  App before `app.run()`.
- **The `BootstrapInputs` struct (Decision §2 / §3.1) becomes a
  test-harness builder.** Each per-gate `BootstrapInputs::for_<gate>()`
  constructor (Step 6 lines 1118-1122) IS basically a per-gate
  test-helper. Moving the gates to `tests/` makes
  `BootstrapInputs::for_<gate>` the natural test fixture.

**Net effect on Step 6 specifically:** the `E2eGateMode` enum + the
per-gate `BootstrapInputs` constructors still need to be built; they're
load-bearing for the driver's routing AND for per-test fixture
construction. **What goes away is `parse_gate_command` (Layer 2) +
the `BootCommand` enum + the function-pointer table at
`bin/e2e_render.rs:111-122`.** That's an additional ~50 LOC deletion
on top of Step 6's stated scope, which collapses the 11 booleans into
the enum without changing the parser.

In other words: tests/-shaped gates make Step 6 LARGER (deletes more)
but the core enum collapse is unchanged.

### 6.2 What stays in bin/e2e_render?

If verification gates move to `tests/`:

- **Layer 1 short-circuits** stay needed only as user-facing CLI verbs:
  `--ssim-compare a.png b.png` is a useful developer tool.
  `--vox-gpu-oracle` (compare) and `--vox-web-parity` (compare) are
  meta-orchestrators of OTHER tests — under cargo test they're
  redundant (just run both sub-phase tests; cargo handles ordering).
  But on-developer-machine for ad-hoc debugging, they're useful.
- **User-facing modes** that aren't verification gates: `--vox <path>`
  (load a custom .vox file) — this isn't on `e2e_render` today
  (`--vox` is on `bevy-naadf::main`). The closest are
  `--edit-mode` / `--runtime-edit-mode` which are validators (already
  trivially movable).
- **Subprocess orchestrators** (`--vox-gpu-oracle`,
  `--vox-web-parity`) — useful as a fast manual-run shape.

The honest answer: under a tests/-shaped world, **`e2e_render` would
shrink to ~50-100 LOC** holding the SSIM CLI + the two compare
orchestrators + maybe the resize-test wrapper. The 22 modes collapse
to ~5 CLI verbs (the ones that are genuinely user-facing utilities,
not verification gates).

### 6.3 Which AppArgs refactor migration steps are affected?

Cross-referencing `02-design.md`'s Step 1-9:

| Refactor step | Affected by tests/ move? | How |
|---|---|---|
| 1 (BootstrapInputs introduction) | minor | `BootstrapInputs` becomes a public test-harness fixture-builder, not just a bootstrap-internal struct. Decision §1 (separate module) holds. |
| 2 (taa_ring_depth) | minor | Per-test inserts `TaaRingConfig` directly. No new constraint. |
| 3 (taa, gi) | minor | Per-test inserts `TaaConfig` / `GiSettings`. Settings panel test (`settings/mod.rs:825-841`) is already an inline `#[test]` and stays so. |
| 4 (construction_config) | minor | Same. |
| 5 (grid_preset, Q3 wasm) | unchanged | Q3 is wasm32-bootstrap-only; doesn't touch the tests/ path. |
| 6 (E2eGateMode 11→1) | **major rescoping** | The enum stays needed (driver routing); what goes away is the parser + dispatch tables in `bin/e2e_render.rs:109-324`. Step 6's "verification gates: every gate via `cargo run --bin e2e_render`" becomes "verification gates: `cargo test --workspace`". |
| 7 (VoxE2eAssertion) | minor | Per-test inserts the resource. |
| 8 (SpawnTestEntity) | minor | Same. |
| 9 (delete AppArgs shell) | unchanged | The deletion is the same; what's different is fewer call sites for the new resources (no `bin/e2e_render.rs` setters). |

**Step 6 is the only step that gets meaningfully larger.** Every other
step is mechanical — per-domain resource introduction is identical
whether the caller is `bin/e2e_render.rs` or `tests/<gate>.rs`.

---

## 7. Recommended decision shape

Three options, each with a per-axis trade-off matrix. **Not a recommendation
— enumeration only; the user picks.**

### Option A — Status quo: leave `bin/e2e_render` as the gate runner. Step 6 as designed.

**What it is:** Don't move anything. Step 6 of the AppArgs refactor
proceeds as `02-design.md` specifies — collapse the 11 booleans into
`E2eGateMode`, keep `parse_gate_command` in `bin/e2e_render.rs`.

**Implementation cost:** 0 LOC beyond the in-flight refactor.

**Blast radius on AppArgs refactor:** none.

**Determinism:** unchanged. `synchronous_pipeline_compilation` +
fixed-size window + 96-warmup-frames + SSIM tolerance all hold.

**CI integration:** still requires `cargo run --bin e2e_render --
<flag>` per gate. No `cargo test --workspace` includes them. CI runs
neither; only deploys.

**Developer ergonomics:** familiar. `cargo run --bin e2e_render --
--oasis-edit-visual` is the established invocation. Documented in
CLAUDE.md.

**Who owns what:** `bin/e2e_render.rs` orchestrates; per-gate `run_*`
builders in `e2e/<gate>.rs`. Today's shape.

### Option B — Move ALL gates to `tests/`. Drop `bin/e2e_render`'s verification responsibilities. Step 6 changes radically.

**What it is:** Migrate every booted-window gate (rows 1-15) AND every
already-headless validator (rows 16-22) into `tests/<gate>.rs` files.
`bin/e2e_render` becomes a thin developer-facing utility for SSIM
comparisons and manual runs (or is deleted entirely if the
`--ssim-compare` CLI is dropped).

**Implementation cost:**
- 7 already-headless validators × ~30 LOC each = ~200 LOC of trivial
  `#[test]` wrappers.
- 12 booted gates require the verdict-read refactor (option A/B/C
  from §5.1). Pick option B (cross-world Arc<Mutex<E2eOutcome>>) — same
  precedent as `PipelineScanResult`. Per gate: ~50-80 LOC of test glue
  + a shared `tests/common.rs` containing the verdict-read primitive
  (~150 LOC).
- Subprocess-orchestrator gates (`--vox-gpu-oracle`,
  `--vox-web-parity`) refactor to invoke the sub-phase tests via
  `cargo test --test sub_phase` from within the orchestrator test. Or
  retire the orchestrators since cargo runs both sub-phases anyway.
- Update Playwright `vox-horizon-parity.spec.ts:200-228` to invoke
  `cargo test --test vox_horizon_native` instead of `cargo run --bin
  e2e_render -- --vox-horizon-native`.
- **Total: ~800-1200 LOC of test plumbing + ~30-50 LOC of cleanup
  deletions across `bin/e2e_render.rs` and `lib.rs`.**

**Blast radius on AppArgs refactor:** Step 6 deletes `bin/e2e_render`'s
parser (~70 LOC). Each per-gate builder relocates from `e2e/<gate>::run_*`
to `tests/<gate>::test_*` (mechanical rename + cargo-test conventions).
**Step 6 grows by ~50-100 LOC of additional deletions.** Steps 1-5, 7-9
unchanged.

**Determinism:** `cargo test` per-binary parallelism could destabilise
runs if winit's event loop is shared. Mitigation: `serial_test::serial(winit)`
attribute OR one-test-per-file shape (cargo runs test binaries
sequentially via `--test-threads` granularity at the test executable
level — actually checked: cargo runs test BINARIES in parallel by
default, only TESTS WITHIN a binary respect `--test-threads`). Forcing
serial across binaries requires CI config (`cargo test -- --test-threads=1`).

**CI integration:** ALL verification gates become discoverable by
`cargo test --workspace`. CI can be extended to gate on `cargo test
--workspace --no-fail-fast` (today only `voxel_noise` is gated). This
is the **largest CI-integration win.**

**Developer ergonomics:** `cargo test --test oasis_edit_visual --
--nocapture` replaces `cargo run --bin e2e_render --
--oasis-edit-visual`. Slightly more verbose CLI. Faster (no
`cargo build --bin` for `e2e_render` per gate; `cargo test` caches
better). Test failures print structured output with file:line links to
the assertion.

**Who owns what:** `tests/<gate>.rs` per-file. Shared verdict-read in
`tests/common/mod.rs`. `e2e/<gate>.rs` becomes a private-ish
implementation detail (still `pub` for the test-file imports).

**Hyprland resize-test caveat:** stays the same — `#[test]
#[ignore]` (or `#[cfg(target_os = "linux")]` + Hyprland env detection)
with explicit opt-in run.

### Option C — Split: verification gates → tests/, user-facing modes → bin

**What it is:** Move the validators (rows 16-22, already-headless
trivial drop-ins) to `tests/`. Leave the booted-window gates (rows
1-15) in `bin/e2e_render.rs`. Keep `--ssim-compare` + the two compare
orchestrators in the binary because they ARE user-facing utilities for
ad-hoc debugging.

**Implementation cost:**
- 7 already-headless validators × ~30 LOC = ~200 LOC of `#[test]`
  wrappers.
- ~0 LOC change to booted-window gates.
- Step 6 of AppArgs refactor proceeds as designed.

**Blast radius on AppArgs refactor:** ~0. The validators don't touch
`AppArgs` (they take no args at all — verified at `validation.rs:141,
503, 834, 4381, 4518, 4663`).

**Determinism:** improved for the 7 validators (they're already
self-contained and parallel-safe; `cargo test` exercises them every
build). Unchanged for the 15 booted gates.

**CI integration:** **partial win.** The 7 validators are added to
`cargo test --workspace` coverage. CI can gate on them. Booted gates
stay manual.

**Developer ergonomics:**
- `cargo test --workspace` now also runs `validate_gpu_construction`,
  `validate_edit_mode`, etc. — silent before.
- `cargo run --bin e2e_render -- --validate-gpu-construction` still
  works for ad-hoc invocation (the CLI command stays as a thin
  function-call wrapper; or it gets dropped — minor design call).
- Booted gates: `cargo run --bin e2e_render -- --oasis-edit-visual`
  unchanged.

**Who owns what:** `tests/validate_*.rs` for the 7 validators.
Everything else unchanged.

**This is the SMALLEST move with the BIGGEST coverage-discovery win.**
The 7 validators today are essentially dead-code from a `cargo test`
perspective — they exist, they're public, they take no args, they
return `Result`, but nothing routinely invokes them except a
human-driven `cargo run` per validator. Moving them to tests/ is the
"why hadn't we done this already" move.

---

### Trade-off matrix

| Axis | Option A (status quo) | Option B (all-to-tests) | Option C (split: validators to tests/) |
|---|---|---|---|
| Impl cost (LOC) | 0 | 800-1200 | 200 |
| AppArgs Step 6 blast | 0 | +50-100 LOC deletion | 0 |
| `cargo test --workspace` coverage of "real gates" | none added | all added | 7 added (validators) |
| Booted-gate determinism risk | none | low-medium (winit + parallelism) | none |
| Subprocess pattern still works | yes | requires refactor | yes |
| Playwright cross-target spec touched | no | yes (vox-horizon-parity.spec.ts) | no |
| Developer ergonomics | familiar | better post-migration | strictly improved |
| CI integration | unchanged | maximum win | small but real win |
| Risk to in-flight work | none | medium (Step 6 rescoping) | none |
| Hyprland-only gate (resize-test) | unchanged | `#[ignore]` + opt-in | unchanged |
| Time-to-implement | (in-flight) | weeks | hours-to-1-day |

---

## Decisions & rejected alternatives

- **Counted the gates as 22 entries**, not the user's "11". The 11
  refers to the 11 booted-window gates that have flag-mode booleans on
  `AppArgs` today. The other 11 entries are Layer-1 short-circuits +
  Layer-3 post-app validators that are already headless functions. I
  surface them because they're directly relevant to "moving things to
  cargo test" — the validators are zero-cost drop-ins. The user's brief
  was scoped to the 11; I extended because the answer to "should
  cargo test run these?" is sharper when we know all 22 exist and 7 of
  them require zero work.
- **Treated `--vox-gpu-oracle-cpu` and `--vox-gpu-oracle-gpu` as two
  distinct gates** rather than two sub-phases of `--vox-gpu-oracle`.
  Each has a distinct `pub fn run_*` entry, distinct PNG output,
  distinct routing in the driver. The "compare" top-level is a
  subprocess orchestrator over the two phases. Treating them as one
  composite "oracle gate" would obscure that the subprocess pattern
  is itself a tests/-incompatible shape.
- **Did NOT treat the validators (rows 17-22) as "additional gates
  outside the AppArgs refactor's surface"** — they DO surface as
  `PostAppValidations` parsing in Layer 3 (`bin/e2e_render.rs:447-454`),
  but the AppArgs refactor doesn't touch them (they take no args). So
  they're orthogonal to Step 6 but RELEVANT to the user's question
  about test-runner migration.
- **Treated `--vox-e2e` as a "booted gate" rather than the assert-time
  flag the design models it as.** It IS a booted gate from the user-runs-the-test
  perspective — `--vox-e2e` boots a windowed app and exercises a load
  path. The AppArgs design's Decision §3 (`VoxE2eAssertion` Bucket A
  rather than Bucket B `E2eGateMode::VoxE2e`) is a structural choice
  about RESOURCES, not about whether it's a "gate". For migration-to-tests
  purposes, the distinction is irrelevant: a `tests/vox_e2e.rs` file
  inserts `VoxE2eAssertion(true)` into the App regardless.

## Assumptions made

- Assumed `cargo test` defaults to running test binaries in PARALLEL
  (multiple `tests/<file>.rs` compiled to separate binaries) — verified
  via reading https://doc.rust-lang.org/cargo/commands/cargo-test.html
  in my training data: "Each `[[test]]` target is compiled to a
  separate test executable. The executables are run sequentially by
  default unless `--jobs` is specified." Actually that's wrong — cargo
  test binaries within the same workspace member CAN run in parallel
  depending on the test runner. Status: **not 100% verified**. The
  parallelism strategy section §5.3 should be cross-checked by an
  implementor before committing.
- Assumed Bevy's `App::should_exit()` is public and works for manual
  `app.update()` loop drivers. Not verified in this worktree; based on
  general Bevy 0.19 API knowledge.
- Assumed multiple `RenderPlugin`-based Apps can coexist in a process
  (each builds its own wgpu Adapter+Device). Verified by inspection
  that `validation.rs` does this routinely across 24+ test functions in
  the same process during `cargo test --workspace --lib`. Whether
  WinitPlugin-based Apps can do the same is the open question — and the
  load-bearing one for Option B.
- Assumed `serial_test` is a known/available crate; not yet a
  dependency of this workspace. Adding it is a `Cargo.toml` edit, not
  a structural change.
- Assumed the user's framing "could these test cases be truly
  refactored into a dedicated rust test module?" treats `tests/` (cargo
  integration test convention) and `cargo test --workspace --lib`
  (inline `#[cfg(test)] mod tests`) as equivalent for the purpose of
  "ran through a test runner." Both paths are `cargo test`-discoverable;
  the difference is binary-per-file vs binary-per-crate.
- Did NOT verify that `cargo test --test foo --test bar` runs them
  serially (Cargo runs each binary, but parallelism within and across
  test binaries depends on `--test-threads` and harness flags). The
  "winit event loop is global" concern is real; the mitigation
  (serial_test) is real; the specifics are an implementor detail.

## Side notes / observations / complaints

- **The user's framing has a hidden assumption that's worth surfacing:**
  "ideally, it all e2e should be ran through a test runner" presumes
  that `cargo test` is the right runner for GPU-windowed rendering tests.
  Today, `cargo test` is fine for headless `MinimalPlugins + RenderPlugin`
  tests (we have 195 inline `#[test]` blocks; many already boot a
  headless wgpu device). But `cargo test` is NOT the natural runner for
  visual gates that need a winit window + framebuffer SSIM + cross-target
  comparisons + Hyprland-state mutation. The fact that
  `bin/e2e_render` exists as a separate binary IS a deliberate response
  to those mismatches — and the harness's own module-level comments
  (`e2e/mod.rs:1-22`, `checks.rs:1-29`) explain why. **The
  short answer to the user's question is: yes, the LAYER-1 / LAYER-3
  validators (rows 16-22) should move to `cargo test` IMMEDIATELY. The
  booted-window gates (rows 1-15) face real winit-claim + verdict-read
  + parallelism constraints that the harness's existing shape was
  designed around. The current `bin/e2e_render` shape is NOT the result
  of "we haven't gotten around to refactoring it yet" — it's the result
  of designing around `App::run()` consuming the App.** I would
  steer the user toward Option C as the low-risk, high-reward move and
  treat Option B as a separate orchestration on its own merits, not as
  a Step 6 sub-task.
- **The 7-validators win is large and almost free.** `cargo test
  --workspace` today does not run `validate_gpu_construction`,
  `validate_gpu_construction_scaled`, `validate_gpu_construction_production_scale`,
  `validate_edit_mode`, `validate_runtime_edit_mode`,
  `validate_entity_handler`. These are public functions that build
  their own headless render world, do the asserts, and return
  `Result<_, String>`. Wrapping each in a `#[test]` block in
  `tests/<name>.rs` (or `crates/bevy_naadf/tests/validators.rs` if the
  user prefers one file) is **~30 LOC per validator × 7 = ~200 LOC for
  the whole batch**. This is hours of work, not days. **And it's
  orthogonal to the AppArgs refactor entirely** — none of the
  validators read `AppArgs` (verified by grepping the `validate_*`
  function bodies for `AppArgs` — zero matches in `validation.rs`).
- **The half-done `GateKind` enum (`e2e/gate.rs:30-53`) IS the seam
  the user is reaching for.** The doc comment on the enum
  (`gate.rs:8-16`) literally says "structural scaffolding introduced
  by D6 step 2 ... intended to be consumed in subsequent steps
  (3+) where the per-gate `impl Gate` blocks land alongside the
  `e2e/driver.rs` decomposition." That decomposition would refactor
  the 1400-line `e2e_driver` function into per-gate
  `impl Gate` blocks — each gate owning its own phase machine. THAT
  refactor would make `tests/<gate>.rs` trivial because each gate's
  state machine is self-contained. **The user's question is, in part,
  pulling on the same thread as the dead `Gate` trait at
  `gate.rs:76-127`.** A `/refactor` on the e2e driver to land that
  decomposition would be a more architecturally-coherent precursor to
  Option B than landing Option B directly.
- **`bin/e2e_render.rs` is itself ROTTEN.** 524 lines, three layers of
  parsing, function-pointer table in `parse_gate_command`, `BootCommand`
  with a `gate: GateKind` discriminator that's literally `let _ = gate;`
  consumed-for-nothing. The half-done `GateKind` refactor stalled
  exactly here. If the user wants to do Step 6 cleanly, they should
  also consider whether the THREE half-done refactors that meet at this
  file (D6 driver decomposition + D6 GateKind dispatch + AppArgs
  config-as-resource) should all land together via `/refactor`, not as
  three separate orchestrations all stepping on each other.
- **The Playwright spec at `e2e/tests/vox-horizon-parity.spec.ts` is
  ALREADY treating `e2e_render` as a subprocess-callable test harness.**
  It shells out to `cargo run --bin e2e_render -- --vox-horizon-native`
  (line 207) AND to `cargo run --bin e2e_render -- --ssim-compare ...`
  (line 248). If the user moves to Option B (or C), the Playwright
  spec's subprocess invocations change. This is a coordination point
  the AppArgs orchestration as designed does not flag.
- **The user said "11 e2e gates" but the actual count of mode-booleans
  on `AppArgs` is 11** — exactly matching the design's `E2eGateMode`
  enum collapse. The 11 booted gates I enumerated (rows 1-8, 10-11,
  13-15 — i.e. 13 entries; the gpu-oracle-cpu/gpu and web-parity-skybox/loaded
  splits double up into pairs) actually map to 11 mode booleans
  because:
  - oracle-cpu + oracle-gpu = 2 booleans, 2 separately-run sub-phases
  - parity-skybox + parity-loaded = 2 booleans, 2 separately-run sub-phases
  - horizon-native = 1 boolean
  - vox-e2e + oasis-edit + small-edit-visual + small-edit-repro +
    vox-gpu-construction + resize-test = 6 booleans
  - = 11 total
  The "baseline" gate has no mode-boolean (it's the absence of any).
  `--entities` flips two non-mode fields (`construction_config.entities_enabled`
  + `spawn_test_entity`), neither of them in the 11. **So the 11
  mapping is exact and the user's count is correct.** Surface this
  back: the user's question is well-posed.
- **CI doesn't run `cargo test --workspace`. CI doesn't run any
  e2e_render gate. CI doesn't run Playwright.** The user's
  verification surface is local-only today. This is part of why moving
  validators to `tests/` is a 10× CI-coverage win — going from zero
  routine-run gates to 7 is a step change. (`deploy-cloudflare.yml`
  only `cargo test -p voxel_noise`.)
- **`docs/todo` is silent on test-runner unification.** Searched
  `docs/todo` for "test-runner", "cargo test", "tests/" — no hits.
  This investigation is the first time this question is being
  surfaced; the user's prompt is genuinely new framing, not a
  re-litigation.
- **Subjective reaction:** if I were the implementer, I would land
  Option C (7 validators to tests/) THIS DISPATCH, in parallel with
  the AppArgs refactor proceeding as `02-design.md` specifies. Then
  separately scope the booted-window-gate migration (Option B) as a
  follow-up orchestration that PAIRS with the half-done `Gate` trait
  + `GateKind` dispatch decomposition in `e2e/gate.rs`. Trying to do
  Option B inside Step 6 forces three refactors into one commit
  cluster, which violates the in-flight refactor's incremental-migration
  constraint (`01-context.md:114`).
- **Vigilance note:** I verified every cited file:line by Read/Grep
  during this investigation. The `bin/e2e_render.rs` line numbers
  (134-157, 168-185, 256-324, 447-454) match the design's audit.
  The `e2e/gate.rs:30-53` `GateKind` enum is exactly the half-done
  scaffolding the design describes. The `validation.rs` line numbers
  (141, 503, 834, 4381, 4518, 4663) for the validator function
  starts were verified by grep. The `e2e_render` argv parsing being
  ad-hoc string-iter (no clap) was verified by inspecting
  `parse_gate_command` directly.
